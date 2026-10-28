//! Storage: how full the drives holding the library are, what takes the most
//! room, and an opt-in clean-up of what you've finished watching. Sizes come
//! from Radarr and Sonarr (they know every file they put there). The clean-up
//! reads the watch log (`history`), episode by episode: an episode's file goes
//! and Sonarr stops looking for it; a movie goes through `waiting::delete_title`
//! like the detail panel's Delete, so Radarr doesn't fetch it back.

use crate::log::log;
use crate::media::{arr, jf_admin};
use crate::model::{AppId, Level};
use crate::tiles::{self, Tile};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::{AppHandle, Manager};

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Drive {
    pub path: String,
    pub free: u64,
    pub total: u64,
}

#[derive(Serialize, Clone, Debug)]
pub struct Item {
    pub tile: Tile,
    pub size: u64,
    pub watched: bool,
    /// When it was last played (RFC 3339), if ever.
    pub last_played: Option<String>,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Overview {
    pub drives: Vec<Drive>,
    pub items: Vec<Item>,
    pub cleanup_days: Option<u32>,
}

/// The clean-up choice, in storage.json next to Chillarr's other files.
#[derive(Serialize, Deserialize, Default, Clone)]
struct Saved {
    cleanup_days: Option<u32>,
    /// Chosen since the clean-up went per episode. A choice made before it
    /// (whole titles only) is switched off once rather than carried over.
    #[serde(default)]
    per_episode: bool,
}

fn saved_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("storage.json"))
}

fn load_saved(app: &AppHandle) -> Saved {
    saved_path(app).map(|p| read_saved(&p)).unwrap_or_default()
}

