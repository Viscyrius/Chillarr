//! Books and light novels (issue #8). Found on Open Library (books) or
//! AniList (light novels, format NOVEL), got through the Prowlarr and
//! qBittorrent Chillarr already runs: no extra app. A pick goes to qBittorrent
//! under the "books" category; when it's done its ebook file is filed into
//! `<Reading>\Books\` and the torrent goes (unless sharing is on). A pack
//! downloads only the file that matches.
//!
//! Measured 2026-11-08 (Project Hail Mary): Prowlarr's book search answers in
//! a few seconds with EPUB and AZW3 releases, TPB and Knaben as magnets,
//! 1337x as Prowlarr download links.
//!
//! Comics and graphic novels (issue #48) go the same way: found on Open
//! Library, got as a CBZ or CBR (Prowlarr's Comics category 7030, then by
//! name), filed into the same folder and read page by page in the comic
//! reader. A CBR that's really RAR is turned into a CBZ when it's filed, with
//! the tar.exe Windows ships (it reads RAR), so no RAR code is in the app.

use crate::read_core;
use crate::zipread;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

pub const CATEGORY: &str = "books";
/// Bigger than this isn't one book: a collection or an audiobook.
const MAX_BOOK: u64 = 150 * 1024 * 1024;
const EBOOK_EXT: [&str; 4] = ["epub", "pdf", "azw3", "mobi"];
/// One graphic novel or an omnibus fits; a whole run of a series doesn't.
const MAX_COMIC: u64 = 2 * 1024 * 1024 * 1024;
const COMIC_EXT: [&str; 3] = ["cbz", "cbr", "pdf"];

// ---------- picking a release ----------

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Release {
    /// What the page sends back to pick it: the link itself stays on the PC,
    /// so a page can only pick something Chillarr offered.
    #[serde(default)]
    pub id: String,
    pub title: String,
    pub indexer: String,
    pub size: u64,
    pub seeders: i64,
    pub format: Option<String>,
    /// A magnet, or Prowlarr's download link.
    pub url: String,
}

/// The format a release title names.
pub fn format_of(title: &str) -> Option<String> {
    let low = title.to_ascii_lowercase();
    ["epub", "azw3", "mobi", "cbz", "cbr", "pdf"].iter().find(|f| low.contains(*f)).map(|f| f.to_string())
}

fn format_rank(f: Option<&str>, comic: bool) -> i32 {
    if comic {
        // CBZ opens as it is; a CBR is turned into one when it's filed.
        return match f {
            Some("cbz") => 3,
            Some("cbr") => 2,
            Some("pdf") => 1,
            _ => 0,
        };
    }
    match f {
        Some("epub") => 3,
        Some("azw3") => 1,
        Some("mobi") => 1,
        Some("pdf") => 1,
        _ => 0,
    }
}

/// Whether a release is this book: every word of the title in it.
pub fn matches(release: &str, title: &str) -> bool {
    let have = read_core::norm_title(release);
    let want = read_core::norm_title(title);
    !want.is_empty() && want.iter().all(|w| have.contains(w))
}

/// The usable releases, best first: the right book, one book's size, EPUB
/// before the rest (it reads everywhere), then the most seeders.
pub fn rank(results: &[Value], title: &str, comic: bool) -> Vec<Release> {
    let max = if comic { MAX_COMIC } else { MAX_BOOK };
    let mut out: Vec<Release> = results
        .iter()
        .filter_map(|x| {
            let t = x["title"].as_str()?;
            let url = x["magnetUrl"].as_str().filter(|u| !u.is_empty()).or(x["downloadUrl"].as_str())?.to_string();
            let size = x["size"].as_u64().unwrap_or(0);
            (matches(t, title) && size > 0 && size <= max && x["protocol"].as_str().is_none_or(|p| p == "torrent")).then(|| Release {
                id: String::new(),
                title: t.to_string(),
                indexer: x["indexer"].as_str().unwrap_or_default().to_string(),
                size,
                seeders: x["seeders"].as_i64().unwrap_or(0),
                format: format_of(t),
                url,
            })
        })
        .filter(|r| r.seeders > 0)
        // An ebook isn't a comic's pages.
        .filter(|r| !comic || !matches!(r.format.as_deref(), Some("epub" | "azw3" | "mobi")))
        .collect();
    out.sort_by(|a, b| format_rank(b.format.as_deref(), comic).cmp(&format_rank(a.format.as_deref(), comic)).then(b.seeders.cmp(&a.seeders)));
    out
}

/// Of a torrent's files, the one ebook to keep: the right format first,
/// then the best name match. Index into `files`.
pub fn pick_file(files: &[(String, u64)], title: &str, comic: bool) -> Option<usize> {
    let ext = |n: &str| n.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    let wanted: &[&str] = if comic { &COMIC_EXT } else { &EBOOK_EXT };
    (0..files.len())
        .filter(|&i| wanted.contains(&ext(&files[i].0).as_str()))
        .max_by(|&a, &b| {
            // A pack of issues: the name decides first, then the format.
            let s = |i: usize| {
                let name = (read_core::title_score(files[i].0.rsplit('/').next().unwrap_or_default(), title) * 100.0) as i64;
                let format = format_rank(Some(&ext(&files[i].0)), comic) as i64;
                if comic { (name, format) } else { (format, name) }
            };
            s(a).cmp(&s(b))
        })
}

/// The info hash in a magnet link, lower case.
pub fn magnet_hash(url: &str) -> Option<String> {
    let at = url.to_ascii_lowercase().find("xt=urn:btih:")? + 12;
    let h: String = url[at..].chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
    (h.len() == 40).then(|| h.to_ascii_lowercase())
}

