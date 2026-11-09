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
//!
//! Host tools (`Room`): SyncPlay has no host, locks, kicks or control rights,
//! so Chillarr keeps them on the PC, per group. Whoever made the party is the
//! host (when they go, whoever has been in longest). Joining goes through
//! `join`, which checks the lock, password and removed list before it asks
//! Jellyfin; `call` refuses `/SyncPlay/Join` so a page can't go round it, and
//! refuses play, pause and seek from anyone but the host while "Only I
//! control" is on. Someone who gets in with another Jellyfin app is seen when
//! a member reports `UserJoined` (`seen`) and taken back out.

use crate::media::{jf, Media};
use crate::party::{self, Gate, Lock, Removed};
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
pub(crate) fn who(app: &AppHandle) -> String {
    if let Some(v) = crate::viewer::current() {
        return v.session.user;
    }
    window_user(app).unwrap_or_default()
}

/// The window's profile name (the owner when none is picked). None while locked.
pub(crate) fn window_user(app: &AppHandle) -> Option<String> {
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
    canonical(path).is_some()
}

/// The calls the pages make, spelled one way. Jellyfin's routes ignore case,
/// so `/SyncPlay/join` must not get past the Join and host checks: a path is
/// matched here without case and only this spelling goes on.
const CALLS: [&str; 22] = [
    "/GetUtcTime", "/Sessions/Capabilities/Full", "/SyncPlay/List", "/SyncPlay/New", "/SyncPlay/Join", "/SyncPlay/Leave",
    "/SyncPlay/Ready", "/SyncPlay/Buffering", "/SyncPlay/Ping", "/SyncPlay/Pause", "/SyncPlay/Unpause", "/SyncPlay/Seek",
    "/SyncPlay/Stop", "/SyncPlay/SetNewQueue", "/SyncPlay/NextItem", "/SyncPlay/PreviousItem", "/SyncPlay/SetPlaylistItem",
    "/SyncPlay/Queue", "/SyncPlay/RemoveFromPlaylist", "/SyncPlay/MovePlaylistItem", "/SyncPlay/SetRepeatMode", "/SyncPlay/SetShuffleMode",
];

fn canonical(path: &str) -> Option<&'static str> {
    CALLS.iter().copied().find(|c| c.eq_ignore_ascii_case(path))
}

/// The calls that move the party for everyone: only the host's go through
/// while "Only I control" is on.
fn moves_party(path: &str) -> bool {
    matches!(
        path,
        "/SyncPlay/Pause" | "/SyncPlay/Unpause" | "/SyncPlay/Seek" | "/SyncPlay/Stop" | "/SyncPlay/SetNewQueue" | "/SyncPlay/NextItem" | "/SyncPlay/PreviousItem" | "/SyncPlay/SetPlaylistItem" | "/SyncPlay/Queue" | "/SyncPlay/RemoveFromPlaylist" | "/SyncPlay/MovePlaylistItem" | "/SyncPlay/SetRepeatMode" | "/SyncPlay/SetShuffleMode"
    )
}

/// One SyncPlay REST call. Empty answers come back as null.
pub async fn call(app: &AppHandle, method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
    let Some(path) = canonical(path) else {
        return Err("Not a SyncPlay call.".into());
    };
    // Joining is checked against the host's lock first (`join`).
    if path == "/SyncPlay/Join" {
        return Err("Join a party from Watch together.".into());
    }
    let me = crate::media::active_user(app);
    if moves_party(path) {
        if let Some(uid) = &me {
            if rooms().lock().unwrap().values().any(|r| r.has(uid) && r.host_only && r.host != *uid) {
                return Err("The host is in control.".into());
            }
        }
    }
    let r = raw(app, method, path, body).await?;
    match (path, me) {
        ("/SyncPlay/New", Some(uid)) => made(app, &uid).await,
        ("/SyncPlay/Leave", Some(uid)) => {
            let (group, people) = take_out(&uid);
            if let Some(g) = group {
                tell_room(app, &g, &people);
            }
        }
        _ => {}
    }
    Ok(r)
}

