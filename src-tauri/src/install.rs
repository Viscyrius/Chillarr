//! Installs a missing app from its official source, only after the user said
//! yes in the window. Sonarr, Radarr, Prowlarr and qBittorrent come through
//! winget (Windows' package manager, which checks each installer's hash).
//! FlareSolverr has no winget package, so one pinned GitHub release zip is
//! fetched and checked against a SHA-256 compiled in here.

use crate::model::AppId;
use sha2::{Digest, Sha256};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, PartialEq, Eq)]
pub enum Source {
    Winget(&'static str),
    /// One release, fixed: `sha256` is compiled in, not read from the
    /// response, so a changed asset or a hijacked "latest" is refused.
    GithubZip { repo: &'static str, tag: &'static str, asset: &'static str, sha256: &'static str },
}

/// FlareSolverr v3.5.2 (released 2026-09-12). The hash is of the asset as
/// downloaded on 2026-10-19 and equals the digest GitHub's API publishes for
/// it. A newer release means updating both here.
const FLARE_TAG: &str = "v3.5.2";
const FLARE_ZIP: &str = "flaresolverr_windows_x64.zip";
const FLARE_SHA256: &str = "309abd7c821546325ae6c509b40d969c5e39034be2314d21b3f5e0517feda7a7";

pub fn source(id: AppId) -> Source {
    match id {
        AppId::Sonarr => Source::Winget("TeamSonarr.Sonarr"),
        AppId::Radarr => Source::Winget("TeamRadarr.Radarr"),
        AppId::Prowlarr => Source::Winget("TeamProwlarr.Prowlarr"),
        AppId::QBittorrent => Source::Winget("qBittorrent.qBittorrent"),
        AppId::Jellyfin => Source::Winget("Jellyfin.Server"),
        AppId::FlareSolverr => Source::GithubZip { repo: "FlareSolverr/FlareSolverr", tag: FLARE_TAG, asset: FLARE_ZIP, sha256: FLARE_SHA256 },
    }
}

/// `want` is the pinned hex SHA-256. No hash, no trust.
pub fn sha256_matches(bytes: &[u8], want: &str) -> bool {
    let got: String = Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
    want.len() == 64 && got.eq_ignore_ascii_case(want)
}

/// The pinned release asset's download URL.
pub fn asset_url(repo: &str, tag: &str, asset: &str) -> String {
    format!("https://github.com/{repo}/releases/download/{tag}/{asset}")
}

pub fn trusted_download(url: &str) -> bool {
    url.starts_with("https://github.com/")
}

/// Where Chillarr puts a FlareSolverr it installed.
pub fn flare_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join("Chillarr").join("FlareSolverr")
}

