//! Jellyfin's own web app (what browsers, phones and TVs that load the
//! server's pages see), served from Chillarr's copy of it: Chillarr's look,
//! Chillarr's logo, and search that covers every library.
//!
//! The install's `jellyfin-web` folder is under Program Files, which the user
//! can't write, so Chillarr keeps a patched copy in its app data folder and
//! starts Jellyfin with `--webdir` pointing at it. Each copy lives in a folder
//! named after a stamp of the installed files plus Chillarr's skin, so a
//! Jellyfin update (or a new skin) makes a fresh copy beside the one the
//! running server still serves, and nothing is swapped under it.
//!
//! What clients use to recognise the server is left alone: the API, the
//! product name in /System/Info/Public, the server id, and the web manifest's
//! name. Only pictures, a stylesheet and one line of the search page change.
//!
//! Native TV apps that don't use jellyfin-web (Android TV, Roku, the webOS
//! and Tizen apps ship their own copy) are out of reach: they keep their
//! stock look and per-library search.

use crate::log::log;
use crate::model::{AppId, Level};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Manager};

const SKIN_CSS: &str = include_str!("../jfskin/chillarr.css");
const FONT: &[u8] = include_bytes!("../../src/fonts/Outfit.woff2");
const LOGO_SVG: &[u8] = include_bytes!("../../src/logo.svg");
const FAVICON: &[u8] = include_bytes!("../icons/icon.ico");
const BANNER_LIGHT: &[u8] = include_bytes!("../jfskin/banner-light.png");
const BANNER_DARK: &[u8] = include_bytes!("../jfskin/banner-dark.png");
const ICON_72: &[u8] = include_bytes!("../jfskin/icon-72.png");
const ICON_114: &[u8] = include_bytes!("../jfskin/icon-114.png");
const ICON_144: &[u8] = include_bytes!("../jfskin/icon-144.png");
const ICON_180: &[u8] = include_bytes!("../jfskin/icon-180.png");
const ICON_440: &[u8] = include_bytes!("../jfskin/icon-440.png");
const ICON_512: &[u8] = include_bytes!("../jfskin/icon-512.png");
const ICON_536: &[u8] = include_bytes!("../jfskin/icon-536.png");

mod assets {
    // Every file of the window's pages (build.rs), as they are in src/.
    include!(concat!(env!("OUT_DIR"), "/web_assets.rs"));
}
use assets::WEB_ASSETS;

/// One of the window's files, by its path under src/.
fn web_asset(rel: &str) -> Option<&'static [u8]> {
    WEB_ASSETS.iter().find(|(p, _)| *p == rel).map(|(_, b)| *b)
}

/// Where the copy of Chillarr's own pages sits inside Jellyfin's web folder
/// (served at /web/<this>/). Named after the stamp, so a browser never mixes
/// an old version's scripts with a new one's.
pub fn app_folder(stamp: &str) -> String {
    format!("chillarr-{stamp}")
}

/// Marks the loader script in jellyfin-web's index.html.
const LOADER_MARK: &str = "data-chillarr-loader";

/// Puts src/web/loader.js in jellyfin-web's head: once signed in, Jellyfin's
/// home is Chillarr's.
pub fn inject_loader(index_html: &str, loader: &str, folder: &str) -> Patch {
    if index_html.contains(LOADER_MARK) {
        return Patch::Already;
    }
    let js = loader.replace("__CHILLARR_DIR__", folder).replace("</script", r"<\/script");
    patch_once(index_html, "<head>", &format!("<head><script {LOADER_MARK}>{js}</script>"))
}

/// Chillarr's index.html inside Jellyfin: the stand-in for Tauri
/// (src/web/shim.js) runs before any of the page's own scripts.
pub fn inject_shim(index_html: &str) -> Patch {
    patch_once(index_html, "<head>", "<head><script src=\"web/shim.js\"></script>")
}

/// Pictures replaced in the copy: (subfolder, name prefix, extension, bytes).
/// jellyfin-web names most of them `<name>.<hash>.<ext>`, so they are found by
/// prefix; the hashed name is kept so its JS and CSS still point at them.
const LOGOS: [(&str, &str, &str, &[u8]); 12] = [
    ("", "banner-light.", ".png", BANNER_LIGHT),
    ("", "banner-dark.", ".png", BANNER_DARK),
    ("", "icon-transparent.", ".png", ICON_536),
    ("", "notificationicon.", ".png", ICON_440),
    ("", "touchicon.", ".png", ICON_180),
    ("", "touchicon144.", ".png", ICON_144),
    ("", "favicon.", ".ico", FAVICON),
    ("favicons", "touchicon.", ".png", ICON_180),
    ("favicons", "touchicon72.", ".png", ICON_72),
    ("favicons", "touchicon114.", ".png", ICON_114),
    ("favicons", "touchicon144.", ".png", ICON_144),
    ("favicons", "touchicon512.", ".png", ICON_512),
];

