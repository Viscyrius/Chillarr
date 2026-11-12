//! Hidden: each profile can hide films, series, anime, manga and books from
//! every page. Hidden things never show on that profile's pages: not in rows,
//! Continue watching, My List, search, the reading tabs or Your week. They
//! live in the Hidden tab only, behind a password of the profile's own (not
//! its PIN). The PC's main profile can reset that password (list kept).
//!
//! Unlocking only opens the Hidden tab, on that one device, for UNLOCK_FOR,
//! and looking doesn't make it last longer. It used to show every hidden
//! title on every page, on every device of the profile, for an hour that
//! started again at each check; setting the password unlocked it too, so the
//! first hide looked like it did nothing (#80).
//!
//! What's hidden is a list of the titles' ids ("tmdb:603", "tvdb:81189",
//! "jf:<id>", "al:<AniList id>", "r:<reading key>"): a title matches when any
//! of its ids is in the list, so an episode is hidden with its series. The
//! list and the password's hash are sealed for this Windows user (seal.rs),
//! one file per profile.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

/// How long an unlock lasts, from the moment it's typed.
const UNLOCK_FOR: Duration = Duration::from_secs(15 * 60);
/// Ids kept per profile.
const MAX_KEYS: usize = 5000;
const MIN_PASSWORD: usize = 4;
const ROUNDS: u32 = 100_000;

#[derive(Serialize, Deserialize, Default, Clone)]
struct Saved {
    /// `salt:hash` (hex, PBKDF2-SHA256).
    password: Option<String>,
    keys: Vec<String>,
    /// The name each hidden title is listed by in Settings (by its first id).
    #[serde(default)]
    labels: HashMap<String, String>,
    /// Every id hidden together with that first one, so showing it again by
    /// name brings back all of them.
    #[serde(default)]
    groups: HashMap<String, Vec<String>>,
    /// The card each hidden title was hidden from (by its first id), so the
    /// unlocked list can open its page. Sealed with the rest.
    #[serde(default)]
    items: HashMap<String, serde_json::Value>,
}

/// A card kept for the hidden list: at most this much JSON.
const MAX_ITEM: usize = 16 * 1024;

/// Which (profile, device) has the Hidden tab open, and until when.
#[derive(Default)]
pub struct Unlocks(HashMap<(String, String), Instant>);

impl Unlocks {
    pub fn open(&mut self, uid: &str, device: &str, now: Instant) {
        self.0.insert((uid.to_string(), device.to_string()), now + UNLOCK_FOR);
    }
    pub fn is_open(&mut self, uid: &str, device: &str, now: Instant) -> bool {
        let k = (uid.to_string(), device.to_string());
        match self.0.get(&k) {
            Some(until) if *until > now => true,
            Some(_) => {
                self.0.remove(&k);
                false
            }
            None => false,
        }
    }
    pub fn close(&mut self, uid: &str, device: &str) {
        self.0.remove(&(uid.to_string(), device.to_string()));
    }
    pub fn clear_profile(&mut self, uid: &str) {
        self.0.retain(|(u, _), _| u != uid);
    }
}

/// The device asking: a web page's Jellyfin device id, or the PC window.
fn device() -> String {
    crate::viewer::current().map(|v| v.session.device_id).unwrap_or_else(|| "pc".into())
}

/// What a profile's pages hide: everything on its list, always.
fn to_hide(s: &Saved) -> Vec<String> {
    s.keys.clone()
}

#[derive(Default)]
pub struct Hidden {
    open: Mutex<Unlocks>,
    /// Profile -> (wrong tries in a row, wait until).
    tries: Mutex<HashMap<String, (u32, Option<Instant>)>>,
    io: Mutex<()>,
}

fn uid(app: &AppHandle) -> Result<String, String> {
    crate::media::active_user(app).ok_or_else(|| "Sign in to Jellyfin first.".to_string())
}

