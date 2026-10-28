//! Watch now: titles playing while they download. One file per Jellyfin
//! profile, watchnow-<jellyfin user id>.json beside history-<id>.json, holds
//! what that profile is watching, where it stopped, and which torrent it owns.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Entry {
    /// "movie:<tmdb>" | "ep:<tvdb>:<season>:<episode>"
    pub key: String,
    /// movie | episode
    pub kind: String,
    /// The movie's name, or for an episode its SHOW's name: the `.strm`
    /// folder is built from it (see `strm_path`).
    pub title: String,
    /// For an episode, its label ("S2 E3" or "S2 E3 · Name").
    pub subtitle: Option<String>,
    pub poster: Option<String>,
    pub backdrop: Option<String>,
    pub tmdb: Option<i64>,
    pub tvdb: Option<i64>,
    pub season: Option<i64>,
    pub episode: Option<i64>,
    pub position: f64,
    pub duration: Option<f64>,
    pub hash: Option<String>,
    /// Chillarr started this torrent for Watch now, so it may pause it.
    pub owned: bool,
    pub strm_jf_id: Option<String>,
    pub token: String,
    pub watching: bool,
    pub updated_at: i64,
}

/// The saved file. A struct so fields can be added without breaking old files.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct Saved {
    #[serde(default)]
    entries: Vec<Entry>,
    /// key -> (seconds, duration) of titles removed from Watch now, so
    /// pressing it again later starts where you were.
    #[serde(default)]
    positions: HashMap<String, (f64, Option<f64>)>,
    /// Torrents Watch now started, kept after their entries hand over to the
    /// library, so a season pack can be reused and every one is cleared
    /// from qBittorrent once nothing wants it (it never seeds).
    #[serde(default)]
    torrents: Vec<Owned>,
}

/// A torrent Watch now started, and the title it was for.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Owned {
    /// Lower case.
    pub hash: String,
    /// movie | episode
    pub kind: String,
    pub title: String,
    pub tmdb: Option<i64>,
    pub tvdb: Option<i64>,
}

impl Owned {
    fn of(e: &Entry) -> Option<Owned> {
        let hash = e.hash.as_deref().filter(|h| e.owned && !h.is_empty())?.to_ascii_lowercase();
        Some(Owned { hash, kind: e.kind.clone(), title: e.title.clone(), tmdb: e.tmdb, tvdb: e.tvdb })
    }

    /// As an entry, for the checks that take one (the shelf, the arr queue).
    fn as_entry(&self) -> Entry {
        Entry { kind: self.kind.clone(), title: self.title.clone(), tmdb: self.tmdb, tvdb: self.tvdb, hash: Some(self.hash.clone()), owned: true, ..Default::default() }
    }
}

pub struct WatchNow {
    dir: PathBuf,
    /// uid -> that profile's state. Every watchnow-*.json is read once, in `load`.
    profiles: Mutex<HashMap<String, Saved>>,
    /// The last save that failed, for `take_error` (`remove` can't return it).
    error: Mutex<Option<String>>,
}

pub fn file(dir: &Path, uid: &str) -> PathBuf {
    dir.join(format!("watchnow-{uid}.json"))
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Reads one profile's file. A file that won't parse is moved aside to
/// `.json.bad` (so the next save can't wipe it) and the profile starts empty.
fn read_file(path: &Path) -> Saved {
    let Ok(text) = std::fs::read_to_string(path) else { return Saved::default() };
    match serde_json::from_str(&text) {
        Ok(s) => s,
        Err(_) => {
            let _ = std::fs::rename(path, path.with_extension("json.bad"));
            Saved::default()
        }
    }
}

impl WatchNow {
    pub fn load(dir: PathBuf) -> WatchNow {
        let mut map = HashMap::new();
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for f in rd.flatten() {
                let name = f.file_name().to_string_lossy().into_owned();
                if let Some(uid) = name.strip_prefix("watchnow-").and_then(|n| n.strip_suffix(".json")) {
                    map.insert(uid.to_string(), read_file(&f.path()));
                }
            }
        }
        WatchNow { dir, profiles: Mutex::new(map), error: Mutex::new(None) }
    }

    /// Runs `f` on the profile's state; saves afterwards when it says so.
    fn with<R>(&self, uid: &str, f: impl FnOnce(&mut Saved) -> (R, bool)) -> (R, Result<(), String>) {
        let mut all = self.profiles.lock().unwrap_or_else(|e| e.into_inner());
        let saved = all.entry(uid.to_string()).or_insert_with(|| read_file(&file(&self.dir, uid)));
        let (r, dirty) = f(saved);
        let result = if dirty { self.save(uid, saved) } else { Ok(()) };
        if let Err(e) = &result {
            *self.error.lock().unwrap_or_else(|e| e.into_inner()) = Some(e.clone());
        }
        (r, result)
    }

    /// Writes a temp file, then renames it over the real one.
    fn save(&self, uid: &str, saved: &Saved) -> Result<(), String> {
        let fail = |e: &dyn std::fmt::Display| format!("Couldn't save Watch now: {e}");
        let text = serde_json::to_string_pretty(saved).map_err(|e| fail(&e))?;
        let path = file(&self.dir, uid);
        let tmp = path.with_extension("json.tmp");
        std::fs::create_dir_all(&self.dir).map_err(|e| fail(&e))?;
        std::fs::write(&tmp, text).map_err(|e| fail(&e))?;
        std::fs::rename(&tmp, &path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            fail(&e)
        })
    }

    /// The last save error since the previous call, if any.
    pub fn take_error(&self) -> Option<String> {
        self.error.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    pub fn entries(&self, uid: &str) -> Vec<Entry> {
        self.with(uid, |s| (s.entries.clone(), false)).0
    }

    /// Adds the entry, or replaces the one with the same key.
    pub fn upsert(&self, uid: &str, e: Entry) -> Result<(), String> {
        self.with(uid, |s| {
            match s.entries.iter_mut().find(|x| x.key == e.key) {
                Some(x) => *x = e,
                None => s.entries.push(e),
            }
            ((), true)
        })
        .1
    }

    /// A failed save is left in `take_error`.
    pub fn remove(&self, uid: &str, key: &str) -> Option<Entry> {
        self.with(uid, |s| {
            let r = s.entries.iter().position(|x| x.key == key).map(|i| s.entries.remove(i));
            let dirty = r.is_some();
            (r, dirty)
        })
        .0
    }

    /// Drops a removed profile: its cached state, its file and any copy set
    /// aside as unreadable. Returns its entries no other profile has, whose
    /// placeholders should go too (`drop_placeholders`).
    pub fn forget(&self, uid: &str) -> Vec<Entry> {
        let mut all = self.profiles.lock().unwrap_or_else(|e| e.into_inner());
        let gone = all.remove(uid).map(|s| s.entries).unwrap_or_default();
        let path = file(&self.dir, uid);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("json.bad"));
        gone.into_iter().filter(|e| !all.values().any(|s| s.entries.iter().any(|x| x.key == e.key))).collect()
    }

    /// Finds an entry by its stream token, across every profile.
    pub fn by_token(&self, token: &str) -> Option<(String, Entry)> {
        if token.is_empty() {
            return None;
        }
        let all = self.profiles.lock().unwrap_or_else(|e| e.into_inner());
        all.iter().find_map(|(uid, s)| s.entries.iter().find(|e| e.token == token).map(|e| (uid.clone(), e.clone())))
    }

    pub fn set_position(&self, uid: &str, key: &str, secs: f64, duration: Option<f64>) -> Result<(), String> {
        self.with(uid, |s| {
            let Some(e) = s.entries.iter_mut().find(|x| x.key == key) else { return ((), false) };
            e.position = secs;
            if duration.is_some() {
                e.duration = duration;
            }
            e.updated_at = now();
            ((), true)
        })
        .1
    }

    /// Remembers the Jellyfin item an entry plays through.
    pub fn set_jf_id(&self, uid: &str, key: &str, id: &str) -> Result<(), String> {
        self.with(uid, |s| {
            let Some(e) = s.entries.iter_mut().find(|x| x.key == key) else { return ((), false) };
            if e.strm_jf_id.as_deref() == Some(id) {
                return ((), false);
            }
            e.strm_jf_id = Some(id.to_string());
            ((), true)
        })
        .1
    }

    /// The stream token for a title. A title has one `.strm`, so one address,
    /// shared by every profile that has it. A new entry for `key` takes this
    /// before `new_token()`. Picked from the first profile by id, so every
    /// caller gets the same one.
    pub fn token_for(&self, key: &str) -> Option<String> {
        let all = self.profiles.lock().unwrap_or_else(|e| e.into_inner());
        let mut uids: Vec<&String> = all.keys().collect();
        uids.sort();
        uids.into_iter().find_map(|u| all[u].entries.iter().find(|e| e.key == key && !e.token.is_empty()).map(|e| e.token.clone()))
    }

    /// Makes every profile's entry for `key` use one token (`token_for`, or
    /// `fallback` when none has one yet). Returns that token.
    pub fn share_token(&self, key: &str, fallback: &str) -> Result<String, String> {
        let token = self.token_for(key).unwrap_or_else(|| fallback.to_string());
        let uids: Vec<String> = {
            let all = self.profiles.lock().unwrap_or_else(|e| e.into_inner());
            all.iter().filter(|(_, s)| s.entries.iter().any(|e| e.key == key && e.token != token)).map(|(u, _)| u.clone()).collect()
        };
        for uid in uids {
            self.with(&uid, |s| {
                let mut dirty = false;
                for e in s.entries.iter_mut().filter(|e| e.key == key && e.token != token) {
                    e.token = token.clone();
                    dirty = true;
                }
                ((), dirty)
            })
            .1?;
        }
        Ok(token)
    }

    /// Whether any profile still has this title. Its `.strm` (and the Jellyfin
    /// item made from it) is shared, so removing one profile's entry must
    /// delete the `.strm` only when this is false afterwards.
    pub fn any_has(&self, key: &str) -> bool {
        let all = self.profiles.lock().unwrap_or_else(|e| e.into_inner());
        all.values().any(|s| s.entries.iter().any(|e| e.key == key))
    }

    /// Every profile's entries, with the profile's id.
    pub fn all(&self) -> Vec<(String, Entry)> {
        let all = self.profiles.lock().unwrap_or_else(|e| e.into_inner());
        all.iter().flat_map(|(u, s)| s.entries.iter().map(move |e| (u.clone(), e.clone()))).collect()
    }

    pub fn entry(&self, uid: &str, key: &str) -> Option<Entry> {
        self.with(uid, |s| (s.entries.iter().find(|e| e.key == key).cloned(), false)).0
    }

    /// Keeps where a profile stopped in a title it no longer has an entry for.
    /// A title watched to the end has nothing to come back to: any position
    /// kept for it goes instead.
    pub fn remember(&self, uid: &str, key: &str, secs: f64, duration: Option<f64>) -> Result<(), String> {
        if !worth_remembering(secs, duration) {
            return self.forget_position(uid, key);
        }
        self.with(uid, |s| {
            s.positions.insert(key.to_string(), (secs, duration));
            ((), true)
        })
        .1
    }

    /// Where the profile stopped, unless that was the end.
    pub fn recall(&self, uid: &str, key: &str) -> Option<(f64, Option<f64>)> {
        self.with(uid, |s| (s.positions.get(key).copied(), false)).0.filter(|(p, d)| worth_remembering(*p, *d))
    }

    /// The title is the library's now (or was finished): no kept position.
    pub fn forget_position(&self, uid: &str, key: &str) -> Result<(), String> {
        self.with(uid, |s| {
            let dirty = s.positions.remove(key).is_some();
            ((), dirty)
        })
        .1
    }

    /// Start over (spec call 4): this profile's Watch now copies of the show's
    /// episodes (one season, or every season but specials) go back to the
    /// start, and the positions kept for them go.
    pub fn reset(&self, uid: &str, tvdb: i64, season: Option<i64>) -> Result<(), String> {
        let inside = |s: i64| season.map_or(s > 0, |n| s == n);
        self.with(uid, |s| {
            let mut dirty = false;
            for e in s.entries.iter_mut().filter(|e| e.kind == "episode" && e.tvdb == Some(tvdb) && e.season.is_some_and(inside)) {
                if e.position != 0.0 {
                    e.position = 0.0;
                    dirty = true;
                }
            }
            let before = s.positions.len();
            s.positions.retain(|k, _| !episode_key(k).is_some_and(|(t, n, _)| t == tvdb && inside(n)));
            dirty |= s.positions.len() != before;
            ((), dirty)
        })
        .1
    }

    /// Nobody is watching anything (on launch: the player isn't open).
    pub fn clear_watching(&self) -> Result<(), String> {
        let uids: Vec<String> = {
            let all = self.profiles.lock().unwrap_or_else(|e| e.into_inner());
            all.iter().filter(|(_, s)| s.entries.iter().any(|e| e.watching)).map(|(u, _)| u.clone()).collect()
        };
        let mut result = Ok(());
        for uid in uids {
            let r = self.with(&uid, |s| {
                s.entries.iter_mut().for_each(|e| e.watching = false);
                ((), true)
            });
            if r.1.is_err() {
                result = r.1;
            }
        }
        result
    }

    /// Records each owned torrent an entry points at in that profile's file,
    /// so it's still known after the entry hands over.
    pub fn keep_owned(&self) -> Result<(), String> {
        let uids: Vec<String> = {
            let all = self.profiles.lock().unwrap_or_else(|e| e.into_inner());
            all.iter().filter(|(_, s)| s.entries.iter().filter_map(Owned::of).any(|o| !s.torrents.iter().any(|t| t.hash == o.hash))).map(|(u, _)| u.clone()).collect()
        };
        let mut result = Ok(());
        for uid in uids {
            let r = self.with(&uid, |s| {
                let mut dirty = false;
                for o in s.entries.iter().filter_map(Owned::of).collect::<Vec<_>>() {
                    if !s.torrents.iter().any(|t| t.hash == o.hash) {
                        s.torrents.push(o);
                        dirty = true;
                    }
                }
                ((), dirty)
            });
            if r.1.is_err() {
                result = r.1;
            }
        }
        result
    }

    /// Every torrent Watch now started, once each: the recorded ones and any
    /// owned entry's not recorded yet.
    pub fn owned_torrents(&self) -> Vec<Owned> {
        let all = self.profiles.lock().unwrap_or_else(|e| e.into_inner());
        let mut uids: Vec<&String> = all.keys().collect();
        uids.sort();
        let mut out: Vec<Owned> = Vec::new();
        for s in uids.into_iter().map(|u| &all[u]) {
            for o in s.torrents.iter().cloned().chain(s.entries.iter().filter_map(Owned::of)) {
                if !out.iter().any(|x| x.hash == o.hash) {
                    out.push(o);
                }
            }
        }
        out
    }

    /// The hashes of `owned_torrents`, lower case. Watch now alone sets
    /// their file priorities and clears them, so `waiting` leaves them be.
    pub fn owned_set(&self) -> HashSet<String> {
        self.owned_torrents().into_iter().map(|o| o.hash).collect()
    }

    /// The torrent is gone (cleared, or no longer in qBittorrent).
    pub fn forget_torrent(&self, hash: &str) -> Result<(), String> {
        let h = hash.to_ascii_lowercase();
        let uids: Vec<String> = {
            let all = self.profiles.lock().unwrap_or_else(|e| e.into_inner());
            all.iter().filter(|(_, s)| s.torrents.iter().any(|t| t.hash == h)).map(|(u, _)| u.clone()).collect()
        };
        let mut result = Ok(());
        for uid in uids {
            let r = self.with(&uid, |s| {
                s.torrents.retain(|t| t.hash != h);
                ((), true)
            });
            if r.1.is_err() {
                result = r.1;
            }
        }
        result
    }

    pub fn set_watching(&self, uid: &str, key: &str, on: bool) -> Result<(), String> {
        self.with(uid, |s| {
            let Some(e) = s.entries.iter_mut().find(|x| x.key == key) else { return ((), false) };
            e.watching = on;
            e.updated_at = now();
            ((), true)
        })
        .1
    }
}

/// True only if some entry with this hash is owned, none with it is being
/// watched, and nothing else wants the torrent.
pub fn may_pause(hash: &str, all: &[(String, Entry)], wanted_elsewhere: bool) -> bool {
    let same: Vec<&Entry> = all.iter().map(|(_, e)| e).filter(|e| e.hash.as_deref().is_some_and(|h| h.eq_ignore_ascii_case(hash))).collect();
    !wanted_elsewhere && same.iter().any(|e| e.owned) && !same.iter().any(|e| e.watching)
}

