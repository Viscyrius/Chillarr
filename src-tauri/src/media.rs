//! The media side: Jellyfin sign-in and setup, the rows on Home and the other
//! tabs, playing, Download, My List, search, and the setup steps the tutorial runs.

use crate::arr::Arr;
use crate::fix::Core;
use crate::jellyfin::{Jf, JfError, Session};
use crate::library;
use crate::log::log;
use crate::model::{AppId, Level};
use crate::tiles::{self, Tile};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

/// After this an anime row is shown as is and refreshed in the background.
const ANIME_TTL: u64 = 6 * 60 * 60;
/// Trending moves by the hour, so its rows are refreshed sooner.
const ANIME_TRENDING_TTL: u64 = 2 * 60 * 60;
/// AniList allows 30 requests a minute (measured 2026-10-17). A burst of
/// ANILIST_BURST goes out at once, then one every ANILIST_GAP.
const ANILIST_GAP: Duration = Duration::from_millis(2100);
const ANILIST_BURST: f64 = 8.0;
/// How long Explore reuses the Jellyfin library it marks owned titles with.
const LIBRARY_TTL: Duration = Duration::from_secs(60);
/// Seed lists from Radarr's metadata server (keyless).
const SEED_LISTS: [&str; 4] = ["tmdb/trending", "tmdb/popular", "imdb/popular", "imdb/top250"];
const RADARR_META: &str = "https://api.radarr.video/v1";

pub struct Media {
    path: PathBuf,
    dir: PathBuf,
    pub device_id: String,
    /// The owner's Jellyfin sign-in (an admin), kept in jellyfin.json.
    pub session: Mutex<Option<Session>>,
    /// The profile watching, when it isn't the owner (see profiles.rs).
    pub profile: Mutex<Option<Session>>,
    /// Nobody has picked a profile yet and one has a PIN: the watching
    /// profile's Jellyfin, history and My List are refused until a
    /// `profiles_switch` passes. Owner work (jf_admin) carries on.
    locked: std::sync::atomic::AtomicBool,
    pub catalog: Mutex<crate::catalog::Catalog>,
    pub shows: Mutex<crate::catalog::Shows>,
    /// AniList rows by "category#page": (fetched at, unix seconds; tiles). Saved
    /// to anime.json so the Anime tab opens at once after a restart.
    anime: Mutex<HashMap<String, (u64, Vec<Tile>)>>,
    /// AniList request budget: (requests left, when it was counted). Held while
    /// waiting, so requests queue instead of tripping the rate limit.
    anilist_budget: tokio::sync::Mutex<(f64, Instant)>,
    /// Rows being refreshed in the background, so one isn't fetched twice.
    anime_refreshing: Mutex<HashSet<String>>,
    /// Titles a Download is following; the waiting list leaves their notice to it.
    following: Mutex<HashSet<(AppId, i64)>>,
    /// Trailer YouTube ids by tile key for this session (None: nothing has one).
    pub trailers: Mutex<HashMap<String, Option<String>>>,
    /// Detail-panel facts and cast by tile key for this session.
    pub details: Mutex<HashMap<String, crate::details::Details>>,
    /// My List, for titles not in the library yet (Radarr and Sonarr hear
    /// nothing until Download). Library titles are Jellyfin favourites instead;
    /// one of these becomes a favourite once it arrives. Kept in saved.json.
    saved: Mutex<Vec<Tile>>,
    /// Held while a web viewer's My List file is read and written back.
    saved_io: Mutex<()>,
    /// Show fanart by TVDB id for the session (None: it has none).
    fanart: Mutex<HashMap<i64, Option<String>>>,
    /// Explore's copy of the Jellyfin library, so each page doesn't fetch it
    /// again, per Jellyfin user (each sees only the libraries they may).
    explore_library: Mutex<HashMap<String, (Instant, Vec<Tile>)>>,
}

#[derive(Serialize, Clone, Debug)]
pub struct Row {
    pub id: String,
    pub title: String,
    pub tiles: Vec<Tile>,
    /// What "Explore all" opens, e.g. "movie:genre:Action".
    pub explore: Option<String>,
    /// "top10" draws the big-number row.
    pub style: Option<String>,
}

fn row(id: &str, title: &str, tiles: Vec<Tile>) -> Option<Row> {
    (!tiles.is_empty()).then(|| Row { id: id.into(), title: title.into(), tiles, explore: None, style: None })
}

/// A row the page fills itself, from page 0 of its Explore, once it scrolls near.
fn lazy_row(id: &str, title: &str, explore: &str) -> Option<Row> {
    Some(Row { id: id.into(), title: title.into(), tiles: vec![], explore: Some(explore.into()), style: Some("lazy".into()) })
}

fn row_x(id: &str, title: &str, tiles: Vec<Tile>, explore: &str) -> Option<Row> {
    row(id, title, tiles).map(|mut r| {
        r.explore = Some(explore.into());
        r
    })
}