/// A SyncPlay call as whoever is watching, no checks: for this file's own use.
async fn raw(app: &AppHandle, method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
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

// ---------- host tools ----------

/// One watch party's host tools, kept on the PC by Jellyfin group id.
#[derive(Default, Debug)]
struct Room {
    /// The host's Jellyfin user id.
    host: String,
    /// (user id, name) in the order they came in: the next host is the first.
    order: Vec<(String, String)>,
    gate: Gate,
    /// "Only I control": nobody but the host plays, pauses or seeks.
    host_only: bool,
}

impl Room {
    fn has(&self, uid: &str) -> bool {
        self.order.iter().any(|(id, _)| id == uid)
    }

    fn people(&self) -> Vec<String> {
        self.order.iter().map(|(id, _)| id.clone()).collect()
    }

    /// Takes someone out. When it was the host, the next one in takes over and
    /// "Only I control" turns off for them to decide.
    fn drop_person(&mut self, uid: &str) {
        self.order.retain(|(id, _)| id != uid);
        if self.host == uid {
            self.host = self.order.first().map(|(id, _)| id.clone()).unwrap_or_default();
            self.host_only = false;
        }
    }
}

fn rooms() -> &'static Mutex<HashMap<String, Room>> {
    static R: OnceLock<Mutex<HashMap<String, Room>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Takes `uid` out of whatever party they were in. The group and who is left.
fn take_out(uid: &str) -> (Option<String>, Vec<String>) {
    let mut all = rooms().lock().unwrap();
    let mut out = (None, vec![]);
    for (g, r) in all.iter_mut() {
        if r.has(uid) {
            r.drop_person(uid);
            out = (Some(g.clone()), r.people());
        }
    }
    all.retain(|_, r| !r.order.is_empty());
    out
}

#[derive(Serialize, Debug)]
pub struct Person {
    pub id: String,
    pub name: String,
    pub host: bool,
}

/// What the party panel shows about the host tools.
#[derive(Serialize, Debug)]
pub struct RoomView {
    pub host: bool,
    pub host_name: String,
    pub lock: Lock,
    pub host_only: bool,
    pub people: Vec<Person>,
    /// The host only: who they removed.
    pub removed: Vec<Removed>,
}

fn view(r: &mut Room, me: &str) -> RoomView {
    let host = r.host == me;
    RoomView {
        host,
        host_name: r.order.iter().find(|(id, _)| *id == r.host).map(|(_, n)| n.clone()).unwrap_or_default(),
        lock: r.gate.lock,
        host_only: r.host_only,
        people: r.order.iter().map(|(id, name)| Person { id: id.clone(), name: name.clone(), host: *id == r.host }).collect(),
        removed: if host { r.gate.removed(party::now()) } else { vec![] },
    }
}

/// Everyone in a party hears its host tools changed ("together-room"): each
/// member's web pages, and the window (which checks the group is its own).
fn tell_room(app: &AppHandle, group: &str, people: &[String]) {
    let ev = serde_json::json!({ "group": group });
    for id in people {
        crate::bridge::emit_to_user(id, "together-room", &ev);
    }
    let _ = app.emit("together-room", ev);
}

/// Jellyfin's users as (name, id), to tell who a SyncPlay name is.
async fn user_ids(app: &AppHandle) -> Vec<(String, String)> {
    let Some(admin) = crate::media::jf_admin(app).filter(|j| j.user_id.is_some()) else { return vec![] };
    let Ok(users) = admin.get_json("/Users").await else { return vec![] };
    users.as_array().into_iter().flatten().filter_map(|u| Some((u["Name"].as_str()?.to_string(), u["Id"].as_str()?.to_string()))).collect()
}

fn id_of(users: &[(String, String)], name: &str) -> Option<String> {
    users.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, id)| id.clone())
}

/// The window's own profile id (the owner when none is picked), whoever asked.
fn window_uid(app: &AppHandle) -> Option<String> {
    let media = app.state::<Media>();
    if media.is_locked() {
        return None;
    }
    let profile = media.profile.lock().unwrap().as_ref().map(|p| p.user_id.clone());
    profile.or_else(|| media.session.lock().unwrap().as_ref().map(|s| s.user_id.clone()))
}

