//! Finds the five apps on this PC: where they live, which port they answer on,
//! and (for the `*arr` apps) the API key from their own config.xml.

use crate::model::{AppConfig, AppId};
use std::path::{Path, PathBuf};

const FLARE_DEFAULT: &str = "http://localhost:8191";
const QBIT_DEFAULT_PORT: u16 = 8080;
const FLARE_SEARCH_DEPTH: usize = 4;

/// `(port, url_base, api_key)` from an `*arr` config.xml.
pub fn parse_arr_config(xml: &str) -> Result<(u16, String, String), String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("config.xml is not valid XML: {e}"))?;
    let field = |name: &str| {
        doc.root_element()
            .children()
            .find(|n| n.has_tag_name(name))
            .and_then(|n| n.text())
            .map(|t| t.trim().to_string())
    };
    let port = field("Port")
        .and_then(|p| p.parse::<u16>().ok())
        .ok_or("config.xml has no Port")?;
    let key = field("ApiKey").filter(|k| !k.is_empty()).ok_or("config.xml has no ApiKey")?;
    let base = field("UrlBase").unwrap_or_default();
    let base = if base.is_empty() || base.starts_with('/') { base } else { format!("/{base}") };
    Ok((port, base.trim_end_matches('/').to_string(), key))
}

/// qBittorrent's Web UI port. Newer builds write `[WebUI] Port=`, older ones
/// `[Preferences] WebUI\Port=`.
pub fn parse_qbit_port(ini: &str) -> Option<u16> {
    let mut section = "";
    for line in ini.lines().map(str::trim) {
        if line.starts_with('[') && line.ends_with(']') {
            section = &line[1..line.len() - 1];
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let hit = (section == "WebUI" && k == "Port") || (section == "Preferences" && k == "WebUI\\Port");
        if hit {
            return v.trim().parse().ok();
        }
    }
    None
}

/// FlareSolverr has no installer, so look for `flaresolverr.exe` a few folders deep.
pub fn find_flaresolverr(roots: &[PathBuf]) -> Option<PathBuf> {
    fn walk(dir: &Path, depth: usize) -> Option<PathBuf> {
        let entries = std::fs::read_dir(dir).ok()?;
        let mut subdirs = Vec::new();
        for e in entries.flatten() {
            let path = e.path();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_file() && e.file_name().eq_ignore_ascii_case("flaresolverr.exe") {
                return Some(path);
            }
            if ft.is_dir() && depth < FLARE_SEARCH_DEPTH {
                let name = e.file_name().to_string_lossy().to_lowercase();
                if !name.starts_with('.') && name != "node_modules" && name != "target" {
                    subdirs.push(path);
                }
            }
        }
        subdirs.into_iter().find_map(|d| walk(&d, depth + 1))
    }
    roots.iter().find_map(|r| walk(r, 0))
}

/// Only an Automatic service is left to Windows to run.
pub fn counts_as_service(start: Option<u32>) -> bool {
    start == Some(2)
}

/// qBittorrent's default download folder from its ini.
pub fn parse_qbit_save_path(ini: &str) -> Option<String> {
    let mut section = "";
    for line in ini.lines().map(str::trim) {
        if line.starts_with('[') && line.ends_with(']') {
            section = &line[1..line.len() - 1];
            continue;
        }
        if section == "BitTorrent" {
            if let Some(v) = line.strip_prefix("Session\\DefaultSavePath=") {
                let p = v.replace("\\\\", "\\").replace('/', "\\");
                let p = p.trim_end_matches('\\').to_string();
                return (!p.is_empty()).then_some(p);
            }
        }
    }
    None
}

/// Where qBittorrent puts downloads: its setting, else the user's Downloads.
pub fn qbit_save_path() -> Option<String> {
    env_path("APPDATA")
        .and_then(|p| std::fs::read_to_string(p.join("qBittorrent").join("qBittorrent.ini")).ok())
        .and_then(|ini| parse_qbit_save_path(&ini))
        .or_else(|| env_path("USERPROFILE").map(|p| p.join("Downloads").to_string_lossy().to_string()))
}

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).map(PathBuf::from)
}

/// One `*arr` app, from its config.xml at `config`.
pub fn arr_config(id: AppId, config: &Path) -> AppConfig {
    let name = id.label();
    let program_data = config.parent().map(Path::to_path_buf).unwrap_or_default();
    // A Manual service is one Chillarr runs as the user instead (see imports.rs).
    let service = counts_as_service(crate::winsvc::start_type(name)).then(|| name.to_string());
    let exe = Some(program_data.join("bin").join(format!("{name}.exe"))).filter(|p| p.exists());
    let mut cfg = AppConfig {
        id,
        base_url: String::new(),
        api_key: None,
        exe,
        service,
        managed: true,
        problem: None,
    };
    match std::fs::read_to_string(config).map_err(|e| format!("Can't read its config ({e})")) {
        Ok(xml) => match parse_arr_config(&xml) {
            Ok((port, base, key)) => {
                cfg.base_url = format!("http://localhost:{port}{base}");
                cfg.api_key = Some(key);
            }
            Err(e) => cfg.problem = Some(e),
        },
        Err(e) => {
            cfg.problem = Some(if cfg.exe.is_none() && cfg.service.is_none() {
                "Not installed (no config found)".to_string()
            } else {
                e
            })
        }
    }
    cfg
}

