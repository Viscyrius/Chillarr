//! Limit downloads: everything qBittorrent is doing together stays within 20% of
//! the internet connection, so friends' downloads don't get in the way of a game
//! on this PC. Upload too, since a full upload is what lag usually comes from.
//!
//! For the PC's owner only. Nothing here is in bridge.rs's list, and the settings
//! are in their own file rather than `Settings`, which web admins can read.
//!
//! qBittorrent has one overall limit and can't let a single torrent past it, so
//! "what I start here ignores the limit" can't be done with that alone. Each
//! limited torrent gets its own share of the 20% instead, worked out again every
//! few seconds. The overall limit is on as well while nothing is exempt: it holds
//! exactly, and it covers a torrent added between two rounds.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Manager};

const PERCENT: u64 = 20;
/// Never so low that a download looks dead on a slow line, in bytes a second.
const LEAST: u64 = 32 * 1024;
const EVERY: Duration = Duration::from_secs(3);
/// Reading Radarr's and Sonarr's queues is slower, and only needed for new marks.
const QUEUES_EVERY: u32 = 5;
const MARK_KEEP_SECS: i64 = 14 * 24 * 3600;
const MAX_MARKS: usize = 200;

/// A title asked for in this PC's own window.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Mark {
    pub tmdb: Option<i64>,
    pub tvdb: Option<i64>,
    /// Squashed, for a title that came without the id its app knows it by.
    pub title: String,
    pub at: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
struct Saved {
    on: bool,
    /// What's started in this PC's window ignores the limit.
    host_free: bool,
    /// The connection, in bytes a second.
    down: u64,
    up: u64,
    /// qBittorrent's overall limits before this took them over.
    prior: Option<(i64, i64)>,
    marks: Vec<Mark>,
    /// Lower case. The torrents those marks turned out to be.
    free_hashes: Vec<String>,
}

static STATE: Mutex<Option<Saved>> = Mutex::new(None);

fn path(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("limit.json"))
}

fn with<R>(app: &AppHandle, f: impl FnOnce(&mut Saved) -> R) -> R {
    let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let s = g.get_or_insert_with(|| path(app).and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default());
    f(s)
}

