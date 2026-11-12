//! New chapters, sooner and said out loud (issue #18, reading part).
//!
//! * Get new chapters: a series switched on asks its site(s) for new chapters every
//!   10 minutes or so, instead of with Suwayomi's own round every 6 hours. One series
//!   at a time, a few seconds apart, with a random spread so it never beats in step,
//!   and a site that errors is asked half as often each time (up to every 2 hours)
//!   until it answers again. 10 minutes is about 150 asks a day per site per series,
//!   well inside what a reader app on a phone does.
//! * New chapter ready: every series you follow, switched on or not. A notification
//!   when chapters newer than the last one told about have downloaded.
//! * Your copy fell behind: once a day per series, every site is asked how far it
//!   has got; one ahead of every copy you read from gets a notification (once per
//!   chapter it's ahead to), and the series panel's Change copy switches.

use crate::read::{self, Followed};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

const TICK: Duration = Duration::from_secs(60);
const KEEP_UP_EVERY: Duration = Duration::from_secs(10 * 60);
const KEEP_UP_MOST: Duration = Duration::from_secs(2 * 60 * 60);
const BEHIND_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// Between two series' asks in one round.
const GAP: Duration = Duration::from_secs(4);

#[derive(Default)]
struct Due {
    /// Per series: when to ask the sites next, and the wait used last.
    keep_up: HashMap<String, (Instant, Duration)>,
    behind: HashMap<String, Instant>,
}

static DUE: Mutex<Option<Due>> = Mutex::new(None);

fn with_due<R>(f: impl FnOnce(&mut Due) -> R) -> R {
    let mut g = DUE.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Due::default))
}

/// A wait with up to a fifth either way, so many series don't ask together.
fn spread(d: Duration) -> Duration {
    let mut b = [0u8; 2];
    let _ = getrandom::getrandom(&mut b);
    let r = u16::from_le_bytes(b) as f64 / u16::MAX as f64; // 0..1
    d.mul_f64(0.8 + 0.4 * r)
}

/// The next wait after an ask: back to normal when it answered, doubled when it didn't.
pub fn next_wait(last: Duration, ok: bool) -> Duration {
    if ok { KEEP_UP_EVERY } else { (last * 2).clamp(KEEP_UP_EVERY, KEEP_UP_MOST) }
}

/// Switched on now: ask at once.
pub fn wake(key: &str) {
    with_due(|d| {
        d.keep_up.insert(key.to_string(), (Instant::now(), KEEP_UP_EVERY));
    });
}

pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(90)).await;
        loop {
            round(&app).await;
            tokio::time::sleep(TICK).await;
        }
    });
}

async fn round(app: &AppHandle) {
    let list = read::followed(app);
    if list.is_empty() {
        return;
    }
    let mut asked = false;
    for f in list.iter().filter(|f| f.new_too) {
        let due = with_due(|d| d.keep_up.get(&f.key).is_none_or(|(at, _)| Instant::now() >= *at));
        if !due {
            continue;
        }
        if asked {
            tokio::time::sleep(GAP).await;
        }
        asked = true;
        let mut ok = true;
        for (id, ..) in f.copies() {
            ok &= crate::suwayomi::refresh(id).await.is_ok();
        }
        with_due(|d| {
            let last = d.keep_up.get(&f.key).map(|x| x.1).unwrap_or(KEEP_UP_EVERY);
            let wait = next_wait(last, ok);
            d.keep_up.insert(f.key.clone(), (Instant::now() + spread(wait), wait));
        });
    }
    if asked {
        // What the sites just listed gets queued now, not at the 10-minute round.
        read::fetch_new(app).await;
    }
    // Telling reads every series' chapter list: every few rounds is soon
    // enough for "chapter N is ready", and right away after a site was asked.
    static ROUND: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let round = ROUND.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if asked || round % 4 == 0 {
        for f in &list {
            tell_new(app, f).await;
        }
    }
    for f in &list {
        let due = with_due(|d| d.behind.get(&f.key).is_none_or(|at| Instant::now() >= *at));
        if due {
            with_due(|d| d.behind.insert(f.key.clone(), Instant::now() + spread(BEHIND_EVERY)));
            tell_behind(app, f).await;
            // One series' every-site search a round: it's the heavy one.
            break;
        }
    }
}

fn notify(app: &AppHandle, body: &str) {
    let _ = app.notification().builder().title("Chillarr").body(body).show();
}

