//! Watch together: Jellyfin's SyncPlay. The player side (syncplay.js) holds
//! the WebSocket and keeps the video in step; this side makes its REST calls
//! as the profile watching, from the same Jellyfin session as that socket
//! (same token and device id), which SyncPlay needs to know who is who.
//!
//! Measured on Jellyfin 12.1: the socket signs in with `?ApiKey=` (the old
//! `api_key=` is refused), and messages only reach it when the REST calls use
//! the token's own device id.
//!
//! Chat in a watch party (`say`): SyncPlay has no messages of its own, so
//! Chillarr carries them. A message goes to everyone in the sender's group,
//! as Jellyfin lists it right then: the window (a Tauri event) when its
//! profile is in the party, and every web page of the others (the bridge's
//! event stream, which each page already has open). Nothing is kept: a
//! message is for whoever is watching now. Jellyfin's own apps don't show it.

use crate::media::{jf, Media};
use crate::tiles::{self, Tile};
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

#[derive(Serialize)]
pub struct Info {
    /// `ws://.../socket?ApiKey=...&deviceId=...`
    pub socket: String,
    pub user: String,
}

/// Where the player's SyncPlay socket connects, as whoever is watching.
pub fn info(app: &AppHandle) -> Result<Info, String> {
    let j = jf(app).filter(|j| j.user_id.is_some()).ok_or("Sign in to Jellyfin first (Settings > Account & devices).")?;
    let (token, device) = j.credentials().ok_or("Sign in to Jellyfin first.")?;
    let ws = j.base.replacen("https://", "wss://", 1).replacen("http://", "ws://", 1);
    Ok(Info { socket: format!("{ws}/socket?ApiKey={token}&deviceId={}", enc(&device)), user: who(app) })
}

/// The Jellyfin user name of whoever is watching: the web page's viewer, or
/// the window's profile (the owner when none is picked).
fn who(app: &AppHandle) -> String {
    if let Some(v) = crate::viewer::current() {
        return v.session.user;
    }
    window_user(app).unwrap_or_default()
}

/// The window's profile name (the owner when none is picked). None while locked.
fn window_user(app: &AppHandle) -> Option<String> {
    let media = app.state::<Media>();
    if media.is_locked() {
        return None;
    }
    let profile = media.profile.lock().unwrap().as_ref().map(|p| p.user.clone());
    profile.or_else(|| media.session.lock().unwrap().as_ref().map(|s| s.user.clone()))
}

