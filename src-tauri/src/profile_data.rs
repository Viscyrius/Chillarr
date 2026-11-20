//! A profile's own data out to a file and back in (Settings > Account > Your
//! data): My List, what it watched and where it stopped, Completed, Where
//! you're at, Notify me, and its reading (the list, the series it follows,
//! where it is in each and what it finished). For a new PC, a reinstall, or
//! moving to another profile.
//!
//! The file names titles the way any Chillarr can find them again: films and
//! shows by TMDB, TVDB and IMDb ids (Jellyfin's own ids are this PC's only),
//! episodes by their season and number, series to read by AniList id or the
//! site's own name for them, and chapters by number. Importing only ever adds:
//! nothing here is unmarked, taken off or moved back, so importing twice
//! changes nothing the second time.
//!
//! Also in it: the songs the profile downloaded in Music (downloaded again on
//! import), the profile's own settings on the device it exported from (look,
//! player, reader: `prefs`), and the Hidden list, but only when it was
//! unlocked at export, so a file never shows it to someone without the password.
//!
//! Left out on purpose: the PIN and passwords, and anything about this PC's
//! files (the clean-up acts on those).
//!
//! An imported file is someone else's writing: every field is checked and cut
//! to size before it is kept (`clean_*`), and a kids' profile takes nothing
//! that is 18+.

use crate::history::{MovieWatch, SeriesWatch};
use crate::marker::Mark;
use crate::notify::Watch;
use crate::read::Spot;
use crate::tiles::Tile;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use tauri::{AppHandle, Manager};

pub const FORMAT: &str = "chillarr-profile";
pub const VERSION: u32 = 1;
/// The most of any one kind an import reads. Far above a real profile's.
const MAX_ITEMS: usize = 20_000;

/// A film or show, as any Chillarr can find it again.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Title {
    /// "movie" or "series".
    pub kind: String,
    pub title: String,
    pub year: Option<i64>,
    pub tmdb: Option<i64>,
    pub tvdb: Option<i64>,
    pub imdb: Option<String>,
    /// When it was last watched (unix seconds), for watched films.
    pub at: Option<i64>,
}

/// A watched episode: its show, and its place in Jellyfin's numbering.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Episode {
    pub series: String,
    pub tmdb: Option<i64>,
    pub tvdb: Option<i64>,
    pub imdb: Option<String>,
    pub season: i64,
    pub episode: i64,
    pub at: Option<i64>,
}

/// Half watched: a film, or an episode of a show.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Resume {
    /// "movie" or "episode".
    pub kind: String,
    pub title: String,
    pub tmdb: Option<i64>,
    pub tvdb: Option<i64>,
    pub imdb: Option<String>,
    pub season: Option<i64>,
    pub episode: Option<i64>,
    pub seconds: f64,
}

/// A chapter as another PC can find it: by number, else by name. The id is
/// this PC's, used only when it still means the same chapter here.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct ChapterRef {
    pub id: i64,
    pub number: Option<f64>,
    pub name: String,
}

/// Where the profile is in a series, with chapters as `ChapterRef`.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Place {
    pub chapter: ChapterRef,
    pub page: usize,
    pub pages: usize,
    pub at: i64,
    /// Chapters finished, with when.
    pub read: Vec<(ChapterRef, i64)>,
    pub mode: Option<String>,
    pub hide_week: bool,
}

/// A manga, manhwa or manhua this profile follows or has read.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct SeriesRead {
    pub title: String,
    /// Where you are, for a person reading the file: "Chapter 96, page 3 of
    /// 15", or "Not started". The import goes by `place`, not by this.
    pub reading_now: String,
    /// The chapter you're on, and the furthest one you finished, by number.
    pub chapter_now: Option<f64>,
    pub last_read: Option<f64>,
    /// Chapters downloaded from this number on (None: all), and whether new
    /// ones download by themselves: how the series is kept up with.
    pub from: Option<f64>,
    pub new_too: bool,
    pub anilist: Option<i64>,
    /// The site's name and the series' name there, as the downloader files them.
    pub source: String,
    pub source_title: String,
    pub cover: Option<String>,
    pub genres: Vec<String>,
    /// AniList's country (JP, KR, CN) and format (MANGA, NOVEL): a card on the
    /// reading list is drawn as a manga, manhwa, manhua or light novel by them.
    pub country: Option<String>,
    pub format: Option<String>,
    pub following: bool,
    pub place: Option<Place>,
}

/// A book or comic this profile has, and where it is in it (parts of the book
/// and how far into each, which mean the same in the same book anywhere).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct BookRead {
    pub title: String,
    pub author: Option<String>,
    pub olid: Option<String>,
    pub anilist: Option<i64>,
    pub cover: Option<String>,
    pub comic: bool,
    pub following: bool,
    pub spot: Option<Spot>,
}

/// The file.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Export {
    pub format: String,
    pub version: u32,
    pub exported_at: i64,
    pub profile: String,
    pub app: String,
    /// My List: titles in the library (Jellyfin favourites)...
    pub my_list: Vec<Title>,
    /// ...and titles not in it yet.
    pub my_list_saved: Vec<Tile>,
    pub watched: Vec<Title>,
    pub watched_episodes: Vec<Episode>,
    pub resume: Vec<Resume>,
    /// The watch log Completed is made from (history.rs), without this PC's files.
    pub history_movies: Vec<MovieWatch>,
    pub history_series: Vec<SeriesWatch>,
    pub completed_hidden: BTreeMap<String, Option<i64>>,
    /// Where you're at, by TVDB id.
    pub markers: HashMap<i64, Mark>,
    pub notify: Vec<Watch>,
    pub reading_list: Vec<Value>,
    pub reading: Vec<SeriesRead>,
    pub books: Vec<BookRead>,
    /// Songs downloaded in Music.
    pub music: Vec<crate::music::Song>,
    /// Liked songs, and what was played lately (newest first).
    pub music_liked: Vec<crate::music::Song>,
    pub music_recent: Vec<crate::music::Song>,
    /// The Hidden list, when it was unlocked at export (None: left out).
    pub hidden: Option<Vec<crate::hidden::Portable>>,
    /// The profile's settings on the device it was exported from (the page's
    /// own; the page checks them again on import).
    pub prefs: BTreeMap<String, String>,
}

/// Settings from the page: short names and values only.
pub fn clean_prefs(p: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    p.iter()
        .filter(|(k, v)| !k.is_empty() && k.len() <= 64 && k.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')) && v.len() <= 4096)
        .take(300)
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

// ---------- reading Jellyfin's answers ----------

fn pid_str(v: &Value, name: &str) -> Option<String> {
    let s = v["ProviderIds"].as_object()?.iter().find(|(k, _)| k.eq_ignore_ascii_case(name))?.1.as_str()?.trim();
    (!s.is_empty()).then(|| s.to_string())
}

fn pid_num(v: &Value, name: &str) -> Option<i64> {
    pid_str(v, name)?.parse().ok().filter(|n| *n > 0)
}

/// Jellyfin's dates ("2026-10-17T20:44:32.1234567Z") as unix seconds.
fn secs(date: &Value) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(date.as_str()?).ok().map(|d| d.timestamp())
}

fn title_of(v: &Value) -> Title {
    Title {
        kind: if v["Type"].as_str() == Some("Series") { "series" } else { "movie" }.into(),
        title: v["Name"].as_str().unwrap_or_default().to_string(),
        year: v["ProductionYear"].as_i64(),
        tmdb: pid_num(v, "Tmdb"),
        tvdb: pid_num(v, "Tvdb"),
        imdb: pid_str(v, "Imdb"),
        at: secs(&v["UserData"]["LastPlayedDate"]),
    }
}

/// Films or shows of this PC's library, findable by any of their ids.
#[derive(Default)]
struct Library<'a> {
    tmdb: HashMap<i64, &'a Value>,
    tvdb: HashMap<i64, &'a Value>,
    imdb: HashMap<String, &'a Value>,
}

impl<'a> Library<'a> {
    fn new(items: &'a [Value]) -> Self {
        let mut l = Library::default();
        for v in items {
            if let Some(id) = pid_num(v, "Tmdb") {
                l.tmdb.entry(id).or_insert(v);
            }
            if let Some(id) = pid_num(v, "Tvdb") {
                l.tvdb.entry(id).or_insert(v);
            }
            if let Some(id) = pid_str(v, "Imdb") {
                l.imdb.entry(id.to_ascii_lowercase()).or_insert(v);
            }
        }
        l
    }

