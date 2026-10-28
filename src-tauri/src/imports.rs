//! Why finished downloads never reach the library: Radarr/Sonarr installed as
//! a Windows service run as LOCAL SERVICE, which is not allowed into the
//! user's Downloads folder, so they can't move what qBittorrent finished.
//! The fix runs the app as the signed-in user instead (one UAC prompt), the
//! way Sonarr is normally installed, then has it import what is already
//! waiting there, as hardlinks.

use crate::procs::{ps_all, ps_chain, ps_quote, ps_tool};
use windows::core::HSTRING;
use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};

/// The SID icacls needs for a service's account. `None` when no grant is needed
/// (LocalSystem can read anything; a user account is the user).
pub fn service_sid(account: &str) -> Option<&'static str> {
    let a = account.to_ascii_lowercase();
    if a.ends_with("localservice") {
        Some("*S-1-5-19")
    } else if a.ends_with("networkservice") {
        Some("*S-1-5-20")
    } else {
        None
    }
}

fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase()
}

/// Folder (or file) names in the download folder that match a release the app
/// grabbed itself. Everything else in Downloads is left alone.
pub fn grabbed_folders(grabbed: &[String], names: &[String]) -> Vec<String> {
    let wanted: Vec<String> = grabbed.iter().map(|g| norm(g)).filter(|g| !g.is_empty()).collect();
    names
        .iter()
        .filter(|n| {
            let stem = std::path::Path::new(n.as_str()).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            wanted.contains(&norm(n)) || wanted.contains(&norm(&stem))
        })
        .cloned()
        .collect()
}

/// The script (for `procs::run_elevated`) that stops a service from starting
/// itself and stops it now, so Chillarr can run the app as the signed-in user
/// instead. A failed step stops the rest and the exit code says so.
pub fn run_as_user_command(service: &str) -> String {
    ps_chain(&[ps_tool("sc.exe", &["config", service, "start=", "demand"]), ps_tool("net.exe", &["stop", service])])
}

/// Lets the signed-in user write an app's data folder (Jellyfin's database was
/// created by its service account, so a copy run as the user couldn't save).
pub fn grant_user_line(path: &str, account: &str) -> String {
    ps_tool("icacls.exe", &[path, "/grant", &format!("{account}:(OI)(CI)M"), "/T", "/Q"])
}

/// Takes that grant away again. A data folder the user can write, loaded by a
/// service running as NETWORK SERVICE, would let anything the user runs put
/// code into that service.
pub fn remove_grant_line(path: &str, account: &str) -> String {
    ps_tool("icacls.exe", &[path, "/remove:g", account, "/T", "/Q"])
}

/// Jellyfin's switch: grant first (if the service is already stopped,
/// `net stop` fails and would skip it), then stop the service. If the service
/// is still running at the end, the grant comes off and its start type goes
/// back, so a half-done switch never leaves the service on a user-writable folder.
pub fn jellyfin_as_user_command(service: &str, path: &str, account: &str) -> String {
    let undo = format!("{}\n{}", remove_grant_line(path, account), ps_tool("sc.exe", &["config", service, "start=", "auto"]));
    format!(
        "{grant}\nif ($LASTEXITCODE) {{ exit $LASTEXITCODE }}\n{config}\nif ($LASTEXITCODE) {{ $rc = $LASTEXITCODE; {remove}; exit $rc }}\n{stop}\nif ((Get-Service -Name {name} -ErrorAction SilentlyContinue).Status -ne 'Stopped') {{\n{undo}\nexit 1\n}}\nexit 0",
        grant = grant_user_line(path, account),
        config = ps_tool("sc.exe", &["config", service, "start=", "demand"]),
        remove = remove_grant_line(path, account),
        stop = ps_tool("net.exe", &["stop", service]),
        name = ps_quote(service),
    )
}

/// Puts a service back the way it was when the switch didn't work out.
pub fn revert_command(service: &str) -> String {
    ps_chain(&[ps_tool("sc.exe", &["config", service, "start=", "auto"]), ps_tool("net.exe", &["start", service])])
}

/// Jellyfin's way back: the user's grant on its data folder comes off too.
/// Every step runs even if one fails (the grant must go either way).
pub fn revert_jellyfin_command(service: &str, path: &str, account: &str) -> String {
    ps_all(&[
        remove_grant_line(path, account),
        ps_tool("sc.exe", &["config", service, "start=", "auto"]),
        ps_tool("net.exe", &["start", service]),
    ])
}

/// What could be a finished download in `folder`: everything at the top, plus
/// one level inside each subfolder (qBittorrent category folders like `radarr`).
pub fn download_candidates(folder: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(top) = std::fs::read_dir(folder) else { return out };
    for e in top.flatten() {
        let p = e.path();
        if p.is_dir() {
            if let Ok(inner) = std::fs::read_dir(&p) {
                out.extend(inner.flatten().map(|x| x.path()));
            }
        }
        out.push(p);
    }
    out
}

