//! A torrent nobody shares is switched for another copy. Sonarr and Radarr
//! never give up on a download by themselves: a "Dual Audio [ENG]" release
//! with no seeders sat in qBittorrent at 0% for good, shown as downloading
//! the whole time (2026-11-13). Here a Sonarr/Radarr download with no seeders
//! and no new bytes for long enough is removed and blocklisted (so that copy
//! is never taken again), and the episodes or film are searched for again,
//! which takes the next best copy.
//!
//! Watch now's torrents are its own and are left alone, as is anything
//! Sonarr/Radarr didn't send (books, torrents added by hand).

use crate::log::log;
use crate::model::{AppId, Level};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

/// No seeder connected and nothing new for this long: switched.
const DEAD_AFTER: Duration = Duration::from_secs(15 * 60);
/// When the tracker still lists seeders that just aren't connecting, a little longer.
const LISTED_AFTER: Duration = Duration::from_secs(45 * 60);
/// Switches for the same episodes or film, at most: after that the last copy
/// is left and the log says none of the copies found are shared.
const MAX_SWITCHES: u32 = 4;

/// Whether a torrent looks dead right now: still wanted, nothing coming, nobody to get it from.
pub fn dead_now(t: &Value) -> bool {
    let state = t["state"].as_str().unwrap_or_default();
    let waiting = matches!(state, "stalledDL" | "metaDL" | "forcedMetaDL");
    waiting && t["progress"].as_f64().unwrap_or(0.0) < 1.0 && t["num_seeds"].as_i64().unwrap_or(0) == 0
}

/// How long a dead torrent gets before it's switched.
pub fn patience(t: &Value) -> Duration {
    if t["num_complete"].as_i64().unwrap_or(0) > 0 { LISTED_AFTER } else { DEAD_AFTER }
}

/// One torrent being watched: since when it's been dead, and its bytes then.
#[derive(Clone, Copy, Debug)]
pub struct Watch {
    since: Instant,
    bytes: i64,
}

/// The next watch for a torrent, or None when it's alive (or gained bytes).
pub fn step(prev: Option<Watch>, t: &Value, now: Instant) -> Option<Watch> {
    if !dead_now(t) {
        return None;
    }
    let bytes = t["downloaded"].as_i64().unwrap_or(0);
    match prev {
        Some(w) if w.bytes == bytes => Some(w),
        _ => Some(Watch { since: now, bytes }),
    }
}

pub fn due(w: &Watch, t: &Value, now: Instant) -> bool {
    now.duration_since(w.since) >= patience(t)
}

#[derive(Default)]
struct State {
    watch: HashMap<String, Watch>,
    /// What a switch was for ("sonarr:ep:1,2" / "radarr:movie:7") -> times.
    switches: HashMap<String, u32>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// The queue rows of one download, and what they're for.
fn rows_of<'a>(rows: &'a [Value], hash: &str) -> Vec<&'a Value> {
    rows.iter().filter(|r| r["downloadId"].as_str().is_some_and(|d| d.eq_ignore_ascii_case(hash))).collect()
}

/// The search that gets the same thing again, and a key for counting switches.
pub fn again(owner: AppId, rows: &[&Value]) -> Option<(Value, String)> {
    match owner {
        AppId::Sonarr => {
            let mut eps: Vec<i64> = rows.iter().filter_map(|r| r["episodeId"].as_i64()).collect();
            eps.sort_unstable();
            eps.dedup();
            if eps.is_empty() {
                return None;
            }
            let key = format!("sonarr:ep:{}", eps.iter().map(i64::to_string).collect::<Vec<_>>().join(","));
            Some((json!({ "name": "EpisodeSearch", "episodeIds": eps }), key))
        }
        AppId::Radarr => {
            let id = rows.iter().find_map(|r| r["movieId"].as_i64())?;
            Some((json!({ "name": "MoviesSearch", "movieIds": [id] }), format!("radarr:movie:{id}")))
        }
        _ => None,
    }
}

