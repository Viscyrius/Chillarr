//! Chillarr inside Jellyfin's own web pages. jfweb.rs puts a copy of the
//! window's pages into Jellyfin's web folder with a stand-in for Tauri's
//! `invoke` and `listen` (src/web/shim.js); this is what that stand-in talks
//! to. Each request runs the same command code the window does, as the
//! Jellyfin user whose sign-in it carries (viewer.rs), so phones, browsers
//! and the desktop Jellyfin app get Chillarr's rows, Download, My List, Watch
//! now, subtitles and the player, each person with their own lists.
//!
//! Who may do what is Jellyfin's say: every request carries the page's
//! Jellyfin token, which Jellyfin itself is asked about (cached a minute).
//! Any signed-in user gets the watching side; deleting needs Jellyfin's
//! "allow media deletion" (or admin); storage and library upkeep need an
//! admin; what changes the PC itself (Fix everything, installs, settings,
//! folders, profiles, programs) is the window's only. A command not listed
//! here is refused, so a new one stays off the web until it is added.
//!
//! The token travels in headers (and the event stream's query, which
//! EventSource can't give headers), never a cookie, so a page on another
//! site can't borrow a visitor's session: answering any origin is safe.

use crate::jellyfin::{Jf, Session};
use crate::log::log;
use crate::model::{AppId, Level};
use crate::tiles::Tile;
use crate::viewer::{self, Viewer};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Listener, Manager};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Tried in order; the first free one is used and written to bridge.json.
const PORTS: [u16; 3] = [8694, 8695, 8696];
/// A request's head and body together (subtitle cue lists are the biggest).
const MAX_REQUEST: usize = 2 * 1024 * 1024;
/// The whole request must arrive within this.
const REQUEST_WAIT: Duration = Duration::from_secs(15);
/// Connections at once, event streams included; more are dropped.
const MAX_CONNECTIONS: usize = 256;
/// How long Jellyfin's answer about a token is reused.
const AUTH_TTL: Duration = Duration::from_secs(60);
/// A token Jellyfin turned down is turned down without asking for this long.
const AUTH_BAD_TTL: Duration = Duration::from_secs(10);
/// An open event stream asks Jellyfin about its token again this often.
const STREAM_RECHECK: Duration = Duration::from_secs(5 * 60);
/// A comment line keeps proxies and phones from closing a quiet stream.
const KEEPALIVE: Duration = Duration::from_secs(25);
/// Events every open page of a signed-in viewer gets: a download's progress,
/// named by its title, as on the shared My downloads page. Search results go
/// only to the page that searched (`emit`), a Watch now start only to the
/// person starting it (`emit_user`).
const SHARED_EVENTS: [&str; 1] = ["get-progress"];
/// Event streams one page may hold open. Browsers allow 6 connections to an
/// address, so a page with many streams has none left for its calls.
const MAX_STREAMS_PER_PAGE: usize = 2;
/// Jellyfin is asked about this many unknown tokens at once at most.
const MAX_PROBES: usize = 8;
/// Sign-ins remembered (good and bad); past this the bad ones go first.
const MAX_KNOWN: usize = 1024;

static PORT: AtomicU16 = AtomicU16::new(0);

/// One event for the pages: for one Jellyfin user (and maybe one of their
/// pages), for one device whoever is signed in on it (the phone remote), or,
/// with neither, for every signed-in page.
#[derive(Clone, Debug)]
struct Ev {
    uid: Option<String>,
    page: Option<String>,
    device: Option<String>,
    data: String,
}

impl Ev {
    fn for_stream(&self, uid: &str, page: &str, device: &str) -> bool {
        self.uid.as_ref().is_none_or(|u| u == uid) && self.page.as_ref().is_none_or(|p| p == page) && self.device.as_ref().is_none_or(|d| !d.is_empty() && d == device)
    }
}

fn hub() -> &'static tokio::sync::broadcast::Sender<Arc<Ev>> {
    static HUB: OnceLock<tokio::sync::broadcast::Sender<Arc<Ev>>> = OnceLock::new();
    HUB.get_or_init(|| tokio::sync::broadcast::channel(512).0)
}

fn send(uid: Option<String>, page: Option<String>, event: &str, payload: &impl Serialize) {
    let data = json!({ "event": event, "payload": payload }).to_string();
    let _ = hub().send(Arc::new(Ev { uid, page, device: None, data }));
}

/// An event for every open page on one device (by its Jellyfin device id).
pub fn emit_device(device: &str, event: &str, payload: &impl Serialize) {
    let data = json!({ "event": event, "payload": payload }).to_string();
    let _ = hub().send(Arc::new(Ev { uid: None, page: None, device: Some(device.to_string()), data }));
}

/// Whether a device has a page open with its event stream (so it hears `emit_device`).
pub fn device_online(device: &str) -> bool {
    device_streams().lock().unwrap().get(device).is_some_and(|n| *n > 0)
}

fn device_streams() -> &'static Mutex<HashMap<String, usize>> {
    static S: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

/// An event from a command: a web viewer's goes only to the page that asked,
/// anyone else's to the window as before.
pub fn emit<S: Serialize + Clone>(app: &AppHandle, event: &str, payload: S) {
    match viewer::current() {
        Some(v) => send(Some(v.session.user_id), Some(v.page), event, &payload),
        None => {
            let _ = app.emit(event, payload);
        }
    }
}