/// `<Author> - <Title>.<ext>`, safe for Windows.
pub fn file_name(author: Option<&str>, title: &str, ext: &str) -> String {
    let clean = |s: &str| s.chars().map(|c| if r#"<>:"/\|?*"#.contains(c) || c.is_control() { ' ' } else { c }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ");
    let base = match author.map(clean).filter(|a| !a.is_empty()) {
        Some(a) => format!("{a} - {}", clean(title)),
        None => clean(title),
    };
    let base: String = base.chars().take(150).collect();
    format!("{}.{ext}", base.trim_end_matches(['.', ' ']))
}

// ---------- the list of books ----------

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Book {
    /// "b<random>".
    pub key: String,
    pub title: String,
    pub author: Option<String>,
    pub cover: Option<String>,
    pub description: Option<String>,
    pub olid: Option<String>,
    pub anilist: Option<i64>,
    /// The release picked, and its torrent once known.
    pub release: String,
    pub hash: Option<String>,
    /// Filed into the Books folder: its name there.
    pub file: Option<String>,
    pub added: i64,
    /// Set when something went wrong (shown on the card).
    pub problem: Option<String>,
    /// The profiles that got it (Jellyfin user ids): each profile's books are its
    /// own. Empty: from before this was kept, the PC owner's.
    #[serde(default)]
    pub owners: Vec<String>,
    /// Its file was deleted on purpose (read_storage.rs): kept in the list, not
    /// looked for. Picking a copy again gets it back.
    #[serde(default)]
    pub not_downloaded: bool,
    /// A comic or graphic novel: CBZ pages, read in the comic reader.
    #[serde(default)]
    pub comic: bool,
}

fn books_file(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app.path().app_data_dir().map_err(|e| e.to_string())?.join("books.json"))
}

pub fn all(app: &AppHandle) -> Vec<Book> {
    books_file(app).ok().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save(app: &AppHandle, list: &[Book]) -> Result<(), String> {
    let p = books_file(app)?;
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string(list).map_err(|e| e.to_string())?).and_then(|_| std::fs::rename(&tmp, &p)).map_err(|e| format!("Couldn't save it: {e}"))
}

/// The watching profile's book: someone else's isn't in your list.
pub fn find(app: &AppHandle, key: &str) -> Result<Book, String> {
    let uid = crate::media::active_user(app).ok_or("Sign in to Jellyfin first.")?;
    let owner = crate::profiles::owner_id(app);
    all(app).into_iter().find(|b| b.key == key && crate::read::owned_by(&b.owners, owner.as_deref(), &uid)).ok_or_else(|| "That book isn't in your list.".into())
}

pub fn path_of(app: &AppHandle, b: &Book) -> Option<PathBuf> {
    Some(crate::read::books_dir(app)?.join(b.file.as_ref()?)).filter(|p| p.is_file())
}

/// What a book's page asks for: everything the card shows.
#[derive(Deserialize, Clone, Debug, Default)]
pub struct BookMeta {
    pub title: String,
    pub author: Option<String>,
    pub cover: Option<String>,
    pub description: Option<String>,
    pub olid: Option<String>,
    pub anilist: Option<i64>,
    #[serde(default)]
    pub comic: bool,
}

/// Copies offered to a page, by id, for an hour: `get` takes only these.
static OFFERED: std::sync::Mutex<Vec<(std::time::Instant, Release)>> = std::sync::Mutex::new(Vec::new());

fn offer(mut list: Vec<Release>) -> Vec<Release> {
    let mut o = OFFERED.lock().unwrap();
    o.retain(|(at, _)| at.elapsed() < std::time::Duration::from_secs(3600));
    for r in list.iter_mut() {
        let mut b = [0u8; 12];
        getrandom::getrandom(&mut b).expect("random");
        r.id = b.iter().map(|x| format!("{x:02x}")).collect();
        o.push((std::time::Instant::now(), r.clone()));
    }
    while o.len() > 500 {
        o.remove(0);
    }
    // The page sees what it shows, never the link.
    list.into_iter().map(|r| Release { url: String::new(), ..r }).collect()
}

fn offered(id: &str) -> Option<Release> {
    OFFERED.lock().unwrap().iter().find(|(at, r)| r.id == id && at.elapsed() < std::time::Duration::from_secs(3600)).map(|(_, r)| r.clone())
}

/// Every usable copy on the download sites, best first.
pub async fn copies(app: &AppHandle, meta: &BookMeta) -> Result<Vec<Release>, String> {
    let p = crate::media::arr(app, crate::model::AppId::Prowlarr).ok_or("Prowlarr isn't set up.")?;
    // The title and the author's last name: enough to find it, not so much that
    // a release named differently is missed.
    let surname = meta.author.as_deref().and_then(|a| a.split_whitespace().last()).unwrap_or_default();
    let mut found = Vec::new();
    let cats = if meta.comic { "categories=7030" } else { "categories=7000&categories=7020" };
    for q in [format!("{} {surname}", meta.title), meta.title.clone()] {
        let path = format!("search?query={}&{cats}&type=search", enc(q.trim()));
        found = p.list_slow(&path).await.map_err(|e| format!("Couldn't search the download sites: {e}"))?;
        if !rank(&found, &meta.title, meta.comic).is_empty() {
            break;
        }
    }
    if meta.comic {
        // Comics filed under no comic category: asked by name, CBR and CBZ.
        for ext in ["cbr", "cbz"] {
            let plain = format!("search?query={}&type=search", enc(format!("{} {ext}", meta.title).trim()));
            if let Ok(more) = p.list_slow(&plain).await {
                found.extend(more.into_iter().filter(looks_like_a_comic));
            }
        }
        return Ok(offer(rank(&dedup(found), &meta.title, true)));
    }
    // Many sites file ebooks under no book category at all (LimeTorrents,
    // TorrentDownload and 1337x had 18, 10 and 2 for "Percy Jackson epub" and
    // none in the categories, 2026-11-08), so ask them once more by name.
    let plain = format!("search?query={}&type=search", enc(format!("{} epub", meta.title).trim()));
    if let Ok(more) = p.list_slow(&plain).await {
        found.extend(more.into_iter().filter(looks_like_a_book));
    }
    Ok(offer(rank(&dedup(found), &meta.title, false)))
}

