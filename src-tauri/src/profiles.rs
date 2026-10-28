//! Profiles, Netflix-style: "Who's watching?" and each person keeps their own
//! Continue watching, watched marks and My List. Every profile is a real
//! Jellyfin account, so that history is the same in Jellyfin's TV and phone
//! apps. Chillarr makes the account with a random password nobody types, and
//! keeps its sign-in (like jellyfin.json does for the owner). A profile can
//! have a 4-digit PIN, kept only as a salted hash.
//!
//! A PIN guards the profile itself too: changing its name, colour, PIN or
//! password needs that PIN, and so does removing it (or, for a forgotten PIN,
//! the Chillarr login, checked against the owner's Jellyfin account). Wrong
//! tries are limited. With a PIN anywhere, the app starts locked (media.rs)
//! until a profile is picked.
//!
//! The owner is the Jellyfin admin Chillarr was set up with; it is always the
//! first profile and can't be removed. Owner-only work (libraries, plugins,
//! deleting files) keeps using the owner's account whoever is watching.

use crate::jellyfin::{Jf, Session};
use crate::log::log;
use crate::media::{jf_admin, Media};
use crate::model::{AppId, Level};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct Profile {
    /// The Jellyfin account's id.
    id: String,
    name: String,
    /// The avatar colour, a CSS colour picked in the app.
    color: String,
    /// `salt:hash` (hex, PBKDF2-SHA256) of the PIN, if it has one.
    pin: Option<String>,
    /// Its own sign-in. None for the owner, who uses jellyfin.json.
    session: Option<Session>,
}

#[derive(Serialize, Deserialize, Default)]
struct Store {
    list: Vec<Profile>,
}

pub struct Profiles {
    file: PathBuf,
    store: Mutex<Store>,
    /// Wrong PIN and login tries, for this session.
    tries: Mutex<Tries>,
}

/// Wrong tries allowed before a wait, the first wait, and how often it doubles at most.
const FREE_TRIES: u32 = 5;
const FIRST_WAIT: Duration = Duration::from_secs(30);
const MOST_DOUBLINGS: u32 = 5;

/// Wrong tries by what was tried (a profile id, or "login"): (count, wait until).
#[derive(Default)]
struct Tries(HashMap<String, (u32, Option<Instant>)>);

impl Tries {
    /// Counts a try, as wrong until `passed` says otherwise (so tries made at
    /// the same time can't slip past the limit). Err: seconds still to wait.
    fn begin(&mut self, key: &str, now: Instant) -> Result<(), u64> {
        let e = self.0.entry(key.to_string()).or_default();
        if let Some(until) = e.1.filter(|u| now < *u) {
            return Err((until - now).as_millis().div_ceil(1000) as u64);
        }
        e.0 += 1;
        if e.0 >= FREE_TRIES {
            e.1 = Some(now + FIRST_WAIT * (1 << (e.0 - FREE_TRIES).min(MOST_DOUBLINGS)));
        }
        Ok(())
    }

    fn passed(&mut self, key: &str) {
        self.0.remove(key);
    }
}

fn wait_message(secs: u64) -> String {
    if secs < 120 {
        format!("Too many wrong tries. Try again in {secs} seconds.")
    } else {
        format!("Too many wrong tries. Try again in {} minutes.", secs.div_ceil(60))
    }
}

#[derive(Serialize, Clone)]
pub struct View {
    pub id: String,
    pub name: String,
    pub color: String,
    pub has_pin: bool,
    pub owner: bool,
    pub active: bool,
}

const OWNER_COLOR: &str = "#e50914";

impl Profiles {
    pub fn load(dir: PathBuf) -> Profiles {
        let file = dir.join("profiles.json");
        let store = crate::seal::read(&file).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        Profiles { file, store: Mutex::new(store), tries: Mutex::new(Tries::default()) }
    }

    fn save(&self) {
        if let Ok(text) = serde_json::to_string(&*self.store.lock().unwrap()) {
            let _ = crate::seal::write(&self.file, &text);
        }
    }
}

fn owner(app: &AppHandle) -> Option<Session> {
    app.state::<Media>().session.lock().unwrap().clone()
}

/// Puts the owner's profile first in the list the first time it's needed.
fn ensure_owner(app: &AppHandle, me: &Session) {
    let profiles = app.state::<Profiles>();
    {
        let mut s = profiles.store.lock().unwrap();
        if !s.list.iter().any(|p| p.id == me.user_id) {
            s.list.insert(0, Profile { id: me.user_id.clone(), name: me.user.clone(), color: OWNER_COLOR.into(), pin: None, session: None });
        }
    }
    profiles.save();
}

/// The profile with this id. Only ids in the list get anywhere near Jellyfin
/// or a file name (`saved-{id}.json`).
fn find_in<'a>(list: &'a [Profile], id: &str) -> Option<&'a Profile> {
    list.iter().find(|p| p.id == id)
}

fn find(app: &AppHandle, me: &Session, id: &str) -> Result<Profile, String> {
    ensure_owner(app, me);
    find_in(&app.state::<Profiles>().store.lock().unwrap().list, id).cloned().ok_or_else(|| "That profile is gone.".into())
}

/// On start: when a profile that can be picked has a PIN, nobody is watching
/// until one is picked (media.rs `lock`).
pub fn lock_if_pinned(app: &AppHandle) {
    let Some(me) = owner(app) else { return };
    let pinned = app.state::<Profiles>().store.lock().unwrap().list.iter().any(|p| (p.id == me.user_id || p.session.is_some()) && p.pin.is_some());
    if pinned {
        app.state::<Media>().lock();
    }
}

