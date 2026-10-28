//! "Where you're at" in a show: one spot per show (by TVDB id), per profile,
//! kept in marker-<jellyfin user id>.json. It moves forward when you finish
//! the episode it points at or one past it, on any device (the watch log's
//! sync calls `advance`). Rewatching something earlier never moves it back.
//! Only the "Where you're at" section moves it on purpose.
//!
//! A spot is in Sonarr's numbering (the episode list the page shows). The
//! sync only knows "season:episode + 1", so a spot can point one past a
//! season's end; `settle` puts it on the episode the show really has next.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tauri::AppHandle;

/// One episode of a show. Ordered by season, then episode.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Spot {
    pub season: i64,
    pub episode: i64,
}

/// Where the spot goes once `finished` is: on to `next` (or stays on the
/// last episode when there is none) if `finished` is the spot or past it.
/// Finishing an earlier episode (a rewatch) leaves it.
pub fn advance(current: Option<Spot>, finished: Spot, next: Option<Spot>) -> Option<Spot> {
    match current {
        Some(c) if finished < c => Some(c),
        _ => Some(next.unwrap_or(finished)),
    }
}

/// The episode a spot means in the show's own list (specials left out): the
/// first at or after it, or the last one when it's past the end.
pub fn settle(spot: Spot, order: &[Spot]) -> Option<Spot> {
    order.iter().find(|s| **s >= spot).or(order.last()).copied()
}

/// A show's spot, and when it was last set on purpose (Start over, Pick a
/// different episode) or first worked out. A finish from before then is old
/// news (a sync that read Jellyfin before a restart, or an episode Jellyfin
/// wouldn't unmark) and never moves it.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Mark {
    pub spot: Spot,
    #[serde(default)]
    pub set_at: i64,
}

pub fn file(dir: &Path, uid: &str) -> PathBuf {
    dir.join(format!("marker-{uid}.json"))
}

/// Every show's mark for one profile, by TVDB id. A file that won't parse is
/// moved aside to `.json.bad` (so the next save can't wipe every spot in it)
/// and the profile starts empty.
pub fn load(dir: &Path, uid: &str) -> HashMap<i64, Mark> {
    let path = file(dir, uid);
    let Ok(text) = std::fs::read_to_string(&path) else { return HashMap::new() };
    match serde_json::from_str(&text) {
        Ok(m) => m,
        Err(_) => {
            let _ = std::fs::rename(&path, path.with_extension("json.bad"));
            HashMap::new()
        }
    }
}

/// Every profile's spot in one show, from each marker-<id>.json in `dir`
/// (a removed profile's file goes with it, see `profiles::remove`).
pub fn spots_in(dir: &Path, tvdb: i64) -> Vec<Spot> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    rd.filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().to_str()?.strip_prefix("marker-")?.strip_suffix(".json").map(String::from))
        .filter_map(|uid| load(dir, &uid).get(&tvdb).map(|m| m.spot))
        .collect()
}

pub fn save_mark(dir: &Path, uid: &str, tvdb: i64, mark: Mark) -> Result<(), String> {
    let mut all = load(dir, uid);
    if all.get(&tvdb) == Some(&mark) {
        return Ok(());
    }
    all.insert(tvdb, mark);
    let path = file(dir, uid);
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string(&all).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, &path)).map_err(|e| format!("Couldn't save where you're at: {e}"))
}

/// The sync saw `finished` finished at `at` (any device). Moves the spot only
/// when the show has one (the page works out the first one when it's opened)
/// and the finish came after it was last set.
pub fn finished(dir: &Path, uid: &str, tvdb: i64, finished: Spot, at: i64) -> Result<(), String> {
    let Some(m) = load(dir, uid).get(&tvdb).copied() else { return Ok(()) };
    if at <= m.set_at {
        return Ok(());
    }
    let next = Spot { season: finished.season, episode: finished.episode + 1 };
    match advance(Some(m.spot), finished, Some(next)) {
        Some(s) if s != m.spot => save_mark(dir, uid, tvdb, Mark { spot: s, set_at: m.set_at }),
        _ => Ok(()),
    }
}