/// From an uncategorised search: only what says it's a CBR or CBZ, or is filed as a comic.
fn looks_like_a_comic(x: &Value) -> bool {
    let filed = x["categories"].as_array().is_some_and(|c| c.iter().any(|c| c["id"].as_i64() == Some(7030)));
    filed || x["title"].as_str().and_then(format_of).is_some_and(|f| f == "cbr" || f == "cbz")
}

/// From an uncategorised search: only what says it's an ebook, or is filed as one.
fn looks_like_a_book(x: &Value) -> bool {
    let filed = x["categories"].as_array().is_some_and(|c| c.iter().any(|c| c["id"].as_i64().is_some_and(|id| (7000..8000).contains(&id))));
    filed || x["title"].as_str().and_then(format_of).is_some()
}

/// The same release found twice (both searches) counts once.
fn dedup(v: Vec<Value>) -> Vec<Value> {
    let mut seen = std::collections::HashSet::new();
    v.into_iter().filter(|x| seen.insert(x["infoHash"].as_str().map(str::to_lowercase).unwrap_or_else(|| format!("{}|{}", x["indexer"], x["title"])))).collect()
}

fn enc(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// Downloads the copy offered as `id` and adds the book to the list.
pub async fn get(app: &AppHandle, meta: BookMeta, id: &str) -> Result<String, String> {
    let me = crate::media::active_user(app).ok_or("Sign in to Jellyfin first.")?;
    let pc_owner = crate::profiles::owner_id(app);
    // Someone else already has this book: this profile gets it too, no second download.
    let same = |b: &Book| match (&b.olid, &meta.olid) {
        (Some(a), Some(c)) => a == c,
        _ => crate::read_core::norm_title(&b.title) == crate::read_core::norm_title(&meta.title) && b.author == meta.author,
    } && b.comic == meta.comic;
    let mut list = all(app);
    // Its file was deleted: this download brings it back, for whoever had it.
    let mut kept_owners: Option<Vec<String>> = None;
    if let Some(i) = list.iter().position(|b| same(b) && b.not_downloaded && crate::read::owned_by(&b.owners, pc_owner.as_deref(), &me)) {
        kept_owners = Some(crate::read::owners_of(&list[i].owners, pc_owner.as_deref()));
    }
    if let Some(b) = list.iter_mut().find(|b| same(b)).filter(|_| kept_owners.is_none()) {
        if crate::read::owned_by(&b.owners, pc_owner.as_deref(), &me) {
            return Ok(format!("{} is already in your books.", b.title));
        }
        let mut owners = crate::read::owners_of(&b.owners, pc_owner.as_deref());
        owners.push(me);
        b.owners = owners;
        let title = b.title.clone();
        save(app, &list)?;
        return Ok(format!("{title} is in your books now. It was already here."));
    }
    let pick = offered(id).ok_or("That list of copies is old. Look for copies again.")?;
    if pick.size > if meta.comic { MAX_COMIC } else { MAX_BOOK } {
        return Err("That copy is too big to be one book.".into());
    }
    crate::read::ensure_root(app).await?;
    let q = crate::waiting::qbit(app).ok_or("qBittorrent isn't set up.")?;
    let before: std::collections::HashSet<String> = q.list().await.unwrap_or_default().iter().filter_map(|t| t["hash"].as_str().map(str::to_ascii_lowercase)).collect();
    q.add_url(&pick.url, CATEGORY).await.map_err(|e| format!("qBittorrent didn't take it: {e:?}"))?;
    // Its hash now, so it's never confused with another book's torrent: from
    // the magnet, else the new torrent that appears in the books category.
    let mut hash = magnet_hash(&pick.url);
    for _ in 0..20 {
        if hash.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        hash = q.list().await.unwrap_or_default().iter().filter(|t| t["category"] == CATEGORY).filter_map(|t| t["hash"].as_str().map(str::to_ascii_lowercase)).find(|h| !before.contains(h));
    }
    let mut key = [0u8; 8];
    getrandom::getrandom(&mut key).map_err(|e| e.to_string())?;
    let mut list = all(app);
    if kept_owners.is_some() {
        list.retain(|b| !(same(b) && b.not_downloaded));
    }
    list.push(Book {
        key: format!("b{}", key.iter().map(|b| format!("{b:02x}")).collect::<String>()),
        title: meta.title.clone(),
        author: meta.author,
        cover: meta.cover,
        description: meta.description,
        olid: meta.olid,
        anilist: meta.anilist,
        hash,
        release: pick.title,
        file: None,
        added: chrono::Utc::now().timestamp(),
        problem: None,
        owners: kept_owners.unwrap_or_else(|| vec![me]),
        not_downloaded: false,
        comic: meta.comic,
    });
    save(app, &list)?;
    let msg = format!("Getting {}.", meta.title);
    crate::log::log(app, crate::model::Level::Ok, &msg);
    Ok(msg)
}

/// Every couple of minutes: books still coming are matched to their torrent,
/// trimmed to the one file, and filed away once it's done.
pub async fn tick(app: &AppHandle) {
    let mut list = all(app);
    if list.iter().all(|b| b.file.is_some() || b.not_downloaded) {
        return;
    }
    let Some(q) = crate::waiting::qbit(app) else { return };
    let Ok(torrents) = q.list().await else { return };
    let Some(dir) = crate::read::books_dir(app) else { return };
    let seeding = matches!(crate::waiting::seeding(app).await, Ok(true));
    let mut changed = false;
    for b in list.iter_mut().filter(|b| b.file.is_none() && !b.not_downloaded) {
        // Matched by its hash, in the books category: never by name, which let
        // "Dune" take "Dune Messiah"'s torrent.
        let Some(h) = b.hash.clone() else {
            b.problem = Some("Couldn't find its download. Remove it and pick a copy again.".into());
            changed = true;
            continue;
        };
        let t = torrents.iter().find(|t| t["category"] == CATEGORY && t["hash"].as_str().is_some_and(|x| x.eq_ignore_ascii_case(&h)));
        let Some(t) = t else { continue };
        let Some(hash) = t["hash"].as_str().map(str::to_ascii_lowercase) else { continue };
        if b.hash.as_deref() != Some(hash.as_str()) {
            b.hash = Some(hash.clone());
            changed = true;
        }
        let Ok(files) = q.files(&hash).await else { continue };
        if files.is_empty() {
            continue; // still fetching its metadata
        }
        let named: Vec<(String, u64)> = files.iter().map(|f| (f["name"].as_str().unwrap_or_default().to_string(), f["size"].as_u64().unwrap_or(0))).collect();
        let Some(keep) = pick_file(&named, &b.title, b.comic) else {
            // Nothing to read in it: it goes, rather than download and share for ever.
            let _ = q.delete(&[hash.clone()], true).await;
            b.problem = Some(if b.comic { "That download had no comic in it. Pick another copy." } else { "That download had no ebook in it. Pick another copy." }.into());
            b.hash = None;
            changed = true;
            continue;
        };
        let others: Vec<i64> = (0..files.len()).filter(|&i| i != keep && files[i]["priority"].as_i64() != Some(0)).map(|i| files[i]["index"].as_i64().unwrap_or(i as i64)).collect();
        if !others.is_empty() {
            let _ = q.set_file_prio(&hash, &others, 0).await;
        }
        if files[keep]["progress"].as_f64().unwrap_or(0.0) < 1.0 {
            continue;
        }
        let save_path = t["save_path"].as_str().unwrap_or_default();
        let from = Path::new(save_path).join(named[keep].0.replace('/', "\\"));
        let ext = named[keep].0.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_else(|| "epub".into());
        // A comic archive is always filed as a CBZ: many CBRs are zips anyway,
        // and a real RAR is unpacked and packed again as one.
        let comic_archive = b.comic && (ext == "cbz" || ext == "cbr");
        let name = file_name(b.author.as_deref(), &b.title, if comic_archive { "cbz" } else { &ext });
        let _ = std::fs::create_dir_all(&dir);
        let to = dir.join(&name);
        let filed = if comic_archive && is_rar(&from) {
            let (from, to) = (from.clone(), to.clone());
            tokio::task::spawn_blocking(move || rar_to_cbz(&from, &to)).await.unwrap_or_else(|e| Err(e.to_string())).map_err(std::io::Error::other)
        } else {
            // A hardlink when it's the same drive (no extra space), else a copy.
            std::fs::hard_link(&from, &to).or_else(|_| std::fs::copy(&from, &to).map(|_| ()))
        };
        match filed {
            Ok(()) => {
                b.file = Some(name);
                b.problem = None;
                crate::log::log(app, crate::model::Level::Ok, format!("{} is ready to read.", b.title));
                if !seeding {
                    let _ = q.delete(&[hash], true).await;
                }
            }
            Err(e) => b.problem = Some(format!("Couldn't put it in the Books folder: {e}")),
        }
        changed = true;
    }
    if changed {
        let _ = save(app, &list);
    }
}

pub async fn remove(app: &AppHandle, key: &str) -> Result<String, String> {
    let b = find(app, key)?;
    let mut list = all(app);
    // Others have it too: it only leaves this profile's books.
    let me = crate::media::active_user(app).unwrap_or_default();
    let mut owners = crate::read::owners_of(&b.owners, crate::profiles::owner_id(app).as_deref());
    owners.retain(|o| o != &me);
    if !owners.is_empty() {
        if let Some(x) = list.iter_mut().find(|x| x.key == key) {
            x.owners = owners;
        }
        save(app, &list)?;
        return Ok(format!("Took {} off your books. Others still have it, so the file stays.", b.title));
    }
    if let (Some(h), Some(q)) = (&b.hash, crate::waiting::qbit(app)) {
        let _ = q.delete(&[h.clone()], true).await;
    }
    if let Some(p) = path_of(app, &b) {
        std::fs::remove_file(&p).map_err(|e| format!("Couldn't delete it: {e}"))?;
    }
    list.retain(|x| x.key != key);
    save(app, &list)?;
    Ok(format!("Removed {}.", b.title))
}

/// Deletes a book's file (and its download), keeping the book in the list as
/// not downloaded. Others having it too need `others_ok`: the file is shared.
pub async fn delete_file(app: &AppHandle, key: &str, others_ok: bool) -> Result<String, String> {
    let me = crate::media::active_user(app).ok_or("Sign in to Jellyfin first.")?;
    let pc_owner = crate::profiles::owner_id(app);
    let b = all(app).into_iter().find(|b| b.key == key).ok_or("That book isn't in the list any more.")?;
    let others = crate::read::owners_of(&b.owners, pc_owner.as_deref()).into_iter().any(|o| o != me);
    if others && !others_ok {
        return Err("Someone else still reads this book.".into());
    }
    if let (Some(h), Some(q)) = (&b.hash, crate::waiting::qbit(app)) {
        let _ = q.delete(&[h.clone()], true).await;
    }
    if let Some(p) = path_of(app, &b) {
        std::fs::remove_file(&p).map_err(|e| format!("Couldn't delete it: {e}"))?;
    }
    let mut list = all(app);
    if let Some(x) = list.iter_mut().find(|x| x.key == key) {
        x.file = None;
        x.hash = None;
        x.problem = None;
        x.not_downloaded = true;
    }
    save(app, &list)?;
    let msg = format!("Deleted the file of {}.", b.title);
    crate::log::log(app, crate::model::Level::Ok, &msg);
    Ok(msg)
}

// ---------- CBR ----------

/// A RAR archive (RAR 4 or 5), whatever its name says.
fn is_rar(path: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 6];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut head)).is_ok() && head == *b"Rar!\x1a\x07"
}

