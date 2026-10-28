//! My List > Completed: everything this profile finished, still on this PC
//! or not, and which of it is deleted now (by the clean-up or by hand).
//! Built from the watch log (`history`), which keeps a title after its file
//! goes; nothing else is stored except the titles taken off the list (and
//! when), in completed-hidden-<jellyfin user id>.json.

use crate::history::{History, MovieWatch};
use crate::media::{active_user, arr, data_dir, Media};
use crate::model::AppId;
use crate::tiles::{self, Tile};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Manager};

/// A show with finished episodes.
#[derive(Clone, Debug, PartialEq)]
pub struct SeriesDone {
    pub title: String,
    pub tvdb: i64,
    /// Seasons where every episode is finished.
    pub seasons_done: Vec<i64>,
    /// (season, finished, episodes in the season when known) for the others.
    pub partial: Vec<(i64, usize, Option<usize>)>,
    /// Finished episodes (specials aside), and how many of them are no longer on this PC.
    pub finished: usize,
    pub gone: usize,
    /// The oldest latest-finish of the finished episodes still here: the
    /// next one the clean-up takes.
    pub oldest_here: Option<i64>,
    /// The latest finish, for the order on the page.
    pub at: i64,
}

/// Movies finished, on this PC or not, that TMDB knows (so they can be
/// downloaded again once deleted). The latest finished first.
pub fn movies_completed(h: &History) -> Vec<(String, MovieWatch)> {
    let mut out: Vec<(String, MovieWatch)> = h.movies.iter().filter(|(_, m)| m.tmdb.is_some()).map(|(k, m)| (k.clone(), m.clone())).collect();
    out.sort_by(|a, b| b.1.at.cmp(&a.1.at).then(a.0.cmp(&b.0)));
    out
}

/// Shows with at least one finished episode, counted per episode and season
/// in Jellyfin's numbering (like the log). `have_file(tvdb, "season:episode")`
/// says whether Jellyfin still has that episode; `season_size(tvdb, season)`
/// is how many episodes the season has. A season whose size isn't known is
/// never called done. A show logged under several Jellyfin ids (deleted and
/// downloaded again, or streamed from a placeholder first) is one show.
/// Specials don't count.
pub fn series_completed(h: &History, have_file: &dyn Fn(i64, &str) -> bool, season_size: &dyn Fn(i64, i64) -> Option<usize>) -> Vec<SeriesDone> {
    // tvdb -> (title, latest finish, season -> finished episode -> its latest finish)
    let mut shows: BTreeMap<i64, (String, i64, BTreeMap<i64, BTreeMap<i64, i64>>)> = BTreeMap::new();
    for w in h.series.values() {
        let Some(tvdb) = w.tvdb else { continue };
        let latest = w.episodes.values().copied().max().unwrap_or(0);
        let s = shows.entry(tvdb).or_insert_with(|| (String::new(), 0, BTreeMap::new()));
        if latest >= s.1 && !w.title.is_empty() || s.0.is_empty() {
            s.0 = w.title.clone();
        }
        s.1 = s.1.max(latest);
        for (key, when) in &w.episodes {
            let Some((season, ep)) = key.split_once(':').and_then(|(a, b)| Some((a.parse::<i64>().ok()?, b.parse::<i64>().ok()?))) else { continue };
            if season > 0 {
                let e = s.2.entry(season).or_default().entry(ep).or_insert(*when);
                *e = (*e).max(*when);
            }
        }
    }
    let mut out = Vec::new();
    for (tvdb, (title, at, seasons)) in shows {
        let mut d = SeriesDone { title, tvdb, seasons_done: vec![], partial: vec![], finished: 0, gone: 0, oldest_here: None, at };
        for (season, eps) in seasons {
            d.finished += eps.len();
            for (e, when) in &eps {
                if have_file(tvdb, &crate::history::ep_key(season, *e)) {
                    d.oldest_here = Some(d.oldest_here.map_or(*when, |o| o.min(*when)));
                } else {
                    d.gone += 1;
                }
            }
            match season_size(tvdb, season).filter(|n| *n > 0) {
                Some(n) if eps.len() >= n => d.seasons_done.push(season),
                Some(n) => d.partial.push((season, eps.len(), Some(n))),
                None => d.partial.push((season, eps.len(), None)),
            }
        }
        if d.finished > 0 {
            out.push(d);
        }
    }
    out.sort_by(|a, b| b.at.cmp(&a.at).then(a.title.cmp(&b.title)));
    out
}