fn save(app: &AppHandle) {
    let text = with(app, |s| serde_json::to_string_pretty(s).ok());
    if let (Some(p), Some(t)) = (path(app), text) {
        let tmp = p.with_extension("json.tmp");
        if std::fs::write(&tmp, t).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// `PERCENT` of a connection, in bytes a second.
fn cap_of(line: u64) -> u64 {
    (line / 100 * PERCENT).max(LEAST)
}

fn mbps(bytes_a_second: u64) -> f64 {
    (bytes_a_second as f64 * 8.0 / 1e5).round() / 10.0
}

fn from_mbps(m: f64) -> u64 {
    if m.is_finite() && m > 0.0 { (m * 1e6 / 8.0) as u64 } else { 0 }
}

/// Remembers a title asked for in this PC's window. Called only from the window's
/// own commands: background work runs with no viewer too, so that can't tell.
pub fn mark_host(app: &AppHandle, tile: &crate::tiles::Tile) {
    let mark = Mark { tmdb: tile.tmdb, tvdb: tile.tvdb, title: crate::media::squash(&tile.title), at: now() };
    with(app, |s| {
        s.marks.retain(|m| !same_title(m, &mark));
        s.marks.push(mark);
        let over = s.marks.len().saturating_sub(MAX_MARKS);
        s.marks.drain(..over);
    });
    save(app);
}

fn same_title(a: &Mark, b: &Mark) -> bool {
    (a.tmdb.is_some() && a.tmdb == b.tmdb) || (a.tvdb.is_some() && a.tvdb == b.tvdb) || (!a.title.is_empty() && a.title == b.title)
}

/// The hash a queue row of Radarr or Sonarr is, when it's a marked title.
fn marked_hash(rec: &Value, marks: &[Mark]) -> Option<String> {
    let hash = rec["downloadId"].as_str().filter(|h| !h.is_empty())?.to_ascii_lowercase();
    let row = Mark {
        tmdb: rec["movie"]["tmdbId"].as_i64(),
        tvdb: rec["series"]["tvdbId"].as_i64(),
        title: crate::media::squash(rec["movie"]["title"].as_str().or_else(|| rec["series"]["title"].as_str()).unwrap_or_default()),
        at: 0,
    };
    marks.iter().any(|m| same_title(m, &row)).then_some(hash)
}

/// Shares `cap` among torrents going at `speeds`. What a slow one isn't using goes
/// to the others, and it can always grow (half as much again each round), but the
/// total never passes `cap`.
pub fn share_out(cap: u64, speeds: &[u64]) -> Vec<u64> {
    let mut order: Vec<usize> = (0..speeds.len()).collect();
    order.sort_by_key(|i| speeds[*i]);
    let mut out = vec![0; speeds.len()];
    let mut left = cap;
    for (done, i) in order.into_iter().enumerate() {
        let fair = left / (speeds.len() - done) as u64;
        // The busiest gets whatever the others left, so the whole share is usable.
        let last = done + 1 == speeds.len();
        let give = if last { left } else { (speeds[i] + speeds[i] / 2).max(fair / 4).min(fair) }.max(1024);
        out[i] = give;
        left = left.saturating_sub(give);
    }
    out
}

fn stopped(t: &Value) -> bool {
    let s = t["state"].as_str().unwrap_or_default();
    s.starts_with("paused") || s.starts_with("stopped") || s == "error" || s == "missingFiles"
}

/// (hash, its limit) for one direction. `speed`/`limit`: the torrent's fields.
/// `wanted`: whether it's moving this way at all. An exempt torrent gets no limit,
/// and one already right isn't touched: a change under 15% isn't worth a call.
fn plan(list: &[Value], free: &HashSet<String>, cap: u64, speed: &str, limit: &str, wanted: impl Fn(&Value) -> bool) -> Vec<(String, u64)> {
    let hash = |t: &Value| t["hash"].as_str().unwrap_or_default().to_ascii_lowercase();
    let mut out = Vec::new();
    let limited: Vec<&Value> = list.iter().filter(|t| !free.contains(&hash(t)) && !stopped(t) && wanted(t)).collect();
    let speeds: Vec<u64> = limited.iter().map(|t| t[speed].as_u64().unwrap_or(0)).collect();
    for (t, give) in limited.iter().zip(share_out(cap, &speeds)) {
        let have = t[limit].as_i64().unwrap_or(0).max(0) as u64;
        if have == 0 || give.abs_diff(have) * 100 > have * 15 {
            out.push((hash(t), give));
        }
    }
    for t in list.iter().filter(|t| free.contains(&hash(t))) {
        if t[limit].as_i64().unwrap_or(0) > 0 {
            out.push((hash(t), 0));
        }
    }
    out
}

fn is_downloading(t: &Value) -> bool {
    t["progress"].as_f64().unwrap_or(0.0) < 1.0
}

/// Whether an exempt torrent is doing anything: then the overall limit must be off.
fn any_free_active(list: &[Value], free: &HashSet<String>) -> bool {
    list.iter().any(|t| !stopped(t) && free.contains(&t["hash"].as_str().unwrap_or_default().to_ascii_lowercase()))
}

async fn read_queues(app: &AppHandle) {
    let marks = with(app, |s| s.marks.clone());
    if marks.is_empty() {
        return;
    }
    let mut found = Vec::new();
    for (id, path) in [(crate::model::AppId::Radarr, "queue?includeMovie=true&pageSize=2000"), (crate::model::AppId::Sonarr, "queue?includeSeries=true&pageSize=2000")] {
        let Some(a) = crate::media::arr(app, id) else { continue };
        if let Ok(v) = a.get_one(path).await {
            found.extend(v["records"].as_array().into_iter().flatten().filter_map(|r| marked_hash(r, &marks)));
        }
    }
    let changed = with(app, |s| {
        let before = s.free_hashes.len();
        for h in found {
            if !s.free_hashes.contains(&h) {
                s.free_hashes.push(h);
            }
        }
        s.free_hashes.len() != before
    });
    if changed {
        save(app);
    }
}

/// One round: every torrent's share, and the overall limit.
async fn round(app: &AppHandle) -> Result<(), crate::qbit::QbError> {
    let Some(q) = crate::waiting::qbit(app) else { return Ok(()) };
    let (down, up, host_free, known) = with(app, |s| (s.down, s.up, s.host_free, s.free_hashes.clone()));
    let list = q.list().await?;
    // A torrent that's gone needs no exemption, and its hash won't come back.
    let there: HashSet<String> = list.iter().filter_map(|t| t["hash"].as_str().map(str::to_ascii_lowercase)).collect();
    if known.iter().any(|h| !there.contains(h)) {
        with(app, |s| {
            s.free_hashes.retain(|h| there.contains(h));
            let old = now() - MARK_KEEP_SECS;
            s.marks.retain(|m| m.at >= old);
        });
        save(app);
    }
    let free: HashSet<String> = if host_free { known.into_iter().filter(|h| there.contains(h)).collect() } else { HashSet::new() };
    let (cap_down, cap_up) = (cap_of(down), cap_of(up));
    // Manga downloads (manga_net.rs) are one more download sharing the same
    // budget: their share comes off what qBittorrent may use.
    let (manga_down, manga_up) = crate::manga_net::take_moved();
    let secs = EVERY.as_secs().max(1);
    let torrents = |field: &str| list.iter().map(|t| t[field].as_u64().unwrap_or(0)).sum::<u64>();
    let split = |cap: u64, torrent_speed: u64, manga_speed: u64| {
        if manga_speed == 0 {
            // Idle: a small allowance so it can start; the torrents keep the rest.
            let m = (cap / 8).max(LEAST);
            return (cap.saturating_sub(m).max(LEAST), m);
        }
        let s = share_out(cap, &[torrent_speed, manga_speed]);
        (s[0].max(LEAST), s[1].max(LEAST))
    };
    let (cap_down, manga_cap_down) = split(cap_down, torrents("dlspeed"), manga_down / secs);
    let (cap_up, manga_cap_up) = split(cap_up, torrents("upspeed"), manga_up / secs);
    crate::manga_net::set_rate(manga_cap_down, manga_cap_up);
    for (hash, limit) in plan(&list, &free, cap_down, "dlspeed", "dl_limit", is_downloading) {
        q.set_torrent_limit("Download", &hash, limit).await?;
    }
    for (hash, limit) in plan(&list, &free, cap_up, "upspeed", "up_limit", |_| true) {
        q.set_torrent_limit("Upload", &hash, limit).await?;
    }
    let exempt = any_free_active(&list, &free);
    let (want_down, want_up) = if exempt { (0, 0) } else { (cap_down as i64, cap_up as i64) };
    let prefs = q.prefs().await?;
    if prefs["dl_limit"].as_i64().unwrap_or(0).max(0) != want_down || prefs["up_limit"].as_i64().unwrap_or(0).max(0) != want_up {
        q.set_prefs(&serde_json::json!({ "dl_limit": want_down, "up_limit": want_up })).await?;
    }
    Ok(())
}

/// Back to how qBittorrent was: no shares, and its own overall limits.
async fn lift(app: &AppHandle) -> Result<(), crate::qbit::QbError> {
    crate::manga_net::set_rate(0, 0);
    let Some(q) = crate::waiting::qbit(app) else { return Ok(()) };
    q.set_torrent_limit("Download", "all", 0).await?;
    q.set_torrent_limit("Upload", "all", 0).await?;
    let (dl, ul) = with(app, |s| s.prior.take()).unwrap_or((0, 0));
    q.set_prefs(&serde_json::json!({ "dl_limit": dl, "up_limit": ul })).await?;
    save(app);
    Ok(())
}

pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut count = 0u32;
        let mut failing = false;
        loop {
            crate::activity::nap(false, EVERY, std::time::Duration::from_secs(30)).await;
            let (on, host_free) = with(&app, |s| (s.on, s.host_free));
            if !on {
                crate::manga_net::set_rate(0, 0);
                continue;
            }
            if host_free && count % QUEUES_EVERY == 0 {
                read_queues(&app).await;
            }
            count = count.wrapping_add(1);
            match round(&app).await {
                Ok(()) => failing = false,
                Err(e) => {
                    if !failing {
                        crate::log::detail(format!("Limit downloads: couldn't set the limits: {e:?}"));
                    }
                    failing = true;
                }
            }
        }
    });
}

