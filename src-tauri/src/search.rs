//! Search, one source at a time. What is on this PC answers at once (the
//! saved movie and show catalogs, and your Jellyfin library); Jellyfin's own
//! search and the Radarr and Sonarr lookups join in as they answer. Each
//! answer redraws the merged list, so the page fills in instead of waiting
//! for the slowest source (Sonarr takes about a second).
//!
//! Order: what you own first, then the closest title match, then the most
//! popular. Sources are merged in a fixed order, whatever order they answer
//! in, so a card's place only changes when a better match arrives.

use crate::log::log;
use crate::media::{arr, explore_library, jf, match_rank, Found, Media};
use crate::model::{AppId, Level};
use crate::tiles::{self, Tile};
use serde::Serialize;
use tauri::{AppHandle, Manager};

/// Every source, in the order their answers are merged.
const SOURCES: [Source; 5] = [Source::Library, Source::Jellyfin, Source::Catalog, Source::Radarr, Source::Sonarr];
/// The most titles one search shows.
const MAX: usize = 80;
/// The newest live search of each page, and when it was made: the window is
/// "", a page of Chillarr inside Jellyfin is its viewer and page id
/// (viewer.rs). Older searches stop drawing.
static LATEST: std::sync::Mutex<Option<std::collections::HashMap<String, (u64, std::time::Instant)>>> = std::sync::Mutex::new(None);
/// A page that hasn't searched for this long is forgotten (it was closed).
const PAGE_TTL: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// Records `seq` as the page's newest (if it is), and says whether it still is.
fn newest(page: &str, seq: u64, record: bool) -> bool {
    let mut g = LATEST.lock().unwrap();
    let map = g.get_or_insert_with(Default::default);
    if record && !map.contains_key(page) {
        map.retain(|k, (_, at)| k.is_empty() || at.elapsed() < PAGE_TTL);
    }
    let cur = map.entry(page.to_string()).or_insert((0, std::time::Instant::now()));
    if record && seq > cur.0 {
        *cur = (seq, std::time::Instant::now());
    }
    cur.0 == seq
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Source {
    Library,
    Jellyfin,
    Catalog,
    Radarr,
    Sonarr,
}

impl Source {
    fn name(self) -> &'static str {
        match self {
            Source::Library | Source::Jellyfin => "Jellyfin",
            Source::Catalog => "Chillarr",
            Source::Radarr => "Radarr",
            Source::Sonarr => "Sonarr",
        }
    }
}

/// One title a source found, with what ordering needs.
#[derive(Clone, Debug)]
struct Hit {
    tile: Tile,
    owned: bool,
    /// `match_rank`, 4 when the source matched something other than the title
    /// (an alternative title, a fuzzy match).
    rank: u8,
    /// Popularity on one 0..1 scale across sources.
    pop: f64,
}

enum Answer {
    Hits(Vec<Hit>),
    Failed(String),
    /// Not set up (no Jellyfin sign-in, no Radarr): nothing to say.
    Skipped,
}

/// TMDb popularity (a few for obscure films, hundreds for a hit) on 0..1.
fn pop_movie(p: f64) -> f64 {
    (p.max(0.0).ln_1p() / 500f64.ln_1p()).min(1.0)
}

/// TVmaze weight (the catalog keeps 85 and up) on 0..1.
fn pop_show(weight: f64) -> f64 {
    ((weight - 80.0) / 20.0).clamp(0.0, 1.0)
}

/// Sonarr's vote count on 0..1.
fn pop_votes(v: f64) -> f64 {
    (v.max(0.0).ln_1p() / 20000f64.ln_1p()).min(1.0)
}

fn hit(tile: Tile, term: &str, owned: bool, pop: f64) -> Hit {
    let rank = match_rank(&tile.title, term).unwrap_or(4);
    Hit { tile, owned, rank, pop }
}

/// The same title from two sources. A movie's TMDb id and a show's are
/// different lists, so ids only match within movies or within shows.
fn same(a: &Tile, b: &Tile) -> bool {
    a.key == b.key || ((a.kind == "movie") == (b.kind == "movie") && ((a.tmdb.is_some() && a.tmdb == b.tmdb) || (a.tvdb.is_some() && a.tvdb == b.tvdb)))
}

