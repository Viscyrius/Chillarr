//! "Similar to this" at the bottom of the detail panel. Movies: TMDb's
//! recommendations (from Radarr's metadata server), then what those recommend.
//! Series: shows sharing its genres (TVmaze has no recommendations). Anime:
//! AniList's recommendations, paced with the anime rows. Anything you own
//! shows as your copy, so a card plays instead of offering Download.

use crate::log::log;
use crate::media::{http, jf, Media};
use crate::model::Level;
use crate::tiles::Tile;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

const RADARR_META: &str = "https://api.radarr.video/v1/movie";
/// Up to this many cards.
const MAX: usize = 20;
/// The picks are asked again after this, so a title's row follows its sources.
const FRESH: Duration = Duration::from_secs(30 * 60);

/// What each title is and its picks, for the session (before marking what you own, which is
/// redone on every open since a download can land in between).
static CACHE: std::sync::Mutex<BTreeMap<String, (Instant, Me, Vec<Tile>)>> = std::sync::Mutex::new(BTreeMap::new());
/// Failures already logged this session, so a dead source logs once.
static LOGGED: std::sync::Mutex<BTreeSet<&'static str>> = std::sync::Mutex::new(BTreeSet::new());

fn log_once(app: &AppHandle, what: &'static str, text: String) {
    if LOGGED.lock().unwrap().insert(what) {
        log(app, Level::Amber, text);
    }
}

/// The title the panel shows, as the picks need it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Me {
    pub key: String,
    pub title: String,
    pub jf: Option<String>,
    pub tmdb: Option<i64>,
    pub tvdb: Option<i64>,
    pub anilist: Option<i64>,
    pub genres: Vec<String>,
    pub anime: bool,
    pub movie: bool,
}

/// What a panel tile is, filled in from Jellyfin for a library title (its
/// outside ids, its genres, and whether it sits in an anime library).
async fn resolve(app: &AppHandle, t: &Tile) -> Me {
    let mut me = Me {
        key: t.key.clone(),
        title: t.title.clone(),
        jf: t.series_id.clone().or(t.jf_id.clone()),
        tmdb: t.tmdb,
        tvdb: t.tvdb,
        anilist: t.key.strip_prefix("anilist:").and_then(|s| s.parse().ok()),
        genres: t.genres.clone(),
        anime: t.kind == "anime",
        movie: t.kind == "movie" || t.format.as_deref() == Some("MOVIE"),
    };
    let (Some(id), Some(j)) = (me.jf.clone(), jf(app).filter(|j| j.user_id.is_some())) else { return me };
    let user = j.user_id.clone().unwrap_or_default();
    if let Ok(v) = j.get_json(&format!("/Items?userId={user}&ids={id}&fields=ProviderIds,Genres")).await {
        let it = &v["Items"][0];
        let pid = |k: &str| it["ProviderIds"][k].as_str().and_then(|s| s.parse::<i64>().ok());
        me.tmdb = me.tmdb.or(pid("Tmdb"));
        me.tvdb = me.tvdb.or(pid("Tvdb"));
        me.anilist = me.anilist.or(pid("AniList"));
        if let Some(name) = it["Name"].as_str() {
            me.title = name.to_string();
        }
        if me.genres.is_empty() {
            me.genres = it["Genres"].as_array().into_iter().flatten().filter_map(|g| g.as_str().map(String::from)).collect();
        }
        if it["Type"] == "Movie" {
            me.movie = true;
        } else if it["Type"] == "Series" {
            me.movie = false;
        }
    }
    if !me.anime {
        // Anime lives in its own Jellyfin libraries (see media::anime_library_ids).
        let libs = crate::media::anime_library_ids(&j).await;
        if !libs.is_empty() {
            if let Ok(a) = j.get_json(&format!("/Items/{id}/Ancestors?userId={user}")).await {
                me.anime = a.as_array().into_iter().flatten().any(|x| x["Id"].as_str().is_some_and(|i| libs.iter().any(|l| l == i)));
            }
        }
    }
    me
}

