//! Getting things: adding a movie or series to Radarr/Sonarr from a
//! recommendation, what is downloading right now, and keeping Defender's
//! protected-folder rule from blocking the apps that move files.

use crate::tiles::Tile;
use serde_json::{json, Value};

/// The root folder for a new item: the anime one for anime, otherwise the first
/// that isn't anime. Falls back to whatever exists.
pub fn pick_root(roots: &[Value], anime: bool) -> Option<String> {
    let paths: Vec<String> = roots.iter().filter_map(|r| r["path"].as_str().map(String::from)).collect();
    // Only the folder's own name: the user's Windows account may itself be called
    // "anime" (C:\Users\anime\...), which would make every path look like anime.
    let is_anime = |p: &String| {
        p.trim_end_matches(['\\', '/'])
            .rsplit(['\\', '/'])
            .next()
            .is_some_and(|name| name.to_ascii_lowercase().contains("anime"))
    };
    paths
        .iter()
        .find(|p| is_anime(p) == anime)
        .or_else(|| paths.first())
        .cloned()
}

/// "HD - 720p/1080p" if it exists, else 1080p, else the first profile.
pub fn pick_profile(profiles: &[Value]) -> Option<i64> {
    let by = |name: &str| profiles.iter().find(|p| p["name"] == name).and_then(|p| p["id"].as_i64());
    by("HD - 720p/1080p").or_else(|| by("HD-1080p")).or_else(|| profiles.first().and_then(|p| p["id"].as_i64()))
}

/// The profile for a quality choice: "best" (the 720p/1080p mix), "4k",
/// "1080p" or "720p". Falls back to the default pick when the app lacks it.
pub fn profile_for(profiles: &[Value], quality: &str) -> Option<i64> {
    let named = |names: &[&str]| profiles.iter().find(|p| p["name"].as_str().is_some_and(|n| names.iter().any(|x| n.eq_ignore_ascii_case(x)))).and_then(|p| p["id"].as_i64());
    // A missing quality steps down to the next best one the app has.
    let p4k = || named(&["Ultra-HD", "UHD", "2160p", "4K"]);
    let p1080 = || named(&["HD-1080p", "1080p"]);
    let p720 = || named(&["HD-720p", "720p"]);
    let hit = match quality {
        "4k" => p4k().or_else(p1080).or_else(p720),
        "1080p" => p1080().or_else(p720),
        "720p" => p720(),
        _ => None,
    };
    hit.or_else(|| pick_profile(profiles))
}

/// Where a requested title stands.
pub fn wanted_status(released: bool, has_file: bool, progress: Option<f64>) -> String {
    if has_file {
        "Ready to watch".into()
    } else if let Some(p) = progress {
        format!("Downloading {}%", (p * 100.0).round() as i64)
    } else if released {
        "Looking for a copy".into()
    } else {
        "Not out yet".into()
    }
}

/// Files Radarr can match in the download folder that should be brought into
/// the library: the movie has no file yet, isn't still downloading, and Radarr
/// has no objection (sample, wrong movie, ...).
pub fn importable(candidates: &[Value], missing: &[i64], downloading: &[i64]) -> Vec<Value> {
    candidates
        .iter()
        .filter(|c| {
            let Some(id) = c["movie"]["id"].as_i64() else { return false };
            missing.contains(&id) && !downloading.contains(&id) && c["rejections"].as_array().is_none_or(|r| r.is_empty())
        })
        .cloned()
        .collect()
}

pub fn movie_payload(mut lookup: Value, profile: i64, root: &str) -> Value {
    lookup["qualityProfileId"] = json!(profile);
    lookup["rootFolderPath"] = json!(root);
    lookup["monitored"] = json!(true);
    lookup["minimumAvailability"] = json!("released");
    lookup["addOptions"] = json!({ "searchForMovie": true });
    lookup
}

pub fn series_payload(mut lookup: Value, profile: i64, root: &str, anime: bool) -> Value {
    lookup["qualityProfileId"] = json!(profile);
    lookup["rootFolderPath"] = json!(root);
    lookup["monitored"] = json!(true);
    lookup["seasonFolder"] = json!(true);
    lookup["seriesType"] = json!(if anime { "anime" } else { "standard" });
    lookup["addOptions"] = json!({ "searchForMissingEpisodes": true, "monitor": "all" });
    // A whole show also takes seasons that come out later. Anything picked
    // (some seasons, some episodes) sets this to "none" instead.
    lookup["monitorNewItems"] = json!("all");
    lookup
}

