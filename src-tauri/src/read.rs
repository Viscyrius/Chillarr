//! Reading: manga, manhua/manhwa and comics, kept and served by Chillarr
//! itself (issue #8). Suwayomi (suwayomi.rs) only downloads; what's followed,
//! where each profile is, the pages, Completed and the clean-up live here.
//!
//! Pages are served byte for byte from the chapter's archive by the bridge,
//! at `/read/<ticket>/<page>`: an `<img>` can't send the sign-in headers, so
//! opening a chapter (a signed-in command) hands out a random ticket for that
//! chapter, for that profile, for a few hours. Nothing is re-encoded, so a
//! page looks exactly as the source made it.

use crate::read_core;
use crate::zipread::{self, Entry};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

const TICKET_LIFE: Duration = Duration::from_secs(6 * 60 * 60);
/// Open chapters kept at once across everyone; the oldest goes first.
const MAX_TICKETS: usize = 512;
/// How long AniList's discovery rows are kept.
const ROWS_LIFE: Duration = Duration::from_secs(6 * 60 * 60);

// ---------- where it all goes ----------

/// The reading folder (Settings > Library & storage): Comics and Books inside.
pub fn root(app: &AppHandle) -> Option<PathBuf> {
    let st = app.state::<crate::SettingsState>();
    let s = st.current.lock().unwrap();
    s.reading_dir.as_deref().filter(|d| !d.trim().is_empty()).map(PathBuf::from)
}

pub fn comics_dir(app: &AppHandle) -> Option<PathBuf> {
    root(app).map(|r| r.join("Comics"))
}

pub fn books_dir(app: &AppHandle) -> Option<PathBuf> {
    root(app).map(|r| r.join("Books"))
}

/// Next to the films: the folder holding Radarr's first root, one level up
/// when that one is called Movies (`D:\Media\Movies\Normal` -> `D:\Media\Reading`).
pub fn default_root(movie_root: &str) -> Option<PathBuf> {
    let p = Path::new(movie_root.trim_end_matches(['\\', '/']));
    let mut base = p.parent()?;
    if p.file_name().is_some_and(|n| n.eq_ignore_ascii_case("movies")) {
        // The root itself is ...\Movies: its parent is the base.
    } else if base.file_name().is_some_and(|n| n.eq_ignore_ascii_case("movies")) {
        base = base.parent()?;
    }
    Some(base.join("Reading"))
}

/// The reading folder, picked next to the films the first time it's needed.
pub async fn ensure_root(app: &AppHandle) -> Result<PathBuf, String> {
    if let Some(r) = root(app) {
        return Ok(r);
    }
    let a = crate::media::arr(app, crate::model::AppId::Radarr).ok_or("Set up your movie folder first (Settings > Library & storage).")?;
    let roots = a.list("rootfolder").await.map_err(|e| format!("Radarr didn't answer: {e}"))?;
    let first = roots.iter().find_map(|r| r["path"].as_str()).ok_or("Set up your movie folder first (Settings > Library & storage).")?;
    let dir = default_root(first).ok_or("Pick a folder for reading in Settings > Library & storage.")?;
    set_root(app, &dir.to_string_lossy())?;
    Ok(dir)
}

/// Moves the reading folder: what's in Comics and Books goes along (a rename,
/// so it must be the same drive), and the downloader restarts on the new one.
pub async fn move_root(app: &AppHandle, dir: &str) -> Result<String, String> {
    let to = PathBuf::from(dir.trim().trim_end_matches(['\\', '/']));
    let Some(from) = root(app) else {
        set_root(app, &to.to_string_lossy())?;
        return Ok("Reading goes there from now on.".into());
    };
    if from == to {
        return Ok("That's already the reading folder.".into());
    }
    let empty = |p: &Path| std::fs::read_dir(p).map(|mut d| d.next().is_none()).unwrap_or(true);
    let subs: Vec<&str> = ["Comics", "Books"].into_iter().filter(|s| from.join(s).is_dir()).collect();
    // Every target checked before anything moves, so a move never stops half way.
    for sub in &subs {
        let b = to.join(sub);
        if b.exists() && !empty(&b) {
            return Err(format!("{} already has things in it. Pick an empty folder.", b.display()));
        }
    }
    std::fs::create_dir_all(&to).map_err(|e| format!("Couldn't make {}: {e}", to.display()))?;
    app.state::<crate::fix::Core>().procs.stop_suwayomi();
    let mut moved: Vec<&str> = Vec::new();
    let mut failed = None;
    for sub in &subs {
        let (a, b) = (from.join(sub), to.join(sub));
        let _ = std::fs::remove_dir(&b);
        match std::fs::rename(&a, &b) {
            Ok(()) => moved.push(sub),
            Err(e) => {
                failed = Some(format!("Couldn't move {} (it has to be on the same drive): {e}", a.display()));
                break;
            }
        }
    }
    if let Some(e) = failed {
        // Put back what did move: the old folder stays the reading folder.
        for sub in moved {
            let _ = std::fs::rename(to.join(sub), from.join(sub));
        }
        if !followed(app).is_empty() {
            let _ = crate::suwayomi::ready(app).await;
        }
        return Err(e);
    }
    set_root(app, &to.to_string_lossy())?;
    if !followed(app).is_empty() {
        let _ = crate::suwayomi::ready(app).await;
    }
    Ok("Moved. Reading lives there now.".into())
}

pub fn set_root(app: &AppHandle, dir: &str) -> Result<(), String> {
    let dir = dir.trim().trim_end_matches(['\\', '/']).to_string();
    for sub in ["Comics", "Books"] {
        std::fs::create_dir_all(Path::new(&dir).join(sub)).map_err(|e| format!("Couldn't make {dir}\\{sub}: {e}"))?;
    }
    let st = app.state::<crate::SettingsState>();
    let saved = {
        let mut s = st.current.lock().unwrap();
        s.reading_dir = Some(dir);
        s.clone()
    };
    crate::settings::save(&st.path, &saved)
}

