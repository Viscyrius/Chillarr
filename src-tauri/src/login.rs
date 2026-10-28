//! One login for every app. The name and password typed once in setup (or in
//! Settings) also become qBittorrent's Web UI login and Sonarr, Radarr and
//! Prowlarr's sign-in page, so none of them stops at its own "set a password"
//! screen. Jellyfin gets the same account from the setup step itself.
//!
//! Chillarr itself never needs the password again: it talks to the *arr apps
//! with their API keys and to qBittorrent as localhost. The *arr sign-in is
//! asked of every browser, this PC's too: "disabledForLocalAddresses" in those
//! apps means every private address, so anyone on the Wi-Fi would get their
//! settings (and Connect > Custom Script runs programs) with no password. An app that isn't answering yet keeps it
//! in memory until the next Fix everything gives it the login; it is never
//! written to disk.

use crate::arr::Arr;
use crate::fix::Core;
use crate::log::log;
use crate::model::{AppId, Level};
use crate::procs;
use serde::Serialize;
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

const ARR_APPS: [AppId; 3] = [AppId::Sonarr, AppId::Radarr, AppId::Prowlarr];

/// The login still owed to apps that weren't answering, for this session only.
#[derive(Default)]
pub struct Pending(Mutex<Option<(String, String, Vec<AppId>)>>);

#[derive(Serialize, Clone)]
pub struct Outcome {
    pub app: String,
    /// "set", "later" (not answering, retried by Fix everything) or "skipped" (not installed).
    pub result: String,
    pub detail: Option<String>,
}

/// Gives every installed app this login. Apps that aren't up yet get it on
/// the next Fix everything.
pub async fn apply(app: &AppHandle, user: &str, password: &str) -> Vec<Outcome> {
    let mut out = Vec::new();
    let mut later = Vec::new();
    for id in [AppId::QBittorrent, AppId::Sonarr, AppId::Radarr, AppId::Prowlarr] {
        let o = apply_one(app, id, user, password).await;
        if o.result == "later" {
            later.push(id);
        }
        out.push(o);
    }
    *app.state::<Pending>().0.lock().unwrap() = (!later.is_empty()).then(|| (user.to_string(), password.to_string(), later));
    out
}

/// Fix everything's turn: hands the login to the apps that missed it.
pub async fn apply_pending(app: &AppHandle) {
    let Some((user, password, ids)) = app.state::<Pending>().0.lock().unwrap().clone() else { return };
    let mut still = Vec::new();
    for id in ids {
        if apply_one(app, id, &user, &password).await.result == "later" {
            still.push(id);
        }
    }
    *app.state::<Pending>().0.lock().unwrap() = (!still.is_empty()).then_some((user, password, still));
}

async fn apply_one(app: &AppHandle, id: AppId, user: &str, password: &str) -> Outcome {
    let name = id.label().to_string();
    let outcome = |result: &str, detail: Option<String>| Outcome { app: name.clone(), result: result.into(), detail };
    let Some(cfg) = app.state::<Core>().config(id) else { return outcome("skipped", None) };
    let r = if id == AppId::QBittorrent {
        if cfg.exe.is_none() {
            return outcome("skipped", None);
        }
        qbit(&cfg, user, password).await
    } else if ARR_APPS.contains(&id) {
        let Some(arr) = Arr::new(&cfg) else { return outcome("skipped", None) };
        if cfg.base_url.is_empty() || !procs::port_open(&cfg.base_url).await {
            return outcome("later", Some(format!("{name} isn't running yet. It gets the login when Fix everything starts it.")));
        }
        arr_login(&arr, user, password).await
    } else {
        return outcome("skipped", None);
    };
    match r {
        Ok(()) => {
            log(app, Level::Ok, format!("{name} now signs in as {user}."));
            outcome("set", None)
        }
        Err(e) => {
            log(app, Level::Red, format!("Couldn't give {name} the login: {e}"));
            outcome("later", Some(e))
        }
    }
}

/// The *arr sign-in page: forms login, asked of everyone (see the top of this file).
async fn arr_login(arr: &Arr, user: &str, password: &str) -> Result<(), String> {
    let mut host = arr.get_one("config/host").await.map_err(|e| format!("{e:?}"))?;
    host["authenticationMethod"] = "forms".into();
    host["authenticationRequired"] = "enabled".into();
    host["username"] = user.into();
    host["password"] = password.into();
    host["passwordConfirmation"] = password.into();
    arr.update("config/host", &host).await.map_err(|e| format!("{e:?}"))
}

