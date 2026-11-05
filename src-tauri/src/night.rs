//! Night mode for devices whose browser can't touch the sound. An iPhone plays
//! HLS in the system player, out of reach of Web Audio, so the player can't
//! compress anything there (and a TV decoding in hardware is the same). Here the
//! track itself is evened out once, into a file beside Jellyfin's own data for
//! the item (never the media folder), which Jellyfin lists as one more audio
//! track. Playing it is then an ordinary change of audio track, the path every
//! device already plays.
//!
//! Measured on a 5.1 film, 2026-11-05 (10 minutes, ebur128): as Jellyfin sends it
//! today, range 14.0 LU, peak +1.5 dBFS; Jellyfin's own NightmodeDialogue downmix
//! barely moves it (13.4 LU, +2.9 dBFS), so it isn't used; `FILTER` gives 8.6 LU,
//! -0.4 dBFS, 6 dB louder overall. Making it takes a few minutes per film.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};
use tokio::io::AsyncBufReadExt;

const FILTER: &str = "dynaudnorm=f=150:g=15:m=6:p=0.9,acompressor=threshold=-24dB:ratio=4:attack=5:release=250:makeup=6dB:knee=6,alimiter=limit=0.89:level=false";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
/// About 145 MB for a two-hour film. The oldest go past this many.
const KEEP: usize = 20;
/// How long Jellyfin gets to list a finished file.
const LIST_WAIT: Duration = Duration::from_secs(90);
/// How long a failure is repeated before it's tried again.
const FAIL_FOR: Duration = Duration::from_secs(300);

/// `<video name>.Night-<track>.m4a`: the name Jellyfin needs to tie it to the
/// video, and the track it was made from. No language in it, so Jellyfin's
/// "preferred audio language" never picks it by itself.
///
/// `track` is the sound's place among the file's own audio tracks (0 = first).
/// Not Jellyfin's track number: that counts files beside the video too, and every
/// number moves when one is added (measured 2026-11-05: making this very copy
/// moved the film's sound from 1 to 2, and the next ask made a copy of the
/// picture's number, which ffmpeg refused).
fn file_name(stem: &str, track: i64) -> String {
    format!("{stem}.Night-{track}.m4a")
}

/// The track a night copy was made from, by its file name.
pub fn copy_of(path: &str) -> Option<i64> {
    let name = path.rsplit(['/', '\\']).next()?.to_ascii_lowercase();
    let rest = name.strip_suffix(".m4a")?;
    let at = rest.rfind(".night-")?;
    rest[at + 7..].parse().ok()
}

/// Jellyfin's own folder for an item's data, where it also looks for extra tracks.
fn item_folder(metadata: &str, id: &str) -> Option<PathBuf> {
    let id: String = id.chars().filter(|c| *c != '-').collect::<String>().to_ascii_lowercase();
    if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(Path::new(metadata).join("library").join(&id[..2]).join(&id))
}

#[derive(Clone)]
enum Job {
    Making(f64),
    /// Made; Jellyfin has until then to list it.
    Listing(Instant),
    Failed(String, Instant),
}

static JOBS: std::sync::LazyLock<Mutex<HashMap<(String, i64), Job>>> = std::sync::LazyLock::new(Default::default);
static ONE_AT_A_TIME: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

fn set_job(key: &(String, i64), job: Option<Job>) {
    let mut m = JOBS.lock().unwrap_or_else(|e| e.into_inner());
    match job {
        Some(j) => m.insert(key.clone(), j),
        None => m.remove(key),
    };
}

/// The place of each of a title's own audio tracks, by Jellyfin's number.
pub fn own_tracks(streams: &[Value]) -> Vec<i64> {
    streams.iter().filter(|s| s["Type"] == "Audio" && s["IsExternal"] != true).filter_map(|s| s["Index"].as_i64()).collect()
}