/// jellyfin-web 12.1's search page reads the library it was opened from out
/// of the URL and searches only that one. Titles on the Discover shelves live
/// in their own libraries, so a search from Movies found nothing. Exact text
/// from search.<hash>.chunk.js; a new Jellyfin that minifies it differently
/// isn't guessed at (see `build`).
const SEARCH_FROM: &str = r#"t=e.get("parentId")||void 0,n=e.get("collectionType")||void 0"#;
const SEARCH_TO: &str = r#"t=void 0/*chillarr: every library*/,n=void 0"#;

/// Marks the stylesheet link in index.html, so injecting twice is a no-op.
const LINK_MARK: &str = "data-chillarr";

#[derive(Debug, PartialEq)]
pub enum Patch {
    Applied(String),
    Already,
    /// The expected text isn't there (or is there more than once).
    Unknown,
}

/// Replaces `from` with `to` when `from` occurs exactly once.
pub fn patch_once(text: &str, from: &str, to: &str) -> Patch {
    match text.matches(from).count() {
        1 => Patch::Applied(text.replacen(from, to, 1)),
        0 if text.contains(to) => Patch::Already,
        _ => Patch::Unknown,
    }
}

/// Links chillarr.css at the end of the body, after the theme stylesheet
/// jellyfin-web adds to the head, so equal selectors resolve to the skin.
pub fn inject_css(index_html: &str, stamp: &str) -> Patch {
    if index_html.contains(LINK_MARK) {
        return Patch::Already;
    }
    let link = format!("<link rel=\"stylesheet\" href=\"chillarr.css?v={stamp}\" {LINK_MARK}></body>");
    patch_once(index_html, "</body>", &link)
}

/// Marks the refresher script in index.html.
const FRESH_MARK: &str = "data-chillarr-fresh";

/// The patched files keep the installed files' URLs (their JS and CSS point at
/// them), and Jellyfin sends them with no Cache-Control, so a browser that
/// opened the server before keeps showing its old copies for days: Brave
/// still showed Jellyfin's logo and per-library search on localhost:8096
/// while the LAN address, never opened before, showed Chillarr's. index.html
/// is always re-checked (no-cache), so it carries this: once per copy, fetch
/// each patched file with cache 'reload' (which replaces the stored copy),
/// then reload the page. No storage means no reload, so it can't loop.
const FRESH_JS: &str = r#"(function(){var k="chillarr-fresh",s="__STAMP__",f=__FILES__;try{if(localStorage.getItem(k)===s)return;localStorage.setItem(k,s);if(localStorage.getItem(k)!==s)return}catch(e){return}Promise.all(f.map(function(u){return fetch(u,{cache:"reload"}).catch(function(){})})).then(function(){location.reload()})})();"#;

/// Puts the refresher first in the head, so it runs before anything loads.
pub fn inject_fresh(index_html: &str, stamp: &str, files: &[String]) -> Patch {
    if index_html.contains(FRESH_MARK) {
        return Patch::Already;
    }
    let list = serde_json::to_string(files).unwrap_or_else(|_| "[]".into());
    let js = FRESH_JS.replace("__STAMP__", stamp).replace("__FILES__", &list);
    patch_once(index_html, "<head>", &format!("<head><script {FRESH_MARK}>{js}</script>"))
}

/// A copied file's URL relative to index.html.
fn rel_url(base: &Path, file: &Path) -> String {
    file.strip_prefix(base).unwrap_or(file).to_string_lossy().replace('\\', "/")
}

/// The first line of the copy's chillarr.css: the stamp it was built for,
/// which is how a running Jellyfin is checked for serving the current copy.
pub fn css_head(stamp: &str) -> String {
    format!("/* chillarr-skin {stamp} */\n")
}

