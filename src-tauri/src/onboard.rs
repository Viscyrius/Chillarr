//! Setting up a new PC in one go. Chillarr downloads every installer itself
//! (so the window can show real bytes), checks each against the hash in
//! Windows Package Manager's manifest, then does everything that needs admin
//! in ONE elevated script: every install, the firewall rules that would
//! otherwise each pop a Windows Security window, the protected-folder
//! allow list, and moving services to run as the user. One UAC prompt, no
//! installer windows. After that it starts the apps, connects them, makes the
//! one login and adds the indexers.
//!
//! winget stays as the fallback for an app whose manifest can't be read. It
//! still works, but asks for permission once per app and shows no bytes.

use crate::fix::{self, Core, Trigger};
use crate::install::{self, Source};
use crate::log::log;
use crate::model::{AppId, Level};
use crate::procs::{ps_quote, ps_tool};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

/// Installers at once. More than this just splits the same connection.
const DOWNLOADS_AT_ONCE: usize = 3;
const STALL: Duration = Duration::from_secs(45);
/// All the installers together, including a UAC prompt left waiting.
const ELEVATED_WAIT_MS: u32 = 25 * 60 * 1000;
const EMIT_GAP: Duration = Duration::from_millis(120);

#[derive(Clone, Serialize, Debug, PartialEq)]
pub struct AppRow {
    pub id: AppId,
    pub label: String,
    /// wait | download | check | ready | install | done | have | failed
    pub state: String,
    pub got: u64,
    pub total: u64,
    pub note: String,
}

/// What the setup screen draws. Sent whole on every change.
#[derive(Clone, Serialize, Default, Debug)]
pub struct Meter {
    pub running: bool,
    pub finished: bool,
    /// resolve | download | permission | install | start | connect | sources | finish
    pub stage: String,
    pub line: String,
    pub percent: f64,
    pub got: u64,
    pub total: u64,
    pub speed: f64,
    pub apps: Vec<AppRow>,
    pub sites_added: usize,
    pub sites_total: usize,
    pub sites_waiting: Vec<String>,
    pub problems: Vec<String>,
    /// The UAC prompt was turned down: nothing installed, downloads kept.
    pub declined: bool,
    pub installing_only: bool,
    #[serde(skip)]
    frac: f64,
    #[serde(skip)]
    has_downloads: bool,
}

#[derive(Default)]
pub struct Onboard {
    meter: Mutex<Meter>,
    emit: Mutex<Option<(Instant, u64)>>,
    busy: tokio::sync::Mutex<()>,
    servers: AtomicBool,
}

/// Where each stage sits on the one bar. Downloads are the long wait when
/// there are any; without them the rest spreads out.
fn span(stage: &str, has_downloads: bool) -> (f64, f64) {
    if has_downloads {
        match stage {
            "resolve" => (0.0, 2.0),
            "download" => (2.0, 62.0),
            "permission" => (62.0, 62.0),
            "install" => (62.0, 78.0),
            "start" => (78.0, 88.0),
            "connect" => (88.0, 92.0),
            "sources" => (92.0, 97.0),
            "finish" => (97.0, 100.0),
            _ => (100.0, 100.0),
        }
    } else {
        match stage {
            "resolve" => (0.0, 2.0),
            "download" | "permission" | "install" => (2.0, 2.0),
            "start" => (2.0, 45.0),
            "connect" => (45.0, 60.0),
            "sources" => (60.0, 88.0),
            "finish" => (88.0, 100.0),
            _ => (100.0, 100.0),
        }
    }
}

pub fn overall(stage: &str, has_downloads: bool, frac: f64) -> f64 {
    let (a, b) = span(stage, has_downloads);
    a + (b - a) * frac.clamp(0.0, 1.0)
}

fn with(app: &AppHandle, f: impl FnOnce(&mut Meter)) {
    let st = app.state::<Onboard>();
    let mut m = st.meter.lock().unwrap();
    f(&mut m);
    m.got = m.apps.iter().map(|a| a.got).sum();
    m.total = m.apps.iter().map(|a| a.total).sum();
    if m.stage == "download" && m.total > 0 {
        m.frac = m.got as f64 / m.total as f64;
    }
    m.percent = if m.finished { 100.0 } else { overall(&m.stage, m.has_downloads, m.frac) };
    let got = m.got;
    let snapshot = m.clone();
    drop(m);
    send(app, &st, snapshot, got, false);
}

/// One event per frame or so, whatever the download's chunk rate is. Stage
/// changes pass `force`, so the screen never misses one.
fn send(app: &AppHandle, st: &Onboard, mut m: Meter, got: u64, force: bool) {
    let mut last = st.emit.lock().unwrap();
    let now = Instant::now();
    if let Some((at, before)) = *last {
        let dt = now.duration_since(at);
        if dt < EMIT_GAP && !force && !m.finished {
            return;
        }
        m.speed = got.saturating_sub(before) as f64 / dt.as_secs_f64().max(0.001);
        // The screen keeps a smoothed speed; this is only the latest slice.
        st.meter.lock().unwrap().speed = m.speed;
    }
    *last = Some((now, got));
    drop(last);
    let _ = app.emit("setup-progress", &m);
}

fn stage(app: &AppHandle, name: &str, line: &str) {
    with(app, |m| {
        m.stage = name.into();
        m.line = line.into();
        if name != "download" {
            m.frac = 0.0;
        }
    });
    let st = app.state::<Onboard>();
    let snap = st.meter.lock().unwrap().clone();
    let got = snap.got;
    send(app, &st, snap, got, true);
}

fn row(app: &AppHandle, id: AppId, f: impl FnOnce(&mut AppRow)) {
    with(app, |m| {
        if let Some(r) = m.apps.iter_mut().find(|r| r.id == id) {
            f(r);
        }
    });
}

pub fn snapshot(app: &AppHandle) -> Meter {
    app.state::<Onboard>().meter.lock().unwrap().clone()
}

/* ---------- Finding the installers ---------- */

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Inno,
    Nullsoft,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Installer {
    pub url: String,
    pub sha256: String,
    pub kind: Kind,
}

/// Known good when the manifest can't be read (GitHub's API allows 60 calls an
/// hour per address, and a shared address can use that up). The apps update
/// themselves afterwards where they can.
fn pinned(pkg: &str) -> Option<Installer> {
    let (url, sha256, kind) = match pkg {
        "TeamSonarr.Sonarr" => ("https://github.com/Sonarr/Sonarr/releases/download/v4.0.14.2939/Sonarr.main.4.0.14.2939.win-x64-installer.exe", "C431C9DBA573B0F647772A2B76F8D8F356BDF7FB735A13CEE8E6A892253D9352", Kind::Inno),
        "TeamRadarr.Radarr" => ("https://github.com/Radarr/Radarr/releases/download/v6.0.4.10291/Radarr.master.6.0.4.10291.windows-core-x64-installer.exe", "C0CB8E177AF9662AE4C7043A878E8E3EA7EF6466F08F0555590ECD5B424BF925", Kind::Inno),
        "TeamProwlarr.Prowlarr" => ("https://github.com/Prowlarr/Prowlarr/releases/download/v2.3.0.5236/Prowlarr.master.2.3.0.5236.windows-core-x64-installer.exe", "A3ED2AD4C6437C60131EECB2FC1F23E544C1A351A8CF09A31BAAEDE6B4BF6E5A", Kind::Inno),
        "qBittorrent.qBittorrent" => ("https://sourceforge.net/projects/qbittorrent/files/qbittorrent-win32/qbittorrent-5.3.0/qbittorrent_5.3.0_x64_setup.exe/download", "AFF5B3BDC00D6E5773D6099BA30F9696DD3DFD1669AB0F64CE5C4E7168486436", Kind::Nullsoft),
        "Jellyfin.Server" => ("https://repo.jellyfin.org/files/server/windows/stable/v12.4/amd64/jellyfin_12.4_windows-x64.exe", "52085AD1156450146FFDBDA16C783683218BFCF9A6FA70B8232C7B6A36B4A9C6", Kind::Nullsoft),
        _ => return None,
    };
    Some(Installer { url: url.into(), sha256: sha256.into(), kind })
}