fn file(app: &AppHandle, uid: &str) -> Result<PathBuf, String> {
    let safe: String = uid.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    Ok(app.path().app_data_dir().map_err(|e| e.to_string())?.join(format!("hidden-{safe}.json")))
}

fn load(app: &AppHandle, uid: &str) -> Saved {
    file(app, uid).ok().and_then(|p| crate::seal::read(&p)).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save(app: &AppHandle, uid: &str, s: &Saved) -> Result<(), String> {
    let text = serde_json::to_string(s).map_err(|e| e.to_string())?;
    crate::seal::write(&file(app, uid)?, &text).map_err(|e| format!("Couldn't save it: {e}"))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn hash(pw: &str, salt: &[u8]) -> String {
    let mut key = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<sha2::Sha256>(pw.as_bytes(), salt, ROUNDS, &mut key);
    hex(&key)
}

fn new_hash(pw: &str) -> Result<String, String> {
    let mut salt = [0u8; 16];
    getrandom::getrandom(&mut salt).map_err(|_| "Couldn't make a password right now.".to_string())?;
    Ok(format!("{}:{}", hex(&salt), hash(pw, &salt)))
}

pub fn matches(stored: &str, pw: &str) -> bool {
    let Some((salt, h)) = stored.split_once(':') else { return false };
    let salt: Vec<u8> = (0..salt.len() / 2).filter_map(|i| u8::from_str_radix(&salt[i * 2..i * 2 + 2], 16).ok()).collect();
    // Same length, every byte compared: no early way out to time.
    let got = hash(pw, &salt);
    got.len() == h.len() && got.bytes().zip(h.bytes()).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0
}

/// Only plain ids of the shapes above, cut short.
fn clean_key(k: &str) -> Option<String> {
    let (kind, id) = k.split_once(':')?;
    let ok_kind = matches!(kind, "tmdb" | "tmdbm" | "tmdbtv" | "tvdb" | "jf" | "al" | "ol" | "r");
    let ok_id = !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    (ok_kind && ok_id).then(|| format!("{kind}:{id}"))
}

fn is_open(app: &AppHandle, uid: &str) -> bool {
    app.state::<Hidden>().open.lock().unwrap().is_open(uid, &device(), Instant::now())
}

/// Counts a try: Err while waiting after too many wrong ones.
fn gate(app: &AppHandle, uid: &str, ok: impl FnOnce() -> bool) -> Result<(), String> {
    let h = app.state::<Hidden>();
    if let Some((_, Some(until))) = h.tries.lock().unwrap().get(uid) {
        if *until > Instant::now() {
            return Err(format!("Too many wrong tries. Wait {} seconds.", (*until - Instant::now()).as_secs() + 1));
        }
    }
    if ok() {
        h.tries.lock().unwrap().remove(uid);
        return Ok(());
    }
    let mut t = h.tries.lock().unwrap();
    let e = t.entry(uid.to_string()).or_insert((0, None));
    e.0 += 1;
    // 3 free tries, then 30 s, 60 s, 120 s... up to 30 minutes.
    if e.0 >= 3 {
        let wait = 30u64.saturating_mul(1 << (e.0 - 3).min(6)).min(1800);
        e.1 = Some(Instant::now() + Duration::from_secs(wait));
    }
    Err("That's not the hidden password.".into())
}

// ---------- commands ----------

#[derive(Serialize)]
pub struct Status {
    pub has_password: bool,
    pub unlocked: bool,
    pub count: usize,
    /// What to hide: the whole list, unlocked or not (only the Hidden tab shows them).
    pub keys: Vec<String>,
}

pub fn status(app: &AppHandle) -> Result<Status, String> {
    let uid = uid(app)?;
    let s = load(app, &uid);
    let unlocked = s.password.is_some() && is_open(app, &uid);
    Ok(Status { has_password: s.password.is_some(), unlocked, count: if s.labels.is_empty() { s.keys.len() } else { s.labels.len() }, keys: to_hide(&s) })
}

/// Sets the password the first time, or changes it (the old one needed).
pub fn set_password(app: &AppHandle, password: &str, old: Option<&str>) -> Result<(), String> {
    let uid = uid(app)?;
    let state = app.state::<Hidden>();
    let _io = state.io.lock().unwrap();
    let mut s = load(app, &uid);
    if let Some(stored) = s.password.clone() {
        gate(app, &uid, || old.is_some_and(|o| matches(&stored, o)))?;
    }
    if password.chars().count() < MIN_PASSWORD {
        return Err(format!("Use at least {MIN_PASSWORD} characters."));
    }
    if password.chars().count() > 200 {
        return Err("That's too long.".into());
    }
    if crate::profiles::is_pin_of(app, &uid, password) {
        return Err("Pick a password that isn't this profile's PIN.".into());
    }
    s.password = Some(new_hash(password)?);
    save(app, &uid, &s)
}

pub fn unlock(app: &AppHandle, password: &str) -> Result<(), String> {
    let uid = uid(app)?;
    let s = load(app, &uid);
    let stored = s.password.ok_or("Set a hidden password first.")?;
    gate(app, &uid, || matches(&stored, password))?;
    app.state::<Hidden>().open.lock().unwrap().open(&uid, &device(), Instant::now());
    Ok(())
}

pub fn lock(app: &AppHandle) -> Result<(), String> {
    let uid = uid(app)?;
    app.state::<Hidden>().open.lock().unwrap().close(&uid, &device());
    Ok(())
}

/// The main profile, at the PC: forgets a profile's hidden password and keeps
/// its list, so it can make a new one. That profile is locked everywhere.
pub fn reset_password(app: &AppHandle, profile: &str) -> Result<(), String> {
    let state = app.state::<Hidden>();
    let _io = state.io.lock().unwrap();
    let mut s = load(app, profile);
    s.password = None;
    save(app, profile, &s)?;
    state.open.lock().unwrap().clear_profile(profile);
    state.tries.lock().unwrap().remove(profile);
    Ok(())
}

/// A removed profile's hidden list goes with it.
pub fn forget_profile(app: &AppHandle, profile: &str) {
    if let Ok(p) = file(app, profile) {
        let _ = std::fs::remove_file(p);
    }
    app.state::<Hidden>().open.lock().unwrap().clear_profile(profile);
}

/// Hides a title (all its ids) or shows it again. Showing again needs it unlocked.
pub fn set(app: &AppHandle, keys: &[String], hide: bool, label: Option<&str>, item: Option<serde_json::Value>) -> Result<bool, String> {
    let uid = uid(app)?;
    let state = app.state::<Hidden>();
    let _io = state.io.lock().unwrap();
    let mut s = load(app, &uid);
    if s.password.is_none() {
        return Err("Set a hidden password first.".into());
    }
    if !hide && !is_open(app, &uid) {
        return Err("Unlock hidden things first.".into());
    }
    let keys: Vec<String> = keys.iter().filter_map(|k| clean_key(k)).take(16).collect();
    if keys.is_empty() {
        return Err("That can't be hidden.".into());
    }
    if hide {
        if let Some(l) = label.map(|l| l.chars().take(200).collect::<String>()).filter(|l| !l.trim().is_empty()) {
            s.labels.insert(keys[0].clone(), l);
        }
        if let Some(item) = item.filter(|v| v.is_object() && serde_json::to_string(v).map_or(false, |t| t.len() <= MAX_ITEM)) {
            s.items.insert(keys[0].clone(), item);
        }
        s.groups.insert(keys[0].clone(), keys.clone());
        for k in keys {
            if !s.keys.contains(&k) {
                s.keys.push(k);
            }
        }
        if s.keys.len() > MAX_KEYS {
            return Err("Too many hidden things. Show some again first.".into());
        }
    } else {
        let mut all = keys.clone();
        for k in &keys {
            if let Some(g) = s.groups.get(k) {
                all.extend(g.iter().cloned());
            }
        }
        // Any group sharing an id goes too.
        let firsts: Vec<String> = s.groups.iter().filter(|(_, g)| g.iter().any(|x| all.contains(x))).map(|(k, _)| k.clone()).collect();
        for f in &firsts {
            if let Some(g) = s.groups.remove(f) {
                all.extend(g);
            }
            s.labels.remove(f);
            s.items.remove(f);
        }
        s.keys.retain(|k| !all.contains(k));
    }
    save(app, &uid, &s)?;
    Ok(hide)
}

#[derive(Serialize)]
pub struct Entry {
    pub key: String,
    pub label: String,
    /// The card it was hidden from, to open its page (None for ones hidden before 0.19.1).
    pub item: Option<serde_json::Value>,
}

/// Everything hidden by name, to show again one by one (only while unlocked).
/// Ids saved with a name are listed once, under that name.
pub fn list(app: &AppHandle) -> Result<Vec<Entry>, String> {
    let uid = uid(app)?;
    if !is_open(app, &uid) {
        return Err("Unlock hidden things first.".into());
    }
    let s = load(app, &uid);
    let mut out: Vec<Entry> = s.labels.iter().map(|(k, l)| Entry { key: k.clone(), label: l.clone(), item: s.items.get(k).cloned() }).collect();
    out.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlock_is_per_device() {
        let mut u = Unlocks::default();
        let now = Instant::now();
        u.open("aria", "phone", now);
        assert!(u.is_open("aria", "phone", now));
        assert!(!u.is_open("aria", "pc", now), "another device stays locked");
        assert!(!u.is_open("giveta", "phone", now), "another profile stays locked");
    }

    #[test]
    fn unlock_expires_fixed() {
        let mut u = Unlocks::default();
        let t0 = Instant::now();
        u.open("aria", "phone", t0);
        // Looking doesn't make it last longer.
        for m in 1..15 {
            assert!(u.is_open("aria", "phone", t0 + Duration::from_secs(m * 60)));
        }
        assert!(!u.is_open("aria", "phone", t0 + UNLOCK_FOR + Duration::from_secs(1)));
    }

    #[test]
    fn reset_locks_every_device_of_that_profile() {
        let mut u = Unlocks::default();
        let now = Instant::now();
        u.open("aria", "phone", now);
        u.open("aria", "pc", now);
        u.open("giveta", "pc", now);
        u.clear_profile("aria");
        assert!(!u.is_open("aria", "phone", now) && !u.is_open("aria", "pc", now));
        assert!(u.is_open("giveta", "pc", now));
    }

    #[test]
    fn typed_tmdb_keys_are_kept() {
        assert_eq!(clean_key("tmdbm:603").as_deref(), Some("tmdbm:603"));
        assert_eq!(clean_key("tmdbtv:1399").as_deref(), Some("tmdbtv:1399"));
    }

    #[test]
    fn what_to_hide_is_the_whole_list_unlocked_or_not() {
        let s = Saved { password: Some("x".into()), keys: vec!["tmdbm:1".into()], ..Default::default() };
        assert_eq!(to_hide(&s), vec!["tmdbm:1".to_string()]);
    }

    #[test]
    fn a_password_matches_only_itself() {
        let h = new_hash("tulip-42").unwrap();
        assert!(matches(&h, "tulip-42"));
        assert!(!matches(&h, "tulip-43"));
        assert!(!matches("junk", "tulip-42"));
    }

    #[test]
    fn only_plain_ids_are_kept() {
        assert_eq!(clean_key("tmdb:603").as_deref(), Some("tmdb:603"));
        assert_eq!(clean_key("r:m12").as_deref(), Some("r:m12"));
        assert_eq!(clean_key("jf:4f2a-b_9").as_deref(), Some("jf:4f2a-b_9"));
        assert!(clean_key("x:1").is_none());
        assert!(clean_key("tmdb:").is_none());
        assert!(clean_key("tmdb:1/../2").is_none());
        assert!(clean_key("nocolon").is_none());
    }
}
