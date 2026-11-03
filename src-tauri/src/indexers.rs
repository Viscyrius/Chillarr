//! The indexers everyone gets, added by themselves. A site that's down, or
//! blocked right now, is tried again later with growing gaps until it works.
//! One the user removed in Prowlarr afterwards stays removed.

use crate::log::log;
use crate::model::{AppId, Level};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager};

const WATCH_EVERY: Duration = Duration::from_secs(5 * 60);
/// A dead site's test can run a long time; it shouldn't hold up the others.
const ONE_SITE: Duration = Duration::from_secs(120);
const AT_ONCE: usize = 3;

pub struct Site {
    pub def: &'static str,
    pub name: &'static str,
    pub why: &'static str,
}

// Each one passed Prowlarr's test and returned results on 2026-10-18.
// Anidex (down), Uindex and TorrentsCSV (rate-limited Sonarr and Radarr's sync) were left out.
pub const SITES: [Site; 11] = [
    Site { def: "1337x", name: "1337x", why: "Huge catalogue of movies and series. Goes through FlareSolverr." },
    Site { def: "Knaben", name: "Knaben", why: "Searches many big sites at once. Great for movies and series." },
    Site { def: "thepiratebay", name: "The Pirate Bay", why: "The biggest general site." },
    Site { def: "limetorrents", name: "LimeTorrents", why: "Movies and series, a good backup." },
    Site { def: "yts", name: "YTS", why: "Small, good-quality movie files." },
    Site { def: "eztv", name: "EZTV", why: "TV episodes, fast after they air." },
    Site { def: "nyaasi", name: "Nyaa.si", why: "The go-to site for anime." },
    Site { def: "animetosho-xyz", name: "Anime Tosho", why: "Anime, mirrors Nyaa and more." },
    Site { def: "SubsPlease", name: "SubsPlease", why: "New anime episodes, out within hours." },
    Site { def: "nekobt", name: "nekoBT", why: "Anime, well sorted releases." },
    Site { def: "tokyotosho", name: "Tokyo Toshokan", why: "Older and rarer anime." },
];

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
pub struct Waiting {
    pub tries: u32,
    pub next: u64,
    pub why: String,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct Memory {
    /// Added at some point, by Chillarr or the user. Gone from Prowlarr now = removed on purpose.
    #[serde(default)]
    pub added: Vec<String>,
    #[serde(default)]
    pub waiting: HashMap<String, Waiting>,
}

/// 30 minutes, 1, 2, 4, then every 6 hours.
pub fn gap(tries: u32) -> u64 {
    (30 * 60u64).saturating_mul(1u64 << tries.saturating_sub(1).min(4)).min(6 * 3600)
}

/// Which sites to add now. `force`: ignore the wait (the setup screen, a retry).
pub fn due(have: &[String], mem: &Memory, now: u64, force: bool) -> Vec<&'static str> {
    SITES
        .iter()
        .filter(|s| !have.iter().any(|h| h == s.def))
        .filter(|s| !mem.added.iter().any(|a| a == s.def))
        .filter(|s| force || mem.waiting.get(s.def).is_none_or(|w| w.next <= now))
        .map(|s| s.def)
        .collect()
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn path(app: &AppHandle) -> PathBuf {
    app.state::<crate::SettingsState>().path.with_file_name("indexers-auto.json")
}

fn load(app: &AppHandle) -> Memory {
    std::fs::read_to_string(path(app)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save(app: &AppHandle, m: &Memory) {
    let p = path(app);
    let tmp = p.with_extension("tmp");
    if std::fs::write(&tmp, serde_json::to_vec(m).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

/// A site added from Settings counts as added for good.
pub fn note_added(app: &AppHandle, def: &str) {
    let mut m = load(app);
    if !m.added.iter().any(|a| a == def) {
        m.added.push(def.to_string());
    }
    m.waiting.remove(def);
    save(app, &m);
}

pub struct Summary {
    pub added: usize,
    pub total: usize,
    pub waiting: Vec<String>,
}

fn name_of(def: &str) -> String {
    SITES.iter().find(|s| s.def == def).map_or(def, |s| s.name).to_string()
}

async fn in_prowlarr(app: &AppHandle) -> Option<Vec<String>> {
    let core = app.state::<crate::fix::Core>();
    let p = core.config(AppId::Prowlarr).as_ref().and_then(crate::arr::Arr::new)?;
    let list = p.list("indexer").await.ok()?;
    Some(list.iter().filter_map(|i| i["definitionName"].as_str().map(String::from)).collect())
}

/// One at a time, this runs; a second caller just waits its turn.
static RUNNING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Adds what's due. `step(added, total, waiting)` follows along. Never fails:
/// what can't be added now waits for its next try.
pub async fn fill(app: &AppHandle, force: bool, step: impl Fn(usize, usize, Vec<String>)) -> Summary {
    let _one = RUNNING.lock().await;
    let total = SITES.len();
    let Some(have) = in_prowlarr(app).await else {
        // Prowlarr itself isn't up: everything waits, and the watcher comes back.
        let waiting: Vec<String> = SITES.iter().map(|s| s.name.to_string()).collect();
        return Summary { added: 0, total, waiting };
    };
    let mut mem = load(app);
    // What's there already counts as added, so taking it out later sticks.
    for h in &have {
        if SITES.iter().any(|s| s.def == h) && !mem.added.contains(h) {
            mem.added.push(h.clone());
        }
        mem.waiting.remove(h);
    }
    let todo = due(&have, &mem, now(), force);
    let in_place = |m: &Memory| SITES.iter().filter(|s| m.added.iter().any(|a| a == s.def)).count();
    let waiting_now = |m: &Memory| SITES.iter().filter(|s| !m.added.iter().any(|a| a == s.def)).map(|s| s.name.to_string()).collect::<Vec<_>>();
    step(in_place(&mem), total, waiting_now(&mem));
    if todo.is_empty() {
        save(app, &mem);
        return Summary { added: in_place(&mem), total, waiting: waiting_now(&mem) };
    }
    // Before any site runs at once: setting FlareSolverr up from two at a time made two proxies.
    if todo.iter().any(|d| crate::media::behind_cloudflare(d)) {
        crate::media::prepare_flare(app).await;
    }
    let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(AT_ONCE));
    let mut set = tokio::task::JoinSet::new();
    for def in todo {
        let (app, permits) = (app.clone(), permits.clone());
        set.spawn(async move {
            let _p = permits.acquire().await;
            let r = tokio::time::timeout(ONE_SITE, crate::media::add_indexer_with(&app, def, false)).await;
            (def, r.unwrap_or_else(|_| Err(format!("{} took too long to answer.", name_of(def)))))
        });
    }
    while let Some(done) = set.join_next().await {
        let Ok((def, r)) = done else { continue };
        match r {
            Ok(_) => {
                mem.added.push(def.to_string());
                mem.waiting.remove(def);
            }
            Err(why) => {
                let w = mem.waiting.entry(def.to_string()).or_default();
                w.tries += 1;
                w.next = now() + gap(w.tries);
                w.why = why.clone();
                // Said once, not on every retry for a site that stays down for days.
                if w.tries == 1 {
                    log(app, Level::Amber, format!("{} can't be added right now, so Chillarr will try again later. {why}", name_of(def)));
                }
            }
        }
        save(app, &mem);
        step(in_place(&mem), total, waiting_now(&mem));
    }
    // One sync for the lot.
    if let Some(p) = app.state::<crate::fix::Core>().config(AppId::Prowlarr).as_ref().and_then(crate::arr::Arr::new) {
        let _ = p.command("ApplicationIndexerSync").await;
    }
    Summary { added: in_place(&mem), total, waiting: waiting_now(&mem) }
}

/// Every site with where it stands, for Settings > Indexers.
pub async fn status(app: &AppHandle) -> serde_json::Value {
    let have = in_prowlarr(app).await;
    let mem = load(app);
    let list = have.clone().unwrap_or_default();
    let sites: Vec<serde_json::Value> = SITES
        .iter()
        .map(|s| {
            let here = list.iter().any(|h| h == s.def);
            let wait = mem.waiting.get(s.def);
            let state = if here {
                "added"
            } else if mem.added.iter().any(|a| a == s.def) {
                "removed"
            } else if wait.is_some() {
                "waiting"
            } else {
                "soon"
            };
            serde_json::json!({
                "def": s.def, "name": s.name, "why": s.why, "state": state,
                "next_in": wait.map(|w| w.next.saturating_sub(now())),
                "note": wait.map(|w| w.why.clone()),
            })
        })
        .collect();
    serde_json::json!({ "ok": have.is_some(), "sites": sites })
}

/// Keeps trying in the background, once setup is done.
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(90)).await;
        loop {
            let done = app.state::<crate::SettingsState>().current.lock().unwrap().setup_done;
            if done {
                fill(&app, false, |_, _, _| {}).await;
            }
            tokio::time::sleep(WATCH_EVERY).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn have(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn adds_only_what_isnt_there() {
        let list = due(&have(&["1337x", "yts"]), &Memory::default(), 100, false);
        assert_eq!(list.len(), SITES.len() - 2);
        assert!(!list.contains(&"1337x"));
        assert!(list.contains(&"nyaasi"));
    }

    #[test]
    fn a_removed_site_stays_removed() {
        let mem = Memory { added: have(&["1337x"]), ..Default::default() };
        assert!(!due(&[], &mem, 100, false).contains(&"1337x"));
        assert!(!due(&[], &mem, 100, true).contains(&"1337x"), "even when forced");
    }

    #[test]
    fn a_site_that_failed_waits_for_its_turn_unless_forced() {
        let mut mem = Memory::default();
        mem.waiting.insert("yts".into(), Waiting { tries: 1, next: 500, why: "down".into() });
        assert!(!due(&[], &mem, 499, false).contains(&"yts"));
        assert!(due(&[], &mem, 500, false).contains(&"yts"));
        assert!(due(&[], &mem, 1, true).contains(&"yts"));
    }

    #[test]
    fn gaps_grow_and_stop_at_six_hours() {
        assert_eq!(gap(1), 1800);
        assert_eq!(gap(2), 3600);
        assert_eq!(gap(3), 7200);
        assert_eq!(gap(4), 14400);
        assert_eq!(gap(5), 21600);
        assert_eq!(gap(40), 21600);
    }

    #[test]
    fn memory_reads_an_empty_file() {
        let m: Memory = serde_json::from_str("{}").unwrap();
        assert!(m.added.is_empty() && m.waiting.is_empty());
    }
}
