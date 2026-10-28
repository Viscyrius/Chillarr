//! What the user can change. Discovery fills everything; an override here
//! wins over it. API keys are never stored: they are re-read from each app.

use crate::model::{AppConfig, AppId};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

fn yes() -> bool {
    true
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Settings {
    #[serde(default = "yes")]
    pub start_with_windows: bool,
    #[serde(default = "yes")]
    pub auto_fix: bool,
    #[serde(default)]
    pub apps: Vec<AppOverride>,
    /// Programs already added to Defender's protected-folder allow list.
    #[serde(default)]
    pub defender_allowed: Vec<String>,
    /// Fix everything may ask to add the media apps to that list. Saying no
    /// turns it off, so it doesn't ask on every press; Settings turns it back on.
    #[serde(default = "yes")]
    pub defender_ask: bool,
    /// The first-run tutorial has been finished (or skipped).
    #[serde(default)]
    pub setup_done: bool,
    /// "chillarr" (built-in player) or "vlc".
    #[serde(default = "default_player")]
    pub player: String,
    /// Downloads never take a full-disc copy (remux, BR-DISK): 25 to 80 GB a
    /// film. On unless turned off in Settings.
    #[serde(default = "yes")]
    pub skip_remux: bool,
    /// Downloads skip copies with a dubbed language added (Hindi, Italian...)
    /// on top of the film's own. On unless turned off in Settings.
    #[serde(default = "yes")]
    pub skip_dubs: bool,
}

fn default_player() -> String {
    "chillarr".into()
}

impl Default for Settings {
    fn default() -> Self {
        Settings { start_with_windows: true, auto_fix: true, apps: Vec::new(), defender_allowed: Vec::new(), defender_ask: true, setup_done: false, player: default_player(), skip_remux: true, skip_dubs: true }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AppOverride {
    pub id: AppId,
    pub exe: Option<PathBuf>,
    pub port: Option<u16>,
    pub managed: bool,
}

impl Settings {
    /// Settings that keep today's discovered values (no overrides yet).
    pub fn from_configs(cfgs: &[AppConfig]) -> Settings {
        Settings {
            apps: cfgs.iter().map(|c| AppOverride { id: c.id, exe: None, port: None, managed: c.managed }).collect(),
            ..Default::default()
        }
    }
}

/// The program names each app really has. A path typed into Settings is
/// only kept when it is one of these, so a stray program can't take an app's
/// place (and with it the app's Defender exception).
pub fn exe_names(id: AppId) -> &'static [&'static str] {
    match id {
        AppId::Prowlarr => &["Prowlarr.exe", "Prowlarr.Console.exe"],
        AppId::Sonarr => &["Sonarr.exe", "Sonarr.Console.exe"],
        AppId::Radarr => &["Radarr.exe", "Radarr.Console.exe"],
        AppId::FlareSolverr => &["flaresolverr.exe"],
        AppId::QBittorrent => &["qbittorrent.exe"],
        AppId::Jellyfin => &["jellyfin.exe"],
    }
}

/// Whether `exe` is a real file with that app's program name.
pub fn exe_ok(id: AppId, exe: &Path) -> bool {
    let named = exe.file_name().is_some_and(|n| exe_names(id).iter().any(|e| n.to_string_lossy().eq_ignore_ascii_case(e)));
    named && exe.is_file()
}

/// Drops the program paths that aren't `exe_ok`. Returns what was dropped, to tell the user.
pub fn keep_real_exes(s: &mut Settings) -> Vec<(AppId, PathBuf)> {
    let mut dropped = Vec::new();
    for o in &mut s.apps {
        if let Some(exe) = o.exe.take() {
            if exe_ok(o.id, &exe) {
                o.exe = Some(exe);
            } else {
                dropped.push((o.id, exe));
            }
        }
    }
    dropped
}

fn with_port(url: &str, port: u16) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("http", url));
    let (host_port, path) = rest.split_once('/').map(|(h, p)| (h, format!("/{p}"))).unwrap_or((rest, String::new()));
    let host = host_port.split(':').next().unwrap_or("localhost");
    format!("{scheme}://{host}:{port}{path}")
}

pub fn apply(mut discovered: Vec<AppConfig>, s: &Settings) -> Vec<AppConfig> {
    for cfg in &mut discovered {
        let Some(o) = s.apps.iter().find(|o| o.id == cfg.id) else { continue };
        cfg.managed = o.managed;
        if let Some(port) = o.port {
            cfg.base_url = with_port(&cfg.base_url, port);
        }
        if let Some(exe) = &o.exe {
            cfg.exe = Some(exe.clone());
            if !cfg.id.is_arr() {
                cfg.problem = None;
            }
        }
    }
    discovered
}