/// The owner first (added the first time it's asked for), then the others.
pub fn list(app: &AppHandle) -> Vec<View> {
    let Some(me) = owner(app) else { return vec![] };
    ensure_owner(app, &me);
    let profiles = app.state::<Profiles>();
    let media = app.state::<Media>();
    // Locked, nobody is watching yet.
    let active = if media.is_locked() { String::new() } else { media.profile.lock().unwrap().as_ref().map(|p| p.user_id.clone()).unwrap_or_else(|| me.user_id.clone()) };
    let s = profiles.store.lock().unwrap();
    let mut out: Vec<View> = s
        .list
        .iter()
        .filter(|p| p.id == me.user_id || p.session.is_some())
        .map(|p| View { id: p.id.clone(), name: p.name.clone(), color: p.color.clone(), has_pin: p.pin.is_some(), owner: p.id == me.user_id, active: p.id == active })
        .collect();
    out.sort_by_key(|v| !v.owner);
    out
}

/// Every profile signed in to Jellyfin as itself, the owner first:
/// (Jellyfin user id, client). For reading each person's watch history.
pub fn all_jf(app: &AppHandle) -> Vec<(String, Jf)> {
    let Some(me) = owner(app) else { return vec![] };
    let mut out: Vec<(String, Jf)> = jf_admin(app).filter(|j| j.user_id.is_some()).map(|j| (me.user_id.clone(), j)).into_iter().collect();
    let Some(base) = app.state::<crate::fix::Core>().config(AppId::Jellyfin).map(|c| c.base_url) else { return out };
    for p in app.state::<Profiles>().store.lock().unwrap().list.iter().filter(|p| p.id != me.user_id) {
        if let Some(s) = &p.session {
            out.push((p.id.clone(), Jf::new(&base, &s.device_id, Some(s))));
        }
    }
    out
}

/// Every profile there is, signed in or not, the owner first: (Jellyfin user
/// id, name). Empty when Jellyfin isn't set up. The clean-up needs all of
/// them, so a profile that lost its sign-in still holds back what it's watching.
pub fn all_ids(app: &AppHandle) -> Vec<(String, String)> {
    let Some(me) = owner(app) else { return vec![] };
    let mut out = vec![(me.user_id.clone(), me.user.clone())];
    for p in app.state::<Profiles>().store.lock().unwrap().list.iter().filter(|p| p.id != me.user_id) {
        out.push((p.id.clone(), p.name.clone()));
    }
    out
}

