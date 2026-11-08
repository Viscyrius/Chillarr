//! Reading's disk space (issue #39): what each series and book takes, where
//! it all is, and deleting downloads without unfollowing.
//!
//! Chapters are deleted through Suwayomi (`deleteDownloadedChapters`), so its
//! database stays right and "Get the other N" brings them back. A chapter
//! deleted by hand is kept in `Followed.removed`, so new-chapter fetching
//! doesn't download it again by itself. Its sharpened copy (sharpen.rs) goes
//! with it.
//!
//! Each profile's reading is its own: deleting what another profile also has
//! in its list needs `others_ok`, which the window sends only after saying who.

use crate::read::{self, Followed};
use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::AppHandle;

#[derive(Serialize, Clone, Debug, Default)]
pub struct Folder {
    /// "Reading", "Comics", "Books", "Sharpened".
    pub name: String,
    pub path: String,
    pub bytes: u64,
    pub exists: bool,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct SeriesUse {
    pub key: String,
    pub title: String,
    pub cover: Option<String>,
    pub bytes: u64,
    /// Of that, the sharpened copies.
    pub sharp_bytes: u64,
    /// Chapters on disk, and how many of them the watching profile has read.
    pub chapters: usize,
    pub read: usize,
    /// In the watching profile's list.
    pub mine: bool,
    /// Other profiles that have it in their list.
    pub others: Vec<String>,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct BookUse {
    pub key: String,
    pub title: String,
    pub author: Option<String>,
    pub bytes: u64,
    pub mine: bool,
    pub others: Vec<String>,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Overview {
    pub folders: Vec<Folder>,
    /// Free and total space on the reading folder's drive.
    pub free: u64,
    pub total: u64,
    /// Biggest first.
    pub series: Vec<SeriesUse>,
    pub books: Vec<BookUse>,
    pub chapters: usize,
    pub comics_bytes: u64,
    pub books_bytes: u64,
    pub sharp_bytes: u64,
}

/// What a delete would take, asked before it happens.
#[derive(Serialize, Clone, Debug, Default)]
pub struct Plan {
    pub chapters: usize,
    pub bytes: u64,
    pub others: Vec<String>,
}

// ---------- sizes ----------

fn file_size(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

pub fn folder_size(dir: &Path) -> u64 {
    let Ok(it) = std::fs::read_dir(dir) else { return 0 };
    it.filter_map(|e| e.ok())
        .map(|e| {
            let p = e.path();
            // Not followed through links: a junction could point anywhere.
            match e.file_type() {
                Ok(t) if t.is_dir() => folder_size(&p),
                Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
                _ => 0,
            }
        })
        .sum()
}

#[cfg(windows)]
fn disk_space(dir: &Path) -> (u64, u64) {
    use windows::core::HSTRING;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    // The nearest folder that exists: the reading folder may not be made yet.
    let mut at = dir.to_path_buf();
    while !at.exists() {
        if !at.pop() {
            return (0, 0);
        }
    }
    let (mut free, mut total) = (0u64, 0u64);
    let p = HSTRING::from(at.as_os_str());
    if unsafe { GetDiskFreeSpaceExW(&p, Some(&mut free), Some(&mut total), None) }.is_ok() {
        (free, total)
    } else {
        (0, 0)
    }
}

#[cfg(not(windows))]
fn disk_space(_dir: &Path) -> (u64, u64) {
    (0, 0)
}

/// Where a chapter's sharpened copy sits: the same place under Sharpened as
/// the original has under mangas.
pub fn sharp_copy_of(comics: &Path, file: &Path) -> Option<PathBuf> {
    file.strip_prefix(comics.join("mangas")).ok().map(|rel| comics.join("Sharpened").join(rel))
}

// ---------- who has what ----------

fn names_of(app: &AppHandle, ids: &[String]) -> Vec<String> {
    let all = crate::profiles::all_ids(app);
    ids.iter().map(|id| all.iter().find(|(i, _)| i == id).map(|(_, n)| n.clone()).unwrap_or_else(|| "Another profile".into())).collect()
}

/// The profiles other than the watching one that have it.
fn others(app: &AppHandle, owners: &[String], me: &str) -> Vec<String> {
    let pc_owner = crate::profiles::owner_id(app);
    let ids: Vec<String> = read::owners_of(owners, pc_owner.as_deref()).into_iter().filter(|o| o != me).collect();
    names_of(app, &ids)
}

/// Any followed series, whoever follows it: Settings lists all of them.
fn find_any(app: &AppHandle, key: &str) -> Result<Followed, String> {
    read::followed(app).into_iter().find(|f| f.key == key).ok_or_else(|| "That series isn't followed any more.".into())
}

/// A series' downloaded chapters: (chapter id, its file, the sharpened copy).
async fn on_disk(app: &AppHandle, f: &Followed) -> Result<Vec<(crate::suwayomi::Chapter, PathBuf, Option<PathBuf>)>, String> {
    let comics = read::comics_dir(app);
    let copies = f.copies();
    let dirs: Vec<Option<PathBuf>> = copies.iter().map(|(_, s, t)| read::series_dir(app, s, t)).collect();
    let merged = read::chapters_of(f).await?;
    Ok(merged
        .into_iter()
        .filter(|(c, _)| c.downloaded)
        .filter_map(|(c, i)| {
            let dir = dirs.get(i.min(dirs.len() - 1))?.as_ref()?;
            let file = read::chapter_file(dir, &c)?;
            let sharp = comics.as_ref().and_then(|cd| sharp_copy_of(cd, &file)).filter(|p| p.is_file());
            Some((c, file, sharp))
        })
        .collect())
}

fn picked<'a, T>(list: &'a [(crate::suwayomi::Chapter, T, Option<PathBuf>)], ids: Option<&[i64]>) -> Vec<&'a (crate::suwayomi::Chapter, T, Option<PathBuf>)> {
    list.iter().filter(|(c, ..)| ids.is_none_or(|ids| ids.contains(&c.id))).collect()
}

// ---------- the window ----------

pub async fn overview(app: &AppHandle) -> Result<Overview, String> {
    let me = read::uid(app)?;
    let mut o = Overview::default();
    let Some(root) = read::root(app) else { return Ok(o) };
    let comics = root.join("Comics");
    let books = root.join("Books");
    let sharp = comics.join("Sharpened");
    let (free, total) = disk_space(&root);
    o.free = free;
    o.total = total;
    let spots = read::spots(app, &me);
    for f in read::followed(app) {
        let Ok(files) = on_disk(app, &f).await else { continue };
        if files.is_empty() {
            continue;
        }
        let sharp_bytes: u64 = files.iter().filter_map(|x| x.2.as_deref()).map(file_size).sum();
        let bytes = files.iter().map(|x| file_size(&x.1)).sum::<u64>() + sharp_bytes;
        let read_here = spots.get(&f.key).map(|s| files.iter().filter(|x| s.read.contains_key(&x.0.id)).count()).unwrap_or(0);
        o.chapters += files.len();
        o.series.push(SeriesUse {
            key: f.key.clone(),
            title: f.title.clone(),
            cover: f.cover.clone(),
            bytes,
            sharp_bytes,
            chapters: files.len(),
            read: read_here,
            mine: read::mine(app, &f, &me),
            others: others(app, &f.owners, &me),
        });
    }
    o.series.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    let pc_owner = crate::profiles::owner_id(app);
    for b in crate::books::all(app) {
        let Some(p) = crate::books::path_of(app, &b) else { continue };
        o.books.push(BookUse {
            key: b.key.clone(),
            title: b.title.clone(),
            author: b.author.clone(),
            bytes: file_size(&p),
            mine: read::owned_by(&b.owners, pc_owner.as_deref(), &me),
            others: others(app, &b.owners, &me),
        });
    }
    o.books.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    o.comics_bytes = folder_size(&comics.join("mangas"));
    o.books_bytes = folder_size(&books);
    o.sharp_bytes = folder_size(&sharp);
    o.folders = vec![
        Folder { name: "Reading".into(), path: root.to_string_lossy().into(), bytes: o.comics_bytes + o.books_bytes + o.sharp_bytes, exists: root.is_dir() },
        Folder { name: "Comics".into(), path: comics.to_string_lossy().into(), bytes: o.comics_bytes, exists: comics.is_dir() },
        Folder { name: "Books".into(), path: books.to_string_lossy().into(), bytes: o.books_bytes, exists: books.is_dir() },
        Folder { name: "Sharpened".into(), path: sharp.to_string_lossy().into(), bytes: o.sharp_bytes, exists: sharp.is_dir() },
    ];
    Ok(o)
}

/// How many chapters and bytes a delete takes (`ids` None: every downloaded one).
pub async fn plan(app: &AppHandle, key: &str, ids: Option<Vec<i64>>) -> Result<Plan, String> {
    let me = read::uid(app)?;
    let f = find_any(app, key)?;
    let files = on_disk(app, &f).await?;
    let chosen = picked(&files, ids.as_deref());
    Ok(Plan {
        chapters: chosen.len(),
        bytes: chosen.iter().map(|x| file_size(&x.1) + x.2.as_deref().map(file_size).unwrap_or(0)).sum(),
        others: others(app, &f.owners, &me),
    })
}

/// Deletes downloaded chapters (`ids` None: all of them); the series stays followed.
pub async fn delete(app: &AppHandle, key: &str, ids: Option<Vec<i64>>, others_ok: bool) -> Result<String, String> {
    let me = read::uid(app)?;
    let f = find_any(app, key)?;
    let who = others(app, &f.owners, &me);
    if !who.is_empty() && !others_ok {
        return Err(format!("{} still reads this.", who.join(", ")));
    }
    let files = on_disk(app, &f).await?;
    let chosen = picked(&files, ids.as_deref());
    if chosen.is_empty() {
        return Ok("Nothing of that is downloaded.".into());
    }
    let doomed: Vec<i64> = chosen.iter().map(|x| x.0.id).collect();
    // Kept first: if the delete half works, what went still doesn't come back by itself.
    read::update_followed(app, key, |x| {
        let new: Vec<i64> = doomed.iter().copied().filter(|id| !x.removed.contains(id)).collect();
        x.removed.extend(new);
        // Bounded: a chapter id is one number, but a long series deleted many times adds up.
        let n = x.removed.len();
        if n > 20_000 {
            x.removed.drain(..n - 20_000);
        }
    });
    crate::suwayomi::delete_chapters(&doomed).await?;
    for x in &chosen {
        if let Some(s) = &x.2 {
            let _ = std::fs::remove_file(s);
        }
    }
    let msg = match doomed.len() {
        1 => format!("Deleted 1 chapter of {}.", f.title),
        n => format!("Deleted {n} chapters of {}.", f.title),
    };
    crate::log::log(app, crate::model::Level::Ok, &msg);
    Ok(msg)
}

/// A book's file goes; the book stays in the list as not downloaded.
pub async fn delete_book(app: &AppHandle, key: &str, others_ok: bool) -> Result<String, String> {
    crate::books::delete_file(app, key, others_ok).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sharpened_copy_mirrors_its_original() {
        let comics = Path::new(r"D:\Media\Reading\Comics");
        let file = Path::new(r"D:\Media\Reading\Comics\mangas\Weeb Central (EN)\Solo Leveling\Ch 1.cbz");
        assert_eq!(sharp_copy_of(comics, file).unwrap(), Path::new(r"D:\Media\Reading\Comics\Sharpened\Weeb Central (EN)\Solo Leveling\Ch 1.cbz"));
        assert!(sharp_copy_of(comics, Path::new(r"E:\elsewhere\Ch 1.cbz")).is_none(), "outside the reading folder: none");
    }

    #[test]
    fn folder_sizes_add_up_inside_and_skip_missing() {
        let dir = std::env::temp_dir().join(format!("chillarr-rs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::write(dir.join("one"), [0u8; 10]).unwrap();
        std::fs::write(dir.join("a/b/two"), [0u8; 32]).unwrap();
        assert_eq!(folder_size(&dir), 42);
        assert_eq!(folder_size(&dir.join("nope")), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn picking_none_takes_every_chapter() {
        let ch = |id| crate::suwayomi::Chapter { id, name: String::new(), number: None, scanlator: None, downloaded: true, pages: 0, uploaded: None };
        let list = vec![(ch(1), (), None), (ch(2), (), None), (ch(3), (), None)];
        assert_eq!(picked(&list, None).len(), 3);
        assert_eq!(picked(&list, Some(&[2, 9])).iter().map(|x| x.0.id).collect::<Vec<_>>(), vec![2]);
    }
}