/// The party `me` just made with /SyncPlay/New: they're its host.
async fn made(app: &AppHandle, uid: &str) {
    let name = who(app);
    let Ok(list) = raw(app, "GET", "/SyncPlay/List", None).await else { return };
    let Some(group) = list.as_array().into_iter().flatten().find(|g| g["Participants"].as_array().is_some_and(|p| p.iter().any(|x| x.as_str().is_some_and(|x| x.eq_ignore_ascii_case(&name))))).and_then(|g| g["GroupId"].as_str()) else { return };
    let (old, left) = take_out(uid);
    rooms().lock().unwrap().insert(group.to_string(), Room { host: uid.to_string(), order: vec![(uid.to_string(), name)], ..Default::default() });
    if let Some(g) = old {
        tell_room(app, &g, &left);
    }
}

/// Joins a watch party, if its host lets this profile in (lock, password,
/// removed list), then asks Jellyfin.
pub async fn join(app: &AppHandle, group: &str, password: Option<&str>) -> Result<(), String> {
    let me = crate::media::active_user(app).ok_or("Sign in to Jellyfin first.")?;
    let name = who(app);
    let known = rooms().lock().unwrap().contains_key(group);
    if !known {
        // A party Chillarr didn't see start (made in a Jellyfin app, or before
        // a restart): whoever is in it now, the first of them hosting.
        let list = raw(app, "GET", "/SyncPlay/List", None).await?;
        let g = list.as_array().into_iter().flatten().find(|g| g["GroupId"].as_str() == Some(group)).ok_or("That party has ended.")?;
        let users = user_ids(app).await;
        let order: Vec<(String, String)> = g["Participants"].as_array().into_iter().flatten().filter_map(|p| p.as_str()).filter_map(|n| Some((id_of(&users, n)?, n.to_string()))).collect();
        let host = order.first().map(|(id, _)| id.clone()).unwrap_or_default();
        rooms().lock().unwrap().entry(group.to_string()).or_insert(Room { host, order, ..Default::default() });
    }
    let already = {
        let mut all = rooms().lock().unwrap();
        let r = all.get_mut(group).ok_or("That party has ended.")?;
        let already = r.has(&me);
        if !already {
            r.gate.admit(&me, password, party::now())?;
        }
        already
    };
    // Back in a party it never left (a reload): its place, and host, stay.
    if already {
        return raw(app, "POST", "/SyncPlay/Join", Some(serde_json::json!({ "GroupId": group }))).await.map(|_| ());
    }
    let (old, left) = take_out(&me);
    if let Some(g) = old.filter(|g| g != group) {
        tell_room(app, &g, &left);
    }
    {
        let mut all = rooms().lock().unwrap();
        let r = all.entry(group.to_string()).or_default();
        if r.host.is_empty() {
            r.host = me.clone();
        }
        r.order.push((me.clone(), name));
    }
    if let Err(e) = raw(app, "POST", "/SyncPlay/Join", Some(serde_json::json!({ "GroupId": group }))).await {
        take_out(&me);
        return Err(e);
    }
    let people = rooms().lock().unwrap().get(group).map(Room::people).unwrap_or_default();
    tell_room(app, group, &people);
    Ok(())
}

/// The host tools as `me` sees them, if Chillarr knows this party and `me` is in it.
pub fn room(app: &AppHandle, group: &str) -> Option<RoomView> {
    let me = crate::media::active_user(app)?;
    let mut all = rooms().lock().unwrap();
    let r = all.get_mut(group).filter(|r| r.has(&me))?;
    Some(view(r, &me))
}

/// Each known party's lock, for the list of parties to join.
pub fn locks() -> HashMap<String, Lock> {
    rooms().lock().unwrap().iter().map(|(g, r)| (g.clone(), r.gate.lock)).collect()
}

