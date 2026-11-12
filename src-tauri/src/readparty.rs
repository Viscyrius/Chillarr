//! Read Party: reading a series together, the way Watch together is for films.
//! Someone starts one from the reader and gets a short code; friends join with
//! it from Read together in the top bar. Everyone in it is on the same chapter
//! and page: whoever turns the page moves the party, and the others follow.
//! Kept in memory on the PC, which every device reads through. Joining a series
//! you don't follow adds it to your list without downloading it again.
//!
//! Host tools: whoever started it is the host (if they go, whoever has been in
//! longest). The host picks how the party reads (`Mode`), can lock it or ask
//! for a password (party.rs), and can remove someone. In Follow me the host's
//! exact spot (page plus how far down it, 0 to 1) goes out as an event the
//! moment it moves, so the others glide with it instead of waiting for a poll.

use crate::party::{self, Gate, Lock, Removed};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};

/// How the party reads, the host's pick.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Anyone who turns the page moves everyone, by page.
    #[default]
    Together,
    /// Only the host moves the party, and everyone follows their exact spot.
    Follow,
    /// Everyone reads at their own pace; the host's spot is kept for "Jump to the host".
    Free,
}

impl Mode {
    fn parse(s: &str) -> Result<Mode, String> {
        match s {
            "together" => Ok(Mode::Together),
            "follow" => Ok(Mode::Follow),
            "free" => Ok(Mode::Free),
            _ => Err("Pick Together, Follow me or Free.".into()),
        }
    }
}

#[derive(Clone, Serialize, Debug, PartialEq)]
pub struct Person {
    pub id: String,
    pub name: String,
    pub host: bool,
}

/// Seconds without hearing from someone before they count as gone.
const GONE: u64 = 45;
/// Parties at once (codes are only handed to signed-in profiles anyway).
const MAX_PARTIES: usize = 50;
const CODE_CHARS: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";

#[derive(Clone, Serialize, Debug, PartialEq)]
pub struct State {
    pub code: String,
    pub key: String,
    pub title: String,
    pub chapter: i64,
    pub page: u32,
    /// How far down that page, 0 to 1 (Follow me's exact spot).
    pub frac: f32,
    /// Goes up on every move, so a device follows only what's newer.
    pub ver: u64,
    /// The last move was this device's own profile.
    pub mine: bool,
    /// How many are reading together now.
    pub people: usize,
    pub who: Vec<Person>,
    /// This device's profile is the host.
    pub host: bool,
    pub host_name: String,
    pub mode: Mode,
    pub lock: Lock,
    /// The host only: who they removed, and for how long.
    pub removed: Vec<Removed>,
}

#[derive(Clone, Debug)]
struct Party {
    key: String,
    title: String,
    chapter: i64,
    page: u32,
    frac: f32,
    ver: u64,
    by: String,
    /// Profile -> last heard from (seconds).
    seen: HashMap<String, u64>,
    /// Profile -> name, in the order they came in (the next host is the first).
    order: Vec<(String, String)>,
    host: String,
    mode: Mode,
    gate: Gate,
}

impl Party {
    fn new(key: String, title: String, chapter: i64, page: u32, me: &str, name: &str) -> Party {
        Party {
            key,
            title,
            chapter,
            page,
            frac: 0.0,
            ver: 1,
            by: me.into(),
            seen: HashMap::from([(me.to_string(), now())]),
            order: vec![(me.to_string(), name.to_string())],
            host: me.into(),
            mode: Mode::Together,
            gate: Gate::default(),
        }
    }

    /// Takes someone out; if it was the host, the next one in takes over and
    /// the party goes back to Together for them to decide.
    fn drop_person(&mut self, uid: &str) {
        self.seen.remove(uid);
        self.fix_host();
    }

    fn fix_host(&mut self) {
        let seen = &self.seen;
        self.order.retain(|(id, _)| seen.contains_key(id));
        if !self.seen.contains_key(&self.host) {
            if let Some((id, _)) = self.order.first() {
                self.host = id.clone();
                self.mode = Mode::Together;
            }
        }
    }
}

