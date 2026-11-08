//! Suwayomi-Server, Chillarr's manga and manhua downloader. It is part of
//! Chillarr the way Radarr is, only more so: Chillarr installs it (a pinned,
//! hash-checked zip with its own Java, the first time someone downloads a
//! manga), starts it hidden in its kill-on-close job, and sets every setting
//! on the command line at each start. No window, no tray icon, no web page
//! (webUIEnabled=false), only 127.0.0.1. The user never sees it.
//!
//! Its embedded Chromium (KCEF, a 268 MB first-start download) is off:
//! sources behind Cloudflare go through the FlareSolverr Chillarr already runs.
//!
//! Measured on v2.4.2366 (2026-11-08):
//! - Extension stores are added with `addExtensionStore(indexUrl)`; the old
//!   `extensionRepos` setting is accepted and ignored.
//! - Downloads land in `<downloadsPath>/mangas/<Source (EN)>/<Title>/<Scanlator>_<Chapter>.cbz`,
//!   deflate-packed, original page bytes.
//! - A source's first search hit is often another series (Webtoons gave
//!   Omniscient Reader for Solo Leveling, Asura gave Ragnarok), so every hit is
//!   matched against all of AniList's names. MangaDex often has no chapters
//!   for licensed series. Sources answer 522 when down: one never fails a search.

use crate::model::{AppConfig, AppId};
use crate::read_core::{self, Sample};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::{AppHandle, Manager};

pub const PORT: u16 = 4567;
pub const START_WAIT_SECS: u64 = 120;
const STORE: &str = "https://raw.githubusercontent.com/keiyoushi/extensions/repo/index.min.json";
/// Installed on first use. Chosen for page quality and reach (measured 2026-11-08):
/// MangaDex (scanlator uploads, many languages), Weeb Central (manga),
/// Asura Scans and Flame Comics (high-resolution manhwa), MangaFire (reach),
/// Webtoons.com (official).
/// Kun Manga and Mangago for manhua: on 2026-11-08 they had all four big
/// manhua tried (Tales of Demons and Gods, Martial Peak, Battle Through the
/// Heavens, Soul Land), where the first six missed most of Battle Through the
/// Heavens; Kun Manga had Soul Land at 3200 px.
const DEFAULT_SOURCES: [&str; 8] = [
    "eu.kanade.tachiyomi.extension.all.mangadex",
    "eu.kanade.tachiyomi.extension.en.weebcentral",
    "eu.kanade.tachiyomi.extension.en.asurascans",
    "eu.kanade.tachiyomi.extension.en.flamecomics",
    "eu.kanade.tachiyomi.extension.all.mangafire",
    "eu.kanade.tachiyomi.extension.all.webtoons",
    "eu.kanade.tachiyomi.extension.en.kunmangaonline",
    "eu.kanade.tachiyomi.extension.en.mangago",
];

/// The password Suwayomi was started with this time. Chillarr starts it, so a
/// fresh random one each start: no other program on the PC can drive it
/// (installing an extension runs code).
static PASSWORD: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
const USER: &str = "chillarr";

