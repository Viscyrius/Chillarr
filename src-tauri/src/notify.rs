//! Notify me (issue #18): a note when a movie, new episodes or a new season of a
//! series or anime, a new light-novel volume or a new book by an author comes
//! out. Each profile keeps its own list. Any title can be on it, out yet or not,
//! and each can also be got by itself the moment it's out ("Get it too").
//!
//! Where the dates come from: the same places the detail panel reads (Radarr's
//! movie data for a film's digital or disc release, Sonarr's SkyHook for a
//! show's episodes, AniList for anime and light novels, Open Library for an
//! author's books). Each title is asked about once an hour, and a title on
//! several profiles is asked once.
//!
//! Where the note goes: the PC's notification when that profile is the one in
//! Chillarr's window, and every open page of that profile in Jellyfin (phones,
//! TV) as a pop-up. Every note is also kept (the last 50 per profile), so a
//! screen that was off shows what it missed the next time it opens.
//! New manga chapters notify through keepup.rs, as before.

use crate::media::http;
use crate::tiles::Tile;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

const TICK: Duration = Duration::from_secs(5 * 60);
const CHECK_EVERY: Duration = Duration::from_secs(60 * 60);
/// Between two titles' asks in one round.
const GAP: Duration = Duration::from_secs(2);
const KEEP_NOTES: usize = 50;
const MAX_WATCHES: usize = 500;

/// What a title is watched by: its kind and the id its dates are found by.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", content = "id", rename_all = "lowercase")]
pub enum Source {
    /// TMDB id.
    Movie(i64),
    /// TVDB id.
    Show(i64),
    /// AniList id, a show or film.
    Anime(i64),
    /// AniList id of a light novel.
    Novel(i64),
    /// An author's name, as Open Library spells it.
    Author(String),
}

