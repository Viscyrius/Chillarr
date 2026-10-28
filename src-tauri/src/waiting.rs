//! The waiting list: what you asked for that isn't watchable yet, including
//! things that aren't out. Radarr and Sonarr grab a copy when a new one is
//! uploaded (they watch their indexers' new releases); this watches them so you
//! hear about it, keeps searching for what's out but not found yet (see
//! search_schedule), and keeps Jellyfin's library in step with what they imported.

use crate::library;
use crate::log::log;
use crate::media::{arr, jf_admin, Media};
use crate::model::{AppId, Level};
use crate::tiles::{self, Tile};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

/// Everything here is a local call to Radarr/Sonarr/Jellyfin, so this is cheap.
const TICK: Duration = Duration::from_secs(120);
const FIRST_CHECK: Duration = Duration::from_secs(45);
const TRIM_TICK: Duration = Duration::from_secs(20);

/// One title on the waiting list, with what Stop needs.
#[derive(Serialize, Clone, Debug)]
pub struct Waiting {
    pub tile: Tile,
    /// "radarr" or "sonarr".
    pub app: &'static str,
    pub id: i64,
}

/// What the last check saw, to notice changes.
#[derive(Default)]
struct Seen {
    /// Radarr id -> has a file.
    movies: HashMap<i64, bool>,
    /// Sonarr id -> episode files.
    series: HashMap<i64, i64>,
    /// Movies Jellyfin was last asked to scan for, so one it can't match isn't rescanned forever.
    scanned_for: HashSet<i64>,
    search: crate::search_schedule::Searcher,
}

fn progress_of(q: &Value, field: &str, id: i64) -> Option<f64> {
    q["records"].as_array()?.iter().find(|r| r[field].as_i64() == Some(id)).map(|r| {
        let size = r["size"].as_f64().unwrap_or(0.0);
        let left = r["sizeleft"].as_f64().unwrap_or(size);
        if size > 0.0 { (size - left) / size } else { 0.0 }
    })
}

fn sonarr_poster(s: &Value) -> Option<String> {
    s["images"].as_array().and_then(|i| i.iter().find(|x| x["coverType"] == "poster")).and_then(|x| x["remoteUrl"].as_str()).map(String::from)
}

/// Monitored titles with nothing to watch yet, newest first, with where each stands.
pub async fn list(app: &AppHandle) -> Vec<Waiting> {
    let mut out = Vec::new();
    if let Some(a) = arr(app, AppId::Radarr) {
        let q = a.get_one("queue?pageSize=200").await.unwrap_or_default();
        let mut movies = a.list("movie").await.unwrap_or_default();
        movies.sort_by(|x, y| y["added"].as_str().cmp(&x["added"].as_str()));
        for m in movies.iter().filter(|m| m["monitored"] == true && m["hasFile"] != true) {
            let id = m["id"].as_i64().unwrap_or(0);
            let p = progress_of(&q, "movieId", id);
            let mut t = tiles::from_radarr(m);
            t.status = Some(library::wanted_status(m["isAvailable"].as_bool().unwrap_or(true), false, p));
            t.progress = p;
            out.push(Waiting { tile: t, app: "radarr", id });
        }
    }
    if let Some(a) = arr(app, AppId::Sonarr) {
        let q = a.get_one("queue?pageSize=200").await.unwrap_or_default();
        // Shows with a wanted episode that has aired: "Looking for a copy" even
        // while the show is "upcoming", and "Not out yet" when only unaired
        // episodes were picked. If Sonarr doesn't answer, its show status decides.
        let aired: Option<HashSet<i64>> = a
            .get_one("wanted/missing?pageSize=2000&monitored=true")
            .await
            .ok()
            .and_then(|v| v["records"].as_array().map(|r| r.iter().filter_map(|e| e["seriesId"].as_i64()).collect()));
        for s in a.list("series").await.unwrap_or_default().iter().filter(|s| s["monitored"] == true) {
            let have = s["statistics"]["episodeFileCount"].as_i64().unwrap_or(0);
            if have > 0 {
                continue;
            }
            let id = s["id"].as_i64().unwrap_or(0);
            let p = progress_of(&q, "seriesId", id);
            let released = aired.as_ref().map_or(s["status"] != "upcoming", |set| set.contains(&id));
            out.push(Waiting {
                tile: Tile {
                    tvdb: s["tvdbId"].as_i64(),
                    year: s["year"].as_i64(),
                    poster: sonarr_poster(s),
                    overview: s["overview"].as_str().map(String::from),
                    status: Some(library::wanted_status(released, false, p)),
                    progress: p,
                    ..Tile::new(&format!("tvdb:{}", s["tvdbId"].as_i64().unwrap_or(0)), "series", s["title"].as_str().unwrap_or_default())
                },
                app: "sonarr",
                id,
            });
        }
    }
    out
}

fn which(name: &str) -> Result<AppId, String> {
    match name {
        "radarr" => Ok(AppId::Radarr),
        "sonarr" => Ok(AppId::Sonarr),
        _ => Err("Unknown list".into()),
    }
}

/// Stop waiting for a title on the waiting list (it has no files yet).
pub async fn stop(app: &AppHandle, name: &str, id: i64) -> Result<String, String> {
    let msg = stop_item(app, which(name)?, id, true).await?;
    log(app, Level::Ok, &msg);
    Ok(msg)
}

/// Cancels everything a title is downloading (the torrents leave qBittorrent,
/// half-done data included) and makes sure it doesn't start again: a title
/// with no files and `forget` set leaves Radarr/Sonarr; otherwise what was
/// downloading is no longer wanted, so the next search doesn't grab it again.
pub async fn stop_item(app: &AppHandle, owner: AppId, id: i64, forget: bool) -> Result<String, String> {
    let (kind, field) = if owner == AppId::Radarr { ("movie", "movieId") } else { ("series", "seriesId") };
    let a = arr(app, owner).ok_or_else(|| format!("{} isn't running", owner.label()))?;
    let item = a.get_one(&format!("{kind}/{id}")).await.map_err(|e| format!("{} couldn't find it: {e}", owner.label()))?;
    let title = item["title"].as_str().unwrap_or("it").to_string();
    let recs: Vec<Value> = a.get_one("queue?pageSize=2000").await.unwrap_or_default()["records"]
        .as_array()
        .map(|r| r.iter().filter(|x| x[field].as_i64() == Some(id)).cloned().collect())
        .unwrap_or_default();
    let grabbed = grabbed_hashes(&a, owner, id).await;
    remove_from_queue(&a, &recs).await?;
    delete_unfinished(app, &grabbed).await;
    // A missing field counts as "has files": the title is only forgotten when it surely has none.
    let files = if owner == AppId::Radarr { (item["hasFile"] != false) as i64 } else { item["statistics"]["episodeFileCount"].as_i64().unwrap_or(1) };
    if forget && files == 0 {
        a.delete(&format!("{kind}/{id}?deleteFiles=false&addImportExclusion=false")).await.map_err(|e| format!("Couldn't stop {title}: {e}"))?;
        return Ok(format!("Stopped {title}."));
    }
    unwant(&a, owner, id, &recs).await;
    Ok(if recs.is_empty() { format!("Stopped {title}.") } else { format!("Cancelled the download of {title}.") })
}