static PARTIES: Mutex<Option<HashMap<String, Party>>> = Mutex::new(None);

fn now() -> u64 {
    party::now()
}

fn new_code() -> String {
    let mut b = [0u8; 6];
    getrandom::getrandom(&mut b).expect("random");
    b.iter().map(|x| CODE_CHARS[*x as usize % CODE_CHARS.len()] as char).collect()
}

/// A code as people type it: case and spaces don't matter.
pub fn clean_code(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).take(12).collect()
}

fn state_of(code: &str, p: &mut Party, uid: &str) -> State {
    let host = p.host == uid;
    let who = p.order.iter().map(|(id, name)| Person { id: id.clone(), name: name.clone(), host: *id == p.host }).collect();
    let host_name = p.order.iter().find(|(id, _)| *id == p.host).map(|(_, n)| n.clone()).unwrap_or_default();
    State {
        code: code.into(),
        key: p.key.clone(),
        title: p.title.clone(),
        chapter: p.chapter,
        page: p.page,
        frac: p.frac,
        ver: p.ver,
        mine: p.by == uid,
        people: p.seen.len(),
        who,
        host,
        host_name,
        mode: p.mode,
        lock: p.gate.lock,
        removed: if host { p.gate.removed(now()) } else { vec![] },
    }
}

/// Drops whoever went quiet, and parties nobody is in.
fn prune(all: &mut HashMap<String, Party>, t: u64) {
    for p in all.values_mut() {
        p.seen.retain(|_, at| t.saturating_sub(*at) <= GONE);
        p.fix_host();
    }
    all.retain(|_, p| !p.seen.is_empty());
}

fn with<T>(f: impl FnOnce(&mut HashMap<String, Party>) -> T) -> T {
    let mut g = PARTIES.lock().unwrap();
    let all = g.get_or_insert_with(HashMap::new);
    prune(all, now());
    f(all)
}

fn uid(app: &AppHandle) -> Result<String, String> {
    crate::media::active_user(app).ok_or_else(|| "Sign in to Jellyfin first.".to_string())
}

fn name(app: &AppHandle) -> String {
    let n = crate::syncplay::who(app);
    if n.is_empty() { "Someone".into() } else { n }
}

/// The party, if `me` is in it: the code alone doesn't let anyone read along.
fn member<'a>(all: &'a mut HashMap<String, Party>, code: &str, me: &str) -> Result<&'a mut Party, String> {
    let p = all.get_mut(code).ok_or("That reading party has ended.")?;
    if !p.seen.contains_key(me) {
        return Err(if p.gate.removed(now()).iter().any(|r| r.id == me) { "You were removed from the reading party.".into() } else { "You're not in that reading party.".into() });
    }
    p.seen.insert(me.to_string(), now());
    Ok(p)
}

fn host_of<'a>(all: &'a mut HashMap<String, Party>, code: &str, me: &str) -> Result<&'a mut Party, String> {
    let p = member(all, code, me)?;
    if p.host != me {
        return Err("Only the host can do that.".into());
    }
    Ok(p)
}

/// Everyone in a party hears that it changed ("readparty-update"): each
/// member's web pages, and the window (which checks the code is its own).
fn tell(app: &AppHandle, code: &str, people: &[String], what: &str) {
    let ev = serde_json::json!({ "code": code, "what": what });
    for id in people {
        crate::bridge::emit_to_user(id, "readparty-update", &ev);
    }
    let _ = app.emit("readparty-update", ev);
}

/// One chat message, as every member's pages get it ("readparty-chat").
#[derive(Serialize, Clone, Debug)]
pub struct Said {
    pub code: String,
    pub id: String,
    pub from: String,
    /// The sender's profile id, so each page knows its own messages.
    pub from_id: String,
    pub text: String,
    /// Unix milliseconds.
    pub at: i64,
}