/// What a notification says about new chapters, or None when nothing's new.
pub fn new_note(title: &str, told: Option<f64>, here: &[f64]) -> (Option<String>, Option<f64>) {
    let newest = here.iter().copied().fold(None, |m: Option<f64>, n| Some(m.map_or(n, |m| m.max(n))));
    let Some(newest) = newest else { return (None, told) };
    let Some(told) = told else { return (None, Some(newest)) }; // first look: nothing to say yet
    if newest <= told {
        return (None, Some(told));
    }
    let fresh = here.iter().filter(|&&n| n > told).count();
    let n = fmt(newest);
    let text = if fresh == 1 { format!("{title}: chapter {n} is ready to read.") } else { format!("{title}: {fresh} new chapters are ready, up to chapter {n}.") };
    (Some(text), Some(newest))
}

fn fmt(n: f64) -> String {
    if n.fract() == 0.0 { format!("{}", n as i64) } else { format!("{n}") }
}

async fn tell_new(app: &AppHandle, f: &Followed) {
    let Ok((chs, complete)) = read::chapter_list_checked(f).await else { return };
    let here: Vec<f64> = chs.iter().filter(|c| c.downloaded).filter_map(|c| c.number).collect();
    // A "told" saved from a stray number (276, 560) comes down to the trusted newest.
    let was = read::lowered_mark(f.told, read::top_number(chs.iter()), complete);
    let (text, told) = new_note(&f.title, was, &here);
    if told != f.told {
        read::update_followed(app, &f.key, |x| x.told = told);
    }
    if let Some(t) = text {
        notify(app, &t);
        crate::log::detail(t);
    }
}

async fn tell_behind(app: &AppHandle, f: &Followed) {
    let Ok(chs) = read::chapter_list_of(f).await else { return };
    let ours = chs.iter().filter_map(|c| c.number).fold(f64::MIN, f64::max);
    if ours == f64::MIN {
        return;
    }
    let mut names = vec![f.title.clone()];
    names.extend(f.names.iter().cloned());
    let lang = f.lang.clone().unwrap_or_else(|| "en".into());
    let Ok(found) = crate::suwayomi::candidates(&names, &lang, false).await else { return };
    let ahead = found.iter().filter(|c| !f.has_manga(c.manga_id)).filter_map(|c| c.latest.map(|l| (l, c))).filter(|(l, _)| *l >= ours + 1.0).max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let Some((latest, c)) = ahead else { return };
    if f.ahead_told.is_some_and(|t| t >= latest) {
        return;
    }
    let site = c.source.split(" (").next().unwrap_or(&c.source);
    let text = format!("{}: {site} is up to chapter {}, your copy stops at {}. Open the series and press Change copy to switch.", f.title, fmt(latest), fmt(ours));
    read::update_followed(app, &f.key, |x| x.ahead_told = Some(latest));
    notify(app, &text);
    crate::log::detail(text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_site_that_errors_is_asked_less_often() {
        assert_eq!(next_wait(KEEP_UP_EVERY, true), KEEP_UP_EVERY);
        assert_eq!(next_wait(KEEP_UP_EVERY, false), KEEP_UP_EVERY * 2);
        assert_eq!(next_wait(KEEP_UP_MOST, false), KEEP_UP_MOST, "never less often than every 2 hours");
        assert_eq!(next_wait(KEEP_UP_MOST, true), KEEP_UP_EVERY, "answered again: back to normal");
        for _ in 0..50 {
            let s = spread(KEEP_UP_EVERY);
            assert!(s >= KEEP_UP_EVERY.mul_f64(0.79) && s <= KEEP_UP_EVERY.mul_f64(1.21));
        }
    }

    #[test]
    fn new_chapters_are_told_once() {
        assert_eq!(new_note("Mage", None, &[1.0, 2.0]), (None, Some(2.0)), "first look: quiet");
        assert_eq!(new_note("Mage", Some(2.0), &[1.0, 2.0]), (None, Some(2.0)));
        assert_eq!(new_note("Mage", Some(2.0), &[1.0, 2.0, 3.0]), (Some("Mage: chapter 3 is ready to read.".into()), Some(3.0)));
        assert_eq!(new_note("Mage", Some(2.0), &[3.0, 4.5]), (Some("Mage: 2 new chapters are ready, up to chapter 4.5.".into()), Some(4.5)));
        assert_eq!(new_note("Mage", Some(9.0), &[]), (None, Some(9.0)), "cleaned up: nothing new");
        assert_eq!(new_note("Mage", Some(9.0), &[3.0]), (None, Some(9.0)), "an old one downloaded again isn't news");
    }
}