fn data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path().app_data_dir().map_err(|e| e.to_string())
}

fn uid(app: &AppHandle) -> Result<String, String> {
    crate::media::active_user(app).ok_or_else(|| "Sign in to Jellyfin first.".to_string())
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn save_json<T: Serialize>(path: &Path, v: &T) -> Result<(), String> {
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string(v).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, path)).map_err(|e| format!("Couldn't save it: {e}"))
}

fn load_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

// ---------- what's followed (shared by every profile) ----------

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Followed {
    /// "m<Suwayomi manga id>".
    pub key: String,
    pub manga_id: i64,
    pub anilist: Option<i64>,
    pub title: String,
    #[serde(default)]
    pub names: Vec<String>,
    pub cover: Option<String>,
    pub banner: Option<String>,
    pub country: Option<String>,
    pub format: Option<String>,
    /// AniList's: RELEASING, FINISHED, HIATUS...
    pub status: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub genres: Vec<String>,
    /// "manga", "webtoon" or "book": how it opens unless a profile changed it.
    pub mode: String,
    /// The source's name as its download folder is named ("Weeb Central (EN)").
    pub source: String,
    /// The series' title at that source (its folder's name).
    pub source_title: String,
    pub width: u32,
    pub added: i64,
    /// Chapters from this number on are wanted (None: all of them).
    #[serde(default)]
    pub from: Option<f64>,
}

fn followed_file(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(data_dir(app)?.join("reading.json"))
}

pub fn followed(app: &AppHandle) -> Vec<Followed> {
    followed_file(app).map(|p| load_json(&p)).unwrap_or_default()
}

fn save_followed(app: &AppHandle, all: &[Followed]) -> Result<(), String> {
    save_json(&followed_file(app)?, &all)
}

/// A key progress may be saved under: a followed series or a book.
fn known(app: &AppHandle, key: &str) -> Result<(), String> {
    if key.starts_with('b') { crate::books::find(app, key).map(|_| ()) } else { find(app, key).map(|_| ()) }
}

fn find(app: &AppHandle, key: &str) -> Result<Followed, String> {
    followed(app).into_iter().find(|f| f.key == key).ok_or_else(|| "That series isn't in your reading list.".into())
}

// ---------- where each profile is ----------

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Spot {
    pub chapter: i64,
    pub page: usize,
    pub pages: usize,
    pub at: i64,
    /// Chapters finished, with when: drives the clean-up.
    #[serde(default)]
    pub read: HashMap<i64, i64>,
    /// This profile's own mode for the series, over the series' default.
    #[serde(default)]
    pub mode: Option<String>,
}

fn spots_file(dir: &Path, uid: &str) -> PathBuf {
    dir.join(format!("read-{uid}.json"))
}

fn spots(app: &AppHandle, uid: &str) -> HashMap<String, Spot> {
    data_dir(app).map(|d| load_json(&spots_file(&d, uid))).unwrap_or_default()
}

fn save_spots(app: &AppHandle, uid: &str, all: &HashMap<String, Spot>) -> Result<(), String> {
    save_json(&spots_file(&data_dir(app)?, uid), all)
}

/// Every profile's spots, for the clean-up.
fn all_spots(app: &AppHandle) -> Vec<HashMap<String, Spot>> {
    let Ok(dir) = data_dir(app) else { return vec![] };
    std::fs::read_dir(&dir)
        .map(|it| {
            it.filter_map(|e| e.ok())
                .filter(|e| e.file_name().to_string_lossy().starts_with("read-") && e.file_name().to_string_lossy().ends_with(".json"))
                .map(|e| load_json(&e.path()))
                .collect()
        })
        .unwrap_or_default()
}

// ---------- chapters on disk ----------

/// The series' download folder: `<Comics>/mangas/<source>/<title>`, the title
/// as Suwayomi wrote it (compared loosely: it strips characters Windows refuses).
fn series_dir(app: &AppHandle, f: &Followed) -> Option<PathBuf> {
    if !crate::suwayomi::plain_name(&f.source) {
        return None;
    }
    let src = comics_dir(app)?.join("mangas").join(&f.source);
    let want = read_core::norm_title(&f.source_title);
    std::fs::read_dir(&src).ok()?.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).find(|p| p.file_name().is_some_and(|n| read_core::norm_title(&n.to_string_lossy()) == want))
}

/// The .cbz of one chapter: by its name, else by its number (and scanlator).
pub fn chapter_file(dir: &Path, ch: &crate::suwayomi::Chapter) -> Option<PathBuf> {
    let files: Vec<(PathBuf, read_core::ChapterName)> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("cbz") || x.eq_ignore_ascii_case("zip")))
        .map(|p| {
            let n = read_core::chapter_name(&p.file_stem().unwrap_or_default().to_string_lossy());
            (p, n)
        })
        .collect();
    let same = |a: &str, b: &str| read_core::norm_title(a) == read_core::norm_title(b);
    files
        .iter()
        .find(|(_, n)| same(&n.name, &ch.name) && (ch.scanlator.is_none() || n.scanlator.as_deref().is_some_and(|s| same(s, ch.scanlator.as_deref().unwrap_or_default()))))
        .or_else(|| files.iter().find(|(_, n)| same(&n.name, &ch.name)))
        .or_else(|| files.iter().find(|(_, n)| ch.number.is_some() && n.number == ch.number))
        .map(|(p, _)| p.clone())
}

// ---------- tickets and pages ----------

