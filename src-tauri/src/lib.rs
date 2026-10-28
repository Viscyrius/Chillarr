mod arr;
mod bridge;
mod viewer;
mod seal;
mod catalog;
mod completed;
mod discover;
mod fix;
mod guard;
mod history;
mod imports;
mod install;
mod jellyfin;
mod jfweb;
mod library;
mod marker;
mod log;
mod login;
mod media;
mod meili;
mod search_tap;
mod procs;
mod qbit;
mod settings;
mod skip;
mod status;
mod storage;
pub mod model;
mod tiles;
mod vlc;
mod waiting;
mod search_schedule;
mod pick;
mod profiles;
mod subs;
mod subfetch;
mod subsync;
mod trickplay;
mod syncplay;
mod details;
mod similar;
mod remote;
mod search;
mod discover_shelf;
mod tray;
mod transcodes;
mod watchnow;
mod wire;
mod winsvc;

use fix::{Core, Trigger};
use log::{LogLine, Logs};
use model::{AppConfig, AppId};
use status::{CardState, IndexerRow};
use tauri::{AppHandle, Manager, State, WindowEvent};
use tauri_plugin_opener::OpenerExt;

/// Runs "Fix everything" without blocking the caller (tray menu).
pub fn spawn_fix(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        fix::fix_all(&app, Trigger::Button).await;
    });
}

#[tauri::command]
async fn fix_everything(app: AppHandle) -> Vec<CardState> {
    fix::fix_all(&app, Trigger::Button).await.cards
}

#[tauri::command]
async fn test_indexers(app: AppHandle) -> Vec<IndexerRow> {
    fix::test_indexers(&app).await
}

#[tauri::command]
async fn refresh(app: AppHandle) -> Vec<CardState> {
    fix::refresh_cards(&app).await
}

#[tauri::command]
async fn indexer_table(app: AppHandle) -> Vec<IndexerRow> {
    fix::indexer_table_now(&app).await
}

#[tauri::command]
fn open_app(app: AppHandle, core: State<'_, Core>, id: AppId) -> Result<(), String> {
    let cfg = core.config(id).ok_or("Unknown app")?;
    app.opener().open_url(cfg.base_url, None::<&str>).map_err(|e| e.to_string())
}

/// Settings and where they live on disk.
pub(crate) struct SettingsState {
    pub(crate) path: std::path::PathBuf,
    pub(crate) current: std::sync::Mutex<settings::Settings>,
}

fn settings_path(app: &AppHandle) -> std::path::PathBuf {
    app.path()
        .app_config_dir()
        .unwrap_or_else(|_| std::env::temp_dir().join("Chillarr"))
        .join("settings.json")
}

fn set_autostart(app: &AppHandle, on: bool) {
    use tauri_plugin_autostart::ManagerExt;
    let mgr = app.autolaunch();
    let _ = if on { mgr.enable() } else { mgr.disable() };
}

#[tauri::command]
async fn rediscover(app: AppHandle) -> Vec<AppConfig> {
    let found = tauri::async_runtime::spawn_blocking(discover::discover_all).await.unwrap_or_default();
    let s = app.state::<SettingsState>().current.lock().unwrap().clone();
    let found = settings::apply(found, &s);
    *app.state::<Core>().configs.write().unwrap() = found.clone();
    fix::refresh_cards(&app).await;
    found
}

#[tauri::command]
async fn install_app(app: AppHandle, id: AppId) -> Result<(), String> {
    fix::install_app(&app, id).await
}

#[tauri::command]
async fn update_app(app: AppHandle, id: AppId) -> Result<(), String> {
    fix::update_app(&app, id).await
}

#[tauri::command]
async fn media_status(app: AppHandle) -> media::MediaStatus {
    media::status(&app).await
}

#[tauri::command]
async fn media_connect(app: AppHandle, user: String, password: String) -> Result<(), String> {
    media::connect(&app, &user, &password).await
}

#[tauri::command]
fn media_sign_out(app: AppHandle) {
    media::sign_out(&app)
}

#[tauri::command]
async fn media_rows(app: AppHandle, tab: String) -> Result<Vec<media::Row>, String> {
    media::rows(&app, &tab).await
}

#[tauri::command]
async fn media_episodes(app: AppHandle, series_id: String) -> Vec<tiles::Tile> {
    media::episodes(&app, &series_id).await
}

