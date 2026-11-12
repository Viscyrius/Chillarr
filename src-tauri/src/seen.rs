//! When each profile was last on Chillarr, from any device. Activity used
//! Jellyfin's LastActivityDate alone, and reading never goes through Jellyfin:
//! a profile reading manhwa on a phone for two days showed "Last on 2 days
//! ago" (2026-11-12). Every page (PC window or a browser) reports real use (a
//! tap, a key, a scroll) at most once a minute; Activity shows the latest of
//! this, Jellyfin's date and the profile's last reading progress.

use std::collections::HashMap;
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

/// Written to disk at most this often (seconds); the latest is in memory.
const FLUSH_EVERY: i64 = 60;

#[derive(Default)]
pub struct Seen {
    at: Mutex<HashMap<String, i64>>,
    flushed: Mutex<i64>,
}

/// Whether to write now, the last write having been at `last`.
pub fn should_flush(last: i64, now: i64) -> bool {
    now - last >= FLUSH_EVERY
}

/// The newest of several times (unix seconds), if any.
pub fn latest(times: &[Option<i64>]) -> Option<i64> {
    times.iter().flatten().copied().max()
}

fn path(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("activity.json"))
}

fn loaded(app: &AppHandle) -> std::sync::MutexGuard<'_, HashMap<String, i64>> {
    let s: &Seen = app.state::<Seen>().inner();
    let mut at = s.at.lock().unwrap();
    if at.is_empty() {
        if let Some(saved) = path(app).and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str::<HashMap<String, i64>>(&t).ok()) {
            *at = saved;
        }
    }
    at
}

/// The profile using this page did something just now.
pub fn touch(app: &AppHandle, uid: &str) {
    let now = chrono::Utc::now().timestamp();
    let snapshot = {
        let mut at = loaded(app);
        at.insert(uid.to_string(), now);
        let s = app.state::<Seen>();
        let mut last = s.flushed.lock().unwrap();
        if !should_flush(*last, now) {
            return;
        }
        *last = now;
        at.clone()
    };
    if let (Some(p), Ok(t)) = (path(app), serde_json::to_string(&snapshot)) {
        let _ = std::fs::write(p, t);
    }
}

/// When a profile was last seen on Chillarr (unix seconds).
pub fn get(app: &AppHandle, uid: &str) -> Option<i64> {
    loaded(app).get(uid).copied()
}

/// A removed profile is forgotten.
pub fn forget(app: &AppHandle, uid: &str) {
    loaded(app).remove(uid);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seen_writes_at_most_once_a_minute() {
        assert!(should_flush(0, 1_000));
        assert!(!should_flush(1_000, 1_030));
        assert!(should_flush(1_000, 1_060));
    }

    #[test]
    fn activity_takes_latest_of_three() {
        assert_eq!(latest(&[Some(10), None, Some(30)]), Some(30), "reading on a phone beats Jellyfin's older date");
        assert_eq!(latest(&[None, None, None]), None);
    }
}
