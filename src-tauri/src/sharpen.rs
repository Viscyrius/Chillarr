//! Sharpen pages with the graphics card. Each downloaded chapter is drawn again at
//! twice its size by Real-ESRGAN (its "animevideov3" model, made for drawn art), once,
//! in the background, and kept as a second copy beside the reading folder's own:
//! `<Comics>\Sharpened\<source>\<title>\<chapter>.cbz`. read.rs opens that copy when
//! there is one. The original is never touched; turning this off deletes the copies.
//!
//! Real-ESRGAN ncnn Vulkan is MIT (the program), its models BSD-3 (xinntao). One
//! release zip is pinned by SHA-256 like install.rs does, and only the program, its
//! runtime and the one model are unpacked. It runs on any Vulkan card; a real page is
//! drawn when it's switched on, so a PC whose card can't is told so then.
//!
//! Measured on the RTX 3060 Ti, a 22-page Weeb Central chapter (720 x 4000 pages):
//! 8.7 s to draw (0.4 s a page), 3.7 s to pack as JPEG with Jellyfin's ffmpeg, 11 MB
//! became 13.8 MB (with JPEG quality 3, as here: 16.8 MB). With VALORANT in front the
//! same chapter took 22 s to draw, so nothing is drawn while a game (or anything else)
//! runs full screen: Windows reports that as "busy" (checked with VALORANT, 2026-11-08).
//!
//! For the PC's owner only: not in bridge.rs's list. Its own file (sharpen.json).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashSet, VecDeque};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};
use tauri::{AppHandle, Manager};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;

/// Real-ESRGAN v0.2.5.0 (released 2022-04-24), the ncnn Vulkan build for Windows.
/// The hash is of the asset as downloaded on 2026-11-08.
const TAG: &str = "v0.2.5.0";
const ZIP: &str = "realesrgan-ncnn-vulkan-20220424-windows.zip";
const SHA256: &str = "abc02804e17982a3be33675e4d471e91ea374e65b70167abc09e31acb412802d";
const EXE: &str = "realesrgan-ncnn-vulkan.exe";
const MODEL: &str = "realesr-animevideov3";
/// What's unpacked from the zip; input.jpg is the page drawn as the test.
const KEEP: [&str; 5] = [EXE, "vcomp140.dll", "models/realesr-animevideov3-x2.bin", "models/realesr-animevideov3-x2.param", "input.jpg"];

/// Pages this wide or wider are sharp enough already and kept as they are.
const WIDE_ENOUGH: u32 = 1200;
/// ffmpeg's JPEG quality (2 best .. 31): 3 keeps the drawn lines clean.
const JPEG_Q: &str = "3";
/// A chapter file this new may still be being written.
const SETTLE: Duration = Duration::from_secs(30);
const IDLE: Duration = Duration::from_secs(60);
/// Pages per ffmpeg run, so the command line stays short.
const PACK_BATCH: usize = 80;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
struct Saved {
    on: bool,
    /// Chapters with no page worth drawing (all wide already): nothing to keep.
    as_is: Vec<String>,
    /// Chapters Real-ESRGAN failed on, so they aren't tried every minute.
    failed: Vec<String>,
}

#[derive(Default)]
struct Live {
    /// Chapters opened in the reader, drawn first.
    wanted: VecDeque<PathBuf>,
    /// The chapter being drawn now, for the window.
    now: Option<String>,
    left: usize,
    paused: bool,
}

static STATE: Mutex<Option<Saved>> = Mutex::new(None);
static LIVE: Mutex<Option<Live>> = Mutex::new(None);
static WAKE: tokio::sync::Notify = tokio::sync::Notify::const_new();

fn path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("sharpen.json"))
}

fn with<R>(app: &AppHandle, f: impl FnOnce(&mut Saved) -> R) -> R {
    let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let s = g.get_or_insert_with(|| path(app).and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default());
    f(s)
}

fn live<R>(f: impl FnOnce(&mut Live) -> R) -> R {
    let mut g = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Live::default))
}

