//! "Fix everything": start what is down, test every indexer in Prowlarr, push
//! them to Sonarr and Radarr, re-test there so their old failure marks clear,
//! then read health back. Order and rules are the spec's "The fix pipeline".

use crate::arr::{Arr, ArrError, Health, TestResult};
use crate::log::log;
use crate::model::{AppConfig, AppId, Level};
use crate::procs::{self, Procs};
use crate::status::{self, CardState, IndexerRow};
use std::collections::HashMap;
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

const GUARD_INDEXER_GAP: Duration = Duration::from_secs(15 * 60);
const WAIT_FLARE_SECS: u64 = 60;
const WAIT_ARR_SECS: u64 = 90;
const WAIT_QBIT_SECS: u64 = 30;
const SYNC_WAIT: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Trigger {
    Button,
    Guard,
}

#[derive(Default)]
pub struct Fixer {
    running: tokio::sync::Mutex<()>,
    /// What holds `running` right now; `None` for an install or nothing.
    current: Mutex<Option<Trigger>>,
    last_indexer_test: Mutex<Option<Instant>>,
}

/// A request that arrives while another run holds the lock joins it (gets its
/// result) only if that run does at least as much. A button press during a
/// guard run or an install waits, then runs the full button pipeline.
pub fn should_join(running: Option<Trigger>, incoming: Trigger) -> bool {
    match running {
        Some(Trigger::Button) => true,
        Some(Trigger::Guard) => incoming == Trigger::Guard,
        None => false,
    }
}

/// An app discovery couldn't fully set up (e.g. installed but never started,
/// so no config.xml yet) is worth looking for again.
pub fn needs_rediscovery(cfgs: &[AppConfig]) -> bool {
    cfgs.iter().any(|c| c.problem.is_some())
}

impl Fixer {
    pub fn try_begin(&self) -> Option<tokio::sync::MutexGuard<'_, ()>> {
        self.running.try_lock().ok()
    }

    pub fn indexer_tests_allowed(&self, trigger: Trigger, now: Instant) -> bool {
        trigger == Trigger::Button
            || match *self.last_indexer_test.lock().unwrap() {
                None => true,
                Some(at) => now.saturating_duration_since(at) > GUARD_INDEXER_GAP,
            }
    }

    pub fn mark_indexer_test(&self, now: Instant) {
        *self.last_indexer_test.lock().unwrap() = Some(now);
    }
}

/// Everything the pipeline and the window share.
pub struct Core {
    pub configs: RwLock<Vec<AppConfig>>,
    pub procs: Procs,
    pub fixer: Fixer,
    pub cards: Mutex<Vec<CardState>>,
    /// Newer versions found by the daily check.
    pub updates: Mutex<HashMap<AppId, String>>,
}

impl Core {
    pub fn new(configs: Vec<AppConfig>) -> Core {
        Core {
            configs: RwLock::new(configs),
            procs: Procs::new(),
            fixer: Fixer::default(),
            cards: Mutex::new(Vec::new()),
            updates: Mutex::new(HashMap::new()),
        }
    }

    pub fn config(&self, id: AppId) -> Option<AppConfig> {
        self.configs.read().unwrap().iter().find(|c| c.id == id).cloned()
    }
}

#[derive(Default)]
pub struct FixReport {
    pub cards: Vec<CardState>,
    /// What Chillarr changed, one plain sentence each (for notifications).
    pub actions: Vec<String>,
    /// Problems only the button can fix (a stopped service needs UAC).
    pub needs_button: Vec<String>,
}

/// What an app still marks failed although it just passed: indexers that passed
/// Whether any card shows a problem a fix can do something about.
pub fn anything_to_fix(cards: &[CardState]) -> bool {
    cards.iter().any(|c| c.fixable && matches!(c.level, Level::Red | Level::Amber))
}

/// in Prowlarr, and (for Prowlarr) the FlareSolverr proxy if its test passed.
pub fn stale_marks(health: &[Health], passed: &[String], proxies_ok: bool) -> Vec<String> {
    let mut out: Vec<String> = status::failing_names(health).into_iter().filter(|n| passed.contains(n)).collect();
    if proxies_ok && health.iter().any(|h| h.source == "IndexerProxyStatusCheck") {
        out.push("FlareSolverr".into());
    }
    out
}

fn set_card(app: &AppHandle, card: CardState) {
    let core = app.state::<Core>();
    let mut cards = core.cards.lock().unwrap();
    match cards.iter_mut().find(|c| c.id == card.id) {
        Some(c) => *c = card.clone(),
        None => cards.push(card.clone()),
    }
    let _ = app.emit("card", &card);
}

/// A short-lived amber state while a step runs ("Starting...", "Testing indexers...").
fn busy(app: &AppHandle, id: AppId, line: &str) {
    let open_url = app.state::<Core>().config(id).filter(|c| c.id != AppId::FlareSolverr).map(|c| c.base_url);
    set_card(app, CardState { id, level: Level::Amber, line: line.into(), notes: vec![], open_url, fixable: false, missing: false, update: None });
}

fn missing_card(cfg: &AppConfig) -> CardState {
    let notes = if cfg.id == AppId::FlareSolverr {
        vec!["Already have it somewhere? Pick flaresolverr.exe in Settings.".into()]
    } else {
        vec![]
    };
    CardState {
        id: cfg.id,
        level: Level::Unknown,
        line: "Not installed".into(),
        notes,
        open_url: None,
        fixable: false,
        missing: true,
        update: None,
    }
}

async fn card_for(core: &Core, cfg: &AppConfig) -> CardState {
    if !crate::discover::is_installed(cfg) && !procs::port_open(&cfg.base_url).await {
        return missing_card(cfg);
    }
    let mut card = if let Some(arr) = Arr::new(cfg) {
        match arr.health().await {
            Ok(h) => status::card_from(cfg.id, Ok(()), &h),
            Err(e) => status::card_from(cfg.id, Err(e), &[]),
        }
    } else if cfg.id.is_arr() {
        CardState {
            id: cfg.id,
            level: Level::Red,
            line: cfg.problem.clone().unwrap_or_else(|| "Not set up".into()),
            notes: vec![],
            open_url: None,
            fixable: true,
            missing: false,
            update: None,
        }
    } else if procs::port_open(&cfg.base_url).await {
        let mut c = CardState { id: cfg.id, level: Level::Ok, line: "Running".into(), notes: vec![], open_url: None, fixable: false, missing: false, update: None };
        if cfg.id == AppId::FlareSolverr && core.procs.owns_flaresolverr() {
            c.notes.push("Started by Chillarr, hidden".into());
        }
        c
    } else {
        CardState {
            id: cfg.id,
            level: Level::Red,
            line: "Not running".into(),
            notes: vec![],
            open_url: None,
            fixable: true,
            missing: false,
            update: None,
        }
    };
    if cfg.id != AppId::FlareSolverr && !cfg.base_url.is_empty() {
        card.open_url = Some(cfg.base_url.clone());
    }
    if !cfg.managed {
        card.notes.push("Chillarr won't start this one (Settings)".into());
    }
    card.update = core.updates.lock().unwrap().get(&cfg.id).cloned();
    if cfg.id == AppId::Jellyfin && card.level == Level::Ok {
        let j = crate::jellyfin::Jf::new(&cfg.base_url, "chillarr-check", None);
        if j.public_info().await == Ok(false) {
            card.level = Level::Amber;
            card.line = "Not set up yet. Open Setup to finish it.".into();
        }
    }
    card
}