/// manifests/t/TeamSonarr/Sonarr for TeamSonarr.Sonarr.
fn manifest_dir(pkg: &str) -> Option<String> {
    let (publisher, name) = pkg.split_once('.')?;
    let first = publisher.chars().next()?.to_ascii_lowercase();
    Some(format!("manifests/{first}/{publisher}/{}", name.replace('.', "/")))
}

/// The newest of the folder names that are versions ("lt2" and "Beta" sit
/// beside the versions of qBittorrent).
pub fn newest_version(names: &[String]) -> Option<String> {
    let key = |v: &str| v.split('.').map(|p| p.parse::<u64>().ok()).collect::<Option<Vec<u64>>>();
    names.iter().filter(|n| key(n).is_some()).max_by_key(|n| key(n)).cloned()
}

fn yaml_value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let rest = line.trim_start().trim_start_matches("- ").strip_prefix(key)?.strip_prefix(':')?;
    Some(rest.trim().trim_matches(|c| c == '"' || c == '\''))
}

fn kind_of(s: &str) -> Option<Kind> {
    match s.to_ascii_lowercase().as_str() {
        "inno" => Some(Kind::Inno),
        "nullsoft" => Some(Kind::Nullsoft),
        _ => None,
    }
}

/// The x64 installer from a winget installer manifest. Only the few fields it
/// needs, in winget's own flat layout, so no YAML library for one file. An
/// installer type Chillarr can't run silently gives None, and winget takes it.
pub fn parse_manifest(text: &str) -> Option<Installer> {
    let mut top_kind = None;
    let mut found: Vec<(String, Option<String>, Option<String>, Option<Kind>)> = Vec::new();
    let mut in_list = false;
    for line in text.lines() {
        if line.starts_with("Installers:") {
            in_list = true;
            continue;
        }
        if !in_list {
            if let Some(v) = yaml_value(line, "InstallerType") {
                top_kind = kind_of(v);
            }
            continue;
        }
        if !line.starts_with(' ') && !line.starts_with('-') {
            in_list = false;
            continue;
        }
        if let Some(v) = yaml_value(line, "Architecture") {
            found.push((v.to_ascii_lowercase(), None, None, None));
        } else if let Some(last) = found.last_mut() {
            if let Some(v) = yaml_value(line, "InstallerUrl") {
                last.1 = Some(v.to_string());
            } else if let Some(v) = yaml_value(line, "InstallerSha256") {
                last.2 = Some(v.to_string());
            } else if let Some(v) = yaml_value(line, "InstallerType") {
                last.3 = kind_of(v);
            }
        }
    }
    let (_, url, sha, own_kind) = found.into_iter().find(|f| f.0 == "x64")?;
    let (url, sha256) = (url?, sha?);
    (sha256.len() == 64 && trusted_host(&url)).then_some(Installer { url, sha256, kind: own_kind.or(top_kind)? })
}

/// Only the apps' own download sites, whatever a manifest says.
pub fn trusted_host(url: &str) -> bool {
    ["https://github.com/", "https://repo.jellyfin.org/", "https://sourceforge.net/", "https://downloads.sourceforge.net/"].iter().any(|h| url.starts_with(h))
}

async fn from_manifest(http: &reqwest::Client, pkg: &str) -> Result<Installer, String> {
    let dir = manifest_dir(pkg).ok_or("bad package name")?;
    let listing: serde_json::Value = http
        .get(format!("https://api.github.com/repos/microsoft/winget-pkgs/contents/{dir}"))
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let names: Vec<String> = listing.as_array().map(|a| a.iter().filter_map(|e| e["name"].as_str().map(String::from)).collect()).unwrap_or_default();
    let version = newest_version(&names).ok_or("no versions listed")?;
    let text = http
        .get(format!("https://raw.githubusercontent.com/microsoft/winget-pkgs/master/{dir}/{version}/{pkg}.installer.yaml"))
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())?;
    parse_manifest(&text).ok_or_else(|| "its manifest has no x64 installer Chillarr can run silently".into())
}

fn folder() -> PathBuf {
    PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join("Chillarr").join("installers")
}

/// Downloads to `path`, picking up where a broken connection left off, then
/// checks the hash. A file already there with the right hash is used as it is
/// (saying no to the prompt and trying again downloads nothing twice).
async fn fetch(app: &AppHandle, id: AppId, http: &reqwest::Client, url: &str, sha: &str, path: &Path) -> Result<(), String> {
    if hash_ok(path, sha).await {
        let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        row(app, id, |r| {
            r.state = "ready".into();
            r.got = len;
            r.total = len;
        });
        return Ok(());
    }
    let mut last = String::new();
    for attempt in 0..5u32 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_secs(2 * attempt as u64)).await;
        }
        match fetch_once(app, id, http, url, path).await {
            Ok(()) => {
                row(app, id, |r| r.state = "check".into());
                if hash_ok(path, sha).await {
                    row(app, id, |r| r.state = "ready".into());
                    return Ok(());
                }
                // Never kept, never run: the next try starts clean.
                let _ = std::fs::remove_file(path);
                last = "the download didn't match the expected file".into();
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

async fn fetch_once(app: &AppHandle, id: AppId, http: &reqwest::Client, url: &str, path: &Path) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let have = tokio::fs::metadata(path).await.map(|m| m.len()).unwrap_or(0);
    let mut req = http.get(url);
    if have > 0 {
        req = req.header("Range", format!("bytes={have}-"));
    }
    let resp = req.send().await.map_err(|e| e.to_string())?;
    // A kept file as long as this version or longer (an older version's): start clean.
    if resp.status().as_u16() == 416 {
        let _ = tokio::fs::remove_file(path).await;
        return Err("the saved part didn't fit this version".into());
    }
    let mut resp = resp.error_for_status().map_err(|e| e.to_string())?;
    let resumed = resp.status().as_u16() == 206;
    let start = if resumed { have } else { 0 };
    let size = resp.content_length().map(|l| l + start);
    let mut file = tokio::fs::OpenOptions::new().create(true).write(true).append(resumed).truncate(!resumed).open(path).await.map_err(|e| e.to_string())?;
    let mut got = start;
    row(app, id, |r| {
        r.state = "download".into();
        r.got = got;
        if let Some(s) = size {
            r.total = s;
        }
    });
    loop {
        let chunk = tokio::time::timeout(STALL, resp.chunk()).await.map_err(|_| "the connection stalled".to_string())?.map_err(|e| e.to_string())?;
        let Some(chunk) = chunk else { break };
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        got += chunk.len() as u64;
        row(app, id, |r| r.got = got);
    }
    file.flush().await.map_err(|e| e.to_string())?;
    if size.is_some_and(|s| got < s) {
        return Err("the connection closed early".into());
    }
    Ok(())
}

async fn hash_ok(path: &Path, want: &str) -> bool {
    let (path, want) = (path.to_path_buf(), want.to_string());
    tokio::task::spawn_blocking(move || {
        let Ok(mut f) = std::fs::File::open(&path) else { return false };
        let mut h = Sha256::new();
        if std::io::copy(&mut f, &mut h).is_err() {
            return false;
        }
        let got: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
        want.len() == 64 && got.eq_ignore_ascii_case(&want)
    })
    .await
    .unwrap_or(false)
}

/* ---------- The one elevated script ---------- */

pub struct Step {
    pub id: AppId,
    pub path: PathBuf,
    pub sha256: String,
    pub kind: Kind,
}

/// A firewall rule for a program, set before it first listens, so Windows
/// never stops to ask "Allow access?".
pub struct Wall {
    pub name: String,
    pub exe: PathBuf,
    pub allow: bool,
    pub lan_only: bool,
}

#[derive(Default)]
pub struct Plan {
    pub steps: Vec<Step>,
    pub walls: Vec<Wall>,
    /// Added to Defender's protected-folder allow list if that rule is on.
    pub allow_folders: Vec<PathBuf>,
    /// Services to leave stopped and Manual, because Chillarr runs the app as the user.
    pub services: Vec<String>,
    /// Folders to make and let the signed-in user change: `(folder, account)`.
    /// An app run as you can't start if its files were made by its service.
    pub grant_folders: Vec<(String, String)>,
    /// Jellyfin's installer always makes a service: `(service, data folder, account)`.
    pub jellyfin_as_user: Option<(String, String, String)>,
}

fn silent_args(kind: Kind) -> &'static str {
    match kind {
        // "none" unticks the service and the startup shortcut: Chillarr starts them.
        Kind::Inno => "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP- /TASKS=none",
        Kind::Nullsoft => "/S",
    }
}