/// The card's mark. All of it deleted: "Deleted". Still here: when the
/// clean-up takes it, counted like the clean-up does (calendar days after
/// `here_at`, the watch it goes by), or with the clean-up off "Watched", or
/// "Partly deleted" when some of it is gone already.
pub fn mark<Tz: chrono::TimeZone>(finished: usize, gone: usize, here_at: Option<i64>, cleanup_days: Option<u32>, now: i64, tz: &Tz) -> String {
    let Some(at) = here_at.filter(|_| gone < finished) else { return "Deleted".into() };
    match cleanup_days {
        Some(days) => match days as i64 - crate::history::days_since(at, now, tz) {
            ..=0 => "Deletes today".into(),
            1 => "Deletes tomorrow".into(),
            n => format!("Deletes in {n} days"),
        },
        None if gone > 0 => "Partly deleted".into(),
        None => "Watched".into(),
    }
}

/// What the active profile has finished, for the "Watched" mark and the
/// download check everywhere else: "tmdb:<id>" for movies, and "tvdb:<id>"
/// and "anilist:<id>" for shows watched to the end. Read from the watch log
/// file (small), so it's cheap to ask.
pub fn watched_keys(h: &History) -> Vec<String> {
    let mut out: BTreeSet<String> = h.movies.values().filter_map(|m| m.tmdb).map(|id| format!("tmdb:{id}")).collect();
    for w in h.series.values().filter(|w| w.complete_at.is_some()) {
        out.extend(w.tvdb.map(|id| format!("tvdb:{id}")));
        out.extend(w.anilist.map(|id| format!("anilist:{id}")));
    }
    out.into_iter().collect()
}

/// "Seasons 1-2 completed · S3: 4 of 10 episodes"
pub fn label(d: &SeriesDone) -> String {
    let mut parts = Vec::new();
    if !d.seasons_done.is_empty() {
        let mut runs: Vec<(i64, i64)> = Vec::new();
        for &s in &d.seasons_done {
            match runs.last_mut() {
                Some(r) if r.1 + 1 == s => r.1 = s,
                _ => runs.push((s, s)),
            }
        }
        let list = runs.iter().map(|(a, b)| if a == b { a.to_string() } else { format!("{a}-{b}") }).collect::<Vec<_>>().join(", ");
        let word = if d.seasons_done.len() == 1 { "Season" } else { "Seasons" };
        parts.push(format!("{word} {list} completed"));
    }
    for (s, done, of) in &d.partial {
        parts.push(match of {
            Some(of) => format!("S{s}: {done} of {of} episodes"),
            None => format!("S{s}: {done} episode{}", if *done == 1 { "" } else { "s" }),
        });
    }
    parts.join(" · ")
}

/// Episodes per season in a skyhook show (specials included, as Sonarr counts them).
pub fn season_sizes(show: &Value) -> HashMap<i64, usize> {
    let mut out = HashMap::new();
    for e in show["episodes"].as_array().into_iter().flatten() {
        if let Some(s) = e["seasonNumber"].as_i64() {
            *out.entry(s).or_insert(0) += 1;
        }
    }
    out
}

// ---------- Taken off the list ----------

/// Titles taken off ("tmdb:<id>" and "tvdb:<id>"), with when. Entries saved
/// before the time was kept have none and stay off for good.
pub type Hidden = BTreeMap<String, Option<i64>>;

/// Off the list, unless something of it was finished after it was taken off.
pub fn is_hidden(hidden: &Hidden, key: &str, finished_at: i64) -> bool {
    match hidden.get(key) {
        Some(Some(when)) => finished_at <= *when,
        Some(None) => true,
        None => false,
    }
}

/// What's found minus what this profile took off and hasn't finished more of since.
pub fn visible((movies, series): (Vec<(String, MovieWatch)>, Vec<SeriesDone>), hidden: &Hidden) -> (Vec<(String, MovieWatch)>, Vec<SeriesDone>) {
    (
        movies.into_iter().filter(|(k, m)| !is_hidden(hidden, k, m.at)).collect(),
        series.into_iter().filter(|d| !is_hidden(hidden, &format!("tvdb:{}", d.tvdb), d.at)).collect(),
    )
}

