//! The watch log: what each profile finished, and when, kept by Chillarr in
//! history-<jellyfin user id>.json. Jellyfin forgets a title's play data the
//! moment its file is deleted; this log doesn't, so "Because you watched" and
//! the clean-up still know about it afterwards.
//!
//! It is filled from Jellyfin every couple of minutes (`sync`, called by the
//! waiting watcher) and only ever grows: a movie once Jellyfin marks it Played,
//! an episode once it's Played, a series once the whole series is Played.
//!
//! Plays of the Discover shelf's and Watch now's placeholders count too (they
//! were watched, streamed before the download), but only as titles and
//! episode numbers: never as files, so the clean-up can't act on them.

use crate::jellyfin::Jf;
use chrono::{NaiveDate, TimeZone};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use tauri::AppHandle;

/// Anime counts as watched (for "Because you watched") after this many episodes, any season.
pub const ANIME_SEED_EPISODES: usize = 5;

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct MovieWatch {
    pub title: String,
    pub tmdb: Option<i64>,
    #[serde(default)]
    pub anime: bool,
    /// When it was last finished (unix seconds).
    pub at: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct SeriesWatch {
    pub title: String,
    pub tvdb: Option<i64>,
    /// AniList's id, when Jellyfin has one for it.
    #[serde(default)]
    pub anilist: Option<i64>,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub anime: bool,
    /// When the whole series was first seen fully watched.
    #[serde(default)]
    pub complete_at: Option<i64>,
    /// "season:episode" (Jellyfin's numbering) -> when it was last finished.
    /// For counting episodes only: Jellyfin and Sonarr can number the same
    /// file differently, so the clean-up goes by `files`.
    #[serde(default)]
    pub episodes: BTreeMap<String, i64>,
    /// The watched episodes' files (`norm`ed path) -> when last finished. A
    /// double episode is one file, so one entry.
    #[serde(default)]
    pub files: BTreeMap<String, i64>,
    /// Episode files half watched right now (from the latest sync).
    #[serde(default)]
    pub resuming: BTreeSet<String>,
}

impl SeriesWatch {
    fn started(&self) -> bool {
        !self.episodes.is_empty() || !self.files.is_empty() || !self.resuming.is_empty()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct History {
    /// Keyed by "tmdb:<id>", or "jf:<id>" for a movie without one.
    #[serde(default)]
    pub movies: HashMap<String, MovieWatch>,
    /// Keyed by Jellyfin's series id. That id changes when a show is deleted
    /// and downloaded again, so the clean-up groups by tvdb instead.
    #[serde(default)]
    pub series: HashMap<String, SeriesWatch>,
    /// Movies half watched right now (from the latest sync). The clean-up
    /// leaves these alone.
    #[serde(default)]
    pub in_progress: HashSet<String>,
    /// When Jellyfin last answered for this profile (unix seconds). The
    /// clean-up waits while any profile's log is out of date.
    #[serde(default)]
    pub synced_at: i64,
}

pub fn ep_key(season: i64, episode: i64) -> String {
    format!("{season}:{episode}")
}

/// A file path as both Jellyfin and Sonarr can be compared by.
pub fn norm(path: &str) -> String {
    path.replace('/', "\\").to_lowercase()
}

/// Jellyfin's dates ("2026-10-17T20:44:32.1234567Z") as unix seconds.
fn secs(date: &Value) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(date.as_str()?).ok().map(|d| d.timestamp())
}

fn provider(v: &Value, name: &str) -> Option<i64> {
    v["ProviderIds"].as_object()?.iter().find(|(k, _)| k.eq_ignore_ascii_case(name))?.1.as_str()?.parse().ok()
}

fn movie_key(v: &Value) -> Option<String> {
    match provider(v, "Tmdb") {
        Some(id) => Some(format!("tmdb:{id}")),
        None => v["Id"].as_str().map(|id| format!("jf:{id}")),
    }
}

/// One look at Jellyfin for one profile.
#[derive(Default)]
pub struct Snapshot {
    pub movies: Vec<Value>,
    pub series: Vec<Value>,
    pub episodes: Vec<Value>,
    pub resumable: Vec<Value>,
    /// Jellyfin ids of the series and movies in an anime library.
    pub anime: HashSet<String>,
}

impl History {
    /// Takes in what Jellyfin says now. Nothing already logged is ever dropped:
    /// a title that's gone from Jellyfin (deleted) keeps its record.
    pub fn record(&mut self, s: &Snapshot) {
        use crate::jellyfin::on_shelf;
        for v in &s.movies {
            // A placeholder without a TMDB id would be logged under its own Jellyfin id, which goes with it.
            if on_shelf(v) && provider(v, "Tmdb").is_none() {
                continue;
            }
            let (Some(key), Some(at)) = (movie_key(v), secs(&v["UserData"]["LastPlayedDate"])) else { continue };
            let anime = v["Id"].as_str().is_some_and(|id| s.anime.contains(id));
            let m = self.movies.entry(key).or_default();
            m.title = v["Name"].as_str().unwrap_or(&m.title).to_string();
            m.tmdb = provider(v, "Tmdb").or(m.tmdb);
            m.anime = anime || m.anime;
            m.at = m.at.max(at);
        }
        let now = chrono::Utc::now().timestamp();
        for v in &s.series {
            let Some(id) = v["Id"].as_str() else { continue };
            // A placeholder show holds only the episodes streamed from it, so
            // Jellyfin calls it Played after one: that isn't the whole series.
            let played = v["UserData"]["Played"] == true && !on_shelf(v);
            // Only series someone has started are worth a record.
            if !played && !self.series.contains_key(id) && !s.episodes.iter().chain(&s.resumable).any(|e| e["SeriesId"] == id) {
                continue;
            }
            let w = self.series.entry(id.to_string()).or_default();
            w.title = v["Name"].as_str().unwrap_or(&w.title).to_string();
            w.tvdb = provider(v, "Tvdb").or(w.tvdb);
            w.anilist = provider(v, "AniList").or(w.anilist);
            if let Some(g) = v["Genres"].as_array().filter(|g| !g.is_empty()) {
                w.genres = g.iter().filter_map(|x| x.as_str().map(String::from)).collect();
            }
            w.anime = s.anime.contains(id) || w.anime;
            if played && w.complete_at.is_none() {
                w.complete_at = Some(secs(&v["UserData"]["LastPlayedDate"]).unwrap_or(now));
            }
        }
        for v in &s.episodes {
            let (Some(series), Some(season), Some(ep), Some(at)) = (v["SeriesId"].as_str(), v["ParentIndexNumber"].as_i64(), v["IndexNumber"].as_i64(), secs(&v["UserData"]["LastPlayedDate"])) else { continue };
            let w = self.series.entry(series.to_string()).or_default();
            if w.title.is_empty() {
                w.title = v["SeriesName"].as_str().unwrap_or_default().to_string();
            }
            // A double episode is one item (E14 to E15): both are watched.
            let last = v["IndexNumberEnd"].as_i64().filter(|l| *l > ep && *l - ep < 10).unwrap_or(ep);
            for n in ep..=last {
                let e = w.episodes.entry(ep_key(season, n)).or_insert(at);
                *e = (*e).max(at);
            }
            // A placeholder's path is a .strm in Chillarr's own folder, not a
            // library file: kept out of `files`, which the clean-up deletes by.
            if let Some(path) = v["Path"].as_str().filter(|_| !on_shelf(v)) {
                let f = w.files.entry(norm(path)).or_insert(at);
                *f = (*f).max(at);
            }
        }
        // Half watched: replaced each time, it's only true right now.
        for w in self.series.values_mut() {
            w.resuming.clear();
        }
        self.in_progress.clear();
        for v in &s.resumable {
            match (v["Type"].as_str(), v["SeriesId"].as_str()) {
                (Some("Episode"), Some(series)) => {
                    if let Some(path) = v["Path"].as_str() {
                        self.series.entry(series.to_string()).or_default().resuming.insert(norm(path));
                    }
                }
                _ => {
                    if let Some(k) = movie_key(v) {
                        self.in_progress.insert(k);
                    }
                }
            }
        }
    }

    /// Starting a show or season over (spec call 4): those episodes leave every
    /// record of the show (Jellyfin numbering), their files too (matched by the
    /// episode numbers in the file name), and the show isn't complete any more.
    /// So the clean-up won't delete a file you're rewatching, and Completed
    /// stops listing them.
    pub fn forget_episodes(&mut self, tvdb: i64, which: &[(i64, i64)]) {
        for w in self.series.values_mut().filter(|w| w.tvdb == Some(tvdb)) {
            for (s, e) in which {
                w.episodes.remove(&ep_key(*s, *e));
            }
            w.files.retain(|path, _| {
                let name = path.rsplit(['\\', '/']).next().unwrap_or(path);
                let eps = crate::qbit::episode_numbers(name);
                eps.is_empty() || !eps.iter().any(|e| which.contains(e))
            });
            w.complete_at = None;
        }
    }

    /// What "Because you watched" may start from, the latest first: finished
    /// movies, series watched to the end, and anime with 5+ episodes watched.
    pub fn seeds(&self) -> Vec<Seed> {
        let mut out: Vec<Seed> = Vec::new();
        for m in self.movies.values() {
            if let Some(tmdb) = m.tmdb {
                out.push(Seed { title: m.title.clone(), kind: SeedKind::Movie { tmdb }, anime: m.anime, at: m.at });
            }
        }
        for w in self.series.values() {
            let last = w.episodes.values().copied().max().unwrap_or(0);
            if w.anime {
                if w.episodes.len().max(w.files.len()) >= ANIME_SEED_EPISODES {
                    out.push(Seed { title: w.title.clone(), kind: SeedKind::Anime { anilist: w.anilist }, anime: true, at: last });
                }
            } else if let Some(done) = w.complete_at {
                out.push(Seed { title: w.title.clone(), kind: SeedKind::Series { tvdb: w.tvdb, genres: w.genres.clone() }, anime: false, at: done.max(last) });
            }
        }
        out.retain(|s| !s.title.is_empty());
        out.sort_by(|a, b| b.at.cmp(&a.at).then(a.title.cmp(&b.title)));
        out
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SeedKind {
    Movie { tmdb: i64 },
    Series { tvdb: Option<i64>, genres: Vec<String> },
    Anime { anilist: Option<i64> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Seed {
    pub title: String,
    pub kind: SeedKind,
    pub anime: bool,
    pub at: i64,
}

// ---------- Days ----------

/// The calendar day a moment fell on, where you are.
pub fn day_of<Tz: TimeZone>(at: i64, tz: &Tz) -> NaiveDate {
    tz.timestamp_opt(at, 0).single().map(|d| d.date_naive()).unwrap_or_default()
}

/// Whole days between watching and now, counted by calendar day: watched
/// at 23:50 and it's 00:10, that's one day.
pub fn days_since<Tz: TimeZone>(at: i64, now: i64, tz: &Tz) -> i64 {
    (day_of(now, tz) - day_of(at, tz)).num_days()
}

// ---------- Clean-up ----------

/// An episode file everyone who watches the show has finished, `days` or more ago.
#[derive(Debug, Clone, PartialEq)]
pub struct DueEpisode {
    pub tvdb: i64,
    pub title: String,
    /// `norm`ed path: the clean-up deletes the Sonarr file at this path only.
    pub path: String,
    /// The latest watch. A file added after it is a new download (the same
    /// path comes back when a show is downloaded again) and stays.
    pub at: i64,
}

/// One profile's view of one show (tvdb), across every Jellyfin id it had.
#[derive(Default)]
struct ShowView<'a> {
    files: HashMap<&'a str, i64>,
    resuming: HashSet<&'a str>,
    started: bool,
}

fn show_views(h: &History) -> HashMap<i64, ShowView<'_>> {
    let mut out: HashMap<i64, ShowView> = HashMap::new();
    for w in h.series.values() {
        let Some(tvdb) = w.tvdb else { continue };
        let v = out.entry(tvdb).or_default();
        v.started |= w.started();
        for (f, at) in &w.files {
            let e = v.files.entry(f.as_str()).or_insert(*at);
            *e = (*e).max(*at);
        }
        v.resuming.extend(w.resuming.iter().map(|f| f.as_str()));
    }
    out
}

/// Episode files that can go: watched `days` or more days ago by every
/// profile that has started the show (the latest of their watches counts),
/// and not half watched by anyone. A profile that never started the show
/// doesn't hold it back; one partway through does. Shows are matched by tvdb,
/// so a show deleted and downloaded again (a new Jellyfin id) is still one show.
pub fn due_episodes<Tz: TimeZone>(all: &[History], days: u32, now: i64, tz: &Tz) -> Vec<DueEpisode> {
    let views: Vec<HashMap<i64, ShowView>> = all.iter().map(show_views).collect();
    let mut shows: Vec<i64> = views.iter().flat_map(|v| v.keys().copied()).collect();
    shows.sort();
    shows.dedup();
    let mut out = Vec::new();
    for tvdb in shows {
        let title = all.iter().flat_map(|h| h.series.values()).find(|w| w.tvdb == Some(tvdb) && !w.title.is_empty()).map(|w| w.title.clone()).unwrap_or_default();
        let mut files: Vec<&str> = views.iter().filter_map(|v| v.get(&tvdb)).flat_map(|v| v.files.keys().copied()).collect();
        files.sort();
        files.dedup();
        'file: for f in files {
            let mut latest = 0;
            for v in &views {
                let Some(show) = v.get(&tvdb) else { continue };
                if show.resuming.contains(f) {
                    continue 'file;
                }
                match show.files.get(f) {
                    Some(at) => latest = latest.max(*at),
                    None if show.started => continue 'file,
                    None => {}
                }
            }
            if days_since(latest, now, tz) >= days as i64 {
                out.push(DueEpisode { tvdb, title: title.clone(), path: f.to_string(), at: latest });
            }
        }
    }
    out
}

/// Movies finished `days` or more days ago (the latest watch by anyone) that
/// nobody is partway through: (tmdb, title, latest watch).
pub fn due_movies<Tz: TimeZone>(all: &[History], days: u32, now: i64, tz: &Tz) -> Vec<(i64, String, i64)> {
    let mut latest: HashMap<&String, &MovieWatch> = HashMap::new();
    for h in all {
        for (k, m) in &h.movies {
            let e = latest.entry(k).or_insert(m);
            if m.at > e.at {
                *e = m;
            }
        }
    }
    let mut out: Vec<(i64, String, i64)> = latest
        .into_iter()
        .filter(|(k, _)| !all.iter().any(|h| h.in_progress.contains(*k)))
        .filter(|(_, m)| days_since(m.at, now, tz) >= days as i64)
        .filter_map(|(_, m)| Some((m.tmdb?, m.title.clone(), m.at)))
        .collect();
    out.sort();
    out
}

/// True when a file on disk (Radarr/Sonarr `dateAdded`) was there by the
/// time it was watched. A later one is a fresh download nobody has watched
/// yet. No date, or one that won't read: false, so nothing is deleted.
pub fn file_predates(date_added: &Value, watched_at: i64) -> bool {
    secs(date_added).is_some_and(|added| added <= watched_at)
}

// ---------- Files and the sync ----------

pub fn file(dir: &Path, user_id: &str) -> PathBuf {
    dir.join(format!("history-{user_id}.json"))
}

pub fn load(dir: &Path, user_id: &str) -> History {
    std::fs::read_to_string(file(dir, user_id)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save(dir: &Path, user_id: &str, h: &History) {
    let path = file(dir, user_id);
    let tmp = path.with_extension("json.tmp");
    if let Ok(text) = serde_json::to_string(h) {
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

/// Reads one profile's play data from Jellyfin.
async fn snapshot(j: &Jf) -> Option<Snapshot> {
    // Placeholders in: a title streamed from the Discover shelf or Watch now
    // was watched, even if its file never arrived. Half-watched ones stay out:
    // nothing about a placeholder's position matters to the clean-up.
    let get = |q: &'static str| j.watch_items_with_placeholders(q);
    let mut s = Snapshot {
        movies: get("includeItemTypes=Movie&filters=IsPlayed").await.ok()?,
        series: get("includeItemTypes=Series").await.ok()?,
        episodes: get("includeItemTypes=Episode&filters=IsPlayed").await.ok()?,
        resumable: j.watch_items("includeItemTypes=Movie,Episode&filters=IsResumable").await.ok()?,
        anime: HashSet::new(),
    };
    for lib in crate::media::anime_library_ids(j).await {
        let q = format!("parentId={lib}&includeItemTypes=Series,Movie");
        for v in j.watch_items(&q).await.unwrap_or_default() {
            if let Some(id) = v["Id"].as_str() {
                s.anime.insert(id.to_string());
            }
        }
    }
    Some(s)
}

/// Episodes finished since `before` (new, or finished again), with when
/// (Jellyfin's last-played time), by show and in order. Specials don't count.
pub fn newly_finished(before: &History, after: &History) -> Vec<(i64, crate::marker::Spot, i64)> {
    let mut out = Vec::new();
    for (id, w) in &after.series {
        let Some(tvdb) = w.tvdb else { continue };
        let old = before.series.get(id);
        for (key, at) in &w.episodes {
            if old.and_then(|o| o.episodes.get(key)).is_some_and(|was| was >= at) {
                continue;
            }
            let Some((s, e)) = key.split_once(':').and_then(|(a, b)| Some((a.parse::<i64>().ok()?, b.parse::<i64>().ok()?))) else { continue };
            if s > 0 {
                out.push((tvdb, crate::marker::Spot { season: s, episode: e }, *at));
            }
        }
    }
    out.sort();
    out
}

/// Brings every profile's log up to date, and moves "where you're at" for
/// whatever was finished since (on any device). Called by the waiting watcher.
pub async fn sync(app: &AppHandle) {
    let Some(dir) = crate::media::data_dir(app) else { return };
    for (user_id, j) in crate::profiles::all_jf(app) {
        let Some(s) = snapshot(&j).await else { continue };
        let mut h = load(&dir, &user_id);
        let before = h.clone();
        h.record(&s);
        h.synced_at = chrono::Utc::now().timestamp();
        save(&dir, &user_id, &h);
        for (tvdb, spot, at) in newly_finished(&before, &h) {
            if let Err(e) = crate::marker::finished(&dir, &user_id, tvdb, spot, at) {
                crate::log::log(app, crate::model::Level::Amber, e);
            }
        }
    }
}

/// Takes episodes out of the watching profile's log (Start over).
pub fn forget(app: &AppHandle, tvdb: i64, which: &[(i64, i64)]) {
    if let (Some(dir), Some(uid)) = (crate::media::data_dir(app), crate::media::active_user(app)) {
        let mut h = load(&dir, &uid);
        h.forget_episodes(tvdb, which);
        save(&dir, &uid, &h);
    }
}

/// The log of whoever is watching now.
pub fn active(app: &AppHandle) -> History {
    match (crate::media::data_dir(app), crate::media::active_user(app)) {
        (Some(dir), Some(uid)) => load(&dir, &uid),
        _ => History::default(),
    }
}

/// A log this old (or never synced) means that profile's watching is unknown.
pub const SYNC_FRESH: i64 = 30 * 60;

/// Every profile's log, for the clean-up: all of them, signed in or not. Err
/// names a profile whose log isn't current, since what it is partway through
/// can't be known, and the clean-up then waits.
pub fn everyone(app: &AppHandle) -> Result<Vec<History>, String> {
    let dir = crate::media::data_dir(app).ok_or("Chillarr's data folder is missing.")?;
    let people = crate::profiles::all_ids(app);
    if people.is_empty() {
        return Err("you".into());
    }
    let now = chrono::Utc::now().timestamp();
    let mut out = Vec::new();
    for (id, name) in people {
        let h = load(&dir, &id);
        if now - h.synced_at > SYNC_FRESH {
            return Err(name);
        }
        out.push(h);
    }
    Ok(out)
}

/// The AniList query for an anime's recommendations, by id or by title.
pub fn anilist_recs_query(anilist: Option<i64>, title: &str) -> String {
    let pick = match anilist {
        Some(id) => format!("id: {id}"),
        None => {
            let safe: String = title.chars().filter(|c| !matches!(c, '"' | '\\' | '{' | '}')).collect();
            format!("search: \"{safe}\"")
        }
    };
    format!(
        "{{ Media({pick}, type: ANIME) {{ recommendations(sort: RATING_DESC, perPage: 25) {{ nodes {{ mediaRecommendation {{ id idMal format genres isAdult title {{ english romaji }} coverImage {{ extraLarge }} bannerImage averageScore description(asHtml: false) seasonYear trailer {{ id site }} }} }} }} }} }}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;
    use serde_json::json;

    fn t(s: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(s).unwrap().timestamp()
    }

    fn ep(series: &str, s: i64, e: i64, at: &str) -> Value {
        json!({ "Type": "Episode", "SeriesId": series, "SeriesName": "Show", "ParentIndexNumber": s, "IndexNumber": e, "Path": path(s, e), "UserData": { "Played": true, "LastPlayedDate": at } })
    }

    fn path(s: i64, e: i64) -> String {
        format!(r"C:\Series\Show\Season {s}\Show S{s:02}E{e:02}.mkv")
    }

    fn series(id: &str, name: &str, tvdb: i64, played: bool) -> Value {
        json!({ "Type": "Series", "Id": id, "Name": name, "ProviderIds": { "Tvdb": tvdb.to_string() }, "Genres": ["Drama"], "UserData": { "Played": played, "LastPlayedDate": "2026-10-10T10:00:00Z" } })
    }

    fn movie(id: &str, name: &str, tmdb: i64, at: &str) -> Value {
        json!({ "Type": "Movie", "Id": id, "Name": name, "ProviderIds": { "Tmdb": tmdb.to_string() }, "UserData": { "Played": true, "LastPlayedDate": at } })
    }

    #[test]
    fn days_are_counted_by_calendar_day_where_you_are() {
        let utc = FixedOffset::east_opt(0).unwrap();
        let baghdad = FixedOffset::east_opt(3 * 3600).unwrap();
        assert_eq!(days_since(t("2026-10-10T23:50:00Z"), t("2026-10-11T00:10:00Z"), &utc), 1, "past midnight is the next day");
        assert_eq!(days_since(t("2026-10-10T00:10:00Z"), t("2026-10-10T23:50:00Z"), &utc), 0, "same day");
        // 22:00 UTC on the 10th is already the 11th in UTC+3.
        assert_eq!(days_since(t("2026-10-10T22:00:00Z"), t("2026-10-11T10:00:00Z"), &baghdad), 0);
        assert_eq!(days_since(t("2026-10-01T12:00:00Z"), t("2026-10-15T08:00:00Z"), &utc), 14);
    }

    #[test]
    fn the_log_keeps_what_jellyfin_forgot() {
        let mut h = History::default();
        h.record(&Snapshot {
            movies: vec![movie("m1", "Godzilla", 124905, "2026-10-01T20:00:00Z")],
            series: vec![series("s1", "Show", 500, false)],
            episodes: vec![ep("s1", 1, 1, "2026-10-02T20:00:00Z"), ep("s1", 1, 2, "2026-10-03T20:00:00Z")],
            ..Default::default()
        });
        // The files were deleted: Jellyfin has nothing left to say about them.
        h.record(&Snapshot::default());
        assert_eq!(h.movies["tmdb:124905"].title, "Godzilla");
        assert_eq!(h.series["s1"].episodes.len(), 2);
        assert_eq!(h.series["s1"].tvdb, Some(500));
        // A rewatch moves the date on; an older date never moves it back.
        h.record(&Snapshot { movies: vec![movie("m9", "Godzilla", 124905, "2026-10-09T20:00:00Z")], ..Default::default() });
        h.record(&Snapshot { movies: vec![movie("m9", "Godzilla", 124905, "2026-09-01T20:00:00Z")], ..Default::default() });
        assert_eq!(h.movies["tmdb:124905"].at, t("2026-10-09T20:00:00Z"));
    }

    #[test]
    fn seeds_only_from_finished_things_and_anime_after_five_episodes() {
        let mut h = History::default();
        let anime: HashSet<String> = ["a1".to_string(), "a2".to_string()].into();
        h.record(&Snapshot {
            movies: vec![movie("m1", "Finished film", 1, "2026-10-01T20:00:00Z")],
            series: vec![series("s1", "Half watched", 10, false), series("s2", "Watched it all", 20, true), series("a1", "Four eps", 30, false), series("a2", "Five eps", 40, false)],
            episodes: vec![
                ep("s1", 1, 1, "2026-10-02T20:00:00Z"),
                ep("a1", 1, 1, "2026-10-02T20:00:00Z"), ep("a1", 1, 2, "2026-10-02T20:00:00Z"), ep("a1", 1, 3, "2026-10-02T20:00:00Z"), ep("a1", 2, 1, "2026-10-02T20:00:00Z"),
                // Five across two seasons counts.
                ep("a2", 1, 1, "2026-10-03T20:00:00Z"), ep("a2", 1, 2, "2026-10-03T20:00:00Z"), ep("a2", 1, 3, "2026-10-03T20:00:00Z"), ep("a2", 2, 1, "2026-10-03T20:00:00Z"), ep("a2", 2, 2, "2026-10-04T20:00:00Z"),
            ],
            anime,
            ..Default::default()
        });
        let titles: Vec<String> = h.seeds().into_iter().map(|s| s.title).collect();
        assert_eq!(titles, vec!["Watched it all", "Five eps", "Finished film"], "latest first; nothing half watched, no 4-episode anime");
        assert!(History::default().seeds().is_empty(), "no history, no stand-in");
    }

    fn due(all: &[History], days: u32, now: &str) -> Vec<String> {
        let utc = FixedOffset::east_opt(0).unwrap();
        due_episodes(all, days, t(now), &utc).into_iter().map(|d| d.path).collect()
    }

    #[test]
    fn clean_up_waits_for_everyone_who_started_the_show() {
        let mut me = History::default();
        me.record(&Snapshot {
            series: vec![series("s1", "Show", 500, false)],
            episodes: vec![ep("s1", 1, 1, "2026-10-01T20:00:00Z"), ep("s1", 1, 2, "2026-10-01T21:00:00Z"), ep("s1", 1, 3, "2026-10-18T20:00:00Z")],
            ..Default::default()
        });
        let stranger = History::default();
        assert_eq!(due(&[me.clone(), stranger], 7, "2026-10-20T12:00:00Z"), vec![norm(&path(1, 1)), norm(&path(1, 2))], "E3 is only 2 days old; someone who never started it doesn't count");

        // A second profile is partway: watched E1, not E2, and halfway into E2.
        let mut sis = History::default();
        sis.record(&Snapshot {
            series: vec![series("s1", "Show", 500, false)],
            episodes: vec![ep("s1", 1, 1, "2026-10-19T20:00:00Z")],
            resumable: vec![json!({ "Type": "Episode", "SeriesId": "s1", "ParentIndexNumber": 1, "IndexNumber": 2, "Path": path(1, 2) })],
            ..Default::default()
        });
        assert!(due(&[me.clone(), sis.clone()], 7, "2026-10-20T12:00:00Z").is_empty(), "E1: her watch is the latest and only 1 day old; E2: she's on it");
        assert_eq!(due(&[me, sis], 7, "2026-10-27T12:00:00Z"), vec![norm(&path(1, 1))], "a week later E1 can go; E2 and E3 are still ahead of her");
    }

    #[test]
    fn a_show_downloaded_again_is_still_one_show() {
        // Her progress is under the show's old Jellyfin id; it was deleted and
        // downloaded again, and my watches are under the new id.
        let mut me = History::default();
        me.record(&Snapshot { series: vec![series("new", "Show", 500, false)], episodes: vec![ep("new", 1, 1, "2026-10-01T20:00:00Z"), ep("new", 1, 2, "2026-10-01T21:00:00Z")], ..Default::default() });
        let mut sis = History::default();
        sis.record(&Snapshot { series: vec![series("old", "Show", 500, false)], episodes: vec![ep("old", 1, 1, "2026-09-01T20:00:00Z")], ..Default::default() });
        assert_eq!(due(&[me, sis], 7, "2026-10-20T12:00:00Z"), vec![norm(&path(1, 1))], "E2 waits: she has started the show, under any id");
    }

    #[test]
    fn half_watched_is_only_true_right_now() {
        let mut me = History::default();
        let half = json!({ "Type": "Episode", "SeriesId": "s1", "ParentIndexNumber": 1, "IndexNumber": 1, "Path": path(1, 1) });
        me.record(&Snapshot { series: vec![series("s1", "Show", 500, false)], resumable: vec![half], ..Default::default() });
        assert!(me.series["s1"].resuming.contains(&norm(&path(1, 1))));
        assert_eq!(me.series["s1"].tvdb, Some(500), "a show only started is recorded, tvdb and all");
        me.record(&Snapshot { episodes: vec![ep("s1", 1, 1, "2026-10-02T20:00:00Z")], ..Default::default() });
        assert!(me.series["s1"].resuming.is_empty(), "finished now");
        assert_eq!(me.series["s1"].files.len(), 1);
    }

    #[test]
    fn movie_clean_up_uses_the_latest_watch_and_skips_half_watched() {
        let utc = FixedOffset::east_opt(0).unwrap();
        let now = t("2026-10-20T12:00:00Z");
        let mut me = History::default();
        me.record(&Snapshot { movies: vec![movie("m1", "Old", 1, "2026-10-01T20:00:00Z"), movie("m2", "Rewatched", 2, "2026-10-01T20:00:00Z"), movie("m3", "Busy", 3, "2026-10-01T20:00:00Z")], ..Default::default() });
        let mut sis = History::default();
        sis.record(&Snapshot {
            movies: vec![movie("m2", "Rewatched", 2, "2026-10-18T20:00:00Z")],
            resumable: vec![json!({ "Type": "Movie", "Id": "m3", "ProviderIds": { "Tmdb": "3" } })],
            ..Default::default()
        });
        assert_eq!(due_movies(&[me, sis], 7, now, &utc), vec![(1, "Old".to_string(), t("2026-10-01T20:00:00Z"))]);
    }

    #[test]
    fn a_file_counts_as_watched_only_if_it_was_there_when_watched() {
        let at = t("2026-08-10T20:00:00Z");
        assert!(file_predates(&json!("2026-08-01T10:00:00Z"), at));
        assert!(!file_predates(&json!("2026-10-18T10:00:00Z"), at), "downloaded again after watching: kept");
        assert!(!file_predates(&Value::Null, at) && !file_predates(&json!("soon"), at), "no date: kept");
    }

    #[test]
    fn anilist_recs_query_by_id_or_safe_title() {
        assert!(anilist_recs_query(Some(21), "x").contains("Media(id: 21, type: ANIME)"));
        let q = anilist_recs_query(None, "Frieren\" } evil {");
        assert!(q.contains("search: \"Frieren  evil \"") && !q.contains("\" }"));
    }

    #[test]
    fn forget_episodes_clears_complete_and_files() {
        let mut h = History::default();
        // The show under two Jellyfin ids (deleted and downloaded again), and another show.
        for (id, tvdb) in [("a", 7), ("b", 7), ("c", 8)] {
            let w = h.series.entry(id.into()).or_default();
            w.tvdb = Some(tvdb);
            w.complete_at = Some(5);
            for e in 1..=3 {
                w.episodes.insert(ep_key(1, e), 10);
                w.files.insert(norm(&path(1, e)), 10);
            }
            w.episodes.insert(ep_key(2, 1), 10);
            w.files.insert(norm(r"C:\Series\Show\Season 2\Show - 2x01.mkv"), 10);
        }
        h.series.get_mut("a").unwrap().files.insert(norm(r"C:\Series\Show\Season 1\Show S01E02E03.mkv"), 10);
        h.forget_episodes(7, &[(1, 2), (1, 3), (2, 1)]);
        for id in ["a", "b"] {
            let w = &h.series[id];
            assert_eq!(w.complete_at, None, "{id}: not complete any more");
            assert_eq!(w.episodes.keys().cloned().collect::<Vec<_>>(), vec![ep_key(1, 1)]);
            assert_eq!(w.files.keys().cloned().collect::<Vec<_>>(), vec![norm(&path(1, 1))], "{id}: the double episode and 2x01 go too");
        }
        let other = &h.series["c"];
        assert_eq!((other.complete_at, other.episodes.len(), other.files.len()), (Some(5), 4, 4), "another show is untouched");
    }

    const SHELF: &str = r"C:\Users\x\AppData\Roaming\com.viscyrius.chillarr\Chillarr Discover\Series\Show (2025) [tmdbid-9]";

    #[test]
    fn placeholder_plays_are_logged_but_never_as_files() {
        // The shelf's show holds only S01E01, so Jellyfin calls the show Played.
        let shelf_show = json!({ "Type": "Series", "Id": "ph", "Name": "Show", "Path": SHELF, "ProviderIds": { "Tvdb": "500" }, "UserData": { "Played": true, "LastPlayedDate": "2026-10-01T20:00:00Z" } });
        let shelf_ep = json!({ "Type": "Episode", "SeriesId": "ph", "ParentIndexNumber": 1, "IndexNumber": 1, "Path": format!(r"{SHELF}\Season 01\Show S01E01.strm"), "UserData": { "Played": true, "LastPlayedDate": "2026-10-01T20:00:00Z" } });
        let wn_movie = json!({ "Type": "Movie", "Id": "wm", "Name": "Inception", "Path": r"C:\a\Chillarr Watch now\Movies\Inception [tmdbid-27205]\Inception [tmdbid-27205].strm", "ProviderIds": { "Tmdb": "27205" }, "UserData": { "Played": true, "LastPlayedDate": "2026-10-01T20:00:00Z" } });
        let no_id = json!({ "Type": "Movie", "Id": "x", "Name": "Odd", "Path": r"C:\a\Chillarr Discover\Movies\Odd\Odd.strm", "UserData": { "Played": true, "LastPlayedDate": "2026-10-01T20:00:00Z" } });
        let mut me = History::default();
        me.record(&Snapshot { movies: vec![wn_movie, no_id], series: vec![shelf_show], episodes: vec![shelf_ep], ..Default::default() });
        let w = &me.series["ph"];
        assert_eq!((w.tvdb, w.episodes.keys().cloned().collect::<Vec<_>>()), (Some(500), vec![ep_key(1, 1)]), "counted, by tvdb and number");
        assert!(w.files.is_empty(), "a .strm is not a library file: the clean-up never sees it");
        assert_eq!(w.complete_at, None, "one streamed episode isn't the whole series");
        assert!(me.seeds().iter().all(|s| s.title != "Show"));
        assert_eq!(me.movies.keys().cloned().collect::<Vec<_>>(), vec!["tmdb:27205"], "a placeholder without a TMDB id isn't logged");
        assert!(due(&[me.clone()], 7, "2026-10-20T12:00:00Z").is_empty());

        // My sister watched E1 and E2 from the library. I've started the show
        // (streamed E1), so neither goes: my E1 was the placeholder, not her file.
        let mut sis = History::default();
        sis.record(&Snapshot { series: vec![series("real", "Show", 500, false)], episodes: vec![ep("real", 1, 1, "2026-10-02T20:00:00Z"), ep("real", 1, 2, "2026-10-02T21:00:00Z")], ..Default::default() });
        assert!(due(&[me, sis], 7, "2026-10-20T12:00:00Z").is_empty());
    }

    #[test]
    fn a_double_episode_is_both_episodes() {
        let mut h = History::default();
        let mut e = ep("s1", 1, 14, "2026-10-02T20:00:00Z");
        e["IndexNumberEnd"] = json!(15);
        e["Path"] = json!(r"C:\Series\Show\Season 1\Show S01E14E15.mkv");
        h.record(&Snapshot { series: vec![series("s1", "Show", 500, false)], episodes: vec![e], ..Default::default() });
        assert_eq!(h.series["s1"].episodes.keys().cloned().collect::<Vec<_>>(), vec![ep_key(1, 14), ep_key(1, 15)]);
        assert_eq!(h.series["s1"].files.len(), 1, "one file");
        assert_eq!(due(&[h], 7, "2026-10-20T12:00:00Z"), vec![norm(r"C:\Series\Show\Season 1\Show S01E14E15.mkv")]);
    }

    #[test]
    fn clean_up_takes_only_the_episodes_finished() {
        // Partway through a 10-episode season: E1-E3 watched, E4 half watched.
        let mut me = History::default();
        let half = json!({ "Type": "Episode", "SeriesId": "s1", "ParentIndexNumber": 1, "IndexNumber": 4, "Path": path(1, 4) });
        me.record(&Snapshot {
            series: vec![series("s1", "Show", 500, false)],
            episodes: (1..=3).map(|e| ep("s1", 1, e, "2026-10-01T20:00:00Z")).collect(),
            resumable: vec![half],
            ..Default::default()
        });
        assert_eq!(due(&[me.clone()], 7, "2026-10-20T12:00:00Z"), (1..=3).map(|e| norm(&path(1, e))).collect::<Vec<_>>(), "E4 (half watched) and E5-E10 (not watched) stay");

        // Watching E2 again moves its clock on; being partway into E3 again keeps it.
        let half3 = json!({ "Type": "Episode", "SeriesId": "s1", "ParentIndexNumber": 1, "IndexNumber": 3, "Path": path(1, 3) });
        me.record(&Snapshot { episodes: vec![ep("s1", 1, 2, "2026-10-18T20:00:00Z")], resumable: vec![half3], ..Default::default() });
        assert_eq!(due(&[me], 7, "2026-10-20T12:00:00Z"), vec![norm(&path(1, 1))]);
    }

    /// Reads this PC's Jellyfin and watch log (never writes either): a play
    /// streamed from a placeholder reaches the log, as numbers only.
    #[tokio::test]
    #[ignore]
    async fn live_placeholder_plays_reach_the_log() {
        let dir = PathBuf::from(std::env::var("APPDATA").unwrap()).join("com.viscyrius.chillarr");
        let s: crate::jellyfin::Session = serde_json::from_str(&crate::seal::read(&dir.join("jellyfin.json")).unwrap()).unwrap();
        let j = Jf::new("http://localhost:8096", &s.device_id, Some(&s));
        let mut h = load(&dir, &s.user_id);
        h.record(&snapshot(&j).await.expect("Jellyfin answered"));
        for w in h.series.values() {
            println!("{} tvdb={:?} episodes={:?} files={} complete={:?}", w.title, w.tvdb, w.episodes.keys().collect::<Vec<_>>(), w.files.len(), w.complete_at);
            assert!(w.files.keys().all(|f| !f.ends_with(".strm")), "{}: a placeholder in files", w.title);
        }
        assert!(h.series.values().any(|w| w.tvdb == Some(464185) && w.episodes.contains_key("1:1") && w.complete_at.is_none()));
    }

    #[test]
    fn newly_finished_lists_each_new_finish_with_when() {
        let show = |eps: &[(i64, i64, i64)]| {
            let mut w = SeriesWatch { tvdb: Some(7), ..Default::default() };
            for (s, e, at) in eps {
                w.episodes.insert(ep_key(*s, *e), *at);
            }
            w
        };
        let mut before = History::default();
        before.series.insert("a".into(), show(&[(1, 1, 10), (1, 2, 10)]));
        let mut after = History::default();
        after.series.insert("a".into(), show(&[(1, 1, 30), (1, 2, 10), (1, 3, 20), (0, 9, 20)]));
        let spot = |season, episode| crate::marker::Spot { season, episode };
        assert_eq!(newly_finished(&before, &after), vec![(7, spot(1, 1), 30), (7, spot(1, 3), 20)], "S1E1 again and S1E3 new, in order; specials don't count");
        assert!(newly_finished(&after, &after).is_empty());
    }
}
