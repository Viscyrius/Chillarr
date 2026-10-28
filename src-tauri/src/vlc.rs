//! Playing in VLC instead of the built-in player: the original file at full
//! quality with every track. VLC is started with its web remote listening
//! only on this PC, and Chillarr reads the position from it every few seconds
//! so Jellyfin's Continue Watching stays right.

use std::path::PathBuf;

#[derive(Debug, PartialEq, Clone, Copy)]
pub struct Status {
    pub time: f64,
    pub length: f64,
    pub paused: bool,
}

/// VLC starts empty with its remote on; the stream address (which carries
/// the Jellyfin key) is then handed over through the remote, so it never sits
/// on the command line or in VLC's recent list.
pub fn vlc_args(title: &str, port: u16, password: &str) -> Vec<String> {
    vec![
        "--extraintf=http".to_string(),
        "--http-host=127.0.0.1".to_string(),
        format!("--http-port={port}"),
        format!("--http-password={password}"),
        format!("--meta-title={title}"),
        "--no-qt-recentplay".to_string(),
        "--play-and-exit".to_string(),
    ]
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

pub fn play_query(url: &str) -> String {
    format!("command=in_play&input={}", enc(url))
}

/// 128 random bits as hex, from the OS-seeded hasher keys.
pub fn new_password() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::new();
    for i in 0..2u8 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u8(i);
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

pub fn parse_status(json: &str) -> Option<Status> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    Some(Status {
        time: v["time"].as_f64()?,
        length: v["length"].as_f64().unwrap_or(0.0),
        paused: v["state"] == "paused",
    })
}

pub fn find_in(roots: &[PathBuf]) -> Option<PathBuf> {
    roots.iter().map(|r| r.join("VideoLAN").join("VLC").join("vlc.exe")).find(|p| p.exists())
}

pub fn find() -> Option<PathBuf> {
    let roots: Vec<PathBuf> = ["ProgramFiles", "ProgramFiles(x86)"].iter().filter_map(|v| std::env::var_os(v).map(PathBuf::from)).collect();
    find_in(&roots)
}

/// A free local port for VLC's remote.
pub fn free_port() -> Option<u16> {
    std::net::TcpListener::bind("127.0.0.1:0").ok()?.local_addr().ok().map(|a| a.port())
}

/// Asks VLC's local remote where playback is.
pub async fn status(http: &reqwest::Client, port: u16, password: &str) -> Option<Status> {
    command(http, port, password, "").await
}

/// Sends a remote command (empty for a plain status read).
pub async fn command(http: &reqwest::Client, port: u16, password: &str, query: &str) -> Option<Status> {
    let q = if query.is_empty() { String::new() } else { format!("?{query}") };
    let r = http
        .get(format!("http://127.0.0.1:{port}/requests/status.json{q}"))
        .basic_auth("", Some(password))
        .send()
        .await
        .ok()?;
    parse_status(&r.text().await.ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vlc_starts_without_the_stream_address() {
        let a = vlc_args("Replicas", 50123, "pw");
        assert!(a.contains(&"--extraintf=http".to_string()));
        assert!(a.contains(&"--http-host=127.0.0.1".to_string()), "the remote only listens on this PC");
        assert!(a.contains(&"--http-port=50123".to_string()));
        assert!(a.contains(&"--http-password=pw".to_string()));
        assert!(a.contains(&"--no-qt-recentplay".to_string()), "the stream address (with its key) is never saved to VLC's recent list");
        assert!(!a.iter().any(|x| x.contains("http://localhost")), "the address goes through the remote, not the command line");
    }

    #[test]
    fn play_command_encodes_the_address() {
        assert_eq!(
            play_query("http://localhost:8096/Videos/x/stream?static=true&api_key=k"),
            "command=in_play&input=http%3A%2F%2Flocalhost%3A8096%2FVideos%2Fx%2Fstream%3Fstatic%3Dtrue%26api_key%3Dk"
        );
    }

    #[test]
    fn remote_password_is_long_and_different_each_time() {
        let a = new_password();
        let b = new_password();
        assert!(a.len() >= 32 && a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn reads_position_from_vlc_status() {
        let s = r#"{"time": 812, "length": 6340, "state": "playing", "position": 0.128}"#;
        assert_eq!(parse_status(s), Some(Status { time: 812.0, length: 6340.0, paused: false }));
        let p = r#"{"time": 10, "length": 100, "state": "paused"}"#;
        assert_eq!(parse_status(p).unwrap().paused, true);
        assert_eq!(parse_status("not json"), None);
    }

    #[test]
    fn finds_vlc_in_program_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("VideoLAN").join("VLC");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("vlc.exe"), b"").unwrap();
        assert_eq!(find_in(&[tmp.path().to_path_buf()]), Some(dir.join("vlc.exe")));
        assert_eq!(find_in(&[]), None);
    }
}
