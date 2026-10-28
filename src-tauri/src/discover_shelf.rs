//! The Discover shelf: popular movies and shows you don't have yet, as two
//! libraries inside Jellyfin, so any Jellyfin app (a TV, a phone) can ask for
//! them. Favourite one (the heart) and Chillarr gets it on this PC; it moves
//! into Movies or Series when it's done. Pressing play starts it too, and on
//! the home network plays it while it downloads.
//!
//! Each title is a folder holding one `.strm` file: a line of text with a
//! Chillarr address. Jellyfin reads the folder name ("Title (Year)
//! [tmdbid-123]") to fill in the poster and details by itself.

use crate::log::log;
// The shelf manages libraries and reads every account's favourites: owner only.
use crate::media::{arr, jf_admin as jf, Media};
use crate::model::{AppId, Level};
use crate::tiles::Tile;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

/// The folder's name is how Chillarr's own rows tell placeholders apart.
pub const FOLDER: &str = "Chillarr Discover";
const MOVIES_LIB: &str = "Discover Movies";
const SERIES_LIB: &str = "Discover Series";
/// Measured 2026-10-19: Jellyfin keeps 4.6 MB of pictures per shelf movie and
/// 5.3 MB per show (with its season and episode), 1.56 GB for the 200 + 120
/// here, plus a share of the 2 GB of cast photos all libraries use. That is
/// already past the 1.5 GB budget, so these stay where they are.
const MAX_MOVIES: usize = 200;
const MAX_SERIES: usize = 120;
/// The other films of a shelf movie's collection (Iron Man brings Iron Man 2
/// and 3), on top of the popular ones: this many per collection, in release
/// order, and this many in all.
const MATES_PER_COLLECTION: usize = 6;
const MAX_MATES: usize = 30;
/// Collections fetched per refresh (one call each to Radarr's metadata server).
const COLLECTION_FETCHES: usize = 25;
/// A title someone searched for in Jellyfin stays this long after the last
/// search, popular or not; at most `MAX_SEARCHED` of them, the latest kept.
const SEARCH_KEEP: u64 = 30 * 24 * 3600;
const MAX_SEARCHED: usize = 60;
/// Close matches one search adds: a handful, not the whole lookup.
const SEARCH_MOVIES: usize = 5;
const SEARCH_SHOWS: usize = 3;
/// Ratings a looked-up title needs before a search puts it on the shelf.
const MIN_VOTES: i64 = 50;
/// How long Jellyfin gets to take in new folders before search is reindexed
/// anyway. A library refresh had a new title in 4 s (measured 2026-10-19);
/// Jellyfin's own change watch waits 60 s (LibraryMonitorDelay) first.
const ARRIVE_WAIT: Duration = Duration::from_secs(90);
const PORTS: [u16; 3] = [8690, 8691, 8692];
/// A request head must arrive within this.
const HEAD_READ: Duration = Duration::from_secs(10);
/// Connections the play server takes at once while their heads come in.
const MAX_HEADS: usize = 64;
const FAVOURITE_EVERY: Duration = Duration::from_secs(30);
const REFRESH_EVERY: Duration = Duration::from_secs(60 * 60);
/// How long a play waits for the start of the film to arrive.
pub(crate) const HEAD_WAIT: Duration = Duration::from_secs(150);
/// How long it waits for any later part (a seek ahead of the download).
const PIECE_WAIT: Duration = Duration::from_secs(120);

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Entry {
    /// "movie" or "series".
    pub kind: String,
    /// TMDB id for movies, TVDB id for shows.
    pub id: i64,
    pub title: String,
    pub year: Option<i64>,
    /// Folder name under Movies/ or Series/.
    pub folder: String,
    /// Asked for (favourite or play); kept until it arrives.
    #[serde(default)]
    pub requested: bool,
    /// For shows: the TMDB id Jellyfin matches by (found once via Sonarr's metadata).
    #[serde(default)]
    pub tmdb: Option<i64>,
    /// Stopped after it was asked for. Reading its play address no longer asks
    /// again: Jellyfin's subtitle extraction and Intro Skipper keep reading it
    /// for minutes after playback ends, and each read used to restart the
    /// download (measured 2026-10-18, Rick and Morty came back 2 min after
    /// Stop). Only someone really playing it, a heart, or Download clears it.
    #[serde(default)]
    pub stopped: bool,
    /// When it was last searched for in Jellyfin (unix seconds): kept for
    /// `SEARCH_KEEP` after that even when it isn't popular.
    #[serde(default)]
    pub searched_at: Option<u64>,
    /// For shows: the episodes that have a placeholder, (season, episode) in
    /// TVDB numbering (Sonarr's). Empty for a show written before episodes
    /// were laid out: it had one S01E01 placeholder.
    #[serde(default)]
    pub episodes: Vec<(i64, i64)>,
    /// When the episode list was last read (unix seconds).
    #[serde(default)]
    pub episodes_at: Option<u64>,
    /// Episodes asked for one by one (played, or their heart pressed), not
    /// the whole show. Kept until they arrive.
    #[serde(default)]
    pub asked: Vec<(i64, i64)>,
    /// Episodes stopped after they were asked for: `stopped`, one episode at a time.
    #[serde(default)]
    pub stopped_eps: Vec<(i64, i64)>,
    /// Episodes of a show asked for episode by episode that are really in
    /// the library now: they have no placeholder (the TV would show two),
    /// the rest of the show stays on the shelf.
    #[serde(default)]
    pub arrived: Vec<(i64, i64)>,
}

impl Default for Entry {
    fn default() -> Self {
        Entry { kind: String::new(), id: 0, title: String::new(), year: None, folder: String::new(), requested: false, tmdb: None, stopped: false, searched_at: None, episodes: vec![], episodes_at: None, asked: vec![], stopped_eps: vec![], arrived: vec![] }
    }
}

impl Entry {
    /// Asked for in any way (the whole show, or some episodes) and on its way.
    fn on_its_way(&self) -> bool {
        self.requested || !self.asked.is_empty()
    }

    /// Being got episode by episode (not the whole show): it stays on the
    /// shelf, so the TV can go on to the next one, until all its episodes are here.
    fn by_episode(&self) -> bool {
        !self.requested && (!self.asked.is_empty() || !self.arrived.is_empty())
    }

    /// Kept on the shelf whatever the popular list says.
    fn kept(&self) -> bool {
        self.on_its_way() || !self.arrived.is_empty()
    }
}

/// Whether a read of this episode's placeholder is a stopped one (only a
/// real playback asks again). A show stopped as a whole covers every
/// episode except one asked for again since.
pub(crate) fn episode_stopped(e: &Entry, ep: (i64, i64)) -> bool {
    e.stopped_eps.contains(&ep) || (e.stopped && !e.asked.contains(&ep))
}

/// Whether this episode is already on its way: asked for itself, or part of
/// what a heart on the show got ("season1": season 1, "all": every season).
fn episode_covered(e: &Entry, mode: &str, ep: (i64, i64)) -> bool {
    if episode_stopped(e, ep) {
        return false;
    }
    e.asked.contains(&ep) || (e.requested && (mode == "all" || ep.0 == 1))
}

/// Shows with at most this many aired episodes get a placeholder for every
/// one; longer ones get whole seasons from the first while they fit (season
/// 1 always, up to this many). Episode pictures are off for the shelf
/// (`shelf_series_options`), so an episode costs Jellyfin a database row.
const MAX_EPISODES: usize = 60;
/// How often a shelf show's episode list is read again (new episodes air).
const EPISODES_EVERY: u64 = 24 * 3600;

/// The episodes of a show (skyhook's or Sonarr's list) to give placeholders:
/// regular seasons, aired by `today` ("YYYY-MM-DD"), in order, bounded by
/// `MAX_EPISODES` as whole seasons. Unaired ones are left out: playing them
/// would ask for something that doesn't exist yet.
fn episodes_to_place(eps: &[Value], today: &str) -> Vec<(i64, i64)> {
    let mut by_season: std::collections::BTreeMap<i64, Vec<i64>> = std::collections::BTreeMap::new();
    for e in eps {
        let (Some(s), Some(n)) = (e["seasonNumber"].as_i64(), e["episodeNumber"].as_i64()) else { continue };
        let aired = e["airDate"].as_str().is_some_and(|d| !d.is_empty() && d <= today);
        if s > 0 && n > 0 && aired {
            by_season.entry(s).or_default().push(n);
        }
    }
    let mut out: Vec<(i64, i64)> = Vec::new();
    for (s, mut list) in by_season {
        list.sort();
        list.dedup();
        if out.is_empty() {
            out.extend(list.into_iter().take(MAX_EPISODES).map(|n| (s, n)));
        } else if out.len() + list.len() <= MAX_EPISODES {
            out.extend(list.into_iter().map(|n| (s, n)));
        } else {
            break;
        }
    }
    out
}

/// What changes between two layouts of a show's placeholders: files to
/// write, files to delete. An old entry (no episodes listed) had only S01E01,
/// at an address that names no episode, so it's written again.
fn layout_change(old: &[(i64, i64)], new: &[(i64, i64)]) -> (Vec<(i64, i64)>, Vec<(i64, i64)>) {
    if old.is_empty() {
        return (new.to_vec(), if new.contains(&(1, 1)) { vec![] } else { vec![(1, 1)] });
    }
    let add = new.iter().filter(|x| !old.contains(x)).copied().collect();
    let remove = old.iter().filter(|x| !new.contains(x)).copied().collect();
    (add, remove)
}

/// The (season, episode) a shelf placeholder's file name gives ("Show S01E02.strm").
fn episode_of_file(path: &str) -> Option<(i64, i64)> {
    let name = path.rsplit(['/', '\\']).next()?;
    crate::qbit::episode_numbers(name).last().copied()
}

#[derive(Serialize, Deserialize, Default)]
struct Saved {
    /// Part of every play address, so nobody else on the network can use it.
    secret: String,
    /// How `secret` was made (`SECRET_V`); an older one is replaced at start.
    #[serde(default)]
    secret_v: u32,
    entries: Vec<Entry>,
    /// The address written into the placeholders; they're rewritten when it changes.
    #[serde(default)]
    base: String,
    /// "season1" or "all".
    #[serde(default = "season1")]
    series_mode: String,
    #[serde(default = "yes")]
    on: bool,
    /// Jellyfin apps show the server as "Chillarr" with its colours.
    #[serde(default = "yes")]
    brand: bool,
    /// The server's name before Chillarr renamed it, put back when branding is off.
    #[serde(default)]
    old_name: Option<String>,
}

fn season1() -> String {
    "season1".into()
}
fn yes() -> bool {
    true
}

pub struct Shelf {
    dir: PathBuf,
    file: PathBuf,
    saved: Mutex<Saved>,
    port: Mutex<Option<u16>>,
    /// Held while the entries and their folders change (the hourly refresh,
    /// a search), so two changes never undo each other.
    busy: tokio::sync::Mutex<()>,
}

#[derive(Serialize)]
pub struct Status {
    pub on: bool,
    pub brand: bool,
    pub series_mode: String,
    pub movies: usize,
    pub series: usize,
    pub requested: Vec<String>,
    pub play_address: Option<String>,
}

/// 32 hex characters from the OS's random source.
fn new_secret() -> String {
    crate::watchnow::new_token()
}

/// Secrets before this were made from the clock and could be guessed.
const SECRET_V: u32 = 2;

/// Gives `saved` a new secret if it has none or an old guessable one; returns
/// the one it replaced (empty when there was none).
fn renew_secret(saved: &mut Saved) -> Option<String> {
    if saved.secret.len() == 32 && saved.secret_v >= SECRET_V {
        return None;
    }
    let old = std::mem::replace(&mut saved.secret, new_secret());
    saved.secret_v = SECRET_V;
    // No saved address: the next refresh rewrites every placeholder.
    saved.base.clear();
    Some(old)
}

/// Swaps `old` for `new` in the play address of every placeholder under
/// `root` (shelf and Watch now alike). Returns how many changed.
fn swap_secret_in_placeholders(root: &std::path::Path, old: &str, new: &str) -> usize {
    if old.is_empty() {
        return 0;
    }
    let (from, to) = (format!("/{old}/"), format!("/{new}/"));
    let mut changed = 0;
    let mut dirs = vec![root.to_path_buf()];
    while let Some(d) = dirs.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for f in rd.flatten() {
            let p = f.path();
            match f.file_type() {
                Ok(t) if t.is_dir() => dirs.push(p),
                Ok(t) if t.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("strm")) => {
                    if let Ok(text) = std::fs::read_to_string(&p) {
                        if text.contains(&from) && std::fs::write(&p, text.replace(&from, &to)).is_ok() {
                            changed += 1;
                        }
                    }
                }
                _ => {}
            }
        }
    }
    changed
}

/// A folder name Windows and Jellyfin both accept.
pub fn safe_name(s: &str) -> String {
    let cleaned: String = s.chars().map(|c| if r#"<>:"/\|?*"#.contains(c) || c.is_control() { ' ' } else { c }).collect();
    let squashed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    squashed.trim_end_matches(['.', ' ']).chars().take(90).collect()
}

/// "Inception (2010) [tmdbid-27205]"; shows match by TMDB id when known, else by name and year.
pub fn folder_for(kind: &str, title: &str, year: Option<i64>, tmdb: Option<i64>) -> String {
    let mut s = safe_name(title);
    if let Some(y) = year {
        s.push_str(&format!(" ({y})"));
    }
    if let Some(t) = tmdb {
        s.push_str(&format!(" [tmdbid-{t}]"));
    }
    let _ = kind;
    s
}

impl Shelf {
    pub fn load(dir: PathBuf) -> Shelf {
        let file = dir.join("discover.json");
        let mut saved: Saved = std::fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| Saved { on: true, brand: true, series_mode: season1(), ..Default::default() });
        let old = renew_secret(&mut saved);
        let (new, had_file) = (saved.secret.clone(), file.exists());
        let shelf = Shelf { dir: dir.join(FOLDER), file, saved: Mutex::new(saved), port: Mutex::new(None), busy: tokio::sync::Mutex::new(()) };
        if let Some(old) = old.filter(|_| had_file) {
            // Saved first: placeholders still holding the old secret are put
            // right by the next refresh even if the swap below is cut short.
            shelf.store();
            swap_secret_in_placeholders(&shelf.dir, &old, &new);
            swap_secret_in_placeholders(&dir.join(crate::watchnow::LIBRARY), &old, &new);
        }
        shelf
    }

    fn store(&self) {
        let text = serde_json::to_string(&*self.saved.lock().unwrap()).unwrap_or_default();
        let _ = std::fs::write(&self.file, text);
    }

    fn kind_dir(&self, kind: &str) -> PathBuf {
        self.dir.join(if kind == "movie" { "Movies" } else { "Series" })
    }

    /// A movie's one placeholder.
    fn strm_path(&self, e: &Entry) -> PathBuf {
        let d = self.kind_dir(&e.kind).join(&e.folder);
        d.join(format!("{}.strm", e.folder))
    }

    /// A show's placeholder for one episode, named the way Jellyfin reads it.
    fn episode_path(&self, e: &Entry, (s, n): (i64, i64)) -> PathBuf {
        self.kind_dir(&e.kind).join(&e.folder).join(format!("Season {s:02}")).join(format!("{} S{s:02}E{n:02}.strm", safe_name(&e.title)))
    }

    /// Writes a placeholder: the movie's, or these episodes' of a show.
    fn write_files(&self, e: &Entry, eps: &[(i64, i64)], base: &str, secret: &str) -> std::io::Result<()> {
        let files: Vec<(PathBuf, String)> = if e.kind == "movie" {
            vec![(self.strm_path(e), format!("{base}/d/{secret}/movie/{}", e.id))]
        } else {
            eps.iter().map(|&(s, n)| (self.episode_path(e, (s, n)), format!("{base}/d/{secret}/series/{}/{s}/{n}", e.id))).collect()
        };
        for (path, text) in files {
            if let Some(p) = path.parent() {
                std::fs::create_dir_all(p)?;
            }
            std::fs::write(path, text)?;
        }
        Ok(())
    }

    /// Writes every placeholder of the entry (or only the missing ones).
    fn write(&self, e: &Entry, base: &str, secret: &str, only_missing: bool) -> std::io::Result<()> {
        if e.kind == "movie" {
            return if only_missing && self.strm_path(e).exists() { Ok(()) } else { self.write_files(e, &[], base, secret) };
        }
        // Not laid out yet (its list couldn't be read): the one S01E01 older versions wrote.
        let all = if e.episodes.is_empty() && e.arrived.is_empty() { vec![(1, 1)] } else { e.episodes.clone() };
        let eps: Vec<(i64, i64)> = all.into_iter().filter(|ep| !only_missing || !self.episode_path(e, *ep).exists()).collect();
        self.write_files(e, &eps, base, secret)
    }

    /// Takes these episodes' placeholders away, and a season folder left empty.
    fn remove_episodes(&self, e: &Entry, eps: &[(i64, i64)]) {
        for ep in eps {
            let p = self.episode_path(e, *ep);
            let _ = std::fs::remove_file(&p);
            if let Some(dir) = p.parent() {
                let _ = std::fs::remove_dir(dir);
            }
        }
    }

    fn remove(&self, e: &Entry) {
        let _ = std::fs::remove_dir_all(self.kind_dir(&e.kind).join(&e.folder));
    }

    /// The play server's port, once it's listening.
    pub(crate) fn port(&self) -> Option<u16> {
        *self.port.lock().unwrap()
    }

    /// The part of every play address that keeps others on the network out.
    pub(crate) fn secret(&self) -> String {
        self.saved.lock().unwrap().secret.clone()
    }

    /// Whether the shelf has asked for this title ("movie" by TMDB id,
    /// "series" by TVDB id) and not been stopped since.
    pub(crate) fn wants(&self, kind: &str, id: i64) -> bool {
        self.saved.lock().unwrap().entries.iter().any(|e| e.kind == kind && e.id == id && ((e.requested && !e.stopped) || !e.asked.is_empty()))
    }

    pub fn status(&self) -> Status {
        let s = self.saved.lock().unwrap();
        Status {
            on: s.on,
            brand: s.brand,
            series_mode: s.series_mode.clone(),
            movies: s.entries.iter().filter(|e| e.kind == "movie" && !e.on_its_way()).count(),
            series: s.entries.iter().filter(|e| e.kind == "series" && !e.on_its_way()).count(),
            requested: s
                .entries
                .iter()
                .filter(|e| e.on_its_way())
                .map(|e| if e.requested { e.title.clone() } else { format!("{} {}", e.title, e.asked.iter().map(|(s, n)| format!("S{s:02}E{n:02}")).collect::<Vec<_>>().join(", ")) })
                .collect(),
            play_address: (!s.base.is_empty()).then(|| s.base.clone()),
        }
    }
}