/// Like `emit`, but to every open page of that viewer.
pub fn emit_user<S: Serialize + Clone>(app: &AppHandle, event: &str, payload: S) {
    match viewer::current() {
        Some(v) => send(Some(v.session.user_id), None, event, &payload),
        None => {
            let _ = app.emit(event, payload);
        }
    }
}

/// The port the bridge answers on, once it does.
pub fn port() -> Option<u16> {
    Some(PORT.load(Ordering::SeqCst)).filter(|p| *p != 0)
}

// ---------- who is asking ----------

/// What the request names itself as (headers, or the stream's query).
#[derive(Clone, Debug, Default, PartialEq)]
struct Claim {
    token: String,
    device: String,
    page: String,
    origin: String,
}

/// Jellyfin tokens are hex; anything else is turned away before Jellyfin is asked.
fn token_ok(t: &str) -> bool {
    (16..=128).contains(&t.len()) && t.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// jellyfin-web's device ids are base64 text. They go into an Authorization
/// header between quotes, so a quote, comma or line break never passes.
fn device_ok(d: &str) -> bool {
    (1..=256).contains(&d.len()) && d.bytes().all(|b| b.is_ascii_alphanumeric() || b"+/=_.-".contains(&b))
}

fn page_ok(p: &str) -> bool {
    (8..=64).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// Jellyfin user ids are GUIDs; one becomes part of a file name (My List).
fn user_id_ok(id: &str) -> bool {
    (16..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}

/// `http(s)://host[:port]` and nothing more, or None.
fn origin_ok(o: &str) -> Option<&str> {
    let rest = o.strip_prefix("http://").or_else(|| o.strip_prefix("https://"))?;
    (!rest.is_empty() && rest.len() <= 255 && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b))).then_some(o)
}

#[derive(Clone)]
enum Known {
    Ok(Viewer),
    Bad,
}

fn streams() -> &'static Mutex<HashMap<String, usize>> {
    static S: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

fn auth_cache() -> &'static Mutex<HashMap<(String, String), (Instant, Known)>> {
    static C: OnceLock<Mutex<HashMap<(String, String), (Instant, Known)>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

enum AuthError {
    /// No usable sign-in, or Jellyfin turned it down.
    Signin,
    /// Jellyfin didn't answer, so nobody can be told apart.
    Down,
}

/// The viewer a claim stands for, as Jellyfin says.
async fn who(app: &AppHandle, c: &Claim, fresh: bool) -> Result<Viewer, AuthError> {
    if !token_ok(&c.token) || !device_ok(&c.device) || !page_ok(&c.page) {
        return Err(AuthError::Signin);
    }
    let key = (c.token.clone(), c.device.clone());
    if !fresh {
        if let Some((at, known)) = auth_cache().lock().unwrap().get(&key).cloned() {
            match known {
                Known::Ok(mut v) if at.elapsed() < AUTH_TTL => {
                    v.page = c.page.clone();
                    return Ok(v);
                }
                Known::Bad if at.elapsed() < AUTH_BAD_TTL => return Err(AuthError::Signin),
                _ => {}
            }
        }
    }
    static PROBES: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(MAX_PROBES);
    let _probe = PROBES.acquire().await.map_err(|_| AuthError::Down)?;
    let base = app.state::<crate::fix::Core>().config(AppId::Jellyfin).map(|c| c.base_url).ok_or(AuthError::Down)?;
    let probe = Session { device_id: c.device.clone(), token: c.token.clone(), ..Default::default() };
    let me = Jf::on_behalf(&base, &probe).get_json("/Users/Me").await;
    let known = match me {
        Ok(me) => {
            let id = me["Id"].as_str().unwrap_or_default().to_string();
            if !user_id_ok(&id) {
                Known::Bad
            } else {
                let admin = me["Policy"]["IsAdministrator"] == true;
                let session = Session { device_id: c.device.clone(), user: me["Name"].as_str().unwrap_or_default().to_string(), user_id: id, token: c.token.clone() };
                Known::Ok(Viewer { session, admin, can_delete: admin || me["Policy"]["EnableContentDeletion"] == true, page: c.page.clone() })
            }
        }
        Err(crate::jellyfin::JfError::Down) => return Err(AuthError::Down),
        Err(_) => Known::Bad,
    };
    let mut cache = auth_cache().lock().unwrap();
    if cache.len() >= MAX_KNOWN {
        cache.retain(|_, (at, k)| at.elapsed() < AUTH_TTL && matches!(k, Known::Ok(_)));
        if cache.len() >= MAX_KNOWN {
            cache.clear();
        }
    }
    cache.insert(key, (Instant::now(), known.clone()));
    match known {
        Known::Ok(v) => Ok(v),
        Known::Bad => Err(AuthError::Signin),
    }
}

// ---------- what each viewer may do ----------

#[derive(Clone, Copy, Debug, PartialEq)]
enum Need {
    /// Anyone signed in to Jellyfin.
    Viewer,
    /// Jellyfin's "allow media deletion", or an admin.
    Delete,
    Admin,
}

/// What a command needs from the web, or None: the window's only. A new
/// command that takes a Jellyfin item under another argument name must add
/// that name to `item_ids`, or its item skips the visibility check.
fn need(cmd: &str) -> Option<Need> {
    Some(match cmd {
        "web_viewer" | "media_status" | "media_rows" | "media_episodes" | "media_search" | "media_search_live" | "media_play" | "media_report" | "media_get" | "media_get_picked" | "media_seasons" | "media_trailer"
        | "media_explore" | "media_filter" | "media_filter_genres" | "media_filter_studios" | "media_anime_tags" | "media_details" | "media_similar" | "media_my_list" | "media_my_list_marks" | "media_completed"
        | "media_watched_keys" | "completed_remove" | "media_show_episodes" | "marker_set" | "marker_restart" | "media_segments" | "subs_search" | "subs_download" | "subs_sync" | "subs_choice" | "subs_set"
        | "media_waiting" | "media_downloads" | "watchnow_start" | "watchnow_leave" | "watchnow_progress" | "watchnow_list" | "syncplay_info" | "syncplay_call" | "syncplay_tile"
        // Takes a title off the viewer's own Watch now (watchnow.rs keeps one
        // list per Jellyfin user), and only stops a download Watch now itself
        // started and nobody else wants.
        | "watchnow_remove"
        // Only for a viewer who is one of Chillarr's profiles (profiles.rs).
        | "profiles_list" | "profiles_switch_web" | "profiles_add"
        // The phone remote's code for the device asking, and only that one.
        | "remote_device" => Need::Viewer,
        "media_delete" | "media_delete_episodes" | "media_stop_download" | "media_stop_waiting" => Need::Delete,
        "storage_overview" | "storage_set_cleanup" | "library_scan" | "library_stats" | "seeding" | "set_seeding" | "get_settings" | "shelf_status" | "meili_status" | "subs_status" => Need::Admin,
        _ => return None,
    })
}

fn allowed(v: &Viewer, n: Need) -> bool {
    match n {
        Need::Viewer => true,
        Need::Delete => v.can_delete || v.admin,
        Need::Admin => v.admin,
    }
}

// ---------- running a command ----------

/// One argument, by the name the page sends it under (Tauri's camelCase).
/// Missing reads as null, so an Option argument may be left out.
fn arg<T: DeserializeOwned>(args: &Value, key: &str) -> Result<T, String> {
    serde_json::from_value(args.get(key).cloned().unwrap_or(Value::Null)).map_err(|e| format!("Bad {key}: {e}"))
}

fn ok<T: Serialize>(v: T) -> Result<Value, String> {
    serde_json::to_value(v).map_err(|e| e.to_string())
}

fn res<T: Serialize>(r: Result<T, String>) -> Result<Value, String> {
    r.and_then(ok)
}

/// Runs a command (already allowed) as the current viewer. Err is the
/// command's own error, for the page to show.
async fn run(app: &AppHandle, cmd: &str, a: &Value) -> Result<Value, String> {
    use crate::{completed, details, history, marker, media, pick, similar, skip, storage, subfetch, subs, syncplay, waiting, watchnow};
    let me = viewer::current().ok_or("No viewer")?;
    let app = app.clone();
    let app = &app;
    match cmd {
        "web_viewer" => ok(json!({ "user_id": me.session.user_id, "name": me.session.user, "admin": me.admin, "can_delete": me.can_delete })),
        // The viewer's own name; the PC's addresses and the owner's account
        // are the window's business.
        "media_status" => {
            let mut st = media::status(app).await;
            st.signed_in = true;
            st.user = Some(me.session.user.clone());
            st.address = None;
            st.tailscale = None;
            st.tailscale_ip = None;
            ok(st)
        }
        "media_rows" => res(media::rows(app, &arg::<String>(a, "tab")?).await),
        "media_episodes" => ok(media::episodes(app, &arg::<String>(a, "seriesId")?).await),
        "media_search" => ok(media::search(app, &arg::<String>(a, "term")?).await),
        "media_search_live" => ok(crate::search::live(app.clone(), arg(a, "term")?, arg(a, "seq")?).await),
        "media_play" => res(media::play(app, &arg::<String>(a, "id")?, arg(a, "start")?, arg(a, "audio")?, arg(a, "subtitle")?).await),
        "media_report" => res(media::report(app, &arg::<String>(a, "what")?, arg(a, "body")?).await),
        "media_get" => res(media::get_it(app, arg(a, "tile")?, arg::<Option<String>>(a, "quality")?.as_deref().unwrap_or("best")).await),
        "media_get_picked" => res(pick::get_picked(app, arg(a, "tile")?, arg::<Option<String>>(a, "quality")?.as_deref().unwrap_or("best"), arg(a, "pick")?).await),
        "media_seasons" => res(pick::seasons(app, &arg::<Tile>(a, "tile")?).await),
        "media_trailer" => ok(pick::trailer(app, &arg::<Tile>(a, "tile")?).await),
        "media_explore" => ok(media::explore(app, &arg::<String>(a, "category")?, arg(a, "page")?).await),
        "media_filter" => ok(media::filter(app, arg(a, "filter")?, arg(a, "page")?).await),
        "media_filter_genres" => ok(media::filter_genres(app, &arg::<String>(a, "kind")?)),
        "media_filter_studios" => ok(media::filter_studios(app, &arg::<String>(a, "kind")?)),
        "media_anime_tags" => res(media::anime_tags().await),
        "media_details" => ok(details::details(app, &arg::<Tile>(a, "tile")?).await),
        "media_similar" => ok(similar::similar(app, &arg::<Tile>(a, "tile")?).await),
        "media_my_list" => res(media::toggle_my_list(app, arg(a, "tile")?, arg(a, "on")?).await),
        "media_my_list_marks" => ok(media::my_list_marks(app).await),
        "media_completed" => ok(completed::tiles(app).await),
        "media_watched_keys" => ok(completed::watched_keys(&history::active(app))),
        "completed_remove" => res(completed::remove(app, &arg::<String>(a, "key")?)),
        "media_show_episodes" => res(pick::show_episodes(app, &arg::<Tile>(a, "tile")?).await),
        "marker_set" => res(marker::set(app, arg(a, "tvdb")?, marker::Spot { season: arg(a, "season")?, episode: arg(a, "episode")? })),
        "marker_restart" => res(pick::restart(app, arg(a, "tvdb")?, arg(a, "season")?).await),
        "media_segments" => ok(skip::segments(app, &arg::<String>(a, "id")?).await),
        "subs_search" => res(subs::search(app, &arg::<String>(a, "id")?, &arg::<String>(a, "lang")?).await),
        "subs_download" => res(subs::download(app, &arg::<String>(a, "id")?, &arg::<String>(a, "sub")?).await),
        "subs_sync" => {
            let j = media::jf(app).ok_or("Sign in to Jellyfin first")?;
            res(crate::subsync::sync(app, &j, &arg::<String>(a, "id")?, arg(a, "index")?, arg(a, "cues")?).await)
        }
        "subs_choice" => res(subfetch::choice(app, &arg::<Tile>(a, "tile")?).await),
        "subs_set" => res(subfetch::set(app, &arg::<String>(a, "key")?, &arg::<String>(a, "title")?, arg(a, "langs")?)),
        "subs_status" => ok(subs::status(app).await),
        "media_waiting" => ok(waiting::list(app).await),
        "media_downloads" => ok(waiting::downloads(app).await),
        "media_stop_waiting" => res(waiting::stop(app, &arg::<String>(a, "which")?, arg(a, "id")?).await),
        "media_stop_download" => res(waiting::stop_download(app, &arg::<String>(a, "which")?, &arg::<String>(a, "downloadId")?).await),
        "media_delete" => res(waiting::delete_title(app, &arg::<Tile>(a, "tile")?).await),
        "media_delete_episodes" => res(waiting::delete_episodes(app, &arg::<Tile>(a, "tile")?, &arg::<Vec<(i64, i64)>>(a, "episodes")?).await),
        "watchnow_start" => res(watchnow::start(app, arg(a, "tile")?, arg(a, "season")?, arg(a, "episode")?).await),
        // Positions are kept per Jellyfin user: always the viewer's own,
        // whatever user id the page sent.
        "watchnow_leave" => res(watchnow::leave_as(app, &me.session.user_id, &arg::<String>(a, "key")?, arg(a, "position")?, arg(a, "duration")?).await),
        "watchnow_progress" => res(watchnow::progress_as(app, &me.session.user_id, &arg::<String>(a, "key")?, arg(a, "position")?, arg(a, "duration")?)),
        "watchnow_remove" => res(watchnow::remove(app, &arg::<String>(a, "key")?).await),
        "watchnow_list" => ok(watchnow::list(app).await),
        "syncplay_info" => res(syncplay::info(app)),
        "syncplay_call" => res(syncplay::call(app, &arg::<String>(a, "method")?, &arg::<String>(a, "path")?, arg(a, "body")?).await),
        "syncplay_tile" => ok(syncplay::tile(app, &arg::<String>(a, "id")?).await),
        "profiles_list" => ok(crate::profiles::list_for_web(app, &me.session.user_id, arg::<Option<String>>(a, "trust")?.as_deref()).await),
        // { trust: key } when the main profile's password was typed: the page keeps it.
        "profiles_switch_web" => res(crate::profiles::authorize_web(app, &me.session.user_id, &arg::<String>(a, "id")?, arg(a, "pin")?, arg(a, "password")?, arg(a, "trust")?, &arg::<String>(a, "code")?).await.map(|t| json!({ "trust": t }))),
        // The device is the one the request's sign-in came from, never one the page names.
        // Known by the key Chillarr gave the device (it keeps it); none or an
        // unknown one makes a new device, so a page can't take over another's.
        "remote_device" => res(crate::remote::for_device(app, arg::<Option<String>>(a, "key")?.as_deref(), &arg::<Option<String>>(a, "name")?.unwrap_or_default(), arg::<Option<bool>>(a, "renew")?.unwrap_or(false))),
        "profiles_add" => res(crate::profiles::add_web(app, &me.session.user_id, &arg::<String>(a, "name")?, &arg::<String>(a, "color")?, arg(a, "pin")?).await),
        "storage_overview" => ok(storage::overview(app).await),
        "storage_set_cleanup" => res(storage::set_cleanup(app, arg(a, "days")?)),
        "library_stats" => ok(media::library_stats(app).await),
        "library_scan" => res(media::scan(app).await),
        "seeding" => res(waiting::seeding(app).await),
        "set_seeding" => res(waiting::set_seeding(app, arg(a, "on")?).await),
        "get_settings" => ok(app.state::<crate::SettingsState>().current.lock().unwrap().clone()),
        "shelf_status" => ok(crate::discover_shelf::status(app)),
        "meili_status" => ok(crate::meili::status(app).await),
        _ => Err("That isn't available here.".into()),
    }
}

/// The Jellyfin item ids a request names: `id`, `seriesId`, and a tile's.
/// The web profile commands' `id` is a person (a Jellyfin user), not a title.
fn item_ids(cmd: &str, args: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if matches!(cmd, "profiles_list" | "profiles_switch_web" | "profiles_add") {
        return out;
    }
    for v in [&args["id"], &args["seriesId"], &args["tile"]["jf_id"], &args["tile"]["series_id"]] {
        if let Some(s) = v.as_str().filter(|s| !s.is_empty()) {
            out.push(s.to_string());
        }
    }
    out
}

fn visible_cache() -> &'static Mutex<HashMap<(String, String), Instant>> {
    static C: OnceLock<Mutex<HashMap<(String, String), Instant>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Every Jellyfin item a request names must be one the viewer can open in
/// Jellyfin. Some commands look items up with the owner's account (subtitles,
/// intro times), which sees every library; a kids' account must not reach
/// past its own through them.
async fn check_items(app: &AppHandle, cmd: &str, args: &Value) -> Result<(), String> {
    let ids = item_ids(cmd, args);
    if ids.is_empty() {
        return Ok(());
    }
    let me = viewer::current().ok_or("No viewer")?;
    let j = crate::media::jf(app).ok_or("Jellyfin isn't answering.")?;
    for id in ids {
        if !user_id_ok(&id) {
            return Err("That isn't a Jellyfin title.".into());
        }
        let key = (me.session.user_id.clone(), id.clone());
        if visible_cache().lock().unwrap().get(&key).is_some_and(|at| at.elapsed() < AUTH_TTL) {
            continue;
        }
        j.get_json(&format!("/Users/{}/Items/{id}", me.session.user_id)).await.map_err(|_| "That title isn't in your Jellyfin.".to_string())?;
        let mut c = visible_cache().lock().unwrap();
        if c.len() >= MAX_KNOWN {
            c.clear();
        }
        c.insert(key, Instant::now());
    }
    Ok(())
}

/// The PC's own address for Jellyfin, as the page reaches it: results name
/// Jellyfin by the address Chillarr uses (often localhost), which a phone
/// can't open. Only the start of a URL is swapped, in this one answer.
fn rewrite(text: &str, base: &str, origin: Option<&str>) -> String {
    let (Some(origin), base) = (origin, base.trim_end_matches('/')) else { return text.to_string() };
    if base.is_empty() || base == origin {
        return text.to_string();
    }
    let ws = |u: &str| u.replacen("https://", "wss://", 1).replacen("http://", "ws://", 1);
    let mut out = text.to_string();
    for (from, to) in [(base.to_string(), origin.to_string()), (ws(base), ws(origin))] {
        // A JSON string starting with exactly that address: then a path, a
        // query, or the string's end (so :8096 never matches :80960).
        for next in ['/', '?', '"'] {
            out = out.replace(&format!("\"{from}{next}"), &format!("\"{to}{next}"));
        }
    }
    out
}

/// Jellyfin tokens a web viewer must never be handed: the owner's and every
/// profile's, unless it is the viewer's own.
fn foreign_tokens(app: &AppHandle, own: &str) -> Vec<String> {
    let mut out = Vec::new();
    let media = app.state::<crate::media::Media>();
    if let Some(s) = media.session.lock().unwrap().as_ref() {
        out.push(s.token.clone());
    }
    if let Some(p) = media.profile.lock().unwrap().as_ref() {
        out.push(p.token.clone());
    }
    out.extend(crate::profiles::tokens(app));
    out.retain(|t| t.len() >= 16 && t != own);
    out
}

// ---------- the server ----------

/// Starts the bridge once, at launch. It stays up while Chillarr runs: it only
/// answers people Jellyfin knows.
pub fn start(app: AppHandle) {
    for name in SHARED_EVENTS {
        app.listen_any(name, move |e| {
            let payload: Value = serde_json::from_str(e.payload()).unwrap_or(Value::Null);
            send(None, None, name, &payload);
        });
    }
    tauri::async_runtime::spawn(async move {
        let mut bound = None;
        for p in PORTS {
            if let Ok(l) = TcpListener::bind(("0.0.0.0", p)).await {
                bound = Some((l, p));
                break;
            }
        }
        let Some((listener, port)) = bound else {
            log(&app, Level::Red, "Chillarr couldn't open a port for its pages inside Jellyfin (8694-8696 are taken), so Jellyfin shows them without Chillarr's features.");
            return;
        };
        PORT.store(port, Ordering::SeqCst);
        crate::jfweb::bridge_moved(&app);
        let slots = Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS));
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                // Out of sockets: give some back before trying again.
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            };
            // Full: the connection is dropped rather than queued.
            let Ok(slot) = slots.clone().try_acquire_owned() else { continue };
            let h = app.clone();
            tauri::async_runtime::spawn(async move {
                let _ = serve(h, sock).await;
                drop(slot);
            });
        }
    });
}

fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().skip(1).find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim().eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

enum Read {
    Request(String, Vec<u8>),
    TooBig,
    Gone,
}

async fn read_request(sock: &mut TcpStream) -> std::io::Result<Read> {
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 8192];
    let (head_end, len) = loop {
        let n = sock.read(&mut chunk).await?;
        if n == 0 {
            return Ok(Read::Gone);
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..i]).to_string();
            let len = header(&head, "content-length").and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
            if i + 4 + len > MAX_REQUEST {
                return Ok(Read::TooBig);
            }
            break (i + 4, len);
        }
        if buf.len() > MAX_REQUEST {
            return Ok(Read::TooBig);
        }
    };
    while buf.len() < head_end + len {
        let n = sock.read(&mut chunk).await?;
        if n == 0 {
            return Ok(Read::Gone);
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    Ok(Read::Request(head, buf[head_end..head_end + len].to_vec()))
}

/// CORS lines for an answer to `origin` (any: see the module note).
fn cors(origin: Option<&str>) -> String {
    format!(
        "Access-Control-Allow-Origin: {}\r\nVary: Origin\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type, X-Chillarr-Token, X-Chillarr-Device, X-Chillarr-Origin, X-Chillarr-Page\r\nAccess-Control-Allow-Private-Network: true\r\nAccess-Control-Max-Age: 600\r\n",
        origin.unwrap_or("*")
    )
}

async fn reply(sock: &mut TcpStream, origin: Option<&str>, status: &str, kind: &str, body: &[u8]) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n{}Connection: close\r\n\r\n",
        body.len(),
        cors(origin)
    );
    sock.write_all(head.as_bytes()).await?;
    sock.write_all(body).await?;
    sock.shutdown().await
}

