//! Reading downloads, live. Pages used to rebuild a whole series panel every
//! 5 s and the Downloads tab every 4 s to show chapter progress (flicker, a
//! jumping scroll, typed numbers lost), and chapters queued while a panel was
//! open never showed at all. Now one loop asks Suwayomi's queue, every 2 s
//! while something downloads and every 10 s otherwise, and tells the pages
//! what changed ("read-live"): a chapter's progress, or that it landed. The
//! pages patch the one row. A failed ask is said as such, not taken for an
//! empty queue.

use serde::Serialize;
use std::collections::HashMap;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

const BUSY_EVERY: Duration = Duration::from_secs(2);
const IDLE_EVERY: Duration = Duration::from_secs(10);

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Change {
    pub manga: i64,
    pub ch: i64,
    /// 0..1; 1 with `done`.
    pub p: f64,
    /// Left the queue: downloaded (or stopped; a Stop redraws its page anyway).
    pub done: bool,
}

/// What changed between two looks at the queue: (manga id, chapter id, progress).
/// Progress is told when it moved by a whole percent.
pub fn diff(prev: &HashMap<i64, (i64, f64)>, now: &[(i64, i64, f64)]) -> Vec<Change> {
    let mut out = Vec::new();
    for &(manga, ch, p) in now {
        match prev.get(&ch) {
            Some((_, q)) if (p - q).abs() < 0.01 => {}
            _ => out.push(Change { manga, ch, p, done: false }),
        }
    }
    for (&ch, &(manga, _)) in prev {
        if !now.iter().any(|n| n.1 == ch) {
            out.push(Change { manga, ch, p: 1.0, done: true });
        }
    }
    out
}

#[derive(Serialize, Clone)]
struct Item {
    key: String,
    ch: i64,
    p: f64,
    done: bool,
}

#[derive(Serialize, Clone)]
struct Live {
    items: Vec<Item>,
    /// The queue couldn't be read right now.
    error: Option<String>,
}

/// Sends `live` to the PC window and to each owner's open pages.
fn tell(app: &AppHandle, owners: &[String], live: &Live) {
    let _ = app.emit("read-live", live);
    for uid in owners {
        crate::bridge::emit_to_user(uid, "read-live", live);
    }
}

pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut prev: HashMap<i64, (i64, f64)> = HashMap::new();
        let mut failing = false;
        loop {
            let busy = !prev.is_empty();
            // Something downloading: every 2 s. Otherwise every 10 s with someone on it, a minute without.
            if busy {
                tokio::time::sleep(BUSY_EVERY).await;
            } else {
                crate::activity::nap(false, IDLE_EVERY, Duration::from_secs(60)).await;
            }
            // Nothing to ask before reading is set up.
            if crate::read::followed(&app).is_empty() {
                continue;
            }
            let now = match crate::suwayomi::queue_checked().await {
                Ok(q) => q,
                Err(e) => {
                    if !failing && busy {
                        failing = true;
                        let owners = all_owners(&app);
                        tell(&app, &owners, &Live { items: vec![], error: Some(format!("Can't see the chapter downloads right now: {e}")) });
                    }
                    continue;
                }
            };
            failing = false;
            let changes = diff(&prev, &now);
            prev = now.iter().map(|&(m, c, p)| (c, (m, p))).collect();
            if changes.is_empty() {
                continue;
            }
            // Each series' changes go to whoever follows it.
            let pc_owner = crate::profiles::owner_id(&app);
            let mut by_owners: HashMap<Vec<String>, Vec<Item>> = HashMap::new();
            for f in crate::read::followed(&app) {
                let items: Vec<Item> = changes.iter().filter(|c| f.has_manga(c.manga)).map(|c| Item { key: f.key.clone(), ch: c.ch, p: c.p, done: c.done }).collect();
                if items.is_empty() {
                    continue;
                }
                by_owners.entry(crate::read::owners_of(&f.owners, pc_owner.as_deref())).or_default().extend(items);
            }
            for (owners, items) in by_owners {
                tell(&app, &owners, &Live { items, error: None });
            }
        }
    });
}

fn all_owners(app: &AppHandle) -> Vec<String> {
    let pc_owner = crate::profiles::owner_id(app);
    let mut v: Vec<String> = crate::read::followed(app).iter().flat_map(|f| crate::read::owners_of(&f.owners, pc_owner.as_deref())).collect();
    v.sort();
    v.dedup();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_emits_progress_and_landed() {
        let mut prev = HashMap::new();
        prev.insert(10, (1, 0.40));
        prev.insert(11, (1, 0.0));
        let now = [(1, 10, 0.405), (1, 12, 0.0)];
        let d = diff(&prev, &now);
        assert!(!d.iter().any(|c| c.ch == 10), "under a percent: nothing to tell");
        assert!(d.contains(&Change { manga: 1, ch: 12, p: 0.0, done: false }), "new in the queue");
        assert!(d.contains(&Change { manga: 1, ch: 11, p: 1.0, done: true }), "left the queue: landed");
        let d = diff(&prev, &[(1, 10, 0.5), (1, 11, 0.0)]);
        assert_eq!(d, vec![Change { manga: 1, ch: 10, p: 0.5, done: false }]);
    }

    #[test]
    fn an_empty_queue_after_an_empty_queue_says_nothing() {
        assert!(diff(&HashMap::new(), &[]).is_empty());
    }
}
