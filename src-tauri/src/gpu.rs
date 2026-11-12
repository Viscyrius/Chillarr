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

/// The formats this card decodes itself. Everything else Jellyfin decodes on
/// the CPU: AV1 anime on an RTX 3060 Ti went through the CPU at about 40%
/// (2026-11-12) because only H.264, HEVC and VC-1 were listed.
pub fn decode_codecs(kind: &str, card: &str) -> Vec<&'static str> {
    let n = card.to_ascii_lowercase();
    let num = |after: &str| n.split(after).nth(1).and_then(|t| t.trim().split(|c: char| !c.is_ascii_digit()).next()).and_then(|d| d.parse::<u32>().ok());
    let mut out = vec!["h264", "hevc", "vc1"];
    let (vp9, av1) = match kind {
        // NVIDIA: VP9 from Pascal (GTX 10), AV1 from Ampere (RTX 30) and the workstation RTX A / Ada cards.
        "nvenc" => {
            let rtx = num("rtx ");
            let gtx = num("gtx ");
            let av1 = rtx.is_some_and(|m| (3000..10000).contains(&m) || (30..100).contains(&m)) || n.contains("rtx a") || n.contains(" ada");
            (av1 || rtx.is_some() || gtx.is_some_and(|m| m >= 1000), av1)
        }
        // Intel: VP9 from 7th gen, AV1 on Arc and Iris Xe (11th gen on).
        "qsv" => (true, n.contains("arc") || n.contains("iris(r) xe") || n.contains("iris xe")),
        // AMD: VP9 from the RX 5000s, AV1 from the RX 6000s.
        "amf" => {
            let rx = num("rx ");
            (rx.is_some_and(|m| m >= 5000), rx.is_some_and(|m| m >= 6000 && m < 10000))
        }
        _ => (false, false),
    };
    if vp9 {
        out.push("vp9");
    }
    if av1 {
        out.push("av1");
    }
    out
}

/// Jellyfin's encoding settings with the card turned on or off. Anything else in
/// them is left as it was.
fn with_card(mut cfg: Value, card: Option<(&str, &str)>) -> Value {
    match card {
        Some((kind, name)) => {
            cfg["HardwareAccelerationType"] = json!(kind);
            cfg["EnableHardwareEncoding"] = json!(true);
            cfg["HardwareDecodingCodecs"] = json!(decode_codecs(kind, name));
            cfg["EnableDecodingColorDepth10Hevc"] = json!(true);
            cfg["EnableDecodingColorDepth10Vp9"] = json!(true);
        }
        None => cfg["HardwareAccelerationType"] = json!("none"),
    }
    cfg
}

/// At start, with the card on: what it decodes is brought up to date (cards
/// switched on before AV1 was added kept decoding AV1 on the CPU).
pub async fn refresh(app: &AppHandle) {
    let Ok(j) = admin(app) else { return };
    let Ok(cfg) = j.get_json("/System/Configuration/encoding").await else { return };
    if !is_on(&cfg) {
        return;
    }
    let Some((kind, name)) = working_card().await else { return };
    let next = with_card(cfg.clone(), Some((kind, &name)));
    if next["HardwareDecodingCodecs"] != cfg["HardwareDecodingCodecs"] && j.post_json("/System/Configuration/encoding", &next).await.is_ok() {
        crate::log::detail(format!("Graphics card now also decodes: {}", next["HardwareDecodingCodecs"]));
    }
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
    let next = with_card(cfg, card.as_ref().map(|(kind, name)| (*kind, name.as_str())));
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
    fn each_card_decodes_what_it_can() {
        let nv = decode_codecs("nvenc", "NVIDIA GeForce RTX 3060 Ti");
        assert!(nv.contains(&"av1") && nv.contains(&"vp9") && nv.contains(&"hevc"), "Ampere decodes AV1 (it was left to the CPU, 40%)");
        let old = decode_codecs("nvenc", "NVIDIA GeForce GTX 1660 SUPER");
        assert!(old.contains(&"vp9") && !old.contains(&"av1"), "Turing GTX has no AV1 decoder");
        assert!(decode_codecs("nvenc", "NVIDIA GeForce RTX 4070").contains(&"av1"));
        assert!(!decode_codecs("nvenc", "NVIDIA GeForce RTX 2080").contains(&"av1"));
        assert!(decode_codecs("qsv", "Intel(R) Arc(TM) A770 Graphics").contains(&"av1"));
        assert!(decode_codecs("amf", "AMD Radeon RX 6700 XT").contains(&"av1"));
        assert!(!decode_codecs("amf", "AMD Radeon RX 580").contains(&"av1"));
    }

    #[test]
    fn only_the_card_settings_change() {
        let cfg = json!({ "HardwareAccelerationType": "none", "EnableThrottling": true, "DownMixAudioBoost": 2 });
        let on = with_card(cfg, Some(("nvenc", "NVIDIA GeForce RTX 3060 Ti")));
        assert!(on["HardwareDecodingCodecs"].as_array().unwrap().iter().any(|c| c == "av1"));
        assert!(is_on(&on));
        assert_eq!(on["EnableThrottling"], true);
        assert_eq!(on["DownMixAudioBoost"], 2);
        let off = with_card(on, None);
        assert!(!is_on(&off));
        assert_eq!(off["EnableThrottling"], true);
        assert!(!is_on(&json!({})));
    }
}