pub async fn refresh_cards(app: &AppHandle) -> Vec<CardState> {
    let core = app.state::<Core>();
    let configs = core.configs.read().unwrap().clone();
    let mut out = Vec::new();
    for cfg in &configs {
        let card = card_for(&core, cfg).await;
        set_card(app, card.clone());
        out.push(card);
    }
    out
}

pub async fn indexer_table_now(app: &AppHandle) -> Vec<IndexerRow> {
    let core = app.state::<Core>();
    let mut cols: [Vec<(String, status::Cell)>; 3] = Default::default();
    for (i, id) in [AppId::Prowlarr, AppId::Sonarr, AppId::Radarr].into_iter().enumerate() {
        let Some(arr) = core.config(id).as_ref().and_then(Arr::new) else { continue };
        if let (Ok(idx), Ok(h)) = (arr.indexers().await, arr.health().await) {
            cols[i] = status::indexer_cells(&idx, &h);
        }
    }
    let rows = status::indexer_table(&cols[0], &cols[1], &cols[2]);
    let _ = app.emit("indexers", &rows);
    rows
}

/// Step 1 and 2 for one app: start it if nothing answers, then wait for it.
async fn start_if_down(app: &AppHandle, cfg: &AppConfig, trigger: Trigger, rep: &mut FixReport) {
    if cfg.base_url.is_empty() || procs::port_open(&cfg.base_url).await {
        return;
    }
    if cfg.id == AppId::Jellyfin {
        // Chillarr is restarting it onto its web pages; starting a second
        // server on the same data folder meanwhile would clash.
        if crate::jfweb::restarting() {
            return;
        }
        // Jellyfin's search plugin looks for its engine once, as Jellyfin starts.
        crate::meili::start(app).await;
    }
    let webdir = match (&cfg.exe, cfg.id) {
        (Some(exe), AppId::Jellyfin) if cfg.service.is_none() => crate::jfweb::prepare(app, exe).await.map(|(d, _)| d),
        _ => None,
    };
    let core = app.state::<Core>();
    let name = cfg.id.label();
    let since = qbit_log_now();
    let (started, wait) = match cfg.id {
        AppId::FlareSolverr => match &cfg.exe {
            Some(exe) if procs::should_spawn(core.procs.owns_flaresolverr(), false) => {
                (core.procs.start_flaresolverr(exe), WAIT_FLARE_SECS)
            }
            Some(_) => return,
            None => return log(app, Level::Red, "FlareSolverr isn't running and Chillarr can't find flaresolverr.exe. Pick it in Settings."),
        },
        AppId::QBittorrent => match &cfg.exe {
            // Open but not answering: its Web UI is off. Starting it again only
            // brings its window to the front, so leave that to Fix everything.
            Some(_) if procs::process_running("qbittorrent.exe") => {
                if trigger == Trigger::Guard {
                    let msg = "qBittorrent is open but Chillarr can't reach it. Press Fix everything to turn on its Web UI.".to_string();
                    if !rep.needs_button.contains(&msg) {
                        rep.needs_button.push(msg);
                    }
                }
                return;
            }
            Some(exe) => (procs::start_qbit(exe), WAIT_QBIT_SECS),
            None => return,
        },
        AppId::Jellyfin if cfg.service.is_none() => match &cfg.exe {
            Some(exe) => (procs::start_jellyfin(exe, webdir.as_deref()), WAIT_ARR_SECS),
            None => return,
        },
        _ => match (&cfg.service, &cfg.exe) {
            (Some(svc), _) => {
                if trigger == Trigger::Guard {
                    // A running service with a closed port is usually still starting
                    // (right after boot). Only a stopped one is worth telling about.
                    if procs::service_running(svc) == Some(true) {
                        return;
                    }
                    let msg = format!("{name} is stopped. Open Chillarr and press Fix everything.");
                    log(app, Level::Red, &msg);
                    rep.needs_button.push(msg);
                    return;
                }
                // Running but not answering means it is stuck: restart it.
                let restart = procs::service_running(svc) == Some(true);
                let svc = svc.clone();
                let r = tokio::task::spawn_blocking(move || procs::start_service_elevated(&svc, restart))
                    .await
                    .unwrap_or_else(|e| Err(e.to_string()));
                (r, WAIT_ARR_SECS)
            }
            (None, Some(exe)) => (procs::start_arr(exe, name), WAIT_ARR_SECS),
            (None, None) => return,
        },
    };
    match started {
        Ok(()) => {
            busy(app, cfg.id, "Starting...");
            if procs::wait_until(&cfg.base_url, wait).await {
                let msg = format!("{name} was not running. Started it.");
                log(app, Level::Ok, &msg);
                rep.actions.push(msg);
            } else {
                let why = (cfg.id == AppId::QBittorrent).then(|| qbit_webui_failure(&since)).flatten();
                let why = why.map(|w| format!(" qBittorrent says: {w}")).unwrap_or_default();
                log(app, Level::Red, format!("Started {name}, but it didn't answer within {wait} seconds.{why}"));
            }
        }
        Err(e) => log(app, Level::Red, e),
    }
}

/// The time as qBittorrent stamps its log lines, to read only what a start wrote.
fn qbit_log_now() -> String {
    chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string()
}

/// What qBittorrent logged about its Web UI failing since `since`, if anything.
/// Settings it ignores (a layout a future version moved) show up here instead
/// of as a silent timeout.
fn qbit_webui_failure(since: &str) -> Option<String> {
    let path = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("qBittorrent").join("logs").join("qbittorrent.log");
    let bytes = std::fs::read(path).ok()?;
    crate::wire::qbit_webui_error(&String::from_utf8_lossy(&bytes), since)
}

fn first_error(r: &TestResult) -> String {
    r.validation_failures.first().map(|f| f.error_message.clone()).unwrap_or_else(|| "failed".into())
}

fn describe(e: &ArrError) -> String {
    match e {
        ArrError::Down => "it isn't answering".into(),
        ArrError::BadKey => "it rejected the API key".into(),
        ArrError::Http(c) => format!("it answered with error {c}"),
        ArrError::Rejected(m) | ArrError::Other(m) => m.clone(),
    }
}

async fn id_names(arr: &Arr, path: &str) -> HashMap<i64, String> {
    arr.names(path).await.unwrap_or_default().into_iter().map(|i| (i.id, i.name)).collect()
}

/// Radarr's test reads a site's newest uploads, and some sites (The Pirate
/// Bay, LimeTorrents) list none in the movie categories even though searching
/// them works (TPB had copies of Hope, 2026-10-19). That's no failure.
const FEEDLESS_SITES: [&str; 2] = ["The Pirate Bay", "LimeTorrents"];

fn feedless(error: &str) -> bool {
    error.contains("no results in the configured categories")
}

/// Logs each failed test; returns the canonical names that passed.
async fn run_tests(app: &AppHandle, app_name: &str, kind: &str, arr: &Arr, list: &str, results: Result<Vec<TestResult>, ArrError>) -> Vec<String> {
    let names = id_names(arr, list).await;
    match results {
        Ok(rs) => {
            let mut passed = Vec::new();
            for r in &rs {
                let n = names.get(&r.id).map(String::as_str).unwrap_or("?");
                if r.is_valid || feedless(&first_error(r)) {
                    passed.push(status::canonical_name(n).to_string());
                } else {
                    log(app, Level::Red, format!("{} failed in {app_name}: {}", status::canonical_name(n), first_error(r)));
                }
            }
            if !rs.is_empty() {
                log(app, Level::Ok, format!("{app_name}: {} of {} {kind} passed.", passed.len(), rs.len()));
            }
            passed
        }
        Err(e) => {
            log(app, Level::Red, format!("Couldn't test {kind} in {app_name}: {}.", describe(&e)));
            Vec::new()
        }
    }
}