/// The address Jellyfin apps on the home network use to reach this PC's player.
fn play_base(port: u16) -> String {
    let ip = crate::arr::lan_ip().unwrap_or_else(|| "127.0.0.1".into());
    format!("http://{ip}:{port}")
}

fn win_path(p: &std::path::Path) -> String {
    p.to_string_lossy().replace('/', "\\")
}

// ---------- Jellyfin libraries ----------

/// Library options for folders of placeholders (the shelf's, Watch now's):
/// no chapter pictures, trickplay or saved files, so nothing in Jellyfin
/// reads a download in the background; quick to scan.
pub(crate) fn placeholder_options() -> Value {
    json!({
        "EnableRealtimeMonitor": true,
        "EnableChapterImageExtraction": false,
        "ExtractChapterImagesDuringLibraryScan": false,
        "SaveLocalMetadata": false,
        "EnableTrickplayImageExtraction": false,
        "EnableEmbeddedTitles": false
    })
}

/// Puts `placeholder_options` into a library's existing options. True when
/// anything changed (so it needs saving).
pub(crate) fn quiet_options(opts: &mut Value) -> bool {
    if !opts.is_object() {
        *opts = json!({});
    }
    let mut changed = false;
    for (k, v) in placeholder_options().as_object().into_iter().flatten() {
        if opts.get(k) != Some(v) {
            opts[k] = v.clone();
            changed = true;
        }
    }
    changed
}

/// The shelf's show library takes no episode pictures. TMDB's stills are
/// 0.65 MB each on average (123 shelf episodes measured 2026-10-19), so a
/// 60-episode show would cost 39 MB. With no fetchers at all Jellyfin still
/// fills in names and plots, and nothing grabs a frame from the placeholder
/// (which would ask for the episode). Measured on a probe library with 35
/// placeholders: no pictures, and no read of any placeholder. True when
/// anything changed.
fn shelf_series_options(opts: &mut Value) -> bool {
    let mut changed = quiet_options(opts);
    let episode = json!({
        "Type": "Episode",
        "MetadataFetchers": ["TheMovieDb", "The Open Movie Database"],
        "MetadataFetcherOrder": ["TheMovieDb", "The Open Movie Database"],
        "ImageFetchers": [],
        "ImageFetcherOrder": [],
        "ImageOptions": []
    });
    let mut types: Vec<Value> = opts["TypeOptions"].as_array().cloned().unwrap_or_default();
    match types.iter_mut().find(|t| t["Type"] == "Episode") {
        Some(t) if t["ImageFetchers"].as_array().is_some_and(|a| a.is_empty()) => {}
        Some(t) => {
            *t = episode;
            changed = true;
        }
        None => {
            types.push(episode);
            changed = true;
        }
    }
    opts["TypeOptions"] = json!(types);
    changed
}

/// The two Discover libraries, made once. Returns their ids (movies, series).
async fn ensure_libraries(app: &AppHandle) -> Option<(String, String)> {
    let j = jf(app).filter(|j| j.user_id.is_some())?;
    let shelf = app.state::<Shelf>();
    for kind in ["movie", "series"] {
        std::fs::create_dir_all(shelf.kind_dir(kind)).ok()?;
    }
    let libs = j.libraries().await.ok()?;
    for (name, kind, sub) in [(MOVIES_LIB, "movies", "movie"), (SERIES_LIB, "tvshows", "series")] {
        if libs.iter().any(|l| l["Name"] == name) {
            continue;
        }
        let path = win_path(&shelf.kind_dir(sub));
        let mut lib = placeholder_options();
        if sub == "series" {
            shelf_series_options(&mut lib);
        }
        lib["PathInfos"] = json!([{ "Path": path }]);
        let opts = json!({ "LibraryOptions": lib });
        let q = format!("/Library/VirtualFolders?name={}&collectionType={kind}&refreshLibrary=false", name.replace(' ', "%20"));
        if j.post_json(&q, &opts).await.is_ok() {
            log(app, Level::Ok, format!("Added the {name} shelf to Jellyfin."));
        }
    }
    let libs = j.libraries().await.ok()?;
    let id = |n: &str| libs.iter().find(|l| l["Name"] == n).and_then(|l| l["ItemId"].as_str()).map(String::from);
    // A series library made by an older version still takes episode pictures.
    if let Some(l) = libs.iter().find(|l| l["Name"] == SERIES_LIB) {
        let mut opts = l["LibraryOptions"].clone();
        if shelf_series_options(&mut opts) {
            let body = json!({ "Id": l["ItemId"], "LibraryOptions": opts });
            // Said once: this runs with every favourites check.
            static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if let Err(e) = j.post_json("/Library/VirtualFolders/LibraryOptions", &body).await {
                if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    log(app, Level::Amber, format!("Couldn't turn off episode pictures for the {SERIES_LIB} shelf, so each episode costs Jellyfin a picture: {e}"));
                }
            }
        }
    }
    Some((id(MOVIES_LIB)?, id(SERIES_LIB)?))
}

async fn remove_libraries(app: &AppHandle) {
    let Some(j) = jf(app).filter(|j| j.user_id.is_some()) else { return };
    for name in [MOVIES_LIB, SERIES_LIB] {
        let _ = j.delete_path(&format!("/Library/VirtualFolders?name={}&refreshLibrary=false", name.replace(' ', "%20"))).await;
    }
}

async fn refresh_library(app: &AppHandle, id: &str) {
    if let Some(j) = jf(app).filter(|j| j.user_id.is_some()) {
        let _ = j.post_json(&format!("/Items/{id}/Refresh?recursive=true&metadataRefreshMode=Default&imageRefreshMode=Default"), &json!({})).await;
    }
}

/// The file paths Jellyfin has in these libraries, lower case.
async fn paths_in(app: &AppHandle, libs: &[String]) -> Option<Vec<String>> {
    let j = jf(app).filter(|j| j.user_id.is_some())?;
    let uid = j.user_id.clone()?;
    let mut out = Vec::new();
    for lib in libs {
        let v = j.get_json(&format!("/Users/{uid}/Items?parentId={lib}&recursive=true&includeItemTypes=Movie,Series&fields=Path&enableImages=false&enableUserData=false&limit=10000")).await.ok()?;
        out.extend(v["Items"].as_array().into_iter().flatten().filter_map(|i| i["Path"].as_str()).map(|p| p.replace('/', "\\").to_ascii_lowercase()));
    }
    Some(out)
}

/// Whether Jellyfin shows every folder in `added` and none in `gone`. A movie's
/// path is its .strm file inside the folder; a show's is the folder itself.
fn settled(paths: &[String], added: &[String], gone: &[String]) -> bool {
    let has = |folder: &String| {
        let f = folder.to_ascii_lowercase();
        let inside = format!("{f}\\");
        paths.iter().any(|p| *p == f || p.starts_with(&inside))
    };
    added.iter().all(has) && !gone.iter().any(has)
}

/// After the shelf's folders change: Jellyfin rescans the changed libraries,
/// Chillarr waits (up to `ARRIVE_WAIT`) until it has them, then Jellyfin's
/// search engine is reindexed so a search finds them. The engine's plugin
/// only indexes on its own schedule, which is why a new title used to show
/// up in search "after a while".
async fn settle(app: &AppHandle, libs: Vec<String>, added: Vec<String>, gone: Vec<String>) {
    for lib in &libs {
        refresh_library(app, lib).await;
    }
    let until = std::time::Instant::now() + ARRIVE_WAIT;
    let mut ready = false;
    while std::time::Instant::now() < until {
        tokio::time::sleep(Duration::from_secs(2)).await;
        if paths_in(app, &libs).await.is_some_and(|p| settled(&p, &added, &gone)) {
            ready = true;
            break;
        }
    }
    if !ready {
        log(app, Level::Amber, format!("Jellyfin hadn't taken in the Discover shelf's changes after {} s, so search may miss some of them until it does.", ARRIVE_WAIT.as_secs()));
    }
    crate::meili::reindex(app).await;
}

// ---------- Filling the shelf ----------

/// What's already here or on its way: TMDB ids of movies, TVDB ids of shows.
async fn have(app: &AppHandle) -> (HashSet<i64>, HashSet<i64>) {
    let (mut movies, mut shows) = (HashSet::new(), HashSet::new());
    // A title Radarr or Sonarr only remembers (stopped: not wanted, no file)
    // isn't had: it belongs on the shelf. Iron Man 2 sat in Radarr like that
    // and so never showed up in Jellyfin's search (2026-10-19). A request
    // switches it back on (media::add).
    if let Some(a) = arr(app, AppId::Radarr) {
        movies.extend(a.list("movie").await.unwrap_or_default().iter().filter(|m| m["monitored"] == true || m["hasFile"] == true).filter_map(|m| m["tmdbId"].as_i64()));
    }
    if let Some(a) = arr(app, AppId::Sonarr) {
        let had = |s: &Value| s["monitored"] == true || s["statistics"]["episodeFileCount"].as_i64().unwrap_or(0) > 0;
        shows.extend(a.list("series").await.unwrap_or_default().iter().filter(|s| had(s)).filter_map(|s| s["tvdbId"].as_i64()));
    }
    if let Some(j) = jf(app).filter(|j| j.user_id.is_some()) {
        for v in j.items("recursive=true&includeItemTypes=Movie,Series&limit=10000").await.unwrap_or_default() {
            let id = |k: &str| v["ProviderIds"][k].as_str().and_then(|s| s.parse::<i64>().ok());
            if v["Type"] == "Movie" {
                movies.extend(id("Tmdb"));
            } else {
                shows.extend(id("Tvdb"));
            }
        }
    }
    (movies, shows)
}

/// What Sonarr's metadata server knows of these shows: the TMDB id (Jellyfin
/// matches shows by TMDB) and every episode. A show it couldn't give is missing.
async fn show_info(tvdbs: Vec<i64>) -> HashMap<i64, Value> {
    let mut out = HashMap::new();
    let mut set = tokio::task::JoinSet::new();
    for chunk in tvdbs.chunks(8) {
        for &t in chunk {
            set.spawn(async move {
                let v: Option<Value> = async { crate::media::http().get(format!("{}/{t}", crate::pick::SKYHOOK)).send().await.ok()?.error_for_status().ok()?.json().await.ok() }.await;
                (t, v.filter(|v| v["episodes"].is_array()))
            });
        }
        while let Some(Ok((t, v))) = set.join_next().await {
            if let Some(v) = v {
                out.insert(t, v);
            }
        }
    }
    out
}

fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// The episodes to lay out for a show, from its metadata.
fn placed(info: &Value) -> Vec<(i64, i64)> {
    episodes_to_place(info["episodes"].as_array().map(|v| v.as_slice()).unwrap_or_default(), &today())
}

/// Shows whose episode list is due a read: never laid out, or read longer ago than `EPISODES_EVERY`.
fn episodes_due(e: &Entry, now: u64) -> bool {
    e.kind == "series" && (e.episodes.is_empty() || e.episodes_at.is_none_or(|at| now.saturating_sub(at) >= EPISODES_EVERY))
}

