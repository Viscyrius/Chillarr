//! Smarter Jellyfin search. Jellyfin's own search only finds titles holding
//! the words as typed: "ironman", "spiderman", "dune 2" or one typo find
//! nothing (measured 2026-10-18). The Meilisearch plugin swaps in a search
//! engine that handles all of those, in every Jellyfin app (web, TV, phone).
//!
//! It needs a Meilisearch server, which Chillarr downloads (pinned and
//! checked), runs on 127.0.0.1 only, with analytics off and its own key, and
//! keeps alive. The plugin is pinned too. Library access and parental limits
//! still apply per user (measured: a rating-limited user's search hides what
//! it may not see).
//!
//! Whenever Meilisearch is down, Jellyfin's own search answers instead
//! (measured 2026-10-18: engine killed, "Iron man" still found, "ironman"
//! not), so nothing breaks; the guard starts it again and tells the plugin
//! to reconnect.

use crate::log::log;
use crate::media::jf_admin;
use crate::model::Level;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use tauri::{AppHandle, Manager};

/// The release the plugin's README recommends ("older versions may have
/// compatibility issues"); newer ones are 350 MB instead of 129.
const VERSION: &str = "v1.34.0";
const EXE_URL: &str = "https://github.com/meilisearch/meilisearch/releases/download/v1.34.0/meilisearch-windows-amd64.exe";
/// From GitHub's release asset digest, checked against the download on 2026-10-18.
const EXE_SHA256: &str = "0dee5114b9fdf888d091c8e349b2a93fff942eb0eeb5fd9dd35f2dac805a13be";
pub(crate) const ADDR: &str = "127.0.0.1:7700";

const PLUGIN_NAME: &str = "Meilisearch";
const PLUGIN_GUID: &str = "974395db-b31d-46a2-bc86-ef9aa5ac04dd";
/// Built for Jellyfin 12.0 (targetAbi 12.0.0.0). Audited 2026-10-18: it
/// talks to nothing but the Meilisearch address it is given.
const PLUGIN_VERSION: &str = "1.12.1.4";
/// The manifest at one commit, not the branch: its versions and the checksum
/// Jellyfin checks can't change under us, and Jellyfin's plugin auto-update
/// finds nothing newer to take.
const REPO_URL: &str = "https://raw.githubusercontent.com/arnesacnussem/jellyfin-plugin-meilisearch/2b82a4397a3c36899f526878d0292dabad176bd6/manifest.json";
/// Marks any source of this plugin, pinned or not (to tidy up older entries).
const REPO_MARK: &str = "arnesacnussem/jellyfin-plugin-meilisearch";

const DETACHED_PROCESS: u32 = 0x0000_0008;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn dir(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("meili"))
}

fn exe(dir: &Path) -> PathBuf {
    dir.join("meilisearch.exe")
}

/// The master key, made once. It only guards a port that is closed to the
/// network, so it lives next to the data.
fn key(dir: &Path) -> Option<String> {
    let file = dir.join("key.txt");
    if let Some(k) = std::fs::read_to_string(&file).ok().map(|k| k.trim().to_string()).filter(|k| k.len() >= 32) {
        return Some(k);
    }
    let mut b = [0u8; 32];
    getrandom::getrandom(&mut b).ok()?;
    let k: String = b.iter().map(|x| format!("{x:02x}")).collect();
    std::fs::create_dir_all(dir).ok()?;
    std::fs::write(&file, &k).ok()?;
    Some(k)
}

fn installed(app: &AppHandle) -> bool {
    dir(app).is_some_and(|d| exe(&d).is_file())
}

async fn answering() -> bool {
    let http = reqwest::Client::builder().timeout(Duration::from_secs(2)).build().unwrap_or_default();
    http.get(format!("http://{ADDR}/health")).send().await.is_ok_and(|r| r.status().is_success())
}