/// The show's own title from an AniList season entry, and whether it was one
/// ("X Season 3", "X 2nd Season", "X Part 2" all name a later season of X).
pub fn base_title(t: &str) -> (String, bool) {
    let words: Vec<&str> = t.split_whitespace().collect();
    let n = words.len();
    let is_num = |w: &str| w.chars().all(|c| c.is_ascii_digit()) || w.trim_end_matches(|c: char| c.is_ascii_alphabetic()).chars().all(|c| c.is_ascii_digit()) && w.chars().next().is_some_and(|c| c.is_ascii_digit());
    if n >= 3 {
        let (a, b) = (words[n - 2].to_ascii_lowercase(), words[n - 1]);
        if (a == "season" || a == "part" || a == "cour") && is_num(b) {
            return (words[..n - 2].join(" ").trim_end_matches([':', '-', ' ']).to_string(), true);
        }
        if b.eq_ignore_ascii_case("season") && is_num(&a) {
            return (words[..n - 2].join(" ").trim_end_matches([':', '-', ' ']).to_string(), true);
        }
    }
    (t.to_string(), false)
}

/// The lookup result with the same year as the tile, else the first.
pub fn pick_lookup(found: &[Value], year: Option<i64>) -> Option<Value> {
    year.and_then(|y| found.iter().find(|f| f["year"].as_i64() == Some(y)))
        .or_else(|| found.first())
        .cloned()
}

/// A later-season entry starts at the latest season; otherwise the first, so a
/// long runner doesn't queue a thousand episodes at once.
pub fn anime_monitor(season_entry: bool) -> &'static str {
    if season_entry { "latestSeason" } else { "firstSeason" }
}

/// A row of Radarr's or Sonarr's download queue as a tile with its progress.
pub fn from_queue(rec: &Value) -> Tile {
    let size = rec["size"].as_f64().unwrap_or(0.0);
    let left = rec["sizeleft"].as_f64().unwrap_or(size);
    let progress = if size > 0.0 { ((size - left) / size * 100.0).round() / 100.0 } else { 0.0 };
    let started = rec["status"].as_str().is_some_and(|s| s != "queued" && s != "delay") && progress > 0.0;
    let paused = rec["status"].as_str().is_some_and(|s| s == "paused");
    let status = if paused {
        format!("Paused · {}%", (progress * 100.0).round() as i64)
    } else if started {
        let mut s = format!("Downloading · {}%", (progress * 100.0).round() as i64);
        if let Some(t) = rec["timeleft"].as_str() {
            s.push_str(&format!(" · {t} left"));
        }
        s
    } else {
        "Waiting to start".to_string()
    };
    let (owner, kind) = if rec["movie"].is_object() { (&rec["movie"], "movie") } else { (&rec["series"], "episode") };
    let poster = owner["images"]
        .as_array()
        .and_then(|a| a.iter().find(|i| i["coverType"] == "poster"))
        .and_then(|i| i["remoteUrl"].as_str())
        .map(|u| u.replace("/original/", "/w500/"));
    let subtitle = rec["episode"].is_object().then(|| {
        format!(
            "S{} E{} · {}",
            rec["episode"]["seasonNumber"].as_i64().unwrap_or(0),
            rec["episode"]["episodeNumber"].as_i64().unwrap_or(0),
            rec["episode"]["title"].as_str().unwrap_or_default()
        )
    });
    Tile {
        key: format!("queue:{}", rec["id"].as_i64().unwrap_or_default()),
        kind: kind.into(),
        title: owner["title"].as_str().unwrap_or_default().into(),
        subtitle,
        year: owner["year"].as_i64(),
        poster,
        progress: Some(progress),
        tmdb: owner["tmdbId"].as_i64(),
        tvdb: owner["tvdbId"].as_i64(),
        status: Some(status),
        genres: crate::tiles::genre_list(&owner["genres"]),
        ..Default::default()
    }
}

/// The script (for `procs::run_elevated`) that adds these programs to
/// Defender's protected-folder allow list, so they can move downloads into
/// Desktop/Videos without being blocked. Each path is one quoted string.
pub fn allow_command(exes: &[String]) -> String {
    let list: Vec<String> = exes.iter().map(|e| crate::procs::ps_quote(e)).collect();
    format!("Add-MpPreference -ControlledFolderAccessAllowedApplications {}\nexit 0", list.join(","))
}

/// Whether Defender's protected-folder rule is on (read without admin).
pub fn controlled_folder_access_on() -> bool {
    use std::os::windows::process::CommandExt;
    std::process::Command::new(crate::procs::powershell())
        .args(["-NoProfile", "-NonInteractive", "-Command", "(Get-MpPreference).EnableControlledFolderAccess"])
        .creation_flags(0x0800_0000)
        .stdin(std::process::Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "1")
        .unwrap_or(false)
}

