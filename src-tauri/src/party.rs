//! Host tools shared by Watch together (syncplay.rs) and Read Party
//! (readparty.rs): who may come in. The host can leave a party open, ask for
//! a password, or lock it; and people the host removed stay out for a while
//! unless the host lets them back. Kept in memory on the PC, which every
//! device joins through. A password is only ever kept hashed, like a PIN.

use serde::Serialize;
use std::collections::HashMap;

/// How long someone the host removed stays out.
pub const REMOVED_SECS: u64 = 600;
/// Wrong passwords in a row before a wait, and how long (like the PIN lockout).
const TRIES: u32 = 5;
const TRIES_WAIT: u64 = 30;
pub const PASSWORD_MIN: usize = 4;
pub const PASSWORD_MAX: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Lock {
    #[default]
    Open,
    Password,
    Locked,
}

impl Lock {
    pub fn parse(s: &str) -> Result<Lock, String> {
        match s {
            "open" => Ok(Lock::Open),
            "password" => Ok(Lock::Password),
            "locked" => Ok(Lock::Locked),
            _ => Err("Pick Open, Password or Locked.".into()),
        }
    }
}

/// Someone the host removed, for the host's "Removed" list.
#[derive(Clone, Serialize, Debug, PartialEq)]
pub struct Removed {
    pub id: String,
    pub name: String,
    /// Minutes until they may come back on their own.
    pub mins: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Gate {
    pub lock: Lock,
    /// `salt:hash` (profiles.rs `seal_secret`), never the password itself.
    password: Option<String>,
    /// User id -> (name, may come back at).
    removed: HashMap<String, (String, u64)>,
    /// User id -> (wrong tries in a row, wait until).
    fails: HashMap<String, (u32, u64)>,
}

/// A password as the host typed it, or why not.
pub fn check_password(p: &str) -> Result<&str, String> {
    let n = p.chars().count();
    if !(PASSWORD_MIN..=PASSWORD_MAX).contains(&n) {
        return Err(format!("A party password is {PASSWORD_MIN} to {PASSWORD_MAX} characters."));
    }
    Ok(p)
}

impl Gate {
    /// The host's new setting. A password is needed to switch to Password
    /// unless one is already set; a new one replaces it. People already in stay in.
    pub fn set(&mut self, lock: Lock, password: Option<&str>) -> Result<(), String> {
        let typed = password.filter(|p| !p.is_empty());
        if lock == Lock::Password {
            match typed {
                Some(p) => self.password = Some(crate::profiles::seal_secret(check_password(p)?)?),
                None if self.password.is_some() => {}
                None => return Err("Type a password for the party first.".into()),
            }
        }
        self.lock = lock;
        if lock == Lock::Open {
            self.password = None;
        }
        Ok(())
    }

    /// Whether `uid` may come in now with `password`, counting a wrong try.
    pub fn admit(&mut self, uid: &str, password: Option<&str>, now: u64) -> Result<(), String> {
        self.removed.retain(|_, (_, until)| *until > now);
        if self.removed.contains_key(uid) {
            return Err("The host removed you from this party. Try again later, or ask the host to let you back.".into());
        }
        match self.lock {
            Lock::Open => Ok(()),
            Lock::Locked => Err("This party is locked. Ask the host to open it.".into()),
            Lock::Password => {
                let (n, until) = self.fails.get(uid).copied().unwrap_or((0, 0));
                if until > now {
                    return Err(format!("Too many wrong tries. Wait {} seconds.", until - now));
                }
                let Some(p) = password.filter(|p| !p.is_empty()) else { return Err("This party needs a password.".into()) };
                let ok = self.password.as_deref().is_some_and(|h| crate::profiles::secret_matches(h, p));
                if ok {
                    self.fails.remove(uid);
                    return Ok(());
                }
                let n = n + 1;
                if n >= TRIES {
                    self.fails.insert(uid.to_string(), (0, now + TRIES_WAIT));
                    Err(format!("Wrong password. Too many tries: wait {TRIES_WAIT} seconds."))
                } else {
                    self.fails.insert(uid.to_string(), (n, 0));
                    Err("Wrong password.".into())
                }
            }
        }
    }