/// Up to `MAX` titles like this one, best first. Empty when none are known.
pub async fn similar(app: &AppHandle, tile: &Tile) -> Vec<Tile> {
    let key = tile.series_id.as_ref().map(|s| format!("jf:{s}")).unwrap_or_else(|| tile.key.clone());
    let cached = CACHE.lock().unwrap().get(&key).filter(|(at, _, _)| at.elapsed() < FRESH).map(|(_, me, t)| (me.clone(), t.clone()));
    let (me, recs) = match cached {
        Some(hit) => hit,
        None => {
            let me = resolve(app, tile).await;
            let t = picks(app, &me).await;
            CACHE.lock().unwrap().insert(key, (Instant::now(), me.clone(), t.clone()));
            (me, t)
        }
    };
    if recs.is_empty() {
        return recs;
    }
    let library = crate::media::explore_library(app).await;
    let tracked = crate::media::tracked(app).await;
    crate::media::mark_tracked(pick(recs, &me, &library, MAX), &tracked)
}

/// The raw picks from the source that fits the title.
async fn picks(app: &AppHandle, me: &Me) -> Vec<Tile> {
    if me.anime {
        return crate::media::anime_recs_now(app, me.anilist, &me.title).await;
    }
    if me.movie {
        return match me.tmdb {
            Some(id) => movies(app, id).await,
            None => vec![],
        };
    }
    let media = app.state::<Media>();
    let shows = media.shows.lock().unwrap();
    let genres = match me.tvdb.and_then(|id| shows.by_tvdb(id)) {
        Some(t) if !t.genres.is_empty() => t.genres,
        _ => me.genres.clone(),
    };
    if genres.is_empty() {
        return vec![];
    }
    // A few spare, for the ones `pick` leaves out as this very show.
    let out = shows.similar(&genres, me.tvdb, MAX + 5);
    out
}

/// A movie's picks. Its own record is read again (TMDb's picks change), and
/// picks the catalog hasn't met yet are fetched in one go before listing.
async fn movies(app: &AppHandle, id: i64) -> Vec<Tile> {
    let media = app.state::<Media>();
    match http().get(format!("{RADARR_META}/{id}")).send().await {
        Ok(r) if r.status().is_success() => {
            if let Ok(v) = r.json::<Value>().await {
                media.catalog.lock().unwrap().add(&v);
            }
        }
        Ok(r) if r.status().as_u16() == 404 => {}
        Ok(r) => log_once(app, "movie", format!("Similar to this: Radarr's metadata server answered {} for a movie.", r.status())),
        Err(e) => log_once(app, "movie", format!("Similar to this: couldn't reach Radarr's metadata server ({e}).")),
    }
    // TMDb's own picks first; the next ring only if those leave the row short.
    let (first, _) = media.catalog.lock().unwrap().similar_missing(id);
    bulk(app, &first).await;
    if media.catalog.lock().unwrap().similar_movies(id, MAX).len() < MAX {
        let (_, second) = media.catalog.lock().unwrap().similar_missing(id);
        bulk(app, &second.into_iter().take(30).collect::<Vec<_>>()).await;
    }
    let out = media.catalog.lock().unwrap().similar_movies(id, MAX + 5);
    out
}

/// Fetches several movies from Radarr's metadata server in one request.
async fn bulk(app: &AppHandle, ids: &[i64]) {
    if ids.is_empty() {
        return;
    }
    match http().post(format!("{RADARR_META}/bulk")).json(ids).send().await {
        Ok(r) if r.status().is_success() => {
            let list: Vec<Value> = r.json().await.unwrap_or_default();
            let media = app.state::<Media>();
            let mut c = media.catalog.lock().unwrap();
            for v in &list {
                c.add(v);
            }
        }
        Ok(r) => log_once(app, "bulk", format!("Similar to this: Radarr's metadata server answered {} for its picks.", r.status())),
        Err(e) => log_once(app, "bulk", format!("Similar to this: couldn't fetch a movie's picks ({e}).")),
    }
}

fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// The row as shown: owned titles swapped for your copy (anime by title, as
/// AniList has no TMDb or TVDB ids), this title itself left out, each title
/// once, up to `n`.
pub fn pick(recs: Vec<Tile>, me: &Me, library: &[Tile], n: usize) -> Vec<Tile> {
    let recs = crate::catalog::merge_owned(recs, library).into_iter().map(|r| {
        if r.jf_id.is_some() || r.kind != "anime" {
            return r;
        }
        let name = norm(&r.title);
        match library.iter().find(|l| !name.is_empty() && norm(&l.title) == name) {
            Some(l) => Tile { genres: r.genres.clone(), trailer: r.trailer.clone(), ..l.clone() },
            None => r,
        }
    });
    let me_title = norm(&me.title);
    let is_me = |t: &Tile| {
        t.key == me.key
            || (me.jf.is_some() && t.jf_id == me.jf)
            || (me.tmdb.is_some() && t.tmdb == me.tmdb && t.kind == "movie" && me.movie)
            || (me.tvdb.is_some() && t.tvdb == me.tvdb)
            || (me.anilist.is_some() && t.key == format!("anilist:{}", me.anilist.unwrap_or_default()))
            // AniList often lists another entry of the same show under its name.
            || (me.anime && !me_title.is_empty() && norm(&t.title) == me_title)
    };
    let mut seen = HashSet::new();
    recs.filter(|t| !is_me(t)).filter(|t| seen.insert(t.jf_id.clone().unwrap_or_else(|| t.key.clone()))).take(n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(key: &str, kind: &str, title: &str) -> Tile {
        Tile::new(key, kind, title)
    }

    #[test]
    fn owned_picks_become_your_copy_and_keep_their_order() {
        let me = Me { key: "tmdb:1".into(), title: "Iron Man".into(), tmdb: Some(1), movie: true, ..Default::default() };
        let recs = vec![
            Tile { tmdb: Some(2), ..t("tmdb:2", "movie", "Iron Man 2") },
            Tile { tmdb: Some(3), ..t("tmdb:3", "movie", "The Avengers") },
        ];
        let lib = vec![Tile { tmdb: Some(3), jf_id: Some("jf3".into()), ..t("jf:jf3", "movie", "The Avengers") }];
        let out = pick(recs, &me, &lib, 20);
        assert_eq!(out.iter().map(|x| x.title.as_str()).collect::<Vec<_>>(), ["Iron Man 2", "The Avengers"]);
        assert!(out[0].jf_id.is_none());
        assert_eq!(out[1].jf_id.as_deref(), Some("jf3"), "owned: clicking opens your copy");
    }

    #[test]
    fn the_title_itself_never_shows() {
        // Opened from the library: its own copy comes back as a pick by TMDb id.
        let me = Me { key: "jf:a".into(), jf: Some("a".into()), title: "Iron Man".into(), tmdb: Some(1), movie: true, ..Default::default() };
        let lib = vec![Tile { tmdb: Some(1), jf_id: Some("a".into()), ..t("jf:a", "movie", "Iron Man") }];
        let recs = vec![Tile { tmdb: Some(1), ..t("tmdb:1", "movie", "Iron Man") }, Tile { tmdb: Some(2), ..t("tmdb:2", "movie", "Hulk") }];
        let out = pick(recs, &me, &lib, 20);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title, "Hulk");
        // A show by TVDB id, an anime by AniList id or by its name.
        let show = Me { key: "jf:s".into(), jf: Some("s".into()), title: "Lost".into(), tvdb: Some(9), ..Default::default() };
        assert!(pick(vec![Tile { tvdb: Some(9), ..t("tvdb:9", "series", "Lost") }], &show, &[], 20).is_empty());
        let anime = Me { key: "jf:d".into(), jf: Some("d".into()), title: "High School DxD".into(), anilist: Some(11617), anime: true, ..Default::default() };
        let recs = vec![t("anilist:11617", "anime", "High School DxD"), t("anilist:5", "anime", "High school DxD!"), t("anilist:6", "anime", "Monster Musume")];
        assert_eq!(pick(recs, &anime, &[], 20).iter().map(|x| x.key.as_str()).collect::<Vec<_>>(), ["anilist:6"]);
    }

    #[test]
    fn owned_anime_is_matched_by_name_and_listed_once() {
        let me = Me { key: "anilist:1".into(), title: "Frieren".into(), anilist: Some(1), anime: true, ..Default::default() };
        let lib = vec![Tile { jf_id: Some("dxd".into()), ..t("jf:dxd", "series", "High School DxD") }];
        let recs = vec![t("anilist:2", "anime", "High School DxD"), t("anilist:3", "anime", "High school DxD"), t("anilist:4", "anime", "Toradora!")];
        let out = pick(recs, &me, &lib, 20);
        assert_eq!(out.len(), 2, "two AniList entries for the one show you own show once");
        assert_eq!(out[0].jf_id.as_deref(), Some("dxd"));
        assert_eq!(out[1].title, "Toradora!");
    }

    #[test]
    fn the_row_stops_at_its_size() {
        let me = Me { key: "tmdb:0".into(), ..Default::default() };
        let recs: Vec<Tile> = (1..40).map(|i| Tile { tmdb: Some(i), year: Some(2000), ..t(&format!("tmdb:{i}"), "movie", &format!("M{i}")) }).collect();
        let out = pick(recs, &me, &[], 20);
        assert_eq!(out.len(), 20);
        assert_eq!(out[0].title, "M1");
    }
}
