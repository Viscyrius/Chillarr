//! Music. What there is to play comes from Apple's free iTunes search and
//! charts (no key, artwork and albums included); the sound itself is fetched
//! from YouTube by yt-dlp, which Chillarr installs on first use.
//!
//! Two ways to listen:
//! * **Play**: the song is fetched into a cache folder, played, and deleted
//!   once nobody has touched it for a while (`cache_life`, Settings > Library
//!   & storage; 30 minutes unless changed). Nothing stays.
//! * **Download**: the song is kept in `Music\Chillarr` and listed in the
//!   Library tab; it plays from there with no fetching at all.
//!
//! The browser gets a song through the bridge (`/music/<ticket>`): opening one
//! is a signed-in command that hands out a random ticket, as an `<audio>`
//! element sends no headers. Same scheme as the reading pages (read.rs).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use tauri::{AppHandle, Manager};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// A played (not downloaded) song is deleted after this long untouched.
/// Minutes a played (not downloaded) song is kept untouched before it is
/// deleted. Settings > Library & storage; read from music-settings.json at start.
static CACHE_MINS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(30);
/// The choices Settings offers, in minutes.
pub const CACHE_CHOICES: &[u64] = &[5, 10, 30, 60, 180, 720, 1440];

fn cache_life() -> Duration {
    Duration::from_secs(CACHE_MINS.load(std::sync::atomic::Ordering::Relaxed) * 60)
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Settings {
    cache_mins: Option<u64>,
}

fn settings_file(app: &AppHandle) -> Option<PathBuf> {
    Some(app.path().app_data_dir().ok()?.join("music-settings.json"))
}

/// Reads the saved cache time, at start.
pub fn load_settings(app: &AppHandle) {
    let s: Settings = settings_file(app).and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    if let Some(n) = s.cache_mins.filter(|n| CACHE_CHOICES.contains(n)) {
        CACHE_MINS.store(n, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Settings > Library & storage: how long played songs stay, and the room
/// played and downloaded songs take.
pub fn settings(app: &AppHandle) -> Value {
    let size_of = |d: &Path| -> (u64, usize) {
        std::fs::read_dir(d).map(|rd| rd.flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).fold((0, 0), |(b, n), m| (b + m.len(), n + 1))).unwrap_or((0, 0))
    };
    let (cache_bytes, cache_count) = size_of(&cache_dir());
    let kept = library(app);
    let kept_bytes: u64 = load(app).iter().filter_map(|k| std::fs::metadata(&k.file).ok()).map(|m| m.len()).sum();
    json!({
        "cache_mins": CACHE_MINS.load(std::sync::atomic::Ordering::Relaxed),
        "choices": CACHE_CHOICES,
        "cache_bytes": cache_bytes,
        "cache_count": cache_count,
        "kept_bytes": kept_bytes,
        "kept_count": kept.len(),
        "kept_dir": music_dir().to_string_lossy(),
    })
}

/// Sets how long played songs stay, and sweeps by it at once.
pub fn set_cache_mins(app: &AppHandle, mins: u64) -> Result<(), String> {
    if !CACHE_CHOICES.contains(&mins) {
        return Err("That isn't one of the choices.".into());
    }
    CACHE_MINS.store(mins, std::sync::atomic::Ordering::Relaxed);
    let p = settings_file(app).ok_or("Chillarr isn't ready yet.")?;
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    std::fs::write(&p, serde_json::to_vec(&Settings { cache_mins: Some(mins) }).map_err(|e| e.to_string())?).map_err(|e| format!("Couldn't save it: {e}"))?;
    sweep(false);
    Ok(())
}

/// Deletes every played song now, except the ones playing in the last minute.
pub fn clear_cache() -> usize {
    let Ok(rd) = std::fs::read_dir(cache_dir()) else { return 0 };
    let mut n = 0;
    for e in rd.flatten() {
        let busy = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|a| a < Duration::from_secs(60));
        if !busy && std::fs::remove_file(e.path()).is_ok() {
            n += 1;
        }
    }
    n
}
const TICKET_LIFE: Duration = Duration::from_secs(6 * 3600);
/// Longest one fetch may take before it is given up.
const FETCH_WAIT: Duration = Duration::from_secs(120);
const ROWS_LIFE: Duration = Duration::from_secs(3600);

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Song {
    /// Apple's track id.
    pub id: String,
    pub title: String,
    pub artist: String,
    #[serde(default)]
    pub album: String,
    #[serde(default)]
    pub album_id: String,
    #[serde(default)]
    pub art: String,
    /// Length in seconds, 0 when the chart didn't say.
    #[serde(default)]
    pub secs: u32,
    /// Apple's artist id, for the artist's page and "more like this".
    #[serde(default)]
    pub artist_id: String,
    #[serde(default)]
    pub genre: String,
}

/// A downloaded song: what it is and the file it is in.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Kept {
    #[serde(flatten)]
    song: Song,
    file: PathBuf,
    added: u64,
    /// The profiles that downloaded it (their Jellyfin ids), for each one's
    /// profile file. The Library itself is the whole PC's.
    #[serde(default)]
    by: Vec<String>,
}

// ---------- the catalog ----------

fn http() -> reqwest::Client {
    static C: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    C.get_or_init(|| reqwest::Client::builder().user_agent("Chillarr").timeout(Duration::from_secs(20)).build().expect("http client")).clone()
}

async fn get_json(url: &str) -> Result<Value, String> {
    let r = http().get(url).send().await.map_err(|_| "The music catalog isn't answering.".to_string())?;
    if !r.status().is_success() {
        return Err(format!("The music catalog said {}.", r.status().as_u16()));
    }
    r.json().await.map_err(|_| "The music catalog sent something unreadable.".to_string())
}

/// Apple's artwork at a size worth showing: its URLs end in `100x100bb.jpg`
/// and take any size in its place.
fn big_art(u: &str) -> String {
    match u.rfind('/') {
        Some(i) if u[i..].contains("bb.") => format!("{}/600x600bb.jpg", &u[..i]),
        _ => u.to_string(),
    }
}

/// A song from the search or lookup API.
fn song_of(v: &Value) -> Option<Song> {
    if v["kind"].as_str() != Some("song") {
        return None;
    }
    Some(Song {
        id: v["trackId"].as_u64()?.to_string(),
        title: v["trackName"].as_str()?.to_string(),
        artist: v["artistName"].as_str().unwrap_or_default().to_string(),
        album: v["collectionName"].as_str().unwrap_or_default().to_string(),
        album_id: v["collectionId"].as_u64().map(|n| n.to_string()).unwrap_or_default(),
        art: big_art(v["artworkUrl100"].as_str().unwrap_or_default()),
        secs: (v["trackTimeMillis"].as_u64().unwrap_or(0) / 1000) as u32,
        artist_id: v["artistId"].as_u64().map(|n| n.to_string()).unwrap_or_default(),
        genre: v["primaryGenreName"].as_str().unwrap_or_default().to_string(),
    })
}

fn album_of(v: &Value) -> Option<Value> {
    Some(json!({
        "id": v["collectionId"].as_u64()?.to_string(),
        "title": v["collectionName"].as_str()?,
        "artist": v["artistName"].as_str().unwrap_or_default(),
        "artist_id": v["artistId"].as_u64().map(|n| n.to_string()).unwrap_or_default(),
        "art": big_art(v["artworkUrl100"].as_str().unwrap_or_default()),
        "year": v["releaseDate"].as_str().and_then(|d| d.get(..4)).unwrap_or_default(),
        "count": v["trackCount"].as_u64().unwrap_or(0),
    }))
}

/// The chart feeds (`/rss/topsongs`): a lone entry comes as an object, not a list.
fn entries(v: &Value) -> Vec<Value> {
    match &v["feed"]["entry"] {
        Value::Array(a) => a.clone(),
        Value::Object(_) => vec![v["feed"]["entry"].clone()],
        _ => vec![],
    }
}

fn chart_art(e: &Value) -> String {
    e["im:image"].as_array().and_then(|a| a.last()).and_then(|i| i["label"].as_str()).map(big_art).unwrap_or_default()
}

/// The number at the end of an Apple link (`.../artist/name/12345?uo=2`).
fn id_in(href: Option<&str>) -> String {
    let last = href.and_then(|h| h.split('?').next()).and_then(|h| h.rsplit('/').next()).unwrap_or_default();
    let n = last.trim_start_matches("id");
    if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) { n.to_string() } else { String::new() }
}