fn read_saved(path: &Path) -> Saved {
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// The clean-up's days after the last play, None when it's off.
pub fn cleanup_days(app: &AppHandle) -> Option<u32> {
    load_saved(app).cleanup_days
}

/// Turns the clean-up on (days after the last play) or off (None).
pub fn set_cleanup(app: &AppHandle, days: Option<u32>) -> Result<(), String> {
    write_cleanup(&saved_path(app).ok_or("No app-data folder")?, days)
}

/// Any number of days from 1 up is kept as is; 0 or None is off.
fn write_cleanup(path: &Path, days: Option<u32>) -> Result<(), String> {
    let text = serde_json::to_string(&Saved { cleanup_days: days.filter(|d| *d > 0), per_episode: true }).unwrap_or_default();
    std::fs::write(path, text).map_err(|e| format!("Couldn't save it: {e}"))
}

/// Keeps the drives a folder of ours sits on (Radarr reports every drive).
fn our_drives(all: Vec<Drive>, folders: &[String]) -> Vec<Drive> {
    let lower: Vec<String> = folders.iter().map(|f| f.to_lowercase()).collect();
    let mine: Vec<Drive> = all.iter().filter(|d| !d.path.is_empty() && lower.iter().any(|f| f.starts_with(&d.path.to_lowercase()))).cloned().collect();
    // Longest match wins: "D:\Media" over "D:\" when both are listed.
    let mut keep: Vec<Drive> = Vec::new();
    for d in mine {
        let inner = lower.iter().filter(|f| f.starts_with(&d.path.to_lowercase())).all(|f| {
            !all.iter().any(|o| o.path.len() > d.path.len() && f.starts_with(&o.path.to_lowercase()))
        });
        if inner && !keep.contains(&d) {
            keep.push(d);
        }
    }
    if keep.is_empty() { all } else { keep }
}

fn poster(v: &Value) -> Option<String> {
    v["images"].as_array()?.iter().find(|i| i["coverType"] == "poster").and_then(|i| i["remoteUrl"].as_str()).map(|u| u.replace("/original/", "/w342/"))
}

pub async fn overview(app: &AppHandle) -> Overview {
    let radarr = arr(app, AppId::Radarr);
    let sonarr = arr(app, AppId::Sonarr);
    let mut folders: Vec<String> = Vec::new();
    for a in [&radarr, &sonarr].into_iter().flatten() {
        for r in a.list("rootfolder").await.unwrap_or_default() {
            if let Some(p) = r["path"].as_str() {
                folders.push(p.to_string());
            }
        }
    }
    if let Some(d) = crate::media::download_folder() {
        folders.push(d);
    }
    let disk_source = radarr.as_ref().or(sonarr.as_ref());
    let all_drives: Vec<Drive> = match disk_source {
        Some(a) => a.list("diskspace").await.unwrap_or_default().iter().filter_map(|d| {
            Some(Drive { path: d["path"].as_str()?.to_string(), free: d["freeSpace"].as_u64()?, total: d["totalSpace"].as_u64()? })
        }).filter(|d| d.total > 0).collect(),
        None => vec![],
    };

    // Jellyfin's copy of each title: playable tile, and whether it was watched.
    let mut by_tmdb = std::collections::HashMap::new();
    let mut by_tvdb = std::collections::HashMap::new();
    if let Some(j) = jf_admin(app).filter(|j| j.user_id.is_some()) {
        for v in j.items("recursive=true&includeItemTypes=Movie,Series&limit=2000").await.unwrap_or_default() {
            let t = tiles::from_jf(&v, &j.base);
            let watched = v["UserData"]["Played"].as_bool().unwrap_or(false);
            let last = v["UserData"]["LastPlayedDate"].as_str().map(String::from);
            if let Some(id) = t.tmdb.filter(|_| t.kind == "movie") {
                by_tmdb.insert(id, (t.clone(), watched, last.clone()));
            }
            if let Some(id) = t.tvdb.filter(|_| t.kind == "series") {
                by_tvdb.insert(id, (t, watched, last));
            }
        }
    }

    let mut items = Vec::new();
    if let Some(a) = &radarr {
        for m in a.list("movie").await.unwrap_or_default() {
            let size = m["sizeOnDisk"].as_u64().unwrap_or(0);
            if size == 0 {
                continue;
            }
            let tmdb = m["tmdbId"].as_i64();
            let (tile, watched, last_played) = match tmdb.and_then(|id| by_tmdb.get(&id)) {
                Some((t, w, l)) => (t.clone(), *w, l.clone()),
                None => {
                    let mut t = Tile::new(&format!("tmdb:{}", tmdb.unwrap_or_default()), "movie", m["title"].as_str().unwrap_or("Movie"));
                    t.year = m["year"].as_i64();
                    t.tmdb = tmdb;
                    t.poster = poster(&m);
                    (t, false, None)
                }
            };
            items.push(Item { tile, size, watched, last_played });
        }
    }
    if let Some(a) = &sonarr {
        for s in a.list("series").await.unwrap_or_default() {
            let size = s["statistics"]["sizeOnDisk"].as_u64().unwrap_or(0);
            if size == 0 {
                continue;
            }
            let tvdb = s["tvdbId"].as_i64();
            let (tile, watched, last_played) = match tvdb.and_then(|id| by_tvdb.get(&id)) {
                Some((t, w, l)) => (t.clone(), *w, l.clone()),
                None => {
                    let mut t = Tile::new(&format!("tvdb:{}", tvdb.unwrap_or_default()), "series", s["title"].as_str().unwrap_or("Series"));
                    t.year = s["year"].as_i64();
                    t.tvdb = tvdb;
                    t.poster = poster(&s);
                    (t, false, None)
                }
            };
            items.push(Item { tile, size, watched, last_played });
        }
    }
    items.sort_by(|a, b| b.size.cmp(&a.size));
    Overview { drives: our_drives(all_drives, &folders), items, cleanup_days: load_saved(app).cleanup_days }
}

/// The opt-in clean-up, every 6 hours, from the watch log: each episode goes
/// `days` calendar days after it was watched, each movie `days` after it was
/// finished. Unwatched and new episodes stay, and nothing another profile is
/// partway through is touched.
pub fn start_cleanup(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(120)).await;
        // Turned on before it went per episode: the first pass would delete
        // every watched episode, shows partway through included. Off once;
        // it's turned back on knowing what it does now.
        let saved = load_saved(&app);
        if saved.cleanup_days.is_some() && !saved.per_episode && set_cleanup(&app, None).is_ok() {
            log(&app, Level::Amber, "Clean-up now works per episode, so it was switched off once. Turn it back on in Settings > Library & storage.");
        }
        loop {
            if let Some(days) = load_saved(&app).cleanup_days {
                clean_up(&app, days).await;
            }
            tokio::time::sleep(Duration::from_secs(6 * 3600)).await;
        }
    });
}