/// The window itself full screen (the whole monitor, no taskbar). A video's
/// own full screen only fills the window in WebView2, so the player asks for
/// this too. `on` missing flips it. Returns whether it's full screen now.
#[tauri::command]
fn window_fullscreen(window: tauri::WebviewWindow, on: Option<bool>) -> bool {
    let want = on.unwrap_or(!window.is_fullscreen().unwrap_or(false));
    let _ = window.set_fullscreen(want);
    window.is_fullscreen().unwrap_or(want)
}

#[tauri::command]
async fn media_search(app: AppHandle, term: String) -> media::Found {
    media::search(&app, &term).await
}

/// Search that answers source by source with "search-update" events (search.rs).
#[tauri::command]
async fn media_search_live(app: AppHandle, term: String, seq: u64) {
    search::live(app, term, seq).await
}

#[tauri::command]
async fn media_play(app: AppHandle, id: String, start: f64, audio: Option<i64>, subtitle: Option<i64>) -> Result<serde_json::Value, String> {
    media::play(&app, &id, start, audio, subtitle).await
}

/// Watch together: where the SyncPlay socket connects (see syncplay.rs).
#[tauri::command]
fn syncplay_info(app: AppHandle) -> Result<syncplay::Info, String> {
    syncplay::info(&app)
}

#[tauri::command]
async fn syncplay_call(app: AppHandle, method: String, path: String, body: Option<serde_json::Value>) -> Result<serde_json::Value, String> {
    syncplay::call(&app, &method, &path, body).await
}

#[tauri::command]
async fn syncplay_tile(app: AppHandle, id: String) -> Option<tiles::Tile> {
    syncplay::tile(&app, &id).await
}

#[tauri::command]
fn profiles_list(app: AppHandle) -> Vec<profiles::View> {
    profiles::list(&app)
}

#[tauri::command]
async fn profiles_add(app: AppHandle, name: String, color: String, pin: Option<String>) -> Result<profiles::View, String> {
    profiles::add(&app, &name, &color, pin).await
}

#[tauri::command]
fn profiles_switch(app: AppHandle, id: String, pin: Option<String>) -> Result<(), String> {
    profiles::switch(&app, &id, pin)
}

/// Settings > Profiles: browsers that typed the main profile's password ask again.
#[tauri::command]
fn profiles_forget_web_devices(app: AppHandle) -> Result<(), String> {
    profiles::forget_web_devices(&app)
}

#[tauri::command]
fn profiles_check_pin(app: AppHandle, id: String, pin: String) -> Result<(), String> {
    profiles::verify_pin(&app, &id, &pin)
}

#[tauri::command]
async fn profiles_update(app: AppHandle, id: String, name: Option<String>, color: Option<String>, pin: Option<String>, current_pin: Option<String>) -> Result<(), String> {
    profiles::update(&app, &id, name, color, pin, current_pin).await
}

#[tauri::command]
async fn profiles_set_password(app: AppHandle, id: String, password: String, current_pin: Option<String>) -> Result<bool, String> {
    profiles::set_password(&app, &id, &password, current_pin).await
}

#[tauri::command]
async fn profiles_remove(app: AppHandle, id: String, pin: Option<String>, login: Option<String>) -> Result<(), String> {
    profiles::remove(&app, &id, pin, login).await
}

/// Drives, the biggest titles, and the clean-up choice (Settings > Library & storage).
/// Smarter search in Jellyfin (meili.rs).
#[tauri::command]
async fn meili_status(app: AppHandle) -> meili::Status {
    meili::status(&app).await
}

#[tauri::command]
async fn meili_set(app: AppHandle, on: bool) -> Result<String, String> {
    if on { meili::enable(&app).await } else { meili::disable(&app).await }
}

#[tauri::command]
async fn storage_overview(app: AppHandle) -> storage::Overview {
    storage::overview(&app).await
}

/// Clean-up after this many days since watching; None turns it off.
#[tauri::command]
fn storage_set_cleanup(app: AppHandle, days: Option<u32>) -> Result<(), String> {
    storage::set_cleanup(&app, days)
}

/// Intro, credits and recap times for one episode (Intro Skipper's media segments).
#[tauri::command]
async fn media_segments(app: AppHandle, id: String) -> Vec<skip::Segment> {
    skip::segments(&app, &id).await
}