/// Two hits of one title become one: your copy wins, then one that knows
/// Radarr/Sonarr already have it; gaps are filled from the other.
fn combine(a: Hit, b: Hit) -> Hit {
    let b_wins = (b.owned && !a.owned) || (!a.owned && !b.owned && a.tile.status.is_none() && b.tile.status.is_some());
    let (mut keep, other) = if b_wins { (b, a) } else { (a, b) };
    let t = &mut keep.tile;
    let o = other.tile;
    if t.genres.is_empty() {
        t.genres = o.genres;
    }
    t.poster = t.poster.take().or(o.poster);
    t.backdrop = t.backdrop.take().or(o.backdrop);
    t.overview = t.overview.take().or(o.overview);
    t.rating = t.rating.or(o.rating);
    t.year = t.year.or(o.year);
    t.trailer = t.trailer.take().or(o.trailer);
    t.tmdb = t.tmdb.or(o.tmdb);
    t.tvdb = t.tvdb.or(o.tvdb);
    keep.owned |= other.owned;
    keep.rank = keep.rank.min(other.rank);
    keep.pop = keep.pop.max(other.pop);
    keep
}

/// Hits from every source answered so far (in `SOURCES` order) as one list.
fn merge(hits: impl IntoIterator<Item = Hit>) -> Vec<Tile> {
    let mut out: Vec<Hit> = Vec::new();
    for h in hits {
        match out.iter().position(|o| same(&o.tile, &h.tile)) {
            Some(i) => out[i] = combine(out[i].clone(), h),
            None => out.push(h),
        }
    }
    // Stable: equal hits keep the order their sources were merged in.
    out.sort_by(|a, b| b.owned.cmp(&a.owned).then(a.rank.cmp(&b.rank)).then(b.pop.partial_cmp(&a.pop).unwrap_or(std::cmp::Ordering::Equal)));
    out.into_iter().take(MAX).map(|h| h.tile).collect()
}

/// A lookup tile without genres takes the catalog's (TVmaze and Radarr's
/// metadata server both carry them).
fn fill_genres(app: &AppHandle, hits: &mut [Hit]) {
    let media = app.state::<Media>();
    for h in hits.iter_mut().filter(|h| h.tile.genres.is_empty()) {
        let found = if h.tile.kind == "movie" {
            h.tile.tmdb.and_then(|id| media.catalog.lock().unwrap().get(id))
        } else {
            h.tile.tvdb.and_then(|id| media.shows.lock().unwrap().by_tvdb(id))
        };
        if let Some(f) = found {
            h.tile.genres = f.genres;
        }
    }
}