    /// A film by TMDB first; a show by TVDB first (each one's own main id).
    fn find(&self, series: bool, tmdb: Option<i64>, tvdb: Option<i64>, imdb: Option<&str>) -> Option<&'a Value> {
        let by_tmdb = || tmdb.and_then(|i| self.tmdb.get(&i).copied());
        let by_tvdb = || tvdb.and_then(|i| self.tvdb.get(&i).copied());
        let by_imdb = || imdb.and_then(|i| self.imdb.get(&i.to_ascii_lowercase()).copied());
        if series { by_tvdb().or_else(by_tmdb).or_else(by_imdb) } else { by_tmdb().or_else(by_imdb).or_else(by_tvdb) }
    }
}

fn played(v: &Value) -> bool {
    v["UserData"]["Played"].as_bool().unwrap_or(false)
}

fn jf_id(v: &Value) -> Option<&str> {
    v["Id"].as_str().filter(|s| !s.is_empty())
}

// ---------- checking what a file says ----------

fn cut(s: &str, n: usize) -> String {
    s.trim().chars().filter(|c| !c.is_control()).take(n).collect()
}

fn cut_opt(s: Option<&str>, n: usize) -> Option<String> {
    s.map(|x| cut(x, n)).filter(|x| !x.is_empty())
}

/// A picture's address, only from the web over https.
fn clean_url(u: Option<&str>) -> Option<String> {
    u.map(str::trim).filter(|u| u.starts_with("https://") && u.len() <= 600 && !u.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '"' | '\'' | '<' | '>' | '\\' | '(' | ')'))).map(String::from)
}

fn clean_imdb(s: Option<&str>) -> Option<String> {
    s.map(str::trim).filter(|s| s.len() <= 16 && s.len() > 2 && s.starts_with("tt") && s[2..].bytes().all(|b| b.is_ascii_digit())).map(String::from)
}

fn clean_id(n: Option<i64>) -> Option<i64> {
    n.filter(|n| *n > 0 && *n < 1_000_000_000_000)
}

fn clean_genres(g: &[String]) -> Vec<String> {
    g.iter().map(|x| cut(x, 40)).filter(|x| !x.is_empty()).take(12).collect()
}

/// A time that can be a real one (from 1990 to a day from now).
fn clean_time(t: Option<i64>) -> Option<i64> {
    let now = chrono::Utc::now().timestamp();
    t.filter(|t| *t > 631_152_000 && *t < now + 86_400)
}

pub fn clean_title(t: &Title) -> Option<Title> {
    let kind = match t.kind.as_str() {
        "movie" | "series" => t.kind.clone(),
        _ => return None,
    };
    let out = Title {
        kind,
        title: cut(&t.title, 300),
        year: t.year.filter(|y| (1850..=3000).contains(y)),
        tmdb: clean_id(t.tmdb),
        tvdb: clean_id(t.tvdb),
        imdb: clean_imdb(t.imdb.as_deref()),
        at: clean_time(t.at),
    };
    (out.tmdb.is_some() || out.tvdb.is_some() || out.imdb.is_some()).then_some(out)
}

pub fn clean_episode(e: &Episode) -> Option<Episode> {
    let out = Episode {
        series: cut(&e.series, 300),
        tmdb: clean_id(e.tmdb),
        tvdb: clean_id(e.tvdb),
        imdb: clean_imdb(e.imdb.as_deref()),
        season: e.season,
        episode: e.episode,
        at: clean_time(e.at),
    };
    let ok = (0..=10_000).contains(&e.season) && (0..=100_000).contains(&e.episode) && (out.tmdb.is_some() || out.tvdb.is_some() || out.imdb.is_some());
    ok.then_some(out)
}

pub fn clean_resume(r: &Resume) -> Option<Resume> {
    let (movie, ep) = (r.kind == "movie", r.kind == "episode");
    let out = Resume {
        kind: r.kind.clone(),
        title: cut(&r.title, 300),
        tmdb: clean_id(r.tmdb),
        tvdb: clean_id(r.tvdb),
        imdb: clean_imdb(r.imdb.as_deref()),
        season: r.season.filter(|s| (0..=10_000).contains(s)),
        episode: r.episode.filter(|e| (0..=100_000).contains(e)),
        seconds: r.seconds,
    };
    let ids = out.tmdb.is_some() || out.tvdb.is_some() || out.imdb.is_some();
    let place = movie || (ep && out.season.is_some() && out.episode.is_some());
    // A day at most: longer is not a real position.
    (ids && place && r.seconds.is_finite() && r.seconds > 0.0 && r.seconds < 86_400.0).then_some(out)
}

/// A My List title kept by Chillarr (not in the library): only what a card
/// shows, and nothing that belongs to the PC it came from (its Jellyfin ids,
/// progress, download state).
pub fn clean_tile(t: &Tile) -> Option<Tile> {
    let (prefix, id) = t.key.split_once(':')?;
    let key_ok = matches!(prefix, "tmdb" | "tvdb" | "anilist" | "imdb") && !id.is_empty() && id.len() <= 16 && id.bytes().all(|b| b.is_ascii_alphanumeric());
    if !key_ok || !matches!(t.kind.as_str(), "movie" | "series" | "anime") {
        return None;
    }
    let title = cut(&t.title, 300);
    if title.is_empty() {
        return None;
    }
    Some(Tile {
        key: t.key.clone(),
        kind: t.kind.clone(),
        title,
        subtitle: cut_opt(t.subtitle.as_deref(), 200),
        year: t.year.filter(|y| (1850..=3000).contains(y)),
        overview: cut_opt(t.overview.as_deref(), 4000),
        poster: clean_url(t.poster.as_deref()),
        backdrop: clean_url(t.backdrop.as_deref()),
        rating: t.rating.filter(|r| r.is_finite() && (0.0..=100.0).contains(r)),
        tmdb: clean_id(t.tmdb),
        tvdb: clean_id(t.tvdb),
        format: cut_opt(t.format.as_deref(), 20),
        genres: clean_genres(&t.genres),
        trailer: t.trailer.as_deref().map(str::trim).filter(|s| !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')).map(String::from),
        ..Tile::default()
    })
}

pub fn clean_movie_watch(m: &MovieWatch) -> Option<MovieWatch> {
    Some(MovieWatch { title: cut(&m.title, 300), tmdb: Some(clean_id(m.tmdb)?), anime: m.anime, at: clean_time(Some(m.at))? })
}

fn clean_ep_key(k: &str) -> bool {
    k.split_once(':').is_some_and(|(s, e)| s.len() <= 5 && e.len() <= 6 && !s.is_empty() && !e.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && e.bytes().all(|b| b.is_ascii_digit()))
}

pub fn clean_series_watch(w: &SeriesWatch) -> Option<SeriesWatch> {
    let tvdb = clean_id(w.tvdb)?;
    let episodes: BTreeMap<String, i64> = w.episodes.iter().filter(|(k, _)| clean_ep_key(k)).filter_map(|(k, at)| Some((k.clone(), clean_time(Some(*at))?))).take(MAX_ITEMS).collect();
    Some(SeriesWatch {
        title: cut(&w.title, 300),
        tvdb: Some(tvdb),
        anilist: clean_id(w.anilist),
        genres: clean_genres(&w.genres),
        anime: w.anime,
        complete_at: clean_time(w.complete_at),
        episodes,
        ..SeriesWatch::default()
    })
}

pub fn clean_watch(w: &Watch) -> Option<Watch> {
    use crate::notify::Source;
    let source = match &w.source {
        Source::Movie(i) => Source::Movie(clean_id(Some(*i))?),
        Source::Show(i) => Source::Show(clean_id(Some(*i))?),
        Source::Anime(i) => Source::Anime(clean_id(Some(*i))?),
        Source::Novel(i) => Source::Novel(clean_id(Some(*i))?),
        Source::Author(n) => Source::Author(Some(cut(n, 200)).filter(|n| !n.is_empty())?),
    };
    let title = cut(&w.title, 300);
    if title.is_empty() {
        return None;
    }
    Some(Watch { source, title, poster: clean_url(w.poster.as_deref()), auto: false, tile: w.tile.as_ref().and_then(clean_tile), seen: None, works: Vec::new(), added: 0 })
}

fn clean_lang_mode(m: Option<&str>) -> Option<String> {
    m.filter(|m| matches!(*m, "manga" | "webtoon" | "book" | "ltr")).map(String::from)
}

fn clean_chapter(c: &ChapterRef) -> ChapterRef {
    ChapterRef { id: c.id, number: c.number.filter(|n| n.is_finite() && *n >= 0.0 && *n < 1_000_000.0), name: cut(&c.name, 200) }
}

pub fn clean_series_read(s: &SeriesRead) -> Option<SeriesRead> {
    let num = |n: Option<f64>| n.filter(|n| n.is_finite() && *n >= 0.0 && *n < 1_000_000.0);
    let out = SeriesRead {
        title: cut(&s.title, 300),
        reading_now: cut(&s.reading_now, 120),
        chapter_now: num(s.chapter_now),
        last_read: num(s.last_read),
        from: num(s.from),
        new_too: s.new_too,
        anilist: clean_id(s.anilist),
        source: cut(&s.source, 200),
        source_title: cut(&s.source_title, 300),
        cover: clean_url(s.cover.as_deref()),
        genres: clean_genres(&s.genres),
        country: s.country.as_deref().map(str::trim).filter(|c| c.len() == 2 && c.bytes().all(|b| b.is_ascii_uppercase())).map(String::from),
        format: s.format.as_deref().map(str::trim).filter(|f| !f.is_empty() && f.len() <= 20 && f.bytes().all(|b| b.is_ascii_uppercase() || b == b'_')).map(String::from),
        following: s.following,
        place: s.place.as_ref().map(|p| Place {
            chapter: clean_chapter(&p.chapter),
            page: p.page.min(100_000),
            pages: p.pages.min(100_000),
            at: clean_time(Some(p.at)).unwrap_or(0),
            read: p.read.iter().take(MAX_ITEMS).filter_map(|(c, at)| Some((clean_chapter(c), clean_time(Some(*at))?))).collect(),
            mode: clean_lang_mode(p.mode.as_deref()),
            hide_week: p.hide_week,
        }),
    };
    (!out.title.is_empty() && (out.anilist.is_some() || !out.source_title.is_empty())).then_some(out)
}

pub fn clean_book(b: &BookRead) -> Option<BookRead> {
    let olid = b.olid.as_deref().map(str::trim).filter(|o| !o.is_empty() && o.len() <= 80 && o.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'/')).map(String::from);
    let out = BookRead {
        title: cut(&b.title, 300),
        author: cut_opt(b.author.as_deref(), 200),
        olid,
        anilist: clean_id(b.anilist),
        cover: clean_url(b.cover.as_deref()),
        comic: b.comic,
        following: b.following,
        spot: b.spot.as_ref().map(|s| Spot {
            chapter: s.chapter.clamp(-1, 100_000),
            page: s.page.min(100_000),
            pages: s.pages.min(100_000),
            at: clean_time(Some(s.at)).unwrap_or(0),
            read: s.read.iter().filter(|(c, _)| (-1..=100_000).contains(*c)).filter_map(|(c, at)| Some((*c, clean_time(Some(*at))?))).take(MAX_ITEMS).collect(),
            mode: clean_lang_mode(s.mode.as_deref()),
            hide_week: s.hide_week,
        }),
    };
    (!out.title.is_empty()).then_some(out)
}