fn chart_song(e: &Value) -> Option<Song> {
    let album_id = e["im:collection"]["link"]["attributes"]["href"].as_str().and_then(|h| h.split('?').next()).and_then(|h| h.rsplit('/').next()).unwrap_or_default();
    Some(Song {
        id: e["id"]["attributes"]["im:id"].as_str()?.to_string(),
        title: e["im:name"]["label"].as_str()?.to_string(),
        artist: e["im:artist"]["label"].as_str().unwrap_or_default().to_string(),
        album: e["im:collection"]["im:name"]["label"].as_str().unwrap_or_default().to_string(),
        album_id: album_id.trim_start_matches("id").to_string(),
        art: chart_art(e),
        secs: 0,
        artist_id: id_in(e["im:artist"]["attributes"]["href"].as_str()),
        genre: e["category"]["attributes"]["label"].as_str().unwrap_or_default().to_string(),
    })
}

fn chart_album(e: &Value) -> Option<Value> {
    Some(json!({
        "id": e["id"]["attributes"]["im:id"].as_str()?,
        "title": e["im:name"]["label"].as_str()?,
        "artist": e["im:artist"]["label"].as_str().unwrap_or_default(),
        "artist_id": id_in(e["im:artist"]["attributes"]["href"].as_str()),
        "art": chart_art(e),
        "year": e["im:releaseDate"]["label"].as_str().and_then(|d| d.get(..4)).unwrap_or_default(),
        "count": e["im:itemCount"]["label"].as_str().and_then(|n| n.parse::<u64>().ok()).unwrap_or(0),
    }))
}

/// Home's rows: (id, title, feed).
const ROWS: &[(&str, &str, &str)] = &[
    ("top", "Top songs right now", "topsongs/limit=50"),
    ("albums", "Top albums", "topalbums/limit=30"),
    ("pop", "Pop", "topsongs/limit=30/genre=14"),
    ("hiphop", "Hip-hop and rap", "topsongs/limit=30/genre=18"),
    ("rnb", "R&B and soul", "topsongs/limit=30/genre=15"),
    ("rock", "Rock", "topsongs/limit=30/genre=21"),
    ("alt", "Alternative", "topsongs/limit=30/genre=20"),
    ("dance", "Dance", "topsongs/limit=30/genre=17"),
    ("electronic", "Electronic", "topsongs/limit=30/genre=7"),
    ("country", "Country", "topsongs/limit=30/genre=6"),
    ("latin", "Latin", "topsongs/limit=30/genre=12"),
    ("kpop", "K-Pop", "topsongs/limit=30/genre=51"),
];

static ROWS_CACHE: Mutex<Option<(Instant, Value)>> = Mutex::new(None);

/// Home: the charts, every row asked at once. Kept an hour.
pub async fn rows() -> Value {
    if let Some((at, v)) = ROWS_CACHE.lock().unwrap().as_ref() {
        if at.elapsed() < ROWS_LIFE {
            return v.clone();
        }
    }
    let tasks: Vec<_> = ROWS.iter().map(|(_, _, feed)| tokio::spawn(get_json_owned(format!("https://itunes.apple.com/us/rss/{feed}/json")))).collect();
    let mut out = Vec::new();
    for ((id, title, feed), t) in ROWS.iter().zip(tasks) {
        let Ok(Ok(v)) = t.await else { continue };
        let es = entries(&v);
        if feed.starts_with("topalbums") {
            let items: Vec<Value> = es.iter().filter_map(chart_album).collect();
            if !items.is_empty() {
                out.push(json!({ "id": id, "title": title, "kind": "albums", "items": items }));
            }
        } else {
            let items: Vec<Song> = es.iter().filter_map(chart_song).collect();
            if !items.is_empty() {
                out.push(json!({ "id": id, "title": title, "kind": "songs", "items": items }));
            }
        }
    }
    let v = Value::Array(out);
    if v.as_array().is_some_and(|a| !a.is_empty()) {
        *ROWS_CACHE.lock().unwrap() = Some((Instant::now(), v.clone()));
    }
    v
}