fn save(app: &AppHandle) {
    let text = with(app, |s| serde_json::to_string(s).ok());
    if let (Some(p), Some(t)) = (path(app), text) {
        let tmp = p.with_extension("json.tmp");
        if std::fs::write(&tmp, t).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }
}

fn is_on(app: &AppHandle) -> bool {
    with(app, |s| s.on)
}

/// Where the program is unpacked.
fn tool_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join("Chillarr").join("Sharpen")
}

fn installed() -> bool {
    let d = tool_dir();
    KEEP.iter().all(|f| d.join(f).is_file())
}

// ---------- where the copies go ----------

fn originals(app: &AppHandle) -> Option<PathBuf> {
    crate::read::comics_dir(app).map(|c| c.join("mangas"))
}

fn copies(app: &AppHandle) -> Option<PathBuf> {
    crate::read::comics_dir(app).map(|c| c.join("Sharpened"))
}

/// The sharpened copy's place for an original chapter, if it's in the reading folder.
fn copy_of(orig_root: &Path, copy_root: &Path, file: &Path) -> Option<PathBuf> {
    file.strip_prefix(orig_root).ok().map(|rel| copy_root.join(rel))
}

fn modified(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// A copy counts while it's newer than its original (a chapter downloaded again is
/// drawn again).
fn fresh(orig: &Path, copy: &Path) -> bool {
    match (modified(orig), modified(copy)) {
        (Some(o), Some(c)) => c >= o,
        _ => false,
    }
}

/// What read.rs opens: the sharpened copy when there is a good one, else the
/// original, which is then drawn next.
pub fn pick(app: &AppHandle, file: PathBuf) -> (PathBuf, bool) {
    if !is_on(app) {
        return (file, false);
    }
    let (Some(o), Some(c)) = (originals(app), copies(app)) else { return (file, false) };
    let Some(copy) = copy_of(&o, &c, &file) else { return (file, false) };
    if fresh(&file, &copy) {
        return (copy, true);
    }
    let key = file.to_string_lossy().to_string();
    if !with(app, |s| s.as_is.contains(&key) || s.failed.contains(&key)) {
        live(|l| {
            l.wanted.retain(|w| w != &file);
            l.wanted.push_front(file.clone());
        });
        WAKE.notify_one();
    }
    (file, false)
}

// ---------- the window ----------

pub fn status(app: &AppHandle) -> Value {
    let on = is_on(app);
    let (now, left, paused) = live(|l| (l.now.clone(), l.left, l.paused));
    let bytes = copies(app).map(|c| folder_size(&c)).unwrap_or(0);
    json!({ "on": on, "now": now, "left": left, "paused": paused, "mb": (bytes as f64 / 1_048_576.0).round() })
}

fn folder_size(dir: &Path) -> u64 {
    let Ok(it) = std::fs::read_dir(dir) else { return 0 };
    it.filter_map(|e| e.ok())
        .map(|e| {
            let p = e.path();
            if p.is_dir() { folder_size(&p) } else { e.metadata().map(|m| m.len()).unwrap_or(0) }
        })
        .sum()
}

/// On: fetch the program if needed and draw a real page with it first. Off: the
/// copies are deleted, the originals stay.
pub async fn set(app: &AppHandle, on: bool) -> Result<Value, String> {
    if on {
        crate::read::comics_dir(app).ok_or("Pick a folder for reading first (Settings > Library & storage).")?;
        if !installed() {
            install().await?;
        }
        test_page().await?;
        with(app, |s| {
            s.on = true;
            s.failed.clear();
        });
        save(app);
        crate::log::detail("Sharpen pages: on");
        WAKE.notify_one();
    } else {
        with(app, |s| {
            s.on = false;
            s.as_is.clear();
            s.failed.clear();
        });
        save(app);
        live(|l| l.wanted.clear());
        if let Some(c) = copies(app) {
            let _ = tokio::task::spawn_blocking(move || std::fs::remove_dir_all(c)).await;
        }
        crate::log::detail("Sharpen pages: off, copies deleted");
    }
    Ok(status(app))
}

async fn install() -> Result<(), String> {
    let url = crate::install::asset_url("xinntao/Real-ESRGAN", TAG, ZIP);
    let http = reqwest::Client::builder().user_agent("Chillarr").build().map_err(|e| e.to_string())?;
    let bytes = http
        .get(&url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("Couldn't download the sharpener: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("Couldn't download the sharpener: {e}"))?;
    if !sha256_matches(&bytes, SHA256) {
        return Err("The sharpener download didn't match the expected file, so it wasn't used.".into());
    }
    let dir = tool_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let zip = dir.join(ZIP);
    std::fs::write(&zip, &bytes).map_err(|e| e.to_string())?;
    let (z, d) = (zip.clone(), dir.clone());
    let ok = tokio::task::spawn_blocking(move || {
        std::process::Command::new(crate::procs::sys32("tar.exe"))
            .arg("-xf")
            .arg(&z)
            .arg("-C")
            .arg(&d)
            .args(KEEP)
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
    .await
    .unwrap_or(false);
    let _ = std::fs::remove_file(&zip);
    if !ok || !installed() {
        return Err("Couldn't unpack the sharpener.".into());
    }
    Ok(())
}

fn sha256_matches(bytes: &[u8], want: &str) -> bool {
    let got: String = Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
    want.len() == 64 && got.eq_ignore_ascii_case(want)
}

/// Draws the zip's own sample page: no Vulkan card (or a broken driver) fails here.
async fn test_page() -> Result<(), String> {
    let dir = tool_dir();
    let out = std::env::temp_dir().join(format!("chillarr-sharpen-test-{}.png", std::process::id()));
    let _ = std::fs::remove_file(&out);
    let ran = draw(&dir.join("input.jpg"), &out, false).await;
    let made = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    let _ = std::fs::remove_file(&out);
    match ran {
        Ok(()) if made > 0 => Ok(()),
        _ => Err("Your graphics card can't sharpen pages here (it needs Vulkan), so nothing was changed.".into()),
    }
}

/// Why a chapter stopped part way: something went full screen. Not a failure.
const PAUSED: &str = "stopped for something full screen";

/// Real-ESRGAN on one file or a whole folder (`input` and `output` both folders).
/// With `watch`, it's stopped the moment something goes full screen: a long chapter
/// keeps the card busy for most of a minute, too long to finish over a game.
async fn draw(input: &Path, output: &Path, watch: bool) -> Result<(), String> {
    let dir = tool_dir();
    let child = tokio::process::Command::new(dir.join(EXE))
        .current_dir(&dir)
        .arg("-i")
        .arg(input)
        .arg("-o")
        .arg(output)
        .args(["-n", MODEL, "-s", "2", "-f", "png"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS)
        .kill_on_drop(true)
        .spawn();
    let mut child = child.map_err(|e| format!("couldn't start the sharpener: {e}"))?;
    let started = std::time::Instant::now();
    loop {
        tokio::select! {
            s = child.wait() => {
                return match s {
                    Ok(s) if s.success() => Ok(()),
                    Ok(s) => Err(format!("the sharpener stopped ({s})")),
                    Err(e) => Err(format!("the sharpener stopped ({e})")),
                };
            }
            _ = tokio::time::sleep(Duration::from_secs(1)) => {
                if watch && full_screen_busy() {
                    let _ = child.kill().await;
                    return Err(PAUSED.into());
                }
                if started.elapsed() > Duration::from_secs(15 * 60) {
                    let _ = child.kill().await;
                    return Err("the sharpener took too long".into());
                }
            }
        }
    }
}

// ---------- the work ----------

pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(20)).await;
        loop {
            if is_on(&app) {
                round(&app).await;
            }
            let _ = tokio::time::timeout(IDLE, WAKE.notified()).await;
        }
    });
}

/// Something full screen (a game, a film, a presentation) is in front.
fn full_screen_busy() -> bool {
    use windows::Win32::UI::Shell::{SHQueryUserNotificationState, QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN};
    // SAFETY: no arguments; it only reports the shell's state.
    match unsafe { SHQueryUserNotificationState() } {
        Ok(s) => s == QUNS_BUSY || s == QUNS_RUNNING_D3D_FULL_SCREEN || s == QUNS_PRESENTATION_MODE,
        Err(_) => false,
    }
}

/// Every chapter file under `dir` (source\title\chapter.cbz), newest first.
fn chapters(dir: &Path) -> Vec<(PathBuf, SystemTime)> {
    let mut out = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), 0)];
    while let Some((d, depth)) = stack.pop() {
        let Ok(it) = std::fs::read_dir(&d) else { continue };
        for e in it.filter_map(|e| e.ok()) {
            let p = e.path();
            if p.is_dir() {
                if depth < 2 {
                    stack.push((p, depth + 1));
                }
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("cbz") || x.eq_ignore_ascii_case("zip")) {
                if let Some(t) = modified(&p) {
                    out.push((p, t));
                }
            }
        }
    }
    out.sort_by(|a, b| b.1.cmp(&a.1));
    out
}

