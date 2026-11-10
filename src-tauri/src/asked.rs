//! Who asked for each download, so the other profiles see only their own.
//!
//! The owner (main profile) sees everything that's downloading or waiting.
//! Anyone else, in this PC's window or on a phone or TV, sees only the titles
//! they asked for themselves (Download, a picked season, Watch now). What's
//! finished and in the library is Jellyfin's to show, by each account's own
//! access, so this only covers what's on its way.
//!
//! asked.json: title key -> Jellyfin user ids that asked. A title is known by
//! every name it goes by (its tile key, TMDB id, TVDB id, plain title), since
//! Radarr's and Sonarr's queues call a title by ids the asking card may not
//! have had (an AniList card turns into a TVDB show).

use crate::tiles::Tile;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Manager};

/// Titles remembered at most; the oldest-added go first past this.
const MAX_TITLES: usize = 4000;

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Store {
    /// key -> user ids
    by_key: HashMap<String, BTreeSet<String>>,
    /// Order keys were first added, to drop the oldest.
    order: Vec<String>,
}

fn store(app: &AppHandle) -> &'static Mutex<Store> {
    static S: OnceLock<Mutex<Store>> = OnceLock::new();
    S.get_or_init(|| {
        let s = path(app).and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        Mutex::new(s)
    })
}

fn path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("asked.json"))
}

fn save(app: &AppHandle, s: &Store) {
    let (Some(p), Ok(text)) = (path(app), serde_json::to_string(s)) else { return };
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

/// A title squashed to letters and digits, so "Spider-Man: No Way Home" and
/// "Spider Man No Way Home" are one.
fn squash(title: &str) -> String {
    title.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Every name a tile goes by.
fn keys(t: &Tile) -> Vec<String> {
    let mut k = Vec::new();
    if !t.key.is_empty() {
        k.push(t.key.clone());
    }
    if let Some(id) = t.tmdb {
        k.push(format!("tmdb:{id}"));
    }
    if let Some(id) = t.tvdb {
        k.push(format!("tvdb:{id}"));
    }
    let name = squash(&t.title);
    if !name.is_empty() {
        k.push(format!("name:{name}"));
    }
    k
}

/// The owner's Jellyfin id, and whoever this request is for (None: nobody
/// picked yet, or a background task, both of which mean the owner).
fn who(app: &AppHandle) -> (Option<String>, Option<String>) {
    (crate::profiles::owner_id(app), crate::media::active_user(app))
}

/// Someone asked for `t`: remembered under every name it goes by.
pub fn note(app: &AppHandle, t: &Tile) {
    note_with(app, t, &[]);
}

/// `note`, plus names found while adding it: "radarr:<id>" / "sonarr:<id>"
/// (how the waiting list knows a title) or a TVDB id the card didn't have.
pub fn note_with(app: &AppHandle, t: &Tile, extra: &[String]) {
    let (owner, Some(uid)) = who(app) else { return };
    // The owner sees everything anyway; their asks needn't be kept.
    if owner.as_deref() == Some(uid.as_str()) {
        return;
    }
    let mut s = store(app).lock().unwrap();
    for k in keys(t).into_iter().chain(extra.iter().cloned()) {
        let set = s.by_key.entry(k.clone()).or_default();
        if set.insert(uid.clone()) && set.len() == 1 {
            s.order.push(k);
        }
    }
    while s.order.len() > MAX_TITLES {
        let k = s.order.remove(0);
        s.by_key.remove(&k);
    }
    save(app, &s);
}

/// The titles this request may see: None = everything (the owner).
fn mine(app: &AppHandle) -> Option<HashSet<String>> {
    let (owner, uid) = who(app);
    let uid = uid?;
    // Without an owner yet (not signed in) there's nothing to hide behind.
    if owner.is_none() || owner.as_deref() == Some(uid.as_str()) {
        return None;
    }
    let s = store(app).lock().unwrap();
    Some(s.by_key.iter().filter(|(_, ids)| ids.contains(&uid)).map(|(k, _)| k.clone()).collect())
}

fn shows(mine: &HashSet<String>, t: &Tile) -> bool {
    keys(t).iter().any(|k| mine.contains(k))
}

/// Only the tiles this request may see.
pub fn keep(app: &AppHandle, tiles: Vec<Tile>) -> Vec<Tile> {
    match mine(app) {
        None => tiles,
        Some(m) => tiles.into_iter().filter(|t| shows(&m, t)).collect(),
    }
}

/// `keep` for anything holding a tile (Downloading now, the waiting list).
pub fn keep_by<T>(app: &AppHandle, list: Vec<T>, tile: impl Fn(&T) -> &Tile) -> Vec<T> {
    match mine(app) {
        None => list,
        Some(m) => list.into_iter().filter(|x| shows(&m, tile(x))).collect(),
    }
}

/// The waiting list: known by the tile, or by Radarr's/Sonarr's own id.
pub fn keep_waiting(app: &AppHandle, list: Vec<crate::waiting::Waiting>) -> Vec<crate::waiting::Waiting> {
    match mine(app) {
        None => list,
        Some(m) => list.into_iter().filter(|w| shows(&m, &w.tile) || m.contains(&format!("{}:{}", w.app, w.id))).collect(),
    }
}

/// Whether this request may see everything (the owner).
pub fn sees_all(app: &AppHandle) -> bool {
    mine(app).is_none()
}

/// The Jellyfin users a progress event about tile key `key` is for: the owner
/// and whoever asked for it.
pub fn audience(app: &AppHandle, key: &str) -> Vec<String> {
    let mut out: Vec<String> = store(app).lock().unwrap().by_key.get(key).map(|s| s.iter().cloned().collect()).unwrap_or_default();
    if let Some(o) = crate::profiles::owner_id(app) {
        if !out.contains(&o) {
            out.push(o);
        }
    }
    out
}

/// A removed profile: its asks are forgotten.
pub fn forget_user(app: &AppHandle, uid: &str) {
    let mut s = store(app).lock().unwrap();
    for ids in s.by_key.values_mut() {
        ids.remove(uid);
    }
    s.by_key.retain(|_, ids| !ids.is_empty());
    let Store { by_key, order } = &mut *s;
    order.retain(|k| by_key.contains_key(k));
    save(app, &s);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(key: &str, title: &str, tmdb: Option<i64>, tvdb: Option<i64>) -> Tile {
        Tile { key: key.into(), title: title.into(), tmdb, tvdb, ..Default::default() }
    }

    #[test]
    fn a_title_is_matched_by_any_name_it_goes_by() {
        let asked = tile("anilist:21", "Spy x Family", None, None);
        let mine: HashSet<String> = keys(&asked).into_iter().collect();
        // Sonarr's queue knows it by TVDB id and its own spelling.
        assert!(shows(&mine, &tile("tvdb:405920", "SPY×FAMILY", None, Some(405920))) == false, "a different spelling with no shared id isn't matched");
        assert!(shows(&mine, &tile("tvdb:405920", "Spy x Family", None, Some(405920))));
        assert!(shows(&mine, &tile("x", "Spy-x-Family!", None, None)));
        assert!(!shows(&mine, &tile("tmdb:1", "Dune", Some(1), None)));
    }
}