/// A RAR comic turned into a CBZ of its pages. Windows' own tar.exe
/// (libarchive) unpacks it into a folder next to `to`, which goes after.
/// tar refuses names that climb out of the folder.
fn rar_to_cbz(from: &Path, to: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let tar = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows")).join(r"System32\tar.exe");
    let work = to.with_extension("unpacking");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let out = std::process::Command::new(&tar).arg("-xf").arg(from).arg("-C").arg(&work).creation_flags(0x0800_0000).output();
    let done = (|| {
        let out = out.map_err(|e| format!("Couldn't unpack the CBR: {e}"))?;
        if !out.status.success() {
            return Err(format!("Couldn't unpack the CBR: {}", String::from_utf8_lossy(&out.stderr).trim()));
        }
        let mut pages = Vec::new();
        collect_pages(&work, &work, &mut pages);
        if pages.is_empty() {
            return Err("That CBR had no pages in it.".to_string());
        }
        let tmp = to.with_extension("cbz.tmp");
        write_cbz(&tmp, &pages).and_then(|_| std::fs::rename(&tmp, to).map_err(|e| e.to_string()))
    })();
    let _ = std::fs::remove_dir_all(&work);
    done
}

/// Every page under `dir`: (its name inside the archive, its file).
fn collect_pages(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        // Links aren't followed: only what tar itself wrote.
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            collect_pages(root, &p, out);
        } else if ft.is_file() {
            let name = p.strip_prefix(root).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_default();
            if read_core::is_page(&name) {
                out.push((name, p));
            }
        }
    }
}