/// "movie:<tmdb>" for a movie (an anime film too), "ep:<tvdb>:<season>:<episode>" for an episode.
pub fn key_for(tile: &crate::tiles::Tile, season: Option<i64>, episode: Option<i64>) -> Option<String> {
    let movie = tile.kind == "movie" || (tile.kind == "anime" && tile.format.as_deref() == Some("MOVIE"));
    if movie {
        return tile.tmdb.map(|t| format!("movie:{t}"));
    }
    match (tile.tvdb, season, episode) {
        (Some(tvdb), Some(s), Some(e)) => Some(format!("ep:{tvdb}:{s}:{e}")),
        _ => None,
    }
}

/// 32 hex characters from the OS's random source.
pub fn new_token() -> String {
    let mut b = [0u8; 16];
    if getrandom::getrandom(&mut b).is_err() {
        // The OS source failing is not expected; fall back to a clock mix
        // rather than panic.
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(1);
        let a = (n as u64) ^ 0x9E37_79B9_7F4A_7C15 ^ (std::process::id() as u64).rotate_left(29);
        let c = a.wrapping_mul(0x2545_F491_4F6C_DD1D).rotate_left(17) ^ (n >> 64) as u64;
        return format!("{a:016x}{c:016x}");
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// ---------- Playing through Jellyfin ----------
//
// Each Watch now title is a `.strm` placeholder in Chillarr's own library,
// pointing at the Discover shelf's play server (`/w/<secret>/<token>`).
// Jellyfin probes and transcodes it like any file, so the usual player works.

/// The folder (under Chillarr's app data) holding the placeholders. Its name is
/// how `jellyfin::on_shelf` keeps them out of every row.
pub const LIBRARY: &str = "Chillarr Watch now";
/// A Jellyfin library has one content type, so movies and episodes get one each.
const MOVIES_LIB: &str = "Chillarr Watch now Movies";
const SERIES_LIB: &str = "Chillarr Watch now Series";

/// One of the Watch now libraries, by its Jellyfin name.
pub fn is_library(name: &str) -> bool {
    name == MOVIES_LIB || name == SERIES_LIB
}
/// How long a new placeholder may take to show up in Jellyfin.
/// Jellyfin can be busy with a long scan (the shelf's first episode scan took
/// minutes), so the wait is generous.
const SCAN_WAIT: std::time::Duration = std::time::Duration::from_secs(150);

/// Movies\<Title> [tmdbid-N]\<same>.strm, or
/// Series\<Show> [tvdbid-N]\Season NN\<Show> SNNENN.strm (Jellyfin's naming,
/// with an episode's `title` taken as its show's name), so it matches the
/// title and the episode by itself. None when the entry lacks the ids or
/// numbers it needs.
pub fn strm_path(root: &Path, e: &Entry) -> Option<PathBuf> {
    use crate::discover_shelf::safe_name;
    match e.kind.as_str() {
        "movie" => {
            let folder = format!("{} [tmdbid-{}]", safe_name(&e.title), e.tmdb?);
            Some(root.join("Movies").join(&folder).join(format!("{folder}.strm")))
        }
        "episode" => {
            let (show, s, n) = (safe_name(&e.title), e.season?, e.episode?);
            let folder = format!("{show} [tvdbid-{}]", e.tvdb?);
            Some(root.join("Series").join(folder).join(format!("Season {s:02}")).join(format!("{show} S{s:02}E{n:02}.strm")))
        }
        _ => None,
    }
}

/// The address inside a placeholder. `localhost`, as the app's CSP only lets
/// media come from there.
pub fn strm_text(port: u16, secret: &str, token: &str) -> String {
    format!("http://localhost:{port}/w/{secret}/{token}")
}

/// Writes the entry's placeholder under `root`; returns its path.
pub fn write_strm(root: &Path, e: &Entry, port: u16, secret: &str) -> Result<PathBuf, String> {
    let path = strm_path(root, e).filter(|_| !e.token.is_empty()).ok_or_else(|| format!("Couldn't set up {} to play: it's missing its details.", e.title))?;
    let fail = |err: std::io::Error| format!("Couldn't set up {} to play: {err}", e.title);
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).map_err(fail)?;
    }
    std::fs::write(&path, strm_text(port, secret, &e.token)).map_err(fail)?;
    Ok(path)
}

fn norm_path(p: &str) -> String {
    p.replace('/', "\\").trim_end_matches('\\').to_ascii_lowercase()
}

/// The id of the item among `items` whose file is `path`.
pub fn item_with_path(items: &[serde_json::Value], path: &Path) -> Option<String> {
    let want = norm_path(&path.to_string_lossy());
    items.iter().find(|i| i["Path"].as_str().is_some_and(|p| norm_path(p) == want)).and_then(|i| i["Id"].as_str()).map(String::from)
}

/// The id of the library (from /Library/VirtualFolders) with `path` as one of its folders.
pub fn library_with_path(libs: &[serde_json::Value], path: &Path) -> Option<String> {
    let want = norm_path(&path.to_string_lossy());
    libs.iter()
        .find(|l| l["Locations"].as_array().is_some_and(|ls| ls.iter().any(|x| x.as_str().is_some_and(|p| norm_path(p) == want))))
        .and_then(|l| l["ItemId"].as_str())
        .map(String::from)
}

/// Where the entry's video is: (arr kind, TMDB/TVDB id, which file).
pub fn wanted(e: &Entry) -> Option<(&'static str, i64, crate::discover_shelf::Want)> {
    use crate::discover_shelf::Want;
    match e.kind.as_str() {
        "movie" => Some(("movie", e.tmdb?, Want::Movie)),
        "episode" => Some(("series", e.tvdb?, Want::Episode(e.season?, e.episode?))),
        _ => None,
    }
}

/// How to find the entry's torrent: the one it already knows, if any, else
/// Radarr's/Sonarr's queue. It's only changed (in-order download, skipping
/// the rest of a pack) when Watch now started it; anyone else's download is
/// just read.
pub fn lookup_for(e: &Entry) -> crate::discover_shelf::Lookup {
    crate::discover_shelf::Lookup { hash: e.hash.clone().filter(|h| !h.is_empty()), tune: e.owned }
}

/// The episodes a torrent file is, by its own name (not the pack folder's).
/// None for anything that isn't a video, and for samples.
fn file_episodes(f: &serde_json::Value) -> Vec<(i64, i64)> {
    let name = f["name"].as_str().unwrap_or_default().to_ascii_lowercase();
    if !is_video(&name) || name.contains("sample") {
        return vec![];
    }
    crate::qbit::episode_numbers(name.rsplit(['/', '\\']).next().unwrap_or_default())
}

fn is_video(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    crate::qbit::VIDEO.iter().any(|x| n.ends_with(x))
}

/// What to do with a pack's files.
#[derive(Debug, PartialEq)]
pub struct PackPlan {
    /// File ids to set to "don't download".
    pub skip: Vec<i64>,
    /// File ids to switch back on.
    pub restore: Vec<i64>,
    /// For each video file, whether something still wants it.
    pub wanted: Vec<bool>,
}

/// Every video whose episodes meet `wants` (or that is `chosen`) is kept and
/// switched on; every other video is skipped, unless it's already fully
/// downloaded (skipping it saves nothing, and a pack with nothing wanted
/// reads as 0% in qBittorrent). Other files are left alone.
pub fn pack_plan(files: &[serde_json::Value], wants: &HashSet<(i64, i64)>, chosen: Option<&serde_json::Value>) -> PackPlan {
    let mut plan = PackPlan { skip: vec![], restore: vec![], wanted: vec![] };
    for (pos, f) in files.iter().enumerate() {
        if !is_video(f["name"].as_str().unwrap_or_default()) {
            continue;
        }
        let id = f["index"].as_i64().unwrap_or(pos as i64);
        let prio = f["priority"].as_i64().unwrap_or(1);
        let is_chosen = chosen.is_some_and(|c| match c["index"].as_i64() {
            Some(i) => f["index"].as_i64() == Some(i),
            None => f["name"] == c["name"],
        });
        let wanted = is_chosen || file_episodes(f).iter().any(|e| wants.contains(e));
        plan.wanted.push(wanted);
        if wanted && prio == 0 {
            plan.restore.push(id);
        } else if !wanted && prio != 0 && f["progress"].as_f64().unwrap_or(0.0) < 1.0 {
            plan.skip.push(id);
        }
    }
    plan
}

/// For a pack Watch now owns: the qBittorrent file ids to skip and to switch
/// on, keeping `chosen` and every file of an episode in `keep` (what the
/// other profiles and normal Downloads want, see `pack_wants`).
pub fn pack_prios(files: &[serde_json::Value], chosen: &serde_json::Value, keep: &HashSet<(i64, i64)>) -> (Vec<i64>, Vec<i64>) {
    let p = pack_plan(files, keep, Some(chosen));
    (p.skip, p.restore)
}

/// Whether every file still set to download, once `plan` is applied, is
/// done. Judged from the files themselves: qBittorrent reports a torrent
/// with nothing wanted as 0%, never finished.
pub fn pack_complete(files: &[serde_json::Value], plan: &PackPlan) -> bool {
    files.iter().enumerate().all(|(pos, f)| {
        let id = f["index"].as_i64().unwrap_or(pos as i64);
        let on = if plan.restore.contains(&id) {
            true
        } else if plan.skip.contains(&id) {
            false
        } else {
            f["priority"].as_i64().unwrap_or(1) != 0
        };
        !on || f["progress"].as_f64().unwrap_or(0.0) >= 1.0
    })
}

/// The UserData that puts the library copy at Watch now's position, played
/// when Watch now last played it (so Continue Watching sorts it there).
fn position_body(secs: f64, played_at: i64) -> serde_json::Value {
    let mut body = serde_json::json!({ "PlaybackPositionTicks": crate::jellyfin::seconds_to_ticks(secs) });
    if let Some(at) = chrono::DateTime::from_timestamp(played_at, 0).filter(|_| played_at > 0) {
        body["LastPlayedDate"] = serde_json::json!(at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    }
    body
}

/// Failures that repeat every tick while a service is down are logged once,
/// when they start or change; `None` (it worked) clears the step.
static LAST_ERRORS: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

fn changed(last: &mut HashMap<String, String>, step: &str, err: Option<&str>) -> bool {
    match err {
        None => {
            last.remove(step);
            false
        }
        Some(e) => last.insert(step.to_string(), e.to_string()).as_deref() != Some(e),
    }
}

fn log_once(app: &tauri::AppHandle, step: &str, err: Option<String>) {
    let new = {
        let mut m = LAST_ERRORS.lock().unwrap_or_else(|e| e.into_inner());
        changed(m.get_or_insert_with(HashMap::new), step, err.as_deref())
    };
    if let (true, Some(e)) = (new, err) {
        crate::log::log(app, crate::model::Level::Amber, e);
    }
}

/// Whether any file of a pack is still wanted.
pub fn pack_still_wanted(files_wanted: &[bool]) -> bool {
    files_wanted.iter().any(|w| *w)
}

/// The episodes an owned pack must keep: every Watch now episode of the
/// show or on the torrent, from every profile, and every episode a normal
/// Download wants (`downloads`: Sonarr's monitored episodes with no file).
/// Spec call 1: Watch now never skips what a normal Download wants.
pub fn pack_wants(hash: &str, tvdb: Option<i64>, all: &[(String, Entry)], downloads: &[(i64, i64)]) -> HashSet<(i64, i64)> {
    let mut out: HashSet<(i64, i64)> = downloads.iter().copied().collect();
    for (_, e) in all.iter().filter(|(_, e)| e.kind == "episode") {
        let on_it = e.hash.as_deref().is_some_and(|h| h.eq_ignore_ascii_case(hash));
        let same_show = tvdb.is_some() && e.tvdb == tvdb;
        if let (true, Some(s), Some(n)) = (on_it || same_show, e.season, e.episode) {
            out.insert((s, n));
        }
    }
    out
}

/// Spec call 2: a pack Watch now owns stays (stopped, never seeding) while
/// a profile's "where you're at" is in a season the pack holds. This only
/// keeps the torrent; it never switches a file on (`pack_wants` decides
/// that), so Watch now on the next episode reuses the pack and starts fast.
pub fn marker_keeps(files: &[serde_json::Value], spots: &[crate::marker::Spot]) -> bool {
    files.iter().flat_map(file_episodes).any(|(s, _)| s > 0 && spots.iter().any(|m| m.season == s))
}

/// What an episode row says about a Watch now copy of it.
#[derive(Debug, Clone, PartialEq)]
pub struct NowState {
    pub key: String,
    /// Watched partway (else pressed, not started).
    pub half: bool,
    /// How far the torrent is, 0..1.
    pub pct: f64,
    /// How far into it, 0..1, once started (none once finished).
    pub progress: Option<f64>,
    /// Watched to the end through Watch now, before the file is in the
    /// library: the row counts it as watched, so where you're at is worked
    /// out past it.
    pub finished: bool,
}

/// The Watch now state of one episode of a show, from this profile's entries
/// and their torrents' progress (`pct_by_hash`, hashes in lower case).
pub fn episode_state(entries: &[Entry], pct_by_hash: &HashMap<String, f64>, tvdb: Option<i64>, season: i64, episode: i64) -> Option<NowState> {
    let e = entries.iter().find(|e| e.kind == "episode" && tvdb.is_some() && e.tvdb == tvdb && e.season == Some(season) && e.episode == Some(episode))?;
    let pct = e.hash.as_deref().and_then(|h| pct_by_hash.get(&h.to_ascii_lowercase())).copied().unwrap_or(0.0);
    let done = finished(e.position, e.duration);
    let progress = e.duration.filter(|d| *d > 0.0 && !done).map(|d| (e.position / d).clamp(0.0, 1.0)).filter(|p| *p > 0.0);
    Some(NowState { key: e.key.clone(), half: e.position > 0.0 && !done, pct, progress, finished: done })
}

/// How far the file a Watch now title plays is, 0..1: the movie's main video,
/// or the episode's own file in a pack (the same choice the stream makes).
/// None until the torrent lists a file for it.
pub fn chosen_progress(files: &[serde_json::Value], e: &Entry) -> Option<f64> {
    let f = match (e.kind.as_str(), e.season, e.episode) {
        ("episode", Some(s), Some(n)) => crate::qbit::pick_episode(files, s, n),
        ("episode", ..) => None,
        _ => crate::qbit::pick_video(files, false),
    }?;
    Some(f["progress"].as_f64().unwrap_or(0.0).clamp(0.0, 1.0))
}

/// Last touched first, as Continue Watching lists them.
pub fn newest_first(mut entries: Vec<Entry>) -> Vec<Entry> {
    entries.sort_by_key(|e| std::cmp::Reverse(e.updated_at));
    entries
}

/// The watching profile's Watch now episodes of one show, and how far each
/// torrent is (for `episode_state`). Empty when signed out.
pub async fn show_states(app: &tauri::AppHandle, tvdb: Option<i64>) -> (Vec<Entry>, HashMap<String, f64>) {
    use tauri::Manager;
    let Some(tvdb) = tvdb else { return (vec![], HashMap::new()) };
    let Ok(uid) = uid(app) else { return (vec![], HashMap::new()) };
    let entries: Vec<Entry> = app.state::<WatchNow>().entries(&uid).into_iter().filter(|e| e.kind == "episode" && e.tvdb == Some(tvdb)).collect();
    let mut pct = HashMap::new();
    if let Some(q) = crate::waiting::qbit(app).filter(|_| !entries.is_empty()) {
        for h in entries.iter().filter_map(|e| e.hash.as_deref()).filter(|h| !h.is_empty()) {
            let h = h.to_ascii_lowercase();
            if pct.contains_key(&h) {
                continue;
            }
            if let Ok(Some(t)) = q.torrent(&h).await {
                pct.insert(h, t["progress"].as_f64().unwrap_or(0.0));
            }
        }
    }
    (entries, pct)
}

/// A Watch now title hands over to its library copy only when that copy
/// exists and the title isn't playing.
pub fn ready_for_handover(e: &Entry, real_item: bool) -> bool {
    real_item && !e.watching
}

/// The position to give the library copy: Watch now's, unless the copy
/// was already played further (or Watch now's never started).
pub fn position_to_write(saved: f64, real: f64) -> Option<f64> {
    (saved > 0.0 && saved > real).then_some(saved)
}

/// Watched to the end (90%, Jellyfin's own line).
fn finished(position: f64, duration: Option<f64>) -> bool {
    duration.is_some_and(|d| d > 0.0 && position >= 0.9 * d)
}

/// A position worth starting from next time: started, and not the end.
fn worth_remembering(position: f64, duration: Option<f64>) -> bool {
    position > 0.0 && !finished(position, duration)
}

/// (tvdb, season, episode) of an "ep:<tvdb>:<season>:<episode>" key.
fn episode_key(key: &str) -> Option<(i64, i64, i64)> {
    let mut it = key.strip_prefix("ep:")?.split(':').map(|x| x.parse::<i64>().ok());
    let (t, s, e) = (it.next()??, it.next()??, it.next()??);
    it.next().is_none().then_some((t, s, e))
}

/// The episode an entry is, when leaving it at `position` finished it: where
/// you're at moves on at once, not when the file reaches the library.
fn finished_spot(e: &Entry, position: f64, duration: Option<f64>) -> Option<(i64, crate::marker::Spot)> {
    if e.kind != "episode" || !finished(position, duration.or(e.duration)) {
        return None;
    }
    Some((e.tvdb?, crate::marker::Spot { season: e.season?, episode: e.episode? }))
}

/// The UserData that marks the library copy watched, when Watch now
/// finished it, so the watch log picks it up (it skips anything without a
/// last-played date).
fn played_body(played_at: i64) -> serde_json::Value {
    let at = if played_at > 0 { played_at } else { now() };
    let mut body = position_body(0.0, at);
    body["Played"] = serde_json::json!(true);
    body
}

/// What removing a Watch now episode does to a torrent only it owned.
#[derive(Debug, PartialEq)]
pub enum OnRemove {
    /// Stop it and delete the partial download.
    Delete,
    /// A normal Download wants another file in it: only these files (the
    /// removed episode's) are switched off, and the torrent carries on.
    Skip(Vec<i64>),
}

/// `downloads`: episodes a normal Download wants (Sonarr: monitored, no
/// file); `watch_now`: every episode Watch now itself asked for, this one
/// included (Sonarr lists those as wanted too, so they don't count).
pub fn on_remove(files: &[serde_json::Value], episode: (i64, i64), downloads: &[(i64, i64)], watch_now: &HashSet<(i64, i64)>) -> OnRemove {
    let needed: HashSet<(i64, i64)> = downloads.iter().copied().filter(|x| *x != episode && !watch_now.contains(x)).collect();
    let open = |f: &serde_json::Value| f["progress"].as_f64().unwrap_or(0.0) < 1.0;
    if !files.iter().any(|f| open(f) && file_episodes(f).iter().any(|x| needed.contains(x))) {
        return OnRemove::Delete;
    }
    let skip = files
        .iter()
        .enumerate()
        .filter(|(_, f)| open(f) && f["priority"].as_i64().unwrap_or(1) != 0)
        .filter(|(_, f)| {
            let eps = file_episodes(f);
            eps.contains(&episode) && !eps.iter().any(|x| needed.contains(x))
        })
        .map(|(pos, f)| f["index"].as_i64().unwrap_or(pos as i64))
        .collect();
    OnRemove::Skip(skip)
}

/// One Watch now press per profile and title at a time. A second press while
/// the first is still getting ready waits for it and gets the same answer,
/// instead of asking Radarr/Sonarr again.
type Outcome = Option<Result<crate::tiles::Tile, String>>;
struct Flights {
    map: Mutex<HashMap<String, tokio::sync::watch::Receiver<Outcome>>>,
}

enum Flight<'a> {
    Lead(FlightGuard<'a>),
    Join(tokio::sync::watch::Receiver<Outcome>),
}

/// Held by the press doing the work; the key is free again when it drops,
/// however `start` ends.
struct FlightGuard<'a> {
    flights: &'a Flights,
    key: String,
    tx: tokio::sync::watch::Sender<Outcome>,
}