/// Fetches the films of these movies' collections into the catalog (the
/// metadata server only names a movie's collection, not its other films).
async fn fetch_collections(app: &AppHandle, movies: &[i64]) {
    let media = app.state::<Media>();
    let todo = media.catalog.lock().unwrap().collections_to_fetch(movies, crate::catalog::now_secs(), COLLECTION_FETCHES);
    if todo.is_empty() {
        return;
    }
    let http = crate::media::http();
    let mut failed = 0;
    for c in todo {
        match http.get(format!("https://api.radarr.video/v1/movie/collection/{c}")).send().await {
            Ok(r) if r.status().is_success() => match r.json::<Value>().await {
                Ok(v) => media.catalog.lock().unwrap().add_collection(&v),
                Err(_) => failed += 1,
            },
            _ => failed += 1,
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    if failed > 0 {
        log(app, Level::Amber, format!("Couldn't fetch {failed} film series from Radarr's metadata server; their other films join the Discover shelf on a later try."));
    }
    if let Some(dir) = crate::media::data_dir(app) {
        let c = media.catalog.lock().unwrap();
        let _ = c.save(&dir.join("catalog.json"));
    }
}

/// The other films of the picked movies' collections to put on the shelf, in
/// pick order: `MATES_PER_COLLECTION` from each, `MAX_MATES` in all, none in
/// `skip` (owned, or on the shelf already).
fn mates_to_add(picked: &[(i64, Option<i64>)], members: &HashMap<i64, Vec<Tile>>, skip: &HashSet<i64>) -> Vec<Tile> {
    let mut out: Vec<Tile> = Vec::new();
    let mut done = HashSet::new();
    for (_, c) in picked {
        let Some(c) = c.filter(|c| done.insert(*c)) else { continue };
        let mut from_this = 0;
        for t in members.get(&c).into_iter().flatten() {
            if out.len() >= MAX_MATES {
                return out;
            }
            if from_this >= MATES_PER_COLLECTION {
                break;
            }
            if t.tmdb.is_some_and(|id| !skip.contains(&id) && !out.iter().any(|o| o.tmdb == Some(id))) {
                out.push(t.clone());
                from_this += 1;
            }
        }
    }
    out
}

/// Searched-for titles that stay: searched in the last `SEARCH_KEEP`, the
/// latest `MAX_SEARCHED`. Asked-for ones are kept anyway, so not counted here.
/// One you have by now goes, like a popular one would: Replicas stayed on the
/// shelf beside its real copy after a search and a download (2026-10-20).
fn kept_searched(entries: &[Entry], now: u64, have: (&HashSet<i64>, &HashSet<i64>)) -> Vec<Entry> {
    let had = |e: &Entry| if e.kind == "movie" { have.0.contains(&e.id) } else { have.1.contains(&e.id) };
    let mut v: Vec<Entry> = entries.iter().filter(|e| !e.requested && !had(e) && e.searched_at.is_some_and(|at| now.saturating_sub(at) < SEARCH_KEEP)).cloned().collect();
    v.sort_by_key(|e| std::cmp::Reverse(e.searched_at));
    v.truncate(MAX_SEARCHED);
    v
}

fn movie_entry(t: &Tile, id: i64) -> Entry {
    Entry { kind: "movie".into(), id, folder: folder_for("movie", &t.title, t.year, Some(id)), title: t.title.clone(), year: t.year, requested: false, tmdb: Some(id), ..Default::default() }
}

/// Brings the shelf up to date: adds popular titles you don't have (and the
/// rest of their film series), drops ones you got some other way, keeps
/// everything that was asked for or recently searched for.
pub async fn refresh(app: &AppHandle) {
    let shelf = app.state::<Shelf>();
    if !shelf.saved.lock().unwrap().on {
        return;
    }
    let Some((movies_lib, series_lib)) = ensure_libraries(app).await else { return };
    let port = *shelf.port.lock().unwrap();
    let Some(port) = port else { return };
    let base = play_base(port);
    let (have_movies, have_shows) = have(app).await;
    let (pop_movies, pop_shows): (Vec<Tile>, Vec<Tile>) = {
        let media = app.state::<Media>();
        let c = media.catalog.lock().unwrap();
        let mut m = c.top10();
        m.extend(c.popular(MAX_MOVIES * 2));
        let s = media.shows.lock().unwrap().popular(MAX_SERIES * 2);
        (m, s)
    };
    let now = crate::catalog::now_secs();
    let searched = kept_searched(&shelf.saved.lock().unwrap().entries, now, (&have_movies, &have_shows));
    let mut want: Vec<Entry> = Vec::new();
    let mut seen = HashSet::new();
    for t in pop_movies {
        let Some(id) = t.tmdb else { continue };
        if have_movies.contains(&id) || !seen.insert(("m", id)) || want.iter().filter(|e| e.kind == "movie").count() >= MAX_MOVIES {
            continue;
        }
        want.push(movie_entry(&t, id));
    }
    // Film series: the rest of each picked (or searched) movie's collection.
    let movie_ids: Vec<i64> = want.iter().chain(searched.iter()).filter(|e| e.kind == "movie").map(|e| e.id).collect();
    fetch_collections(app, &movie_ids).await;
    let mates = {
        let media = app.state::<Media>();
        let c = media.catalog.lock().unwrap();
        let picked: Vec<(i64, Option<i64>)> = movie_ids.iter().map(|id| (*id, c.collection_of(*id))).collect();
        let members = c.collection_members(&picked.iter().filter_map(|p| p.1).collect());
        let mut skip: HashSet<i64> = have_movies.clone();
        skip.extend(movie_ids.iter().copied());
        mates_to_add(&picked, &members, &skip)
    };
    for t in &mates {
        let id = t.tmdb.unwrap_or_default();
        if seen.insert(("m", id)) {
            want.push(movie_entry(t, id));
        }
    }
    let show_list: Vec<Tile> = pop_shows.into_iter().filter(|t| t.tvdb.is_some_and(|id| !have_shows.contains(&id))).take(MAX_SERIES).collect();
    let known: HashMap<i64, Entry> = shelf.saved.lock().unwrap().entries.iter().filter(|e| e.kind == "series").map(|e| (e.id, e.clone())).collect();
    // Episode lists: a new show needs one before it goes on the shelf; one
    // already there is read again once a day (new episodes air). Only shows
    // that will stay are read.
    let listed: HashSet<i64> = show_list.iter().filter_map(|t| t.tvdb).collect();
    let mut to_read: Vec<i64> = listed.iter().filter(|id| !known.contains_key(id)).copied().collect();
    to_read.extend(known.values().filter(|e| episodes_due(e, now) && (listed.contains(&e.id) || e.kept() || e.searched_at.is_some())).map(|e| e.id));
    let info = show_info(to_read.clone()).await;
    let unread: Vec<i64> = to_read.iter().filter(|id| !info.contains_key(id)).copied().collect();
    for t in show_list {
        let id = t.tvdb.unwrap();
        if !seen.insert(("s", id)) {
            continue;
        }
        if let Some(prev) = known.get(&id) {
            want.push(prev.clone());
            continue;
        }
        // No episode list (yet): it joins on a later refresh. None aired: nothing to play.
        let Some(i) = info.get(&id) else { continue };
        let episodes = placed(i);
        if episodes.is_empty() {
            continue;
        }
        let tmdb = i["tmdbId"].as_i64();
        want.push(Entry { kind: "series".into(), id, folder: folder_for("series", &t.title, t.year, tmdb), title: t.title, year: t.year, tmdb, episodes, episodes_at: Some(now), ..Default::default() });
    }

    let busy = shelf.busy.lock().await;
    let (secret, old_base, old) = {
        let s = shelf.saved.lock().unwrap();
        (s.secret.clone(), s.base.clone(), s.entries.clone())
    };
    let rewrite_all = old_base != base;
    let mut keep: Vec<Entry> = Vec::new();
    let (mut added, mut gone): (Vec<&Entry>, Vec<&Entry>) = (Vec::new(), Vec::new());
    // Asked-for titles stay until they arrive, even if they fell off the popular
    // list; searched-for ones stay a while (a search may have marked more since).
    let searched = kept_searched(&old, now, (&have_movies, &have_shows));
    for e in old.iter().filter(|e| e.kept()).chain(searched.iter()) {
        if !keep.iter().any(|k| k.kind == e.kind && k.id == e.id) {
            keep.push(e.clone());
        }
    }
    for w in &want {
        if keep.iter().any(|k| k.kind == w.kind && k.id == w.id) {
            continue;
        }
        if let Some(prev) = old.iter().find(|o| o.kind == w.kind && o.id == w.id) {
            keep.push(prev.clone());
        } else {
            added.push(w);
            keep.push(w.clone());
        }
    }
    for o in &old {
        if !keep.iter().any(|k| k.kind == o.kind && k.id == o.id) {
            shelf.remove(o);
            gone.push(o);
        }
    }
    // A show's placeholders change only when its episode list did (or it's
    // new, or from before episodes were laid out), not every refresh.
    let mut relaid = 0;
    for e in keep.iter_mut() {
        // Episodes already in the library keep no placeholder.
        let fresh = info.get(&e.id).filter(|_| e.kind == "series").map(placed).map(|v| v.into_iter().filter(|x| !e.arrived.contains(x)).collect::<Vec<_>>());
        if let Some(new) = fresh.filter(|v| !v.is_empty()) {
            e.episodes_at = Some(now);
            if new != e.episodes {
                let (add, remove) = layout_change(&e.episodes, &new);
                shelf.remove_episodes(e, &remove);
                e.episodes = new;
                let _ = shelf.write_files(e, &add, &base, &secret);
                relaid += 1;
            }
        }
        let _ = shelf.write(e, &base, &secret, !rewrite_all);
    }
    {
        let mut s = shelf.saved.lock().unwrap();
        s.entries = keep;
        s.base = base;
    }
    shelf.store();
    drop(busy);
    if !unread.is_empty() {
        log(app, Level::Amber, format!("Couldn't read the episode lists of {} shows from Sonarr's metadata server; they're put on the Discover shelf (or brought up to date) on a later refresh.", unread.len()));
    }
    if !added.is_empty() || !gone.is_empty() || rewrite_all || relaid > 0 {
        let mates_added = added.iter().filter(|a| a.kind == "movie" && mates.iter().any(|m| m.tmdb == Some(a.id))).count();
        let note = if mates_added > 0 { format!(" ({mates_added} of them the rest of a film series)") } else { String::new() };
        let relaid_note = if relaid > 0 { format!(", episodes of {relaid} shows updated") } else { String::new() };
        log(app, Level::Ok, format!("Discover shelf in Jellyfin: {} new{note}, {} gone{relaid_note}.", added.len(), gone.len()));
        let changed = |kind: &str| rewrite_all || (kind == "series" && relaid > 0) || added.iter().chain(gone.iter()).any(|e| e.kind == kind);
        let libs: Vec<String> = [("movie", &movies_lib), ("series", &series_lib)].into_iter().filter(|(k, _)| changed(k)).map(|(_, l)| l.clone()).collect();
        let folder = |e: &&Entry| win_path(&shelf.kind_dir(&e.kind).join(&e.folder));
        let (added, gone) = (added.iter().map(folder).collect(), gone.iter().map(folder).collect());
        settle(app, libs, added, gone).await;
    }
}

// ---------- Titles searched for in Jellyfin ----------

/// One title a Radarr or Sonarr lookup found.
#[derive(Debug, Clone, PartialEq)]
struct Hit {
    kind: &'static str,
    /// TMDB id for movies, TVDB id for shows.
    id: i64,
    title: String,
    year: Option<i64>,
    tmdb: Option<i64>,
    /// Ratings count: how known it is.
    votes: i64,
}

fn movie_hits(v: &[Value]) -> Vec<Hit> {
    v.iter()
        .filter_map(|m| {
            let id = m["tmdbId"].as_i64().filter(|i| *i > 0)?;
            Some(Hit { kind: "movie", id, title: m["title"].as_str()?.to_string(), year: m["year"].as_i64().filter(|y| *y > 0), tmdb: Some(id), votes: m["ratings"]["tmdb"]["votes"].as_i64().unwrap_or(0) })
        })
        .collect()
}

fn show_hits(v: &[Value]) -> Vec<Hit> {
    v.iter()
        .filter_map(|s| {
            let id = s["tvdbId"].as_i64().filter(|i| *i > 0)?;
            Some(Hit { kind: "series", id, title: s["title"].as_str()?.to_string(), year: s["year"].as_i64().filter(|y| *y > 0), tmdb: s["tmdbId"].as_i64().filter(|i| *i > 0), votes: s["ratings"]["votes"].as_i64().unwrap_or(0) })
        })
        .collect()
}

/// The lookup results close to what was typed (the same closeness Chillarr's
/// own search uses), at most `n`, none you already have and none almost
/// nobody has rated ("iron man" also finds a 1931 Iron Man with 20 votes).
/// The exact title and titles starting with it count as equally close, so
/// the best known come first (Iron Man 3 before the 1951 Iron Man).
fn close_hits(term: &str, hits: Vec<Hit>, owned: &HashSet<i64>, n: usize) -> Vec<Hit> {
    let mut v: Vec<(u8, Hit)> = hits.into_iter().filter(|h| !owned.contains(&h.id) && h.votes >= MIN_VOTES).filter_map(|h| crate::media::match_rank(&h.title, term).map(|r| (r.max(1), h))).collect();
    v.sort_by_key(|(r, h)| (*r, std::cmp::Reverse(h.votes)));
    let mut out: Vec<Hit> = Vec::new();
    for (_, h) in v {
        if out.len() < n && !out.iter().any(|o| o.id == h.id) {
            out.push(h);
        }
    }
    out
}

async fn lookup(app: &AppHandle, which: AppId, path: &str) -> Vec<Value> {
    let Some(a) = arr(app, which) else { return vec![] };
    match a.list(path).await {
        Ok(v) => v,
        Err(e) => {
            log(app, Level::Amber, format!("A search in Jellyfin couldn't look in {} for titles to add to the Discover shelf: {e}", if which == AppId::Radarr { "Radarr" } else { "Sonarr" }));
            vec![]
        }
    }
}

/// Someone searched Jellyfin for `term` (seen by the search engine's proxy):
/// the close matches you don't have go on the shelf, marked as searched for,
/// and search is reindexed so the next search finds them.
pub async fn add_searched(app: &AppHandle, term: &str) {
    let shelf = app.state::<Shelf>();
    if !shelf.saved.lock().unwrap().on {
        return;
    }
    let port = *shelf.port.lock().unwrap();
    let Some(port) = port else { return };
    let Some((movies_lib, series_lib)) = ensure_libraries(app).await else { return };
    let enc = urlenc(term);
    let (mpath, spath) = (format!("movie/lookup?term={enc}"), format!("series/lookup?term={enc}"));
    let (movies, shows, (have_movies, have_shows)) = tokio::join!(lookup(app, AppId::Radarr, &mpath), lookup(app, AppId::Sonarr, &spath), have(app));
    let mut hits = close_hits(term, movie_hits(&movies), &have_movies, SEARCH_MOVIES);
    let mut shows = close_hits(term, show_hits(&shows), &have_shows, SEARCH_SHOWS);
    let known: HashSet<i64> = shelf.saved.lock().unwrap().entries.iter().filter(|e| e.kind == "series").map(|e| e.id).collect();
    // A new show goes on the shelf with its episodes, so it needs their list.
    let info = show_info(shows.iter().filter(|h| !known.contains(&h.id)).map(|h| h.id).collect()).await;
    let mut unread = Vec::new();
    let mut episodes: HashMap<i64, Vec<(i64, i64)>> = HashMap::new();
    shows.retain_mut(|h| {
        if known.contains(&h.id) {
            return true;
        }
        let Some(i) = info.get(&h.id) else {
            unread.push(h.title.clone());
            return false;
        };
        h.tmdb = h.tmdb.or_else(|| i["tmdbId"].as_i64());
        let eps = placed(i);
        // Nothing aired yet: nothing to play.
        if eps.is_empty() {
            return false;
        }
        episodes.insert(h.id, eps);
        true
    });
    if !unread.is_empty() {
        log(app, Level::Amber, format!("Couldn't read the episode list of {} from Sonarr's metadata server, so a search didn't put it on the Discover shelf.", unread.join(", ")));
    }
    hits.extend(shows);
    if hits.is_empty() {
        return;
    }
    let busy = shelf.busy.lock().await;
    let now = crate::catalog::now_secs();
    let (secret, base) = {
        let s = shelf.saved.lock().unwrap();
        (s.secret.clone(), if s.base.is_empty() { play_base(port) } else { s.base.clone() })
    };
    let mut added: Vec<Entry> = Vec::new();
    {
        let mut s = shelf.saved.lock().unwrap();
        for h in hits {
            if let Some(e) = s.entries.iter_mut().find(|e| e.kind == h.kind && e.id == h.id) {
                e.searched_at = Some(now);
                continue;
            }
            let eps = episodes.remove(&h.id).unwrap_or_default();
            let e = Entry { kind: h.kind.into(), id: h.id, folder: folder_for(h.kind, &h.title, h.year, h.tmdb), title: h.title, year: h.year, tmdb: h.tmdb, searched_at: Some(now), episodes_at: (!eps.is_empty()).then_some(now), episodes: eps, ..Default::default() };
            s.entries.push(e.clone());
            added.push(e);
        }
    }
    let mut failed = Vec::new();
    added.retain(|e| match shelf.write(e, &base, &secret, false) {
        Ok(()) => true,
        Err(err) => {
            failed.push(format!("{} ({err})", e.title));
            false
        }
    });
    shelf.store();
    drop(busy);
    if !failed.is_empty() {
        log(app, Level::Amber, format!("Couldn't put these on the Discover shelf: {}", failed.join(", ")));
    }
    if added.is_empty() {
        return;
    }
    let names: Vec<&str> = added.iter().map(|e| e.title.as_str()).collect();
    log(app, Level::Ok, format!("Searched for \"{term}\" in Jellyfin: added {} to the Discover shelf.", names.join(", ")));
    let libs: Vec<String> = [("movie", &movies_lib), ("series", &series_lib)].into_iter().filter(|(k, _)| added.iter().any(|e| e.kind == *k)).map(|(_, l)| l.clone()).collect();
    let folders = added.iter().map(|e| win_path(&shelf.kind_dir(&e.kind).join(&e.folder))).collect();
    settle(app, libs, folders, vec![]).await;
}

// ---------- Asking for a title ----------

/// Gets a shelf title on this PC (once), and marks it as on its way.
async fn request(app: &AppHandle, kind: &str, id: i64, why: &str) -> Result<(), String> {
    let shelf = app.state::<Shelf>();
    let entry = {
        let s = shelf.saved.lock().unwrap();
        s.entries.iter().find(|e| e.kind == kind && e.id == id).cloned()
    };
    let Some(entry) = entry else { return Err("That title isn't on the shelf any more.".into()) };
    if entry.requested {
        return Ok(());
    }
    let mode = shelf.saved.lock().unwrap().series_mode.clone();
    let tile = if kind == "movie" {
        Tile { tmdb: Some(id), year: entry.year, ..Tile::new(&format!("tmdb:{id}"), "movie", &entry.title) }
    } else {
        Tile { tvdb: Some(id), year: entry.year, ..Tile::new(&format!("tvdb:{id}"), "series", &entry.title) }
    };
    let msg = if kind == "series" && mode == "season1" {
        crate::pick::get_picked(app, tile, "best", crate::pick::Pick { seasons: vec![1], episodes: vec![] }).await?
    } else {
        crate::media::get_it(app, tile, "best").await?
    };
    {
        let mut s = shelf.saved.lock().unwrap();
        if let Some(e) = s.entries.iter_mut().find(|e| e.kind == kind && e.id == id) {
            e.requested = true;
            e.stopped = false;
        }
    }
    shelf.store();
    let note = format!("{} ({why}). {msg}", entry.title);
    log(app, Level::Ok, &note);
    use tauri_plugin_notification::NotificationExt;
    let _ = app.notification().builder().title("Chillarr").body(format!("Getting {} on this PC ({why}).", entry.title)).show();
    Ok(())
}

/// Episodes being asked for right now (TVDB id, season, episode): a player
/// opens several reads at once, and each would otherwise ask Sonarr again
/// (a new show would be added twice, the second add failing).
static ASKING: Mutex<Vec<(i64, i64, i64)>> = Mutex::new(Vec::new());

/// Gets one episode of a shelf show on this PC and nothing else. A show new
/// to Sonarr is added with nothing wanted, then only this episode is
/// (`pick::get_picked`); the rest of a season pack that comes for it is
/// skipped when it plays (`watchnow::tune_pack`) and by `waiting::trim_packs`.
async fn request_episode(app: &AppHandle, id: i64, ep: (i64, i64), why: &str) -> Result<(), String> {
    let key = (id, ep.0, ep.1);
    let until = std::time::Instant::now() + Duration::from_secs(90);
    loop {
        {
            let mut a = ASKING.lock().unwrap_or_else(|e| e.into_inner());
            if !a.contains(&key) {
                a.push(key);
                break;
            }
        }
        if std::time::Instant::now() > until {
            return Err("Sonarr is still being asked for this episode. Try again in a minute.".into());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let r = ask_episode(app, id, ep, why).await;
    ASKING.lock().unwrap_or_else(|e| e.into_inner()).retain(|k| *k != key);
    r
}

async fn ask_episode(app: &AppHandle, id: i64, ep: (i64, i64), why: &str) -> Result<(), String> {
    let shelf = app.state::<Shelf>();
    let (entry, mode) = {
        let s = shelf.saved.lock().unwrap();
        (s.entries.iter().find(|e| e.kind == "series" && e.id == id).cloned(), s.series_mode.clone())
    };
    let Some(entry) = entry else { return Err("That show isn't on the shelf any more.".into()) };
    if episode_covered(&entry, &mode, ep) {
        return Ok(());
    }
    let tile = Tile { tvdb: Some(id), year: entry.year, ..Tile::new(&format!("tvdb:{id}"), "series", &entry.title) };
    let msg = crate::pick::get_picked(app, tile, "best", crate::pick::Pick { seasons: vec![], episodes: vec![ep] }).await?;
    {
        let mut s = shelf.saved.lock().unwrap();
        if let Some(e) = s.entries.iter_mut().find(|e| e.kind == "series" && e.id == id) {
            if !e.asked.contains(&ep) {
                e.asked.push(ep);
            }
            e.stopped_eps.retain(|x| *x != ep);
        }
    }
    shelf.store();
    let label = format!("{} S{:02}E{:02}", entry.title, ep.0, ep.1);
    log(app, Level::Ok, format!("{label} ({why}). {msg}"));
    use tauri_plugin_notification::NotificationExt;
    let _ = app.notification().builder().title("Chillarr").body(format!("Getting {label} on this PC ({why}).")).show();
    Ok(())
}

/// The shelf entry a Jellyfin item belongs to, from its file path.
fn entry_for_path(shelf: &Shelf, path: &str) -> Option<Entry> {
    let p = path.replace('/', "\\").to_ascii_lowercase();
    let s = shelf.saved.lock().unwrap();
    s.entries
        .iter()
        .find(|e| {
            let dir = win_path(&shelf.kind_dir(&e.kind).join(&e.folder)).to_ascii_lowercase();
            p.starts_with(&(dir + "\\"))
        })
        .cloned()
}

/// Tells people on the TV that it's coming, in the title's description.
async fn mark_coming(app: &AppHandle, item_id: &str, kind: &str) {
    let Some(j) = jf(app).filter(|j| j.user_id.is_some()) else { return };
    let Some(uid) = j.user_id.clone() else { return };
    let Ok(mut item) = j.get_json(&format!("/Users/{uid}/Items/{item_id}")).await else { return };
    let note = format!("⬇ On its way: downloading on your PC. It moves to {} when it's ready.", if kind == "movie" { "Movies" } else { "Series" });
    let overview = item["Overview"].as_str().unwrap_or_default().to_string();
    if overview.starts_with('⬇') {
        return;
    }
    item["Overview"] = json!(format!("{note}\n\n{overview}"));
    let mut locked: Vec<Value> = item["LockedFields"].as_array().cloned().unwrap_or_default();
    if !locked.iter().any(|v| v == "Overview") {
        locked.push(json!("Overview"));
    }
    item["LockedFields"] = json!(locked);
    let _ = j.post_json(&format!("/Items/{item_id}"), &item).await;
}

/// Favourites on the shelf, from every Jellyfin account, become downloads:
/// a heart on a show gets it as `series_mode` says (season 1, or all of
/// it); a heart on one episode gets that episode only.
async fn check_favourites(app: &AppHandle) {
    let Some(j) = jf(app).filter(|j| j.user_id.is_some()) else { return };
    let Some((movies_lib, series_lib)) = ensure_libraries(app).await else { return };
    let users = all_users(&j).await;
    let shelf = app.state::<Shelf>();
    let mode = shelf.saved.lock().unwrap().series_mode.clone();
    // (entry, Jellyfin item, the episode when the heart is on one)
    let mut asked: Vec<(Entry, String, Option<(i64, i64)>)> = Vec::new();
    for uid in users {
        for lib in [&movies_lib, &series_lib] {
            let q = format!("/Users/{uid}/Items?parentId={lib}&recursive=true&isFavorite=true&includeItemTypes=Movie,Series,Episode&fields=Path");
            let Ok(v) = j.get_json(&q).await else { continue };
            for it in v["Items"].as_array().into_iter().flatten() {
                let (Some(id), path) = (it["Id"].as_str(), it["Path"].as_str().unwrap_or_default()) else { continue };
                // A show's own path is its folder; add the separator so it matches.
                let probe = if it["Type"] == "Series" { format!("{path}\\x") } else { path.to_string() };
                let Some(e) = entry_for_path(&shelf, &probe) else { continue };
                let ep = if it["Type"] == "Episode" { episode_of_file(path) } else { None };
                if it["Type"] == "Episode" && ep.is_none() {
                    continue;
                }
                // A stopped episode's heart is cleared when it stops, so a
                // heart on it now is pressed again: it asks again.
                let wanted = match ep {
                    Some(ep) => !episode_covered(&e, &mode, ep),
                    None => !e.requested,
                };
                if wanted && !asked.iter().any(|(a, _, x)| a.kind == e.kind && a.id == e.id && *x == ep) {
                    asked.push((e, id.to_string(), ep));
                }
            }
        }
    }
    for (e, item_id, ep) in asked {
        let r = match ep {
            Some(ep) => request_episode(app, e.id, ep, "favourited in Jellyfin").await,
            None => request(app, &e.kind, e.id, "favourited in Jellyfin").await,
        };
        match r {
            Ok(()) => mark_coming(app, &item_id, &e.kind).await,
            Err(err) => log(app, Level::Red, format!("Couldn't get {}: {err}", e.title)),
        }
    }
}

/// The Jellyfin item for a shelf entry (its movie or show), if it's been scanned in.
async fn item_for(app: &AppHandle, e: &Entry) -> Option<String> {
    let j = jf(app).filter(|j| j.user_id.is_some())?;
    let uid = j.user_id.clone()?;
    let shelf = app.state::<Shelf>();
    let dir = win_path(&shelf.kind_dir(&e.kind).join(&e.folder));
    let (kind, name) = if e.kind == "movie" { ("Movie", MOVIES_LIB) } else { ("Series", SERIES_LIB) };
    // By library, not searchTerm: a search would reach the search proxy and
    // count as someone searching for the title.
    let libs = j.libraries().await.ok()?;
    let lib = libs.iter().find(|l| l["Name"] == name).and_then(|l| l["ItemId"].as_str())?;
    let v = j.get_json(&format!("/Users/{uid}/Items?parentId={lib}&recursive=true&includeItemTypes={kind}&fields=Path&enableImages=false&limit=10000")).await.ok()?;
    v["Items"].as_array()?.iter().find(|i| i["Path"].as_str().is_some_and(|p| p.to_ascii_lowercase().starts_with(&dir.to_ascii_lowercase()))).and_then(|i| i["Id"].as_str()).map(String::from)
}

fn urlenc(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// Takes the hearts off an item for every account (or the next check would
/// ask again) and the "on its way" note out of its description.
async fn clear_asked(j: &crate::jellyfin::Jf, users: &[String], item: &str) {
    for uid in users {
        let _ = j.delete_path(&format!("/Users/{uid}/FavoriteItems/{item}")).await;
    }
    if let Some(uid) = j.user_id.clone() {
        if let Ok(mut it) = j.get_json(&format!("/Users/{uid}/Items/{item}")).await {
            let overview = it["Overview"].as_str().unwrap_or_default().to_string();
            if overview.starts_with('⬇') {
                it["Overview"] = json!(overview.splitn(2, "\n\n").nth(1).unwrap_or_default());
                let locked: Vec<Value> = it["LockedFields"].as_array().cloned().unwrap_or_default().into_iter().filter(|v| v != "Overview").collect();
                it["LockedFields"] = json!(locked);
                let _ = j.post_json(&format!("/Items/{item}"), &it).await;
            }
        }
    }
}

/// Jellyfin can't read a placeholder's length (a .strm; measured on 12.1, an
/// .nfo <runtime> and the item editor are both ignored for them), so it takes
/// any stop on one as "watched to the end": a stop at 0 s marked it Played.
/// And a placeholder shares its watched mark with the real file, which
/// Jellyfin keys by TMDB/TVDB id: 28 Years Later, started and stopped on a TV
/// before it had downloaded, showed up as Completed once it arrived, and the
/// clean-up would have deleted it 3 days later.
///
/// So every 30 s a played placeholder is unmarked, for every Jellyfin user,
/// while its real file isn't here yet (nobody has finished a film that isn't
/// on the PC). Once Radarr or Sonarr has the file the mark is left alone.
/// On purpose, the one real watch this undoes (a film streamed to the end from
/// the shelf before Radarr imported it) is the safe way round: it shows as not
/// watched, where the wrong way round the clean-up would delete it.
async fn unmark_placeholder_plays(app: &AppHandle) {
    let Some(j) = jf(app).filter(|j| j.user_id.is_some()) else { return };
    let Some((movies_lib, series_lib)) = ensure_libraries(app).await else { return };
    let shelf = app.state::<Shelf>();
    for uid in all_users(&j).await {
        for lib in [&movies_lib, &series_lib] {
            let q = format!("/Users/{uid}/Items?parentId={lib}&recursive=true&filters=IsPlayed&includeItemTypes=Movie,Episode&fields=Path&enableImages=false");
            let Ok(v) = j.get_json(&q).await else { continue };
            for it in v["Items"].as_array().into_iter().flatten() {
                let (Some(id), Some(path)) = (it["Id"].as_str(), it["Path"].as_str()) else { continue };
                if !path.to_ascii_lowercase().ends_with(".strm") {
                    continue;
                }
                let Some(e) = entry_for_path(&shelf, path) else { continue };
                let ep = if it["Type"] == "Episode" { episode_of_file(path) } else { None };
                if it["Type"] == "Episode" && ep.is_none() {
                    continue;
                }
                // Unknown counts as here: a real watched mark is never taken away.
                if real_file_here(app, &e, ep).await != Some(false) {
                    continue;
                }
                if j.delete_path(&format!("/UserPlayedItems/{id}?userId={uid}")).await.is_ok() {
                    log(app, Level::Ok, format!("{} was started and stopped before it had downloaded, and Jellyfin marked it watched. Chillarr took that back.", label(&e, ep)));
                }
            }
        }
    }
}

/// Whether Radarr or Sonarr has the real file of a shelf title (or episode).
/// None when it can't be told.
async fn real_file_here(app: &AppHandle, e: &Entry, ep: Option<(i64, i64)>) -> Option<bool> {
    if e.kind == "movie" {
        let movies = arr(app, AppId::Radarr)?.list(&format!("movie?tmdbId={}", e.id)).await.ok()?;
        return Some(movies.iter().any(|m| m["hasFile"] == true));
    }
    let (season, number) = ep?;
    if e.arrived.contains(&(season, number)) {
        return Some(true);
    }
    let sonarr = arr(app, AppId::Sonarr)?;
    let series = sonarr.list(&format!("series?tvdbId={}", e.id)).await.ok()?;
    let Some(sid) = series.first().and_then(|s| s["id"].as_i64()) else { return Some(false) };
    let eps = sonarr.list(&format!("episode?seriesId={sid}&seasonNumber={season}")).await.ok()?;
    Some(eps.iter().any(|x| x["episodeNumber"].as_i64() == Some(number) && x["hasFile"] == true))
}

async fn all_users(j: &crate::jellyfin::Jf) -> Vec<String> {
    j.get_json("/Users").await.ok().and_then(|v| v.as_array().cloned()).unwrap_or_default().iter().filter_map(|u| u["Id"].as_str().map(String::from)).collect()
}

/// A title asked for, then stopped in Chillarr, Radarr or Sonarr: back to a
/// normal shelf title, its heart cleared and the "on its way" note off.
async fn unrequest(app: &AppHandle, e: &Entry) {
    if let (Some(j), Some(item)) = (jf(app).filter(|j| j.user_id.is_some()), item_for(app, e).await) {
        clear_asked(&j, &all_users(&j).await, &item).await;
    }
    let shelf = app.state::<Shelf>();
    if let Some(x) = shelf.saved.lock().unwrap().entries.iter_mut().find(|x| x.kind == e.kind && x.id == e.id) {
        x.requested = false;
        x.stopped = true;
    }
    shelf.store();
    log(app, Level::Ok, format!("{} was stopped, so it's a normal Discover title again.", e.title));
}

/// The Jellyfin items of a shelf show's episodes, with their (season, episode).
async fn episode_items(app: &AppHandle, e: &Entry) -> Vec<(String, (i64, i64))> {
    let Some(j) = jf(app).filter(|j| j.user_id.is_some()) else { return vec![] };
    let Some(uid) = j.user_id.clone() else { return vec![] };
    let shelf = app.state::<Shelf>();
    let dir = win_path(&shelf.kind_dir(&e.kind).join(&e.folder)).to_ascii_lowercase() + "\\";
    let Ok(libs) = j.libraries().await else { return vec![] };
    let Some(lib) = libs.iter().find(|l| l["Name"] == SERIES_LIB).and_then(|l| l["ItemId"].as_str()) else { return vec![] };
    let Ok(v) = j.get_json(&format!("/Users/{uid}/Items?parentId={lib}&recursive=true&includeItemTypes=Episode&fields=Path&enableImages=false&limit=10000")).await else { return vec![] };
    v["Items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|i| {
            let path = i["Path"].as_str()?;
            if !path.replace('/', "\\").to_ascii_lowercase().starts_with(&dir) {
                return None;
            }
            Some((i["Id"].as_str()?.to_string(), episode_of_file(path)?))
        })
        .collect()
}

/// Episodes asked for one by one, then stopped (unwanted in Sonarr, or the
/// show taken out of it): their reads no longer ask, their hearts come off.
async fn unrequest_episodes(app: &AppHandle, e: &Entry, eps: &[(i64, i64)]) {
    if eps.is_empty() {
        return;
    }
    if let Some(j) = jf(app).filter(|j| j.user_id.is_some()) {
        let users = all_users(&j).await;
        for (item, ep) in episode_items(app, e).await {
            if eps.contains(&ep) {
                clear_asked(&j, &users, &item).await;
            }
        }
    }
    let shelf = app.state::<Shelf>();
    if let Some(x) = shelf.saved.lock().unwrap().entries.iter_mut().find(|x| x.kind == e.kind && x.id == e.id) {
        x.asked.retain(|a| !eps.contains(a));
        for ep in eps {
            if !x.stopped_eps.contains(ep) {
                x.stopped_eps.push(*ep);
            }
        }
    }
    shelf.store();
    let names: Vec<String> = eps.iter().map(|(s, n)| format!("S{s:02}E{n:02}")).collect();
    log(app, Level::Ok, format!("{} {} was stopped, so it's a normal Discover episode again.", e.title, names.join(", ")));
}

/// What Sonarr's episode list says about a show got episode by episode.
#[derive(Debug, PartialEq, Default)]
struct EpisodeNews {
    /// Episodes with a placeholder (or asked for) that are in the library now: their placeholders go.
    arrived: Vec<(i64, i64)>,
    /// Asked-for episodes Sonarr no longer wants (or doesn't list): stopped.
    stopped: Vec<(i64, i64)>,
    /// Episodes taken off as arrived whose file is gone again: their placeholder comes back.
    back: Vec<(i64, i64)>,
}

fn episode_news(e: &Entry, sonarr_eps: &[Value]) -> EpisodeNews {
    let row = |ep: &(i64, i64)| sonarr_eps.iter().find(|x| x["seasonNumber"].as_i64() == Some(ep.0) && x["episodeNumber"].as_i64() == Some(ep.1));
    let has = |ep: &(i64, i64)| row(ep).is_some_and(|r| r["hasFile"] == true);
    // A show from before episodes were laid out has its one S01E01.
    let mut layout: Vec<(i64, i64)> = if e.episodes.is_empty() && e.arrived.is_empty() { vec![(1, 1)] } else { e.episodes.clone() };
    layout.extend(e.asked.iter().filter(|a| !layout.contains(a)).copied().collect::<Vec<_>>());
    EpisodeNews {
        arrived: layout.iter().filter(|ep| has(ep)).copied().collect(),
        stopped: e.asked.iter().filter(|ep| !has(ep) && !row(ep).is_some_and(|r| r["monitored"] == true)).copied().collect(),
        back: e.arrived.iter().filter(|ep| !has(ep)).copied().collect(),
    }
}

/// The entry after `news`: arrived episodes leave the placeholders (and
/// what's asked for), ones whose file went come back. Returns it and
/// whether no placeholder is left (the whole show is in the library).
fn apply_news(mut e: Entry, news: &EpisodeNews) -> (Entry, bool) {
    e.asked.retain(|a| !news.arrived.contains(a));
    e.arrived.retain(|a| !news.back.contains(a));
    for a in &news.arrived {
        if !e.arrived.contains(a) {
            e.arrived.push(*a);
        }
    }
    e.episodes.retain(|x| !news.arrived.contains(x));
    for b in &news.back {
        if !e.episodes.contains(b) {
            e.episodes.push(*b);
        }
    }
    e.episodes.sort();
    e.arrived.sort();
    let all_here = e.episodes.is_empty() && !e.arrived.is_empty();
    (e, all_here)
}

/// Shelf titles being played right now (kind, id): one that arrives meanwhile
/// stays on the shelf until the play ends, so Jellyfin doesn't lose the item
/// under the player. One episode downloads fast, so arriving mid-play is usual.
static PLAYING: Mutex<Vec<(String, i64)>> = Mutex::new(Vec::new());

struct Playing((String, i64));

impl Playing {
    fn start(kind: &str, id: i64) -> Playing {
        let key = (kind.to_string(), id);
        PLAYING.lock().unwrap_or_else(|e| e.into_inner()).push(key.clone());
        Playing(key)
    }
}

impl Drop for Playing {
    fn drop(&mut self) {
        let mut p = PLAYING.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = p.iter().position(|k| *k == self.0) {
            p.remove(i);
        }
    }
}

fn is_playing(e: &Entry) -> bool {
    PLAYING.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|(k, id)| *k == e.kind && *id == e.id)
}

/// Asked-for titles that have arrived leave the shelf; the real one shows in
/// Movies/Series. A show asked for episode by episode leaves once every one
/// of them has arrived (or been stopped, with at least one arrived).
async fn check_arrivals(app: &AppHandle) {
    let shelf = app.state::<Shelf>();
    let requested: Vec<Entry> = shelf.saved.lock().unwrap().entries.iter().filter(|e| e.kept()).cloned().collect();
    if requested.is_empty() {
        return;
    }
    let mut arrived_movies = HashSet::new();
    // TVDB id -> episode files Sonarr has.
    let mut show_files: HashMap<i64, i64> = HashMap::new();
    let (mut tracked_movies, mut tracked_shows) = (None::<HashSet<i64>>, None::<HashSet<i64>>);
    let mut sonarr_ids: HashMap<i64, i64> = HashMap::new();
    let sonarr = arr(app, AppId::Sonarr);
    if let Some(a) = arr(app, AppId::Radarr) {
        if let Ok(list) = a.list("movie").await {
            arrived_movies.extend(list.iter().filter(|m| m["hasFile"] == true).filter_map(|m| m["tmdbId"].as_i64()));
            // Stopped from My downloads leaves it in Radarr, switched off with no file: not wanted.
            tracked_movies = Some(list.iter().filter(|m| m["monitored"] == true || m["hasFile"] == true).filter_map(|m| m["tmdbId"].as_i64()).collect());
        }
    }
    if let Some(a) = &sonarr {
        if let Ok(list) = a.list("series").await {
            show_files = list.iter().filter_map(|s| Some((s["tvdbId"].as_i64()?, s["statistics"]["episodeFileCount"].as_i64().unwrap_or(0)))).collect();
            sonarr_ids = list.iter().filter_map(|s| Some((s["tvdbId"].as_i64()?, s["id"].as_i64()?))).collect();
            tracked_shows = Some(
                list.iter()
                    .filter(|s| s["monitored"] == true || s["statistics"]["episodeFileCount"].as_i64().unwrap_or(0) > 0)
                    .filter_map(|s| s["tvdbId"].as_i64())
                    .collect(),
            );
        }
    }
    let mut done: Vec<Entry> = Vec::new();
    let mut rescan = false;
    for e in &requested {
        // Only when Radarr/Sonarr answered: a missing title means it was stopped.
        if e.requested {
            let gone = if e.kind == "movie" { tracked_movies.as_ref().is_some_and(|t| !t.contains(&e.id)) } else { tracked_shows.as_ref().is_some_and(|t| !t.contains(&e.id)) };
            // A show got episode by episode before its heart: the heart's
            // part has arrived once there's more than those episodes.
            let arrived = if e.kind == "movie" { arrived_movies.contains(&e.id) } else { show_files.get(&e.id).is_some_and(|n| *n > e.arrived.len() as i64) };
            if gone {
                unrequest(app, e).await;
            } else if arrived {
                done.push(e.clone());
            }
            continue;
        }
        // Episodes one by one: judged on Sonarr's own episode list, read only
        // when Sonarr answered (a show it doesn't have: every one stopped).
        let (Some(a), true) = (&sonarr, tracked_shows.is_some()) else { continue };
        let eps = match sonarr_ids.get(&e.id) {
            Some(sid) => match a.list(&format!("episode?seriesId={sid}")).await {
                Ok(v) => v,
                Err(_) => continue,
            },
            None => vec![],
        };
        let news = episode_news(e, &eps);
        unrequest_episodes(app, e, &news.stopped).await;
        if news.arrived.is_empty() && news.back.is_empty() {
            continue;
        }
        // Not under a player: the TV would lose the episode it's on.
        if is_playing(e) {
            continue;
        }
        let (secret, base) = {
            let s = shelf.saved.lock().unwrap();
            (s.secret.clone(), s.base.clone())
        };
        let Some(now) = shelf.saved.lock().unwrap().entries.iter().find(|x| x.kind == e.kind && x.id == e.id).cloned() else { continue };
        let (after, all_here) = apply_news(now, &news);
        if all_here {
            done.push(after);
            continue;
        }
        shelf.remove_episodes(&after, &news.arrived);
        if !base.is_empty() {
            let _ = shelf.write_files(&after, &news.back, &base, &secret);
        }
        if let Some(x) = shelf.saved.lock().unwrap().entries.iter_mut().find(|x| x.kind == e.kind && x.id == e.id) {
            *x = after.clone();
        }
        shelf.store();
        let names = |v: &[(i64, i64)]| v.iter().map(|(s, n)| format!("S{s:02}E{n:02}")).collect::<Vec<_>>().join(", ");
        if !news.arrived.is_empty() {
            log(app, Level::Ok, format!("{} {} is in your library now, so its Discover placeholder is gone. The rest of the show stays on the shelf.", e.title, names(&news.arrived)));
        }
        if !news.back.is_empty() {
            log(app, Level::Ok, format!("{} {} isn't in your library any more, so it's back on the Discover shelf.", e.title, names(&news.back)));
        }
        rescan = true;
    }
    done.retain(|e| !is_playing(e));
    if done.is_empty() {
        if rescan {
            if let Some(j) = jf(app).filter(|j| j.user_id.is_some()) {
                let _ = j.scan().await;
            }
        }
        return;
    }
    for e in &done {
        shelf.remove(e);
        log(app, Level::Ok, format!("{} is ready. It's in your library now, and off the Discover shelf.", e.title));
    }
    shelf.saved.lock().unwrap().entries.retain(|e| !done.iter().any(|d| d.kind == e.kind && d.id == e.id));
    shelf.store();
    if let Some(j) = jf(app).filter(|j| j.user_id.is_some()) {
        let _ = j.scan().await;
    }
}

/// Turns the shelf on or off. Off takes the libraries and placeholders away.
pub async fn set(app: &AppHandle, on: bool, series_mode: Option<String>) -> Status {
    let shelf = app.state::<Shelf>();
    {
        let mut s = shelf.saved.lock().unwrap();
        s.on = on;
        if let Some(m) = series_mode.filter(|m| m == "season1" || m == "all") {
            s.series_mode = m;
        }
        if !on {
            s.entries.retain(|e| e.kept());
        }
    }
    shelf.store();
    if on {
        refresh(app).await;
    } else {
        remove_libraries(app).await;
        let _ = std::fs::remove_dir_all(&shelf.dir);
        log(app, Level::Ok, "Took the Discover shelf out of Jellyfin.");
    }
    shelf.status()
}

/// Runs the shelf: the play server, an hourly refresh, a favourites check every 30s.
pub fn spawn(app: AppHandle) {
    start_server(app.clone());
    tauri::async_runtime::spawn(async move {
        // Let Jellyfin and the catalog come up first.
        tokio::time::sleep(Duration::from_secs(60)).await;
        let mut last_refresh: Option<std::time::Instant> = None;
        let mut tick = 0u64;
        loop {
            let on = app.state::<Shelf>().saved.lock().unwrap().on;
            if on && jf(&app).is_some_and(|j| j.user_id.is_some()) {
                if last_refresh.is_none_or(|t| t.elapsed() >= REFRESH_EVERY) {
                    apply_brand(&app).await;
                    apply_auto_subtitles(&app, "eng").await;
                    refresh(&app).await;
                    last_refresh = Some(std::time::Instant::now());
                }
                check_arrivals(&app).await;
                check_favourites(&app).await;
                unmark_placeholder_plays(&app).await;
            }
            tick += 1;
            tokio::time::sleep(FAVOURITE_EVERY).await;
        }
    });
}

// ---------- Playing while it downloads ----------

fn start_server(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut found = None;
        for p in PORTS {
            if let Ok(l) = tokio::net::TcpListener::bind(("0.0.0.0", p)).await {
                found = Some((l, p));
                break;
            }
        }
        let Some((listener, port)) = found else {
            log(&app, Level::Amber, "The Discover shelf couldn't open a port for playing (8690-8692 are taken). Favourites still work.");
            return;
        };
        *app.state::<Shelf>().port.lock().unwrap() = Some(port);
        // Caps connections still sending their request head. A connection lets
        // go of its slot once the head is in, so long plays never starve it.
        let slots = std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_HEADS));
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { continue };
            let Ok(slot) = slots.clone().try_acquire_owned() else { continue };
            let h = app.clone();
            tauri::async_runtime::spawn(async move {
                let req = read_head(&mut sock).await;
                drop(slot);
                if let Ok(Some(req)) = req {
                    let _ = serve(h, sock, req).await;
                }
            });
        }
    });
}