/// The file's text as an export, or why it isn't one.
pub fn parse(text: &str) -> Result<Export, String> {
    let v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|_| "That isn't a Chillarr profile file.".to_string())?;
    if v["format"].as_str() != Some(FORMAT) {
        return Err("That isn't a Chillarr profile file.".into());
    }
    let version = v["version"].as_u64().unwrap_or(0);
    if version == 0 {
        return Err("That file is damaged: it doesn't say which version it is.".into());
    }
    if version > VERSION as u64 {
        return Err("That file was made by a newer Chillarr. Update this one first, then import it again.".into());
    }
    let mut e: Export = serde_json::from_value(v).map_err(|e| format!("That file is damaged ({e})."))?;
    e.reading_list.truncate(MAX_ITEMS);
    e.my_list.truncate(MAX_ITEMS);
    e.my_list_saved.truncate(MAX_ITEMS);
    e.watched.truncate(MAX_ITEMS);
    e.watched_episodes.truncate(MAX_ITEMS * 5);
    e.resume.truncate(MAX_ITEMS);
    e.history_movies.truncate(MAX_ITEMS);
    e.history_series.truncate(MAX_ITEMS);
    e.notify.truncate(MAX_ITEMS);
    e.reading.truncate(MAX_ITEMS);
    e.books.truncate(MAX_ITEMS);
    Ok(e)
}

/// The chapter here a `ChapterRef` from the file means: the same number, else
/// the same name, else the same id when its name agrees (the same PC).
pub fn resolve_chapter(here: &[crate::suwayomi::Chapter], c: &ChapterRef) -> Option<i64> {
    if let Some(n) = c.number {
        return here.iter().find(|x| x.number.is_some_and(|m| (m - n).abs() < 1e-9)).map(|x| x.id);
    }
    if !c.name.is_empty() {
        if let Some(x) = here.iter().find(|x| x.name.eq_ignore_ascii_case(&c.name)) {
            return Some(x.id);
        }
    }
    here.iter().find(|x| x.id == c.id && c.id > 0 && x.name.eq_ignore_ascii_case(&c.name)).map(|x| x.id)
}

fn adult(genres: &[String]) -> bool {
    genres.iter().any(|g| g.eq_ignore_ascii_case("Ecchi") || g.eq_ignore_ascii_case("Hentai"))
}

fn card_adult(c: &Value) -> bool {
    c["genres"].as_array().is_some_and(|a| a.iter().any(|g| g.as_str().is_some_and(|g| g.eq_ignore_ascii_case("Ecchi") || g.eq_ignore_ascii_case("Hentai"))))
}

// ---------- out ----------

fn profile_name(app: &AppHandle, uid: &str) -> String {
    if let Some(p) = crate::profiles::list(app).into_iter().find(|p| p.id == uid) {
        return p.name;
    }
    crate::viewer::current().map(|v| v.session.user).unwrap_or_else(|| "Profile".into())
}