/// Lines that may fail without stopping the rest. A failed firewall rule costs
/// one Windows question later; it is no reason to give up on the others.
fn soft(line: &str) -> String {
    format!("try {{ {line} }} catch {{ }}\n")
}

/// The script for `run_elevated`. `log` gets a line per step ("i Sonarr",
/// "d Sonarr", "f Sonarr 2"), which the window turns into progress. Each
/// installer is copied somewhere only an admin can change and hashed again
/// THERE, so nothing can swap the file between the check and the run.
pub fn script(plan: &Plan, log: &Path) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "$log = {}\nfunction Say($m) {{ try {{ Add-Content -LiteralPath $log -Value $m -Encoding ascii }} catch {{ }} }}\n",
        ps_quote(&log.to_string_lossy())
    ));
    if !plan.steps.is_empty() {
        s.push_str(concat!(
            "$box = Join-Path (Split-Path $sys) ('Temp\\chillarr-' + [guid]::NewGuid())\n",
            "New-Item -ItemType Directory -Path $box -Force | Out-Null\n",
            "function Inst($id, $src, $sha, $a) {\n",
            " Say \"i $id\"\n",
            " try {\n",
            "  $f = Join-Path $box ($id + '.exe')\n",
            "  Copy-Item -LiteralPath $src -Destination $f -Force\n",
            "  if ((Get-FileHash -LiteralPath $f -Algorithm SHA256).Hash -ne $sha) { Say \"f $id hash\"; return }\n",
            "  $p = Start-Process -FilePath $f -ArgumentList $a -WindowStyle Hidden -PassThru\n",
            "  $null = $p.Handle\n",
            "  if (-not $p.WaitForExit(600000)) { $p.Kill(); Say \"f $id timeout\"; return }\n",
            "  if ($p.ExitCode -in 0, 3010, 1641) { Say \"d $id\" } else { Say \"f $id $($p.ExitCode)\" }\n",
            " } catch { Say \"f $id error\" }\n",
            "}\n",
        ));
        for st in &plan.steps {
            s.push_str(&format!(
                "Inst {} {} {} {}\n",
                ps_quote(st.id.label()),
                ps_quote(&st.path.to_string_lossy()),
                ps_quote(&st.sha256.to_ascii_uppercase()),
                ps_quote(silent_args(st.kind))
            ));
        }
        s.push_str("try { Remove-Item -LiteralPath $box -Recurse -Force } catch { }\n");
    }
    for svc in &plan.services {
        s.push_str(&soft(&format!(
            "{} | Out-Null; {} | Out-Null",
            ps_tool("sc.exe", &["config", svc, "start=", "demand"]),
            ps_tool("net.exe", &["stop", svc])
        )));
    }
    for (dir, account) in &plan.grant_folders {
        s.push_str(&soft(&format!("New-Item -ItemType Directory -Path {} -Force | Out-Null; {} | Out-Null", ps_quote(dir), crate::imports::grant_user_line(dir, account))));
    }
    if let Some((svc, data, account)) = &plan.jellyfin_as_user {
        // Stopped first: it writes into that folder for as long as it runs.
        s.push_str(&soft(&format!(
            "if (Get-Service -Name {} -ErrorAction SilentlyContinue) {{ {} | Out-Null; {} | Out-Null; {} | Out-Null; Say 'j' }}",
            ps_quote(svc),
            ps_tool("sc.exe", &["config", svc, "start=", "demand"]),
            ps_tool("net.exe", &["stop", svc]),
            crate::imports::grant_user_line(data, account),
        )));
    }
    if !plan.walls.is_empty() {
        s.push_str("function Wall($n, $e, $act, $from) { try { Remove-NetFirewallRule -DisplayName $n -ErrorAction SilentlyContinue; New-NetFirewallRule -DisplayName $n -Direction Inbound -Program $e -Action $act -RemoteAddress $from -Profile Any | Out-Null } catch { } }\n");
        for w in &plan.walls {
            s.push_str(&format!(
                "Wall {} {} {} {}\n",
                ps_quote(&format!("Chillarr: {}", w.name)),
                ps_quote(&w.exe.to_string_lossy()),
                ps_quote(if w.allow { "Allow" } else { "Block" }),
                ps_quote(if w.lan_only { "LocalSubnet" } else { "Any" })
            ));
        }
    }
    if !plan.allow_folders.is_empty() {
        let list: Vec<String> = plan.allow_folders.iter().map(|p| ps_quote(&p.to_string_lossy())).collect();
        s.push_str(&soft(&format!(
            "if ((Get-MpPreference).EnableControlledFolderAccess -eq 1) {{ foreach ($e in @({})) {{ Add-MpPreference -ControlledFolderAccessAllowedApplications $e }}; Say 'cfa' }}",
            list.join(", ")
        )));
    }
    s.push_str("Say 'end'\nexit 0");
    s
}

/// Where an installer puts each program: what the firewall rule has to name
/// before the program exists.
fn expected_exe(id: AppId) -> Option<PathBuf> {
    let env = |v: &str| std::env::var_os(v).map(PathBuf::from);
    Some(match id {
        AppId::Prowlarr | AppId::Sonarr | AppId::Radarr => env("ProgramData")?.join(id.label()).join("bin").join(format!("{}.exe", id.label())),
        AppId::QBittorrent => env("ProgramFiles")?.join("qBittorrent").join("qbittorrent.exe"),
        AppId::Jellyfin => env("ProgramFiles")?.join("Jellyfin").join("Server").join("jellyfin.exe"),
        AppId::FlareSolverr => install::flare_dir().join("flaresolverr").join("flaresolverr.exe"),
    })
}