async fn get_json_owned(url: String) -> Result<Value, String> {
    get_json(&url).await
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            b' ' => "+".into(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Search: songs and albums.
pub async fn search(term: &str) -> Result<Value, String> {
    let term = term.trim();
    if term.len() < 2 {
        return Ok(json!({ "songs": [], "albums": [] }));
    }
    let q = enc(term);
    let songs = tokio::spawn(get_json_owned(format!("https://itunes.apple.com/search?term={q}&media=music&entity=song&limit=50")));
    let albums = tokio::spawn(get_json_owned(format!("https://itunes.apple.com/search?term={q}&media=music&entity=album&limit=24")));
    let songs = songs.await.map_err(|e| e.to_string())??;
    let albums = albums.await.ok().and_then(|r| r.ok()).unwrap_or_default();
    let s: Vec<Song> = songs["results"].as_array().into_iter().flatten().filter_map(song_of).collect();
    let a: Vec<Value> = albums["results"].as_array().into_iter().flatten().filter_map(album_of).collect();
    // Artists: the ones the songs are by, most songs first, each with a picture
    // from one of their songs (the catalog has none of its own for artists).
    let mut artists: Vec<(String, String, String, usize)> = Vec::new();
    for x in &s {
        if x.artist_id.is_empty() {
            continue;
        }
        match artists.iter_mut().find(|a| a.0 == x.artist_id) {
            Some(a) => a.3 += 1,
            None => artists.push((x.artist_id.clone(), x.artist.clone(), x.art.clone(), 1)),
        }
    }
    artists.sort_by(|a, b| b.3.cmp(&a.3));
    let ar: Vec<Value> = artists.into_iter().take(10).map(|(id, name, art, _)| json!({ "id": id, "name": name, "art": art })).collect();
    Ok(json!({ "songs": s, "albums": a, "artists": ar }))
}

/// An album and its songs in order.
pub async fn album(id: &str) -> Result<Value, String> {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return Err("That isn't an album.".into());
    }
    let v = get_json(&format!("https://itunes.apple.com/lookup?id={id}&entity=song&limit=200")).await?;
    let res = v["results"].as_array().cloned().unwrap_or_default();
    let head = res.iter().find(|r| r["wrapperType"] == "collection").and_then(album_of).ok_or("That album isn't in the catalog.")?;
    let songs: Vec<Song> = res.iter().filter_map(song_of).collect();
    Ok(json!({ "album": head, "songs": songs }))
}

/// An artist: their best-known songs and their albums.
pub async fn artist(id: &str) -> Result<Value, String> {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return Err("That isn't an artist.".into());
    }
    let songs = tokio::spawn(get_json_owned(format!("https://itunes.apple.com/lookup?id={id}&entity=song&limit=50")));
    let albums = tokio::spawn(get_json_owned(format!("https://itunes.apple.com/lookup?id={id}&entity=album&limit=50")));
    let songs = songs.await.map_err(|e| e.to_string())??;
    let albums = albums.await.ok().and_then(|r| r.ok()).unwrap_or_default();
    let res = songs["results"].as_array().cloned().unwrap_or_default();
    let head = res.iter().find(|r| r["wrapperType"] == "artist").ok_or("That artist isn't in the catalog.")?;
    let mut list: Vec<Song> = Vec::new();
    for s in res.iter().filter_map(song_of) {
        // The same song on a single and an album: once.
        if !list.iter().any(|x| x.title.eq_ignore_ascii_case(&s.title)) {
            list.push(s);
        }
    }
    let mut al: Vec<Value> = albums["results"].as_array().into_iter().flatten().filter_map(album_of).collect();
    al.sort_by(|a, b| b["year"].as_str().cmp(&a["year"].as_str()));
    let art = list.first().map(|s| s.art.clone()).unwrap_or_default();
    Ok(json!({
        "artist": { "id": id, "name": head["artistName"].as_str().unwrap_or_default(), "genre": head["primaryGenreName"].as_str().unwrap_or_default(), "art": art },
        "songs": list,
        "albums": al,
    }))
}

async fn artist_songs(id: String) -> Vec<Song> {
    let Ok(v) = get_json(&format!("https://itunes.apple.com/lookup?id={id}&entity=song&limit=25")).await else { return vec![] };
    let mut out: Vec<Song> = Vec::new();
    for s in v["results"].as_array().into_iter().flatten().filter_map(song_of) {
        if !out.iter().any(|x| x.title.eq_ignore_ascii_case(&s.title)) {
            out.push(s);
        }
    }
    out
}

// ---------- each profile's own: liked songs and what it played ----------

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct Mine {
    /// Newest first.
    pub liked: Vec<Song>,
    /// Played, newest first, each song once.
    pub recent: Vec<Song>,
    /// How many times each song was played (by id).
    pub plays: HashMap<String, u32>,
    /// The profile's own playlists, newest first.
    pub playlists: Vec<Playlist>,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub songs: Vec<Song>,
    pub created: u64,
}

const MAX_PLAYLISTS: usize = 500;
const MAX_IN_PLAYLIST: usize = 5000;

fn now_secs() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn clean_name(n: &str) -> Option<String> {
    let n: String = n.trim().chars().filter(|c| !c.is_control()).take(100).collect();
    (!n.trim().is_empty()).then_some(n)
}

/// Changes one profile's music under its lock, and saves.
fn change<T>(app: &AppHandle, f: impl FnOnce(&mut Mine) -> Result<T, String>) -> Result<T, String> {
    let uid = me(app);
    let _g = MINE_LOCK.lock().unwrap();
    let mut m = mine_of(app, &uid);
    let out = f(&mut m)?;
    save_mine(app, &uid, &m)?;
    FOR_YOU.lock().unwrap().as_mut().map(|c| c.remove(&uid));
    Ok(out)
}

/// A new playlist (with a first song, if one is given). Returns it.
pub fn playlist_new(app: &AppHandle, name: &str, song: Option<Song>) -> Result<Playlist, String> {
    let name = clean_name(name).ok_or("Give it a name.")?;
    let mut b = [0u8; 8];
    getrandom::getrandom(&mut b).map_err(|e| e.to_string())?;
    let p = Playlist { id: b.iter().map(|x| format!("{x:02x}")).collect(), name, songs: song.as_ref().and_then(clean).into_iter().collect(), created: now_secs() };
    change(app, |m| {
        if m.playlists.len() >= MAX_PLAYLISTS {
            return Err("That's a lot of playlists. Delete one first.".into());
        }
        m.playlists.insert(0, p.clone());
        Ok(p)
    })
}

/// Adds a song to a playlist (once). Returns false when it was in it already.
pub fn playlist_add(app: &AppHandle, id: &str, song: Song) -> Result<bool, String> {
    let s = clean(&song).ok_or("That isn't a song.")?;
    change(app, |m| {
        let p = m.playlists.iter_mut().find(|p| p.id == id).ok_or("That playlist is gone.")?;
        if p.songs.iter().any(|x| x.id == s.id) {
            return Ok(false);
        }
        if p.songs.len() >= MAX_IN_PLAYLIST {
            return Err("That playlist is full.".into());
        }
        p.songs.push(s);
        Ok(true)
    })
}

