//! Keeps Chillarr's torrents out of the user's own Downloads folder, and
//! clears finished torrents nothing wants any more.
//!
//! Why: older setups pointed qBittorrent at %USERPROFILE%\Downloads, so every
//! film showed up there next to the user's own files. And a title that left
//! Radarr/Sonarr after its torrent finished (stopped, deleted in the app
//! itself) left that torrent seeding forever: Meet Joe Black, 2026-11-08,
//! 4.3 GB, one link, in no app's queue or history.

use crate::model::{AppId, Level};
use crate::log::log;
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::AppHandle;

/// qBittorrent categories Radarr and Sonarr put their grabs in.
const ARR_CATEGORIES: [(&str, AppId); 2] = [("radarr", AppId::Radarr), ("tv-sonarr", AppId::Sonarr)];
/// A finished torrent younger than this may still be on its way into an app's queue.
const ORPHAN_GRACE_SECS: u64 = 60 * 60;
/// The folder's name, wherever it goes.
const FOLDER: &str = "Chillarr Downloads";

fn norm(p: &str) -> String {
    p.trim().trim_end_matches(['\\', '/']).replace('/', "\\").to_ascii_lowercase()
}

fn drive(p: &str) -> Option<String> {
    let p = p.trim();
    (p.len() >= 2 && p.as_bytes()[1] == b':').then(|| p[..2].to_ascii_uppercase())
}

/// Where downloads should go instead of `save`, or None when `save` isn't the
/// user's own Downloads folder (a folder they picked is theirs to keep).
/// Same drive as the library, so imports stay hardlinks and moving is a rename.
pub fn new_home(save: &str, user_downloads: &Path, local_appdata: &Path, roots: &[String]) -> Option<PathBuf> {
    if norm(save) != norm(&user_downloads.to_string_lossy()) {
        return None;
    }
    let lib_drive = roots.iter().find_map(|r| drive(r));
    let app_drive = drive(&local_appdata.to_string_lossy());
    match lib_drive {
        Some(d) if Some(&d) != app_drive.as_ref() => Some(PathBuf::from(format!("{d}\\{FOLDER}"))),
        _ => Some(local_appdata.join(FOLDER)),
    }
}

/// Torrents Chillarr owns and may move: Radarr/Sonarr grabs and Watch now.
fn ours(t: &Value, watch_now: &HashSet<String>) -> bool {
    let cat = t["category"].as_str().unwrap_or_default();
    ARR_CATEGORIES.iter().any(|(c, _)| *c == cat) || t["hash"].as_str().is_some_and(|h| watch_now.contains(&h.to_ascii_lowercase()))
}

async fn library_roots(app: &AppHandle) -> Option<Vec<String>> {
    let mut roots = Vec::new();
    for owner in [AppId::Radarr, AppId::Sonarr] {
        let Some(a) = crate::media::arr(app, owner) else { continue };
        let folders = a.list("rootfolder").await.ok()?;
        roots.extend(folders.iter().filter_map(|f| f["path"].as_str().map(String::from)));
    }
    Some(roots)
}

/// Moves qBittorrent's default folder (and Chillarr's torrents) out of the
/// user's Downloads. Runs live over the Web UI: no restart, and qBittorrent
/// writes the new folder into its own settings file when it closes.
pub async fn move_out_of_user_downloads(app: &AppHandle) {
    let Some(q) = crate::waiting::qbit(app) else { return };
    let Ok(prefs) = q.prefs().await else { return };
    let Some(save) = prefs["save_path"].as_str().map(String::from) else { return };
    let (Some(home), Some(local)) = (std::env::var_os("USERPROFILE"), std::env::var_os("LOCALAPPDATA")) else { return };
    // Every app must answer: the wrong drive would turn imports into full copies.
    let Some(roots) = library_roots(app).await else { return };
    let Some(target) = new_home(&save, &PathBuf::from(home).join("Downloads"), &PathBuf::from(local), &roots) else { return };
    if let Err(e) = std::fs::create_dir_all(&target) {
        return log(app, Level::Amber, format!("Couldn't make {}: {e}", target.display()));
    }
    let to = target.to_string_lossy().to_string();
    if let Err(e) = q.set_prefs(&serde_json::json!({ "save_path": to, "temp_path_enabled": false })).await {
        return log(app, Level::Amber, format!("Couldn't move downloads out of your Downloads folder: {e:?}"));
    }
    let watch_now = tauri::Manager::state::<crate::watchnow::WatchNow>(app).owned_set();
    let moving: Vec<String> = q
        .list()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|t| ours(t, &watch_now) && t["save_path"].as_str().is_some_and(|p| norm(p) == norm(&save)))
        .filter_map(|t| t["hash"].as_str().map(|h| h.to_ascii_lowercase()))
        .collect();
    if !moving.is_empty() {
        if let Err(e) = q.set_location(&moving, &to).await {
            log(app, Level::Amber, format!("New downloads go to {to}, but the old ones couldn't move: {e:?}"));
        }
    }
    log(app, Level::Ok, format!("Downloads now go to {to}, not your own Downloads folder ({} moved).", moving.len()));
}