/// Runs `f` on the room if `me` hosts it, then tells everyone in it.
fn as_host<T>(app: &AppHandle, group: &str, f: impl FnOnce(&mut Room, &str) -> Result<T, String>) -> Result<RoomView, String> {
    let me = crate::media::active_user(app).ok_or("Sign in to Jellyfin first.")?;
    let (v, people) = {
        let mut all = rooms().lock().unwrap();
        let r = all.get_mut(group).filter(|r| r.has(&me)).ok_or("You're not in that watch party.")?;
        if r.host != me {
            return Err("Only the host can do that.".into());
        }
        f(r, &me)?;
        (view(r, &me), r.people())
    };
    tell_room(app, group, &people);
    Ok(v)
}

/// The host's settings: who may join, and whether only they control.
pub fn set(app: &AppHandle, group: &str, lock: Option<&str>, password: Option<&str>, host_only: Option<bool>) -> Result<RoomView, String> {
    let lock = lock.map(Lock::parse).transpose()?;
    as_host(app, group, |r, _| {
        if let Some(l) = lock {
            r.gate.set(l, password)?;
        }
        if let Some(h) = host_only {
            r.host_only = h;
        }
        Ok(())
    })
}

/// The host hands the party to someone else in it.
pub fn make_host(app: &AppHandle, group: &str, uid: &str) -> Result<RoomView, String> {
    as_host(app, group, |r, _| {
        if !r.has(uid) {
            return Err("They already left.".into());
        }
        r.host = uid.to_string();
        Ok(())
    })
}

/// The host lets someone they removed come back before the 10 minutes are up.
pub fn allow(app: &AppHandle, group: &str, uid: &str) -> Result<RoomView, String> {
    as_host(app, group, |r, _| {
        r.gate.allow_back(uid);
        Ok(())
    })
}

/// The host removes someone: out of the party now, and kept out for 10 minutes.
pub async fn kick(app: &AppHandle, group: &str, uid: &str) -> Result<RoomView, String> {
    let v = as_host(app, group, |r, me| {
        if uid == me {
            return Err("You can't remove yourself. Leave the party instead.".into());
        }
        let name = r.order.iter().find(|(id, _)| id == uid).map(|(_, n)| n.clone()).ok_or("They already left.")?;
        r.drop_person(uid);
        r.gate.remove(uid, &name, party::now());
        Ok(())
    })?;
    put_out(app, group, uid).await;
    Ok(v)
}

/// Ends `uid`'s membership: their Chillarr pages and window are told to leave
/// ("together-kicked"), and when they're one of Chillarr's profiles the PC
/// leaves the group for them too.
async fn put_out(app: &AppHandle, group: &str, uid: &str) {
    let ev = serde_json::json!({ "group": group });
    crate::bridge::emit_to_user(uid, "together-kicked", &ev);
    if window_uid(app).as_deref() == Some(uid) {
        let _ = app.emit("together-kicked", ev);
    }
    if let Some((_, j)) = crate::profiles::all_jf(app).into_iter().find(|(id, _)| id == uid) {
        let _ = j.post_json("/SyncPlay/Leave", &serde_json::json!({})).await;
    }
}

/// A member's player saw someone join (`UserJoined`). Someone who came in
/// around Chillarr while the party is locked, or who the host removed, is put
/// back out; anyone else is counted in.
pub async fn seen(app: &AppHandle, group: &str, name: &str) -> Result<(), String> {
    let me = crate::media::active_user(app).ok_or("Sign in to Jellyfin first.")?;
    if !rooms().lock().unwrap().get(group).is_some_and(|r| r.has(&me)) {
        return Ok(());
    }
    // Only someone Jellyfin really has in this party: a member can't name
    // anyone they like to put them out or add them (as `gone` checks too).
    let list = raw(app, "GET", "/SyncPlay/List", None).await?;
    let there = list.as_array().into_iter().flatten().filter(|g| g["GroupId"].as_str() == Some(group)).flat_map(|g| g["Participants"].as_array().cloned().unwrap_or_default()).any(|p| p.as_str().is_some_and(|p| p.eq_ignore_ascii_case(name)));
    if !there {
        return Ok(());
    }
    let uid = id_of(&user_ids(app).await, name).ok_or("No such user.")?;
    let out = {
        let mut all = rooms().lock().unwrap();
        let Some(r) = all.get_mut(group) else { return Ok(()) };
        if r.has(&uid) {
            return Ok(());
        }
        let out = r.gate.closed_to(&uid, party::now());
        if !out {
            r.order.push((uid.clone(), name.to_string()));
        }
        out
    };
    if out {
        put_out(app, group, &uid).await;
    } else {
        let people = rooms().lock().unwrap().get(group).map(Room::people).unwrap_or_default();
        tell_room(app, group, &people);
    }
    Ok(())
}