/// Sends `text` to everyone in the reading party, the sender included (#76).
/// Same length and rate limit as Watch together's chat; only a member can talk.
pub fn say(app: &AppHandle, code: &str, text: &str) -> Result<(), String> {
    let text = crate::syncplay::say_text(text)?;
    let me = uid(app)?;
    let code = clean_code(code);
    let (from, people) = with(|all| {
        let p = member(all, &code, &me)?;
        let from = p.order.iter().find(|(id, _)| *id == me).map(|(_, n)| n.clone()).unwrap_or_default();
        Ok::<_, String>((from, p.seen.keys().cloned().collect::<Vec<String>>()))
    })?;
    {
        let mut t = crate::syncplay::turns().lock().unwrap_or_else(|e| e.into_inner());
        crate::syncplay::take_turn(&mut t, &me, std::time::Instant::now()).map_err(|s| format!("Slow down a little. Try again in {s} seconds."))?;
    }
    let mut raw = [0u8; 8];
    getrandom::getrandom(&mut raw).map_err(|_| "Couldn't send that right now.".to_string())?;
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
    let said = Said { code, id: raw.iter().map(|b| format!("{b:02x}")).collect(), from, from_id: me, text, at };
    for id in &people {
        crate::bridge::emit_to_user(id, "readparty-chat", &said);
    }
    // The window checks the code is its own party's.
    let _ = app.emit("readparty-chat", said);
    Ok(())
}

/// Starts a party on the series and chapter open in the reader.
pub fn start(app: &AppHandle, key: &str, chapter: i64, page: u32) -> Result<State, String> {
    let me = uid(app)?;
    let who = name(app);
    let f = crate::read::followed(app).into_iter().find(|f| f.key == key && crate::read::mine(app, f, &me)).ok_or("That series isn't in your reading list.")?;
    with(|all| {
        // One party per profile: starting again leaves the old one.
        for p in all.values_mut() {
            p.drop_person(&me);
        }
        all.retain(|_, p| !p.seen.is_empty());
        if all.len() >= MAX_PARTIES {
            return Err("Too many reading parties right now. Try again later.".into());
        }
        let mut code = new_code();
        while all.contains_key(&code) {
            code = new_code();
        }
        let mut p = Party::new(f.key.clone(), f.title.clone(), chapter, page, &me, &who);
        let s = state_of(&code, &mut p, &me);
        all.insert(code, p);
        Ok(s)
    })
}

/// Joins with a code (and the party's password, when it has one). A series
/// you don't follow is added to your list (no second download).
pub fn join(app: &AppHandle, code: &str, password: Option<&str>) -> Result<State, String> {
    let me = uid(app)?;
    let who = name(app);
    let code = clean_code(code);
    let (key, people) = with(|all| {
        let p = all.get_mut(&code).ok_or("No reading party has that code. Check it and try again.")?;
        if !p.seen.contains_key(&me) {
            p.gate.admit(&me, password, now())?;
            p.order.push((me.clone(), who.clone()));
        }
        p.seen.insert(me.clone(), now());
        Ok::<_, String>((p.key.clone(), p.seen.keys().cloned().collect::<Vec<_>>()))
    })?;
    tell(app, &code, &people, "people");
    if let Some(f) = crate::read::followed(app).into_iter().find(|f| f.key == key) {
        if !crate::read::mine(app, &f, &me) {
            let mut owners = crate::read::owners_of(&f.owners, crate::profiles::owner_id(app).as_deref());
            owners.push(me.clone());
            crate::read::update_followed(app, &key, |x| x.owners = owners);
        }
    } else {
        return Err("That series isn't on this PC any more.".into());
    }
    state(app, &code)
}

/// Where the party is now (and this device is still in it).
pub fn state(app: &AppHandle, code: &str) -> Result<State, String> {
    let me = uid(app)?;
    let code = clean_code(code);
    with(|all| {
        let p = member(all, &code, &me)?;
        Ok(state_of(&code, p, &me))
    })
}

