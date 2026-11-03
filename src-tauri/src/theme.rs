//! The look the user picked. The window works out the colours and draws the
//! logo in them (it already has the logo and a canvas); this keeps the choice,
//! and puts that logo on the window, the tray and the shortcuts. The .exe's own
//! icon can't change, so shortcuts get an IconLocation pointing at an .ico.

use base64::Engine;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Theme {
    /// A preset's id, or "custom".
    pub preset: String,
    /// "#rrggbb".
    pub accent: String,
    /// "tinted" or "black".
    pub surface: String,
}

impl Default for Theme {
    fn default() -> Self {
        Theme { preset: "ember".into(), accent: "#ff6a3d".into(), surface: "tinted".into() }
    }
}

pub fn valid_colour(c: &str) -> bool {
    c.len() == 7 && c.starts_with('#') && c[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// A name is only ever a word: it goes in a settings file and a web page.
pub fn clean(t: Theme) -> Result<Theme, String> {
    if !valid_colour(&t.accent) {
        return Err("That isn't a colour".into());
    }
    if t.preset.is_empty() || t.preset.len() > 20 || !t.preset.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err("Unknown theme".into());
    }
    let surface = if t.surface == "black" { "black" } else { "tinted" }.to_string();
    Ok(Theme { surface, ..t })
}

const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// An .ico holding these PNGs (Windows reads PNG entries since Vista). Each is
/// `(size in pixels, png bytes)`.
pub fn ico_from_pngs(pngs: &[(u32, Vec<u8>)]) -> Result<Vec<u8>, String> {
    if pngs.is_empty() || pngs.len() > 8 {
        return Err("Wrong number of icon sizes".into());
    }
    let mut out = vec![0u8, 0, 1, 0];
    out.extend_from_slice(&(pngs.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * pngs.len() as u32;
    for (size, png) in pngs {
        if !(1..=256).contains(size) || !png.starts_with(&PNG_MAGIC) || png.len() > 2_000_000 {
            return Err("That icon isn't usable".into());
        }
        // 0 means 256 in this format.
        let side = if *size == 256 { 0u8 } else { *size as u8 };
        out.extend_from_slice(&[side, side, 0, 0, 1, 0, 32, 0]);
        out.extend_from_slice(&(png.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for (_, png) in pngs {
        out.extend_from_slice(png);
    }
    Ok(out)
}

fn data_dir(app: &AppHandle) -> Option<PathBuf> {
    app.state::<crate::SettingsState>().path.parent().map(Path::to_path_buf)
}

/// A new name per colour: Explorer caches icons by path, and would keep drawing
/// the old colour from the same file name.
fn ico_name(accent: &str) -> String {
    format!("chillarr-icon-{}.ico", accent.trim_start_matches('#').to_ascii_lowercase())
}

fn drop_other_icons(dir: &Path, keep: &str) {
    let Ok(list) = std::fs::read_dir(dir) else { return };
    for e in list.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with("chillarr-icon-") && name.ends_with(".ico") && name != keep {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Points every Chillarr shortcut of this user at `icon`, and only shortcuts
/// that really start this copy of Chillarr. Pinned-to-taskbar ones count.
fn repoint_shortcuts(icon: &str, exe: &Path) -> Result<(), String> {
    use crate::procs::ps_quote;
    use std::os::windows::process::CommandExt;
    let exe_s = exe.to_string_lossy();
    let script = format!(
        concat!(
            "$exe = {exe}\n$icon = {icon}\n$s = New-Object -ComObject WScript.Shell\n",
            "$dirs = @([Environment]::GetFolderPath('Desktop'), [Environment]::GetFolderPath('Programs'), ",
            "(Join-Path $env:APPDATA 'Microsoft\\Internet Explorer\\Quick Launch\\User Pinned\\TaskBar'))\n",
            "foreach ($d in $dirs) {{ Get-ChildItem -LiteralPath $d -Filter *.lnk -Recurse -ErrorAction SilentlyContinue | ForEach-Object {{\n",
            " $l = $s.CreateShortcut($_.FullName)\n",
            " if ($l.TargetPath -ieq $exe) {{ $l.IconLocation = $icon; $l.Save() }}\n",
            "}} }}"
        ),
        exe = ps_quote(&exe_s),
        icon = ps_quote(icon)
    );
    use base64::Engine as _;
    let utf16: Vec<u8> = script.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let status = std::process::Command::new(crate::procs::powershell())
        .args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &base64::engine::general_purpose::STANDARD.encode(utf16)])
        .creation_flags(0x0800_0000)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| e.to_string())?;
    unsafe {
        use windows::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};
        SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
    }
    status.success().then_some(()).ok_or_else(|| "Couldn't update the shortcuts".into())
}

/// The shortcut pass costs a PowerShell start, so it runs when the colour or the
/// installed exe changed (a reinstall writes fresh shortcuts), not on every launch.
fn stamp_for(accent: &str, exe: &Path) -> String {
    let when = std::fs::metadata(exe).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    format!("{accent}|{when}")
}

/// Saves the choice.
pub fn save(app: &AppHandle, theme: Theme) -> Result<(), String> {
    let theme = clean(theme)?;
    let st = app.state::<crate::SettingsState>();
    let saved = {
        let mut s = st.current.lock().unwrap();
        s.theme = theme;
        s.clone()
    };
    crate::settings::save(&st.path, &saved)
}

fn window_icon(app: &AppHandle, rgba: Vec<u8>, side: u32) -> Result<(), String> {
    if side == 0 || side > 256 || rgba.len() != (side * side * 4) as usize {
        return Err("That icon isn't usable".into());
    }
    let img = tauri::image::Image::new_owned(rgba, side, side);
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_icon(img.clone());
    }
    if let Some(t) = app.tray_by_id("main") {
        let _ = t.set_icon(Some(img));
    }
    Ok(())
}

/// The logo in the theme's colours, drawn by the window. `pngs` go in the .ico,
/// `rgba` is one square for the window and the tray. No pictures: the default
/// logo, so shortcuts go back to the .exe's own icon.
pub async fn apply_icon(app: &AppHandle, accent: String, pngs: Vec<(u32, String)>, rgba: Option<String>, side: u32) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = data_dir(app).ok_or("No settings folder")?;
    let decode = |s: &str| base64::engine::general_purpose::STANDARD.decode(s).map_err(|_| "That icon isn't usable".to_string());
    if let Some(r) = rgba {
        window_icon(app, decode(&r)?, side)?;
    } else if let Some(icon) = app.default_window_icon().cloned() {
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.set_icon(icon.clone());
        }
        if let Some(t) = app.tray_by_id("main") {
            let _ = t.set_icon(Some(icon));
        }
    }
    let target = if pngs.is_empty() {
        drop_other_icons(&dir, "");
        format!("{},0", exe.to_string_lossy())
    } else {
        let name = ico_name(&accent);
        let path = dir.join(&name);
        if !path.exists() {
            let decoded = pngs.iter().map(|(s, b)| Ok((*s, decode(b)?))).collect::<Result<Vec<_>, String>>()?;
            let bytes = ico_from_pngs(&decoded)?;
            let tmp = path.with_extension("tmp");
            std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
            std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
        }
        drop_other_icons(&dir, &name);
        format!("{},0", path.to_string_lossy())
    };
    let marker = dir.join("theme-shortcuts.txt");
    let stamp = stamp_for(&accent, &exe);
    if std::fs::read_to_string(&marker).ok().as_deref() == Some(stamp.as_str()) {
        return Ok(());
    }
    tokio::task::spawn_blocking(move || repoint_shortcuts(&target, &exe)).await.map_err(|e| e.to_string())??;
    let _ = std::fs::write(&marker, stamp);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(extra: &[u8]) -> Vec<u8> {
        let mut v = PNG_MAGIC.to_vec();
        v.extend_from_slice(extra);
        v
    }

    #[test]
    fn builds_an_ico_windows_can_read() {
        let (a, b) = (png(b"aa"), png(b"bbbb"));
        let ico = ico_from_pngs(&[(32, a.clone()), (256, b.clone())]).unwrap();
        assert_eq!(&ico[..6], &[0, 0, 1, 0, 2, 0]);
        assert_eq!(ico[6], 32, "width");
        assert_eq!(ico[22], 0, "256 is written as 0");
        let first = u32::from_le_bytes(ico[18..22].try_into().unwrap());
        assert_eq!(first, 6 + 32);
        let second = u32::from_le_bytes(ico[34..38].try_into().unwrap());
        assert_eq!(second as usize, 6 + 32 + a.len());
        assert_eq!(&ico[first as usize..first as usize + a.len()], &a[..]);
        assert_eq!(&ico[second as usize..], &b[..]);
    }

    #[test]
    fn refuses_what_isnt_a_small_png() {
        assert!(ico_from_pngs(&[]).is_err());
        assert!(ico_from_pngs(&[(32, b"not a png".to_vec())]).is_err());
        assert!(ico_from_pngs(&[(512, png(b"x"))]).is_err());
    }

    #[test]
    fn colours_and_names_are_checked() {
        assert!(valid_colour("#ff6a3d"));
        for bad in ["ff6a3d", "#ff6a3", "#gggggg", "#ff6a3d;x", ""] {
            assert!(!valid_colour(bad), "{bad}");
        }
        let t = |p: &str, a: &str| clean(Theme { preset: p.into(), accent: a.into(), surface: "x".into() });
        assert_eq!(t("tide", "#3fb8ff").unwrap().surface, "tinted");
        assert!(t("a b", "#3fb8ff").is_err());
        assert!(t("tide", "red").is_err());
    }

    #[test]
    fn each_colour_gets_its_own_file_name() {
        assert_eq!(ico_name("#3FB8FF"), "chillarr-icon-3fb8ff.ico");
    }
}