pub(crate) fn qbit(app: &AppHandle) -> Option<crate::qbit::Qbit> {
    app.state::<crate::fix::Core>().config(AppId::QBittorrent).filter(|c| !c.base_url.is_empty()).map(|c| crate::qbit::Qbit::new(&c.base_url))
}

/// Seeding: whether finished downloads keep sharing. Off means qBittorrent
/// stops each torrent the moment it finishes (ratio limit 0, then stop); the
/// file stays for Radarr/Sonarr to bring into the library.
pub async fn seeding(app: &AppHandle) -> Result<bool, String> {
    let q = qbit(app).ok_or("qBittorrent isn't set up")?;
    let p = q.prefs().await.map_err(|e| format!("{e:?}"))?;
    Ok(!(p["max_ratio_enabled"] == true && p["max_ratio"].as_f64().unwrap_or(-1.0) <= 0.0))
}

pub async fn set_seeding(app: &AppHandle, on: bool) -> Result<String, String> {
    let q = qbit(app).ok_or("qBittorrent isn't set up. Install it first.")?;
    let prefs = if on {
        json!({ "max_ratio_enabled": false, "max_seeding_time_enabled": false })
    } else {
        // 0 = stop the torrent (not remove it: Radarr/Sonarr still need to find the finished file).
        json!({ "max_ratio_enabled": true, "max_ratio": 0, "max_ratio_act": 0 })
    };
    q.set_prefs(&prefs).await.map_err(|e| match e {
        crate::qbit::QbError::Locked => "qBittorrent wants a password. Press Fix everything first.".to_string(),
        crate::qbit::QbError::Down => "qBittorrent isn't running. Press Fix everything first.".to_string(),
        crate::qbit::QbError::Other(m) => m,
    })?;
    let msg = if on { "Finished downloads keep sharing (seeding)." } else { "Finished downloads stop sharing and leave Downloads once they're in your library." }.to_string();
    log(app, Level::Ok, &msg);
    Ok(msg)
}

/// With seeding off, a finished torrent Radarr/Sonarr have imported leaves
/// qBittorrent with its files. Their own clean-up waits for the torrent to stop,
/// which never comes: Sonarr gives each torrent its indexer's seed ratio (1),
/// overriding the global "stop at 0" (One Piece, 2026-10-18). Library copies are
/// hardlinks, so they stay; episodes of a pack that weren't wanted go with it.
async fn tidy_imported(app: &AppHandle) {
    if !matches!(seeding(app).await, Ok(false)) {
        return;
    }
    let Some(q) = qbit(app) else { return };
    let Ok(all) = q.list().await else { return };
    // Watch now clears its own torrents (watchnow::tick), whatever the seeding setting.
    let watch_now = app.state::<crate::watchnow::WatchNow>().owned_set();
    // A torrent with no content_path can't be checked against the library, so it's left alone.
    let finished: Vec<(String, String, u64, String)> = all
        .iter()
        .filter(|t| t["progress"].as_f64().unwrap_or(0.0) >= 1.0)
        .filter(|t| t["hash"].as_str().is_none_or(|h| !watch_now.contains(&h.to_ascii_lowercase())))
        .filter_map(|t| {
            let path = t["content_path"].as_str().filter(|p| !p.is_empty())?.to_string();
            Some((t["hash"].as_str()?.to_ascii_lowercase(), t["name"].as_str().unwrap_or("a download").to_string(), t["size"].as_u64().unwrap_or(0), path))
        })
        .collect();
    if finished.is_empty() {
        return;
    }
    // Both apps must answer: a torrent still in either queue is mid-import.
    let mut apps = Vec::new();
    let mut queued: HashSet<String> = HashSet::new();
    let mut roots: Vec<String> = Vec::new();
    let mut sonarr_rows: Vec<Value> = Vec::new();
    for owner in [AppId::Radarr, AppId::Sonarr] {
        let Some(a) = arr(app, owner) else { continue };
        let Ok(v) = a.get_one("queue?pageSize=2000").await else { return };
        let Some(rows) = v["records"].as_array() else { return };
        queued.extend(rows.iter().filter_map(|r| r["downloadId"].as_str().map(|h| h.to_ascii_lowercase())));
        if owner == AppId::Sonarr {
            sonarr_rows = rows.clone();
        }
        let Ok(folders) = a.list("rootfolder").await else { return };
        roots.extend(folders.iter().filter_map(|f| f["path"].as_str().map(norm_path)));
        apps.push(a);
    }
    // Finished torrents not imported (added by hand, import dismissed) are looked at again every 30 minutes, not every tick.
    static NOT_IMPORTED: std::sync::Mutex<Option<HashMap<String, std::time::Instant>>> = std::sync::Mutex::new(None);
    let (mut cleared, mut bytes) = (Vec::new(), 0u64);
    let sonarr = arr(app, AppId::Sonarr);
    for (hash, name, size, path) in finished {
        // A torrent whose data sits inside the library would take library files with it.
        if inside_any(&path, &roots) {
            continue;
        }
        if queued.contains(&hash) {
            // A trimmed pack: Sonarr imported the picked episodes and keeps the
            // rest "missing" in its queue. Once every wanted one has its file,
            // Sonarr removes it (and its data) itself.
            if let Some(a) = &sonarr {
                if pack_done(a, &sonarr_rows, &hash).await {
                    let ids: Vec<i64> = sonarr_rows.iter().filter(|r| r["downloadId"].as_str().is_some_and(|d| d.eq_ignore_ascii_case(&hash))).filter_map(|r| r["id"].as_i64()).collect();
                    if a.delete_with("queue/bulk?removeFromClient=true&blocklist=false", &json!({ "ids": ids })).await.is_ok() {
                        cleared.push(name);
                        bytes += size;
                    }
                }
            }
            continue;
        }
        if NOT_IMPORTED.lock().unwrap().get_or_insert_with(HashMap::new).get(&hash).is_some_and(|at| at.elapsed() < Duration::from_secs(30 * 60)) {
            continue;
        }
        let mut imported = false;
        for a in &apps {
            let Ok(v) = a.get_one(&format!("history?downloadId={}&pageSize=200", hash.to_ascii_uppercase())).await else { continue };
            imported |= v["records"].as_array().is_some_and(|r| r.iter().any(|x| x["eventType"] == "downloadFolderImported"));
        }
        if !imported {
            NOT_IMPORTED.lock().unwrap().get_or_insert_with(HashMap::new).insert(hash, std::time::Instant::now());
            continue;
        }
        if q.delete(&[hash], true).await.is_ok() {
            cleared.push(name);
            bytes += size;
        }
    }
    match cleared.len() {
        0 => {}
        1 => log(app, Level::Ok, format!("Cleared {} from Downloads ({}): it's in your library now.", cleared[0], crate::storage::gb(bytes))),
        n => log(app, Level::Ok, format!("Cleared {n} finished downloads from Downloads ({}): they're in your library now.", crate::storage::gb(bytes))),
    }
}