async fn clean_up(app: &AppHandle, days: u32) {
    // Up to the minute, so a pass right after starting doesn't see old logs.
    crate::history::sync(app).await;
    let all = match crate::history::everyone(app) {
        Ok(all) => all,
        Err(name) => {
            log(app, Level::Amber, format!("Clean-up is waiting: Chillarr can't read {name}'s watch history right now, so it can't tell what they're partway through. If that profile no longer signs in, remove it."));
            return;
        }
    };
    let now = chrono::Utc::now().timestamp();
    let mut deleted = false;

    let movies = crate::history::due_movies(&all, days, now, &chrono::Local);
    if let (Some(a), false) = (arr(app, AppId::Radarr), movies.is_empty()) {
        // What's on disk, and since when: a copy added after the watch is a
        // download nobody has watched yet.
        let on_disk: HashMap<i64, Value> = a.list("movie").await.unwrap_or_default().iter().filter(|m| m["hasFile"] == true).filter_map(|m| Some((m["tmdbId"].as_i64()?, m["movieFile"]["dateAdded"].clone()))).collect();
        for (tmdb, title, _) in movies.into_iter().filter(|(id, _, at)| on_disk.get(id).is_some_and(|added| crate::history::file_predates(added, *at))) {
            let mut tile = Tile::new(&format!("tmdb:{tmdb}"), "movie", &title);
            tile.tmdb = Some(tmdb);
            match crate::waiting::delete_title(app, &tile).await {
                Ok(_) => log(app, Level::Ok, format!("Cleaned up {title}: finished more than {days} days ago.")),
                Err(e) => log(app, Level::Amber, format!("Couldn't clean up {title}: {e}")),
            }
        }
    }

    let episodes = crate::history::due_episodes(&all, days, now, &chrono::Local);
    if let (Some(s), false) = (arr(app, AppId::Sonarr), episodes.is_empty()) {
        let series = s.list("series").await.unwrap_or_default();
        let mut by_show: HashMap<i64, Vec<&crate::history::DueEpisode>> = HashMap::new();
        for e in &episodes {
            by_show.entry(e.tvdb).or_default().push(e);
        }
        for (tvdb, due) in by_show {
            let Some(show) = series.iter().find(|x| x["tvdbId"].as_i64() == Some(tvdb)) else { continue };
            let Some(sid) = show["id"].as_i64() else { continue };
            let title = show["title"].as_str().unwrap_or(&due[0].title).to_string();
            let (Ok(files), Ok(eps)) = (s.list(&format!("episodefile?seriesId={sid}")).await, s.list(&format!("episode?seriesId={sid}")).await) else { continue };
            let want: HashMap<String, i64> = due.iter().map(|d| (d.path.clone(), d.at)).collect();
            let plan = episode_plan(&files, &eps, &want);
            if plan.files.is_empty() {
                continue;
            }
            // Unmonitored first, so Sonarr doesn't fetch them again the moment the files go.
            if let Err(e) = s.update("episode/monitor", &json!({ "episodeIds": plan.episodes, "monitored": false })).await {
                log(app, Level::Amber, format!("Couldn't clean up {title}: Sonarr wouldn't stop looking for those episodes ({e})."));
                continue;
            }
            let mut gone = 0u64;
            for (file, size) in &plan.files {
                if crate::subfetch::delete_episode_file(app, &s, *file).await {
                    gone += size;
                    deleted = true;
                }
            }
            log(app, Level::Ok, format!("Cleaned up {title}: {} ({}), watched more than {days} days ago.", plan.names.join(", "), gb(gone)));
            drop_seeding_copies(app, &s, sid).await;
        }
    }
    if deleted {
        if let Some(j) = jf_admin(app) {
            let _ = j.scan().await;
        }
    }
}

