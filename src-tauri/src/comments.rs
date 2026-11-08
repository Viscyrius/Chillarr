//! Comments on a title's page (movies, series, anime; books later). One
//! list per title, shared by everyone on this Chillarr: the profiles in the
//! window and whoever signs in to Jellyfin's web pages (bridge.rs). Each
//! comment keeps who wrote it (their Jellyfin user id and name) and when;
//! only its writer can edit or delete it.
//!
//! Kept in comments.json in Chillarr's data folder, written whole each time
//! (catalog::save_atomic). A title is named by a key that doesn't depend on
//! where it was opened from: "tmdb:<id>" for movies, "tvdb:<id>" for series,
//! "anilist:<id>" for anime only AniList knows, "jf:<id>" as a last resort.

use crate::tiles::Tile;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::AppHandle;

/// The longest comment, in characters.
pub const MAX_LEN: usize = 2000;
/// Comments one title holds at most.
const MAX_PER_TITLE: usize = 1000;
/// Posts and edits one person can make in `RATE_WINDOW`.
const RATE_MAX: usize = 5;
const RATE_WINDOW: Duration = Duration::from_secs(60);

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Comment {
    id: String,
    /// The writer's Jellyfin user id.
    by: String,
    /// Their name when they wrote it (shown when they're no longer a profile).
    name: String,
    /// Unix seconds.
    at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    edited: Option<i64>,
    text: String,
}

#[derive(Serialize, Deserialize, Default, Debug)]
struct Store {
    #[serde(default)]
    titles: HashMap<String, Vec<Comment>>,
}

/// One comment as a page shows it.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct View {
    pub id: String,
    pub name: String,
    pub at: i64,
    pub edited: Option<i64>,
    pub text: String,
    /// Written by whoever is asking: they can edit and delete it.
    pub mine: bool,
}

/// Held while comments.json is read and written back.
static IO: Mutex<()> = Mutex::new(());

fn file(dir: &Path) -> PathBuf {
    dir.join("comments.json")
}

/// Every comment. Only a missing file is an empty store: one that can't be
/// read right now (another program holding it) is an error, so a post never
/// saves an empty store over everyone's comments; a damaged one is kept aside
/// as comments.json.bad.
fn load(dir: &Path) -> Result<Store, String> {
    let path = file(dir);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Store::default()),
        Err(_) => return Err("Comments couldn't be read just now. Try again in a moment.".into()),
    };
    serde_json::from_str(&text).map_err(|_| {
        let _ = std::fs::rename(&path, path.with_extension("json.bad"));
        "The comments file was damaged and was set aside. New comments start fresh.".to_string()
    })
}

fn save(dir: &Path, s: &Store) -> Result<(), String> {
    let text = serde_json::to_string(s).map_err(|e| e.to_string())?;
    crate::catalog::save_atomic(&file(dir), &text).map_err(|e| format!("Couldn't save the comment: {e}"))
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// A title key a page may send: "<source>:<id>", short, letters, digits and dashes.
fn key_ok(key: &str) -> bool {
    let Some((source, id)) = key.split_once(':') else { return false };
    key.len() <= 80
        && (1..=12).contains(&source.len())
        && source.bytes().all(|b| b.is_ascii_lowercase())
        && !id.is_empty()
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The comment's text, trimmed, or why it can't be posted.
fn clean(text: &str) -> Result<String, String> {
    let t = text.trim();
    if t.is_empty() {
        return Err("Write something first.".into());
    }
    if t.chars().count() > MAX_LEN {
        return Err(format!("That's too long. Keep it under {MAX_LEN} characters."));
    }
    Ok(t.to_string())
}

/// Recent posts and edits per person, for the rate limit.
#[derive(Default)]
struct Rate(HashMap<String, VecDeque<Instant>>);

impl Rate {
    /// Counts one post or edit. Err: seconds to wait.
    fn take(&mut self, who: &str, now: Instant) -> Result<(), u64> {
        let q = self.0.entry(who.to_string()).or_default();
        while q.front().is_some_and(|t| now.duration_since(*t) >= RATE_WINDOW) {
            q.pop_front();
        }
        if q.len() >= RATE_MAX {
            let wait = RATE_WINDOW - now.duration_since(q[0]);
            return Err(wait.as_secs().max(1));
        }
        q.push_back(now);
        Ok(())
    }
}

fn rate() -> &'static Mutex<Rate> {
    static R: OnceLock<Mutex<Rate>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(Rate::default()))
}