fn new_password() -> String {
    let mut b = [0u8; 24];
    getrandom::getrandom(&mut b).expect("random");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn authed(r: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    match PASSWORD.lock().unwrap().clone() {
        Some(p) => r.basic_auth(USER, Some(p)),
        None => r,
    }
}

/// A folder name Suwayomi made: no separators, no "..", not a drive.
pub fn plain_name(s: &str) -> bool {
    !s.is_empty() && !s.contains(['/', '\\', ':']) && s != "." && s != ".." && !s.starts_with("..")
}

/// (source folder, series folder) for a manga, as Suwayomi names them.
pub async fn folder_names(manga_id: i64) -> Result<(String, String), String> {
    let v = gql("query($id:Int!){manga(id:$id){title source{displayName}}}", json!({ "id": manga_id })).await?;
    let title = v["manga"]["title"].as_str().unwrap_or_default().to_string();
    let source = v["manga"]["source"]["displayName"].as_str().unwrap_or_default().to_string();
    if !plain_name(&source) || title.trim().is_empty() {
        return Err("The manga downloader gave an odd name for that copy. Pick another one.".into());
    }
    Ok((source, title))
}

/// Queues chapters and makes sure the downloader is running.
pub async fn enqueue(ids: &[i64]) -> Result<(), String> {
    gql("mutation($ids:[Int!]!){enqueueChapterDownloads(input:{ids:$ids}){clientMutationId}}", json!({ "ids": ids })).await?;
    gql("mutation{startDownloader(input:{}){clientMutationId}}", json!({})).await.map(|_| ())
}

pub fn base() -> String {
    #[cfg(test)]
    if let Ok(b) = std::env::var("CHILLARR_SUWA_BASE") {
        return b;
    }
    format!("http://127.0.0.1:{PORT}")
}

pub fn java_in(bundle: &Path) -> PathBuf {
    bundle.join("jre").join("bin").join("java.exe")
}

fn jar_for(java: &Path) -> Option<PathBuf> {
    // <bundle>\jre\bin\java.exe -> <bundle>\bin\Suwayomi-Server.jar
    let bundle = java.parent()?.parent()?.parent()?;
    Some(bundle.join("bin").join("Suwayomi-Server.jar")).filter(|p| p.is_file())
}

/// The unpacked bundle Chillarr installed, if any.
fn installed_java() -> Option<PathBuf> {
    let dir = crate::install::suwa_dir();
    let mut found: Vec<PathBuf> = std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("Suwayomi-Server-")))
        .map(|b| java_in(&b))
        .filter(|j| j.is_file() && jar_for(j).is_some())
        .collect();
    found.sort();
    found.pop()
}

/// Its data (library, extensions, settings, logs), apart from the program.
pub fn data_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join("Chillarr").join("Suwayomi-data")
}

pub fn config() -> AppConfig {
    let exe = installed_java();
    AppConfig {
        id: AppId::Suwayomi,
        base_url: base(),
        api_key: None,
        problem: exe.is_none().then(|| "Not installed".to_string()),
        exe,
        service: None,
        managed: true,
    }
}

/// Every setting, given at start so nothing in its own files can drift.
pub fn launch_args(app: &AppHandle, java: &Path) -> Result<Vec<String>, String> {
    let jar = jar_for(java).ok_or("The manga downloader's files are incomplete. Press Fix everything to get it again.")?;
    let comics = crate::read::comics_dir(app).ok_or("Pick a folder for reading in Settings > Library & storage first.")?;
    std::fs::create_dir_all(&comics).map_err(|e| format!("Couldn't make {}: {e}", comics.display()))?;
    let data = data_dir();
    std::fs::create_dir_all(&data).map_err(|e| format!("Couldn't make {}: {e}", data.display()))?;
    let flare = app.state::<crate::fix::Core>().config(AppId::FlareSolverr).map(|c| c.base_url).unwrap_or_else(|| "http://localhost:8191".into());
    let password = new_password();
    *PASSWORD.lock().unwrap() = Some(password.clone());
    Ok(args(&jar, &data, &comics, &flare, &password))
}

fn args(jar: &Path, data: &Path, comics: &Path, flare: &str, password: &str) -> Vec<String> {
    let p = |k: &str, v: &str| format!("-Dsuwayomi.tachidesk.config.server.{k}={v}");
    let mut a = vec!["-Xmx768m".to_string(), "-Dfile.encoding=UTF-8".to_string()];
    for (k, v) in [
        ("rootDir", data.to_string_lossy().to_string()),
        ("ip", "127.0.0.1".into()),
        ("port", PORT.to_string()),
        ("webUIEnabled", "false".into()),
        ("initialOpenInBrowserEnabled", "false".into()),
        ("systemTrayEnabled", "false".into()),
        ("kcefEnabled", "false".into()),
        ("flareSolverrEnabled", "true".into()),
        ("flareSolverrUrl", flare.to_string()),
        ("downloadsPath", comics.to_string_lossy().to_string()),
        ("downloadAsCbz", "true".into()),
        // Chillarr queues new chapters itself (read::fetch_new), one copy each:
        // Suwayomi's own would take every group's copy of each chapter.
        ("autoDownloadNewChapters", "false".into()),
        ("authMode", "BASIC_AUTH".into()),
        ("authUsername", USER.into()),
        ("authPassword", password.to_string()),
        ("excludeEntryWithUnreadChapters", "false".into()),
        ("autoDownloadNewChaptersLimit", "0".into()),
        ("excludeUnreadChapters", "false".into()),
        ("excludeNotStarted", "false".into()),
        ("excludeCompleted", "true".into()),
        ("globalUpdateInterval", "6".into()),
        ("maxLogFolderSize", "20mb".into()),
        ("webUIUpdateCheckInterval", "0".into()),
    ] {
        a.push(p(k, &v));
    }
    a.push("-jar".into());
    a.push(jar.to_string_lossy().to_string());
    a
}