fn winget(id: &str) -> Result<(), String> {
    let out = Command::new("winget")
        .args([
            "install",
            "--id",
            id,
            "--exact",
            "--source",
            "winget",
            "--silent",
            "--accept-package-agreements",
            "--accept-source-agreements",
            "--disable-interactivity",
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("Couldn't run winget (Windows Package Manager): {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let last = text.lines().map(str::trim).filter(|l| !l.is_empty()).last().unwrap_or("no details");
    Err(format!("winget couldn't install it: {last}"))
}

async fn github_zip(repo: &str, tag: &str, asset: &str, sha256: &str, dir: &std::path::Path) -> Result<(), String> {
    let http = reqwest::Client::builder()
        .user_agent("Chillarr")
        .build()
        .map_err(|e| e.to_string())?;
    let url = asset_url(repo, tag, asset);
    if !trusted_download(&url) {
        return Err(format!("Refusing to download from {url}"));
    }
    let bytes = http
        .get(&url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("Download failed: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("Download failed: {e}"))?;
    if !sha256_matches(&bytes, sha256) {
        return Err("The download didn't match the expected file, so it wasn't used.".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let zip = dir.join(asset);
    std::fs::write(&zip, &bytes).map_err(|e| e.to_string())?;
    // Windows' own tar unpacks zips.
    let status = Command::new(crate::procs::sys32("tar.exe"))
        .arg("-xf")
        .arg(&zip)
        .arg("-C")
        .arg(dir)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("Couldn't unpack it: {e}"))?;
    let _ = std::fs::remove_file(&zip);
    if !status.success() {
        return Err("Couldn't unpack the download.".into());
    }
    Ok(())
}

/// The newer version winget offers, from `winget list --upgrade-available`.
pub fn parse_upgrade(out: &str, id: &str) -> Option<String> {
    let line = out.lines().find(|l| l.split_whitespace().any(|t| t == id))?;
    let after: Vec<&str> = line.split_whitespace().skip_while(|t| *t != id).skip(1).collect();
    // [version, available, source] when there is an upgrade; [version, source] when not.
    (after.len() >= 3).then(|| after[1].to_string())
}

fn winget_run(args: &[&str]) -> Result<std::process::Output, String> {
    Command::new("winget")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("Couldn't run winget (Windows Package Manager): {e}"))
}

/// A newer version of a winget-installed app, if one is out.
pub async fn update_available(id: AppId) -> Option<String> {
    let Source::Winget(pkg) = source(id) else { return None };
    let out = tokio::task::spawn_blocking(move || {
        winget_run(&["list", "--id", pkg, "--exact", "--upgrade-available", "--accept-source-agreements", "--disable-interactivity"])
    })
    .await
    .ok()?
    .ok()?;
    parse_upgrade(&String::from_utf8_lossy(&out.stdout), pkg)
}

/// Updates a winget-installed app to its newest version.
pub async fn upgrade(id: AppId) -> Result<(), String> {
    let Source::Winget(pkg) = source(id) else { return Err("Chillarr can't update this one".into()) };
    let out = tokio::task::spawn_blocking(move || {
        winget_run(&[
            "upgrade",
            "--id",
            pkg,
            "--exact",
            "--silent",
            "--accept-package-agreements",
            "--accept-source-agreements",
            "--disable-interactivity",
        ])
    })
    .await
    .map_err(|e| e.to_string())??;
    if out.status.success() {
        Ok(())
    } else {
        Err("winget couldn't update it".into())
    }
}

/// Installs one app. Blocking parts run off the async runtime.
pub async fn install(id: AppId) -> Result<(), String> {
    match source(id) {
        Source::Winget(pkg) => {
            let pkg = pkg.to_string();
            tokio::task::spawn_blocking(move || winget(&pkg)).await.map_err(|e| e.to_string())?
        }
        Source::GithubZip { repo, tag, asset, sha256 } => github_zip(repo, tag, asset, sha256, &flare_dir()).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AppId;

    #[test]
    fn each_app_has_an_official_source() {
        assert_eq!(source(AppId::Sonarr), Source::Winget("TeamSonarr.Sonarr"));
        assert_eq!(source(AppId::Radarr), Source::Winget("TeamRadarr.Radarr"));
        assert_eq!(source(AppId::Prowlarr), Source::Winget("TeamProwlarr.Prowlarr"));
        assert_eq!(source(AppId::QBittorrent), Source::Winget("qBittorrent.qBittorrent"));
        assert_eq!(source(AppId::Jellyfin), Source::Winget("Jellyfin.Server"));
        assert_eq!(
            source(AppId::FlareSolverr),
            Source::GithubZip { repo: "FlareSolverr/FlareSolverr", tag: FLARE_TAG, asset: FLARE_ZIP, sha256: FLARE_SHA256 }
        );
    }

    #[test]
    fn sha256_check() {
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(sha256_matches(b"abc", abc));
        assert!(sha256_matches(b"abc", &abc.to_uppercase()));
        assert!(!sha256_matches(b"abd", abc));
        assert!(!sha256_matches(b"abc", ""), "no hash means no trust");
    }

    #[test]
    fn flaresolverr_is_one_pinned_release() {
        let url = asset_url("FlareSolverr/FlareSolverr", FLARE_TAG, FLARE_ZIP);
        assert_eq!(url, "https://github.com/FlareSolverr/FlareSolverr/releases/download/v3.5.2/flaresolverr_windows_x64.zip");
        assert!(trusted_download(&url));
        assert_eq!(FLARE_SHA256.len(), 64);
    }

    #[tokio::test]
    #[ignore]
    async fn live_flaresolverr_download_verifies_and_unpacks() {
        let dir = tempfile::tempdir().unwrap();
        github_zip("FlareSolverr/FlareSolverr", FLARE_TAG, FLARE_ZIP, FLARE_SHA256, dir.path()).await.unwrap();
        assert!(crate::discover::find_flaresolverr(&[dir.path().to_path_buf()]).is_some());
        assert!(!dir.path().join("flaresolverr_windows_x64.zip").exists(), "zip is cleaned up");
    }

    #[test]
    fn reads_the_available_version_from_winget() {
        let out = "Name        Id                      Version Available Source\n\
                   ------------------------------------------------------------\n\
                   qBittorrent qBittorrent.qBittorrent 5.2.4   5.3.1     winget\n";
        assert_eq!(parse_upgrade(out, "qBittorrent.qBittorrent").as_deref(), Some("5.3.1"));
        let none = "Name        Id                      Version Source\n---\nqBittorrent qBittorrent.qBittorrent 5.3.0   winget\n";
        assert_eq!(parse_upgrade(none, "qBittorrent.qBittorrent"), None);
        assert_eq!(parse_upgrade("No installed package found matching input criteria.", "qBittorrent.qBittorrent"), None);
    }

    #[test]
    fn download_url_must_be_github() {
        // The release JSON is data from the network; only follow GitHub URLs.
        assert!(trusted_download("https://github.com/FlareSolverr/FlareSolverr/releases/download/v1/x.zip"));
        assert!(!trusted_download("https://evil.example/x.zip"));
        assert!(!trusted_download("http://github.com/x.zip"));
    }
}