/// What the window shows.
pub fn status(app: &AppHandle) -> Value {
    with(app, |s| {
        serde_json::json!({
            "on": s.on,
            "host_free": s.host_free,
            "down_mbps": mbps(s.down),
            "up_mbps": mbps(s.up),
            "cap_down_mbps": mbps(cap_of(s.down)),
            "cap_up_mbps": mbps(cap_of(s.up)),
            "percent": PERCENT,
        })
    })
}

/// Changes the setting and applies it at once.
pub async fn set(app: &AppHandle, on: bool, host_free: bool, down_mbps: f64, up_mbps: f64) -> Result<Value, String> {
    let (down, up) = (from_mbps(down_mbps), from_mbps(up_mbps));
    if on && (down == 0 || up == 0) {
        return Err("Chillarr needs your internet speed first. Press Measure, or type it in.".into());
    }
    let was_on = with(app, |s| s.on);
    if on && !was_on {
        // Saved once, so turning this off gives back what was there.
        let q = crate::waiting::qbit(app).ok_or("qBittorrent isn't set up yet.")?;
        let prefs = q.prefs().await.map_err(|_| "qBittorrent isn't answering. Press Fix everything first.")?;
        let prior = (prefs["dl_limit"].as_i64().unwrap_or(0).max(0), prefs["up_limit"].as_i64().unwrap_or(0).max(0));
        with(app, |s| s.prior = Some(prior));
    }
    with(app, |s| {
        s.on = on;
        s.host_free = host_free;
        s.down = down;
        s.up = up;
    });
    save(app);
    if on {
        if host_free {
            read_queues(app).await;
        }
        round(app).await.map_err(|_| "Saved, but qBittorrent isn't answering, so it starts when qBittorrent is back.".to_string())?;
    } else if was_on {
        lift(app).await.map_err(|_| "Turned off, but qBittorrent isn't answering. Press Fix everything, then turn this on and off again.".to_string())?;
    }
    Ok(status(app))
}