/// Everything of the watching profile's that the file carries.
pub async fn export(app: &AppHandle, prefs: BTreeMap<String, String>) -> Result<Export, String> {
    let uid = crate::media::active_user(app).ok_or("Sign in to Jellyfin first.")?;
    let dir = crate::media::data_dir(app).ok_or("Chillarr isn't ready yet.")?;
    let j = crate::media::jf(app).filter(|j| j.user_id.as_deref() == Some(uid.as_str())).ok_or("Sign in to Jellyfin first.")?;
    // All of Jellyfin's part at once, and all of it or nothing: a file
    // missing the watched marks would look complete and not be.
    let (favs, movies, episodes, series, resumable) = tokio::join!(
        j.watch_items("includeItemTypes=Movie,Series&isFavorite=true"),
        j.watch_items("includeItemTypes=Movie&filters=IsPlayed"),
        j.watch_items("includeItemTypes=Episode&filters=IsPlayed"),
        j.watch_items("includeItemTypes=Series"),
        j.watch_items("includeItemTypes=Movie,Episode&filters=IsResumable"),
    );
    let jf_err = |e: crate::jellyfin::JfError| format!("Jellyfin didn't answer, so nothing was saved ({e:?}). Try again in a moment.");
    let (favs, movies, episodes, series, resumable) = (favs.map_err(jf_err)?, movies.map_err(jf_err)?, episodes.map_err(jf_err)?, series.map_err(jf_err)?, resumable.map_err(jf_err)?);
    let shows: HashMap<&str, &Value> = series.iter().filter_map(|v| Some((jf_id(v)?, v))).collect();
    let show_of = |v: &Value| v["SeriesId"].as_str().and_then(|id| shows.get(id).copied());

    let mut out = Export {
        format: FORMAT.into(),
        version: VERSION,
        exported_at: chrono::Utc::now().timestamp(),
        profile: profile_name(app, &uid),
        app: app.package_info().version.to_string(),
        ..Export::default()
    };
    out.my_list = favs.iter().map(|v| Title { at: None, ..title_of(v) }).filter(|t| t.tmdb.is_some() || t.tvdb.is_some() || t.imdb.is_some()).collect();
    out.my_list_saved = app.state::<crate::media::Media>().saved_list().iter().filter_map(clean_tile).collect();
    out.watched = movies.iter().map(title_of).filter(|t| t.tmdb.is_some() || t.imdb.is_some()).collect();
    for e in &episodes {
        let (Some(show), Some(season), Some(number)) = (show_of(e), e["ParentIndexNumber"].as_i64(), e["IndexNumber"].as_i64()) else { continue };
        let t = title_of(show);
        if t.tmdb.is_none() && t.tvdb.is_none() && t.imdb.is_none() {
            continue;
        }
        out.watched_episodes.push(Episode { series: t.title, tmdb: t.tmdb, tvdb: t.tvdb, imdb: t.imdb, season, episode: number, at: secs(&e["UserData"]["LastPlayedDate"]) });
    }
    for v in &resumable {
        let seconds = crate::jellyfin::ticks_to_seconds(v["UserData"]["PlaybackPositionTicks"].as_i64().unwrap_or(0));
        if seconds <= 0.0 {
            continue;
        }
        let r = if v["Type"].as_str() == Some("Episode") {
            let Some(show) = show_of(v) else { continue };
            let t = title_of(show);
            Resume { kind: "episode".into(), title: t.title, tmdb: t.tmdb, tvdb: t.tvdb, imdb: t.imdb, season: v["ParentIndexNumber"].as_i64(), episode: v["IndexNumber"].as_i64(), seconds }
        } else {
            let t = title_of(v);
            Resume { kind: "movie".into(), title: t.title, tmdb: t.tmdb, tvdb: t.tvdb, imdb: t.imdb, season: None, episode: None, seconds }
        };
        if clean_resume(&r).is_some() {
            out.resume.push(r);
        }
    }

    let h = crate::history::load(&dir, &uid);
    (out.history_movies, out.history_series) = crate::history::portable(&h);
    out.completed_hidden = crate::completed::load_hidden(&dir, &uid);
    out.markers = crate::marker::load(&dir, &uid);
    // As they are, Get it too included: the import decides what comes in on.
    out.notify = crate::notify::watches_of(app, &uid);

    // Reading: the list, every series followed or read, and the books.
    out.reading_list = crate::read::list_of(app, &uid);
    let spots = crate::read::spots(app, &uid);
    let pc_owner = crate::profiles::owner_id(app);
    let followed = crate::read::followed(app);
    // Chapter numbers come from the downloader: started if it's set up here
    // (it is, when anything is followed), never installed for an export.
    if followed.iter().any(|f| spots.contains_key(&f.key)) {
        let _ = crate::suwayomi::ready(app).await;
    }
    for f in followed {
        let following = crate::read::owned_by(&f.owners, pc_owner.as_deref(), &uid);
        let spot = spots.get(&f.key);
        if !following && spot.is_none() {
            continue;
        }
        let place = match spot {
            Some(s) => {
                // Chapters by number need the downloader's list; without it the
                // place still goes, findable again on this PC by id and name.
                let here: Vec<crate::suwayomi::Chapter> = crate::read::chapters_of(&f).await.map(|v| v.into_iter().map(|x| x.0).collect()).unwrap_or_default();
                let reference = |id: i64| match here.iter().find(|c| c.id == id) {
                    Some(c) => ChapterRef { id, number: c.number, name: c.name.clone() },
                    None => ChapterRef { id, number: None, name: String::new() },
                };
                Some(Place {
                    chapter: reference(s.chapter),
                    page: s.page,
                    pages: s.pages,
                    at: s.at,
                    read: s.read.iter().map(|(id, at)| (reference(*id), *at)).collect(),
                    mode: s.mode.clone(),
                    hide_week: s.hide_week,
                })
            }
            None => None,
        };
        let chapter_now = place.as_ref().and_then(|p| p.chapter.number);
        let last_read = place.as_ref().and_then(|p| p.read.iter().filter_map(|(c, _)| c.number).fold(None, |a: Option<f64>, n| Some(a.map_or(n, |a| a.max(n)))));
        out.reading.push(SeriesRead {
            title: f.title.clone(),
            reading_now: where_text(place.as_ref()),
            chapter_now,
            last_read,
            from: f.from,
            new_too: f.new_too,
            anilist: f.anilist,
            source: f.source.clone(),
            source_title: f.source_title.clone(),
            cover: clean_url(f.cover.as_deref()),
            genres: f.genres.clone(),
            country: f.country.clone(),
            format: f.format.clone(),
            following,
            place,
        });
    }
    for b in crate::books::all(app) {
        let following = crate::read::owned_by(&b.owners, pc_owner.as_deref(), &uid);
        let spot = spots.get(&b.key).cloned();
        if !following && spot.is_none() {
            continue;
        }
        out.books.push(BookRead { title: b.title.clone(), author: b.author.clone(), olid: b.olid.clone(), anilist: b.anilist, cover: clean_url(b.cover.as_deref()), comic: b.comic, following, spot });
    }
    out.music = crate::music::kept_by(app, &uid);
    let mine = crate::music::mine_of(app, &uid);
    (out.music_liked, out.music_recent) = (mine.liked, mine.recent);
    out.hidden = crate::hidden::portable(app);
    out.prefs = clean_prefs(&prefs);
    Ok(out)
}

/// The file's name: "Chillarr - Sam - 2026-11-15.json".
pub fn file_name(profile: &str) -> String {
    let safe: String = profile.chars().filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_')).take(40).collect::<String>().trim().to_string();
    let who = if safe.is_empty() { "Profile".to_string() } else { safe };
    format!("Chillarr - {who} - {}.json", chrono::Local::now().format("%Y-%m-%d"))
}

/// The PC window's export: Windows asks where it goes (Downloads and the
/// usual name to start with). Returns where it went, or None when the window
/// was closed without saving. The data is read first, so a Jellyfin that
/// doesn't answer says so before anyone picks a folder.
pub async fn export_to_file(app: &AppHandle, prefs: BTreeMap<String, String>) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let e = export(app, prefs).await?;
    let text = serde_json::to_string_pretty(&e).map_err(|e| e.to_string())?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut dialog = app.dialog().file().set_title("Save your Chillarr data").set_file_name(file_name(&e.profile)).add_filter("Chillarr profile file", &["json"]);
    if let Ok(dir) = crate::support::save_dir() {
        dialog = dialog.set_directory(dir);
    }
    dialog.save_file(move |f| {
        let _ = tx.send(f.and_then(|p| p.into_path().ok()));
    });
    let Some(mut path) = rx.await.ok().flatten() else { return Ok(None) };
    if path.extension().is_none() {
        path.set_extension("json");
    }
    std::fs::write(&path, text).map_err(|e| format!("Couldn't save the file: {e}"))?;
    Ok(Some(path.to_string_lossy().to_string()))
}

// ---------- a place in a series ----------

/// 96 as "96", 12.5 as "12.5".
fn num_text(n: f64) -> String {
    if n.fract() == 0.0 { format!("{}", n as i64) } else { format!("{n}") }
}

/// Where a place is, in words, for the file and the follow panel.
pub fn where_text(p: Option<&Place>) -> String {
    let Some(p) = p else { return "Not started".into() };
    let name = match p.chapter.number {
        Some(n) => format!("Chapter {}", num_text(n)),
        None if !p.chapter.name.is_empty() => p.chapter.name.clone(),
        None => "A chapter".into(),
    };
    let finished = p.read.iter().any(|(c, _)| c.id == p.chapter.id && (c.number == p.chapter.number));
    if finished {
        match p.chapter.number {
            Some(n) => format!("Finished chapter {}", num_text(n)),
            None => format!("Finished {name}"),
        }
    } else if p.pages > 0 {
        format!("{name}, page {} of {}", (p.page + 1).min(p.pages), p.pages)
    } else {
        name
    }
}

/// The followed series here that is this one: by AniList id, else by the
/// site and the series' name there.
fn matching<'a>(here: &'a [crate::read::Followed], anilist: Option<i64>, source: &str, source_title: &str) -> Option<&'a crate::read::Followed> {
    here.iter()
        .find(|f| anilist.is_some() && f.anilist == anilist)
        .or_else(|| here.iter().find(|f| !source_title.is_empty() && f.source.eq_ignore_ascii_case(source) && f.source_title.eq_ignore_ascii_case(source_title)))
}

/// Puts a place from a file into this profile's place in `f`, its chapters
/// found by number. Ok(true) when it changed; Err when the chapters can't be
/// listed, or none of the place's chapters are in this copy.
async fn place_into(app: &AppHandle, uid: &str, f: &crate::read::Followed, p: &Place) -> Result<bool, String> {
    let chs: Vec<crate::suwayomi::Chapter> = crate::read::chapters_of(f).await?.into_iter().map(|x| x.0).collect();
    let read: HashMap<i64, i64> = p.read.iter().filter_map(|(c, at)| Some((resolve_chapter(&chs, c)?, *at))).collect();
    let spot = match resolve_chapter(&chs, &p.chapter) {
        Some(id) => Spot { chapter: id, page: p.page, pages: p.pages, at: p.at, read, mode: p.mode.clone(), hide_week: p.hide_week },
        // The chapter it was on isn't here: the finished ones still count,
        // and a place with no time never wins over one already here.
        None => {
            let last = chs.iter().filter(|c| read.contains_key(&c.id)).max_by(|a, b| a.number.unwrap_or(0.0).total_cmp(&b.number.unwrap_or(0.0))).map(|c| c.id).ok_or("None of those chapters are in this copy.")?;
            Spot { chapter: last, page: 0, pages: 0, at: 0, read, mode: p.mode.clone(), hide_week: p.hide_week }
        }
    };
    crate::read::absorb_spot(app, uid, &f.key, &spot)
}

