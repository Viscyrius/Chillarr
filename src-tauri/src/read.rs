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
use std::collections::{HashMap, HashSet};
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

pub(crate) fn uid(app: &AppHandle) -> Result<String, String> {
    crate::media::active_user(app).ok_or_else(|| "Sign in to Jellyfin first.".to_string())
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn save_json<T: Serialize>(path: &Path, v: &T) -> Result<(), String> {
    // A temp name of its own per write: two saves at once used to share one
    // and the second rename failed.
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = path.with_extension(format!("json.{}-{n}.tmp", std::process::id()));
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
    /// The translation it's read in ("en", "es", "pt-BR"...).
    #[serde(default)]
    pub lang: Option<String>,
    /// The one "Get new chapters" button: its sites are asked every few minutes
    /// (keepup.rs) and new chapters download by themselves.
    #[serde(default = "yes")]
    pub new_too: bool,
    /// The highest chapter number the site listed when it was followed (or last
    /// checked). Only chapters above it are new; older ones left out were left out
    /// on purpose. None: not known yet, the next check sets it and fetches nothing.
    #[serde(default)]
    pub known_upto: Option<f64>,
    /// Other copies filling in what this one lacks (a site further ahead).
    #[serde(default)]
    pub fill: Vec<Fill>,
    /// From before the one button: Keep up to date. Read once as `new_too` on.
    #[serde(default)]
    pub live: bool,
    /// The newest chapter a "ready to read" notification has covered.
    #[serde(default)]
    pub told: Option<f64>,
    /// The chapter another site was ahead to when that was last said.
    #[serde(default)]
    pub ahead_told: Option<f64>,
    /// The profiles (Jellyfin user ids) that follow it: each profile's reading is
    /// its own. Empty: from before this was kept, the PC owner's.
    #[serde(default)]
    pub owners: Vec<String>,
    /// Chapters deleted by hand (read_storage.rs): new-chapter fetching leaves
    /// them alone, as it does read ones. Getting one again takes it off.
    #[serde(default)]
    pub removed: Vec<i64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Fill {
    pub manga_id: i64,
    pub source: String,
    pub source_title: String,
    pub width: u32,
}

impl Followed {
    /// Every copy: (Suwayomi manga id, source folder, series folder), the main one first.
    pub fn copies(&self) -> Vec<(i64, String, String)> {
        let mut v = vec![(self.manga_id, self.source.clone(), self.source_title.clone())];
        v.extend(self.fill.iter().map(|x| (x.manga_id, x.source.clone(), x.source_title.clone())));
        v
    }

    pub fn has_manga(&self, id: i64) -> bool {
        self.manga_id == id || self.fill.iter().any(|x| x.manga_id == id)
    }
}

/// One series out of several copies, in order: each chapter number from the
/// first copy that has it downloaded, else the first that lists it. A chapter
/// with no number is only taken from the main copy. Returns (chapter, copy).
pub fn merge_copies(lists: Vec<Vec<crate::suwayomi::Chapter>>) -> Vec<(crate::suwayomi::Chapter, usize)> {
    let mut out: Vec<(crate::suwayomi::Chapter, usize)> = Vec::new();
    for (i, list) in lists.into_iter().enumerate() {
        for c in list {
            let Some(n) = c.number else {
                if i == 0 {
                    out.push((c, 0));
                }
                continue;
            };
            match out.iter_mut().find(|(o, _)| o.number == Some(n)) {
                None => out.push((c, i)),
                Some(slot) => {
                    if c.downloaded && !slot.0.downloaded {
                        *slot = (c, i);
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| a.0.number.unwrap_or(f64::MAX).partial_cmp(&b.0.number.unwrap_or(f64::MAX)).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// A chapter as `read_core::keep_mask` sees it: its number and upload time (ms).
fn shape(c: &crate::suwayomi::Chapter) -> (Option<f64>, Option<i64>) {
    (c.number, c.uploaded.as_deref().and_then(|u| u.parse::<i64>().ok()).filter(|&d| d > 0))
}

/// Each copy without the numbers it can't be trusted with (read_core::keep_mask):
/// a copy's own run of chapters, plus what another copy's own run backs.
pub fn clean_copies(lists: Vec<Vec<crate::suwayomi::Chapter>>) -> Vec<Vec<crate::suwayomi::Chapter>> {
    let shapes: Vec<Vec<(Option<f64>, Option<i64>)>> = lists.iter().map(|l| l.iter().map(shape).collect()).collect();
    let tops: Vec<Option<f64>> = shapes.iter().map(|s| read_core::own_run_top(s)).collect();
    lists
        .into_iter()
        .enumerate()
        .map(|(i, list)| {
            let backing = tops.iter().enumerate().filter(|(j, _)| *j != i).filter_map(|(_, t)| *t).fold(None, |m: Option<f64>, n| Some(m.map_or(n, |m| m.max(n))));
            let mask = read_core::keep_mask(&shapes[i], backing);
            list.into_iter().zip(mask).filter(|(_, k)| *k).map(|(c, _)| c).collect()
        })
        .collect()
}

/// A saved mark ("known up to", "told") above what the list can be trusted
/// with comes down to it: a stray 276 or 560 saved before kept every real
/// chapter after the true newest from being fetched or told.
pub fn lowered_mark(mark: Option<f64>, top: Option<f64>) -> Option<f64> {
    match (mark, top) {
        (Some(m), Some(t)) if m > t => Some(t),
        _ => mark,
    }
}

/// A followed series' chapters across its copies. The main copy must answer.
pub async fn chapters_of(f: &Followed) -> Result<Vec<(crate::suwayomi::Chapter, usize)>, String> {
    let mut lists = vec![crate::suwayomi::chapters(f.manga_id).await?];
    for x in &f.fill {
        lists.push(crate::suwayomi::chapters(x.manga_id).await.unwrap_or_default());
    }
    Ok(merge_copies(clean_copies(lists)))
}

async fn chapter_list(f: &Followed) -> Result<Vec<crate::suwayomi::Chapter>, String> {
    Ok(chapters_of(f).await?.into_iter().map(|x| x.0).collect())
}

pub async fn chapter_list_of(f: &Followed) -> Result<Vec<crate::suwayomi::Chapter>, String> {
    chapter_list(f).await
}

/// Held around every read, change and save of reading.json, so a background
/// check saving its copy can't undo a change made meanwhile. Never held across
/// an await: whoever waits on the network reads the list again afterwards.
static FOLLOWED_LOCK: Mutex<()> = Mutex::new(());

fn followed_lock() -> std::sync::MutexGuard<'static, ()> {
    FOLLOWED_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Changes one followed series in place (a no-op when it's gone).
pub fn update_followed(app: &AppHandle, key: &str, change: impl FnOnce(&mut Followed)) {
    let _held = followed_lock();
    let mut all = followed(app);
    if let Some(f) = all.iter_mut().find(|f| f.key == key) {
        change(f);
        let _ = save_followed(app, &all);
    }
}

/// Get new chapters on or off, for everyone (what's followed is shared).
/// Switched on, what the site lists now is the starting line: only chapters
/// out after that come by themselves.
pub fn set_live(app: &AppHandle, key: &str, on: bool) -> Result<bool, String> {
    find(app, key)?;
    update_followed(app, key, |f| {
        f.new_too = on;
        f.live = false;
        if on {
            f.known_upto = None;
        }
    });
    if on {
        crate::keepup::wake(key);
    }
    Ok(on)
}

fn yes() -> bool {
    true
}

fn followed_file(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(data_dir(app)?.join("reading.json"))
}

pub fn followed(app: &AppHandle) -> Vec<Followed> {
    let mut all: Vec<Followed> = followed_file(app).map(|p| load_json(&p)).unwrap_or_default();
    for f in &mut all {
        one_button(f);
    }
    all
}

/// Follow's new chapters and Keep up to date are one button now: either one on is on.
pub fn one_button(f: &mut Followed) {
    if f.live {
        f.new_too = true;
        f.live = false;
    }
}

fn save_followed(app: &AppHandle, all: &[Followed]) -> Result<(), String> {
    save_json(&followed_file(app)?, &all)
}

/// A key progress may be saved under: a followed series or a book.
fn known(app: &AppHandle, key: &str) -> Result<(), String> {
    if key.starts_with('b') { crate::books::find(app, key).map(|_| ()) } else { find(app, key).map(|_| ()) }
}

/// Who has something, with the old (empty) lists read as the PC owner's.
pub fn owners_of(list: &[String], pc_owner: Option<&str>) -> Vec<String> {
    if list.is_empty() { pc_owner.map(|o| vec![o.to_string()]).unwrap_or_default() } else { list.to_vec() }
}

pub fn owned_by(list: &[String], pc_owner: Option<&str>, uid: &str) -> bool {
    owners_of(list, pc_owner).iter().any(|o| o == uid)
}

pub(crate) fn mine(app: &AppHandle, f: &Followed, uid: &str) -> bool {
    owned_by(&f.owners, crate::profiles::owner_id(app).as_deref(), uid)
}

/// The watching profile's series only: someone else's isn't in your list.
fn find(app: &AppHandle, key: &str) -> Result<Followed, String> {
    let uid = uid(app)?;
    followed(app).into_iter().find(|f| f.key == key && mine(app, f, &uid)).ok_or_else(|| "That series isn't in your reading list.".into())
}

/// What the watching profile follows.
fn followed_mine(app: &AppHandle) -> Vec<Followed> {
    let Ok(uid) = uid(app) else { return vec![] };
    followed(app).into_iter().filter(|f| mine(app, f, &uid)).collect()
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
    /// Kept out of Your week and Latest updates for this profile (someone else
    /// may be looking at the screen). Still in the list, and opens as usual.
    #[serde(default)]
    pub hide_week: bool,
}

fn spots_file(dir: &Path, uid: &str) -> PathBuf {
    dir.join(format!("read-{uid}.json"))
}

pub(crate) fn spots(app: &AppHandle, uid: &str) -> HashMap<String, Spot> {
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
pub(crate) fn series_dir(app: &AppHandle, source: &str, source_title: &str) -> Option<PathBuf> {
    if !crate::suwayomi::plain_name(source) {
        return None;
    }
    let src = comics_dir(app)?.join("mangas").join(source);
    let want = read_core::norm_title(source_title);
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
    /// kind_rows' answers, per kind.
    kind_rows: tokio::sync::Mutex<HashMap<String, (Instant, Value)>>,
    /// for_you's rows, per profile and the series they were made from.
    for_you: Mutex<HashMap<String, (Instant, Value)>>,
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
    /// The pages are the graphics card's sharpened copy (sharpen.rs).
    pub sharp: bool,
}

/// Opens one downloaded chapter for the watching profile.
pub async fn open(app: &AppHandle, key: &str, chapter: i64) -> Result<Opened, String> {
    let uid = uid(app)?;
    let f = find(app, key)?;
    let merged = chapters_of(&f).await?;
    let at = merged.iter().position(|(c, _)| c.id == chapter).ok_or("That chapter isn't there any more.")?;
    let (ch, copy) = merged[at].clone();
    let copies = f.copies();
    let (_, src, title) = &copies[copy.min(copies.len() - 1)];
    let dir = series_dir(app, src, title).ok_or("This series has nothing downloaded yet.")?;
    let chs: Vec<crate::suwayomi::Chapter> = merged.into_iter().map(|x| x.0).collect();
    let file = chapter_file(&dir, &ch).ok_or("That chapter isn't downloaded yet.")?;
    // The graphics card's sharpened copy, when Settings has it on and it's made.
    let (file, sharp) = crate::sharpen::pick(app, file);
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
    // The nearest chapter each way that is here: one missing in between (a
    // failed download, a deleted one) doesn't end the series there.
    let prev = chs.get(..at).and_then(|before| before.iter().rev().find(|c| c.downloaded)).map(|c| c.id);
    let next = chs.get(at + 1..).and_then(|after| after.iter().find(|c| c.downloaded)).map(|c| c.id);
    let s = spots(app, &uid);
    let spot = s.get(key);
    let page = spot.filter(|s| s.chapter == chapter).map(|s| s.page.min(pages.len() - 1)).unwrap_or(0);
    let mode = spot.and_then(|s| s.mode.clone()).unwrap_or_else(|| f.mode.clone());
    let ticket = app.state::<Reading>().issue(file, Arc::new(entries), pages);
    Ok(Opened { ticket, pages: sizes, chapter: ch, prev, next, mode, page, title: f.title, sharp })
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
    let comic = key.starts_with('b') && crate::books::find(app, key).is_ok_and(|b| b.comic);
    let mut all = spots(app, &uid);
    let s = all.entry(key.to_string()).or_default();
    s.chapter = chapter;
    s.page = page;
    s.pages = pages;
    s.at = now();
    if key.starts_with('b') {
        // A book: chapter = part of the book, page = how far into it (0..1000).
        // Done when the last part is read to its end (kept under -1).
        // A comic: chapter 0, page = the page it's on, of `pages`.
        let done = if comic { page + 1 >= pages } else { chapter as usize + 1 >= pages && page >= 990 };
        if pages > 0 && done {
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

/// Hides a series from Your week and Latest updates for the watching profile, or shows it again.
pub fn set_hide_week(app: &AppHandle, key: &str, hide: bool) -> Result<bool, String> {
    let uid = uid(app)?;
    find(app, key)?;
    let mut all = spots(app, &uid);
    all.entry(key.to_string()).or_default().hide_week = hide;
    save_spots(app, &uid, &all)?;
    Ok(hide)
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
    /// A comic ("comic"): each page's width and height, the page it's on,
    /// and how it's read. Its pages are `/read/<ticket>/<n>`.
    pub pages: Vec<(u32, u32)>,
    pub page: usize,
    pub mode: String,
}

pub async fn open_book(app: &AppHandle, key: &str) -> Result<OpenedBook, String> {
    let uid = uid(app)?;
    let b = crate::books::find(app, key)?;
    let file = crate::books::path_of(app, &b).ok_or("That book isn't downloaded yet.")?;
    let ext = file.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let spot = spots(app, &uid).remove(key);
    if ext == "cbz" {
        return open_comic(app, b, file, spot).await;
    }
    let (chapter, at) = spot.map(|s| (s.chapter.max(0) as usize, s.page)).unwrap_or((0, 0));
    if ext != "epub" {
        let ticket = app.state::<Reading>().issue_kind(file, Arc::new(vec![]), vec![], "file");
        return Ok(OpenedBook { ticket, kind: if ext == "pdf" { "pdf".into() } else { "other".into() }, spine: vec![], title: b.title, chapter: 0, at: 0, pages: vec![], page: 0, mode: String::new() });
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
    Ok(OpenedBook { ticket, kind: "epub".into(), spine, title: b.title, chapter, at, pages: vec![], page: 0, mode: String::new() })
}

/// A comic's pages, like a chapter's: read straight out of its CBZ.
async fn open_comic(app: &AppHandle, b: crate::books::Book, file: PathBuf, spot: Option<Spot>) -> Result<OpenedBook, String> {
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
        return Err("This comic has no pages in it.".into());
    }
    let page = spot.as_ref().map(|s| s.page.min(pages.len() - 1)).unwrap_or(0);
    // Western comics read left to right unless the profile picked another way.
    let mode = spot.and_then(|s| s.mode).unwrap_or_else(|| "ltr".into());
    let ticket = app.state::<Reading>().issue(file, Arc::new(entries), pages);
    Ok(OpenedBook { ticket, kind: "comic".into(), spine: vec![], title: b.title, chapter: 0, at: 0, pages: sizes, page, mode })
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
    /// 90% in, or past the last page: counts as read (BOOK_DONE).
    pub finished: bool,
    pub at: i64,
}

/// How far into a book counts as finished.
pub const BOOK_DONE: f64 = 0.9;

pub async fn books(app: &AppHandle) -> Result<Vec<BookShelf>, String> {
    let uid = uid(app)?;
    let s = spots(app, &uid);
    let torrents = match crate::waiting::qbit(app) {
        Some(q) => q.list().await.unwrap_or_default(),
        None => vec![],
    };
    let pc_owner = crate::profiles::owner_id(app);
    let mut out: Vec<BookShelf> = crate::books::all(app)
        .into_iter()
        .filter(|b| owned_by(&b.owners, pc_owner.as_deref(), &uid))
        .map(|b| {
            let spot = s.get(&b.key);
            let progress = b.hash.as_ref().and_then(|h| torrents.iter().find(|t| t["hash"].as_str() == Some(h.as_str()))).and_then(|t| t["progress"].as_f64());
            let read = spot
                .map(|s| {
                    if s.read.contains_key(&-1) {
                        1.0
                    } else if s.pages == 0 {
                        0.0
                    } else if b.comic {
                        (s.page + 1) as f64 / s.pages as f64
                    } else {
                        ((s.chapter as f64) + s.page as f64 / 1000.0) / s.pages as f64
                    }
                })
                .unwrap_or(0.0)
                .clamp(0.0, 1.0);
            // Most books end in notes, thanks and previews: 90% in counts as read
            // (user, 2026-11-08). The place itself is kept, to go on from there.
            let finished = read >= BOOK_DONE;
            let at = spot.map(|s| s.at).unwrap_or(b.added);
            BookShelf { ready: b.file.is_some(), progress, read, finished, at, book: b }
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
    /// Chapters new since this profile last read it (is_fresh), the newest's
    /// label, and when that came out (seconds).
    pub fresh: usize,
    pub fresh_label: Option<String>,
    pub fresh_at: i64,
    /// Kept out of Your week and Latest updates (Spot.hide_week).
    pub hide_week: bool,
    /// When its newest chapter came out (seconds), so the page can put the
    /// series that updated lately first.
    pub latest_at: i64,
}

/// Every followed series, as the watching profile sees it.
pub async fn library(app: &AppHandle) -> Result<Vec<Shelf>, String> {
    let uid = uid(app)?;
    let s = spots(app, &uid);
    // Each series' chapters at the same time (a few at once, so the manga
    // downloader isn't flooded): one after another made every reading page
    // wait for the whole list.
    let gate = Arc::new(tokio::sync::Semaphore::new(6));
    let mut set = tokio::task::JoinSet::new();
    for f in followed_mine(app) {
        let gate = gate.clone();
        set.spawn(async move {
            let _turn = gate.acquire_owned().await;
            let chs = chapter_list(&f).await.unwrap_or_default();
            (f, chs)
        });
    }
    let mut out = Vec::new();
    while let Some(done) = set.join_next().await {
        if let Ok((f, chs)) = done {
            out.push(shelf_of(&f, &chs, s.get(&f.key)));
        }
    }
    out.sort_by(|a, b| b.at.cmp(&a.at).then(a.title.cmp(&b.title)));
    Ok(out)
}

/// When a chapter came out, in seconds (Suwayomi gives milliseconds, as text).
fn uploaded_secs(c: &crate::suwayomi::Chapter) -> i64 {
    c.uploaded.as_deref().and_then(|u| u.trim().parse::<i64>().ok()).map(|ms| ms / 1000).unwrap_or(0)
}

/// New for this profile: here, not read, and out since they last read the series
/// (or since it was followed: what was already out then isn't news).
pub fn is_fresh(c: &crate::suwayomi::Chapter, f: &Followed, spot: Option<&Spot>) -> bool {
    let since = spot.map(|s| s.at).unwrap_or(0).max(f.added);
    c.downloaded && !spot.is_some_and(|s| s.read.contains_key(&c.id)) && uploaded_secs(c) > since
}

pub fn shelf_of(f: &Followed, chs: &[crate::suwayomi::Chapter], spot: Option<&Spot>) -> Shelf {
    let read = |id: i64| spot.is_some_and(|s| s.read.contains_key(&id));
    let unread = chs.iter().filter(|c| c.downloaded && !read(c.id)).count();
    let fresh: Vec<&crate::suwayomi::Chapter> = chs.iter().filter(|c| is_fresh(c, f, spot)).collect();
    let fresh_label = fresh.iter().filter_map(|c| c.number).fold(None, |m: Option<f64>, n| Some(m.map_or(n, |m| m.max(n)))).map(|n| if n.fract() == 0.0 { format!("Chapter {}", n as i64) } else { format!("Chapter {n}") });
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
        fresh: fresh.len(),
        fresh_label,
        fresh_at: fresh.iter().map(|c| uploaded_secs(c)).max().unwrap_or(0),
        hide_week: spot.is_some_and(|s| s.hide_week),
        latest_at: chs.iter().map(uploaded_secs).max().unwrap_or(0),
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
    /// New since this profile last read the series.
    pub fresh: bool,
}

pub async fn detail(app: &AppHandle, key: &str) -> Result<Detail, String> {
    let uid = uid(app)?;
    let f = find(app, key)?;
    let chs = chapter_list(&f).await?;
    let q = crate::suwayomi::queue().await;
    let spot = spots(app, &uid).remove(key);
    let rows = chs
        .into_iter()
        .map(|ch| {
            let read = spot.as_ref().is_some_and(|s| s.read.contains_key(&ch.id));
            let progress = spot.as_ref().filter(|s| s.chapter == ch.id && !read && s.pages > 0).map(|s| (s.page + 1) as f64 / s.pages as f64);
            let queued = q.iter().find(|x| x.1 == ch.id).map(|x| x.2);
            let fresh = is_fresh(&ch, &f, spot.as_ref());
            ChapterRow { ch, read, progress, queued, fresh }
        })
        .collect::<Vec<_>>();
    let mode = spot.as_ref().and_then(|s| s.mode.clone()).unwrap_or_else(|| f.mode.clone());
    Ok(Detail { downloading: rows.iter().filter(|r| r.queued.is_some()).count(), followed: Some(f), chapters: rows, spot, mode })
}

/// Marks chapters read or unread for the watching profile. Marked read past where
/// you are, your place moves on to them: Resume and the clean-up go by it.
pub async fn mark(app: &AppHandle, key: &str, chapters: &[i64], read: bool) -> Result<(), String> {
    let uid = uid(app)?;
    known(app, key)?;
    let order: Vec<i64> = if read && key.starts_with('m') {
        match find(app, key) {
            Ok(f) => chapter_list(&f).await.map(|chs| chs.iter().map(|c| c.id).collect()).unwrap_or_default(),
            Err(_) => vec![],
        }
    } else {
        vec![]
    };
    let mut all = spots(app, &uid);
    let s = all.entry(key.to_string()).or_default();
    for c in chapters {
        if read {
            s.read.entry(*c).or_insert(now());
        } else {
            s.read.remove(c);
        }
    }
    if read {
        move_on(s, &order);
    }
    save_spots(app, &uid, &all)
}

/// Moves a place to the furthest chapter marked read, when that's past it.
fn move_on(s: &mut Spot, order: &[i64]) {
    let pos = |id: i64| order.iter().position(|&c| c == id);
    let Some(far) = s.read.keys().filter_map(|&id| pos(id)).max() else { return };
    if s.chapter == 0 || pos(s.chapter).is_none_or(|at| at < far) {
        s.chapter = order[far];
        s.page = 0;
        s.pages = 0;
        s.at = now();
    }
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
    /// AniList marks the title 18+.
    #[serde(default)]
    pub adult: bool,
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
/// The copies of a series in one language, best first, and what Best takes.
#[derive(Serialize)]
pub struct Copies {
    pub list: Vec<crate::suwayomi::Candidate>,
    /// Best's main copy and its fill, as indexes into `list`.
    pub best: Option<usize>,
    pub fill: Option<usize>,
    /// A copy with chapters already on disk (kept when it was unfollowed): picked first.
    pub kept: Option<usize>,
}

pub async fn sources(app: &AppHandle, meta: &Meta, lang: &str) -> Result<Copies, String> {
    if !lang_ok(lang) {
        return Err("Unknown language.".into());
    }
    ensure_root(app).await?;
    crate::suwayomi::ready(app).await?;
    // 18+ sources only for an 18+ title, and only for those allowed (adult_ok).
    let adult = meta.adult && adult_ok(app);
    if adult {
        crate::suwayomi::ensure_adult().await;
    }
    let list = crate::suwayomi::candidates(&meta.all_names(), lang, adult).await?;
    let pair = crate::suwayomi::best_pair(&list);
    let has_files = |c: &crate::suwayomi::Candidate| {
        series_dir(app, &c.source, &c.title).and_then(|d| std::fs::read_dir(d).ok()).is_some_and(|mut it| it.next().is_some())
    };
    let kept = list.iter().position(has_files);
    // Following again: the chapters you kept are read from where they are.
    let (best, fill) = match kept {
        Some(k) => (Some(k), None),
        None => (pair.map(|p| p.0), pair.and_then(|p| p.1)),
    };
    Ok(Copies { best, fill, kept, list })
}

/// A language code as sources name them: "en", "pt-BR", "zh-Hans".
pub fn lang_ok(l: &str) -> bool {
    (2..=10).contains(&l.len()) && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// A language's own name, for the picker.
pub fn lang_name(code: &str) -> String {
    match code {
        "en" => "English", "es" => "Español", "es-419" => "Español (Latinoamérica)", "pt" => "Português", "pt-BR" => "Português (Brasil)",
        "fr" => "Français", "de" => "Deutsch", "it" => "Italiano", "ru" => "Русский", "uk" => "Українська", "pl" => "Polski", "tr" => "Türkçe",
        "id" => "Bahasa Indonesia", "vi" => "Tiếng Việt", "th" => "ไทย", "ar" => "العربية", "ja" => "日本語", "ko" => "한국어",
        "zh" | "zh-Hans" => "中文 (简体)", "zh-Hant" | "zh-hant" => "中文 (繁體)", "hi" => "हिन्दी", "ms" => "Bahasa Melayu", "fil" | "tl" => "Filipino",
        "nl" => "Nederlands", "cs" => "Čeština", "hu" => "Magyar", "ro" => "Română", "bg" => "Български", "el" => "Ελληνικά", "he" => "עברית",
        "sv" => "Svenska", "da" => "Dansk", "no" | "nb" => "Norsk", "fi" => "Suomi", "ca" => "Català", "fa" => "فارسی", "bn" => "বাংলা",
        other => return other.to_string(),
    }
    .to_string()
}

/// Every language a series can be got in, with its own name.
pub async fn languages(app: &AppHandle) -> Result<Vec<Value>, String> {
    ensure_root(app).await?;
    crate::suwayomi::ready(app).await?;
    Ok(crate::suwayomi::languages().await?.into_iter().map(|(code, n)| json!({ "code": code, "name": lang_name(&code), "sources": n })).collect())
}

#[derive(Serialize)]
pub struct PickRow {
    #[serde(flatten)]
    pub ch: crate::suwayomi::Chapter,
    /// From the fill copy (the site further ahead).
    pub fill: bool,
}

/// A copy's chapters (with a fill, both merged), before following: so they can be picked.
pub async fn copy_chapters(app: &AppHandle, manga_id: i64, fill_id: Option<i64>) -> Result<Vec<PickRow>, String> {
    crate::suwayomi::ready(app).await?;
    let mut lists = vec![crate::suwayomi::chapters(manga_id).await?];
    if let Some(f) = fill_id {
        lists.push(crate::suwayomi::chapters(f).await.unwrap_or_default());
    }
    Ok(merge_copies(clean_copies(lists)).into_iter().map(|(ch, i)| PickRow { ch, fill: i > 0 }).collect())
}

/// What to download when following.
#[derive(Deserialize, Clone, Debug, Default)]
pub struct Pick {
    /// Chapters picked one by one; None: every chapter from `from` on.
    pub ids: Option<Vec<i64>>,
    pub from: Option<f64>,
    #[serde(default = "yes")]
    pub new_too: bool,
}

/// Gets chapters of a series already followed.
pub async fn get_chapters(app: &AppHandle, key: &str, ids: &[i64]) -> Result<String, String> {
    let f = find(app, key)?;
    let chs = chapter_list(&f).await?;
    let want: Vec<i64> = chs.iter().filter(|c| !c.downloaded && ids.contains(&c.id)).map(|c| c.id).collect();
    if want.is_empty() {
        return Ok("Those are already here.".into());
    }
    crate::suwayomi::enqueue(&want).await?;
    if f.removed.iter().any(|id| want.contains(id)) {
        update_followed(app, key, |x| x.removed.retain(|id| !want.contains(id)));
    }
    Ok(match want.len() {
        1 => format!("Getting 1 chapter of {}.", f.title),
        n => format!("Getting {n} chapters of {}.", f.title),
    })
}

/// Stops what a series has downloading.
pub async fn stop_chapters(app: &AppHandle, key: &str) -> Result<String, String> {
    let f = find(app, key)?;
    let queued: Vec<i64> = crate::suwayomi::queue().await.into_iter().filter(|q| f.has_manga(q.0)).map(|q| q.1).collect();
    crate::suwayomi::dequeue(&queued).await?;
    Ok(format!("Stopped the downloads of {}.", f.title))
}

/// Follows the series from `pick` (a candidate from `sources`), downloading
/// every chapter from `from` on. Following it again from another source
/// replaces the old one; what was read is kept.
pub async fn follow(app: &AppHandle, meta: Meta, pick: crate::suwayomi::Candidate, fill: Option<crate::suwayomi::Candidate>, sel: Pick, lang: Option<String>) -> Result<String, String> {
    let from = sel.from;
    if lang.as_deref().is_some_and(|l| !lang_ok(l)) {
        return Err("Unknown language.".into());
    }
    ensure_root(app).await?;
    crate::suwayomi::ready(app).await?;
    let me = uid(app)?;
    let pc_owner = crate::profiles::owner_id(app);
    // Someone else already follows it: this profile joins. Nothing downloads
    // twice and their copy stays as it is; picked chapters still come.
    let others = followed(app).into_iter().find(|f| meta.anilist.is_some() && f.anilist == meta.anilist && !owned_by(&f.owners, pc_owner.as_deref(), &me));
    if let Some(f) = others {
        let mut owners = owners_of(&f.owners, pc_owner.as_deref());
        owners.push(me.clone());
        update_followed(app, &f.key, |x| x.owners = owners);
        if let Some(ids) = sel.ids.as_ref().filter(|ids| !ids.is_empty() && f.has_manga(pick.manga_id)) {
            let _ = crate::suwayomi::enqueue(ids).await;
        }
        let msg = format!("{} is on your list now. Its chapters are already here.", f.title);
        crate::log::log(app, crate::model::Level::Ok, &msg);
        return Ok(msg);
    }
    // Where its chapters land is Suwayomi's say, never the page's: the folder
    // names come from the downloader itself and are checked to be plain names.
    let (source, source_title) = crate::suwayomi::folder_names(pick.manga_id).await?;
    let fill = match fill.filter(|x| x.manga_id != pick.manga_id) {
        Some(x) => {
            let (s, t) = crate::suwayomi::folder_names(x.manga_id).await?;
            vec![Fill { manga_id: x.manga_id, source: s, source_title: t, width: x.width }]
        }
        None => vec![],
    };
    // What to get, out of every copy merged: each chapter from one copy only.
    let mut lists = vec![crate::suwayomi::chapters(pick.manga_id).await?];
    for x in &fill {
        lists.push(crate::suwayomi::chapters(x.manga_id).await.unwrap_or_default());
    }
    let merged = merge_copies(clean_copies(lists));
    let known_upto = top_number(merged.iter().map(|x| &x.0));
    let wanted: Vec<i64> = merged
        .into_iter()
        .map(|x| x.0)
        .filter(|c| !c.downloaded)
        .filter(|c| match &sel.ids {
            Some(p) => p.contains(&c.id),
            None => from.is_none_or(|f| c.number.unwrap_or(f64::MAX) >= f),
        })
        .map(|c| c.id)
        .collect();
    crate::suwayomi::follow(pick.manga_id, from, Some(&[])).await?;
    for x in &fill {
        crate::suwayomi::follow(x.manga_id, from, Some(&[])).await?;
    }
    if !wanted.is_empty() {
        crate::suwayomi::enqueue(&wanted).await?;
    }
    let queued = wanted.len();
    let all = followed(app);
    let mode = read_core::default_mode(meta.country.as_deref(), meta.format.as_deref()).to_string();
    let key = format!("m{}", pick.manga_id);
    // The same series from another source: the old one stops.
    let old: Vec<Followed> = all.iter().filter(|f| f.key != key && meta.anilist.is_some() && f.anilist == meta.anilist).cloned().collect();
    for o in &old {
        for (id, _, _) in o.copies() {
            if id != pick.manga_id && !fill.iter().any(|x| x.manga_id == id) {
                let _ = crate::suwayomi::unfollow(id).await;
            }
        }
    }
    let was_live = all.iter().any(|f| (f.live || f.new_too) && (f.key == key || old.iter().any(|o| o.key == f.key)));
    // Whoever followed the old copy follows the new one, and so does this profile.
    let mut owners: Vec<String> = all.iter().filter(|f| f.key == key || old.iter().any(|o| o.key == f.key)).flat_map(|f| owners_of(&f.owners, pc_owner.as_deref())).collect();
    owners.push(me.clone());
    owners.sort();
    owners.dedup();
    // Read again now the waiting is over: a background check may have saved meanwhile.
    let _held = followed_lock();
    let mut all = followed(app);
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
        lang,
        // A changed copy keeps getting new chapters; what's been told starts over quietly.
        new_too: sel.new_too || was_live,
        known_upto,
        fill: fill.clone(),
        live: false,
        told: None,
        ahead_told: None,
        owners,
        removed: vec![],
    });
    save_followed(app, &all)?;
    let msg = match queued {
        0 if sel.new_too => format!("Following {}. New chapters download by themselves.", meta.title),
        0 => format!("{} is on your list.", meta.title),
        1 => format!("Getting 1 chapter of {}.", meta.title),
        n => format!("Getting {n} chapters of {}.", meta.title),
    };
    crate::log::log(app, crate::model::Level::Ok, &msg);
    Ok(msg)
}

/// Stops following; with `delete`, its downloaded chapters go too.
///
/// It leaves the list first, so it's gone from the page at once and a second
/// press can't find it half done. Suwayomi is told after: deleting a few
/// hundred chapters takes a while, and a hiccup there is tried again in the
/// background instead of failing the whole thing.
pub async fn unfollow(app: &AppHandle, key: &str, delete: bool) -> Result<String, String> {
    let f = find(app, key)?;
    // Others still follow it: it only leaves this profile's list.
    let me = uid(app)?;
    let mut owners = owners_of(&f.owners, crate::profiles::owner_id(app).as_deref());
    owners.retain(|o| o != &me);
    if !owners.is_empty() {
        update_followed(app, key, |x| x.owners = owners);
        return Ok(format!("Took {} off your list. Others still read it, so its chapters stay.", f.title));
    }
    {
        let _held = followed_lock();
        let mut all = followed(app);
        all.retain(|x| x.key != key);
        save_followed(app, &all)?;
    }
    let copies: Vec<i64> = f.copies().into_iter().map(|c| c.0).collect();
    let left = let_go(&copies, delete).await;
    if left.is_empty() {
        return Ok(format!("Stopped following {}.", f.title));
    }
    // Tried again a few times, quietly; the series is already off the list.
    let (app2, title) = (app.clone(), f.title.clone());
    tauri::async_runtime::spawn(async move {
        let mut left = left;
        for _ in 0..4 {
            tokio::time::sleep(Duration::from_secs(20)).await;
            // Followed again meanwhile: that copy is wanted now, leave it be.
            let again: Vec<i64> = followed(&app2).iter().flat_map(|f| f.copies().into_iter().map(|c| c.0)).collect();
            left.retain(|id| !again.contains(id));
            if left.is_empty() {
                return;
            }
            left = let_go(&left, delete).await;
            if left.is_empty() {
                return;
            }
        }
        crate::log::log(&app2, crate::model::Level::Amber, format!("Stopped following {title}, but the manga downloader didn't let go of it. Its chapters may still be on disk."));
    });
    Ok(format!("Stopped following {}. Its chapters are being cleaned up.", f.title))
}

/// Takes each copy out of Suwayomi (its chapters too with `delete`); the ones it didn't take.
async fn let_go(copies: &[i64], delete: bool) -> Vec<i64> {
    let mut left = Vec::new();
    for &id in copies {
        let gone = async {
            if delete {
                let chs = crate::suwayomi::chapters(id).await?;
                let ids: Vec<i64> = chs.iter().filter(|c| c.downloaded).map(|c| c.id).collect();
                crate::suwayomi::delete_chapters(&ids).await?;
            }
            crate::suwayomi::unfollow(id).await
        }
        .await;
        if let Err(e) = gone {
            crate::log::detail(format!("Reading: letting go of manga {id}: {e}"));
            left.push(id);
        }
    }
    left
}

// ---------- clean-up of read chapters ----------

/// Chapters to delete: read by someone at least `days` ago, and behind every
/// profile that is reading the series (a profile further back keeps them).
pub fn cleanup_pick(chs: &[crate::suwayomi::Chapter], spots: &[&Spot], days: u32, now: i64, whole: bool) -> Vec<i64> {
    if days == 0 || spots.is_empty() {
        return vec![];
    }
    let pos = |id: i64| chs.iter().position(|c| c.id == id);
    // Each reader's furthest point: the chapter they're on, unless it's read.
    let behind_all = spots.iter().map(|s| pos(s.chapter).map(|p| if s.read.contains_key(&s.chapter) { p + 1 } else { p }).unwrap_or(0)).min().unwrap_or(0);
    if whole {
        // The whole series, once everyone has read all of it, `days` after the last read.
        let last_read = spots.iter().flat_map(|s| chs.iter().filter_map(|c| s.read.get(&c.id))).max().copied().unwrap_or(now);
        if behind_all < chs.len() || now - last_read < days as i64 * 86_400 {
            return vec![];
        }
        return chs.iter().filter(|c| c.downloaded).map(|c| c.id).collect();
    }
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
        Err(e) => crate::log::log(app, crate::model::Level::Amber, format!("Reading isn't set up yet: {}. Chillarr tries again next start.", e.trim_end_matches('.'))),
    }
}

/// New chapters of what's followed, one copy each (Suwayomi's own auto
/// download takes every group's copy). A chapter someone has read isn't
/// fetched again: the clean-up may have deleted it on purpose.
pub async fn fetch_new(app: &AppHandle) {
    // Two loops call this (keepup.rs and waiting.rs): one at a time, or the
    // same chapters were queued and told about twice.
    // A call that finds one running asks it to go round once more, so what a
    // site just listed is still fetched now rather than at the next round.
    use std::sync::atomic::{AtomicBool, Ordering};
    static BUSY: AtomicBool = AtomicBool::new(false);
    static AGAIN: AtomicBool = AtomicBool::new(false);
    struct Free;
    impl Drop for Free {
        fn drop(&mut self) {
            BUSY.store(false, Ordering::Release);
        }
    }
    if BUSY.swap(true, Ordering::AcqRel) {
        AGAIN.store(true, Ordering::Release);
        return;
    }
    let _free = Free;
    loop {
        AGAIN.store(false, Ordering::Release);
        fetch_new_once(app).await;
        if !AGAIN.load(Ordering::Acquire) {
            break;
        }
    }
}

async fn fetch_new_once(app: &AppHandle) {
    let list = followed(app);
    if list.is_empty() {
        return;
    }
    let profiles = all_spots(app);
    let queued: std::collections::HashSet<i64> = crate::suwayomi::queue().await.into_iter().map(|q| q.1).collect();
    for f in list.into_iter().filter(|f| f.new_too) {
        let Ok(chs) = chapter_list(&f).await else { continue };
        let top = top_number(chs.iter());
        // A mark saved from a stray number comes down, so what follows the real newest is fetched.
        let lowered = lowered_mark(f.known_upto, top);
        if lowered != f.known_upto {
            update_followed(app, &f.key, |x| x.known_upto = lowered);
        }
        // Not known yet (followed before this was kept): what's listed now is the line.
        let Some(mark) = lowered else {
            if top.is_some() {
                update_followed(app, &f.key, |x| x.known_upto = top);
            }
            continue;
        };
        // Deleted by hand counts like read: it was let go on purpose.
        let read_by_someone = |id: i64| f.removed.contains(&id) || profiles.iter().filter_map(|p| p.get(&f.key)).any(|s| s.read.contains_key(&id));
        let want = new_wanted(&chs, &queued, &read_by_someone, f.from, mark);
        // The mark moves only once what's new is queued: a failed queue is tried again next round.
        let queued_ok = want.is_empty() || crate::suwayomi::enqueue(&want).await.is_ok();
        if !want.is_empty() && queued_ok {
            crate::log::log(app, crate::model::Level::Ok, format!("Getting {} new chapter(s) of {}.", want.len(), f.title));
        }
        if let Some(t) = top.filter(|&t| t > mark && queued_ok) {
            update_followed(app, &f.key, |x| x.known_upto = Some(t));
        }
    }
}

/// The highest chapter number in a list, if any has one.
pub fn top_number<'a>(chs: impl Iterator<Item = &'a crate::suwayomi::Chapter>) -> Option<f64> {
    chs.filter_map(|c| c.number).fold(None, |m: Option<f64>, n| Some(m.map_or(n, |m| m.max(n))))
}

/// The chapters to fetch as new: only ones the site listed after the series was
/// followed (above `mark`). Older ones left out were left out on purpose, and
/// read ones the clean-up deleted must not come back.
pub fn new_wanted(chs: &[crate::suwayomi::Chapter], queued: &std::collections::HashSet<i64>, read_by_someone: &dyn Fn(i64) -> bool, from: Option<f64>, mark: f64) -> Vec<i64> {
    chs.iter()
        .filter(|c| !c.downloaded && !queued.contains(&c.id) && !read_by_someone(c.id))
        .filter(|c| from.is_none_or(|from| c.number.unwrap_or(f64::MAX) >= from))
        .filter(|c| c.number.is_some_and(|n| n > mark))
        .map(|c| c.id)
        .collect()
}

pub async fn clean_up(app: &AppHandle) {
    let (days, whole) = {
        let st = app.state::<crate::SettingsState>();
        let s = st.current.lock().unwrap();
        (s.read_cleanup_days, s.read_cleanup_whole)
    };
    if days == 0 {
        return;
    }
    let profiles = all_spots(app);
    for f in followed(app) {
        let readers: Vec<&Spot> = profiles.iter().filter_map(|p| p.get(&f.key)).collect();
        let Ok(chs) = chapter_list(&f).await else { continue };
        let doomed = cleanup_pick(&chs, &readers, days, now(), whole);
        if !doomed.is_empty() && crate::suwayomi::delete_chapters(&doomed).await.is_ok() {
            crate::log::log(app, crate::model::Level::Ok, format!("Deleted {} read chapter(s) of {}.", doomed.len(), f.title));
        }
    }
}

// ---------- My List for reading (per profile) ----------

const LIST_MAX: usize = 1000;

fn list_file(dir: &Path, uid: &str) -> PathBuf {
    dir.join(format!("read-list-{uid}.json"))
}

/// A card as it may be kept: only the fields a card shows, each cut short.
pub fn keep_card(c: &Value) -> Option<(String, Value)> {
    let s = |k: &str, n: usize| c[k].as_str().map(|v| v.chars().take(n).collect::<String>());
    let key = match (c["anilist"].as_i64(), s("olid", 80)) {
        (Some(a), _) => format!("al:{a}"),
        (None, Some(o)) => format!("ol:{o}"),
        _ => return None,
    };
    let url = |k: &str| s(k, 400).filter(|u| u.starts_with("https://") || u.starts_with("/read/"));
    let names: Vec<String> = c["names"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).take(12).map(|x| x.chars().take(200).collect()).collect()).unwrap_or_default();
    let genres: Vec<String> = c["genres"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).take(12).map(|x| x.chars().take(40).collect()).collect()).unwrap_or_default();
    Some((
        key,
        json!({
            "kind": s("kind", 10), "anilist": c["anilist"].as_i64(), "olid": s("olid", 80), "title": s("title", 300)?, "names": names,
            "author": s("author", 200), "cover": url("cover"), "banner": url("banner"), "country": s("country", 4), "format": s("format", 20),
            "status": s("status", 30), "chapters": c["chapters"].as_i64(), "score": c["score"].as_i64(), "genres": genres,
            "description": s("description", 4000), "year": c["year"].as_i64(),
        }),
    ))
}

pub fn my_list(app: &AppHandle) -> Result<Vec<Value>, String> {
    let uid = uid(app)?;
    let list: Vec<(String, Value)> = load_json(&list_file(&data_dir(app)?, &uid));
    Ok(list.into_iter().map(|(_, v)| v).collect())
}

/// Adds a card to the watching profile's reading list, or takes it off.
pub fn toggle_list(app: &AppHandle, card: &Value, on: bool) -> Result<bool, String> {
    let uid = uid(app)?;
    let (key, keep) = keep_card(card).ok_or("That can't go on your list.")?;
    let path = list_file(&data_dir(app)?, &uid);
    let mut list: Vec<(String, Value)> = load_json(&path);
    list.retain(|(k, _)| k != &key);
    if on {
        if list.len() >= LIST_MAX {
            return Err("Your reading list is full.".into());
        }
        list.insert(0, (key, keep));
    }
    save_json(&path, &list)?;
    Ok(on)
}

/// What reading has downloading: each series with chapters queued, and books on their way.
pub async fn downloads(app: &AppHandle) -> Result<Value, String> {
    let q = crate::suwayomi::queue().await;
    let series: Vec<Value> = followed(app)
        .into_iter()
        .filter_map(|f| {
            let mine: Vec<&(i64, i64, f64)> = q.iter().filter(|x| f.has_manga(x.0)).collect();
            (!mine.is_empty()).then(|| json!({ "key": f.key, "title": f.title, "cover": f.cover, "country": f.country, "chapters": mine.len(), "progress": mine.iter().map(|x| x.2).sum::<f64>() / mine.len() as f64 }))
        })
        .collect();
    let books: Vec<Value> = books(app).await?.into_iter().filter(|b| !b.ready).map(|b| json!(b)).collect();
    Ok(json!({ "series": series, "books": books }))
}

// ---------- discovery (AniList) ----------

const MEDIA_FIELDS: &str = "id isAdult title{romaji english native} synonyms coverImage{extraLarge large color} bannerImage countryOfOrigin format status chapters volumes averageScore genres description(asHtml:false) startDate{year}";

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
        "adult": m["isAdult"] == true,
    })
}

/// AniList descriptions carry <br> and <i>: plain text for the page.
pub(crate) fn clean_text(s: &str) -> String {
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

/// One page of a reading tab's grid: manga, manhwa, manhua, novels or all comics,
/// with filters like Movies': genres and tags in or out, status, rating, years,
/// length, and 18+ (hidden, included or only, for those allowed).
#[derive(Deserialize, Default, Debug)]
pub struct Explore {
    pub kind: String,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub not_genres: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub not_tags: Vec<String>,
    pub status: Option<String>,
    pub sort: Option<String>,
    /// 0..100, AniList's average score.
    #[serde(default)]
    pub min_score: Option<u32>,
    #[serde(default)]
    pub year_from: Option<u32>,
    #[serde(default)]
    pub year_to: Option<u32>,
    #[serde(default)]
    pub chapters_min: Option<u32>,
    #[serde(default)]
    pub chapters_max: Option<u32>,
    /// "hide" (the default), "include" or "only".
    #[serde(default)]
    pub adult: Option<String>,
    /// Porn inside 18+: "hide" (the default) or "include".
    #[serde(default)]
    pub porn: Option<String>,
    #[serde(default)]
    pub page: u32,
}

/// 18+ in reading: every profile may turn it on (the owner's call, 2026-11-08:
/// "it's a universal thing"). It stays off until someone picks Include or Only
/// 18+ in the filters, and 18+ sources are only searched for 18+ titles. The
/// one place to narrow it again.
pub fn adult_ok(app: &AppHandle) -> bool {
    // A kids' profile never gets it (profiles.rs).
    !crate::profiles::viewer_kids(app)
}

pub fn explore_args(e: &Explore, adult_allowed: bool) -> Result<String, String> {
    let mut a = vec!["type:MANGA".to_string()];
    // 18+ is Ecchi and the like; porn is AniList's isAdult (see adult_rule).
    let rule = if adult_allowed { crate::catalog::adult_rule(e.adult.as_deref(), e.porn.as_deref()) } else { crate::catalog::adult_rule(None, None) };
    match rule.is_adult {
        Some(true) => a.push("isAdult:true".into()),
        Some(false) => a.push("isAdult:false".into()),
        None => {}
    }
    let mut genres = e.genres.clone();
    let mut not_genres = e.not_genres.clone();
    match rule.ecchi {
        Some(true) if !genres.iter().any(|g| g == "Ecchi") => genres.push("Ecchi".into()),
        // Picking the Ecchi chip asks for it by itself.
        Some(false) if !not_genres.iter().chain(&genres).any(|g| g == "Ecchi") => not_genres.push("Ecchi".into()),
        _ => {}
    }
    match e.kind.as_str() {
        "manga" => a.extend(["countryOfOrigin:\"JP\"".into(), "format_in:[MANGA,ONE_SHOT]".into()]),
        "manhwa" => a.push("countryOfOrigin:\"KR\"".into()),
        "manhua" => a.push("countryOfOrigin:\"CN\"".into()),
        "novels" => a.push("format:NOVEL".into()),
        "all" => a.push("format_in:[MANGA,ONE_SHOT]".into()),
        _ => return Err("Unknown kind.".into()),
    }
    // Names go into the query as strings: letters, digits, spaces, - and ' only.
    let word = |s: &str| !s.is_empty() && s.len() <= 40 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '\'');
    let list = |v: &[String]| -> Vec<String> { v.iter().filter(|g| word(g)).take(10).map(|g| format!("\"{g}\"")).collect() };
    let pairs = [("genre_in", list(&genres)), ("genre_not_in", list(&not_genres)), ("tag_in", list(&e.tags)), ("tag_not_in", list(&e.not_tags))];
    for (k, v) in pairs {
        if !v.is_empty() {
            a.push(format!("{k}:[{}]", v.join(",")));
        }
    }
    if let Some(s) = e.min_score.filter(|s| (1..100).contains(s)) {
        a.push(format!("averageScore_greater:{}", s - 1));
    }
    // Years as AniList's dates (YYYYMMDD), between 1900 and 2100.
    let year = |y: Option<u32>| y.filter(|y| (1900..=2100).contains(y));
    if let Some(y) = year(e.year_from) {
        a.push(format!("startDate_greater:{}", y * 10000));
    }
    if let Some(y) = year(e.year_to) {
        a.push(format!("startDate_lesser:{}", (y + 1) * 10000));
    }
    if let Some(n) = e.chapters_min.filter(|n| *n > 0 && *n < 100_000) {
        a.push(format!("chapters_greater:{}", n - 1));
    }
    if let Some(n) = e.chapters_max.filter(|n| *n > 0 && *n < 100_000) {
        a.push(format!("chapters_lesser:{}", n + 1));
    }
    if let Some(s) = e.status.as_deref().filter(|s| matches!(*s, "RELEASING" | "FINISHED" | "HIATUS" | "NOT_YET_RELEASED")) {
        a.push(format!("status:{s}"));
    }
    let sort = match e.sort.as_deref() {
        Some("trending") => "TRENDING_DESC",
        Some("score") => "SCORE_DESC",
        Some("new") => "START_DATE_DESC",
        Some("title") => "TITLE_ENGLISH",
        _ => "POPULARITY_DESC",
    };
    a.push(format!("sort:{sort}"));
    Ok(a.join(","))
}

/// The genres a Manga, Manhwa, Manhua or Novels tab has a row for, in order.
pub const KIND_GENRES: [&str; 14] = ["Action", "Fantasy", "Romance", "Comedy", "Drama", "Adventure", "Slice of Life", "Supernatural", "Mystery", "Psychological", "Sci-Fi", "Horror", "Sports", "Thriller"];

/// The row ids of a reading tab: "trending", "popular", "top", "ongoing",
/// "new", then "g:<Genre>". Each is also how its "Explore all" page asks
/// (explore_for_row), so the row and its page always agree.
pub fn kind_row_query(kind: &str, id: &str) -> Option<Explore> {
    let base = Explore { kind: kind.to_string(), ..Default::default() };
    let year = chrono::Datelike::year(&chrono::Utc::now()) as u32;
    Some(match id {
        "trending" => Explore { sort: Some("trending".into()), ..base },
        "popular" => Explore { sort: Some("popular".into()), ..base },
        "top" => Explore { sort: Some("score".into()), ..base },
        "ongoing" => Explore { sort: Some("popular".into()), status: Some("RELEASING".into()), ..base },
        "new" => Explore { sort: Some("popular".into()), year_from: Some(year - 1), ..base },
        g => {
            let g = g.strip_prefix("g:")?;
            if !KIND_GENRES.contains(&g) {
                return None;
            }
            Explore { sort: Some("popular".into()), genres: vec![g.to_string()], ..base }
        }
    })
}

/// A Manga, Manhwa, Manhua or Novels tab's rows, like Movies' tabs: trending,
/// most popular, top rated, ongoing and new, then one row per genre. A few
/// AniList calls (it limits how much one call may ask), kept six hours.
/// Never 18+: those are for the filters, where it can be turned on.
pub async fn kind_rows(app: &AppHandle, kind: &str) -> Result<Value, String> {
    if !matches!(kind, "manga" | "manhwa" | "manhua" | "novels") {
        return Err("Unknown kind.".into());
    }
    let state = app.state::<Reading>();
    let mut cache = state.kind_rows.lock().await;
    if let Some((at, v)) = cache.get(kind) {
        if at.elapsed() < ROWS_LIFE {
            return Ok(v.clone());
        }
    }
    let what = match kind { "manga" => "manga", "manhwa" => "manhwa", "manhua" => "manhua", _ => "light novels" };
    let mut defs: Vec<(String, String)> = vec![
        ("trending".into(), "Trending now".into()),
        ("popular".into(), format!("Most popular {what}")),
        ("top".into(), "Top rated".into()),
        ("ongoing".into(), "Ongoing".into()),
        ("new".into(), "New and rising".into()),
    ];
    defs.extend(KIND_GENRES.iter().map(|g| (format!("g:{g}"), g.to_string())));
    let mut out = Vec::new();
    for chunk in defs.chunks(7) {
        let mut q = String::from("{");
        for (i, (id, _)) in chunk.iter().enumerate() {
            let Some(e) = kind_row_query(kind, id) else { continue };
            let args = explore_args(&e, false)?;
            q.push_str(&format!("r{i}:Page(perPage:24){{media({args}){{{MEDIA_FIELDS}}}}} "));
        }
        q.push('}');
        let v = crate::media::anilist(&json!({ "query": q })).await?;
        if v["data"].is_null() {
            return Err("AniList didn't answer. Try again in a minute.".into());
        }
        for (i, (id, title)) in chunk.iter().enumerate() {
            let items: Vec<Value> = v["data"][format!("r{i}")]["media"].as_array().map(|a| a.iter().map(card).collect()).unwrap_or_default();
            if items.len() >= 4 {
                out.push(json!({ "id": id, "title": title, "items": items }));
            }
        }
    }
    let v = json!(out);
    cache.insert(kind.to_string(), (Instant::now(), v.clone()));
    Ok(v)
}

pub async fn explore(app: &AppHandle, e: &Explore) -> Result<Value, String> {
    let args = explore_args(e, adult_ok(app))?;
    let page = e.page.clamp(1, 200);
    let q = format!("{{Page(page:{page},perPage:40){{pageInfo{{hasNextPage}} media({args}){{{MEDIA_FIELDS}}}}}}}");
    let v = crate::media::anilist(&json!({ "query": q })).await?;
    let p = &v["data"]["Page"];
    Ok(json!({ "items": p["media"].as_array().map(|a| a.iter().map(card).collect::<Vec<_>>()).unwrap_or_default(), "more": p["pageInfo"]["hasNextPage"] }))
}

/// Tags people filter manga by most (AniList's names), for More filters.
pub const TAGS: [&str; 72] = [
    // The first 20 show as Themes beside the genres; the rest under More filters.
    "Isekai", "Reincarnation", "Regression", "Revenge", "Murim", "Cultivation", "Martial Arts", "Dungeon", "Super Power", "Magic",
    "Overpowered Main Characters", "Villainess", "Female Protagonist", "Male Protagonist", "Royal Affairs", "Female Harem", "School", "Time Skip", "Survival", "Boys' Love",
    "Swordplay", "Demons", "Gods", "Mythology", "Nobility", "Anti-Hero", "Male Harem", "Office Lilies", "Love Triangle", "Age Gap",
    "Yuri", "Tragedy", "Post-Apocalyptic", "Video Games", "Virtual World", "Zombie", "Vampire", "Historical", "Full Color", "Long Strip",
    "Shounen", "Seinen", "Shoujo", "Josei", "Ninja", "Samurai", "Assassins", "Kingdom Management", "Politics", "Military",
    "War", "Kemonomimi", "Elf", "Dragons", "Witch", "Necromancy", "Alchemy", "Cooking", "Family Life", "Coming of Age",
    "Body Swapping", "Time Manipulation", "Tsundere", "Yandere", "Twins", "Delinquents", "Gyaru", "Gender Bending", "Monster Girl", "Teacher",
    "Pirates", "Heterosexual",
];

/// AniList's manga genres, for the filter chips (A to Z).
pub const GENRES: [&str; 18] = ["Action", "Adventure", "Comedy", "Drama", "Ecchi", "Fantasy", "Horror", "Mahou Shoujo", "Mecha", "Music", "Mystery", "Psychological", "Romance", "Sci-Fi", "Slice of Life", "Sports", "Supernatural", "Thriller"];

/// "Similar to this": AniList's readers' recommendations for a series, best rated
/// first, adult ones only for those allowed 18+.
pub async fn similar(app: &AppHandle, anilist: i64) -> Result<Vec<Value>, String> {
    if anilist <= 0 {
        return Ok(vec![]);
    }
    let q = format!("query($id:Int){{Media(id:$id,type:MANGA){{recommendations(sort:RATING_DESC,perPage:18){{nodes{{rating mediaRecommendation{{isAdult {MEDIA_FIELDS}}}}}}}}}}}");
    let v = crate::media::anilist(&json!({ "query": q, "variables": { "id": anilist } })).await?;
    let adult = adult_ok(app);
    Ok(v["data"]["Media"]["recommendations"]["nodes"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|n| n["rating"].as_i64().unwrap_or(0) > 0)
                .map(|n| &n["mediaRecommendation"])
                .filter(|m| m.is_object() && (adult || m["isAdult"] != true))
                .map(card)
                .collect()
        })
        .unwrap_or_default())
}

/// The details under a series' description, like a film's: who made it, how
/// many chapters and volumes, how far along it is, and when it ran.
pub async fn about(anilist: i64) -> Result<Vec<(String, String)>, String> {
    if anilist <= 0 {
        return Ok(vec![]);
    }
    let q = "query($id:Int){Media(id:$id,type:MANGA){format status chapters volumes genres countryOfOrigin source averageScore synonyms startDate{year month day} endDate{year month day} staff(perPage:12,sort:RELEVANCE){edges{role node{name{full}}}} tags{name rank isMediaSpoiler}}}";
    let v = crate::media::anilist(&json!({ "query": q, "variables": { "id": anilist } })).await?;
    Ok(about_facts(&v["data"]["Media"]))
}

/// The roles that count as who made a series: its writers and artists.
const MAKER_ROLES: [&str; 6] = ["Story", "Art", "Original Creator", "Original Story", "Author", "Illustration"];

/// Who made a series (story, art, original creator), with AniList's staff ids,
/// so each name opens everything else they made, like a film's director.
pub async fn makers(anilist: i64) -> Result<Vec<Value>, String> {
    if anilist <= 0 {
        return Ok(vec![]);
    }
    let q = "query($id:Int){Media(id:$id,type:MANGA){staff(perPage:12,sort:RELEVANCE){edges{role node{id name{full}}}}}}";
    let v = crate::media::anilist(&json!({ "query": q, "variables": { "id": anilist } })).await?;
    Ok(makers_of(&v["data"]["Media"]))
}

/// One entry per person, their roles joined: "Story and Art".
pub fn makers_of(m: &Value) -> Vec<Value> {
    let mut out: Vec<(i64, String, Vec<String>)> = Vec::new();
    for e in m["staff"]["edges"].as_array().into_iter().flatten() {
        let role = e["role"].as_str().unwrap_or_default();
        let role = role.split(" (").next().unwrap_or_default().trim().to_string();
        let (Some(id), Some(name)) = (e["node"]["id"].as_i64(), e["node"]["name"]["full"].as_str()) else { continue };
        if !MAKER_ROLES.iter().any(|r| role.starts_with(r)) {
            continue;
        }
        match out.iter_mut().find(|(i, _, _)| *i == id) {
            Some((_, _, roles)) => {
                if !roles.contains(&role) {
                    roles.push(role);
                }
            }
            None => out.push((id, name.to_string(), vec![role])),
        }
    }
    out.into_iter()
        .take(6)
        .map(|(id, name, roles)| json!({ "id": id, "name": name, "role": roles.join(" and ").replace(" & ", " and ") }))
        .collect()
}

/// Everything one writer or artist made, most read first. 18+ titles only
/// when the reading filters let them in (`adult`).
pub async fn maker_works(app: &AppHandle, staff: i64, adult: bool) -> Result<Vec<Value>, String> {
    if staff <= 0 {
        return Ok(vec![]);
    }
    let q = format!("query($id:Int){{Staff(id:$id){{staffMedia(type:MANGA,sort:POPULARITY_DESC,perPage:50){{nodes{{{MEDIA_FIELDS}}}}}}}}}");
    let v = crate::media::anilist(&json!({ "query": q, "variables": { "id": staff } })).await?;
    let adult = adult && adult_ok(app);
    let mut seen = std::collections::HashSet::new();
    Ok(v["data"]["Staff"]["staffMedia"]["nodes"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|m| m.is_object() && (adult || m["isAdult"] != true))
                .filter(|m| seen.insert(m["id"].as_i64().unwrap_or(0)))
                .map(card)
                .collect()
        })
        .unwrap_or_default())
}

const FOR_YOU_LIFE: Duration = Duration::from_secs(60 * 60);

/// The genres to suggest more of: the ones this profile's series have most,
/// two at most, only AniList genre names, never Ecchi on its own.
pub fn top_genres(lists: &[Vec<String>]) -> Vec<String> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for g in lists.iter().flatten().filter(|g| GENRES.contains(&g.as_str()) && g.as_str() != "Ecchi") {
        match counts.iter_mut().find(|(x, _)| x == g) {
            Some((_, n)) => *n += 1,
            None => counts.push((g.clone(), 1)),
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    counts.into_iter().take(2).map(|(g, _)| g).collect()
}

/// Reading's own "Because you watched": rows made from what this profile
/// reads. AniList's readers' picks after the three series read most lately,
/// then more of the two genres it reads most. Nothing it already follows, no
/// 18+. One AniList call, kept an hour for the same series.
pub async fn for_you(app: &AppHandle) -> Result<Value, String> {
    let uid = uid(app)?;
    let mine = followed_mine(app);
    let at = spots(app, &uid);
    let mut seeds: Vec<&Followed> = mine.iter().filter(|f| f.anilist.is_some()).collect();
    seeds.sort_by_key(|f| std::cmp::Reverse(at.get(&f.key).map(|x| x.at).unwrap_or(f.added)));
    seeds.truncate(3);
    if seeds.is_empty() {
        return Ok(json!([]));
    }
    let genres = top_genres(&mine.iter().map(|f| f.genres.clone()).collect::<Vec<_>>());
    let have: Vec<i64> = mine.iter().filter_map(|f| f.anilist).collect();
    let key = format!("{uid}|{:?}|{genres:?}|{}", seeds.iter().map(|f| f.anilist).collect::<Vec<_>>(), have.len());
    {
        let state = app.state::<Reading>();
        let cache = state.for_you.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((born, v)) = cache.get(&key) {
            if born.elapsed() < FOR_YOU_LIFE {
                return Ok(v.clone());
            }
        }
    }
    let not_in = have.iter().take(60).map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    let mut q = String::from("{");
    for (i, f) in seeds.iter().enumerate() {
        q.push_str(&format!("b{i}:Media(id:{},type:MANGA){{recommendations(sort:RATING_DESC,perPage:24){{nodes{{rating mediaRecommendation{{{MEDIA_FIELDS}}}}}}}}} ", f.anilist.unwrap_or(0)));
    }
    for (j, g) in genres.iter().enumerate() {
        q.push_str(&format!("g{j}:Page(perPage:30){{media(type:MANGA,isAdult:false,sort:POPULARITY_DESC,genre:\"{g}\",id_not_in:[{not_in}]){{{MEDIA_FIELDS}}}}} "));
    }
    q.push('}');
    let v = crate::media::anilist(&json!({ "query": q })).await?;
    let data = &v["data"];
    if data.is_null() {
        return Err("AniList didn't answer. Try again in a minute.".into());
    }
    let keep = |m: &&Value| m.is_object() && m["isAdult"] != true && !have.contains(&m["id"].as_i64().unwrap_or(0));
    let mut rows = Vec::new();
    for (i, f) in seeds.iter().enumerate() {
        let items: Vec<Value> = data[format!("b{i}")]["recommendations"]["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|n| n["rating"].as_i64().unwrap_or(0) > 0)
            .map(|n| &n["mediaRecommendation"])
            .filter(keep)
            .map(card)
            .collect();
        if items.len() >= 4 {
            rows.push(json!({ "id": format!("because-{}", f.anilist.unwrap_or(0)), "title": format!("Because you read {}", f.title), "items": items }));
        }
    }
    for (j, g) in genres.iter().enumerate() {
        let items: Vec<Value> = data[format!("g{j}")]["media"].as_array().into_iter().flatten().filter(keep).map(card).collect();
        if items.len() >= 4 {
            rows.push(json!({ "id": format!("foryou-{g}"), "title": format!("More {g} for you"), "items": items }));
        }
    }
    let out = json!(rows);
    let state = app.state::<Reading>();
    let mut cache = state.for_you.lock().unwrap_or_else(|e| e.into_inner());
    cache.retain(|_, (born, _)| born.elapsed() < FOR_YOU_LIFE);
    cache.insert(key, (Instant::now(), out.clone()));
    Ok(out)
}

/// AniList's Media as (label, value) pairs, in the order they show.
fn about_facts(m: &Value) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut put = |k: &str, v: String| {
        if !v.trim().is_empty() {
            out.push((k.to_string(), v));
        }
    };
    let strs = |v: &Value| -> Vec<String> { v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default() };
    put("Genres", strs(&m["genres"]).join(", "));
    // The people who made it, by what they did: "Story", "Art", "Story & Art".
    let mut roles: Vec<(String, Vec<String>)> = Vec::new();
    for e in m["staff"]["edges"].as_array().into_iter().flatten() {
        let role = e["role"].as_str().unwrap_or_default();
        let role = role.split(" (").next().unwrap_or_default().trim();
        let keep = MAKER_ROLES.iter().any(|r| role.starts_with(r));
        let Some(name) = e["node"]["name"]["full"].as_str() else { continue };
        if !keep {
            continue;
        }
        match roles.iter_mut().find(|(r, _)| r == role) {
            Some((_, names)) => {
                if !names.iter().any(|n| n == name) {
                    names.push(name.to_string());
                }
            }
            None => roles.push((role.to_string(), vec![name.to_string()])),
        }
    }
    for (role, names) in roles.into_iter().take(4) {
        put(&role.replace(" & ", " and "), names.join(", "));
    }
    let status = match m["status"].as_str().unwrap_or_default() {
        "RELEASING" => "Ongoing",
        "FINISHED" => "Finished",
        "HIATUS" => "On hiatus",
        "CANCELLED" => "Cancelled",
        "NOT_YET_RELEASED" => "Coming soon",
        _ => "",
    };
    put("Status", status.to_string());
    if let Some(n) = m["chapters"].as_i64() {
        put("Chapters", n.to_string());
    }
    if let Some(n) = m["volumes"].as_i64() {
        put("Volumes", n.to_string());
    }
    let date = |d: &Value| -> Option<String> {
        let y = d["year"].as_i64()?;
        const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        Some(match d["month"].as_i64().filter(|m| (1..=12).contains(m)) {
            Some(mo) => format!("{} {y}", MONTHS[mo as usize - 1]),
            None => y.to_string(),
        })
    };
    match (date(&m["startDate"]), date(&m["endDate"])) {
        (Some(a), Some(b)) if a != b => put("Ran", format!("{a} to {b}")),
        (Some(a), Some(_)) => put("Year", a),
        (Some(a), None) if status == "Ongoing" || status == "On hiatus" => put("Started", a),
        (Some(a), None) => put("Year", a),
        _ => {}
    }
    let kind = match (m["format"].as_str().unwrap_or_default(), m["countryOfOrigin"].as_str().unwrap_or_default()) {
        ("NOVEL", _) => "Light novel",
        ("ONE_SHOT", _) => "One shot",
        (_, "KR") => "Manhwa (Korea)",
        (_, "CN") | (_, "TW") => "Manhua (China)",
        ("MANGA", _) => "Manga (Japan)",
        _ => "",
    };
    put("Type", kind.to_string());
    let source = match m["source"].as_str().unwrap_or_default() {
        "LIGHT_NOVEL" => "A light novel",
        "NOVEL" => "A novel",
        "WEB_NOVEL" => "A web novel",
        "VIDEO_GAME" | "GAME" => "A game",
        "ANIME" => "An anime",
        _ => "",
    };
    put("Based on", source.to_string());
    if let Some(s) = m["averageScore"].as_i64() {
        put("Score", format!("{s}%"));
    }
    let themes: Vec<String> = m["tags"]
        .as_array()
        .map(|a| a.iter().filter(|t| t["isMediaSpoiler"] != true && t["rank"].as_i64().unwrap_or(0) >= 60).filter_map(|t| t["name"].as_str().map(String::from)).take(6).collect())
        .unwrap_or_default();
    put("Themes", themes.join(", "));
    put("Also known as", strs(&m["synonyms"]).into_iter().take(3).collect::<Vec<_>>().join(", "));
    out
}

/// `adult`: this profile turned 18+ on (Include or Only in a reading tab), so
/// Ecchi titles are found too. `porn`: porn is on there as well, so isAdult
/// titles are found too. Otherwise they never are.
pub async fn search(app: &AppHandle, term: &str, adult: bool, porn: bool) -> Result<Vec<Value>, String> {
    let term = term.trim();
    if term.is_empty() {
        return Ok(vec![]);
    }
    let adult = adult && adult_ok(app);
    let only_safe = search_args(adult, porn);
    let q = format!("query($s:String){{Page(perPage:30){{media(type:MANGA,{only_safe}search:$s,sort:SEARCH_MATCH){{{MEDIA_FIELDS}}}}}}}");
    // MangaUpdates at the same time: the names AniList doesn't have (read_mu.rs).
    let ask = json!({ "query": q, "variables": { "s": term } });
    let (v, mu) = tokio::join!(crate::media::anilist(&ask), crate::read_mu::search(term));
    let mut cards: Vec<Value> = v?["data"]["Page"]["media"].as_array().map(|a| a.iter().map(card).collect()).unwrap_or_default();
    // Its series go first: each matched the very name typed.
    let mut first: Vec<Value> = Vec::new();
    for hit in mu {
        if (crate::read_mu::porn(&hit) && !(adult && porn)) || (crate::read_mu::ecchi(&hit) && !adult) {
            continue;
        }
        let known: HashSet<String> = hit.names.iter().map(|n| crate::read_mu::norm(n)).collect();
        let same = |c: &Value| card_names(c).iter().any(|n| known.contains(&crate::read_mu::norm(n)));
        if let Some(i) = cards.iter().position(|c| same(c)) {
            first.push(cards.remove(i));
            continue;
        }
        if first.iter().any(|c| same(c)) {
            continue;
        }
        // AniList again, by the names it goes by there (Chronicle of Runes is Emblem Wars).
        match by_names(&hit.names, only_safe).await.into_iter().find(|c| same(c)) {
            Some(c) => first.push(c),
            None => first.push(crate::read_mu::card(&hit)),
        }
    }
    first.extend(cards);
    Ok(first)
}

/// A card's title and every other name.
fn card_names(c: &Value) -> Vec<String> {
    let mut v: Vec<String> = c["title"].as_str().map(String::from).into_iter().collect();
    v.extend(c["names"].as_array().into_iter().flatten().filter_map(|n| n.as_str().map(String::from)));
    v
}

/// AniList searched by several names in one ask (its limit is 30 a minute).
async fn by_names(names: &[String], only_safe: &str) -> Vec<Value> {
    let names: Vec<&String> = names.iter().take(6).collect();
    if names.is_empty() {
        return vec![];
    }
    let vars: Vec<String> = (0..names.len()).map(|i| format!("$n{i}:String")).collect();
    let mut q = format!("query({}){{", vars.join(","));
    for i in 0..names.len() {
        q.push_str(&format!("p{i}:Page(perPage:5){{media(type:MANGA,{only_safe}search:$n{i}){{{MEDIA_FIELDS}}}}} "));
    }
    q.push('}');
    let variables: serde_json::Map<String, Value> = names.iter().enumerate().map(|(i, n)| (format!("n{i}"), json!(n))).collect();
    let Ok(v) = crate::media::anilist(&json!({ "query": q, "variables": variables })).await else { return vec![] };
    (0..names.len()).flat_map(|i| v["data"][format!("p{i}")]["media"].as_array().cloned().unwrap_or_default()).map(|m| card(&m)).collect()
}

/// The 18+ part of a search: Ecchi only with 18+ on, isAdult only with porn on too.
fn search_args(adult: bool, porn: bool) -> &'static str {
    match (adult, porn) {
        (true, true) => "",
        (true, false) => "isAdult:false,",
        _ => "isAdult:false,genre_not_in:[\"Ecchi\"],",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::suwayomi::Chapter;

    fn dated(id: i64, n: f64, ms: i64) -> Chapter {
        Chapter { uploaded: Some(ms.to_string()), ..ch(id, n, false) }
    }

    #[test]
    fn stray_numbers_are_dropped_before_copies_merge() {
        const W: i64 = 7 * 86_400_000;
        let base = 1_700_000_000_000;
        // Bbato (main) 1..116, MangaK 1..120 + "276" from long before.
        let main: Vec<Chapter> = (1..=116).map(|n| dated(n, n as f64, base + n * W)).collect();
        let mut fill: Vec<Chapter> = (1..=120).map(|n| dated(1000 + n, n as f64, base + n * W)).collect();
        fill.push(dated(9999, 276.0, base));
        let merged = merge_copies(clean_copies(vec![main, fill]));
        assert_eq!(top_number(merged.iter().map(|x| &x.0)), Some(120.0));
        assert!(merged.iter().all(|x| x.0.id != 9999));
    }

    #[test]
    fn a_mark_above_what_can_be_trusted_comes_down() {
        assert_eq!(lowered_mark(Some(276.0), Some(120.0)), Some(120.0));
        assert_eq!(lowered_mark(Some(100.0), Some(120.0)), Some(100.0));
        assert_eq!(lowered_mark(None, Some(120.0)), None);
        assert_eq!(lowered_mark(Some(276.0), None), Some(276.0), "nothing listed: left alone");
    }

    fn ch(id: i64, n: f64, downloaded: bool) -> Chapter {
        Chapter { id, name: format!("Chapter {n}"), number: Some(n), scanlator: None, downloaded, pages: 10, uploaded: None }
    }

    #[test]
    fn copies_merge_chapter_by_chapter() {
        let ch = |id: i64, n: f64, dl: bool| Chapter { id, name: format!("Chapter {n}"), number: Some(n), scanlator: None, downloaded: dl, pages: 1, uploaded: None };
        // Main (sharp, free chapters 1-3), fill (far ahead, 1-5, chapter 2 already here).
        let main = vec![ch(1, 1.0, false), ch(2, 2.0, false), ch(3, 3.0, false)];
        let fill = vec![ch(11, 1.0, false), ch(12, 2.0, true), ch(13, 3.0, false), ch(14, 4.0, false), ch(15, 5.0, false)];
        let m: Vec<(i64, usize)> = merge_copies(vec![main, fill]).into_iter().map(|(c, i)| (c.id, i)).collect();
        assert_eq!(m, vec![(1, 0), (12, 1), (3, 0), (14, 1), (15, 1)], "sharp where it has it, a copy already here is kept, the rest filled");
    }

    #[test]
    fn kind_rows_ask_what_they_say() {
        let e = kind_row_query("manhwa", "g:Romance").unwrap();
        assert_eq!(e.genres, vec!["Romance".to_string()]);
        let args = explore_args(&e, false).unwrap();
        assert!(args.contains("countryOfOrigin:\"KR\"") && args.contains("genre_in:[\"Romance\"]") && args.contains("isAdult:false"), "{args}");
        assert!(kind_row_query("manga", "g:Made up").is_none());
        assert!(kind_row_query("manga", "nope").is_none());
        assert_eq!(kind_row_query("novels", "ongoing").unwrap().status.as_deref(), Some("RELEASING"));
    }

    #[test]
    fn makers_merge_roles_and_skip_others() {
        let m = json!({ "staff": { "edges": [
            { "role": "Story", "node": { "id": 1, "name": { "full": "Chugong" } } },
            { "role": "Art (ch 1-100)", "node": { "id": 2, "name": { "full": "DUBU" } } },
            { "role": "Original Story", "node": { "id": 1, "name": { "full": "Chugong" } } },
            { "role": "Translator", "node": { "id": 3, "name": { "full": "X" } } },
        ] } });
        let got = makers_of(&m);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0]["name"], "Chugong");
        assert_eq!(got[0]["role"], "Story and Original Story");
        assert_eq!(got[1]["role"], "Art");
    }

    #[test]
    fn top_genres_counts_and_skips_ecchi() {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let got = top_genres(&[v(&["Action", "Fantasy", "Ecchi"]), v(&["Action", "Romance"]), v(&["Ecchi", "Ecchi", "Made up"])]);
        assert_eq!(got, vec!["Action".to_string(), "Fantasy".to_string()]);
    }

    #[test]
    fn about_reads_anilist_plainly() {
        let m = json!({
            "format": "MANGA", "status": "RELEASING", "chapters": null, "volumes": 12, "genres": ["Action", "Drama"], "countryOfOrigin": "KR",
            "source": "WEB_NOVEL", "averageScore": 84, "synonyms": ["Solo Leveling"], "startDate": { "year": 2018, "month": 3 }, "endDate": {},
            "staff": { "edges": [
                { "role": "Story", "node": { "name": { "full": "A" } } },
                { "role": "Art (Ch. 1-10)", "node": { "name": { "full": "B" } } },
                { "role": "Art", "node": { "name": { "full": "C" } } },
                { "role": "Translator", "node": { "name": { "full": "D" } } }
            ] },
            "tags": [{ "name": "Dungeon", "rank": 90 }, { "name": "Twist", "rank": 95, "isMediaSpoiler": true }, { "name": "Weak", "rank": 20 }]
        });
        let f: HashMap<String, String> = about_facts(&m).into_iter().collect();
        assert_eq!(f["Genres"], "Action, Drama");
        assert_eq!(f["Story"], "A");
        assert_eq!(f["Art"], "B, C");
        assert!(!f.contains_key("Translator") && !f.contains_key("Chapters"));
        assert_eq!(f["Volumes"], "12");
        assert_eq!(f["Status"], "Ongoing");
        assert_eq!(f["Started"], "Mar 2018");
        assert_eq!(f["Type"], "Manhwa (Korea)");
        assert_eq!(f["Based on"], "A web novel");
        assert_eq!(f["Themes"], "Dungeon");
    }

    #[test]
    fn explore_only_builds_safe_queries() {
        let e = Explore { kind: "manhua".into(), genres: vec!["Action".into(), "x\"}){evil".into()], status: Some("FINISHED".into()), sort: Some("score".into()), page: 1, ..Default::default() };
        let a = explore_args(&e, false).unwrap();
        assert!(a.contains("countryOfOrigin:\"CN\"") && a.contains("genre_in:[\"Action\"]") && a.contains("status:FINISHED") && a.contains("sort:SCORE_DESC"));
        assert!(!a.contains("evil"));
        assert!(explore_args(&Explore { kind: "x".into(), ..Default::default() }, false).is_err());
        // Out, tags, rating, years, length.
        let e = Explore { kind: "all".into(), not_genres: vec!["Romance".into()], tags: vec!["Boys' Love".into()], not_tags: vec!["Isekai".into()], min_score: Some(80), year_from: Some(2018), year_to: Some(2020), chapters_min: Some(50), chapters_max: Some(150), ..Default::default() };
        let a = explore_args(&e, false).unwrap();
        for want in ["genre_not_in:[\"Romance\",\"Ecchi\"]", "tag_in:[\"Boys\' Love\"]", "tag_not_in:[\"Isekai\"]", "averageScore_greater:79", "startDate_greater:20180000", "startDate_lesser:20210000", "chapters_greater:49", "chapters_lesser:151", "format_in:[MANGA,ONE_SHOT]"] {
            assert!(a.contains(want), "{want} in {a}");
        }
    }

    #[test]
    fn adult_only_for_those_allowed() {
        let ask = |adult: &str, porn: &str| Explore { kind: "manhwa".into(), adult: Some(adult.into()), porn: Some(porn.into()), ..Default::default() };
        let hidden = explore_args(&ask("include", "include"), false).unwrap();
        assert!(hidden.contains("isAdult:false") && hidden.contains("genre_not_in:[\"Ecchi\"]"), "not allowed: hidden whatever is asked");
        let a = explore_args(&ask("include", "hide"), true).unwrap();
        assert!(a.contains("isAdult:false") && !a.contains("Ecchi"), "18+ without porn: {a}");
        assert!(!explore_args(&ask("include", "include"), true).unwrap().contains("isAdult"), "with porn: both");
        let a = explore_args(&ask("only", "hide"), true).unwrap();
        assert!(a.contains("isAdult:false") && a.contains("genre_in:[\"Ecchi\"]"), "{a}");
        assert!(explore_args(&ask("only", "include"), true).unwrap().contains("isAdult:true"));
        let a = explore_args(&ask("hide", "include"), true).unwrap();
        assert!(a.contains("isAdult:false") && a.contains("genre_not_in:[\"Ecchi\"]"), "porn needs 18+: {a}");
        assert!(explore_args(&Explore { kind: "manhwa".into(), ..Default::default() }, true).unwrap().contains("isAdult:false"), "off unless asked");
        assert_eq!(search_args(true, true), "");
        assert_eq!(search_args(true, false), "isAdult:false,");
        assert!(search_args(false, true).contains("Ecchi"));
    }

    #[test]
    fn list_cards_keep_only_what_shows() {
        let (k, v) = keep_card(&json!({ "anilist": 5, "title": "T", "cover": "javascript:alert(1)", "banner": "https://img/x.jpg", "junk": "x" })).unwrap();
        assert_eq!(k, "al:5");
        assert!(v["cover"].is_null() && v["banner"] == "https://img/x.jpg" && v.get("junk").is_none());
        assert!(keep_card(&json!({ "title": "no id" })).is_none());
        assert!(lang_ok("pt-BR") && lang_ok("en") && !lang_ok("../x") && !lang_ok(""));
        assert_eq!(lang_name("es"), "Español");
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
        assert_eq!(cleanup_pick(&chs, &[&ahead], 3, now, false), vec![1, 2, 3, 4]);
        assert_eq!(cleanup_pick(&chs, &[&ahead, &behind], 3, now, false), vec![1], "the other profile is on chapter 2");
        assert!(cleanup_pick(&chs, &[&ahead], 0, now, false).is_empty(), "off");
        assert!(cleanup_pick(&chs, &[&ahead], 7, now, false).is_empty(), "read 5 days ago, keep for 7");
        let whole = |s: &[&Spot]| cleanup_pick(&chs, s, 3, now, true);
        // `ahead` is on chapter 5 and hasn't read it: the series isn't finished yet.
        assert!(whole(&[&ahead]).is_empty(), "not finished: everything stays");
        let mut done = ahead.clone();
        done.read.insert(5, now - 4 * day);
        assert_eq!(whole(&[&done]), vec![1, 2, 3, 4, 5], "finished 4 days ago, 3 to keep: all of it goes");
        let mut fresh = done.clone();
        fresh.read.insert(5, now - day);
        assert!(whole(&[&fresh]).is_empty(), "finished yesterday: kept for 3 days");
        assert!(whole(&[&done, &behind]).is_empty(), "someone else is still reading it");
    }

    #[test]
    fn deleted_read_chapters_dont_come_back() {
        let mut chs: Vec<Chapter> = (1..=6).map(|i| ch(i, i as f64, false)).collect();
        let none = std::collections::HashSet::new();
        // Followed at 6. Chapters 3 to 5 were picked, read and cleaned up; 1, 2 and 6 never wanted.
        let read = |id: i64| (3..=5).contains(&id);
        assert!(new_wanted(&chs, &none, &read, None, 6.0).is_empty(), "nothing out since following");
        chs.push(ch(7, 7.0, false));
        chs.push(ch(8, 8.0, false));
        assert_eq!(new_wanted(&chs, &none, &read, None, 6.0), vec![7, 8], "out since following: new");
        let q: std::collections::HashSet<i64> = [7].into_iter().collect();
        assert!(new_wanted(&chs, &q, &|id| id == 8, None, 6.0).is_empty(), "queued or read: not again");
    }

    #[test]
    fn picked_chapters_of_a_finished_series_dont_pull_in_the_rest() {
        // Noblesse: 545 chapters, 1 to 100 picked, none downloaded yet.
        let mut chs: Vec<Chapter> = (1..=545).map(|i| ch(i, i as f64, false)).collect();
        let picked: std::collections::HashSet<i64> = (1..=100).collect();
        let mark = top_number(chs.iter()).unwrap();
        assert_eq!(mark, 545.0);
        assert!(new_wanted(&chs, &picked, &|_| false, None, mark).is_empty(), "nothing downloaded yet: still nothing new");
        assert!(new_wanted(&chs, &std::collections::HashSet::new(), &|_| false, None, mark).is_empty(), "picked ones done: 101 to 545 stay left out");
        chs.push(ch(546, 546.0, false));
        assert_eq!(new_wanted(&chs, &picked, &|_| false, None, mark), vec![546], "a real new chapter comes");
    }

    #[test]
    fn a_series_from_before_the_mark_wants_nothing_its_first_round() {
        let raw = r#"{"key":"m1","manga_id":1,"anilist":null,"title":"X","cover":null,"banner":null,"country":null,"format":null,"status":null,"description":null,"mode":"manga","source":"S","source_title":"X","width":0,"added":0,"live":true,"new_too":false}"#;
        let mut f: Followed = serde_json::from_str(raw).unwrap();
        assert_eq!(f.known_upto, None, "not known: the first round only sets it");
        one_button(&mut f);
        assert!(f.new_too && !f.live, "Keep up to date on: the one button is on");
        let chs: Vec<Chapter> = (1..=50).map(|i| ch(i, i as f64, false)).collect();
        let mark = top_number(chs.iter()).unwrap();
        assert!(new_wanted(&chs, &std::collections::HashSet::new(), &|_| false, None, mark).is_empty());
    }

    #[test]
    fn new_chapters_are_the_ones_out_since_you_last_read() {
        let f = Followed { key: "m1".into(), added: 1_000, ..Default::default() };
        let at = |id: i64, secs: i64, here: bool| Chapter { uploaded: Some((secs * 1000).to_string()), ..ch(id, id as f64, here) };
        let chs = vec![at(1, 500, true), at(2, 2_000, true), at(3, 3_000, true), at(4, 3_500, false)];
        // Never read: new since following (chapter 1 was already out).
        assert_eq!(chs.iter().filter(|c| is_fresh(c, &f, None)).map(|c| c.id).collect::<Vec<_>>(), vec![2, 3]);
        let mut s = Spot { chapter: 2, at: 2_500, ..Default::default() };
        s.read.insert(2, 2_500);
        // Read up to 2 at 2500: only 3 is new (4 isn't here yet).
        assert_eq!(chs.iter().filter(|c| is_fresh(c, &f, Some(&s))).map(|c| c.id).collect::<Vec<_>>(), vec![3]);
        let shelf = shelf_of(&f, &chs, Some(&s));
        assert_eq!((shelf.fresh, shelf.fresh_label.as_deref(), shelf.fresh_at), (1, Some("Chapter 3"), 3_000));
    }

    #[test]
    fn each_profile_has_its_own_reading() {
        let old: Vec<String> = vec![];
        assert!(owned_by(&old, Some("me"), "me"), "from before: the PC owner's");
        assert!(!owned_by(&old, Some("me"), "friend"), "not a friend's");
        let both = vec!["me".to_string(), "friend".to_string()];
        assert!(owned_by(&both, Some("me"), "friend"));
        assert_eq!(owners_of(&old, Some("me")), vec!["me".to_string()]);
        assert!(!owned_by(&["friend".to_string()], Some("me"), "me"), "a friend's own isn't the owner's");
    }

    #[test]
    fn marking_read_moves_the_place_on() {
        let order = vec![10, 11, 12, 13];
        let mut s = Spot { chapter: 11, page: 4, pages: 20, ..Default::default() };
        s.read.insert(12, 0);
        move_on(&mut s, &order);
        assert_eq!((s.chapter, s.page), (12, 0));
        let mut back = Spot { chapter: 13, page: 3, pages: 20, ..Default::default() };
        back.read.insert(10, 0);
        move_on(&mut back, &order);
        assert_eq!((back.chapter, back.page), (13, 3), "marking behind you keeps your place");
        let mut new = Spot::default();
        new.read.insert(11, 0);
        move_on(&mut new, &order);
        assert_eq!(new.chapter, 11, "never opened: placed at the furthest marked");
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