/// The night copy of `track` (the item's default when None): ready with its
/// Jellyfin track number, or being made. Asked every few seconds by the player.
pub async fn prepare(app: &AppHandle, id: &str, track: Option<i64>) -> Result<Value, String> {
    let j = crate::media::jf_admin(app).filter(|j| j.user_id.is_some()).ok_or("Jellyfin isn't signed in.")?;
    let admin = j.user_id.clone().unwrap_or_default();
    let item = j.get_json(&format!("/Users/{admin}/Items/{id}?fields=Path,MediaSources,MediaStreams")).await.map_err(|e| e.to_string())?;
    if crate::jellyfin::on_shelf(&item) {
        return Err("Night mode works here once it's downloaded.".into());
    }
    let path = item["Path"].as_str().filter(|p| !p.is_empty()).ok_or("Jellyfin doesn't say where this file is.")?.to_string();
    let source = &item["MediaSources"][0];
    let streams = source["MediaStreams"].as_array().cloned().unwrap_or_default();
    let tracks: Vec<&Value> = streams.iter().filter(|s| s["Type"] == "Audio").collect();
    let is_night = |s: &Value| s["IsExternal"] == true && copy_of(s["Path"].as_str().unwrap_or_default()).is_some();
    let own: Vec<&Value> = tracks.iter().copied().filter(|s| s["IsExternal"] != true).collect();
    if own.is_empty() {
        return Err("This has no sound to change.".into());
    }
    let track = track.unwrap_or_else(|| own.iter().position(|s| s["IsDefault"] == true).unwrap_or(0) as i64);
    if track < 0 || track as usize >= own.len() {
        return Err("That sound track isn't there any more.".into());
    }
    let key = (id.to_string(), track);
    let info = j.get_json("/System/Info").await.map_err(|e| e.to_string())?;
    let metadata = info["InternalMetadataPath"].as_str().ok_or("Jellyfin doesn't say where it keeps its data.")?;
    let stem = Path::new(&path).file_stem().and_then(|s| s.to_str()).ok_or("This file's name can't be read.")?;
    let folder = item_folder(metadata, id).ok_or("That isn't a Jellyfin title.")?;
    let file = folder.join(file_name(stem, track));

    let listed = tracks.iter().find(|s| is_night(s) && copy_of(s["Path"].as_str().unwrap_or_default()) == Some(track));
    if let (Some(s), true) = (listed, file.is_file()) {
        set_job(&key, None);
        return Ok(json!({ "state": "ready", "index": s["Index"] }));
    }
    let job = JOBS.lock().unwrap_or_else(|e| e.into_inner()).get(&key).cloned();
    match job {
        Some(Job::Making(p)) => return Ok(json!({ "state": "making", "pct": p })),
        Some(Job::Listing(until)) => {
            if Instant::now() < until {
                return Ok(json!({ "state": "making", "pct": 100.0 }));
            }
            let msg = "Jellyfin didn't pick up the night sound.".to_string();
            set_job(&key, Some(Job::Failed(msg.clone(), Instant::now() + FAIL_FOR)));
            return Err(msg);
        }
        Some(Job::Failed(msg, until)) if Instant::now() < until => return Err(msg),
        _ => {}
    }
    set_job(&key, Some(Job::Making(0.0)));
    let seconds = source["RunTimeTicks"].as_i64().map(crate::jellyfin::ticks_to_seconds).unwrap_or(0.0);
    let (app2, id2) = (app.clone(), id.to_string());
    tauri::async_runtime::spawn(async move {
        let mut made = Ok(());
        if !file.is_file() {
            // The encoder very rarely gives up partway on one run and not the next.
            made = make(&key, Path::new(&path), track, &folder, &file, seconds).await;
            if let Err(e) = &made {
                crate::log::detail(format!("Night sound, first try: {e}"));
                made = make(&key, Path::new(&path), track, &folder, &file, seconds).await;
            }
        }
        match made {
            Ok(()) => {
                keep_list(&app2, &file);
                set_job(&key, Some(Job::Listing(Instant::now() + LIST_WAIT)));
                // Jellyfin only looks for extra tracks on a full refresh. Nothing
                // the item already has is replaced, and no pictures are fetched.
                let _ = j.post_json(&format!("/Items/{id2}/Refresh?metadataRefreshMode=FullRefresh&imageRefreshMode=None&replaceAllMetadata=false&replaceAllImages=false"), &json!({})).await;
            }
            Err(e) => {
                crate::log::log(&app2, crate::model::Level::Amber, format!("Couldn't make the night sound: {e}"));
                set_job(&key, Some(Job::Failed("Couldn't make the night sound for this one.".into(), Instant::now() + FAIL_FOR)));
            }
        }
    });
    Ok(json!({ "state": "making", "pct": 0.0 }))
}