/// A short hash of everything a copy is made from: the installed files
/// (relative path, size, modified time) and what Chillarr puts in. Any change
/// means a new copy.
pub fn stamp(installed: &[(String, u64, u64)]) -> String {
    let mut h = Sha256::new();
    let mut files: Vec<_> = installed.iter().collect();
    files.sort();
    for (p, size, mtime) in files {
        h.update(p.as_bytes());
        h.update(size.to_le_bytes());
        h.update(mtime.to_le_bytes());
    }
    h.update(SKIN_CSS.as_bytes());
    h.update(SEARCH_TO.as_bytes());
    h.update(FRESH_JS.as_bytes());
    for (dir, pre, ext, bytes) in LOGOS {
        h.update(format!("{dir}/{pre}*{ext}").as_bytes());
        h.update(bytes);
    }
    h.update(FONT);
    h.update(LOGO_SVG);
    for (p, b) in WEB_ASSETS {
        h.update(p.as_bytes());
        h.update((b.len() as u64).to_le_bytes());
        h.update(b);
    }
    h.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// A copy is needed when the folder for the current stamp doesn't exist yet
/// (it only appears once complete: it's built under `.tmp` and renamed).
pub fn copy_needed(root: &Path, stamp: &str) -> bool {
    !root.join(stamp).join("index.html").is_file()
}

fn walk(base: &Path, rel: &Path, out: &mut Vec<(String, u64, u64)>) -> std::io::Result<()> {
    for e in std::fs::read_dir(base.join(rel))? {
        let e = e?;
        let r = rel.join(e.file_name());
        let md = e.metadata()?;
        if md.is_dir() {
            walk(base, &r, out)?;
        } else {
            let mtime = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
            out.push((r.to_string_lossy().replace('\\', "/"), md.len(), mtime));
        }
    }
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let dest = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_tree(&e.path(), &dest)?;
        } else {
            std::fs::copy(e.path(), dest)?;
        }
    }
    Ok(())
}

fn find_prefixed(dir: &Path, pre: &str, ext: &str) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    rd.flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            n.starts_with(pre) && n.ends_with(ext)
        })
        .map(|e| e.path())
        .collect()
}

/// Every file Chillarr writes into a copy (path relative to the copy, with
/// `/`), made from the installed files and what's compiled in. The same
/// install and stamp always give the same plan, so a copy is checked against
/// a fresh plan and nothing recorded in the user-writable folder is trusted.
pub struct Plan {
    pub files: Vec<(String, Vec<u8>)>,
    pub notes: Vec<String>,
}

/// Works out the patched files from `installed` (read only). A patch whose
/// text isn't found is skipped and reported; the copy is still used, it only
/// lacks that change.
pub fn plan(installed: &Path, stamp: &str) -> std::io::Result<Plan> {
    let mut notes = Vec::new();
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut index = std::fs::read_to_string(installed.join("index.html"))?;
    match inject_css(&index, stamp) {
        Patch::Applied(t) => index = t,
        Patch::Already => {}
        Patch::Unknown => notes.push("Chillarr's look couldn't be added to this Jellyfin version's pages (its index.html has changed), so they keep Jellyfin's own look.".into()),
    }
    files.push(("chillarr.css".into(), format!("{}{SKIN_CSS}", css_head(stamp)).into_bytes()));
    files.push(("chillarr/Outfit.woff2".into(), FONT.to_vec()));
    files.push(("chillarr/logo.svg".into(), LOGO_SVG.to_vec()));

    // Every file written over an installed one, for the refresher.
    let mut changed = Vec::new();
    let mut searched = false;
    for c in find_prefixed(installed, "search.", ".chunk.js") {
        if let Patch::Applied(t) = patch_once(&std::fs::read_to_string(&c)?, SEARCH_FROM, SEARCH_TO) {
            let rel = rel_url(installed, &c);
            files.push((rel.clone(), t.into_bytes()));
            changed.push(rel);
            searched = true;
        }
    }
    if !searched {
        notes.push("Jellyfin's search couldn't be switched to search every library on this Jellyfin version, so a search started inside a library still covers only that library.".into());
    }

    let mut missing = Vec::new();
    for (sub, pre, ext, bytes) in LOGOS {
        let found = find_prefixed(&installed.join(sub), pre, ext);
        if found.is_empty() {
            missing.push(format!("{sub}/{pre}*{ext}").trim_start_matches('/').to_string());
        }
        for f in found {
            let rel = rel_url(installed, &f);
            files.push((rel.clone(), bytes.to_vec()));
            changed.push(rel);
        }
    }
    if !missing.is_empty() {
        notes.push(format!("Some of Jellyfin's logos weren't where Chillarr expected ({}), so those still show Jellyfin's.", missing.join(", ")));
    }
    changed.sort();
    changed.dedup();

    // Chillarr's own pages, and the loader that opens them from Jellyfin's home.
    let folder = app_folder(stamp);
    let mut has_shim = false;
    for (rel, bytes) in WEB_ASSETS {
        // The page, and its copy for older TV browsers (legacy.html, with the
        // scripts build.rs bundled for them): both get the stand-in for Tauri first.
        if *rel == "index.html" || *rel == "legacy.html" {
            let page = String::from_utf8_lossy(bytes);
            if let (Some(_), Patch::Applied(t)) = (web_asset("web/shim.js"), inject_shim(&page)) {
                has_shim |= *rel == "index.html";
                files.push((format!("{folder}/{rel}"), t.into_bytes()));
            }
        } else {
            files.push((format!("{folder}/{rel}"), bytes.to_vec()));
        }
    }
    if let Some(port) = crate::bridge::port() {
        files.push((format!("{folder}/bridge.json"), bridge_json(port)));
    }
    match web_asset("web/loader.js").filter(|_| has_shim) {
        Some(loader) => match inject_loader(&index, &String::from_utf8_lossy(loader), &folder) {
            Patch::Applied(t) => index = t,
            Patch::Already => {}
            Patch::Unknown => notes.push("Chillarr's home couldn't be added to this Jellyfin version's pages, so they open on Jellyfin's own home.".into()),
        },
        None => notes.push("This Chillarr build has no pages for inside Jellyfin, so Jellyfin opens on its own home.".into()),
    }
    if let Patch::Applied(t) = inject_fresh(&index, stamp, &changed) {
        index = t;
    }
    files.push(("index.html".into(), index.into_bytes()));
    Ok(Plan { files, notes })
}

