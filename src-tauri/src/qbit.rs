//! qBittorrent's Web API: the downloads list, seeding, and streaming a
//! Discover shelf download to the phone.
//! Chillarr talks to it as a program on this PC ("bypass authentication for
//! localhost"), which qBittorrent offers as a setting.

use serde_json::Value;
use std::time::Duration;

pub(crate) const VIDEO: [&str; 6] = [".mkv", ".mp4", ".avi", ".m4v", ".mov", ".ts"];

/// The file to play: the biggest video for a movie, the first episode by name
/// for a season pack. Samples and extras lose either way.
pub fn pick_video(files: &[Value], series: bool) -> Option<Value> {
    let mut vids: Vec<&Value> = files
        .iter()
        .filter(|f| {
            let n = f["name"].as_str().unwrap_or_default().to_ascii_lowercase();
            VIDEO.iter().any(|e| n.ends_with(e)) && !n.contains("sample")
        })
        .collect();
    if vids.is_empty() {
        return None;
    }
    if series {
        vids.sort_by_key(|f| f["name"].as_str().unwrap_or_default().to_ascii_lowercase());
        // Drop tiny extras before taking the first episode.
        let biggest = vids.iter().map(|f| f["size"].as_i64().unwrap_or(0)).max().unwrap_or(0);
        vids.retain(|f| f["size"].as_i64().unwrap_or(0) * 4 >= biggest);
        vids.first().map(|f| (*f).clone())
    } else {
        vids.into_iter().max_by_key(|f| f["size"].as_i64().unwrap_or(0)).cloned()
    }
}