/// A finished torrent Sonarr is holding with a warning, whose every episode is
/// now either on disk or not wanted (the files trim_packs skipped).
pub(crate) async fn pack_done(a: &crate::arr::Arr, rows: &[Value], hash: &str) -> bool {
    let mine: Vec<&Value> = rows.iter().filter(|r| r["downloadId"].as_str().is_some_and(|d| d.eq_ignore_ascii_case(hash))).collect();
    let Some(first) = mine.first() else { return false };
    // Only a download Sonarr has finished with; one still importing is left to it.
    let state = first["trackedDownloadState"].as_str().unwrap_or_default();
    if !matches!(state, "importBlocked" | "importFailed" | "imported") && first["trackedDownloadStatus"] != "warning" {
        return false;
    }
    let Some(series) = first["seriesId"].as_i64() else { return false };
    let Ok(eps) = a.list(&format!("episode?seriesId={series}")).await else { return false };
    let ids: HashSet<i64> = mine.iter().filter_map(|r| r["episodeId"].as_i64()).collect();
    let mut imported_any = false;
    for e in eps.iter().filter(|e| e["id"].as_i64().is_some_and(|i| ids.contains(&i))) {
        if e["hasFile"] == true {
            imported_any = true;
        } else if e["monitored"] == true {
            return false;
        }
    }
    imported_any
}

/// Lower case, forward slashes, no trailing slash: for comparing Windows paths.
pub(crate) fn norm_path(p: &str) -> String {
    p.replace('\\', "/").trim_end_matches('/').to_ascii_lowercase()
}

/// Whether `path` is one of `roots` or inside one.
pub(crate) fn inside_any(path: &str, roots: &[String]) -> bool {
    let p = norm_path(path);
    roots.iter().any(|r| !r.is_empty() && (p == *r || p.starts_with(&format!("{r}/"))))
}

/// Season packs Sonarr grabs for a few episodes: the files of episodes nobody
/// asked for are set to "don't download". You picked one episode and a
/// 16-episode pack came down whole (One Piece, 2026-10-18). Only a pack whose
/// file names list exactly the episodes Sonarr says it holds is touched (anime
/// releases often number differently), never all of its files, and a file
/// picked later is switched back on while the pack is still downloading.
async fn trim_packs(app: &AppHandle) {
    // Finished, or not readable: never looked at again.
    static DONE: std::sync::Mutex<Option<HashSet<String>>> = std::sync::Mutex::new(None);
    let Some(q) = qbit(app) else { return };
    let Some(a) = arr(app, AppId::Sonarr) else { return };
    let Ok(v) = a.get_one("queue?pageSize=2000").await else { return };
    // downloadId -> (series, Sonarr's episode ids for the release)
    let mut torrents: HashMap<String, (i64, HashSet<i64>)> = HashMap::new();
    for r in v["records"].as_array().into_iter().flatten() {
        if let (Some(h), Some(s)) = (r["downloadId"].as_str(), r["seriesId"].as_i64()) {
            let e = torrents.entry(h.to_ascii_lowercase()).or_insert((s, HashSet::new()));
            e.1.extend(r["episodeId"].as_i64());
        }
    }
    // (TVDB id, episode) of every Watch now episode, any profile.
    let watch_now: Vec<(i64, (i64, i64))> = app
        .state::<crate::watchnow::WatchNow>()
        .all()
        .into_iter()
        .filter_map(|(_, e)| Some((e.tvdb?, (e.season?, e.episode?))))
        .collect();
    let done = |h: &str| DONE.lock().unwrap().get_or_insert_with(HashSet::new).insert(h.to_string());
    // Watch now sets its own packs' files from what every profile and every
    // Download wants (watchnow::pack_wants); two owners would undo each other.
    let watch_now_owns = app.state::<crate::watchnow::WatchNow>().owned_set();
    for (hash, (series, ep_ids)) in torrents.into_iter().filter(|(h, _)| !watch_now_owns.contains(h)) {
        if DONE.lock().unwrap().get_or_insert_with(HashSet::new).contains(&hash) {
            continue;
        }
        let Ok(Some(t)) = q.torrent(&hash).await else { continue };
        if t["progress"].as_f64().unwrap_or(1.0) >= 1.0 {
            done(&hash);
            continue;
        }
        let Ok(files) = q.files(&hash).await else { continue };
        // Torrent metadata not in yet: no file list to look at.
        if files.is_empty() {
            continue;
        }
        let Ok(eps) = a.list(&format!("episode?seriesId={series}")).await else { continue };
        let se = |e: &Value| Some((e["seasonNumber"].as_i64()?, e["episodeNumber"].as_i64()?));
        let release: HashSet<(i64, i64)> = eps.iter().filter(|e| e["id"].as_i64().is_some_and(|i| ep_ids.contains(&i))).filter_map(se).collect();
        let mut wanted: Vec<(i64, i64)> = eps.iter().filter(|e| e["monitored"] == true && e["hasFile"] != true).filter_map(se).collect();
        // A Watch now episode is kept even if Sonarr no longer wants it.
        if !watch_now.is_empty() {
            let tvdb = a.get_one(&format!("series/{series}")).await.ok().and_then(|s| s["tvdbId"].as_i64());
            wanted.extend(watch_now.iter().filter(|(t, _)| Some(*t) == tvdb).map(|(_, se)| *se));
        }
        let wanted_set: HashSet<(i64, i64)> = wanted.iter().copied().collect();
        let Some((_, restore)) = plan_files(&files, &release, &wanted_set) else {
            // Hands off for good, unless files were skipped earlier: then a later pick may still need them back.
            if files.iter().all(|f| f["priority"].as_i64() != Some(0)) {
                done(&hash);
            }
            continue;
        };
        let skip = files_to_skip(&files, &release, &wanted);
        let name = t["name"].as_str().unwrap_or("a download");
        if !restore.is_empty() && q.set_file_prio(&hash, &restore, 1).await.is_ok() {
            log(app, Level::Ok, format!("Getting {} more episode files in {name}.", restore.len()));
        }
        if !skip.is_empty() && q.set_file_prio(&hash, &skip, 0).await.is_ok() {
            log(app, Level::Ok, format!("Skipping {} episode files you didn't pick in {name}.", skip.len()));
        }
    }
}

const VIDEO: [&str; 6] = ["mkv", "mp4", "avi", "m4v", "ts", "webm"];

/// Samples and anime extras (clean openings and endings): not episodes, so neither kept nor skipped.
fn is_extra(path: &str) -> bool {
    let p = path.to_ascii_lowercase().replace('\\', "/");
    p.split('/').rev().skip(1).any(|dir| matches!(dir, "sample" | "samples" | "extras" | "extra" | "featurettes"))
        || ["sample", "ncop", "nced", "nc op", "nc ed", "creditless"].iter().any(|w| p.rsplit('/').next().unwrap_or(&p).contains(w))
}