pub fn is_installed(cfg: &AppConfig) -> bool {
    cfg.exe.is_some() || cfg.service.is_some()
}

/// Where to look for flaresolverr.exe, Chillarr's own install first.
pub fn flare_roots() -> Vec<PathBuf> {
    [
        Some(crate::install::flare_dir()),
        env_path("USERPROFILE").map(|p| p.join("Desktop")),
        env_path("USERPROFILE").map(|p| p.join("Downloads")),
        env_path("ProgramFiles"),
        env_path("LOCALAPPDATA"),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn flaresolverr_config() -> AppConfig {
    let exe = find_flaresolverr(&flare_roots());
    AppConfig {
        id: AppId::FlareSolverr,
        base_url: FLARE_DEFAULT.to_string(),
        api_key: None,
        problem: exe.is_none().then(|| "Not installed".to_string()),
        exe,
        service: None,
        managed: true,
    }
}

fn qbit_config() -> AppConfig {
    let exe = [env_path("ProgramFiles"), env_path("ProgramFiles(x86)")]
        .into_iter()
        .flatten()
        .map(|p| p.join("qBittorrent").join("qbittorrent.exe"))
        .find(|p| p.exists());
    let port = env_path("APPDATA")
        .and_then(|p| std::fs::read_to_string(p.join("qBittorrent").join("qBittorrent.ini")).ok())
        .and_then(|ini| parse_qbit_port(&ini))
        .unwrap_or(QBIT_DEFAULT_PORT);
    AppConfig {
        id: AppId::QBittorrent,
        base_url: format!("http://localhost:{port}"),
        api_key: None,
        problem: exe.is_none().then(|| "Not installed".to_string()),
        exe,
        service: None,
        managed: true,
    }
}

/// Jellyfin's web port from its network.xml.
pub fn parse_jellyfin_port(xml: &str) -> Option<u16> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    doc.descendants().find(|n| n.has_tag_name("InternalHttpPort"))?.text()?.trim().parse().ok()
}

fn jellyfin_config() -> AppConfig {
    let pf = env_path("ProgramFiles").unwrap_or_else(|| PathBuf::from("C:\\Program Files"));
    let exe = Some(pf.join("Jellyfin").join("Server").join("jellyfin.exe")).filter(|p| p.exists());
    let service = counts_as_service(crate::winsvc::start_type("JellyfinServer")).then(|| "JellyfinServer".to_string());
    let port = env_path("ProgramData")
        .and_then(|p| std::fs::read_to_string(p.join("Jellyfin").join("Server").join("config").join("network.xml")).ok())
        .and_then(|x| parse_jellyfin_port(&x))
        .unwrap_or(8096);
    AppConfig {
        id: AppId::Jellyfin,
        base_url: format!("http://localhost:{port}"),
        api_key: None,
        problem: (exe.is_none() && service.is_none()).then(|| "Not installed".to_string()),
        exe,
        service,
        managed: true,
    }
}

/// Always one entry per app, in `AppId::ALL` order. A failure goes in `problem`.
pub fn discover_all() -> Vec<AppConfig> {
    let program_data = env_path("ProgramData").unwrap_or_else(|| PathBuf::from("C:\\ProgramData"));
    AppId::ALL
        .iter()
        .map(|&id| match id {
            AppId::FlareSolverr => flaresolverr_config(),
            AppId::QBittorrent => qbit_config(),
            AppId::Jellyfin => jellyfin_config(),
            _ => arr_config(id, &program_data.join(id.label()).join("config.xml")),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SONARR_XML: &str = include_str!("../tests/fixtures/sonarr-config.xml");
    const QBIT_INI: &str = include_str!("../tests/fixtures/qbittorrent.ini");

    #[test]
    fn parses_arr_config() {
        let (port, base, key) = parse_arr_config(SONARR_XML).unwrap();
        assert_eq!(port, 8989);
        assert_eq!(base, "");
        assert_eq!(key, "00000000000000000000000000000000");
    }

    #[test]
    fn missing_api_key_is_error() {
        let err = parse_arr_config("<Config><Port>1</Port></Config>").unwrap_err();
        assert!(err.contains("ApiKey"), "{err}");
    }

    #[test]
    fn parses_qbit_port_both_layouts() {
        assert_eq!(parse_qbit_port(QBIT_INI), Some(6767));
        assert_eq!(parse_qbit_port("[WebUI]\nPort=6767\n"), Some(6767));
        assert_eq!(parse_qbit_port("[Preferences]\nWebUI\\Port=8080\n"), Some(8080));
        assert_eq!(parse_qbit_port("[BitTorrent]\nPort=1234\n"), None);
    }

    #[test]
    fn finds_flaresolverr_in_nested_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("a").join("b").join("flaresolverr");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("flaresolverr.exe"), b"").unwrap();
        assert_eq!(
            find_flaresolverr(&[tmp.path().to_path_buf()]),
            Some(dir.join("flaresolverr.exe"))
        );

        let deep = tempfile::tempdir().unwrap();
        let d = deep.path().join("1").join("2").join("3").join("4").join("5");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("flaresolverr.exe"), b"").unwrap();
        assert_eq!(find_flaresolverr(&[deep.path().to_path_buf()]), None);
    }

    #[test]
    fn discover_all_never_panics_and_has_five() {
        let all = discover_all();
        let ids: Vec<AppId> = all.iter().map(|a| a.id).collect();
        assert_eq!(ids, AppId::ALL.to_vec());
    }

    #[test]
    fn unreadable_config_is_a_problem_not_a_panic() {
        // Review Focus 1: an installed app whose config.xml can't be read.
        let cfg = arr_config(AppId::Sonarr, std::path::Path::new("Z:/nope/config.xml"));
        assert!(cfg.api_key.is_none());
        assert!(cfg.problem.as_deref().unwrap_or("").contains("config"));
    }

    #[test]
    fn only_an_automatic_service_counts_as_a_service() {
        // Start=2 is Automatic. A Manual (3) or Disabled (4) one is run as the user.
        assert!(counts_as_service(Some(2)));
        assert!(!counts_as_service(Some(3)));
        assert!(!counts_as_service(Some(4)));
        assert!(!counts_as_service(None));
    }

    #[test]
    fn qbit_save_path_from_ini() {
        let ini = "[BitTorrent]\nSession\\DefaultSavePath=D:\\\\Torrents\n";
        assert_eq!(parse_qbit_save_path(ini).as_deref(), Some(r"D:\Torrents"));
        let ini2 = "[BitTorrent]\nSession\\DefaultSavePath=D:/Torrents/\n";
        assert_eq!(parse_qbit_save_path(ini2).as_deref(), Some(r"D:\Torrents"));
        assert_eq!(parse_qbit_save_path("[WebUI]\nPort=1\n"), None);
    }

    #[test]
    fn jellyfin_port_from_network_xml() {
        let xml = "<?xml version=\"1.0\"?><NetworkConfiguration><InternalHttpPort>8097</InternalHttpPort></NetworkConfiguration>";
        assert_eq!(parse_jellyfin_port(xml), Some(8097));
        assert_eq!(parse_jellyfin_port("<NetworkConfiguration/>"), None);
    }

    #[test]
    #[ignore]
    fn live_jellyfin_is_found() {
        let all = discover_all();
        let jf = all.iter().find(|a| a.id == AppId::Jellyfin).unwrap();
        assert_eq!(jf.base_url, "http://localhost:8096");
        assert_eq!(jf.service.as_deref(), Some("JellyfinServer"));
        assert!(jf.exe.as_ref().unwrap().ends_with("jellyfin.exe"));
    }

    #[test]
    fn installed_means_an_exe_or_a_service() {
        let mut c = arr_config(AppId::Sonarr, std::path::Path::new("Z:/nope/config.xml"));
        c.exe = None;
        c.service = None;
        assert!(!is_installed(&c));
        c.service = Some("Sonarr".into());
        assert!(is_installed(&c));
        c.service = None;
        c.exe = Some("C:/x/Sonarr.exe".into());
        assert!(is_installed(&c));
    }

    #[test]
    fn chillarr_installed_flaresolverr_is_found_first() {
        assert_eq!(flare_roots()[0], crate::install::flare_dir());
    }

    #[test]
    #[ignore]
    fn live_discovery_matches_this_pc() {
        let all = discover_all();
        let url = |id| all.iter().find(|a| a.id == id).unwrap().base_url.clone();
        assert_eq!(url(AppId::Prowlarr), "http://localhost:9696");
        assert_eq!(url(AppId::Radarr), "http://localhost:7878");
        assert_eq!(url(AppId::Sonarr), "http://localhost:8989");
        assert_eq!(url(AppId::FlareSolverr), "http://localhost:8191");
        assert_eq!(url(AppId::QBittorrent), "http://localhost:6767");
        let get = |id| all.iter().find(|a| a.id == id).unwrap().clone();
        assert_eq!(get(AppId::Prowlarr).service.as_deref(), Some("Prowlarr"));
        assert_eq!(get(AppId::Sonarr).service, None);
        assert!(get(AppId::Sonarr).exe.unwrap().ends_with("Sonarr.exe"));
        assert!(get(AppId::FlareSolverr).exe.is_some());
        assert!(get(AppId::Radarr).api_key.is_some());
    }
}