pub fn playlist_remove(app: &AppHandle, id: &str, song_id: &str) -> Result<(), String> {
    change(app, |m| {
        let p = m.playlists.iter_mut().find(|p| p.id == id).ok_or("That playlist is gone.")?;
        p.songs.retain(|x| x.id != song_id);
        Ok(())
    })
}

pub fn playlist_rename(app: &AppHandle, id: &str, name: &str) -> Result<(), String> {
    let name = clean_name(name).ok_or("Give it a name.")?;
    change(app, |m| {
        m.playlists.iter_mut().find(|p| p.id == id).ok_or("That playlist is gone.")?.name = name;
        Ok(())
    })
}

pub fn playlist_delete(app: &AppHandle, id: &str) -> Result<(), String> {
    change(app, |m| {
        m.playlists.retain(|p| p.id != id);
        Ok(())
    })
}

/// Moves a song within a playlist, from one place to another.
pub fn playlist_move(app: &AppHandle, id: &str, from: usize, to: usize) -> Result<(), String> {
    change(app, |m| {
        let p = m.playlists.iter_mut().find(|p| p.id == id).ok_or("That playlist is gone.")?;
        if from >= p.songs.len() || to >= p.songs.len() {
            return Err("That song isn't there.".into());
        }
        let s = p.songs.remove(from);
        p.songs.insert(to, s);
        Ok(())
    })
}

/// A profile file's playlists, added (a name already here gets its songs merged).
pub fn take_playlists(app: &AppHandle, uid: &str, list: &[Playlist]) -> usize {
    let _g = MINE_LOCK.lock().unwrap();
    let mut m = mine_of(app, uid);
    let mut n = 0;
    for p in list.iter().take(MAX_PLAYLISTS) {
        let Some(name) = clean_name(&p.name) else { continue };
        let songs: Vec<Song> = p.songs.iter().filter_map(clean).take(MAX_IN_PLAYLIST).collect();
        match m.playlists.iter_mut().find(|x| x.name == name) {
            Some(x) => {
                for s in songs {
                    if !x.songs.iter().any(|y| y.id == s.id) {
                        x.songs.push(s);
                    }
                }
            }
            None => {
                let id = if p.id.len() <= 32 && !p.id.is_empty() && p.id.bytes().all(|b| b.is_ascii_hexdigit()) { p.id.clone() } else { format!("{:x}", now_secs() + n as u64) };
                m.playlists.push(Playlist { id, name, songs, created: p.created });
                n += 1;
            }
        }
    }
    let _ = save_mine(app, uid, &m);
    n
}

const MAX_RECENT: usize = 300;
const MAX_LIKED: usize = 10_000;
static MINE_LOCK: Mutex<()> = Mutex::new(());

fn mine_file(app: &AppHandle, uid: &str) -> Option<PathBuf> {
    let safe: String = uid.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    Some(app.path().app_data_dir().ok()?.join(format!("music-{}.json", if safe.is_empty() { "pc" } else { &safe })))
}

pub fn mine_of(app: &AppHandle, uid: &str) -> Mine {
    mine_file(app, uid).and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_mine(app: &AppHandle, uid: &str, m: &Mine) -> Result<(), String> {
    let p = mine_file(app, uid).ok_or("Chillarr isn't ready yet.")?;
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(m).map_err(|e| e.to_string())?).and_then(|_| std::fs::rename(&tmp, &p)).map_err(|e| format!("Couldn't save it: {e}"))
}

fn me(app: &AppHandle) -> String {
    crate::media::active_user(app).unwrap_or_default()
}

pub fn mine(app: &AppHandle) -> Mine {
    mine_of(app, &me(app))
}

/// Likes a song, or takes the like off.
pub fn like(app: &AppHandle, s: Song, on: bool) -> Result<bool, String> {
    let s = clean(&s).ok_or("That isn't a song.")?;
    let uid = me(app);
    let _g = MINE_LOCK.lock().unwrap();
    let mut m = mine_of(app, &uid);
    m.liked.retain(|x| x.id != s.id);
    if on {
        m.liked.insert(0, s);
        m.liked.truncate(MAX_LIKED);
    }
    save_mine(app, &uid, &m)?;
    FOR_YOU.lock().unwrap().as_mut().map(|c| c.remove(&uid));
    Ok(on)
}

/// A song started playing: it goes to the top of Recently played.
pub fn played(app: &AppHandle, s: Song) -> Result<(), String> {
    let s = clean(&s).ok_or("That isn't a song.")?;
    let uid = me(app);
    let _g = MINE_LOCK.lock().unwrap();
    let mut m = mine_of(app, &uid);
    m.recent.retain(|x| x.id != s.id);
    *m.plays.entry(s.id.clone()).or_insert(0) += 1;
    m.recent.insert(0, s);
    m.recent.truncate(MAX_RECENT);
    // Counts for songs long gone from the list go too.
    if m.plays.len() > MAX_RECENT * 4 {
        let keep: std::collections::HashSet<String> = m.recent.iter().chain(m.liked.iter()).map(|x| x.id.clone()).collect();
        m.plays.retain(|k, _| keep.contains(k));
    }
    save_mine(app, &uid, &m)?;
    // What you listen to changes what's recommended: the next Home asks again.
    FOR_YOU.lock().unwrap().as_mut().map(|c| c.remove(&uid));
    Ok(())
}

/// A profile file's likes and history, merged into this profile's.
pub fn take_mine(app: &AppHandle, uid: &str, liked: &[Song], recent: &[Song]) -> (usize, usize) {
    let _g = MINE_LOCK.lock().unwrap();
    let mut m = mine_of(app, uid);
    let mut nl = 0;
    for s in liked.iter().filter_map(clean).take(MAX_LIKED) {
        if !m.liked.iter().any(|x| x.id == s.id) {
            m.liked.push(s);
            nl += 1;
        }
    }
    let mut nr = 0;
    for s in recent.iter().filter_map(clean).take(MAX_RECENT) {
        if !m.recent.iter().any(|x| x.id == s.id) {
            m.recent.push(s);
            nr += 1;
        }
    }
    m.recent.truncate(MAX_RECENT);
    let _ = save_mine(app, uid, &m);
    (nl, nr)
}