/// The account a Windows service runs as, from the registry.
pub fn service_account(service: &str) -> Option<String> {
    let key = HSTRING::from(format!("SYSTEM\\CurrentControlSet\\Services\\{service}"));
    let name = HSTRING::from("ObjectName");
    let mut buf = vec![0u16; 256];
    let mut size = (buf.len() * 2) as u32;
    unsafe {
        RegGetValueW(HKEY_LOCAL_MACHINE, &key, &name, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut size))
            .ok()
            .ok()?;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_accounts_map_to_their_sid() {
        assert_eq!(service_sid("NT AUTHORITY\\LocalService"), Some("*S-1-5-19"));
        assert_eq!(service_sid("NT Authority\\NetworkService"), Some("*S-1-5-20"));
        // SYSTEM can already read everything; a user account is the user.
        assert_eq!(service_sid("LocalSystem"), None);
        assert_eq!(service_sid(".\\anime"), None);
    }

    #[test]
    #[ignore]
    fn live_radarr_runs_as_local_service() {
        assert_eq!(service_account("Radarr").as_deref().and_then(service_sid), Some("*S-1-5-19"));
    }

    #[test]
    fn run_as_user_command_stops_at_the_first_failure() {
        assert_eq!(
            run_as_user_command("Radarr"),
            "& \"$sys\\sc.exe\" 'config' 'Radarr' 'start=' 'demand'\nif ($LASTEXITCODE) { exit $LASTEXITCODE }\n& \"$sys\\net.exe\" 'stop' 'Radarr'\nif ($LASTEXITCODE) { exit $LASTEXITCODE }\nexit 0"
        );
    }

    #[test]
    fn data_grant_lets_the_user_write_the_app_data() {
        assert_eq!(
            grant_user_line(r"C:\ProgramData\Jellyfin\Server", r"MY-PC\anime"),
            r#"& "$sys\icacls.exe" 'C:\ProgramData\Jellyfin\Server' '/grant' 'MY-PC\anime:(OI)(CI)M' '/T' '/Q'"#
        );
        // A name with a quote can't end the string.
        assert!(grant_user_line(r"C:\x", "PC\\O'Neil").contains("'PC\\O''Neil:(OI)(CI)M'"));
    }

    #[test]
    fn revert_command_puts_the_service_back() {
        assert_eq!(
            revert_command("Radarr"),
            "& \"$sys\\sc.exe\" 'config' 'Radarr' 'start=' 'auto'\nif ($LASTEXITCODE) { exit $LASTEXITCODE }\n& \"$sys\\net.exe\" 'start' 'Radarr'\nif ($LASTEXITCODE) { exit $LASTEXITCODE }\nexit 0"
        );
    }

    #[test]
    fn jellyfin_revert_takes_the_grant_away() {
        let c = revert_jellyfin_command("JellyfinServer", r"C:\ProgramData\Jellyfin\Server", r"MY-PC\anime");
        let remove = r#"& "$sys\icacls.exe" 'C:\ProgramData\Jellyfin\Server' '/remove:g' 'MY-PC\anime' '/T' '/Q'"#;
        assert!(c.starts_with(&format!("$rc = 0\n{remove}\n")), "{c}");
        let (r, s, n) = (c.find("/remove:g").unwrap(), c.find("'start=' 'auto'").unwrap(), c.find("'start' 'JellyfinServer'").unwrap());
        assert!(r < s && s < n, "grant off, then the service back: {c}");
        assert!(!c.contains("{ exit $LASTEXITCODE }"), "a failed step doesn't skip the rest: {c}");
    }

    #[test]
    fn jellyfin_switch_undoes_the_grant_if_the_service_keeps_running() {
        let c = jellyfin_as_user_command("JellyfinServer", r"C:\ProgramData\Jellyfin\Server", r"MY-PC\anime");
        assert!(c.starts_with(r#"& "$sys\icacls.exe" 'C:\ProgramData\Jellyfin\Server' '/grant'"#), "{c}");
        assert_eq!(c.matches("/remove:g").count(), 2, "taken back when the config fails and when the stop fails: {c}");
        assert!(c.contains("(Get-Service -Name 'JellyfinServer' -ErrorAction SilentlyContinue).Status -ne 'Stopped'"), "{c}");
    }

    #[test]
    fn download_candidates_include_category_subfolders() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("Top.Release.2020")).unwrap();
        std::fs::create_dir_all(d.path().join("radarr").join("Nested.Release.2021")).unwrap();
        std::fs::write(d.path().join("radarr").join("Loose.File.2022.mkv"), b"").unwrap();
        let mut got: Vec<String> = download_candidates(d.path())
            .into_iter()
            .map(|p| p.strip_prefix(d.path()).unwrap().to_string_lossy().replace('\\', "/"))
            .collect();
        got.sort();
        assert_eq!(got, vec!["Top.Release.2020", "radarr", "radarr/Loose.File.2022.mkv", "radarr/Nested.Release.2021"]);
    }

    #[test]
    fn only_folders_the_app_grabbed_are_imported() {
        let grabbed = vec![
            "Replicas.2018.1080p.BluRay.REMUX.AVC.DTS HD.MA.5.1-FGT".to_string(),
            "The.Dark.Tower.2017.1080p.BluRay.H264.AAC-R4RBG[TGx]".to_string(),
            "Only The Brave (2017) 1080p Bluray x265 10bit DTS Omikron".to_string(),
        ];
        let folders = vec![
            "Replicas.2018.1080p.BluRay.REMUX.AVC.DTS-HD.MA.5.1-FGT".to_string(),
            "Only The Brave (2017) 1080p Bluray x265 10bit DTS Omikron".to_string(),
            "Roblox Account Manager 3.8".to_string(),
            "good luck 2".to_string(),
        ];
        assert_eq!(
            grabbed_folders(&grabbed, &folders),
            vec![
                "Replicas.2018.1080p.BluRay.REMUX.AVC.DTS-HD.MA.5.1-FGT".to_string(),
                "Only The Brave (2017) 1080p Bluray x265 10bit DTS Omikron".to_string(),
            ]
        );
    }
}