impl Source {
    pub fn key(&self) -> String {
        match self {
            Source::Movie(i) => format!("movie:{i}"),
            Source::Show(i) => format!("show:{i}"),
            Source::Anime(i) => format!("anime:{i}"),
            Source::Novel(i) => format!("novel:{i}"),
            Source::Author(n) => format!("author:{}", n.trim().to_lowercase()),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Watch {
    pub source: Source,
    pub title: String,
    pub poster: Option<String>,
    /// Get it too, the moment it's out.
    #[serde(default)]
    pub auto: bool,
    /// The title as the page had it, for getting it.
    #[serde(default)]
    pub tile: Option<Tile>,
    /// How far it had got when last looked at: a film 0 or 1, a show its newest
    /// aired episode (season * 10000 + episode), anime its aired episodes, a
    /// light novel its volumes. None until the first look, which says nothing.
    #[serde(default)]
    pub seen: Option<i64>,
    /// An author's books already known (Open Library work keys).
    #[serde(default)]
    pub works: Vec<String>,
    pub added: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Note {
    pub at: i64,
    pub text: String,
    /// The watched title's key (Source::key).
    pub key: String,
    pub poster: Option<String>,
    #[serde(default)]
    pub seen: bool,
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct Mine {
    watches: Vec<Watch>,
    notes: Vec<Note>,
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct Saved {
    /// Jellyfin user id -> that profile's list and notes.
    profiles: HashMap<String, Mine>,
}

static IO: Mutex<()> = Mutex::new(());
/// Per title: when it may be asked about next.
static DUE: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);

fn file(app: &AppHandle) -> Option<PathBuf> {
    Some(crate::media::data_dir(app)?.join("notify.json"))
}

fn load(app: &AppHandle) -> Saved {
    file(app).and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save(app: &AppHandle, s: &Saved) -> Result<(), String> {
    let p = file(app).ok_or("Chillarr isn't ready yet.")?;
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string(s).map_err(|e| e.to_string())?).and_then(|_| std::fs::rename(&tmp, &p)).map_err(|e| format!("Couldn't save it: {e}"))
}

/// Changes the saved list under the lock.
fn edit<R>(app: &AppHandle, f: impl FnOnce(&mut Saved) -> R) -> Result<R, String> {
    let _g = IO.lock().unwrap_or_else(|e| e.into_inner());
    let mut s = load(app);
    let r = f(&mut s);
    save(app, &s)?;
    Ok(r)
}

fn uid(app: &AppHandle) -> Result<String, String> {
    crate::media::active_user(app).ok_or_else(|| "Sign in to Jellyfin first.".to_string())
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn wake(key: &str) {
    let mut g = DUE.lock().unwrap_or_else(|e| e.into_inner());
    g.get_or_insert_with(HashMap::new).insert(key.to_string(), Instant::now());
}

// ---------- what the page asks ----------

/// What the page sends: a film or show's tile, or a book's card.
#[derive(Deserialize, Clone, Debug, Default)]
pub struct Target {
    pub tile: Option<Tile>,
    pub book: Option<BookCard>,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct BookCard {
    pub title: String,
    pub author: Option<String>,
    pub cover: Option<String>,
    pub anilist: Option<i64>,
}

/// The watch a page's title would be, or why it can't be one.
pub fn watch_of(t: &Target) -> Result<Watch, String> {
    let base = |source, title: &str, poster: Option<String>, tile: Option<Tile>| Watch { source, title: title.to_string(), poster, auto: false, tile, seen: None, works: Vec::new(), added: now() };
    if let Some(tile) = &t.tile {
        let anilist = tile.key.strip_prefix("anilist:").and_then(|s| s.parse::<i64>().ok());
        let movie = tile.kind == "movie" || tile.format.as_deref() == Some("MOVIE");
        let source = match (anilist, movie, tile.tmdb, tile.tvdb) {
            (Some(id), ..) => Source::Anime(id),
            (None, true, Some(id), _) => Source::Movie(id),
            (None, false, _, Some(id)) => Source::Show(id),
            _ => return Err("Chillarr can't tell when this one comes out.".into()),
        };
        // An episode's card watches its show.
        let title = if tile.kind == "episode" { tile.title.split(" · ").next().unwrap_or(&tile.title) } else { &tile.title };
        return Ok(base(source, title, tile.poster.clone(), Some(tile.clone())));
    }
    if let Some(b) = &t.book {
        if let Some(id) = b.anilist {
            return Ok(base(Source::Novel(id), &b.title, b.cover.clone(), None));
        }
        if let Some(a) = b.author.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
            return Ok(base(Source::Author(a.to_string()), a, b.cover.clone(), None));
        }
    }
    Err("Chillarr can't tell when this one comes out.".into())
}

#[derive(Serialize)]
pub struct State {
    pub on: bool,
    pub auto: bool,
    /// Whether Get it too can be offered for it.
    pub can_get: bool,
}

fn can_get(s: &Source) -> bool {
    !matches!(s, Source::Novel(_))
}

pub fn state(app: &AppHandle, t: &Target) -> Result<State, String> {
    let w = watch_of(t)?;
    let me = uid(app)?;
    let _g = IO.lock().unwrap_or_else(|e| e.into_inner());
    let s = load(app);
    let found = s.profiles.get(&me).and_then(|m| m.watches.iter().find(|x| x.source == w.source));
    Ok(State { on: found.is_some(), auto: found.is_some_and(|x| x.auto), can_get: can_get(&w.source) })
}

/// Notify me on or off for the profile watching, and Get it too.
pub fn set(app: &AppHandle, t: &Target, on: bool, auto: bool) -> Result<String, String> {
    let mut w = watch_of(t)?;
    w.auto = auto && can_get(&w.source);
    let me = uid(app)?;
    let key = w.source.key();
    let what = match &w.source {
        Source::Movie(_) => format!("when {} is out to watch", w.title),
        Source::Show(_) | Source::Anime(_) => format!("when new episodes of {} come out", w.title),
        Source::Novel(_) => format!("when a new volume of {} comes out", w.title),
        Source::Author(_) => format!("when {} has a new book out", w.title),
    };
    let msg = edit(app, |s| {
        let mine = s.profiles.entry(me).or_default();
        let at = mine.watches.iter().position(|x| x.source == w.source);
        match (on, at) {
            (false, Some(i)) => {
                let t = mine.watches.remove(i).title;
                Ok(format!("You won't be told about {t} any more."))
            }
            (false, None) => Ok("That wasn't on your list.".to_string()),
            (true, Some(i)) => {
                // Already on: only Get it too changes; what it has seen stays.
                mine.watches[i].auto = w.auto;
                Ok(if w.auto { "It'll be got for you too.".to_string() } else { format!("You'll be told {what}.") })
            }
            (true, None) if mine.watches.len() >= MAX_WATCHES => Err("Your Notify me list is full. Take something off it first.".to_string()),
            (true, None) => {
                mine.watches.push(w.clone());
                Ok(format!("You'll get a notification {what}.{}", if w.auto { " It'll be got for you too." } else { "" }))
            }
        }
    })??;
    if on {
        wake(&key);
    }
    Ok(msg)
}

/// The profile's notes not shown yet, oldest first. They count as shown now.
pub fn unseen(app: &AppHandle) -> Result<Vec<Note>, String> {
    let me = uid(app)?;
    edit(app, |s| {
        let Some(mine) = s.profiles.get_mut(&me) else { return Vec::new() };
        let out: Vec<Note> = mine.notes.iter().filter(|n| !n.seen).cloned().collect();
        for n in mine.notes.iter_mut() {
            n.seen = true;
        }
        out
    })
}

/// The profile's list, for Settings.
pub fn list(app: &AppHandle) -> Result<Vec<Watch>, String> {
    let me = uid(app)?;
    let _g = IO.lock().unwrap_or_else(|e| e.into_inner());
    Ok(load(app).profiles.remove(&me).map(|m| m.watches).unwrap_or_default())
}

// ---------- looking ----------

/// Where a title has got, as asked of its source.
#[derive(Clone, Debug, PartialEq)]
pub enum Now {
    /// A film: out to watch at home or not.
    Film(bool),
    /// A show's aired episodes (season, episode), specials left out.
    Episodes(Vec<(i64, i64)>),
    /// Anime episodes aired so far.
    Aired(i64),
    Volumes(i64),
    /// An author's recent books: (work key, title, cover).
    Books(Vec<(String, String, Option<String>)>),
}

fn past(v: &Value, now: chrono::DateTime<chrono::Utc>) -> bool {
    let Some(s) = v.as_str() else { return false };
    if let Ok(d) = chrono::DateTime::parse_from_rfc3339(s) {
        return d <= now;
    }
    // A date with no time: out from the start of that day.
    s.get(..10).and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()).and_then(|d| d.and_hms_opt(0, 0, 0)).is_some_and(|d| d.and_utc() <= now)
}

/// A film counts as out once it can be watched at home (digital or disc).
pub fn film_out(m: &Value, now: chrono::DateTime<chrono::Utc>) -> bool {
    past(&m["DigitalRelease"], now) || past(&m["PhysicalRelease"], now)
}

pub fn aired_episodes(s: &Value, now: chrono::DateTime<chrono::Utc>) -> Vec<(i64, i64)> {
    let mut v: Vec<(i64, i64)> = s["episodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| e["seasonNumber"].as_i64().unwrap_or(0) > 0 && past(&e["airDateUtc"], now))
        .filter_map(|e| Some((e["seasonNumber"].as_i64()?, e["episodeNumber"].as_i64()?)))
        .collect();
    v.sort();
    v
}

/// Anime aired so far: one less than the next to air, or all of a finished one.
pub fn anime_aired(m: &Value) -> i64 {
    if m["format"] == "MOVIE" {
        return i64::from(m["status"] == "FINISHED");
    }
    match m["nextAiringEpisode"]["episode"].as_i64() {
        Some(n) => n - 1,
        None if m["status"] == "FINISHED" => m["episodes"].as_i64().unwrap_or(1),
        None => 0,
    }
}

async fn look(source: &Source) -> Option<Now> {
    let t = chrono::Utc::now();
    match source {
        Source::Movie(id) => {
            let m: Value = http().get(format!("{}/{id}", crate::details::RADARR_META)).send().await.ok()?.json().await.ok()?;
            Some(Now::Film(film_out(&m, t)))
        }
        Source::Show(id) => {
            let s: Value = http().get(format!("{}/{id}", crate::details::SKYHOOK)).send().await.ok()?.json().await.ok()?;
            s["episodes"].is_array().then(|| Now::Episodes(aired_episodes(&s, t)))
        }
        Source::Anime(id) => {
            let q = format!("{{ Media(id: {id}) {{ format status episodes nextAiringEpisode {{ episode }} }} }}");
            let v = crate::media::anilist(&json!({ "query": q })).await.ok()?;
            let m = &v["data"]["Media"];
            (!m.is_null()).then(|| Now::Aired(anime_aired(m)))
        }
        Source::Novel(id) => {
            let q = format!("{{ Media(id: {id}) {{ volumes }} }}");
            let v = crate::media::anilist(&json!({ "query": q })).await.ok()?;
            let m = &v["data"]["Media"];
            (!m.is_null()).then(|| Now::Volumes(m["volumes"].as_i64().unwrap_or(0)))
        }
        Source::Author(name) => {
            let url = format!("https://openlibrary.org/search.json?author={}&sort=new&fields=key,title,first_publish_year,cover_i&limit=20", crate::books::enc(name));
            let v = crate::books::ol_get(&url).await.ok()?;
            let docs = v["docs"].as_array()?;
            Some(Now::Books(recent_books(docs, chrono::Datelike::year(&t) as i64)))
        }
    }
}

/// An author's books from this year or last: older ones newly listed aren't news.
pub fn recent_books(docs: &[Value], year: i64) -> Vec<(String, String, Option<String>)> {
    docs.iter()
        .filter(|d| d["first_publish_year"].as_i64().is_some_and(|y| y >= year - 1))
        .filter_map(|d| Some((d["key"].as_str()?.to_string(), d["title"].as_str()?.to_string(), d["cover_i"].as_i64().map(|c| format!("https://covers.openlibrary.org/b/id/{c}-L.jpg")))))
        .collect()
}

/// What's new since the watch last looked.
#[derive(Clone, Debug, PartialEq)]
pub enum News {
    Film,
    Episodes(Vec<(i64, i64)>),
    Anime(i64),
    Volume(i64),
    Books(Vec<(String, String, Option<String>)>),
}

/// Compares a look with what was seen: the news (if any) and what to keep.
/// The first look only learns where it stands.
pub fn judge(w: &Watch, now: &Now) -> (Option<News>, Option<i64>, Vec<String>) {
    let first = w.seen.is_none();
    let seen = w.seen.unwrap_or(0);
    match now {
        Now::Film(out) => {
            let n = i64::from(*out);
            ((!first && n > seen).then_some(News::Film), Some(n.max(seen)), Vec::new())
        }
        Now::Episodes(eps) => {
            let code = |(s, e): &(i64, i64)| s * 10000 + e;
            let top = eps.iter().map(code).max().unwrap_or(0);
            let fresh: Vec<(i64, i64)> = eps.iter().filter(|x| code(x) > seen).copied().collect();
            ((!first && !fresh.is_empty()).then_some(News::Episodes(fresh)), Some(top.max(seen)), Vec::new())
        }
        Now::Aired(n) => ((!first && *n > seen).then_some(News::Anime(*n)), Some((*n).max(seen)), Vec::new()),
        Now::Volumes(n) => ((!first && *n > seen).then_some(News::Volume(*n)), Some((*n).max(seen)), Vec::new()),
        Now::Books(list) => {
            let fresh: Vec<_> = list.iter().filter(|(k, ..)| !w.works.contains(k)).cloned().collect();
            let mut works = w.works.clone();
            works.extend(fresh.iter().map(|(k, ..)| k.clone()));
            // Kept short: the newest few hundred are enough to know what's new.
            let cut = works.len().saturating_sub(300);
            works.drain(..cut);
            ((!first && !fresh.is_empty()).then_some(News::Books(fresh)), Some(0), works)
        }
    }
}

/// What the note says.
pub fn text_of(title: &str, news: &News) -> String {
    match news {
        News::Film => format!("{title} is out. You can watch it now."),
        News::Episodes(eps) => {
            if let Some((s, _)) = eps.iter().rev().find(|(_, e)| *e == 1) {
                format!("Season {s} of {title} is out.")
            } else if let [(s, e)] = eps.as_slice() {
                format!("{title}: season {s} episode {e} is out.")
            } else {
                format!("{title}: {} new episodes are out.", eps.len())
            }
        }
        News::Anime(n) => format!("{title}: episode {n} is out."),
        News::Volume(n) => format!("{title}: volume {n} is out."),
        News::Books(books) => match books.as_slice() {
            [(_, b, _)] => format!("New book by {title}: {b}."),
            _ => format!("{title} has {} new books out.", books.len()),
        },
    }
}

/// Gets what came out, for a watch with Get it too on.
async fn get_it(app: &AppHandle, w: &Watch, news: &News) -> Result<String, String> {
    match (news, &w.tile) {
        (News::Episodes(eps), Some(t)) => crate::pick::get_picked(app, t.clone(), "best", crate::pick::Pick { seasons: Vec::new(), episodes: eps.clone() }).await,
        (News::Film | News::Anime(_), Some(t)) => crate::media::get_it(app, t.clone(), "best").await,
        (News::Books(books), _) => {
            let mut said = Vec::new();
            for (key, title, cover) in books.iter().take(3) {
                let meta = crate::books::BookMeta { title: title.clone(), author: Some(w.title.clone()), cover: cover.clone(), description: None, olid: Some(key.clone()), anilist: None, comic: false };
                let found = crate::books::copies(app, &meta).await?;
                let Some(best) = found.first() else { continue };
                said.push(crate::books::get(app, meta, &best.id).await?);
            }
            if said.is_empty() { Err("No copy was found yet.".into()) } else { Ok(said.join(" ")) }
        }
        _ => Err("Nothing to get.".into()),
    }
}

/// Tells one profile: kept, sent to its open pages, and shown on the PC when
/// that profile is the one in the window.
fn tell(app: &AppHandle, uid: &str, w: &Watch, text: String) {
    crate::log::detail(text.clone());
    let window = crate::media::active_user(app);
    let shown_here = window.as_deref() == Some(uid);
    let note = Note { at: now(), text: text.clone(), key: w.source.key(), poster: w.poster.clone(), seen: shown_here };
    let _ = edit(app, |s| {
        let mine = s.profiles.entry(uid.to_string()).or_default();
        mine.notes.push(note.clone());
        let cut = mine.notes.len().saturating_sub(KEEP_NOTES);
        mine.notes.drain(..cut);
    });
    crate::bridge::emit_to_user(uid, "release-note", &note);
    if shown_here {
        use tauri_plugin_notification::NotificationExt;
        let _ = app.notification().builder().title("Chillarr").body(&text).show();
        let _ = app.emit("release-note-seen", &note);
    }
}

pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(120)).await;
        loop {
            round(&app).await;
            tokio::time::sleep(TICK).await;
        }
    });
}

async fn round(app: &AppHandle) {
    let all = {
        let _g = IO.lock().unwrap_or_else(|e| e.into_inner());
        load(app)
    };
    // Every title once, whoever watches it.
    let mut sources: Vec<Source> = Vec::new();
    for m in all.profiles.values() {
        for w in &m.watches {
            if !sources.contains(&w.source) {
                sources.push(w.source.clone());
            }
        }
    }
    let mut asked = false;
    for source in sources {
        let key = source.key();
        let due = DUE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).get(&key).is_none_or(|at| Instant::now() >= *at);
        if !due {
            continue;
        }
        if asked {
            tokio::time::sleep(GAP).await;
        }
        asked = true;
        let looked = look(&source).await;
        DUE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(key, Instant::now() + CHECK_EVERY);
        let Some(now) = looked else { continue };
        // Judged against each profile's own list, as saved now (not the copy above).
        let mut told: Vec<(String, Watch, News)> = Vec::new();
        let _ = edit(app, |s| {
            for (uid, m) in s.profiles.iter_mut() {
                for w in m.watches.iter_mut().filter(|w| w.source == source) {
                    let (news, seen, works) = judge(w, &now);
                    w.seen = seen;
                    w.works = works;
                    if let Some(n) = news {
                        told.push((uid.clone(), w.clone(), n));
                    }
                }
            }
        });
        let mut got = false;
        for (uid, w, news) in told {
            let mut text = text_of(&w.title, &news);
            // Got once, however many profiles asked for it.
            if w.auto && !got {
                got = true;
                match get_it(app, &w, &news).await {
                    Ok(_) => text.push_str(" It's being downloaded."),
                    Err(e) => crate::log::detail(format!("Notify me couldn't get {}: {e}", w.title)),
                }
            }
            tell(app, &uid, &w, text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn watch(source: Source, seen: Option<i64>) -> Watch {
        Watch { source, title: "Dune".into(), poster: None, auto: false, tile: None, seen, works: Vec::new(), added: 0 }
    }

    fn at(s: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&chrono::Utc)
    }

    #[test]
    fn a_film_is_out_once_it_can_be_watched_at_home() {
        let m = json!({ "InCinema": "2026-03-01T00:00:00Z", "DigitalRelease": "2026-05-10T00:00:00Z" });
        assert!(!film_out(&m, at("2026-04-01T00:00:00Z")), "in cinemas only");
        assert!(film_out(&m, at("2026-05-10T12:00:00Z")));
        assert!(film_out(&json!({ "PhysicalRelease": "2026-05-10" }), at("2026-05-11T00:00:00Z")), "a disc date with no time");
        assert!(!film_out(&json!({}), at("2026-05-11T00:00:00Z")));
    }

    #[test]
    fn the_first_look_is_quiet_then_news_is_told_once() {
        let mut w = watch(Source::Movie(1), None);
        let (n, seen, _) = judge(&w, &Now::Film(false));
        assert_eq!((n, seen), (None, Some(0)));
        w.seen = seen;
        let (n, seen, _) = judge(&w, &Now::Film(true));
        assert_eq!((n, seen), (Some(News::Film), Some(1)));
        w.seen = seen;
        assert_eq!(judge(&w, &Now::Film(true)).0, None, "told once");
        // Already out when added: nothing to say.
        assert_eq!(judge(&watch(Source::Movie(1), None), &Now::Film(true)).0, None);
    }

    #[test]
    fn episodes_and_seasons() {
        let s = json!({ "episodes": [
            { "seasonNumber": 0, "episodeNumber": 1, "airDateUtc": "2020-01-01T00:00:00Z" },
            { "seasonNumber": 1, "episodeNumber": 1, "airDateUtc": "2020-01-01T00:00:00Z" },
            { "seasonNumber": 1, "episodeNumber": 2, "airDateUtc": "2020-01-08T00:00:00Z" },
            { "seasonNumber": 2, "episodeNumber": 1, "airDateUtc": "2026-11-01T00:00:00Z" },
            { "seasonNumber": 2, "episodeNumber": 2, "airDateUtc": "2026-11-08T00:00:00Z" },
            { "seasonNumber": 2, "episodeNumber": 3, "airDateUtc": "2026-11-15T00:00:00Z" },
        ] });
        let before = aired_episodes(&s, at("2026-10-01T00:00:00Z"));
        assert_eq!(before, vec![(1, 1), (1, 2)], "specials and future episodes left out");
        let mut w = watch(Source::Show(1), None);
        w.seen = judge(&w, &Now::Episodes(before)).1;
        assert_eq!(w.seen, Some(10002));
        let (n, seen, _) = judge(&w, &Now::Episodes(aired_episodes(&s, at("2026-11-02T00:00:00Z"))));
        assert_eq!(n, Some(News::Episodes(vec![(2, 1)])));
        assert_eq!(text_of("Dune", &n.unwrap()), "Season 2 of Dune is out.");
        w.seen = seen;
        let (n, ..) = judge(&w, &Now::Episodes(aired_episodes(&s, at("2026-11-09T00:00:00Z"))));
        assert_eq!(text_of("Dune", &n.unwrap()), "Dune: season 2 episode 2 is out.");
        assert_eq!(text_of("Dune", &News::Episodes(vec![(2, 2), (2, 3)])), "Dune: 2 new episodes are out.");
    }

    #[test]
    fn anime_counts_what_has_aired() {
        assert_eq!(anime_aired(&json!({ "format": "TV", "status": "RELEASING", "nextAiringEpisode": { "episode": 6 } })), 5);
        assert_eq!(anime_aired(&json!({ "format": "TV", "status": "FINISHED", "episodes": 12 })), 12);
        assert_eq!(anime_aired(&json!({ "format": "TV", "status": "NOT_YET_RELEASED" })), 0);
        assert_eq!(anime_aired(&json!({ "format": "MOVIE", "status": "FINISHED" })), 1);
        let w = watch(Source::Anime(1), Some(5));
        assert_eq!(judge(&w, &Now::Aired(6)).0, Some(News::Anime(6)));
        assert_eq!(judge(&w, &Now::Aired(5)).0, None);
    }

    #[test]
    fn an_authors_new_books() {
        let docs = vec![
            json!({ "key": "/works/OL1W", "title": "Old", "first_publish_year": 2001 }),
            json!({ "key": "/works/OL2W", "title": "Fresh", "first_publish_year": 2026, "cover_i": 7 }),
        ];
        let books = recent_books(&docs, 2026);
        assert_eq!(books, vec![("/works/OL2W".to_string(), "Fresh".to_string(), Some("https://covers.openlibrary.org/b/id/7-L.jpg".to_string()))]);
        let mut w = watch(Source::Author("Andy Weir".into()), None);
        let (n, seen, works) = judge(&w, &Now::Books(Vec::new()));
        assert_eq!(n, None);
        (w.seen, w.works) = (seen, works);
        let (n, _, works) = judge(&w, &Now::Books(books.clone()));
        assert_eq!(text_of("Andy Weir", &n.unwrap()), "New book by Andy Weir: Fresh.");
        w.works = works;
        assert_eq!(judge(&w, &Now::Books(books)).0, None, "told once");
    }

    #[test]
    fn page_titles_become_watches() {
        let tile = |key: &str, kind: &str, tmdb: Option<i64>, tvdb: Option<i64>| Target { tile: Some(Tile { key: key.into(), kind: kind.into(), title: "X".into(), tmdb, tvdb, ..Default::default() }), book: None };
        assert_eq!(watch_of(&tile("tmdb:5", "movie", Some(5), None)).unwrap().source, Source::Movie(5));
        assert_eq!(watch_of(&tile("jf:a", "series", None, Some(9))).unwrap().source, Source::Show(9));
        assert_eq!(watch_of(&tile("anilist:3", "anime", None, Some(9))).unwrap().source, Source::Anime(3));
        assert!(watch_of(&tile("jf:a", "movie", None, None)).is_err());
        let book = |anilist: Option<i64>, author: Option<&str>| Target { tile: None, book: Some(BookCard { title: "B".into(), author: author.map(String::from), cover: None, anilist }) };
        assert_eq!(watch_of(&book(Some(4), Some("A"))).unwrap().source, Source::Novel(4));
        assert_eq!(watch_of(&book(None, Some(" Andy Weir "))).unwrap().source.key(), "author:andy weir");
        assert!(watch_of(&book(None, None)).is_err());
    }
}