/// The artists a profile listens to most: (id, name, score). A like counts
/// three plays; recent plays count more than old ones.
fn top_artists(m: &Mine) -> Vec<(String, String, f64)> {
    let mut score: Vec<(String, String, f64)> = Vec::new();
    let mut add = |s: &Song, w: f64| {
        if s.artist_id.is_empty() {
            return;
        }
        match score.iter_mut().find(|a| a.0 == s.artist_id) {
            Some(a) => a.2 += w,
            None => score.push((s.artist_id.clone(), s.artist.clone(), w)),
        }
    };
    for (i, s) in m.recent.iter().enumerate() {
        let plays = *m.plays.get(&s.id).unwrap_or(&1) as f64;
        add(s, plays.min(10.0) * (1.0 / (1.0 + i as f64 / 40.0)));
    }
    for s in &m.liked {
        add(s, 3.0);
    }
    for s in m.playlists.iter().flat_map(|p| p.songs.iter()) {
        add(s, 2.0);
    }
    score.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    score
}

/// The genres a profile listens to, most first.
fn top_genres(m: &Mine) -> Vec<String> {
    let mut g: Vec<(String, usize)> = Vec::new();
    for s in m.recent.iter().chain(m.liked.iter()) {
        if s.genre.is_empty() {
            continue;
        }
        match g.iter_mut().find(|x| x.0 == s.genre) {
            Some(x) => x.1 += 1,
            None => g.push((s.genre.clone(), 1)),
        }
    }
    g.sort_by(|a, b| b.1.cmp(&a.1));
    g.into_iter().map(|x| x.0).collect()
}

/// A cheap, stable shuffle (no rand crate): by a hash of the id and a seed.
fn mix(list: &mut [Song], seed: u64) {
    use std::hash::{Hash, Hasher};
    list.sort_by_cached_key(|s| {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (seed, &s.id).hash(&mut h);
        h.finish()
    });
}

static FOR_YOU: Mutex<Option<HashMap<String, (Instant, Value)>>> = Mutex::new(None);

/// The profile's own rows: Made for you (new songs from the artists it plays,
/// none it has heard), Because you listened to..., and its top artists.
/// Empty when it hasn't played anything yet. Kept 20 minutes.
pub async fn for_you(app: &AppHandle) -> Value {
    let uid = me(app);
    if let Some((at, v)) = FOR_YOU.lock().unwrap().as_ref().and_then(|c| c.get(&uid)) {
        if at.elapsed() < Duration::from_secs(20 * 60) {
            return v.clone();
        }
    }
    let m = mine_of(app, &uid);
    let artists: Vec<_> = top_artists(&m).into_iter().take(6).collect();
    if artists.is_empty() {
        return json!({ "rows": [], "artists": [], "genres": [] });
    }
    let tasks: Vec<_> = artists.iter().map(|a| tokio::spawn(artist_songs(a.0.clone()))).collect();
    let mut per: Vec<Vec<Song>> = Vec::new();
    for t in tasks {
        per.push(t.await.unwrap_or_default());
    }
    let heard: std::collections::HashSet<&str> = m.recent.iter().map(|s| s.id.as_str()).collect();
    let seed = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs() / 3600).unwrap_or(0);
    // Made for you: up to 6 unheard songs from each artist, mixed.
    let mut made: Vec<Song> = per.iter().flat_map(|l| l.iter().filter(|s| !heard.contains(s.id.as_str())).take(6).cloned()).collect();
    mix(&mut made, seed);
    made.truncate(30);
    // The charts in the genres it plays most, for a second mix.
    let genres = top_genres(&m);
    let chart = rows().await;
    let mut discover: Vec<Song> = Vec::new();
    for r in chart.as_array().into_iter().flatten().filter(|r| r["kind"] == "songs") {
        for s in r["items"].as_array().into_iter().flatten().filter_map(|v| serde_json::from_value::<Song>(v.clone()).ok()) {
            if genres.iter().take(3).any(|g| g == &s.genre) && !heard.contains(s.id.as_str()) && !discover.iter().any(|x| x.id == s.id) {
                discover.push(s);
            }
        }
    }
    mix(&mut discover, seed + 1);
    discover.truncate(30);
    let mut rows_out = Vec::new();
    if !made.is_empty() {
        rows_out.push(json!({ "id": "made", "title": "Made for you", "note": "New songs from the artists you play", "items": made }));
    }
    if !discover.is_empty() {
        rows_out.push(json!({ "id": "discover", "title": "Discover", "note": format!("Charting in {}", genres.iter().take(3).cloned().collect::<Vec<_>>().join(", ")), "items": discover }));
    }
    for (a, l) in artists.iter().zip(per.iter()).take(3) {
        if l.len() >= 4 {
            rows_out.push(json!({ "id": format!("because-{}", a.0), "title": format!("Because you listened to {}", a.1), "artist_id": a.0, "items": l }));
        }
    }
    let top: Vec<Value> = artists.iter().zip(per.iter()).map(|(a, l)| json!({ "id": a.0, "name": a.1, "art": l.first().map(|s| s.art.clone()).unwrap_or_default() })).collect();
    let v = json!({ "rows": rows_out, "artists": top, "genres": genres.into_iter().take(6).collect::<Vec<_>>() });
    FOR_YOU.lock().unwrap().get_or_insert_with(HashMap::new).insert(uid, (Instant::now(), v.clone()));
    v
}

/// What plays after the last song of a list runs out: more by the same
/// artist, then the chart in its genre. None of `skip` (what was just played).
pub async fn radio(s: Song, skip: Vec<String>) -> Vec<Song> {
    let mut out: Vec<Song> = Vec::new();
    if !s.artist_id.is_empty() {
        out.extend(artist_songs(s.artist_id.clone()).await.into_iter().take(8));
    }
    let chart = rows().await;
    for r in chart.as_array().into_iter().flatten().filter(|r| r["kind"] == "songs") {
        for x in r["items"].as_array().into_iter().flatten().filter_map(|v| serde_json::from_value::<Song>(v.clone()).ok()) {
            if (!s.genre.is_empty() && x.genre == s.genre) && !out.iter().any(|o| o.id == x.id) {
                out.push(x);
            }
        }
    }
    out.retain(|x| x.id != s.id && !skip.contains(&x.id));
    let seed = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    mix(&mut out, seed);
    out.truncate(25);
    out
}

// ---------- yt-dlp ----------

fn tool_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join("Chillarr").join("yt-dlp")
}

fn ytdlp() -> PathBuf {
    tool_dir().join("yt-dlp.exe")
}

static INSTALL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The SHA-256 the release's own SHA2-256SUMS gives for `name`.
fn sum_for(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        let (h, n) = (it.next()?, it.next()?);
        (n.trim_start_matches('*') == name && h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit())).then(|| h.to_ascii_lowercase())
    })
}

