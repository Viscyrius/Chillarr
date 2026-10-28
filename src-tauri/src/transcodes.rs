//! Jellyfin's transcode folder. With its defaults (segment deletion and
//! throttling off) Jellyfin encodes a whole film ahead of the viewer and keeps
//! every segment, and a session that ends without a clean stop leaves them
//! all behind: one film left 26 GB on 2026-10-18. So Jellyfin is told to keep
//! only what's near the playhead, and segments nobody has touched for a while
//! are removed.

use crate::log::log;
use crate::media::jf_admin;
use crate::model::Level;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};
use tauri::AppHandle;

/// Segments older than this belong to a session that's over. A paused one
/// that comes back has Jellyfin encode again from where it is.
const STALE: Duration = Duration::from_secs(30 * 60);

static SETTINGS_DONE: AtomicBool = AtomicBool::new(false);

/// Called from the waiting-list watcher (every 2 minutes).
pub async fn keep_tidy(app: &AppHandle) {
    let Some(j) = jf_admin(app).filter(|j| j.user_id.is_some()) else { return };
    let Ok(enc) = j.get_json("/System/Configuration/encoding").await else { return };
    if !SETTINGS_DONE.load(Ordering::Relaxed) {
        if enc["EnableSegmentDeletion"] != true || enc["EnableThrottling"] != true {
            let mut next = enc.clone();
            next["EnableSegmentDeletion"] = json!(true);
            next["EnableThrottling"] = json!(true);
            match j.post_json("/System/Configuration/encoding", &next).await {
                Ok(_) => {
                    log(app, Level::Ok, "Jellyfin now deletes watched video pieces as you go, so playing doesn't fill your drive.");
                    SETTINGS_DONE.store(true, Ordering::Relaxed);
                }
                // Tried again on the next check.
                Err(e) => log(app, Level::Amber, format!("Couldn't change Jellyfin's transcode settings: {e:?}")),
            }
        } else {
            SETTINGS_DONE.store(true, Ordering::Relaxed);
        }
    }
    let Some(dir) = folder(&j, &enc).await else { return };
    let freed = tauri::async_runtime::spawn_blocking(move || sweep(&dir, SystemTime::now())).await.unwrap_or(0);
    if freed > 0 {
        log(app, Level::Ok, format!("Cleared {} of leftover video pieces from Jellyfin's cache.", crate::storage::gb(freed)));
    }
}

/// Where Jellyfin writes transcodes, if it's on this PC.
async fn folder(j: &crate::jellyfin::Jf, enc: &Value) -> Option<PathBuf> {
    let set = enc["TranscodingTempPath"].as_str().filter(|p| !p.trim().is_empty()).map(PathBuf::from);
    let dir = match set {
        Some(p) => p,
        None => PathBuf::from(j.get_json("/System/Info").await.ok()?["CachePath"].as_str()?).join("transcodes"),
    };
    is_transcode_folder(&dir).then_some(dir)
}

/// Only a folder Jellyfin marked as its own is ever swept.
fn is_transcode_folder(dir: &Path) -> bool {
    dir.join(".jellyfin-transcode").is_file()
}

/// Jellyfin names transcode files after a 32-hex job id, maybe followed by a
/// segment number. Anything else, should the transcode path point at a shared
/// folder, is never touched.
fn jellyfin_named(name: &str) -> bool {
    let Some((stem, ext)) = name.rsplit_once('.') else { return false };
    let hex = stem.get(..32).is_some_and(|h| h.bytes().all(|b| b.is_ascii_hexdigit()));
    hex && stem[32..].bytes().all(|b| b.is_ascii_digit()) && ["ts", "mp4", "m4s", "m3u8", "vtt", "mkv", "ass", "srt", "log"].contains(&ext.to_ascii_lowercase().as_str())
}

/// Deletes files in `dir` (not below it) untouched for STALE. Returns bytes freed.
fn sweep(dir: &Path, now: SystemTime) -> u64 {
    if !is_transcode_folder(dir) {
        return 0;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let mut freed = 0;
    for e in entries.flatten() {
        let Ok(meta) = e.metadata() else { continue };
        if !meta.is_file() || !jellyfin_named(&e.file_name().to_string_lossy()) {
            continue;
        }
        let old = meta.modified().ok().and_then(|m| now.duration_since(m).ok()).is_some_and(|age| age > STALE);
        if old && std::fs::remove_file(e.path()).is_ok() {
            freed += meta.len();
        }
    }
    freed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_jellyfins_names() {
        assert!(jellyfin_named("dc1392fc54a4f8b0491a2765e51e2c92.m3u8"));
        assert!(jellyfin_named("dc1392fc54a4f8b0491a2765e51e2c921081.ts"));
        assert!(!jellyfin_named("Toy Story 5.mkv"));
        assert!(!jellyfin_named("dc1392fc54a4f8b0491a2765e51e2c92-final.ts"));
        assert!(!jellyfin_named("dc1392fc54a4f8b0491a2765e51e2c92.exe"));
    }

    #[test]
    fn sweeps_only_old_files_in_a_marked_folder() {
        let dir = std::env::temp_dir().join(format!("chillarr-transcodes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let seg = "dc1392fc54a4f8b0491a2765e51e2c920.ts";
        std::fs::write(dir.join(seg), vec![0u8; 100]).unwrap();
        // Not Jellyfin's naming: never touched.
        std::fs::write(dir.join("holiday.mp4"), vec![0u8; 7]).unwrap();
        // Not marked as Jellyfin's: left alone.
        assert_eq!(sweep(&dir, SystemTime::now() + STALE * 2), 0);
        std::fs::write(dir.join(".jellyfin-transcode"), b"").unwrap();
        // Fresh: kept.
        assert_eq!(sweep(&dir, SystemTime::now()), 0);
        // Old: gone, and the marker stays.
        assert_eq!(sweep(&dir, SystemTime::now() + STALE * 2), 100);
        assert!(!dir.join(seg).exists() && dir.join(".jellyfin-transcode").exists() && dir.join("holiday.mp4").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