/// What's left to draw, the chapters opened in the reader first.
fn todo(app: &AppHandle, orig: &Path, copy: &Path) -> Vec<PathBuf> {
    let (as_is, failed) = with(app, |s| (s.as_is.iter().cloned().collect::<HashSet<_>>(), s.failed.iter().cloned().collect::<HashSet<_>>()));
    let now = SystemTime::now();
    let skip = |p: &Path| {
        let k = p.to_string_lossy().to_string();
        as_is.contains(&k) || failed.contains(&k) || copy_of(orig, copy, p).is_some_and(|c| fresh(p, &c))
    };
    let mut out: Vec<PathBuf> = live(|l| l.wanted.drain(..).collect::<Vec<_>>()).into_iter().filter(|p| p.is_file() && !skip(p)).collect();
    for (p, t) in chapters(orig) {
        let settled = now.duration_since(t).map(|d| d >= SETTLE).unwrap_or(false);
        if settled && !out.contains(&p) && !skip(&p) {
            out.push(p);
        }
    }
    out
}

async fn round(app: &AppHandle) {
    let (Some(orig), Some(copy)) = (originals(app), copies(app)) else { return };
    let (o, c) = (orig.clone(), copy.clone());
    let _ = tokio::task::spawn_blocking(move || sweep(&o, &c)).await;
    loop {
        if !is_on(app) {
            break;
        }
        let list = todo(app, &orig, &copy);
        live(|l| l.left = list.len());
        let Some(next) = list.into_iter().next() else { break };
        if full_screen_busy() {
            live(|l| l.paused = true);
            // Put it back: it's still the one to do first.
            live(|l| l.wanted.push_front(next));
            tokio::time::sleep(Duration::from_secs(30)).await;
            continue;
        }
        live(|l| {
            l.paused = false;
            l.now = next.file_stem().map(|s| s.to_string_lossy().to_string());
        });
        let Some(target) = copy_of(&orig, &copy, &next) else { break };
        let key = next.to_string_lossy().to_string();
        let started = std::time::Instant::now();
        match chapter(&next, &target).await {
            Ok(Done::Made(pages)) => crate::log::detail(format!("Sharpened {} ({pages} pages, {:.1} s)", next.display(), started.elapsed().as_secs_f64())),
            Ok(Done::AsIs) => with(app, |s| s.as_is.push(key)),
            // Something went full screen part way: it's done first once that's gone.
            Err(e) if e == PAUSED => live(|l| {
                l.paused = true;
                l.wanted.push_front(next.clone());
            }),
            Err(e) => {
                crate::log::detail(format!("Couldn't sharpen {}: {e}", next.display()));
                with(app, |s| s.failed.push(key));
            }
        }
        save(app);
        // Turned off while it worked: what it just made goes too.
        if !is_on(app) {
            let _ = std::fs::remove_file(&target);
        }
    }
    live(|l| {
        l.now = None;
        l.left = 0;
        l.paused = false;
    });
}