pub(crate) struct Req {
    pub(crate) method: String,
    pub(crate) path: String,
    pub(crate) range: Option<(u64, Option<u64>)>,
}

/// Reads a request head; the whole head must arrive within `HEAD_READ`, so a
/// client sending a byte at a time can't hold the connection open.
pub(crate) async fn read_head(sock: &mut tokio::net::TcpStream) -> std::io::Result<Option<Req>> {
    let read = async {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 2048];
        loop {
            let n = sock.read(&mut chunk).await?;
            if n == 0 {
                return Ok::<_, std::io::Error>(None);
            }
            buf.extend_from_slice(&chunk[..n]);
            if buf.len() > 16 * 1024 {
                return Ok(None);
            }
            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                return Ok(Some(buf));
            }
        }
    };
    let Some(buf) = tokio::time::timeout(HEAD_READ, read).await.map_err(|_| std::io::ErrorKind::TimedOut)?? else { return Ok(None) };
    let head = String::from_utf8_lossy(&buf).to_string();
    let mut first = head.lines().next().unwrap_or_default().split_whitespace();
    let (method, path) = (first.next().unwrap_or_default().to_string(), first.next().unwrap_or_default().to_string());
    let range = head.lines().find_map(|l| {
        let v = l.strip_prefix("Range:").or_else(|| l.strip_prefix("range:"))?.trim().strip_prefix("bytes=")?;
        let (a, b) = v.split_once('-')?;
        Some((a.trim().parse().ok()?, b.trim().parse().ok()))
    });
    Ok(Some(Req { method, path, range }))
}