/// Who may reach each program from another device. Jellyfin and qBittorrent
/// need the world (TVs over Tailscale, torrent peers); the *arr apps sit behind
/// their login on the home network; FlareSolverr has no login at all and only
/// Prowlarr on this PC talks to it, so it's closed.
fn wall_for(id: AppId, exe: PathBuf) -> Wall {
    let (allow, lan_only) = match id {
        AppId::Jellyfin | AppId::QBittorrent => (true, false),
        AppId::FlareSolverr => (false, false),
        _ => (true, true),
    };
    Wall { name: id.label().to_string(), exe, allow, lan_only }
}

fn signed_in_account() -> Option<String> {
    Some(format!("{}\\{}", std::env::var("USERDOMAIN").ok()?, std::env::var("USERNAME").ok()?))
}

fn jellyfin_data_folder() -> Option<String> {
    Some(PathBuf::from(std::env::var_os("ProgramData")?).join("Jellyfin").join("Server").to_string_lossy().to_string())
}

/// What the elevated script should do on this PC besides the installs.
fn extras(app: &AppHandle, plan: &mut Plan, ids: &[AppId]) {
    let core = app.state::<Core>();
    for id in AppId::ALL {
        let known = core.config(id).and_then(|c| c.exe);
        let exe = if ids.contains(&id) { expected_exe(id) } else { known.clone() };
        if let Some(exe) = exe {
            // The *arr apps run from their no-tray program, which may not exist yet.
            if id.is_arr() {
                let console = exe.with_file_name(format!("{}.Console.exe", id.label()));
                plan.walls.push(Wall { name: format!("{} (no icon)", id.label()), ..wall_for(id, console) });
            }
            plan.walls.push(wall_for(id, exe));
        }
    }
    // Only programs in folders an admin alone can change go on the allow list.
    // Radarr and Sonarr's own folder isn't one (see fix::defender_plan).
    for id in [AppId::QBittorrent, AppId::Jellyfin] {
        if let Some(exe) = core.config(id).and_then(|c| c.exe).or_else(|| expected_exe(id)) {
            plan.allow_folders.push(exe);
        }
    }
    if let Ok(me) = std::env::current_exe() {
        plan.walls.push(Wall { name: "Chillarr".into(), exe: me.clone(), allow: true, lan_only: false });
        plan.allow_folders.push(me);
    }
    for id in [AppId::Radarr, AppId::Sonarr, AppId::Prowlarr] {
        if let Some(svc) = core.config(id).and_then(|c| c.service) {
            if crate::imports::service_account(&svc).and_then(|a| crate::imports::service_sid(&a)).is_some() {
                plan.services.push(svc);
            }
        }
    }
    if let (Some(pd), Some(account)) = (std::env::var_os("ProgramData").map(PathBuf::from), signed_in_account()) {
        for id in [AppId::Radarr, AppId::Sonarr, AppId::Prowlarr] {
            let was_service = core.config(id).and_then(|c| c.service).is_some();
            if ids.contains(&id) || was_service {
                plan.grant_folders.push((pd.join(id.label()).to_string_lossy().to_string(), account.clone()));
            }
        }
    }
    if let (Some(data), Some(account)) = (jellyfin_data_folder(), signed_in_account()) {
        let installing = ids.contains(&AppId::Jellyfin);
        let locked = core.config(AppId::Jellyfin).and_then(|c| c.service).is_some_and(|svc| crate::imports::service_account(&svc).and_then(|a| crate::imports::service_sid(&a)).is_some());
        if installing || locked {
            plan.jellyfin_as_user = Some(("JellyfinServer".into(), data, account));
        }
    }
}

/* ---------- The whole run ---------- */

/// Downloads and installs `ids`. Returns what failed, in plain words.
async fn get_apps(app: &AppHandle, ids: &[AppId]) -> Result<Vec<(AppId, String)>, String> {
    let core = app.state::<Core>();
    let _guard = core.fixer.running.lock().await;
    let _ = app.emit("fix-state", true);
    let result = get_apps_locked(app, ids).await;
    fix::rediscover_into(app).await;
    fix::first_starts(app).await;
    fix::refresh_cards(app).await;
    let _ = app.emit("fix-state", false);
    result
}

async fn get_apps_locked(app: &AppHandle, ids: &[AppId]) -> Result<Vec<(AppId, String)>, String> {
    let http = reqwest::Client::builder().user_agent("Chillarr").connect_timeout(Duration::from_secs(20)).build().map_err(|e| e.to_string())?;
    let dir = folder();
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't make a folder for the downloads: {e}"))?;
    with(app, |m| m.has_downloads = !ids.is_empty());
    stage(app, "resolve", "Finding the latest versions");

    // Find every installer and its size first, so the bar's total is real from the start.
    let mut lookups = tokio::task::JoinSet::new();
    for &id in ids {
        let http = http.clone();
        lookups.spawn(async move {
            let found = match install::source(id) {
                Source::Winget(pkg) => {
                    let got = match from_manifest(&http, pkg).await {
                        Ok(i) => Some(i),
                        Err(_) => pinned(pkg),
                    };
                    got.map(|i| (i.url, i.sha256, Some(i.kind)))
                }
                Source::GithubZip { repo, tag, asset, sha256 } => Some((install::asset_url(repo, tag, asset), sha256.to_string(), None)),
            };
            let size = match &found {
                Some((url, ..)) => http.head(url).timeout(Duration::from_secs(10)).send().await.ok().filter(|r| r.status().is_success()).and_then(|r| {
                    // reqwest's content_length() is the empty body's 0 on a HEAD: read the header.
                    r.headers().get(reqwest::header::CONTENT_LENGTH)?.to_str().ok()?.parse().ok()
                }).unwrap_or(0),
                None => 0,
            };
            (id, found, size)
        });
    }
    let mut work = Vec::new();
    let mut winget_only = Vec::new();
    while let Some(done) = lookups.join_next().await {
        let Ok((id, found, size)) = done else { continue };
        match found {
            Some((url, sha, kind)) => {
                row(app, id, |r| r.total = size.max(1));
                work.push((id, url, sha, kind));
            }
            None => winget_only.push(id),
        }
    }

    stage(app, "download", "Downloading");
    let mut failed: Vec<(AppId, String)> = Vec::new();
    let mut ready = Vec::new();
    let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(DOWNLOADS_AT_ONCE));
    let mut set = tokio::task::JoinSet::new();
    for (id, url, sha, kind) in work {
        let (app, http, permits, dir) = (app.clone(), http.clone(), permits.clone(), dir.clone());
        set.spawn(async move {
            let _p = permits.acquire().await;
            let path = dir.join(if kind.is_some() { format!("{}.exe", id.label()) } else { format!("{}.zip", id.label()) });
            let r = fetch(&app, id, &http, &url, &sha, &path).await;
            (id, kind, path, sha, r)
        });
    }
    while let Some(done) = set.join_next().await {
        let Ok((id, kind, path, sha, r)) = done else { continue };
        match r {
            Ok(()) => ready.push((id, kind, path, sha)),
            Err(e) => {
                row(app, id, |r| {
                    r.state = "failed".into();
                    r.note = "Couldn't download".into();
                });
                log(app, Level::Red, format!("Couldn't download {}: {e}", id.label()));
                failed.push((id, format!("Couldn't download it: {e}")));
            }
        }
    }

    // FlareSolverr has no installer and needs no admin.
    let mut steps = Vec::new();
    for (id, kind, path, sha) in ready {
        match kind {
            Some(kind) => steps.push(Step { id, path, sha256: sha, kind }),
            None => {
                row(app, id, |r| r.state = "install".into());
                match install::unpack_zip(&path, &install::flare_dir()).await {
                    Ok(()) => row(app, id, |r| r.state = "done".into()),
                    Err(e) => {
                        row(app, id, |r| {
                            r.state = "failed".into();
                            r.note = "Couldn't unpack".into();
                        });
                        failed.push((id, e));
                    }
                }
                let _ = std::fs::remove_file(&path);
            }
        }
    }

    let installing: Vec<AppId> = steps.iter().map(|s| s.id).collect();
    let mut plan = Plan { steps, ..Default::default() };
    extras(app, &mut plan, ids);
    if !installing.is_empty() || !plan.services.is_empty() || plan.jellyfin_as_user.is_some() {
        stage(app, "permission", "Windows will ask for your OK once");
        elevated_pass(app, &plan, &installing, &mut failed).await?;
    }

    // Apps with no readable manifest: winget's own route, one at a time.
    for id in winget_only {
        let Source::Winget(pkg) = install::source(id) else { continue };
        row(app, id, |r| {
            r.state = "install".into();
            r.note = "Windows may ask for this one".into();
        });
        let pkg = pkg.to_string();
        match tokio::task::spawn_blocking(move || install::winget(&pkg)).await.unwrap_or_else(|e| Err(e.to_string())) {
            Ok(()) => row(app, id, |r| {
                r.state = "done".into();
                r.note.clear();
            }),
            Err(e) => {
                row(app, id, |r| {
                    r.state = "failed".into();
                    r.note = "Couldn't install".into();
                });
                failed.push((id, e));
            }
        }
    }
    if failed.is_empty() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    Ok(failed)
}