/// Step 3: Prowlarr tests its proxy, indexers and apps, then syncs to Sonarr and Radarr.
async fn prowlarr_steps(app: &AppHandle) -> (Vec<String>, bool) {
    let Some(arr) = app.state::<Core>().config(AppId::Prowlarr).as_ref().and_then(Arr::new) else { return (vec![], false) };
    busy(app, AppId::Prowlarr, "Testing indexers...");
    let mut proxies_ok = false;
    match arr.test_proxies().await {
        Ok(ps) => {
            proxies_ok = !ps.is_empty() && ps.iter().all(|(_, e)| e.is_none());
            for (name, err) in ps {
                match err {
                    None => log(app, Level::Ok, format!("Prowlarr can use {name}.")),
                    Some(e) => log(app, Level::Red, format!("Prowlarr can't use {name}: {e}")),
                }
            }
        }
        Err(e) => log(app, Level::Red, format!("Couldn't test proxies in Prowlarr: {}.", describe(&e))),
    }
    let passed = run_tests(app, "Prowlarr", "indexers", &arr, "indexer", arr.test_all_indexers().await).await;
    run_tests(app, "Prowlarr", "apps", &arr, "applications", arr.test_all_applications().await).await;
    match arr.command("ApplicationIndexerSync").await {
        Ok(id) => {
            let deadline = Instant::now() + SYNC_WAIT;
            while Instant::now() < deadline && !arr.command_done(id).await.unwrap_or(true) {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            log(app, Level::Ok, "Prowlarr pushed its indexers to Sonarr and Radarr.");
        }
        Err(e) => log(app, Level::Red, format!("Prowlarr couldn't sync its indexers: {}.", describe(&e))),
    }
    (passed, proxies_ok)
}

/// Health pages are only re-read on a schedule, so after the tests they still
/// show the old failures. CheckHealth makes each app re-read them now.
async fn check_health(app: &AppHandle, ids: &[AppId]) {
    let core = app.state::<Core>();
    for &id in ids {
        let Some(arr) = core.config(id).as_ref().and_then(Arr::new) else { continue };
        if let Ok(cmd) = arr.command("CheckHealth").await {
            let deadline = Instant::now() + SYNC_WAIT;
            while Instant::now() < deadline && !arr.command_done(cmd).await.unwrap_or(true) {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
}

/// Step 4 for Sonarr or Radarr: re-test indexers and download clients there.
async fn arr_tests(app: &AppHandle, id: AppId) {
    let Some(arr) = app.state::<Core>().config(id).as_ref().and_then(Arr::new) else { return };
    busy(app, id, "Testing...");
    let name = id.label();
    // Radarr: sites with no movie feed always fail its test, and each failure
    // benches them for longer (The Pirate Bay, 2026-10-19), so they're left out.
    let results = if id == AppId::Radarr { arr.test_indexers_skipping(|n| FEEDLESS_SITES.iter().any(|s| n.contains(s))).await } else { arr.test_all_indexers().await };
    run_tests(app, name, "indexers", &arr, "indexer", results).await;
    run_tests(app, name, "download clients", &arr, "downloadclient", arr.test_all_download_clients().await).await;
}

/// The button's fallback when a passing test did not clear a failure mark:
/// restart the app (through its own API, no admin) and test it once more.
async fn restart_if_stale(app: &AppHandle, id: AppId, passed: &[String], proxies_ok: bool, rep: &mut FixReport) {
    let Some(cfg) = app.state::<Core>().config(id) else { return };
    let Some(arr) = Arr::new(&cfg) else { return };
    let Ok(health) = arr.health().await else { return };
    let stale = stale_marks(&health, passed, id == AppId::Prowlarr && proxies_ok);
    if stale.is_empty() {
        return;
    }
    let name = id.label();
    log(app, Level::Amber, format!("{name} still marks {} failed. Restarting {name} to clear it.", stale.join(", ")));
    busy(app, id, "Restarting...");
    if let Err(e) = arr.restart().await {
        return log(app, Level::Red, format!("Couldn't restart {name}: {}.", describe(&e)));
    }
    // Give it a moment to go down before waiting for it to come back.
    tokio::time::sleep(Duration::from_secs(5)).await;
    if procs::wait_until(&cfg.base_url, WAIT_ARR_SECS).await {
        // The web host answers before the app has finished starting.
        for _ in 0..30 {
            if arr.status().await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        rep.actions.push(format!("Restarted {name} to clear old failure marks."));
        if id == AppId::Prowlarr {
            let _ = prowlarr_steps(app).await;
        } else {
            arr_tests(app, id).await;
        }
        check_health(app, &[id]).await;
    } else {
        log(app, Level::Red, format!("{name} didn't come back after the restart."));
    }
}

/// Before starting anything: a qBittorrent whose Web UI is off can't be
/// reached by Sonarr or Radarr. Switch it on in its settings file while it
/// is closed (a running qBittorrent would overwrite the file on exit).
async fn wire_qbit_ini(app: &AppHandle, rep: &mut FixReport) {
    let Some(cfg) = app.state::<Core>().config(AppId::QBittorrent) else { return };
    let Some(exe) = cfg.exe.clone() else { return };
    let running = procs::process_running("qbittorrent.exe");
    if running && !cfg.base_url.is_empty() && procs::port_open(&cfg.base_url).await {
        return;
    }
    let Some(dir) = std::env::var_os("APPDATA").map(|p| std::path::PathBuf::from(p).join("qBittorrent")) else { return };
    let path = dir.join("qBittorrent.ini");
    let ini = std::fs::read_to_string(&path).unwrap_or_default();
    let port = cfg.base_url.rsplit(':').next().and_then(|p| p.parse().ok()).unwrap_or(8080);
    let new = crate::wire::enable_webui(&ini, port);
    if new == ini && !running {
        return;
    }
    // qBittorrent rewrites its settings file when it exits, so an open one is
    // closed first, changed, then started again. Its torrents pick up again.
    if running {
        busy(app, AppId::QBittorrent, "Turning on its Web UI...");
        tokio::task::spawn_blocking(|| procs::force_close("qbittorrent.exe")).await.ok();
    }
    let ini = std::fs::read_to_string(&path).unwrap_or(ini);
    let new = crate::wire::enable_webui(&ini, port);
    if std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, new)).is_ok() {
        let msg = format!("Turned on qBittorrent's Web UI (port {port}) so Sonarr, Radarr and Chillarr can reach it.");
        log(app, Level::Ok, &msg);
        rep.actions.push(msg);
    }
    if running {
        let since = qbit_log_now();
        let _ = procs::start_qbit(&exe);
        if !procs::wait_until(&cfg.base_url, WAIT_QBIT_SECS).await {
            if let Some(why) = qbit_webui_failure(&since) {
                log(app, Level::Red, format!("qBittorrent's Web UI didn't start. qBittorrent says: {why}"));
            }
        }
    }
}

/// Connects the apps to each other now (used before adding indexers).
pub(crate) async fn wire_now(app: &AppHandle) {
    let mut rep = FixReport::default();
    wire_links(app, &mut rep).await;
}

/// After starting: add whatever link between the apps is missing.
async fn wire_links(app: &AppHandle, rep: &mut FixReport) {
    use crate::wire::{fill_fields, has_impl, pick_schema};
    use serde_json::json;
    let core = app.state::<Core>();
    let qbit = core.config(AppId::QBittorrent);
    let qbit_port: u16 = qbit
        .as_ref()
        .and_then(|c| c.base_url.rsplit(':').next()?.parse().ok())
        .unwrap_or(8080);
    let mut said = |msg: String| {
        log(app, Level::Ok, &msg);
        rep.actions.push(msg);
    };

    // qBittorrent as the download client in Sonarr and Radarr. Sonarr and Radarr
    // test the connection when it's added, so it has to be answering first:
    // on a fresh install it can still be starting (or closed, when adding an
    // indexer calls this), and the add would fail with "Unable to connect".
    let qbit_up = match qbit.as_ref().filter(|c| crate::discover::is_installed(c)) {
        Some(c) => {
            procs::port_open(&c.base_url).await
                || (procs::process_running("qbittorrent.exe") && procs::wait_until(&c.base_url, WAIT_QBIT_SECS).await)
        }
        None => false,
    };
    if !qbit_up && qbit.as_ref().is_some_and(crate::discover::is_installed) {
        log(app, Level::Amber, "qBittorrent isn't answering yet, so it isn't connected to Sonarr and Radarr. Open qBittorrent, then press Fix everything.");
    }
    if qbit_up {
        for (id, field, category) in [(AppId::Sonarr, "tvCategory", "tv-sonarr"), (AppId::Radarr, "movieCategory", "radarr")] {
            let Some(arr) = core.config(id).as_ref().and_then(Arr::new) else { continue };
            let Ok(clients) = arr.list("downloadclient").await else { continue };
            if has_impl(&clients, "QBittorrent") {
                continue;
            }
            let Some(schema) = arr.list("downloadclient/schema").await.ok().and_then(|s| pick_schema(&s, "QBittorrent")) else { continue };
            let mut item = fill_fields(schema, &[("host", json!("localhost")), ("port", json!(qbit_port)), (field, json!(category))]);
            item["name"] = json!("qBittorrent");
            item["enable"] = json!(true);
            match arr.create("downloadclient", &item).await {
                Ok(_) => said(format!("Connected qBittorrent to {}.", id.label())),
                Err(e) => log(app, Level::Red, format!("Couldn't connect qBittorrent to {}: {}.", id.label(), describe(&e))),
            }
        }
    }

    let Some(pcfg) = core.config(AppId::Prowlarr) else { return };
    let Some(prowlarr) = Arr::new(&pcfg) else { return };

    // Sonarr and Radarr as apps in Prowlarr, so it can hand them its indexers.
    if let Ok(apps) = prowlarr.list("applications").await {
        for id in [AppId::Sonarr, AppId::Radarr] {
            let name = id.label();
            let Some(cfg) = core.config(id) else { continue };
            let Some(key) = cfg.api_key.clone() else { continue };
            if has_impl(&apps, name) {
                continue;
            }
            let Some(schema) = prowlarr.list("applications/schema").await.ok().and_then(|s| pick_schema(&s, name)) else { continue };
            let mut item = fill_fields(
                schema,
                &[("prowlarrUrl", json!(pcfg.base_url)), ("baseUrl", json!(cfg.base_url)), ("apiKey", json!(key))],
            );
            item["name"] = json!(name);
            item["syncLevel"] = json!("fullSync");
            match prowlarr.create("applications", &item).await {
                Ok(_) => said(format!("Connected {name} to Prowlarr.")),
                Err(e) => log(app, Level::Red, format!("Couldn't connect {name} to Prowlarr: {}.", describe(&e))),
            }
        }
    }

    // FlareSolverr as Prowlarr's proxy, for indexers behind Cloudflare.
    let flare_installed = core.config(AppId::FlareSolverr).as_ref().is_some_and(crate::discover::is_installed);
    if let (true, Ok(proxies)) = (flare_installed, prowlarr.list("indexerproxy").await) {
        if !has_impl(&proxies, "FlareSolverr") {
            let tags = prowlarr.list("tag").await.unwrap_or_default();
            let tag_id = match tags.iter().find(|t| t["label"] == "flaresolverr").and_then(|t| t["id"].as_i64()) {
                Some(id) => Some(id),
                None => prowlarr.create("tag", &json!({ "label": "flaresolverr" })).await.ok().and_then(|t| t["id"].as_i64()),
            };
            if let (Some(tag_id), Some(schema)) =
                (tag_id, prowlarr.list("indexerproxy/schema").await.ok().and_then(|s| pick_schema(&s, "FlareSolverr")))
            {
                let mut item = fill_fields(schema, &[("host", json!("http://localhost:8191/"))]);
                item["name"] = json!("FlareSolverr");
                item["tags"] = json!([tag_id]);
                match prowlarr.create("indexerproxy", &item).await {
                    Ok(_) => said("Set up FlareSolverr in Prowlarr. Give indexers behind Cloudflare (like 1337x) the \"flaresolverr\" tag in Prowlarr.".into()),
                    Err(e) => log(app, Level::Red, format!("Couldn't set up FlareSolverr in Prowlarr: {}.", describe(&e))),
                }
            }
        }
    }
}

/// Button only: with Defender's protected-folder rule on, Radarr, Sonarr,
/// qBittorrent and Jellyfin get blocked writing into Desktop/Videos. Chillarr
/// asks first (the window shows a themed question, `defender-ask`), and only
/// on yes adds exactly those programs to its allow list, with one UAC prompt
/// (`defender_answer`). A no is remembered until Settings > General turns it back on.
async fn defender_step(app: &AppHandle) {
    let ask = app.state::<crate::SettingsState>().current.lock().unwrap().defender_ask;
    if !ask {
        return;
    }
    let on = tokio::task::spawn_blocking(crate::library::controlled_folder_access_on).await.unwrap_or(false);
    if !on {
        return;
    }
    let plan = defender_plan(app);
    if plan.is_empty() {
        return;
    }
    let names: Vec<&str> = plan.iter().map(|(n, _)| n.as_str()).collect();
    let _ = app.emit("defender-ask", &names);
}

/// Program paths already logged as left out this session, so each is said once.
static DEFENDER_SKIPPED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// `(name, program)` still missing from Defender's allow list. Only programs
/// Chillarr can vouch for: each app's own program (by name, from discovery or
/// a checked Settings pick), in a folder only an admin can change.
fn defender_plan(app: &AppHandle) -> Vec<(String, String)> {
    let core = app.state::<Core>();
    let already = app.state::<crate::SettingsState>().current.lock().unwrap().defender_allowed.clone();
    let missing = |p: &str| !already.iter().any(|a| a.eq_ignore_ascii_case(p));
    let mut out = Vec::new();
    for id in [AppId::Radarr, AppId::Sonarr, AppId::QBittorrent, AppId::Jellyfin] {
        let Some(exe) = core.config(id).and_then(|c| c.exe) else { continue };
        let path = exe.to_string_lossy().to_string();
        if !missing(&path) {
            continue;
        }
        let why = if !crate::settings::exe_ok(id, &exe) {
            Some(format!("it isn't {}'s own program", id.label()))
        } else if crate::library::user_can_change(&exe) {
            Some("its folder can be changed without admin, so another program could take its place".to_string())
        } else {
            None
        };
        if let Some(why) = why {
            let mut said = DEFENDER_SKIPPED.lock().unwrap();
            if !said.contains(&path) {
                said.push(path.clone());
                log(app, Level::Amber, format!("Didn't add {} ({path}) to Defender's allow list: {why}.", id.label()));
            }
            continue;
        }
        out.push((id.label().to_string(), path));
    }
    // Chillarr itself saves subtitles beside the videos, so it needs the
    // exception too. It stays in even from a folder the user can write (a
    // per-user install), or subtitles for protected folders would fail.
    if let Ok(me) = std::env::current_exe() {
        let path = me.to_string_lossy().to_string();
        if missing(&path) {
            out.push(("Chillarr".to_string(), path));
        }
    }
    out
}

/// The window's answer to `defender-ask`. Yes: one UAC prompt adds the apps.
/// No: Fix everything stops asking until Settings > General turns it back on.
#[tauri::command]
pub async fn defender_answer(app: AppHandle, yes: bool) -> Result<(), String> {
    let st = app.state::<crate::SettingsState>();
    if !yes {
        let saved = {
            let mut s = st.current.lock().unwrap();
            s.defender_ask = false;
            s.clone()
        };
        let _ = crate::settings::save(&st.path, &saved);
        log(&app, Level::Amber, "Left Defender as it is. To allow the apps later, turn on Defender access in Settings > General.");
        return Ok(());
    }
    let plan = defender_plan(&app);
    if plan.is_empty() {
        return Ok(());
    }
    let paths: Vec<String> = plan.into_iter().map(|(_, p)| p).collect();
    let script = crate::library::allow_command(&paths);
    let done = tokio::task::spawn_blocking(move || procs::run_elevated(&script)).await.unwrap_or_else(|e| Err(e.to_string()));
    match done {
        Ok(()) => {
            let saved = {
                let mut s = st.current.lock().unwrap();
                s.defender_allowed.extend(paths.iter().cloned());
                s.clone()
            };
            let _ = crate::settings::save(&st.path, &saved);
            log(&app, Level::Ok, "Let your media apps past Defender's protected-folder rule, so imports aren't blocked.");
            Ok(())
        }
        Err(e) => {
            let msg = e.replace("{what}", "Adding your media apps to Defender's allow list");
            log(&app, Level::Red, &msg);
            Err(msg)
        }
    }
}

/// Button only: fills an empty Allowed Hosts (the "Allowed Hosts is not
/// configured" warning) with this PC's own names, then restarts the app so it
/// applies. Never touches a value the user already set.
async fn allowed_hosts_step(app: &AppHandle, rep: &mut FixReport) {
    let computer = std::env::var("COMPUTERNAME").unwrap_or_default();
    let value = crate::arr::allowed_hosts_value(&computer, crate::arr::lan_ip().as_deref());
    for id in [AppId::Prowlarr, AppId::Sonarr, AppId::Radarr] {
        let Some(cfg) = app.state::<Core>().config(id) else { continue };
        let Some(arr) = Arr::new(&cfg) else { continue };
        let Ok(health) = arr.health().await else { continue };
        if !health.iter().any(|h| h.source == "AllowedHostsCheck") {
            continue;
        }
        let name = id.label();
        match arr.set_allowed_hosts_if_empty(&value).await {
            Ok(true) => {
                busy(app, id, "Restarting...");
                if arr.restart().await.is_ok() {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    procs::wait_until(&cfg.base_url, WAIT_ARR_SECS).await;
                    for _ in 0..30 {
                        if arr.status().await.is_ok() {
                            break;
                        }
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
                let msg = format!("Set {name}'s Allowed Hosts to {value}.");
                log(app, Level::Ok, format!("{msg} To open {name} from another device, add its address in {name} > Settings > General."));
                rep.actions.push(msg);
            }
            Ok(false) => {}
            Err(e) => log(app, Level::Red, format!("Couldn't set {name}'s Allowed Hosts: {}.", describe(&e))),
        }
    }
}

/// Button only: Jellyfin installed as a Windows service runs as NETWORK
/// SERVICE, which can't open files that came from the user's Downloads (a
/// hardlink keeps the original file's permissions), so nothing plays. Run it
/// as the user instead, with the same data folder, in one UAC prompt.
async fn jellyfin_as_user_step(app: &AppHandle, rep: &mut FixReport) {
    let core = app.state::<Core>();
    let Some(cfg) = core.config(AppId::Jellyfin) else { return };
    let (Some(svc), Some(exe)) = (cfg.service.clone(), cfg.exe.clone()) else { return };
    let locked = crate::imports::service_account(&svc).and_then(|a| crate::imports::service_sid(&a)).is_some();
    if !locked {
        return;
    }
    let data = std::env::var_os("ProgramData").map(std::path::PathBuf::from).unwrap_or_default().join("Jellyfin").join("Server");
    let account = format!("{}\\{}", std::env::var("USERDOMAIN").unwrap_or_default(), std::env::var("USERNAME").unwrap_or_default());
    busy(app, AppId::Jellyfin, "Switching to run as you...");
    let data = data.to_string_lossy().to_string();
    // Grant first: if the service is already stopped, `net stop` fails and would skip it.
    let params = crate::imports::jellyfin_as_user_command(&svc, &data, &account);
    let done = tokio::task::spawn_blocking(move || procs::run_elevated(&params)).await.unwrap_or_else(|e| Err(e.to_string()));
    // What counts is the end state: stopped and Manual (net stop fails on an already stopped service).
    let stopped = procs::service_running(&svc) == Some(false) && crate::winsvc::start_type(&svc) == Some(3);
    if !stopped {
        let e = done.err().unwrap_or_else(|| "{what} didn't stop the service".into());
        log(app, Level::Red, e.replace("{what}", "Switching Jellyfin to run as you"));
        return;
    }
    if let Some(c) = core.configs.write().unwrap().iter_mut().find(|c| c.id == AppId::Jellyfin) {
        c.service = None;
    }
    let webdir = crate::jfweb::prepare(app, &exe).await.map(|(d, _)| d);
    let started = procs::start_jellyfin(&exe, webdir.as_deref()).is_ok() && procs::wait_until(&cfg.base_url, WAIT_ARR_SECS).await;
    let answering = started && crate::jellyfin::Jf::new(&cfg.base_url, "chillarr-check", None).public_info().await.is_ok();
    if !answering {
        // The service comes back, and the user's write access to its data
        // folder goes: a service loading a folder the user can change would
        // run whatever the user's programs put there.
        let params = crate::imports::revert_jellyfin_command(&svc, &data, &account);
        let _ = tokio::task::spawn_blocking(move || procs::run_elevated(&params)).await;
        if let Some(c) = core.configs.write().unwrap().iter_mut().find(|c| c.id == AppId::Jellyfin) {
            c.service = Some(svc.clone());
        }
        log(app, Level::Red, "Jellyfin didn't start as you, so Chillarr put its Windows service back.");
        return;
    }
    let msg = "Jellyfin now runs as you, so it can play everything in your library. Chillarr starts it when you sign in.".to_string();
    log(app, Level::Ok, &msg);
    rep.actions.push(msg);
}

/// Button only: Radarr or Sonarr installed as a Windows service runs as LOCAL
/// SERVICE, which can't open the user's folders, so finished downloads never
/// reach the library. Switch it to run as the signed-in user (one UAC
/// prompt), like a normal Sonarr install, then import what was stuck.
async fn run_as_user_step(app: &AppHandle, rep: &mut FixReport) {
    for (id, scan) in [(AppId::Radarr, "DownloadedMoviesScan"), (AppId::Sonarr, "DownloadedEpisodesScan")] {
        let core = app.state::<Core>();
        let Some(cfg) = core.config(id) else { continue };
        let (Some(svc), Some(exe)) = (cfg.service.clone(), cfg.exe.clone()) else { continue };
        let locked = crate::imports::service_account(&svc).and_then(|a| crate::imports::service_sid(&a)).is_some();
        if !locked {
            continue;
        }
        let name = id.label();
        busy(app, id, "Switching to run as you...");
        let save = crate::discover::qbit_save_path();
        let params = crate::imports::run_as_user_command(&svc);
        let done = tokio::task::spawn_blocking(move || procs::run_elevated(&params))
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
        let stopped = procs::service_running(&svc) == Some(false) && crate::winsvc::start_type(&svc) == Some(3);
        if let Err(e) = done.and_then(|_| if stopped { Ok(()) } else { Err("{what} didn't stop the service".into()) }) {
            log(app, Level::Red, e.replace("{what}", &format!("Switching {name} to run as you")));
            continue;
        }
        // The service no longer counts as one; Chillarr starts the app from now on.
        if let Some(c) = core.configs.write().unwrap().iter_mut().find(|c| c.id == id) {
            c.service = None;
        }
        if let Err(e) = procs::start_arr(&exe, name) {
            log(app, Level::Red, e);
            continue;
        }
        procs::wait_until(&cfg.base_url, WAIT_ARR_SECS).await;
        let Some(arr) = Arr::new(&cfg) else { continue };
        let mut up = false;
        for _ in 0..60 {
            if arr.status().await.is_ok() {
                up = true;
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        if !up {
            // Put the service back so the app isn't left down after a reboot.
            let params = crate::imports::revert_command(&svc);
            let _ = tokio::task::spawn_blocking(move || procs::run_elevated(&params)).await;
            if let Some(c) = core.configs.write().unwrap().iter_mut().find(|c| c.id == id) {
                c.service = Some(svc.clone());
            }
            log(app, Level::Red, format!("{name} didn't start as you, so Chillarr put its Windows service back."));
            continue;
        }
        let msg = format!("{name} now runs as you, so it can move finished downloads into your library. Chillarr starts it when you sign in.");
        log(app, Level::Ok, &msg);
        rep.actions.push(msg);
        check_health(app, &[id]).await;

        // Import what finished while it was locked out.
        let Some(folder) = save.clone() else { continue };
        let grabbed = arr.grabbed_titles().await.unwrap_or_default();
        let candidates = crate::imports::download_candidates(std::path::Path::new(&folder));
        let names: Vec<String> = candidates.iter().filter_map(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).collect();
        let wanted = crate::imports::grabbed_folders(&grabbed, &names);
        let mut asked = 0;
        for path in candidates.iter().filter(|p| p.file_name().is_some_and(|n| wanted.iter().any(|w| w.as_str() == n.to_string_lossy()))) {
            if arr.import_from(scan, &path.to_string_lossy()).await.is_ok() {
                asked += 1;
            }
        }
        if asked > 0 {
            let msg = format!("Asked {name} to import {asked} finished download(s) that were stuck in {folder}.");
            log(app, Level::Ok, &msg);
            rep.actions.push(msg);
        }
    }
}

/// Button only: finished downloads that never made it into the library (for
/// example from before Radarr could read the Downloads folder) are imported
/// now, as hardlinks. Anything still downloading is left alone.
pub(crate) async fn import_finished_step(app: &AppHandle, rep: &mut FixReport) {
    let Some(a) = app.state::<Core>().config(AppId::Radarr).as_ref().and_then(Arr::new) else { return };
    let Some(folder) = crate::discover::qbit_save_path() else { return };
    let movies = a.list("movie").await.unwrap_or_default();
    let missing: Vec<i64> = movies.iter().filter(|m| m["hasFile"] != true).filter_map(|m| m["id"].as_i64()).collect();
    if missing.is_empty() {
        return;
    }
    let queue = a.get_one("queue?pageSize=200").await.unwrap_or_default();
    let downloading: Vec<i64> = queue["records"].as_array().map(|r| r.iter().filter_map(|x| x["movieId"].as_i64()).collect()).unwrap_or_default();
    let enc: String = folder.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
    let candidates = a.list(&format!("manualimport?folder={enc}&filterExistingFiles=true")).await.unwrap_or_default();
    let picked = crate::library::importable(&candidates, &missing, &downloading);
    if picked.is_empty() {
        return;
    }
    let files: Vec<serde_json::Value> = picked
        .iter()
        .map(|c| serde_json::json!({ "path": c["path"], "movieId": c["movie"]["id"], "quality": c["quality"], "languages": c["languages"], "releaseGroup": c["releaseGroup"] }))
        .collect();
    let names: Vec<String> = picked.iter().filter_map(|c| c["movie"]["title"].as_str().map(String::from)).collect();
    match a.command_with(serde_json::json!({ "name": "ManualImport", "files": files, "importMode": "copy" })).await {
        Ok(_) => {
            let msg = format!("Brought {} finished download(s) into your library: {}.", names.len(), names.join(", "));
            log(app, Level::Ok, &msg);
            rep.actions.push(msg);
        }
        Err(e) => log(app, Level::Red, format!("Couldn't import finished downloads: {}.", describe(&e))),
    }
}

async fn indexer_steps(app: &AppHandle, trigger: Trigger, rep: &mut FixReport) {
    let (passed, proxies_ok) = prowlarr_steps(app).await;
    for id in [AppId::Sonarr, AppId::Radarr] {
        arr_tests(app, id).await;
    }
    let all = [AppId::Prowlarr, AppId::Sonarr, AppId::Radarr];
    check_health(app, &all).await;
    if trigger == Trigger::Button {
        for id in all {
            restart_if_stale(app, id, &passed, proxies_ok, rep).await;
        }
    }
}

pub async fn fix_all(app: &AppHandle, trigger: Trigger) -> FixReport {
    let core = app.state::<Core>();
    let _guard = match core.fixer.try_begin() {
        Some(g) => g,
        None => {
            let running = *core.fixer.current.lock().unwrap();
            if should_join(running, trigger) {
                // The run in progress covers this request: wait and report its result.
                let _g = core.fixer.running.lock().await;
                return FixReport { cards: core.cards.lock().unwrap().clone(), ..Default::default() };
            }
            core.fixer.running.lock().await
        }
    };
    *core.fixer.current.lock().unwrap() = Some(trigger);
    if needs_rediscovery(&core.configs.read().unwrap()) {
        rediscover_into(app).await;
    }
    let _ = app.emit("fix-state", true);
    if trigger == Trigger::Button {
        log(app, Level::Ok, "Fix everything started.");
    }
    let mut rep = FixReport::default();
    if trigger == Trigger::Button {
        wire_qbit_ini(app, &mut rep).await;
        if first_starts(app).await {
            rep.actions.push("Started apps that had never run, so they set themselves up.".into());
        }
    } else if !procs::process_running("qbittorrent.exe") {
        // Closed, so nothing has to be interrupted: switch its Web UI on now.
        wire_qbit_ini(app, &mut rep).await;
    }
    let configs = core.configs.read().unwrap().clone();
    for cfg in configs.iter().filter(|c| c.managed) {
        start_if_down(app, cfg, trigger, &mut rep).await;
    }
    // Every trigger: an *arr app that skips its login for the home network asks for it everywhere.
    crate::login::require_login_everywhere(app).await;
    crate::pick::keep_deleted_gone(app).await;
    crate::pick::no_disc_copies(app).await;
    crate::pick::no_dubs(app).await;
    crate::media::flare_for_blocked(app).await;
    if trigger == Trigger::Button {
        wire_links(app, &mut rep).await;
        // The one login, for apps that weren't running when it was picked.
        crate::login::apply_pending(app).await;
        // Skip intro needs Intro Skipper in Jellyfin.
        if let Some(msg) = crate::skip::ensure_plugin(app).await {
            rep.actions.push(msg);
        }
        // Folders picked in setup before Radarr/Sonarr were ready.
        for msg in crate::media::apply_pending_folders(app).await {
            rep.actions.push(msg);
        }
    }
    if trigger == Trigger::Button {
        defender_step(app).await;
        allowed_hosts_step(app, &mut rep).await;
        run_as_user_step(app, &mut rep).await;
        jellyfin_as_user_step(app, &mut rep).await;
        import_finished_step(app, &mut rep).await;
        // Downloads nothing will bring into the library: say so, and where to stop them.
        let orphans = crate::waiting::orphans(app).await;
        if !orphans.is_empty() {
            let msg = format!(
                "{} download(s) in qBittorrent aren't tied to Radarr or Sonarr, so they won't reach your library: {}. Stop them under My downloads.",
                orphans.len(),
                orphans.iter().take(3).cloned().collect::<Vec<_>>().join(", ")
            );
            log(app, Level::Amber, &msg);
            rep.needs_button.push(msg);
        }
    }
    // Only test and re-sync when something is actually wrong: a press while
    // everything is green changes nothing.
    let now = Instant::now();
    let before = refresh_cards(app).await;
    let broken = anything_to_fix(&before);
    if broken && core.fixer.indexer_tests_allowed(trigger, now) {
        core.fixer.mark_indexer_test(now);
        indexer_steps(app, trigger, &mut rep).await;
    }
    rep.cards = refresh_cards(app).await;
    indexer_table_now(app).await;
    let bad = rep.cards.iter().filter(|c| matches!(c.level, Level::Red | Level::Amber)).count();
    if trigger == Trigger::Button {
        log(app, if bad == 0 { Level::Ok } else { Level::Amber }, if bad == 0 && !broken && rep.actions.is_empty() {
            "Everything already works. Nothing to fix.".to_string()
        } else if bad == 0 {
            "Done. Everything works.".to_string()
        } else {
            format!("Done. {bad} of {} still need attention.", rep.cards.len())
        });
    }
    let _ = app.emit("fix-state", false);
    *core.fixer.current.lock().unwrap() = None;
    rep
}

/// Re-reads every app's config and applies the user's overrides.
pub async fn rediscover_into(app: &AppHandle) {
    let found = tauri::async_runtime::spawn_blocking(crate::discover::discover_all).await.unwrap_or_default();
    let s = app.state::<crate::SettingsState>().current.lock().unwrap().clone();
    *app.state::<Core>().configs.write().unwrap() = crate::settings::apply(found, &s);
}

/// A fresh Sonarr/Radarr/Prowlarr writes its API key into config.xml the first
/// time it runs. One that was installed but never started (no service, or the
/// installer didn't launch it) has no key, so nothing can talk to it. Start
/// those once and wait for the key. True when any was started.
pub(crate) async fn first_starts(app: &AppHandle) -> bool {
    let core = app.state::<Core>();
    let waiting: Vec<AppConfig> = core
        .configs
        .read()
        .unwrap()
        .iter()
        .filter(|c| matches!(c.id, AppId::Sonarr | AppId::Radarr | AppId::Prowlarr))
        .filter(|c| c.problem.as_deref().is_some_and(|p| !p.starts_with("Not installed")) && c.exe.is_some())
        .cloned()
        .collect();
    let mut started = false;
    for cfg in &waiting {
        let name = cfg.id.label();
        if procs::process_running(&format!("{name}.exe")) {
            continue;
        }
        busy(app, cfg.id, "Starting for the first time...");
        match procs::start_arr(cfg.exe.as_ref().unwrap(), name) {
            Ok(()) => {
                started = true;
                log(app, Level::Ok, format!("Started {name} for the first time so it sets itself up."));
            }
            Err(e) => log(app, Level::Red, e),
        }
    }
    if !started {
        return false;
    }
    // A first start takes a while: it creates its database, then its key.
    for _ in 0..45 {
        tokio::time::sleep(Duration::from_secs(2)).await;
        rediscover_into(app).await;
        if waiting.iter().all(|w| core.config(w.id).is_some_and(|c| c.problem.is_none())) {
            break;
        }
    }
    true
}

/// Gets one app answering: a first start for a fresh install, or a normal
/// start when it's just not running. True when it answers.
pub(crate) async fn ensure_running(app: &AppHandle, id: AppId) -> bool {
    rediscover_into(app).await;
    first_starts(app).await;
    let Some(cfg) = app.state::<Core>().config(id) else { return false };
    if cfg.problem.is_some() || cfg.base_url.is_empty() {
        return false;
    }
    if !procs::port_open(&cfg.base_url).await {
        let mut rep = FixReport::default();
        start_if_down(app, &cfg, Trigger::Button, &mut rep).await;
    }
    procs::port_open(&cfg.base_url).await
}

/// "Install all": every missing app downloads and installs at the same time,
/// then each is started once and everything is connected (Fix everything).
pub async fn install_apps(app: &AppHandle, ids: Vec<AppId>) -> Vec<(AppId, String)> {
    let mut failed = Vec::new();
    {
        let core = app.state::<Core>();
        let _guard = core.fixer.running.lock().await;
        let _ = app.emit("fix-state", true);
        let mut set = tokio::task::JoinSet::new();
        for id in ids {
            busy(app, id, "Installing...");
            log(app, Level::Ok, format!("Installing {}...", id.label()));
            set.spawn(async move { (id, crate::install::install(id).await) });
        }
        while let Some(done) = set.join_next().await {
            let Ok((id, result)) = done else { continue };
            match result {
                Ok(()) => log(app, Level::Ok, format!("Installed {}.", id.label())),
                Err(e) => {
                    log(app, Level::Red, format!("Couldn't install {}. {e}", id.label()));
                    failed.push((id, e));
                }
            }
            rediscover_into(app).await;
            refresh_cards(app).await;
        }
        rediscover_into(app).await;
        first_starts(app).await;
        refresh_cards(app).await;
        let _ = app.emit("fix-state", false);
    }
    // Now connect them: qBittorrent's Web UI, download clients, Prowlarr apps.
    fix_all(app, Trigger::Button).await;
    failed
}

/// Installs a missing app after the user said yes, then finds it again.
pub async fn install_app(app: &AppHandle, id: AppId) -> Result<(), String> {
    let core = app.state::<Core>();
    let _guard = core.fixer.running.lock().await;
    let _ = app.emit("fix-state", true);
    let name = id.label();
    busy(app, id, "Installing...");
    log(app, Level::Ok, format!("Installing {name}..."));
    let result = crate::install::install(id).await;
    match &result {
        Ok(()) => log(app, Level::Ok, format!("Installed {name}.")),
        Err(e) => log(app, Level::Red, format!("Couldn't install {name}. {e}")),
    }
    // A fresh Servarr install writes its config.xml once it first starts.
    for _ in 0..10 {
        rediscover_into(app).await;
        if core.config(id).map(|c| c.problem.is_none()).unwrap_or(true) {
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    // Still no key: it was never started. Start it once.
    first_starts(app).await;
    refresh_cards(app).await;
    let _ = app.emit("fix-state", false);
    drop(_guard);
    if result.is_ok() {
        // Connect it to the others right away.
        let h = app.clone();
        tauri::async_runtime::spawn(async move {
            fix_all(&h, Trigger::Button).await;
        });
    }
    result
}

/// The daily check: is a newer version of a winget-installed app out?
/// Returns the newly found ones, for a notification.
pub async fn check_updates(app: &AppHandle) -> Vec<(AppId, String)> {
    let core = app.state::<Core>();
    let mut found = Vec::new();
    for id in [AppId::QBittorrent] {
        if !core.config(id).as_ref().is_some_and(crate::discover::is_installed) {
            continue;
        }
        if let Some(v) = crate::install::update_available(id).await {
            let new = core.updates.lock().unwrap().insert(id, v.clone()).as_deref() != Some(v.as_str());
            if new {
                found.push((id, v));
            }
        }
    }
    refresh_cards(app).await;
    found
}

/// Updates an app after the user said yes. qBittorrent is closed for the
/// update and started again; its torrents resume.
pub async fn update_app(app: &AppHandle, id: AppId) -> Result<(), String> {
    let core = app.state::<Core>();
    let _guard = core.fixer.running.lock().await;
    let _ = app.emit("fix-state", true);
    let name = id.label();
    busy(app, id, "Updating...");
    log(app, Level::Ok, format!("Updating {name}..."));
    if id == AppId::QBittorrent {
        tokio::task::spawn_blocking(|| procs::force_close("qbittorrent.exe")).await.ok();
    }
    let result = crate::install::upgrade(id).await;
    match &result {
        Ok(()) => {
            core.updates.lock().unwrap().remove(&id);
            log(app, Level::Ok, format!("Updated {name}."));
        }
        Err(e) => log(app, Level::Red, format!("Couldn't update {name}. {e}")),
    }
    if let Some(exe) = core.config(id).and_then(|c| c.exe) {
        if id == AppId::QBittorrent {
            let _ = procs::start_qbit(&exe);
        }
    }
    refresh_cards(app).await;
    let _ = app.emit("fix-state", false);
    result
}

/// The Indexers page's "Test all": only the indexer steps.
pub async fn test_indexers(app: &AppHandle) -> Vec<IndexerRow> {
    let core = app.state::<Core>();
    {
        let _guard = core.fixer.running.lock().await;
        let _ = app.emit("fix-state", true);
        core.fixer.mark_indexer_test(Instant::now());
        let mut rep = FixReport::default();
        indexer_steps(app, Trigger::Button, &mut rep).await;
        refresh_cards(app).await;
        let _ = app.emit("fix-state", false);
    }
    indexer_table_now(app).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn second_fix_joins_first() {
        // Review Focus 5: never two pipelines at once.
        let f = Fixer::default();
        let g = f.try_begin();
        assert!(g.is_some());
        assert!(f.try_begin().is_none());
        drop(g);
        assert!(f.try_begin().is_some());
    }

    #[test]
    fn a_button_press_never_joins_a_guard_run() {
        // Review Focus 5 still holds (one pipeline at a time), but a press
        // during a guard run must run the full button pipeline afterwards.
        assert!(should_join(Some(Trigger::Button), Trigger::Button));
        assert!(should_join(Some(Trigger::Button), Trigger::Guard));
        assert!(should_join(Some(Trigger::Guard), Trigger::Guard));
        assert!(!should_join(Some(Trigger::Guard), Trigger::Button));
        assert!(!should_join(None, Trigger::Button), "an install holding the lock: wait, then run");
    }

    #[test]
    fn rediscover_when_any_app_has_a_problem() {
        let ok = AppConfig {
            id: AppId::Radarr,
            base_url: "http://localhost:7878".into(),
            api_key: Some("k".into()),
            exe: None,
            service: None,
            managed: true,
            problem: None,
        };
        let mut bad = ok.clone();
        bad.id = AppId::Sonarr;
        bad.problem = Some("Can't read its config".into());
        assert!(!needs_rediscovery(&[ok.clone()]));
        assert!(needs_rediscovery(&[ok, bad]));
    }

    #[test]
    fn nothing_is_retested_when_everything_works() {
        let card = |level: Level, fixable: bool| CardState {
            id: AppId::Radarr,
            level,
            line: String::new(),
            notes: vec![],
            open_url: None,
            fixable,
            missing: false,
            update: None,
        };
        assert!(!anything_to_fix(&[card(Level::Ok, false), card(Level::Ok, false)]));
        assert!(anything_to_fix(&[card(Level::Ok, false), card(Level::Amber, true)]));
        assert!(!anything_to_fix(&[card(Level::Amber, false)]), "a problem a fix can't touch isn't a reason to retest");
        assert!(!anything_to_fix(&[card(Level::Unknown, false)]), "not installed isn't broken");
    }

    #[test]
    fn guard_throttles_indexer_tests() {
        let f = Fixer::default();
        let now = Instant::now();
        assert!(f.indexer_tests_allowed(Trigger::Guard, now));
        f.mark_indexer_test(now);
        assert!(f.indexer_tests_allowed(Trigger::Button, now));
        assert!(!f.indexer_tests_allowed(Trigger::Guard, now + Duration::from_secs(14 * 60)));
        assert!(f.indexer_tests_allowed(Trigger::Guard, now + Duration::from_secs(15 * 60 + 1)));
    }

    #[test]
    fn stale_marks_are_indexers_prowlarr_passed() {
        let passed = vec!["1337x".to_string(), "YTS".to_string()];
        let health = vec![Health {
            source: "IndexerLongTermStatusCheck".into(),
            kind: "warning".into(),
            message: "Indexers unavailable due to failures for more than 6 hours: 1337x (Prowlarr), EZTV (Prowlarr)".into(),
        }];
        assert_eq!(stale_marks(&health, &passed, false), vec!["1337x".to_string()]);
    }

    #[test]
    fn stale_proxy_mark_after_passing_proxy_test() {
        // Live 2026-10-16: Prowlarr kept "All indexer proxies are unavailable"
        // after its FlareSolverr test passed.
        let health = vec![Health {
            source: "IndexerProxyStatusCheck".into(),
            kind: "error".into(),
            message: "All indexer proxies are unavailable due to failures".into(),
        }];
        assert_eq!(stale_marks(&health, &[], true), vec!["FlareSolverr".to_string()]);
        assert!(stale_marks(&health, &[], false).is_empty());
    }
}

#[cfg(test)]
mod feedless_tests {
    #[test]
    fn an_empty_movie_feed_is_not_a_failure() {
        assert!(super::feedless("Query successful, but no results in the configured categories were returned from your indexer. This may be an issue with the indexer or your indexer category settings."));
        assert!(!super::feedless("Unable to connect to indexer"));
        assert!(crate::media::behind_cloudflare("tokyotosho") && crate::media::behind_cloudflare("1337x") && !crate::media::behind_cloudflare("nyaasi"));
    }
}