fn encode(term: &str) -> String {
    term.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

async fn ask(app: &AppHandle, term: &str, source: Source) -> Answer {
    let enc = encode(term);
    let mut hits = match source {
        Source::Catalog => {
            let media = app.state::<Media>();
            let movies = media.catalog.lock().unwrap().matching(term, 30);
            let shows = media.shows.lock().unwrap().matching(term, 20);
            let mut v: Vec<Hit> = movies.into_iter().map(|(t, rank, p)| Hit { tile: t, owned: false, rank, pop: pop_movie(p) }).collect();
            v.extend(shows.into_iter().map(|(t, rank, w)| Hit { tile: t, owned: false, rank, pop: pop_show(w) }));
            v
        }
        Source::Library => {
            if jf(app).filter(|j| j.user_id.is_some()).is_none() {
                return Answer::Skipped;
            }
            let mut owned: Vec<(u8, Tile)> = explore_library(app).await.into_iter().filter_map(|t| match_rank(&t.title, term).map(|r| (r, t))).collect();
            owned.sort_by_key(|(r, _)| *r);
            owned.into_iter().take(20).map(|(rank, t)| Hit { pop: t.rating.unwrap_or(0.0) / 10.0, tile: t, owned: true, rank }).collect()
        }
        Source::Jellyfin => {
            let Some(j) = jf(app).filter(|j| j.user_id.is_some()) else { return Answer::Skipped };
            match j.items(&format!("recursive=true&includeItemTypes=Movie,Series&searchTerm={enc}&limit=20")).await {
                Ok(v) => v.iter().map(|x| tiles::from_jf(x, &j.base)).map(|t| { let p = t.rating.unwrap_or(0.0) / 10.0; hit(t, term, true, p) }).collect(),
                Err(e) => return Answer::Failed(e.to_string()),
            }
        }
        Source::Radarr => {
            let Some(a) = arr(app, AppId::Radarr) else { return Answer::Skipped };
            match a.list(&format!("movie/lookup?term={enc}")).await {
                Ok(v) => v.iter().take(20).map(|m| hit(tiles::from_radarr(m), term, false, pop_movie(m["popularity"].as_f64().unwrap_or(0.0)))).collect(),
                Err(e) => return Answer::Failed(e.to_string()),
            }
        }
        Source::Sonarr => {
            let Some(a) = arr(app, AppId::Sonarr) else { return Answer::Skipped };
            match a.list(&format!("series/lookup?term={enc}")).await {
                Ok(v) => v.iter().take(20).map(|s| hit(tiles::from_sonarr(s), term, false, pop_votes(s["ratings"]["votes"].as_f64().unwrap_or(0.0)))).collect(),
                Err(e) => return Answer::Failed(e.to_string()),
            }
        }
    };
    fill_genres(app, &mut hits);
    Answer::Hits(hits)
}

/// The list so far, and who hasn't answered or couldn't.
fn summary(answers: &[Option<Answer>]) -> (Vec<Tile>, Vec<&'static str>, Vec<String>) {
    let mut hits = Vec::new();
    let mut waiting = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    for (s, a) in SOURCES.iter().zip(answers) {
        match a {
            Some(Answer::Hits(h)) => hits.extend(h.iter().cloned()),
            Some(Answer::Failed(_)) => {
                if !failed.iter().any(|f| f == s.name()) {
                    failed.push(s.name().to_string());
                }
            }
            Some(Answer::Skipped) => {}
            None => waiting.push(s.name()),
        }
    }
    waiting.dedup();
    (merge(hits), waiting, failed)
}

fn log_failure(app: &AppHandle, s: Source, a: &Answer) {
    if let Answer::Failed(e) = a {
        // The screen names the app; the reason goes to the log.
        log(app, Level::Amber, format!("Search couldn't ask {}: {e}.", s.name()));
    }
}

/// Everything at once, for the phone remote.
pub async fn all(app: &AppHandle, term: &str) -> Found {
    let term = term.trim();
    if term.chars().count() < 2 {
        return Found::default();
    }
    let (a, b, c, d, e) = tokio::join!(ask(app, term, SOURCES[0]), ask(app, term, SOURCES[1]), ask(app, term, SOURCES[2]), ask(app, term, SOURCES[3]), ask(app, term, SOURCES[4]));
    let answers = [Some(a), Some(b), Some(c), Some(d), Some(e)];
    for (s, a) in SOURCES.iter().zip(&answers) {
        log_failure(app, *s, a.as_ref().unwrap());
    }
    let (tiles, _, failed) = summary(&answers);
    Found { tiles, failed }
}

/// One step of a live search: the merged list so far.
#[derive(Serialize, Clone)]
pub struct Update {
    pub seq: u64,
    pub term: String,
    pub tiles: Vec<Tile>,
    /// Sources still being asked ("Radarr", "Sonarr").
    pub waiting: Vec<&'static str>,
    pub failed: Vec<String>,
}

/// Asks every source at once and sends a "search-update" each time one
/// answers. A newer search (a higher `seq`) silences this one.
pub async fn live(app: AppHandle, term: String, seq: u64) {
    let page = crate::viewer::current().map(|v| format!("{}/{}", v.session.user_id, v.page)).unwrap_or_default();
    newest(&page, seq, true);
    let term = term.trim().to_string();
    if term.chars().count() < 2 {
        return;
    }
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    for (i, s) in SOURCES.iter().copied().enumerate() {
        let (app, term, tx) = (app.clone(), term.clone(), tx.clone());
        tauri::async_runtime::spawn(crate::viewer::carry(async move {
            let a = ask(&app, &term, s).await;
            log_failure(&app, s, &a);
            let _ = tx.send((i, a));
        }));
    }
    drop(tx);
    let mut answers: Vec<Option<Answer>> = SOURCES.iter().map(|_| None).collect();
    while let Some((i, a)) = rx.recv().await {
        answers[i] = Some(a);
        if !newest(&page, seq, false) {
            continue;
        }
        let (tiles, waiting, failed) = summary(&answers);
        crate::bridge::emit(&app, "search-update", Update { seq, term: term.clone(), tiles, waiting, failed });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(key: &str, kind: &str, title: &str) -> Tile {
        Tile::new(key, kind, title)
    }
    fn h(tile: Tile, owned: bool, rank: u8, pop: f64) -> Hit {
        Hit { tile, owned, rank, pop }
    }

    #[test]
    fn owned_then_closest_then_most_popular() {
        let tiles = merge([
            h(t("tmdb:2", "movie", "Iron Man 2"), false, 1, 0.9),
            h(t("tmdb:1", "movie", "Iron Man"), false, 0, 0.5),
            h(t("tmdb:3", "movie", "Iron Man 3"), false, 1, 0.95),
            h(t("jf:a", "movie", "The Iron Giant"), true, 4, 0.1),
        ]);
        let titles: Vec<&str> = tiles.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["The Iron Giant", "Iron Man", "Iron Man 3", "Iron Man 2"]);
    }

    #[test]
    fn duplicates_merge_and_your_copy_wins() {
        let lookup = Tile { tmdb: Some(1726), genres: vec!["Action".into()], status: Some("Already added".into()), ..t("tmdb:1726", "movie", "Iron Man") };
        let mine = Tile { tmdb: Some(1726), jf_id: Some("x".into()), ..t("jf:x", "movie", "Iron Man") };
        let tiles = merge([h(lookup, false, 0, 0.8), h(mine, true, 0, 0.0)]);
        assert_eq!(tiles.len(), 1);
        assert_eq!(tiles[0].key, "jf:x", "the library copy is kept");
        assert_eq!(tiles[0].genres, ["Action"], "and takes the lookup's genres");
    }

    #[test]
    fn a_lookup_that_knows_it_is_added_beats_the_catalog() {
        let cat = Tile { tmdb: Some(5), genres: vec!["Drama".into()], poster: Some("p".into()), ..t("tmdb:5", "movie", "Heat") };
        let look = Tile { tmdb: Some(5), status: Some("Already added".into()), ..t("tmdb:5", "movie", "Heat") };
        let tiles = merge([h(cat, false, 0, 0.5), h(look, false, 0, 0.4)]);
        assert_eq!(tiles.len(), 1);
        assert_eq!(tiles[0].status.as_deref(), Some("Already added"));
        assert_eq!((tiles[0].genres.len(), tiles[0].poster.as_deref()), (1, Some("p")));
    }

    #[test]
    fn a_movie_and_a_show_with_the_same_tmdb_number_stay_apart() {
        let movie = Tile { tmdb: Some(34723), ..t("tmdb:34723", "movie", "Some Film") };
        let show = Tile { tmdb: Some(34723), tvdb: Some(252506), ..t("tvdb:252506", "series", "High School DxD") };
        assert_eq!(merge([h(movie, false, 0, 0.1), h(show, false, 0, 0.1)]).len(), 2);
    }

    #[test]
    fn popularity_scales_stay_in_range() {
        for p in [0.0, 1.0, 22.5, 500.0, 9000.0] {
            assert!((0.0..=1.0).contains(&pop_movie(p)));
        }
        assert_eq!(pop_show(85.0), 0.25);
        assert_eq!(pop_show(100.0), 1.0);
        assert!(pop_votes(100.0) < pop_votes(10000.0));
    }

    #[test]
    fn waiting_names_each_app_once() {
        let answers: Vec<Option<Answer>> = vec![None, None, Some(Answer::Hits(vec![])), Some(Answer::Failed("x".into())), None];
        let (_, waiting, failed) = summary(&answers);
        assert_eq!(waiting, ["Jellyfin", "Sonarr"]);
        assert_eq!(failed, ["Radarr"]);
    }
}