fn enc(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// Only SyncPlay's own endpoints and the server clock go through here. A
/// SyncPlay call is one plain word (`/SyncPlay/Join`): no dots, slashes,
/// `%`, `?`, `#` or `\`, so URL normalising can't turn it into another path
/// (`/SyncPlay/%2e%2e/Users/New` reached /Users/New).
fn allowed(path: &str) -> bool {
    path == "/GetUtcTime"
        || path == "/Sessions/Capabilities/Full"
        || path.strip_prefix("/SyncPlay/").is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// One SyncPlay REST call. Empty answers come back as null.
pub async fn call(app: &AppHandle, method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
    if !allowed(path) {
        return Err("Not a SyncPlay call.".into());
    }
    let j = jf(app).filter(|j| j.user_id.is_some()).ok_or("Sign in to Jellyfin first.")?;
    let r = match method {
        "GET" => j.get_json(path).await,
        _ => j.post_json(path, &body.unwrap_or(Value::Object(Default::default()))).await,
    };
    r.map_err(|e| e.to_string())
}

/// A playable tile for a Jellyfin item, for when the group starts something.
pub async fn tile(app: &AppHandle, id: &str) -> Option<Tile> {
    let j = jf(app).filter(|j| j.user_id.is_some())?;
    let v = j.items(&format!("ids={id}")).await.ok()?;
    v.first().map(|v| tiles::from_jf(v, &j.base))
}

// ---------- chat ----------

/// The longest chat message, in characters.
pub const SAY_MAX: usize = 300;
/// Messages one person can send in `SAY_WINDOW`.
const SAY_RATE: usize = 6;
const SAY_WINDOW: Duration = Duration::from_secs(10);

/// One chat message, as every page in the party gets it ("together-chat").
#[derive(Serialize, Clone, Debug)]
pub struct Said {
    pub group: String,
    pub id: String,
    pub from: String,
    pub text: String,
    /// Unix milliseconds.
    pub at: i64,
}

/// The people in `group` by Jellyfin's list (/SyncPlay/List), if `me` is one of them.
fn members(list: &Value, group: &str, me: &str) -> Option<Vec<String>> {
    let g = list.as_array()?.iter().find(|g| g["GroupId"].as_str() == Some(group))?;
    let people: Vec<String> = g["Participants"].as_array()?.iter().filter_map(|p| p.as_str().map(String::from)).collect();
    people.iter().any(|p| p.eq_ignore_ascii_case(me)).then_some(people)
}

/// The text to send, trimmed and on one line, or why not.
fn say_text(text: &str) -> Result<String, String> {
    let t = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.is_empty() {
        return Err("Type a message first.".into());
    }
    if t.chars().count() > SAY_MAX {
        return Err(format!("Keep it under {SAY_MAX} characters."));
    }
    Ok(t)
}

/// Counts one message from `who`. Err: seconds to wait.
fn take_turn(sent: &mut HashMap<String, VecDeque<Instant>>, who: &str, now: Instant) -> Result<(), u64> {
    let q = sent.entry(who.to_lowercase()).or_default();
    while q.front().is_some_and(|t| now.duration_since(*t) >= SAY_WINDOW) {
        q.pop_front();
    }
    if q.len() >= SAY_RATE {
        return Err((SAY_WINDOW - now.duration_since(q[0])).as_secs().max(1));
    }
    q.push_back(now);
    Ok(())
}

fn turns() -> &'static Mutex<HashMap<String, VecDeque<Instant>>> {
    static T: OnceLock<Mutex<HashMap<String, VecDeque<Instant>>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Sends `text` to everyone in watch party `group`, the sender included.
pub async fn say(app: &AppHandle, group: &str, text: &str) -> Result<(), String> {
    let text = say_text(text)?;
    let me = who(app);
    if me.is_empty() {
        return Err("Sign in to Jellyfin first.".into());
    }
    // Who is in the party right now, asked as the sender: only a member can talk in it.
    let list = call(app, "GET", "/SyncPlay/List", None).await?;
    let people = members(&list, group, &me).ok_or("You're not in that watch party.")?;
    {
        let mut t = turns().lock().unwrap();
        t.retain(|_, q| q.back().is_some_and(|x| x.elapsed() < SAY_WINDOW));
        take_turn(&mut t, &me, Instant::now()).map_err(|s| format!("Slow down a little. Try again in {s} seconds."))?;
    }
    let mut raw = [0u8; 8];
    getrandom::getrandom(&mut raw).map_err(|_| "Couldn't send that right now.".to_string())?;
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
    let said = Said { group: group.to_string(), id: raw.iter().map(|b| format!("{b:02x}")).collect(), from: me, text, at };
    // The window, when the profile it has open is in the party.
    if window_user(app).is_some_and(|u| people.iter().any(|p| p.eq_ignore_ascii_case(&u))) {
        let _ = app.emit("together-chat", said.clone());
    }
    // Every web page of each person in the party (by their Jellyfin user id).
    let admin = crate::media::jf_admin(app).filter(|j| j.user_id.is_some()).ok_or("Jellyfin isn't answering.")?;
    let users = admin.get_json("/Users").await.map_err(|e| e.to_string())?;
    for u in users.as_array().into_iter().flatten() {
        let (Some(name), Some(id)) = (u["Name"].as_str(), u["Id"].as_str()) else { continue };
        if people.iter().any(|p| p.eq_ignore_ascii_case(name)) {
            crate::bridge::emit_to_user(id, "together-chat", &said);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_member_talks_in_a_party() {
        let list = serde_json::json!([
            { "GroupId": "g1", "GroupName": "Ann's watch party", "Participants": ["Ann", "Bob"] },
            { "GroupId": "g2", "GroupName": "Cy's watch party", "Participants": ["Cy"] }
        ]);
        assert_eq!(members(&list, "g1", "bob"), Some(vec!["Ann".to_string(), "Bob".to_string()]), "names match in any case");
        assert_eq!(members(&list, "g2", "Ann"), None, "not in that one");
        assert_eq!(members(&list, "g9", "Ann"), None, "no such party");
        assert_eq!(members(&serde_json::json!(null), "g1", "Ann"), None);
    }

    #[test]
    fn messages_are_one_short_line() {
        assert_eq!(say_text("  hi\n  there  ").unwrap(), "hi there");
        assert!(say_text(" \n ").is_err());
        assert!(say_text(&"\u{e9}".repeat(SAY_MAX)).is_ok(), "counted in characters");
        assert!(say_text(&"a".repeat(SAY_MAX + 1)).is_err());
    }

    #[test]
    fn chat_is_rate_limited_per_person() {
        let mut sent = HashMap::new();
        let t0 = Instant::now();
        for _ in 0..SAY_RATE {
            take_turn(&mut sent, "Ann", t0).unwrap();
        }
        assert_eq!(take_turn(&mut sent, "ann", t0 + Duration::from_secs(4)), Err(6), "the same person, any case");
        assert!(take_turn(&mut sent, "Bob", t0).is_ok());
        assert!(take_turn(&mut sent, "Ann", t0 + SAY_WINDOW).is_ok());
    }

    #[test]
    fn only_syncplay_paths_pass() {
        assert!(allowed("/SyncPlay/New") && allowed("/GetUtcTime") && allowed("/SyncPlay/List"));
        assert!(!allowed("/Users/New") && !allowed("/SyncPlay/../Users") && !allowed("/Items/1"));
        for bad in ["/SyncPlay/%2e%2e/Users/New", "/SyncPlay/New?x=1", "/SyncPlay/New#a", r"/SyncPlay/..\Users", r"/SyncPlay/New\x", "/SyncPlay/", "/SyncPlay/a/b", "/SyncPlay/.%2e"] {
            assert!(!allowed(bad), "{bad}");
        }
    }
}