async fn elevated_pass(app: &AppHandle, plan: &Plan, installing: &[AppId], failed: &mut Vec<(AppId, String)>) -> Result<(), String> {
    let log_file = folder().join("progress.txt");
    let _ = std::fs::remove_file(&log_file);
    let text = script(plan, &log_file);
    let wait = tokio::task::spawn_blocking(move || crate::procs::run_elevated_for(&text, ELEVATED_WAIT_MS));
    tokio::pin!(wait);
    let mut seen = 0usize;
    let mut allowed = false;
    let total = installing.len().max(1);
    let mut done_count = 0usize;
    let mut asked = false;
    let result = loop {
        // Read after the script ends too: its last lines come just before it exits.
        let ended = tokio::select! {
            r = &mut wait => Some(r.unwrap_or_else(|e| Err(e.to_string()))),
            _ = tokio::time::sleep(Duration::from_millis(350)) => None,
        };
        for line in std::fs::read_to_string(&log_file).unwrap_or_default().lines().skip(seen) {
            seen += 1;
            if !asked {
                asked = true;
                stage(app, "install", "Installing");
            }
            let mut parts = line.split_whitespace();
            let verb = parts.next().unwrap_or("");
            if verb == "cfa" {
                allowed = true;
                continue;
            }
            let Some(id) = parts.next().and_then(app_by_label) else { continue };
            match verb {
                "i" => row(app, id, |r| r.state = "install".into()),
                "d" => {
                    done_count += 1;
                    row(app, id, |r| r.state = "done".into());
                    with(app, |m| m.frac = done_count as f64 / total as f64);
                }
                "f" => {
                    done_count += 1;
                    let why = parts.next().unwrap_or("error").to_string();
                    row(app, id, |r| {
                        r.state = "failed".into();
                        r.note = "Couldn't install".into();
                    });
                    log(app, Level::Red, format!("{} didn't install ({why}).", id.label()));
                    failed.push((id, format!("Its installer stopped ({why}).")));
                }
                _ => {}
            }
        }
        if let Some(r) = ended {
            break r;
        }
    };
    let _ = std::fs::remove_file(&log_file);
    if let Err(e) = result {
        if e.contains("declined") {
            return Err("declined".into());
        }
        return Err(e.replace("{what}", "Installing"));
    }
    if allowed {
        let st = app.state::<crate::SettingsState>();
        let saved = {
            let mut s = st.current.lock().unwrap();
            for p in &plan.allow_folders {
                let p = p.to_string_lossy().to_string();
                if !s.defender_allowed.contains(&p) {
                    s.defender_allowed.push(p);
                }
            }
            s.clone()
        };
        let _ = crate::settings::save(&st.path, &saved);
    }
    Ok(())
}

fn app_by_label(s: &str) -> Option<AppId> {
    AppId::ALL.into_iter().find(|a| a.label() == s)
}

/// The bar keeps moving through stages with no way to measure them (a first
/// start takes as long as it takes), slowing as it nears the stage's end so it
/// never claims more than it knows. Rows follow what each app reports.
fn creeping(app: &AppHandle) -> tauri::async_runtime::JoinHandle<()> {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(400)).await;
            let cards = app.state::<Core>().cards.lock().unwrap().clone();
            with(&app, |m| {
                if !m.running {
                    return;
                }
                if !matches!(m.stage.as_str(), "download" | "permission") {
                    m.frac += (0.92 - m.frac).max(0.0) * 0.012;
                }
                if matches!(m.stage.as_str(), "start" | "connect" | "sources" | "finish") {
                    for r in m.apps.iter_mut().filter(|r| r.state != "failed") {
                        if let Some(c) = cards.iter().find(|c| c.id == r.id) {
                            r.note = if c.level == Level::Ok { String::new() } else { c.line.clone() };
                        }
                    }
                }
            });
        }
    })
}

fn rows_for(app: &AppHandle, ids: &[AppId]) -> Vec<AppRow> {
    let core = app.state::<Core>();
    AppId::ALL
        .into_iter()
        .filter(|id| ids.contains(id) || core.config(*id).is_some_and(|c| crate::discover::is_installed(&c)))
        .map(|id| AppRow {
            id,
            label: id.label().to_string(),
            state: if ids.contains(&id) { "wait" } else { "have" }.to_string(),
            got: 0,
            total: 0,
            note: String::new(),
        })
        .collect()
}

fn missing_apps(app: &AppHandle) -> Vec<AppId> {
    let core = app.state::<Core>();
    AppId::ALL.into_iter().filter(|id| !core.config(*id).is_some_and(|c| crate::discover::is_installed(&c))).collect()
}

#[derive(Deserialize, Debug, Default)]
pub struct Request {
    #[serde(default)]
    pub movies: Vec<String>,
    #[serde(default)]
    pub series: Vec<String>,
    pub downloads: Option<String>,
    pub user: Option<String>,
    pub password: Option<String>,
    /// "sub" or "dub" (settings.audio_pref).
    pub audio_pref: Option<String>,
}

/// Everything, start to finish. Safe to run again: what's done stays done.
pub async fn run(app: &AppHandle, req: Request) -> Result<Meter, String> {
    let st = app.state::<Onboard>();
    let Ok(_one) = st.busy.try_lock() else { return Ok(snapshot(app)) };
    fix::rediscover_into(app).await;
    let ids = missing_apps(app);
    {
        let rows = rows_for(app, &ids);
        *st.meter.lock().unwrap() = Meter { running: true, stage: "resolve".into(), apps: rows, has_downloads: !ids.is_empty(), ..Default::default() };
    }
    *st.emit.lock().unwrap() = None;
    let tick = creeping(app);
    let done = whole_run(app, &ids, req).await;
    tick.abort();
    match done {
        Ok(()) => with(app, |m| {
            m.running = false;
            m.finished = true;
            m.stage = "done".into();
            m.line = "All set".into();
            m.speed = 0.0;
        }),
        Err(e) => with(app, |m| {
            m.running = false;
            if e == "declined" {
                m.declined = true;
            } else {
                m.problems.push(e);
            }
        }),
    }
    let snap = snapshot(app);
    let _ = app.emit("setup-progress", &snap);
    Ok(snap)
}