/// Copies whose original is gone (cleaned up, unfollowed) go too.
fn sweep(orig: &Path, copy: &Path) {
    for (p, _) in chapters(copy) {
        let Ok(rel) = p.strip_prefix(copy) else { continue };
        if !orig.join(rel).is_file() {
            let _ = std::fs::remove_file(&p);
        }
    }
    let _ = std::fs::remove_dir_all(copy.join(".work"));
}

enum Done {
    Made(usize),
    AsIs,
}

/// One chapter: pages out of the archive, the narrow ones drawn at twice the size,
/// packed as JPEG, and stored (not compressed again) in a new archive.
async fn chapter(file: &Path, target: &Path) -> Result<Done, String> {
    let copy_root = target.ancestors().find(|a| a.file_name().is_some_and(|n| n == "Sharpened")).ok_or("bad place")?.to_path_buf();
    let work = copy_root.join(".work").join(format!("{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    let (inp, drawn) = (work.join("in"), work.join("drawn"));
    std::fs::create_dir_all(&inp).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&drawn).map_err(|e| e.to_string())?;
    let done = chapter_in(file, target, &work, &inp, &drawn).await;
    let _ = std::fs::remove_dir_all(&work);
    done
}

async fn chapter_in(file: &Path, target: &Path, work: &Path, inp: &Path, drawn: &Path) -> Result<Done, String> {
    // Each page: (name in the new archive, the original bytes, or None when drawn).
    let (pages, drawing) = {
        let (file, inp) = (file.to_path_buf(), inp.to_path_buf());
        tokio::task::spawn_blocking(move || unpack(&file, &inp)).await.map_err(|e| e.to_string())??
    };
    if drawing == 0 {
        return Ok(Done::AsIs);
    }
    draw(inp, drawn, true).await?;
    pack_jpeg(work, &pages).await?;
    let (pages, work, target) = (pages, work.to_path_buf(), target.to_path_buf());
    tokio::task::spawn_blocking(move || store(&pages, &work, &target)).await.map_err(|e| e.to_string())?
}

/// A page of the new chapter.
struct Page {
    /// "0001": the drawn one is drawn\0001.png, packed to out\0001.jpg.
    stem: String,
    /// Kept as it was (wide already, or a kind Real-ESRGAN doesn't read).
    kept: Option<(Vec<u8>, &'static str)>,
}

fn unpack(file: &Path, inp: &Path) -> Result<(Vec<Page>, usize), String> {
    let entries = crate::zipread::entries(file)?;
    let names: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
    let order = crate::read_core::page_order(&names);
    if order.is_empty() {
        return Err("no pages".into());
    }
    let mut pages = Vec::with_capacity(order.len());
    let mut drawing = 0;
    for (n, &i) in order.iter().enumerate() {
        let e = &entries[i];
        let bytes = crate::zipread::read(file, e)?;
        let stem = format!("{:04}", n + 1);
        let ext = e.name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        let readable = matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp");
        let narrow = crate::read_core::dims(&bytes).is_some_and(|(w, _)| w > 0 && w < WIDE_ENOUGH);
        if readable && narrow {
            std::fs::write(inp.join(format!("{stem}.{ext}")), &bytes).map_err(|e| e.to_string())?;
            drawing += 1;
            pages.push(Page { stem, kept: None });
        } else {
            let ext: &'static str = match ext.as_str() {
                "jpeg" | "jpg" => "jpg",
                "png" => "png",
                "webp" => "webp",
                "gif" => "gif",
                "avif" => "avif",
                _ => "jxl",
            };
            pages.push(Page { stem, kept: Some((bytes, ext)) });
        }
    }
    Ok((pages, drawing))
}

/// The drawn PNGs (2-3 times the size of a JPEG) packed as JPEG by Jellyfin's ffmpeg,
/// many pages to one run.
async fn pack_jpeg(work: &Path, pages: &[Page]) -> Result<(), String> {
    let out = work.join("out");
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let drawn: Vec<&str> = pages.iter().filter(|p| p.kept.is_none()).map(|p| p.stem.as_str()).collect();
    for batch in drawn.chunks(PACK_BATCH) {
        let mut cmd = tokio::process::Command::new(crate::subsync::ffmpeg());
        cmd.current_dir(work).args(["-nostdin", "-v", "error", "-y"]);
        for s in batch {
            cmd.arg("-i").arg(format!("drawn/{s}.png"));
        }
        for (i, s) in batch.iter().enumerate() {
            cmd.args(["-map", &i.to_string(), "-q:v", JPEG_Q]).arg(format!("out/{s}.jpg"));
        }
        let run = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS).kill_on_drop(true).status();
        match tokio::time::timeout(Duration::from_secs(10 * 60), run).await {
            Ok(Ok(s)) if s.success() => {}
            _ => return Err("couldn't pack the sharpened pages".into()),
        }
    }
    Ok(())
}