/// Whether Chillarr, running without admin, could replace this program or put
/// a file (a DLL it would load) beside it. Then so could anything else the
/// user runs, and it would inherit the program's Defender exception. Asked by
/// trying: a probe file in the folder (removed at once), and opening the exe
/// for writing without changing it. "In use" means the write was allowed.
pub fn user_can_change(exe: &std::path::Path) -> bool {
    let Some(dir) = exe.parent() else { return true };
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default();
    let probe = dir.join(format!(".chillarr-write-check-{}-{stamp}", std::process::id()));
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(f) => {
            drop(f);
            let _ = std::fs::remove_file(&probe);
            return true;
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return true,
        Err(_) => {}
    }
    match std::fs::OpenOptions::new().write(true).open(exe) {
        Ok(_) => true,
        // ERROR_SHARING_VIOLATION: allowed to write, the program is just running.
        Err(e) => e.raw_os_error() == Some(32),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn roots() -> Vec<Value> {
        vec![
            json!({"path": "C:\\Users\\anime\\Desktop\\Desktop\\Movies\\Normal"}),
            json!({"path": "C:\\Users\\anime\\Desktop\\Desktop\\Movies\\Anime"}),
        ]
    }

    #[test]
    fn anime_goes_to_the_anime_folder() {
        assert_eq!(pick_root(&roots(), true).as_deref(), Some("C:\\Users\\anime\\Desktop\\Desktop\\Movies\\Anime"));
        assert_eq!(pick_root(&roots(), false).as_deref(), Some("C:\\Users\\anime\\Desktop\\Desktop\\Movies\\Normal"));
        assert_eq!(pick_root(&roots()[..1], true).as_deref(), Some("C:\\Users\\anime\\Desktop\\Desktop\\Movies\\Normal"), "no anime folder: use what there is");
        assert_eq!(pick_root(&[], false), None);
    }

    #[test]
    fn prefers_the_720p_1080p_profile() {
        let p = vec![json!({"id":1,"name":"Any"}), json!({"id":4,"name":"HD-1080p"}), json!({"id":6,"name":"HD - 720p/1080p"})];
        assert_eq!(pick_profile(&p), Some(6));
        assert_eq!(pick_profile(&p[..2]), Some(4));
        assert_eq!(pick_profile(&p[..1]), Some(1));
    }

    #[test]
    fn movie_is_added_monitored_and_searched() {
        let lookup = json!({"title": "Predator", "tmdbId": 106, "year": 1987, "images": []});
        let m = movie_payload(lookup, 6, "C:\\Movies");
        assert_eq!(m["qualityProfileId"], 6);
        assert_eq!(m["rootFolderPath"], "C:\\Movies");
        assert_eq!(m["monitored"], true);
        assert_eq!(m["addOptions"]["searchForMovie"], true);
        assert_eq!(m["tmdbId"], 106);
    }

    #[test]
    fn anime_series_is_added_as_anime() {
        let lookup = json!({"title": "Frieren", "tvdbId": 424536});
        let s = series_payload(lookup.clone(), 6, "C:\\Series\\Anime", true);
        assert_eq!(s["seriesType"], "anime");
        assert_eq!(s["seasonFolder"], true);
        assert_eq!(s["addOptions"]["searchForMissingEpisodes"], true);
        assert_eq!(s["addOptions"]["monitor"], "all");
        assert_eq!(s["monitorNewItems"], "all");
        assert_eq!(series_payload(lookup, 6, "C:\\Series", false)["seriesType"], "standard");
    }

    #[test]
    fn queue_record_shows_download_progress() {
        let rec = json!({"title": "Predator.1987.1080p", "size": 1000.0, "sizeleft": 250.0, "status": "downloading",
            "timeleft": "00:12:30", "movie": {"title": "Predator", "year": 1987, "tmdbId": 106,
            "images": [{"coverType": "poster", "remoteUrl": "https://image.tmdb.org/t/p/original/x.jpg"}]}});
        let t = from_queue(&rec);
        assert_eq!(t.title, "Predator");
        assert_eq!(t.progress, Some(0.75));
        assert_eq!(t.status.as_deref(), Some("Downloading · 75% · 00:12:30 left"));
        assert_eq!(t.kind, "movie");
        let ep = json!({"size": 10.0, "sizeleft": 10.0, "status": "queued",
            "series": {"title": "Peaky Blinders", "tvdbId": 270915, "images": []},
            "episode": {"seasonNumber": 2, "episodeNumber": 3, "title": "Ep"}});
        let t = from_queue(&ep);
        assert_eq!((t.kind.as_str(), t.subtitle.as_deref()), ("episode", Some("S2 E3 · Ep")));
        assert_eq!(t.status.as_deref(), Some("Waiting to start"));
    }

    #[test]
    fn anilist_season_titles_find_the_show() {
        assert_eq!(base_title("The Apothecary Diaries Season 3"), ("The Apothecary Diaries".to_string(), true));
        assert_eq!(base_title("Dr. Stone: Science Future Part 2"), ("Dr. Stone: Science Future".to_string(), true));
        assert_eq!(base_title("Mushoku Tensei 2nd Season"), ("Mushoku Tensei".to_string(), true));
        assert_eq!(base_title("Frieren: Beyond Journey's End"), ("Frieren: Beyond Journey's End".to_string(), false));
    }

    #[test]
    fn lookup_prefers_the_matching_year() {
        let found = vec![json!({"title":"Frieren","year":2010}), json!({"title":"Frieren","year":2023}), json!({"title":"Frieren","year":2024})];
        assert_eq!(pick_lookup(&found, Some(2023)).unwrap()["year"], 2023);
        assert_eq!(pick_lookup(&found, None).unwrap()["year"], 2010);
        assert_eq!(pick_lookup(&found, Some(1999)).unwrap()["year"], 2010, "no match: first result");
        assert!(pick_lookup(&[], Some(1)).is_none());
    }

    #[test]
    fn long_anime_is_not_queued_whole() {
        assert_eq!(anime_monitor(true), "latestSeason");
        assert_eq!(anime_monitor(false), "firstSeason");
    }

    #[test]
    fn quality_choice_maps_to_the_apps_profiles() {
        let p = vec![json!({"id":1,"name":"Any"}), json!({"id":3,"name":"HD-720p"}), json!({"id":4,"name":"HD-1080p"}), json!({"id":5,"name":"Ultra-HD"}), json!({"id":6,"name":"HD - 720p/1080p"})];
        assert_eq!(profile_for(&p, "4k"), Some(5));
        assert_eq!(profile_for(&p, "1080p"), Some(4));
        assert_eq!(profile_for(&p, "720p"), Some(3));
        assert_eq!(profile_for(&p, "best"), Some(6), "the default mix");
        assert_eq!(profile_for(&p[..2], "4k"), Some(3), "no 4K profile: fall back to the default pick");
    }

    #[test]
    fn my_list_says_where_each_title_is() {
        assert_eq!(wanted_status(false, false, None), "Not out yet");
        assert_eq!(wanted_status(true, false, None), "Looking for a copy");
        assert_eq!(wanted_status(true, false, Some(0.45)), "Downloading 45%");
        assert_eq!(wanted_status(true, true, None), "Ready to watch");
    }

    #[test]
    fn only_finished_files_for_movies_without_one_are_imported() {
        let c = vec![
            json!({"path": "D:/dl/Dark.Tower.mp4", "movie": {"id": 8}, "rejections": []}),
            json!({"path": "D:/dl/Replicas.mkv", "movie": {"id": 1}, "rejections": []}),
            json!({"path": "D:/dl/Spider-Man.mkv", "movie": {"id": 30}, "rejections": []}),
            json!({"path": "D:/dl/Sample.mkv", "movie": {"id": 9}, "rejections": [{"reason": "Sample"}]}),
            json!({"path": "D:/dl/Unknown.mkv", "rejections": []}),
        ];
        // 8 and 9 have no file; 1 already has one; 30 is still downloading.
        let picked = importable(&c, &[8, 9, 30], &[30]);
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0]["path"], "D:/dl/Dark.Tower.mp4");
    }

    #[test]
    fn defender_allow_command_lists_only_our_apps() {
        let cmd = allow_command(&["C:\\Program Files\\qBittorrent\\qbittorrent.exe".to_string(), "C:\\ProgramData\\Radarr\\bin\\Radarr.exe".to_string()]);
        assert_eq!(
            cmd,
            "Add-MpPreference -ControlledFolderAccessAllowedApplications 'C:\\Program Files\\qBittorrent\\qbittorrent.exe','C:\\ProgramData\\Radarr\\bin\\Radarr.exe'\nexit 0"
        );
    }

    #[test]
    fn defender_paths_cant_break_out() {
        // A folder name made to end the string and run something else stays a path.
        let cmd = allow_command(&[r#"C:\x' ; calc ; '" & calc & "%COMSPEC%\qbittorrent.exe"#.to_string()]);
        assert_eq!(cmd, "Add-MpPreference -ControlledFolderAccessAllowedApplications 'C:\\x'' ; calc ; ''\" & calc & \"%COMSPEC%\\qbittorrent.exe'\nexit 0");
    }

    #[test]
    fn a_folder_the_user_can_write_is_noticed() {
        let d = tempfile::tempdir().unwrap();
        let exe = d.path().join("qbittorrent.exe");
        std::fs::write(&exe, b"").unwrap();
        assert!(user_can_change(&exe));
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 1, "the probe file is removed");
    }

    #[test]
    #[ignore]
    fn live_program_files_is_not_user_writable() {
        let windir = std::env::var("SystemRoot").unwrap();
        assert!(!user_can_change(&std::path::Path::new(&windir).join("System32").join("notepad.exe")));
    }
}