async fn error(sock: &mut TcpStream, origin: Option<&str>, status: &str, msg: &str) -> std::io::Result<()> {
    reply(sock, origin, status, "application/json", json!({ "error": msg }).to_string().as_bytes()).await
}

fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                match u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16) {
                    Ok(x) => {
                        out.push(x);
                        i += 3;
                        continue;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            b'+' => out.push(b' '),
            x => out.push(x),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query(path: &str) -> HashMap<String, String> {
    path.split_once('?')
        .map(|(_, q)| q.split('&').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (unescape(k), unescape(v))).collect())
        .unwrap_or_default()
}

async fn serve(app: AppHandle, mut sock: TcpStream) -> std::io::Result<()> {
    let Ok(read) = tokio::time::timeout(REQUEST_WAIT, read_request(&mut sock)).await else { return Ok(()) };
    let (head, body) = match read? {
        Read::Request(h, b) => (h, b),
        Read::TooBig => return error(&mut sock, None, "413 Payload Too Large", "That's too much to send.").await,
        Read::Gone => return Ok(()),
    };
    let origin = header(&head, "origin").and_then(origin_ok).map(String::from);
    let origin = origin.as_deref();
    let mut first = head.lines().next().unwrap_or_default().split_whitespace();
    let (method, path) = (first.next().unwrap_or_default(), first.next().unwrap_or_default());
    if method == "OPTIONS" {
        return reply(&mut sock, origin, "204 No Content", "text/plain", b"").await;
    }
    if method == "GET" && (path == "/events" || path.starts_with("/events?")) {
        let q = query(path);
        let get = |k: &str| q.get(k).cloned().unwrap_or_default();
        let claim = Claim { token: get("token"), device: get("device"), page: get("page"), origin: String::new() };
        return events(app, sock, origin, claim, get("remote")).await;
    }
    let Some(cmd) = path.strip_prefix("/invoke/").filter(|c| !c.is_empty() && c.len() <= 64 && c.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')) else {
        return error(&mut sock, origin, "404 Not Found", "Not here").await;
    };
    if method != "POST" {
        return error(&mut sock, origin, "405 Method Not Allowed", "Use POST").await;
    }
    let claim = Claim {
        token: header(&head, "x-chillarr-token").unwrap_or_default().to_string(),
        device: header(&head, "x-chillarr-device").unwrap_or_default().to_string(),
        page: header(&head, "x-chillarr-page").unwrap_or_default().to_string(),
        origin: header(&head, "x-chillarr-origin").unwrap_or_default().to_string(),
    };
    let v = match who(&app, &claim, false).await {
        Ok(v) => v,
        Err(AuthError::Signin) => return error(&mut sock, origin, "401 Unauthorized", "Sign in to Jellyfin again.").await,
        Err(AuthError::Down) => return error(&mut sock, origin, "503 Service Unavailable", "Jellyfin isn't answering.").await,
    };
    let Some(n) = need(cmd) else {
        return error(&mut sock, origin, "403 Forbidden", "That's only in the Chillarr app on the PC.").await;
    };
    if !allowed(&v, n) {
        let msg = if n == Need::Delete { "Your Jellyfin account isn't allowed to delete." } else { "Only a Jellyfin admin can do that." };
        return error(&mut sock, origin, "403 Forbidden", msg).await;
    }
    let args: Value = if body.is_empty() { json!({}) } else {
        match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(_) => return error(&mut sock, origin, "400 Bad Request", "That request wasn't readable.").await,
        }
    };
    let own = v.session.token.clone();
    let out = viewer::run(v, async {
        check_items(&app, cmd, &args).await?;
        run(&app, cmd, &args).await
    })
    .await;
    match out {
        Ok(value) => {
            let text = value.to_string();
            if foreign_tokens(&app, &own).iter().any(|t| text.contains(t.as_str())) {
                log(&app, Level::Red, format!("Chillarr stopped an answer to Jellyfin's pages ({cmd}) that held another account's sign-in."));
                return error(&mut sock, origin, "500 Internal Server Error", "Something went wrong.").await;
            }
            let base = app.state::<crate::fix::Core>().config(AppId::Jellyfin).map(|c| c.base_url).unwrap_or_default();
            // The browser's own Origin header, which a page can't set; the
            // shim's copy only for a client that sends none.
            let text = rewrite(&text, &base, origin.or_else(|| origin_ok(&claim.origin)));
            reply(&mut sock, origin, "200 OK", "application/json", text.as_bytes()).await
        }
        Err(e) => error(&mut sock, origin, "400 Bad Request", &e).await,
    }
}