/// Every (season, episode) a file name claims: `S02E03` (and `S02E03E04`),
/// `S02 E03`, and `2x03`. Resolutions like 1920x1080 and years don't count.
pub fn episode_numbers(name: &str) -> Vec<(i64, i64)> {
    let b = name.to_ascii_lowercase().into_bytes();
    let digits = |from: usize| b[from..].iter().take_while(|c| c.is_ascii_digit()).count();
    let num = |from: usize, n: usize| std::str::from_utf8(&b[from..from + n]).ok().and_then(|t| t.parse::<i64>().ok()).unwrap_or(0);
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        // A token starts at the beginning or after a non-alphanumeric.
        if i > 0 && b[i - 1].is_ascii_alphanumeric() {
            i += 1;
            continue;
        }
        if b[i] == b's' {
            let sd = digits(i + 1);
            if (1..=4).contains(&sd) {
                let season = num(i + 1, sd);
                let mut j = i + 1 + sd;
                while j < b.len() && b[j] == b' ' {
                    j += 1;
                }
                let mut found = false;
                while j < b.len() && b[j] == b'e' {
                    let ed = digits(j + 1);
                    if !(1..=4).contains(&ed) {
                        break;
                    }
                    let first = num(j + 1, ed);
                    out.push((season, first));
                    found = true;
                    j += 1 + ed;
                    // A range: S02E03-E04 or S02E03-04 (a bare number is two digits, so "-720p" isn't one).
                    if b.get(j) == Some(&b'-') {
                        let has_e = b.get(j + 1) == Some(&b'e');
                        let k = j + 1 + usize::from(has_e);
                        let rd = digits(k);
                        if (1..=if has_e { 4 } else { 2 }).contains(&rd) {
                            let last = num(k, rd);
                            if last > first && last - first < 100 {
                                out.extend((first + 1..=last).map(|e| (season, e)));
                                j = k + rd;
                            }
                        }
                    }
                }
                if found {
                    i = j;
                    continue;
                }
            }
        } else if b[i].is_ascii_digit() {
            let sd = digits(i);
            let x = i + sd;
            if sd <= 2 && x < b.len() && b[x] == b'x' {
                let ed = digits(x + 1);
                let end = x + 1 + ed;
                if (2..=3).contains(&ed) && !b.get(end).is_some_and(|c| c.is_ascii_alphanumeric()) {
                    out.push((num(i, sd), num(x + 1, ed)));
                    i = end;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

/// The video file that is this episode, out of a season pack. Samples lose;
/// if several files match, the biggest wins.
pub fn pick_episode(files: &[Value], season: i64, episode: i64) -> Option<Value> {
    files
        .iter()
        .filter(|f| {
            let n = f["name"].as_str().unwrap_or_default().to_ascii_lowercase();
            // Names carry the pack folder; only the file's own name says which episode it is.
            let base = n.rsplit(['/', '\\']).next().unwrap_or_default();
            VIDEO.iter().any(|e| n.ends_with(e)) && !n.contains("sample") && episode_numbers(base).contains(&(season, episode))
        })
        .max_by_key(|f| f["size"].as_i64().unwrap_or(0))
        .cloned()
}

/// Where the file may be on disk: the incomplete folder (if set) while it
/// downloads, with or without qBittorrent's ".!qB" suffix, else the save folder.
pub fn candidate_paths(save: &str, download: &str, name: &str, complete: bool) -> Vec<std::path::PathBuf> {
    let mut bases = Vec::new();
    if !complete && !download.is_empty() {
        bases.push(download);
    }
    bases.push(save);
    let mut out = Vec::new();
    for b in bases {
        let p = std::path::Path::new(b).join(name);
        out.push(p.clone());
        let mut q = p.into_os_string();
        q.push(".!qB");
        out.push(q.into());
    }
    out
}

#[derive(Debug, PartialEq)]
pub enum Pick {
    Hash(String),
    /// Radarr/Sonarr are holding the release back (delay profile).
    Delayed,
    /// The release went to a usenet client: there's no torrent to stream.
    NotTorrent,
    Nothing,
}

/// The torrent behind a title in Radarr's/Sonarr's queue.
pub fn torrent_hash(records: &[Value], field: &str, id: i64) -> Pick {
    let mine: Vec<&Value> = records.iter().filter(|r| r[field].as_i64() == Some(id)).collect();
    if let Some(h) = mine
        .iter()
        .filter(|r| r["protocol"].as_str().is_none_or(|p| p == "torrent"))
        .find_map(|r| r["downloadId"].as_str())
    {
        return Pick::Hash(h.to_ascii_lowercase());
    }
    if mine.iter().any(|r| r["status"] == "delay") {
        Pick::Delayed
    } else if mine.iter().any(|r| r["protocol"] == "usenet") {
        Pick::NotTorrent
    } else {
        Pick::Nothing
    }
}

pub fn speed(bytes_per_sec: i64) -> String {
    if bytes_per_sec <= 0 {
        "waiting for peers".into()
    } else if bytes_per_sec < 1_000_000 {
        format!("{} KB/s", bytes_per_sec / 1024)
    } else {
        format!("{:.1} MB/s", bytes_per_sec as f64 / 1024.0 / 1024.0)
    }
}

#[derive(Debug, PartialEq)]
pub enum QbError {
    Down,
    /// Needs the "skip the password for apps on this PC" setting.
    Locked,
    Other(String),
}

pub struct Qbit {
    http: reqwest::Client,
    base: String,
}

impl Qbit {
    pub fn new(base: &str) -> Qbit {
        Qbit {
            http: reqwest::Client::builder().connect_timeout(Duration::from_secs(2)).timeout(Duration::from_secs(10)).build().unwrap_or_default(),
            base: base.trim_end_matches('/').to_string(),
        }
    }

    async fn call(&self, path: &str, form: Option<&[(&str, &str)]>) -> Result<String, QbError> {
        let url = format!("{}/api/v2/{path}", self.base);
        // qBittorrent checks Referer/Origin against its own address.
        let req = match form {
            Some(f) => self.http.post(&url).form(f),
            None => self.http.get(&url),
        }
        .header("Referer", &self.base);
        let r = req.send().await.map_err(|e| if e.is_connect() || e.is_timeout() { QbError::Down } else { QbError::Other(e.to_string()) })?;
        match r.status().as_u16() {
            200..=299 => Ok(r.text().await.unwrap_or_default()),
            401 | 403 => Err(QbError::Locked),
            c => Err(QbError::Other(format!("qBittorrent answered with error {c}"))),
        }
    }

    pub async fn torrent(&self, hash: &str) -> Result<Option<Value>, QbError> {
        let t = self.call(&format!("torrents/info?hashes={hash}"), None).await?;
        Ok(serde_json::from_str::<Vec<Value>>(&t).ok().and_then(|v| v.into_iter().next()))
    }

    pub async fn files(&self, hash: &str) -> Result<Vec<Value>, QbError> {
        let t = self.call(&format!("torrents/files?hash={hash}"), None).await?;
        Ok(serde_json::from_str(&t).unwrap_or_default())
    }

    pub async fn piece_states(&self, hash: &str) -> Result<Vec<u8>, QbError> {
        let t = self.call(&format!("torrents/pieceStates?hash={hash}"), None).await?;
        Ok(serde_json::from_str(&t).unwrap_or_default())
    }

    pub async fn piece_size(&self, hash: &str) -> Result<i64, QbError> {
        let t = self.call(&format!("torrents/properties?hash={hash}"), None).await?;
        Ok(serde_json::from_str::<Value>(&t).ok().and_then(|v| v["piece_size"].as_i64()).unwrap_or(0))
    }

    pub async fn prefs(&self) -> Result<Value, QbError> {
        let t = self.call("app/preferences", None).await?;
        serde_json::from_str(&t).map_err(|e| QbError::Other(e.to_string()))
    }

    pub async fn set_prefs(&self, prefs: &Value) -> Result<(), QbError> {
        let json = prefs.to_string();
        self.call("app/setPreferences", Some(&[("json", json.as_str())])).await.map(|_| ())
    }

    /// Every torrent qBittorrent has.
    pub async fn list(&self) -> Result<Vec<Value>, QbError> {
        let t = self.call("torrents/info", None).await?;
        Ok(serde_json::from_str(&t).unwrap_or_default())
    }

    /// Pauses a torrent. qBittorrent 5 calls it stop, 4.x calls it pause.
    pub async fn stop(&self, hash: &str) -> Result<(), QbError> {
        self.stop_or_start("stop", "pause", hash).await
    }

    /// Resumes a torrent. qBittorrent 5 calls it start, 4.x calls it resume.
    pub async fn start(&self, hash: &str) -> Result<(), QbError> {
        self.stop_or_start("start", "resume", hash).await
    }

    /// Tries the 5.x endpoint, and the 4.x one when this qBittorrent has no such route (404).
    async fn stop_or_start(&self, new: &str, old: &str, hash: &str) -> Result<(), QbError> {
        match self.call(&format!("torrents/{new}"), Some(&[("hashes", hash)])).await {
            Err(QbError::Other(m)) if m.ends_with("error 404") => self.call(&format!("torrents/{old}"), Some(&[("hashes", hash)])).await.map(|_| ()),
            r => r.map(|_| ()),
        }
    }

    /// Removes torrents, and what they downloaded when `files` is set.
    pub async fn delete(&self, hashes: &[String], files: bool) -> Result<(), QbError> {
        if hashes.is_empty() {
            return Ok(());
        }
        let joined = hashes.iter().map(|h| h.to_ascii_lowercase()).collect::<Vec<_>>().join("|");
        self.call("torrents/delete", Some(&[("hashes", joined.as_str()), ("deleteFiles", if files { "true" } else { "false" })])).await.map(|_| ())
    }

    /// Sets the priority of some of a torrent's files (0 = don't download).
    pub async fn set_file_prio(&self, hash: &str, ids: &[i64], prio: u8) -> Result<(), QbError> {
        if ids.is_empty() {
            return Ok(());
        }
        let id = ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join("|");
        let p = prio.to_string();
        self.call("torrents/filePrio", Some(&[("hash", hash), ("id", id.as_str()), ("priority", p.as_str())])).await.map(|_| ())
    }

    /// Download in order, first and last pieces first (only flips what's off).
    pub async fn stream_mode(&self, hash: &str, info: &Value) -> Result<(), QbError> {
        if info["seq_dl"] != true {
            self.call("torrents/toggleSequentialDownload", Some(&[("hashes", hash)])).await?;
        }
        if info["f_l_piece_prio"] != true {
            self.call("torrents/toggleFirstLastPiecePrio", Some(&[("hashes", hash)])).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn episode_numbers_reads_common_namings() {
        assert_eq!(episode_numbers("Show.S02E03.1080p.mkv"), vec![(2, 3)]);
        assert_eq!(episode_numbers("Show - 2x03 - Name.mkv"), vec![(2, 3)]);
        assert_eq!(episode_numbers("Show S02 E03 720p.mkv"), vec![(2, 3)]);
        assert_eq!(episode_numbers("Show.S02E03E04.mkv"), vec![(2, 3), (2, 4)]);
        assert!(episode_numbers("Show.2019.1080p.mkv").is_empty());
    }

    #[test]
    fn episode_numbers_reads_ranges() {
        assert_eq!(episode_numbers("Show.S02E03-E04.mkv"), vec![(2, 3), (2, 4)]);
        assert_eq!(episode_numbers("Show.S02E03-04.mkv"), vec![(2, 3), (2, 4)]);
        assert_eq!(episode_numbers("Show.S02E03-720p.mkv"), vec![(2, 3)]);
    }

    #[test]
    fn pick_episode_takes_that_file_and_skips_samples() {
        let files = vec![
            json!({"index":0,"name":"Pack/Show.S02E01.mkv","size":900}),
            json!({"index":1,"name":"Pack/Show.S02E03.sample.mkv","size":20}),
            json!({"index":2,"name":"Pack/Show.S02E03.mkv","size":950}),
        ];
        assert_eq!(pick_episode(&files, 2, 3).unwrap()["index"], 2);
        assert!(pick_episode(&files, 2, 9).is_none());
    }

    #[test]
    fn pick_episode_ignores_the_pack_folder_name() {
        let files = vec![
            json!({"index":0,"name":"Show.S02E01-E10.1080p/Show.S02E01.mkv","size":900}),
            json!({"index":1,"name":"Show.S02E01-E10.1080p/Show.S02E02.mkv","size":900}),
            json!({"index":2,"name":"Show S02E01 pack\\Show.S02E03.mkv","size":900}),
        ];
        assert_eq!(pick_episode(&files, 2, 1).unwrap()["index"], 0);
        assert_eq!(pick_episode(&files, 2, 2).unwrap()["index"], 1);
        assert_eq!(pick_episode(&files, 2, 3).unwrap()["index"], 2);
    }

    #[test]
    fn pick_episode_reads_other_namings() {
        let files = vec![
            json!({"index":0,"name":"P/Show - 2x03 - Name.mkv","size":900}),
            json!({"index":1,"name":"P/Show S02 E05.mkv","size":900}),
            json!({"index":2,"name":"P/Show.S02E07E08.mkv","size":900}),
        ];
        assert_eq!(pick_episode(&files, 2, 3).unwrap()["index"], 0);
        assert_eq!(pick_episode(&files, 2, 5).unwrap()["index"], 1);
        assert_eq!(pick_episode(&files, 2, 8).unwrap()["index"], 2);
    }

    #[test]
    fn picks_the_movie_file_not_the_sample() {
        let files = vec![
            json!({"index": 0, "name": "Movie/sample.mkv", "size": 50_000_000i64, "progress": 0.0}),
            json!({"index": 1, "name": "Movie/Movie.2020.1080p.mkv", "size": 4_000_000_000i64, "progress": 0.1}),
            json!({"index": 2, "name": "Movie/readme.txt", "size": 1000, "progress": 1.0}),
        ];
        assert_eq!(pick_video(&files, false).unwrap()["index"], 1);
    }

    #[test]
    fn picks_the_first_episode_of_a_season_pack() {
        let files = vec![
            json!({"index": 0, "name": "Show.S01/Show.S01E02.mkv", "size": 900_000_000i64}),
            json!({"index": 1, "name": "Show.S01/Show.S01E01.mkv", "size": 800_000_000i64}),
            json!({"index": 2, "name": "Show.S01/Show.S01E10.mkv", "size": 950_000_000i64}),
        ];
        assert_eq!(pick_video(&files, true).unwrap()["index"], 1);
        assert!(pick_video(&[json!({"name": "a.nfo", "size": 1})], false).is_none());
    }

    #[test]
    fn the_file_is_looked_for_where_qbittorrent_keeps_it() {
        let c = candidate_paths(r"D:\Done", r"D:\Incomplete", "Movie/Movie.mkv", false);
        assert_eq!(c[0], std::path::Path::new(r"D:\Incomplete").join("Movie/Movie.mkv"));
        assert!(c.contains(&std::path::Path::new(r"D:\Incomplete").join("Movie/Movie.mkv.!qB")));
        assert!(c.contains(&std::path::Path::new(r"D:\Done").join("Movie/Movie.mkv")));
        let done = candidate_paths(r"D:\Done", "", "Movie/Movie.mkv", true);
        assert_eq!(done[0], std::path::Path::new(r"D:\Done").join("Movie/Movie.mkv"));
    }

    #[test]
    fn queue_record_gives_the_torrent_hash() {
        let recs = vec![
            json!({"movieId": 7, "status": "delay", "protocol": "torrent"}),
            json!({"movieId": 7, "status": "downloading", "protocol": "torrent", "downloadId": "ABC123"}),
        ];
        assert_eq!(torrent_hash(&recs, "movieId", 7), Pick::Hash("abc123".into()));
        let delayed = vec![json!({"movieId": 7, "status": "delay", "protocol": "torrent"})];
        assert_eq!(torrent_hash(&delayed, "movieId", 7), Pick::Delayed);
        let usenet = vec![json!({"movieId": 7, "protocol": "usenet", "downloadId": "SABnzbd_1"})];
        assert_eq!(torrent_hash(&usenet, "movieId", 7), Pick::NotTorrent);
        assert_eq!(torrent_hash(&recs, "movieId", 8), Pick::Nothing);
    }

    #[test]
    fn speed_reads_plainly() {
        assert_eq!(speed(0), "waiting for peers");
        assert_eq!(speed(512_000), "500 KB/s");
        assert_eq!(speed(3_500_000), "3.3 MB/s");
    }
}