/// One login for qBittorrent, Sonarr, Radarr and Prowlarr (see login.rs).
#[tauri::command]
async fn apps_set_login(app: AppHandle, user: String, password: String) -> Result<Vec<login::Outcome>, String> {
    let user = user.trim().to_string();
    if user.is_empty() || password.len() < 5 {
        return Err("Pick a name and a password of at least 5 characters.".into());
    }
    Ok(login::apply(&app, &user, &password).await)
}

#[tauri::command]
async fn media_my_list(app: AppHandle, tile: tiles::Tile, on: bool) -> Result<bool, String> {
    media::toggle_my_list(&app, tile, on).await
}

/// Every episode of a show, what's on this PC, and where you're at.
#[tauri::command]
async fn media_show_episodes(app: AppHandle, tile: tiles::Tile) -> Result<pick::ShowEpisodes, String> {
    pick::show_episodes(&app, &tile).await
}

/// "Pick a different episode": where you're at in a show, set on purpose.
#[tauri::command]
fn marker_set(app: AppHandle, tvdb: i64, season: i64, episode: i64) -> Result<(), String> {
    marker::set(&app, tvdb, marker::Spot { season, episode })
}

/// Start the show (no season) or one season over, for this profile.
#[tauri::command]
async fn marker_restart(app: AppHandle, tvdb: i64, season: Option<i64>) -> Result<String, String> {
    pick::restart(&app, tvdb, season).await
}

/// My List > Completed for this profile, asked for once My List is on screen.
#[tauri::command]
async fn media_completed(app: AppHandle) -> Vec<tiles::Tile> {
    completed::tiles(&app).await
}

/// What the watching profile has finished ("tmdb:<id>", "tvdb:<id>", "anilist:<id>"), for the Watched mark.
#[tauri::command]
fn media_watched_keys(app: AppHandle) -> Vec<String> {
    completed::watched_keys(&history::active(&app))
}

/// Takes a title ("tmdb:<id>" / "tvdb:<id>") off My List > Completed for this profile.
#[tauri::command]
fn completed_remove(app: AppHandle, key: String) -> Result<(), String> {
    completed::remove(&app, &key)
}

/// `(keys, jellyfin ids)` on My List.
#[tauri::command]
async fn media_my_list_marks(app: AppHandle) -> (Vec<String>, Vec<String>) {
    media::my_list_marks(&app).await
}

#[tauri::command]
fn shelf_status(app: AppHandle) -> discover_shelf::Status {
    discover_shelf::status(&app)
}

#[tauri::command]
async fn shelf_set(app: AppHandle, on: bool, series_mode: Option<String>) -> discover_shelf::Status {
    discover_shelf::set(&app, on, series_mode).await
}

#[tauri::command]
async fn shelf_brand(app: AppHandle, on: bool) -> discover_shelf::Status {
    discover_shelf::set_brand(&app, on).await
}

#[tauri::command]
fn remote_status(app: AppHandle) -> remote::Status {
    app.state::<remote::Remote>().status()
}

#[tauri::command]
fn remote_set(app: AppHandle, on: bool, new_code: bool) -> remote::Status {
    remote::set(&app, on, new_code)
}

#[tauri::command]
fn remote_forget(app: AppHandle, id: String) -> remote::Status {
    remote::forget(&app, &id)
}

#[tauri::command]
async fn media_filter(app: AppHandle, filter: catalog::Filter, page: usize) -> Vec<tiles::Tile> {
    media::filter(&app, filter, page).await
}

#[tauri::command]
async fn media_anime_tags() -> Result<Vec<(String, String)>, String> {
    media::anime_tags().await
}

#[tauri::command]
fn media_filter_genres(app: AppHandle, kind: String) -> Vec<String> {
    media::filter_genres(&app, &kind)
}

/// Turns off WebView2's autofill and password saving for the window.
#[cfg(windows)]
fn no_autofill(w: &tauri::WebviewWindow) {
    let _ = w.with_webview(|wv| unsafe {
        use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Settings4;
        use windows_core::Interface;
        let Ok(core) = wv.controller().CoreWebView2() else { return };
        let Ok(settings) = core.Settings() else { return };
        if let Ok(s) = settings.cast::<ICoreWebView2Settings4>() {
            let _ = s.SetIsGeneralAutofillEnabled(false);
            let _ = s.SetIsPasswordAutosaveEnabled(false);
        }
    });
}