struct Ticket {
    file: PathBuf,
    entries: Arc<Vec<Entry>>,
    /// A comic's pages in order (empty for a book).
    pages: Vec<usize>,
    /// "comic", "epub" or "file" (a PDF or other ebook served whole).
    kind: &'static str,
    born: Instant,
}

#[derive(Default)]
pub struct Reading {
    tickets: Mutex<HashMap<String, Ticket>>,
    rows: tokio::sync::Mutex<Option<(Instant, Value)>>,
}

fn new_ticket() -> String {
    let mut b = [0u8; 16];
    getrandom::getrandom(&mut b).expect("random");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

impl Reading {
    fn issue(&self, file: PathBuf, entries: Arc<Vec<Entry>>, pages: Vec<usize>) -> String {
        self.issue_kind(file, entries, pages, "comic")
    }

    fn issue_kind(&self, file: PathBuf, entries: Arc<Vec<Entry>>, pages: Vec<usize>, kind: &'static str) -> String {
        let mut t = self.tickets.lock().unwrap();
        t.retain(|_, v| v.born.elapsed() < TICKET_LIFE);
        while t.len() >= MAX_TICKETS {
            let Some(old) = t.iter().min_by_key(|(_, v)| v.born).map(|(k, _)| k.clone()) else { break };
            t.remove(&old);
        }
        let id = new_ticket();
        t.insert(id.clone(), Ticket { file, entries, pages, kind, born: Instant::now() });
        id
    }
}

/// The first bytes of a page, enough to read its size: the whole entry for a
/// stored one, the first 64 KB inflated for a packed one.
fn page_head(file: &Path, e: &Entry) -> Option<(u32, u32)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(file).ok()?;
    let mut head = [0u8; 30];
    f.seek(SeekFrom::Start(e.header_at)).ok()?;
    f.read_exact(&mut head).ok()?;
    let skip = u16::from_le_bytes([head[26], head[27]]) as i64 + u16::from_le_bytes([head[28], head[29]]) as i64;
    f.seek(SeekFrom::Current(skip)).ok()?;
    let mut raw = vec![0u8; e.compressed.min(256 * 1024) as usize];
    f.read_exact(&mut raw).ok()?;
    let bytes = if e.method == 8 {
        let mut out = Vec::new();
        let _ = flate2::read::DeflateDecoder::new(&raw[..]).take(64 * 1024).read_to_end(&mut out);
        out
    } else {
        raw
    };
    read_core::dims(&bytes)
}

#[derive(Serialize)]
pub struct Opened {
    pub ticket: String,
    pub pages: Vec<(u32, u32)>,
    pub chapter: crate::suwayomi::Chapter,
    pub prev: Option<i64>,
    pub next: Option<i64>,
    pub mode: String,
    pub page: usize,
    pub title: String,
}

/// Opens one downloaded chapter for the watching profile.
pub async fn open(app: &AppHandle, key: &str, chapter: i64) -> Result<Opened, String> {
    let uid = uid(app)?;
    let f = find(app, key)?;
    let chs = crate::suwayomi::chapters(f.manga_id).await?;
    let at = chs.iter().position(|c| c.id == chapter).ok_or("That chapter isn't there any more.")?;
    let ch = chs[at].clone();
    let dir = series_dir(app, &f).ok_or("This series has nothing downloaded yet.")?;
    let file = chapter_file(&dir, &ch).ok_or("That chapter isn't downloaded yet.")?;
    let (entries, pages, sizes) = tokio::task::spawn_blocking({
        let file = file.clone();
        move || -> Result<_, String> {
            let entries = zipread::entries(&file)?;
            let names: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
            let pages = read_core::page_order(&names);
            let sizes = pages.iter().map(|&i| page_head(&file, &entries[i]).unwrap_or((0, 0))).collect::<Vec<_>>();
            Ok((entries, pages, sizes))
        }
    })
    .await
    .map_err(|e| e.to_string())??;
    if pages.is_empty() {
        return Err("That chapter has no pages. It may still be downloading.".into());
    }
    let downloaded = |i: usize| chs.get(i).filter(|c| c.downloaded).map(|c| c.id);
    let prev = at.checked_sub(1).and_then(downloaded);
    let next = downloaded(at + 1);
    let s = spots(app, &uid);
    let spot = s.get(key);
    let page = spot.filter(|s| s.chapter == chapter).map(|s| s.page.min(pages.len() - 1)).unwrap_or(0);
    let mode = spot.and_then(|s| s.mode.clone()).unwrap_or_else(|| f.mode.clone());
    let ticket = app.state::<Reading>().issue(file, Arc::new(entries), pages);
    Ok(Opened { ticket, pages: sizes, chapter: ch, prev, next, mode, page, title: f.title })
}