pub fn hidden_file(dir: &Path, uid: &str) -> PathBuf {
    dir.join(format!("completed-hidden-{uid}.json"))
}

/// The saved list: a map of key to when, or the older plain list of keys.
pub fn load_hidden(dir: &Path, uid: &str) -> Hidden {
    let Ok(text) = std::fs::read_to_string(hidden_file(dir, uid)) else { return Hidden::new() };
    serde_json::from_str::<Hidden>(&text)
        .or_else(|_| serde_json::from_str::<Vec<String>>(&text).map(|keys| keys.into_iter().map(|k| (k, None)).collect()))
        .unwrap_or_default()
}

/// Takes a title ("tmdb:<id>" or "tvdb:<id>") off this profile's Completed
/// at `now`; finishing more of it later brings it back.
pub fn hide(dir: &Path, uid: &str, key: &str, now: i64) -> Result<(), String> {
    let mut all = load_hidden(dir, uid);
    all.insert(key.to_string(), Some(now));
    let path = hidden_file(dir, uid);
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string(&all).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, &path)).map_err(|e| format!("Couldn't save it: {e}"))
}

/// `completed_remove`: off the watching profile's Completed.
pub fn remove(app: &AppHandle, key: &str) -> Result<(), String> {
    let (Some(dir), Some(uid)) = (data_dir(app), active_user(app)) else { return Err("Sign in to Jellyfin first.".into()) };
    hide(&dir, &uid, key, chrono::Utc::now().timestamp())
}

// ---------- The row ----------

/// Posters looked up from Radarr or Sonarr for titles no catalog has, by key,
/// for this session (None: the lookup found nothing).
fn looked_up() -> &'static Mutex<HashMap<String, Option<Tile>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<Tile>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// At most this many new lookups per page load; the rest show without a poster until the next.
const LOOKUPS_PER_LOAD: usize = 12;

fn poster(v: &Value) -> Option<String> {
    v["images"].as_array()?.iter().find(|i| i["coverType"] == "poster").and_then(|i| i["remoteUrl"].as_str()).map(|u| u.replace("/original/", "/w342/"))
}

/// A tile from a Radarr or Sonarr entry (in the library or a lookup).
fn arr_tile(key: &str, kind: &str, v: &Value) -> Tile {
    let mut t = Tile::new(key, kind, v["title"].as_str().unwrap_or_default());
    t.year = v["year"].as_i64().filter(|y| *y > 0);
    t.overview = v["overview"].as_str().map(String::from);
    t.poster = poster(v);
    t.tmdb = v["tmdbId"].as_i64().filter(|i| *i > 0);
    t.tvdb = v["tvdbId"].as_i64().filter(|i| *i > 0);
    t
}

/// Season sizes from skyhook by TVDB id, for this session.
fn skyhook_cache() -> &'static Mutex<HashMap<i64, HashMap<i64, usize>>> {
    static CACHE: OnceLock<Mutex<HashMap<i64, HashMap<i64, usize>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Episodes per season for shows Sonarr can't count (deleted from it, or it's
/// down), from Sonarr's metadata server, all at once. One that doesn't answer
/// is asked again next time.
async fn skyhook_sizes(tvdbs: Vec<i64>) -> HashMap<i64, HashMap<i64, usize>> {
    let mut out = HashMap::new();
    let mut ask = tokio::task::JoinSet::new();
    for tvdb in tvdbs {
        if let Some(hit) = skyhook_cache().lock().unwrap().get(&tvdb) {
            out.insert(tvdb, hit.clone());
            continue;
        }
        ask.spawn(async move {
            let show: Option<Value> = async { crate::media::http().get(format!("{}/{tvdb}", crate::pick::SKYHOOK)).send().await.ok()?.json().await.ok() }.await;
            (tvdb, show.map(|v| season_sizes(&v)).filter(|m| !m.is_empty()))
        });
    }
    while let Some(Ok((tvdb, sizes))) = ask.join_next().await {
        if let Some(sizes) = sizes {
            skyhook_cache().lock().unwrap().insert(tvdb, sizes.clone());
            out.insert(tvdb, sizes);
        }
    }
    out
}