/// Starts Meilisearch if it is installed and not running. Called before
/// Jellyfin starts, so the plugin finds it there.
pub async fn start(app: &AppHandle) -> bool {
    let Some(d) = dir(app).filter(|d| exe(d).is_file()) else { return false };
    if answering().await {
        return true;
    }
    // Checked on every start, not only after the download: the folder is
    // the user's to write, so a swapped exe would otherwise run at each boot.
    let e = exe(&d);
    let sum = tokio::task::spawn_blocking(move || sha256_file(&e)).await.ok().flatten();
    if !hash_ok(sum.as_deref()) {
        let _ = std::fs::remove_file(exe(&d));
        log(app, Level::Amber, "Jellyfin's search engine on disk wasn't the file Chillarr downloaded, so it was removed and not run. Fetching it again; Jellyfin's own search answers until then.");
        let (app, d) = (app.clone(), d.clone());
        tauri::async_runtime::spawn(async move {
            match download(&app, &d).await {
                // The guard's keep_alive starts it from here.
                Ok(()) => log(&app, Level::Ok, "Downloaded Jellyfin's search engine again."),
                Err(e) => log(&app, Level::Amber, format!("Couldn't download Jellyfin's search engine again: {e}")),
            }
        });
        return false;
    }
    let Some(k) = key(&d) else { return false };
    let spawned = Command::new(exe(&d))
        .args(["--http-addr", ADDR, "--master-key", &k, "--no-analytics", "--env", "production"])
        .arg("--db-path")
        .arg(d.join("data"))
        .arg("--dump-dir")
        .arg(d.join("dumps"))
        .arg("--snapshot-dir")
        .arg(d.join("snapshots"))
        .current_dir(&d)
        .creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Err(e) = spawned {
        log(app, Level::Amber, format!("Couldn't start the search engine for Jellyfin: {e}. Jellyfin's search still works, just not as well."));
        return false;
    }
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if answering().await {
            return true;
        }
    }
    false
}

/// The guard's check: a Meilisearch that stopped is started again and the
/// plugin told to reconnect (until then Jellyfin's plain search answers).
pub async fn keep_alive(app: &AppHandle) {
    if !installed(app) || answering().await {
        return;
    }
    if start(app).await {
        if let Some(j) = jf_admin(app).filter(|j| j.user_id.is_some()) {
            let _ = j.get_json("/meilisearch/reconnect").await;
        }
        log(app, Level::Ok, "Jellyfin's search engine had stopped. Started it again.");
    }
}

#[derive(Serialize, Default)]
pub struct Status {
    /// Downloaded and the plugin in Jellyfin.
    pub on: bool,
    pub running: bool,
    /// The plugin is connected to it.
    pub connected: bool,
    /// Titles, seasons and episodes it knows.
    pub items: Option<u64>,
}

pub async fn status(app: &AppHandle) -> Status {
    let running = answering().await;
    let mut s = Status { running, ..Default::default() };
    let Some(j) = jf_admin(app).filter(|j| j.user_id.is_some()) else { return s };
    let plugin = plugin_installed(&j).await;
    s.on = plugin && installed(app);
    if plugin {
        if let Ok(v) = j.get_json("/meilisearch/status").await {
            s.connected = v["meilisearchOk"] == true && running;
            s.items = v["indexStatus"]["Items"].as_str().and_then(|n| n.parse().ok());
        }
    }
    s
}

async fn plugin_installed(j: &crate::jellyfin::Jf) -> bool {
    plugin(j).await.is_some_and(|p| p["Status"] == "Active")
}

/// The plugin's entry in Jellyfin: Id, Version, and Status ("Active", or
/// "Restart" once installed and waiting for Jellyfin to restart).
async fn plugin(j: &crate::jellyfin::Jf) -> Option<Value> {
    let v = j.get_json("/Plugins").await.ok()?;
    v.as_array()?.iter().find(|p| p["Name"] == PLUGIN_NAME).cloned()
}

/// Jellyfin's plugin sources without ours (any copy of it).
async fn repos_without_ours(j: &crate::jellyfin::Jf) -> Result<Vec<Value>, String> {
    let repos = j.get_json("/Repositories").await.map_err(|e| e.to_string())?;
    Ok(repos.as_array().cloned().unwrap_or_default().into_iter().filter(|r| !r["Url"].as_str().unwrap_or_default().contains(REPO_MARK)).collect())
}