/// The event stream for one page: its own searches and the shared events,
/// until it closes or its token stops working. With the phone remote's key
/// for this device (`remote`), the remote's presses for it too.
async fn events(app: AppHandle, mut sock: TcpStream, origin: Option<&str>, claim: Claim, remote: String) -> std::io::Result<()> {
    let v = match who(&app, &claim, false).await {
        Ok(v) => v,
        Err(AuthError::Signin) => return error(&mut sock, origin, "401 Unauthorized", "Sign in to Jellyfin again.").await,
        Err(AuthError::Down) => return error(&mut sock, origin, "503 Service Unavailable", "Jellyfin isn't answering.").await,
    };
    // Counted until this stream ends, however it ends: per page, and per device.
    struct Open(&'static Mutex<HashMap<String, usize>>, String);
    impl Drop for Open {
        fn drop(&mut self) {
            let mut m = self.0.lock().unwrap();
            if let Some(n) = m.get_mut(&self.1) {
                *n -= 1;
                if *n == 0 {
                    m.remove(&self.1);
                }
            }
        }
    }
    let key = format!("{}/{}", v.session.user_id, v.page);
    let full = {
        let mut m = streams().lock().unwrap();
        let n = m.entry(key.clone()).or_insert(0);
        let full = *n >= MAX_STREAMS_PER_PAGE;
        if !full {
            *n += 1;
        }
        full
    };
    if full {
        return error(&mut sock, origin, "429 Too Many Requests", "This page already has its event stream.").await;
    }
    let _open = Open(streams(), key);
    // The phone remote's device, by the key Chillarr gave it: never the
    // Jellyfin device id, which any signed-in caller can claim.
    let remote_id = (!remote.is_empty()).then(|| crate::remote::stream_device(&app, &remote)).flatten();
    let _device_open = remote_id.clone().map(|id| {
        *device_streams().lock().unwrap().entry(id.clone()).or_insert(0) += 1;
        Open(device_streams(), id)
    });
    let device = remote_id.unwrap_or_default();
    let mut rx = hub().subscribe();
    let head = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n{}\r\n", cors(origin));
    sock.write_all(head.as_bytes()).await?;
    sock.write_all(b"retry: 3000\n\n").await?;
    let (page, uid) = (v.page.clone(), v.session.user_id.clone());
    let mut checked = Instant::now();
    let r = async {
        loop {
            let msg = tokio::time::timeout(KEEPALIVE, rx.recv()).await;
            if checked.elapsed() > STREAM_RECHECK {
                if who(&app, &claim, true).await.is_err() {
                    return Ok(());
                }
                checked = Instant::now();
            }
            match msg {
                Err(_) => sock.write_all(b": keep-alive\n\n").await?,
                Ok(Ok(ev)) => {
                    if ev.for_stream(&uid, &page, &device) {
                        sock.write_all(format!("data: {}\n\n", ev.data).as_bytes()).await?;
                    }
                }
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {}
                Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => return Ok(()),
            }
        }
    }
    .await;
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewer(admin: bool, can_delete: bool) -> Viewer {
        Viewer { session: Session::default(), admin, can_delete, page: "page-1234".into() }
    }

    #[test]
    fn only_listed_commands_reach_the_web() {
        for c in ["fix_everything", "set_settings", "install_apps", "profiles_switch", "profiles_remove", "profiles_update", "profiles_set_password", "profiles_check_pin", "profiles_forget_web_devices", "pick_folder", "open_folder", "open_link", "remote_set", "remote_forget", "remote_status", "apps_set_login", "setup_set_folders", "set_download_folder", "meili_set", "shelf_set", "media_connect", "media_sign_out", "subs_setup", "get_logs", "media_play_vlc", "window_fullscreen", "defender_answer", "update_app", "a_new_command"] {
            assert_eq!(need(c), None, "{c}");
        }
        assert_eq!(need("media_rows"), Some(Need::Viewer));
        assert_eq!(need("media_delete"), Some(Need::Delete));
        assert_eq!(need("storage_set_cleanup"), Some(Need::Admin));
    }

    #[test]
    fn deleting_and_upkeep_follow_jellyfins_permissions() {
        assert!(allowed(&viewer(false, false), Need::Viewer));
        assert!(!allowed(&viewer(false, false), Need::Delete));
        assert!(allowed(&viewer(false, true), Need::Delete));
        assert!(!allowed(&viewer(false, true), Need::Admin));
        assert!(allowed(&viewer(true, false), Need::Delete) && allowed(&viewer(true, false), Need::Admin));
    }

    #[test]
    fn every_listed_command_is_run() {
        // A command listed in `need` but missing from `run` would answer
        // "isn't available"; the match arms are checked against the list here.
        let src = include_str!("bridge.rs");
        let run_body = &src[src.find("async fn run(").unwrap()..src.find("fn rewrite(").unwrap()];
        let need_body = &src[src.find("fn need(").unwrap()..src.find("fn allowed(").unwrap()];
        for c in need_body.split('"').skip(1).step_by(2) {
            assert!(run_body.contains(&format!("\"{c}\" =>")), "{c} is allowed but not run");
        }
    }

    #[test]
    fn claims_are_checked_before_jellyfin_is_asked() {
        assert!(token_ok("0123456789abcdef0123456789abcdef"));
        assert!(!token_ok("short") && !token_ok("0123456789abcdef\"x") && !token_ok(&"a".repeat(200)));
        assert!(device_ok("TW96aWxsYS81LjAgKFdpbmRvd3MgTlQ=") && device_ok("chillarr-1f2e"));
        for bad in ["", "a\"b", "a,b", "a\r\nb", "a b"] {
            assert!(!device_ok(bad), "{bad:?}");
        }
        assert!(page_ok("3f2a9c1e-77aa") && !page_ok("short") && !page_ok("../../etc"));
        assert!(user_id_ok("0123456789abcdef0123456789abcdef") && !user_id_ok("..\\..\\x0123456789abcd"));
    }

    #[test]
    fn origins_are_bare_addresses() {
        assert_eq!(origin_ok("http://192.168.1.50:8096"), Some("http://192.168.1.50:8096"));
        assert_eq!(origin_ok("https://my-pc.tail1234.ts.net"), Some("https://my-pc.tail1234.ts.net"));
        for bad in ["null", "", "http://", "javascript:alert(1)", "http://a/b", "http://a\"b", "http://a?x", "ftp://a"] {
            assert_eq!(origin_ok(bad), None, "{bad}");
        }
    }

    #[test]
    fn jellyfin_urls_are_given_the_pages_address() {
        let text = r#"{"url":"http://localhost:8096/Videos/1/stream.mp4","socket":"ws://localhost:8096/socket","other":"https://image.tmdb.org/x","note":"see http://localhost:8096 later"}"#;
        let out = rewrite(text, "http://localhost:8096/", Some("https://tv.example:443"));
        assert_eq!(rewrite(r#"{"a":"http://localhost:80960/x","b":"http://localhost:8096"}"#, "http://localhost:8096", Some("http://pc:8096")), r#"{"a":"http://localhost:80960/x","b":"http://pc:8096"}"#);
        assert!(out.contains(r#""url":"https://tv.example:443/Videos/1/stream.mp4""#), "{out}");
        assert!(out.contains(r#""socket":"wss://tv.example:443/socket""#), "{out}");
        assert!(out.contains("https://image.tmdb.org/x") && out.contains("see http://localhost:8096 later"), "only URL values that start with it");
        assert_eq!(rewrite(text, "http://localhost:8096", None), text);
    }

    #[test]
    fn every_item_a_request_names_is_checked() {
        let args = json!({ "id": "a1", "tile": { "jf_id": "b2", "series_id": "c3" }, "seriesId": "d4", "key": "tmdb:1" });
        let mut ids = item_ids("media_details", &args);
        ids.sort();
        assert_eq!(ids, ["a1", "b2", "c3", "d4"]);
        assert!(item_ids("media_stop_waiting", &json!({ "id": 5, "tile": { "jf_id": null } })).is_empty(), "Radarr/Sonarr numbers and no item");
        assert!(item_ids("profiles_switch_web", &json!({ "id": "dbe66171bc7341e5948735285f43113c" })).is_empty(), "a profile is a person, not a title");
    }

    #[test]
    fn a_stream_gets_its_own_and_shared_events_only() {
        let ev = |uid: Option<&str>, page: Option<&str>| Ev { uid: uid.map(String::from), page: page.map(String::from), device: None, data: String::new() };
        assert!(ev(None, None).for_stream("u1", "p1", "d1"));
        assert!(ev(Some("u1"), None).for_stream("u1", "p2", "d1"));
        assert!(!ev(Some("u2"), None).for_stream("u1", "p1", "d1"));
        assert!(ev(Some("u1"), Some("p1")).for_stream("u1", "p1", "d1"));
        assert!(!ev(Some("u1"), Some("p1")).for_stream("u1", "p2", "d1"));
        assert!(!ev(Some("u2"), Some("p1")).for_stream("u1", "p1", "d1"), "knowing a page id isn't enough");
        // The phone remote: one device, whoever is signed in on it.
        let tv = Ev { uid: None, page: None, device: Some("tv".into()), data: String::new() };
        assert!(tv.for_stream("u1", "p1", "tv") && tv.for_stream("u2", "p9", "tv"));
        assert!(!tv.for_stream("u1", "p1", "phone"));
        assert!(!Ev { device: Some(String::new()), ..tv.clone() }.for_stream("u1", "p1", ""), "a stream without the remote's key hears none of it");
    }

    #[test]
    fn query_values_are_decoded() {
        let q = query("/events?token=abc&device=TW96%3D%3D&page=p-1");
        assert_eq!(q["device"], "TW96==");
        assert_eq!(q["page"], "p-1");
        assert_eq!(unescape("a%2"), "a%2");
        assert_eq!(unescape("%zz"), "%zz");
    }
}