fn store(pages: &[Page], work: &Path, target: &Path) -> Result<Done, String> {
    let mut files: Vec<(String, Vec<u8>)> = Vec::with_capacity(pages.len());
    for p in pages {
        match &p.kept {
            Some((bytes, ext)) => files.push((format!("{}.{ext}", p.stem), bytes.clone())),
            None => {
                let b = std::fs::read(work.join("out").join(format!("{}.jpg", p.stem))).map_err(|_| format!("page {} wasn't drawn", p.stem))?;
                if crate::read_core::dims(&b).is_none() {
                    return Err(format!("page {} came out unreadable", p.stem));
                }
                files.push((format!("{}.jpg", p.stem), b));
            }
        }
    }
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = target.with_extension("cbz.tmp");
    std::fs::write(&tmp, stored_zip(&files)).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, target).map_err(|e| e.to_string())?;
    Ok(Done::Made(files.len()))
}

/// A zip with every file stored as it is: JPEGs don't shrink when packed again, and
/// read.rs then serves a page straight off the disk.
fn stored_zip(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let total: usize = files.iter().map(|(n, b)| 30 + 46 + 2 * n.len() + b.len()).sum::<usize>() + 22;
    let mut out = Vec::with_capacity(total);
    let mut central = Vec::new();
    for (name, bytes) in files {
        let mut crc = flate2::Crc::new();
        crc.update(bytes);
        let (crc, size, at) = (crc.sum(), bytes.len() as u32, out.len() as u32);
        let n = name.len() as u16;
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0x21, 0]); // version, flags, stored, time, date (1980-01-01)
        for v in [crc, size, size] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(bytes);

        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0x21, 0]);
        for v in [crc, size, size] {
            central.extend_from_slice(&v.to_le_bytes());
        }
        central.extend_from_slice(&n.to_le_bytes());
        central.extend_from_slice(&[0; 12]); // extra, comment, disk, internal attrs, external attrs
        central.extend_from_slice(&at.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let dir_at = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    let count = files.len() as u16;
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&dir_at.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stored_zip_reads_back_page_for_page() {
        let dir = tempfile::tempdir().unwrap();
        let mut jpg = vec![0xFF, 0xD8, 0xFF, 0xC0, 0, 17, 8, 0x1F, 0x40, 0x05, 0xA0, 3];
        jpg.extend_from_slice(&[0; 20]);
        let files = vec![("0001.jpg".to_string(), jpg.clone()), ("0002.png".to_string(), b"not really a png".to_vec()), ("0010.jpg".to_string(), vec![7; 70_000])];
        let p = dir.path().join("x.cbz");
        std::fs::write(&p, stored_zip(&files)).unwrap();
        let entries = crate::zipread::entries(&p).unwrap();
        assert_eq!(entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), vec!["0001.jpg", "0002.png", "0010.jpg"]);
        for (e, (_, bytes)) in entries.iter().zip(&files) {
            assert_eq!(e.method, 0);
            assert_eq!(&crate::zipread::read(&p, e).unwrap(), bytes);
        }
        let names: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
        assert_eq!(crate::read_core::page_order(&names), vec![0, 1, 2], "pages stay in order");
        assert_eq!(crate::read_core::dims(&crate::zipread::read(&p, &entries[0]).unwrap()), Some((1440, 8000)));
    }

    #[test]
    fn a_copy_sits_beside_its_original_and_goes_stale_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let (o, c) = (dir.path().join("mangas"), dir.path().join("Sharpened"));
        let orig = o.join("Weeb Central (EN)").join("Solo").join("Ch 1.cbz");
        let copy = copy_of(&o, &c, &orig).unwrap();
        assert_eq!(copy, c.join("Weeb Central (EN)").join("Solo").join("Ch 1.cbz"));
        assert!(copy_of(&o, &c, Path::new(r"C:\elsewhere\x.cbz")).is_none());
        std::fs::create_dir_all(orig.parent().unwrap()).unwrap();
        std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
        std::fs::write(&orig, b"a").unwrap();
        assert!(!fresh(&orig, &copy), "no copy yet");
        std::fs::write(&copy, b"b").unwrap();
        assert!(fresh(&orig, &copy));
        let later = SystemTime::now() + Duration::from_secs(60);
        std::fs::File::options().write(true).open(&orig).unwrap().set_modified(later).unwrap();
        assert!(!fresh(&orig, &copy), "downloaded again: drawn again");
        // The original goes: so does the copy.
        std::fs::remove_file(&orig).unwrap();
        sweep(&o, &c);
        assert!(!copy.exists());
    }

    #[test]
    fn the_pinned_release_is_on_github() {
        assert_eq!(
            crate::install::asset_url("xinntao/Real-ESRGAN", TAG, ZIP),
            "https://github.com/xinntao/Real-ESRGAN/releases/download/v0.2.5.0/realesrgan-ncnn-vulkan-20220424-windows.zip"
        );
        assert_eq!(SHA256.len(), 64);
        assert!(!sha256_matches(b"abc", SHA256));
    }

    /// Draws the harness chapter for real (`CHILLARR_SHARPEN_PAGES` = a folder of
    /// pages). Needs the program installed and a Vulkan card.
    #[tokio::test]
    #[ignore]
    async fn live_sharpen_a_chapter() {
        if !installed() {
            install().await.unwrap();
        }
        test_page().await.unwrap();
        let src = PathBuf::from(std::env::var("CHILLARR_SHARPEN_PAGES").unwrap());
        let mut files: Vec<(String, Vec<u8>)> = std::fs::read_dir(&src).unwrap().filter_map(|e| e.ok()).map(|e| (e.file_name().to_string_lossy().to_string(), std::fs::read(e.path()).unwrap())).collect();
        files.sort_by(|a, b| crate::read_core::natural_cmp(&a.0, &b.0));
        let dir = tempfile::tempdir().unwrap();
        let orig = dir.path().join("mangas").join("S").join("T").join("Ch 1.cbz");
        std::fs::create_dir_all(orig.parent().unwrap()).unwrap();
        std::fs::write(&orig, stored_zip(&files)).unwrap();
        let target = dir.path().join("Sharpened").join("S").join("T").join("Ch 1.cbz");
        let t = std::time::Instant::now();
        let Done::Made(n) = chapter(&orig, &target).await.unwrap() else { panic!("nothing drawn") };
        let secs = t.elapsed().as_secs_f64();
        let (a, b) = (std::fs::metadata(&orig).unwrap().len(), std::fs::metadata(&target).unwrap().len());
        let e = crate::zipread::entries(&target).unwrap();
        let first = crate::read_core::dims(&crate::zipread::read(&target, &e[0]).unwrap());
        println!("{n} pages in {secs:.1} s ({:.2} s a page), {:.1} MB -> {:.1} MB, first page {first:?}", secs / n as f64, a as f64 / 1e6, b as f64 / 1e6);
        assert_eq!(n, files.len());
        assert!(!dir.path().join("Sharpened").join(".work").join(format!("{}", std::process::id())).exists());
    }
}