pub(crate) async fn plain(sock: &mut tokio::net::TcpStream, status: &str, text: &str) -> std::io::Result<()> {
    let head = format!("HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", text.len());
    sock.write_all(head.as_bytes()).await?;
    sock.write_all(text.as_bytes()).await?;
    sock.shutdown().await
}

/// Where a requested title's video sits in qBittorrent, once it's downloading.
pub(crate) struct Source {
    hash: String,
    file: PathBuf,
    /// Where the video starts inside the torrent, and its size.
    offset: u64,
    size: u64,
    piece: u64,
    q: crate::qbit::Qbit,
}

/// Which file of the torrent is the one to play.
pub(crate) enum Want {
    /// The biggest video.
    Movie,
    /// This episode's file (season, episode), out of a pack or on its own.
    Episode(i64, i64),
    /// The first episode by name (the shelf's season packs).
    FirstOfPack,
}

pub(crate) fn choose(files: &[Value], want: &Want) -> Option<Value> {
    match want {
        Want::Movie => crate::qbit::pick_video(files, false),
        Want::FirstOfPack => crate::qbit::pick_video(files, true),
        Want::Episode(s, e) => crate::qbit::pick_episode(files, *s, *e),
    }
}

/// Every torrent Radarr/Sonarr have for this title, so an episode can be
/// looked for in each (a show can have several downloads at once).
fn torrent_hashes(records: &[Value], field: &str, id: i64) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for r in records.iter().filter(|r| r[field].as_i64() == Some(id) && r["protocol"].as_str().is_none_or(|p| p == "torrent")) {
        if let Some(h) = r["downloadId"].as_str().map(str::to_ascii_lowercase) {
            if !out.contains(&h) {
                out.push(h);
            }
        }
    }
    out
}

/// Whether Sonarr grabbed this torrent for this one episode and nothing else.
fn only_for_episode(records: &[Value], hash: &str, episode_id: i64) -> bool {
    let mine: Vec<&Value> = records.iter().filter(|r| r["downloadId"].as_str().is_some_and(|d| d.eq_ignore_ascii_case(hash))).collect();
    !mine.is_empty() && mine.iter().all(|r| r["episodeId"].as_i64() == Some(episode_id))
}

/// The torrent's one video (samples aside), when it has exactly one.
fn lone_video(files: &[Value]) -> Option<Value> {
    let vids: Vec<&Value> = files
        .iter()
        .filter(|f| {
            let n = f["name"].as_str().unwrap_or_default().to_ascii_lowercase();
            crate::qbit::VIDEO.iter().any(|e| n.ends_with(e)) && !n.contains("sample")
        })
        .collect();
    match vids.as_slice() {
        [one] => Some((*one).clone()),
        _ => None,
    }
}

/// How to find the torrent, and whether it may be changed.
pub(crate) struct Lookup {
    /// This torrent, instead of looking in Radarr's/Sonarr's queue.
    pub(crate) hash: Option<String>,
    /// May switch on in-order downloading and, for an episode, skip the rest
    /// of a pack. Only for torrents that are ours to change.
    pub(crate) tune: bool,
}

/// The shelf's own: the movie, or the first episode of a show's pack.
async fn find_source(app: &AppHandle, kind: &str, id: i64, deadline: std::time::Instant) -> Option<Source> {
    let want = if kind == "movie" { Want::Movie } else { Want::FirstOfPack };
    find_source_for(app, kind, id, want, Lookup { hash: None, tune: true }, deadline).await
}