/// Which Sonarr episode files a clean-up deletes: the ones at the paths of
/// watched files that were already there when watched. Matched by path, never by number, since Jellyfin and Sonarr
/// can number the same file differently (anime seasons especially). A path
/// Sonarr doesn't have is left alone.
#[derive(Debug, Default, PartialEq)]
struct EpisodePlan {
    /// (episodeFile id, size)
    files: Vec<(i64, u64)>,
    /// Sonarr episode ids in those files, to unmonitor.
    episodes: Vec<i64>,
    /// "S1 E2" (Sonarr's numbering) for the log.
    names: Vec<String>,
}

fn episode_plan(files: &[Value], eps: &[Value], want: &HashMap<String, i64>) -> EpisodePlan {
    let mut plan = EpisodePlan::default();
    for f in files {
        let (Some(id), Some(path)) = (f["id"].as_i64(), f["path"].as_str()) else { continue };
        // Same path, but downloaded again after the watch: not watched yet.
        if !want.get(&crate::history::norm(path)).is_some_and(|at| crate::history::file_predates(&f["dateAdded"], *at)) {
            continue;
        }
        plan.files.push((id, f["size"].as_u64().unwrap_or(0)));
        for e in eps.iter().filter(|e| e["episodeFileId"].as_i64() == Some(id)) {
            plan.episodes.extend(e["id"].as_i64());
            plan.names.push(format!("S{} E{}", e["seasonNumber"].as_i64().unwrap_or(0), e["episodeNumber"].as_i64().unwrap_or(0)));
        }
    }
    plan
}

/// The seeding copy in Downloads is a hardlink to the library file, so the
/// space only comes back once the torrent goes too. A torrent goes when every
/// episode it brought in has been cleaned up (no file, not monitored).
pub(crate) async fn drop_seeding_copies(app: &AppHandle, s: &crate::arr::Arr, sid: i64) {
    let Some(q) = crate::waiting::qbit(app) else { return };
    let history = s.list(&format!("history/series?seriesId={sid}")).await.unwrap_or_default();
    let eps = s.list(&format!("episode?seriesId={sid}")).await.unwrap_or_default();
    let doomed = finished_torrents(&history, &eps);
    if doomed.is_empty() {
        return;
    }
    let safe = safe_to_delete_files(app, &q, &doomed).await;
    if !safe.is_empty() {
        let _ = q.delete(&safe, true).await;
    }
}

/// Those of `hashes` whose files may go with them. Never a torrent whose files
/// are the library's own (a download folder set inside a root folder, or a
/// pack used in place): deleting its files would delete the library, other
/// titles included. Both apps' root folders and qBittorrent's list must
/// answer, or nothing goes.
pub(crate) async fn safe_to_delete_files(app: &AppHandle, q: &crate::qbit::Qbit, hashes: &[String]) -> Vec<String> {
    let mut roots = Vec::new();
    for owner in [AppId::Radarr, AppId::Sonarr] {
        let Some(a) = arr(app, owner) else { continue };
        let Ok(folders) = a.list("rootfolder").await else { return Vec::new() };
        roots.extend(folders.iter().filter_map(|f| f["path"].as_str().map(crate::waiting::norm_path)));
    }
    let Ok(torrents) = q.list().await else { return Vec::new() };
    outside_library(hashes, &torrents, &roots)
}

/// Those of `hashes` whose files sit outside every library root folder. A
/// torrent with no content path can't be checked, so it stays.
fn outside_library(hashes: &[String], torrents: &[Value], roots: &[String]) -> Vec<String> {
    torrents
        .iter()
        .filter_map(|t| Some((t["hash"].as_str()?.to_ascii_lowercase(), t["content_path"].as_str().filter(|p| !p.is_empty())?)))
        .filter(|(h, path)| hashes.contains(h) && !crate::waiting::inside_any(path, roots))
        .map(|(h, _)| h)
        .collect()
}