// ---------- places waiting for a series to be followed ----------

const MAX_WAITING: usize = 2000;

fn waiting_file(dir: &std::path::Path, uid: &str) -> std::path::PathBuf {
    dir.join(format!("read-waiting-{uid}.json"))
}

fn load_waiting(dir: &std::path::Path, uid: &str) -> Vec<SeriesRead> {
    std::fs::read_to_string(waiting_file(dir, uid)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save_waiting(dir: &std::path::Path, uid: &str, list: &[SeriesRead]) -> Result<(), String> {
    let path = waiting_file(dir, uid);
    if list.is_empty() {
        let _ = std::fs::remove_file(&path);
        return Ok(());
    }
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string(list).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, &path)).map_err(|e| format!("Couldn't save it: {e}"))
}

/// The same series in two lists: by AniList id, else by site and name, else by title.
fn same_series(a: &SeriesRead, b: &SeriesRead) -> bool {
    match (a.anilist, b.anilist) {
        (Some(x), Some(y)) => x == y,
        _ => (!a.source_title.is_empty() && a.source.eq_ignore_ascii_case(&b.source) && a.source_title.eq_ignore_ascii_case(&b.source_title)) || a.title.eq_ignore_ascii_case(&b.title),
    }
}

/// Keeps places for series not followed here. A later import of the same
/// series replaces the older one when it's further on. Returns how many are new.
fn keep_waiting(dir: &std::path::Path, uid: &str, incoming: Vec<SeriesRead>) -> Result<usize, String> {
    if incoming.is_empty() {
        return Ok(0);
    }
    let mut list = load_waiting(dir, uid);
    let mut n = 0;
    for s in incoming {
        match list.iter().position(|x| same_series(x, &s)) {
            Some(i) => {
                let newer = s.place.as_ref().map(|p| p.at).unwrap_or(0) > list[i].place.as_ref().map(|p| p.at).unwrap_or(0);
                if newer {
                    list[i] = s;
                }
            }
            None if list.len() < MAX_WAITING => {
                list.push(s);
                n += 1;
            }
            None => {}
        }
    }
    save_waiting(dir, uid, &list)?;
    Ok(n)
}

/// What the follow panel shows for a series an import knows about: where
/// you were, so "Continue from chapter" can start there.
#[derive(Serialize, Debug, PartialEq)]
pub struct Waiting {
    pub reading_now: String,
    /// The chapter to continue from: the one you were on, else the one after the last finished.
    pub continue_from: Option<f64>,
}

pub fn waiting_for(app: &AppHandle, anilist: Option<i64>, title: &str) -> Option<Waiting> {
    let uid = crate::media::active_user(app)?;
    let dir = crate::media::data_dir(app)?;
    let probe = SeriesRead { title: title.to_string(), anilist, ..SeriesRead::default() };
    let s = load_waiting(&dir, &uid).into_iter().find(|x| same_series(x, &probe))?;
    Some(waiting_view(&s))
}

fn waiting_view(s: &SeriesRead) -> Waiting {
    let finished_now = s.place.as_ref().is_some_and(|p| p.read.iter().any(|(c, _)| c.number.is_some() && c.number == p.chapter.number));
    let continue_from = match (s.chapter_now, s.last_read) {
        (Some(n), _) if !finished_now => Some(n),
        (_, Some(l)) => Some((l + 1.0).floor()),
        (Some(n), None) => Some((n + 1.0).floor()),
        (None, None) => s.from,
    };
    Waiting { reading_now: if s.reading_now.is_empty() { where_text(s.place.as_ref()) } else { s.reading_now.clone() }, continue_from }
}

/// A series was just followed (or joined): a place waiting for it goes in,
/// and it leaves the waiting list. Never fails the follow.
pub async fn apply_waiting(app: &AppHandle, uid: &str, key: &str) {
    let Some(dir) = crate::media::data_dir(app) else { return };
    let mut list = load_waiting(&dir, uid);
    if list.is_empty() {
        return;
    }
    let here = crate::read::followed(app);
    let Some(f) = here.iter().find(|f| f.key == key) else { return };
    let Some(at) = list.iter().position(|s| matching(std::slice::from_ref(f), s.anilist, &s.source, &s.source_title).is_some() || (s.anilist.is_none() && s.title.eq_ignore_ascii_case(&f.title))) else { return };
    let s = list[at].clone();
    let done = match &s.place {
        Some(p) => match place_into(app, uid, f, p).await {
            Ok(_) => true,
            // The chapter list wasn't there yet: it stays waiting for the next open.
            Err(_) => false,
        },
        None => true,
    };
    if done {
        list.remove(at);
        let _ = save_waiting(&dir, uid, &list);
        if s.place.is_some() {
            crate::log::log(app, crate::model::Level::Ok, format!("{}: put you back on {} from your imported data.", f.title, where_text(s.place.as_ref()).to_lowercase()));
        }
    }
}

// ---------- in ----------