/// Every profile's Jellyfin token, for bridge.rs to keep out of what web pages get.
pub fn tokens(app: &AppHandle) -> Vec<String> {
    app.state::<Profiles>().store.lock().unwrap().list.iter().filter_map(|p| p.session.as_ref().map(|s| s.token.clone())).collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hash_pin(pin: &str, salt: &[u8]) -> String {
    let mut key = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<sha2::Sha256>(pin.as_bytes(), salt, 20_000, &mut key);
    hex(&key)
}

fn new_pin(pin: &str) -> Result<String, String> {
    if pin.len() != 4 || !pin.chars().all(|c| c.is_ascii_digit()) {
        return Err("A PIN is 4 digits.".into());
    }
    let mut salt = [0u8; 16];
    getrandom::getrandom(&mut salt).map_err(|_| "Couldn't make a PIN right now.".to_string())?;
    Ok(format!("{}:{}", hex(&salt), hash_pin(pin, &salt)))
}

fn pin_matches(stored: &str, pin: &str) -> bool {
    let Some((salt, hash)) = stored.split_once(':') else { return false };
    let salt: Vec<u8> = (0..salt.len() / 2).filter_map(|i| u8::from_str_radix(&salt[i * 2..i * 2 + 2], 16).ok()).collect();
    hash_pin(pin, &salt) == hash
}

/// Whether `pin` opens a profile whose PIN is `stored`, counting the try.
/// A profile without a PIN needs none.
fn pin_gate(tries: &Mutex<Tries>, id: &str, stored: Option<&str>, pin: Option<&str>, now: Instant) -> Result<(), String> {
    let Some(stored) = stored else { return Ok(()) };
    let Some(pin) = pin.filter(|p| !p.is_empty()) else { return Err("Enter this profile's PIN first.".into()) };
    tries.lock().unwrap().begin(id, now).map_err(wait_message)?;
    if !pin_matches(stored, pin) {
        return Err("Wrong PIN.".into());
    }
    tries.lock().unwrap().passed(id);
    Ok(())
}

fn check_pin(app: &AppHandle, p: &Profile, pin: Option<&str>) -> Result<(), String> {
    pin_gate(&app.state::<Profiles>().tries, &p.id, p.pin.as_deref(), pin, Instant::now())
}

/// Checks a profile's PIN and does nothing else (the editor asks for it
/// before it shows a locked profile's settings).
pub fn verify_pin(app: &AppHandle, id: &str, pin: &str) -> Result<(), String> {
    let me = owner(app).ok_or("Sign in to Jellyfin first.")?;
    let p = find(app, &me, id)?;
    check_pin(app, &p, Some(pin))
}

/// Whether `password` is the Chillarr login: the one picked in setup, which
/// is also the owner's Jellyfin account (login.rs), so Jellyfin checks it.
/// Nothing new is stored, and the sign-in made to check it is signed out.
async fn check_login(app: &AppHandle, me: &Session, password: &str) -> Result<(), String> {
    let profiles = app.state::<Profiles>();
    profiles.tries.lock().unwrap().begin("login", Instant::now()).map_err(wait_message)?;
    let base = app.state::<crate::fix::Core>().config(AppId::Jellyfin).ok_or("Jellyfin isn't installed")?.base_url;
    // A device of its own, so this sign-in can't replace the owner's.
    let device = format!("{}-check", app.state::<Media>().device_id);
    let wrong = || "That isn't your Chillarr login.".to_string();
    let session = match Jf::new(&base, &device, None).sign_in(&me.user, password).await {
        Ok(s) => s,
        Err(crate::jellyfin::JfError::Rejected) => return Err(wrong()),
        Err(e) => return Err(format!("Couldn't check it with Jellyfin: {e}")),
    };
    let _ = Jf::new(&base, &device, Some(&session)).post_json("/Sessions/Logout", &serde_json::json!({})).await;
    if session.user_id != me.user_id {
        return Err(wrong());
    }
    profiles.tries.lock().unwrap().passed("login");
    Ok(())
}

/// Profile colours are `#rrggbb`: they go into the page's styles as they are.
fn valid_color(c: &str) -> bool {
    c.len() == 7 && c.starts_with('#') && c[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// Each profile signs in as its own device, so Jellyfin keeps their sign-ins
/// apart (a new sign-in on the same device can replace the last one).
fn device_for(app: &AppHandle, user_id: &str) -> String {
    format!("{}-{}", app.state::<Media>().device_id, &user_id[..user_id.len().min(8)])
}

/// Makes the Jellyfin account and the profile for it.
pub async fn add(app: &AppHandle, name: &str, color: &str, pin: Option<String>) -> Result<View, String> {
    let name = name.trim();
    if name.is_empty() || name.len() > 30 {
        return Err("Give the profile a name (up to 30 letters).".into());
    }
    if !valid_color(color) {
        return Err("Pick one of the colours.".into());
    }
    let pin = match pin.filter(|p| !p.is_empty()) {
        Some(p) => Some(new_pin(&p)?),
        None => None,
    };
    let admin = jf_admin(app).filter(|j| j.user_id.is_some()).ok_or("Sign in to Jellyfin first (Settings > Jellyfin & streaming).")?;
    let mut raw = [0u8; 24];
    getrandom::getrandom(&mut raw).map_err(|_| "Couldn't make a password right now.".to_string())?;
    let password = hex(&raw);
    let id = admin.create_user(name, &password).await.map_err(|e| match e {
        crate::jellyfin::JfError::Other(m) if m.contains("400") => format!("Jellyfin already has an account called {name}. Pick another name."),
        e => format!("Jellyfin couldn't make the account: {e}"),
    })?;
    let base = app.state::<crate::fix::Core>().config(AppId::Jellyfin).ok_or("Jellyfin isn't installed")?.base_url;
    let session = Jf::new(&base, &device_for(app, &id), None).sign_in(name, &password).await.map_err(|e| format!("Made the account, but couldn't sign in to it: {e}"))?;
    let profiles = app.state::<Profiles>();
    profiles.store.lock().unwrap().list.push(Profile { id: id.clone(), name: name.into(), color: color.into(), pin, session: Some(session) });
    profiles.save();
    log(app, Level::Ok, format!("Added the profile {name} (a Jellyfin account of its own)."));
    list(app).into_iter().find(|v| v.id == id).ok_or_else(|| "The profile didn't save.".into())
}

/// Starts watching as this profile. A profile with a PIN needs it.
pub fn switch(app: &AppHandle, id: &str, pin: Option<String>) -> Result<(), String> {
    let me = owner(app).ok_or("Sign in to Jellyfin first.")?;
    let p = find(app, &me, id)?;
    check_pin(app, &p, pin.as_deref())?;
    let media = app.state::<Media>();
    if p.id == me.user_id {
        media.set_profile(None);
    } else {
        media.set_profile(Some(p.session.ok_or("That profile has no sign-in. Remove it and add it again.")?));
    }
    media.unlock();
    Ok(())
}

// ---------- inside Jellyfin's web pages (bridge.rs) ----------

/// The most profiles there can be once one is added from a browser.
const WEB_MAX_PROFILES: usize = 12;

#[derive(Serialize, Clone)]
pub struct WebView {
    #[serde(flatten)]
    pub view: View,
    /// The main (owner's) profile, on a device that hasn't proven its
    /// Jellyfin password yet: switching into it asks for that password once.
    pub needs_password: bool,
}

/// "Who's watching?" for a page signed in as `viewer` (a Jellyfin user id).
/// Empty unless that user is one of the profiles: someone else's Jellyfin
/// account doesn't get to pick the household's. `trusted`: this device has
/// typed the main profile's password before.
pub fn list_for(app: &AppHandle, viewer: &str, trusted: bool) -> Vec<WebView> {
    let all = list(app);
    if !all.iter().any(|v| v.id == viewer) {
        return vec![];
    }
    all.into_iter()
        .map(|mut v| {
            v.active = v.id == viewer;
            let needs_password = needs_password(v.owner, &v.id, viewer, trusted);
            WebView { view: v, needs_password }
        })
        .collect()
}

/// The user's rule: only the main profile asks for a password, only from
/// someone else's profile, and only on a device that hasn't typed it before.
fn needs_password(main: bool, id: &str, viewer: &str, trusted: bool) -> bool {
    main && id != viewer && !trusted
}

// ---------- devices that typed the main profile's password ----------
// The main profile is the owner's Jellyfin admin account, so a browser asks
// for its password the first time it switches into it (the user's call: only
// the main profile, only once per device). That device is then given a random
// key it keeps (localStorage) and sends back; Chillarr keeps only a hash.

/// Remembered devices at most; the oldest goes first.
const MAX_TRUSTED: usize = 50;
/// A device is asked again after this long.
const TRUST_DAYS: i64 = 180;
/// Held while the remembered devices are read and written back.
static TRUST_IO: Mutex<()> = Mutex::new(());

/// One remembered device: the hash of its key, and when (unix seconds).
#[derive(Serialize, Deserialize, Clone)]
struct Trust {
    hash: String,
    at: i64,
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn fresh(t: &Trust, now: i64) -> bool {
    now - t.at < TRUST_DAYS * 24 * 3600
}

fn trust_file(app: &AppHandle) -> Option<PathBuf> {
    crate::media::data_dir(app).map(|d| d.join("web-trust.json"))
}

fn trust_hash(key: &str) -> String {
    use sha2::Digest;
    hex(&sha2::Sha256::digest(key.as_bytes()))
}

fn trust_list(app: &AppHandle) -> Vec<Trust> {
    trust_file(app).and_then(|f| crate::seal::read(&f)).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save_trust(app: &AppHandle, list: &[Trust]) -> Result<(), String> {
    let f = trust_file(app).ok_or("Chillarr's data folder is missing.")?;
    crate::seal::write(&f, &serde_json::to_string(list).unwrap_or_default()).map_err(|e| e.to_string())
}

/// Forgets every browser that knows the main profile's password (Settings in
/// the Chillarr app: a lost phone, a lent laptop). Each asks again.
pub fn forget_web_devices(app: &AppHandle) -> Result<(), String> {
    let _io = TRUST_IO.lock().unwrap();
    save_trust(app, &[])?;
    log(app, Level::Ok, "Browsers that knew the main profile's password will ask for it again.");
    Ok(())
}

/// Whether `key` is one a device was given (the main profile's password once typed on it).
pub fn trusted(app: &AppHandle, key: Option<&str>) -> bool {
    let Some(key) = key.filter(|k| k.len() == 64 && k.bytes().all(|b| b.is_ascii_hexdigit())) else { return false };
    let want = trust_hash(key);
    let now = now_secs();
    trust_list(app).iter().any(|t| fresh(t, now) && crate::remote::ct_eq(t.hash.as_bytes(), want.as_bytes()))
}

/// A new key for a device that just typed the main profile's password.
fn trust_device(app: &AppHandle) -> Result<String, String> {
    let mut raw = [0u8; 32];
    getrandom::getrandom(&mut raw).map_err(|_| "Couldn't remember this device right now.".to_string())?;
    let key = hex(&raw);
    let _io = TRUST_IO.lock().unwrap();
    let now = now_secs();
    let mut list: Vec<Trust> = trust_list(app).into_iter().filter(|t| fresh(t, now)).collect();
    list.push(Trust { hash: trust_hash(&key), at: now });
    if list.len() > MAX_TRUSTED {
        list.drain(..list.len() - MAX_TRUSTED);
    }
    save_trust(app, &list)?;
    Ok(key)
}

/// Wrong tries from a browser are counted apart from the PC's, so someone on
/// the web can't make the people at the PC wait.
fn web_key(what: &str) -> String {
    format!("web:{what}")
}

/// How many wrong passwords Jellyfin allows an account before it disables it
/// (its policy: -1 never, 0 its default of 5 for admins and 3 for others).
fn lockout_after(policy: &serde_json::Value) -> Option<i64> {
    match policy["LoginAttemptsBeforeLockout"].as_i64().unwrap_or(-1) {
        n if n < 0 => None,
        0 => Some(if policy["IsAdministrator"] == true { 5 } else { 3 }),
        n => Some(n),
    }
}

/// Checks the main profile's Jellyfin password typed in a browser. A wrong
/// try that would make Jellyfin disable the account (which would also cut
/// Chillarr off) is refused before Jellyfin sees it.
async fn check_password_web(app: &AppHandle, id: &str, password: &str) -> Result<(), String> {
    let admin = jf_admin(app).filter(|j| j.user_id.is_some()).ok_or("Sign in to Jellyfin first.")?;
    let user = admin.get_json(&format!("/Users/{id}")).await.map_err(|e| format!("Couldn't check it with Jellyfin: {e}"))?;
    let name = user["Name"].as_str().unwrap_or_default().to_string();
    if let Some(limit) = lockout_after(&user["Policy"]) {
        if user["Policy"]["InvalidLoginAttemptCount"].as_i64().unwrap_or(0) + 1 >= limit {
            return Err(format!("Too many wrong passwords for {name}. Switch to it in the Chillarr app on the PC."));
        }
    }
    let profiles = app.state::<Profiles>();
    let key = web_key(&format!("login:{id}"));
    profiles.tries.lock().unwrap().begin(&key, Instant::now()).map_err(wait_message)?;
    let base = app.state::<crate::fix::Core>().config(AppId::Jellyfin).ok_or("Jellyfin isn't installed")?.base_url;
    let device = format!("{}-check", app.state::<Media>().device_id);
    let wrong = || format!("That isn't {name}'s Jellyfin password.");
    let session = match Jf::new(&base, &device, None).sign_in(&name, password).await {
        Ok(s) => s,
        Err(crate::jellyfin::JfError::Rejected) => return Err(wrong()),
        Err(e) => return Err(format!("Couldn't check it with Jellyfin: {e}")),
    };
    let _ = Jf::new(&base, &device, Some(&session)).post_json("/Sessions/Logout", &serde_json::json!({})).await;
    if session.user_id != id {
        return Err(wrong());
    }
    profiles.tries.lock().unwrap().passed(&key);
    Ok(())
}

/// Switching in a browser goes through Jellyfin's Quick Connect, which a
/// server can have off. Turned on (as the owner) the first time the picker is
/// asked for, so the switch never stops at "Quick Connect is off".
async fn ensure_quick_connect(app: &AppHandle) {
    static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if DONE.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let Some(j) = jf_admin(app).filter(|j| j.user_id.is_some()) else { return };
    match j.get_json("/QuickConnect/Enabled").await {
        Ok(serde_json::Value::Bool(true)) => {}
        Ok(_) => {
            let Ok(mut cfg) = j.get_json("/System/Configuration").await else { return };
            cfg["QuickConnectAvailable"] = serde_json::Value::Bool(true);
            match j.post_json("/System/Configuration", &cfg).await {
                Ok(_) => log(app, Level::Ok, "Turned on Jellyfin's Quick Connect, so profiles can be switched inside Jellyfin's pages."),
                Err(e) => {
                    log(app, Level::Amber, format!("Couldn't turn on Jellyfin's Quick Connect, so switching profiles inside Jellyfin's pages won't work: {e}"));
                    return;
                }
            }
        }
        Err(_) => return,
    }
    DONE.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// The web picker's list, with Quick Connect made ready for it first.
pub async fn list_for_web(app: &AppHandle, viewer: &str, trust: Option<&str>) -> Vec<WebView> {
    let out = list_for(app, viewer, trusted(app, trust));
    if !out.is_empty() {
        ensure_quick_connect(app).await;
    }
    out
}

/// Quick Connect codes are digits.
fn code_ok(code: &str) -> bool {
    (4..=12).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_digit())
}

/// Lets a browser signed in as `viewer` become profile `id`: after its PIN
/// (and, for the main profile on a device not yet trusted, its Jellyfin
/// password), Chillarr approves the Quick Connect `code` the page asked
/// Jellyfin for, as that profile. The page then collects its own sign-in for
/// its own device; no password or token leaves Chillarr. Some(key) when the
/// password was typed: the device keeps it so it isn't asked again.
pub async fn authorize_web(app: &AppHandle, viewer: &str, id: &str, pin: Option<String>, password: Option<String>, trust: Option<String>, code: &str) -> Result<Option<String>, String> {
    if !code_ok(code) {
        return Err("That sign-in code isn't right. Try again.".into());
    }
    let me = owner(app).ok_or("Sign in to Jellyfin first.")?;
    if !list(app).iter().any(|v| v.id == viewer) {
        return Err("Only Chillarr's profiles can switch here.".into());
    }
    let p = find(app, &me, id)?;
    // The page sends any error naming the PIN back to the PIN pad.
    pin_gate(&app.state::<Profiles>().tries, &web_key(&p.id), p.pin.as_deref(), pin.as_deref(), Instant::now())
        .map_err(|e| if e.contains("PIN") { e } else { e.replace("wrong tries", "wrong PINs") })?;
    // The main profile is the server's admin: a device types its Jellyfin
    // password the first time, then it's remembered.
    let mut new_trust = None;
    if p.id == me.user_id && p.id != viewer && !trusted(app, trust.as_deref()) {
        let password = password.filter(|p| !p.is_empty()).ok_or_else(|| format!("Type {}'s Jellyfin password.", p.name))?;
        check_password_web(app, &p.id, &password).await?;
        new_trust = Some(trust_device(app)?);
    }
    let session = if p.id == me.user_id { me.clone() } else { p.session.clone().ok_or("That profile has no sign-in. Remove it and add it again in the Chillarr app.")? };
    let base = app.state::<crate::fix::Core>().config(AppId::Jellyfin).ok_or("Jellyfin isn't installed")?.base_url;
    Jf::new(&base, &session.device_id, Some(&session))
        .post_json(&format!("/QuickConnect/Authorize?code={code}"), &serde_json::json!({}))
        .await
        .map_err(|e| format!("Jellyfin didn't take the switch: {e}"))?;
    Ok(new_trust)
}

/// Adds a profile from a browser signed in as one of the profiles.
pub async fn add_web(app: &AppHandle, viewer: &str, name: &str, color: &str, pin: Option<String>) -> Result<View, String> {
    let all = list(app);
    if !all.iter().any(|v| v.id == viewer) {
        return Err("Only Chillarr's profiles can add one here.".into());
    }
    if all.len() >= WEB_MAX_PROFILES {
        return Err(format!("That's {WEB_MAX_PROFILES} profiles already. Remove one in the Chillarr app first."));
    }
    let admin = jf_admin(app).filter(|j| j.user_id.is_some()).ok_or("Sign in to Jellyfin first.")?;
    let theirs = admin.get_json(&format!("/Users/{viewer}")).await.map_err(|e| format!("Couldn't read your Jellyfin account: {e}"))?;
    let made = add(app, name, color, pin).await?;
    // The new profile can see no more than whoever made it: a kids' account
    // can't make itself an unrestricted one.
    let copied = async {
        let mut policy = admin.get_json(&format!("/Users/{}", made.id)).await?["Policy"].take();
        restrict_like(&mut policy, &theirs["Policy"]);
        admin.post_json(&format!("/Users/{}/Policy", made.id), &policy).await
    }
    .await;
    if let Err(e) = copied {
        // Not left half made: without the limits it would be a way around them.
        let _ = admin.call_delete(&format!("/Users/{}", made.id)).await;
        let profiles = app.state::<Profiles>();
        profiles.store.lock().unwrap().list.retain(|p| p.id != made.id);
        profiles.save();
        return Err(format!("Couldn't give the new profile your limits, so it wasn't kept: {e}"));
    }
    Ok(made)
}

/// What a profile added from a browser takes from its maker's Jellyfin
/// account: the same libraries, ratings, tags and hours, never more rights.
fn restrict_like(policy: &mut serde_json::Value, maker: &serde_json::Value) {
    for k in ["MaxParentalRating", "MaxParentalSubRating", "EnableAllFolders", "EnabledFolders", "BlockedTags", "AllowedTags", "BlockUnratedItems", "AccessSchedules", "EnableAllChannels", "EnabledChannels", "BlockedMediaFolders", "BlockedChannels"] {
        if let Some(v) = maker.get(k) {
            policy[k] = v.clone();
        }
    }
    // Rights: the maker's, or none.
    for k in ["EnableLiveTvManagement", "EnableLiveTvAccess", "EnableRemoteAccess", "EnableContentDownloading", "EnableSharedDeviceControl", "EnableRemoteControlOfOtherUsers", "EnableCollectionManagement", "EnableSubtitleManagement", "EnableMediaConversion", "EnableSyncTranscoding", "EnablePublicSharing"] {
        policy[k] = serde_json::Value::Bool(maker[k] == true);
    }
    if let Some(v) = maker.get("SyncPlayAccess") {
        policy["SyncPlayAccess"] = v.clone();
    }
    policy["IsAdministrator"] = serde_json::Value::Bool(false);
    policy["EnableContentDeletion"] = serde_json::Value::Bool(maker["EnableContentDeletion"] == true && maker["IsAdministrator"] != true);
}

/// Renames, recolours, or sets or clears the PIN (`Some("")` clears it). A
/// profile with a PIN needs it (`current_pin`) for any of these.
pub async fn update(app: &AppHandle, id: &str, name: Option<String>, color: Option<String>, pin: Option<String>, current_pin: Option<String>) -> Result<(), String> {
    let me = owner(app).ok_or("Sign in to Jellyfin first.")?;
    let p = find(app, &me, id)?;
    check_pin(app, &p, current_pin.as_deref())?;
    if color.as_deref().is_some_and(|c| !valid_color(c)) {
        return Err("Pick one of the colours.".into());
    }
    let new_pin = match pin {
        Some(p) if p.is_empty() => Some(None),
        Some(p) => Some(Some(new_pin(&p)?)),
        None => None,
    };
    let name = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty() && n.len() <= 30);
    // The owner's account name is its Jellyfin login, so only the profile's label changes.
    if let (Some(n), true) = (&name, id != me.user_id) {
        let admin = jf_admin(app).ok_or("Jellyfin isn't installed")?;
        admin.rename_user(id, n).await.map_err(|e| format!("Jellyfin couldn't rename it: {e}"))?;
    }
    let profiles = app.state::<Profiles>();
    {
        let mut s = profiles.store.lock().unwrap();
        let p = s.list.iter_mut().find(|p| p.id == id).ok_or("That profile is gone.")?;
        if let Some(n) = name {
            p.name = n;
        }
        if let Some(c) = color {
            p.color = c;
        }
        if let Some(pin) = new_pin {
            p.pin = pin;
        }
    }
    profiles.save();
    Ok(())
}

/// Gives a profile a password of your choosing, so its person can sign in to
/// Jellyfin's own apps (and join a watch party) from their devices. Not the
/// owner, whose password is their own. Setting it signs the account out, so
/// Chillarr signs it back in with the new one. True when Jellyfin's sign-in
/// screens list the profile now. A profile with a PIN needs it.
pub async fn set_password(app: &AppHandle, id: &str, password: &str, current_pin: Option<String>) -> Result<bool, String> {
    let me = owner(app).ok_or("Sign in to Jellyfin first.")?;
    if id == me.user_id {
        return Err("The main profile uses your own Jellyfin password.".into());
    }
    let p = find(app, &me, id)?;
    check_pin(app, &p, current_pin.as_deref())?;
    if password.len() < 5 {
        return Err("Use at least 5 characters.".into());
    }
    let name = p.name;
    let admin = jf_admin(app).ok_or("Jellyfin isn't installed")?;
    admin.set_password(id, password).await.map_err(|e| format!("Jellyfin couldn't set it: {e}"))?;
    // With a password they know, the profile can be picked on Jellyfin's TV
    // and phone sign-in screens (a new account is hidden there).
    let listed = match admin.set_hidden(id, false).await {
        Ok(()) => true,
        Err(e) => {
            log(app, Level::Amber, format!("{name}'s password is set, but Jellyfin's sign-in screens don't list them: {e}"));
            false
        }
    };
    let base = app.state::<crate::fix::Core>().config(AppId::Jellyfin).ok_or("Jellyfin isn't installed")?.base_url;
    let session = Jf::new(&base, &device_for(app, id), None).sign_in(&name, password).await.map_err(|e| format!("Set it, but couldn't sign back in: {e}"))?;
    let media = app.state::<Media>();
    if media.profile.lock().unwrap().as_ref().is_some_and(|p| p.user_id == id) {
        *media.profile.lock().unwrap() = Some(session.clone());
    }
    let profiles = app.state::<Profiles>();
    if let Some(p) = profiles.store.lock().unwrap().list.iter_mut().find(|p| p.id == id) {
        p.session = Some(session);
    }
    profiles.save();
    log(app, Level::Ok, format!("{name} can now sign in to Jellyfin's apps with their own password."));
    Ok(listed)
}

/// Removes a profile and its Jellyfin account, history included. Not the
/// owner. A profile with a PIN needs it, or the Chillarr login (`login`) for
/// when the PIN is forgotten.
pub async fn remove(app: &AppHandle, id: &str, pin: Option<String>, login: Option<String>) -> Result<(), String> {
    let me = owner(app).ok_or("Sign in to Jellyfin first.")?;
    if id == me.user_id {
        return Err("The main profile stays.".into());
    }
    let p = find(app, &me, id)?;
    match login.filter(|l| !l.is_empty()) {
        Some(password) if p.pin.is_some() => check_login(app, &me, &password).await?,
        _ => check_pin(app, &p, pin.as_deref())?,
    }
    let admin = jf_admin(app).ok_or("Jellyfin isn't installed")?;
    match admin.delete_path(&format!("/Users/{id}")).await {
        Ok(()) => {}
        // Already gone from Jellyfin: just forget it here too.
        Err(crate::jellyfin::JfError::Other(m)) if m.contains("404") => {}
        Err(e) => return Err(format!("Jellyfin couldn't remove the account: {e}")),
    }
    let media = app.state::<Media>();
    if media.profile.lock().unwrap().as_ref().is_some_and(|p| p.user_id == id) {
        media.set_profile(None);
    }
    let _ = std::fs::remove_file(app.path().app_data_dir().map(|d| d.join(format!("saved-{id}.json"))).unwrap_or_default());
    let _ = std::fs::remove_file(app.path().app_data_dir().map(|d| crate::history::file(&d, id)).unwrap_or_default());
    let gone = app.state::<crate::watchnow::WatchNow>().forget(id);
    crate::watchnow::drop_placeholders(app, &gone).await;
    let _ = std::fs::remove_file(app.path().app_data_dir().map(|d| crate::completed::hidden_file(&d, id)).unwrap_or_default());
    let _ = std::fs::remove_file(app.path().app_data_dir().map(|d| crate::marker::file(&d, id)).unwrap_or_default());
    let _ = std::fs::remove_file(app.path().app_data_dir().map(|d| crate::marker::file(&d, id).with_extension("json.bad")).unwrap_or_default());
    let profiles = app.state::<Profiles>();
    let name = {
        let mut s = profiles.store.lock().unwrap();
        let name = s.list.iter().find(|p| p.id == id).map(|p| p.name.clone()).unwrap_or_default();
        s.list.retain(|p| p.id != id);
        name
    };
    profiles.save();
    log(app, Level::Ok, format!("Removed the profile {name}."));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_web_profile_never_gets_more_than_its_maker() {
        let maker = serde_json::json!({ "MaxParentalRating": 10, "EnableAllFolders": false, "EnabledFolders": ["kids"], "BlockUnratedItems": ["Movie"], "IsAdministrator": false, "EnableContentDeletion": false });
        let mut p = serde_json::json!({ "MaxParentalRating": null, "EnableAllFolders": true, "EnabledFolders": [], "IsAdministrator": true, "EnableContentDeletion": true, "IsHidden": true });
        restrict_like(&mut p, &maker);
        assert_eq!(p["MaxParentalRating"], 10);
        assert_eq!(p["EnableAllFolders"], false);
        assert_eq!(p["EnabledFolders"], serde_json::json!(["kids"]));
        assert_eq!(p["IsAdministrator"], false);
        assert_eq!(p["EnableContentDeletion"], false);
        assert_eq!(p["IsHidden"], true, "the rest of Jellyfin's defaults stay");
        // An admin's new profile is an ordinary one.
        let mut q = serde_json::json!({});
        restrict_like(&mut q, &serde_json::json!({ "IsAdministrator": true, "EnableContentDeletion": true, "EnableAllFolders": true }));
        assert_eq!(q["IsAdministrator"], false);
        assert_eq!(q["EnableContentDeletion"], false);
    }

    #[test]
    fn jellyfins_lockout_counts_are_read_as_jellyfin_means_them() {
        assert_eq!(lockout_after(&serde_json::json!({ "LoginAttemptsBeforeLockout": -1 })), None);
        assert_eq!(lockout_after(&serde_json::json!({ "LoginAttemptsBeforeLockout": 0, "IsAdministrator": true })), Some(5));
        assert_eq!(lockout_after(&serde_json::json!({ "LoginAttemptsBeforeLockout": 0 })), Some(3));
        assert_eq!(lockout_after(&serde_json::json!({ "LoginAttemptsBeforeLockout": 7 })), Some(7));
    }

    #[test]
    fn web_pin_errors_name_the_pin() {
        let tries = Mutex::new(Tries::default());
        let stored = {
            let salt = [7u8; 16];
            format!("{}:{}", hex(&salt), hash_pin("1234", &salt))
        };
        let now = Instant::now();
        let mut last = String::new();
        for _ in 0..(FREE_TRIES + 1) {
            if let Err(e) = pin_gate(&tries, &web_key("p"), Some(&stored), Some("9999"), now) {
                last = if e.contains("PIN") { e } else { e.replace("wrong tries", "wrong PINs") };
            }
        }
        assert!(last.contains("PIN"), "{last}");
        assert!(pin_gate(&tries, "p", Some(&stored), Some("1234"), now).is_ok(), "the PC's own tries are separate");
    }

    #[test]
    fn only_a_real_device_key_is_trusted_shape() {
        assert_eq!(trust_hash("a").len(), 64);
        assert_ne!(trust_hash("a"), trust_hash("b"));
        let now = 1_000_000_000;
        assert!(fresh(&Trust { hash: String::new(), at: now - 10 }, now));
        assert!(!fresh(&Trust { hash: String::new(), at: now - TRUST_DAYS * 24 * 3600 }, now), "asked again after 180 days");
    }

    #[test]
    fn only_the_main_profile_asks_for_a_password_and_only_once() {
        assert!(needs_password(true, "owner", "kid", false), "main profile, new device: asks");
        assert!(!needs_password(true, "owner", "kid", true), "main profile, remembered device: one tap");
        assert!(!needs_password(true, "owner", "owner", false), "already the main profile");
        assert!(!needs_password(false, "kid", "owner", false), "any other profile never asks (its PIN still does)");
    }

    #[test]
    fn quick_connect_codes_are_digits_only() {
        assert!(code_ok("529979"));
        for bad in ["", "12", "52997a", "529979&userId=x", "1234567890123"] {
            assert!(!code_ok(bad), "{bad}");
        }
    }

    #[test]
    fn a_pin_checks_out_only_with_the_right_digits() {
        let stored = new_pin("4821").unwrap();
        assert!(pin_matches(&stored, "4821"));
        assert!(!pin_matches(&stored, "4822"));
        assert!(!stored.contains("4821"), "only a hash is kept");
        assert_ne!(new_pin("4821").unwrap(), stored, "salted");
        assert!(new_pin("12a4").is_err() && new_pin("123").is_err());
    }

    #[test]
    fn a_locked_profile_needs_its_pin_and_an_open_one_needs_none() {
        let tries = Mutex::new(Tries::default());
        let stored = new_pin("4821").unwrap();
        let now = Instant::now();
        assert!(pin_gate(&tries, "a", None, None, now).is_ok(), "no PIN: nothing asked");
        assert!(pin_gate(&tries, "a", Some(&stored), None, now).is_err(), "no PIN given");
        assert!(pin_gate(&tries, "a", Some(&stored), Some(""), now).is_err(), "an empty PIN isn't one");
        assert_eq!(pin_gate(&tries, "a", Some(&stored), Some("0000"), now), Err("Wrong PIN.".into()));
        assert!(pin_gate(&tries, "a", Some(&stored), Some("4821"), now).is_ok());
        assert!(tries.lock().unwrap().0.is_empty(), "the right PIN clears the count");
    }

    #[test]
    fn wrong_tries_make_you_wait_longer_each_time() {
        let tries = Mutex::new(Tries::default());
        let stored = new_pin("4821").unwrap();
        let t0 = Instant::now();
        for _ in 0..FREE_TRIES {
            assert_eq!(pin_gate(&tries, "a", Some(&stored), Some("0000"), t0), Err("Wrong PIN.".into()));
        }
        // Even the right PIN waits now; another profile doesn't.
        assert!(pin_gate(&tries, "a", Some(&stored), Some("4821"), t0).unwrap_err().contains("30 seconds"));
        assert!(pin_gate(&tries, "b", Some(&stored), Some("4821"), t0).is_ok());
        // After the wait, one more wrong try waits twice as long.
        let t1 = t0 + FIRST_WAIT;
        assert_eq!(pin_gate(&tries, "a", Some(&stored), Some("0000"), t1), Err("Wrong PIN.".into()));
        assert!(pin_gate(&tries, "a", Some(&stored), Some("4821"), t1).unwrap_err().contains("60 seconds"));
        // The right PIN after the wait starts the count over.
        let t2 = t1 + FIRST_WAIT * 2;
        assert!(pin_gate(&tries, "a", Some(&stored), Some("4821"), t2).is_ok());
        assert!(tries.lock().unwrap().0.get("a").is_none());
        // The wait stops doubling at 16 minutes.
        let mut t = Tries::default();
        let mut now = t0;
        for _ in 0..40 {
            now += Duration::from_secs(3600);
            t.begin("x", now).unwrap();
        }
        assert_eq!(t.begin("x", now), Err(FIRST_WAIT.as_secs() << MOST_DOUBLINGS));
        assert_eq!(wait_message(960), "Too many wrong tries. Try again in 16 minutes.");
    }

    #[test]
    fn only_listed_ids_are_found() {
        let list = vec![Profile { id: "0f8fad5bd9cb469fa16570867728950e".into(), ..Default::default() }];
        assert!(find_in(&list, "0f8fad5bd9cb469fa16570867728950e").is_some());
        for bad in [r"..\..\settings", "../profiles", "", "0f8fad5b"] {
            assert!(find_in(&list, bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn colours_are_plain_hex() {
        assert!(valid_color("#e50914") && valid_color("#ABCDEF") && valid_color(OWNER_COLOR));
        for bad in ["red", "#fff", "#e5091", "#e509144", "#e5091g", "#e50914;background:url(x)", "e509144", "", "#e5091\u{e9}"] {
            assert!(!valid_color(bad), "{bad}");
        }
    }
}