async fn whole_run(app: &AppHandle, ids: &[AppId], req: Request) -> Result<(), String> {
    let mut problems: Vec<String> = Vec::new();
    if let Some(pref) = req.audio_pref.as_deref() {
        let st = app.state::<crate::SettingsState>();
        let saved = {
            let mut s = st.current.lock().unwrap();
            s.audio_pref = if pref == "dub" { "dub" } else { "sub" }.into();
            s.clone()
        };
        let _ = crate::settings::save(&st.path, &saved);
    }
    if !ids.is_empty() {
        for (id, why) in get_apps(app, ids).await? {
            problems.push(format!("{}: {why}", id.label()));
        }
    }
    // Before anything reads it, and qBittorrent only reads it as it starts.
    if let Some(p) = req.downloads.as_deref().filter(|p| !p.is_empty()) {
        stage(app, "start", "Starting your apps");
        if let Err(e) = crate::media::set_download_folder(app, p).await {
            problems.push(e);
        }
    }
    stage(app, "start", "Starting your apps");
    fix::fix_all(app, Trigger::Button).await;

    stage(app, "connect", "Connecting everything");
    if let (Some(user), Some(password)) = (req.user.as_deref(), req.password.as_deref()) {
        if let Err(e) = crate::media::connect(app, user, password).await {
            problems.push(format!("Jellyfin: {e}"));
        }
        crate::login::apply(app, user, password).await;
    }
    if !req.movies.is_empty() || !req.series.is_empty() {
        if let Err(e) = crate::media::set_folders(app, req.movies, req.series).await {
            problems.push(e);
        }
    }

    stage(app, "sources", "Adding sources");
    let app2 = app.clone();
    let summary = crate::indexers::fill(app, true, move |added, total, waiting| {
        with(&app2, |m| {
            m.sites_added = added;
            m.sites_total = total;
            m.sites_waiting = waiting;
            if total > 0 {
                m.frac = added as f64 / total as f64;
            }
        })
    })
    .await;
    with(app, |m| {
        m.sites_added = summary.added;
        m.sites_total = summary.total;
        m.sites_waiting = summary.waiting;
    });

    stage(app, "finish", "Finishing up");
    // Everything's signed in now: the intro plugin, the saved folders and the
    // last round of checks that couldn't happen before.
    fix::fix_all(app, Trigger::Button).await;
    finish_setup(app);
    with(app, |m| m.problems.extend(problems));
    Ok(())
}

/// Marks setup finished and starts what listens for other devices.
fn finish_setup(app: &AppHandle) {
    let st = app.state::<crate::SettingsState>();
    let saved = {
        let mut s = st.current.lock().unwrap();
        s.setup_done = true;
        s.clone()
    };
    let _ = crate::settings::save(&st.path, &saved);
    start_servers(app);
}

/// The servers that listen for other devices. Starting them is what would make
/// Windows ask about Chillarr itself, so a new PC waits until the firewall rule
/// is in place rather than greeting a new user with that window.
pub fn start_servers(app: &AppHandle) {
    let st = app.state::<Onboard>();
    if st.servers.swap(true, Ordering::SeqCst) {
        return;
    }
    crate::remote::start(app.clone());
    crate::bridge::start(app.clone());
    crate::discover_shelf::spawn(app.clone());
}

/// One icon in the tray: Chillarr's. Sonarr and Radarr started in their tray
/// version (an older Chillarr, or a Startup shortcut) are closed and started
/// again without it, and qBittorrent's icon is switched off (it needs a restart
/// for that; its torrents carry on). Runs once, when Chillarr starts.
pub async fn one_tray_icon(app: &AppHandle) {
    let core = app.state::<Core>();
    // Never in the middle of a Fix everything or an install.
    let _busy = core.fixer.running.lock().await;
    for id in [AppId::Sonarr, AppId::Radarr, AppId::Prowlarr] {
        let Some(cfg) = core.config(id) else { continue };
        let (Some(exe), None, true) = (cfg.exe.clone(), cfg.service.as_ref(), cfg.managed) else { continue };
        if crate::procs::console_of(&exe).is_none() {
            continue;
        }
        let tray_name = format!("{}.exe", id.label());
        let check = tray_name.clone();
        if !tokio::task::spawn_blocking(move || crate::procs::process_running(&check)).await.unwrap_or(false) {
            continue;
        }
        let Some(arr) = crate::arr::Arr::new(&cfg) else { continue };
        if arr.shutdown().await.is_err() {
            continue;
        }
        for _ in 0..30 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let check = tray_name.clone();
            if !tokio::task::spawn_blocking(move || crate::procs::process_running(&check)).await.unwrap_or(false) {
                break;
            }
        }
        if let Err(e) = crate::procs::start_arr(&exe, id.label()) {
            log(app, Level::Red, e);
        }
    }
    let mut rep = fix::FixReport::default();
    if core.config(AppId::QBittorrent).is_some_and(|c| c.managed) {
        fix::wire_qbit_ini(app, &mut rep).await;
    }
    // Started by something else, or before this: out of sight too.
    if core.config(AppId::QBittorrent).is_some_and(|c| c.managed) && tokio::task::spawn_blocking(|| crate::procs::process_running("qbittorrent.exe")).await.unwrap_or(false) {
        let _ = tokio::task::spawn_blocking(crate::procs::hide_qbit_windows).await;
    }
}

/// The tray's Stop everything: with one icon, this is the way to close them all.
pub async fn stop_everything(app: &AppHandle) {
    let core = app.state::<Core>();
    for id in [AppId::Sonarr, AppId::Radarr, AppId::Prowlarr] {
        if let Some(arr) = core.config(id).filter(|c| c.service.is_none()).as_ref().and_then(crate::arr::Arr::new) {
            let _ = arr.shutdown().await;
        }
    }
    if let Some(cfg) = core.config(AppId::QBittorrent) {
        fix::close_qbit(&cfg.base_url).await;
    }
    if core.config(AppId::Jellyfin).is_some_and(|c| c.service.is_none()) {
        let _ = tokio::task::spawn_blocking(|| crate::procs::force_close("jellyfin.exe")).await;
    }
    let _ = tokio::task::spawn_blocking(|| crate::procs::force_close("meilisearch.exe")).await;
}