#[tauri::command]
fn media_filter_studios(app: AppHandle, kind: String) -> Vec<String> {
    media::filter_studios(&app, &kind)
}

#[tauri::command]
async fn media_details(app: AppHandle, tile: tiles::Tile) -> details::Details {
    details::details(&app, &tile).await
}

#[tauri::command]
async fn media_similar(app: AppHandle, tile: tiles::Tile) -> Vec<tiles::Tile> {
    similar::similar(&app, &tile).await
}

#[tauri::command]
async fn subs_status(app: AppHandle) -> subs::Status {
    subs::status(&app).await
}

#[tauri::command]
async fn subs_setup(app: AppHandle, username: String, password: String) -> Result<String, String> {
    subs::setup(&app, &username, &password).await
}

#[tauri::command]
async fn subs_search(app: AppHandle, id: String, lang: String) -> Result<Vec<subs::Found>, String> {
    subs::search(&app, &id, &lang).await
}

#[tauri::command]
async fn subs_download(app: AppHandle, id: String, sub: String) -> Result<(), String> {
    subs::download(&app, &id, &sub).await
}

/// Lines a subtitle track up with the film's audio (subsync.rs).
#[tauri::command]
async fn subs_sync(app: AppHandle, id: String, index: i64, cues: Vec<(f64, f64)>) -> Result<subsync::Fit, String> {
    let j = media::jf(&app).ok_or("Sign in to Jellyfin first")?;
    subsync::sync(&app, &j, &id, index, cues).await
}

/// The subtitle languages a title gets (saved, or the default rule) for the Download picker and detail panel.
#[tauri::command]
async fn subs_choice(app: AppHandle, tile: tiles::Tile) -> Result<subfetch::Choice, String> {
    subfetch::choice(&app, &tile).await
}

#[tauri::command]
fn subs_set(app: AppHandle, key: String, title: String, langs: Vec<String>) -> Result<(), String> {
    subfetch::set(&app, &key, &title, langs)
}

#[tauri::command]
async fn media_report(app: AppHandle, what: String, body: serde_json::Value) -> Result<(), String> {
    media::report(&app, &what, body).await
}

#[tauri::command]
async fn media_get(app: AppHandle, tile: tiles::Tile, quality: Option<String>) -> Result<String, String> {
    media::get_it(&app, tile, quality.as_deref().unwrap_or("best")).await
}

#[tauri::command]
async fn media_play_vlc(app: AppHandle, id: String, start: f64, title: String, watch_key: Option<String>) -> Result<(), String> {
    media::play_vlc(&app, &id, start, &title, watch_key).await
}

#[tauri::command]
async fn library_stats(app: AppHandle) -> Vec<serde_json::Value> {
    media::library_stats(&app).await
}

#[tauri::command]
async fn library_scan(app: AppHandle) -> Result<(), String> {
    media::scan(&app).await
}