/// `/read/<ticket>/<page>` (a comic's page), `/read/<ticket>/f/<path>` (a
/// file inside an EPUB) or `/read/<ticket>/file` (a PDF whole).
pub fn serve(app: &AppHandle, path: &str) -> (&'static str, &'static str, Vec<u8>) {
    const GONE: (&str, &str, &[u8]) = ("404 Not Found", "text/plain", b"Open it again.");
    let path = path.split('?').next().unwrap_or(path);
    let parts: Vec<&str> = path.trim_start_matches('/').splitn(4, '/').collect();
    if let ["read", ticket, rest @ ..] = parts.as_slice() {
        if rest.first() == Some(&"f") || rest.first() == Some(&"file") {
            let found = {
                let t = app.state::<Reading>();
                let t = t.tickets.lock().unwrap();
                t.get(*ticket).filter(|v| v.born.elapsed() < TICKET_LIFE && v.kind != "comic").map(|v| (v.file.clone(), v.entries.clone(), v.kind))
            };
            let Some((file, entries, kind)) = found else { return (GONE.0, GONE.1, GONE.2.to_vec()) };
            if rest.first() == Some(&"file") {
                return match std::fs::read(&file) {
                    Ok(b) => ("200 OK", read_core::mime(&file.to_string_lossy()), b),
                    Err(_) => ("500 Internal Server Error", "text/plain", b"Couldn't read it.".to_vec()),
                };
            }
            let Some(inner) = rest.get(1).filter(|_| kind == "epub") else { return (GONE.0, GONE.1, GONE.2.to_vec()) };
            return match crate::books::epub_file(&file, &entries, inner) {
                Some(b) => ("200 OK", read_core::mime(inner), b),
                None => ("404 Not Found", "text/plain", b"Not in this book.".to_vec()),
            };
        }
    }
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let ["read", ticket, n] = parts.as_slice() else { return ("404 Not Found", "text/plain", b"not here".to_vec()) };
    let Ok(n) = n.parse::<usize>() else { return ("404 Not Found", "text/plain", b"not here".to_vec()) };
    let found = {
        let t = app.state::<Reading>();
        let t = t.tickets.lock().unwrap();
        t.get(*ticket).filter(|v| v.born.elapsed() < TICKET_LIFE).and_then(|v| v.pages.get(n).map(|&i| (v.file.clone(), v.entries[i].clone())))
    };
    let Some((file, e)) = found else { return ("404 Not Found", "text/plain", b"Open the chapter again.".to_vec()) };
    match zipread::read(&file, &e) {
        Ok(b) => ("200 OK", read_core::mime(&e.name), b),
        Err(_) => ("500 Internal Server Error", "text/plain", b"Couldn't read that page.".to_vec()),
    }
}

/// Where the watching profile is now. A chapter read to its last page counts as read.
pub fn set_spot(app: &AppHandle, key: &str, chapter: i64, page: usize, pages: usize) -> Result<(), String> {
    let uid = uid(app)?;
    known(app, key)?;
    let mut all = spots(app, &uid);
    let s = all.entry(key.to_string()).or_default();
    s.chapter = chapter;
    s.page = page;
    s.pages = pages;
    s.at = now();
    if key.starts_with('b') {
        // A book: chapter = part of the book, page = how far into it (0..1000).
        // Done when the last part is read to its end (kept under -1).
        if pages > 0 && chapter as usize + 1 >= pages && page >= 990 {
            s.read.entry(-1).or_insert(now());
        }
    } else if pages > 0 && page + 1 >= pages {
        s.read.entry(chapter).or_insert(now());
    }
    save_spots(app, &uid, &all)
}

pub fn set_mode(app: &AppHandle, key: &str, mode: &str) -> Result<(), String> {
    if !matches!(mode, "manga" | "webtoon" | "book" | "ltr") {
        return Err("Unknown reading mode.".into());
    }
    let uid = uid(app)?;
    known(app, key)?;
    let mut all = spots(app, &uid);
    all.entry(key.to_string()).or_default().mode = Some(mode.to_string());
    save_spots(app, &uid, &all)
}

// ---------- books ----------

#[derive(Serialize)]
pub struct OpenedBook {
    pub ticket: String,
    /// "epub", "pdf" or "other" (AZW3/MOBI: saved, not shown).
    pub kind: String,
    pub spine: Vec<String>,
    pub title: String,
    pub chapter: usize,
    /// How far into that spine item, 0..1000.
    pub at: usize,
}

pub async fn open_book(app: &AppHandle, key: &str) -> Result<OpenedBook, String> {
    let uid = uid(app)?;
    let b = crate::books::find(app, key)?;
    let file = crate::books::path_of(app, &b).ok_or("That book isn't downloaded yet.")?;
    let ext = file.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let spot = spots(app, &uid).remove(key);
    let (chapter, at) = spot.map(|s| (s.chapter.max(0) as usize, s.page)).unwrap_or((0, 0));
    if ext != "epub" {
        let ticket = app.state::<Reading>().issue_kind(file, Arc::new(vec![]), vec![], "file");
        return Ok(OpenedBook { ticket, kind: if ext == "pdf" { "pdf".into() } else { "other".into() }, spine: vec![], title: b.title, chapter: 0, at: 0 });
    }
    let (entries, spine) = tokio::task::spawn_blocking({
        let file = file.clone();
        move || -> Result<_, String> {
            let entries = zipread::entries(&file)?;
            let container = crate::books::epub_file(&file, &entries, "META-INF/container.xml").ok_or("This EPUB is damaged (no container).")?;
            let (spine, _) = crate::books::epub_spine(&String::from_utf8_lossy(&container), |p| crate::books::epub_file(&file, &entries, p).map(|b| String::from_utf8_lossy(&b).into_owned()))
                .ok_or("This EPUB is damaged (no reading order).")?;
            Ok((entries, spine))
        }
    })
    .await
    .map_err(|e| e.to_string())??;
    let chapter = chapter.min(spine.len().saturating_sub(1));
    let ticket = app.state::<Reading>().issue_kind(file, Arc::new(entries), vec![], "epub");
    Ok(OpenedBook { ticket, kind: "epub".into(), spine, title: b.title, chapter, at })
}

#[derive(Serialize)]
pub struct BookShelf {
    #[serde(flatten)]
    pub book: crate::books::Book,
    pub ready: bool,
    /// Downloading: how far, 0..1.
    pub progress: Option<f64>,
    /// Read so far, 0..1, for this profile.
    pub read: f64,
    pub at: i64,
}