fn slow_down(secs: u64) -> String {
    format!("You're posting fast. Try again in {secs} seconds.")
}

/// Who is writing: (Jellyfin user id, name). A web page's viewer, or the
/// profile picked in the window. None while the window is locked.
fn author(app: &AppHandle) -> Option<(String, String)> {
    if let Some(v) = crate::viewer::current() {
        return Some((v.session.user_id, v.session.user));
    }
    let id = crate::media::active_user(app)?;
    let name = names(app).remove(&id).unwrap_or_default();
    Some((id, name))
}

/// Every profile's name now, by Jellyfin user id: a renamed profile shows its new name.
fn names(app: &AppHandle) -> HashMap<String, String> {
    crate::profiles::all_ids(app).into_iter().collect()
}

fn view(c: &Comment, me: Option<&str>, names: &HashMap<String, String>) -> View {
    let name = names.get(&c.by).filter(|n| !n.is_empty()).cloned().unwrap_or_else(|| c.name.clone());
    View { id: c.id.clone(), name: if name.is_empty() { "Someone".into() } else { name }, at: c.at, edited: c.edited, text: c.text.clone(), mine: me == Some(c.by.as_str()) }
}

/// A title's comments, newest first.
fn list_in(s: &Store, key: &str, me: Option<&str>, names: &HashMap<String, String>) -> Vec<View> {
    let mut out: Vec<View> = s.titles.get(key).into_iter().flatten().map(|c| view(c, me, names)).collect();
    out.sort_by(|a, b| b.at.cmp(&a.at).then(b.id.cmp(&a.id)));
    out
}

fn add_in(s: &mut Store, key: &str, by: &str, name: &str, text: String, id: String, at: i64) -> Result<Comment, String> {
    let list = s.titles.entry(key.to_string()).or_default();
    if list.len() >= MAX_PER_TITLE {
        return Err("This title can't take more comments.".into());
    }
    let c = Comment { id, by: by.into(), name: name.into(), at, edited: None, text };
    list.push(c.clone());
    Ok(c)
}

/// Finds `id` under `key`, and only if `by` wrote it.
fn own_mut<'a>(s: &'a mut Store, key: &str, id: &str, by: &str) -> Result<&'a mut Comment, String> {
    let c = s.titles.get_mut(key).and_then(|l| l.iter_mut().find(|c| c.id == id)).ok_or("That comment is gone.")?;
    if c.by != by {
        return Err("You can only change your own comments.".into());
    }
    Ok(c)
}

fn delete_in(s: &mut Store, key: &str, id: &str, by: &str) -> Result<(), String> {
    own_mut(s, key, id, by)?;
    if let Some(list) = s.titles.get_mut(key) {
        list.retain(|c| c.id != id);
        if list.is_empty() {
            s.titles.remove(key);
        }
    }
    Ok(())
}

fn new_id() -> Result<String, String> {
    let mut raw = [0u8; 8];
    getrandom::getrandom(&mut raw).map_err(|_| "Couldn't post that right now.".to_string())?;
    Ok(raw.iter().map(|b| format!("{b:02x}")).collect())
}

fn dir(app: &AppHandle) -> Result<PathBuf, String> {
    crate::media::data_dir(app).ok_or_else(|| "Chillarr's data folder is missing.".into())
}

/// Runs file work off the async threads.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tokio::task::spawn_blocking(f).await.map_err(|e| e.to_string())?
}