/// One look at qBittorrent; run from the downloads tick (every 2 minutes while anything downloads).
pub async fn tick(app: &AppHandle) {
    let Some(q) = crate::waiting::qbit(app) else { return };
    let Ok(all) = q.list().await else { return };
    let now = Instant::now();
    let watch_now = app.state::<crate::watchnow::WatchNow>().owned_set();
    let mut ripe: Vec<(String, String)> = Vec::new();
    {
        let mut g = STATE.lock().unwrap();
        let st = g.get_or_insert_with(State::default);
        let mut next = HashMap::new();
        for t in &all {
            let Some(hash) = t["hash"].as_str().map(str::to_ascii_lowercase) else { continue };
            if watch_now.contains(&hash) {
                continue;
            }
            if let Some(w) = step(st.watch.get(&hash).copied(), t, now) {
                if due(&w, t, now) {
                    ripe.push((hash.clone(), t["name"].as_str().unwrap_or("a download").to_string()));
                }
                next.insert(hash, w);
            }
        }
        st.watch = next;
    }
    if ripe.is_empty() {
        return;
    }
    for owner in [AppId::Sonarr, AppId::Radarr] {
        let Some(a) = crate::media::arr(app, owner) else { continue };
        let Ok(v) = a.get_one("queue?pageSize=2000").await else { continue };
        let Some(rows) = v["records"].as_array() else { continue };
        for (hash, name) in &ripe {
            let mine = rows_of(rows, hash);
            if mine.is_empty() {
                continue;
            }
            let Some((search, key)) = again(owner, &mine) else { continue };
            let times = STATE.lock().unwrap().get_or_insert_with(State::default).switches.get(&key).copied().unwrap_or(0);
            if times >= MAX_SWITCHES {
                continue;
            }
            let ids: Vec<i64> = mine.iter().filter_map(|r| r["id"].as_i64()).collect();
            // Blocklisted so the same copy is never taken again; the search below takes the next one.
            if let Err(e) = a.delete_with("queue/bulk?removeFromClient=true&blocklist=true&skipRedownload=true", &json!({ "ids": ids })).await {
                log(app, Level::Amber, format!("{name} has nobody sharing it, but it couldn't be removed: {e}"));
                continue;
            }
            {
                let mut g = STATE.lock().unwrap();
                let st = g.get_or_insert_with(State::default);
                *st.switches.entry(key).or_default() += 1;
                st.watch.remove(hash);
            }
            let found = a.create("command", &search).await.is_ok();
            let msg = match (found, times + 1 >= MAX_SWITCHES) {
                (true, false) => format!("Nobody was sharing {name}, so it was dropped and another copy is being found."),
                (true, true) => format!("Nobody was sharing {name}, so it was dropped. That's {MAX_SWITCHES} dead copies in a row; the next one found is kept even if it's slow."),
                (false, _) => format!("Nobody was sharing {name}, so it was dropped. Search for it again in Sonarr or Radarr."),
            };
            log(app, Level::Amber, msg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(state: &str, seeds: i64, listed: i64, done: f64, bytes: i64) -> Value {
        json!({ "hash": "ab", "state": state, "num_seeds": seeds, "num_complete": listed, "progress": done, "downloaded": bytes })
    }

    #[test]
    fn only_stuck_torrents_with_nobody_count() {
        assert!(dead_now(&t("stalledDL", 0, 0, 0.0, 0)));
        assert!(dead_now(&t("metaDL", 0, 0, 0.0, 0)));
        assert!(!dead_now(&t("stalledDL", 1, 1, 0.2, 5)), "a seeder is connected");
        assert!(!dead_now(&t("downloading", 0, 0, 0.2, 5)), "bytes are coming");
        assert!(!dead_now(&t("stoppedDL", 0, 0, 0.2, 5)), "stopped by hand");
        assert!(!dead_now(&t("pausedDL", 0, 0, 0.2, 5)), "paused by hand");
        assert!(!dead_now(&t("stalledUP", 0, 0, 1.0, 5)), "finished");
    }

    #[test]
    fn new_bytes_restart_the_clock() {
        let t0 = Instant::now();
        let w = step(None, &t("stalledDL", 0, 0, 0.1, 100), t0).unwrap();
        let later = t0 + Duration::from_secs(10 * 60);
        let same = step(Some(w), &t("stalledDL", 0, 0, 0.1, 100), later).unwrap();
        assert_eq!(same.since, t0);
        let grew = step(Some(w), &t("stalledDL", 0, 0, 0.1, 200), later).unwrap();
        assert_eq!(grew.since, later);
        assert!(step(Some(w), &t("downloading", 2, 2, 0.1, 200), later).is_none());
    }

    #[test]
    fn switched_after_patience_longer_when_tracker_lists_seeders() {
        let t0 = Instant::now();
        let none = t("stalledDL", 0, 0, 0.0, 0);
        let w = step(None, &none, t0).unwrap();
        assert!(!due(&w, &none, t0 + Duration::from_secs(14 * 60)));
        assert!(due(&w, &none, t0 + DEAD_AFTER));
        let listed = t("stalledDL", 0, 3, 0.0, 0);
        assert!(!due(&w, &listed, t0 + DEAD_AFTER));
        assert!(due(&w, &listed, t0 + LISTED_AFTER));
    }

    #[test]
    fn searches_again_for_what_the_download_was_for() {
        let rows = [json!({ "id": 1, "episodeId": 12 }), json!({ "id": 2, "episodeId": 11 }), json!({ "id": 3, "episodeId": 12 })];
        let refs: Vec<&Value> = rows.iter().collect();
        let (cmd, key) = again(AppId::Sonarr, &refs).unwrap();
        assert_eq!(cmd["episodeIds"], json!([11, 12]));
        assert_eq!(key, "sonarr:ep:11,12");
        let m = [json!({ "id": 9, "movieId": 7 })];
        let (cmd, key) = again(AppId::Radarr, &m.iter().collect::<Vec<_>>()).unwrap();
        assert_eq!(cmd, json!({ "name": "MoviesSearch", "movieIds": [7] }));
        assert_eq!(key, "radarr:movie:7");
        assert!(again(AppId::Sonarr, &[]).is_none());
    }
}