    /// Whether `uid` would be turned away now without being let in by the host
    /// (someone who got in around Chillarr, with another Jellyfin app).
    pub fn closed_to(&mut self, uid: &str, now: u64) -> bool {
        self.removed.retain(|_, (_, until)| *until > now);
        self.lock != Lock::Open || self.removed.contains_key(uid)
    }

    pub fn remove(&mut self, uid: &str, name: &str, now: u64) {
        self.removed.insert(uid.to_string(), (name.to_string(), now + REMOVED_SECS));
    }

    pub fn allow_back(&mut self, uid: &str) {
        self.removed.remove(uid);
    }

    pub fn removed(&mut self, now: u64) -> Vec<Removed> {
        self.removed.retain(|_, (_, until)| *until > now);
        let mut out: Vec<Removed> = self.removed.iter().map(|(id, (name, until))| Removed { id: id.clone(), name: name.clone(), mins: (until - now).div_ceil(60) }).collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_lets_anyone_in_and_locked_nobody() {
        let mut g = Gate::default();
        assert!(g.admit("a", None, 1).is_ok());
        g.set(Lock::Locked, None).unwrap();
        assert!(g.admit("a", Some("anything"), 1).is_err());
        assert!(g.closed_to("b", 1));
    }

    #[test]
    fn a_password_is_kept_hashed_and_checked() {
        let mut g = Gate::default();
        assert!(g.set(Lock::Password, None).is_err(), "needs one the first time");
        assert!(g.set(Lock::Password, Some("abc")).is_err(), "too short");
        g.set(Lock::Password, Some("popcorn")).unwrap();
        assert!(!g.password.as_deref().unwrap().contains("popcorn"));
        assert_eq!(g.admit("a", Some("Popcorn"), 1), Err("Wrong password.".into()));
        assert!(g.admit("a", None, 1).is_err());
        assert!(g.admit("a", Some("popcorn"), 1).is_ok());
        g.set(Lock::Password, Some("")).unwrap();
        assert!(g.admit("b", Some("popcorn"), 1).is_ok(), "keeps the one it has");
        g.set(Lock::Open, None).unwrap();
        assert!(g.password.is_none());
    }

    #[test]
    fn five_wrong_tries_wait_thirty_seconds() {
        let mut g = Gate::default();
        g.set(Lock::Password, Some("popcorn")).unwrap();
        for _ in 0..4 {
            assert_eq!(g.admit("a", Some("x"), 100), Err("Wrong password.".into()));
        }
        assert!(g.admit("a", Some("x"), 100).unwrap_err().contains("wait 30"));
        assert!(g.admit("a", Some("popcorn"), 110).unwrap_err().contains("Wait 20"), "even the right one waits");
        assert!(g.admit("b", Some("popcorn"), 110).is_ok(), "per person");
        assert!(g.admit("a", Some("popcorn"), 130).is_ok());
    }

    #[test]
    fn removed_people_stay_out_for_ten_minutes_unless_let_back() {
        let mut g = Gate::default();
        g.remove("a", "Alex", 1000);
        assert!(g.admit("a", None, 1000 + REMOVED_SECS - 1).is_err());
        assert!(g.closed_to("a", 1001));
        assert_eq!(g.removed(1000)[0].mins, 10);
        assert!(g.admit("a", None, 1000 + REMOVED_SECS).is_ok());
        g.remove("b", "Bea", 1000);
        g.allow_back("b");
        assert!(g.admit("b", None, 1001).is_ok());
        g.set(Lock::Password, Some("popcorn")).unwrap();
        g.remove("c", "Cy", 1000);
        assert!(g.admit("c", Some("popcorn"), 1001).is_err(), "a password doesn't get round it");
    }
}