/// Installs yt-dlp from its GitHub release the first time music is played.
/// It is not pinned like FlareSolverr: YouTube changes weekly and an old
/// yt-dlp stops working, so the newest is taken and checked against the hash
/// list published in the same release.
async fn ensure_ytdlp() -> Result<PathBuf, String> {
    let exe = ytdlp();
    if exe.is_file() {
        return Ok(exe);
    }
    let _g = INSTALL.lock().await;
    if exe.is_file() {
        return Ok(exe);
    }
    let base = "https://github.com/yt-dlp/yt-dlp/releases/latest/download";
    let c = reqwest::Client::builder().user_agent("Chillarr").timeout(Duration::from_secs(300)).build().map_err(|e| e.to_string())?;
    let fail = |_| "Couldn't download the music player (yt-dlp). Check the internet connection.".to_string();
    let sums = c.get(format!("{base}/SHA2-256SUMS")).send().await.map_err(fail)?.text().await.map_err(fail)?;
    let want = sum_for(&sums, "yt-dlp.exe").ok_or("yt-dlp's release has no hash for its Windows program, so it wasn't installed.")?;
    let bytes = c.get(format!("{base}/yt-dlp.exe")).send().await.map_err(fail)?.error_for_status().map_err(fail)?.bytes().await.map_err(fail)?;
    use sha2::{Digest, Sha256};
    let got: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    if got != want {
        return Err("The downloaded yt-dlp didn't match its published hash, so it wasn't installed.".into());
    }
    std::fs::create_dir_all(tool_dir()).map_err(|e| e.to_string())?;
    let tmp = exe.with_extension("exe.tmp");
    std::fs::write(&tmp, &bytes).and_then(|_| std::fs::rename(&tmp, &exe)).map_err(|e| format!("Couldn't save yt-dlp: {e}"))?;
    crate::log::detail("Music: installed yt-dlp.".to_string());
    Ok(exe)
}

/// Asks yt-dlp to update itself, once per run, after a fetch failed.
async fn update_once(exe: &Path) {
    static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if DONE.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let mut cmd = tokio::process::Command::new(exe);
    cmd.arg("-U").creation_flags(CREATE_NO_WINDOW).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    let _ = tokio::time::timeout(Duration::from_secs(120), cmd.status()).await;
}

/// What yt-dlp is asked to find: the song's own upload, not a cover or a live take.
fn query_of(s: &Song) -> String {
    let clean = |t: &str| t.chars().filter(|c| !c.is_control() && *c != '"').collect::<String>();
    format!("ytsearch1:{} - {} official audio", clean(&s.artist), clean(&s.title))
}

/// The file a fetched song landed in (`<stem>.<whatever yt-dlp chose>`).
fn landed(dir: &Path, stem: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.filter_map(|e| e.ok()).map(|e| e.path()).find(|p| {
        p.file_stem().and_then(|s| s.to_str()) == Some(stem) && matches!(p.extension().and_then(|e| e.to_str()), Some("m4a" | "webm" | "opus" | "mp3" | "mp4"))
    })
}

async fn run_ytdlp(exe: &Path, s: &Song, dir: &Path, stem: &str) -> Result<(), String> {
    let mut cmd = tokio::process::Command::new(exe);
    cmd.args(["-f", "bestaudio[ext=m4a]/bestaudio", "--no-playlist", "--no-part", "--no-mtime", "--quiet", "--no-warnings", "--max-downloads", "1", "-o"])
        .arg(dir.join(format!("{stem}.%(ext)s")))
        .arg(query_of(s))
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let out = tokio::time::timeout(FETCH_WAIT, cmd.output()).await.map_err(|_| "Fetching the song took too long.".to_string())?.map_err(|e| format!("Couldn't run yt-dlp: {e}"))?;
    // --max-downloads stops with code 101 once it has its one song.
    if landed(dir, stem).is_some() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let last = err.lines().map(str::trim).filter(|l| !l.is_empty()).last().unwrap_or("no details").to_string();
    Err(format!("Couldn't find that song to play ({last})."))
}

/// One fetch per song at a time: a second listener waits for the first.
static FETCHING: Mutex<Option<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = Mutex::new(None);

fn fetch_lock(id: &str) -> Arc<tokio::sync::Mutex<()>> {
    FETCHING.lock().unwrap().get_or_insert_with(HashMap::new).entry(id.to_string()).or_default().clone()
}

/// The song's sound in the cache folder, fetched if it isn't there.
async fn cached(s: &Song) -> Result<PathBuf, String> {
    if s.id.is_empty() || !s.id.bytes().all(|b| b.is_ascii_digit()) || s.title.trim().is_empty() {
        return Err("That isn't a song.".into());
    }
    let dir = cache_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let lock = fetch_lock(&s.id);
    let _g = lock.lock().await;
    if let Some(p) = landed(&dir, &s.id) {
        touch(&p);
        return Ok(p);
    }
    let exe = ensure_ytdlp().await?;
    if let Err(e) = run_ytdlp(&exe, s, &dir, &s.id).await {
        update_once(&exe).await;
        run_ytdlp(&exe, s, &dir, &s.id).await.map_err(|_| e)?;
    }
    landed(&dir, &s.id).ok_or_else(|| "The song didn't arrive.".into())
}

// ---------- the cache: played songs go ----------

fn cache_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join("Chillarr").join("MusicCache")
}

fn touch(p: &Path) {
    if let Ok(f) = std::fs::File::options().write(true).open(p) {
        let _ = f.set_modified(SystemTime::now());
    }
}

/// Deletes every played song nobody has touched for `cache_life()`
/// (`all`: every one, at start). A song still being listened to is touched
/// each time the player asks for more of it, so it stays.
pub fn sweep(all: bool) {
    let Ok(rd) = std::fs::read_dir(cache_dir()) else { return };
    for e in rd.flatten() {
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).map_or(true, |a| a > cache_life());
        if all || old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

// ---------- downloaded songs ----------

fn library_file(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app.path().app_data_dir().map_err(|e| e.to_string())?.join("music.json"))
}

static LIB_LOCK: Mutex<()> = Mutex::new(());

fn load(app: &AppHandle) -> Vec<Kept> {
    library_file(app).ok().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save(app: &AppHandle, list: &[Kept]) -> Result<(), String> {
    let p = library_file(app)?;
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(list).map_err(|e| e.to_string())?).and_then(|_| std::fs::rename(&tmp, &p)).map_err(|e| format!("Couldn't save the music list: {e}"))
}

/// `Music\Chillarr`, where downloaded songs go.
fn music_dir() -> PathBuf {
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    home.join("Music").join("Chillarr")
}