/// A web page Chillarr points people to, by name. Only these, so the page
/// can't be asked to open any address it likes.
#[tauri::command]
fn open_link(app: AppHandle, which: String) -> Result<(), String> {
    let url = match which.as_str() {
        "opensubtitles" => "https://www.opensubtitles.com/en/users/sign_up",
        _ => return Err("Unknown link".into()),
    };
    app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
fn open_folder(app: AppHandle, path: String) -> Result<(), String> {
    // Only folders: the window never gets to launch a program this way.
    if !std::path::Path::new(&path).is_dir() {
        return Err("That isn't a folder".into());
    }
    app.opener().open_path(path, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
async fn media_explore(app: AppHandle, category: String, page: usize) -> Vec<tiles::Tile> {
    media::explore(&app, &category, page).await
}

#[tauri::command]
async fn media_seasons(app: AppHandle, tile: tiles::Tile) -> Result<pick::Seasons, String> {
    pick::seasons(&app, &tile).await
}

#[tauri::command]
async fn media_get_picked(app: AppHandle, tile: tiles::Tile, quality: Option<String>, pick: pick::Pick) -> Result<String, String> {
    pick::get_picked(&app, tile, quality.as_deref().unwrap_or("best"), pick).await
}

#[tauri::command]
async fn media_trailer(app: AppHandle, tile: tiles::Tile) -> Option<String> {
    pick::trailer(&app, &tile).await
}

#[tauri::command]
async fn media_downloads(app: AppHandle) -> Vec<waiting::Download> {
    waiting::downloads(&app).await
}

#[tauri::command]
async fn media_stop_download(app: AppHandle, which: String, download_id: String) -> Result<String, String> {
    waiting::stop_download(&app, &which, &download_id).await
}

#[tauri::command]
async fn media_delete_episodes(app: AppHandle, tile: tiles::Tile, episodes: Vec<(i64, i64)>) -> Result<String, String> {
    waiting::delete_episodes(&app, &tile, &episodes).await
}

#[tauri::command]
async fn media_delete(app: AppHandle, tile: tiles::Tile) -> Result<String, String> {
    waiting::delete_title(&app, &tile).await
}

#[tauri::command]
async fn media_waiting(app: AppHandle) -> Vec<waiting::Waiting> {
    waiting::list(&app).await
}

#[tauri::command]
async fn media_stop_waiting(app: AppHandle, which: String, id: i64) -> Result<String, String> {
    waiting::stop(&app, &which, id).await
}

#[tauri::command]
async fn watchnow_start(app: AppHandle, tile: tiles::Tile, season: Option<i64>, episode: Option<i64>) -> Result<tiles::Tile, String> {
    watchnow::start(&app, tile, season, episode).await
}

#[tauri::command]
async fn watchnow_leave(app: AppHandle, key: String, position: f64, duration: Option<f64>, uid: Option<String>) -> Result<(), String> {
    match uid.filter(|u| !u.is_empty()) {
        Some(uid) => watchnow::leave_as(&app, &uid, &key, position, duration).await,
        None => watchnow::leave(&app, &key, position, duration).await,
    }
}

/// Every 10 s while Watch now plays: where you are, nothing else.
#[tauri::command]
async fn watchnow_progress(app: AppHandle, key: String, position: f64, duration: Option<f64>, uid: String) -> Result<(), String> {
    watchnow::progress_as(&app, &uid, &key, position, duration)
}

#[tauri::command]
async fn watchnow_remove(app: AppHandle, key: String) -> Result<String, String> {
    watchnow::remove(&app, &key).await
}

#[tauri::command]
async fn watchnow_list(app: AppHandle) -> Vec<tiles::Tile> {
    watchnow::list(&app).await
}

#[tauri::command]
async fn setup_folders(app: AppHandle) -> serde_json::Value {
    media::folders(&app).await
}

#[tauri::command]
async fn setup_set_folders(app: AppHandle, movies: Vec<String>, series: Vec<String>) -> Result<(), String> {
    media::set_folders(&app, movies, series).await
}

#[tauri::command]
async fn seeding(app: AppHandle) -> Result<bool, String> {
    waiting::seeding(&app).await
}

#[tauri::command]
async fn set_seeding(app: AppHandle, on: bool) -> Result<String, String> {
    waiting::set_seeding(&app, on).await
}

#[tauri::command]
fn download_folder() -> Option<String> {
    media::download_folder()
}

#[tauri::command]
async fn set_download_folder(app: AppHandle, path: String) -> Result<(), String> {
    media::set_download_folder(&app, &path).await
}

#[tauri::command]
async fn install_apps(app: AppHandle, ids: Vec<AppId>) -> Vec<(AppId, String)> {
    fix::install_apps(&app, ids).await
}

#[tauri::command]
async fn setup_indexers(app: AppHandle) -> serde_json::Value {
    media::indexers(&app).await
}

#[tauri::command]
async fn setup_add_indexer(app: AppHandle, definition: String) -> Result<String, String> {
    media::add_indexer(&app, &definition).await
}

#[tauri::command]
async fn pick_folder(app: AppHandle) -> Option<std::path::PathBuf> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |f| {
        let _ = tx.send(f.and_then(|p| p.into_path().ok()));
    });
    rx.await.ok().flatten()
}

#[tauri::command]
fn get_configs(core: State<'_, Core>) -> Vec<AppConfig> {
    core.configs.read().unwrap().clone()
}

#[tauri::command]
fn get_settings(st: State<'_, SettingsState>) -> settings::Settings {
    st.current.lock().unwrap().clone()
}

/// Applies at once: autostart, auto-fix, and the per-app overrides.
#[tauri::command]
async fn set_settings(app: AppHandle, mut s: settings::Settings) -> Result<Vec<AppConfig>, String> {
    // A program path is only kept when it is that app's real program: it is
    // started, and it may be given a Defender exception.
    for (id, exe) in settings::keep_real_exes(&mut s) {
        let names = settings::exe_names(id).join(" or ");
        log::log(&app, model::Level::Amber, format!("Didn't use {} for {}: it has to be {names}.", exe.display(), id.label()));
    }
    {
        let st = app.state::<SettingsState>();
        // Chillarr's own record of what Defender already allows, never the window's copy.
        s.defender_allowed = st.current.lock().unwrap().defender_allowed.clone();
        settings::save(&st.path, &s)?;
        let remux_changed = st.current.lock().unwrap().skip_remux != s.skip_remux;
        let dubs_changed = st.current.lock().unwrap().skip_dubs != s.skip_dubs;
        *st.current.lock().unwrap() = s.clone();
        if remux_changed {
            let a = app.clone();
            tauri::async_runtime::spawn(async move { crate::pick::no_disc_copies(&a).await });
        }
        if dubs_changed {
            let a = app.clone();
            tauri::async_runtime::spawn(async move { crate::pick::no_dubs(&a).await });
        }
    }
    set_autostart(&app, s.start_with_windows);
    app.state::<guard::AutoFix>().0.store(s.auto_fix, std::sync::atomic::Ordering::Relaxed);
    Ok(rediscover(app).await)
}

#[tauri::command]
async fn pick_exe(app: AppHandle) -> Option<std::path::PathBuf> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().add_filter("Program", &["exe"]).pick_file(move |f| {
        let _ = tx.send(f.and_then(|p| p.into_path().ok()));
    });
    rx.await.ok().flatten()
}

