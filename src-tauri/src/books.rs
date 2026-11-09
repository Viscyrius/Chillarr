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
    ["epub", "azw3", "mobi", "pdf"].iter().find(|f| low.contains(*f)).map(|f| f.to_string())
}

fn format_rank(f: Option<&str>) -> i32 {
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
pub fn rank(results: &[Value], title: &str) -> Vec<Release> {
    let mut out: Vec<Release> = results
        .iter()
        .filter_map(|x| {
            let t = x["title"].as_str()?;
            let url = x["magnetUrl"].as_str().filter(|u| !u.is_empty()).or(x["downloadUrl"].as_str())?.to_string();
            let size = x["size"].as_u64().unwrap_or(0);
            (matches(t, title) && size > 0 && size <= MAX_BOOK && x["protocol"].as_str().is_none_or(|p| p == "torrent")).then(|| Release {
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
        .collect();
    out.sort_by(|a, b| format_rank(b.format.as_deref()).cmp(&format_rank(a.format.as_deref())).then(b.seeders.cmp(&a.seeders)));
    out
}

/// Of a torrent's files, the one ebook to keep: the right format first,
/// then the best name match. Index into `files`.
pub fn pick_file(files: &[(String, u64)], title: &str) -> Option<usize> {
    let ext = |n: &str| n.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    (0..files.len())
        .filter(|&i| EBOOK_EXT.contains(&ext(&files[i].0).as_str()))
        .max_by(|&a, &b| {
            let s = |i: usize| (format_rank(Some(&ext(&files[i].0))), (read_core::title_score(files[i].0.rsplit('/').next().unwrap_or_default(), title) * 100.0) as i64);
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
    for q in [format!("{} {surname}", meta.title), meta.title.clone()] {
        let path = format!("search?query={}&categories=7000&categories=7020&type=search", enc(q.trim()));
        found = p.list_slow(&path).await.map_err(|e| format!("Couldn't search the download sites: {e}"))?;
        if !rank(&found, &meta.title).is_empty() {
            break;
        }
    }
    // Many sites file ebooks under no book category at all (LimeTorrents,
    // TorrentDownload and 1337x had 18, 10 and 2 for "Percy Jackson epub" and
    // none in the categories, 2026-11-08), so ask them once more by name.
    let plain = format!("search?query={}&type=search", enc(format!("{} epub", meta.title).trim()));
    if let Ok(more) = p.list_slow(&plain).await {
        found.extend(more.into_iter().filter(looks_like_a_book));
    }
    Ok(offer(rank(&dedup(found), &meta.title)))
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

pub(crate) fn enc(s: &str) -> String {
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
    };
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
    if pick.size > MAX_BOOK {
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
        let Some(keep) = pick_file(&named, &b.title) else {
            // Nothing to read in it: it goes, rather than download and share for ever.
            let _ = q.delete(&[hash.clone()], true).await;
            b.problem = Some("That download had no ebook in it. Pick another copy.".into());
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
        let name = file_name(b.author.as_deref(), &b.title, &ext);
        let _ = std::fs::create_dir_all(&dir);
        let to = dir.join(&name);
        // A hardlink when it's the same drive (no extra space), else a copy.
        let filed = std::fs::hard_link(&from, &to).or_else(|_| std::fs::copy(&from, &to).map(|_| ()));
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

pub(crate) async fn ol_get(url: &str) -> Result<Value, String> {
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

/// One page of a book row's See all: { items, more }.
pub async fn more(id: &str, page: u32) -> Result<Value, String> {
    let url = more_url(id, page).ok_or("That isn't a book row.")?;
    let v = ol_get(&url).await?;
    let raw = v["works"].as_array().map(|a| a.len()).unwrap_or(0);
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
        ], "Project Hail Mary");
        assert_eq!(r.iter().map(|x| (x.format.clone().unwrap_or_default(), x.seeders)).collect::<Vec<_>>(), vec![("epub".into(), 380), ("epub".into(), 36), ("azw3".into(), 130)]);
        assert!(r[0].url.starts_with("magnet:") && r[1].url.starts_with("http"), "magnet when there is one");
    }

    #[test]
    fn the_one_ebook_in_a_torrent() {
        let files = vec![("Pack/cover.jpg".to_string(), 1), ("Pack/Artemis.epub".to_string(), 1), ("Pack/Project Hail Mary.pdf".to_string(), 1), ("Pack/Project Hail Mary.epub".to_string(), 1)];
        assert_eq!(pick_file(&files, "Project Hail Mary"), Some(3));
        assert_eq!(pick_file(&[("a.txt".to_string(), 1)], "x"), None);
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
