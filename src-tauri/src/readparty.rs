//! Read Party: reading a series together, the way Watch together is for films.
//! Someone starts one from the reader and gets a short code; friends join with
//! it from Read together in the top bar. Everyone in it is on the same chapter
//! and page: whoever turns the page moves the party, and the others follow.
//! Kept in memory on the PC, which every device reads through. Joining a series
//! you don't follow adds it to your list without downloading it again.

use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::AppHandle;

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
    /// Goes up on every move, so a device follows only what's newer.
    pub ver: u64,
    /// The last move was this device's own profile.
    pub mine: bool,
    /// How many are reading together now.
    pub people: usize,
}

#[derive(Clone, Debug)]
struct Party {
    key: String,
    title: String,
    chapter: i64,
    page: u32,
    ver: u64,
    by: String,
    /// Profile -> last heard from (seconds).
    seen: HashMap<String, u64>,
}

static PARTIES: Mutex<Option<HashMap<String, Party>>> = Mutex::new(None);

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
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

fn state_of(code: &str, p: &Party, uid: &str) -> State {
    State { code: code.into(), key: p.key.clone(), title: p.title.clone(), chapter: p.chapter, page: p.page, ver: p.ver, mine: p.by == uid, people: p.seen.len() }
}

/// Drops whoever went quiet, and parties nobody is in.
fn prune(all: &mut HashMap<String, Party>, t: u64) {
    for p in all.values_mut() {
        p.seen.retain(|_, at| t.saturating_sub(*at) <= GONE);
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

/// Starts a party on the series and chapter open in the reader.
pub fn start(app: &AppHandle, key: &str, chapter: i64, page: u32) -> Result<State, String> {
    let me = uid(app)?;
    let f = crate::read::followed(app).into_iter().find(|f| f.key == key && crate::read::mine(app, f, &me)).ok_or("That series isn't in your reading list.")?;
    with(|all| {
        // One party per profile: starting again leaves the old one.
        for p in all.values_mut() {
            p.seen.remove(&me);
        }
        all.retain(|_, p| !p.seen.is_empty());
        if all.len() >= MAX_PARTIES {
            return Err("Too many reading parties right now. Try again later.".into());
        }
        let mut code = new_code();
        while all.contains_key(&code) {
            code = new_code();
        }
        let p = Party { key: f.key.clone(), title: f.title.clone(), chapter, page, ver: 1, by: me.clone(), seen: HashMap::from([(me.clone(), now())]) };
        let s = state_of(&code, &p, &me);
        all.insert(code, p);
        Ok(s)
    })
}

/// Joins with a code. A series you don't follow is added to your list (no second download).
pub fn join(app: &AppHandle, code: &str) -> Result<State, String> {
    let me = uid(app)?;
    let code = clean_code(code);
    let key = with(|all| {
        let p = all.get_mut(&code).ok_or("No reading party has that code. Check it and try again.")?;
        p.seen.insert(me.clone(), now());
        Ok::<_, String>(p.key.clone())
    })?;
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
        let p = all.get_mut(&code).ok_or("That reading party has ended.")?;
        p.seen.insert(me.clone(), now());
        Ok(state_of(&code, p, &me))
    })
}

/// This device turned the page: the party moves with it.
pub fn moved(app: &AppHandle, code: &str, chapter: i64, page: u32) -> Result<State, String> {
    let me = uid(app)?;
    let code = clean_code(code);
    with(|all| {
        let p = all.get_mut(&code).ok_or("That reading party has ended.")?;
        if !p.seen.contains_key(&me) {
            return Err("You're not in that reading party.".into());
        }
        p.seen.insert(me.clone(), now());
        if p.chapter != chapter || p.page != page {
            p.chapter = chapter;
            p.page = page;
            p.ver += 1;
            p.by = me.clone();
        }
        Ok(state_of(&code, p, &me))
    })
}

pub fn leave(app: &AppHandle, code: &str) -> Result<(), String> {
    let me = uid(app)?;
    let code = clean_code(code);
    with(|all| {
        if let Some(p) = all.get_mut(&code) {
            p.seen.remove(&me);
        }
        all.retain(|_, p| !p.seen.is_empty());
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn party(seen: &[(&str, u64)]) -> Party {
        Party { key: "m1".into(), title: "T".into(), chapter: 5, page: 0, ver: 1, by: "a".into(), seen: seen.iter().map(|(k, v)| (k.to_string(), *v)).collect() }
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
        let p = party(&[("a", 1), ("b", 1)]);
        assert!(state_of("A", &p, "a").mine);
        let s = state_of("A", &p, "b");
        assert!(!s.mine);
        assert_eq!((s.chapter, s.page, s.people), (5, 0, 2));
    }
}
