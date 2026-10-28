//! Watch together: Jellyfin's SyncPlay. The player side (syncplay.js) holds
//! the WebSocket and keeps the video in step; this side makes its REST calls
//! as the profile watching, from the same Jellyfin session as that socket
//! (same token and device id), which SyncPlay needs to know who is who.
//!
//! Measured on Jellyfin 12.1: the socket signs in with `?ApiKey=` (the old
//! `api_key=` is refused), and messages only reach it when the REST calls use
//! the token's own device id.

use crate::media::{jf, Media};
use crate::tiles::{self, Tile};
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Manager};

#[derive(Serialize)]
pub struct Info {
    /// `ws://.../socket?ApiKey=...&deviceId=...`
    pub socket: String,
    pub user: String,
}

/// Where the player's SyncPlay socket connects, as whoever is watching.
pub fn info(app: &AppHandle) -> Result<Info, String> {
    let j = jf(app).filter(|j| j.user_id.is_some()).ok_or("Sign in to Jellyfin first (Settings > Jellyfin & streaming).")?;
    let (token, device) = j.credentials().ok_or("Sign in to Jellyfin first.")?;
    let ws = j.base.replacen("https://", "wss://", 1).replacen("http://", "ws://", 1);
    let media = app.state::<Media>();
    let profile = match crate::viewer::current() {
        Some(v) => Some(v.session.user),
        None => media.profile.lock().unwrap().as_ref().map(|p| p.user.clone()),
    };
    let user = profile.or_else(|| media.session.lock().unwrap().as_ref().map(|s| s.user.clone())).unwrap_or_default();
    Ok(Info { socket: format!("{ws}/socket?ApiKey={token}&deviceId={}", enc(&device)), user })
}

fn enc(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// Only SyncPlay's own endpoints and the server clock go through here. A
/// SyncPlay call is one plain word (`/SyncPlay/Join`): no dots, slashes,
/// `%`, `?`, `#` or `\`, so URL normalising can't turn it into another path
/// (`/SyncPlay/%2e%2e/Users/New` reached /Users/New).
fn allowed(path: &str) -> bool {
    path == "/GetUtcTime"
        || path == "/Sessions/Capabilities/Full"
        || path.strip_prefix("/SyncPlay/").is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// One SyncPlay REST call. Empty answers come back as null.
pub async fn call(app: &AppHandle, method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
    if !allowed(path) {
        return Err("Not a SyncPlay call.".into());
    }
    let j = jf(app).filter(|j| j.user_id.is_some()).ok_or("Sign in to Jellyfin first.")?;
    let r = match method {
        "GET" => j.get_json(path).await,
        _ => j.post_json(path, &body.unwrap_or(Value::Object(Default::default()))).await,
    };
    r.map_err(|e| e.to_string())
}

/// A playable tile for a Jellyfin item, for when the group starts something.
pub async fn tile(app: &AppHandle, id: &str) -> Option<Tile> {
    let j = jf(app).filter(|j| j.user_id.is_some())?;
    let v = j.items(&format!("ids={id}")).await.ok()?;
    v.first().map(|v| tiles::from_jf(v, &j.base))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_syncplay_paths_pass() {
        assert!(allowed("/SyncPlay/New") && allowed("/GetUtcTime") && allowed("/SyncPlay/List"));
        assert!(!allowed("/Users/New") && !allowed("/SyncPlay/../Users") && !allowed("/Items/1"));
        for bad in ["/SyncPlay/%2e%2e/Users/New", "/SyncPlay/New?x=1", "/SyncPlay/New#a", r"/SyncPlay/..\Users", r"/SyncPlay/New\x", "/SyncPlay/", "/SyncPlay/a/b", "/SyncPlay/.%2e"] {
            assert!(!allowed(bad), "{bad}");
        }
    }
}