/// Settings' Install button: the same one prompt and meter for just these apps.
pub async fn install_only(app: &AppHandle, ids: Vec<AppId>) -> Vec<(AppId, String)> {
    let st = app.state::<Onboard>();
    let Ok(_one) = st.busy.try_lock() else { return vec![] };
    {
        let rows: Vec<AppRow> = rows_for(app, &ids).into_iter().filter(|r| ids.contains(&r.id)).collect();
        *st.meter.lock().unwrap() = Meter { running: true, stage: "resolve".into(), apps: rows, has_downloads: true, installing_only: true, ..Default::default() };
    }
    *st.emit.lock().unwrap() = None;
    let tick = creeping(app);
    let result = get_apps(app, &ids).await;
    tick.abort();
    let failed = match result {
        Ok(f) => f,
        Err(e) if e == "declined" => {
            with(app, |m| {
                m.running = false;
                m.declined = true;
            });
            let snap = snapshot(app);
            let _ = app.emit("setup-progress", &snap);
            return ids.iter().map(|id| (*id, "Windows' prompt was turned down.".to_string())).collect();
        }
        Err(e) => ids.iter().map(|id| (*id, e.clone())).collect(),
    };
    with(app, |m| {
        m.running = false;
        m.finished = true;
        m.stage = "done".into();
        m.problems = failed.iter().map(|(id, e)| format!("{}: {e}", id.label())).collect();
    });
    let snap = snapshot(app);
    let _ = app.emit("setup-progress", &snap);
    let h = app.clone();
    tauri::async_runtime::spawn(async move {
        fix::fix_all(&h, Trigger::Button).await;
    });
    failed
}

/// Where the setup screen suggests putting everything: the fixed drive with the
/// most room. On C: that's a folder in the user's profile rather than the root.
pub fn suggested_folder() -> serde_json::Value {
    use windows::core::HSTRING;
    use windows::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives};
    const DRIVE_FIXED: u32 = 3;
    let mut best: Option<(char, u64)> = None;
    let mask = unsafe { GetLogicalDrives() };
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root = HSTRING::from(format!("{letter}:\\"));
        if unsafe { GetDriveTypeW(&root) } != DRIVE_FIXED {
            continue;
        }
        let mut free = 0u64;
        if unsafe { GetDiskFreeSpaceExW(&root, Some(&mut free), None, None) }.is_ok() && best.is_none_or(|(_, f)| free > f) {
            best = Some((letter, free));
        }
    }
    let (letter, free) = best.unwrap_or(('C', 0));
    let path = suggested_path(letter, std::env::var("USERPROFILE").ok().as_deref(), std::env::var("SystemDrive").ok().as_deref());
    serde_json::json!({ "path": path, "free": free })
}

fn suggested_path(letter: char, profile: Option<&str>, system: Option<&str>) -> String {
    let on_system = system.and_then(|s| s.chars().next()).unwrap_or('C').eq_ignore_ascii_case(&letter);
    match profile.filter(|_| on_system) {
        // Not under Videos or Documents: those are what Defender's protected-folder rule guards.
        Some(home) => format!("{}\\Chillarr Media", home.trim_end_matches('\\')),
        None => format!("{letter}:\\Media"),
    }
}