/// What an import did, line by line, for the page.
#[derive(Serialize, Default, Debug)]
pub struct Report {
    pub lines: Vec<String>,
    /// Something in the file couldn't be put in (a title this PC doesn't have).
    pub missing: usize,
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Adds a file's data to the watching profile. Nothing is taken away.
pub async fn import(app: &AppHandle, text: &str) -> Result<Report, String> {
    let file = parse(text)?;
    let uid = crate::media::active_user(app).ok_or("Sign in to Jellyfin first.")?;
    let dir = crate::media::data_dir(app).ok_or("Chillarr isn't ready yet.")?;
    let j = crate::media::jf(app).filter(|j| j.user_id.as_deref() == Some(uid.as_str())).ok_or("Sign in to Jellyfin first.")?;
    let kids = crate::profiles::is_kids(app, &uid);
    let for_kids_ok = |t: &Tile| !kids || !crate::media::not_for_kids(app, t);
    // The library as this profile sees it (a kids' profile sees less), asked
    // before anything changes: when Jellyfin can't answer, nothing is done.
    let (movies, series) = tokio::join!(j.watch_items("includeItemTypes=Movie"), j.watch_items("includeItemTypes=Series"));
    let jf_err = |e: crate::jellyfin::JfError| format!("Jellyfin didn't answer, so nothing was imported ({e:?}). Try again in a moment.");
    let (movies, series) = (movies.map_err(jf_err)?, series.map_err(jf_err)?);
    let (films, shows) = (Library::new(&movies), Library::new(&series));
    let mut rep = Report::default();
    let mut failed = 0usize;

    // My List: a title in the library becomes a favourite, one that isn't is
    // kept by Chillarr like any other title not here yet.
    let mut favs = 0;
    let mut saved: Vec<Tile> = Vec::new();
    let mut fav_ids: Vec<String> = Vec::new();
    for t in file.my_list.iter().filter_map(clean_title) {
        let series = t.kind == "series";
        match (if series { &shows } else { &films }).find(series, t.tmdb, t.tvdb, t.imdb.as_deref()) {
            Some(v) => {
                if v["UserData"]["IsFavorite"].as_bool() != Some(true) {
                    if let Some(id) = jf_id(v) {
                        fav_ids.push(id.to_string());
                    }
                }
            }
            None => {
                let key = match (series, t.tmdb, t.tvdb) {
                    (false, Some(id), _) => format!("tmdb:{id}"),
                    (true, _, Some(id)) => format!("tvdb:{id}"),
                    _ => continue,
                };
                let tile = Tile { key, kind: t.kind.clone(), title: t.title.clone(), year: t.year, tmdb: t.tmdb, tvdb: t.tvdb, ..Tile::default() };
                saved.push(tile);
            }
        }
    }
    for t in file.my_list_saved.iter().filter_map(clean_tile) {
        let series = t.kind != "movie";
        match (if series { &shows } else { &films }).find(series, t.tmdb, t.tvdb, None) {
            Some(v) => {
                if v["UserData"]["IsFavorite"].as_bool() != Some(true) {
                    if let Some(id) = jf_id(v) {
                        fav_ids.push(id.to_string());
                    }
                }
            }
            None => saved.push(t),
        }
    }
    fav_ids.sort();
    fav_ids.dedup();
    for id in &fav_ids {
        match j.set_favourite(id, true).await {
            Ok(()) => favs += 1,
            Err(_) => failed += 1,
        }
    }
    saved.retain(|t| for_kids_ok(t));
    let kept = app.state::<crate::media::Media>().absorb_saved(saved);
    if favs + kept > 0 {
        rep.lines.push(format!("My List: {} added.", plural(favs + kept, "title", "titles")));
    }

    // Watched films, with the day they were watched.
    let mut marked_films = 0;
    for t in file.watched.iter().filter_map(clean_title).filter(|t| t.kind == "movie") {
        match films.find(false, t.tmdb, t.tvdb, t.imdb.as_deref()) {
            Some(v) if !played(v) => match jf_id(v) {
                Some(id) => match j.set_played_at(id, t.at).await {
                    Ok(()) => marked_films += 1,
                    Err(_) => failed += 1,
                },
                None => {}
            },
            Some(_) => {}
            None => rep.missing += 1,
        }
    }

    // Watched episodes and half-watched ones: each show's episodes asked once.
    let mut episodes_of: HashMap<String, Vec<Value>> = HashMap::new();
    let mut marked_eps = 0;
    let mut shows_missing: Vec<(Option<i64>, Option<i64>, Option<String>)> = Vec::new();
    for e in file.watched_episodes.iter().filter_map(clean_episode) {
        let Some(show) = shows.find(true, e.tmdb, e.tvdb, e.imdb.as_deref()).and_then(jf_id) else {
            let k = (e.tmdb, e.tvdb, e.imdb.clone());
            if !shows_missing.contains(&k) {
                shows_missing.push(k);
            }
            continue;
        };
        if !episodes_of.contains_key(show) {
            let eps = j.episodes(show).await.unwrap_or_default();
            episodes_of.insert(show.to_string(), eps);
        }
        let found = episodes_of[show].iter().find(|x| x["ParentIndexNumber"].as_i64() == Some(e.season) && x["IndexNumber"].as_i64() == Some(e.episode));
        if let Some(x) = found.filter(|x| !played(x)) {
            if let Some(id) = jf_id(x) {
                match j.set_played_at(id, e.at).await {
                    Ok(()) => marked_eps += 1,
                    Err(_) => failed += 1,
                }
            }
        }
    }
    rep.missing += shows_missing.len();
    if marked_films + marked_eps > 0 {
        let mut parts = Vec::new();
        if marked_films > 0 {
            parts.push(plural(marked_films, "film", "films"));
        }
        if marked_eps > 0 {
            parts.push(plural(marked_eps, "episode", "episodes"));
        }
        rep.lines.push(format!("Watched: {} marked.", parts.join(" and ")));
    }

    // Continue watching: only where nothing is going on here (not watched,
    // not started), so this PC's own place is never moved back.
    let mut resumed = 0;
    for r in file.resume.iter().filter_map(|r| clean_resume(r)) {
        let item: Option<Value> = if r.kind == "movie" {
            films.find(false, r.tmdb, r.tvdb, r.imdb.as_deref()).cloned()
        } else {
            let Some(show) = shows.find(true, r.tmdb, r.tvdb, r.imdb.as_deref()).and_then(jf_id) else { continue };
            if !episodes_of.contains_key(show) {
                let eps = j.episodes(show).await.unwrap_or_default();
                episodes_of.insert(show.to_string(), eps);
            }
            episodes_of[show].iter().find(|x| x["ParentIndexNumber"].as_i64() == r.season && x["IndexNumber"].as_i64() == r.episode).cloned()
        };
        let Some(item) = item else { continue };
        let untouched = !played(&item) && item["UserData"]["PlaybackPositionTicks"].as_i64().unwrap_or(0) == 0;
        if let (true, Some(id)) = (untouched, jf_id(&item)) {
            match j.set_position(id, r.seconds).await {
                Ok(()) => resumed += 1,
                Err(_) => failed += 1,
            }
        }
    }
    if resumed > 0 {
        rep.lines.push(format!("Continue watching: {}.", plural(resumed, "place", "places")));
    }

    // Completed, from the watch log (titles watched elsewhere and never here too).
    let hist_movies: Vec<MovieWatch> = file
        .history_movies
        .iter()
        .filter_map(clean_movie_watch)
        .filter(|m| for_kids_ok(&Tile { kind: "movie".into(), tmdb: m.tmdb, ..Tile::default() }))
        .collect();
    let hist_series: Vec<SeriesWatch> = file.history_series.iter().filter_map(clean_series_watch).filter(|w| !kids || !adult(&w.genres)).collect();
    let (hm, hs) = crate::history::absorb_into(&dir, &uid, &hist_movies, &hist_series)?;
    let off = crate::completed::absorb_hidden(&dir, &uid, &file.completed_hidden)?;
    if hm + hs > 0 {
        rep.lines.push(format!("Completed and your watch history: {} and {} brought in.", plural(hm, "film", "films"), plural(hs, "show", "shows")));
        crate::newep::forget_user(app, &uid);
    }
    if off > 0 {
        rep.lines.push(format!("Taken off Completed: {}.", plural(off, "title", "titles")));
    }
    let marks = crate::marker::absorb(&dir, &uid, &file.markers)?;
    if marks > 0 {
        rep.lines.push(format!("Where you're at: {}.", plural(marks, "show", "shows")));
    }

    // Notify me, with Get it too off (notify.rs `absorb` says why).
    let watches: Vec<Watch> = file.notify.iter().filter_map(clean_watch).filter(|w| w.tile.as_ref().is_none_or(|t| for_kids_ok(t))).collect();
    let had_auto = file.notify.iter().any(|w| w.auto);
    let notes = crate::notify::absorb(app, &uid, watches)?;
    if notes > 0 {
        rep.lines.push(format!("Notify me: {}.{}", plural(notes, "title", "titles"), if had_auto { " \"Get it too\" is off for them: turn it on again where you want it." } else { "" }));
    }

    // Reading. A series someone here follows: this profile joins it and gets
    // its place back. One nobody here has goes on the reading list to follow.
    let pc_owner = crate::profiles::owner_id(app);
    let here = crate::read::followed(app);
    // Places are found by the downloader's chapter lists: started if it's set
    // up here (it is, when anything is followed), never installed for this.
    if !here.is_empty() && file.reading.iter().any(|s| s.place.is_some()) {
        let _ = crate::suwayomi::ready(app).await;
    }
    let (mut joined, mut placed, mut unplaced) = (0usize, 0usize, 0usize);
    let mut cards: Vec<Value> = Vec::new();
    let mut waiting: Vec<SeriesRead> = Vec::new();
    for s in file.reading.iter().filter_map(clean_series_read) {
        if kids && adult(&s.genres) {
            continue;
        }
        let Some(f) = matching(&here, s.anilist, &s.source, &s.source_title) else {
            if let Some(id) = s.anilist {
                cards.push(json!({ "anilist": id, "title": s.title, "cover": s.cover, "genres": s.genres, "country": s.country, "format": s.format }));
            }
            // Its place waits for the day it's followed here (`apply_waiting`).
            if s.place.is_some() || s.following {
                waiting.push(s.clone());
            }
            rep.missing += 1;
            continue;
        };
        if kids && adult(&f.genres) {
            continue;
        }
        if s.following && !crate::read::owned_by(&f.owners, pc_owner.as_deref(), &uid) && crate::read::join(app, &f.key, &uid) {
            joined += 1;
        }
        let Some(p) = &s.place else { continue };
        match place_into(app, &uid, f, p).await {
            Ok(true) => placed += 1,
            Ok(false) => {}
            Err(_) => unplaced += 1,
        }
    }
    let kept_waiting = keep_waiting(&dir, &uid, waiting)?;
    let books_here = crate::books::all(app);
    let mut books_in = 0usize;
    for b in file.books.iter().filter_map(clean_book) {
        let local = books_here
            .iter()
            .find(|x| b.olid.is_some() && x.olid == b.olid)
            .or_else(|| books_here.iter().find(|x| b.anilist.is_some() && x.anilist == b.anilist))
            .or_else(|| books_here.iter().find(|x| x.title.eq_ignore_ascii_case(&b.title)));
        let Some(x) = local else {
            match (b.anilist, &b.olid) {
                (Some(id), _) => cards.push(json!({ "anilist": id, "title": b.title, "author": b.author, "cover": b.cover, "kind": if b.comic { "comic" } else { "book" } })),
                (None, Some(o)) => cards.push(json!({ "olid": o, "title": b.title, "author": b.author, "cover": b.cover, "kind": if b.comic { "comic" } else { "book" } })),
                _ => {}
            }
            rep.missing += 1;
            continue;
        };
        if b.following && crate::books::join(app, &x.key, &uid)? {
            books_in += 1;
        }
        if let Some(s) = &b.spot {
            if crate::read::absorb_spot(app, &uid, &x.key, s)? {
                placed += 1;
            }
        }
    }
    let listed = crate::read::absorb_list(app, &uid, &file.reading_list.iter().chain(cards.iter()).cloned().collect::<Vec<_>>(), |c| !kids || !card_adult(c))?;
    let mut reading = Vec::new();
    if joined > 0 {
        reading.push(format!("{} followed", plural(joined, "series", "series")));
    }
    if books_in > 0 {
        reading.push(format!("{} added", plural(books_in, "book", "books")));
    }
    if placed > 0 {
        reading.push(format!("your place in {}", plural(placed, "title", "titles")));
    }
    if listed > 0 {
        reading.push(format!("{} on your reading list", plural(listed, "title", "titles")));
    }
    if !reading.is_empty() {
        rep.lines.push(format!("Reading: {}.", reading.join(", ")));
    }
    if kept_waiting > 0 {
        rep.lines.push(format!(
            "{} {} followed on this PC yet. Follow {} from the reading list and you're put back on your chapter, with the ones before it counted as read.",
            plural(kept_waiting, "series", "series"),
            if kept_waiting == 1 { "isn't" } else { "aren't" },
            if kept_waiting == 1 { "it" } else { "them" }
        ));
    }
    if unplaced > 0 {
        rep.lines.push(format!("Your place in {} couldn't be found: the manga downloader didn't answer, or this PC doesn't have those chapters. Import the file again later to try once more.", plural(unplaced, "series", "series")));
    }
    if rep.missing > 0 {
        rep.lines.push(format!("{} aren't on this PC. Films and shows are kept in Completed and My List; series and books go on your reading list.", plural(rep.missing, "title", "titles")));
    }
    if failed > 0 {
        rep.lines.push(format!("Jellyfin didn't take {}. Import the file again to try once more.", plural(failed, "change", "changes")));
    }
    let (liked, heard) = crate::music::take_mine(app, &uid, &file.music_liked, &file.music_recent);
    if liked > 0 || heard > 0 {
        rep.lines.push(format!("Music: {} liked, {} in Recently played.", plural(liked, "song", "songs"), plural(heard, "song", "songs")));
    }
    let songs = crate::music::take(app, &uid, file.music.clone());
    if songs > 0 {
        rep.lines.push(format!("Music: {} downloading now, into Music\\Chillarr.", plural(songs, "song", "songs")));
    }
    if let Some(h) = file.hidden.as_ref().filter(|h| !h.is_empty()) {
        match crate::hidden::take(app, h) {
            Ok(n) if n > 0 => rep.lines.push(format!("Hidden: {}.", plural(n, "title", "titles"))),
            Ok(_) => {}
            Err(_) => rep.lines.push(format!("The file has {} hidden. Set a hidden password in Settings, then import it again to hide them here.", plural(h.len(), "title", "titles"))),
        }
    }
    if !file.prefs.is_empty() {
        rep.lines.push("Your settings: put back on this device.".into());
    }
    if rep.lines.is_empty() {
        rep.lines.push("Everything in that file was already here.".into());
    }
    crate::log::log(app, crate::model::Level::Ok, format!("{} imported a profile file: {}", profile_name(app, &uid), rep.lines.join(" ")));
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_must_say_what_it_is() {
        assert!(parse("{}").unwrap_err().contains("isn't a Chillarr profile"));
        assert!(parse("not json").unwrap_err().contains("isn't a Chillarr profile"));
        assert!(parse(r#"{"format":"chillarr-profile"}"#).unwrap_err().contains("damaged"));
        assert!(parse(r#"{"format":"chillarr-profile","version":99}"#).unwrap_err().contains("newer Chillarr"));
        let e = parse("\u{feff}{\"format\":\"chillarr-profile\",\"version\":1,\"profile\":\"Sam\"}").unwrap();
        assert_eq!(e.profile, "Sam");
        assert!(e.my_list.is_empty(), "every part may be missing");
    }

    #[test]
    fn an_export_reads_back_the_same() {
        let mut e = Export { format: FORMAT.into(), version: VERSION, profile: "Sam".into(), ..Export::default() };
        e.my_list.push(Title { kind: "movie".into(), title: "Dune".into(), tmdb: Some(438631), ..Title::default() });
        e.markers.insert(81189, Mark { spot: crate::marker::Spot { season: 2, episode: 3 }, set_at: 5 });
        e.completed_hidden.insert("tmdb:1".into(), None);
        e.reading.push(SeriesRead { title: "Solo".into(), anilist: Some(105398), following: true, place: Some(Place { chapter: ChapterRef { id: 9, number: Some(12.5), name: "Ch 12.5".into() }, read: vec![(ChapterRef { id: 8, number: Some(12.0), name: String::new() }, 1_700_000_000)], ..Place::default() }), ..SeriesRead::default() });
        let back = parse(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(back.my_list, e.my_list);
        assert_eq!(back.markers, e.markers);
        assert_eq!(back.completed_hidden, e.completed_hidden);
        assert_eq!(back.reading, e.reading);
    }

    #[test]
    fn titles_need_an_id_any_pc_knows() {
        let t = |kind: &str, tmdb, imdb: Option<&str>| Title { kind: kind.into(), title: "X".into(), tmdb, imdb: imdb.map(String::from), ..Title::default() };
        assert!(clean_title(&t("movie", Some(1), None)).is_some());
        assert!(clean_title(&t("movie", None, Some("tt0111161"))).is_some());
        assert!(clean_title(&t("movie", None, None)).is_none(), "a Jellyfin id alone means nothing elsewhere");
        assert!(clean_title(&t("movie", Some(-4), None)).is_none());
        assert!(clean_title(&t("movie", None, Some("tt0111161; drop"))).is_none());
        assert!(clean_title(&t("episode", Some(1), None)).is_none());
    }

    #[test]
    fn a_tile_from_a_file_keeps_only_what_a_card_shows() {
        let t = Tile {
            key: "tmdb:603".into(),
            kind: "movie".into(),
            title: "  The Matrix\u{0007} ".into(),
            poster: Some("javascript:alert(1)".into()),
            backdrop: Some("https://image.tmdb.org/t/p/w780/x.jpg".into()),
            jf_id: Some("abc".into()),
            progress: Some(0.5),
            status: Some("Downloading".into()),
            profile: Some("someone".into()),
            trailer: Some("dQw4w9WgXcQ".into()),
            ..Tile::default()
        };
        let c = clean_tile(&t).unwrap();
        assert_eq!(c.title, "The Matrix");
        assert_eq!(c.poster, None, "only https pictures");
        assert_eq!(c.backdrop.as_deref(), Some("https://image.tmdb.org/t/p/w780/x.jpg"));
        assert_eq!((c.jf_id, c.progress, c.status, c.profile), (None, None, None, None), "nothing of the other PC's");
        assert_eq!(c.trailer.as_deref(), Some("dQw4w9WgXcQ"));
        assert!(clean_tile(&Tile { key: "jf:abc".into(), ..t.clone() }).is_none());
        assert!(clean_tile(&Tile { key: "tmdb:6 03".into(), ..t.clone() }).is_none());
        assert!(clean_tile(&Tile { kind: "episode".into(), ..t.clone() }).is_none());
        assert!(clean_url(Some("https://x.y/a\"b.jpg")).is_none());
        assert!(clean_url(Some("https://x.y/a(b).jpg")).is_none(), "nothing that could break out of a CSS url()");
    }

    #[test]
    fn a_watch_comes_in_quiet() {
        use crate::notify::Source;
        let w = Watch { source: Source::Show(81189), title: "Breaking Bad".into(), poster: None, auto: true, tile: None, seen: Some(50003), works: vec!["/works/OL1W".into()], added: 9 };
        let c = clean_watch(&w).unwrap();
        assert!(!c.auto, "Get it too never comes in on");
        assert_eq!(c.seen, None, "the first look says nothing, so nothing old is announced");
        assert!(c.works.is_empty());
        assert!(clean_watch(&Watch { source: Source::Movie(0), ..w.clone() }).is_none());
        assert!(clean_watch(&Watch { source: Source::Author("   ".into()), ..w }).is_none());
    }

    #[test]
    fn the_watch_log_comes_in_without_files() {
        let w = SeriesWatch {
            title: "Show".into(),
            tvdb: Some(81189),
            episodes: [("1:1".to_string(), 1_700_000_000), ("1:x".to_string(), 1_700_000_000), ("2:1".to_string(), 5)].into_iter().collect(),
            files: [(r"c:\series\show\s01e01.mkv".to_string(), 1_700_000_000)].into_iter().collect(),
            ..SeriesWatch::default()
        };
        let c = clean_series_watch(&w).unwrap();
        assert!(c.files.is_empty(), "the clean-up must never act on another PC's files");
        assert_eq!(c.episodes.keys().cloned().collect::<Vec<_>>(), vec!["1:1".to_string()], "bad keys and impossible times are dropped");
        assert!(clean_series_watch(&SeriesWatch { tvdb: None, ..w }).is_none());
        assert!(clean_movie_watch(&MovieWatch { title: "M".into(), tmdb: None, anime: false, at: 1_700_000_000 }).is_none());
    }

    #[test]
    fn half_watched_needs_a_real_place() {
        let r = Resume { kind: "episode".into(), title: "S".into(), tvdb: Some(1), season: Some(1), episode: Some(2), seconds: 300.0, ..Resume::default() };
        assert!(clean_resume(&r).is_some());
        assert!(clean_resume(&Resume { episode: None, ..r.clone() }).is_none());
        assert!(clean_resume(&Resume { seconds: f64::NAN, ..r.clone() }).is_none());
        assert!(clean_resume(&Resume { seconds: 0.0, ..r.clone() }).is_none());
        assert!(clean_resume(&Resume { kind: "series".into(), ..r }).is_none());
    }

    fn ch(id: i64, number: Option<f64>, name: &str) -> crate::suwayomi::Chapter {
        crate::suwayomi::Chapter { id, name: name.into(), number, scanlator: None, downloaded: true, pages: 10, uploaded: None }
    }

    #[test]
    fn chapters_are_found_by_number_then_name() {
        let here = vec![ch(101, Some(1.0), "Chapter 1"), ch(102, Some(12.5), "Chapter 12.5"), ch(103, None, "Oneshot")];
        assert_eq!(resolve_chapter(&here, &ChapterRef { id: 7, number: Some(12.5), name: String::new() }), Some(102), "another PC's id means nothing");
        assert_eq!(resolve_chapter(&here, &ChapterRef { id: 7, number: Some(13.0), name: "Chapter 12.5".into() }), None, "a number that isn't here isn't guessed by name");
        assert_eq!(resolve_chapter(&here, &ChapterRef { id: 7, number: None, name: "oneshot".into() }), Some(103));
        assert_eq!(resolve_chapter(&here, &ChapterRef { id: 101, number: None, name: "Chapter 1".into() }), Some(101));
        assert_eq!(resolve_chapter(&here, &ChapterRef { id: 101, number: None, name: "Other".into() }), None, "an id whose name changed is another chapter");
    }

    #[test]
    fn a_library_finds_titles_by_their_own_id_first() {
        let items = vec![
            json!({ "Id": "a", "Type": "Movie", "ProviderIds": { "Tmdb": "603", "Imdb": "tt0133093" } }),
            json!({ "Id": "b", "Type": "Movie", "ProviderIds": { "tmdb": "604", "Tvdb": "603" } }),
        ];
        let l = Library::new(&items);
        assert_eq!(l.find(false, Some(603), None, None).and_then(jf_id), Some("a"));
        assert_eq!(l.find(false, None, None, Some("TT0133093")).and_then(jf_id), Some("a"));
        assert_eq!(l.find(true, Some(603), Some(603), None).and_then(jf_id), Some("b"), "a show goes by TVDB first");
        assert_eq!(l.find(false, Some(1), None, None), None);
    }

    #[test]
    fn the_file_says_where_you_are_in_words() {
        let ch = |id, n: Option<f64>| ChapterRef { id, number: n, name: String::new() };
        assert_eq!(where_text(None), "Not started");
        let p = Place { chapter: ch(97, Some(96.0)), page: 2, pages: 15, ..Place::default() };
        assert_eq!(where_text(Some(&p)), "Chapter 96, page 3 of 15");
        let half = Place { chapter: ch(5, Some(12.5)), ..Place::default() };
        assert_eq!(where_text(Some(&half)), "Chapter 12.5");
        let done = Place { chapter: ch(97, Some(96.0)), page: 14, pages: 15, read: vec![(ch(97, Some(96.0)), 1)], ..Place::default() };
        assert_eq!(where_text(Some(&done)), "Finished chapter 96");
    }

    #[test]
    fn continue_from_where_you_were() {
        let ch = |id, n: Option<f64>| ChapterRef { id, number: n, name: String::new() };
        let on = SeriesRead { chapter_now: Some(96.0), last_read: Some(95.0), place: Some(Place { chapter: ch(97, Some(96.0)), ..Place::default() }), ..SeriesRead::default() };
        assert_eq!(waiting_view(&on).continue_from, Some(96.0), "on chapter 96: start there");
        let finished = SeriesRead { chapter_now: Some(96.0), last_read: Some(96.0), place: Some(Place { chapter: ch(97, Some(96.0)), read: vec![(ch(97, Some(96.0)), 1)], ..Place::default() }), ..SeriesRead::default() };
        assert_eq!(waiting_view(&finished).continue_from, Some(97.0), "finished 96: the next one");
        let followed_only = SeriesRead { from: Some(40.0), ..SeriesRead::default() };
        assert_eq!(waiting_view(&followed_only).continue_from, Some(40.0), "never opened: where its downloads started");
        assert_eq!(waiting_view(&SeriesRead::default()).continue_from, None);
    }

    #[test]
    fn the_same_series_in_two_lists() {
        let a = SeriesRead { title: "Solo".into(), anilist: Some(1), ..SeriesRead::default() };
        assert!(same_series(&a, &SeriesRead { title: "Other name".into(), anilist: Some(1), ..SeriesRead::default() }));
        assert!(!same_series(&a, &SeriesRead { title: "Solo".into(), anilist: Some(2), ..SeriesRead::default() }), "two AniList ids are two series");
        assert!(same_series(&SeriesRead { title: "Solo".into(), ..SeriesRead::default() }, &SeriesRead { title: "solo".into(), ..SeriesRead::default() }));
    }

    #[test]
    fn the_file_name_is_safe() {
        let n = file_name("Sam/../..\\x:y");
        assert!(n.starts_with("Chillarr - Sam") && n.ends_with(".json"));
        assert!(!n.contains('/') && !n.contains('\\') && !n.contains(':'));
        assert!(file_name("???").starts_with("Chillarr - Profile - "));
    }
    #[test]
    fn music_hidden_and_settings_go_in_the_file() {
        let mut prefs = BTreeMap::new();
        prefs.insert("watch-style".to_string(), "cinema".to_string());
        prefs.insert("bad key!".to_string(), "x".to_string());
        prefs.insert("big".to_string(), "x".repeat(5000));
        let kept = clean_prefs(&prefs);
        assert_eq!(kept.len(), 1);
        let e = Export {
            format: FORMAT.into(),
            version: VERSION,
            music: vec![crate::music::Song { id: "1".into(), title: "Song".into(), artist: "Band".into(), ..Default::default() }],
            hidden: Some(vec![crate::hidden::Portable { keys: vec!["tmdb:1".into()], label: "A film".into(), item: None }]),
            prefs: kept,
            ..Export::default()
        };
        let back = parse(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(back.music.len(), 1);
        assert_eq!(back.hidden.unwrap()[0].label, "A film");
        assert_eq!(back.prefs.get("watch-style").map(String::as_str), Some("cinema"));
        // An older file, without any of them, still reads.
        let old = parse(&format!(r#"{{"format":"{FORMAT}","version":1}}"#)).unwrap();
        assert!(old.music.is_empty() && old.hidden.is_none() && old.prefs.is_empty());
    }

    #[test]
    fn songs_from_a_file_are_checked() {
        use crate::music::{clean, Song};
        assert!(clean(&Song { id: "12".into(), title: "x".into(), art: "javascript:alert(1)".into(), ..Default::default() }).unwrap().art.is_empty());
        assert!(clean(&Song { id: "../x".into(), title: "x".into(), ..Default::default() }).is_none());
        assert!(clean(&Song { id: "12".into(), title: " ".into(), ..Default::default() }).is_none());
    }
}
