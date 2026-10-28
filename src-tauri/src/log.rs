//! Chillarr's own activity log: plain sentences for the Logs page, plus a
//! small rolling file. Never given an API key.

use crate::model::Level;
use serde::Serialize;
use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};

const KEEP: usize = 500;
const FILE_LIMIT: u64 = 1024 * 1024;

#[derive(Serialize, Clone, Debug)]
pub struct LogLine {
    pub at: String,
    pub text: String,
    pub level: Level,
}

#[derive(Default)]
pub struct Logs(pub Mutex<VecDeque<LogLine>>);

fn log_file() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Chillarr");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("chillarr.log"))
}

fn append_file(line: &LogLine) {
    let Some(path) = log_file() else { return };
    if std::fs::metadata(&path).map(|m| m.len() > FILE_LIMIT).unwrap_or(false) {
        let _ = std::fs::rename(&path, path.with_extension("log.old"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let day = chrono::Local::now().format("%Y-%m-%d");
        let _ = writeln!(f, "{day} {} {:?} {}", line.at, line.level, line.text);
    }
}

/// Hides API keys. Sonarr/Radarr put Prowlarr's feed URL, key included, in
/// their indexer test errors, and those errors are logged as-is. A quoted
/// value (`Token="..."`, JSON `"apiKey": "..."`) is hidden up to its closing quote.
pub fn redact(text: &str) -> String {
    const KEYS: [&str; 9] =
        ["apikey=", "api_key=", "x-api-key: ", "token=", "x-emby-token: ", "\"apikey\":", "\"api_key\":", "\"token\":", "\"accesstoken\":"];
    let lower = text.to_ascii_lowercase();
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        let hit = KEYS.iter().find(|k| lower[i..].starts_with(*k));
        if let Some(k) = hit {
            out.push_str(&text[i..i + k.len()]);
            i += k.len();
            // `"apiKey": "x"` and `Token= "x"`: keep the spacing, then the quote.
            while i < text.len() && bytes[i] == b' ' {
                out.push(' ');
                i += 1;
            }
            let quote = (i < text.len() && (bytes[i] == b'"' || bytes[i] == b'\'')).then(|| bytes[i]);
            if let Some(q) = quote {
                out.push(q as char);
                i += 1;
            }
            out.push_str("***");
            while i < text.len() {
                let c = bytes[i];
                let end = match quote {
                    Some(q) => c == q || c == b'\n',
                    None => matches!(c, b'&' | b' ' | b']' | b'"' | b'\'' | b'\n' | b'\r' | b',' | b'}' | b';'),
                };
                if end {
                    break;
                }
                i += 1;
            }
        } else {
            let ch = text[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

pub fn log(app: &AppHandle, level: Level, text: impl Into<String>) {
    let text = redact(&text.into());
    let line = LogLine { at: chrono::Local::now().format("%H:%M").to_string(), text, level };
    if let Some(logs) = app.try_state::<Logs>() {
        let mut q = logs.0.lock().unwrap();
        q.push_front(line.clone());
        q.truncate(KEEP);
    }
    #[cfg(debug_assertions)]
    println!("[log] {} {:?} {}", line.at, line.level, line.text);
    append_file(&line);
    let _ = app.emit("log", &line);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_keys_never_reach_the_log() {
        // The real failure text Sonarr/Radarr return for a Torznab indexer.
        let raw = "Unable to connect to indexer: http://localhost:9696/3/api?t=caps&apikey=deadbeef12345678abcdef0011223344 [404]";
        let clean = redact(raw);
        assert!(!clean.contains("deadbeef"), "{clean}");
        assert!(clean.contains("apikey=***"), "{clean}");
        assert_eq!(redact("X-Api-Key: abcdef0123456789abcdef0123456789"), "X-Api-Key: ***");
        assert_eq!(redact("api_key=abc&x=1"), "api_key=***&x=1");
        assert_eq!(redact("nothing secret here"), "nothing secret here");
    }

    #[test]
    fn quoted_tokens_and_json_keys_are_hidden() {
        // Jellyfin's MediaBrowser header.
        let h = r#"MediaBrowser Client="Chillarr", Device="PC", Token="0123456789abcdef0123456789abcdef""#;
        let clean = redact(h);
        assert!(!clean.contains("0123456789abcdef"), "{clean}");
        assert!(clean.ends_with(r#"Token="***""#), "{clean}");
        assert!(clean.contains(r#"Client="Chillarr""#), "{clean}");
        // An *arr item as JSON, with and without a space after the colon.
        let j = r#"{"name":"Sonarr","apiKey":"deadbeefdeadbeef","baseUrl":"http://localhost:8989"}"#;
        assert_eq!(redact(j), r#"{"name":"Sonarr","apiKey":"***","baseUrl":"http://localhost:8989"}"#);
        assert_eq!(redact(r#"{"apiKey": "abc123", "x": 1}"#), r#"{"apiKey": "***", "x": 1}"#);
        assert_eq!(redact(r#"{"token":"t0k3n"}"#), r#"{"token":"***"}"#);
        assert_eq!(redact("Token='abc def'"), "Token='***'");
        // An unquoted value still stops at the usual places.
        assert_eq!(redact("token=abc,next"), "token=***,next");
        assert_eq!(redact("X-Emby-Token: abc123"), "X-Emby-Token: ***");
    }
}