/// Whether a move by `me` moves the party: in Together anyone's does, in
/// Follow me and Free only the host's (in Free nobody follows it, it's only
/// kept for "Jump to the host").
fn may_move(p: &Party, me: &str) -> bool {
    p.mode == Mode::Together || p.host == me
}

/// This device moved: the party moves with it (if it may). `frac` is how far
/// down the page, 0 to 1.
pub fn moved(app: &AppHandle, code: &str, chapter: i64, page: u32, frac: Option<f32>) -> Result<State, String> {
    let me = uid(app)?;
    let code = clean_code(code);
    let frac = frac.filter(|f| f.is_finite()).unwrap_or(0.0).clamp(0.0, 1.0);
    let (s, follow) = with(|all| {
        let p = member(all, &code, &me)?;
        let mut follow = None;
        if may_move(p, &me) && (p.chapter != chapter || p.page != page || (p.frac - frac).abs() > 0.002) {
            p.chapter = chapter;
            p.page = page;
            p.frac = frac;
            p.ver += 1;
            p.by = me.clone();
            if p.mode == Mode::Follow {
                follow = Some(p.seen.keys().filter(|id| **id != me).cloned().collect::<Vec<_>>());
            }
        }
        Ok::<_, String>((state_of(&code, p, &me), follow))
    })?;
    // Follow me: the others hear it now, not at their next poll.
    if let Some(people) = follow {
        let ev = serde_json::json!({ "code": code, "chapter": s.chapter, "page": s.page, "frac": s.frac, "ver": s.ver });
        for id in &people {
            crate::bridge::emit_to_user(id, "readparty-follow", &ev);
        }
        let _ = app.emit("readparty-follow", ev);
    }
    Ok(s)
}

/// The host's settings: how the party reads, and who may join.
pub fn set(app: &AppHandle, code: &str, mode: Option<&str>, lock: Option<&str>, password: Option<&str>) -> Result<State, String> {
    let me = uid(app)?;
    let code = clean_code(code);
    let mode = mode.map(Mode::parse).transpose()?;
    let lock = lock.map(Lock::parse).transpose()?;
    let (s, people) = with(|all| {
        let p = host_of(all, &code, &me)?;
        if let Some(l) = lock {
            p.gate.set(l, password)?;
        }
        if let Some(m) = mode {
            p.mode = m;
            p.ver += 1;
        }
        Ok::<_, String>((state_of(&code, p, &me), p.seen.keys().cloned().collect::<Vec<_>>()))
    })?;
    tell(app, &code, &people, "settings");
    Ok(s)
}

/// The host removes someone. They're out at once and can't come back for 10 minutes.
pub fn kick(app: &AppHandle, code: &str, who: &str) -> Result<State, String> {
    let me = uid(app)?;
    let code = clean_code(code);
    if who == me {
        return Err("You can't remove yourself. Leave the party instead.".into());
    }
    let (s, people) = with(|all| {
        let p = host_of(all, &code, &me)?;
        let name = p.order.iter().find(|(id, _)| id == who).map(|(_, n)| n.clone()).ok_or("They already left.")?;
        p.drop_person(who);
        p.gate.remove(who, &name, now());
        Ok::<_, String>((state_of(&code, p, &me), p.seen.keys().cloned().collect::<Vec<_>>()))
    })?;
    let ev = serde_json::json!({ "code": code, "what": "kicked", "who": who });
    crate::bridge::emit_to_user(who, "readparty-update", &ev);
    let _ = app.emit("readparty-update", ev);
    tell(app, &code, &people, "people");
    Ok(s)
}

/// The host lets someone they removed come back before the 10 minutes are up.
pub fn allow(app: &AppHandle, code: &str, who: &str) -> Result<State, String> {
    let me = uid(app)?;
    let code = clean_code(code);
    with(|all| {
        let p = host_of(all, &code, &me)?;
        p.gate.allow_back(who);
        Ok(state_of(&code, p, &me))
    })
}