fn write_planned(dir: &Path, rel: &str, bytes: &[u8]) -> std::io::Result<()> {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes)
}

/// Writes back every planned file whose bytes in the copy differ (edited,
/// replaced or deleted since it was built), and says how many it fixed. A
/// handful of small files, so this runs before each Jellyfin start.
pub fn repair(dir: &Path, plan: &Plan) -> std::io::Result<usize> {
    let mut fixed = 0;
    for (rel, bytes) in &plan.files {
        let same = std::fs::read(dir.join(rel)).is_ok_and(|have| Sha256::digest(&have) == Sha256::digest(bytes));
        if !same {
            write_planned(dir, rel, bytes)?;
            fixed += 1;
        }
    }
    Ok(fixed)
}

/// What a build did, in sentences for the log (empty when all went in).
pub struct Built {
    pub dir: PathBuf,
    pub notes: Vec<String>,
    /// Chillarr's files in an existing copy that had changed and were put back.
    pub repaired: usize,
}

/// Makes the patched copy of `installed` under `root/<stamp>` and removes
/// older copies. An existing copy has Chillarr's files checked and put back
/// where they differ: the folder is the user's to write, and what's in it is
/// served to every Jellyfin client.
pub fn build(installed: &Path, root: &Path, stamp: &str) -> std::io::Result<Built> {
    let dir = root.join(stamp);
    let p = plan(installed, stamp)?;
    let mut repaired = 0;
    if copy_needed(root, stamp) {
        let tmp = root.join(format!("{stamp}.tmp"));
        let _ = std::fs::remove_dir_all(&tmp);
        copy_tree(installed, &tmp)?;
        for (rel, bytes) in &p.files {
            write_planned(&tmp, rel, bytes)?;
        }
        std::fs::rename(&tmp, &dir)?;
    } else {
        repaired = repair(&dir, &p)?;
    }
    // Older copies: the one a running Jellyfin still serves can't be removed
    // yet and goes on a later build.
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy() != stamp {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    Ok(Built { dir, notes: p.notes, repaired })
}

fn root(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("jellyfin-web"))
}

/// The installed web app beside jellyfin.exe.
fn installed_web(exe: &Path) -> PathBuf {
    exe.parent().unwrap_or(Path::new(".")).join("jellyfin-web")
}

static BUILDING: Mutex<()> = Mutex::new(());
/// The copy Jellyfin was last started on (or checked against): where the
/// bridge's port is written when it changes.
static CURRENT: Mutex<Option<(PathBuf, String)>> = Mutex::new(None);

fn bridge_json(port: u16) -> Vec<u8> {
    serde_json::json!({ "port": port }).to_string().into_bytes()
}

