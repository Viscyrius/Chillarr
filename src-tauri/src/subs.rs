//! Subtitle downloads through Jellyfin's own Open Subtitles plugin. Chillarr
//! installs it and passes on the user's free OpenSubtitles.com login, which
//! stays in Jellyfin's plugin settings; Chillarr keeps no copy.

use crate::jellyfin::{Jf, JfError};
use crate::log::log;
use crate::media::jf_admin;
use crate::model::Level;
use serde::Serialize;
use serde_json::json;
use std::time::Duration;
use tauri::AppHandle;

/// The plugin in Jellyfin's official catalog.
const PLUGIN_NAME: &str = "Open Subtitles";
const PLUGIN_GUID: &str = "4b9ed42f518548b598036ff2989014c4";
/// Built for Jellyfin 12.0 (targetAbi 12.0.0.0), the newest in the official
/// catalog on 2026-10-19.
const PLUGIN_VERSION: &str = "25.0.0.0";
/// Jellyfin's own catalog, as Jellyfin lists it by default. Named in the
/// install, so a plugin of the same GUID from any other source isn't taken.
const REPO_URL: &str = "https://repo.jellyfin.org/files/plugin/manifest.json";

/// The install path: this GUID, this version, from the official catalog only.
fn install_path() -> String {
    format!(
        "/Packages/Installed/{}?assemblyGuid={PLUGIN_GUID}&version={PLUGIN_VERSION}&repositoryUrl={}",
        PLUGIN_NAME.replace(' ', "%20"),
        REPO_URL.replace(':', "%3A").replace('/', "%2F")
    )
}

/// Puts the official catalog back in Jellyfin's sources if it was removed
/// (Jellyfin only installs from a listed source).
async fn ensure_official_repo(j: &Jf) -> Result<(), String> {
    let mut repos = j.get_json("/Repositories").await.map_err(err)?.as_array().cloned().unwrap_or_default();
    if repos.iter().any(|r| r["Url"] == REPO_URL) {
        return Ok(());
    }
    repos.push(json!({ "Name": "Jellyfin Stable", "Url": REPO_URL, "Enabled": true }));
    j.post_json("/Repositories", &serde_json::Value::Array(repos)).await.map(|_| ()).map_err(err)
}