fn http() -> reqwest::Client {
    reqwest::Client::builder().timeout(Duration::from_secs(120)).build().unwrap_or_default()
}

/// One GraphQL call. An error list from the server is an error here.
pub async fn gql(query: &str, vars: Value) -> Result<Value, String> {
    let r = authed(http().post(format!("{}/api/graphql", base())))
        .json(&json!({ "query": query, "variables": vars }))
        .send()
        .await
        .map_err(|e| if e.is_connect() { "The manga downloader isn't running.".to_string() } else { format!("The manga downloader didn't answer: {e}") })?;
    let v: Value = r.json().await.map_err(|e| format!("The manga downloader's answer wasn't readable: {e}"))?;
    if let Some(msg) = v["errors"].as_array().and_then(|e| e.first()).and_then(|e| e["message"].as_str()) {
        return Err(first_line(msg));
    }
    Ok(v["data"].clone())
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or(s).chars().take(200).collect()
}

/// Installed, running, answering and set up: everything a download needs.
/// Installs it the first time (one download, no Windows prompt).
pub async fn ready(app: &AppHandle) -> Result<(), String> {
    if crate::read::comics_dir(app).is_none() {
        return Err("Pick a folder for reading in Settings > Library & storage first.".into());
    }
    if installed_java().is_none() {
        crate::log::log(app, crate::model::Level::Ok, "Setting up the manga downloader (one time, about 340 MB).");
        let failed = crate::onboard::install_only(app, vec![AppId::Suwayomi]).await;
        if let Some((_, e)) = failed.first() {
            return Err(format!("Couldn't set up the manga downloader: {e}"));
        }
    }
    if !crate::fix::ensure_running(app, AppId::Suwayomi).await {
        return Err("The manga downloader didn't start. Try again in a minute.".into());
    }
    // The port opens a few seconds before GraphQL answers.
    for _ in 0..60 {
        if gql("{aboutServer{version}}", json!({})).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    set_up_sources().await
}

/// The extension store and the default sources. Cheap when already done.
async fn set_up_sources() -> Result<(), String> {
    let have = gql("{extensions(condition:{isInstalled:true}){nodes{pkgName}}}", json!({})).await?;
    let installed: Vec<String> = have["extensions"]["nodes"].as_array().map(|a| a.iter().filter_map(|e| e["pkgName"].as_str().map(String::from)).collect()).unwrap_or_default();
    if DEFAULT_SOURCES.iter().all(|p| installed.iter().any(|i| i == p)) {
        return Ok(());
    }
    // Adding a store twice is refused; that's fine.
    let _ = gql("mutation($u:String!){addExtensionStore(input:{indexUrl:$u}){clientMutationId}}", json!({ "u": STORE })).await;
    gql("mutation{fetchExtensions(input:{}){extensions{pkgName}}}", json!({})).await?;
    for p in DEFAULT_SOURCES.iter().filter(|p| !installed.iter().any(|i| i == *p)) {
        if let Err(e) = gql("mutation($id:String!){updateExtension(input:{id:$id,patch:{install:true}}){extension{isInstalled}}}", json!({ "id": p })).await {
            // One missing source isn't a failure: the others still search.
            eprintln!("suwayomi: couldn't install {p}: {e}");
        }
    }
    Ok(())
}

/// One source's copy of a series, as offered in the source picker.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Candidate {
    pub source_id: String,
    pub source: String,
    pub manga_id: i64,
    pub title: String,
    pub chapters: usize,
    /// Newest chapter number it has.
    pub latest: Option<f64>,
    /// read_core::quality of its sampled pages; 0 when they couldn't be read.
    pub quality: f64,
    /// Median sampled page width, shown as "1600 px".
    pub width: u32,
    /// Median bytes per pixel (WebP counted as JPEG would be): breaks near ties.
    #[serde(default)]
    pub density: f64,
}

