//! Whether anyone is using Chillarr right now. Nobody on it: the app only
//! keeps up with what's new (new films, episodes and chapters) and finishes
//! downloads; everything else waits until someone opens a page, taps, plays.
//! Before this, with nobody on it, Jellyfin was asked about every profile's
//! library every 30 seconds and Home was rebuilt every 20 (Jellyfin sat at
//! about 4% CPU, measured 2026-11-12).
//!
//! "Using" is: real input on a page or something playing in the last
//! `RECENT` (seen.rs and the player's reports say so), or a phone or TV page
//! with its event stream open. A window left open on the PC with nobody at it
//! doesn't count: it only stops its own timers when hidden (`window_shown`).
//! A loop sleeps with `nap`, which wakes early the moment someone starts.

use serde_json::Value;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

/// Input this recent still counts as someone on it.
pub const RECENT: Duration = Duration::from_secs(15 * 60);

static LAST: AtomicI64 = AtomicI64::new(0);
static WINDOW_SHOWN: AtomicBool = AtomicBool::new(false);

fn woke() -> &'static tokio::sync::Notify {
    static N: OnceLock<tokio::sync::Notify> = OnceLock::new();
    N.get_or_init(tokio::sync::Notify::new)
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Someone did something: loops waiting out an idle nap wake up.
pub fn touch() {
    let before = LAST.swap(now(), Ordering::Relaxed);
    if now() - before > 60 {
        woke().notify_waiters();
    }
}

/// The PC window was shown or hidden (to the tray).
pub fn window_shown(on: bool) {
    WINDOW_SHOWN.store(on, Ordering::Relaxed);
    if on {
        touch();
    }
}

pub fn window_is_shown() -> bool {
    WINDOW_SHOWN.load(Ordering::Relaxed)
}

/// The rule, apart from where its inputs come from.
pub fn in_use(last_input: i64, now: i64, within: Duration, pages_open: bool) -> bool {
    pages_open || (last_input > 0 && now - last_input < within.as_secs() as i64)
}

/// Is anyone using Chillarr now?
pub fn active() -> bool {
    in_use(LAST.load(Ordering::Relaxed), now(), RECENT, crate::bridge::any_page_open())
}

/// How long a loop waits: `busy` when in use (or work is under way), `idle` otherwise.
pub fn wait_for(in_use: bool, working: bool, busy: Duration, idle: Duration) -> Duration {
    if in_use || working { busy } else { idle }
}

/// Sleeps `busy` while someone is on it or `working`, else `idle`, waking
/// early when someone starts.
pub async fn nap(working: bool, busy: Duration, idle: Duration) {
    let d = wait_for(active(), working, busy, idle);
    if d == busy {
        tokio::time::sleep(d).await;
        return;
    }
    tokio::select! {
        _ = tokio::time::sleep(d) => {}
        _ = woke().notified() => {}
    }
}

/// A torrent list from qBittorrent has something coming in.
pub fn downloading(torrents: &[Value]) -> bool {
    torrents.iter().any(|t| {
        matches!(t["state"].as_str().unwrap_or(""), "downloading" | "metaDL" | "forcedDL" | "stalledDL" | "queuedDL" | "checkingDL" | "allocating" | "forcedMetaDL")
    })
}

/// Anything downloading right now (asked of qBittorrent; no answer counts as no).
pub async fn downloads_running(app: &tauri::AppHandle) -> bool {
    match crate::waiting::qbit(app) {
        Some(q) => q.list().await.map(|l| downloading(&l)).unwrap_or(false),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn nobody_on_it_is_idle() {
        let now = 10_000;
        assert!(!in_use(0, now, RECENT, false), "never touched, no pages");
        assert!(!in_use(now - 16 * 60, now, RECENT, false), "last input or play 16 minutes ago");
        assert!(in_use(now - 60, now, RECENT, false), "input or a play report a minute ago");
        assert!(in_use(0, now, RECENT, true), "a phone has a page open");
    }

    #[test]
    fn idle_loops_wait_longer_unless_something_downloads() {
        let (b, i) = (Duration::from_secs(30), Duration::from_secs(600));
        assert_eq!(wait_for(false, false, b, i), i);
        assert_eq!(wait_for(true, false, b, i), b);
        assert_eq!(wait_for(false, true, b, i), b, "a download finishing still gets noticed soon");
    }

    #[test]
    fn only_incoming_torrents_count_as_downloading() {
        assert!(downloading(&[json!({ "state": "stalledUP" }), json!({ "state": "downloading" })]));
        assert!(!downloading(&[json!({ "state": "uploading" }), json!({ "state": "pausedUP" }), json!({ "state": "stoppedDL" })]));
    }
}