/// Finished Radarr/Sonarr torrents whose app has no record of them: not in
/// its queue, not in its history (deleting a title deletes its history).
pub fn orphan_candidates(torrents: &[Value], watch_now: &HashSet<String>, now: u64) -> Vec<(String, AppId, String)> {
    torrents
        .iter()
        .filter(|t| t["progress"].as_f64().unwrap_or(0.0) >= 1.0)
        .filter(|t| now.saturating_sub(t["completion_on"].as_u64().or(t["added_on"].as_u64()).unwrap_or(now)) >= ORPHAN_GRACE_SECS)
        .filter(|t| t["hash"].as_str().is_some_and(|h| !watch_now.contains(&h.to_ascii_lowercase())))
        .filter_map(|t| {
            let cat = t["category"].as_str()?;
            let owner = ARR_CATEGORIES.iter().find(|(c, _)| *c == cat)?.1;
            Some((t["hash"].as_str()?.to_ascii_lowercase(), owner, t["name"].as_str().unwrap_or("a download").to_string()))
        })
        .collect()
}

pub async fn clear_orphans(app: &AppHandle) {
    let Some(q) = crate::waiting::qbit(app) else { return };
    let Ok(all) = q.list().await else { return };
    let watch_now = tauri::Manager::state::<crate::watchnow::WatchNow>(app).owned_set();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let cands = orphan_candidates(&all, &watch_now, now);
    if cands.is_empty() {
        return;
    }
    let mut doomed = Vec::new();
    let mut names = Vec::new();
    for owner in [AppId::Radarr, AppId::Sonarr] {
        let mine: Vec<_> = cands.iter().filter(|c| c.1 == owner).collect();
        if mine.is_empty() {
            continue;
        }
        // The app must answer, or nothing of its goes.
        let Some(a) = crate::media::arr(app, owner) else { continue };
        let Ok(queue) = a.get_one("queue?pageSize=2000").await else { continue };
        let Some(rows) = queue["records"].as_array() else { continue };
        let queued: HashSet<String> = rows.iter().filter_map(|r| r["downloadId"].as_str().map(|h| h.to_ascii_lowercase())).collect();
        for (hash, _, name) in mine {
            if queued.contains(hash) {
                continue;
            }
            let Ok(h) = a.get_one(&format!("history?downloadId={}&pageSize=1", hash.to_ascii_uppercase())).await else { continue };
            if h["totalRecords"].as_i64() == Some(0) {
                doomed.push(hash.clone());
                names.push(name.clone());
            }
        }
    }
    if doomed.is_empty() {
        return;
    }
    let safe = crate::storage::safe_to_delete_files(app, &q, &doomed).await;
    if safe.is_empty() || q.delete(&safe, true).await.is_err() {
        return;
    }
    let gone: Vec<&String> = doomed.iter().zip(&names).filter(|(h, _)| safe.contains(h)).map(|(_, n)| n).collect();
    log(app, Level::Ok, format!("Removed {} finished download(s) nothing wanted any more: {}.", gone.len(), gone.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_the_users_own_downloads_moves() {
        let dl = Path::new(r"C:\Users\anime\Downloads");
        let la = Path::new(r"C:\Users\anime\AppData\Local");
        let roots = vec![r"C:\Users\anime\Desktop\Desktop\Movies\Normal".to_string()];
        assert_eq!(new_home(r"C:/Users/anime/Downloads/", dl, la, &roots), Some(la.join(FOLDER)));
        assert_eq!(new_home(r"D:\Media\Downloads", dl, la, &roots), None, "a folder the user picked stays");
        let d_roots = vec![r"D:\Media\Movies".to_string()];
        assert_eq!(new_home(r"C:\Users\anime\Downloads", dl, la, &d_roots), Some(PathBuf::from(r"D:\Chillarr Downloads")), "same drive as the library");
        assert_eq!(new_home(r"C:\Users\anime\Downloads", dl, la, &[]), Some(la.join(FOLDER)));
    }

    #[test]
    fn orphans_are_finished_old_arr_torrents_only() {
        let now = 100_000;
        let t = |h: &str, cat: &str, p: f64, done: u64| json!({ "hash": h, "category": cat, "progress": p, "completion_on": done, "name": h });
        let all = vec![
            t("AA", "radarr", 1.0, now - 7200),
            t("bb", "radarr", 0.5, now - 7200),
            t("cc", "radarr", 1.0, now - 60),
            t("dd", "", 1.0, now - 7200),
            t("ee", "tv-sonarr", 1.0, now - 7200),
            t("ff", "radarr", 1.0, now - 7200),
        ];
        let wn: HashSet<String> = ["ff".to_string()].into();
        let got: Vec<(String, AppId)> = orphan_candidates(&all, &wn, now).into_iter().map(|(h, o, _)| (h, o)).collect();
        assert_eq!(got, vec![("aa".to_string(), AppId::Radarr), ("ee".to_string(), AppId::Sonarr)]);
    }
}