#[tauri::command]
fn get_logs(logs: State<'_, Logs>) -> Vec<LogLine> {
    logs.0.lock().unwrap().iter().cloned().collect()
}

/// Dev-only live check: starts FlareSolverr and Sonarr the way the app does,
/// waits for both, then exits (which must take FlareSolverr down with it).
#[cfg(debug_assertions)]
fn selftest_start() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let all = discover::discover_all();
        let get = |id| all.iter().find(|a| a.id == id).unwrap().clone();
        let procs = procs::Procs::new();
        let flare = get(model::AppId::FlareSolverr);
        if procs::should_spawn(procs.owns_flaresolverr(), procs::port_open(&flare.base_url).await) {
            println!("flare start: {:?}", procs.start_flaresolverr(flare.exe.as_ref().unwrap()));
        }
        let sonarr = get(model::AppId::Sonarr);
        if !procs::port_open(&sonarr.base_url).await {
            println!("sonarr start: {:?}", procs::start_arr(sonarr.exe.as_ref().unwrap(), "Sonarr"));
        }
        println!("flare up: {}", procs::wait_until(&flare.base_url, 60).await);
        println!("sonarr up: {}", procs::wait_until(&sonarr.base_url, 90).await);
        tokio::time::sleep(std::time::Duration::from_secs(15)).await;
        println!("exiting");
    });
}