/// The key comments for this title are kept under, or None when there is
/// nothing to name it by. A library title is looked up in Jellyfin (as the
/// viewer) for its TMDb or TVDB id, so it shares comments with the same
/// title opened from a row of things to download.
pub async fn key_for(app: &AppHandle, t: &Tile) -> Option<String> {
    let from_ids = |kind: &str, tmdb: Option<i64>, tvdb: Option<i64>| match (kind, tmdb, tvdb) {
        ("movie", Some(id), _) => Some(format!("tmdb:{id}")),
        (_, _, Some(id)) => Some(format!("tvdb:{id}")),
        (_, Some(id), _) => Some(format!("tmdb:{id}")),
        _ => None,
    };
    if let Some(k) = from_ids(&t.kind, t.tmdb.filter(|i| *i > 0), t.tvdb.filter(|i| *i > 0)) {
        return Some(k);
    }
    if t.key.starts_with("anilist:") && key_ok(&t.key) {
        return Some(t.key.clone());
    }
    let id = t.series_id.as_deref().or(t.jf_id.as_deref()).filter(|s| !s.is_empty())?;
    let fallback = format!("jf:{id}");
    if !key_ok(&fallback) {
        return None;
    }
    if let Some(j) = crate::media::jf(app).filter(|j| j.user_id.is_some()) {
        let uid = j.user_id.clone().unwrap_or_default();
        if let Ok(v) = j.get_json(&format!("/Items?userId={uid}&ids={id}&fields=ProviderIds")).await {
            let it = &v["Items"][0];
            let kind = if it["Type"] == "Movie" { "movie" } else { "series" };
            let num = |k: &str| it["ProviderIds"][k].as_str().and_then(|s| s.parse::<i64>().ok()).filter(|i| *i > 0);
            if let Some(k) = from_ids(kind, num("Tmdb"), num("Tvdb")) {
                return Some(k);
            }
        }
    }
    Some(fallback)
}

pub async fn list(app: &AppHandle, key: &str) -> Result<Vec<View>, String> {
    if !key_ok(key) {
        return Err("That title can't have comments.".into());
    }
    let (d, me, names) = (dir(app)?, author(app).map(|a| a.0), names(app));
    let key = key.to_string();
    blocking(move || {
        let _io = IO.lock().unwrap();
        Ok(list_in(&load(&d)?, &key, me.as_deref(), &names))
    })
    .await
}

pub async fn add(app: &AppHandle, key: &str, text: &str) -> Result<View, String> {
    if !key_ok(key) {
        return Err("That title can't have comments.".into());
    }
    let text = clean(text)?;
    let (by, name) = author(app).ok_or("Pick a profile first.")?;
    rate().lock().unwrap().take(&by, Instant::now()).map_err(slow_down)?;
    let (d, key, id) = (dir(app)?, key.to_string(), new_id()?);
    let names = names(app);
    blocking(move || {
        let _io = IO.lock().unwrap();
        let mut s = load(&d)?;
        let c = add_in(&mut s, &key, &by, &name, text, id, now_secs())?;
        save(&d, &s)?;
        Ok(view(&c, Some(by.as_str()), &names))
    })
    .await
}

pub async fn edit(app: &AppHandle, key: &str, id: &str, text: &str) -> Result<View, String> {
    let text = clean(text)?;
    let (by, _) = author(app).ok_or("Pick a profile first.")?;
    rate().lock().unwrap().take(&by, Instant::now()).map_err(slow_down)?;
    let (d, key, id) = (dir(app)?, key.to_string(), id.to_string());
    let names = names(app);
    blocking(move || {
        let _io = IO.lock().unwrap();
        let mut s = load(&d)?;
        let c = own_mut(&mut s, &key, &id, &by)?;
        if c.text != text {
            c.text = text;
            c.edited = Some(now_secs());
        }
        let c = c.clone();
        save(&d, &s)?;
        Ok(view(&c, Some(by.as_str()), &names))
    })
    .await
}