/// Every installed source's copy of a series named any of `names`, best first.
/// Each source is asked at once; a slow or broken one is left out after 45 s.
pub async fn candidates(names: &[String]) -> Result<Vec<Candidate>, String> {
    let srcs = gql("{sources{nodes{id displayName lang}}}", json!({})).await?;
    let srcs: Vec<(String, String)> = srcs["sources"]["nodes"]
        .as_array()
        .map(|a| a.iter().filter(|s| matches!(s["lang"].as_str(), Some("en") | Some("all"))).filter_map(|s| Some((s["id"].as_str()?.to_string(), s["displayName"].as_str()?.to_string()))).collect())
        .unwrap_or_default();
    let mut set = tokio::task::JoinSet::new();
    for (id, name) in srcs {
        let names = names.to_vec();
        set.spawn(async move { tokio::time::timeout(Duration::from_secs(45), one_source(id, name, names)).await.ok().flatten() });
    }
    let mut out = Vec::new();
    while let Some(r) = set.join_next().await {
        if let Ok(Some(c)) = r {
            out.push(c);
        }
    }
    rank(&mut out);
    Ok(out)
}

/// Best first: among copies with (nearly) every chapter, the sharpest; a copy
/// missing many chapters goes after them, however sharp.
/// Qualities within 10% count as the same look; then the less squeezed wins.
pub fn rank(c: &mut [Candidate]) {
    use std::cmp::Ordering::Equal;
    // "Has it all" is about how far it goes, not how many rows: extras such
    // as 10.5 made MangaFire 225 to everyone else's 201 (same newest, 200).
    let newest = c.iter().filter_map(|x| x.latest).fold(0.0, f64::max);
    let most = c.iter().map(|x| x.chapters).max().unwrap_or(0) as f64;
    let full = |x: &Candidate| match x.latest {
        Some(l) if newest > 0.0 => l >= newest * 0.97 && x.chapters as f64 >= most * 0.6,
        _ => most > 0.0 && x.chapters as f64 >= most * 0.9,
    };
    let band = |q: f64| (q.max(1.0).ln() / 1.1f64.ln()).floor() as i64;
    c.sort_by(|a, b| {
        full(b)
            .cmp(&full(a))
            .then(band(b.quality).cmp(&band(a.quality)))
            .then(b.density.partial_cmp(&a.density).unwrap_or(Equal))
            .then(b.chapters.cmp(&a.chapters))
    });
}

/// One copy of each chapter. Sources such as MangaFire list a chapter once
/// per group that translated it (449 rows for Solo Leveling's 201, measured
/// 2026-11-08); following all of them downloads everything twice. Kept: a
/// downloaded copy, else the series' most common group's, else the newest.
/// Chapters with no number are all kept.
pub fn one_per_chapter(chs: Vec<Chapter>) -> Vec<Chapter> {
    let mut counts: std::collections::HashMap<Option<String>, usize> = Default::default();
    for c in &chs {
        *counts.entry(c.scanlator.clone()).or_default() += 1;
    }
    let main = counts.into_iter().max_by_key(|(_, n)| *n).map(|(s, _)| s).flatten();
    let mut out: Vec<Chapter> = Vec::new();
    for c in chs {
        let Some(n) = c.number else {
            out.push(c);
            continue;
        };
        match out.iter_mut().find(|o| o.number == Some(n)) {
            None => out.push(c),
            Some(o) => {
                let score = |x: &Chapter| (x.downloaded, x.scanlator == main, x.uploaded.clone());
                if score(&c) > score(o) {
                    *o = c;
                }
            }
        }
    }
    out
}