pub fn run() {
    #[cfg(debug_assertions)]
    if std::env::args().any(|a| a == "--selftest-start") {
        return selftest_start();
    }
    let start_in_tray = std::env::args().any(|a| a == "--tray");

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_main(app);
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--tray"]),
        ))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(Logs::default())
        .manage(login::Pending::default())
        .invoke_handler(tauri::generate_handler![
            window_fullscreen,
            open_link,
            fix_everything,
            fix::defender_answer,
            test_indexers,
            refresh,
            indexer_table,
            open_app,
            rediscover,
            get_configs,
            install_app,
            media_status,
            media_connect,
            media_sign_out,
            media_rows,
            media_episodes,
            media_search,
            media_search_live,
            meili_status,
            meili_set,
            media_play,
            media_report,
            media_get,
            media_play_vlc,
            media_explore,
            media_waiting,
            subs_status,
            media_details,
            media_similar,
            download_folder,
            seeding,
            set_seeding,
            set_download_folder,
            install_apps,
            media_filter,
            media_filter_genres,
            media_filter_studios,
            media_anime_tags,
            remote_status,
            shelf_status,
            shelf_set,
            shelf_brand,
            remote_set,
            remote_forget,
            media_my_list,
            apps_set_login,
            media_segments,
            storage_overview,
            profiles_list,
            syncplay_info,
            syncplay_call,
            syncplay_tile,
            profiles_add,
            profiles_switch,
            profiles_check_pin,
            profiles_forget_web_devices,
            profiles_update,
            profiles_remove,
            profiles_set_password,
            storage_set_cleanup,
            media_my_list_marks,
            completed_remove,
            media_completed,
            media_watched_keys,
            media_show_episodes,
            marker_set,
            marker_restart,
            subs_setup,
            subs_search,
            subs_download,
            subs_sync,
            subs_choice,
            subs_set,
            media_delete,
            media_downloads,
            media_stop_download,
            media_delete_episodes,
            media_seasons,
            media_get_picked,
            media_trailer,
            media_stop_waiting,
            watchnow_start,
            watchnow_leave,
            watchnow_progress,
            watchnow_remove,
            watchnow_list,
            library_stats,
            library_scan,
            open_folder,
            setup_folders,
            setup_set_folders,
            setup_indexers,
            setup_add_indexer,
            pick_folder,
            update_app,
            get_settings,
            set_settings,
            pick_exe,
            get_logs
        ])
        .setup(move |app| {
            // WebView2 offered past searches in its own dropdown, which doesn't
            // look like Chillarr; the search box keeps its own list instead.
            #[cfg(windows)]
            if let Some(w) = app.get_webview_window("main") {
                no_autofill(&w);
            }
            let path = settings_path(app.handle());
            let (mut s, first_run) = settings::load(&path);
            let found = discover::discover_all();
            if first_run {
                s = settings::Settings::from_configs(&found);
                let _ = settings::save(&path, &s);
            }
            // Spec: on by default. Registered on every launch so the entry
            // always points at this copy of Chillarr (an update or a move
            // would otherwise leave it pointing at the old one).
            set_autostart(app.handle(), s.start_with_windows);
            app.manage(guard::AutoFix(std::sync::atomic::AtomicBool::new(s.auto_fix)));
            app.manage(Core::new(settings::apply(found, &s)));
            app.manage(SettingsState { path, current: std::sync::Mutex::new(s) });
            let data_dir = app.path().app_config_dir().unwrap_or_else(|_| std::env::temp_dir().join("Chillarr"));
            app.manage(remote::Remote::load(data_dir.clone()));
            app.manage(discover_shelf::Shelf::load(data_dir.clone()));
            app.manage(profiles::Profiles::load(data_dir.clone()));
            app.manage(watchnow::WatchNow::load(data_dir.clone()));
            app.manage(subfetch::SubsWanted::load(data_dir.clone()));
            app.manage(media::Media::load(data_dir));
            // A profile with a PIN: nobody watches until a profile is picked.
            profiles::lock_if_pinned(app.handle());
            remote::start(app.handle().clone());
            bridge::start(app.handle().clone());
            storage::start_cleanup(app.handle().clone());
            // The catalogs grow quietly in the background.
            let h = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                loop {
                    media::grow(&h).await;
                    tokio::time::sleep(std::time::Duration::from_secs(15 * 60)).await;
                }
            });
            tray::build(app.handle())?;
            #[cfg(debug_assertions)]
            if std::env::args().any(|a| a == "--selftest-fix") {
                let h = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let rep = fix::fix_all(&h, Trigger::Button).await;
                    for c in &rep.cards {
                        println!("[card] {:?} {:?} {} | {:?}", c.id, c.level, c.line, c.notes);
                    }
                    for r in fix::indexer_table_now(&h).await {
                        println!("[row] {} p={:?} s={:?} r={:?}", r.name, r.prowlarr.map(|c| c.level), r.sonarr.map(|c| c.level), r.radarr.map(|c| c.level));
                    }
                    println!("[actions] {:?} [needs_button] {:?}", rep.actions, rep.needs_button);
                    h.exit(0);
                });
                return Ok(());
            }
            guard::spawn(app.handle().clone());
            waiting::spawn(app.handle().clone());
            watchnow::spawn(app.handle().clone());
            discover_shelf::spawn(app.handle().clone());
            subfetch::spawn(app.handle().clone());
            search_tap::spawn(app.handle().clone());
            jfweb::spawn(app.handle().clone());
            if !start_in_tray {
                tray::show_main(app.handle());
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window keeps Chillarr running in the tray.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Chillarr");
}