/// Where a title's video sits in qBittorrent, waiting until `deadline` for it
/// to start downloading. `id` is the TMDB id for a movie, the TVDB id for a show.
pub(crate) async fn find_source_for(app: &AppHandle, kind: &str, id: i64, want: Want, how: Lookup, deadline: std::time::Instant) -> Option<Source> {
    let owner = if kind == "movie" { AppId::Radarr } else { AppId::Sonarr };
    let a = arr(app, owner);
    if a.is_none() && how.hash.is_none() {
        return None;
    }
    let q = crate::waiting::qbit(app)?;
    let (list, field, idk) = if kind == "movie" { ("movie", "movieId", "tmdbId") } else { ("series", "seriesId", "tvdbId") };
    let episode = matches!(want, Want::Episode(..));
    // Sonarr's id for the episode, read once: a torrent grabbed for it alone
    // may name its file only by absolute number (anime: "Show - 05.mkv").
    let mut episode_id: Option<i64> = None;
    let mut single: Vec<String> = Vec::new();
    while std::time::Instant::now() < deadline {
        let hashes = match (&how.hash, &a) {
            (Some(h), _) => vec![h.to_ascii_lowercase()],
            (None, Some(a)) => {
                let arr_id = a.list(list).await.unwrap_or_default().iter().find(|x| x[idk].as_i64() == Some(id)).and_then(|x| x["id"].as_i64());
                match arr_id {
                    Some(arr_id) => {
                        let path = if kind == "movie" { "queue?includeMovie=true&pageSize=2000" } else { "queue?includeSeries=true&pageSize=2000" };
                        let recs = a.get_one(path).await.ok().and_then(|v| v["records"].as_array().cloned()).unwrap_or_default();
                        // The shelf takes the title's one torrent, as it always has; an
                        // episode is looked for in each of the show's torrents.
                        if episode {
                            if let (None, Want::Episode(se, n)) = (episode_id, &want) {
                                episode_id = a.list(&format!("episode?seriesId={arr_id}")).await.unwrap_or_default().iter().find(|x| x["seasonNumber"].as_i64() == Some(*se) && x["episodeNumber"].as_i64() == Some(*n)).and_then(|x| x["id"].as_i64());
                            }
                            let hashes = torrent_hashes(&recs, field, arr_id);
                            single = hashes.iter().filter(|h| episode_id.is_some_and(|id| only_for_episode(&recs, h, id))).cloned().collect();
                            hashes
                        } else {
                            match crate::qbit::torrent_hash(&recs, field, arr_id) {
                                crate::qbit::Pick::Hash(h) => vec![h],
                                _ => vec![],
                            }
                        }
                    }
                    None => vec![],
                }
            }
            (None, None) => vec![],
        };
        for hash in hashes {
            let Ok(Some(info)) = q.torrent(&hash).await else { continue };
            if !episode && how.tune {
                let _ = q.stream_mode(&hash, &info).await;
            }
            let files = q.files(&hash).await.unwrap_or_default();
            let f = choose(&files, &want).or_else(|| if single.contains(&hash) { lone_video(&files) } else { None });
            let Some(f) = f else { continue };
            if episode && how.tune {
                let _ = q.stream_mode(&hash, &info).await;
                // Only this episode's file of a pack downloads (and what others still want), so it starts fast.
                crate::watchnow::tune_pack(app, &q, &hash, id, &files, &f).await;
            }
            let name = f["name"].as_str().unwrap_or_default();
            let idx = f["index"].as_u64();
            let mut offset = 0u64;
            for other in &files {
                let same = match idx { Some(x) => other["index"].as_u64() == Some(x), None => other["name"] == f["name"] };
                if same {
                    break;
                }
                offset += other["size"].as_u64().unwrap_or(0);
            }
            let complete = f["progress"].as_f64().unwrap_or(0.0) >= 1.0;
            let paths = crate::qbit::candidate_paths(info["save_path"].as_str().unwrap_or_default(), info["download_path"].as_str().unwrap_or_default(), name, complete);
            let piece = q.piece_size(&hash).await.unwrap_or(0) as u64;
            if let (Some(file), true) = (paths.into_iter().find(|p| p.exists()), piece > 0) {
                return Some(Source { hash, file, offset, size: f["size"].as_u64().unwrap_or(0), piece, q });
            }
            // The file isn't on disk yet: try again after the pause.
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    None
}

impl Source {
    /// Waits until the bytes [from, to] of the video are downloaded.
    pub(crate) async fn wait_for(&self, from: u64, to: u64, limit: Duration) -> bool {
        let first = ((self.offset + from) / self.piece) as usize;
        let last = ((self.offset + to) / self.piece) as usize;
        let until = std::time::Instant::now() + limit;
        loop {
            if let Ok(states) = self.q.piece_states(&self.hash).await {
                if first <= last && last < states.len() && states[first..=last].iter().all(|s| *s == 2) {
                    return true;
                }
                // Finished torrents may report nothing piece by piece.
                if states.is_empty() {
                    if let Ok(Some(t)) = self.q.torrent(&self.hash).await {
                        if t["progress"].as_f64().unwrap_or(0.0) >= 1.0 {
                            return true;
                        }
                    }
                }
            }
            if std::time::Instant::now() > until {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(700)).await;
        }
    }
}

/// A stopped title is wanted again when Radarr/Sonarr are after it once more
/// (Download in Chillarr) or whoever is reading is really playing it. Asking
/// Jellyfin's sessions doesn't work here: a player reports itself only once
/// bytes arrive, and a held read sends none. So it goes by the reader itself.
/// `ep` is the episode read, for a show's episode placeholder.
async fn wanted_again(app: &AppHandle, e: &Entry, ep: Option<(i64, i64)>, peer: Option<std::net::SocketAddr>, local: Option<std::net::SocketAddr>) -> bool {
    let (which, list, key) = if e.kind == "movie" { (AppId::Radarr, "movie", "tmdbId") } else { (AppId::Sonarr, "series", "tvdbId") };
    if let Some(a) = arr(app, which) {
        let item = a.list(list).await.unwrap_or_default().into_iter().find(|x| x[key].as_i64() == Some(e.id) && x["monitored"] == true);
        let wanted = match (item, ep) {
            (None, _) => false,
            (Some(_), None) => true,
            // One episode: wanted when Sonarr wants that episode again.
            (Some(show), Some((s, n))) => match show["id"].as_i64() {
                Some(sid) => a.list(&format!("episode?seriesId={sid}")).await.unwrap_or_default().iter().any(|x| x["seasonNumber"].as_i64() == Some(s) && x["episodeNumber"].as_i64() == Some(n) && x["monitored"] == true),
                None => false,
            },
        };
        if wanted {
            return true;
        }
    }
    let name = label(e, ep);
    match reader(app, peer, local).await {
        Reader::Remote => true,
        Reader::Local(prog, cmd, play) => {
            if play {
                log(app, Level::Ok, format!("{name} was played by {prog} after it was stopped, so it's asked for again."));
            } else {
                log(app, Level::Ok, format!("{name} was read by Jellyfin in the background after it was stopped, ignored: {prog} {}", hide_secret(app, &cmd)));
            }
            play
        }
        Reader::Unknown => {
            log(app, Level::Amber, format!("{name} was read after it was stopped, and Chillarr couldn't tell by what, so it stays stopped."));
            false
        }
    }
}

/// "Show S01E02", or the title.
fn label(e: &Entry, ep: Option<(i64, i64)>) -> String {
    match ep {
        Some((s, n)) => format!("{} S{s:02}E{n:02}", e.title),
        None => e.title.clone(),
    }
}

/// The whole command line, so a wrong call can be seen in the log, minus
/// the play address's secret (logs get shared).
fn hide_secret(app: &AppHandle, cmd: &str) -> String {
    let secret = app.state::<Shelf>().saved.lock().unwrap().secret.clone();
    if secret.is_empty() { cmd.to_string() } else { cmd.replace(&secret, "***") }
}

/// Who is reading a placeholder.
enum Reader {
    /// Another device: a TV or phone playing it directly.
    Remote,
    /// A program on this PC: (exe name, command line, playing it).
    Local(String, String, bool),
    Unknown,
}

async fn reader(app: &AppHandle, peer: Option<std::net::SocketAddr>, local: Option<std::net::SocketAddr>) -> Reader {
    let (Some(peer), Some(local)) = (peer, local) else { return Reader::Unknown };
    if !peer.ip().is_loopback() && peer.ip() != local.ip() {
        return Reader::Remote;
    }
    match tauri::async_runtime::spawn_blocking(move || reader_of(peer.port(), local.port())).await.ok().flatten() {
        Some((name, cmd)) => {
            let play = is_playback(&name, &cmd, transcode_dir(app).await.as_deref());
            Reader::Local(name, cmd, play)
        }
        None => Reader::Unknown,
    }
}

/// Whether a read of an episode not asked for yet asks for it. A show's
/// episodes are many placeholders now, so a background job reading through
/// them (Intro Skipper's analysis is ffmpeg, never into the transcode
/// folder) must not ask for each. Jellyfin's ffprobe does ask: it looks at
/// a title just before it plays. A reader that can't be told keeps the old
/// rule (a read asks), so a real play is never refused.
fn first_read_asks(name: &str, cmd: &str, transcode_dir: Option<&str>) -> bool {
    name.to_ascii_lowercase().starts_with("ffprobe") || is_playback(name, cmd, transcode_dir)
}

/// Episodes whose background reads were already logged, so a job reading a
/// whole season says so once.
static IGNORED: Mutex<Vec<(i64, i64, i64)>> = Mutex::new(Vec::new());

/// The program on this PC holding the other end of a connection to the play
/// server: (exe name, command line).
fn reader_of(peer_port: u16, our_port: u16) -> Option<(String, String)> {
    use std::os::windows::process::CommandExt;
    let script = format!(
        "$c = Get-NetTCPConnection -LocalPort {peer_port} -RemotePort {our_port} -ErrorAction SilentlyContinue | Select-Object -First 1; \
         if ($c) {{ $p = Get-CimInstance Win32_Process -Filter \"ProcessId=$($c.OwningProcess)\"; $p.Name; $p.CommandLine }}"
    );
    let out = std::process::Command::new(crate::procs::powershell())
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .creation_flags(0x0800_0000)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let mut lines = text.lines();
    let name = lines.next()?.trim().to_string();
    (!name.is_empty()).then(|| (name, lines.collect::<Vec<_>>().join(" ")))
}

/// Jellyfin's transcode folder (Dashboard > Playback), if it was moved from
/// the default cache\transcodes.
async fn transcode_dir(app: &AppHandle) -> Option<String> {
    let j = jf(app).filter(|j| j.user_id.is_some())?;
    let v = j.get_json("/System/Configuration/encoding").await.ok()?;
    v["TranscodingTempPath"].as_str().map(str::trim).filter(|p| !p.is_empty()).map(String::from)
}

/// Whether a reader on this PC is playing the title. Jellyfin's playback
/// ffmpeg (transcode or remux) writes into its transcode folder; subtitle
/// extraction (data\subtitles), trickplay (data\trickplay) and Intro Skipper
/// write elsewhere, and ffprobe only looks. Jellyfin itself reads a .strm
/// when it streams it on to a player, so it counts as playing, as does
/// anything else (a browser, VLC).
fn is_playback(name: &str, cmd: &str, transcode_dir: Option<&str>) -> bool {
    let name = name.to_ascii_lowercase();
    let cmd = cmd.to_ascii_lowercase().replace('/', "\\");
    if name.starts_with("ffmpeg") {
        let moved = transcode_dir.map(|d| d.to_ascii_lowercase().replace('/', "\\").trim_end_matches('\\').to_string() + "\\");
        return cmd.contains("\\transcodes\\") || moved.is_some_and(|d| cmd.contains(&d));
    }
    !name.starts_with("ffprobe")
}

/// What a request to the play server is for.
#[derive(Debug, PartialEq)]
pub(crate) enum Route {
    /// `/d/<secret>/<movie|series>/<id>`: a Discover shelf title (`id` is 0
    /// when it isn't a number). `/d/<secret>/series/<tvdb>/<season>/<episode>`
    /// is one episode of a show; the address without one is what older
    /// versions wrote into a show's single S01E01 placeholder.
    Shelf { secret: String, kind: String, id: i64, ep: Option<(i64, i64)> },
    /// `/w/<secret>/<token>`: a Watch now title.
    WatchNow { secret: String, token: String },
    NotFound,
}

pub(crate) fn route(path: &str) -> Route {
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match parts.as_slice() {
        ["d", s, k, i] => Route::Shelf { secret: (*s).into(), kind: (*k).into(), id: i.parse::<i64>().unwrap_or(0), ep: None },
        ["d", s, "series", i, se, ep] => match (se.parse::<i64>(), ep.parse::<i64>()) {
            (Ok(se), Ok(ep)) if se > 0 && ep > 0 => Route::Shelf { secret: (*s).into(), kind: "series".into(), id: i.parse::<i64>().unwrap_or(0), ep: Some((se, ep)) },
            _ => Route::NotFound,
        },
        ["w", s, t] => Route::WatchNow { secret: (*s).into(), token: (*t).into() },
        _ => Route::NotFound,
    }
}

/// The episode a show's placeholder plays: the one its address names, or
/// S01E01 for an address from before episodes were laid out (that
/// placeholder was named S01E01). A movie has none.
fn episode_for(kind: &str, ep: Option<(i64, i64)>) -> Option<(i64, i64)> {
    (kind == "series").then(|| ep.unwrap_or((1, 1)))
}

/// Whether a play address carries the shelf's secret (compared in constant time).
fn secret_ok(app: &AppHandle, given: &str) -> bool {
    let want = app.state::<Shelf>().secret();
    !want.is_empty() && crate::remote::ct_eq(given.as_bytes(), want.as_bytes())
}

async fn serve(app: AppHandle, mut sock: tokio::net::TcpStream, req: Req) -> std::io::Result<()> {
    let (secret, kind, id, ep) = match route(&req.path) {
        Route::Shelf { secret, kind, id, ep } => (secret, kind, id, ep),
        Route::WatchNow { secret, token } => {
            if !secret_ok(&app, &secret) {
                return plain(&mut sock, "404 Not Found", "not here").await;
            }
            return crate::watchnow::serve(app, sock, req, &token).await;
        }
        Route::NotFound => return plain(&mut sock, "404 Not Found", "not here").await,
    };
    let ok_secret = secret_ok(&app, &secret);
    let kind = kind.as_str();
    if !ok_secret || !(kind == "movie" || kind == "series") || id <= 0 {
        return plain(&mut sock, "404 Not Found", "not here").await;
    }
    let ep = episode_for(kind, ep);
    let shelf = app.state::<Shelf>();
    let (entry, mode) = {
        let s = shelf.saved.lock().unwrap();
        (s.entries.iter().find(|e| e.kind == kind && e.id == id).cloned(), s.series_mode.clone())
    };
    let Some(entry) = entry else { return plain(&mut sock, "404 Not Found", "That title isn't on the shelf any more.").await };
    let _playing = Playing::start(kind, id);
    let Some(ep) = ep else {
        // A movie.
        // A stopped title is read by Jellyfin in the background for a while after
        // (subtitles, Intro Skipper): only a real playback asks for it again.
        if entry.stopped && !wanted_again(&app, &entry, None, sock.peer_addr().ok(), sock.local_addr().ok()).await {
            return plain(&mut sock, "404 Not Found", "Stopped in Chillarr. Press the heart to get it again.").await;
        }
        // Pressing play asks for it, whether or not it can be watched yet.
        if let Err(e) = request(&app, kind, id, "played in Jellyfin").await {
            return plain(&mut sock, "404 Not Found", &e).await;
        }
        let deadline = std::time::Instant::now() + HEAD_WAIT;
        let Some(src) = find_source(&app, kind, id, deadline).await else {
            return plain(&mut sock, "503 Service Unavailable", "No copy is downloading yet. It's been asked for and shows up in your library when it's ready.").await;
        };
        return stream_range(&mut sock, &req, &src, deadline).await;
    };
    // One episode of a show: that episode is asked for, and only it.
    // In the library already: its placeholder is gone, an old read of it never asks.
    if entry.arrived.contains(&ep) {
        return plain(&mut sock, "404 Not Found", "It's in your library now.").await;
    }
    if episode_stopped(&entry, ep) {
        if !wanted_again(&app, &entry, Some(ep), sock.peer_addr().ok(), sock.local_addr().ok()).await {
            return plain(&mut sock, "404 Not Found", "Stopped in Chillarr. Press the heart to get it again.").await;
        }
    } else if !episode_covered(&entry, &mode, ep) {
        if let Reader::Local(prog, cmd, _) = reader(&app, sock.peer_addr().ok(), sock.local_addr().ok()).await {
            if !first_read_asks(&prog, &cmd, transcode_dir(&app).await.as_deref()) {
                let first = {
                    let mut seen = IGNORED.lock().unwrap_or_else(|e| e.into_inner());
                    let k = (id, ep.0, ep.1);
                    !seen.contains(&k) && {
                        seen.push(k);
                        true
                    }
                };
                if first {
                    log(&app, Level::Ok, format!("{} was read by Jellyfin in the background, not played, so it wasn't asked for: {prog} {}", label(&entry, Some(ep)), hide_secret(&app, &cmd)));
                }
                return plain(&mut sock, "404 Not Found", "Not asked for. Play it or press its heart to get it.").await;
            }
        }
    }
    if let Err(e) = request_episode(&app, id, ep, "played in Jellyfin").await {
        return plain(&mut sock, "404 Not Found", &e).await;
    }
    let deadline = std::time::Instant::now() + HEAD_WAIT;
    let Some(src) = find_source_for(&app, "series", id, Want::Episode(ep.0, ep.1), Lookup { hash: None, tune: true }, deadline).await else {
        return plain(&mut sock, "503 Service Unavailable", "No copy of this episode is downloading yet. It's been asked for and shows up in your library when it's ready.").await;
    };
    stream_range(&mut sock, &req, &src, deadline).await
}

/// The first and last byte a request gets of a `size`-byte video, or None when
/// it can't be met (starts past the end, ends before it starts, empty file).
/// An end past the file is cut to the file, as HTTP allows.
fn byte_span(range: Option<(u64, Option<u64>)>, size: u64) -> Option<(u64, u64)> {
    let last = size.checked_sub(1)?;
    let (start, end) = match range {
        Some((a, b)) => (a, b.unwrap_or(last).min(last)),
        None => (0, last),
    };
    (start <= end).then_some((start, end))
}

/// Sends the asked-for bytes of the video as they download: waits (until
/// `deadline`, at least 20s) for the start, then goes piece by piece.
pub(crate) async fn stream_range(sock: &mut tokio::net::TcpStream, req: &Req, src: &Source, deadline: std::time::Instant) -> std::io::Result<()> {
    let size = src.size;
    let Some((start, end)) = byte_span(req.range, size) else {
        let head = format!("HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{size}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        sock.write_all(head.as_bytes()).await?;
        return sock.shutdown().await;
    };
    // The first bytes (the file's header) must be here before anything can play.
    let head_to = (start + 4 * 1024 * 1024).min(end);
    let left = deadline.saturating_duration_since(std::time::Instant::now()).max(Duration::from_secs(20));
    if !src.wait_for(start, head_to, left).await {
        return plain(sock, "503 Service Unavailable", "Still buffering. Try again in a minute.").await;
    }
    let status = if req.range.is_some() { "206 Partial Content" } else { "200 OK" };
    let ctype = match src.file.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("mp4") | Some("m4v") => "video/mp4",
        Some("webm") => "video/webm",
        Some("avi") => "video/x-msvideo",
        Some("ts") => "video/mp2t",
        _ => "video/x-matroska",
    };
    let mut head = format!("HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nAccept-Ranges: bytes\r\nContent-Length: {}\r\nConnection: close\r\n", end - start + 1);
    if req.range.is_some() {
        head.push_str(&format!("Content-Range: bytes {start}-{end}/{size}\r\n"));
    }
    head.push_str("\r\n");
    sock.write_all(head.as_bytes()).await?;
    if req.method == "HEAD" {
        return sock.shutdown().await;
    }
    let mut f = tokio::fs::File::open(&src.file).await?;
    let mut pos = start;
    let mut buf = vec![0u8; 256 * 1024];
    while pos <= end {
        // Send what's downloaded; wait (a while) for the next piece if it isn't.
        let piece_end = ((src.offset + pos) / src.piece + 1) * src.piece - src.offset - 1;
        let chunk_end = piece_end.min(end);
        if !src.wait_for(pos, chunk_end, PIECE_WAIT).await {
            break;
        }
        f.seek(std::io::SeekFrom::Start(pos)).await?;
        let mut left = chunk_end - pos + 1;
        while left > 0 {
            let want = left.min(buf.len() as u64) as usize;
            let n = f.read(&mut buf[..want]).await?;
            if n == 0 {
                break;
            }
            sock.write_all(&buf[..n]).await?;
            pos += n as u64;
            left -= n as u64;
        }
        if left > 0 {
            break;
        }
    }
    sock.shutdown().await
}

// ---------- Chillarr's look in every Jellyfin app ----------

/// Accent colour and a few touches for Jellyfin's own pages (web, and the
/// apps that show its web pages). Small on purpose: Jellyfin updates its
/// markup, and a missed selector only means a default colour. The full look
/// is jfweb.rs's skin; this is the part that still reaches a Jellyfin started
/// without Chillarr's web pages (a Windows service). Its text is also how
/// apply_brand finds and removes it, so it stays as it is.
const CSS: &str = "/* Chillarr */
:root { --chillarr: #ff6a3d; }
.button-submit, .raised.button-submit, .emby-button.button-submit { background: var(--chillarr) !important; color: #140a06 !important; }
.itemProgressBarForeground, .playedIndicator, .countIndicator, .mdl-slider-background-lower { background: var(--chillarr) !important; }
.emby-tab-button-active, .emby-tab-button-active .emby-button-foreground, .sectionTitle a:hover { color: var(--chillarr) !important; }
.cardBox:focus .cardScalable, .card:focus .cardBox { box-shadow: 0 0 0 3px var(--chillarr) !important; }
";
const DISCLAIMER: &str = "Chillarr: your movies and series, played from your own PC. New here? Open Discover Movies or Discover Series and press the heart on anything you want.";

/// Names the server "Chillarr" and styles Jellyfin's pages, or puts it back.
pub async fn apply_brand(app: &AppHandle) {
    let Some(j) = jf(app).filter(|j| j.user_id.is_some()) else { return };
    let shelf = app.state::<Shelf>();
    let (on, old_name) = {
        let s = shelf.saved.lock().unwrap();
        (s.brand, s.old_name.clone())
    };
    let Ok(mut cfg) = j.get_json("/System/Configuration").await else { return };
    let name = cfg["ServerName"].as_str().unwrap_or_default().to_string();
    let want = if on { "Chillarr".to_string() } else { old_name.clone().unwrap_or_else(|| std::env::var("COMPUTERNAME").unwrap_or_default()) };
    if name != want && !want.is_empty() {
        if on && old_name.is_none() && name != "Chillarr" {
            shelf.saved.lock().unwrap().old_name = Some(name.clone());
            shelf.store();
        }
        cfg["ServerName"] = json!(want);
        if j.post_json("/System/Configuration", &cfg).await.is_ok() {
            log(app, Level::Ok, if on { "Jellyfin apps now show this server as Chillarr.".to_string() } else { format!("Jellyfin's server name is back to {want}.") });
        }
    }
    let Ok(mut b) = j.get_json("/System/Configuration/branding").await else { return };
    let css = b["CustomCss"].as_str().unwrap_or_default().to_string();
    let ours = css.contains("/* Chillarr */");
    let (new_css, new_disc) = if on {
        (if ours { css.clone() } else { format!("{CSS}{css}") }, DISCLAIMER.to_string())
    } else {
        // Only what Chillarr added comes out; anything the user wrote stays.
        (css.replacen(CSS, "", 1), if b["LoginDisclaimer"].as_str() == Some(DISCLAIMER) { String::new() } else { b["LoginDisclaimer"].as_str().unwrap_or_default().to_string() })
    };
    if new_css != css || b["LoginDisclaimer"].as_str().unwrap_or_default() != new_disc {
        b["CustomCss"] = json!(new_css);
        b["LoginDisclaimer"] = json!(new_disc);
        let _ = j.post_json("/System/Configuration/branding", &b).await;
    }
}

/// Subtitles for people watching in Jellyfin apps: once subtitle downloads
/// are set up, each real library fetches missing ones in this language by
/// itself (skipped when the file already has them, or the audio is in it).
pub async fn apply_auto_subtitles(app: &AppHandle, lang: &str) {
    if !crate::subs::status(app).await.ready {
        return;
    }
    let Some(j) = jf(app).filter(|j| j.user_id.is_some()) else { return };
    let Ok(libs) = j.libraries().await else { return };
    for l in libs {
        let name = l["Name"].as_str().unwrap_or_default();
        if name == MOVIES_LIB || name == SERIES_LIB || crate::watchnow::is_library(name) {
            continue;
        }
        let Some(id) = l["ItemId"].as_str() else { continue };
        let mut opts = l["LibraryOptions"].clone();
        if opts["SubtitleDownloadLanguages"].as_array().is_some_and(|a| a.iter().any(|x| x == lang)) {
            continue;
        }
        opts["SubtitleDownloadLanguages"] = json!([lang]);
        opts["SkipSubtitlesIfEmbeddedSubtitlesPresent"] = json!(true);
        opts["SkipSubtitlesIfAudioTrackMatches"] = json!(true);
        opts["RequirePerfectSubtitleMatch"] = json!(false);
        opts["SaveSubtitlesWithMedia"] = json!(true);
        if j.post_json("/Library/VirtualFolders/LibraryOptions", &json!({ "Id": id, "LibraryOptions": opts })).await.is_ok() {
            log(app, Level::Ok, format!("Jellyfin's {name} library now fetches {lang} subtitles it's missing."));
        }
    }
}

/// Get it from the phone page: movies as usual; shows follow the shelf's
/// "season 1 or the whole show" choice, like a favourite on the TV.
pub async fn get_for_phone(app: &AppHandle, tile: Tile) -> Result<String, String> {
    let is_show = tile.kind == "series" || (tile.kind == "anime" && tile.format.as_deref() != Some("MOVIE"));
    let mode = app.state::<Shelf>().saved.lock().unwrap().series_mode.clone();
    if is_show && mode == "season1" {
        crate::pick::get_picked(app, tile, "best", crate::pick::Pick { seasons: vec![1], episodes: vec![] }).await
    } else {
        crate::media::get_it(app, tile, "best").await
    }
}

pub async fn set_brand(app: &AppHandle, on: bool) -> Status {
    app.state::<Shelf>().saved.lock().unwrap().brand = on;
    app.state::<Shelf>().store();
    apply_brand(app).await;
    app.state::<Shelf>().status()
}

pub fn status(app: &AppHandle) -> Status {
    app.state::<Shelf>().status()
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_a_reader_that_plays_asks_again() {
        // Transcode and subtitle lines are from Jellyfin 12.1's log, 2026-10-18;
        // the Intro Skipper one is its chromaprint shape (not seen in a log).
        let transcode = r#""C:\Program Files\Jellyfin\Server\ffmpeg.exe" -i "http://192.168.1.50:8690/d/x/series/275274" -f hls -y "C:\ProgramData\Jellyfin\Server\cache\transcodes\537717f2.m3u8""#;
        let subtitles = r#""C:\Program Files\Jellyfin\Server\ffmpeg.exe" -nostdin -y -i "http://192.168.1.50:8690/d/x/series/275274" -map 0:3 -an -vn -c:s copy "C:\ProgramData\Jellyfin\Server\data\subtitles\48\3.srt""#;
        let intro = r#""C:\Program Files\Jellyfin\Server\ffmpeg.exe" -i "http://192.168.1.50:8690/d/x/series/275274" -to 300 -ac 2 -f chromaprint -fp_format raw -"#;
        // Trickplay's shape (thumbnails into data\trickplay), not seen in a log either.
        let trickplay = r#""C:\Program Files\Jellyfin\Server\ffmpeg.exe" -i "http://192.168.1.50:8690/d/x/series/275274" -vf fps=0.1,scale=320:-1 -f image2 "C:\ProgramData\Jellyfin\Server\data\trickplay\48\320 - 10x10\%08d.jpg""#;
        let moved = r#""C:\Program Files\Jellyfin\Server\ffmpeg.exe" -i "http://192.168.1.50:8690/d/x/series/275274" -f hls -y "D:\JF Temp\537717f2.m3u8""#;
        assert!(is_playback("ffmpeg.exe", transcode, None));
        assert!(!is_playback("ffmpeg.exe", subtitles, None));
        assert!(!is_playback("ffmpeg.exe", intro, None));
        assert!(!is_playback("ffmpeg.exe", trickplay, None));
        assert!(!is_playback("ffmpeg.exe", moved, None));
        assert!(is_playback("ffmpeg.exe", moved, Some("D:/JF Temp/")));
        assert!(!is_playback("ffmpeg.exe", subtitles, Some("D:/JF Temp")));
        assert!(!is_playback("ffprobe.exe", "ffprobe -i http://x", None));
        assert!(is_playback("jellyfin.exe", "", None));
        assert!(is_playback("msedge.exe", "", None));
        assert!(is_playback("vlc.exe", "", None));
    }

    #[test]
    fn entries_saved_before_stop_tracking_load_as_not_stopped() {
        let e: Entry = serde_json::from_str(r#"{"kind":"series","id":275274,"title":"Rick and Morty","year":2013,"folder":"x","requested":true}"#).unwrap();
        assert!(!e.stopped);
    }

    use super::*;

    #[test]
    fn folder_names_are_safe_and_matchable() {
        assert_eq!(folder_for("movie", "Inception", Some(2010), Some(27205)), "Inception (2010) [tmdbid-27205]");
        assert_eq!(safe_name("Mission: Impossible / Fallout?"), "Mission Impossible Fallout");
        assert_eq!(safe_name("Dots..."), "Dots");
        assert_eq!(folder_for("series", "Breaking Bad", Some(2008), None), "Breaking Bad (2008)");
    }

    #[test]
    fn placeholder_options_are_shared_and_merged() {
        // The shelf's libraries are made with exactly these.
        let o = placeholder_options();
        for k in ["EnableChapterImageExtraction", "ExtractChapterImagesDuringLibraryScan", "SaveLocalMetadata", "EnableTrickplayImageExtraction", "EnableEmbeddedTitles"] {
            assert_eq!(o[k], false, "{k}");
        }
        assert_eq!(o["EnableRealtimeMonitor"], true);
        // Jellyfin's defaults get them laid over; other settings stay.
        let mut lib = serde_json::json!({ "EnableTrickplayImageExtraction": true, "PreferredMetadataLanguage": "en" });
        assert!(quiet_options(&mut lib));
        assert_eq!(lib["EnableTrickplayImageExtraction"], false);
        assert_eq!(lib["PreferredMetadataLanguage"], "en");
        assert!(!quiet_options(&mut lib), "already set: nothing to save");
        let mut missing = serde_json::Value::Null;
        assert!(quiet_options(&mut missing));
        assert_eq!(missing["SaveLocalMetadata"], false);
    }

    fn tile(id: i64, title: &str, year: i64) -> Tile {
        Tile { tmdb: Some(id), year: Some(year), ..Tile::new(&format!("tmdb:{id}"), "movie", title) }
    }

    #[test]
    fn a_shelf_movie_brings_the_rest_of_its_series() {
        let mut members = HashMap::new();
        members.insert(131292, vec![tile(1726, "Iron Man", 2008), tile(10138, "Iron Man 2", 2010), tile(68721, "Iron Man 3", 2013)]);
        members.insert(9, (0..20).map(|i| tile(1000 + i, "Bond", 1962 + i)).collect());
        // Iron Man is picked, Iron Man 3 is owned.
        let picked = vec![(1726, Some(131292)), (5, None), (1000, Some(9)), (1001, Some(9))];
        let skip: HashSet<i64> = [1726, 68721, 1000, 1001].into();
        let got = mates_to_add(&picked, &members, &skip);
        let ids: Vec<i64> = got.iter().filter_map(|t| t.tmdb).collect();
        assert_eq!(ids[0], 10138, "Iron Man 2 joins");
        assert!(!ids.contains(&68721) && !ids.contains(&1726));
        assert_eq!(ids.iter().filter(|i| (1000..1100).contains(*i)).count(), MATES_PER_COLLECTION, "a long series only adds a few, in release order");
        assert_eq!(ids[1], 1002);
        // Many collections stop at the overall bound.
        let many: HashMap<i64, Vec<Tile>> = (0..50).map(|c| (c, vec![tile(c * 10, "a", 2000), tile(c * 10 + 1, "b", 2001)])).collect();
        let picked: Vec<(i64, Option<i64>)> = (0..50).map(|c| (c * 10, Some(c))).collect();
        let skip: HashSet<i64> = (0..50).map(|c| c * 10).collect();
        assert_eq!(mates_to_add(&picked, &many, &skip).len(), MAX_MATES);
    }

    fn entry(id: i64, searched_at: Option<u64>, requested: bool) -> Entry {
        Entry { kind: "movie".into(), id, title: format!("m{id}"), year: None, folder: format!("m{id}"), requested, tmdb: Some(id), searched_at, ..Default::default() }
    }

    #[test]
    fn searched_titles_stay_a_month_and_the_latest_win() {
        let now = 100 * SEARCH_KEEP;
        let v = vec![entry(1, Some(now - 60), false), entry(2, Some(now - SEARCH_KEEP - 1), false), entry(3, None, false), entry(4, Some(now - 5), true)];
        let none = HashSet::new();
        let kept: Vec<i64> = kept_searched(&v, now, (&none, &none)).iter().map(|e| e.id).collect();
        assert_eq!(kept, vec![1], "expired, never searched and asked-for (kept anyway) aren't counted");
        // Replicas: searched, then downloaded. Owned now, so it leaves the shelf.
        let owned: HashSet<i64> = [1].into();
        assert!(kept_searched(&v, now, (&owned, &none)).is_empty());
        let lots: Vec<Entry> = (0..(MAX_SEARCHED as i64 + 10)).map(|i| entry(i, Some(now - 1000 + i as u64), false)).collect();
        let kept = kept_searched(&lots, now, (&none, &none));
        assert_eq!(kept.len(), MAX_SEARCHED);
        assert_eq!(kept[0].id, MAX_SEARCHED as i64 + 9, "most recently searched first");
        assert!(!kept.iter().any(|e| e.id < 10), "the oldest searches go first");
        let old: Entry = serde_json::from_str(r#"{"kind":"movie","id":1,"title":"A","year":null,"folder":"A"}"#).unwrap();
        assert_eq!(old.searched_at, None);
    }

    fn hit(id: i64, title: &str, votes: i64) -> Hit {
        Hit { kind: "movie", id, title: title.into(), year: Some(2010), tmdb: Some(id), votes }
    }

    #[test]
    fn a_search_adds_a_handful_of_close_matches() {
        // Radarr's lookup for "iron man" on 2026-10-19, votes from it.
        let found = vec![
            hit(1726, "Iron Man", 29001),
            hit(10138, "Iron Man 2", 22983),
            hit(68721, "Iron Man 3", 24007),
            hit(1564058, "Spidey and Iron Man: Avengers Team Up!", 6),
            hit(9313, "The Man in the Iron Mask", 4249),
            hit(69592, "Iron Man", 22),
            hit(13647, "The Invincible Iron Man", 274),
            hit(41428, "Tetsuo: The Iron Man", 741),
            hit(2, "Ironclad", 5000),
        ];
        let owned: HashSet<i64> = [10138].into();
        let got: Vec<i64> = close_hits("iron man", found.clone(), &owned, 4).iter().map(|h| h.id).collect();
        assert_eq!(got, vec![1726, 68721, 41428, 13647], "exact and starts-with first, best known first; owned and unknown left out");
        let all: Vec<i64> = close_hits("man iron", found, &HashSet::new(), 20).iter().map(|h| h.id).collect();
        assert!(all.contains(&9313) && all.contains(&1726), "every word, any order");
        assert!(!all.contains(&2), "Ironclad isn't Iron Man");
        let raw = serde_json::json!([{"tmdbId": 10138, "title": "Iron Man 2", "year": 2010}, {"tmdbId": 0, "title": "x"}, {"title": "no id"}]);
        assert_eq!(movie_hits(raw.as_array().unwrap()), vec![hit(10138, "Iron Man 2", 0)]);
        let shows = serde_json::json!([{"tvdbId": 81189, "title": "Breaking Bad", "year": 2008, "tmdbId": 1396}, {"tvdbId": 5, "title": "No tmdb", "year": 0}]);
        let s = show_hits(shows.as_array().unwrap());
        assert_eq!((s[0].kind, s[0].tmdb, s[1].tmdb, s[1].year), ("series", Some(1396), None, None));
    }

    #[test]
    fn jellyfin_has_settled_once_folders_match() {
        let paths = vec![r"c:\d\movies\iron man 2 (2010) [tmdbid-10138]\iron man 2 (2010) [tmdbid-10138].strm".to_string(), r"c:\d\series\breaking bad (2008)".to_string()];
        let added = vec![r"C:\d\Movies\Iron Man 2 (2010) [tmdbid-10138]".to_string(), r"C:\d\Series\Breaking Bad (2008)".to_string()];
        assert!(settled(&paths, &added, &[]));
        assert!(!settled(&paths, &[r"C:\d\Movies\Iron Man 2".to_string()], &[]), "a folder name that only starts the same isn't it");
        assert!(!settled(&paths, &[], &[r"C:\d\Series\Breaking Bad (2008)".to_string()]), "a removed one still there");
        assert!(settled(&paths, &[], &[r"C:\d\Movies\Dune (2021)".to_string()]));
    }

    #[test]
    fn route_w_is_parsed() {
        assert_eq!(route("/d/abc/movie/1"), Route::Shelf { secret: "abc".into(), kind: "movie".into(), id: 1, ep: None });
        assert_eq!(route("/d/abc/series/x"), Route::Shelf { secret: "abc".into(), kind: "series".into(), id: 0, ep: None });
        assert_eq!(route("/w/abc/tok"), Route::WatchNow { secret: "abc".into(), token: "tok".into() });
        assert_eq!(route("/w/abc"), Route::NotFound);
        assert_eq!(route("/w/abc/tok/more"), Route::NotFound);
        assert_eq!(route("/x/abc/tok"), Route::NotFound);
        assert_eq!(route("/"), Route::NotFound);
    }

    #[test]
    fn an_episode_address_names_its_episode_and_old_ones_mean_s01e01() {
        assert_eq!(route("/d/abc/series/81189/2/5"), Route::Shelf { secret: "abc".into(), kind: "series".into(), id: 81189, ep: Some((2, 5)) });
        assert_eq!(route("/d/abc/series/81189/0/1"), Route::NotFound, "no specials");
        assert_eq!(route("/d/abc/series/81189/x/1"), Route::NotFound);
        assert_eq!(route("/d/abc/movie/1/2/3"), Route::NotFound, "movies have no episodes");
        // What 0.7.0 wrote into a show's single "S01E01" placeholder.
        let Route::Shelf { kind, ep, .. } = route("/d/abc/series/275274") else { panic!() };
        assert_eq!(episode_for(&kind, ep), Some((1, 1)));
        assert_eq!(episode_for("series", Some((3, 4))), Some((3, 4)));
        assert_eq!(episode_for("movie", None), None);
    }

    fn sky(s: i64, e: i64, aired: Option<&str>) -> Value {
        serde_json::json!({ "seasonNumber": s, "episodeNumber": e, "airDate": aired })
    }

    #[test]
    fn which_episodes_get_placeholders() {
        let today = "2026-10-19";
        // Breaking Bad's shape: specials, and one unaired episode for the test.
        let mut eps = vec![sky(0, 1, Some("2009-02-17")), sky(1, 2, Some("2008-01-27")), sky(1, 1, Some("2008-01-20")), sky(2, 1, Some("2009-03-08")), sky(2, 2, Some("2099-01-01")), sky(2, 3, None)];
        assert_eq!(episodes_to_place(&eps, today), vec![(1, 1), (1, 2), (2, 1)], "regular, aired, in order");
        // A long show: whole seasons from the first while they fit.
        eps = (1..=4).flat_map(|s| (1..=22).map(move |e| sky(s, e, Some("2010-01-01")))).collect();
        let got = episodes_to_place(&eps, today);
        assert_eq!(got.len(), 44, "seasons 1 and 2 (44) fit in {MAX_EPISODES}, season 3 would not");
        assert_eq!((got[0], got[43]), ((1, 1), (2, 22)));
        // One huge first season (anime by TVDB): its first MAX_EPISODES.
        eps = (1..=200).map(|e| sky(1, e, Some("2001-01-01"))).chain([sky(2, 1, Some("2005-01-01"))]).collect();
        let got = episodes_to_place(&eps, today);
        assert_eq!((got.len(), got.last().copied()), (MAX_EPISODES, Some((1, MAX_EPISODES as i64))));
        assert!(episodes_to_place(&[sky(1, 1, Some("2099-01-01"))], today).is_empty(), "nothing aired: nothing to place");
    }

    #[test]
    fn placeholders_change_only_when_the_list_does() {
        let list = vec![(1, 1), (1, 2), (1, 3)];
        assert_eq!(layout_change(&list, &list), (vec![], vec![]), "unchanged: nothing written");
        assert_eq!(layout_change(&list, &[(1, 1), (1, 2), (1, 3), (1, 4)]), (vec![(1, 4)], vec![]), "a new episode aired");
        assert_eq!(layout_change(&list, &[(1, 2), (1, 3)]), (vec![], vec![(1, 1)]));
        // From 0.7.0 (one S01E01, an address with no episode): all of them, S01E01 rewritten.
        assert_eq!(layout_change(&[], &list), (list.clone(), vec![]));
        assert_eq!(layout_change(&[], &[(2, 1)]), (vec![(2, 1)], vec![(1, 1)]));
        let now = 10 * EPISODES_EVERY;
        let fresh = Entry { kind: "series".into(), episodes: list.clone(), episodes_at: Some(now - 60), ..Default::default() };
        assert!(!episodes_due(&fresh, now), "read an hour ago: not read again");
        assert!(episodes_due(&Entry { episodes_at: Some(now - EPISODES_EVERY), ..fresh.clone() }, now), "a day on");
        assert!(episodes_due(&Entry { episodes: vec![], ..fresh.clone() }, now), "never laid out");
        assert!(!episodes_due(&Entry { kind: "movie".into(), ..Default::default() }, now));
    }

    #[test]
    fn a_stopped_episode_stays_stopped_alone() {
        let e = Entry { kind: "series".into(), asked: vec![(1, 2)], stopped_eps: vec![(1, 1)], ..Default::default() };
        assert!(episode_stopped(&e, (1, 1)));
        assert!(!episode_stopped(&e, (1, 2)) && !episode_stopped(&e, (1, 3)), "only the stopped one");
        assert!(episode_covered(&e, "season1", (1, 2)));
        assert!(!episode_covered(&e, "season1", (1, 1)), "stopped: a real play asks again");
        assert!(!episode_covered(&e, "season1", (1, 3)), "never asked: a play asks for it alone");
        // The whole show stopped: every episode, except one asked for again since.
        let show = Entry { kind: "series".into(), requested: false, stopped: true, asked: vec![(2, 1)], ..Default::default() };
        assert!(episode_stopped(&show, (1, 1)) && !episode_stopped(&show, (2, 1)));
        // A heart on the show covers what it got.
        let hearted = Entry { kind: "series".into(), requested: true, ..Default::default() };
        assert!(episode_covered(&hearted, "season1", (1, 7)) && !episode_covered(&hearted, "season1", (2, 1)));
        assert!(episode_covered(&hearted, "all", (5, 1)));
        assert!(Entry { asked: vec![(1, 1)], ..Default::default() }.on_its_way());
        assert!(!Entry::default().on_its_way());
    }

    #[test]
    fn a_background_read_never_asks_for_an_episode() {
        let intro = r#""C:\Program Files\Jellyfin\Server\ffmpeg.exe" -i "http://192.168.1.50:8690/d/x/series/275274/1/2" -to 300 -ac 2 -f chromaprint -fp_format raw -"#;
        let transcode = r#""C:\Program Files\Jellyfin\Server\ffmpeg.exe" -i "http://192.168.1.50:8690/d/x/series/275274/1/2" -f hls -y "C:\ProgramData\Jellyfin\Server\cache\transcodes\5.m3u8""#;
        assert!(!first_read_asks("ffmpeg.exe", intro, None));
        assert!(first_read_asks("ffmpeg.exe", transcode, None));
        assert!(first_read_asks("ffprobe.exe", "ffprobe -i http://x", None), "Jellyfin looks just before it plays");
        assert!(first_read_asks("jellyfin.exe", "", None));
    }

    #[test]
    fn a_show_got_episode_by_episode_loses_only_what_arrived() {
        let row = |s: i64, e: i64, monitored: bool, has: bool| serde_json::json!({ "seasonNumber": s, "episodeNumber": e, "monitored": monitored, "hasFile": has });
        let e = Entry { kind: "series".into(), id: 7, episodes: vec![(1, 1), (1, 2), (1, 3), (1, 4)], asked: vec![(1, 1), (1, 2), (1, 3)], ..Default::default() };
        // S1E1 is here, S1E2 still coming, S1E3 stopped (unwanted), S1E4 downloaded outside the shelf.
        let sonarr = vec![row(1, 1, true, true), row(1, 2, true, false), row(1, 3, false, false), row(1, 4, false, true)];
        let news = episode_news(&e, &sonarr);
        assert_eq!(news, EpisodeNews { arrived: vec![(1, 1), (1, 4)], stopped: vec![(1, 3)], back: vec![] });
        let (after, all_here) = apply_news(e.clone(), &news);
        assert!(!all_here, "S1E2 and S1E3 are still placeholders: the show stays");
        assert_eq!(after.episodes, vec![(1, 2), (1, 3)]);
        assert_eq!(after.arrived, vec![(1, 1), (1, 4)]);
        assert_eq!(after.asked, vec![(1, 2), (1, 3)], "S1E3 is stopped by unrequest_episodes, not here");
        assert!(after.by_episode() && after.kept());
        // The arrived ones are never laid out again, and a read of one never asks.
        assert!(!after.episodes.contains(&(1, 1)));
        // A deleted file brings its placeholder back.
        let gone = vec![row(1, 1, false, false), row(1, 2, true, false), row(1, 3, false, false), row(1, 4, false, true)];
        let after2 = Entry { asked: vec![(1, 2)], ..after.clone() };
        let news = episode_news(&after2, &gone);
        assert_eq!(news.back, vec![(1, 1)]);
        let (back, _) = apply_news(after2, &news);
        assert_eq!((back.episodes, back.arrived), (vec![(1, 1), (1, 2), (1, 3)], vec![(1, 4)]));
        // Every placeholder in the library: the show leaves the shelf.
        let last = Entry { episodes: vec![(1, 2)], asked: vec![(1, 2)], arrived: vec![(1, 1)], ..Default::default() };
        let news = episode_news(&last, &[row(1, 1, true, true), row(1, 2, true, true)]);
        assert!(apply_news(last, &news).1);
        // The show taken out of Sonarr: asked ones stop, arrived ones come back.
        let news = episode_news(&after, &[]);
        assert_eq!((news.stopped, news.back), (vec![(1, 2), (1, 3)], vec![(1, 1), (1, 4)]));
        // From 0.7.0: one S01E01 placeholder, asked by its old address.
        let old = Entry { kind: "series".into(), asked: vec![(1, 1)], ..Default::default() };
        assert_eq!(episode_news(&old, &[row(1, 1, true, true)]).arrived, vec![(1, 1)]);
        assert!(!Entry { requested: true, arrived: vec![(1, 1)], ..Default::default() }.by_episode(), "hearted: the whole show's rule");
    }

    #[test]
    fn an_absolute_numbered_file_plays_when_the_torrent_is_that_episode_alone() {
        let recs = vec![serde_json::json!({ "downloadId": "AAA", "episodeId": 5 }), serde_json::json!({ "downloadId": "bbb", "episodeId": 5 }), serde_json::json!({ "downloadId": "bbb", "episodeId": 6 })];
        assert!(only_for_episode(&recs, "aaa", 5));
        assert!(!only_for_episode(&recs, "bbb", 5), "a pack for more episodes");
        assert!(!only_for_episode(&recs, "ccc", 5));
        let one = vec![serde_json::json!({"index": 0, "name": "[SubsPlease] Show - 05 (1080p).mkv"}), serde_json::json!({"index": 1, "name": "sample.mkv"}), serde_json::json!({"index": 2, "name": "x.nfo"})];
        assert_eq!(lone_video(&one).unwrap()["index"], 0);
        assert_eq!(choose(&one, &Want::Episode(1, 5)), None, "the name gives no season and episode");
        let two = vec![serde_json::json!({"name": "Show - 05.mkv"}), serde_json::json!({"name": "Show - 06.mkv"})];
        assert!(lone_video(&two).is_none());
    }

    #[test]
    fn the_shelf_series_library_takes_no_episode_pictures() {
        let mut o = serde_json::json!({ "TypeOptions": [{ "Type": "Series", "ImageFetchers": ["TheMovieDb"] }] });
        assert!(shelf_series_options(&mut o));
        let ep = o["TypeOptions"].as_array().unwrap().iter().find(|t| t["Type"] == "Episode").unwrap().clone();
        assert_eq!(ep["ImageFetchers"], serde_json::json!([]), "not TMDB, not the Screen Grabber");
        assert_eq!(o["TypeOptions"][0]["Type"], "Series", "other types left alone");
        assert_eq!(o["SaveLocalMetadata"], false);
        assert!(!shelf_series_options(&mut o), "already set: nothing to save");
        let mut grabbing = serde_json::json!({ "TypeOptions": [{ "Type": "Episode", "ImageFetchers": ["Screen Grabber"] }] });
        assert!(shelf_series_options(&mut grabbing));
        assert_eq!(grabbing["TypeOptions"][0]["ImageFetchers"], serde_json::json!([]));
    }

    #[test]
    fn entries_from_0_7_0_load_without_episodes() {
        let e: Entry = serde_json::from_str(r#"{"kind":"series","id":275274,"title":"Rick and Morty","year":2013,"folder":"x","requested":false,"stopped":true}"#).unwrap();
        assert!(e.episodes.is_empty() && e.asked.is_empty() && e.stopped_eps.is_empty() && e.episodes_at.is_none());
        assert!(episode_stopped(&e, (1, 1)), "stopped as a show: its old S01E01 too");
        assert_eq!(episode_of_file(r"C:\x\Series\Rick (2013)\Season 02\Rick S02E07.strm"), Some((2, 7)));
        assert_eq!(episode_of_file(r"C:\x\Series\S01E01 Show\Season 01\S01E01 Show S01E03.strm"), Some((1, 3)), "the last one is the file's");
        assert_eq!(episode_of_file("Movie.strm"), None);
    }

    #[test]
    fn want_chooses_the_file() {
        let files = vec![
            serde_json::json!({"index": 0, "name": "Pack/Show.S01E01.mkv", "size": 900}),
            serde_json::json!({"index": 1, "name": "Pack/Show.S01E02.mkv", "size": 1000}),
            serde_json::json!({"index": 2, "name": "Pack/sample.mkv", "size": 5000}),
        ];
        assert_eq!(choose(&files, &Want::FirstOfPack).unwrap()["index"], 0);
        assert_eq!(choose(&files, &Want::Movie).unwrap()["index"], 1);
        assert_eq!(choose(&files, &Want::Episode(1, 2)).unwrap()["index"], 1);
        assert!(choose(&files, &Want::Episode(1, 3)).is_none());
    }

    #[test]
    fn placeholders_point_at_the_secret_address() {
        let dir = std::env::temp_dir().join(format!("chillarr-shelf-test-{}", std::process::id()));
        let shelf = Shelf::load(dir.clone());
        let e = Entry { kind: "movie".into(), id: 27205, title: "Inception".into(), year: Some(2010), folder: "Inception (2010) [tmdbid-27205]".into(), tmdb: Some(27205), ..Default::default() };
        shelf.write(&e, "http://192.168.0.5:8690", "abc", false).unwrap();
        let text = std::fs::read_to_string(shelf.strm_path(&e)).unwrap();
        assert_eq!(text, "http://192.168.0.5:8690/d/abc/movie/27205");
        let path = win_path(&shelf.strm_path(&e));
        shelf.saved.lock().unwrap().entries.push(e.clone());
        assert_eq!(entry_for_path(&shelf, &path).map(|x| x.id), Some(27205));
        // A show: one placeholder per episode, each address naming its episode.
        let mut s = Entry { kind: "series".into(), id: 81189, title: "Breaking Bad".into(), year: Some(2008), folder: "Breaking Bad (2008) [tmdbid-1396]".into(), tmdb: Some(1396), episodes: vec![(1, 1), (1, 2), (2, 1)], ..Default::default() };
        shelf.write(&s, "http://192.168.0.5:8690", "abc", false).unwrap();
        let p = shelf.episode_path(&s, (2, 1));
        assert!(win_path(&p).ends_with("Series\\Breaking Bad (2008) [tmdbid-1396]\\Season 02\\Breaking Bad S02E01.strm"));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "http://192.168.0.5:8690/d/abc/series/81189/2/1");
        assert_eq!(route("/d/abc/series/81189/2/1"), Route::Shelf { secret: "abc".into(), kind: "series".into(), id: 81189, ep: Some((2, 1)) });
        // Only missing ones are written again; a dropped episode's file goes, and its empty season folder.
        std::fs::write(shelf.episode_path(&s, (1, 1)), "kept").unwrap();
        shelf.write(&s, "http://other:1", "abc", true).unwrap();
        assert_eq!(std::fs::read_to_string(shelf.episode_path(&s, (1, 1))).unwrap(), "kept");
        let (add, remove) = layout_change(&s.episodes, &[(1, 1), (1, 2)]);
        shelf.remove_episodes(&s, &remove);
        s.episodes = vec![(1, 1), (1, 2)];
        shelf.write_files(&s, &add, "http://192.168.0.5:8690", "abc").unwrap();
        assert!(!p.exists() && !p.parent().unwrap().exists());
        assert!(shelf.episode_path(&s, (1, 2)).exists());
        shelf.saved.lock().unwrap().entries.push(s.clone());
        assert_eq!(entry_for_path(&shelf, &win_path(&shelf.episode_path(&s, (1, 2)))).map(|x| x.id), Some(81189));
        // Not laid out yet (0.7.0's entry, list unreadable): its one S01E01 stays, at the new address.
        let old = Entry { id: 5, folder: "Old (2001)".into(), title: "Old".into(), episodes: vec![], ..s.clone() };
        shelf.write(&old, "http://192.168.0.5:8690", "abc", false).unwrap();
        assert_eq!(std::fs::read_to_string(shelf.episode_path(&old, (1, 1))).unwrap(), "http://192.168.0.5:8690/d/abc/series/5/1/1");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ranges_past_the_end_or_backwards_are_refused() {
        assert_eq!(byte_span(None, 100), Some((0, 99)));
        assert_eq!(byte_span(Some((10, None)), 100), Some((10, 99)));
        assert_eq!(byte_span(Some((10, Some(20))), 100), Some((10, 20)));
        assert_eq!(byte_span(Some((10, Some(500))), 100), Some((10, 99)), "an end past the file is cut");
        assert_eq!(byte_span(Some((99, Some(99))), 100), Some((99, 99)));
        assert_eq!(byte_span(Some((20, Some(10))), 100), None, "backwards");
        assert_eq!(byte_span(Some((100, None)), 100), None, "starts past the end");
        assert_eq!(byte_span(Some((u64::MAX, Some(5))), 100), None);
        assert_eq!(byte_span(None, 0), None, "empty file");
    }

    #[test]
    fn an_old_secret_is_replaced_once_and_placeholders_follow() {
        let mut saved = Saved { secret: "0123456789abcdef0123456789abcdef".into(), base: "http://1.2.3.4:8690".into(), ..Default::default() };
        let old = renew_secret(&mut saved).expect("a clock-made secret is replaced");
        assert_eq!(old, "0123456789abcdef0123456789abcdef");
        assert_eq!(saved.secret.len(), 32);
        assert!(saved.secret.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(saved.secret, old);
        assert!(saved.base.is_empty(), "the next refresh rewrites every placeholder");
        let kept = saved.secret.clone();
        assert!(renew_secret(&mut saved).is_none(), "only once");
        assert_eq!(saved.secret, kept);
        // Saved and loaded again, it stays.
        let mut again: Saved = serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        assert!(renew_secret(&mut again).is_none());

        let dir = tempfile::tempdir().unwrap();
        let movie = dir.path().join("Movies").join("A (2000)").join("A (2000).strm");
        let ep = dir.path().join("Series").join("B").join("Season 01").join("B S01E01.strm");
        let other = dir.path().join("Movies").join("note.txt");
        for (p, t) in [(&movie, format!("http://1.2.3.4:8690/d/{old}/movie/1")), (&ep, format!("http://localhost:8690/w/{old}/tok")), (&other, format!("/{old}/"))] {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, t).unwrap();
        }
        assert_eq!(swap_secret_in_placeholders(dir.path(), &old, &kept), 2);
        assert_eq!(std::fs::read_to_string(&movie).unwrap(), format!("http://1.2.3.4:8690/d/{kept}/movie/1"));
        assert_eq!(std::fs::read_to_string(&ep).unwrap(), format!("http://localhost:8690/w/{kept}/tok"));
        assert_eq!(std::fs::read_to_string(&other).unwrap(), format!("/{old}/"), "only placeholders");
        assert_eq!(swap_secret_in_placeholders(dir.path(), "", &kept), 0);
    }
}