impl Media {
    /// Reads the saved sign-in and catalogs from Chillarr's app-data folder.
    pub fn load(dir: PathBuf) -> Media {
        let path = dir.join("jellyfin.json");
        let session: Option<Session> = crate::seal::read(&path).and_then(|t| serde_json::from_str(&t).ok());
        let device_id = session.as_ref().map(|s| s.device_id.clone()).filter(|d| !d.is_empty()).unwrap_or_else(|| {
            let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
            format!("chillarr-{:x}{:x}", n, std::process::id())
        });
        Media {
            catalog: Mutex::new(crate::catalog::Catalog::load(&dir.join("catalog.json"))),
            shows: Mutex::new(crate::catalog::Shows::load(&dir.join("shows.json"))),
            anime: Mutex::new(std::fs::read_to_string(dir.join("anime.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()),
            anilist_budget: tokio::sync::Mutex::new((ANILIST_BURST, Instant::now())),
            anime_refreshing: Mutex::new(HashSet::new()),
            following: Mutex::new(HashSet::new()),
            trailers: Mutex::new(HashMap::new()),
            details: Mutex::new(HashMap::new()),
            fanart: Mutex::new(HashMap::new()),
            saved: Mutex::new(std::fs::read_to_string(dir.join("saved.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()),
            saved_io: Mutex::new(()),
            explore_library: Mutex::new(HashMap::new()),
            path,
            dir,
            device_id,
            session: Mutex::new(session),
            profile: Mutex::new(None),
            locked: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Starts locked (profiles.rs, when a profile has a PIN). The owner's My
    /// List is put away until someone picks a profile.
    pub fn lock(&self) {
        self.locked.store(true, std::sync::atomic::Ordering::SeqCst);
        self.saved.lock().unwrap().clear();
    }

    pub fn unlock(&self) {
        self.locked.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn is_locked(&self) -> bool {
        self.locked.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// My List for titles not in the library is per profile: saved.json for
    /// the owner, saved-<jellyfin user id>.json for the others.
    fn saved_file(&self) -> PathBuf {
        match self.profile.lock().unwrap().as_ref() {
            Some(p) => self.dir.join(format!("saved-{}.json", p.user_id)),
            None => self.dir.join("saved.json"),
        }
    }

    /// Switches who is watching (None: the owner) and loads their My List.
    pub fn set_profile(&self, p: Option<Session>) {
        *self.profile.lock().unwrap() = p;
        let list = std::fs::read_to_string(self.saved_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        *self.saved.lock().unwrap() = list;
        self.explore_library.lock().unwrap().clear();
    }

    pub fn is_following(&self, owner: AppId, id: i64) -> bool {
        self.following.lock().unwrap().contains(&(owner, id))
    }

    fn save_saved(&self) {
        // Locked, the list in memory is empty, not the owner's: don't write it over theirs.
        if self.is_locked() {
            return;
        }
        let text = serde_json::to_string(&*self.saved.lock().unwrap()).unwrap_or_default();
        let _ = std::fs::write(self.saved_file(), text);
    }

    /// Whose My List the window holds in memory: its profile, or the owner.
    fn window_user(&self) -> Option<String> {
        let profile = self.profile.lock().unwrap().as_ref().map(|p| p.user_id.clone());
        profile.or_else(|| self.session.lock().unwrap().as_ref().map(|s| s.user_id.clone()))
    }

    /// A web viewer's My List file (viewer.rs), read and written each time.
    /// None when there's no viewer, or it's whoever the window has open, whose
    /// list is the one in memory (so neither writes over the other).
    fn viewer_saved_file(&self) -> Option<PathBuf> {
        let v = crate::viewer::current()?;
        if !self.is_locked() && self.window_user().as_deref() == Some(v.session.user_id.as_str()) {
            return None;
        }
        let owner = self.session.lock().unwrap().as_ref().map(|s| s.user_id.clone());
        Some(if owner.as_deref() == Some(v.session.user_id.as_str()) { self.dir.join("saved.json") } else { self.dir.join(format!("saved-{}.json", v.session.user_id)) })
    }

    /// Reads or changes the My List of whoever this runs for.
    fn edit_saved<R>(&self, write: bool, f: impl FnOnce(&mut Vec<Tile>) -> R) -> R {
        match self.viewer_saved_file() {
            Some(path) => {
                let _io = self.saved_io.lock().unwrap();
                let mut list: Vec<Tile> = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
                let r = f(&mut list);
                if write {
                    let _ = std::fs::write(&path, serde_json::to_string(&list).unwrap_or_default());
                }
                r
            }
            None => {
                let r = f(&mut self.saved.lock().unwrap());
                if write {
                    self.save_saved();
                }
                r
            }
        }
    }

    /// Adds or removes a title not in the library from My List. True when it's now on it.
    pub fn toggle_saved(&self, tile: Tile) -> bool {
        self.edit_saved(true, |list| {
            if let Some(i) = list.iter().position(|t| t.key == tile.key) {
                list.remove(i);
                false
            } else {
                let mut t = tile;
                t.status = None;
                t.progress = None;
                list.insert(0, t);
                true
            }
        })
    }

    pub fn saved_keys(&self) -> Vec<String> {
        self.edit_saved(false, |list| list.iter().map(|t| t.key.clone()).collect())
    }

    fn saved_list(&self) -> Vec<Tile> {
        self.edit_saved(false, |list| list.clone())
    }

    /// Drops titles that are now in the library (they became favourites).
    fn forget_saved(&self, keys: &[String]) {
        if keys.is_empty() {
            return;
        }
        self.edit_saved(true, |list| list.retain(|t| !keys.contains(&t.key)));
    }

    fn save_anime(&self) {
        let text = serde_json::to_string(&*self.anime.lock().unwrap()).unwrap_or_default();
        let _ = std::fs::write(self.dir.join("anime.json"), text);
    }

    fn save(&self, s: &Session) {
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string(s) {
            let _ = crate::seal::write(&self.path, &text);
        }
        *self.session.lock().unwrap() = Some(s.clone());
    }
}

/// Jellyfin as whoever is watching: the web viewer a bridge request is for
/// (viewer.rs), else the profile picked (profiles.rs), or the owner's account. Rows, playback, progress and My List go through this.
pub(crate) fn jf(app: &AppHandle) -> Option<Jf> {
    if let Some(v) = crate::viewer::current() {
        let cfg = app.state::<Core>().config(AppId::Jellyfin)?;
        return Some(Jf::on_behalf(&cfg.base_url, &v.session));
    }
    let media = app.state::<Media>();
    if media.is_locked() {
        return None;
    }
    let profile = media.profile.lock().unwrap().clone();
    match profile {
        Some(p) => {
            let cfg = app.state::<Core>().config(AppId::Jellyfin)?;
            Some(Jf::new(&cfg.base_url, &p.device_id, Some(&p)))
        }
        None => jf_admin(app),
    }
}

/// Jellyfin as the owner's (admin) account, whatever profile is watching:
/// libraries, plugins, scans, deleting files, and other accounts' data.
pub(crate) fn jf_admin(app: &AppHandle) -> Option<Jf> {
    let cfg = app.state::<Core>().config(AppId::Jellyfin)?;
    let media = app.state::<Media>();
    let s = media.session.lock().unwrap().clone();
    Some(Jf::new(&cfg.base_url, &media.device_id, s.as_ref()))
}

pub(crate) fn arr(app: &AppHandle, id: AppId) -> Option<Arr> {
    app.state::<Core>().config(id).as_ref().and_then(Arr::new)
}

/// Chillarr's app-data folder (catalogs, My List, watch logs).
pub(crate) fn data_dir(app: &AppHandle) -> Option<PathBuf> {
    app.try_state::<Media>().map(|m| m.dir.clone())
}

/// The Jellyfin user id of whoever is watching now.
pub(crate) fn active_user(app: &AppHandle) -> Option<String> {
    if let Some(v) = crate::viewer::current() {
        return Some(v.session.user_id);
    }
    let media = app.state::<Media>();
    if media.is_locked() {
        return None;
    }
    let profile = media.profile.lock().unwrap().as_ref().map(|p| p.user_id.clone());
    profile.or_else(|| media.session.lock().unwrap().as_ref().map(|s| s.user_id.clone()))
}

// ---------- Jellyfin account ----------

#[derive(Serialize)]
pub struct MediaStatus {
    pub vlc: bool,
    pub installed: bool,
    pub up: bool,
    pub setup_done: bool,
    pub signed_in: bool,
    pub user: Option<String>,
    pub address: Option<String>,
    /// The Jellyfin address over Tailscale, for watching away from home.
    pub tailscale: Option<String>,
    /// The same by Tailscale's IP, for apps that can't use the name.
    pub tailscale_ip: Option<String>,
}

/// This PC's Tailscale name and IP, when Tailscale is installed and connected.
pub(crate) fn tailscale_self() -> Option<(Option<String>, Option<String>)> {
    use std::os::windows::process::CommandExt;
    let exe = std::env::var_os("ProgramFiles").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\Program Files")).join("Tailscale").join("tailscale.exe");
    if !exe.exists() {
        return None;
    }
    let out = std::process::Command::new(exe).args(["status", "--json"]).creation_flags(0x0800_0000).output().ok()?;
    let v: Value = serde_json::from_slice(&out.stdout).ok()?;
    if v["BackendState"] != "Running" {
        return None;
    }
    let name = v["Self"]["DNSName"].as_str().map(|s| s.trim_end_matches('.').to_string()).filter(|s| !s.is_empty());
    let ip = v["Self"]["TailscaleIPs"].as_array().and_then(|a| a.iter().filter_map(|x| x.as_str()).find(|x| x.contains('.')).map(String::from));
    Some((name, ip))
}

pub async fn status(app: &AppHandle) -> MediaStatus {
    let installed = app.state::<Core>().config(AppId::Jellyfin).as_ref().is_some_and(crate::discover::is_installed);
    let session = app.state::<Media>().session.lock().unwrap().clone();
    let mut st = MediaStatus { vlc: crate::vlc::find().is_some(), installed, up: false, setup_done: false, signed_in: session.is_some(), user: session.map(|s| s.user), address: None, tailscale: None, tailscale_ip: None };
    if let Some(j) = jf_admin(app) {
        if let Ok(done) = j.public_info().await {
            st.up = true;
            st.setup_done = done;
            let port = j.base.rsplit(':').next().unwrap_or("8096").to_string();
            st.address = crate::arr::lan_ip().map(|ip| format!("http://{ip}:{port}"));
            if let Ok(Some((name, ip))) = tokio::task::spawn_blocking(tailscale_self).await {
                st.tailscale = name.map(|n| format!("http://{n}:{port}"));
                st.tailscale_ip = ip.map(|i| format!("http://{i}:{port}"));
            }
        }
    }
    st
}

/// First time: finishes Jellyfin's setup with this account. Otherwise signs in.
/// Then makes sure the library folders are in Jellyfin.
pub async fn connect(app: &AppHandle, user: &str, password: &str) -> Result<(), String> {
    let j = jf_admin(app).ok_or("Jellyfin isn't installed")?;
    let done = j.public_info().await.map_err(|e| e.to_string())?;
    if !done {
        j.complete_setup(user, password).await.map_err(|e| e.to_string())?;
        log(app, Level::Ok, format!("Set up Jellyfin with the account {user}."));
    }
    let s = j.sign_in(user, password).await.map_err(|e| match e {
        crate::jellyfin::JfError::Rejected => "That name or password is wrong".to_string(),
        e => e.to_string(),
    })?;
    app.state::<Media>().save(&s);
    log(app, Level::Ok, format!("Signed in to Jellyfin as {user}."));
    if let Err(e) = ensure_libraries(app).await {
        log(app, Level::Amber, format!("Signed in, but couldn't add your folders to Jellyfin: {e}"));
    }
    Ok(())
}

pub fn sign_out(app: &AppHandle) {
    let media = app.state::<Media>();
    let _ = std::fs::remove_file(&media.path);
    *media.session.lock().unwrap() = None;
    media.set_profile(None);
}

/// Jellyfin library name and type for one of Radarr's/Sonarr's root folders.
pub fn library_for(path: &str, movies: bool) -> (String, &'static str) {
    let name = path.trim_end_matches(['\\', '/']).rsplit(['\\', '/']).next().unwrap_or("").to_ascii_lowercase();
    let anime = name.contains("anime");
    match (movies, anime) {
        (true, false) => ("Movies".into(), "movies"),
        (true, true) => ("Anime Movies".into(), "movies"),
        (false, false) => ("Series".into(), "tvshows"),
        (false, true) => ("Anime".into(), "tvshows"),
    }
}

/// Every Radarr/Sonarr root folder becomes a Jellyfin library.
pub async fn ensure_libraries(app: &AppHandle) -> Result<(), String> {
    let j = jf_admin(app).ok_or("Jellyfin isn't installed")?;
    for (id, movies) in [(AppId::Radarr, true), (AppId::Sonarr, false)] {
        let Some(a) = arr(app, id) else { continue };
        for r in a.list("rootfolder").await.unwrap_or_default() {
            let Some(path) = r["path"].as_str() else { continue };
            let (name, kind) = library_for(path, movies);
            if j.ensure_library(&name, kind, path).await.map_err(|e| e.to_string())? {
                log(app, Level::Ok, format!("Added {path} to Jellyfin as {name}."));
            }
        }
    }
    Ok(())
}

// ---------- Rows ----------

pub(crate) fn http() -> reqwest::Client {
    reqwest::Client::builder().user_agent("Chillarr").timeout(Duration::from_secs(15)).build().unwrap_or_default()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Grows the catalogs a little: refreshes the seed lists every 3 hours, fetches
/// recommended movies it hasn't seen (a few dozen per run), the movies in your
/// library (so "Because you watched" knows them), and more TVmaze pages. It
/// also re-reads the movies and shows the rows rank, since popularity is live.
pub async fn grow(app: &AppHandle) {
    let http = http();
    let media = app.state::<Media>();
    // Seed lists: every 3 hours (IMDb's 13 MB top 250 weekly). Only a run that
    // actually got movies counts as seeded, so being offline at boot retries.
    let (seeded, top250) = {
        let c = media.catalog.lock().unwrap();
        (c.seeded_at, c.top250_at)
    };
    if now_secs().saturating_sub(seeded) > 3 * 3600 {
        let mut got = false;
        for list in SEED_LISTS {
            if list == "imdb/top250" && now_secs().saturating_sub(top250) < 7 * 24 * 3600 {
                continue;
            }
            let Ok(r) = http.get(format!("{RADARR_META}/list/{list}")).send().await else { continue };
            if !r.status().is_success() {
                continue;
            }
            let items: Vec<Value> = r.json().await.unwrap_or_default();
            if items.is_empty() {
                continue;
            }
            got = true;
            let mut c = media.catalog.lock().unwrap();
            for v in &items {
                c.add(v);
            }
            if list == "tmdb/trending" {
                c.set_trending(items.iter().filter_map(|v| v["TmdbId"].as_i64()).collect());
            }
            if list == "imdb/top250" {
                c.top250_at = now_secs();
            }
        }
        if got {
            media.catalog.lock().unwrap().seeded_at = now_secs();
        }
    }
    // Your movies (Radarr library, and what you watched or added in Jellyfin)
    // first, so "Because you watched" fills in; their recommendations jump the queue.
    let mut mine: Vec<i64> = Vec::new();
    if let Some(a) = arr(app, AppId::Radarr) {
        mine.extend(a.list("movie").await.unwrap_or_default().iter().filter_map(|m| m["tmdbId"].as_i64()));
    }
    if let Some(j) = jf(app).filter(|j| j.user_id.is_some()) {
        for q in ["recursive=true&includeItemTypes=Movie&filters=IsPlayed&sortBy=DatePlayed&sortOrder=Descending&limit=10", "recursive=true&includeItemTypes=Movie&sortBy=DateCreated&sortOrder=Descending&limit=10"] {
            mine.extend(jf_tiles(&j, q).await.iter().filter_map(|t| t.tmdb));
        }
    }
    // Finished movies from the watch log too, even ones since deleted.
    mine.extend(crate::history::active(app).movies.values().filter_map(|m| m.tmdb));
    let missing: Vec<i64> = {
        let c = media.catalog.lock().unwrap();
        let mut seen = std::collections::HashSet::new();
        mine.iter().copied().filter(|id| !c.has(*id) && seen.insert(*id)).take(20).collect()
    };
    let mut queue = missing;
    {
        let mut c = media.catalog.lock().unwrap();
        // Stale numbers on ranked movies first, so the rows stay live.
        queue.extend(c.stale(now_secs(), 60));
        queue.extend(c.next_to_fetch(100));
    }
    for (i, id) in queue.iter().copied().enumerate() {
        match http.get(format!("{RADARR_META}/movie/{id}")).send().await {
            Ok(r) if r.status().is_success() => {
                if let Ok(v) = r.json::<Value>().await {
                    media.catalog.lock().unwrap().add(&v);
                }
            }
            Ok(r) if r.status().as_u16() == 404 => media.catalog.lock().unwrap().dead(id),
            Ok(r) if r.status().as_u16() == 429 => {
                // Asked to slow down: stop here and try the rest next time.
                media.catalog.lock().unwrap().retry(&queue[i..]);
                break;
            }
            _ => {
                // Offline or a hiccup: keep the rest for later.
                media.catalog.lock().unwrap().retry(&queue[i..]);
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    // Movies saved before Filters kept their length, rating and language are
    // read again, a few hundred a run, 100 to a request.
    let missing = media.catalog.lock().unwrap().facts_missing(400);
    for ids in missing.chunks(100) {
        let Ok(r) = http.post(format!("{RADARR_META}/movie/bulk")).json(ids).send().await else { break };
        if !r.status().is_success() {
            break;
        }
        let list: Vec<Value> = r.json().await.unwrap_or_default();
        {
            let mut c = media.catalog.lock().unwrap();
            for v in &list {
                c.add(v);
            }
            // Ones the server no longer sends back aren't asked for every run.
            if !list.is_empty() {
                c.facts_gone(ids);
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    {
        let mut c = media.catalog.lock().unwrap();
        let recs: Vec<i64> = mine.iter().flat_map(|id| c.recommendations(*id)).collect();
        c.prioritize(&recs);
        let _ = c.save(&media.dir.join("catalog.json"));
    }
    // TVmaze allows 20 calls per 10 seconds; 600 ms apart stays well under.
    for _ in 0..5 {
        let page = media.shows.lock().unwrap().next_page;
        let Ok(r) = http.get(format!("https://api.tvmaze.com/shows?page={page}")).send().await else { break };
        if r.status().as_u16() == 404 || r.status().is_success() {
            let shows: Vec<Value> = if r.status().is_success() { r.json().await.unwrap_or_default() } else { vec![] };
            if !shows.is_empty() {
                media.shows.lock().unwrap().add_page(&shows);
                tokio::time::sleep(Duration::from_millis(600)).await;
                continue;
            }
            // Caught up. New shows land on the last page, so ask it again.
            if page > 0 {
                if let Ok(r) = http.get(format!("https://api.tvmaze.com/shows?page={}", page - 1)).send().await {
                    let last: Vec<Value> = r.json().await.unwrap_or_default();
                    media.shows.lock().unwrap().add_shows(&last, now_secs());
                }
            }
            media.shows.lock().unwrap().reached_end(now_secs());
        }
        break;
    }
    // The most watched shows' weights, re-read once a day.
    let stale = media.shows.lock().unwrap().stale(now_secs(), 30);
    for id in stale {
        tokio::time::sleep(Duration::from_millis(600)).await;
        let Ok(r) = http.get(format!("https://api.tvmaze.com/shows/{id}")).send().await else { break };
        match r.status().as_u16() {
            200 => {
                if let Ok(v) = r.json::<Value>().await {
                    media.shows.lock().unwrap().add_shows(&[v], now_secs());
                }
            }
            404 => media.shows.lock().unwrap().forget(id),
            _ => break,
        }
    }
    let _ = media.shows.lock().unwrap().save(&media.dir.join("shows.json"));
}

/// Wide fanart for shows that have none, from Sonarr's metadata server, all
/// at once and remembered for the session.
async fn add_fanart(app: &AppHandle, tiles: &mut [Tile]) {
    let media = app.state::<Media>();
    let mut set = tokio::task::JoinSet::new();
    for t in tiles.iter().filter(|t| t.backdrop.is_none()) {
        let Some(tvdb) = t.tvdb else { continue };
        if media.fanart.lock().unwrap().contains_key(&tvdb) {
            continue;
        }
        set.spawn(async move {
            let v: Option<Value> = async { http().get(format!("https://skyhook.sonarr.tv/v1/tvdb/shows/en/{tvdb}")).send().await.ok()?.json().await.ok() }.await;
            let art = v.and_then(|v| v["images"].as_array()?.iter().find(|i| i["coverType"] == "Fanart")?["url"].as_str().map(String::from));
            (tvdb, art)
        });
    }
    while let Some(Ok((tvdb, art))) = set.join_next().await {
        media.fanart.lock().unwrap().insert(tvdb, art);
    }
    let cache = media.fanart.lock().unwrap();
    for t in tiles.iter_mut().filter(|t| t.backdrop.is_none()) {
        if let Some(Some(art)) = t.tvdb.and_then(|id| cache.get(&id)) {
            t.backdrop = Some(art.clone());
        }
    }
}

/// One AniList row. A saved copy is returned at once; a stale one is
/// refreshed in the background, so only a row never seen before waits.
async fn anime_row(app: &AppHandle, category: &str, page: u32) -> Vec<Tile> {
    let key = format!("{category}#{page}");
    let cached = app.state::<Media>().anime.lock().unwrap().get(&key).cloned();
    match cached {
        Some((at, tiles)) if !tiles.is_empty() => {
            let ttl = if category == "trending" { ANIME_TRENDING_TTL } else { ANIME_TTL };
            if now_secs().saturating_sub(at) > ttl && app.state::<Media>().anime_refreshing.lock().unwrap().insert(key.clone()) {
                let (h, cat) = (app.clone(), category.to_string());
                tauri::async_runtime::spawn(async move {
                    fetch_anime(&h, &cat, page).await;
                    h.state::<Media>().anime_refreshing.lock().unwrap().remove(&format!("{cat}#{page}"));
                });
            }
            tiles
        }
        // AniList answered nothing a moment ago: don't ask again straight away.
        Some((at, _)) if now_secs().saturating_sub(at) < 120 => vec![],
        _ => fetch_anime(app, category, page).await,
    }
}

/// Asks AniList for one row (within the rate limit) and saves it.
async fn fetch_anime(app: &AppHandle, category: &str, page: u32) -> Vec<Tile> {
    let key = format!("{category}#{page}");
    let now = chrono::Local::now();
    use chrono::Datelike;
    let query = json!({ "query": crate::catalog::anilist_args(category, page, now.year(), now.month()) });
    let media = app.state::<Media>();
    // Another caller may have fetched this row a moment ago (checked again
    // once our turn with AniList comes, so queued callers share one answer).
    let fresh = || media.anime.lock().unwrap().get(&key).filter(|(at, t)| !t.is_empty() && now_secs().saturating_sub(*at) < 60).map(|(_, t)| t.clone());
    if let Some(tiles) = fresh() {
        return tiles;
    }
    let answer = anilist_post(app, &query, || fresh().is_some()).await;
    if let (None, Some(tiles)) = (&answer, fresh()) {
        return tiles;
    }
    let tiles: Vec<Tile> = answer.and_then(|v| v["data"]["Page"]["media"].as_array().cloned()).unwrap_or_default().iter().map(tiles::from_anilist).collect();
    {
        let mut cache = media.anime.lock().unwrap();
        // Keep a good copy over an empty answer (AniList busy).
        if tiles.is_empty() {
            if let Some((_, good)) = cache.get(&key).filter(|(_, t)| !t.is_empty()) {
                return good.clone();
            }
        }
        cache.insert(key, (now_secs(), tiles.clone()));
    }
    if !tiles.is_empty() {
        media.save_anime();
    }
    tiles
}

/// One AniList request, paced to its limit (30 a minute, shared by every
/// caller). `answered` is asked once it's this caller's turn: true means
/// someone queued ahead already got the answer, so nothing is sent (None).
async fn anilist_post(app: &AppHandle, query: &Value, answered: impl Fn() -> bool) -> Option<Value> {
    let media = app.state::<Media>();
    let mut budget = media.anilist_budget.lock().await;
    if answered() {
        return None;
    }
    let refill = budget.1.elapsed().as_secs_f64() / ANILIST_GAP.as_secs_f64();
    *budget = ((budget.0 + refill).min(ANILIST_BURST), Instant::now());
    if budget.0 < 1.0 {
        tokio::time::sleep(ANILIST_GAP.mul_f64(1.0 - budget.0)).await;
        *budget = (1.0, Instant::now());
    }
    budget.0 -= 1.0;
    match http().post("https://graphql.anilist.co").json(query).send().await {
        Ok(r) if r.status().as_u16() == 429 => {
            // Rate limited anyway: hold everyone back for as long as AniList asks.
            let secs = r.headers().get("retry-after").and_then(|v| v.to_str().ok()?.parse::<f64>().ok()).unwrap_or(60.0).min(90.0);
            budget.0 = -secs / ANILIST_GAP.as_secs_f64();
            None
        }
        Ok(r) => r.json::<Value>().await.ok(),
        Err(_) => None,
    }
}

/// What a "Because you watched" row shows for one seed: a movie's TMDb
/// recommendations, shows sharing a series' genres (TVmaze has no
/// recommendations), or AniList's recommendations for an anime.
async fn because_recs(app: &AppHandle, seed: &crate::history::Seed) -> Vec<Tile> {
    use crate::history::SeedKind;
    let media = app.state::<Media>();
    match &seed.kind {
        SeedKind::Movie { tmdb } => {
            let c = media.catalog.lock().unwrap();
            c.recommendations(*tmdb).into_iter().filter_map(|id| c.get(id)).collect()
        }
        SeedKind::Series { tvdb, genres } => media.shows.lock().unwrap().similar(genres, *tvdb, 30),
        SeedKind::Anime { anilist } => anime_recs(app, *anilist, &seed.title).await,
    }
}

/// AniList's recommendations for one anime, saved with the other anime rows.
/// Only a saved copy is ever shown, so no tab waits on AniList: a missing or
/// stale one is fetched in the background, and the tab redraws when it lands.
async fn anime_recs(app: &AppHandle, anilist: Option<i64>, title: &str) -> Vec<Tile> {
    let key = match anilist {
        Some(id) => format!("recs:{id}"),
        None => format!("recs:{}", title.to_lowercase()),
    };
    let media = app.state::<Media>();
    let cached = media.anime.lock().unwrap().get(&key).cloned();
    let stale = cached.as_ref().map_or(true, |(at, _)| now_secs().saturating_sub(*at) > ANIME_TTL);
    if stale && media.anime_refreshing.lock().unwrap().insert(key.clone()) {
        let (h, title, key) = (app.clone(), title.to_string(), key.clone());
        let had = cached.as_ref().is_some_and(|(_, t)| !t.is_empty());
        tauri::async_runtime::spawn(async move {
            let got = fetch_anime_recs(&h, anilist, &title, &key).await;
            h.state::<Media>().anime_refreshing.lock().unwrap().remove(&key);
            if !had && !got.is_empty() {
                let _ = tauri::Emitter::emit(&h, "get-progress", json!({ "key": "", "refresh": true }));
            }
        });
    }
    cached.map(|(_, t)| t).unwrap_or_default()
}

/// AniList's recommendations for one anime, for a panel that can wait: a
/// fresh saved copy at once, a stale one at once while it's refreshed, and
/// only one never asked for waits its turn with AniList.
pub(crate) async fn anime_recs_now(app: &AppHandle, anilist: Option<i64>, title: &str) -> Vec<Tile> {
    let key = match anilist {
        Some(id) => format!("recs:{id}"),
        None => format!("recs:{}", title.to_lowercase()),
    };
    let had = app.state::<Media>().anime.lock().unwrap().contains_key(&key);
    if had { anime_recs(app, anilist, title).await } else { fetch_anime_recs(app, anilist, title, &key).await }
}

/// Asks AniList and saves the answer. An empty answer is kept too (for
/// ANIME_TTL, so a title AniList can't find isn't asked about on every
/// render); a failed request is tried again in 5 minutes.
async fn fetch_anime_recs(app: &AppHandle, anilist: Option<i64>, title: &str, key: &str) -> Vec<Tile> {
    let query = json!({ "query": crate::history::anilist_recs_query(anilist, title) });
    let answer = anilist_post(app, &query, || false).await;
    let tiles: Vec<Tile> = answer
        .as_ref()
        .and_then(|v| v["data"]["Media"]["recommendations"]["nodes"].as_array())
        .into_iter()
        .flatten()
        .map(|n| &n["mediaRecommendation"])
        .filter(|m| m.is_object() && m["isAdult"] != true)
        .map(tiles::from_anilist)
        .collect();
    let media = app.state::<Media>();
    {
        let mut cache = media.anime.lock().unwrap();
        // AniList answered "no such anime": that's an answer. A request that failed isn't.
        let found_nothing = answer.as_ref().is_some_and(|v| v["data"].is_object());
        if tiles.is_empty() && !found_nothing {
            let keep = cache.get(key).map(|(_, t)| t.clone()).unwrap_or_default();
            cache.insert(key.to_string(), (now_secs().saturating_sub(ANIME_TTL) + 300, keep.clone()));
            return keep;
        }
        cache.insert(key.to_string(), (now_secs(), tiles.clone()));
    }
    media.save_anime();
    tiles
}

/// The Anime tab's genre rows: (AniList category, row title). Harem and
/// School are AniList tags; the rest are its genres.
const ANIME_GENRES: [(&str, &str); 14] = [
    ("genre:Action", "Action"),
    ("genre:Ecchi", "Ecchi"),
    ("tag:Female Harem", "Harem"),
    ("genre:Romance", "Romance"),
    ("genre:Comedy", "Comedy"),
    ("genre:Fantasy", "Fantasy"),
    ("genre:Supernatural", "Supernatural"),
    ("tag:School", "School"),
    ("genre:Sci-Fi", "Sci-Fi"),
    ("genre:Slice of Life", "Slice of Life"),
    ("genre:Drama", "Drama"),
    ("genre:Adventure", "Adventure"),
    ("genre:Mystery", "Mystery"),
    ("genre:Sports", "Sports"),
];

/// What Radarr/Sonarr already track and where it stands: (tmdb, tvdb, title, label).
pub(crate) async fn tracked(app: &AppHandle) -> Vec<(Option<i64>, Option<i64>, String, String)> {
    let mut out = Vec::new();
    if let Some(a) = arr(app, AppId::Radarr) {
        for m in a.list("movie").await.unwrap_or_default() {
            let has = m["hasFile"] == true;
            if !has && m["monitored"] != true {
                continue;
            }
            out.push((m["tmdbId"].as_i64(), None, m["title"].as_str().unwrap_or_default().to_lowercase(), tracked_label(has, m["isAvailable"].as_bool().unwrap_or(true))));
        }
    }
    if let Some(a) = arr(app, AppId::Sonarr) {
        for x in a.list("series").await.unwrap_or_default() {
            let has = x["statistics"]["episodeFileCount"].as_i64().unwrap_or(0) > 0;
            if !has && x["monitored"] != true {
                continue;
            }
            out.push((None, x["tvdbId"].as_i64(), x["title"].as_str().unwrap_or_default().to_lowercase(), tracked_label(has, x["status"] != "upcoming")));
        }
    }
    out
}

/// The label for a tracked title with no playable copy in Jellyfin. One with
/// a file is waiting on Jellyfin to add it, not "downloaded" for you to play.
fn tracked_label(has_file: bool, released: bool) -> String {
    if has_file { "Adding to library".into() } else { library::wanted_status(released, false, None) }
}

/// Titles Radarr/Sonarr already have, but Jellyfin doesn't show yet, say so
/// instead of offering Download again.
pub(crate) fn mark_tracked(tiles: Vec<Tile>, tracked: &[(Option<i64>, Option<i64>, String, String)]) -> Vec<Tile> {
    tiles
        .into_iter()
        .map(|mut t| {
            if t.jf_id.is_none() && t.status.is_none() {
                let hit = tracked.iter().find(|(tm, tv, title, _)| {
                    (t.tmdb.is_some() && *tm == t.tmdb) || (t.tvdb.is_some() && *tv == t.tvdb) || (t.kind == "anime" && *title == t.title.to_lowercase())
                });
                if let Some((_, _, _, label)) = hit {
                    t.status = Some(label.clone());
                }
            }
            t
        })
        .collect()
}

pub async fn queue(app: &AppHandle) -> Vec<Tile> {
    let mut out = Vec::new();
    for (id, path) in [(AppId::Radarr, "queue?includeMovie=true&pageSize=50"), (AppId::Sonarr, "queue?includeSeries=true&includeEpisode=true&pageSize=50")] {
        if let Some(a) = arr(app, id) {
            if let Ok(v) = a.get_one(path).await {
                out.extend(v["records"].as_array().cloned().unwrap_or_default().iter().map(library::from_queue));
            }
        }
    }
    out
}

/// Jellyfin libraries whose name says anime.
pub(crate) async fn anime_library_ids(j: &Jf) -> Vec<String> {
    j.views()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|l| l["Name"].as_str().is_some_and(|n| n.to_ascii_lowercase().contains("anime")))
        .filter_map(|l| l["Id"].as_str().map(String::from))
        .collect()
}

async fn jf_tiles(j: &Jf, query: &str) -> Vec<Tile> {
    j.items(query).await.unwrap_or_default().iter().map(|v| tiles::from_jf(v, &j.base)).collect()
}

/// Continue watching: Watch now's titles first (newest first, as listed),
/// then Jellyfin's. Jellyfin's lists already leave the placeholder items out
/// (`jellyfin::on_shelf`); one that slipped through (no path) is dropped here.
pub fn continue_first(watch: Vec<Tile>, cont: Vec<Tile>) -> Vec<Tile> {
    let ids: std::collections::HashSet<String> = watch.iter().filter_map(|t| t.jf_id.clone()).collect();
    watch.into_iter().chain(cont.into_iter().filter(|t| !t.jf_id.as_ref().is_some_and(|id| ids.contains(id)))).collect()
}

/// The rows for one tab: home, movies, series, anime, downloads.
///
/// Err only on My downloads, when Jellyfin couldn't list the library (still
/// starting, which takes it ~20-30 s, or too busy to answer). That page is the
/// library, so an empty one would say "Nothing downloaded yet" about a full
/// library. Other tabs just go without the library rows.
pub async fn rows(app: &AppHandle, tab: &str) -> Result<Vec<Row>, String> {
    let j = jf(app).filter(|j| j.user_id.is_some());
    if tab == "mylist" {
        return Ok(my_list_page(app, j.as_ref()).await);
    }
    let mut out: Vec<Option<Row>> = Vec::new();
    let (mut lib_movies, mut lib_series, mut lib_anime, mut cont, mut latest) = (vec![], vec![], vec![], vec![], vec![]);
    if let Some(j) = &j {
        let anime_ids = anime_library_ids(j).await;
        cont = j.resume().await.unwrap_or_default().iter().map(|v| tiles::from_jf(v, &j.base)).collect();
        let next: Vec<Tile> = j.next_up().await.unwrap_or_default().iter().map(|v| tiles::from_jf(v, &j.base)).collect();
        for t in next {
            if !cont.iter().any(|c| c.series_id.is_some() && c.series_id == t.series_id) {
                cont.push(t);
            }
        }
        latest = j.latest().await.unwrap_or_default().iter().map(|v| tiles::from_jf(v, &j.base)).collect();
        let lib = |q: String| async move { j.items(&q).await.map(|v| v.iter().map(|v| tiles::from_jf(v, &j.base)).collect::<Vec<Tile>>()) };
        let mut failed = None;
        let mut keep = |r: Result<Vec<Tile>, JfError>| r.unwrap_or_else(|e| { failed.get_or_insert(e); vec![] });
        lib_movies = keep(lib("recursive=true&includeItemTypes=Movie&sortBy=SortName&limit=500".into()).await);
        lib_series = keep(lib("recursive=true&includeItemTypes=Series&sortBy=SortName&limit=500".into()).await);
        for id in &anime_ids {
            lib_anime.extend(keep(lib(format!("parentId={id}&recursive=true&includeItemTypes=Series,Movie&sortBy=SortName&limit=500")).await));
        }
        if let (Some(e), "downloads") = (failed, tab) {
            return Err(format!("Couldn't get your library from Jellyfin. {e}."));
        }
        let is_anime = |t: &Tile| lib_anime.iter().any(|a| a.key == t.key);
        lib_movies.retain(|t| !is_anime(t));
        lib_series.retain(|t| !is_anime(t));
        if tab != "downloads" {
            cont = continue_first(crate::watchnow::list(app).await, cont);
        }
    }
    let downloading = queue(app).await;
    let library: Vec<Tile> = lib_movies.iter().chain(lib_series.iter()).chain(lib_anime.iter()).cloned().collect();
    let tracked = tracked(app).await;
    let own = |v: Vec<Tile>| mark_tracked(crate::catalog::merge_owned(v, &library), &tracked);
    let year = { use chrono::Datelike; chrono::Local::now().year() as i64 };

    // "Because you watched X", from the watch log, so it survives deleting X:
    // a movie once finished, a series once watched to the end, anime after 5
    // episodes. Nothing watched yet means no row, not a stand-in.
    let mut because: Vec<(String, Vec<Tile>)> = Vec::new();
    if tab != "downloads" {
        use crate::history::SeedKind;
        for s in crate::history::active(app).seeds() {
            let fits = match tab {
                "movies" => matches!(s.kind, SeedKind::Movie { .. }) && !s.anime,
                "series" => matches!(s.kind, SeedKind::Series { .. }),
                "anime" => s.anime,
                _ => true,
            };
            if !fits || because.iter().any(|(t, _)| *t == s.title) {
                continue;
            }
            let recs = because_recs(app, &s).await;
            if !recs.is_empty() {
                because.push((s.title.clone(), recs));
            }
            if because.len() >= 3 {
                break;
            }
        }
    }
    let because_rows: Vec<Option<Row>> = because
        .into_iter()
        .enumerate()
        .map(|(i, (title, recs))| row(&format!("because-{i}"), &format!("Because you watched {title}"), own(recs)))
        .collect();

    let (popular, top10, newr, top, genres) = {
        let media = app.state::<Media>();
        let c = media.catalog.lock().unwrap();
        let genres: Vec<(String, Vec<Tile>)> = c.genres(8).into_iter().take(14).map(|g| { let t = c.by_genre(&g, 30); (g, t) }).collect();
        (c.popular(40), c.top10(), c.new_releases(year, 30), c.top_rated(30), genres)
    };
    let (mut s_pop, s_top, s_genres) = {
        let media = app.state::<Media>();
        let sh = media.shows.lock().unwrap();
        let g: Vec<(String, Vec<Tile>)> = sh.genres(5).into_iter().take(12).map(|x| { let t = sh.by_genre(&x, 30); (x, t) }).collect();
        (sh.popular(30), sh.top_rated(30), g)
    };
    // The hero needs wide pictures, which TVmaze doesn't have: borrow the top shows' fanart.
    if matches!(tab, "series" | "home") {
        let n = s_pop.len().min(6);
        add_fanart(app, &mut s_pop[..n]).await;
    }
    // "Trending now" carries on where the Top 10 stops, without repeating it.
    let popular_rest: Vec<Tile> = popular.into_iter().filter(|t| !top10.iter().any(|x| x.key == t.key)).take(30).collect();
    let top10_row = row_x("top10", "Top 10 movies this week", own(top10), "movie:popular").map(|mut r| {
        r.style = Some("top10".into());
        r
    });
    let genre_rows = |prefix: &str, list: Vec<(String, Vec<Tile>)>, take: usize| -> Vec<Option<Row>> {
        list.into_iter().take(take).map(|(g, t)| row_x(&format!("genre-{g}"), &g, own(t), &format!("{prefix}:genre:{g}"))).collect()
    };
    let kind_is = |t: &Tile, k: &[&str]| k.contains(&t.kind.as_str());
    match tab {
        "movies" => {
            out.push(row("continue", "Continue watching", cont.iter().filter(|t| kind_is(t, &["movie"])).cloned().collect()));
            out.push(top10_row);
            out.extend(because_rows);
            out.push(row_x("new", "New releases", own(newr), &format!("movie:new:{year}")));
            out.push(row_x("top-rated", "Top rated", own(top), "movie:top"));
            out.push(row("downloading", "Downloading now", downloading.into_iter().filter(|t| t.kind == "movie").collect()));
            out.extend(genre_rows("movie", genres, 14));
            out.push(row("library-movies", "My movies", lib_movies));
        }
        "series" => {
            out.push(row("continue", "Continue watching", cont.iter().filter(|t| kind_is(t, &["episode", "series"])).cloned().collect()));
            out.push(row_x("popular-series", "Popular series", own(s_pop), "series:popular"));
            out.extend(because_rows);
            out.push(row_x("top-series", "Top rated series", own(s_top), "series:top"));
            out.push(row("downloading", "Downloading now", downloading.into_iter().filter(|t| t.kind == "episode").collect()));
            out.extend(genre_rows("series", s_genres, 12));
            out.push(row("library-series", "My series", lib_series));
        }
        "anime" => {
            let mine: Vec<String> = lib_anime.iter().filter_map(|t| t.jf_id.clone()).collect();
            out.push(row("continue", "Continue watching", cont.iter().filter(|t| t.series_id.as_ref().or(t.jf_id.as_ref()).is_some_and(|id| mine.contains(id))).cloned().collect()));
            let mut because_iter = because_rows.into_iter();
            for (cat, title) in [("trending", "Trending anime"), ("season", "This season"), ("popular", "All-time favourites"), ("top", "Top rated anime")] {
                out.push(row_x(&format!("anime-{cat}"), title, anime_row(app, cat, 1).await, &format!("anime:{cat}")));
                if cat == "trending" {
                    out.extend(because_iter.by_ref());
                }
            }
            // Genre rows fill in as you scroll to them, so the tab doesn't wait on a dozen more AniList requests.
            for (cat, title) in ANIME_GENRES {
                out.push(lazy_row(&format!("genre-{title}"), title, &format!("anime:{cat}")));
            }
            out.push(row("library-anime", "My anime", lib_anime));
        }
        "downloads" => {
            out.push(row("mylist", "On the way", my_list(app).await));
            out.push(row("saved", "My List", my_list_row(app, j.as_ref(), &own).await));
            out.push(row("downloading", "Downloading now", downloading));
            out.push(row("latest", "Recently added", latest));
            out.push(row("library-movies", "My movies", lib_movies));
            out.push(row("library-series", "My series", lib_series));
            out.push(row("library-anime", "My anime", lib_anime));
        }
        _ => {
            out.push(row("continue", "Continue watching", cont));
            out.push(row("saved", "My List", my_list_row(app, j.as_ref(), &own).await));
            out.push(row("mylist", "On the way", my_list(app).await));
            out.push(top10_row);
            let mut because_iter = because_rows.into_iter();
            out.push(because_iter.next().flatten());
            out.push(row("latest", "Recently added", latest));
            out.push(row("downloading", "Downloading now", downloading));
            out.push(row_x("trending", "Trending now", own(popular_rest), "movie:popular"));
            out.push(row_x("new", "New releases", own(newr), &format!("movie:new:{year}")));
            out.push(row_x("popular-series", "Popular series", own(s_pop), "series:popular"));
            out.push(row_x("anime-trending", "Trending anime", anime_row(app, "trending", 1).await, "anime:trending"));
            out.extend(because_iter);
            out.push(row_x("top-rated", "Top rated movies", own(top), "movie:top"));
            out.extend(genre_rows("movie", genres, 4));
            out.push(row("library-movies", "My movies", lib_movies));
            out.push(row("library-series", "My series", lib_series));
            out.push(row("library-anime", "My anime", lib_anime));
        }
    }
    Ok(out.into_iter().flatten().collect())
}

/// One page of "Explore all": "movie:<cat>", "series:<cat>", "anime:<cat>".
pub async fn explore(app: &AppHandle, category: &str, page: usize) -> Vec<Tile> {
    let (kind, cat) = category.split_once(':').unwrap_or(("movie", "popular"));
    let tiles = match kind {
        "series" => app.state::<Media>().shows.lock().unwrap().page(cat, page, 40),
        "anime" => anime_row(app, cat, page as u32 + 1).await,
        _ => app.state::<Media>().catalog.lock().unwrap().page(cat, page, 40),
    };
    crate::catalog::merge_owned(tiles, &explore_library(app).await)
}

/// The library, reused for a minute, to mark owned titles in Explore and Filters.
pub(crate) async fn explore_library(app: &AppHandle) -> Vec<Tile> {
    let Some(j) = jf(app).filter(|j| j.user_id.is_some()) else { return vec![] };
    let uid = j.user_id.clone().unwrap_or_default();
    let cached = app.state::<Media>().explore_library.lock().unwrap().get(&uid).filter(|(at, _)| at.elapsed() < LIBRARY_TTL).map(|(_, l)| l.clone());
    if let Some(l) = cached {
        return l;
    }
    let l = jf_tiles(&j, "recursive=true&includeItemTypes=Movie,Series&limit=1000").await;
    let media = app.state::<Media>();
    let mut cache = media.explore_library.lock().unwrap();
    cache.retain(|_, (at, _)| at.elapsed() < LIBRARY_TTL);
    cache.insert(uid, (Instant::now(), l.clone()));
    l
}

/// AniList's genres (the ones it files shows under).
/// Hentai is left out: rows ask AniList for non-adult titles only.
const ANILIST_GENRES: [&str; 18] = ["Action", "Adventure", "Comedy", "Drama", "Ecchi", "Fantasy", "Horror", "Mahou Shoujo", "Mecha", "Music", "Mystery", "Psychological", "Romance", "Sci-Fi", "Slice of Life", "Sports", "Supernatural", "Thriller"];

/// Genres the Filters panel offers for a kind.
pub fn filter_genres(app: &AppHandle, kind: &str) -> Vec<String> {
    match kind {
        "anime" => ANILIST_GENRES.iter().map(|g| g.to_string()).collect(),
        "series" => app.state::<Media>().shows.lock().unwrap().genres(3),
        _ => app.state::<Media>().catalog.lock().unwrap().genres(3),
    }
}

/// Studios (movies) or networks and streaming services (series) the Filters
/// panel offers: the 24 with the most titles.
pub fn filter_studios(app: &AppHandle, kind: &str) -> Vec<String> {
    match kind {
        "series" => app.state::<Media>().shows.lock().unwrap().studios(24),
        "movie" => app.state::<Media>().catalog.lock().unwrap().studios(24),
        _ => vec![],
    }
}

/// The library titles a filter keeps ("Only what I have").
async fn owned_matching(app: &AppHandle, f: &crate::catalog::Filter) -> Vec<Tile> {
    let Some(j) = jf(app).filter(|j| j.user_id.is_some()) else { return vec![] };
    // The whole library is asked for at once and filtered and paged here:
    // Jellyfin can't combine genres (its list means "any of") or leave one out.
    let mut q = format!("recursive=true&includeItemTypes={}&limit=5000", if f.kind == "movie" { "Movie" } else { "Series" });
    q.push_str(match f.sort.as_str() {
        "top" => "&sortBy=CommunityRating,SortName&sortOrder=Descending",
        "new" => "&sortBy=PremiereDate,SortName&sortOrder=Descending",
        "title" => "&sortBy=SortName&sortOrder=Ascending",
        _ => "&sortBy=DateCreated,SortName&sortOrder=Descending",
    });
    let mut tiles = jf_tiles(&j, &q).await;
    // Anime has its own libraries; the other kinds leave them out.
    let anime_ids = anime_library_ids(&j).await;
    if !anime_ids.is_empty() {
        let mut lib_anime = Vec::new();
        for id in &anime_ids {
            lib_anime.extend(jf_tiles(&j, &format!("parentId={id}&recursive=true&includeItemTypes=Series,Movie&limit=2000")).await.into_iter().map(|t| t.key));
        }
        tiles.retain(|t| (f.kind == "anime") == lib_anime.contains(&t.key));
    }
    tiles.retain(|t| f.keeps(t));
    tiles
}

/// One page (40) of what a filter finds. "Only what I have" asks Jellyfin;
/// otherwise movies and series come from the saved catalogs and anime from AniList.
pub async fn filter(app: &AppHandle, f: crate::catalog::Filter, page: usize) -> Vec<Tile> {
    if f.owned {
        // Scrolling asks for page after page of the same filter: the library is read once.
        static OWNED: std::sync::Mutex<Option<(String, Instant, Vec<Tile>)>> = std::sync::Mutex::new(None);
        let key = format!("{f:?}");
        let hit = OWNED.lock().unwrap().as_ref().filter(|(k, at, _)| *k == key && at.elapsed() < Duration::from_secs(60)).map(|(_, _, t)| t.clone());
        let tiles = match hit {
            Some(t) => t,
            None => {
                let t = owned_matching(app, &f).await;
                *OWNED.lock().unwrap() = Some((key, Instant::now(), t.clone()));
                t
            }
        };
        return tiles.into_iter().skip(page * 40).take(40).collect();
    }
    let tiles = match f.kind.as_str() {
        "anime" => anime_row(app, &crate::catalog::anilist_filter(&f), page as u32 + 1).await,
        "series" => app.state::<Media>().shows.lock().unwrap().filtered(&f, page, 40),
        _ => app.state::<Media>().catalog.lock().unwrap().filtered(&f, page, 40),
    };
    crate::catalog::merge_owned(tiles, &explore_library(app).await)
}

/// AniList's tags (not the adult ones), by group, fetched once a run.
pub async fn anime_tags() -> Result<Vec<(String, String)>, String> {
    static TAGS: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());
    let have = TAGS.lock().unwrap().clone();
    if !have.is_empty() {
        return Ok(have);
    }
    let r = http()
        .post("https://graphql.anilist.co")
        .json(&json!({ "query": "{ MediaTagCollection { name category isAdult } }" }))
        .send()
        .await
        .map_err(|e| format!("AniList didn't answer: {e}"))?;
    let v: Value = r.json().await.map_err(|e| format!("AniList sent something odd: {e}"))?;
    let tags: Vec<(String, String)> = v["data"]["MediaTagCollection"]
        .as_array()
        .ok_or("AniList sent no tags")?
        .iter()
        .filter(|t| t["isAdult"] != true)
        .filter_map(|t| Some((t["name"].as_str()?.to_string(), t["category"].as_str().unwrap_or("Other").to_string())))
        .collect();
    *TAGS.lock().unwrap() = tags.clone();
    Ok(tags)
}

pub async fn episodes(app: &AppHandle, series_id: &str) -> Vec<Tile> {
    let Some(j) = jf(app) else { return vec![] };
    j.episodes(series_id).await.unwrap_or_default().iter().map(|v| tiles::from_jf(v, &j.base)).collect()
}

/// What a search found, and which sources couldn't be asked (so an outage
/// says so instead of passing for "Nothing found").
#[derive(Serialize, Default)]
pub struct Found {
    pub tiles: Vec<Tile>,
    pub failed: Vec<String>,
}

/// A title or search term with case, spaces and punctuation gone:
/// "Spider-Man" and "spiderman" both become "spiderman".
pub(crate) fn squash(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// How well a library title matches the term, best first; `None` when it doesn't.
/// Jellyfin's own search wants the words as typed ("spiderman" finds no
/// "Spider-Man"), so the library is also matched here, loosely.
pub(crate) fn match_rank(title: &str, term: &str) -> Option<u8> {
    let (t, q) = (squash(title), squash(term));
    if q.is_empty() {
        return None;
    }
    if t == q {
        return Some(0);
    }
    if t.starts_with(&q) {
        return Some(1);
    }
    // Shorter than 3 letters, "in the middle" matches half the library.
    if q.chars().count() < 3 {
        return None;
    }
    if t.contains(&q) {
        return Some(2);
    }
    // Every word somewhere in the title, in any order ("man iron").
    let words: Vec<String> = term.split(|c: char| !c.is_alphanumeric()).map(squash).filter(|w| !w.is_empty()).collect();
    (words.len() > 1 && words.iter().all(|w| t.contains(w.as_str()))).then_some(3)
}

/// Every source at once, merged (search.rs). The window asks source by
/// source instead (`search::live`) so the page fills in as they answer.
pub async fn search(app: &AppHandle, term: &str) -> Found {
    crate::search::all(app, term).await
}

// ---------- Playing ----------

/// What the player needs: a direct file or an HLS stream, tracks, length.
pub async fn play(app: &AppHandle, jf_id: &str, start: f64, audio_index: Option<i64>, subtitle_index: Option<i64>) -> Result<Value, String> {
    let j = jf(app).filter(|j| j.user_id.is_some()).ok_or("Sign in to Jellyfin first (Settings > Account)")?;
    let token = j.token().ok_or("Sign in to Jellyfin first")?.to_string();
    let info = j.playback(jf_id, start, audio_index, subtitle_index).await.map_err(|e| e.to_string())?;
    let src = info["MediaSources"][0].clone();
    let source_id = src["Id"].as_str().unwrap_or_default().to_string();
    let session = info["PlaySessionId"].as_str().unwrap_or_default().to_string();
    let with_key = |url: &str| if url.contains("api_key=") || url.contains("ApiKey=") { url.to_string() } else { format!("{url}{}api_key={token}", if url.contains('?') { "&" } else { "?" }) };
    let direct = src["SupportsDirectPlay"].as_bool().unwrap_or(false);
    let (url, hls) = match (direct, src["TranscodingUrl"].as_str()) {
        (true, _) => (format!("{}/Videos/{jf_id}/stream.mp4?static=true&mediaSourceId={source_id}&api_key={token}", j.base), false),
        (false, Some(t)) => (with_key(&format!("{}{}", j.base, t)), true),
        (false, None) => return Err("Jellyfin can't convert this file for playing here. Try Play in VLC.".into()),
    };
    let streams = src["MediaStreams"].as_array().cloned().unwrap_or_default();
    // Every subtitle track. Text ones (SRT, ASS, VTT, embedded or not) come as
    // WebVTT, which Jellyfin converts on the fly; picture ones (PGS, DVD) can
    // only be drawn into the video, so picking one reloads the stream.
    let subtitles: Vec<Value> = streams
        .iter()
        .filter(|s| s["Type"] == "Subtitle")
        .map(|s| {
            let index = s["Index"].as_i64().unwrap_or(0);
            let text = s["IsTextSubtitleStream"].as_bool().unwrap_or(false);
            let ass = matches!(s["Codec"].as_str().map(|c| c.to_ascii_lowercase()).as_deref(), Some("ass") | Some("ssa"));
            json!({
                "index": index,
                "label": s["DisplayTitle"],
                "lang": s["Language"],
                "forced": s["IsForced"],
                "default": s["IsDefault"],
                "external": s["IsExternal"],
                // Which file it is, since Jellyfin renumbers tracks when one is added.
                "path": s["Path"],
                "burn": !text,
                "url": text.then(|| format!("{}/Videos/{jf_id}/{source_id}/Subtitles/{index}/0/Stream.vtt?api_key={token}", j.base)),
                // The original styled file, drawn by the player itself like VLC does.
                "ass_url": ass.then(|| format!("{}/Videos/{jf_id}/{source_id}/Subtitles/{index}/0/Stream.ass?api_key={token}", j.base)),
            })
        })
        .collect();
    let audio: Vec<Value> = streams.iter().filter(|s| s["Type"] == "Audio").map(|s| json!({ "index": s["Index"], "label": s["DisplayTitle"], "lang": s["Language"], "default": s["IsDefault"] })).collect();
    // Preview pictures for the progress bar, when Jellyfin has made them (trickplay.rs).
    let trickplay = match j.user_id.as_deref() {
        Some(uid) => j.get_json(&format!("/Items?userId={uid}&ids={jf_id}&fields=Trickplay")).await.ok().and_then(|v| crate::trickplay::info(&v["Items"][0], &source_id)).map(|mut t| {
            t["url"] = json!(format!("{}/Videos/{jf_id}/Trickplay/{}/{{n}}.jpg?mediaSourceId={source_id}&api_key={token}", j.base, t["width"]));
            t
        }),
        None => None,
    };
    Ok(json!({
        "url": url,
        "hls": hls,
        "playSessionId": session,
        "mediaSourceId": source_id,
        "playMethod": if hls { "Transcode" } else { "DirectPlay" },
        "duration": src["RunTimeTicks"].as_i64().map(crate::jellyfin::ticks_to_seconds),
        "subtitles": subtitles,
        "audio": audio,
        "trickplay": trickplay,
    }))
}

/// Plays the original file in VLC and keeps Jellyfin told where you are.
/// `watch_key`: a Watch now title, left (download paused, position kept) when VLC closes.
pub async fn play_vlc(app: &AppHandle, jf_id: &str, start: f64, title: &str, watch_key: Option<String>) -> Result<(), String> {
    let exe = crate::vlc::find().ok_or("VLC isn't installed. Get it from videolan.org, or switch Play with back to Chillarr in Settings.")?;
    let j = jf(app).filter(|j| j.user_id.is_some()).ok_or("Sign in to Jellyfin first (Settings > Jellyfin & streaming)")?;
    let token = j.token().ok_or("Sign in to Jellyfin first")?.to_string();
    let info = j.playback(jf_id, 0.0, None, None).await.map_err(|e| e.to_string())?;
    let source_id = info["MediaSources"][0]["Id"].as_str().unwrap_or_default().to_string();
    let session = info["PlaySessionId"].as_str().unwrap_or_default().to_string();
    let url = format!("{}/Videos/{jf_id}/stream?static=true&mediaSourceId={source_id}&api_key={token}", j.base);
    let port = crate::vlc::free_port().ok_or("No free port for VLC's remote")?;
    let password = crate::vlc::new_password();
    let mut child = std::process::Command::new(&exe)
        .args(crate::vlc::vlc_args(title, port, &password))
        .spawn()
        .map_err(|e| format!("Couldn't start VLC: {e}"))?;
    // Hand the address over through the local remote once it answers, then jump to the resume point.
    let http0 = reqwest::Client::builder().timeout(Duration::from_secs(3)).build().unwrap_or_default();
    let mut ready = false;
    for _ in 0..40 {
        if crate::vlc::status(&http0, port, &password).await.is_some() {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    if !ready {
        let _ = child.kill();
        return Err("VLC started but its remote didn't answer. Check that VLC's web interface isn't blocked.".into());
    }
    crate::vlc::command(&http0, port, &password, &crate::vlc::play_query(&url)).await;
    if start >= 1.0 {
        tokio::time::sleep(Duration::from_millis(1200)).await;
        crate::vlc::command(&http0, port, &password, &format!("command=seek&val={}", start.floor() as i64)).await;
    }
    let item = jf_id.to_string();
    let body = move |pos: f64, paused: bool| json!({
        "ItemId": item, "MediaSourceId": source_id, "PlaySessionId": session,
        "PlayMethod": "DirectPlay", "PositionTicks": crate::jellyfin::seconds_to_ticks(pos), "IsPaused": paused
    });
    let body = std::sync::Arc::new(body);
    // The profile playing now, even if another is picked before VLC closes.
    let watch = watch_key.and_then(|k| Some((active_user(app)?, k)));
    let app = app.clone();
    let _ = j.report("start", &body(start, false)).await;
    tauri::async_runtime::spawn(async move {
        let http = reqwest::Client::builder().timeout(Duration::from_secs(3)).build().unwrap_or_default();
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let _ = child.wait();
            let _ = tx.send(());
        });
        let mut last = crate::vlc::Status { time: start, length: 0.0, paused: false };
        let mut ticks = 0u32;
        loop {
            tokio::select! {
                _ = &mut rx => break,
                _ = tokio::time::sleep(Duration::from_secs(5)) => {
                    if let Some(s) = crate::vlc::status(&http, port, &password).await {
                        if s.length > 0.0 { last = s; }
                    }
                    ticks += 1;
                    if ticks % 2 == 0 {
                        if let Some(j) = jf(&app) { let _ = j.report("progress", &body(last.time, last.paused)).await; }
                        // Watch now: kept as it goes, so a crash doesn't lose the session.
                        if let Some((uid, key)) = &watch {
                            if let Err(e) = crate::watchnow::progress_as(&app, uid, key, last.time, Some(last.length).filter(|l| *l > 0.0)) {
                                log(&app, Level::Amber, e);
                            }
                        }
                    }
                }
            }
        }
        if let Some(j) = jf(&app) {
            let _ = j.report("stop", &body(last.time, false)).await;
        }
        if let Some((uid, key)) = watch {
            if let Err(e) = crate::watchnow::leave_as(&app, &uid, &key, last.time, Some(last.length).filter(|l| *l > 0.0)).await {
                log(&app, Level::Amber, format!("VLC closed, but the Watch now download wasn't paused: {e}"));
            }
        }
    });
    Ok(())
}

/// Each library folder with what's in it and the free space on its drive.
pub async fn library_stats(app: &AppHandle) -> Vec<Value> {
    let mut out = Vec::new();
    for (id, movies, list) in [(AppId::Radarr, true, "movie"), (AppId::Sonarr, false, "series")] {
        let Some(a) = arr(app, id) else { continue };
        let items = a.list(list).await.unwrap_or_default();
        for r in a.list("rootfolder").await.unwrap_or_default() {
            let Some(path) = r["path"].as_str() else { continue };
            let prefix = path.trim_end_matches(['\\', '/']).to_ascii_lowercase();
            let inside: Vec<&Value> = items
                .iter()
                .filter(|i| i["path"].as_str().is_some_and(|p| p.to_ascii_lowercase().starts_with(&prefix)))
                .collect();
            let have = inside
                .iter()
                .filter(|i| if movies { i["hasFile"] == true } else { i["statistics"]["episodeFileCount"].as_i64().unwrap_or(0) > 0 })
                .count();
            let size: i64 = inside
                .iter()
                .map(|i| if movies { i["sizeOnDisk"].as_i64().unwrap_or(0) } else { i["statistics"]["sizeOnDisk"].as_i64().unwrap_or(0) })
                .sum();
            let (name, _) = library_for(path, movies);
            out.push(json!({
                "path": path, "name": name, "app": id.label(),
                "tracked": inside.len(), "have": have, "size": size,
                "free": r["freeSpace"], "total": r["totalSpace"], "accessible": r["accessible"],
            }));
        }
    }
    out
}

pub async fn scan(app: &AppHandle) -> Result<(), String> {
    let j = jf_admin(app).filter(|j| j.user_id.is_some()).ok_or("Sign in to Jellyfin first")?;
    j.scan().await.map_err(|e| e.to_string())?;
    log(app, Level::Ok, "Asked Jellyfin to scan the library for new files.");
    Ok(())
}

pub async fn report(app: &AppHandle, what: &str, body: Value) -> Result<(), String> {
    let j = jf(app).ok_or("Jellyfin isn't installed")?;
    let sent = j.report(what, &body).await.map_err(|e| e.to_string());
    if what == "stop" {
        // A stop report alone left a whole film's segments behind (26 GB, 2026-10-18).
        if let Some(id) = body["PlaySessionId"].as_str().filter(|s| !s.is_empty()) {
            let _ = j.kill_encoding(id).await;
        }
    }
    sent
}

// ---------- Download ----------

/// What Download did: which app took it, and the id it has there.
pub struct Added {
    pub msg: String,
    pub app: AppId,
    pub id: i64,
}

pub async fn get_it(app: &AppHandle, tile: Tile, quality: &str) -> Result<String, String> {
    let added = add(app, &tile, quality).await?;
    follow(app.clone(), added.app, added.id, tile.key.clone(), tile.title.clone());
    Ok(added.msg)
}

/// Adds a title to Radarr or Sonarr and starts the search. Something they
/// already track is searched again instead of added twice.
pub async fn add(app: &AppHandle, tile: &Tile, quality: &str) -> Result<Added, String> {
    let anime = tile.kind == "anime";
    let anime_movie = anime && tile.format.as_deref() == Some("MOVIE");
    if tile.kind == "movie" || anime_movie {
        let a = arr(app, AppId::Radarr).ok_or("Radarr isn't set up")?;
        let lookup = match tile.tmdb {
            Some(tmdb) => a.get_one(&format!("movie/lookup/tmdb?tmdbId={tmdb}")).await.map_err(|e| format!("Radarr couldn't look it up: {e}"))?,
            None => {
                let found = a.list(&format!("movie/lookup?term={}", enc(&tile.title))).await.map_err(|e| format!("Radarr couldn't look it up: {e}"))?;
                library::pick_lookup(&found, tile.year).ok_or_else(|| format!("Radarr couldn't find {}", tile.title))?
            }
        };
        let title = lookup["title"].as_str().unwrap_or(&tile.title).to_string();
        if let Some(id) = lookup["id"].as_i64().filter(|i| *i > 0) {
            // Stopped earlier (switched off): switch it back on, or a found copy isn't taken.
            if let Ok(mut m) = a.get_one(&format!("movie/{id}")).await {
                if m["monitored"] != true {
                    m["monitored"] = json!(true);
                    let _ = a.update(&format!("movie/{id}"), &m).await;
                }
            }
            let _ = a.command_with(json!({ "name": "MoviesSearch", "movieIds": [id] })).await;
            return Ok(Added { msg: format!("{title} is already in Radarr. Searching again."), app: AppId::Radarr, id });
        }
        let root = library::pick_root(&a.list("rootfolder").await.unwrap_or_default(), anime).ok_or("Radarr has no movie folder yet. Add one in Settings > Library & storage.")?;
        let profile = library::profile_for(&a.list("qualityprofile").await.unwrap_or_default(), quality).ok_or("Radarr has no quality profile")?;
        let made = a.create("movie", &library::movie_payload(lookup, profile, &root)).await.map_err(|e| format!("Radarr couldn't add {title}: {e}"))?;
        let msg = format!("Added {title} to Radarr. It's searching now.");
        log(app, Level::Ok, &msg);
        return Ok(Added { msg, app: AppId::Radarr, id: made["id"].as_i64().unwrap_or(0) });
    }
    let (a, lookup, _, season_entry) = series_lookup(app, tile).await?;
    let title = lookup["title"].as_str().unwrap_or(&tile.title).to_string();
    if let Some(id) = lookup["id"].as_i64().filter(|i| *i > 0) {
        if let Ok(mut s) = a.get_one(&format!("series/{id}")).await {
            if s["monitored"] != true {
                s["monitored"] = json!(true);
                let _ = a.update(&format!("series/{id}"), &s).await;
            }
        }
        let _ = a.command_with(json!({ "name": "SeriesSearch", "seriesId": id })).await;
        return Ok(Added { msg: format!("{title} is already in Sonarr. Searching again."), app: AppId::Sonarr, id });
    }
    let root = library::pick_root(&a.list("rootfolder").await.unwrap_or_default(), anime).ok_or("Sonarr has no series folder yet. Add one in Settings > Library & storage.")?;
    let profile = library::profile_for(&a.list("qualityprofile").await.unwrap_or_default(), quality).ok_or("Sonarr has no quality profile")?;
    let mut payload = library::series_payload(lookup, profile, &root, anime);
    if anime {
        payload["addOptions"]["monitor"] = json!(library::anime_monitor(season_entry));
        // Only that season: a later one isn't fetched by itself.
        payload["monitorNewItems"] = json!("none");
    }
    let made = a.create("series", &payload).await.map_err(|e| format!("Sonarr couldn't add {title}: {e}"))?;
    let msg = format!("Added {title} to Sonarr. It's searching now.");
    log(app, Level::Ok, &msg);
    Ok(Added { msg, app: AppId::Sonarr, id: made["id"].as_i64().unwrap_or(0) })
}

/// Sonarr's match for a series or anime tile: (Sonarr, its lookup, anime,
/// whether the tile was a later-season entry like "X Season 3"). The lookup
/// carries an id when Sonarr already has the show.
pub(crate) async fn series_lookup(app: &AppHandle, tile: &Tile) -> Result<(Arr, Value, bool, bool), String> {
    let anime = tile.kind == "anime";
    let a = arr(app, AppId::Sonarr).ok_or("Sonarr isn't set up")?;
    let (base, season_entry) = if anime { library::base_title(&tile.title) } else { (tile.title.clone(), false) };
    let term = match tile.tvdb {
        Some(id) if !anime => format!("tvdb:{id}"),
        _ => base.clone(),
    };
    let found = a.list(&format!("series/lookup?term={}", enc(&term))).await.map_err(|e| format!("Sonarr couldn't look it up: {e}"))?;
    let year = if season_entry { None } else { tile.year };
    let lookup = library::pick_lookup(&found, year).ok_or_else(|| format!("Sonarr couldn't find {base}"))?;
    Ok((a, lookup, anime, season_entry))
}

/// Follows one Download in the background: shows progress on its poster, and
/// when the file lands, has Jellyfin pick it up and says it's ready.
pub fn follow(app: AppHandle, owner: AppId, id: i64, key: String, title: String) {
    if id <= 0 {
        return;
    }
    tauri::async_runtime::spawn(async move {
        app.state::<Media>().following.lock().unwrap().insert((owner, id));
        follow_until_ready(app.clone(), owner, id, key, title).await;
        app.state::<Media>().following.lock().unwrap().remove(&(owner, id));
    });
}

async fn follow_until_ready(app: AppHandle, owner: AppId, id: i64, key: String, title: String) {
    {
        use tauri::Emitter;
        let Some(a) = arr(&app, owner) else { return };
        let series = owner == AppId::Sonarr;
        let files = |v: &Value| if series { v["statistics"]["episodeFileCount"].as_i64().unwrap_or(0) } else { (v["hasFile"] == true) as i64 };
        let path = if series { format!("series/{id}") } else { format!("movie/{id}") };
        let start = a.get_one(&path).await.map(|v| files(&v)).unwrap_or(0);
        // Up to 6 hours: long enough for a slow download, short enough not to linger.
        for _ in 0..2160 {
            tokio::time::sleep(Duration::from_secs(10)).await;
            let Ok(item) = a.get_one(&path).await else { continue };
            if files(&item) > start {
                if let Some(j) = jf_admin(&app).filter(|j| j.user_id.is_some()) {
                    let _ = j.scan().await;
                }
                let msg = format!("{title} is ready to watch.");
                log(&app, Level::Ok, &msg);
                let _ = app.emit("get-progress", json!({ "key": key, "progress": 1.0, "status": "Ready to watch", "ready": true }));
                use tauri_plugin_notification::NotificationExt;
                let _ = app.notification().builder().title("Chillarr").body(&msg).show();
                // Jellyfin needs a moment to add it; then the screen redraws.
                tokio::time::sleep(Duration::from_secs(8)).await;
                let _ = app.emit("get-progress", json!({ "key": key, "refresh": true }));
                return;
            }
            let q = if series { "queue?includeSeries=true&pageSize=200" } else { "queue?includeMovie=true&pageSize=200" };
            let field = if series { "seriesId" } else { "movieId" };
            let progress = a.get_one(q).await.ok().and_then(|v| {
                v["records"].as_array()?.iter().find(|r| r[field].as_i64() == Some(id)).map(|r| {
                    let size = r["size"].as_f64().unwrap_or(0.0);
                    let left = r["sizeleft"].as_f64().unwrap_or(size);
                    if size > 0.0 { (size - left) / size } else { 0.0 }
                })
            });
            let released = series || item["isAvailable"].as_bool().unwrap_or(true);
            let status = library::wanted_status(released, false, progress);
            let _ = app.emit("get-progress", json!({ "key": key, "progress": progress, "status": status }));
        }
    }
}

/// My List: the Jellyfin favourites (what you have), then what you saved that
/// isn't here yet, with where its download stands. A saved title that has
/// since arrived is made a favourite and leaves saved.json, so the list lives
/// in Jellyfin for everything Jellyfin can hold.
async fn my_list_row(app: &AppHandle, j: Option<&Jf>, own: &(dyn Fn(Vec<Tile>) -> Vec<Tile> + Sync)) -> Vec<Tile> {
    let media = app.state::<Media>();
    let list = media.saved_list();
    let keys: Vec<String> = list.iter().map(|t| t.key.clone()).collect();
    let marked = own(list);
    let mut arrived = Vec::new();
    if let Some(j) = j {
        for (k, t) in keys.iter().zip(&marked) {
            if let Some(id) = &t.jf_id {
                if j.set_favourite(id, true).await.is_ok() {
                    arrived.push(k.clone());
                }
            }
        }
    }
    media.forget_saved(&arrived);
    let favs: Vec<Tile> = match j {
        Some(j) => j.favourites().await.unwrap_or_default().iter().map(|v| tiles::from_jf(v, &j.base)).collect(),
        None => vec![],
    };
    let waiting = marked.into_iter().filter(|t| t.jf_id.is_none());
    favs.into_iter().chain(waiting).collect()
}

/// The My List tab: only that row, so it doesn't wait on the rest of Home.
/// The library (all of it, anime included) says which saved titles have
/// arrived. Completed comes after, from `media_completed`.
async fn my_list_page(app: &AppHandle, j: Option<&Jf>) -> Vec<Row> {
    let mut library = Vec::new();
    if let Some(j) = j {
        library = jf_tiles(j, "recursive=true&includeItemTypes=Movie,Series&sortBy=SortName&limit=1000").await;
    }
    let tracked = tracked(app).await;
    let own = |v: Vec<Tile>| mark_tracked(crate::catalog::merge_owned(v, &library), &tracked);
    row("saved", "My List", my_list_row(app, j, &own).await).into_iter().collect()
}

/// The keys and Jellyfin ids on My List, for its button in the detail panel.
pub async fn my_list_marks(app: &AppHandle) -> (Vec<String>, Vec<String>) {
    let keys = app.state::<Media>().saved_keys();
    let ids = match jf(app).filter(|j| j.user_id.is_some()) {
        Some(j) => j.favourites().await.unwrap_or_default().iter().filter_map(|v| v["Id"].as_str().map(String::from)).collect(),
        None => vec![],
    };
    (keys, ids)
}

/// Adds or removes a title from My List. A library title is a Jellyfin
/// favourite; anything else is kept by Chillarr until it arrives.
pub async fn toggle_my_list(app: &AppHandle, tile: Tile, on: bool) -> Result<bool, String> {
    if let Some(id) = &tile.jf_id {
        let j = jf(app).filter(|j| j.user_id.is_some()).ok_or("Sign in to Jellyfin first.")?;
        j.set_favourite(id, on).await.map_err(|e| format!("Jellyfin didn't take it: {e:?}"))?;
        return Ok(on);
    }
    let media = app.state::<Media>();
    let has = media.saved_keys().contains(&tile.key);
    Ok(if has == on { on } else { media.toggle_saved(tile) })
}

/// Everything you asked for that isn't watchable yet, with where it stands.
pub async fn my_list(app: &AppHandle) -> Vec<Tile> {
    crate::waiting::list(app).await.into_iter().take(40).map(|w| w.tile).collect()
}

fn enc(term: &str) -> String {
    term.bytes().map(|b| if b.is_ascii_alphanumeric() || b == b':' { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

// ---------- Tutorial steps ----------

/// The folders Radarr and Sonarr save into.
pub async fn folders(app: &AppHandle) -> Value {
    let mut out = json!({ "movies": [], "series": [] });
    for (id, key) in [(AppId::Radarr, "movies"), (AppId::Sonarr, "series")] {
        if let Some(a) = arr(app, id) {
            out[key] = json!(a.list("rootfolder").await.unwrap_or_default().iter().filter_map(|r| r["path"].as_str()).collect::<Vec<_>>());
        }
    }
    out
}

/// Adds the folders the user picked (creating them if needed) to Radarr/Sonarr
/// and Jellyfin. Folders already there are left alone.
pub async fn set_folders(app: &AppHandle, movies: Vec<String>, series: Vec<String>) -> Result<(), String> {
    let norm = |p: &String| p.trim().trim_end_matches(['\\', '/']).to_ascii_lowercase();
    if movies.iter().any(|m| series.iter().any(|s| norm(m) == norm(s))) {
        return Err("Movies and series need different folders, or Radarr and Sonarr mix them up. Use \"Everything in one place\" to have Chillarr make both inside one folder.".into());
    }
    let mut waiting_for: Vec<&str> = Vec::new();
    for (id, list) in [(AppId::Radarr, movies), (AppId::Sonarr, series)] {
        let list: Vec<String> = list.into_iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect();
        if list.is_empty() {
            continue;
        }
        // A fresh install may never have run, or just not be running: start it first.
        let mut a = arr(app, id);
        let reachable = match &a {
            Some(x) => x.list("rootfolder").await.is_ok(),
            None => false,
        };
        if !reachable {
            crate::fix::ensure_running(app, id).await;
            a = arr(app, id);
        }
        let Some(a) = a else {
            remember_pending(app, id, &list);
            waiting_for.push(id.label());
            continue;
        };
        let Ok(roots) = a.list("rootfolder").await else {
            remember_pending(app, id, &list);
            waiting_for.push(id.label());
            continue;
        };
        let have: Vec<String> = roots.iter().filter_map(|r| r["path"].as_str().map(|s| s.to_ascii_lowercase())).collect();
        for path in list.into_iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()) {
            if have.contains(&path.to_ascii_lowercase()) {
                continue;
            }
            std::fs::create_dir_all(&path).map_err(|e| format!("Couldn't create {path}: {e}"))?;
            a.create("rootfolder", &json!({ "path": path })).await.map_err(|e| format!("{} couldn't use {path}: {e}", id.label()))?;
            log(app, Level::Ok, format!("Added {path} to {}.", id.label()));
        }
    }
    if jf_admin(app).is_some_and(|j| j.user_id.is_some()) {
        ensure_libraries(app).await?;
    }
    if !waiting_for.is_empty() {
        log(app, Level::Amber, format!("{} isn't ready yet. Chillarr saved your folders and adds them as soon as it is (Fix everything does it too).", waiting_for.join(" and ")));
    }
    Ok(())
}

fn pending_path(app: &AppHandle) -> PathBuf {
    app.state::<Media>().dir.join("pending_folders.json")
}

/// Folders for an app that wasn't ready, kept until it is.
fn remember_pending(app: &AppHandle, id: AppId, list: &[String]) {
    let path = pending_path(app);
    let mut all: HashMap<String, Vec<String>> = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    all.insert(id.label().to_string(), list.to_vec());
    let _ = std::fs::write(&path, serde_json::to_string(&all).unwrap_or_default());
}

/// Adds the saved folders to apps that are ready now. Returns what it did.
pub async fn apply_pending_folders(app: &AppHandle) -> Vec<String> {
    let path = pending_path(app);
    let Some(all) = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<HashMap<String, Vec<String>>>(&t).ok()) else { return vec![] };
    let mut left = all.clone();
    let mut done = Vec::new();
    for (name, list) in all {
        let id = if name == "Radarr" { AppId::Radarr } else { AppId::Sonarr };
        let Some(a) = arr(app, id) else { continue };
        let Ok(roots) = a.list("rootfolder").await else { continue };
        let have: Vec<String> = roots.iter().filter_map(|r| r["path"].as_str().map(|s| s.to_ascii_lowercase())).collect();
        let mut ok = true;
        for p in &list {
            if have.contains(&p.to_ascii_lowercase()) {
                continue;
            }
            if std::fs::create_dir_all(p).is_err() || a.create("rootfolder", &json!({ "path": p })).await.is_err() {
                ok = false;
                continue;
            }
            let msg = format!("Added {p} to {name}.");
            log(app, Level::Ok, &msg);
            done.push(msg);
        }
        if ok {
            left.remove(&name);
        }
    }
    if left.is_empty() {
        let _ = std::fs::remove_file(&path);
    } else {
        let _ = std::fs::write(&path, serde_json::to_string(&left).unwrap_or_default());
    }
    if !done.is_empty() && jf_admin(app).is_some_and(|j| j.user_id.is_some()) {
        let _ = ensure_libraries(app).await;
    }
    done
}

/// Where qBittorrent saves downloads.
pub fn download_folder() -> Option<String> {
    crate::discover::qbit_save_path()
}

/// A value that can go on one line of qBittorrent.ini. A line break would
/// end the value and start a setting of its own.
fn ini_safe(value: &str) -> bool {
    !value.contains(['\r', '\n'])
}

/// Points qBittorrent's downloads at a folder. qBittorrent only reads its
/// settings file at start, so a running one is closed for a moment and started again.
pub async fn set_download_folder(app: &AppHandle, path: &str) -> Result<(), String> {
    let path = path.trim().trim_end_matches(['\\', '/']).to_string();
    if path.is_empty() {
        return Err("Pick a folder".into());
    }
    if !ini_safe(&path) {
        return Err("That folder name has a line break in it. Pick another folder.".into());
    }
    std::fs::create_dir_all(&path).map_err(|e| format!("Couldn't create {path}: {e}"))?;
    let dir = std::env::var_os("APPDATA").map(PathBuf::from).ok_or("No AppData folder")?.join("qBittorrent");
    let file = dir.join("qBittorrent.ini");
    let was_running = crate::procs::process_running("qbittorrent.exe");
    if was_running {
        tokio::task::spawn_blocking(|| crate::procs::force_close("qbittorrent.exe")).await.ok();
    }
    let ini = std::fs::read_to_string(&file).unwrap_or_default();
    let forward = path.replace('\\', "/");
    let ini = crate::wire::set_ini_value(&ini, "BitTorrent", "Session\\DefaultSavePath", &forward);
    let ini = crate::wire::set_ini_value(&ini, "BitTorrent", "Session\\TempPathEnabled", "false");
    std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&file, ini)).map_err(|e| format!("Couldn't change qBittorrent's settings: {e}"))?;
    if was_running {
        if let Some(exe) = app.state::<Core>().config(AppId::QBittorrent).and_then(|c| c.exe) {
            let _ = crate::procs::start_qbit(&exe);
        }
    }
    log(app, Level::Ok, format!("Downloads now go to {path}."));
    Ok(())
}

/// Prowlarr's indexers by name, and whether the recommended ones are there.
pub async fn indexers(app: &AppHandle) -> Value {
    let Some(p) = arr(app, AppId::Prowlarr) else { return json!({ "have": [], "ok": false }) };
    let list = p.list("indexer").await.unwrap_or_default();
    json!({
        "ok": true,
        "have": list.iter().map(|i| json!({ "name": i["name"], "definition": i["definitionName"] })).collect::<Vec<_>>(),
    })
}

/// Adds a public indexer from Prowlarr's catalogue by its definition name
/// (e.g. "1337x", "nyaasi"). Cloudflare-protected ones get FlareSolverr's tag.
pub async fn add_indexer(app: &AppHandle, definition: &str) -> Result<String, String> {
    let p = arr(app, AppId::Prowlarr).ok_or("Prowlarr isn't set up")?;
    if p.list("indexer").await.unwrap_or_default().iter().any(|i| i["definitionName"] == definition) {
        return Ok("Already added".into());
    }
    let schemas = p.list("indexer/schema").await.map_err(|e| format!("Prowlarr: {e}"))?;
    let mut item = schemas.into_iter().find(|s| s["definitionName"] == definition).ok_or_else(|| format!("Prowlarr doesn't know {definition}"))?;
    let profile = p.list("appprofile").await.unwrap_or_default().first().and_then(|x| x["id"].as_i64()).unwrap_or(1);
    item["appProfileId"] = json!(profile);
    item["enable"] = json!(true);
    if behind_cloudflare(definition) {
        // These sit behind Cloudflare: FlareSolverr has to be running and set up in Prowlarr first.
        let (tag, up) = flare_tag(app, &p).await;
        match tag {
            Some(t) => item["tags"] = json!([t]),
            None => return Err(if up { "FlareSolverr is running but Prowlarr couldn't use it. Press Fix everything, then try again.".into() } else { "This site needs FlareSolverr, which isn't installed or won't start. Install it in step 1 (or Settings > System), then try again.".into() }),
        }
    }
    let name = item["name"].as_str().unwrap_or(definition).to_string();
    // Prowlarr tests the site when it's added. A mirror that's blocked or behind
    // a challenge FlareSolverr can't pass may still have a sibling that works,
    // so each of Prowlarr's other mirrors is tried before giving up.
    let mirrors: Vec<String> =
        item["indexerUrls"].as_array().map(|a| a.iter().filter_map(|u| u.as_str().map(String::from)).collect()).unwrap_or_default();
    let mut result = p.create("indexer", &item).await.map(|_| ());
    for mirror in mirrors.iter().skip(1) {
        let Err(e) = &result else { break };
        if !blocked_error(&e.to_string()) {
            break;
        }
        log(app, Level::Amber, format!("{name} didn't get through. Trying its mirror {mirror}..."));
        item = crate::wire::fill_fields(item, &[("baseUrl", json!(mirror))]);
        result = p.create("indexer", &item).await.map(|_| ());
    }
    result.map_err(|e| {
        let e = e.to_string();
        if e.contains("SSL connection") || e.contains("DNS") {
            format!("Prowlarr couldn't reach {name}. Your internet provider is probably blocking it. Setting Windows DNS to 1.1.1.1 (Cloudflare) usually fixes that, or skip {name}.")
        } else if e.contains("CloudFlare") || e.contains("Cloudflare") {
            format!("{name} is behind Cloudflare and FlareSolverr couldn't get through on any of its mirrors. A VPN usually helps, or skip {name}.")
        } else {
            format!("Prowlarr couldn't add {name}: {e}")
        }
    })?;
    let _ = p.command("ApplicationIndexerSync").await;
    let msg = format!("Added {name} to Prowlarr and passed it to Sonarr and Radarr.");
    log(app, Level::Ok, &msg);
    Ok(msg)
}

/// Prowlarr sites that put Cloudflare in front of their searches, so they
/// only work through FlareSolverr. Tokyo Toshokan started on 2026-10-19
/// (its RSS still passes Prowlarr's test; searches got blocked).
pub(crate) fn behind_cloudflare(definition: &str) -> bool {
    ["1337x", "eztv", "tokyotosho"].contains(&definition)
}

/// Prowlarr's FlareSolverr tag (setting the proxy up if it's missing), and
/// whether FlareSolverr is running.
pub(crate) async fn flare_tag(app: &AppHandle, p: &crate::arr::Arr) -> (Option<i64>, bool) {
    let find = |list: Vec<Value>| list.iter().find(|x| x["implementation"] == "FlareSolverr").and_then(|x| x["tags"][0].as_i64());
    let mut tag = find(p.list("indexerproxy").await.unwrap_or_default());
    let up = crate::fix::ensure_running(app, AppId::FlareSolverr).await;
    if tag.is_none() && up {
        crate::fix::wire_now(app).await;
        tag = find(p.list("indexerproxy").await.unwrap_or_default());
    }
    (tag, up)
}

/// Sites added before they needed FlareSolverr (or before Chillarr knew)
/// get its tag, so their searches stop failing on Cloudflare.
pub async fn flare_for_blocked(app: &AppHandle) {
    let Some(p) = arr(app, AppId::Prowlarr) else { return };
    let Ok(list) = p.list("indexer").await else { return };
    let missing: Vec<Value> = list
        .into_iter()
        .filter(|i| i["definitionName"].as_str().is_some_and(behind_cloudflare))
        .collect();
    let (mut tag, mut up) = (None, false);
    for mut i in missing {
        if tag.is_none() {
            (tag, up) = flare_tag(app, &p).await;
        }
        let Some(t) = tag else { break };
        if i["tags"].as_array().is_some_and(|ts| ts.iter().any(|x| x.as_i64() == Some(t))) {
            continue;
        }
        let name = i["name"].as_str().unwrap_or("A site").to_string();
        let mut tags = i["tags"].as_array().cloned().unwrap_or_default();
        tags.push(json!(t));
        i["tags"] = json!(tags);
        let Some(id) = i["id"].as_i64() else { continue };
        match p.update(&format!("indexer/{id}"), &i).await {
            Ok(()) => log(app, Level::Ok, format!("{name} now searches through FlareSolverr (it's behind Cloudflare).")),
            Err(e) => log(app, Level::Amber, format!("Couldn't send {name} through FlareSolverr: {e:?}")),
        }
    }
    if tag.is_none() && up {
        log(app, Level::Amber, "Some sites need FlareSolverr, but Prowlarr couldn't use it. Press Fix everything.");
    }
}

/// An add that failed because the site couldn't be reached (ISP block) or
/// Cloudflare stopped it: worth trying another mirror.
fn blocked_error(e: &str) -> bool {
    ["SSL connection", "DNS", "CloudFlare", "Cloudflare"].iter().any(|s| e.contains(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_folder_cant_add_ini_lines() {
        assert!(ini_safe(r"C:\Users\anime\Downloads"));
        assert!(!ini_safe("C:\\Downloads\n[Preferences]\nWebUI\\LocalHostAuth=false"));
        assert!(!ini_safe("C:\\Downloads\rX"));
    }

    #[test]
    fn continue_watching_puts_watch_now_first() {
        let t = |key: &str, jf: Option<&str>| Tile { jf_id: jf.map(String::from), ..Tile::new(key, "movie", key) };
        let watch = vec![t("movie:2", Some("p2")), t("movie:1", Some("p1")), t("movie:3", None)];
        let cont = vec![t("jf:a", Some("a")), t("jf:p1", Some("p1")), t("jf:b", Some("b"))];
        let got: Vec<String> = continue_first(watch, cont).into_iter().map(|t| t.key).collect();
        assert_eq!(got, vec!["movie:2", "movie:1", "movie:3", "jf:a", "jf:b"], "Watch now first, in its order; its placeholder never twice");
        assert_eq!(continue_first(vec![], vec![t("jf:a", Some("a"))]).len(), 1);
    }

    #[test]
    fn search_matches_library_titles_loosely() {
        assert_eq!(match_rank("Iron Man", "iron man"), Some(0));
        assert_eq!(match_rank("Iron Man", "ironman"), Some(0));
        assert_eq!(match_rank("Spider-Man: No Way Home", "spiderman"), Some(1));
        assert_eq!(match_rank("The Amazing Spider-Man", "spider man"), Some(2));
        assert_eq!(match_rank("Iron Man", "man iron"), Some(3));
        assert_eq!(match_rank("Iron Man", "iron man 2"), None);
        assert_eq!(match_rank("Iron Man", "--"), None);
        // Two letters only match the start, not the middle of every title.
        assert_eq!(match_rank("Iron Man", "ir"), Some(1));
        assert_eq!(match_rank("The Batman", "an"), None);
    }

    #[test]
    fn only_reachability_errors_try_another_mirror() {
        assert!(blocked_error("Unable to access 1337x.to, blocked by CloudFlare Protection."));
        assert!(blocked_error("The SSL connection could not be established"));
        assert!(blocked_error("No such host is known (DNS)"));
        assert!(!blocked_error("Validation failed: Name should be unique"));
    }

    #[test]
    fn titles_radarr_already_has_say_so() {
        let tracked = vec![(Some(5), None, "owned".to_string(), tracked_label(true, true)), (Some(6), None, "wanted".to_string(), tracked_label(false, true)), (None, Some(9), "frieren".to_string(), tracked_label(false, false))];
        let tiles = vec![
            Tile { tmdb: Some(5), ..Tile::new("tmdb:5", "movie", "Owned") },
            Tile { tmdb: Some(6), ..Tile::new("tmdb:6", "movie", "Wanted") },
            Tile { tmdb: Some(7), ..Tile::new("tmdb:7", "movie", "New") },
            Tile::new("anilist:1", "anime", "Frieren"),
            Tile { jf_id: Some("x".into()), tmdb: Some(5), ..Tile::new("jf:x", "movie", "Owned") },
        ];
        let out = mark_tracked(tiles, &tracked);
        assert_eq!(out[0].status.as_deref(), Some("Adding to library"), "a file Jellyfin doesn't list yet isn't playable");
        assert_eq!(out[1].status.as_deref(), Some("Looking for a copy"));
        assert_eq!(out[2].status, None);
        assert_eq!(out[3].status.as_deref(), Some("Not out yet"), "anime matched by title");
        assert_eq!(out[4].status, None, "a playable copy needs no label");
    }

    #[test]
    fn root_folders_map_to_jellyfin_libraries() {
        assert_eq!(library_for(r"C:\Users\anime\Desktop\Desktop\Movies\Normal", true), ("Movies".into(), "movies"));
        assert_eq!(library_for(r"C:\Users\anime\Desktop\Desktop\Movies\Anime", true), ("Anime Movies".into(), "movies"));
        assert_eq!(library_for(r"C:\Users\anime\Desktop\Desktop\Series\Normal", false), ("Series".into(), "tvshows"));
        assert_eq!(library_for(r"C:\Users\anime\Desktop\Desktop\Series\Anime\", false), ("Anime".into(), "tvshows"));
    }
}