/// A plain zip of these files, stored (pictures don't pack any smaller).
pub fn write_cbz(to: &Path, files: &[(String, PathBuf)]) -> Result<(), String> {
    use std::io::Write;
    const TOO_BIG: &str = "That comic is too big to keep.";
    let mut f = std::io::BufWriter::new(std::fs::File::create(to).map_err(|e| e.to_string())?);
    let mut central = Vec::new();
    let mut at: u64 = 0;
    for (name, path) in files {
        let data = std::fs::read(path).map_err(|e| e.to_string())?;
        if data.len() as u64 > zipread::MAX_ENTRY || at + data.len() as u64 + 1024 > u32::MAX as u64 {
            return Err(TOO_BIG.into());
        }
        let mut crc = flate2::Crc::new();
        crc.update(&data);
        let crc = crc.sum();
        let n = name.as_bytes();
        let size = (data.len() as u32).to_le_bytes();
        // Version 2.0, bit 11 (UTF-8 names), stored, a fixed 1980-01-01 time.
        let mut head = Vec::with_capacity(30 + n.len());
        head.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        head.extend_from_slice(&[20, 0, 0, 8, 0, 0, 0, 0, 0x21, 0]);
        head.extend_from_slice(&crc.to_le_bytes());
        head.extend_from_slice(&size);
        head.extend_from_slice(&size);
        head.extend_from_slice(&(n.len() as u16).to_le_bytes());
        head.extend_from_slice(&[0, 0]);
        head.extend_from_slice(n);
        f.write_all(&head).and_then(|_| f.write_all(&data)).map_err(|e| e.to_string())?;
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&[20, 0, 20, 0, 0, 8, 0, 0, 0, 0, 0x21, 0]);
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&size);
        central.extend_from_slice(&size);
        central.extend_from_slice(&(n.len() as u16).to_le_bytes());
        central.extend_from_slice(&[0; 12]);
        central.extend_from_slice(&(at as u32).to_le_bytes());
        central.extend_from_slice(n);
        at += head.len() as u64 + data.len() as u64;
    }
    if files.len() > 0xFFFF || at + central.len() as u64 > u32::MAX as u64 {
        return Err(TOO_BIG.into());
    }
    let mut end = Vec::new();
    end.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    end.extend_from_slice(&[0; 4]);
    end.extend_from_slice(&(files.len() as u16).to_le_bytes());
    end.extend_from_slice(&(files.len() as u16).to_le_bytes());
    end.extend_from_slice(&(central.len() as u32).to_le_bytes());
    end.extend_from_slice(&(at as u32).to_le_bytes());
    end.extend_from_slice(&[0, 0]);
    f.write_all(&central).and_then(|_| f.write_all(&end)).and_then(|_| f.flush()).map_err(|e| e.to_string())
}

// ---------- EPUB ----------

/// An EPUB's reading order: (spine item paths inside the archive, title).
pub fn epub_spine(container: &str, opf_path_and_text: impl Fn(&str) -> Option<String>) -> Option<(Vec<String>, Option<String>)> {
    let c = roxmltree::Document::parse(container).ok()?;
    let opf = c.descendants().find(|n| n.has_tag_name("rootfile"))?.attribute("full-path")?.to_string();
    let text = opf_path_and_text(&opf)?;
    let d = roxmltree::Document::parse(&text).ok()?;
    let dir = opf.rsplit_once('/').map(|(d, _)| format!("{d}/")).unwrap_or_default();
    let manifest: std::collections::HashMap<&str, &str> = d.descendants().filter(|n| n.has_tag_name("item")).filter_map(|n| Some((n.attribute("id")?, n.attribute("href")?))).collect();
    let spine: Vec<String> = d
        .descendants()
        .filter(|n| n.has_tag_name("itemref") && n.attribute("linear") != Some("no"))
        .filter_map(|n| manifest.get(n.attribute("idref")?))
        .map(|href| normalize(&format!("{dir}{}", unescape(href))))
        .collect();
    let title = d.descendants().find(|n| n.tag_name().name() == "title").and_then(|n| n.text()).map(String::from);
    (!spine.is_empty()).then_some((spine, title))
}