/// Which files to switch off (episodes not wanted) and back on (wanted again).
/// None means hands off: a video file whose name doesn't give its episode, file
/// names that don't list the same episodes Sonarr says the release holds, or
/// nothing in it wanted at all.
fn plan_files(files: &[Value], release: &HashSet<(i64, i64)>, wanted: &HashSet<(i64, i64)>) -> Option<(Vec<i64>, Vec<i64>)> {
    let mut parsed: Vec<(i64, i64, Vec<(i64, i64)>)> = Vec::new();
    for (i, f) in files.iter().enumerate() {
        let name = f["name"].as_str().unwrap_or_default();
        let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
        if !VIDEO.contains(&ext.as_str()) || is_extra(name) {
            continue;
        }
        let file = name.rsplit(['/', '\\']).next().unwrap_or(name);
        parsed.push((f["index"].as_i64().unwrap_or(i as i64), f["priority"].as_i64().unwrap_or(1), episodes_in(file)?));
    }
    let named: HashSet<(i64, i64)> = parsed.iter().flat_map(|(_, _, e)| e.iter().copied()).collect();
    if named.is_empty() || named != *release {
        return None;
    }
    let (mut skip, mut restore, mut keep) = (Vec::new(), Vec::new(), 0);
    for (id, prio, eps) in parsed {
        if eps.iter().any(|e| wanted.contains(e)) {
            keep += 1;
            if prio == 0 {
                restore.push(id);
            }
        } else if prio != 0 {
            skip.push(id);
        }
    }
    (keep > 0).then_some((skip, restore))
}

/// The pack files trim_packs switches off: plan_files' skip list, where
/// `wanted` is every episode still wanted (Sonarr's, plus each Watch now
/// entry's), so a file a Watch now entry wants is never in it.
fn files_to_skip(files: &[Value], release: &HashSet<(i64, i64)>, wanted: &[(i64, i64)]) -> Vec<i64> {
    plan_files(files, release, &wanted.iter().copied().collect()).map(|(skip, _)| skip).unwrap_or_default()
}

/// The (season, episode)s a file name gives as S01E09, S01E09E10 or S01E09-E10/-10.
fn episodes_in(name: &str) -> Option<Vec<(i64, i64)>> {
    let b = name.as_bytes();
    let digits = |from: usize| -> (i64, usize) {
        let mut end = from;
        while end < b.len() && b[end].is_ascii_digit() && end - from < 4 {
            end += 1;
        }
        (name[from..end].parse().unwrap_or(-1), end)
    };
    for i in 0..b.len() {
        let boundary = i == 0 || !b[i - 1].is_ascii_alphanumeric();
        if !boundary || !(b[i] == b'S' || b[i] == b's') || i + 1 >= b.len() || !b[i + 1].is_ascii_digit() {
            continue;
        }
        let (season, at) = digits(i + 1);
        if at >= b.len() || !(b[at] == b'E' || b[at] == b'e') || at + 1 >= b.len() || !b[at + 1].is_ascii_digit() {
            continue;
        }
        let (first, mut at) = digits(at + 1);
        let mut last = first;
        // A range or a run: E10E11, E10-E12, E10-12.
        loop {
            let mut next = at;
            if next < b.len() && b[next] == b'-' {
                next += 1;
            }
            if next < b.len() && (b[next] == b'E' || b[next] == b'e') {
                next += 1;
            }
            if next == at || next >= b.len() || !b[next].is_ascii_digit() {
                break;
            }
            let (n, end) = digits(next);
            if n <= last || n - first > 50 {
                break;
            }
            last = n;
            at = end;
        }
        if season < 0 || first < 0 {
            return None;
        }
        return Some((first..=last).map(|e| (season, e)).collect());
    }
    None
}

/// Downloads qBittorrent has that Radarr and Sonarr don't (left behind when a
/// title was removed, or added by hand). Fix everything names them.
pub async fn orphans(app: &AppHandle) -> Vec<String> {
    let Some(q) = qbit(app) else { return vec![] };
    let Ok(all) = q.list().await else { return vec![] };
    let mut known: Vec<String> = Vec::new();
    for (owner, path) in [(AppId::Radarr, "queue?pageSize=2000"), (AppId::Sonarr, "queue?pageSize=2000")] {
        if let Some(a) = arr(app, owner) {
            for r in a.get_one(path).await.unwrap_or_default()["records"].as_array().into_iter().flatten() {
                if let Some(h) = r["downloadId"].as_str() {
                    known.push(h.to_ascii_lowercase());
                }
            }
        }
    }
    all.iter()
        .filter(|t| t["progress"].as_f64().unwrap_or(0.0) < 1.0)
        .filter(|t| t["hash"].as_str().is_some_and(|h| !known.contains(&h.to_ascii_lowercase())))
        .filter_map(|t| t["name"].as_str().map(String::from))
        .collect()
}