/// Stops our Meilisearch only (by its path), not one something else runs.
fn stop_ours(exe: &Path) {
    let script = format!("Get-Process meilisearch -ErrorAction SilentlyContinue | Where-Object {{ $_.Path -eq '{}' }} | Stop-Process -Force", exe.display().to_string().replace('\'', "''"));
    let _ = Command::new(crate::procs::powershell())
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// The pinned build, and nothing else (an unreadable file isn't it either).
fn hash_ok(sum: Option<&str>) -> bool {
    sum == Some(EXE_SHA256)
}

fn sha256_file(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Some(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Downloads Meilisearch and checks it before anything runs it.
async fn download(app: &AppHandle, d: &Path) -> Result<(), String> {
    if exe(d).is_file() && hash_ok(sha256_file(&exe(d)).as_deref()) {
        return Ok(());
    }
    std::fs::create_dir_all(d).map_err(|e| format!("Couldn't make its folder: {e}"))?;
    log(app, Level::Ok, format!("Downloading Jellyfin's search engine (Meilisearch {VERSION}, 129 MB)..."));
    let http = reqwest::Client::builder().timeout(Duration::from_secs(30 * 60)).build().map_err(|e| e.to_string())?;
    let mut resp = http.get(EXE_URL).send().await.and_then(|r| r.error_for_status()).map_err(|e| format!("Couldn't download it: {e}"))?;
    let part = d.join("meilisearch.exe.part");
    let mut file = std::fs::File::create(&part).map_err(|e| format!("Couldn't save it: {e}"))?;
    while let Some(chunk) = resp.chunk().await.map_err(|e| format!("The download broke off: {e}"))? {
        std::io::Write::write_all(&mut file, &chunk).map_err(|e| format!("Couldn't save it: {e}"))?;
    }
    drop(file);
    let part2 = part.clone();
    let sum = tokio::task::spawn_blocking(move || sha256_file(&part2)).await.ok().flatten();
    if !hash_ok(sum.as_deref()) {
        let _ = std::fs::remove_file(&part);
        return Err("The download didn't match the expected file, so it was thrown away. Try again later.".into());
    }
    std::fs::rename(&part, exe(d)).map_err(|e| format!("Couldn't save it: {e}"))
}

/// Turns smarter search on: the engine, then the plugin pointed at it, then
/// Jellyfin restarted to load it and the library indexed.
pub async fn enable(app: &AppHandle) -> Result<String, String> {
    let j = jf_admin(app).filter(|j| j.user_id.is_some()).ok_or("Sign in to Jellyfin first (Settings > Account)")?;
    let d = dir(app).ok_or("No app data folder")?;
    download(app, &d).await?;
    let k = key(&d).ok_or("Couldn't make the search engine's key")?;
    if !start(app).await {
        return Err("The search engine didn't start. Try again, or restart the PC.".into());
    }
    // Only the pinned source: an older branch-following entry goes.
    let mut repos = repos_without_ours(&j).await?;
    repos.push(json!({ "Name": PLUGIN_NAME, "Url": REPO_URL, "Enabled": true }));
    j.post_json("/Repositories", &Value::Array(repos)).await.map_err(|e| format!("Couldn't add the plugin's source to Jellyfin: {e}"))?;
    if !plugin_installed(&j).await {
        if plugin(&j).await.is_none() {
            let path = format!(
                "/Packages/Installed/{PLUGIN_NAME}?assemblyGuid={PLUGIN_GUID}&version={PLUGIN_VERSION}&repositoryUrl={}",
                REPO_URL.replace(':', "%3A").replace('/', "%2F")
            );
            j.post_json(&path, &json!({})).await.map_err(|e| format!("Jellyfin couldn't install the search plugin: {e}"))?;
            // The download and unpack run in the background: restart only once it's there.
            let mut there = false;
            for _ in 0..90 {
                tokio::time::sleep(Duration::from_secs(2)).await;
                if plugin(&j).await.is_some() {
                    there = true;
                    break;
                }
            }
            if !there {
                return Err("Jellyfin didn't finish installing the search plugin. Try again in a minute.".into());
            }
        }
        let _ = j.post_json("/System/Restart", &json!({})).await;
        let mut loaded = false;
        for _ in 0..60 {
            tokio::time::sleep(Duration::from_secs(2)).await;
            if plugin_installed(&j).await {
                loaded = true;
                break;
            }
        }
        if !loaded {
            return Err("Jellyfin didn't come back with the search plugin loaded. Restart Jellyfin and try again.".into());
        }
    }
    let p = plugin(&j).await.ok_or("The search plugin isn't loaded in Jellyfin")?;
    let id = p["Id"].as_str().unwrap_or_default().to_string();
    let mut cfg = j.get_json(&format!("/Plugins/{id}/Configuration")).await.map_err(|e| e.to_string())?;
    cfg["Url"] = json!(plugin_url());
    cfg["ApiKey"] = json!(k);
    j.post_json(&format!("/Plugins/{id}/Configuration"), &cfg).await.map_err(|e| format!("Couldn't set up the search plugin: {e}"))?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let _ = j.get_json("/meilisearch/reindex").await;
    let msg = "Smarter search is on in Jellyfin: typos, \"spiderman\" and \"dune 2\" find the right title, in every Jellyfin app.".to_string();
    log(app, Level::Ok, &msg);
    Ok(msg)
}

/// Where the plugin is told to find the engine: Chillarr's search proxy when
/// it's listening (so Chillarr sees what's searched for), else the engine itself.
fn plugin_url() -> String {
    match crate::search_tap::port() {
        Some(p) => format!("http://127.0.0.1:{p}"),
        None => format!("http://{ADDR}"),
    }
}

/// Points the plugin at `plugin_url` if it isn't already (at start, the proxy
/// may have moved, or an older Chillarr pointed it at the engine). True once
/// done or nothing needs doing; false while Jellyfin can't be asked yet.
pub async fn sync_url(app: &AppHandle) -> bool {
    if !installed(app) {
        return true;
    }
    let Some(j) = jf_admin(app).filter(|j| j.user_id.is_some()) else { return false };
    let Ok(v) = j.get_json("/Plugins").await else { return false };
    let Some(p) = v.as_array().and_then(|a| a.iter().find(|p| p["Name"] == PLUGIN_NAME)).cloned() else { return true };
    if p["Status"] != "Active" {
        return true;
    }
    let id = p["Id"].as_str().unwrap_or_default().to_string();
    let Ok(mut cfg) = j.get_json(&format!("/Plugins/{id}/Configuration")).await else { return false };
    let want = plugin_url();
    if cfg["Url"].as_str() == Some(want.as_str()) {
        return true;
    }
    cfg["Url"] = json!(want);
    match j.post_json(&format!("/Plugins/{id}/Configuration"), &cfg).await {
        Ok(_) => {
            log(app, Level::Ok, format!("Jellyfin's search now goes through Chillarr ({want}), so titles you search for can join the Discover shelf."));
            true
        }
        Err(e) => {
            log(app, Level::Amber, format!("Couldn't point Jellyfin's search plugin at {want}: {e}"));
            false
        }
    }
}

/// Has Jellyfin's search engine take in the library as it is now. The plugin
/// only indexes on its schedule, so a title new to Jellyfin isn't found by
/// search until this runs (measured 2026-10-19: searchable 3-6 s after).
/// Does nothing while smarter search is off.
pub async fn reindex(app: &AppHandle) {
    if !installed(app) {
        return;
    }
    let Some(j) = jf_admin(app).filter(|j| j.user_id.is_some()) else { return };
    if !plugin_installed(&j).await {
        return;
    }
    if let Err(e) = j.get_json("/meilisearch/reindex").await {
        log(app, Level::Amber, format!("Couldn't have Jellyfin's search take in new titles: {e}. They show up in search after its next scheduled index."));
    }
}

/// Turns it off: the plugin comes out (Jellyfin's own search answers again
/// after its restart) and the engine and its data are removed.
pub async fn disable(app: &AppHandle) -> Result<String, String> {
    let j = jf_admin(app).filter(|j| j.user_id.is_some()).ok_or("Sign in to Jellyfin first (Settings > Account)")?;
    if let Some(p) = plugin(&j).await {
        // Whatever version is installed, not the one we'd install.
        let (id, version) = (p["Id"].as_str().unwrap_or_default(), p["Version"].as_str().unwrap_or_default());
        j.delete_path(&format!("/Plugins/{id}/{version}")).await.map_err(|e| format!("Jellyfin couldn't remove the search plugin: {e}"))?;
        let _ = j.post_json("/System/Restart", &json!({})).await;
    }
    if let Ok(repos) = repos_without_ours(&j).await {
        let _ = j.post_json("/Repositories", &Value::Array(repos)).await;
    }
    if let Some(d) = dir(app) {
        let e = exe(&d);
        tokio::task::spawn_blocking(move || stop_ours(&e)).await.ok();
        tokio::time::sleep(Duration::from_secs(1)).await;
        let _ = std::fs::remove_dir_all(d);
    }
    let msg = "Smarter search is off. Jellyfin uses its own search again once it has restarted.".to_string();
    log(app, Level::Ok, &msg);
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_pinned_exe_passes() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("meilisearch.exe");
        std::fs::write(&f, b"abc").unwrap();
        let sum = sha256_file(&f);
        assert_eq!(sum.as_deref(), Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"));
        assert!(!hash_ok(sum.as_deref()));
        assert!(!hash_ok(sha256_file(&dir.path().join("missing.exe")).as_deref()));
        assert!(hash_ok(Some(EXE_SHA256)));
    }
}