/// The bridge (bridge.rs) has its port: the pages in the current copy are told.
pub fn bridge_moved(_app: &AppHandle) {
    let Some(port) = crate::bridge::port() else { return };
    let Some((dir, stamp)) = CURRENT.lock().unwrap().clone() else { return };
    let folder = dir.join(app_folder(&stamp));
    if folder.is_dir() {
        let _ = std::fs::write(folder.join("bridge.json"), bridge_json(port));
    }
}
/// The stamp the last notes were logged for, so they're said once per version.
static NOTED: Mutex<String> = Mutex::new(String::new());

/// The copy to start Jellyfin with, made first if needed, or None (logged)
/// when it can't be made: Jellyfin then starts with its own pages as before.
pub async fn prepare(app: &AppHandle, exe: &Path) -> Option<(PathBuf, String)> {
    let installed = installed_web(exe);
    let root = root(app)?;
    let r = tokio::task::spawn_blocking(move || -> Result<(Built, String), String> {
        let _g = BUILDING.lock().unwrap_or_else(|e| e.into_inner());
        let mut files = Vec::new();
        walk(&installed, Path::new(""), &mut files).map_err(|e| format!("couldn't read {}: {e}", installed.display()))?;
        let s = stamp(&files);
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        build(&installed, &root, &s).map(|b| (b, s)).map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()));
    match r {
        Ok((b, s)) => {
            if b.repaired > 0 {
                log(app, Level::Amber, format!("{} of Chillarr's files in its copy of Jellyfin's web pages had been changed since it made them, so they were put back.", b.repaired));
            }
            let mut noted = NOTED.lock().unwrap();
            if *noted != s {
                for n in &b.notes {
                    log(app, Level::Amber, n);
                }
                *noted = s.clone();
            }
            *CURRENT.lock().unwrap() = Some((b.dir.clone(), s.clone()));
            bridge_moved(app);
            Some((b.dir, s))
        }
        Err(e) => {
            let mut noted = NOTED.lock().unwrap();
            if *noted != e {
                log(app, Level::Amber, format!("Couldn't make Chillarr's copy of Jellyfin's web pages ({e}), so Jellyfin shows its own look and searches one library at a time."));
                *noted = e;
            }
            None
        }
    }
}

static RESTARTING: AtomicBool = AtomicBool::new(false);

/// True while Chillarr is restarting Jellyfin onto its copy; the guard
/// leaves Jellyfin alone meanwhile so two servers never start on one data folder.
pub fn restarting() -> bool {
    RESTARTING.load(Ordering::Relaxed)
}

/// Whether the Jellyfin answering at `base` serves the copy for `stamp`.
async fn serving(base: &str, stamp: &str) -> Option<bool> {
    let http = reqwest::Client::builder().timeout(Duration::from_secs(5)).build().ok()?;
    let r = http.get(format!("{}/web/chillarr.css", base.trim_end_matches('/'))).send().await.ok()?;
    if !r.status().is_success() {
        return Some(false);
    }
    Some(r.text().await.ok()?.starts_with(&css_head(stamp)))
}

/// Anyone watching or listening right now (a restart would cut them off).
fn playing(sessions: &serde_json::Value) -> bool {
    sessions.as_array().is_some_and(|a| a.iter().any(|s| !s["NowPlayingItem"].is_null()))
}

/// Every few minutes: a Jellyfin started before Chillarr had its copy (by an
/// older Chillarr, or by hand) is restarted onto the copy, but only while
/// nobody is playing anything. A Windows service can't be given --webdir.
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(90)).await;
        let mut said_service = false;
        let mut tried = String::new();
        loop {
            check(&app, &mut said_service, &mut tried).await;
            tokio::time::sleep(Duration::from_secs(5 * 60)).await;
        }
    });
}