/// Torrents Radarr/Sonarr ever grabbed for a title, from their history. The
/// queue can miss one (a title removed before its download showed up there);
/// the history doesn't.
async fn grabbed_hashes(a: &crate::arr::Arr, owner: AppId, id: i64) -> Vec<String> {
    let path = if owner == AppId::Radarr { format!("history/movie?movieId={id}") } else { format!("history/series?seriesId={id}") };
    let mut out: Vec<String> = a
        .list(&path)
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r["eventType"] == "grabbed")
        .filter_map(|r| r["downloadId"].as_str().map(|h| h.to_ascii_lowercase()))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Deletes those of `hashes` that are still downloading (finished ones may be
/// seeding a file in the library, so they stay).
async fn delete_unfinished(app: &AppHandle, hashes: &[String]) {
    let Some(q) = qbit(app) else { return };
    let Ok(all) = q.list().await else { return };
    let doomed: Vec<String> = all
        .iter()
        .filter(|t| t["progress"].as_f64().unwrap_or(0.0) < 1.0)
        .filter_map(|t| t["hash"].as_str().map(|h| h.to_ascii_lowercase()))
        .filter(|h| hashes.contains(h))
        .collect();
    let _ = q.delete(&doomed, true).await;
}

/// Takes queue rows out of Radarr/Sonarr and their torrents out of qBittorrent.
async fn remove_from_queue(a: &crate::arr::Arr, recs: &[Value]) -> Result<(), String> {
    let ids: Vec<i64> = recs.iter().filter_map(|r| r["id"].as_i64()).collect();
    if ids.is_empty() {
        return Ok(());
    }
    a.delete_with("queue/bulk?removeFromClient=true&blocklist=false", &json!({ "ids": ids })).await.map_err(|e| format!("Couldn't cancel the download: {e}"))
}

/// What a cancelled download was for stops being wanted: the movie, or just
/// those episodes, so Radarr/Sonarr don't fetch it straight back.
async fn unwant(a: &crate::arr::Arr, owner: AppId, id: i64, recs: &[Value]) {
    if owner == AppId::Radarr {
        if let Ok(mut m) = a.get_one(&format!("movie/{id}")).await {
            if m["hasFile"] != true {
                m["monitored"] = json!(false);
                let _ = a.update(&format!("movie/{id}"), &m).await;
            }
        }
        return;
    }
    let episodes: Vec<i64> = recs.iter().filter_map(|r| r["episodeId"].as_i64()).collect();
    if !episodes.is_empty() {
        let _ = a.update("episode/monitor", &json!({ "episodeIds": episodes, "monitored": false })).await;
    }
}

/// Deletes a movie or show from the library, files included. Radarr/Sonarr
/// do it when they have the title (so they don't fetch it back); otherwise
/// Jellyfin does. Then Jellyfin forgets it.
pub async fn delete_title(app: &AppHandle, tile: &Tile) -> Result<String, String> {
    let j = jf_admin(app).filter(|j| j.user_id.is_some());
    let (mut tmdb, mut tvdb) = (tile.tmdb, tile.tvdb);
    if let (Some(j), Some(id)) = (&j, &tile.jf_id) {
        if let Ok(v) = j.trailer_info(id).await {
            tmdb = tmdb.or_else(|| v["ProviderIds"]["Tmdb"].as_str()?.parse().ok());
            tvdb = tvdb.or_else(|| v["ProviderIds"]["Tvdb"].as_str()?.parse().ok());
        }
    }
    let movie = tile.kind == "movie" || tile.format.as_deref() == Some("MOVIE");
    let (owner, kind, field, want) = if movie { (AppId::Radarr, "movie", "tmdbId", tmdb) } else { (AppId::Sonarr, "series", "tvdbId", tvdb) };
    let mut done = false;
    if let (Some(a), Some(want)) = (arr(app, owner), want) {
        let hit = a.list(kind).await.unwrap_or_default().into_iter().find(|x| x[field].as_i64() == Some(want));
        if let Some(id) = hit.and_then(|x| x["id"].as_i64()) {
            // The seeding copy in Downloads is a hardlink to the library file:
            // left behind, the space never comes back. Read the history first,
            // since deleting the title deletes it.
            let grabbed = grabbed_hashes(&a, owner, id).await;
            a.delete(&format!("{kind}/{id}?deleteFiles=true&addImportExclusion=false")).await.map_err(|e| format!("{} couldn't delete it: {e}", owner.label()))?;
            if let (Some(q), false) = (qbit(app), grabbed.is_empty()) {
                // Only torrents outside the library: a pack used in place in a
                // root folder holds other titles' files too.
                let safe = crate::storage::safe_to_delete_files(app, &q, &grabbed).await;
                if !safe.is_empty() {
                    let _ = q.delete(&safe, true).await;
                }
            }
            done = true;
        }
    }
    if let Some(j) = &j {
        if !done {
            let id = tile.jf_id.as_deref().ok_or("Chillarr can't find this in Radarr, Sonarr or Jellyfin.")?;
            // Jellyfin takes the video; the subtitle files beside it go too.
            let video = crate::subfetch::video_path(j, id).await;
            j.delete_item(id).await.map_err(|e| match e {
                crate::jellyfin::JfError::Rejected => "Jellyfin won't let your account delete files. Allow it under Dashboard > Users > your user > Allow media deletion.".to_string(),
                e => format!("Jellyfin couldn't delete it: {e:?}"),
            })?;
            if let Some(v) = video {
                crate::subfetch::drop_sidecars(app, &v);
            }
        }
        let _ = j.scan().await;
    } else if !done {
        return Err("Chillarr can't find this in Radarr or Sonarr.".into());
    }
    crate::subfetch::forget_title(app, crate::subfetch::title_key(movie, tmdb, tvdb));
    let msg = format!("Deleted {}.", tile.title);
    log(app, Level::Ok, &msg);
    Ok(msg)
}

/// Deletes some episodes of a show from disk, through Sonarr, and stops
/// wanting them so they aren't fetched back. A file holding several episodes
/// goes when any of them is picked.
pub async fn delete_episodes(app: &AppHandle, tile: &Tile, episodes: &[(i64, i64)]) -> Result<String, String> {
    if episodes.is_empty() {
        return Err("Pick the episodes to delete first.".into());
    }
    let (a, lookup, _, _) = crate::media::series_lookup(app, tile).await?;
    let title = lookup["title"].as_str().unwrap_or(&tile.title).to_string();
    let id = lookup["id"].as_i64().filter(|i| *i > 0).ok_or_else(|| format!("Sonarr doesn't have {title}."))?;
    let eps = a.list(&format!("episode?seriesId={id}")).await.map_err(|e| format!("Sonarr couldn't list the episodes: {e}"))?;
    let picked: HashSet<(i64, i64)> = episodes.iter().copied().collect();
    let chosen: Vec<&Value> = eps
        .iter()
        .filter(|e| picked.contains(&(e["seasonNumber"].as_i64().unwrap_or(-1), e["episodeNumber"].as_i64().unwrap_or(-1))))
        .collect();
    let mut files: Vec<i64> = chosen.iter().filter_map(|e| e["episodeFileId"].as_i64().filter(|f| *f > 0)).collect();
    files.sort();
    files.dedup();
    if files.is_empty() {
        return Err("Those episodes aren't on your PC.".into());
    }
    // Every episode in those files goes (a double-episode file holds two), so
    // all of them stop being wanted, or Sonarr would fetch the partner back.
    let going: Vec<&Value> = eps.iter().filter(|e| e["episodeFileId"].as_i64().is_some_and(|f| files.contains(&f))).collect();
    let ids: Vec<i64> = going.iter().filter_map(|e| e["id"].as_i64()).collect();
    a.update("episode/monitor", &json!({ "episodeIds": ids, "monitored": false })).await.map_err(|e| format!("Sonarr couldn't mark them: {e}"))?;
    let mut gone = 0;
    for f in &files {
        if crate::subfetch::delete_episode_file(app, &a, *f).await {
            gone += 1;
        }
    }
    // The seeding copy of a torrent all of whose episodes are now gone.
    crate::storage::drop_seeding_copies(app, &a, id).await;
    if let Some(j) = jf_admin(app).filter(|j| j.user_id.is_some()) {
        let _ = j.scan().await;
    }
    if gone == 0 {
        return Err(format!("Sonarr couldn't delete the files of {title}."));
    }
    let n = going.len();
    let extra = n.saturating_sub(chosen.iter().filter(|e| e["hasFile"] == true).count());
    let msg = if extra > 0 {
        format!("Deleted {n} episodes of {title} ({extra} shared a file with one you picked).")
    } else {
        format!("Deleted {n} episode{} of {title}.", if n == 1 { "" } else { "s" })
    };
    log(app, Level::Ok, &msg);
    Ok(msg)
}

/// One download under way: a torrent, which for a season pack covers many episodes.
#[derive(Serialize, Clone, Debug)]
pub struct Download {
    pub tile: Tile,
    pub app: &'static str,
    /// Radarr/Sonarr's id for the download (the torrent hash).
    pub download_id: String,
}

/// "Downloading now" without Watch now's own torrents: those show under
/// My downloads > Watch now, with Remove, and nowhere else.
pub fn without_watch_now(list: Vec<Download>, owned: &HashSet<String>) -> Vec<Download> {
    list.into_iter().filter(|d| !owned.contains(&d.download_id.to_ascii_lowercase())).collect()
}

/// What Radarr and Sonarr are downloading, one entry per torrent (Watch
/// now's own left out, see `without_watch_now`).
pub async fn downloads(app: &AppHandle) -> Vec<Download> {
    let mut out: Vec<Download> = Vec::new();
    for (owner, name, q) in [(AppId::Radarr, "radarr", "queue?includeMovie=true&pageSize=2000"), (AppId::Sonarr, "sonarr", "queue?includeSeries=true&includeEpisode=true&pageSize=2000")] {
        let Some(a) = arr(app, owner) else { continue };
        let Ok(v) = a.get_one(q).await else { continue };
        for r in v["records"].as_array().into_iter().flatten() {
            let did = r["downloadId"].as_str().map(String::from).unwrap_or_else(|| format!("q{}", r["id"].as_i64().unwrap_or(0)));
            if let Some(d) = out.iter_mut().find(|d| d.download_id == did) {
                // A season pack: say how many episodes it holds instead of naming one.
                let n = d.tile.subtitle.as_deref().and_then(|s| s.strip_suffix(" episodes")).and_then(|n| n.parse::<i64>().ok()).unwrap_or(1) + 1;
                d.tile.subtitle = Some(format!("{n} episodes"));
                continue;
            }
            let mut tile = library::from_queue(r);
            tile.key = format!("dl:{did}");
            out.push(Download { tile, app: name, download_id: did });
        }
    }
    // Anything else qBittorrent is downloading: added by hand, or left behind
    // when its title was removed. Shown so it can be stopped from here too.
    if let Some(q) = qbit(app) {
        for t in q.list().await.unwrap_or_default() {
            let Some(hash) = t["hash"].as_str().map(|h| h.to_ascii_lowercase()) else { continue };
            let progress = t["progress"].as_f64().unwrap_or(0.0);
            if progress >= 1.0 || out.iter().any(|d| d.download_id.eq_ignore_ascii_case(&hash)) {
                continue;
            }
            let state = t["state"].as_str().unwrap_or_default();
            let pct = (progress * 100.0).round() as i64;
            let status = if state.starts_with("paused") || state.starts_with("stopped") {
                format!("Paused · {pct}%")
            } else if state.contains("stalled") || state == "metaDL" {
                format!("Waiting for people to share · {pct}%")
            } else {
                format!("Downloading · {pct}% · {}", crate::qbit::speed(t["dlspeed"].as_i64().unwrap_or(0)))
            };
            let tile = Tile {
                progress: Some(progress),
                status: Some(status),
                subtitle: Some("Not in Radarr or Sonarr".into()),
                ..Tile::new(&format!("dl:{hash}"), "movie", t["name"].as_str().unwrap_or("Download"))
            };
            out.push(Download { tile, app: "qbit", download_id: hash });
        }
    }
    without_watch_now(out, &app.state::<crate::watchnow::WatchNow>().owned_set())
}

/// Cancels one torrent: out of qBittorrent, and what it was for stops being
/// wanted. A title left with nothing on disk and nothing else downloading
/// leaves Radarr/Sonarr altogether, so it doesn't sit on the waiting list and
/// get fetched again.
pub async fn stop_download(app: &AppHandle, name: &str, download_id: &str) -> Result<String, String> {
    // The torrent can be one Radarr/Sonarr grabbed but hasn't queued (or has
    // already let go of): find its owner from the queue, then the history.
    let mut found: Option<(AppId, crate::arr::Arr, Vec<Value>, i64)> = None;
    let owners: Vec<AppId> = match name {
        "qbit" => vec![AppId::Radarr, AppId::Sonarr],
        n => vec![which(n)?],
    };
    for owner in owners {
        let Some(a) = arr(app, owner) else {
            // Cancelling behind its back would leave the title wanted, and it'd be grabbed again.
            if name != "qbit" {
                return Err(format!("{} isn't running", owner.label()));
            }
            continue;
        };
        let field = if owner == AppId::Radarr { "movieId" } else { "seriesId" };
        let recs: Vec<Value> = a.get_one("queue?pageSize=2000").await.unwrap_or_default()["records"]
            .as_array()
            .map(|r| {
                r.iter()
                    .filter(|x| x["downloadId"].as_str().is_some_and(|d| d.eq_ignore_ascii_case(download_id)) || format!("q{}", x["id"].as_i64().unwrap_or(0)) == download_id)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let (id, recs) = match recs.first().and_then(|r| r[field].as_i64()) {
            Some(id) => (Some(id), recs),
            // Not queued: the history's grab records name the title and (for a
            // show) each episode, which stand in for queue rows below.
            None => match grabbed_by(&a, field, download_id).await {
                Some((id, grabs)) => (Some(id), grabs),
                None => (None, recs),
            },
        };
        if let Some(id) = id {
            found = Some((owner, a, recs, id));
            break;
        }
    }
    let Some((owner, a, recs, id)) = found else {
        // Nobody asked for it: just a torrent in qBittorrent.
        let q = qbit(app).ok_or("qBittorrent isn't set up")?;
        q.delete(&[download_id.to_ascii_lowercase()], true).await.map_err(|e| format!("qBittorrent couldn't remove it: {e:?}"))?;
        let msg = "Cancelled that download and deleted what it had.".to_string();
        log(app, Level::Ok, &msg);
        return Ok(msg);
    };
    remove_from_queue(&a, &recs).await?;
    // Removing a queue row takes the torrent with it; one that wasn't queued goes here.
    if let Some(q) = qbit(app) {
        let _ = q.delete(&[download_id.to_ascii_lowercase()], true).await;
    }
    let (kind, field) = if owner == AppId::Radarr { ("movie", "movieId") } else { ("series", "seriesId") };
    let item = a.get_one(&format!("{kind}/{id}")).await.unwrap_or_default();
    let title = item["title"].as_str().unwrap_or("it").to_string();
    if nothing_else_wanted(&a, owner, id, &item, &recs, download_id, field).await {
        a.delete(&format!("{kind}/{id}?deleteFiles=false&addImportExclusion=false")).await.map_err(|e| format!("Cancelled it, but couldn't take {title} off the waiting list: {e}"))?;
    } else {
        unwant(&a, owner, id, &recs).await;
    }
    let msg = format!("Cancelled the download of {title}.");
    log(app, Level::Ok, &msg);
    Ok(msg)
}

/// Whether the cancelled torrent was all a title was for: nothing on disk,
/// nothing else downloading and (for a show) no other wanted episode still to
/// find. Anything unknown (a field missing, a request failing) counts as "no",
/// so a title is only ever kept by mistake, never dropped by one.
async fn nothing_else_wanted(a: &crate::arr::Arr, owner: AppId, id: i64, item: &Value, recs: &[Value], download_id: &str, field: &str) -> bool {
    let no_files = if owner == AppId::Radarr { item["hasFile"] == false } else { item["statistics"]["episodeFileCount"].as_i64() == Some(0) };
    if !no_files {
        return false;
    }
    let Ok(queue) = a.get_one("queue?pageSize=2000").await else { return false };
    let Some(rows) = queue["records"].as_array() else { return false };
    if rows.iter().any(|x| x[field].as_i64() == Some(id) && !x["downloadId"].as_str().is_some_and(|d| d.eq_ignore_ascii_case(download_id))) {
        return false;
    }
    if owner == AppId::Radarr {
        return true;
    }
    let Ok(eps) = a.list(&format!("episode?seriesId={id}")).await else { return false };
    let cancelled: HashSet<i64> = recs.iter().filter_map(|r| r["episodeId"].as_i64()).collect();
    !others_wanted(&eps, &cancelled)
}

/// A monitored episode without a file that this torrent wasn't for.
fn others_wanted(eps: &[Value], cancelled: &HashSet<i64>) -> bool {
    eps.iter().any(|e| e["monitored"] == true && e["hasFile"] != true && e["id"].as_i64().is_some_and(|i| !cancelled.contains(&i)))
}

/// The movie or show (by `field`, "movieId" or "seriesId") a torrent was
/// grabbed for, and its grab records (with `episodeId` for a show), from the history.
async fn grabbed_by(a: &crate::arr::Arr, field: &str, download_id: &str) -> Option<(i64, Vec<Value>)> {
    // Radarr/Sonarr keep the hash in upper case; qBittorrent hands it out in lower.
    for hash in [download_id.to_ascii_uppercase(), download_id.to_string()] {
        let Ok(v) = a.get_one(&format!("history?downloadId={hash}&pageSize=200")).await else { continue };
        let grabs: Vec<Value> = v["records"].as_array().into_iter().flatten().filter(|x| x["eventType"] == "grabbed").cloned().collect();
        if let Some(id) = grabs.iter().find_map(|x| x[field].as_i64().filter(|i| *i > 0)) {
            // History rows have no queue id, so remove_from_queue skips them.
            let grabs = grabs.into_iter().map(|mut g| { if let Some(o) = g.as_object_mut() { o.remove("id"); } g }).collect();
            return Some((id, grabs));
        }
    }
    None
}

/// Has Radarr and Sonarr tell Jellyfin the moment they import something, so
/// it shows up in seconds rather than at the next check. Adds the connection
/// only where there's none; an existing one is left alone.
async fn link_jellyfin(app: &AppHandle) {
    let Some(j) = jf_admin(app).filter(|j| j.user_id.is_some()) else { return };
    let mut key: Option<String> = None;
    for (owner, name) in [(AppId::Radarr, "Radarr"), (AppId::Sonarr, "Sonarr")] {
        let Some(a) = arr(app, owner) else { continue };
        let Ok(existing) = a.list("notification").await else { continue };
        if existing.iter().any(|n| n["implementation"] == "MediaBrowser") {
            continue;
        }
        let Some(mut n) = a.list("notification/schema").await.unwrap_or_default().into_iter().find(|n| n["implementation"] == "MediaBrowser") else { continue };
        if key.is_none() {
            // A Jellyfin API key just for this (Dashboard > API Keys shows it as "Chillarr").
            let find = |v: &Value| v["Items"].as_array().and_then(|i| i.iter().find(|k| k["AppName"] == "Chillarr")).and_then(|k| k["AccessToken"].as_str()).map(String::from);
            key = j.get_json("/Auth/Keys").await.ok().as_ref().and_then(find);
            if key.is_none() && j.post_json("/Auth/Keys?app=Chillarr", &json!({})).await.is_ok() {
                key = j.get_json("/Auth/Keys").await.ok().as_ref().and_then(find);
            }
        }
        let Some(k) = key.clone() else { return };
        let port: i64 = j.base.rsplit(':').next().and_then(|p| p.trim_end_matches('/').parse().ok()).unwrap_or(8096);
        for f in n["fields"].as_array_mut().into_iter().flatten() {
            match f["name"].as_str() {
                Some("host") => f["value"] = json!("localhost"),
                Some("port") => f["value"] = json!(port),
                Some("apiKey") => f["value"] = json!(k),
                Some("updateLibrary") => f["value"] = json!(true),
                Some("notify") => f["value"] = json!(false),
                _ => {}
            }
        }
        // Library changes only: imports, upgrades, renames and deletes.
        let wanted = ["onDownload", "onUpgrade", "onRename", "onMovieDelete", "onMovieFileDelete", "onMovieFileDeleteForUpgrade", "onSeriesDelete", "onEpisodeFileDelete", "onEpisodeFileDeleteForUpgrade"];
        if let Some(obj) = n.as_object_mut() {
            let keys: Vec<String> = obj.keys().filter(|k| k.starts_with("on")).cloned().collect();
            for k in keys {
                let supported = obj.get(&format!("supports{}", &k[..1].to_uppercase().chars().chain(k[1..].chars()).collect::<String>())).map(|v| v != &json!(false)).unwrap_or(true);
                obj.insert(k.clone(), json!(wanted.contains(&k.as_str()) && supported));
            }
            obj.insert("name".into(), json!("Jellyfin (Chillarr)"));
        }
        match a.create("notification", &n).await {
            Ok(_) => log(app, Level::Ok, format!("{name} now tells Jellyfin about new downloads right away.")),
            Err(e) => log(app, Level::Amber, format!("Couldn't connect {name} to Jellyfin: {e}")),
        }
    }
}

pub fn spawn(app: AppHandle) {
    // Packs are trimmed early, before much of what wasn't asked for comes in.
    let trim = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(TRIM_TICK).await;
            trim_packs(&trim).await;
        }
    });
    tauri::async_runtime::spawn(async move {
        let mut seen = Seen::default();
        tokio::time::sleep(FIRST_CHECK).await;
        link_jellyfin(&app).await;
        for n in 0u64.. {
            // Every 10 minutes: finished downloads Radarr never brought in
            // (it missed them, or Chillarr was closed) get imported now.
            if n % 5 == 4 {
                let mut rep = crate::fix::FixReport::default();
                crate::fix::import_finished_step(&app, &mut rep).await;
            }
            check(&app, &mut seen).await;
            crate::history::sync(&app).await;
            crate::transcodes::keep_tidy(&app).await;
            crate::trickplay::ensure(&app).await;
            tidy_imported(&app).await;
            crate::watchnow::tick(&app).await;
            tokio::time::sleep(TICK).await;
        }
    });
}