fn who(app: &AppHandle) -> Option<(PathBuf, String)> {
    Some((crate::media::data_dir(app)?, crate::media::active_user(app)?))
}

/// The watching profile's mark in a show.
pub fn get(app: &AppHandle, tvdb: i64) -> Option<Mark> {
    let (dir, uid) = who(app)?;
    load(&dir, &uid).get(&tvdb).copied()
}

/// Sets the watching profile's spot on purpose: from now on only finishes after this count.
pub fn set(app: &AppHandle, tvdb: i64, spot: Spot) -> Result<(), String> {
    put(app, tvdb, Mark { spot, set_at: chrono::Utc::now().timestamp() })
}

/// Saves a mark as given (the page settling one, or working out the first).
pub fn put(app: &AppHandle, tvdb: i64, mark: Mark) -> Result<(), String> {
    let (dir, uid) = who(app).ok_or("Sign in to Jellyfin first.")?;
    save_mark(&dir, &uid, tvdb, mark)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(season: i64, episode: i64) -> Spot {
        Spot { season, episode }
    }

    #[test]
    fn advance_moves_forward_on_finish() {
        assert_eq!(advance(Some(s(1, 3)), s(1, 3), Some(s(1, 4))), Some(s(1, 4)));
        assert_eq!(advance(Some(s(1, 3)), s(1, 7), Some(s(1, 8))), Some(s(1, 8)), "finished past it: past that");
        assert_eq!(advance(Some(s(1, 10)), s(1, 10), Some(s(2, 1))), Some(s(2, 1)), "into the next season");
        assert_eq!(advance(None, s(2, 4), Some(s(2, 5))), Some(s(2, 5)), "no spot yet");
    }

    #[test]
    fn rewatch_earlier_does_not_move_back() {
        assert_eq!(advance(Some(s(2, 5)), s(1, 3), Some(s(1, 4))), Some(s(2, 5)));
        assert_eq!(advance(Some(s(2, 5)), s(2, 4), Some(s(2, 5))), Some(s(2, 5)));
    }

    #[test]
    fn finishing_last_keeps_spot_at_last() {
        assert_eq!(advance(Some(s(3, 10)), s(3, 10), None), Some(s(3, 10)));
        assert_eq!(advance(None, s(3, 10), None), Some(s(3, 10)));
    }

    #[test]
    fn a_spot_settles_on_an_episode_the_show_has() {
        let order = vec![s(1, 1), s(1, 2), s(2, 1), s(2, 2)];
        assert_eq!(settle(s(1, 3), &order), Some(s(2, 1)), "past a season's end: the next season");
        assert_eq!(settle(s(1, 2), &order), Some(s(1, 2)));
        assert_eq!(settle(s(2, 3), &order), Some(s(2, 2)), "past the last: the last");
        assert_eq!(settle(s(1, 1), &[]), None);
    }

    #[test]
    fn markers_are_saved_per_profile() {
        let dir = tempfile::tempdir().unwrap();
        save_mark(dir.path(), "u1", 7, Mark { spot: s(2, 3), set_at: 5 }).unwrap();
        save_mark(dir.path(), "u1", 8, Mark { spot: s(1, 1), set_at: 5 }).unwrap();
        assert_eq!(load(dir.path(), "u1").get(&7).map(|m| m.spot), Some(s(2, 3)));
        assert!(load(dir.path(), "u2").is_empty());
        assert!(file(dir.path(), "u1").ends_with("marker-u1.json"));
    }

    #[test]
    fn every_profiles_spot_in_a_show() {
        let dir = tempfile::tempdir().unwrap();
        save_mark(dir.path(), "u1", 7, Mark { spot: s(2, 3), set_at: 5 }).unwrap();
        save_mark(dir.path(), "u2", 7, Mark { spot: s(1, 1), set_at: 5 }).unwrap();
        save_mark(dir.path(), "u2", 8, Mark { spot: s(4, 4), set_at: 5 }).unwrap();
        std::fs::write(dir.path().join("history-u3.json"), "{}").unwrap();
        let mut got = spots_in(dir.path(), 7);
        got.sort();
        assert_eq!(got, vec![s(1, 1), s(2, 3)]);
        assert!(spots_in(dir.path(), 9).is_empty());
        assert!(spots_in(&dir.path().join("missing"), 7).is_empty());
    }

    #[test]
    fn a_corrupt_file_is_set_aside_not_wiped() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(file(dir.path(), "u"), "{not json").unwrap();
        assert!(load(dir.path(), "u").is_empty());
        let bad = dir.path().join("marker-u.json.bad");
        assert_eq!(std::fs::read_to_string(&bad).unwrap(), "{not json", "kept as it was");
        save_mark(dir.path(), "u", 7, Mark { spot: s(1, 1), set_at: 5 }).unwrap();
        assert!(bad.exists(), "the next save doesn't touch it");
    }

    fn spot_of(dir: &Path) -> Option<Spot> {
        load(dir, "u").get(&7).map(|m| m.spot)
    }

    #[test]
    fn finished_moves_a_saved_spot_forward_only() {
        let dir = tempfile::tempdir().unwrap();
        save_mark(dir.path(), "u", 7, Mark { spot: s(1, 3), set_at: 100 }).unwrap();
        finished(dir.path(), "u", 7, s(1, 3), 200).unwrap();
        assert_eq!(spot_of(dir.path()), Some(s(1, 4)));
        finished(dir.path(), "u", 7, s(1, 1), 300).unwrap();
        assert_eq!(spot_of(dir.path()), Some(s(1, 4)), "a rewatch of something earlier");
        finished(dir.path(), "u", 8, s(5, 5), 300).unwrap();
        assert_eq!(spot_of(dir.path()), Some(s(1, 4)), "another show");
        assert_eq!(load(dir.path(), "u")[&7].set_at, 100, "moving on its own doesn't count as setting it");
    }

    #[test]
    fn a_rewatch_before_the_page_was_ever_opened_sets_nothing() {
        let dir = tempfile::tempdir().unwrap();
        finished(dir.path(), "u", 7, s(1, 2), 200).unwrap();
        assert_eq!(spot_of(dir.path()), None, "no spot yet: the page works out the first one");
        assert!(!file(dir.path(), "u").exists());
    }

    /// Start season 1 over at t=1000. A sync that read Jellyfin before the
    /// restart then logs S1E1-3 again (last played at t=500): not news.
    #[test]
    fn a_stale_sync_after_a_restart_doesnt_move_it() {
        use crate::history::{ep_key, History, SeriesWatch};
        let dir = tempfile::tempdir().unwrap();
        save_mark(dir.path(), "u", 7, Mark { spot: s(1, 0), set_at: 1000 }).unwrap();
        let forgotten = History { series: [("a".to_string(), SeriesWatch { tvdb: Some(7), ..Default::default() })].into(), ..Default::default() };
        let mut stale = forgotten.clone();
        for e in 1..=3 {
            stale.series.get_mut("a").unwrap().episodes.insert(ep_key(1, e), 500);
        }
        let news = crate::history::newly_finished(&forgotten, &stale);
        assert_eq!(news.len(), 3, "the log does see them as new");
        for (tvdb, spot, at) in news {
            finished(dir.path(), "u", tvdb, spot, at).unwrap();
        }
        assert_eq!(spot_of(dir.path()), Some(s(1, 0)), "but they're from before the restart");
    }

    /// Jellyfin wouldn't unmark S1E2: it stays Played (t=500), comes back into
    /// the log on the next sync, and still doesn't count. Watching S1E1 again
    /// after the restart does.
    #[test]
    fn an_episode_jellyfin_wouldnt_unmark_doesnt_move_it() {
        let dir = tempfile::tempdir().unwrap();
        save_mark(dir.path(), "u", 7, Mark { spot: s(1, 0), set_at: 1000 }).unwrap();
        finished(dir.path(), "u", 7, s(1, 2), 500).unwrap();
        assert_eq!(spot_of(dir.path()), Some(s(1, 0)));
        finished(dir.path(), "u", 7, s(1, 1), 1500).unwrap();
        assert_eq!(spot_of(dir.path()), Some(s(1, 2)));
    }
}