/// A plan with every kind of line, for the check that the script is valid PowerShell.
#[cfg(test)]
pub fn sample_script() -> String {
    script(
        &Plan {
            steps: vec![Step { id: AppId::Sonarr, path: PathBuf::from(r"C:\Users\O'Brien\Sonarr.exe"), sha256: "ab".repeat(32), kind: Kind::Inno }],
            walls: vec![Wall { name: "Sonarr".into(), exe: PathBuf::from(r"C:\a b\Sonarr.exe"), allow: true, lan_only: true }],
            allow_folders: vec![PathBuf::from(r"C:\a b\x.exe"), PathBuf::from(r"C:\c\y.exe")],
            services: vec!["Radarr".into()],
            grant_folders: vec![(r"C:\ProgramData\Sonarr".into(), r"PC\me".into())],
            jellyfin_as_user: Some(("JellyfinServer".into(), r"C:\ProgramData\Jellyfin\Server".into(), r"PC\me".into())),
        },
        Path::new(r"C:\Users\O'Brien\log.txt"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SONARR: &str = "PackageIdentifier: TeamSonarr.Sonarr\nInstallerType: inno\nScope: machine\nAppsAndFeaturesEntries:\n- DisplayVersion: '4'\nInstallers:\n- Architecture: x86\n  InstallerUrl: https://github.com/Sonarr/Sonarr/releases/download/v4/x86-installer.exe\n  InstallerSha256: 6291A088F0504B9746C288912D8809F74B2CC804B6BBA8D8E60A8CB2C7C079DD\n- Architecture: x64\n  InstallerUrl: https://github.com/Sonarr/Sonarr/releases/download/v4/x64-installer.exe\n  InstallerSha256: C431C9DBA573B0F647772A2B76F8D8F356BDF7FB735A13CEE8E6A892253D9352\nManifestType: installer\n";

    #[test]
    fn reads_the_x64_installer_from_a_manifest() {
        let i = parse_manifest(SONARR).unwrap();
        assert!(i.url.ends_with("x64-installer.exe"));
        assert_eq!(i.kind, Kind::Inno);
        assert_eq!(i.sha256.len(), 64);
    }

    #[test]
    fn reads_quoted_values_and_the_installer_type_inside_the_list() {
        let y = "Installers:\n- Architecture: x64\n  InstallerType: nullsoft\n  InstallerUrl: \"https://repo.jellyfin.org/a.exe\"\n  InstallerSha256: '52085AD1156450146FFDBDA16C783683218BFCF9A6FA70B8232C7B6A36B4A9C6'\nManifestType: installer\n";
        assert_eq!(parse_manifest(y).unwrap().kind, Kind::Nullsoft);
    }

    #[test]
    fn a_manifest_it_cant_trust_or_run_gives_nothing() {
        assert_eq!(parse_manifest(&SONARR.replace("https://github.com", "https://evil.example")), None, "another host");
        assert_eq!(parse_manifest(&SONARR.replace("inno", "burn")), None, "an installer type with no known silent switch");
        assert_eq!(parse_manifest(&SONARR.replace("C431C9DBA573B0F647772A2B76F8D8F356BDF7FB735A13CEE8E6A892253D9352", "short")), None);
        assert_eq!(parse_manifest("Installers:\n- Architecture: x86\n"), None);
    }

    #[test]
    fn newest_version_skips_folders_that_arent_versions() {
        let v = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(newest_version(&v(&["5.2.4", "5.3.0", "Beta", "lt2"])).as_deref(), Some("5.3.0"));
        assert_eq!(newest_version(&v(&["4.0.9.1", "4.0.14.2939"])).as_deref(), Some("4.0.14.2939"), "numbers, not text");
        assert_eq!(newest_version(&v(&["Beta"])), None);
    }

    #[test]
    fn manifest_folders() {
        assert_eq!(manifest_dir("TeamSonarr.Sonarr").unwrap(), "manifests/t/TeamSonarr/Sonarr");
        assert_eq!(manifest_dir("qBittorrent.qBittorrent").unwrap(), "manifests/q/qBittorrent/qBittorrent");
    }

    #[test]
    fn every_pinned_installer_is_trusted_and_whole() {
        for pkg in ["TeamSonarr.Sonarr", "TeamRadarr.Radarr", "TeamProwlarr.Prowlarr", "qBittorrent.qBittorrent", "Jellyfin.Server"] {
            let i = pinned(pkg).unwrap();
            assert!(trusted_host(&i.url), "{pkg}");
            assert_eq!(i.sha256.len(), 64, "{pkg}");
        }
    }

    #[test]
    fn the_bar_only_goes_forward_through_the_stages() {
        for has in [true, false] {
            let mut at = 0.0;
            for s in ["resolve", "download", "permission", "install", "start", "connect", "sources", "finish", "done"] {
                let (a, b) = span(s, has);
                assert!(a >= at, "{s} starts before the last one ended");
                assert!(b >= a);
                at = b;
            }
            assert_eq!(at, 100.0);
        }
        assert_eq!(overall("download", true, 0.5), 32.0);
        assert_eq!(overall("download", true, 9.0), 62.0, "never past the stage's end");
    }

    fn sample_plan() -> Plan {
        Plan {
            steps: vec![
                Step { id: AppId::Sonarr, path: PathBuf::from(r"C:\Users\O'Brien\AppData\Local\Chillarr\installers\Sonarr.exe"), sha256: "ab".repeat(32), kind: Kind::Inno },
                Step { id: AppId::Jellyfin, path: PathBuf::from(r"C:\x\Jellyfin.exe"), sha256: "cd".repeat(32), kind: Kind::Nullsoft },
            ],
            walls: vec![
                Wall { name: "Jellyfin".into(), exe: PathBuf::from(r"C:\Program Files\Jellyfin\Server\jellyfin.exe"), allow: true, lan_only: false },
                Wall { name: "FlareSolverr".into(), exe: PathBuf::from(r"C:\f\flaresolverr.exe"), allow: false, lan_only: false },
            ],
            allow_folders: vec![PathBuf::from(r"C:\Program Files\qBittorrent\qbittorrent.exe")],
            services: vec!["Radarr".into()],
            grant_folders: vec![(r"C:\ProgramData\Sonarr".into(), r"PC\me".into())],
            jellyfin_as_user: Some(("JellyfinServer".into(), r"C:\ProgramData\Jellyfin\Server".into(), r"PC\me".into())),
        }
    }

    #[test]
    fn the_script_checks_the_hash_where_the_file_will_run_from() {
        let s = script(&sample_plan(), Path::new(r"C:\log.txt"));
        assert!(s.contains("Copy-Item"));
        let copy = s.find("Copy-Item").unwrap();
        let hash = s.find("Get-FileHash").unwrap();
        let run = s.find("Start-Process").unwrap();
        assert!(copy < hash && hash < run, "copied, then hashed, then run");
        assert!(s.contains(&format!("'{}'", "AB".repeat(32))), "hashes are compared in Get-FileHash's upper case");
    }

    #[test]
    fn the_script_installs_silently_and_never_makes_a_service_for_the_arr_apps() {
        let s = script(&sample_plan(), Path::new(r"C:\log.txt"));
        assert!(s.contains("/VERYSILENT") && s.contains("/TASKS=none"));
        assert!(s.contains("'/S'"));
        assert!(s.contains(r"'C:\Users\O''Brien\AppData"), "paths stay quoted");
    }

    #[test]
    fn the_script_closes_flaresolverr_and_opens_jellyfin() {
        let s = script(&sample_plan(), Path::new(r"C:\log.txt"));
        assert!(s.contains("Wall 'Chillarr: FlareSolverr' 'C:\\f\\flaresolverr.exe' 'Block' 'Any'"), "{s}");
        assert!(s.contains("Wall 'Chillarr: Jellyfin'") && s.contains("'Allow' 'Any'"));
    }

    #[test]
    fn jellyfins_service_stops_before_its_folder_is_granted() {
        let s = script(&sample_plan(), Path::new(r"C:\log.txt"));
        assert!(s.find("'stop' 'JellyfinServer'").unwrap() < s.find(r"'C:\ProgramData\Jellyfin\Server'").unwrap());
    }

    #[test]
    fn the_script_fits_on_a_command_line() {
        let s = script(&sample_plan(), Path::new(r"C:\Users\someone\AppData\Local\Chillarr\installers\progress.txt"));
        let encoded = s.encode_utf16().count() * 2 * 4 / 3;
        assert!(encoded < 28_000, "{encoded} of Windows' 32k");
    }

    #[test]
    fn folders_land_on_the_roomy_drive_or_in_the_profile() {
        assert_eq!(suggested_path('D', Some(r"C:\Users\me"), Some("C:")), r"D:\Media");
        assert_eq!(suggested_path('C', Some(r"C:\Users\me\"), Some("C:")), r"C:\Users\me\Chillarr Media");
        assert_eq!(suggested_path('c', None, Some("C:")), r"c:\Media");
    }

    /// Runs the real script without admin against tiny fake installers, in a
    /// temp folder: the copy, the hash check and the result lines.
    #[test]
    #[ignore]
    fn live_script_runs_and_refuses_a_swapped_file() {
        let dir = tempfile::tempdir().unwrap();
        let bat = |name: &str, code: u8| {
            let p = dir.path().join(name);
            // Any small program that exits at once, whatever its arguments.
            std::fs::copy(crate::procs::sys32("hostname.exe"), &p).unwrap();
            let _ = code;
            p
        };
        let good = bat("Sonarr.exe", 0);
        let hash = |p: &Path| -> String { Sha256::digest(std::fs::read(p).unwrap()).iter().map(|b| format!("{b:02x}")).collect() };
        let plan = Plan {
            steps: vec![
                Step { id: AppId::Radarr, path: good.clone(), sha256: "00".repeat(32), kind: Kind::Nullsoft },
                Step { id: AppId::Sonarr, path: good.clone(), sha256: hash(&good), kind: Kind::Nullsoft },
            ],
            ..Default::default()
        };
        let log_path = dir.path().join("log.txt");
        // Writable stand-in for Windows' Temp, since this isn't elevated.
        let body = script(&plan, &log_path).replace("(Split-Path $sys)", &ps_quote(&dir.path().to_string_lossy()));
        let full = format!("$sys = [Environment]::SystemDirectory\n{body}");
        let file = dir.path().join("t.ps1");
        std::fs::write(&file, full).unwrap();
        let status = std::process::Command::new(crate::procs::powershell()).args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]).arg(&file).status().unwrap();
        assert!(status.success());
        let got = std::fs::read_to_string(&log_path).unwrap();
        assert!(got.contains("f Radarr hash"), "{got}");
        assert!(got.contains("i Sonarr"), "{got}");
        assert!(got.trim_end().ends_with("end"), "{got}");
    }

    /// The real manifests and hosts: every installer can be found and its size read.
    #[tokio::test]
    #[ignore]
    async fn live_every_installer_resolves() {
        let http = reqwest::Client::builder().user_agent("Chillarr").build().unwrap();
        for pkg in ["TeamSonarr.Sonarr", "TeamRadarr.Radarr", "TeamProwlarr.Prowlarr", "qBittorrent.qBittorrent", "Jellyfin.Server"] {
            let i = from_manifest(&http, pkg).await.unwrap_or_else(|e| panic!("{pkg}: {e}"));
            let r = http.head(&i.url).send().await.unwrap();
            println!("{pkg}: {:?} {:?} {} {}", i.kind, r.content_length(), r.status(), i.url);
            assert!(r.status().is_success(), "{pkg}");
        }
    }

    /// qBittorrent's SourceForge link really hands over the file, not a web page.
    #[tokio::test]
    #[ignore]
    async fn live_sourceforge_gives_the_installer() {
        let http = reqwest::Client::builder().user_agent("Chillarr").build().unwrap();
        let i = pinned("qBittorrent.qBittorrent").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("q.exe");
        let mut resp = http.get(&i.url).send().await.unwrap().error_for_status().unwrap();
        let mut f = std::fs::File::create(&path).unwrap();
        while let Some(c) = resp.chunk().await.unwrap() {
            std::io::Write::write_all(&mut f, &c).unwrap();
        }
        drop(f);
        assert!(hash_ok(&path, &i.sha256).await, "size {}", std::fs::metadata(&path).unwrap().len());
    }
}