async fn make(key: &(String, i64), video: &Path, track: i64, folder: &Path, file: &Path, seconds: f64) -> Result<(), String> {
    let _one = ONE_AT_A_TIME.acquire().await.map_err(|e| e.to_string())?;
    std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    // Not a name Jellyfin would take for a track while it's half made.
    let part = folder.join(format!("night-{track}.making"));
    let mut child = tokio::process::Command::new(crate::subsync::ffmpeg())
        .args(["-nostdin", "-v", "error", "-progress", "pipe:1", "-i"])
        .arg(video)
        .args(["-map", &format!("0:a:{track}"), "-vn", "-sn", "-dn", "-ac", "2", "-af", FILTER, "-c:a", "aac", "-b:a", "160k", "-movflags", "+faststart", "-f", "mp4", "-y"])
        .arg(&part)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS)
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("couldn't start ffmpeg: {e}"))?;
    let out = child.stdout.take().ok_or("ffmpeg gave no output")?;
    let err = child.stderr.take().ok_or("ffmpeg gave no output")?;
    // Read as it comes, or ffmpeg stalls on a full pipe; the end says what went wrong.
    let said = tauri::async_runtime::spawn(async move {
        let mut text = Vec::new();
        let _ = tokio::io::AsyncReadExt::read_to_end(&mut tokio::io::AsyncReadExt::take(err, 64 * 1024), &mut text).await;
        String::from_utf8_lossy(&text).chars().rev().take(300).collect::<Vec<_>>().into_iter().rev().collect::<String>()
    });
    let mut lines = tokio::io::BufReader::new(out).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(us) = line.strip_prefix("out_time_us=").and_then(|v| v.trim().parse::<f64>().ok()) {
            if seconds > 0.0 {
                set_job(key, Some(Job::Making((us / 1e6 / seconds * 100.0).clamp(0.0, 99.0))));
            }
        }
    }
    let status = child.wait().await.map_err(|e| e.to_string())?;
    if !status.success() {
        let _ = std::fs::remove_file(&part);
        return Err(format!("ffmpeg stopped with {status}: {}", said.await.unwrap_or_default().trim()));
    }
    std::fs::rename(&part, file).map_err(|e| e.to_string())
}

/// Remembers the file, and takes out the oldest past `KEEP`.
fn keep_list(app: &AppHandle, file: &Path) {
    let Ok(dir) = app.path().app_data_dir() else { return };
    let list = dir.join("night.json");
    let all: Vec<String> = std::fs::read_to_string(&list).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let (keep, gone) = over_limit(all, &file.to_string_lossy());
    for g in gone {
        let _ = std::fs::remove_file(g);
    }
    if let Ok(text) = serde_json::to_string(&keep) {
        let _ = std::fs::write(list, text);
    }
}

fn over_limit(mut all: Vec<String>, newest: &str) -> (Vec<String>, Vec<String>) {
    all.retain(|p| p != newest);
    all.push(newest.to_string());
    let cut = all.len().saturating_sub(KEEP);
    let gone = all.drain(..cut).collect();
    (all, gone)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_copy_is_known_by_its_name() {
        assert_eq!(copy_of(r"C:\data\library\ab\abc\Film (2020).Night-1.m4a"), Some(1));
        assert_eq!(copy_of(r"C:\x\Film.night-12.M4A"), Some(12));
        assert_eq!(copy_of(r"C:\x\Film.eng.m4a"), None);
        assert_eq!(copy_of(r"C:\x\Night-1.mkv"), None);
        assert_eq!(copy_of(""), None);
        assert_eq!(copy_of(&file_name("Film.2020.1080p", 3)), Some(3));
    }

    #[test]
    fn a_tracks_place_ignores_files_beside_the_video() {
        let streams = vec![
            json!({ "Type": "Audio", "Index": 0, "IsExternal": true }),
            json!({ "Type": "Video", "Index": 1, "IsExternal": false }),
            json!({ "Type": "Audio", "Index": 2, "IsExternal": false }),
            json!({ "Type": "Subtitle", "Index": 3 }),
            json!({ "Type": "Audio", "Index": 4 }),
        ];
        assert_eq!(own_tracks(&streams), vec![2, 4]);
    }

    #[test]
    fn the_folder_is_jellyfins_own_for_that_item() {
        let f = item_folder(r"C:\meta", "D9CC9BE8-BD13-813F-FF79-E6D88A07AF0F").unwrap();
        assert_eq!(f, Path::new(r"C:\meta").join("library").join("d9").join("d9cc9be8bd13813fff79e6d88a07af0f"));
        assert_eq!(item_folder(r"C:\meta", "../../etc"), None);
    }

    #[test]
    fn the_oldest_go_past_the_limit() {
        let all: Vec<String> = (0..KEEP).map(|n| n.to_string()).collect();
        let (keep, gone) = over_limit(all, "new");
        assert_eq!(keep.len(), KEEP);
        assert_eq!(gone, vec!["0".to_string()]);
        assert_eq!(keep.last().map(String::as_str), Some("new"));
        let (keep, gone) = over_limit(vec!["a".into(), "b".into()], "a");
        assert_eq!(keep, vec!["b".to_string(), "a".to_string()]);
        assert!(gone.is_empty());
    }
}