pub async fn books(app: &AppHandle) -> Result<Vec<BookShelf>, String> {
    let uid = uid(app)?;
    let s = spots(app, &uid);
    let torrents = match crate::waiting::qbit(app) {
        Some(q) => q.list().await.unwrap_or_default(),
        None => vec![],
    };
    let mut out: Vec<BookShelf> = crate::books::all(app)
        .into_iter()
        .map(|b| {
            let spot = s.get(&b.key);
            let progress = b.hash.as_ref().and_then(|h| torrents.iter().find(|t| t["hash"].as_str() == Some(h.as_str()))).and_then(|t| t["progress"].as_f64());
            let read = spot
                .map(|s| if s.read.contains_key(&-1) { 1.0 } else if s.pages > 0 { ((s.chapter as f64) + s.page as f64 / 1000.0) / s.pages as f64 } else { 0.0 })
                .unwrap_or(0.0)
                .clamp(0.0, 1.0);
            let at = spot.map(|s| s.at).unwrap_or(b.added);
            BookShelf { ready: b.file.is_some(), progress, read, at, book: b }
        })
        .collect();
    out.sort_by(|a, b| b.at.cmp(&a.at));
    Ok(out)
}

// ---------- the library as the pages show it ----------

#[derive(Serialize)]
pub struct Shelf {
    pub key: String,
    pub anilist: Option<i64>,
    pub title: String,
    pub cover: Option<String>,
    pub country: Option<String>,
    pub status: Option<String>,
    pub source: String,
    pub width: u32,
    pub chapters: usize,
    pub downloaded: usize,
    pub unread: usize,
    /// The chapter to open next and its page.
    pub resume: Option<(i64, usize)>,
    pub resume_label: Option<String>,
    pub finished: bool,
    pub at: i64,
}

/// Every followed series, as the watching profile sees it.
pub async fn library(app: &AppHandle) -> Result<Vec<Shelf>, String> {
    let uid = uid(app)?;
    let s = spots(app, &uid);
    let mut out = Vec::new();
    for f in followed(app) {
        let chs = crate::suwayomi::chapters(f.manga_id).await.unwrap_or_default();
        out.push(shelf_of(&f, &chs, s.get(&f.key)));
    }
    out.sort_by(|a, b| b.at.cmp(&a.at).then(a.title.cmp(&b.title)));
    Ok(out)
}

pub fn shelf_of(f: &Followed, chs: &[crate::suwayomi::Chapter], spot: Option<&Spot>) -> Shelf {
    let read = |id: i64| spot.is_some_and(|s| s.read.contains_key(&id));
    let unread = chs.iter().filter(|c| c.downloaded && !read(c.id)).count();
    // Resume where you were; finished that chapter, the next one; never started, the first.
    let resume = match spot {
        Some(s) if !read(s.chapter) && chs.iter().any(|c| c.id == s.chapter) => Some((s.chapter, s.page)),
        Some(s) => {
            let at = chs.iter().position(|c| c.id == s.chapter).map(|i| i + 1).unwrap_or(0);
            chs.iter().skip(at).find(|c| c.downloaded && !read(c.id)).map(|c| (c.id, 0))
        }
        None => chs.iter().find(|c| c.downloaded).map(|c| (c.id, 0)),
    };
    let label = resume.and_then(|(id, _)| chs.iter().find(|c| c.id == id)).map(|c| match c.number {
        Some(n) if n.fract() == 0.0 => format!("Chapter {}", n as i64),
        Some(n) => format!("Chapter {n}"),
        None => c.name.clone(),
    });
    let all_read = !chs.is_empty() && chs.iter().all(|c| read(c.id));
    Shelf {
        key: f.key.clone(),
        anilist: f.anilist,
        title: f.title.clone(),
        cover: f.cover.clone(),
        country: f.country.clone(),
        status: f.status.clone(),
        source: f.source.clone(),
        width: f.width,
        chapters: chs.len(),
        downloaded: chs.iter().filter(|c| c.downloaded).count(),
        unread,
        resume,
        resume_label: label,
        // Read to the end of a series that has ended.
        finished: all_read && f.status.as_deref() == Some("FINISHED"),
        at: spot.map(|s| s.at).unwrap_or(f.added),
    }
}

#[derive(Serialize)]
pub struct Detail {
    pub followed: Option<Followed>,
    pub chapters: Vec<ChapterRow>,
    pub spot: Option<Spot>,
    pub mode: String,
    pub downloading: usize,
}

#[derive(Serialize)]
pub struct ChapterRow {
    #[serde(flatten)]
    pub ch: crate::suwayomi::Chapter,
    pub read: bool,
    pub progress: Option<f64>,
    pub queued: Option<f64>,
}

pub async fn detail(app: &AppHandle, key: &str) -> Result<Detail, String> {
    let uid = uid(app)?;
    let f = find(app, key)?;
    let chs = crate::suwayomi::chapters(f.manga_id).await?;
    let q = crate::suwayomi::queue().await;
    let spot = spots(app, &uid).remove(key);
    let rows = chs
        .into_iter()
        .map(|ch| {
            let read = spot.as_ref().is_some_and(|s| s.read.contains_key(&ch.id));
            let progress = spot.as_ref().filter(|s| s.chapter == ch.id && !read && s.pages > 0).map(|s| (s.page + 1) as f64 / s.pages as f64);
            let queued = q.iter().find(|x| x.1 == ch.id).map(|x| x.2);
            ChapterRow { ch, read, progress, queued }
        })
        .collect::<Vec<_>>();
    let mode = spot.as_ref().and_then(|s| s.mode.clone()).unwrap_or_else(|| f.mode.clone());
    Ok(Detail { downloading: rows.iter().filter(|r| r.queued.is_some()).count(), followed: Some(f), chapters: rows, spot, mode })
}

/// Marks chapters read or unread for the watching profile.
pub fn mark(app: &AppHandle, key: &str, chapters: &[i64], read: bool) -> Result<(), String> {
    let uid = uid(app)?;
    known(app, key)?;
    let mut all = spots(app, &uid);
    let s = all.entry(key.to_string()).or_default();
    for c in chapters {
        if read {
            s.read.entry(*c).or_insert(now());
        } else {
            s.read.remove(c);
        }
    }
    save_spots(app, &uid, &all)
}