/// 25 MB at a time: it answers 403 to 100 MB in one go (measured 2026-11-05).
const TEST_DOWN: &str = "https://speed.cloudflare.com/__down?bytes=25000000";
const TEST_UP: &str = "https://speed.cloudflare.com/__up";
const TEST_FOR: Duration = Duration::from_secs(6);
/// The first moments of a transfer are slower than the line.
const WARM_UP: Duration = Duration::from_secs(1);

/// Measures the connection, a few seconds each way, and adds what qBittorrent is
/// using meanwhile (the line is bigger than what was left over for the test).
pub async fn measure(app: &AppHandle) -> Result<Value, String> {
    let http = reqwest::Client::builder().connect_timeout(Duration::from_secs(8)).timeout(TEST_FOR + Duration::from_secs(20)).build().map_err(|e| e.to_string())?;
    let fail = "Couldn't measure your internet from here. Type your speed in instead.".to_string();
    let (mut qd, mut qu) = (0, 0);
    if let Some(q) = crate::waiting::qbit(app) {
        if let Ok(v) = q.transfer().await {
            qd = v["dl_info_speed"].as_u64().unwrap_or(0);
            qu = v["up_info_speed"].as_u64().unwrap_or(0);
        }
    }
    let got_down = test_down(&http).await;
    let got_up = test_up(&http).await;
    if got_down == 0 || got_up == 0 {
        return Err(fail);
    }
    Ok(serde_json::json!({ "down_mbps": mbps(got_down + qd), "up_mbps": mbps(got_up + qu) }))
}

async fn test_down(http: &reqwest::Client) -> u64 {
    let start = std::time::Instant::now();
    let mut runs = tokio::task::JoinSet::new();
    for _ in 0..4 {
        let http = http.clone();
        runs.spawn(async move {
            let mut n = 0u64;
            'test: while start.elapsed() < TEST_FOR + WARM_UP {
                let Ok(mut r) = http.get(TEST_DOWN).header("User-Agent", "Chillarr").send().await else { break };
                if !r.status().is_success() {
                    break;
                }
                while start.elapsed() < TEST_FOR + WARM_UP {
                    match tokio::time::timeout(Duration::from_secs(3), r.chunk()).await {
                        Ok(Ok(Some(c))) => {
                            if start.elapsed() > WARM_UP {
                                n += c.len() as u64;
                            }
                        }
                        Ok(Ok(None)) => continue 'test,
                        _ => break 'test,
                    }
                }
            }
            n
        });
    }
    let mut total = 0;
    while let Some(n) = runs.join_next().await {
        total += n.unwrap_or(0);
    }
    // What came in after the warm-up, over how long that really was: a test cut
    // short by a refusal still gives a fair answer, not a low one.
    total * 1000 / (start.elapsed().saturating_sub(WARM_UP).as_millis().max(1000) as u64)
}