async fn one_source(source_id: String, source: String, names: Vec<String>) -> Option<Candidate> {
    // The first name is the most common (English, else romaji); a second try
    // with another name catches sources that list the original title.
    for q in names.iter().take(2) {
        let found = gql(
            "mutation($s:LongString!,$q:String){fetchSourceManga(input:{source:$s,type:SEARCH,page:1,query:$q}){mangas{id title}}}",
            json!({ "s": source_id, "q": q }),
        )
        .await
        .ok()?;
        let hit = found["fetchSourceManga"]["mangas"]
            .as_array()?
            .iter()
            .filter_map(|m| Some((m["id"].as_i64()?, m["title"].as_str()?.to_string())))
            .map(|(id, t)| (read_core::best_title_score(&t, &names), id, t))
            .filter(|(s, ..)| *s >= read_core::SAME_TITLE)
            .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let Some((_, manga_id, title)) = hit else { continue };
        let got = gql("mutation($id:Int!){fetchMangaAndChapters(input:{id:$id,fetchManga:true,fetchChapters:true}){chapters{id chapterNumber}}}", json!({ "id": manga_id })).await.ok()?;
        let mut chs: Vec<(i64, f64)> = got["fetchMangaAndChapters"]["chapters"].as_array()?.iter().filter_map(|c| Some((c["id"].as_i64()?, c["chapterNumber"].as_f64()?))).collect();
        // One per chapter number, for the count: duplicates aren't more chapters.
        chs.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        chs.dedup_by(|a, b| a.1 == b.1);
        if chs.is_empty() {
            return None;
        }
        let latest = chs.iter().map(|c| c.1).fold(None, |m: Option<f64>, n| Some(m.map_or(n, |m| m.max(n))));
        let samples = sample_pages(&chs).await;
        let mut widths: Vec<u32> = samples.iter().map(|s| s.w).collect();
        widths.sort();
        let mut dens: Vec<f64> = samples.iter().map(|s| s.bytes as f64 / (s.w as f64 * s.h as f64).max(1.0) * if s.webp { 1.6 } else { 1.0 }).collect();
        dens.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let density = dens.get(dens.len() / 2).copied().unwrap_or(0.0);
        return Some(Candidate { source_id, source, manga_id, title, chapters: chs.len(), latest, quality: read_core::quality(&samples), width: widths.get(widths.len() / 2).copied().unwrap_or(0), density });
    }
    None
}

/// Three pages from a chapter near the start (past any prologue).
async fn sample_pages(chs: &[(i64, f64)]) -> Vec<Sample> {
    let mut sorted = chs.to_vec();
    sorted.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    let Some(&(ch, _)) = sorted.get(sorted.len().min(6).saturating_sub(1)) else { return vec![] };
    let Ok(p) = gql("mutation($c:Int!){fetchChapterPages(input:{chapterId:$c}){pages}}", json!({ "c": ch })).await else { return vec![] };
    let urls: Vec<String> = p["fetchChapterPages"]["pages"].as_array().map(|a| a.iter().filter_map(|u| u.as_str().map(String::from)).collect()).unwrap_or_default();
    let mut out = Vec::new();
    for u in urls.iter().skip(1).take(3) {
        let Ok(r) = authed(http().get(format!("{}{u}", base()))).timeout(Duration::from_secs(30)).send().await else { continue };
        let webp = r.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|t| t.contains("webp"));
        let Ok(b) = r.bytes().await else { continue };
        if let Some((w, h)) = read_core::dims(&b) {
            out.push(Sample { w, h, bytes: b.len() as u64, webp: webp || b.starts_with(b"RIFF") });
        }
    }
    out
}

/// Follows a series: in the library (so new chapters come by themselves) and
/// every chapter from `from` on queued. Returns how many were queued.
pub async fn follow(manga_id: i64, from: Option<f64>) -> Result<usize, String> {
    gql("mutation($id:Int!){updateManga(input:{id:$id,patch:{inLibrary:true}}){manga{inLibrary}}}", json!({ "id": manga_id })).await?;
    let chs = chapters(manga_id).await?;
    let ids: Vec<i64> = chs.iter().filter(|c| !c.downloaded && from.is_none_or(|f| c.number.unwrap_or(f64::MAX) >= f)).map(|c| c.id).collect();
    if !ids.is_empty() {
        enqueue(&ids).await?;
    }
    Ok(ids.len())
}