/// A member's player saw someone leave (`UserLeft`): checked with Jellyfin,
/// then taken out (the host's job passes on if it was them).
pub async fn gone(app: &AppHandle, group: &str, name: &str) -> Result<(), String> {
    let me = crate::media::active_user(app).ok_or("Sign in to Jellyfin first.")?;
    if !rooms().lock().unwrap().get(group).is_some_and(|r| r.has(&me)) {
        return Ok(());
    }
    let list = raw(app, "GET", "/SyncPlay/List", None).await?;
    let still = list.as_array().into_iter().flatten().filter(|g| g["GroupId"].as_str() == Some(group)).flat_map(|g| g["Participants"].as_array().cloned().unwrap_or_default()).any(|p| p.as_str().is_some_and(|p| p.eq_ignore_ascii_case(name)));
    if still {
        return Ok(());
    }
    let people = {
        let mut all = rooms().lock().unwrap();
        let Some(r) = all.get_mut(group) else { return Ok(()) };
        let Some(uid) = r.order.iter().find(|(_, n)| n.eq_ignore_ascii_case(name)).map(|(id, _)| id.clone()) else { return Ok(()) };
        r.drop_person(&uid);
        r.people()
    };
    tell_room(app, group, &people);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_case_is_one_spelling() {
        assert_eq!(canonical("/SyncPlay/join"), Some("/SyncPlay/Join"));
        assert_eq!(canonical("/syncplay/PAUSE"), Some("/SyncPlay/Pause"));
        assert!(moves_party(canonical("/SyncPlay/seek").unwrap()));
        assert_eq!(canonical("/SyncPlay/%2e%2e/Users/New"), None);
        assert_eq!(canonical("/SyncPlay/Whatever"), None);
    }

    #[test]
    fn joining_and_host_moves_go_through_chillarr() {
        assert!(moves_party("/SyncPlay/Seek") && moves_party("/SyncPlay/Unpause") && moves_party("/SyncPlay/SetNewQueue"));
        assert!(!moves_party("/SyncPlay/Ready") && !moves_party("/SyncPlay/Buffering") && !moves_party("/SyncPlay/Leave") && !moves_party("/SyncPlay/Ping"));
    }

    #[test]
    fn the_longest_in_takes_over_and_control_goes_back_to_everyone() {
        let mut r = Room { host: "a".into(), order: vec![("a".into(), "Ann".into()), ("b".into(), "Bob".into()), ("c".into(), "Cy".into())], host_only: true, ..Default::default() };
        r.drop_person("b");
        assert_eq!((r.host.as_str(), r.host_only), ("a", true), "someone else leaving changes nothing");
        r.drop_person("a");
        assert_eq!((r.host.as_str(), r.host_only), ("c", false));
        let v = view(&mut r, "c");
        assert!(v.host && v.people[0].host && v.host_name == "Cy");
    }

    #[test]
    fn only_the_host_sees_who_was_removed() {
        let mut r = Room { host: "a".into(), order: vec![("a".into(), "Ann".into()), ("b".into(), "Bob".into())], ..Default::default() };
        r.gate.remove("x", "Xan", party::now());
        assert_eq!(view(&mut r, "a").removed.len(), 1);
        assert!(view(&mut r, "b").removed.is_empty());
    }
}

#[cfg(test)]
mod chat_tests {
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