async fn test_up(http: &reqwest::Client) -> u64 {
    let start = std::time::Instant::now();
    let mut runs = tokio::task::JoinSet::new();
    for _ in 0..3 {
        let http = http.clone();
        runs.spawn(async move {
            const SIZE: usize = 2 * 1024 * 1024;
            let mut n = 0u64;
            while start.elapsed() < TEST_FOR {
                match http.post(TEST_UP).header("User-Agent", "Chillarr").body(vec![0x5a_u8; SIZE]).send().await {
                    Ok(r) if r.status().is_success() => n += SIZE as u64,
                    _ => break,
                }
            }
            n
        });
    }
    let mut total = 0;
    while let Some(n) = runs.join_next().await {
        total += n.unwrap_or(0);
    }
    (total as f64 / start.elapsed().as_secs_f64().max(1.0)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_budget_is_shared_and_never_passed() {
        let cap = 1_000_000;
        for speeds in [vec![0; 10], vec![900_000; 10], vec![10, 20, 900_000], vec![500_000], vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]] {
            let out = share_out(cap, &speeds);
            assert_eq!(out.len(), speeds.len());
            assert!(out.iter().sum::<u64>() <= cap + 1024 * speeds.len() as u64, "{speeds:?} -> {out:?}");
        }
        assert_eq!(share_out(cap, &[500_000]), vec![1_000_000], "one torrent can use all of it");
        assert_eq!(share_out(cap, &[0]), vec![1_000_000], "even from a standing start");
        let ten = share_out(cap, &[900_000; 10]);
        assert!(ten.iter().all(|x| (90_000..=110_000).contains(x)), "ten busy ones split it evenly: {ten:?}");
    }

    #[test]
    fn what_a_slow_torrent_leaves_goes_to_the_busy_one() {
        let out = share_out(1_000_000, &[1_000, 1_000_000]);
        assert!(out[1] > 700_000, "{out:?}");
        assert!(out[0] >= 125_000, "and the slow one can still grow: {out:?}");
    }

    #[test]
    fn a_marked_title_is_exempt_and_the_rest_are_not() {
        let list = vec![
            json!({ "hash": "AAA", "state": "downloading", "progress": 0.1, "dlspeed": 5000, "dl_limit": 0 }),
            json!({ "hash": "bbb", "state": "downloading", "progress": 0.1, "dlspeed": 5000, "dl_limit": 900 }),
            json!({ "hash": "ccc", "state": "pausedDL", "progress": 0.1, "dlspeed": 0, "dl_limit": 0 }),
        ];
        let free: HashSet<String> = ["bbb".to_string()].into();
        let p = plan(&list, &free, 100_000, "dlspeed", "dl_limit", is_downloading);
        assert_eq!(p, vec![("aaa".to_string(), 100_000), ("bbb".to_string(), 0)]);
        assert!(any_free_active(&list, &free));
        assert!(!any_free_active(&list, &HashSet::new()));
    }

    #[test]
    fn a_small_change_isnt_worth_a_call() {
        let list = vec![json!({ "hash": "a", "state": "downloading", "progress": 0.1, "dlspeed": 100_000, "dl_limit": 95_000 })];
        assert!(plan(&list, &HashSet::new(), 100_000, "dlspeed", "dl_limit", is_downloading).is_empty());
    }

    #[test]
    fn queue_rows_are_matched_to_what_was_asked_for_here() {
        let marks = vec![Mark { tmdb: Some(155), tvdb: None, title: "thedarkknight".into(), at: 1 }, Mark { tmdb: None, tvdb: Some(9), title: "show".into(), at: 1 }];
        assert_eq!(marked_hash(&json!({ "downloadId": "ABC", "movie": { "tmdbId": 155, "title": "x" } }), &marks).as_deref(), Some("abc"));
        assert_eq!(marked_hash(&json!({ "downloadId": "DEF", "series": { "tvdbId": 9, "title": "y" } }), &marks).as_deref(), Some("def"));
        assert_eq!(marked_hash(&json!({ "downloadId": "GHI", "series": { "tvdbId": 1, "title": "Show" } }), &marks).as_deref(), Some("ghi"), "anime: by name");
        assert_eq!(marked_hash(&json!({ "downloadId": "JKL", "movie": { "tmdbId": 7, "title": "Other" } }), &marks), None, "a friend's");
        assert_eq!(marked_hash(&json!({ "movie": { "tmdbId": 155 } }), &marks), None, "no torrent yet");
    }

    #[test]
    fn speeds_go_both_ways() {
        assert_eq!(mbps(from_mbps(100.0)), 100.0);
        assert_eq!(cap_of(from_mbps(100.0)), 2_500_000, "20% of 100 Mbps");
        assert_eq!(cap_of(0), LEAST);
        assert_eq!(from_mbps(f64::NAN), 0);
        assert_eq!(from_mbps(-1.0), 0);
    }
}
