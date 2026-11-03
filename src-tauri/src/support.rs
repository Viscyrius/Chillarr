//! "Save logs": one text file someone can send when something's wrong. What
//! Chillarr did, how the PC looks, and the end of each app's own log. Keys,
//! tokens and passwords are hidden, the same way as in Chillarr's own log.

use crate::fix::Core;
use crate::log::redact;
use crate::model::AppId;
use std::fmt::Write as _;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

/// Enough to see what went wrong, small enough to send in a chat.
const PER_LOG: u64 = 256 * 1024;

/// The end of a file, starting at a whole line.
pub fn tail(path: &Path, limit: u64) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(limit);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes).to_string();
    Some(if start > 0 { text.split_once('\n').map_or(text.clone(), |(_, rest)| rest.to_string()) } else { text })
}

/// The newest file in a folder whose name starts with `prefix`.
fn newest(dir: &Path, prefix: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().to_ascii_lowercase().starts_with(prefix))
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
        .map(|e| e.path())
}

fn section(out: &mut String, title: &str, body: Option<String>) {
    let _ = writeln!(out, "\n===== {title} =====");
    out.push_str(&redact(&body.unwrap_or_else(|| "(nothing here)".into())));
    out.push('\n');
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).map(PathBuf::from)
}

/// Where the report goes: the Downloads folder, where people look for what they just saved.
fn target(stamp: &str) -> Result<PathBuf, String> {
    let home = env_path("USERPROFILE").ok_or("No user folder")?;
    let dir = [home.join("Downloads"), home.join("Desktop"), home].into_iter().find(|d| d.is_dir()).ok_or("No folder to save in")?;
    Ok(dir.join(format!("Chillarr logs {stamp}.txt")))
}

/// Writes the report; returns its path.
pub async fn save(app: &AppHandle) -> Result<String, String> {
    let cards = crate::fix::refresh_cards(app).await;
    let media = crate::media::status(app).await;
    let (settings, configs) = {
        let st = app.state::<crate::SettingsState>();
        let s = st.current.lock().unwrap().clone();
        (s, app.state::<Core>().configs.read().unwrap().clone())
    };
    let version = app.package_info().version.to_string();
    let identifier = app.config().identifier.clone();
    let lines = app.state::<crate::log::Logs>().0.lock().unwrap().clone();
    let path = target(&chrono::Local::now().format("%Y-%m-%d %H%M").to_string())?;
    let shown = path.clone();
    tokio::task::spawn_blocking(move || {
        let mut out = String::new();
        let _ = writeln!(out, "Chillarr {version} ({identifier}), saved {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
        let _ = writeln!(out, "Windows: {}, setup done: {}, audio: {}, skip disc copies: {}, skip dubs: {}", std::env::var("OS").unwrap_or_default(), settings.setup_done, settings.audio_pref, settings.skip_remux, settings.skip_dubs);
        let _ = writeln!(out, "Jellyfin: installed {} answering {} wizard done {} signed in {}", media.installed, media.up, media.setup_done, media.signed_in);

        let _ = writeln!(out, "\n===== Apps =====");
        for c in &configs {
            let card = cards.iter().find(|x| x.id == c.id);
            let _ = writeln!(
                out,
                "{}: {:?} \"{}\" url={} exe={} service={} managed={} problem={}",
                c.id.label(),
                card.map(|x| x.level),
                card.map_or("", |x| x.line.as_str()),
                c.base_url,
                c.exe.as_ref().map_or("none".to_string(), |e| e.display().to_string()),
                c.service.as_deref().unwrap_or("none"),
                c.managed,
                c.problem.as_deref().unwrap_or("none"),
            );
            if let Some(svc) = &c.service {
                let _ = writeln!(out, "  service running: {:?}, start type: {:?}", crate::procs::service_running(svc), crate::winsvc::start_type(svc));
            }
        }
        let _ = writeln!(out, "\n===== Running =====");
        for name in ["chillarr", "Prowlarr", "Prowlarr.Console", "Sonarr", "Sonarr.Console", "Radarr", "Radarr.Console", "qbittorrent", "jellyfin", "flaresolverr", "meilisearch", "ffmpeg"] {
            let _ = writeln!(out, "{name}: {}", crate::procs::process_running(&format!("{name}.exe")));
        }

        let screen: Vec<String> = lines.iter().rev().map(|l| format!("{} {:?} {}", l.at, l.level, l.text)).collect();
        section(&mut out, "What Chillarr did, as shown on the Logs page", Some(screen.join("\n")));
        let mine = env_path("LOCALAPPDATA").map(|d| d.join("Chillarr"));
        section(&mut out, "Chillarr's log, older", mine.as_ref().and_then(|d| tail(&d.join("chillarr.log.old"), PER_LOG)));
        section(&mut out, "Chillarr's log", mine.as_ref().and_then(|d| tail(&d.join("chillarr.log"), PER_LOG)));
        let pd = env_path("ProgramData");
        for id in [AppId::Prowlarr, AppId::Sonarr, AppId::Radarr] {
            let name = id.label();
            let file = pd.as_ref().map(|d| d.join(name).join("logs").join(format!("{}.txt", name.to_ascii_lowercase())));
            section(&mut out, &format!("{name}'s log"), file.and_then(|f| tail(&f, PER_LOG)));
        }
        let jf = pd.as_ref().map(|d| d.join("Jellyfin").join("Server").join("log"));
        section(&mut out, "Jellyfin's log", jf.and_then(|d| newest(&d, "log_")).and_then(|f| tail(&f, PER_LOG)));
        let qb = mine.as_ref().and_then(|d| d.parent().map(|p| p.join("qBittorrent").join("logs").join("qbittorrent.log")));
        section(&mut out, "qBittorrent's log", qb.and_then(|f| tail(&f, PER_LOG)));
        std::fs::write(&path, out).map_err(|e| format!("Couldn't save the logs: {e}"))
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(shown.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tail_starts_on_a_whole_line() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.log");
        std::fs::write(&f, "first line\nsecond line\nthird\n").unwrap();
        assert_eq!(tail(&f, 1000).unwrap(), "first line\nsecond line\nthird\n", "small file: all of it");
        assert_eq!(tail(&f, 10).unwrap(), "third\n", "cut mid-line: that line goes");
        assert!(tail(&dir.path().join("none"), 10).is_none());
    }

    #[test]
    fn newest_picks_by_time_and_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("other.txt"), "x").unwrap();
        std::fs::write(dir.path().join("log_1.log"), "x").unwrap();
        assert_eq!(newest(dir.path(), "log_").unwrap().file_name().unwrap(), "log_1.log");
        assert!(newest(dir.path(), "zzz").is_none());
    }
}