/// `a/b/../c.xhtml` -> `a/c.xhtml`; anchors dropped.
pub fn normalize(p: &str) -> String {
    let p = p.split('#').next().unwrap_or(p);
    let mut out: Vec<&str> = Vec::new();
    for part in p.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            x => out.push(x),
        }
    }
    out.join("/")
}

fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Reads one named file out of an EPUB.
pub fn epub_file(path: &Path, entries: &[zipread::Entry], name: &str) -> Option<Vec<u8>> {
    let want = normalize(&unescape(name));
    let e = entries.iter().find(|e| normalize(&e.name) == want)?;
    zipread::read(path, e).ok()
}

// ---------- discovery (Open Library) ----------

fn ol_card(title: &str, authors: Vec<String>, cover: Option<i64>, year: Option<i64>, key: Option<&str>) -> Value {
    json!({
        "kind": "book",
        "title": title,
        "author": authors.first(),
        "authors": authors,
        "cover": cover.map(|c| format!("https://covers.openlibrary.org/b/id/{c}-L.jpg")),
        "year": year,
        "olid": key,
    })
}

async fn ol_get(url: &str) -> Result<Value, String> {
    reqwest::Client::builder()
        .user_agent("Chillarr (https://github.com/Viscyrius/Chillarr)")
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?
        .get(url)
        .send()
        .await
        .map_err(|e| format!("Open Library didn't answer: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Open Library sent something odd: {e}"))
}

pub async fn search(term: &str) -> Result<Vec<Value>, String> {
    let v = ol_get(&format!("https://openlibrary.org/search.json?q={}&fields=key,title,author_name,first_publish_year,cover_i&limit=30", enc(term.trim()))).await?;
    Ok(v["docs"].as_array().map(|a| a.iter().filter(|d| d["cover_i"].is_i64()).map(doc_card).collect()).unwrap_or_default())
}