/// A file name Windows takes: no reserved characters, not too long.
fn file_name(s: &Song) -> String {
    let raw = format!("{} - {}", s.artist.trim(), s.title.trim());
    let clean: String = raw.chars().map(|c| if r#"<>:"/\|?*"#.contains(c) || c.is_control() { '_' } else { c }).collect();
    let clean = clean.trim().trim_end_matches('.').to_string();
    let cut: String = clean.chars().take(120).collect();
    if cut.is_empty() { "Song".into() } else { cut }
}

fn kept_file(app: &AppHandle, id: &str) -> Option<PathBuf> {
    load(app).into_iter().find(|k| k.song.id == id).map(|k| k.file).filter(|f| f.is_file())
}

/// Every downloaded song, newest first. One whose file was deleted outside
/// Chillarr is left off.
pub fn library(app: &AppHandle) -> Vec<Song> {
    let _g = LIB_LOCK.lock().unwrap();
    let mut list = load(app);
    list.retain(|k| k.file.is_file());
    list.sort_by(|a, b| b.added.cmp(&a.added));
    list.into_iter().map(|k| k.song).collect()
}

/// Downloads a song for keeping.
pub async fn keep(app: &AppHandle, s: Song) -> Result<Value, String> {
    keep_as(app, s, crate::media::active_user(app).unwrap_or_default()).await
}

/// Downloads a song as `me`'s (a Jellyfin user id; "" for nobody in particular).
async fn keep_as(app: &AppHandle, s: Song, me: String) -> Result<Value, String> {
    if kept_file(app, &s.id).is_some() {
        // Already here: it counts as this profile's too.
        let _g = LIB_LOCK.lock().unwrap();
        let mut list = load(app);
        if let Some(k) = list.iter_mut().find(|k| k.song.id == s.id) {
            if !me.is_empty() && !k.by.contains(&me) {
                k.by.push(me);
                save(app, &list)?;
            }
        }
        return Ok(json!({ "kept": true }));
    }
    let from = cached(&s).await?;
    let dir = music_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't make {}: {e}", dir.display()))?;
    let ext = from.extension().and_then(|e| e.to_str()).unwrap_or("m4a").to_string();
    let mut to = dir.join(format!("{}.{ext}", file_name(&s)));
    let mut n = 2;
    while to.exists() {
        to = dir.join(format!("{} ({n}).{ext}", file_name(&s)));
        n += 1;
    }
    std::fs::copy(&from, &to).map_err(|e| format!("Couldn't save the song: {e}"))?;
    let added = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let _g = LIB_LOCK.lock().unwrap();
    let mut list = load(app);
    list.retain(|k| k.song.id != s.id);
    list.push(Kept { song: s, file: to, added, by: if me.is_empty() { vec![] } else { vec![me] } });
    save(app, &list)?;
    Ok(json!({ "kept": true }))
}

/// The songs a profile downloaded, for its profile file.
pub fn kept_by(app: &AppHandle, uid: &str) -> Vec<Song> {
    let _g = LIB_LOCK.lock().unwrap();
    load(app).into_iter().filter(|k| k.by.iter().any(|b| b == uid)).map(|k| k.song).collect()
}

/// A song from someone else's file: only what a song can be.
pub fn clean(s: &Song) -> Option<Song> {
    let cut = |t: &str, n: usize| t.trim().chars().filter(|c| !c.is_control()).take(n).collect::<String>();
    let digits = |t: &str| !t.is_empty() && t.len() <= 20 && t.bytes().all(|b| b.is_ascii_digit());
    if !digits(&s.id) || s.title.trim().is_empty() {
        return None;
    }
    let art = s.art.trim();
    let art = if art.starts_with("https://") && art.len() <= 400 && !art.chars().any(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '\\')) { art.to_string() } else { String::new() };
    Some(Song {
        id: s.id.clone(),
        title: cut(&s.title, 200),
        artist: cut(&s.artist, 200),
        album: cut(&s.album, 200),
        album_id: if digits(&s.album_id) { s.album_id.clone() } else { String::new() },
        art,
        secs: s.secs.min(24 * 3600),
        artist_id: if digits(&s.artist_id) { s.artist_id.clone() } else { String::new() },
        genre: cut(&s.genre, 60),
    })
}

/// Downloads the songs of an imported file that aren't here yet, one after
/// another in the background, as the importing profile's.
pub fn take(app: &AppHandle, uid: &str, songs: Vec<Song>) -> usize {
    let have: Vec<String> = library(app).into_iter().map(|s| s.id).collect();
    let mut want: Vec<Song> = songs.iter().filter_map(clean).take(5000).collect();
    // Ones already here just count as this profile's.
    {
        let _g = LIB_LOCK.lock().unwrap();
        let mut list = load(app);
        let mut changed = false;
        for k in list.iter_mut().filter(|k| want.iter().any(|w| w.id == k.song.id)) {
            if !k.by.iter().any(|b| b == uid) {
                k.by.push(uid.to_string());
                changed = true;
            }
        }
        if changed {
            let _ = save(app, &list);
        }
    }
    want.retain(|s| !have.contains(&s.id));
    let n = want.len();
    if n > 0 {
        let app = app.clone();
        let uid = uid.to_string();
        tauri::async_runtime::spawn(async move {
            let mut failed = 0;
            for s in want {
                if keep_as(&app, s, uid.clone()).await.is_err() {
                    failed += 1;
                }
            }
            crate::log::detail(format!("Music import: {} downloaded, {failed} not found.", n - failed));
        });
    }
    n
}

/// Deletes a downloaded song's file and takes it off the list.
pub fn forget(app: &AppHandle, id: &str) -> Result<(), String> {
    let _g = LIB_LOCK.lock().unwrap();
    let mut list = load(app);
    if let Some(k) = list.iter().find(|k| k.song.id == id) {
        if k.file.is_file() {
            std::fs::remove_file(&k.file).map_err(|e| format!("Couldn't delete the file: {e}"))?;
        }
    }
    list.retain(|k| k.song.id != id);
    save(app, &list)
}

// ---------- playing: tickets for the bridge ----------

static TICKETS: Mutex<Option<HashMap<String, (PathBuf, Instant)>>> = Mutex::new(None);

fn new_ticket(file: PathBuf) -> String {
    let mut b = [0u8; 16];
    getrandom::getrandom(&mut b).expect("random");
    let t: String = b.iter().map(|x| format!("{x:02x}")).collect();
    let mut m = TICKETS.lock().unwrap();
    let m = m.get_or_insert_with(HashMap::new);
    m.retain(|_, (_, at)| at.elapsed() < TICKET_LIFE);
    m.insert(t.clone(), (file, Instant::now()));
    t
}