// ---------- following ----------

/// AniList's facts about a series, as the Read tab passes them in.
#[derive(Deserialize, Clone, Debug, Default)]
pub struct Meta {
    pub anilist: Option<i64>,
    pub title: String,
    #[serde(default)]
    pub names: Vec<String>,
    pub cover: Option<String>,
    pub banner: Option<String>,
    pub country: Option<String>,
    pub format: Option<String>,
    pub status: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub genres: Vec<String>,
}

impl Meta {
    fn all_names(&self) -> Vec<String> {
        let mut v = vec![self.title.clone()];
        v.extend(self.names.iter().cloned());
        v.retain(|n| !n.trim().is_empty());
        v.dedup();
        v
    }
}

/// Every source's copy of the series, best first (slow: each source is searched).
pub async fn sources(app: &AppHandle, meta: &Meta) -> Result<Vec<crate::suwayomi::Candidate>, String> {
    ensure_root(app).await?;
    crate::suwayomi::ready(app).await?;
    crate::suwayomi::candidates(&meta.all_names()).await
}

/// Follows the series from `pick` (a candidate from `sources`), downloading
/// every chapter from `from` on. Following it again from another source
/// replaces the old one; what was read is kept.
pub async fn follow(app: &AppHandle, meta: Meta, pick: crate::suwayomi::Candidate, from: Option<f64>) -> Result<String, String> {
    ensure_root(app).await?;
    crate::suwayomi::ready(app).await?;
    // Where its chapters land is Suwayomi's say, never the page's: the folder
    // names come from the downloader itself and are checked to be plain names.
    let (source, source_title) = crate::suwayomi::folder_names(pick.manga_id).await?;
    let queued = crate::suwayomi::follow(pick.manga_id, from).await?;
    let mut all = followed(app);
    let mode = read_core::default_mode(meta.country.as_deref(), meta.format.as_deref()).to_string();
    let key = format!("m{}", pick.manga_id);
    // The same series from another source: the old one stops.
    let old: Vec<Followed> = all.iter().filter(|f| f.key != key && meta.anilist.is_some() && f.anilist == meta.anilist).cloned().collect();
    for o in &old {
        let _ = crate::suwayomi::unfollow(o.manga_id).await;
    }
    all.retain(|f| f.key != key && !old.iter().any(|o| o.key == f.key));
    all.push(Followed {
        key,
        manga_id: pick.manga_id,
        anilist: meta.anilist,
        title: meta.title.clone(),
        names: meta.names.clone(),
        cover: meta.cover.clone(),
        banner: meta.banner.clone(),
        country: meta.country.clone(),
        format: meta.format.clone(),
        status: meta.status.clone(),
        description: meta.description.clone(),
        genres: meta.genres.clone(),
        mode,
        source: source.clone(),
        source_title,
        width: pick.width,
        added: now(),
        from,
    });
    save_followed(app, &all)?;
    let msg = match queued {
        0 => format!("Following {}. New chapters download by themselves.", meta.title),
        1 => format!("Getting 1 chapter of {} from {}.", meta.title, source),
        n => format!("Getting {n} chapters of {} from {}.", meta.title, source),
    };
    crate::log::log(app, crate::model::Level::Ok, &msg);
    Ok(msg)
}

/// Stops following; with `delete`, its downloaded chapters go too.
pub async fn unfollow(app: &AppHandle, key: &str, delete: bool) -> Result<String, String> {
    let f = find(app, key)?;
    if delete {
        let chs = crate::suwayomi::chapters(f.manga_id).await.unwrap_or_default();
        let ids: Vec<i64> = chs.iter().filter(|c| c.downloaded).map(|c| c.id).collect();
        crate::suwayomi::delete_chapters(&ids).await?;
    }
    crate::suwayomi::unfollow(f.manga_id).await?;
    let mut all = followed(app);
    all.retain(|x| x.key != key);
    save_followed(app, &all)?;
    Ok(format!("Stopped following {}.", f.title))
}

// ---------- clean-up of read chapters ----------

/// Chapters to delete: read by someone at least `days` ago, and behind every
/// profile that is reading the series (a profile further back keeps them).
pub fn cleanup_pick(chs: &[crate::suwayomi::Chapter], spots: &[&Spot], days: u32, now: i64) -> Vec<i64> {
    if days == 0 || spots.is_empty() {
        return vec![];
    }
    let pos = |id: i64| chs.iter().position(|c| c.id == id);
    // Each reader's furthest point: the chapter they're on, unless it's read.
    let behind_all = spots.iter().map(|s| pos(s.chapter).map(|p| if s.read.contains_key(&s.chapter) { p + 1 } else { p }).unwrap_or(0)).min().unwrap_or(0);
    chs.iter()
        .take(behind_all)
        .filter(|c| c.downloaded)
        .filter(|c| spots.iter().filter_map(|s| s.read.get(&c.id)).any(|&at| now - at >= days as i64 * 86_400))
        .map(|c| c.id)
        .collect()
}

/// Once per PC: the reading folder, the manga downloader and its sources, so
/// the Read tab works on the first visit. Setup calls it; a PC set up before
/// reading existed gets it on its first start after the update.
pub async fn set_up_once(app: &AppHandle) {
    let (done, setup) = {
        let st = app.state::<crate::SettingsState>();
        let s = st.current.lock().unwrap();
        (s.reading_ready, s.setup_done)
    };
    if done || !setup {
        return;
    }
    let r = async {
        ensure_root(app).await?;
        crate::suwayomi::ready(app).await
    }
    .await;
    match r {
        Ok(()) => {
            let st = app.state::<crate::SettingsState>();
            let saved = {
                let mut s = st.current.lock().unwrap();
                s.reading_ready = true;
                s.clone()
            };
            let _ = crate::settings::save(&st.path, &saved);
            crate::log::log(app, crate::model::Level::Ok, "Reading is set up: manga, manhwa, manhua and books are in the Read tab.");
        }
        Err(e) => crate::log::log(app, crate::model::Level::Amber, format!("Reading isn't set up yet: {e}. Chillarr tries again next start.")),
    }
}