/// The watching profile's Completed row (`media_completed`, asked for after
/// My List is on screen): every finished movie and show, the latest first,
/// minus what was taken off. One still here opens on its library copy; one
/// deleted says so. Jellyfin's library has to answer (and its episodes, for
/// shows), or that part is left out rather than guessed: everything would
/// look deleted.
pub async fn tiles(app: &AppHandle) -> Vec<Tile> {
    let j = crate::media::jf(app).filter(|j| j.user_id.is_some());
    let (Some(dir), Some(uid), Some(j)) = (data_dir(app), active_user(app), j) else { return vec![] };
    let h = crate::history::load(&dir, &uid);
    if h.movies.is_empty() && h.series.is_empty() {
        return vec![];
    }
    let hidden = load_hidden(&dir, &uid);
    let days = crate::storage::cleanup_days(app);
    let now = chrono::Utc::now().timestamp();
    // The clean-up goes by the latest watch of a movie by anyone.
    let everyone: Vec<History> = crate::profiles::all_ids(app).iter().map(|(id, _)| crate::history::load(&dir, id)).collect();
    let movie_at = |key: &str, mine: i64| everyone.iter().filter_map(|o| o.movies.get(key)).map(|m| m.at).fold(mine, i64::max);
    let radarr = arr(app, AppId::Radarr);
    let sonarr = arr(app, AppId::Sonarr);

    // Everything at once: Radarr's movies, the Jellyfin library, its episodes, Sonarr's shows.
    // `items` and `watch_items` leave out the placeholders: those aren't on this PC.
    let (radarr_movies, library, jf_eps, sonarr_series) = tokio::join!(
        async { radarr.as_ref()?.list("movie").await.ok() },
        j.items("recursive=true&includeItemTypes=Movie,Series&limit=5000"),
        j.watch_items("includeItemTypes=Episode&isMissing=false"),
        async { sonarr.as_ref()?.list("series").await.ok() },
    );
    let Ok(library) = library else { return vec![] };
    let sonarr_series = sonarr_series.unwrap_or_default();
    let tvdb_of = |v: &Value| v["ProviderIds"]["Tvdb"].as_str().and_then(|s| s.parse::<i64>().ok());
    let tmdb_of = |v: &Value| v["ProviderIds"]["Tmdb"].as_str().and_then(|s| s.parse::<i64>().ok());

    // Movies: here if Jellyfin has it (it plays from there) or Radarr has its file.
    let jf_movies: HashMap<i64, &Value> = library.iter().filter(|v| v["Type"] == "Movie").filter_map(|v| Some((tmdb_of(v)?, v))).collect();
    let radarr_files: HashSet<i64> = radarr_movies.iter().flatten().filter(|m| m["hasFile"] == true).filter_map(|m| m["tmdbId"].as_i64()).collect();
    let movies = movies_completed(&h);

    // Series: Jellyfin's episodes now, by the show's TVDB id and Jellyfin's numbering.
    let shows: Vec<&Value> = library.iter().filter(|v| v["Type"] == "Series").collect();
    let mut series = vec![];
    let mut have: HashSet<(i64, String)> = HashSet::new();
    if let Ok(je) = &jf_eps {
        let by_id: HashMap<&str, i64> = shows.iter().filter_map(|v| Some((v["Id"].as_str()?, tvdb_of(v)?))).collect();
        for e in je {
            let (Some(tvdb), Some(s), Some(n)) = (e["SeriesId"].as_str().and_then(|id| by_id.get(id)), e["ParentIndexNumber"].as_i64(), e["IndexNumber"].as_i64()) else { continue };
            // A double episode file is both episodes.
            let last = e["IndexNumberEnd"].as_i64().filter(|l| *l > n && *l - n < 10).unwrap_or(n);
            for k in n..=last {
                have.insert((*tvdb, crate::history::ep_key(s, k)));
            }
        }
        let have_file = |tvdb: i64, key: &str| have.contains(&(tvdb, key.to_string()));
        let mut sizes: HashMap<(i64, i64), usize> = sonarr_series
            .iter()
            .filter_map(|s| Some((s["tvdbId"].as_i64()?, s["seasons"].as_array()?)))
            .flat_map(|(tvdb, seasons)| seasons.iter().filter_map(move |x| Some(((tvdb, x["seasonNumber"].as_i64()?), x["statistics"]["totalEpisodeCount"].as_u64()? as usize))))
            .filter(|(_, n)| *n > 0)
            .collect();
        // Shows Sonarr can't count: skyhook, only for the ones that would show.
        let first = series_completed(&h, &have_file, &|tvdb, season| sizes.get(&(tvdb, season)).copied());
        let unknown: Vec<i64> = first.iter().filter(|d| d.partial.iter().any(|p| p.2.is_none()) && !is_hidden(&hidden, &format!("tvdb:{}", d.tvdb), d.at)).map(|d| d.tvdb).collect();
        if !unknown.is_empty() {
            for (tvdb, by_season) in skyhook_sizes(unknown).await {
                for (season, n) in by_season {
                    sizes.entry((tvdb, season)).or_insert(n);
                }
            }
            series = series_completed(&h, &have_file, &|tvdb, season| sizes.get(&(tvdb, season)).copied());
        } else {
            series = first;
        }
    }
    // Shows with any episode in Jellyfin: those open there.
    let shows_here: HashSet<i64> = have.iter().map(|(tvdb, _)| *tvdb).collect();
    let (movies, series) = visible((movies, series), &hidden);

    let mut lookups = 0;
    let mut out: Vec<(i64, Tile)> = Vec::new();
    for (key, m) in movies {
        let tmdb = m.tmdb.unwrap_or_default();
        let here = jf_movies.contains_key(&tmdb) || radarr_files.contains(&tmdb);
        let mut t = match jf_movies.get(&tmdb) {
            Some(v) => tiles::from_jf(v, &j.base),
            None => {
                let known = app.state::<Media>().catalog.lock().unwrap().get(tmdb);
                let in_radarr = || radarr_movies.iter().flatten().find(|r| r["tmdbId"].as_i64() == Some(tmdb)).map(|r| arr_tile(&key, "movie", r));
                let mut t = match known.or_else(in_radarr) {
                    Some(t) => t,
                    None => lookup(&key, &mut lookups, async {
                        let a = radarr.as_ref()?;
                        a.get_one(&format!("movie/lookup/tmdb?tmdbId={tmdb}")).await.ok().filter(|v| v.is_object()).map(|v| arr_tile(&key, "movie", &v))
                    })
                    .await
                    .unwrap_or_else(|| Tile::new(&key, "movie", &m.title)),
                };
                t.key = key.clone();
                t.kind = "movie".into();
                t
            }
        };
        t.tmdb = Some(tmdb);
        if t.title.is_empty() {
            t.title = m.title.clone();
        }
        t.completed = Some("Completed".into());
        t.mark = Some(mark(1, usize::from(!here), Some(movie_at(&key, m.at)), days, now, &chrono::Local));
        out.push((m.at, t));
    }
    for d in series {
        let key = format!("tvdb:{}", d.tvdb);
        // Still in Jellyfin: its own tile, so it opens on the episodes there.
        let in_jf = shows_here.contains(&d.tvdb).then(|| shows.iter().find(|v| tvdb_of(v) == Some(d.tvdb)).map(|v| tiles::from_jf(v, &j.base))).flatten();
        let known = || app.state::<Media>().shows.lock().unwrap().by_tvdb(d.tvdb);
        let mut t = match in_jf.or_else(|| sonarr_series.iter().find(|s| s["tvdbId"].as_i64() == Some(d.tvdb)).map(|s| arr_tile(&key, "series", s))).or_else(known) {
            Some(t) => t,
            None => lookup(&key, &mut lookups, async {
                let a = sonarr.as_ref()?;
                let found = a.list(&format!("series/lookup?term=tvdb:{}", d.tvdb)).await.ok()?;
                found.first().map(|v| arr_tile(&key, "series", v))
            })
            .await
            .unwrap_or_else(|| Tile::new(&key, "series", &d.title)),
        };
        t.tvdb = Some(d.tvdb);
        if t.title.is_empty() {
            t.title = d.title.clone();
        }
        t.completed = Some(label(&d));
        t.mark = Some(mark(d.finished, d.gone, d.oldest_here, days, now, &chrono::Local));
        out.push((d.at, t));
    }
    out.sort_by(|a, b| b.0.cmp(&a.0));
    out.into_iter().map(|(_, t)| t).collect()
}