/// Gets a song ready and hands back where the player fetches it:
/// `{ path: "/music/<ticket>", kept }`. A downloaded song plays from its file.
pub async fn play(app: &AppHandle, s: Song) -> Result<Value, String> {
    sweep(false);
    if let Some(f) = kept_file(app, &s.id) {
        return Ok(json!({ "path": format!("/music/{}", new_ticket(f)), "kept": true }));
    }
    let f = cached(&s).await?;
    Ok(json!({ "path": format!("/music/{}", new_ticket(f)), "kept": false }))
}

/// The file behind `/music/<ticket>`; a played one is touched so the sweep leaves it.
pub fn file_for(path: &str) -> Option<PathBuf> {
    let t = path.split('?').next()?.strip_prefix("/music/")?;
    if t.len() != 32 || !t.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let f = TICKETS.lock().unwrap().as_ref()?.get(t).filter(|(_, at)| at.elapsed() < TICKET_LIFE).map(|(f, _)| f.clone())?;
    if f.starts_with(cache_dir()) {
        touch(&f);
    }
    f.is_file().then_some(f)
}

pub fn mime(p: &Path) -> &'static str {
    match p.extension().and_then(|e| e.to_str()) {
        Some("webm") => "audio/webm",
        Some("opus") => "audio/ogg",
        Some("mp3") => "audio/mpeg",
        _ => "audio/mp4",
    }
}

/// `Range: bytes=a-b` against a file `len` long: the inclusive span to send.
pub fn range(header: Option<&str>, len: u64) -> Option<(u64, u64)> {
    let spec = header?.trim().strip_prefix("bytes=")?.split(',').next()?.trim();
    let (a, b) = spec.split_once('-')?;
    if len == 0 {
        return None;
    }
    let (start, end) = if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        (len.saturating_sub(n), len - 1)
    } else {
        let s: u64 = a.parse().ok()?;
        let e: u64 = if b.is_empty() { len - 1 } else { b.parse::<u64>().ok()?.min(len - 1) };
        (s, e)
    };
    (start <= end && start < len).then_some((start, end))
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artwork_is_asked_big() {
        assert_eq!(big_art("https://x/a.jpg/100x100bb.jpg"), "https://x/a.jpg/600x600bb.jpg");
        assert_eq!(big_art("https://x/a.jpg/55x55bb.png"), "https://x/a.jpg/600x600bb.jpg");
        assert_eq!(big_art("https://x/other.png"), "https://x/other.png");
    }

    #[test]
    fn ranges() {
        assert_eq!(range(Some("bytes=0-"), 100), Some((0, 99)));
        assert_eq!(range(Some("bytes=10-19"), 100), Some((10, 19)));
        assert_eq!(range(Some("bytes=90-500"), 100), Some((90, 99)));
        assert_eq!(range(Some("bytes=-10"), 100), Some((90, 99)));
        assert_eq!(range(Some("bytes=100-"), 100), None);
        assert_eq!(range(None, 100), None);
        assert_eq!(range(Some("items=0-1"), 100), None);
    }

    #[test]
    fn hash_list_is_read_exactly() {
        let h = "a".repeat(64);
        let sums = format!("{} yt-dlp\n{h}  yt-dlp.exe\n{} yt-dlp_x86.exe\n", "b".repeat(64), "c".repeat(64));
        assert_eq!(sum_for(&sums, "yt-dlp.exe"), Some(h));
        assert_eq!(sum_for(&sums, "nope.exe"), None);
        assert_eq!(sum_for("short yt-dlp.exe", "yt-dlp.exe"), None);
    }

    #[test]
    fn file_names_windows_takes() {
        let s = Song { id: "1".into(), title: "What? / Why: \"Now\"".into(), artist: "A*B".into(), ..Default::default() };
        assert_eq!(file_name(&s), "A_B - What_ _ Why_ _Now_");
    }

    #[test]
    fn tickets_only_open_their_own_file() {
        assert!(file_for("/music/zz").is_none());
        assert!(file_for("/music/../../etc").is_none());
        assert!(file_for(&format!("/music/{}", "0".repeat(32))).is_none());
    }

    #[test]
    fn chart_entries_are_read() {
        let v: Value = serde_json::from_str(r#"{"feed":{"entry":{"im:name":{"label":"Song"},"im:artist":{"label":"Band"},"id":{"attributes":{"im:id":"42"}},
            "im:collection":{"im:name":{"label":"LP"},"link":{"attributes":{"href":"https://music.apple.com/us/album/lp/777?uo=2"}}},
            "im:image":[{"label":"https://x/a.jpg/55x55bb.png"},{"label":"https://x/a.jpg/170x170bb.png"}]}}}"#).unwrap();
        let s = chart_song(&entries(&v)[0]).unwrap();
        assert_eq!((s.id.as_str(), s.title.as_str(), s.artist.as_str(), s.album_id.as_str()), ("42", "Song", "Band", "777"));
        assert_eq!(s.art, "https://x/a.jpg/600x600bb.jpg");
    }

    #[test]
    fn the_artists_played_most_come_first() {
        let s = |id: &str, a: &str| Song { id: id.into(), title: id.into(), artist: a.into(), artist_id: a.into(), genre: "Pop".into(), ..Default::default() };
        let mut m = Mine { recent: vec![s("1", "10"), s("2", "20"), s("3", "20")], liked: vec![s("4", "30")], ..Default::default() };
        m.plays.insert("1".into(), 1);
        let top = top_artists(&m);
        assert_eq!(top[0].0, "30", "a like counts three plays");
        assert_eq!(top[1].0, "20");
        assert_eq!(top_genres(&m), vec!["Pop".to_string()]);
        let mut l = vec![s("a", "1"), s("b", "1"), s("c", "1")];
        let mut l2 = l.clone();
        mix(&mut l, 7);
        mix(&mut l2, 7);
        assert_eq!(l, l2, "the same seed mixes the same way");
    }

    /// Installs yt-dlp and fetches one real song, timed. Needs the internet.
    #[tokio::test]
    #[ignore]
    async fn live_fetch_a_song() {
        let r = search("daft punk one more time").await.unwrap();
        let s: Song = serde_json::from_value(r["songs"][0].clone()).unwrap();
        let t = Instant::now();
        let f = cached(&s).await.unwrap();
        println!("{} - {}: {:.1} s, {} KB, {}", s.artist, s.title, t.elapsed().as_secs_f64(), std::fs::metadata(&f).unwrap().len() / 1024, f.display());
        let t = Instant::now();
        cached(&s).await.unwrap();
        println!("again: {:.3} s", t.elapsed().as_secs_f64());
        assert!(rows().await.as_array().unwrap().len() > 3);
    }
}