#[derive(Serialize, Debug)]
pub struct Status {
    pub installed: bool,
    /// A login is saved and OpenSubtitles accepted it.
    pub ready: bool,
    pub user: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct Found {
    pub id: String,
    pub name: String,
    pub format: Option<String>,
    pub downloads: Option<i64>,
    pub rating: Option<f64>,
    pub hearing_impaired: bool,
    pub machine: bool,
}

fn signed_in(app: &AppHandle) -> Result<Jf, String> {
    jf_admin(app).filter(|j| j.user_id.is_some()).ok_or_else(|| "Sign in to Jellyfin first (Settings > Jellyfin & streaming)".to_string())
}

fn err(e: JfError) -> String {
    match e {
        JfError::Down => "Jellyfin isn't answering. Press Fix everything first.".into(),
        JfError::Rejected => "Jellyfin refused. Your Jellyfin account needs to be an administrator for this.".into(),
        JfError::Other(e) => e,
    }
}

/// The plugin's id once installed (and active).
async fn plugin_id(j: &Jf) -> Result<Option<String>, String> {
    let list = j.get_json("/Plugins").await.map_err(err)?;
    Ok(list.as_array().into_iter().flatten().find(|p| p["Name"] == PLUGIN_NAME).and_then(|p| p["Id"].as_str()).map(String::from))
}

pub async fn status(app: &AppHandle) -> Status {
    let Ok(j) = signed_in(app) else { return Status { installed: false, ready: false, user: None } };
    let Ok(Some(id)) = plugin_id(&j).await else { return Status { installed: false, ready: false, user: None } };
    let cfg = j.get_json(&format!("/Plugins/{id}/Configuration")).await.unwrap_or_default();
    let user = cfg["Username"].as_str().filter(|u| !u.is_empty()).map(String::from);
    let ready = user.is_some() && cfg["CredentialsInvalid"] != true;
    Status { installed: true, ready, user }
}

/// Installs the plugin when it's missing (Jellyfin restarts once for that),
/// checks the login with OpenSubtitles, and saves it in the plugin.
pub async fn setup(app: &AppHandle, username: &str, password: &str) -> Result<String, String> {
    let j = signed_in(app)?;
    let id = match plugin_id(&j).await? {
        Some(id) => id,
        None => {
            ensure_official_repo(&j).await?;
            j.post_json(&install_path(), &json!({})).await.map_err(err)?;
            log(app, Level::Ok, "Installed Jellyfin's Open Subtitles plugin. Restarting Jellyfin.");
            // The install finishes in the background; then a restart loads it.
            tokio::time::sleep(Duration::from_secs(4)).await;
            let _ = j.post_json("/System/Restart", &json!({})).await;
            let mut found = None;
            for _ in 0..60 {
                tokio::time::sleep(Duration::from_secs(2)).await;
                if let Ok(Some(id)) = plugin_id(&j).await {
                    found = Some(id);
                    break;
                }
            }
            found.ok_or("Jellyfin didn't come back with the Open Subtitles plugin. Restart Jellyfin and try again.")?
        }
    };
    let login = json!({ "Username": username, "Password": password, "CustomApiKey": "" });
    match j.post_json("/Jellyfin.Plugin.OpenSubtitles/ValidateLoginInfo", &login).await {
        Ok(_) => {}
        // A plugin version without the check: save it and let the first search tell.
        Err(JfError::Other(m)) if m.contains("404") => {}
        Err(JfError::Rejected) | Err(JfError::Other(_)) => return Err("OpenSubtitles didn't accept that name and password.".into()),
        Err(e) => return Err(err(e)),
    }
    let mut cfg = j.get_json(&format!("/Plugins/{id}/Configuration")).await.map_err(err)?;
    cfg["Username"] = json!(username);
    cfg["Password"] = json!(password);
    cfg["CredentialsInvalid"] = json!(false);
    j.post_json(&format!("/Plugins/{id}/Configuration"), &cfg).await.map_err(err)?;
    let msg = format!("Subtitle downloads are on, using OpenSubtitles as {username}.");
    log(app, Level::Ok, &msg);
    Ok(msg)
}

/// Subtitles OpenSubtitles has for this file, in a three-letter language (eng, spa...).
pub async fn search(app: &AppHandle, jf_id: &str, lang: &str) -> Result<Vec<Found>, String> {
    let j = signed_in(app)?;
    if !status(app).await.ready {
        return Err("setup".into());
    }
    let lang: String = lang.chars().filter(|c| c.is_ascii_alphabetic()).take(3).collect();
    let list = j.get_json(&format!("/Items/{jf_id}/RemoteSearch/Subtitles/{lang}?isPerfectMatch=false")).await.map_err(err)?;
    let mut out: Vec<Found> = list
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            Some(Found {
                id: s["Id"].as_str()?.to_string(),
                name: s["Name"].as_str().unwrap_or("Subtitles").to_string(),
                format: s["Format"].as_str().map(String::from),
                downloads: s["DownloadCount"].as_i64(),
                rating: s["CommunityRating"].as_f64(),
                hearing_impaired: s["IsHearingImpaired"] == true,
                machine: s["MachineTranslated"] == true || s["AiTranslated"] == true,
            })
        })
        .collect();
    // People's own uploads first, then the most downloaded.
    out.sort_by(|a, b| a.machine.cmp(&b.machine).then(b.downloads.unwrap_or(0).cmp(&a.downloads.unwrap_or(0))));
    Ok(out)
}

/// What `download` says when the free account's daily downloads are used up.
pub const LIMIT: &str = "OpenSubtitles wouldn't hand it over (the free account allows a few downloads a day).";

/// Downloads one result next to the video; it shows up as a track after a reload.
pub async fn download(app: &AppHandle, jf_id: &str, sub_id: &str) -> Result<(), String> {
    let j = signed_in(app)?;
    let enc: String = sub_id.bytes().map(|b| if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
    j.post_json(&format!("/Items/{jf_id}/RemoteSearch/Subtitles/{enc}"), &json!({})).await.map_err(|e| match e {
        JfError::Other(m) if m.contains("error 5") => LIMIT.to_string(),
        e => err(e),
    })?;
    // Jellyfin adds the file to the item in the background.
    tokio::time::sleep(Duration::from_secs(2)).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_names_the_official_catalog_and_a_version() {
        assert_eq!(
            install_path(),
            "/Packages/Installed/Open%20Subtitles?assemblyGuid=4b9ed42f518548b598036ff2989014c4&version=25.0.0.0&repositoryUrl=https%3A%2F%2Frepo.jellyfin.org%2Ffiles%2Fplugin%2Fmanifest.json"
        );
    }
}