async fn check(app: &AppHandle, said_service: &mut bool, tried: &mut String) {
    let Some(cfg) = app.state::<crate::fix::Core>().config(AppId::Jellyfin) else { return };
    let Some(exe) = cfg.exe.clone() else { return };
    if cfg.service.is_some() {
        if !*said_service {
            log(app, Level::Amber, "Jellyfin runs as a Windows service, so its own pages keep Jellyfin's look and per-library search. Press Fix everything to run it as you, and Chillarr's version takes over.");
            *said_service = true;
        }
        return;
    }
    if !crate::procs::port_open(&cfg.base_url).await {
        return; // The guard starts it, already on the copy.
    }
    let Some((dir, stamp)) = prepare(app, &exe).await else { return };
    // One restart per copy: if Jellyfin still isn't on it afterwards, that
    // was logged, and restarting it every few minutes wouldn't help.
    if *tried == stamp || serving(&cfg.base_url, &stamp).await != Some(false) {
        return;
    }
    // Something else answering on Jellyfin's port isn't ours to restart.
    if !tokio::task::spawn_blocking(|| crate::procs::process_running("jellyfin.exe")).await.unwrap_or(false) {
        return;
    }
    let Some(j) = crate::media::jf_admin(app) else { return };
    match j.get_json("/Sessions").await {
        Ok(s) if !playing(&s) => {}
        _ => return, // Busy, or unknown: try again later rather than cut someone off.
    }
    *tried = stamp.clone();
    RESTARTING.store(true, Ordering::Relaxed);
    let ok = restart(app, &j, &exe, &dir, &cfg.base_url, &stamp).await;
    RESTARTING.store(false, Ordering::Relaxed);
    if ok {
        log(app, Level::Ok, "Restarted Jellyfin with Chillarr's look and logo, and its search now covers every library.");
    }
}