/// The host hands the party to someone else in it.
pub fn make_host(app: &AppHandle, code: &str, who: &str) -> Result<State, String> {
    let me = uid(app)?;
    let code = clean_code(code);
    let (s, people) = with(|all| {
        let p = host_of(all, &code, &me)?;
        if !p.seen.contains_key(who) {
            return Err("They already left.".into());
        }
        p.host = who.to_string();
        p.ver += 1;
        Ok::<_, String>((state_of(&code, p, &me), p.seen.keys().cloned().collect::<Vec<_>>()))
    })?;
    tell(app, &code, &people, "settings");
    Ok(s)
}

pub fn leave(app: &AppHandle, code: &str) -> Result<(), String> {
    let me = uid(app)?;
    let code = clean_code(code);
    let people = with(|all| {
        let people = all.get_mut(&code).map(|p| {
            p.drop_person(&me);
            p.seen.keys().cloned().collect::<Vec<_>>()
        });
        all.retain(|_, p| !p.seen.is_empty());
        people
    });
    if let Some(people) = people {
        tell(app, &code, &people, "people");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn party(seen: &[(&str, u64)]) -> Party {
        let mut p = Party::new("m1".into(), "T".into(), 5, 0, "a", "Ann");
        p.seen = seen.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        p.order = seen.iter().map(|(k, _)| (k.to_string(), k.to_uppercase())).collect();
        p
    }

    #[test]
    fn codes_read_however_they_are_typed() {
        assert_eq!(clean_code(" ab3 k-9z "), "AB3K9Z");
        assert_eq!(new_code().len(), 6);
        assert!(new_code().bytes().all(|b| CODE_CHARS.contains(&b)), "no 0, O, 1, I or L to misread");
    }

    #[test]
    fn quiet_readers_leave_and_empty_parties_end() {
        let mut all = HashMap::from([("A".to_string(), party(&[("a", 100), ("b", 10)])), ("B".to_string(), party(&[("c", 10)]))]);
        prune(&mut all, 120);
        assert_eq!(all.len(), 1, "the party nobody is in is gone");
        assert_eq!(all["A"].seen.keys().collect::<Vec<_>>(), vec!["a"]);
    }

    #[test]
    fn state_says_whose_move_it_was() {
        let mut p = party(&[("a", 1), ("b", 1)]);
        let a = state_of("A", &mut p, "a");
        assert!(a.mine && a.host);
        let s = state_of("A", &mut p, "b");
        assert!(!s.mine && !s.host);
        assert_eq!((s.chapter, s.page, s.people, s.host_name.as_str()), (5, 0, 2, "A"));
    }

    #[test]
    fn host_leaving_passes_to_whoever_came_in_first_and_resets_the_mode() {
        let mut p = party(&[("a", 1), ("b", 1), ("c", 1)]);
        p.mode = Mode::Follow;
        p.drop_person("a");
        assert_eq!((p.host.as_str(), p.mode), ("b", Mode::Together));
        let mut all = HashMap::from([("A".to_string(), party(&[("a", 10), ("b", 100)]))]);
        prune(&mut all, 120);
        assert_eq!(all["A"].host, "b", "a quiet host hands over too");
    }

    #[test]
    fn only_the_host_moves_a_follow_or_free_party() {
        let mut p = party(&[("a", 1), ("b", 1)]);
        assert!(may_move(&p, "b"));
        p.mode = Mode::Follow;
        assert!(may_move(&p, "a") && !may_move(&p, "b"));
        p.mode = Mode::Free;
        assert!(may_move(&p, "a") && !may_move(&p, "b"));
    }

    #[test]
    fn the_code_alone_isnt_membership() {
        let mut all = HashMap::from([("A".to_string(), party(&[("a", now())]))]);
        assert!(member(&mut all, "A", "x").is_err());
        assert!(host_of(&mut all, "A", "a").is_ok());
        all.get_mut("A").unwrap().gate.remove("x", "X", now());
        assert!(member(&mut all, "A", "x").unwrap_err().contains("removed"));
    }
}