/// `(settings, first_run)`. A missing or unreadable file means first run.
pub fn load(path: &Path) -> (Settings, bool) {
    match std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()) {
        Some(s) => (s, false),
        None => (Settings::default(), true),
    }
}

pub fn save(path: &Path, s: &Settings) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AppConfig, AppId};

    fn cfg(id: AppId, url: &str) -> AppConfig {
        AppConfig {
            id,
            base_url: url.into(),
            api_key: Some("secret-key-value".into()),
            exe: None,
            service: None,
            managed: true,
            problem: Some("Can't find flaresolverr.exe. Pick it in Settings.".into()),
        }
    }

    #[test]
    fn settings_roundtrip_has_no_key() {
        let s = Settings::from_configs(&[cfg(AppId::Radarr, "http://localhost:7878")]);
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("secret-key-value"), "{json}");
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert!(back.start_with_windows && back.auto_fix);
        assert_eq!(back.apps.len(), 1);
        // AppConfig itself must not leak the key either (it is sent to the window).
        assert!(!serde_json::to_string(&cfg(AppId::Radarr, "x")).unwrap().contains("secret-key-value"));
    }

    #[test]
    fn override_wins() {
        let mut s = Settings::default();
        s.apps.push(AppOverride { id: AppId::Radarr, exe: None, port: Some(9999), managed: false });
        s.apps.push(AppOverride {
            id: AppId::FlareSolverr,
            exe: Some("D:/fs/flaresolverr.exe".into()),
            port: None,
            managed: true,
        });
        let out = apply(
            vec![cfg(AppId::Radarr, "http://localhost:7878/radarr"), cfg(AppId::FlareSolverr, "http://localhost:8191")],
            &s,
        );
        assert_eq!(out[0].base_url, "http://localhost:9999/radarr");
        assert!(!out[0].managed);
        assert_eq!(out[1].exe.as_deref(), Some(std::path::Path::new("D:/fs/flaresolverr.exe")));
        assert!(out[1].problem.is_none(), "a picked exe clears the 'can't find' problem");
    }

    #[test]
    fn only_the_apps_own_programs_are_kept() {
        let d = tempfile::tempdir().unwrap();
        let qb = d.path().join("qbittorrent.exe");
        let other = d.path().join("evil.exe");
        let sonarr = d.path().join("SONARR.EXE");
        for f in [&qb, &other, &sonarr] {
            std::fs::write(f, b"").unwrap();
        }
        assert!(exe_ok(AppId::QBittorrent, &qb));
        assert!(exe_ok(AppId::Sonarr, &sonarr), "case doesn't matter");
        assert!(!exe_ok(AppId::QBittorrent, &other), "wrong name");
        assert!(!exe_ok(AppId::Radarr, &qb), "another app's program");
        assert!(!exe_ok(AppId::Jellyfin, &d.path().join("jellyfin.exe")), "not there");
        assert!(!exe_ok(AppId::QBittorrent, d.path()), "a folder");

        let mut s = Settings::default();
        s.apps.push(AppOverride { id: AppId::QBittorrent, exe: Some(qb.clone()), port: None, managed: true });
        s.apps.push(AppOverride { id: AppId::Radarr, exe: Some(other.clone()), port: Some(7000), managed: true });
        s.apps.push(AppOverride { id: AppId::Jellyfin, exe: None, port: None, managed: false });
        let dropped = keep_real_exes(&mut s);
        assert_eq!(dropped, vec![(AppId::Radarr, other)]);
        assert_eq!(s.apps[0].exe.as_deref(), Some(qb.as_path()));
        assert!(s.apps[1].exe.is_none());
        assert_eq!(s.apps[1].port, Some(7000), "the rest of the row stays");
    }

    #[test]
    fn old_settings_still_ask_about_defender() {
        let s: Settings = serde_json::from_str(r#"{"apps":[],"defender_allowed":[]}"#).unwrap();
        assert!(s.defender_ask);
    }

    #[test]
    fn missing_file_means_first_run() {
        let dir = tempfile::tempdir().unwrap();
        let (s, first) = load(&dir.path().join("settings.json"));
        assert!(first);
        assert!(s.start_with_windows && s.auto_fix);
        save(&dir.path().join("settings.json"), &s).unwrap();
        assert!(!load(&dir.path().join("settings.json")).1);
    }
}