impl Flights {
    fn new() -> Flights {
        Flights { map: Mutex::new(HashMap::new()) }
    }

    fn claim(&self, key: &str) -> Flight<'_> {
        let mut m = self.map.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(rx) = m.get(key) {
            return Flight::Join(rx.clone());
        }
        let (tx, rx) = tokio::sync::watch::channel(None);
        m.insert(key.to_string(), rx);
        Flight::Lead(FlightGuard { flights: self, key: key.to_string(), tx })
    }
}

impl FlightGuard<'_> {
    fn finish(self, r: &Result<crate::tiles::Tile, String>) {
        let _ = self.tx.send(Some(r.clone()));
    }
}

impl Drop for FlightGuard<'_> {
    fn drop(&mut self) {
        self.flights.map.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.key);
    }
}

static FLIGHTS: std::sync::LazyLock<Flights> = std::sync::LazyLock::new(Flights::new);

/// A library file, not a placeholder (`on_shelf`, which covers the Watch now
/// libraries) and not an episode Jellyfin only knows is missing.
fn is_real(v: &serde_json::Value) -> bool {
    !crate::jellyfin::on_shelf(v) && v["Path"].as_str().is_some_and(|p| !p.is_empty()) && v["LocationType"] != "Virtual"
}

fn provider(v: &serde_json::Value, name: &str) -> Option<i64> {
    v["ProviderIds"][name].as_str().and_then(|s| s.parse().ok()).or_else(|| v["ProviderIds"][name].as_i64())
}

/// The library's copy of a movie.
pub fn real_movie(items: &[serde_json::Value], tmdb: i64) -> Option<&serde_json::Value> {
    items.iter().find(|v| is_real(v) && provider(v, "Tmdb") == Some(tmdb))
}

/// The library's show (its Jellyfin id), not the Watch now one.
pub fn real_series(items: &[serde_json::Value], tvdb: i64) -> Option<&str> {
    items.iter().find(|v| !crate::jellyfin::on_shelf(v) && provider(v, "Tvdb") == Some(tvdb)).and_then(|v| v["Id"].as_str())
}

/// The library's copy of an episode; a two-episode file counts for both.
pub fn real_episode(items: &[serde_json::Value], season: i64, episode: i64) -> Option<&serde_json::Value> {
    items.iter().find(|v| {
        let first = v["IndexNumber"].as_i64();
        let last = v["IndexNumberEnd"].as_i64().or(first);
        is_real(v) && v["ParentIndexNumber"].as_i64() == Some(season) && first.is_some_and(|f| f <= episode) && last.is_some_and(|l| episode <= l)
    })
}

/// Streams a Watch now title to whoever reads its placeholder (Jellyfin's
/// ffmpeg). Unlike the shelf, a read never asks for the title: Watch now
/// already did when it was pressed.
pub async fn serve(app: tauri::AppHandle, mut sock: tokio::net::TcpStream, req: crate::discover_shelf::Req, token: &str) -> std::io::Result<()> {
    use crate::discover_shelf::{find_source_for, plain, stream_range};
    use tauri::Manager;
    let found = app.state::<WatchNow>().by_token(token).and_then(|(_, e)| wanted(&e).map(|w| (w, lookup_for(&e))));
    let Some(((kind, id, want), how)) = found else {
        return plain(&mut sock, "404 Not Found", "not here").await;
    };
    let deadline = std::time::Instant::now() + crate::discover_shelf::HEAD_WAIT;
    let Some(src) = find_source_for(&app, kind, id, want, how, deadline).await else {
        return plain(&mut sock, "503 Service Unavailable", "Nothing is downloading for this yet.").await;
    };
    stream_range(&mut sock, &req, &src, deadline).await
}

/// Keeps a Watch now library off every account's home page and menu
/// (Jellyfin's "My Media" excludes). Its items still play; it's Chillarr's
/// plumbing, not a library to browse. Before this it showed up as "Chillarr
/// Watch now Series" next to Movies (seen 2026-10-19).
async fn hide_library(j: &crate::jellyfin::Jf, lib: &str) {
    let Ok(users) = j.get_json("/Users").await else { return };
    for u in users.as_array().into_iter().flatten() {
        let Some(uid) = u["Id"].as_str() else { continue };
        let mut cfg = u["Configuration"].clone();
        if !cfg.is_object() {
            continue;
        }
        let mut ex: Vec<serde_json::Value> = cfg["MyMediaExcludes"].as_array().cloned().unwrap_or_default();
        if ex.iter().any(|x| x.as_str() == Some(lib)) {
            continue;
        }
        ex.push(serde_json::json!(lib));
        cfg["MyMediaExcludes"] = serde_json::json!(ex);
        let _ = j.post_json(&format!("/Users/{uid}/Configuration"), &cfg).await;
    }
}

/// Gives the Watch now library the shelf's placeholder options (nothing
/// reads a download in the background). Only saves when something differs.
async fn quiet_library(j: &crate::jellyfin::Jf, lib_dir: &Path) -> Result<(), String> {
    let libs = j.libraries().await.map_err(|err| format!("Couldn't read Jellyfin's libraries: {err}"))?;
    let want = norm_path(&lib_dir.to_string_lossy());
    let Some(l) = libs.iter().find(|l| l["Locations"].as_array().is_some_and(|ls| ls.iter().any(|x| x.as_str().is_some_and(|p| norm_path(p) == want)))) else {
        return Err("Jellyfin didn't keep the Watch now library.".into());
    };
    // A library made while Jellyfin is busy scanning has no id for a while,
    // and saving options with an empty id is a 400 (measured 2026-10-19,
    // during the shelf's first episode scan: Watch now failed). It was made
    // with these options already, so without an id there's nothing to do.
    let Some(id) = l["ItemId"].as_str().filter(|s| !s.is_empty()) else { return Ok(()) };
    let mut opts = l["LibraryOptions"].clone();
    if crate::discover_shelf::quiet_options(&mut opts) {
        j.post_json("/Library/VirtualFolders/LibraryOptions", &serde_json::json!({ "Id": id, "LibraryOptions": opts }))
            .await
            .map_err(|err| format!("Couldn't set up the Watch now library in Jellyfin: {err}"))?;
    }
    Ok(())
}