/// Whether this `config/host` skips the sign-in for the home network while a
/// real login (forms or basic, with a name) is set up, so asking for it
/// everywhere can't lock anyone out. Earlier Chillarr versions set it up that way.
fn local_bypass_with_login(host: &serde_json::Value) -> bool {
    let method = host["authenticationMethod"].as_str().unwrap_or_default().to_ascii_lowercase();
    host["authenticationRequired"].as_str().is_some_and(|r| r.eq_ignore_ascii_case("disabledForLocalAddresses"))
        && (method == "forms" || method == "basic")
        && host["username"].as_str().is_some_and(|u| !u.trim().is_empty())
}

/// Apps already checked this session (nothing to change, changed, or the
/// change was refused and logged once), so the minute-by-minute runs skip them.
static CHECKED: Mutex<Vec<AppId>> = Mutex::new(Vec::new());

/// Every Fix everything (any trigger): an *arr app that has a login but skips
/// it for the home network asks for it everywhere from now on. One API call
/// with its key, no admin. The password isn't touched: the app's own settings
/// go back as read, with only that one field changed. An app with no login
/// set up is left alone.
pub async fn require_login_everywhere(app: &AppHandle) {
    for id in ARR_APPS {
        if CHECKED.lock().unwrap().contains(&id) {
            continue;
        }
        let Some(cfg) = app.state::<Core>().config(id) else { continue };
        let Some(arr) = Arr::new(&cfg) else { continue };
        if cfg.base_url.is_empty() || !procs::port_open(&cfg.base_url).await {
            continue;
        }
        let Ok(mut host) = arr.get_one("config/host").await else { continue };
        if !local_bypass_with_login(&host) {
            // Already asked everywhere, or no login to ask for. A login set up
            // later through Chillarr is asked everywhere from the start.
            CHECKED.lock().unwrap().push(id);
            continue;
        }
        host["authenticationRequired"] = "enabled".into();
        let name = id.label();
        match arr.update("config/host", &host).await {
            Ok(()) => {
                CHECKED.lock().unwrap().push(id);
                log(app, Level::Ok, format!("{name} now asks for your login on every device, this PC too, so no one else on your Wi-Fi can open it."));
            }
            Err(e) => {
                CHECKED.lock().unwrap().push(id);
                log(app, Level::Red, format!("Couldn't make {name} ask for your login on every device: {e:?}. Set Authentication Required to Enabled in {name} > Settings > General."));
            }
        }
    }
}

/// qBittorrent reads its login at start and rewrites its settings file on
/// exit, so an open one is closed, changed and started again.
async fn qbit(cfg: &crate::model::AppConfig, user: &str, password: &str) -> Result<(), String> {
    let dir = std::env::var_os("APPDATA").map(|p| std::path::PathBuf::from(p).join("qBittorrent")).ok_or("No AppData folder")?;
    let path = dir.join("qBittorrent.ini");
    let running = procs::process_running("qbittorrent.exe");
    if running {
        tokio::task::spawn_blocking(|| procs::force_close("qbittorrent.exe")).await.ok();
    }
    let ini = match std::fs::read(&path) {
        Ok(bytes) => String::from_utf8(bytes).map_err(|_| "its settings file isn't plain text, so Chillarr left it alone".to_string())?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("couldn't read its settings ({e})")),
    };
    let new = crate::wire::set_qbit_login(&ini, user, password).ok_or("couldn't make a password hash")?;
    let written = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, new)).map_err(|e| format!("couldn't save its settings ({e})"));
    if running {
        if let Some(exe) = &cfg.exe {
            let _ = procs::start_qbit(exe);
        }
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_a_real_login_is_asked_for_everywhere() {
        let host = |method: &str, required: &str, user: &str| {
            json!({"authenticationMethod": method, "authenticationRequired": required, "username": user, "password": ""})
        };
        assert!(local_bypass_with_login(&host("forms", "disabledForLocalAddresses", "anime")));
        assert!(local_bypass_with_login(&host("basic", "disabledForLocalAddresses", "anime")));
        assert!(!local_bypass_with_login(&host("forms", "enabled", "anime")), "already asked everywhere");
        assert!(!local_bypass_with_login(&host("none", "disabledForLocalAddresses", "anime")), "no login: left alone");
        assert!(!local_bypass_with_login(&host("external", "disabledForLocalAddresses", "anime")), "someone else's sign-in: left alone");
        assert!(!local_bypass_with_login(&host("forms", "disabledForLocalAddresses", " ")), "no name: it would lock you out");
        assert!(!local_bypass_with_login(&json!({"authenticationMethod": "forms", "authenticationRequired": "disabledForLocalAddresses"})));
    }
}