/// New chapters of what's followed, one copy each (Suwayomi's own auto
/// download takes every group's copy). A chapter someone has read isn't
/// fetched again: the clean-up may have deleted it on purpose.
pub async fn fetch_new(app: &AppHandle) {
    let list = followed(app);
    if list.is_empty() {
        return;
    }
    let profiles = all_spots(app);
    let queued: std::collections::HashSet<i64> = crate::suwayomi::queue().await.into_iter().map(|q| q.1).collect();
    for f in list {
        let Ok(chs) = crate::suwayomi::chapters(f.manga_id).await else { continue };
        let read_by_someone = |id: i64| profiles.iter().filter_map(|p| p.get(&f.key)).any(|s| s.read.contains_key(&id));
        let want: Vec<i64> = chs
            .iter()
            .filter(|c| !c.downloaded && !queued.contains(&c.id) && !read_by_someone(c.id))
            .filter(|c| f.from.is_none_or(|from| c.number.unwrap_or(f64::MAX) >= from))
            .map(|c| c.id)
            .collect();
        if !want.is_empty() && crate::suwayomi::enqueue(&want).await.is_ok() {
            crate::log::log(app, crate::model::Level::Ok, format!("Getting {} new chapter(s) of {}.", want.len(), f.title));
        }
    }
}

pub async fn clean_up(app: &AppHandle) {
    let days = app.state::<crate::SettingsState>().current.lock().unwrap().read_cleanup_days;
    if days == 0 {
        return;
    }
    let profiles = all_spots(app);
    for f in followed(app) {
        let readers: Vec<&Spot> = profiles.iter().filter_map(|p| p.get(&f.key)).collect();
        let Ok(chs) = crate::suwayomi::chapters(f.manga_id).await else { continue };
        let doomed = cleanup_pick(&chs, &readers, days, now());
        if !doomed.is_empty() && crate::suwayomi::delete_chapters(&doomed).await.is_ok() {
            crate::log::log(app, crate::model::Level::Ok, format!("Deleted {} read chapter(s) of {}.", doomed.len(), f.title));
        }
    }
}

// ---------- discovery (AniList) ----------

const MEDIA_FIELDS: &str = "id title{romaji english native} synonyms coverImage{extraLarge large color} bannerImage countryOfOrigin format status chapters volumes averageScore genres description(asHtml:false) startDate{year}";

/// A card on the Read tab.
pub fn card(m: &Value) -> Value {
    let t = &m["title"];
    let title = t["english"].as_str().or(t["romaji"].as_str()).unwrap_or_default().to_string();
    let mut names: Vec<String> = [t["english"].as_str(), t["romaji"].as_str(), t["native"].as_str()].into_iter().flatten().map(String::from).collect();
    names.extend(m["synonyms"].as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect::<Vec<_>>()).unwrap_or_default());
    names.retain(|n| n != &title);
    names.dedup();
    json!({
        "anilist": m["id"],
        "title": title,
        "names": names,
        "cover": m["coverImage"]["extraLarge"].as_str().or(m["coverImage"]["large"].as_str()),
        "color": m["coverImage"]["color"],
        "banner": m["bannerImage"],
        "country": m["countryOfOrigin"],
        "format": m["format"],
        "status": m["status"],
        "chapters": m["chapters"],
        "score": m["averageScore"],
        "genres": m["genres"],
        "description": m["description"].as_str().map(clean_text),
        "year": m["startDate"]["year"],
    })
}

/// AniList descriptions carry <br> and <i>: plain text for the page.
fn clean_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut tag = false;
    for c in s.replace("<br>", "\n").replace("<br/>", "\n").replace("<br />", "\n").chars() {
        match c {
            '<' => tag = true,
            '>' if tag => tag = false,
            c if !tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&quot;", "\"").replace("&#039;", "'").replace("&amp;", "&").replace("\n\n\n", "\n\n").trim().to_string()
}

/// The Read tab's rows, kept six hours.
pub async fn rows(app: &AppHandle) -> Result<Value, String> {
    let state = app.state::<Reading>();
    let mut cache = state.rows.lock().await;
    if let Some((at, v)) = cache.as_ref() {
        if at.elapsed() < ROWS_LIFE {
            return Ok(v.clone());
        }
    }
    let defs: [(&str, &str); 8] = [
        ("trending", "sort:TRENDING_DESC"),
        ("manhwa", "sort:POPULARITY_DESC,countryOfOrigin:\"KR\""),
        ("manhua", "sort:POPULARITY_DESC,countryOfOrigin:\"CN\""),
        ("manga", "sort:POPULARITY_DESC,countryOfOrigin:\"JP\",format:MANGA"),
        ("top", "sort:SCORE_DESC,popularity_greater:20000"),
        ("new", "sort:POPULARITY_DESC,status:RELEASING,startDate_greater:20240000"),
        ("action", "sort:POPULARITY_DESC,genre:\"Action\""),
        ("romance", "sort:POPULARITY_DESC,genre:\"Romance\""),
    ];
    let mut q = String::from("{");
    for (name, args) in defs {
        q.push_str(&format!("{name}:Page(perPage:30){{media(type:MANGA,isAdult:false,{args}){{{MEDIA_FIELDS}}}}} "));
    }
    q.push('}');
    let v = crate::media::anilist(&json!({ "query": q })).await?;
    let data = &v["data"];
    if data.is_null() {
        return Err("AniList didn't answer. Try again in a minute.".into());
    }
    let titles = [
        ("trending", "Trending now"),
        ("manhwa", "Popular manhwa"),
        ("manhua", "Popular manhua"),
        ("manga", "Popular manga"),
        ("top", "Top rated"),
        ("new", "Ongoing hits"),
        ("action", "Action"),
        ("romance", "Romance"),
    ];
    let out: Vec<Value> = titles
        .iter()
        .map(|(k, title)| json!({ "id": k, "title": title, "items": data[*k]["media"].as_array().map(|a| a.iter().map(card).collect::<Vec<_>>()).unwrap_or_default() }))
        .collect();
    let v = json!(out);
    *cache = Some((Instant::now(), v.clone()));
    Ok(v)
}