async fn restart(app: &AppHandle, j: &crate::jellyfin::Jf, exe: &Path, dir: &Path, base: &str, stamp: &str) -> bool {
    // A clean shutdown first, so Jellyfin closes its database itself.
    if j.post_json("/System/Shutdown", &serde_json::json!({})).await.is_err() {
        log(app, Level::Amber, "Jellyfin didn't accept Chillarr's request to restart it, so its pages keep Jellyfin's look until it next starts.");
        return false;
    }
    let mut stopped = false;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if !tokio::task::spawn_blocking(|| crate::procs::process_running("jellyfin.exe")).await.unwrap_or(true) {
            stopped = true;
            break;
        }
    }
    if !stopped {
        log(app, Level::Amber, "Jellyfin took over 40 seconds to stop, so Chillarr closed it.");
        crate::procs::force_close("jellyfin.exe");
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    // Jellyfin's search plugin looks for its engine once, as Jellyfin starts.
    crate::meili::start(app).await;
    if let Err(e) = crate::procs::start_jellyfin(exe, Some(dir)) {
        log(app, Level::Red, e);
        return false;
    }
    if !crate::procs::wait_until(base, 120).await {
        log(app, Level::Red, "Chillarr restarted Jellyfin, but it didn't answer within 120 seconds.");
        return false;
    }
    // Jellyfin answers 503 for a while as it starts (measured 2026-10-19: the
    // first check said "not showing" while it was still starting, then it was).
    for _ in 0..30 {
        if serving(base, stamp).await == Some(true) {
            return true;
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    log(app, Level::Amber, "Jellyfin restarted, but it isn't showing Chillarr's version of its pages.");
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHUNK: &str = r#"ve=function(){var e=he((0,o.ok)(),1)[0],t=e.get("parentId")||void 0,n=e.get("collectionType")||void 0,i=he((0,de.A)("query"),2)"#;

    #[test]
    fn search_patch_applies_once_and_is_idempotent() {
        let Patch::Applied(once) = patch_once(CHUNK, SEARCH_FROM, SEARCH_TO) else { panic!("should apply") };
        assert!(once.contains("t=void 0/*chillarr: every library*/,n=void 0,i=he("));
        assert!(!once.contains("get(\"parentId\")"));
        assert_eq!(patch_once(&once, SEARCH_FROM, SEARCH_TO), Patch::Already);
    }

    #[test]
    fn search_patch_refuses_unknown_text() {
        // A future minifier naming the variables differently.
        let other = CHUNK.replace("t=e.get", "r=e.get");
        assert_eq!(patch_once(&other, SEARCH_FROM, SEARCH_TO), Patch::Unknown);
        // Twice is ambiguous, not "patch both".
        let twice = format!("{CHUNK};{CHUNK}");
        assert_eq!(patch_once(&twice, SEARCH_FROM, SEARCH_TO), Patch::Unknown);
    }

    #[test]
    fn css_link_goes_last_in_the_body_once() {
        let html = "<html><head><title>Jellyfin</title></head><body><div class=\"splashLogo\"></div></body></html>";
        let Patch::Applied(out) = inject_css(html, "abc") else { panic!("should inject") };
        assert!(out.ends_with("<link rel=\"stylesheet\" href=\"chillarr.css?v=abc\" data-chillarr></body></html>"));
        // The page's identity is untouched.
        assert!(out.contains("<title>Jellyfin</title>"));
        assert_eq!(inject_css(&out, "abc"), Patch::Already);
        assert_eq!(inject_css("<html>no body end", "abc"), Patch::Unknown);
    }

    #[test]
    fn stamp_follows_the_installed_files() {
        let a = vec![("index.html".to_string(), 10, 1), ("main.js".to_string(), 20, 1)];
        let mut b = a.clone();
        b.reverse();
        assert_eq!(stamp(&a), stamp(&b), "order doesn't matter");
        let mut newer = a.clone();
        newer[1].2 = 2;
        assert_ne!(stamp(&a), stamp(&newer), "an updated file means a new copy");
        let mut more = a.clone();
        more.push(("search.x.chunk.js".into(), 5, 1));
        assert_ne!(stamp(&a), stamp(&more), "a new file list means a new copy");
    }

    #[test]
    fn build_patches_a_copy_and_reports_what_it_couldnt() {
        let src = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let s = src.path();
        std::fs::write(s.join("index.html"), "<html><head></head><body></body></html>").unwrap();
        std::fs::write(s.join("search.123.chunk.js"), CHUNK).unwrap();
        std::fs::write(s.join("banner-light.9f.png"), b"old").unwrap();
        std::fs::create_dir(s.join("favicons")).unwrap();
        std::fs::write(s.join("favicons").join("touchicon512.png"), b"old").unwrap();
        // A leftover copy from an older Jellyfin goes.
        std::fs::create_dir_all(root.path().join("old")).unwrap();

        assert!(copy_needed(root.path(), "st"));
        let b = build(s, root.path(), "st").unwrap();
        assert!(!copy_needed(root.path(), "st"));
        assert!(!root.path().join("old").exists());
        let d = &b.dir;
        assert!(std::fs::read_to_string(d.join("index.html")).unwrap().contains("chillarr.css?v=st"));
        assert!(std::fs::read_to_string(d.join("chillarr.css")).unwrap().starts_with(&css_head("st")));
        assert!(std::fs::read_to_string(d.join("search.123.chunk.js")).unwrap().contains(SEARCH_TO));
        assert_eq!(std::fs::read(d.join("banner-light.9f.png")).unwrap(), BANNER_LIGHT);
        assert_eq!(std::fs::read(d.join("favicons").join("touchicon512.png")).unwrap(), ICON_512);
        assert!(d.join("chillarr").join("Outfit.woff2").is_file());
        // index.html re-fetches exactly the overwritten files, once per stamp.
        let index = std::fs::read_to_string(d.join("index.html")).unwrap();
        assert!(index.starts_with("<html><head><script data-chillarr-fresh>"), "{index}");
        assert!(index.contains(r#"s="st",f=["banner-light.9f.png","favicons/touchicon512.png","search.123.chunk.js"]"#), "{index}");
        // The installed files are never written.
        assert_eq!(std::fs::read(s.join("banner-light.9f.png")).unwrap(), b"old");
        // The search was patched; only the logos this fake install lacks are reported.
        assert_eq!(b.notes.len(), 1, "{:?}", b.notes);
        assert!(b.notes[0].contains("logos"));

        // A second build of the same stamp reuses the copy.
        std::fs::write(d.join("marker"), "x").unwrap();
        let again = build(s, root.path(), "st").unwrap();
        assert!(d.join("marker").exists());
        assert_eq!(again.repaired, 0, "an untouched copy is left alone");
    }

    #[test]
    fn a_changed_copy_is_put_back() {
        let src = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let s = src.path();
        std::fs::write(s.join("index.html"), "<html><head></head><body></body></html>").unwrap();
        std::fs::write(s.join("search.123.chunk.js"), CHUNK).unwrap();
        std::fs::write(s.join("banner-light.9f.png"), b"old").unwrap();
        let d = build(s, root.path(), "st").unwrap().dir;
        let good_index = std::fs::read(d.join("index.html")).unwrap();

        std::fs::write(d.join("index.html"), "<script src=//evil></script>").unwrap();
        std::fs::write(d.join("search.123.chunk.js"), "evil()").unwrap();
        std::fs::remove_file(d.join("chillarr.css")).unwrap();
        std::fs::write(d.join("banner-light.9f.png"), b"other").unwrap();

        let b = build(s, root.path(), "st").unwrap();
        assert_eq!(b.repaired, 4);
        assert_eq!(std::fs::read(d.join("index.html")).unwrap(), good_index);
        assert!(std::fs::read_to_string(d.join("search.123.chunk.js")).unwrap().contains(SEARCH_TO));
        assert!(std::fs::read_to_string(d.join("chillarr.css")).unwrap().starts_with(&css_head("st")));
        assert_eq!(std::fs::read(d.join("banner-light.9f.png")).unwrap(), BANNER_LIGHT);
        assert_eq!(build(s, root.path(), "st").unwrap().repaired, 0);
    }

    #[test]
    fn build_reports_a_search_page_it_doesnt_know() {
        let src = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("index.html"), "<body></body>").unwrap();
        std::fs::write(src.path().join("search.9.chunk.js"), "something else").unwrap();
        let b = build(src.path(), root.path(), "st").unwrap();
        assert!(b.notes.iter().any(|n| n.contains("search")));
        assert_eq!(std::fs::read_to_string(b.dir.join("search.9.chunk.js")).unwrap(), "something else", "served unpatched");
    }

    #[test]
    fn chillarrs_pages_ride_along_with_the_loader_and_shim() {
        let src = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("index.html"), "<html><head><title>Jellyfin</title></head><body></body></html>").unwrap();
        let b = build(src.path(), root.path(), "st").unwrap();
        let app = b.dir.join(app_folder("st"));
        let page = std::fs::read_to_string(app.join("index.html")).unwrap();
        assert!(page.starts_with("<!doctype html>"), "the window's page, not Jellyfin's");
        assert!(page.contains("<head><script src=\"web/shim.js\"></script>"), "the shim runs before the page's own scripts");
        assert!(app.join("js").join("api.js").is_file() && app.join("web").join("shim.js").is_file());
        let index = std::fs::read_to_string(b.dir.join("index.html")).unwrap();
        assert!(index.starts_with("<html><head><script data-chillarr-fresh>"), "the refresher still runs first");
        assert!(index.contains("<script data-chillarr-loader>") && index.contains("chillarr-st") && !index.contains("__CHILLARR_DIR__"), "{index}");
    }

    #[test]
    fn loader_goes_in_once_and_cant_close_its_own_tag() {
        let Patch::Applied(t) = inject_loader("<html><head></head></html>", "go('__CHILLARR_DIR__/');x='</script>'", "chillarr-ab") else { panic!() };
        assert!(t.starts_with(r"<html><head><script data-chillarr-loader>go('chillarr-ab/');x='<\/script>'</script></head>"), "{t}");
        assert_eq!(inject_loader(&t, "x", "chillarr-ab"), Patch::Already);
        assert_eq!(inject_shim("<html>no head</html>"), Patch::Unknown);
    }

    #[test]
    fn refresher_goes_in_once() {
        let files = vec!["a.png".to_string()];
        let Patch::Applied(t) = inject_fresh("<html><head><meta></head></html>", "st", &files) else { panic!() };
        assert!(t.starts_with("<html><head><script data-chillarr-fresh>(function(){"));
        assert!(t.contains(r#"f=["a.png"]"#) && !t.contains("__"));
        assert_eq!(inject_fresh(&t, "st", &files), Patch::Already);
        assert_eq!(inject_fresh("<html></html>", "st", &files), Patch::Unknown);
    }

    #[test]
    fn a_session_with_something_playing_is_busy() {
        assert!(!playing(&serde_json::json!([{ "Id": "a" }, { "Id": "b", "NowPlayingItem": null }])));
        assert!(playing(&serde_json::json!([{ "Id": "a" }, { "Id": "b", "NowPlayingItem": { "Id": "x" } }])));
    }

    /// Builds the copy from the real install into $JFWEB_OUT, for looking at
    /// it in a browser. `cargo test --lib real_copy -- --ignored`.
    #[test]
    #[ignore]
    fn real_copy() {
        let out = PathBuf::from(std::env::var("JFWEB_OUT").expect("JFWEB_OUT"));
        let installed = PathBuf::from(r"C:\Program Files\Jellyfin\Server\jellyfin-web");
        let mut files = Vec::new();
        walk(&installed, Path::new(""), &mut files).unwrap();
        let s = stamp(&files);
        std::fs::create_dir_all(&out).unwrap();
        let b = build(&installed, &out, &s).unwrap();
        println!("built {} notes {:?}", b.dir.display(), b.notes);
    }
}