/// Stops following: out of the library, its queued chapters dropped.
pub async fn unfollow(manga_id: i64) -> Result<(), String> {
    let chs = chapters(manga_id).await.unwrap_or_default();
    let queued: Vec<i64> = chs.iter().filter(|c| !c.downloaded).map(|c| c.id).collect();
    if !queued.is_empty() {
        let _ = gql("mutation($ids:[Int!]!){dequeueChapterDownloads(input:{ids:$ids}){clientMutationId}}", json!({ "ids": queued })).await;
    }
    gql("mutation($id:Int!){updateManga(input:{id:$id,patch:{inLibrary:false}}){manga{inLibrary}}}", json!({ "id": manga_id })).await.map(|_| ())
}

#[derive(Serialize, Clone, Debug)]
pub struct Chapter {
    pub id: i64,
    pub name: String,
    pub number: Option<f64>,
    pub scanlator: Option<String>,
    pub downloaded: bool,
    pub pages: i64,
    pub uploaded: Option<String>,
}

pub async fn chapters(manga_id: i64) -> Result<Vec<Chapter>, String> {
    let v = gql(
        "query($id:Int!){manga(id:$id){chapters{nodes{id name chapterNumber scanlator isDownloaded pageCount uploadDate}}}}",
        json!({ "id": manga_id }),
    )
    .await?;
    let mut out: Vec<Chapter> = v["manga"]["chapters"]["nodes"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|c| {
                    Some(Chapter {
                        id: c["id"].as_i64()?,
                        name: c["name"].as_str().unwrap_or_default().to_string(),
                        number: c["chapterNumber"].as_f64().filter(|n| *n >= 0.0),
                        scanlator: c["scanlator"].as_str().filter(|s| !s.is_empty()).map(String::from),
                        downloaded: c["isDownloaded"] == true,
                        pages: c["pageCount"].as_i64().unwrap_or(0),
                        uploaded: c["uploadDate"].as_str().map(String::from),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| a.number.unwrap_or(f64::MAX).partial_cmp(&b.number.unwrap_or(f64::MAX)).unwrap_or(std::cmp::Ordering::Equal));
    Ok(one_per_chapter(out))
}

/// What's downloading right now: (manga id, chapter id, 0..1).
pub async fn queue() -> Vec<(i64, i64, f64)> {
    let Ok(v) = gql("{downloadStatus{queue{progress chapter{id manga{id}}}}}", json!({})).await else { return vec![] };
    v["downloadStatus"]["queue"]
        .as_array()
        .map(|a| a.iter().filter_map(|q| Some((q["chapter"]["manga"]["id"].as_i64()?, q["chapter"]["id"].as_i64()?, q["progress"].as_f64().unwrap_or(0.0)))).collect())
        .unwrap_or_default()
}

/// Deletes downloaded chapters' files (they stay listed, so they can come back).
pub async fn delete_chapters(ids: &[i64]) -> Result<(), String> {
    if ids.is_empty() {
        return Ok(());
    }
    gql("mutation($ids:[Int!]!){deleteDownloadedChapters(input:{ids:$ids}){clientMutationId}}", json!({ "ids": ids })).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_args_keep_it_hidden_and_local() {
        let a = args(Path::new(r"C:\s\bin\Suwayomi-Server.jar"), Path::new(r"C:\d"), Path::new(r"D:\Reading\Comics"), "http://localhost:8191", "pw");
        let has = |s: &str| a.iter().any(|x| x == s);
        for want in [
            "-Dsuwayomi.tachidesk.config.server.ip=127.0.0.1",
            "-Dsuwayomi.tachidesk.config.server.webUIEnabled=false",
            "-Dsuwayomi.tachidesk.config.server.systemTrayEnabled=false",
            "-Dsuwayomi.tachidesk.config.server.initialOpenInBrowserEnabled=false",
            "-Dsuwayomi.tachidesk.config.server.kcefEnabled=false",
            "-Dsuwayomi.tachidesk.config.server.downloadAsCbz=true",
            r"-Dsuwayomi.tachidesk.config.server.downloadsPath=D:\Reading\Comics",
            "-Dsuwayomi.tachidesk.config.server.authMode=BASIC_AUTH",
            "-Dsuwayomi.tachidesk.config.server.authPassword=pw",
            "-Dsuwayomi.tachidesk.config.server.autoDownloadNewChapters=false",
        ] {
            assert!(has(want), "{want} missing from {a:?}");
        }
        assert_eq!(a[a.len() - 2], "-jar");
    }

    #[test]
    fn folder_names_are_plain() {
        assert!(plain_name("Weeb Central (EN)"));
        for bad in ["", "..", "../x", "C:", r"a\b", "a/b"] {
            assert!(!plain_name(bad), "{bad}");
        }
    }

    #[test]
    fn full_copies_rank_before_sharp_partial_ones() {
        let c = |s: &str, ch: usize, q: f64, d: f64| Candidate { source_id: s.into(), source: s.into(), manga_id: 1, title: "x".into(), chapters: ch, latest: None, quality: q, width: 0, density: d };
        let mut v = vec![c("fire", 200, 450.0, 0.03), c("asura", 71, 800.0, 0.2), c("weeb", 201, 720.0, 0.22)];
        rank(&mut v);
        assert_eq!(v.iter().map(|x| x.source.as_str()).collect::<Vec<_>>(), vec!["weeb", "fire", "asura"]);
        // 760 vs 720 is the same look: the less squeezed copy wins.
        let mut v = vec![c("fire", 201, 760.0, 0.09), c("weeb", 201, 720.0, 0.22)];
        rank(&mut v);
        assert_eq!(v[0].source, "weeb");
        // Measured 2026-11-08: extra half chapters don't make the others "partial".
        let l = |mut x: Candidate, n: f64| { x.latest = Some(n); x };
        let mut v = vec![l(c("fire", 225, 765.0, 0.09), 200.5), l(c("dex", 25, 720.0, 0.3), 46.0), l(c("weeb", 201, 720.0, 0.22), 200.0)];
        rank(&mut v);
        assert_eq!(v.iter().map(|x| x.source.as_str()).collect::<Vec<_>>(), vec!["weeb", "fire", "dex"]);
    }

    #[test]
    fn duplicate_chapters_collapse_to_one() {
        let ch = |id: i64, n: Option<f64>, s: &str, dl: bool| Chapter { id, name: String::new(), number: n, scanlator: Some(s.into()), downloaded: dl, pages: 1, uploaded: None };
        let v = one_per_chapter(vec![ch(1, Some(1.0), "A", false), ch(2, Some(1.0), "B", false), ch(3, Some(2.0), "A", false), ch(4, Some(2.0), "B", true), ch(7, Some(3.0), "A", false), ch(5, None, "A", false), ch(6, None, "B", false)]);
        assert_eq!(v.iter().map(|c| c.id).collect::<Vec<_>>(), vec![1, 4, 7, 5, 6], "main group A, but a downloaded copy is kept");
    }

    /// `CHILLARR_SUWA_BASE=http://127.0.0.1:4568 cargo test --lib live_candidates -- --ignored --nocapture`
    /// against a running Suwayomi with sources installed.
    #[tokio::test]
    #[ignore]
    async fn live_candidates() {
        if let Ok(pw) = std::env::var("CHILLARR_SUWA_PW") {
            *PASSWORD.lock().unwrap() = Some(pw);
        }
        let t = std::time::Instant::now();
        let names: Vec<String> = std::env::var("CHILLARR_TITLES").map(|t| t.split('|').map(String::from).collect()).unwrap_or_else(|_| vec!["Solo Leveling".to_string(), "Na Honjaman Level Up".to_string(), "Only I Level Up".to_string()]);
        let c = candidates(&names).await.unwrap();
        for x in &c {
            println!("{:24} {:30} ch={:4} latest={:?} width={} q={:.0}", x.source, x.title, x.chapters, x.latest, x.width, x.quality);
        }
        println!("{} candidates in {:?}", c.len(), t.elapsed());
        assert!(c.iter().all(|x| crate::read_core::best_title_score(&x.title, &names) >= crate::read_core::SAME_TITLE));
    }
}