/// Makes the entry playable through Jellyfin: writes its placeholder, makes
/// sure the Watch now libraries exist, has Jellyfin scan it in, and returns
/// the item's id (also saved on the entry).
pub async fn ensure_item(app: &tauri::AppHandle, uid: &str, e: &Entry) -> Result<String, String> {
    use crate::discover_shelf::Shelf;
    use serde_json::json;
    use tauri::Manager;
    let (port, secret) = {
        let shelf = app.state::<Shelf>();
        (shelf.port().ok_or("Chillarr's play server isn't running, so Watch now can't play. Restart Chillarr and try again.")?, shelf.secret())
    };
    let wn = app.state::<WatchNow>();
    let root = wn.dir.join(LIBRARY);
    // Every profile's entry for this title plays through the same `.strm`.
    let token = wn.share_token(&e.key, &e.token)?;
    let e = &Entry { token, ..e.clone() };
    let path = write_strm(&root, e, port, &secret)?;
    // Library changes need the owner's account.
    let j = crate::media::jf_admin(app).filter(|j| j.user_id.is_some()).ok_or("Jellyfin isn't signed in.")?;
    let uid_admin = j.user_id.clone().unwrap_or_default();
    let (lib_name, kind, sub) = if e.kind == "movie" { (MOVIES_LIB, "movies", "Movies") } else { (SERIES_LIB, "tvshows", "Series") };
    let lib_dir = root.join(sub);
    std::fs::create_dir_all(&lib_dir).map_err(|err| format!("Couldn't set up Watch now: {err}"))?;
    let lib_path = lib_dir.to_string_lossy().replace('/', "\\");
    j.ensure_library_with(lib_name, kind, &lib_path, crate::discover_shelf::placeholder_options()).await.map_err(|err| format!("Couldn't add the Watch now library to Jellyfin: {err}"))?;
    quiet_library(&j, &lib_dir).await?;
    let file = path.to_string_lossy().replace('/', "\\");
    j.post_json("/Library/Media/Updated", &json!({ "Updates": [{ "Path": file, "UpdateType": "Created" }] }))
        .await
        .map_err(|err| format!("Couldn't ask Jellyfin to look for {}: {err}", e.title))?;
    // Media/Updated alone waits out Jellyfin's 60 s library monitor delay;
    // refreshing just this library finds the file in about 4 s (measured).
    let mut refreshed = false;
    let item_type = if e.kind == "movie" { "Movie" } else { "Episode" };
    let until = std::time::Instant::now() + SCAN_WAIT;
    loop {
        let libs = j.libraries().await.unwrap_or_default();
        if let Some(lib) = library_with_path(&libs, &lib_dir) {
            if !refreshed {
                refreshed = true;
                hide_library(&j, &lib).await;
                let _ = j.post_json(&format!("/Items/{lib}/Refresh?recursive=true&metadataRefreshMode=Default&imageRefreshMode=Default"), &json!({})).await;
            }
            // Straight from Jellyfin: `Jf::items` leaves these placeholders out.
            let q = format!("/Users/{uid_admin}/Items?parentId={lib}&recursive=true&includeItemTypes={item_type}&fields=Path&enableImages=false&enableUserData=false");
            let items = j.get_json(&q).await.ok().and_then(|v| v["Items"].as_array().cloned()).unwrap_or_default();
            if let Some(id) = item_with_path(&items, &path) {
                // A failed save is left in `take_error`; the item plays either way.
                let _ = app.state::<WatchNow>().set_jf_id(uid, &e.key, &id);
                return Ok(id);
            }
        }
        if std::time::Instant::now() >= until {
            return Err(format!("Jellyfin didn't pick up {} in time. Try again in a minute.", e.title));
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}

// ---------- Commands ----------

/// How long Watch now waits for Radarr/Sonarr to find a release.
const FIND_WAIT: std::time::Duration = std::time::Duration::from_secs(180);
const NOTHING_YET: &str = "Nothing to download yet. It's on the way and will show up when it's found.";

/// Watch now owns a torrent (may pause and delete it) only when it was the
/// one to ask for it: nothing was downloading or wanted before the press.
pub fn decide_owned(queue_had_record: bool) -> bool {
    !queue_had_record
}

/// The line under a Watch now title. `progress` is 0..1 of the download.
pub fn status(progress: f64, paused: bool, complete: bool) -> String {
    let pct = (progress.clamp(0.0, 1.0) * 100.0).floor() as i64;
    if complete {
        "Ready".into()
    } else if paused {
        format!("Paused, {pct}% downloaded")
    } else {
        format!("Downloading {pct}%")
    }
}

/// A fresh entry for `tile`. For an episode, `title` is the show's name (a
/// series tile's title, or an episode tile's, which Jellyfin gives as the
/// show) and `subtitle` the episode's label.
pub fn new_entry(tile: &crate::tiles::Tile, key: &str, season: Option<i64>, episode: Option<i64>) -> Entry {
    let movie = key.starts_with("movie:");
    let subtitle = match (movie, season, episode) {
        (false, Some(s), Some(e)) => Some(tile.subtitle.clone().filter(|_| tile.kind == "episode").unwrap_or_else(|| format!("S{s} E{e}"))),
        _ => None,
    };
    Entry {
        key: key.to_string(),
        kind: if movie { "movie" } else { "episode" }.into(),
        title: tile.title.clone(),
        subtitle,
        poster: tile.poster.clone(),
        backdrop: tile.backdrop.clone(),
        tmdb: tile.tmdb,
        tvdb: tile.tvdb,
        season: if movie { None } else { season },
        episode: if movie { None } else { episode },
        updated_at: now(),
        ..Default::default()
    }
}

/// The entry as a tile the player and the rows can use.
fn tile_of(e: &Entry) -> crate::tiles::Tile {
    crate::tiles::Tile {
        subtitle: e.subtitle.clone(),
        poster: e.poster.clone(),
        backdrop: e.backdrop.clone(),
        tmdb: e.tmdb,
        tvdb: e.tvdb,
        jf_id: e.strm_jf_id.clone(),
        // Watched to the end: nothing to resume, it plays from the start (as a kept position would).
        resume_seconds: Some(e.position).filter(|p| worth_remembering(*p, e.duration)),
        progress: e.duration.filter(|d| *d > 0.0 && !finished(e.position, e.duration)).map(|d| (e.position / d).clamp(0.0, 1.0)),
        ..crate::tiles::Tile::new(&e.key, &e.kind, &e.title)
    }
}

fn emit_state(app: &tauri::AppHandle, key: &str, stage: &str, pct: f64) {
    // A web viewer's start is only theirs to watch (bridge.rs).
    crate::bridge::emit_user(app, "watchnow-state", serde_json::json!({ "key": key, "stage": stage, "pct": pct }));
}

/// Logs a failed save; the command carries on (the title still plays).
fn note(app: &tauri::AppHandle, r: Result<(), String>) {
    if let Err(e) = r {
        crate::log::log(app, crate::model::Level::Amber, e);
    }
}

/// `remove` keeps its save error aside; this logs it.
fn note_taken(app: &tauri::AppHandle) {
    use tauri::Manager;
    if let Some(err) = app.state::<WatchNow>().take_error() {
        crate::log::log(app, crate::model::Level::Amber, err);
    }
}

fn uid(app: &tauri::AppHandle) -> Result<String, String> {
    crate::media::active_user(app).ok_or_else(|| "Sign in to Jellyfin first.".to_string())
}

/// What Radarr/Sonarr hold for an entry's title: whether it was already
/// being downloaded or wanted, and its torrent if one is queued.
#[derive(Debug, PartialEq)]
struct ArrState {
    had_record: bool,
    hash: crate::qbit::Pick,
}

/// Reads the title's item, queue rows and (for an episode) episode list.
/// Any read that fails is an error, never an empty answer: an empty queue
/// would make a normal Download look like Watch now's own.
async fn arr_state(app: &tauri::AppHandle, e: &Entry) -> Result<ArrState, String> {
    use crate::model::AppId;
    let movie = e.kind == "movie";
    let owner = if movie { AppId::Radarr } else { AppId::Sonarr };
    let name = owner.label();
    let a = crate::media::arr(app, owner).ok_or_else(|| format!("{name} isn't set up"))?;
    let (list, idk, want) = if movie { ("movie", "tmdbId", e.tmdb) } else { ("series", "tvdbId", e.tvdb) };
    let items = a.list(list).await.map_err(|err| format!("{name} didn't answer: {err}"))?;
    let Some(item) = items.into_iter().find(|x| want.is_some() && x[idk].as_i64() == want) else {
        return Ok(ArrState { had_record: false, hash: crate::qbit::Pick::Nothing });
    };
    let id = item["id"].as_i64().unwrap_or(-1);
    let queue = a.get_one("queue?pageSize=2000").await.map_err(|err| format!("{name} didn't answer: {err}"))?;
    let recs = queue["records"].as_array().cloned().ok_or_else(|| format!("{name} didn't say what it's downloading."))?;
    let eps = if movie { vec![] } else { a.list(&format!("episode?seriesId={id}")).await.map_err(|err| format!("{name} didn't list the episodes: {err}"))? };
    Ok(judge_arr(movie, &item, &recs, &eps, e.season, e.episode))
}

/// From what Radarr/Sonarr returned: a queue row for the movie or episode,
/// or it already being wanted (monitored, no file), counts as a record.
fn judge_arr(movie: bool, item: &serde_json::Value, recs: &[serde_json::Value], eps: &[serde_json::Value], season: Option<i64>, episode: Option<i64>) -> ArrState {
    let id = item["id"].as_i64().unwrap_or(-1);
    if movie {
        let mine: Vec<serde_json::Value> = recs.iter().filter(|r| r["movieId"].as_i64() == Some(id)).cloned().collect();
        let wanted = item["monitored"] == true && item["hasFile"] != true;
        let hash = crate::qbit::torrent_hash(&mine, "movieId", id);
        return ArrState { had_record: !mine.is_empty() || wanted, hash };
    }
    let ep = eps.iter().find(|x| x["seasonNumber"].as_i64() == season && x["episodeNumber"].as_i64() == episode);
    let episode_id = ep.and_then(|x| x["id"].as_i64());
    let wanted = ep.is_some_and(|x| x["monitored"] == true && x["hasFile"] != true);
    let mine: Vec<serde_json::Value> = recs.iter().filter(|r| episode_id.is_some() && r["episodeId"].as_i64() == episode_id).cloned().collect();
    let hash = crate::qbit::torrent_hash(&mine, "seriesId", id);
    ArrState { had_record: !mine.is_empty() || wanted, hash }
}

/// Whether the Discover shelf has asked for this title.
fn shelf_wants(app: &tauri::AppHandle, e: &Entry) -> bool {
    use tauri::Manager;
    let (kind, id) = if e.kind == "movie" { ("movie", e.tmdb) } else { ("series", e.tvdb) };
    id.is_some_and(|id| app.state::<crate::discover_shelf::Shelf>().wants(kind, id))
}

fn qb_err(e: crate::qbit::QbError) -> String {
    match e {
        crate::qbit::QbError::Locked => "qBittorrent wants a password. Press Fix everything first.".into(),
        crate::qbit::QbError::Down => "qBittorrent isn't running. Press Fix everything first.".into(),
        crate::qbit::QbError::Other(m) => m,
    }
}

fn paused_state(t: &serde_json::Value) -> bool {
    let s = t["state"].as_str().unwrap_or_default();
    s.starts_with("paused") || s.starts_with("stopped")
}

/// Watch now: asks for the title, waits for its torrent, makes it playable
/// through Jellyfin and returns the tile to play. `tile` is the movie or the
/// show; `season`/`episode` pick the episode.
/// The tile carries the profile it was started for (`profile`), to be passed
/// back on leave.
pub async fn start(app: &tauri::AppHandle, tile: crate::tiles::Tile, season: Option<i64>, episode: Option<i64>) -> Result<crate::tiles::Tile, String> {
    let uid = uid(app)?;
    let key = key_for(&tile, season, episode).ok_or("Chillarr can't tell which title this is.")?;
    let guard = match FLIGHTS.claim(&format!("{uid}|{key}")) {
        Flight::Lead(g) => g,
        Flight::Join(mut rx) => {
            return match rx.wait_for(|o| o.is_some()).await {
                Ok(o) => o.clone().unwrap_or_else(|| Err(TRY_AGAIN.into())),
                Err(_) => Err(TRY_AGAIN.into()),
            };
        }
    };
    let r = start_as(app, &uid, &key, tile, season, episode).await.map(|t| crate::tiles::Tile { profile: Some(uid.clone()), ..t });
    guard.finish(&r);
    r
}

const TRY_AGAIN: &str = "Couldn't get that ready. Try again.";

async fn start_as(app: &tauri::AppHandle, uid: &str, key: &str, tile: crate::tiles::Tile, season: Option<i64>, episode: Option<i64>) -> Result<crate::tiles::Tile, String> {
    use tauri::Manager;
    let (uid, key) = (uid.to_string(), key.to_string());
    let wn = app.state::<WatchNow>();
    emit_state(app, &key, "asking", 0.0);
    let q = crate::waiting::qbit(app).ok_or("qBittorrent isn't set up. Install it first.")?;
    let existing = wn.entry(&uid, &key);
    let fresh = existing.is_none();
    let mut e = match existing {
        Some(e) => e,
        None => {
            let mut e = new_entry(&tile, &key, season, episode);
            if let Some((pos, dur)) = wn.recall(&uid, &key) {
                e.position = pos;
                e.duration = dur;
            }
            e.token = wn.token_for(&key).unwrap_or_else(new_token);
            e
        }
    };
    // Still in qBittorrent from an earlier press: no new request needed.
    let live = match e.hash.as_deref().filter(|h| !h.is_empty()) {
        Some(h) => q.torrent(h).await.ok().flatten().is_some(),
        None => false,
    };
    if !live {
        e.hash = None;
    }
    // Another episode of a season pack Watch now already has: that file is
    // switched on and the pack started, instead of a new search.
    if !live && e.kind == "episode" {
        if let Some(h) = reuse_pack(app, &q, &e).await {
            e.hash = Some(h);
            e.owned = true;
            note(app, wn.upsert(&uid, fresher(&wn, &uid, &e)));
        }
    }
    if !live && e.hash.is_none() {
        let before = arr_state(app, &e).await?;
        if fresh {
            // Another profile's Watch now started it: that ownership carries over.
            let others_own = wn.all().iter().any(|(_, x)| x.key == key && x.owned);
            e.owned = others_own || decide_owned(before.had_record);
        }
        if let crate::qbit::Pick::Hash(h) = &before.hash {
            e.hash = Some(h.clone());
        }
        note(app, wn.upsert(&uid, fresher(&wn, &uid, &e)));
        if e.hash.is_none() {
            let asked = if e.kind == "movie" {
                crate::media::add(app, &tile, "best").await.map(|_| ())
            } else {
                let pick = crate::pick::Pick { seasons: vec![], episodes: vec![(e.season.unwrap_or(0), e.episode.unwrap_or(0))] };
                crate::pick::get_picked(app, tile.clone(), "best", pick).await.map(|_| ())
            };
            if let Err(err) = asked {
                if fresh {
                    drop_entry(app, &uid, &key, &e);
                }
                return Err(err);
            }
        }
        let until = std::time::Instant::now() + FIND_WAIT;
        while e.hash.is_none() {
            emit_state(app, &key, "searching", 0.0);
            match arr_state(app, &e).await.map(|s| s.hash) {
                Ok(crate::qbit::Pick::Hash(h)) => {
                    e.hash = Some(h);
                    // Saved straight away, so the stream tunes this torrent and no other.
                    note(app, wn.upsert(&uid, fresher(&wn, &uid, &e)));
                    break;
                }
                Ok(crate::qbit::Pick::NotTorrent) => {
                    if fresh {
                        drop_entry(app, &uid, &key, &e);
                    }
                    return Err("This copy can't play while it downloads. It'll show up when it's done.".into());
                }
                _ => {}
            }
            if std::time::Instant::now() >= until {
                emit_state(app, &key, "nothing", 0.0);
                // The request stays and shows under On the way, like a Download.
                if fresh {
                    drop_entry(app, &uid, &key, &e);
                }
                return Err(NOTHING_YET.into());
            }
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }
    }
    let hash = e.hash.clone().unwrap_or_default();
    emit_state(app, &key, "starting", 0.0);
    let info = q.torrent(&hash).await.map_err(qb_err)?;
    let pct = info.as_ref().and_then(|t| t["progress"].as_f64()).unwrap_or(0.0) * 100.0;
    if info.as_ref().is_some_and(paused_state) {
        q.start(&hash).await.map_err(qb_err)?;
    }
    emit_state(app, &key, "buffering", pct);
    // From the saved copy: it carries the title's shared token.
    let saved = wn.entry(&uid, &key).unwrap_or(e);
    let jf_id = match ensure_item(app, &uid, &saved).await {
        Ok(id) => id,
        Err(err) => {
            // Nothing plays, so the download pauses as if the player had closed.
            if may_pause(&hash, &wn.all(), shelf_wants(app, &saved)) {
                if let Err(qe) = q.stop(&hash).await {
                    crate::log::log(app, crate::model::Level::Amber, format!("Couldn't pause the Watch now download of {}: {}", saved.title, qb_err(qe)));
                }
            }
            return Err(err);
        }
    };
    note(app, wn.set_watching(&uid, &key, true));
    let e = wn.entry(&uid, &key).unwrap_or(saved);
    emit_state(app, &key, "ready", pct);
    Ok(crate::tiles::Tile { jf_id: Some(jf_id), ..tile_of(&e) })
}

/// `e`'s hash and ownership over the saved entry, so a token shared or a
/// Jellyfin item set by another call meanwhile isn't written back over.
fn fresher(wn: &WatchNow, uid: &str, e: &Entry) -> Entry {
    match wn.entry(uid, &e.key) {
        Some(saved) => merge_fresh(saved, e),
        None => e.clone(),
    }
}

/// The saved entry with what `start` decided: hash and ownership.
fn merge_fresh(saved: Entry, e: &Entry) -> Entry {
    Entry { hash: e.hash.clone(), owned: e.owned, ..saved }
}

/// Takes a never-played entry back out, keeping its position.
fn drop_entry(app: &tauri::AppHandle, uid: &str, key: &str, e: &Entry) {
    use tauri::Manager;
    let wn = app.state::<WatchNow>();
    wn.remove(uid, key);
    note_taken(app);
    if e.position > 0.0 {
        note(app, wn.remember(uid, key, e.position, e.duration));
    }
}

/// The player closed: saves the position and pauses the download if
/// nothing else wants it.
pub async fn leave(app: &tauri::AppHandle, key: &str, position: f64, duration: Option<f64>) -> Result<(), String> {
    leave_as(app, &uid(app)?, key, position, duration).await
}

/// `leave` for the profile that started playing (VLC may close after a
/// profile switch).
pub async fn leave_as(app: &tauri::AppHandle, uid: &str, key: &str, position: f64, duration: Option<f64>) -> Result<(), String> {
    use tauri::Manager;
    let uid = uid.to_string();
    let wn = app.state::<WatchNow>();
    let Some(e) = wn.entry(&uid, key) else {
        return wn.remember(&uid, key, position, duration);
    };
    note(app, wn.set_position(&uid, key, position, duration));
    note(app, wn.set_watching(&uid, key, false));
    // Finished: where you're at moves on now (the rewatch rule is the
    // marker's), and the download goes on to the end so the file reaches
    // the library.
    if let Some((tvdb, spot)) = finished_spot(&e, position, duration) {
        note(app, crate::marker::finished(&wn.dir, &uid, tvdb, spot, now()));
        return Ok(());
    }
    if finished(position, duration.or(e.duration)) {
        return Ok(());
    }
    let Some(hash) = e.hash.clone().filter(|h| !h.is_empty()) else { return Ok(()) };
    if may_pause(&hash, &wn.all(), shelf_wants(app, &e)) {
        let q = crate::waiting::qbit(app).ok_or("qBittorrent isn't set up.")?;
        q.stop(&hash).await.map_err(qb_err)?;
    }
    Ok(())
}

/// While it plays (every 10 s, and VLC's poll): where you are, saved, so a
/// crash or a power cut loses seconds, not the session. Nothing else.
pub fn progress_as(app: &tauri::AppHandle, uid: &str, key: &str, position: f64, duration: Option<f64>) -> Result<(), String> {
    use tauri::Manager;
    if position <= 0.0 {
        return Ok(());
    }
    app.state::<WatchNow>().set_position(uid, key, position, duration)
}

/// Takes a title out of Watch now. Its download goes too when Watch now
/// started it and no other profile has it; the placeholder goes when no
/// profile has the title. Where you were is kept.
pub async fn remove(app: &tauri::AppHandle, key: &str) -> Result<String, String> {
    use tauri::Manager;
    let uid = uid(app)?;
    let wn = app.state::<WatchNow>();
    let e = wn.entry(&uid, key).ok_or("That isn't in Watch now any more.")?;
    // Read before anything is taken out: with qBittorrent or Sonarr down a
    // guess could delete a download someone else wants, or hide one Watch now
    // still owns.
    let plan = match e.hash.clone().filter(|h| e.owned && !h.is_empty()) {
        Some(hash) => removal(app, &uid, &e, &hash).await?.map(|p| (hash, p)),
        None => None,
    };
    wn.remove(&uid, key);
    note_taken(app);
    note(app, wn.remember(&uid, key, e.position, e.duration));
    let name = match &e.subtitle {
        Some(s) if e.kind == "episode" => format!("{} {s}", e.title),
        _ => e.title.clone(),
    };
    let stopped: Result<(), String> = match plan {
        None => Ok(()),
        Some((hash, Removal::Finished)) => drop_finished(app, &e, &hash).await.map(|_| ()),
        Some((hash, Removal::Partial(OnRemove::Delete))) => {
            let which = if e.kind == "movie" { "radarr" } else { "sonarr" };
            crate::waiting::stop_download(app, which, &hash).await.map(|_| ())
        }
        Some((hash, Removal::Partial(OnRemove::Skip(ids)))) => {
            let skipped = match crate::waiting::qbit(app) {
                Some(q) => q.set_file_prio(&hash, &ids, 0).await.map_err(qb_err),
                None => Ok(()),
            };
            // Sonarr stops wanting this one episode; the rest of the pack is someone's Download.
            if let (Ok(()), Some(t), Some(s), Some(n)) = (&skipped, e.tvdb, e.season, e.episode) {
                if let Err(err) = monitor_episode(app, t, s, n, false).await {
                    crate::log::log(app, crate::model::Level::Amber, format!("{name} won't download any more, but Sonarr still lists it as wanted: {err}"));
                }
            }
            skipped
        }
    };
    // The entry is gone either way, so its placeholder goes before any stop error is told.
    if !wn.any_has(key) {
        remove_item(app, &e).await;
    }
    stopped?;
    let msg = format!("Removed {name} from Watch now.");
    crate::log::log(app, crate::model::Level::Ok, &msg);
    Ok(msg)
}

/// What `remove` does to an owned torrent.
enum Removal {
    /// Done downloading: the library's (cleared from qBittorrent, files kept).
    Finished,
    Partial(OnRemove),
}

/// The torrent's fate when this profile's entry goes. None: leave it (another
/// profile is on it, the Discover shelf asked for it, or it's gone). A read
/// that fails is an error, never "finished" or "nobody wants it".
async fn removal(app: &tauri::AppHandle, uid: &str, e: &Entry, hash: &str) -> Result<Option<Removal>, String> {
    use tauri::Manager;
    let wn = app.state::<WatchNow>();
    let others: Vec<(String, Entry)> = wn.all().into_iter().filter(|(u, x)| !(u == uid && x.key == e.key)).collect();
    let shared = others.iter().any(|(_, x)| x.hash.as_deref().is_some_and(|h| h.eq_ignore_ascii_case(hash)));
    if shared || shelf_wants(app, e) {
        return Ok(None);
    }
    let Some(q) = crate::waiting::qbit(app) else { return Ok(None) };
    let fail = |err| format!("Couldn't check the download, so nothing was removed. {}", qb_err(err));
    let Some(t) = q.torrent(hash).await.map_err(fail)? else { return Ok(None) };
    if t["progress"].as_f64().unwrap_or(0.0) >= 1.0 {
        return Ok(Some(Removal::Finished));
    }
    let (Some(tvdb), Some(s), Some(n)) = (e.tvdb.filter(|_| e.kind == "episode"), e.season, e.episode) else {
        return Ok(Some(Removal::Partial(OnRemove::Delete)));
    };
    let files = q.files(hash).await.map_err(fail)?;
    let downloads = downloads_wanted(app, tvdb).await.map_err(|err| format!("Couldn't check what else is downloading, so nothing was removed. {err}"))?;
    let mut mine = pack_wants(hash, Some(tvdb), &others, &[]);
    mine.insert((s, n));
    Ok(Some(Removal::Partial(on_remove(&files, (s, n), &downloads, &mine))))
}

/// A finished Watch now torrent leaves qBittorrent (so it never seeds),
/// its files kept: the library copy is a hardlink to them. One Radarr/Sonarr
/// still list in their queue hasn't been brought in yet, so it stays for them;
/// a trimmed pack Sonarr holds only for episodes nobody wants (`pack_done`)
/// can go. True when it was cleared.
async fn drop_finished(app: &tauri::AppHandle, e: &Entry, hash: &str) -> Result<bool, String> {
    use crate::model::AppId;
    let owner = if e.kind == "movie" { AppId::Radarr } else { AppId::Sonarr };
    let Some(a) = crate::media::arr(app, owner) else { return Ok(false) };
    let Ok(queue) = a.get_one("queue?pageSize=2000").await else { return Ok(false) };
    let Some(rows) = queue["records"].as_array() else { return Ok(false) };
    if rows.iter().any(|r| r["downloadId"].as_str().is_some_and(|d| d.eq_ignore_ascii_case(hash))) && !(owner == AppId::Sonarr && crate::waiting::pack_done(&a, rows, hash).await) {
        return Ok(false);
    }
    let q = crate::waiting::qbit(app).ok_or("qBittorrent isn't set up.")?;
    q.delete(&[hash.to_string()], false).await.map_err(qb_err)?;
    Ok(true)
}

/// A removed profile's titles no other profile has (`WatchNow::forget`):
/// their placeholders go.
pub async fn drop_placeholders(app: &tauri::AppHandle, gone: &[Entry]) {
    for e in gone {
        remove_item(app, e).await;
    }
}

/// Deletes the title's placeholder: Jellyfin's item, then the `.strm` and
/// the folders it leaves empty.
async fn remove_item(app: &tauri::AppHandle, e: &Entry) {
    use tauri::Manager;
    if let (Some(id), Some(j)) = (&e.strm_jf_id, crate::media::jf_admin(app).filter(|j| j.user_id.is_some())) {
        if let Err(err) = j.delete_item(id).await {
            crate::log::log(app, crate::model::Level::Amber, format!("Couldn't take {} out of Jellyfin's Watch now library: {err:?}", e.title));
        }
    }
    let root = app.state::<WatchNow>().dir.join(LIBRARY);
    let Some(path) = strm_path(&root, e) else { return };
    if path.exists() {
        if let Err(err) = std::fs::remove_file(&path) {
            crate::log::log(app, crate::model::Level::Amber, format!("Couldn't delete the Watch now file of {}: {err}", e.title));
            return;
        }
    }
    remove_empty_dirs(&root, &path);
}

/// The folders above `file` that are now empty, up to (not including) the
/// library's own Movies/Series folders.
fn remove_empty_dirs(root: &Path, file: &Path) {
    let keep = [root.join("Movies"), root.join("Series"), root.to_path_buf()];
    let mut dir = file.parent().map(Path::to_path_buf);
    while let Some(d) = dir.filter(|d| d.starts_with(root) && !keep.contains(d)) {
        // remove_dir refuses a folder that still holds anything.
        if std::fs::remove_dir(&d).is_err() {
            break;
        }
        dir = d.parent().map(Path::to_path_buf);
    }
}

/// This profile's Watch now titles, with how far each download is.
pub async fn list(app: &tauri::AppHandle) -> Vec<crate::tiles::Tile> {
    use tauri::Manager;
    let Ok(uid) = uid(app) else { return vec![] };
    let entries = newest_first(app.state::<WatchNow>().entries(&uid));
    let q = crate::waiting::qbit(app);
    let mut out = Vec::new();
    for e in entries {
        let info = match (&q, e.hash.as_deref().filter(|h| !h.is_empty())) {
            (Some(q), Some(h)) => q.torrent(h).await.ok().flatten(),
            _ => None,
        };
        // The file it plays, not the whole torrent (a season pack is mostly skipped).
        let file = match (&q, &info, e.hash.as_deref()) {
            (Some(q), Some(_), Some(h)) => q.files(h).await.ok().and_then(|f| chosen_progress(&f, &e)),
            _ => None,
        };
        let progress = file.or_else(|| info.as_ref().and_then(|t| t["progress"].as_f64())).unwrap_or(0.0);
        let paused = info.as_ref().is_some_and(paused_state);
        let mut t = tile_of(&e);
        t.status = Some(status(progress, paused, progress >= 1.0));
        // Unknown file: nothing drawn as downloaded, so a seek shows the spinner rather than promising data.
        t.downloaded = info.is_some().then_some(file.unwrap_or(0.0));
        out.push(t);
    }
    out
}

/// Each owned torrent once, with an entry on it (for its title).
fn owned_hashes(all: &[(String, Entry)]) -> Vec<(String, Entry)> {
    let mut out: Vec<(String, Entry)> = Vec::new();
    for (_, e) in all.iter().filter(|(_, e)| e.owned) {
        if let Some(h) = e.hash.as_deref().filter(|h| !h.is_empty()).map(str::to_ascii_lowercase) {
            if !out.iter().any(|(x, _)| *x == h) {
                out.push((h, e.clone()));
            }
        }
    }
    out
}

/// On launch: the player isn't open, so nobody is watching, and Watch now's
/// own downloads that nothing else wants are paused. qBittorrent may start
/// after Chillarr, so it's waited for (up to 10 minutes).
pub fn spawn(app: tauri::AppHandle) {
    use tauri::Manager;
    note(&app, app.state::<WatchNow>().clear_watching());
    tauri::async_runtime::spawn(async move {
        if owned_hashes(&app.state::<WatchNow>().all()).is_empty() {
            return;
        }
        for _ in 0..20 {
            if let Some(q) = crate::waiting::qbit(&app) {
                if q.list().await.is_ok() {
                    // Read now, not before the wait: a Watch now pressed meanwhile is being watched.
                    let all = app.state::<WatchNow>().all();
                    for (h, e) in &owned_hashes(&all) {
                        if may_pause(h, &all, shelf_wants(&app, e)) {
                            if let Err(err) = q.stop(h).await {
                                crate::log::log(&app, crate::model::Level::Amber, format!("Couldn't pause the Watch now download of {}: {}", e.title, qb_err(err)));
                            }
                        }
                    }
                    return;
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        }
    });
}

// ---------- Season packs ----------

/// Sonarr's episodes of a show, by TVDB id. Empty when Sonarr doesn't have
/// the show; an error when it can't be read (never taken as "nothing wanted").
async fn sonarr_episodes(app: &tauri::AppHandle, tvdb: i64) -> Result<Vec<serde_json::Value>, String> {
    let a = crate::media::arr(app, crate::model::AppId::Sonarr).ok_or("Sonarr isn't set up")?;
    let shows = a.list("series").await.map_err(|err| format!("Sonarr didn't answer: {err}"))?;
    let Some(id) = shows.iter().find(|x| x["tvdbId"].as_i64() == Some(tvdb)).and_then(|x| x["id"].as_i64()) else { return Ok(vec![]) };
    a.list(&format!("episode?seriesId={id}")).await.map_err(|err| format!("Sonarr didn't list the episodes: {err}"))
}

/// The episodes of a show a normal Download wants: monitored, no file yet.
async fn downloads_wanted(app: &tauri::AppHandle, tvdb: i64) -> Result<Vec<(i64, i64)>, String> {
    let eps = sonarr_episodes(app, tvdb).await?;
    Ok(eps.iter().filter(|e| e["monitored"] == true && e["hasFile"] != true).filter_map(|e| Some((e["seasonNumber"].as_i64()?, e["episodeNumber"].as_i64()?))).collect())
}

/// Hashes whose files were last tuned at stream start, and when: the stream
/// asks again every couple of seconds while it waits, Sonarr only once a minute.
static TUNED: Mutex<Option<HashMap<String, std::time::Instant>>> = Mutex::new(None);

/// At stream start on a pack Watch now owns: the episode's file is switched
/// on and every other video skipped, except what other profiles or a normal
/// Download still want (`pack_wants`). If Sonarr can't be read, nothing is
/// skipped.
pub(crate) async fn tune_pack(app: &tauri::AppHandle, q: &crate::qbit::Qbit, hash: &str, tvdb: i64, files: &[serde_json::Value], chosen: &serde_json::Value) {
    use tauri::Manager;
    let (skip, on) = pack_prios(files, chosen, &HashSet::new());
    if let Err(err) = q.set_file_prio(hash, &on, 1).await {
        crate::log::log(app, crate::model::Level::Amber, format!("Couldn't switch on the episode Watch now is playing: {}", qb_err(err)));
    }
    if skip.is_empty() {
        return;
    }
    let due = {
        let mut t = TUNED.lock().unwrap_or_else(|e| e.into_inner());
        let t = t.get_or_insert_with(HashMap::new);
        let key = hash.to_ascii_lowercase();
        let due = t.get(&key).is_none_or(|at| at.elapsed() > std::time::Duration::from_secs(60));
        if due {
            t.insert(key, std::time::Instant::now());
        }
        due
    };
    if !due {
        return;
    }
    let step = format!("tune:{}", hash.to_ascii_lowercase());
    let downloads = match downloads_wanted(app, tvdb).await {
        Ok(d) => d,
        Err(err) => {
            log_once(app, &step, Some(format!("Watch now didn't skip the rest of the season pack: {err}")));
            return;
        }
    };
    log_once(app, &step, None);
    let wants = pack_wants(hash, Some(tvdb), &app.state::<WatchNow>().all(), &downloads);
    let (skip, on) = pack_prios(files, chosen, &wants);
    for (ids, prio) in [(on, 1), (skip, 0)] {
        if let Err(err) = q.set_file_prio(hash, &ids, prio).await {
            crate::log::log(app, crate::model::Level::Amber, format!("Couldn't change which files of a season pack download: {}", qb_err(err)));
        }
    }
}

/// A season pack Watch now owns that holds this episode: its file is
/// switched on, the episode marked wanted in Sonarr (no search, so it's
/// brought into the library from the pack), and the pack's hash returned.
async fn reuse_pack(app: &tauri::AppHandle, q: &crate::qbit::Qbit, e: &Entry) -> Option<String> {
    use tauri::Manager;
    let (tvdb, season, episode) = (e.tvdb?, e.season?, e.episode?);
    let packs: Vec<Owned> = app.state::<WatchNow>().owned_torrents().into_iter().filter(|o| o.kind == "episode" && o.tvdb == Some(tvdb)).collect();
    for o in packs {
        let Ok(files) = q.files(&o.hash).await else { continue };
        let Some(f) = crate::qbit::pick_episode(&files, season, episode) else { continue };
        if f["priority"].as_i64() == Some(0) {
            if let Some(id) = f["index"].as_i64() {
                if let Err(err) = q.set_file_prio(&o.hash, &[id], 1).await {
                    crate::log::log(app, crate::model::Level::Amber, format!("Couldn't switch on {} in its season pack: {}", e.title, qb_err(err)));
                    continue;
                }
            }
        }
        if let Err(err) = monitor_episode(app, tvdb, season, episode, true).await {
            crate::log::log(app, crate::model::Level::Amber, format!("{} plays from the season pack, but Sonarr wasn't told to keep it: {err}", e.title));
        }
        return Some(o.hash);
    }
    None
}

async fn monitor_episode(app: &tauri::AppHandle, tvdb: i64, season: i64, episode: i64, on: bool) -> Result<(), String> {
    let eps = sonarr_episodes(app, tvdb).await?;
    let ep = eps.iter().find(|x| x["seasonNumber"].as_i64() == Some(season) && x["episodeNumber"].as_i64() == Some(episode)).ok_or("Sonarr doesn't list that episode")?;
    if (ep["monitored"] == true) == on {
        return Ok(());
    }
    let id = ep["id"].as_i64().ok_or("Sonarr doesn't list that episode")?;
    let a = crate::media::arr(app, crate::model::AppId::Sonarr).ok_or("Sonarr isn't set up")?;
    a.update("episode/monitor", &serde_json::json!({ "episodeIds": [id], "monitored": on })).await.map_err(|err| format!("Sonarr didn't answer: {err}"))
}

// ---------- Every couple of minutes ----------

/// Hands finished titles over to the library and clears Watch now's
/// torrents once nothing wants them. Run from `waiting`'s loop. A step that
/// fails is logged and the rest still run.
pub async fn tick(app: &tauri::AppHandle) {
    use tauri::Manager;
    note(app, app.state::<WatchNow>().keep_owned());
    handover(app).await;
    tidy_torrents(app).await;
}

/// Each profile's lists from Jellyfin, read once per tick.
type Lists = HashMap<String, Option<Vec<serde_json::Value>>>;

async fn read_list<'c>(app: &tauri::AppHandle, cache: &'c mut Lists, uid: &str, j: &crate::jellyfin::Jf, query: &str) -> Option<&'c Vec<serde_json::Value>> {
    let k = format!("{uid}|{query}");
    if !cache.contains_key(&k) {
        let step = format!("library:{uid}");
        let got = match j.watch_items(query).await {
            Ok(v) => {
                log_once(app, &step, None);
                Some(v)
            }
            Err(err) => {
                log_once(app, &step, Some(format!("Watch now couldn't look in your library: {err}")));
                None
            }
        };
        cache.insert(k.clone(), got);
    }
    cache.get(&k)?.as_ref()
}

/// The library's copy of the entry's title as this profile sees it, with
/// this profile's play data.
async fn real_item(app: &tauri::AppHandle, cache: &mut Lists, uid: &str, j: &crate::jellyfin::Jf, e: &Entry) -> Option<serde_json::Value> {
    match e.kind.as_str() {
        "movie" => real_movie(read_list(app, cache, uid, j, "includeItemTypes=Movie").await?, e.tmdb?).cloned(),
        "episode" => {
            let series = real_series(read_list(app, cache, uid, j, "includeItemTypes=Series").await?, e.tvdb?)?.to_string();
            real_episode(read_list(app, cache, uid, j, &format!("parentId={series}&includeItemTypes=Episode")).await?, e.season?, e.episode?).cloned()
        }
        _ => None,
    }
}

/// For every title whose real file is now in the library and that nobody
/// is playing: each profile's position moves to the library copy (as that
/// profile), its entry goes, and the placeholder goes once no profile has it.
async fn handover(app: &tauri::AppHandle) {
    use tauri::Manager;
    let wn = app.state::<WatchNow>();
    let all = wn.all();
    let mut keys: Vec<String> = all.iter().map(|(_, e)| e.key.clone()).collect();
    keys.sort();
    keys.dedup();
    if keys.is_empty() {
        return;
    }
    let jfs = crate::profiles::all_jf(app);
    let mut cache = Lists::new();
    for key in keys {
        let holders: Vec<&(String, Entry)> = all.iter().filter(|(_, e)| e.key == key).collect();
        // Review focus 3: never under a player that has it open, whoever's it is.
        if holders.iter().any(|(_, e)| e.watching) {
            continue;
        }
        let mut last: Option<Entry> = None;
        for (uid, e) in holders.iter().copied() {
            // A profile that isn't signed in keeps its entry until it is.
            let Some((_, j)) = jfs.iter().find(|(u, _)| u == uid) else { continue };
            let real = real_item(app, &mut cache, uid, j, e).await;
            if !ready_for_handover(e, real.is_some()) {
                continue;
            }
            let real = real.unwrap_or_default();
            let Some(id) = real["Id"].as_str() else { continue };
            // Read again: a player may have opened since the snapshot.
            if wn.all().iter().any(|(_, x)| x.key == key && x.watching) {
                break;
            }
            let played = real["UserData"]["Played"] == true;
            let at = crate::jellyfin::ticks_to_seconds(real["UserData"]["PlaybackPositionTicks"].as_i64().unwrap_or(0));
            let wrote = if played {
                Ok(())
            } else if finished(e.position, e.duration) {
                j.set_user_data(id, &played_body(e.updated_at)).await
            } else {
                match position_to_write(e.position, at) {
                    // With when it was played, so Continue Watching sorts it there.
                    Some(p) if e.updated_at > 0 => j.set_user_data(id, &position_body(p, e.updated_at)).await,
                    Some(p) => j.set_position(id, p).await,
                    None => Ok(()),
                }
            };
            if let Err(err) = wrote {
                // The entry stays, so the next tick tries again and nothing is lost.
                crate::log::log(app, crate::model::Level::Amber, format!("Couldn't carry where you were in {} over to your library: {err}", e.title));
                continue;
            }
            wn.remove(uid, &key);
            note_taken(app);
            // The library copy holds the position now; a later Watch now starts from its own.
            note(app, wn.forget_position(uid, &key));
            let mut gone = e.clone();
            gone.strm_jf_id = gone.strm_jf_id.or_else(|| holders.iter().find_map(|(_, x)| x.strm_jf_id.clone()));
            last = Some(gone);
        }
        // The `.strm` is shared: it goes only when no profile still has the title.
        if let Some(e) = last {
            if !wn.any_has(&key) {
                remove_item(app, &e).await;
                let name = match &e.subtitle {
                    Some(s) if e.kind == "episode" => format!("{} {s}", e.title),
                    _ => e.title.clone(),
                };
                crate::log::log(app, crate::model::Level::Ok, format!("{name} is in your library now."));
            }
        }
    }
}

/// Watch now's torrents never seed: a finished one is stopped, and cleared
/// from qBittorrent (files kept, the library copy is a hardlink) once it's
/// imported and nothing wants it: no entry on it, no Discover shelf request,
/// no file of a pack wanted (spec call 2). Watch now alone sets an owned
/// pack's file priorities, from `pack_wants`.
async fn tidy_torrents(app: &tauri::AppHandle) {
    use tauri::Manager;
    let wn = app.state::<WatchNow>();
    let owned = wn.owned_torrents();
    if owned.is_empty() {
        return;
    }
    let Some(q) = crate::waiting::qbit(app) else { return };
    let all = wn.all();
    let amber = |m: String| crate::log::log(app, crate::model::Level::Amber, m);
    for o in owned {
        let info = match q.torrent(&o.hash).await {
            Ok(Some(t)) => t,
            Ok(None) => {
                note(app, wn.forget_torrent(&o.hash));
                continue;
            }
            Err(err) => {
                log_once(app, "qbit", Some(format!("Watch now couldn't check its downloads: {}", qb_err(err))));
                return;
            }
        };
        log_once(app, "qbit", None);
        let e = o.as_entry();
        let on_it: Vec<&Entry> = all.iter().map(|(_, x)| x).filter(|x| x.hash.as_deref().is_some_and(|h| h.eq_ignore_ascii_case(&o.hash))).collect();
        let watched = on_it.iter().any(|x| x.watching);
        let shelf = shelf_wants(app, &e);
        let mut complete = info["progress"].as_f64().unwrap_or(0.0) >= 1.0;
        let paused = paused_state(&info);
        let mut kept = !on_it.is_empty() || shelf;
        // A file a normal Download still needs keeps the torrent going.
        let mut needed = false;
        if o.kind == "episode" {
            let Some(tvdb) = o.tvdb else { continue };
            let files = match q.files(&o.hash).await {
                Ok(f) => f,
                Err(err) => {
                    amber(format!("Watch now couldn't read the files of {}: {}", o.title, qb_err(err)));
                    continue;
                }
            };
            // No file list yet (metadata still coming): nothing to judge.
            if files.is_empty() {
                continue;
            }
            let step = format!("pack:{}", o.hash);
            let downloads = match downloads_wanted(app, tvdb).await {
                Ok(d) => d,
                Err(err) => {
                    log_once(app, &step, Some(format!("Watch now left the season pack of {} as it is: {err}", o.title)));
                    continue;
                }
            };
            log_once(app, &step, None);
            let wants = pack_wants(&o.hash, Some(tvdb), &all, &downloads);
            let mut plan = pack_plan(&files, &wants, None);
            // Titles on it, but no file name says which episode it is: hands off,
            // rather than skip the very file one of them is playing.
            if !on_it.is_empty() && !pack_still_wanted(&plan.wanted) {
                plan.skip.clear();
            }
            for (ids, prio) in [(&plan.restore, 1), (&plan.skip, 0)] {
                if let Err(err) = q.set_file_prio(&o.hash, ids, prio).await {
                    amber(format!("Couldn't change which files of {} download: {}", o.title, qb_err(err)));
                }
            }
            // From the files: a pack with nothing left wanted reads 0% in qBittorrent.
            complete = pack_complete(&files, &plan);
            kept |= pack_still_wanted(&plan.wanted) || marker_keeps(&files, &crate::marker::spots_in(&wn.dir, tvdb));
            // Watch now's own episodes don't count: leaving the player paused those on purpose.
            let mine = pack_wants(&o.hash, Some(tvdb), &all, &[]);
            needed = files.iter().any(|f| f["progress"].as_f64().unwrap_or(0.0) < 1.0 && file_episodes(f).iter().any(|x| downloads.contains(x) && !mine.contains(x)));
        }
        if needed && paused {
            if let Err(err) = q.start(&o.hash).await {
                amber(format!("Couldn't start the download of {}: {}", o.title, qb_err(err)));
            }
            continue;
        }
        // Never seeds. A Discover shelf request keeps the shelf's rules (spec call 5).
        if complete && !paused && !watched && !shelf {
            if let Err(err) = q.stop(&o.hash).await {
                amber(format!("Couldn't stop {} sharing: {}", o.title, qb_err(err)));
            }
        }
        if !kept && complete {
            match drop_finished(app, &e, &o.hash).await {
                Ok(true) => {
                    note(app, wn.forget_torrent(&o.hash));
                    crate::log::log(app, crate::model::Level::Ok, format!("Cleared {} from Downloads: it's in your library now.", o.title));
                }
                Ok(false) => {}
                Err(err) => amber(format!("Couldn't clear {} from Downloads: {err}", o.title)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: &str, hash: Option<&str>, owned: bool, watching: bool) -> (String, Entry) {
        ("u".into(), Entry { key: key.into(), hash: hash.map(Into::into), owned, watching, token: new_token(), ..Default::default() })
    }

    #[test]
    fn entries_survive_reload() {
        let dir = tempfile::tempdir().unwrap();
        let e = Entry { key: "movie:1".into(), kind: "movie".into(), title: "A".into(), position: 12.5, token: "t".into(), ..Default::default() };
        WatchNow::load(dir.path().to_path_buf()).upsert("a", e.clone()).unwrap();
        let again = WatchNow::load(dir.path().to_path_buf());
        assert_eq!(again.entries("a"), vec![e.clone()]);
        assert_eq!(again.by_token("t"), Some(("a".to_string(), e)));
    }

    #[test]
    fn profiles_are_separate() {
        let dir = tempfile::tempdir().unwrap();
        let w = WatchNow::load(dir.path().to_path_buf());
        w.upsert("a", Entry { key: "movie:1".into(), ..Default::default() }).unwrap();
        assert!(w.entries("b").is_empty());
        assert_eq!(w.entries("a").len(), 1);
    }

    #[test]
    fn may_pause_only_owned_unwatched_unwanted() {
        let owned = vec![entry("movie:1", Some("h"), true, false)];
        assert!(may_pause("h", &owned, false));
        assert!(!may_pause("h", &owned, true));
        let watched = vec![entry("movie:1", Some("h"), true, false), entry("movie:2", Some("h"), false, true)];
        assert!(!may_pause("h", &watched, false));
        let not_owned = vec![entry("movie:1", Some("h"), false, false)];
        assert!(!may_pause("h", &not_owned, false));
        assert!(!may_pause("other", &owned, false));
    }

    #[test]
    fn key_for_movie_and_episode() {
        let mut m = crate::tiles::Tile::new("tmdb:27205", "movie", "Inception");
        assert_eq!(key_for(&m, None, None), None);
        m.tmdb = Some(27205);
        assert_eq!(key_for(&m, None, None).as_deref(), Some("movie:27205"));
        let mut s = crate::tiles::Tile::new("tvdb:81189", "series", "BB");
        assert_eq!(key_for(&s, Some(2), Some(3)), None);
        s.tvdb = Some(81189);
        assert_eq!(key_for(&s, Some(2), Some(3)).as_deref(), Some("ep:81189:2:3"));
    }

    #[test]
    fn key_for_anime_and_episode_tiles() {
        let mut film = crate::tiles::Tile::new("anilist:1", "anime", "Film");
        film.format = Some("MOVIE".into());
        film.tmdb = Some(55);
        film.tvdb = Some(9);
        assert_eq!(key_for(&film, Some(1), Some(1)).as_deref(), Some("movie:55"));
        let mut show = crate::tiles::Tile::new("anilist:2", "anime", "Show");
        show.format = Some("TV".into());
        show.tmdb = Some(56);
        show.tvdb = Some(10);
        assert_eq!(key_for(&show, Some(1), Some(4)).as_deref(), Some("ep:10:1:4"));
        assert_eq!(key_for(&show, None, None), None);
        let mut ep = crate::tiles::Tile::new("tvdb:7", "episode", "Ep");
        ep.tvdb = Some(7);
        assert_eq!(key_for(&ep, Some(2), Some(3)).as_deref(), Some("ep:7:2:3"));
    }

    #[test]
    fn forget_drops_cache_and_file() {
        let dir = tempfile::tempdir().unwrap();
        let w = WatchNow::load(dir.path().to_path_buf());
        w.upsert("a", Entry { key: "movie:1".into(), token: "t".into(), ..Default::default() }).unwrap();
        assert!(file(dir.path(), "a").exists());
        assert_eq!(w.forget("a").len(), 1);
        assert!(w.by_token("t").is_none());
        assert!(w.entries("a").is_empty());
        assert!(!file(dir.path(), "a").exists());
    }

    #[test]
    fn old_file_missing_fields_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(file(dir.path(), "a"), r#"{"entries":[{"key":"movie:1","title":"A","token":"t"}]}"#).unwrap();
        let w = WatchNow::load(dir.path().to_path_buf());
        let e = w.entries("a");
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].title, "A");
        assert_eq!(w.by_token("t").unwrap().0, "a");
    }

    #[test]
    fn unreadable_file_is_set_aside_not_wiped() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(file(dir.path(), "a"), "not json").unwrap();
        let w = WatchNow::load(dir.path().to_path_buf());
        assert!(w.entries("a").is_empty());
        assert!(dir.path().join("watchnow-a.json.bad").exists());
    }

    #[test]
    fn save_failure_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        // A file where the folder should be makes every save fail.
        let blocker = dir.path().join("x");
        std::fs::write(&blocker, "").unwrap();
        let w = WatchNow::load(blocker);
        assert!(w.upsert("a", Entry::default()).is_err());
        assert!(w.take_error().is_some());
    }

    fn movie() -> Entry {
        Entry { key: "movie:27205".into(), kind: "movie".into(), title: "Inception".into(), tmdb: Some(27205), token: "tok".into(), ..Default::default() }
    }

    fn episode() -> Entry {
        Entry { key: "ep:81189:2:3".into(), kind: "episode".into(), title: "Breaking Bad".into(), tvdb: Some(81189), season: Some(2), episode: Some(3), token: "tok2".into(), ..Default::default() }
    }

    #[test]
    fn strm_points_at_localhost() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_strm(dir.path(), &movie(), 8690, "sec").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("http://localhost:"), "{text}");
        assert!(text.ends_with("/w/sec/tok"), "{text}");
        assert_eq!(text, "http://localhost:8690/w/sec/tok");
        assert!(path.starts_with(dir.path()));
        assert!(write_strm(dir.path(), &Entry { token: String::new(), ..movie() }, 8690, "sec").is_err());
    }

    #[test]
    fn strm_paths_follow_jellyfin_naming() {
        let root = Path::new(r"C:\data\Chillarr Watch now");
        let m = strm_path(root, &movie()).unwrap();
        assert_eq!(m, root.join("Movies").join("Inception [tmdbid-27205]").join("Inception [tmdbid-27205].strm"));
        let e = strm_path(root, &episode()).unwrap();
        assert_eq!(e, root.join("Series").join("Breaking Bad [tvdbid-81189]").join("Season 02").join("Breaking Bad S02E03.strm"));
        assert!(strm_path(root, &Entry { tmdb: None, ..movie() }).is_none());
        assert!(strm_path(root, &Entry { episode: None, ..episode() }).is_none());
        assert!(strm_path(root, &Entry { kind: "other".into(), ..movie() }).is_none());
    }

    #[test]
    fn source_wanted_by_kind() {
        assert!(matches!(wanted(&movie()), Some(("movie", 27205, crate::discover_shelf::Want::Movie))));
        assert!(matches!(wanted(&episode()), Some(("series", 81189, crate::discover_shelf::Want::Episode(2, 3)))));
        assert!(wanted(&Entry { tvdb: None, ..episode() }).is_none());
    }

    #[test]
    fn item_found_by_path() {
        let items = vec![
            serde_json::json!({ "Id": "a", "Path": r"C:\x\Other.strm" }),
            serde_json::json!({ "Id": "b", "Path": "c:/data/chillarr watch now/movies/inception/inception.strm" }),
        ];
        assert_eq!(item_with_path(&items, Path::new(r"C:\data\Chillarr Watch now\Movies\Inception\Inception.strm")).as_deref(), Some("b"));
        assert!(item_with_path(&items, Path::new(r"C:\nope.strm")).is_none());
        let libs = vec![
            serde_json::json!({ "Name": "Movies", "ItemId": "m", "Locations": [r"D:\Movies"] }),
            serde_json::json!({ "Name": "Chillarr Watch now Movies", "ItemId": "w", "Locations": [r"C:\data\Chillarr Watch now\Movies\"] }),
        ];
        assert_eq!(library_with_path(&libs, Path::new(r"C:\data\Chillarr Watch now\Movies")).as_deref(), Some("w"));
        assert!(library_with_path(&libs, Path::new(r"C:\data\Elsewhere")).is_none());
    }

    #[test]
    fn a_pack_keeps_only_the_episode() {
        let files = vec![
            serde_json::json!({ "index": 0, "name": "P/S02E01.mkv", "priority": 1 }),
            serde_json::json!({ "index": 1, "name": "P/S02E03.mkv", "priority": 0 }),
            serde_json::json!({ "index": 2, "name": "P/S02E04.mkv", "priority": 0 }),
            serde_json::json!({ "index": 3, "name": "P/info.nfo", "priority": 1 }),
            serde_json::json!({ "index": 4, "name": "P/S02E05.mp4", "priority": 6 }),
        ];
        let (skip, keep) = pack_prios(&files, &files[1], &HashSet::new());
        assert_eq!(skip, vec![0, 4]);
        assert_eq!(keep, vec![1]);
        // Already set: nothing to ask qBittorrent.
        let done = vec![serde_json::json!({ "index": 0, "name": "S02E03.mkv", "priority": 1 })];
        assert_eq!(pack_prios(&done, &done[0], &HashSet::new()), (vec![], vec![]));
    }

    #[test]
    fn only_an_owned_torrent_is_changed() {
        let mine = Entry { hash: Some("ABC".into()), owned: true, ..episode() };
        let l = lookup_for(&mine);
        assert!(l.tune);
        assert_eq!(l.hash.as_deref(), Some("ABC"));
        let theirs = Entry { hash: Some("abc".into()), owned: false, ..episode() };
        assert!(!lookup_for(&theirs).tune);
        let unknown = Entry { hash: None, owned: false, ..movie() };
        let l = lookup_for(&unknown);
        assert!(!l.tune && l.hash.is_none());
        assert!(lookup_for(&Entry { hash: Some(String::new()), ..movie() }).hash.is_none());
    }

    #[test]
    fn one_token_per_title_across_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let w = WatchNow::load(dir.path().to_path_buf());
        assert_eq!(w.token_for("movie:27205"), None);
        w.upsert("b", Entry { token: "tb".into(), ..movie() }).unwrap();
        w.upsert("a", Entry { token: "ta".into(), ..movie() }).unwrap();
        w.upsert("a", episode()).unwrap();
        // The first profile by id wins, whatever order they were added in.
        assert_eq!(w.token_for("movie:27205").as_deref(), Some("ta"));
        assert_eq!(w.share_token("movie:27205", "new").unwrap(), "ta");
        assert_eq!(w.by_token("tb"), None);
        assert!(w.entries("b").iter().all(|e| e.token == "ta"));
        // Saved, not just in memory.
        let again = WatchNow::load(dir.path().to_path_buf());
        assert_eq!(again.entries("b")[0].token, "ta");
        // Other titles keep theirs; a title nobody has gets the fallback.
        assert_eq!(w.token_for("ep:81189:2:3").as_deref(), Some("tok2"));
        assert_eq!(w.share_token("movie:1", "fresh").unwrap(), "fresh");
        assert!(w.any_has("movie:27205"));
        w.remove("a", "movie:27205");
        assert!(w.any_has("movie:27205"));
        w.remove("b", "movie:27205");
        assert!(!w.any_has("movie:27205"));
    }

    #[test]
    fn watch_now_libraries_are_known_by_name() {
        assert!(is_library("Chillarr Watch now Movies"));
        assert!(is_library("Chillarr Watch now Series"));
        assert!(!is_library("Movies"));
    }

    #[test]
    fn tokens_differ_and_are_hex() {
        let (a, b) = (new_token(), new_token());
        assert_ne!(a, b);
        assert!(a.len() == 32 && a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn owned_only_when_not_already_queued() {
        assert!(decide_owned(false));
        assert!(!decide_owned(true));
    }

    #[test]
    fn status_text() {
        assert_eq!(status(0.42, false, false), "Downloading 42%");
        assert_eq!(status(0.42, true, false), "Paused, 42% downloaded");
        assert_eq!(status(1.0, true, true), "Ready");
        assert_eq!(status(0.999, false, false), "Downloading 99%");
    }

    #[test]
    fn positions_outlive_the_entry() {
        let dir = tempfile::tempdir().unwrap();
        let w = WatchNow::load(dir.path().to_path_buf());
        assert_eq!(w.recall("a", "movie:1"), None);
        w.remember("a", "movie:1", 61.5, Some(5400.0)).unwrap();
        assert_eq!(WatchNow::load(dir.path().to_path_buf()).recall("a", "movie:1"), Some((61.5, Some(5400.0))));
        assert_eq!(w.recall("b", "movie:1"), None);
    }

    #[test]
    fn clearing_watching_covers_every_profile() {
        let dir = tempfile::tempdir().unwrap();
        let w = WatchNow::load(dir.path().to_path_buf());
        w.upsert("a", Entry { watching: true, ..movie() }).unwrap();
        w.upsert("b", Entry { watching: true, ..episode() }).unwrap();
        w.clear_watching().unwrap();
        assert!(w.all().iter().all(|(_, e)| !e.watching));
        assert_eq!(w.all().len(), 2);
    }

    #[test]
    fn an_episode_entry_carries_the_show_name() {
        let mut show = crate::tiles::Tile::new("tvdb:81189", "series", "Breaking Bad");
        show.tvdb = Some(81189);
        let e = new_entry(&show, "ep:81189:2:3", Some(2), Some(3));
        assert_eq!((e.kind.as_str(), e.title.as_str(), e.subtitle.as_deref()), ("episode", "Breaking Bad", Some("S2 E3")));
        let mut m = crate::tiles::Tile::new("tmdb:1", "movie", "Inception");
        m.tmdb = Some(1);
        let e = new_entry(&m, "movie:1", None, None);
        assert_eq!((e.kind.as_str(), e.title.as_str(), e.subtitle), ("movie", "Inception", None));
    }

    #[test]
    fn removing_a_placeholder_leaves_the_library_folders() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(LIBRARY);
        let a = write_strm(&root, &episode(), 1, "s").unwrap();
        let b = write_strm(&root, &Entry { key: "ep:81189:2:4".into(), episode: Some(4), ..episode() }, 1, "s").unwrap();
        std::fs::remove_file(&a).unwrap();
        remove_empty_dirs(&root, &a);
        assert!(b.exists());
        std::fs::remove_file(&b).unwrap();
        remove_empty_dirs(&root, &b);
        assert!(!root.join("Series").join("Breaking Bad [tvdbid-81189]").exists());
        assert!(root.join("Series").exists());
    }

    #[test]
    fn each_owned_torrent_is_listed_once() {
        let all = vec![entry("ep:1:1:1", Some("AB"), true, false), entry("ep:1:1:2", Some("ab"), true, false), entry("movie:2", Some("cd"), false, false)];
        let h: Vec<String> = owned_hashes(&all).into_iter().map(|(h, _)| h).collect();
        assert_eq!(h, vec!["ab".to_string()]);
    }

    #[test]
    fn a_queued_or_wanted_title_is_not_owned() {
        use serde_json::json;
        let movie = json!({ "id": 7, "monitored": true, "hasFile": false });
        let row = json!({ "movieId": 7, "protocol": "torrent", "downloadId": "ABC" });
        let s = judge_arr(true, &movie, &[row], &[], None, None);
        assert_eq!(s, ArrState { had_record: true, hash: crate::qbit::Pick::Hash("abc".into()) });
        // Still searching (monitored, no file, nothing queued): someone's Download.
        assert!(judge_arr(true, &movie, &[], &[], None, None).had_record);
        let idle = json!({ "id": 7, "monitored": false, "hasFile": false });
        assert!(!judge_arr(true, &idle, &[], &[], None, None).had_record);
        let show = json!({ "id": 3 });
        let eps = vec![
            json!({ "id": 31, "seasonNumber": 1, "episodeNumber": 2, "monitored": false, "hasFile": false }),
            json!({ "id": 32, "seasonNumber": 1, "episodeNumber": 3, "monitored": true, "hasFile": false }),
        ];
        let other = json!({ "seriesId": 3, "episodeId": 32, "downloadId": "X" });
        // Another episode's download doesn't make this one taken.
        assert_eq!(judge_arr(false, &show, &[other.clone()], &eps, Some(1), Some(2)), ArrState { had_record: false, hash: crate::qbit::Pick::Nothing });
        assert!(judge_arr(false, &show, &[other], &eps, Some(1), Some(3)).had_record);
    }

    #[test]
    fn a_fresher_copy_keeps_what_others_saved() {
        let saved = Entry { token: "shared".into(), strm_jf_id: Some("jf".into()), ..movie() };
        let mine = Entry { token: "old".into(), hash: Some("h".into()), owned: true, ..movie() };
        let m = merge_fresh(saved, &mine);
        assert_eq!((m.token.as_str(), m.strm_jf_id.as_deref(), m.hash.as_deref(), m.owned), ("shared", Some("jf"), Some("h"), true));
    }

    #[test]
    fn handover_waits_while_watching() {
        assert!(!ready_for_handover(&Entry { watching: true, ..movie() }, true));
        assert!(!ready_for_handover(&movie(), false));
        assert!(ready_for_handover(&movie(), true));
    }

    #[test]
    fn later_position_wins() {
        assert_eq!(position_to_write(600.0, 120.0), Some(600.0));
        // The library copy was played further on another device: left alone.
        assert_eq!(position_to_write(120.0, 600.0), None);
        assert_eq!(position_to_write(300.0, 300.0), None);
        // Never started: nothing to carry over.
        assert_eq!(position_to_write(0.0, 0.0), None);
    }

    #[test]
    fn pack_kept_while_wanted() {
        assert!(pack_still_wanted(&[false, true, false]));
        assert!(!pack_still_wanted(&[false, false]));
        assert!(!pack_still_wanted(&[]));
    }

    #[test]
    fn finished_means_ninety_percent() {
        assert!(finished(95.0, Some(100.0)));
        assert!(!finished(50.0, Some(100.0)));
        assert!(!finished(50.0, None));
    }

    fn ep_on(uid: &str, s: i64, n: i64, hash: &str) -> (String, Entry) {
        (uid.into(), Entry { key: format!("ep:81189:{s}:{n}"), season: Some(s), episode: Some(n), hash: Some(hash.into()), owned: true, ..episode() })
    }

    #[test]
    fn a_marker_in_the_packs_season_keeps_it() {
        use crate::marker::Spot;
        let f = |i: i64, name: &str| serde_json::json!({ "index": i, "name": name, "priority": 0, "progress": 0.0 });
        let files = vec![f(0, "Show.S02E01.mkv"), f(1, "Show - 2x02.mkv"), f(2, "Show.S02E01.sample.mkv"), f(3, "notes.txt")];
        assert!(marker_keeps(&files, &[Spot { season: 2, episode: 9 }]), "anywhere in the pack's season");
        assert!(marker_keeps(&files, &[Spot { season: 1, episode: 3 }, Spot { season: 2, episode: 1 }]), "any profile's");
        assert!(!marker_keeps(&files, &[Spot { season: 3, episode: 1 }]), "moved on to the next season");
        assert!(!marker_keeps(&files, &[]));
        // A marker never switches a file on: the pack still starts fast on the next episode.
        let all = vec![ep_on("a", 2, 1, "pack")];
        assert_eq!(pack_wants("pack", Some(81189), &all, &[]).len(), 1);
    }

    #[test]
    fn an_episode_row_shows_its_watch_now_state() {
        let pct: HashMap<String, f64> = [("pack".to_string(), 0.42)].into();
        let mut half = ep_on("a", 2, 3, "PACK").1;
        half.position = 300.0;
        half.duration = Some(1200.0);
        let fresh = ep_on("a", 2, 4, "other").1;
        let entries = vec![half, fresh, Entry { tvdb: Some(1), ..ep_on("a", 2, 5, "pack").1 }];
        assert_eq!(episode_state(&entries, &pct, Some(81189), 2, 3), Some(NowState { key: "ep:81189:2:3".into(), half: true, pct: 0.42, progress: Some(0.25), finished: false }));
        assert_eq!(episode_state(&entries, &pct, Some(81189), 2, 4), Some(NowState { key: "ep:81189:2:4".into(), half: false, pct: 0.0, progress: None, finished: false }), "not started: downloading");
        assert_eq!(episode_state(&entries, &pct, Some(81189), 2, 5), None, "another show's");
        assert_eq!(episode_state(&entries, &pct, None, 2, 3), None, "no show id: nothing to match");
        assert_eq!(episode_state(&[movie()], &pct, Some(81189), 2, 3), None);
    }

    #[test]
    fn the_seek_bar_reads_the_chosen_files_own_progress() {
        let f = |i: i64, name: &str, size: i64, progress: f64| serde_json::json!({ "index": i, "name": name, "size": size, "progress": progress });
        let pack = vec![f(0, "Show/Show.S02E03.mkv", 900, 0.4), f(1, "Show/Show.S02E04.mkv", 900, 0.0), f(2, "Show/Show.S02E03.sample.mkv", 10, 1.0)];
        assert_eq!(chosen_progress(&pack, &episode()), Some(0.4), "S2E3's file, not the pack's 20%");
        assert_eq!(chosen_progress(&pack, &Entry { episode: Some(9), ..episode() }), None, "no file for it yet");
        let film = vec![f(0, "Film/extras.mkv", 50, 1.0), f(1, "Film/Film.2020.mkv", 5000, 0.25)];
        assert_eq!(chosen_progress(&film, &movie()), Some(0.25), "the movie's main file");
        assert_eq!(chosen_progress(&[], &movie()), None);
    }

    #[test]
    fn the_list_is_newest_first() {
        let e = |key: &str, at: i64| Entry { key: key.into(), updated_at: at, ..movie() };
        let got: Vec<String> = newest_first(vec![e("a", 5), e("b", 9), e("c", 1)]).into_iter().map(|x| x.key).collect();
        assert_eq!(got, vec!["b", "a", "c"]);
    }

    #[test]
    fn a_pack_wants_every_profile_and_every_download() {
        // Two profiles on one pack, a third episode a normal Download wants,
        // another show and a movie on the same hash's neighbours don't count.
        let all = vec![
            ep_on("a", 2, 3, "PACK"),
            ep_on("b", 2, 5, "pack"),
            ("c".into(), Entry { key: "ep:1:2:7".into(), tvdb: Some(1), season: Some(2), episode: Some(7), hash: Some("other".into()), ..episode() }),
            ("a".into(), Entry { hash: Some("pack".into()), ..movie() }),
        ];
        let w = pack_wants("pack", Some(81189), &all, &[(2, 8)]);
        let mut got: Vec<(i64, i64)> = w.into_iter().collect();
        got.sort();
        assert_eq!(got, vec![(2, 3), (2, 5), (2, 8)]);
        // The same show's episode on another torrent still counts: it may be in this pack too.
        let more = vec![ep_on("a", 2, 4, "elsewhere")];
        assert!(pack_wants("pack", Some(81189), &more, &[]).contains(&(2, 4)));
    }

    #[test]
    fn a_pack_plan_keeps_the_union_and_reads_odd_names() {
        let f = |i: i64, name: &str, prio: i64| serde_json::json!({ "index": i, "name": name, "priority": prio, "progress": 0.0 });
        let files = vec![
            f(0, "Pack/Show.S02E03.mkv", 1),
            f(1, "Pack/Show - 2x04.mkv", 0),
            f(2, "Pack/Show S02 E05 1080p.mkv", 1),
            f(3, "Pack/Show.S02E06E07.mkv", 1),
            f(4, "Pack/Show.S02E08.mkv", 0),
            f(5, "Pack/info.nfo", 1),
            f(6, "Pack/Sample/Show.S02E03.sample.mkv", 1),
        ];
        // Profile A watches E3, profile B E4, a normal Download wants E7.
        let wants: HashSet<(i64, i64)> = [(2, 3), (2, 4), (2, 7)].into_iter().collect();
        let p = pack_plan(&files, &wants, None);
        assert_eq!(p.skip, vec![2, 6]);
        assert_eq!(p.restore, vec![1]);
        assert!(pack_still_wanted(&p.wanted));
        // Nothing wanted any more: every video goes to skip, and the pack can go.
        let none = pack_plan(&files, &HashSet::new(), None);
        assert!(!pack_still_wanted(&none.wanted));
        assert_eq!(none.skip, vec![0, 2, 3, 6]);
    }

    #[test]
    fn pack_prios_never_skips_what_another_wants() {
        let files = vec![
            serde_json::json!({ "index": 0, "name": "P/S02E01.mkv", "priority": 1 }),
            serde_json::json!({ "index": 1, "name": "P/S02E03.mkv", "priority": 0 }),
            serde_json::json!({ "index": 2, "name": "P/S02E04.mkv", "priority": 0 }),
        ];
        let keep: HashSet<(i64, i64)> = [(2, 4)].into_iter().collect();
        let (skip, on) = pack_prios(&files, &files[1], &keep);
        assert_eq!(skip, vec![0]);
        assert_eq!(on, vec![1, 2]);
    }

    #[test]
    fn real_item_skips_placeholders() {
        use serde_json::json;
        let placeholder = json!({ "Id": "p", "Type": "Movie", "Path": r"C:\a\Chillarr Watch now\Movies\Inception [tmdbid-27205]\Inception [tmdbid-27205].strm", "ProviderIds": { "Tmdb": "27205" } });
        let missing = json!({ "Id": "v", "Type": "Movie", "LocationType": "Virtual", "ProviderIds": { "Tmdb": "27205" } });
        let real = json!({ "Id": "r", "Type": "Movie", "Path": r"D:\Movies\Inception (2010)\Inception.mkv", "ProviderIds": { "Tmdb": "27205" } });
        assert!(real_movie(&[placeholder.clone(), missing.clone()], 27205).is_none());
        assert_eq!(real_movie(&[placeholder, missing, real], 27205).unwrap()["Id"], "r");
        let shows = vec![
            json!({ "Id": "ws", "Type": "Series", "Path": r"C:\a\Chillarr Watch now\Series\BB [tvdbid-81189]", "ProviderIds": { "Tvdb": "81189" } }),
            json!({ "Id": "s", "Type": "Series", "Path": r"D:\TV\Breaking Bad", "ProviderIds": { "Tvdb": "81189" } }),
        ];
        assert_eq!(real_series(&shows, 81189), Some("s"));
        let eps = vec![
            json!({ "Id": "e1", "ParentIndexNumber": 2, "IndexNumber": 3, "IndexNumberEnd": 4, "Path": r"D:\TV\Breaking Bad\S02E03E04.mkv" }),
            json!({ "Id": "e2", "ParentIndexNumber": 2, "IndexNumber": 5, "LocationType": "Virtual" }),
        ];
        assert_eq!(real_episode(&eps, 2, 4).unwrap()["Id"], "e1");
        assert!(real_episode(&eps, 2, 5).is_none());
    }

    #[test]
    fn owned_torrents_outlive_their_entries() {
        let dir = tempfile::tempdir().unwrap();
        let w = WatchNow::load(dir.path().to_path_buf());
        w.upsert("a", Entry { hash: Some("ABC".into()), owned: true, ..episode() }).unwrap();
        w.upsert("b", Entry { hash: Some("xyz".into()), owned: false, ..movie() }).unwrap();
        w.keep_owned().unwrap();
        w.remove("a", "ep:81189:2:3");
        // Handed over: the pack is still Watch now's, kept on disk.
        let again = WatchNow::load(dir.path().to_path_buf());
        let t = again.owned_torrents();
        assert_eq!(t.len(), 1);
        assert_eq!((t[0].hash.as_str(), t[0].tvdb, t[0].kind.as_str()), ("abc", Some(81189), "episode"));
        assert!(again.owned_set().contains("abc") && !again.owned_set().contains("xyz"));
        again.forget_torrent("ABC").unwrap();
        assert!(again.owned_torrents().is_empty());
    }

    #[test]
    fn a_finished_file_nobody_wants_is_not_skipped_and_the_pack_reads_done() {
        let f = |i: i64, name: &str, prio: i64, progress: f64| serde_json::json!({ "index": i, "name": name, "priority": prio, "progress": progress });
        // E3 was watched and brought in; nothing in the pack is wanted any more.
        let files = vec![f(0, "Pack/Show.S02E03.mkv", 1, 1.0), f(1, "Pack/Show.S02E04.mkv", 0, 0.1), f(2, "Pack/Show.S02E05.mkv", 1, 0.0)];
        let plan = pack_plan(&files, &HashSet::new(), None);
        assert_eq!(plan.skip, vec![2]);
        assert!(!pack_still_wanted(&plan.wanted));
        // Done once the plan is applied, whatever qBittorrent says for the whole torrent.
        assert!(pack_complete(&files, &plan));
        // A wanted file still downloading: not done; one switched back on: not done either.
        let wants: HashSet<(i64, i64)> = [(2, 5)].into_iter().collect();
        assert!(!pack_complete(&files, &pack_plan(&files, &wants, None)));
        let wants: HashSet<(i64, i64)> = [(2, 4)].into_iter().collect();
        let p = pack_plan(&files, &wants, None);
        assert_eq!(p.restore, vec![1]);
        assert!(!pack_complete(&files, &p));
    }

    #[test]
    fn the_position_carries_when_it_was_played() {
        let b = position_body(90.0, 1_760_000_000);
        assert_eq!(b["PlaybackPositionTicks"], 900_000_000);
        assert_eq!(b["LastPlayedDate"], "2025-10-09T08:53:20Z");
        assert!(position_body(90.0, 0).get("LastPlayedDate").is_none());
    }

    #[test]
    fn a_finished_position_is_never_kept_or_resumed() {
        let dir = tempfile::tempdir().unwrap();
        let w = WatchNow::load(dir.path().to_path_buf());
        w.remember("a", "movie:1", 600.0, Some(5400.0)).unwrap();
        assert_eq!(w.recall("a", "movie:1"), Some((600.0, Some(5400.0))));
        // Removed after watching to the end: the half-way spot goes too.
        w.remember("a", "movie:1", 5000.0, Some(5400.0)).unwrap();
        assert_eq!(w.recall("a", "movie:1"), None);
        assert_eq!(WatchNow::load(dir.path().to_path_buf()).recall("a", "movie:1"), None, "gone from the file, not just hidden");
        w.remember("a", "movie:2", 0.0, None).unwrap();
        assert_eq!(w.recall("a", "movie:2"), None, "never started: nothing to keep");
        // An old file holding a finished position: not resumed from.
        std::fs::write(file(dir.path(), "b"), r#"{"positions":{"movie:3":[95.0,100.0]}}"#).unwrap();
        assert_eq!(WatchNow::load(dir.path().to_path_buf()).recall("b", "movie:3"), None);
    }

    #[test]
    fn a_handover_drops_the_kept_position() {
        let dir = tempfile::tempdir().unwrap();
        let w = WatchNow::load(dir.path().to_path_buf());
        w.remember("a", "ep:81189:2:3", 300.0, Some(1200.0)).unwrap();
        w.forget_position("a", "ep:81189:2:3").unwrap();
        assert_eq!(w.recall("a", "ep:81189:2:3"), None);
        assert_eq!(WatchNow::load(dir.path().to_path_buf()).recall("a", "ep:81189:2:3"), None);
    }

    #[test]
    fn starting_over_resets_watch_nows_memory_for_that_profile_only() {
        let dir = tempfile::tempdir().unwrap();
        let w = WatchNow::load(dir.path().to_path_buf());
        let at = |s: i64, n: i64, pos: f64| Entry { key: format!("ep:81189:{s}:{n}"), season: Some(s), episode: Some(n), position: pos, duration: Some(1200.0), ..episode() };
        w.upsert("a", at(1, 2, 300.0)).unwrap();
        w.upsert("a", at(2, 3, 400.0)).unwrap();
        w.upsert("a", at(0, 1, 50.0)).unwrap();
        w.upsert("a", Entry { tvdb: Some(5), key: "ep:5:1:1".into(), position: 70.0, ..episode() }).unwrap();
        w.upsert("b", at(2, 3, 500.0)).unwrap();
        w.remember("a", "ep:81189:2:5", 100.0, Some(1200.0)).unwrap();
        w.remember("a", "ep:81189:1:7", 100.0, Some(1200.0)).unwrap();
        w.remember("a", "movie:81189", 100.0, Some(1200.0)).unwrap();

        w.reset("a", 81189, Some(2)).unwrap();
        let pos = |uid: &str, key: &str| w.entry(uid, key).map(|e| e.position);
        assert_eq!(pos("a", "ep:81189:2:3"), Some(0.0), "season 2 starts over");
        assert_eq!(pos("a", "ep:81189:1:2"), Some(300.0), "season 1 left alone");
        assert_eq!(w.recall("a", "ep:81189:2:5"), None);
        assert!(w.recall("a", "ep:81189:1:7").is_some());
        assert_eq!(pos("b", "ep:81189:2:3"), Some(500.0), "another profile keeps theirs");

        w.reset("a", 81189, None).unwrap();
        assert_eq!(pos("a", "ep:81189:1:2"), Some(0.0), "the whole show");
        assert_eq!(pos("a", "ep:81189:0:1"), Some(50.0), "specials aren't part of it");
        assert_eq!(pos("a", "ep:5:1:1"), Some(70.0), "another show");
        assert_eq!(w.recall("a", "ep:81189:1:7"), None);
        assert!(w.recall("a", "movie:81189").is_some(), "a movie with the same number isn't an episode");
        assert_eq!(WatchNow::load(dir.path().to_path_buf()).entry("a", "ep:81189:2:3").unwrap().position, 0.0, "saved");
    }

    #[test]
    fn episode_keys_read_back() {
        assert_eq!(episode_key("ep:81189:2:3"), Some((81189, 2, 3)));
        assert_eq!(episode_key("movie:5"), None);
        assert_eq!(episode_key("ep:1:2"), None);
        assert_eq!(episode_key("ep:1:2:3:4"), None);
        assert_eq!(episode_key("ep:a:2:3"), None);
    }

    #[test]
    fn finishing_an_episode_on_leave_moves_where_youre_at_at_once() {
        use crate::marker::{load, save_mark, Mark, Spot};
        let e = Entry { duration: Some(1200.0), ..episode() };
        assert_eq!(finished_spot(&e, 1150.0, Some(1200.0)), Some((81189, Spot { season: 2, episode: 3 })));
        assert_eq!(finished_spot(&e, 1150.0, None), Some((81189, Spot { season: 2, episode: 3 })), "the saved length counts");
        assert_eq!(finished_spot(&e, 600.0, Some(1200.0)), None, "half way: nothing moves");
        assert_eq!(finished_spot(&Entry { duration: Some(100.0), ..movie() }, 99.0, Some(100.0)), None, "a movie has no spot");
        // What leave then does with it, under the marker's own rules.
        let dir = tempfile::tempdir().unwrap();
        save_mark(dir.path(), "u", 81189, Mark { spot: Spot { season: 2, episode: 3 }, set_at: 100 }).unwrap();
        let (tvdb, spot) = finished_spot(&e, 1150.0, Some(1200.0)).unwrap();
        crate::marker::finished(dir.path(), "u", tvdb, spot, now()).unwrap();
        assert_eq!(load(dir.path(), "u")[&81189].spot, Spot { season: 2, episode: 4 });
        // Rewatching an earlier one through Watch now never moves it back.
        let early = Entry { season: Some(1), episode: Some(2), ..e.clone() };
        let (tvdb, spot) = finished_spot(&early, 1150.0, Some(1200.0)).unwrap();
        crate::marker::finished(dir.path(), "u", tvdb, spot, now()).unwrap();
        assert_eq!(load(dir.path(), "u")[&81189].spot, Spot { season: 2, episode: 4 });
    }

    #[test]
    fn a_finish_carried_to_the_library_reaches_the_watch_log() {
        let b = played_body(1_760_000_000);
        assert_eq!((b["Played"].clone(), b["PlaybackPositionTicks"].clone()), (serde_json::json!(true), serde_json::json!(0)));
        assert_eq!(b["LastPlayedDate"], "2025-10-09T08:53:20Z");
        assert!(played_body(0)["LastPlayedDate"].is_string(), "never without a date");
        // The log skips anything without one; with it, the episode is logged.
        let ep = serde_json::json!({ "Id": "e", "SeriesId": "s", "ParentIndexNumber": 2, "IndexNumber": 3, "Path": r"D:\TV\BB\S02E03.mkv", "UserData": b });
        let mut h = crate::history::History::default();
        h.record(&crate::history::Snapshot { episodes: vec![ep], ..Default::default() });
        assert_eq!(h.series["s"].episodes.get("2:3"), Some(&1_760_000_000));
    }

    #[test]
    fn removing_never_deletes_what_a_normal_download_wants() {
        let f = |i: i64, name: &str, prio: i64, progress: f64| serde_json::json!({ "index": i, "name": name, "priority": prio, "progress": progress });
        let pack = vec![f(0, "P/Show.S02E03.mkv", 1, 0.3), f(1, "P/Show.S02E04.mkv", 1, 0.1), f(2, "P/Show.S02E05.mkv", 0, 0.0), f(3, "P/info.nfo", 1, 0.0)];
        let mine: HashSet<(i64, i64)> = [(2, 3)].into_iter().collect();
        // Only Watch now's own episode is wanted (Sonarr lists it too): delete.
        assert_eq!(on_remove(&pack, (2, 3), &[(2, 3)], &mine), OnRemove::Delete);
        assert_eq!(on_remove(&pack, (2, 3), &[], &mine), OnRemove::Delete);
        // A normal Download wants E4 from the same pack: only E3 is switched off.
        assert_eq!(on_remove(&pack, (2, 3), &[(2, 3), (2, 4)], &mine), OnRemove::Skip(vec![0]));
        // Wanted, but already fully downloaded: nothing left to lose, delete.
        let done = vec![f(0, "P/Show.S02E03.mkv", 1, 0.3), f(1, "P/Show.S02E04.mkv", 1, 1.0)];
        assert_eq!(on_remove(&done, (2, 3), &[(2, 4)], &mine), OnRemove::Delete);
        // Another profile's Watch now episode isn't a normal Download.
        let both: HashSet<(i64, i64)> = [(2, 3), (2, 4)].into_iter().collect();
        assert_eq!(on_remove(&pack, (2, 3), &[(2, 3), (2, 4)], &both), OnRemove::Delete);
        // A double episode file that a Download still wants half of stays on.
        let double = vec![f(0, "P/Show.S02E03E04.mkv", 1, 0.2)];
        assert_eq!(on_remove(&double, (2, 3), &[(2, 4)], &mine), OnRemove::Skip(vec![]));
        // Already skipped: nothing to ask qBittorrent.
        let off = vec![f(0, "P/Show.S02E03.mkv", 0, 0.3), f(1, "P/Show.S02E04.mkv", 1, 0.1)];
        assert_eq!(on_remove(&off, (2, 3), &[(2, 4)], &mine), OnRemove::Skip(vec![]));
    }

    #[test]
    fn a_second_press_joins_the_first() {
        let flights = Flights::new();
        let Flight::Lead(lead) = flights.claim("u|movie:1") else { panic!("the first press leads") };
        let Flight::Join(rx) = flights.claim("u|movie:1") else { panic!("the second joins") };
        assert!(matches!(flights.claim("v|movie:1"), Flight::Lead(_)), "another profile is its own press");
        assert!(matches!(flights.claim("u|movie:2"), Flight::Lead(_)), "another title too");
        assert_eq!(*rx.borrow(), None, "still getting ready");
        let tile = crate::tiles::Tile::new("movie:1", "movie", "A");
        lead.finish(&Ok(tile.clone()));
        assert_eq!(*rx.borrow(), Some(Ok(tile)), "the joiner gets the leader's answer");
        assert!(matches!(flights.claim("u|movie:1"), Flight::Lead(_)), "free again once done");
    }

    #[test]
    fn a_press_that_dies_frees_the_title() {
        let flights = Flights::new();
        let Flight::Lead(lead) = flights.claim("u|movie:1") else { panic!() };
        let Flight::Join(mut rx) = flights.claim("u|movie:1") else { panic!() };
        drop(lead);
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        assert!(rt.block_on(rx.wait_for(|o| o.is_some())).is_err(), "the joiner is told, not left waiting");
        assert!(matches!(flights.claim("u|movie:1"), Flight::Lead(_)));
    }

    #[test]
    fn a_removed_profile_takes_its_own_titles_and_bad_copy() {
        let dir = tempfile::tempdir().unwrap();
        let w = WatchNow::load(dir.path().to_path_buf());
        w.upsert("a", movie()).unwrap();
        w.upsert("a", episode()).unwrap();
        w.upsert("b", episode()).unwrap();
        std::fs::write(dir.path().join("watchnow-a.json.bad"), "x").unwrap();
        let gone: Vec<String> = w.forget("a").into_iter().map(|e| e.key).collect();
        assert_eq!(gone, vec!["movie:27205".to_string()], "the episode is still profile b's");
        assert!(!dir.path().join("watchnow-a.json.bad").exists());
        assert!(w.any_has("ep:81189:2:3"));
    }

    #[test]
    fn a_finished_live_entry_is_not_offered_to_resume() {
        let done = Entry { position: 1150.0, duration: Some(1200.0), ..episode() };
        let t = tile_of(&done);
        assert_eq!((t.resume_seconds, t.progress), (None, None), "watched to the end: plays from the start");
        let half = tile_of(&Entry { position: 300.0, duration: Some(1200.0), ..episode() });
        assert_eq!((half.resume_seconds, half.progress), (Some(300.0), Some(0.25)));
        assert_eq!(tile_of(&episode()).resume_seconds, None, "never started");
        let s = episode_state(&[done], &HashMap::new(), Some(81189), 2, 3).unwrap();
        assert_eq!((s.finished, s.half, s.progress), (true, false, None), "a finished row is watched, not half watched");
    }

    #[test]
    fn a_repeated_failure_is_logged_once() {
        let mut m = HashMap::new();
        assert!(changed(&mut m, "pack:a", Some("down")));
        assert!(!changed(&mut m, "pack:a", Some("down")));
        assert!(changed(&mut m, "pack:a", Some("other")));
        assert!(changed(&mut m, "pack:b", Some("down")));
        assert!(!changed(&mut m, "pack:a", None));
        assert!(changed(&mut m, "pack:a", Some("down")));
    }
}