fn doc_card(d: &Value) -> Value {
    let authors = d["author_name"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
    ol_card(d["title"].as_str().unwrap_or_default(), authors, d["cover_i"].as_i64(), d["first_publish_year"].as_i64(), d["key"].as_str())
}

/// The same card, as a comic: it's got as a CBZ or CBR and read page by page.
fn comic_card(d: &Value) -> Value {
    let mut c = doc_card(d);
    c["kind"] = json!("comic");
    c
}

/// Open Library's own subject for Western comics and graphic novels.
const COMIC_SUBJECT: &str = r#"subject:"comic books, strips, etc""#;

/// The comic rows: (row id, Open Library query, title). Measured 2026-11-09:
/// each gives Western comics with covers (Watchmen, Saga, Hellboy, Sandman).
fn comic_rows() -> Vec<(&'static str, String, &'static str)> {
    let recent = chrono::Utc::now().format("%Y").to_string().parse::<i32>().unwrap_or(2026) - 4;
    vec![
        ("comics-popular", format!("{COMIC_SUBJECT} language:eng"), "Popular comics and graphic novels"),
        ("comics-new", format!(r#"subject:"graphic novels" language:eng first_publish_year:[{recent} TO *]"#), "New graphic novels"),
        ("comics-heroes", format!(r#"{COMIC_SUBJECT} subject:"superheroes""#), "Superheroes"),
        ("comics-marvel", format!(r#"{COMIC_SUBJECT} publisher:"Marvel""#), "Marvel"),
        ("comics-dc", r#"publisher:"DC Comics""#.to_string(), "DC"),
        ("comics-image", r#"publisher:"Image Comics""#.to_string(), "Image Comics"),
        ("comics-darkhorse", format!(r#"{COMIC_SUBJECT} publisher:"Dark Horse""#), "Dark Horse"),
    ]
}

fn comic_url(q: &str, page: u32) -> String {
    format!("https://openlibrary.org/search.json?q={}&sort=rating&fields=key,title,author_name,first_publish_year,cover_i&limit={PER_PAGE}&page={}", enc(q), page.clamp(1, 50))
}

fn comic_items(v: &Value) -> Vec<Value> {
    v["docs"].as_array().map(|a| a.iter().filter(|d| d["cover_i"].is_i64()).map(comic_card).collect()).unwrap_or_default()
}

/// Comics only: the term, among Open Library's comics and graphic novels.
pub async fn search_comics(term: &str) -> Result<Vec<Value>, String> {
    let term = term.trim().replace('"', " ");
    let mut out = Vec::new();
    for q in [format!("{term} {COMIC_SUBJECT}"), format!(r#"{term} subject:"graphic novels""#)] {
        let v = ol_get(&comic_url(&q, 1)).await?;
        out.extend(comic_items(&v));
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|c| seen.insert(c["olid"].as_str().unwrap_or_default().to_string()));
    Ok(out)
}

/// The subject rows: (row id, Open Library subject, title).
const SUBJECTS: [(&str, &str, &str); 4] = [("books-fantasy", "fantasy", "Fantasy books"), ("books-scifi", "science_fiction", "Science fiction"), ("books-thriller", "thrillers", "Thrillers"), ("books-romance", "romance", "Romance novels")];
/// Books per row, and per page of a row's See all.
const PER_PAGE: u32 = 30;

fn subject_items(v: &Value) -> Vec<Value> {
    v["works"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|w| w["cover_id"].is_i64())
                .map(|w| {
                    let authors = w["authors"].as_array().map(|x| x.iter().filter_map(|a| a["name"].as_str().map(String::from)).collect()).unwrap_or_default();
                    ol_card(w["title"].as_str().unwrap_or_default(), authors, w["cover_id"].as_i64(), w["first_publish_year"].as_i64(), w["key"].as_str())
                })
                .collect()
        })
        .unwrap_or_default()
}

fn trending_items(v: &Value) -> Vec<Value> {
    v["works"].as_array().map(|a| a.iter().filter(|d| d["cover_i"].is_i64()).map(doc_card).collect()).unwrap_or_default()
}

/// Where a row's page `page` (from 1) comes from; None for a row that isn't a book row.
fn more_url(id: &str, page: u32) -> Option<String> {
    let page = page.clamp(1, 50);
    if let Some((_, q, _)) = comic_rows().into_iter().find(|(k, _, _)| *k == id) {
        return Some(comic_url(&q, page));
    }
    if id == "books-trending" {
        return Some(format!("https://openlibrary.org/trending/weekly.json?limit={PER_PAGE}&page={page}"));
    }
    let (_, subject, _) = SUBJECTS.iter().find(|(k, _, _)| *k == id)?;
    Some(format!("https://openlibrary.org/subjects/{subject}.json?limit={PER_PAGE}&offset={}&sort=rating", (page - 1) * PER_PAGE))
}

/// Book rows for the Read tab.
pub async fn rows() -> Vec<Value> {
    let mut out = Vec::new();
    if let Ok(v) = ol_get(&more_url("books-trending", 1).unwrap_or_default()).await {
        out.push(json!({ "id": "books-trending", "title": "Books people are reading", "items": trending_items(&v) }));
    }
    for (id, _, title) in SUBJECTS {
        let Ok(v) = ol_get(&more_url(id, 1).unwrap_or_default()).await else { continue };
        out.push(json!({ "id": id, "title": title, "items": subject_items(&v) }));
    }
    out
}

/// Comic rows for the Read tab's Books page.
pub async fn comic_rows_now() -> Vec<Value> {
    let rows = comic_rows();
    let got = futures_join(rows.iter().map(|(_, q, _)| comic_url(q, 1)).collect()).await;
    rows.into_iter().zip(got).filter_map(|((id, _, title), v)| Some(json!({ "id": id, "title": title, "items": comic_items(&v?) }))).collect()
}

/// Every URL at once (Open Library takes a few seconds each).
async fn futures_join(urls: Vec<String>) -> Vec<Option<Value>> {
    let tasks: Vec<_> = urls.into_iter().map(|u| tokio::spawn(async move { ol_get(&u).await.ok() })).collect();
    let mut out = Vec::new();
    for t in tasks {
        out.push(t.await.ok().flatten());
    }
    out
}

/// One page of a book row's See all: { items, more }.
pub async fn more(id: &str, page: u32) -> Result<Value, String> {
    let url = more_url(id, page).ok_or("That isn't a book row.")?;
    let v = ol_get(&url).await?;
    let raw = v["works"].as_array().map(|a| a.len()).unwrap_or(0);
    if id.starts_with("comics-") {
        let raw = v["docs"].as_array().map(|a| a.len()).unwrap_or(0);
        return Ok(json!({ "items": comic_items(&v), "more": raw as u32 >= PER_PAGE && page < 50 }));
    }
    let items = if id == "books-trending" { trending_items(&v) } else { subject_items(&v) };
    // A full page means there may be more; Open Library's trending list stops after a few pages.
    Ok(json!({ "items": items, "more": raw as u32 >= PER_PAGE && page < 50 }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn res(title: &str, size_mb: u64, seeders: i64, magnet: bool) -> Value {
        json!({ "title": title, "size": size_mb * 1024 * 1024, "seeders": seeders, "indexer": "x", "protocol": "torrent",
            "magnetUrl": if magnet { json!("magnet:?xt=urn:btih:0123456789ABCDEF0123456789ABCDEF01234567&dn=x") } else { Value::Null },
            "downloadUrl": "http://localhost:9696/1/download?link=abc" })
    }

    #[test]
    fn book_rows_page_through_open_library() {
        assert_eq!(more_url("books-trending", 2).as_deref(), Some("https://openlibrary.org/trending/weekly.json?limit=30&page=2"));
        assert_eq!(more_url("books-fantasy", 1).as_deref(), Some("https://openlibrary.org/subjects/fantasy.json?limit=30&offset=0&sort=rating"));
        assert_eq!(more_url("books-scifi", 3).as_deref(), Some("https://openlibrary.org/subjects/science_fiction.json?limit=30&offset=60&sort=rating"));
        assert_eq!(more_url("books-scifi", 0), more_url("books-scifi", 1), "page 0 is the first page");
        assert_eq!(more_url("trending", 1), None, "only book rows");
        assert_eq!(more_url("../etc", 1), None);
    }

    #[test]
    fn offered_copies_hide_their_links() {
        let shown = offer(vec![Release { id: String::new(), title: "A".into(), indexer: "x".into(), size: 1, seeders: 1, format: None, url: "magnet:?xt=urn:btih:00".into() }]);
        assert!(shown[0].url.is_empty() && !shown[0].id.is_empty());
        assert_eq!(offered(&shown[0].id).unwrap().url, "magnet:?xt=urn:btih:00");
        assert!(offered("made-up").is_none());
    }

    #[test]
    fn epub_first_then_seeders_and_only_this_book() {
        // Measured 2026-11-08 from Prowlarr, plus a pack and a wrong book.
        let r = rank(&[
            res("Project Hail Mary by Andy Weir AZW3", 5, 130, true),
            res("Project Hail Mary by Andy Weir EPUB", 10, 380, true),
            res("Project Hail Mary by Andy Weir EPUB", 10, 36, false),
            res("Andy Weir Collection (Artemis, The Martian, Project Hail Mary) EPUB", 900, 50, true),
            res("Artemis by Andy Weir EPUB", 3, 999, true),
            res("Project Hail Mary audiobook", 10, 0, true),
        ], "Project Hail Mary", false);
        assert_eq!(r.iter().map(|x| (x.format.clone().unwrap_or_default(), x.seeders)).collect::<Vec<_>>(), vec![("epub".into(), 380), ("epub".into(), 36), ("azw3".into(), 130)]);
        assert!(r[0].url.starts_with("magnet:") && r[1].url.starts_with("http"), "magnet when there is one");
    }

    #[test]
    fn the_one_ebook_in_a_torrent() {
        let files = vec![("Pack/cover.jpg".to_string(), 1), ("Pack/Artemis.epub".to_string(), 1), ("Pack/Project Hail Mary.pdf".to_string(), 1), ("Pack/Project Hail Mary.epub".to_string(), 1)];
        assert_eq!(pick_file(&files, "Project Hail Mary", false), Some(3));
        assert_eq!(pick_file(&[("a.txt".to_string(), 1)], "x", false), None);
    }

    #[test]
    fn comics_want_cbz_then_cbr() {
        let r = rank(&[
            res("Watchmen (1986-1987) (Digital) (CBR)", 600, 90, true),
            res("Watchmen (1986-1987) (Digital) (CBZ)", 700, 40, true),
            res("Watchmen EPUB", 5, 999, true),
            res("Watchmen complete 4K remux", 9000, 999, true),
        ], "Watchmen", true);
        assert_eq!(r.iter().map(|x| x.format.clone().unwrap_or_default()).collect::<Vec<_>>(), vec!["cbz", "cbr"]);
        // In a pack of issues, the one named for it, CBZ or CBR alike.
        let files = vec![("Pack/Saga 001.cbr".to_string(), 1), ("Pack/Saga Vol 1.cbr".to_string(), 1), ("Pack/Watchmen.epub".to_string(), 1), ("Pack/cover.jpg".to_string(), 1)];
        assert_eq!(pick_file(&files, "Saga Vol 1", true), Some(1));
        assert_eq!(pick_file(&[("Watchmen.epub".to_string(), 1)], "Watchmen", true), None, "an ebook isn't a comic");
        assert!(looks_like_a_comic(&json!({ "title": "Hellboy v01 (2003) (cbr)" })));
        assert!(looks_like_a_comic(&json!({ "title": "Hellboy", "categories": [{ "id": 7030 }] })));
        assert!(!looks_like_a_comic(&json!({ "title": "Hellboy 2019 1080p" })));
    }

    #[test]
    fn comic_rows_page_through_open_library() {
        let u = more_url("comics-marvel", 2).unwrap();
        assert!(u.starts_with("https://openlibrary.org/search.json?q=") && u.contains("page=2") && u.contains("Marvel"), "{u}");
        assert!(more_url("comics-nope", 1).is_none());
    }

    #[test]
    fn a_cbz_written_here_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("1.jpg"), dir.path().join("2.png"));
        std::fs::write(&a, b"first page").unwrap();
        std::fs::write(&b, vec![7u8; 70_000]).unwrap();
        let out = dir.path().join("c.cbz");
        write_cbz(&out, &[("Issue 1/p01.jpg".into(), a), ("Issue 1/p02.png".into(), b)]).unwrap();
        let es = zipread::entries(&out).unwrap();
        assert_eq!(es.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), vec!["Issue 1/p01.jpg", "Issue 1/p02.png"]);
        assert_eq!(zipread::read(&out, &es[0]).unwrap(), b"first page");
        assert_eq!(zipread::read(&out, &es[1]).unwrap(), vec![7u8; 70_000]);
        assert!(!is_rar(&out));
    }

    /// Needs the tar.exe Windows ships, and a RAR made by it is impossible, so
    /// this only checks a fake RAR is refused cleanly and leaves nothing behind.
    #[test]
    fn a_broken_cbr_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("x.cbr");
        std::fs::write(&from, b"Rar!\x1a\x07\x00 not really").unwrap();
        assert!(is_rar(&from));
        let to = dir.path().join("x.cbz");
        assert!(rar_to_cbz(&from, &to).is_err());
        assert!(!to.exists() && !dir.path().join("x.unpacking").exists());
    }

    #[test]
    fn magnets_and_names() {
        assert_eq!(magnet_hash("magnet:?xt=urn:btih:0123456789ABCDEF0123456789ABCDEF01234567&dn=x").as_deref(), Some("0123456789abcdef0123456789abcdef01234567"));
        assert_eq!(magnet_hash("http://x/y"), None);
        assert_eq!(file_name(Some("Andy Weir"), "Project: Hail Mary?", "epub"), "Andy Weir - Project Hail Mary.epub");
        assert_eq!(file_name(None, "A/B", "pdf"), "A B.pdf");
    }

    #[test]
    fn epub_reading_order() {
        let container = r#"<?xml version="1.0"?><container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#;
        let opf = r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Hail Mary</dc:title></metadata>
            <manifest><item id="c" href="Text/cover.xhtml"/><item id="a" href="Text/ch%201.xhtml"/><item id="b" href="../Extra/ch2.xhtml"/><item id="n" href="nav.xhtml"/></manifest>
            <spine><itemref idref="c" linear="no"/><itemref idref="a"/><itemref idref="b"/></spine></package>"#;
        let (spine, title) = epub_spine(container, |p| (p == "OEBPS/content.opf").then(|| opf.to_string())).unwrap();
        assert_eq!(spine, vec!["OEBPS/Text/ch 1.xhtml", "Extra/ch2.xhtml"]);
        assert_eq!(title.as_deref(), Some("Hail Mary"));
        assert_eq!(normalize("a/./b/../c.xhtml#p3"), "a/c.xhtml");
    }

    #[test]
    fn real_epub_files_come_out() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("b.epub");
        std::fs::write(&p, crate::zipread::tests::make_zip(&[("mimetype", b"application/epub+zip", false), ("OEBPS/Text/ch 1.xhtml", b"<p>Hi</p>", true)])).unwrap();
        let es = crate::zipread::entries(&p).unwrap();
        assert_eq!(epub_file(&p, &es, "OEBPS/Text/ch%201.xhtml").unwrap(), b"<p>Hi</p>");
        assert!(epub_file(&p, &es, "../../etc/passwd").is_none());
    }
}