/// A poster lookup, remembered for the session and capped per load.
async fn lookup(key: &str, count: &mut usize, find: impl std::future::Future<Output = Option<Tile>>) -> Option<Tile> {
    if let Some(hit) = looked_up().lock().unwrap().get(key).cloned() {
        return hit;
    }
    if *count >= LOOKUPS_PER_LOAD {
        return None;
    }
    *count += 1;
    let found = find.await;
    looked_up().lock().unwrap().insert(key.to_string(), found.clone());
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{ep_key, History, MovieWatch, SeriesWatch};

    fn movie(title: &str, tmdb: Option<i64>, at: i64) -> MovieWatch {
        MovieWatch { title: title.into(), tmdb, anime: false, at }
    }

    #[test]
    fn every_finished_movie_is_completed_on_disk_or_not() {
        let mut h = History::default();
        h.movies.insert("tmdb:1".into(), movie("Dune", Some(1), 100));
        h.movies.insert("tmdb:2".into(), movie("Heat", Some(2), 300));
        h.movies.insert("jf:x".into(), movie("Home video", None, 200));
        let got: Vec<String> = movies_completed(&h).into_iter().map(|(k, m)| format!("{k} {}", m.title)).collect();
        assert_eq!(got, vec!["tmdb:2 Heat", "tmdb:1 Dune"], "latest first; no tmdb id: can't be downloaded again, left out");
    }

    fn show(eps: &[(i64, i64)], at: i64) -> SeriesWatch {
        let mut w = SeriesWatch { title: "Show".into(), tvdb: Some(7), ..Default::default() };
        for (s, e) in eps {
            w.episodes.insert(ep_key(*s, *e), at);
        }
        w
    }

    #[test]
    fn a_show_still_on_disk_is_completed_too() {
        let mut h = History::default();
        h.series.insert("jf-show".into(), show(&[(1, 1), (1, 2)], 10));
        let size = |_: i64, _: i64| Some(2);
        let here = series_completed(&h, &|_, _| true, &size);
        assert_eq!((here[0].seasons_done.clone(), here[0].finished, here[0].gone), (vec![1], 2, 0));
        let utc = chrono::FixedOffset::east_opt(0).unwrap();
        let m = |d: &SeriesDone, days| mark(d.finished, d.gone, d.oldest_here, days, 10, &utc);
        assert_eq!(m(&here[0], None), "Watched", "all still here, clean-up off");
        let deleted = series_completed(&h, &|_, _| false, &size);
        assert_eq!((m(&deleted[0], None), m(&deleted[0], Some(3))), ("Deleted".into(), "Deleted".into()));
        let partly = series_completed(&h, &|_, key| key == "1:2", &size);
        assert_eq!(m(&partly[0], None), "Partly deleted");
        // Only a special watched: nothing to list.
        h.series.insert("jf-show".into(), show(&[(0, 1)], 10));
        assert!(series_completed(&h, &|_, _| false, &size).is_empty());
    }

    #[test]
    fn label_full_and_partial_seasons() {
        let mut h = History::default();
        // Seasons 1 and 2 (3 episodes each) watched; S3: 4 of 10 watched,
        // E4 still on disk. Specials don't count.
        h.series.insert("old-id".into(), show(&[(1, 1), (1, 2), (1, 3), (2, 1), (2, 2), (2, 3), (3, 1), (3, 2), (3, 3), (0, 1)], 10));
        // The same show again under a new Jellyfin id (downloaded again, or a
        // placeholder it was streamed from): merged by tvdb.
        h.series.insert("new-id".into(), show(&[(3, 4)], 20));
        let have = |tvdb: i64, key: &str| tvdb == 7 && key == "3:4";
        let size = |_tvdb: i64, season: i64| Some(if season == 3 { 10 } else { 3 });
        let done = series_completed(&h, &have, &size);
        assert_eq!(done.len(), 1);
        let d = &done[0];
        assert_eq!((d.title.as_str(), d.tvdb, d.seasons_done.clone(), d.partial.clone(), d.finished, d.gone, d.at), ("Show", 7, vec![1, 2], vec![(3, 4, Some(10))], 10, 9, 20));
        assert_eq!(label(d), "Seasons 1-2 completed · S3: 4 of 10 episodes");

        let one = SeriesDone { title: "x".into(), tvdb: 1, seasons_done: vec![2], partial: vec![], finished: 3, gone: 0, oldest_here: None, at: 0 };
        assert_eq!(label(&one), "Season 2 completed");
        let gaps = SeriesDone { seasons_done: vec![1, 2, 3, 5], partial: vec![(6, 1, Some(8))], ..one.clone() };
        assert_eq!(label(&gaps), "Seasons 1-3, 5 completed · S6: 1 of 8 episodes");
        // Season 3's size unknown (show gone from Sonarr, skyhook unreachable):
        // never called done, and no "of".
        let unknown = series_completed(&h, &have, &|_, season| (season != 3).then_some(3));
        assert_eq!(unknown[0].partial, vec![(3, 4, None)]);
        assert_eq!(label(&unknown[0]), "Seasons 1-2 completed · S3: 4 episodes");
    }

    #[test]
    fn a_title_still_here_counts_down_to_the_clean_up() {
        let utc = chrono::FixedOffset::east_opt(0).unwrap();
        let t = |s: &str| chrono::DateTime::parse_from_rfc3339(s).unwrap().timestamp();
        let watched = Some(t("2026-10-15T22:00:00Z"));
        let now = t("2026-10-19T09:00:00Z");
        assert_eq!(mark(1, 0, watched, Some(7), now, &utc), "Deletes in 3 days", "4 calendar days of 7");
        assert_eq!(mark(1, 0, watched, Some(5), now, &utc), "Deletes tomorrow");
        assert_eq!(mark(1, 0, watched, Some(4), now, &utc), "Deletes today");
        assert_eq!(mark(1, 0, watched, Some(1), now, &utc), "Deletes today", "overdue: the next pass takes it");
        assert_eq!(mark(1, 0, watched, None, now, &utc), "Watched", "clean-up off: no countdown");
        assert_eq!(mark(1, 1, watched, Some(7), now, &utc), "Deleted");
        // A show: the oldest finished episode still here goes first.
        let mut h = History::default();
        let mut w = show(&[(1, 1), (1, 2), (1, 3)], 0);
        w.episodes.insert(ep_key(1, 2), t("2026-10-12T10:00:00Z"));
        w.episodes.insert(ep_key(1, 3), t("2026-10-18T10:00:00Z"));
        h.series.insert("s".into(), w);
        let d = &series_completed(&h, &|_, key| key != "1:1", &|_, _| Some(3))[0];
        assert_eq!((d.gone, d.oldest_here), (1, Some(t("2026-10-12T10:00:00Z"))));
        assert_eq!(mark(d.finished, d.gone, d.oldest_here, Some(10), now, &utc), "Deletes in 3 days");
    }

    #[test]
    fn watched_keys_are_finished_movies_and_whole_shows() {
        let mut h = History::default();
        h.movies.insert("tmdb:1".into(), movie("Dune", Some(1), 100));
        h.movies.insert("jf:x".into(), movie("Home video", None, 100));
        h.series.insert("a".into(), show(&[(1, 1)], 10));
        h.series.insert("b".into(), SeriesWatch { tvdb: Some(8), anilist: Some(21), complete_at: Some(5), ..show(&[(1, 1)], 10) });
        assert_eq!(watched_keys(&h), vec!["anilist:21", "tmdb:1", "tvdb:8"], "a show partway through isn't Watched");
    }

    #[test]
    fn a_season_of_unknown_size_is_never_completed() {
        let mut h = History::default();
        h.series.insert("id".into(), show(&[(1, 1), (1, 2), (1, 3), (2, 1)], 10));
        let d = &series_completed(&h, &|_, _| false, &|_, _| None)[0];
        assert!(d.seasons_done.is_empty(), "every episode watched, but how many the season has isn't known");
        assert_eq!(d.partial, vec![(1, 3, None), (2, 1, None)]);
        assert_eq!(label(d), "S1: 3 episodes · S2: 1 episode");
    }

    #[test]
    fn a_streamed_episode_counts_for_completed() {
        // The Discover shelf's placeholder for The Terminal List: Dark Wolf,
        // S01E01 streamed, as Jellyfin lists it (live 2026-10-19).
        let ph = r"C:\Users\x\AppData\Roaming\com.viscyrius.chillarr\Chillarr Discover\Series\The Terminal List Dark Wolf (2025) [tmdbid-219760]";
        let series = serde_json::json!({ "Type": "Series", "Id": "ph", "Name": "The Terminal List: Dark Wolf", "Path": ph, "ProviderIds": { "Tvdb": "464185", "Tmdb": "219760" }, "UserData": { "Played": true, "LastPlayedDate": "2026-10-18T19:49:44Z" } });
        let ep = serde_json::json!({ "Type": "Episode", "SeriesId": "ph", "SeriesName": "The Terminal List: Dark Wolf", "ParentIndexNumber": 1, "IndexNumber": 1, "Path": format!(r"{ph}\Season 01\The Terminal List Dark Wolf S01E01.strm"), "UserData": { "Played": true, "LastPlayedDate": "2026-10-18T19:49:44Z" } });
        let mut h = History::default();
        h.record(&crate::history::Snapshot { series: vec![series], episodes: vec![ep], ..Default::default() });
        let d = &series_completed(&h, &|_, _| false, &|_, s| (s == 1).then_some(8))[0];
        assert_eq!((d.tvdb, d.partial.clone()), (464185, vec![(1, 1, Some(8))]));
        assert_eq!(label(d), "S1: 1 of 8 episodes");
        assert_eq!(mark(d.finished, d.gone, d.oldest_here, Some(7), 0, &chrono::Utc), "Deleted", "never downloaded: not on this PC");
    }

    #[test]
    fn skyhook_counts_episodes_per_season() {
        let show = serde_json::json!({ "episodes": [
            { "seasonNumber": 1, "episodeNumber": 1 }, { "seasonNumber": 1, "episodeNumber": 2 },
            { "seasonNumber": 2, "episodeNumber": 1 }, { "seasonNumber": 0, "episodeNumber": 1 }, { "episodeNumber": 9 }
        ] });
        let got = season_sizes(&show);
        assert_eq!((got.get(&1), got.get(&2), got.get(&0), got.len()), (Some(&2), Some(&1), Some(&1), 3));
    }

    #[test]
    fn hidden_until_something_new_is_finished() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_hidden(dir.path(), "u1").is_empty());
        hide(dir.path(), "u1", "tmdb:2", 500).unwrap();
        hide(dir.path(), "u1", "tvdb:7", 500).unwrap();
        hide(dir.path(), "u1", "tmdb:2", 500).unwrap();
        let hidden = load_hidden(dir.path(), "u1");
        assert_eq!(hidden.len(), 2);
        assert!(load_hidden(dir.path(), "u2").is_empty(), "per profile");
        assert!(hidden_file(dir.path(), "u1").ends_with("completed-hidden-u1.json"));

        let mut h = History::default();
        h.movies.insert("tmdb:1".into(), movie("Dune", Some(1), 100));
        h.movies.insert("tmdb:2".into(), movie("Heat", Some(2), 300));
        let show = |tvdb: i64, at: i64| SeriesDone { title: format!("Show {tvdb}"), tvdb, seasons_done: vec![1], partial: vec![], finished: 1, gone: 1, oldest_here: None, at };
        let keys = |(m, s): (Vec<(String, MovieWatch)>, Vec<SeriesDone>)| m.into_iter().map(|(k, _)| k).chain(s.into_iter().map(|d| format!("tvdb:{}", d.tvdb))).collect::<Vec<_>>();
        let found = |h: &History, show7: i64| (movies_completed(h), vec![show(7, show7), show(8, 0)]);
        assert_eq!(keys(visible(found(&h, 400), &hidden)), vec!["tmdb:1", "tvdb:8"]);
        assert_eq!(keys(visible(found(&h, 400), &Hidden::new())).len(), 4, "nothing hidden: everything shows");
        // Heat watched again, and another episode of show 7 finished, after they were taken off: back.
        h.movies.get_mut("tmdb:2").unwrap().at = 900;
        assert_eq!(keys(visible(found(&h, 600), &load_hidden(dir.path(), "u1"))), vec!["tmdb:2", "tmdb:1", "tvdb:7", "tvdb:8"]);
    }

    #[test]
    fn an_old_hidden_list_without_times_stays_hidden() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(hidden_file(dir.path(), "u1"), r#"["tmdb:2","tvdb:7"]"#).unwrap();
        let hidden = load_hidden(dir.path(), "u1");
        assert!(is_hidden(&hidden, "tmdb:2", i64::MAX) && is_hidden(&hidden, "tvdb:7", i64::MAX), "no time: off for good");
        // Taking another off keeps the old ones as they were.
        hide(dir.path(), "u1", "tmdb:3", 500).unwrap();
        let hidden = load_hidden(dir.path(), "u1");
        assert_eq!((hidden.get("tmdb:2"), hidden.get("tmdb:3")), (Some(&None), Some(&Some(500))));
        assert!(!is_hidden(&hidden, "tmdb:3", 501) && is_hidden(&hidden, "tmdb:3", 500));
    }
}