fn finished_torrents(history: &[Value], eps: &[Value]) -> Vec<String> {
    let mut by_hash: HashMap<String, HashSet<i64>> = HashMap::new();
    for r in history.iter().filter(|r| r["eventType"] == "grabbed") {
        if let (Some(h), Some(e)) = (r["downloadId"].as_str(), r["episodeId"].as_i64()) {
            by_hash.entry(h.to_ascii_lowercase()).or_default().insert(e);
        }
    }
    let cleaned = |id: &i64| eps.iter().find(|e| e["id"].as_i64() == Some(*id)).is_some_and(|e| e["hasFile"] != true && e["monitored"] != true);
    let mut out: Vec<String> = by_hash.into_iter().filter(|(_, ids)| ids.iter().all(cleaned)).map(|(h, _)| h).collect();
    out.sort();
    out
}

pub(crate) fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive(p: &str) -> Drive {
        Drive { path: p.into(), free: 1, total: 2 }
    }

    #[test]
    fn one_and_three_days_are_saved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("storage.json");
        for days in [1, 3, 7, 90] {
            write_cleanup(&path, Some(days)).unwrap();
            let saved = read_saved(&path);
            assert_eq!(saved.cleanup_days, Some(days));
            assert!(saved.per_episode);
        }
        write_cleanup(&path, Some(0)).unwrap();
        assert_eq!(read_saved(&path).cleanup_days, None, "0 means off");
        write_cleanup(&path, None).unwrap();
        assert_eq!(read_saved(&path).cleanup_days, None);
    }

    #[test]
    fn only_the_drives_our_folders_are_on() {
        let all = vec![drive("C:\\"), drive("D:\\"), drive("D:\\Media"), drive("E:\\")];
        let got = our_drives(all, &["D:\\Media\\Movies".into(), "c:\\Users\\x\\Downloads".into()]);
        assert_eq!(got, vec![drive("C:\\"), drive("D:\\Media")]);
        // Nothing matches (odd paths): show them all rather than nothing.
        assert_eq!(our_drives(vec![drive("C:\\")], &["\\\\nas\\share".into()]).len(), 1);
    }

    #[test]
    fn files_are_matched_by_path_not_by_number() {
        let file = |id: i64, path: &str| json!({ "id": id, "path": path, "size": 1_000_000_000u64, "dateAdded": "2026-09-01T10:00:00Z" });
        let ep = |id: i64, s: i64, e: i64, file: i64| json!({ "id": id, "seasonNumber": s, "episodeNumber": e, "episodeFileId": file });
        // Jellyfin calls the file "S2E1"; Sonarr calls the same file S1E13.
        let files = vec![file(10, r"C:\Series\Show\Season 1\Show - 13.mkv"), file(11, r"C:\Series\Show\Season 2\Show - 01.mkv"), file(12, "C:/Series/Show/Season 1/Show - 14-15.mkv")];
        let eps = vec![ep(1, 1, 13, 10), ep(2, 2, 1, 11), ep(3, 1, 14, 12), ep(4, 1, 15, 12)];
        let watched = chrono::DateTime::parse_from_rfc3339("2026-10-01T10:00:00Z").unwrap().timestamp();
        let want: HashMap<String, i64> = [crate::history::norm(r"c:\series\show\season 1\SHOW - 13.mkv"), crate::history::norm(r"C:\Series\Show\Season 1\Show - 14-15.mkv"), r"c:\elsewhere\x.mkv".to_string()].into_iter().map(|p| (p, watched)).collect();
        let plan = episode_plan(&files, &eps, &want);
        assert_eq!(plan.files.iter().map(|f| f.0).collect::<Vec<_>>(), vec![10, 12], "the watched file, not Sonarr's S2E1; a double episode is one file; an unknown path is skipped");
        assert_eq!(plan.episodes, vec![1, 3, 4]);
        assert_eq!(plan.names, vec!["S1 E13", "S1 E14", "S1 E15"]);

        // Deleted, then downloaded again to the same path after the watch.
        let again = vec![json!({ "id": 20, "path": r"C:\Series\Show\Season 1\Show - 13.mkv", "size": 1, "dateAdded": "2026-10-18T10:00:00Z" }), json!({ "id": 21, "path": r"C:\Series\Show\Season 1\Show - 14-15.mkv", "size": 1 })];
        assert!(episode_plan(&again, &eps, &want).files.is_empty(), "downloaded again after watching: kept; no date: kept");
    }

    #[test]
    fn a_season_pack_loses_only_its_watched_files() {
        // One torrent, ten files. E1-E3 watched; E2 was downloaded again after
        // the watch; E4-E10 not watched.
        let path = |e: i64| format!(r"C:\Series\Show\Season 1\Show S01E{e:02}.mkv");
        let files: Vec<Value> = (1..=10).map(|e| json!({ "id": 100 + e, "path": path(e), "size": 1_000_000_000u64, "dateAdded": if e == 2 { "2026-10-18T10:00:00Z" } else { "2026-09-01T10:00:00Z" } })).collect();
        let eps: Vec<Value> = (1..=10).map(|e| json!({ "id": e, "seasonNumber": 1, "episodeNumber": e, "episodeFileId": 100 + e, "monitored": true, "hasFile": true })).collect();
        let watched = chrono::DateTime::parse_from_rfc3339("2026-10-01T10:00:00Z").unwrap().timestamp();
        let want: HashMap<String, i64> = (1..=3).map(|e| (crate::history::norm(&path(e)), watched)).collect();
        let plan = episode_plan(&files, &eps, &want);
        assert_eq!(plan.files.iter().map(|f| f.0).collect::<Vec<_>>(), vec![101, 103], "E2 is a fresh copy");
        assert_eq!(plan.episodes, vec![1, 3], "only those are unmonitored: E2 and E4-E10 stay monitored");

        // After that clean-up the pack's torrent still has files in use: it stays.
        let grabs: Vec<Value> = (1..=10).map(|e| json!({ "eventType": "grabbed", "downloadId": "PACK", "episodeId": e })).collect();
        let after: Vec<Value> = (1..=10).map(|e| json!({ "id": e, "hasFile": !(e == 1 || e == 3), "monitored": !(e == 1 || e == 3) })).collect();
        assert!(finished_torrents(&grabs, &after).is_empty());
        // Every episode in it cleaned up: now it goes.
        let all: Vec<Value> = (1..=10).map(|e| json!({ "id": e, "hasFile": false, "monitored": false })).collect();
        assert_eq!(finished_torrents(&grabs, &all), vec!["pack"]);
        // Deleted by hand outside Chillarr (no file, still monitored): Sonarr
        // wants it back, so the seeding copy stays.
        let by_hand: Vec<Value> = (1..=10).map(|e| json!({ "id": e, "hasFile": false, "monitored": e == 5 })).collect();
        assert!(finished_torrents(&grabs, &by_hand).is_empty());
    }

    #[test]
    fn a_seeding_copy_inside_the_library_is_never_deleted() {
        let roots = vec![crate::waiting::norm_path(r"C:\Media\Series")];
        let t = |h: &str, path: &str| json!({ "hash": h, "content_path": path });
        let torrents = vec![t("AAA", r"C:\Users\x\Downloads\Show S01"), t("bbb", r"C:\Media\Series\Show\Season 1"), t("ccc", ""), t("ddd", r"C:\Users\x\Downloads\Other")];
        let doomed = vec!["aaa".to_string(), "bbb".to_string(), "ccc".to_string()];
        assert_eq!(outside_library(&doomed, &torrents, &roots), vec!["aaa"], "in the library or no path: kept; not asked for: kept");
    }

    #[test]
    fn a_seeding_copy_goes_once_all_its_episodes_are_cleaned_up() {
        let grab = |h: &str, e: i64| json!({ "eventType": "grabbed", "downloadId": h, "episodeId": e });
        let history = vec![grab("PACK", 1), grab("PACK", 2), grab("single", 3), json!({ "eventType": "downloadFolderImported", "downloadId": "x", "episodeId": 9 })];
        let ep = |id: i64, file: bool, mon: bool| json!({ "id": id, "hasFile": file, "monitored": mon });
        assert_eq!(finished_torrents(&history, &[ep(1, false, false), ep(2, true, true), ep(3, false, false)]), vec!["single"], "the pack still has E2");
        assert_eq!(finished_torrents(&history, &[ep(1, false, false), ep(2, false, false), ep(3, false, true)]), vec!["pack"], "E3 is wanted again (monitored): its copy stays");
    }
}