pub async fn search(term: &str) -> Result<Vec<Value>, String> {
    let term = term.trim();
    if term.is_empty() {
        return Ok(vec![]);
    }
    let q = format!("query($s:String){{Page(perPage:30){{media(type:MANGA,isAdult:false,search:$s,sort:SEARCH_MATCH){{{MEDIA_FIELDS}}}}}}}");
    let v = crate::media::anilist(&json!({ "query": q, "variables": { "s": term } })).await?;
    Ok(v["data"]["Page"]["media"].as_array().map(|a| a.iter().map(card).collect()).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::suwayomi::Chapter;

    fn ch(id: i64, n: f64, downloaded: bool) -> Chapter {
        Chapter { id, name: format!("Chapter {n}"), number: Some(n), scanlator: None, downloaded, pages: 10, uploaded: None }
    }

    #[test]
    fn reading_folder_sits_beside_the_films() {
        assert_eq!(default_root(r"C:\Users\anime\Desktop\Desktop\Movies\Normal"), Some(PathBuf::from(r"C:\Users\anime\Desktop\Desktop\Reading")));
        assert_eq!(default_root(r"D:\Media\Movies"), Some(PathBuf::from(r"D:\Media\Reading")));
        assert_eq!(default_root(r"D:\Films\"), Some(PathBuf::from(r"D:\Reading")));
    }

    #[test]
    fn chapter_files_are_found_by_name_then_number() {
        let dir = tempfile::tempdir().unwrap();
        for f in ["Official_Chapter 1.cbz", "Official_Chapter 2.cbz", "Asura_Chapter 10.5 - Gate.cbz", "notes.txt"] {
            std::fs::write(dir.path().join(f), b"x").unwrap();
        }
        let mut c = ch(1, 2.0, true);
        c.scanlator = Some("Official".into());
        assert_eq!(chapter_file(dir.path(), &c).unwrap().file_name().unwrap(), "Official_Chapter 2.cbz");
        let mut odd = ch(2, 10.5, true);
        odd.name = "Ch 10.5".into();
        assert_eq!(chapter_file(dir.path(), &odd).unwrap().file_name().unwrap(), "Asura_Chapter 10.5 - Gate.cbz", "by number when the name differs");
        assert!(chapter_file(dir.path(), &ch(3, 99.0, true)).is_none());
    }

    #[test]
    fn resume_goes_to_the_next_unread_chapter() {
        let f = Followed { key: "m1".into(), status: Some("FINISHED".into()), ..Default::default() };
        let chs = vec![ch(1, 1.0, true), ch(2, 2.0, true), ch(3, 3.0, true)];
        assert_eq!(shelf_of(&f, &chs, None).resume, Some((1, 0)), "never started: the first");
        let mut s = Spot { chapter: 2, page: 5, pages: 10, ..Default::default() };
        assert_eq!(shelf_of(&f, &chs, Some(&s)).resume, Some((2, 5)), "half read: where you were");
        s.read.insert(1, 0);
        s.read.insert(2, 0);
        assert_eq!(shelf_of(&f, &chs, Some(&s)).resume, Some((3, 0)), "finished it: the next");
        s.read.insert(3, 0);
        let done = shelf_of(&f, &chs, Some(&s));
        assert!(done.finished && done.resume.is_none() && done.unread == 0);
    }

    #[test]
    fn cleanup_keeps_what_anyone_still_needs() {
        let chs: Vec<Chapter> = (1..=5).map(|i| ch(i, i as f64, true)).collect();
        let day = 86_400;
        let now = 100 * day;
        let mut ahead = Spot { chapter: 5, ..Default::default() };
        for i in 1..=4 {
            ahead.read.insert(i, now - 5 * day);
        }
        let behind = Spot { chapter: 2, ..Default::default() };
        assert_eq!(cleanup_pick(&chs, &[&ahead], 3, now), vec![1, 2, 3, 4]);
        assert_eq!(cleanup_pick(&chs, &[&ahead, &behind], 3, now), vec![1], "the other profile is on chapter 2");
        assert!(cleanup_pick(&chs, &[&ahead], 0, now).is_empty(), "off");
        assert!(cleanup_pick(&chs, &[&ahead], 7, now).is_empty(), "read 5 days ago, keep for 7");
    }

    #[test]
    fn anilist_cards_are_plain() {
        let m = json!({ "id": 105398, "title": { "romaji": "Na Honjaman Level Up", "english": "Solo Leveling", "native": "나 혼자만 레벨업" }, "synonyms": ["Only I Level Up"], "countryOfOrigin": "KR", "description": "A <i>hunter</i>.<br><br>Rises &amp; falls." });
        let c = card(&m);
        assert_eq!(c["title"], "Solo Leveling");
        assert_eq!(c["names"], json!(["Na Honjaman Level Up", "나 혼자만 레벨업", "Only I Level Up"]));
        assert_eq!(c["description"], "A hunter.\n\nRises & falls.");
    }
}
