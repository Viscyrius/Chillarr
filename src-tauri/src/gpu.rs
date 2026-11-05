//! Use the graphics card to convert video. Jellyfin converts on the processor until
//! told otherwise, and a film that needs it (x265 for a TV, say) can then fall behind
//! while a game has the processor: stutter on the TV, and a slower game.
//!
//! For the PC's owner only, like limit.rs: not in bridge.rs's list. Jellyfin keeps
//! the setting itself, so there's nothing of ours to go stale. A card is only offered
//! after a short real encode has worked on it: Jellyfin has no fallback, so a wrong
//! setting would mean no picture on any device that needs converting.

use serde_json::{json, Value};
use std::os::windows::process::CommandExt;
use tauri::AppHandle;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// (what's in the card's name, Jellyfin's name for it, ffmpeg's encoder). Best first:
/// a PC with a graphics card also has Intel graphics, and the card is what to use.
const KINDS: [(&str, &str, &str); 3] = [("nvidia", "nvenc", "h264_nvenc"), ("amd|radeon", "amf", "h264_amf"), ("intel", "qsv", "h264_qsv")];

/// The kinds these card names can be, best first.
fn kinds_for(names: &[String]) -> Vec<(&'static str, &'static str, String)> {
    let mut out = Vec::new();
    for (words, kind, encoder) in KINDS {
        if let Some(n) = names.iter().find(|n| words.split('|').any(|w| n.to_ascii_lowercase().contains(w))) {
            out.push((kind, encoder, n.clone()));
        }
    }
    out
}

async fn card_names() -> Vec<String> {
    let run = tokio::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", "Get-CimInstance Win32_VideoController | ForEach-Object { $_.Name }"])
        .creation_flags(CREATE_NO_WINDOW)
        .kill_on_drop(true)
        .output();
    let Ok(Ok(out)) = tokio::time::timeout(std::time::Duration::from_secs(20), run).await else { return vec![] };
    String::from_utf8_lossy(&out.stdout).lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()
}

/// A second of real encoding, so a card or driver that can't is found out here.
async fn encoder_works(encoder: &str) -> bool {
    let run = tokio::process::Command::new(crate::subsync::ffmpeg())
        .args(["-nostdin", "-v", "error", "-f", "lavfi", "-i", "testsrc2=size=1280x720:rate=24:duration=1", "-pix_fmt", "yuv420p", "-c:v", encoder, "-f", "null", "-"])
        .creation_flags(CREATE_NO_WINDOW)
        .kill_on_drop(true)
        .output();
    matches!(tokio::time::timeout(std::time::Duration::from_secs(20), run).await, Ok(Ok(o)) if o.status.success())
}

/// The best card here that really encodes: (Jellyfin's name for it, the card's name).
async fn working_card() -> Option<(&'static str, String)> {
    for (kind, encoder, name) in kinds_for(&card_names().await) {
        if encoder_works(encoder).await {
            return Some((kind, name));
        }
    }
    None
}

fn admin(app: &AppHandle) -> Result<crate::jellyfin::Jf, String> {
    crate::media::jf_admin(app).filter(|j| j.user_id.is_some()).ok_or_else(|| "Sign in to Jellyfin first.".to_string())
}

/// Jellyfin's encoding settings with the card turned on or off. Anything else in
/// them is left as it was.
fn with_card(mut cfg: Value, kind: Option<&str>) -> Value {
    match kind {
        Some(kind) => {
            cfg["HardwareAccelerationType"] = json!(kind);
            cfg["EnableHardwareEncoding"] = json!(true);
            // x265 is what a TV needs converted most, 10-bit too.
            cfg["HardwareDecodingCodecs"] = json!(["h264", "hevc", "vc1"]);
            cfg["EnableDecodingColorDepth10Hevc"] = json!(true);
        }
        None => cfg["HardwareAccelerationType"] = json!("none"),
    }
    cfg
}

fn is_on(cfg: &Value) -> bool {
    cfg["HardwareAccelerationType"].as_str().is_some_and(|k| !k.is_empty() && k != "none")
}

/// What the window shows.
pub async fn status(app: &AppHandle) -> Result<Value, String> {
    let j = admin(app)?;
    let cfg = j.get_json("/System/Configuration/encoding").await.map_err(|e| e.to_string())?;
    let card = working_card().await;
    Ok(json!({ "on": is_on(&cfg), "card": card.map(|(_, name)| name) }))
}

/// Applies at once to anything that starts converting from now on.
pub async fn set(app: &AppHandle, on: bool) -> Result<Value, String> {
    let j = admin(app)?;
    let cfg = j.get_json("/System/Configuration/encoding").await.map_err(|e| e.to_string())?;
    let card = if on { Some(working_card().await.ok_or("Your graphics card can't convert video here, so nothing was changed.")?) } else { None };
    let next = with_card(cfg, card.as_ref().map(|(kind, _)| *kind));
    j.post_json("/System/Configuration/encoding", &next).await.map_err(|e| format!("Couldn't change that in Jellyfin: {e}"))?;
    crate::log::detail(format!("Graphics card for video: {}", if on { "on" } else { "off" }));
    status(app).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_real_card_comes_before_the_built_in_one() {
        let names = vec!["Parsec Virtual Display Adapter".to_string(), "Intel(R) UHD Graphics 770".to_string(), "NVIDIA GeForce RTX 3060 Ti".to_string()];
        let k = kinds_for(&names);
        assert_eq!(k.iter().map(|x| x.0).collect::<Vec<_>>(), vec!["nvenc", "qsv"]);
        assert_eq!(k[0].2, "NVIDIA GeForce RTX 3060 Ti");
        assert_eq!(kinds_for(&["AMD Radeon RX 6600".to_string()])[0].0, "amf");
        assert!(kinds_for(&["Parsec Virtual Display Adapter".to_string()]).is_empty(), "nothing that can encode");
    }

    #[test]
    fn only_the_card_settings_change() {
        let cfg = json!({ "HardwareAccelerationType": "none", "EnableThrottling": true, "DownMixAudioBoost": 2 });
        let on = with_card(cfg, Some("nvenc"));
        assert!(is_on(&on));
        assert_eq!(on["EnableThrottling"], true);
        assert_eq!(on["DownMixAudioBoost"], 2);
        let off = with_card(on, None);
        assert!(!is_on(&off));
        assert_eq!(off["EnableThrottling"], true);
        assert!(!is_on(&json!({})));
    }
}