pub async fn delete(app: &AppHandle, key: &str, id: &str) -> Result<(), String> {
    let (by, _) = author(app).ok_or("Pick a profile first.")?;
    let (d, key, id) = (dir(app)?, key.to_string(), id.to_string());
    blocking(move || {
        let _io = IO.lock().unwrap();
        let mut s = load(&d)?;
        delete_in(&mut s, &key, &id, &by)?;
        save(&d, &s)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_title_keys_are_taken() {
        for k in ["tmdb:603", "tvdb:81189", "anilist:21", "jf:4f1c2a9be0d34e7b", "book:abc-1"] {
            assert!(key_ok(k), "{k}");
        }
        for k in ["", "tmdb", "tmdb:", ":1", "TMDB:1", "tmdb:1/../x", "tmdb:1 2", "a:b:c", &format!("tmdb:{}", "9".repeat(80))] {
            assert!(!key_ok(k), "{k}");
        }
    }

    #[test]
    fn text_is_trimmed_and_limited() {
        assert_eq!(clean("  hi there \n").unwrap(), "hi there");
        assert!(clean("   ").is_err(), "nothing to post");
        assert!(clean(&"é".repeat(MAX_LEN)).is_ok(), "counted in characters, not bytes");
        assert!(clean(&"a".repeat(MAX_LEN + 1)).is_err());
    }

    #[test]
    fn only_the_writer_edits_or_deletes() {
        let mut s = Store::default();
        let c = add_in(&mut s, "tmdb:1", "ann", "Ann", "first".into(), "c1".into(), 10).unwrap();
        add_in(&mut s, "tmdb:1", "bob", "Bob", "second".into(), "c2".into(), 20).unwrap();
        assert_eq!(c.by, "ann");
        assert!(own_mut(&mut s, "tmdb:1", "c1", "bob").is_err(), "Bob can't touch Ann's");
        assert!(delete_in(&mut s, "tmdb:1", "c1", "bob").is_err());
        own_mut(&mut s, "tmdb:1", "c1", "ann").unwrap().text = "edited".into();
        assert!(delete_in(&mut s, "tmdb:1", "nope", "ann").is_err(), "a gone comment");
        delete_in(&mut s, "tmdb:1", "c2", "bob").unwrap();
        let names = HashMap::new();
        let l = list_in(&s, "tmdb:1", Some("ann"), &names);
        assert_eq!(l.len(), 1);
        assert_eq!((l[0].text.as_str(), l[0].mine), ("edited", true));
        delete_in(&mut s, "tmdb:1", "c1", "ann").unwrap();
        assert!(s.titles.is_empty(), "an empty title goes from the file");
    }

    #[test]
    fn newest_first_with_the_current_profile_name() {
        let mut s = Store::default();
        add_in(&mut s, "tvdb:5", "ann", "Ann", "old".into(), "a".into(), 10).unwrap();
        add_in(&mut s, "tvdb:5", "bob", "Bob", "new".into(), "b".into(), 20).unwrap();
        let names: HashMap<String, String> = [("ann".to_string(), "Annie".to_string())].into();
        let l = list_in(&s, "tvdb:5", Some("bob"), &names);
        assert_eq!(l.iter().map(|v| v.text.as_str()).collect::<Vec<_>>(), ["new", "old"]);
        assert_eq!((l[1].name.as_str(), l[0].name.as_str()), ("Annie", "Bob"), "renamed profile, then the name kept");
        assert!(l[0].mine && !l[1].mine);
        assert!(list_in(&s, "tvdb:6", None, &names).is_empty());
    }

    #[test]
    fn a_title_has_room_for_so_many() {
        let mut s = Store::default();
        for i in 0..MAX_PER_TITLE {
            add_in(&mut s, "tmdb:1", "a", "A", "x".into(), i.to_string(), 0).unwrap();
        }
        assert!(add_in(&mut s, "tmdb:1", "a", "A", "x".into(), "last".into(), 0).is_err());
    }

    #[test]
    fn posting_is_rate_limited_per_person() {
        let mut r = Rate::default();
        let t0 = Instant::now();
        for _ in 0..RATE_MAX {
            r.take("ann", t0).unwrap();
        }
        let wait = r.take("ann", t0 + Duration::from_secs(10)).unwrap_err();
        assert_eq!(wait, 50, "until the oldest leaves the minute");
        assert!(r.take("bob", t0).is_ok(), "someone else isn't held up");
        assert!(r.take("ann", t0 + RATE_WINDOW).is_ok(), "a minute later");
    }

    #[test]
    fn saved_and_read_back() {
        let dir = std::env::temp_dir().join(format!("chillarr-comments-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Store::default();
        add_in(&mut s, "anilist:21", "ann", "Ann", "<b>hi</b>".into(), "a".into(), 1).unwrap();
        save(&dir, &s).unwrap();
        let back = load(&dir).unwrap();
        assert_eq!(back.titles["anilist:21"][0].text, "<b>hi</b>", "kept as typed; the page shows it as text");
        assert!(load(&dir.join("none")).unwrap().titles.is_empty());
        // A damaged file is set aside, never written over.
        std::fs::write(dir.join("comments.json"), b"{not json").unwrap();
        assert!(load(&dir).is_err());
        assert!(dir.join("comments.json.bad").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