async fn check(app: &AppHandle, seen: &mut Seen) {
    let first = seen.movies.is_empty() && seen.series.is_empty();
    let mut landed: Vec<String> = Vec::new();
    let mut scan = false;
    let mut radarr_files: HashSet<i64> = HashSet::new();
    let following = |owner: AppId, id: i64| app.state::<Media>().is_following(owner, id);

    let mut movie_list: Option<Vec<Value>> = None;
    let mut series_list: Option<Vec<Value>> = None;
    if let Some(a) = arr(app, AppId::Radarr) {
        if let Ok(movies) = a.list("movie").await {
            for m in &movies {
                let Some(id) = m["id"].as_i64() else { continue };
                let file = m["hasFile"] == true;
                let title = m["title"].as_str().unwrap_or_default();
                if file {
                    if let Some(tmdb) = m["tmdbId"].as_i64() {
                        radarr_files.insert(tmdb);
                    }
                }
                if let Some(&had_file) = seen.movies.get(&id) {
                    if file && !had_file {
                        scan = true;
                        if !following(AppId::Radarr, id) {
                            landed.push(format!("{title} is ready to watch."));
                        }
                    }
                }
                seen.movies.insert(id, file);
            }
            movie_list = Some(movies);
        }
    }
    if let Some(a) = arr(app, AppId::Sonarr) {
        if let Ok(series) = a.list("series").await {
            for s in &series {
                let Some(id) = s["id"].as_i64() else { continue };
                let files = s["statistics"]["episodeFileCount"].as_i64().unwrap_or(0);
                if let Some(&before) = seen.series.get(&id) {
                    if files > before && s["monitored"] == true {
                        scan = true;
                        let title = s["title"].as_str().unwrap_or_default();
                        if !following(AppId::Sonarr, id) {
                            landed.push(if before == 0 { format!("{title} is ready to watch.") } else { format!("New episodes of {title} are ready to watch.") });
                        }
                    }
                }
                seen.series.insert(id, files);
            }
            series_list = Some(series);
        }
    }
    // Searches for what's out with no copy yet, and says "out now" once per title.
    crate::search_schedule::tick(app, &mut seen.search, movie_list.as_deref(), series_list.as_deref()).await;

    // Movies Radarr imported that Jellyfin doesn't list yet (imported while
    // Chillarr was closed, or by Fix everything): have Jellyfin look again.
    if let Some(j) = jf_admin(app).filter(|j| j.user_id.is_some()) {
        if !radarr_files.is_empty() {
            if let Ok(items) = j.items("recursive=true&includeItemTypes=Movie&fields=ProviderIds&limit=10000").await {
                let in_jf: HashSet<i64> = items.iter().filter_map(|v| v["ProviderIds"]["Tmdb"].as_str()?.parse().ok()).collect();
                let missing: HashSet<i64> = radarr_files.difference(&in_jf).copied().collect();
                if !missing.is_empty() && missing != seen.scanned_for {
                    scan = true;
                }
                seen.scanned_for = missing;
            }
        }
        if scan {
            let _ = j.scan().await;
        }
    }
    // The first check after launch only learns where things stand.
    for msg in landed.iter().filter(|_| !first) {
        log(app, Level::Ok, msg);
        use tauri_plugin_notification::NotificationExt;
        let _ = app.notification().builder().title("Chillarr").body(msg).show();
    }
    if scan {
        // Jellyfin needs a moment to add it; then the screen redraws.
        let h = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(10)).await;
            let _ = h.emit("get-progress", json!({ "key": "", "refresh": true }));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_now_downloads_are_left_out_of_downloading_now() {
        let d = |id: &str| Download { tile: Tile::new(&format!("dl:{id}"), "movie", id), app: "radarr", download_id: id.into() };
        let owned: HashSet<String> = ["abc".to_string()].into();
        let got: Vec<String> = without_watch_now(vec![d("ABC"), d("def"), d("q12")], &owned).into_iter().map(|x| x.download_id).collect();
        assert_eq!(got, vec!["def", "q12"], "Watch now's own torrent shows only under Watch now, with Remove");
    }

    #[test]
    fn reads_episode_numbers_from_names() {
        assert_eq!(episodes_in("One Piece S01E09 MULTi 1080p.mkv"), Some(vec![(1, 9)]));
        assert_eq!(episodes_in("show.s02e03e04.720p.mkv"), Some(vec![(2, 3), (2, 4)]));
        assert_eq!(episodes_in("Show S01E05-E07.mkv"), Some(vec![(1, 5), (1, 6), (1, 7)]));
        assert_eq!(episodes_in("Show S01E05-07.mkv"), Some(vec![(1, 5), (1, 6), (1, 7)]));
        assert_eq!(episodes_in("One Piece - 1085 [1080p].mkv"), None);
        assert_eq!(episodes_in("Seasons.of.Love.S03E01.mkv"), Some(vec![(3, 1)]));
    }

    #[test]
    fn skips_only_unpicked_episode_files() {
        let f = |i: i64, name: &str, prio: i64| json!({ "index": i, "name": name, "priority": prio });
        let set = |v: &[(i64, i64)]| v.iter().copied().collect::<HashSet<(i64, i64)>>();
        let release = set(&[(1, 1), (1, 2), (1, 3)]);
        let files = vec![f(0, "Pack/One Piece S01E01.mkv", 1), f(1, "Pack/One Piece S01E02.mkv", 1), f(2, "Pack/One Piece S01E03.mkv", 1), f(3, "Pack/info.nfo", 1), f(4, "Pack/Sample/sample.mkv", 1), f(5, "Pack/One Piece NCOP1.mkv", 1)];
        assert_eq!(plan_files(&files, &release, &set(&[(1, 2)])), Some((vec![0, 2], vec![])));
        // Picked later: switched back on.
        let trimmed = vec![f(0, "Pack/One Piece S01E01.mkv", 0), f(1, "Pack/One Piece S01E02.mkv", 1), f(2, "Pack/One Piece S01E03.mkv", 0)];
        assert_eq!(plan_files(&trimmed, &release, &set(&[(1, 2), (1, 3)])), Some((vec![], vec![2])));
        // Nothing wanted in it: hands off rather than skip everything.
        assert_eq!(plan_files(&files, &release, &HashSet::new()), None);
        // A file it can't read: hands off.
        let odd = vec![f(0, "Pack/One Piece - 001.mkv", 1), f(1, "Pack/One Piece S01E02.mkv", 1)];
        assert_eq!(plan_files(&odd, &release, &set(&[(1, 2)])), None);
        // Names numbered differently from Sonarr (scene vs TVDB): hands off.
        let scene = vec![f(0, "Show S02E01.mkv", 1), f(1, "Show S02E02.mkv", 1)];
        assert_eq!(plan_files(&scene, &set(&[(1, 13), (1, 14)]), &set(&[(1, 14), (2, 1)])), None);
    }

    #[test]
    fn library_paths_are_recognised() {
        let roots = vec![norm_path("C:\\Users\\anime\\Desktop\\Desktop\\Series\\Anime\\")];
        assert!(inside_any("c:/users/anime/Desktop/Desktop/Series/Anime/One Piece/S01E01.mkv", &roots));
        assert!(inside_any("C:\\Users\\anime\\Desktop\\Desktop\\Series\\Anime", &roots));
        assert!(!inside_any("C:\\Users\\anime\\Downloads\\One Piece S01E01-16", &roots));
        assert!(!inside_any("C:\\Users\\anime\\Desktop\\Desktop\\Series\\Anime2\\x.mkv", &roots));
    }

    #[test]
    fn a_show_stays_while_other_episodes_are_wanted() {
        let ep = |id: i64, monitored: bool, has: bool| json!({ "id": id, "monitored": monitored, "hasFile": has });
        let cancelled: HashSet<i64> = [1, 2].into_iter().collect();
        // S2 wanted but not found yet: the show stays.
        assert!(others_wanted(&[ep(1, true, false), ep(2, true, false), ep(3, true, false)], &cancelled));
        // Only the cancelled ones were wanted: it can go.
        assert!(!others_wanted(&[ep(1, true, false), ep(2, true, false), ep(3, false, false)], &cancelled));
        // Downloaded or unmonitored episodes don't hold it.
        assert!(!others_wanted(&[ep(1, true, false), ep(4, true, true), ep(5, false, false)], &cancelled));
    }

    #[test]
    fn trim_packs_keeps_wanted_file() {
        let f = |i: i64, name: &str| json!({ "index": i, "name": name, "priority": 1 });
        let release: HashSet<(i64, i64)> = [(1, 1), (1, 2), (1, 3)].into_iter().collect();
        let files = vec![f(0, "Pack/Show S01E01.mkv"), f(1, "Pack/Show S01E02.mkv"), f(2, "Pack/Show S01E03.mkv")];
        // Sonarr wants E01; a Watch now entry wants E02.
        let skip = files_to_skip(&files, &release, &[(1, 1), (1, 2)]);
        assert_eq!(skip, vec![2]);
        assert!(!skip.contains(&1));
        // Only Watch now wants anything: its file still stays.
        assert_eq!(files_to_skip(&files, &release, &[(1, 2)]), vec![0, 2]);
        assert!(files_to_skip(&files, &release, &[]).is_empty());
    }
}
