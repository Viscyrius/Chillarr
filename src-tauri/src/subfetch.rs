//! Subtitles per title: which languages each movie or show wants (picked on
//! Download, changed from its detail panel, or the default rule), and a
//! background fetcher that adds them as files land in Jellyfin.
//!
//! Sources, in order: OpenSubtitles.com through Jellyfin's plugin when it's
//! signed in (subs.rs); then two that need no account: OpenSubtitles.org's
//! anonymous REST (movies and episodes, 200 downloads a day per address) and
//! Gestdown (Addic7ed's TV subtitles). Podnapisi was the planned backup, but
//! podnapisi.net has had no address since April 2026 (checked 2026-10-19).
//! Nothing here needs a key: the .com login stays in Jellyfin's plugin.

use crate::jellyfin::Jf;
use crate::log::log;
use crate::media::{arr, jf_admin};
use crate::model::{AppId, Level};
use crate::tiles::Tile;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Manager};

const HOUR: u64 = 3600;
const DAY: u64 = 24 * HOUR;
/// New releases often get subtitles days later; after this, stop looking.
const GIVE_UP_AFTER: u64 = 14 * DAY;
/// A source that couldn't be reached (not one that had nothing) is asked again after this.
const ERROR_RETRY: u64 = HOUR;
/// How often the library is checked when nothing nudges it sooner.
const CYCLE: Duration = Duration::from_secs(3 * 60);
/// Attempts per cycle, so a whole season landing at once is spread out.
const PER_CYCLE: usize = 12;
/// Size caps: a subtitle is well under a megabyte; anything bigger isn't one.
const MAX_DOWNLOAD: u64 = 4 << 20;
const MAX_SUBTITLE: u64 = 8 << 20;
const ORG: &str = "https://rest.opensubtitles.org";
const GESTDOWN: &str = "https://api.gestdown.info";
/// OpenSubtitles.org wants a named client; it's an address, not a key.
const USER_AGENT: &str = concat!("Chillarr v", env!("CARGO_PKG_VERSION"));
/// Subtitle files Jellyfin reads as external tracks.
const SUB_EXTS: [&str; 8] = ["srt", "ass", "ssa", "vtt", "sub", "idx", "sup", "smi"];
const VIDEO_EXTS: [&str; 10] = ["mkv", "mp4", "avi", "m4v", "mov", "ts", "webm", "wmv", "mpg", "m2ts"];

/// A language: ISO 639-2/B (what Jellyfin reports, "eng"), 639-2/T where it
/// differs ("fra" for "fre"), 639-1 ("en"), and its English name, which is
/// what Radarr, Sonarr and Gestdown use.
#[derive(Debug, PartialEq)]
pub struct Lang {
    pub code: &'static str,
    pub alt: &'static str,
    pub two: &'static str,
    pub name: &'static str,
}

const fn l(code: &'static str, alt: &'static str, two: &'static str, name: &'static str) -> Lang {
    Lang { code, alt, two, name }
}

pub const LANGS: &[Lang] = &[
    l("eng", "eng", "en", "English"),
    l("spa", "spa", "es", "Spanish"),
    l("fre", "fra", "fr", "French"),
    l("ger", "deu", "de", "German"),
    l("ita", "ita", "it", "Italian"),
    l("por", "por", "pt", "Portuguese"),
    l("pob", "pob", "pb", "Portuguese (Brazil)"),
    l("dut", "nld", "nl", "Dutch"),
    l("tur", "tur", "tr", "Turkish"),
    l("ara", "ara", "ar", "Arabic"),
    l("per", "fas", "fa", "Persian"),
    l("heb", "heb", "he", "Hebrew"),
    l("hin", "hin", "hi", "Hindi"),
    l("jpn", "jpn", "ja", "Japanese"),
    l("kor", "kor", "ko", "Korean"),
    l("chi", "zho", "zh", "Chinese"),
    l("tha", "tha", "th", "Thai"),
    l("vie", "vie", "vi", "Vietnamese"),
    l("ind", "ind", "id", "Indonesian"),
    l("may", "msa", "ms", "Malay"),
    l("rus", "rus", "ru", "Russian"),
    l("ukr", "ukr", "uk", "Ukrainian"),
    l("pol", "pol", "pl", "Polish"),
    l("cze", "ces", "cs", "Czech"),
    l("slo", "slk", "sk", "Slovak"),
    l("hun", "hun", "hu", "Hungarian"),
    l("rum", "ron", "ro", "Romanian"),
    l("bul", "bul", "bg", "Bulgarian"),
    l("gre", "ell", "el", "Greek"),
    l("scc", "srp", "sr", "Serbian"),
    l("hrv", "hrv", "hr", "Croatian"),
    l("slv", "slv", "sl", "Slovenian"),
    l("swe", "swe", "sv", "Swedish"),
    l("nor", "nor", "no", "Norwegian"),
    l("dan", "dan", "da", "Danish"),
    l("fin", "fin", "fi", "Finnish"),
    l("ice", "isl", "is", "Icelandic"),
    l("kur", "kur", "ku", "Kurdish"),
    l("urd", "urd", "ur", "Urdu"),
    l("ben", "ben", "bn", "Bengali"),
    l("tam", "tam", "ta", "Tamil"),
    l("tel", "tel", "te", "Telugu"),
    l("tgl", "tgl", "tl", "Tagalog"),
];

/// A language by any of its names or codes, ignoring case.
pub fn lang(s: &str) -> Option<&'static Lang> {
    let s = s.trim();
    LANGS.iter().find(|x| [x.code, x.alt, x.two, x.name].iter().any(|c| c.eq_ignore_ascii_case(s)))
}

/// The default rule: English for anything whose original language isn't
/// English, nothing for English titles or ones whose language isn't known.
pub fn default_langs(original: Option<&str>) -> Vec<String> {
    match original.map(str::trim) {
        Some(n) if !n.is_empty() && !n.eq_ignore_ascii_case("unknown") && !n.eq_ignore_ascii_case("any") && lang(n).map(|x| x.code) != Some("eng") => vec!["eng".into()],
        _ => vec![],
    }
}

/// Whether Jellyfin already lists a subtitle in this language, embedded or a
/// file beside it. A forced track only covers foreign bits, so it doesn't count.
pub fn has_language(streams: &[Value], want: &Lang) -> bool {
    streams.iter().any(|s| {
        s["Type"] == "Subtitle" && s["IsForced"] != true && s["Language"].as_str().and_then(lang) == Some(want)
    })
}

/// When to look again after `tries` looks found nothing: 6 hours, then daily,
/// until two weeks after the first. None: stop.
pub fn next_try(first: u64, tries: u32, now: u64) -> Option<u64> {
    let at = now + if tries <= 1 { 6 * HOUR } else { DAY };
    (at <= first + GIVE_UP_AFTER).then_some(at)
}

/// Where a fetched subtitle goes: beside the video, "<video name>.<lang>.<ext>",
/// which Jellyfin picks up as an external track in that language.
pub fn sidecar_path(video: &Path, want: &Lang, ext: &str) -> Option<PathBuf> {
    let ext = ext.to_ascii_lowercase();
    if !SUB_EXTS.contains(&ext.as_str()) {
        return None;
    }
    let stem = video.file_stem()?.to_str()?;
    Some(video.parent()?.join(format!("{stem}.{}.{ext}", want.code)))
}

fn ext_of(name: &str) -> Option<String> {
    Path::new(name).extension()?.to_str().map(|e| e.to_ascii_lowercase())
}

/// The subtitle files in a video's folder that belong to it, out of that
/// folder's file names: "<video name>.<anything>.<subtitle ext>" or
/// "<video name>.<subtitle ext>". One that belongs to a longer-named video in
/// the same folder ("Movie.Part2.mkv" next to "Movie.mkv") is left alone.
pub fn sidecars_of(video: &Path, names: &[String]) -> Vec<String> {
    let Some(stem) = video.file_stem().and_then(|s| s.to_str()) else { return vec![] };
    let low = |s: &str| s.to_lowercase();
    let prefix = format!("{}.", low(stem));
    let others: Vec<String> = names
        .iter()
        .filter(|n| ext_of(n).is_some_and(|e| VIDEO_EXTS.contains(&e.as_str())))
        .filter_map(|n| Path::new(n).file_stem()?.to_str().map(|s| format!("{}.", low(s))))
        .filter(|s| s.len() > prefix.len() && s.starts_with(&prefix))
        .collect();
    names
        .iter()
        .filter(|n| {
            let n = low(n);
            n.starts_with(&prefix) && ext_of(&n).is_some_and(|e| SUB_EXTS.contains(&e.as_str())) && !others.iter().any(|o| n.starts_with(o.as_str()))
        })
        .cloned()
        .collect()
}

/// Words of a release name, for matching a subtitle to the file it was timed to.
fn words(s: &str) -> HashSet<String> {
    s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect()
}

/// How alike two release names are: the words they share.
pub fn release_score(video: &str, sub: &str) -> usize {
    let a = words(video);
    words(sub).iter().filter(|w| a.contains(*w)).count()
}

/// OpenSubtitles.org puts an advert cue or two in its files; those go, and
/// the rest is renumbered. Works on raw bytes so any encoding passes through.
pub fn strip_ads(srt: &[u8]) -> Vec<u8> {
    // Bytes as chars one to one, so nothing is lost whatever the encoding.
    let text: String = srt.iter().map(|&b| b as char).collect();
    let norm = text.replace("\r\n", "\n");
    let blocks: Vec<&str> = norm.split("\n\n").filter(|b| !b.trim().is_empty()).collect();
    let ad = |b: &str| {
        let b = b.to_lowercase();
        b.contains("osdb.link") || b.contains("opensubtitles.") || b.contains("opensubtitles com")
    };
    if !blocks.iter().any(|b| ad(b)) {
        return srt.to_vec();
    }
    let mut out = String::new();
    let mut n = 0;
    for b in blocks.iter().filter(|b| !ad(b)) {
        let mut lines = b.trim_start_matches('\n').lines();
        let first = lines.next().unwrap_or("");
        n += 1;
        // A numbered cue gets its new number; anything else is kept as it was.
        if first.trim().parse::<u32>().is_ok() {
            out.push_str(&format!("{n}\r\n"));
        } else {
            out.push_str(first);
            out.push_str("\r\n");
        }
        for line in lines {
            out.push_str(line);
            out.push_str("\r\n");
        }
        out.push_str("\r\n");
    }
    out.chars().map(|c| c as u32 as u8).collect()
}

/// Unpacks a gzip download, refusing anything over the size cap.
pub fn gunzip(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(bytes).take(MAX_SUBTITLE + 1).read_to_end(&mut out).map_err(|e| format!("not a subtitle file ({e})"))?;
    if out.len() as u64 > MAX_SUBTITLE {
        return Err("too big to be a subtitle".into());
    }
    Ok(out)
}

/// OpenSubtitles' video hash: the size plus the first and last 64 KB as
/// little-endian u64s, wrapping. Finds subtitles timed to this exact file.
pub fn movie_hash(path: &Path) -> Option<(u64, String)> {
    use std::io::{Seek, SeekFrom};
    const CHUNK: u64 = 64 * 1024;
    let mut f = std::fs::File::open(path).ok()?;
    let size = f.metadata().ok()?.len();
    if size < CHUNK * 2 {
        return None;
    }
    let mut sum = size;
    let mut buf = vec![0u8; CHUNK as usize];
    for at in [0, size - CHUNK] {
        f.seek(SeekFrom::Start(at)).ok()?;
        f.read_exact(&mut buf).ok()?;
        for c in buf.chunks_exact(8) {
            sum = sum.wrapping_add(u64::from_le_bytes(c.try_into().ok()?));
        }
    }
    Some((size, format!("{sum:016x}")))
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The start of the next UTC day, when the daily limits reset.
fn tomorrow(now: u64) -> u64 {
    (now / DAY + 1) * DAY
}

/* ---------- What's saved ---------- */

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
struct Want {
    title: String,
    /// Three-letter codes ("eng"); empty is None.
    langs: Vec<String>,
}

/// One subtitle being looked for: a Jellyfin item and a language.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
struct Job {
    /// The title's key ("tmdb:1" / "tvdb:2"), so a delete can drop its jobs.
    key: String,
    path: String,
    label: String,
    lang: String,
    first: u64,
    tries: u32,
    next: u64,
    done: bool,
    gave_up: bool,
}

#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(default)]
struct Saved {
    /// When this first ran: titles added after it get the default rule.
    since: u64,
    titles: BTreeMap<String, Want>,
    /// "<jellyfin id>:<lang>"
    jobs: BTreeMap<String, Job>,
    /// Daily limits: a source isn't asked again before these.
    com_blocked_until: u64,
    org_blocked_until: u64,
}

pub struct SubsWanted {
    path: PathBuf,
    saved: Mutex<Saved>,
    nudge: tokio::sync::Notify,
}

impl SubsWanted {
    pub fn load(dir: PathBuf) -> SubsWanted {
        let path = dir.join("subtitles.json");
        let mut saved: Saved = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let fresh = saved.since == 0;
        if fresh {
            saved.since = now();
        }
        let me = SubsWanted { path, saved: Mutex::new(saved), nudge: tokio::sync::Notify::new() };
        if fresh {
            me.save(&me.saved.lock().unwrap());
        }
        me
    }

    fn save(&self, s: &Saved) {
        if let Ok(text) = serde_json::to_string_pretty(s) {
            let tmp = self.path.with_extension("json.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }

    fn edit<T>(&self, f: impl FnOnce(&mut Saved) -> T) -> T {
        let mut s = self.saved.lock().unwrap();
        let out = f(&mut s);
        self.save(&s);
        out
    }
}

/* ---------- The choice, for the window ---------- */

#[derive(Serialize, Debug)]
pub struct Choice {
    pub key: String,
    pub title: String,
    pub langs: Vec<String>,
    /// Picked by you (not the default rule).
    pub picked: bool,
    pub original: Option<String>,
    /// OpenSubtitles.com is signed in; otherwise only the backups are asked.
    pub signed_in: bool,
    pub languages: Vec<(String, String)>,
}

fn arr_err(e: impl std::fmt::Display) -> String {
    format!("couldn't look it up: {e}")
}

/// The title's key and original language, through Radarr or Sonarr (their
/// lookups work for titles they don't have, too).
async fn resolve(app: &AppHandle, tile: &Tile) -> Result<(String, String, Option<String>), String> {
    let movie = tile.kind == "movie" || tile.format.as_deref() == Some("MOVIE");
    let (mut tmdb, mut tvdb) = (tile.tmdb, tile.tvdb);
    if let (Some(id), Some(j)) = (&tile.jf_id, jf_admin(app).filter(|j| j.user_id.is_some())) {
        if let Ok(v) = j.trailer_info(id).await {
            tmdb = tmdb.or_else(|| v["ProviderIds"]["Tmdb"].as_str()?.parse().ok());
            tvdb = tvdb.or_else(|| v["ProviderIds"]["Tvdb"].as_str()?.parse().ok());
        }
    }
    let original = |v: &Value| v["originalLanguage"]["name"].as_str().map(String::from);
    if movie {
        let a = arr(app, AppId::Radarr).ok_or("Radarr isn't set up")?;
        let v = match tmdb {
            Some(t) => a.get_one(&format!("movie/lookup/tmdb?tmdbId={t}")).await.map_err(arr_err)?,
            None => {
                let found = a.list(&format!("movie/lookup?term={}", urlenc(&tile.title))).await.map_err(arr_err)?;
                crate::library::pick_lookup(&found, tile.year).ok_or_else(|| format!("Radarr couldn't find {}", tile.title))?
            }
        };
        let id = v["tmdbId"].as_i64().or(tmdb).ok_or("Radarr didn't say which movie that is")?;
        Ok((format!("tmdb:{id}"), v["title"].as_str().unwrap_or(&tile.title).to_string(), original(&v)))
    } else {
        let t = Tile { tvdb, ..tile.clone() };
        let (_, v, _, _) = crate::media::series_lookup(app, &t).await?;
        let id = v["tvdbId"].as_i64().or(tvdb).ok_or("Sonarr didn't say which show that is")?;
        Ok((format!("tvdb:{id}"), v["title"].as_str().unwrap_or(&tile.title).to_string(), original(&v)))
    }
}

pub async fn choice(app: &AppHandle, tile: &Tile) -> Result<Choice, String> {
    let (key, title, original) = resolve(app, tile).await?;
    let saved = app.state::<SubsWanted>().saved.lock().unwrap().titles.get(&key).cloned();
    let picked = saved.is_some();
    let langs = saved.map(|w| w.langs).unwrap_or_else(|| default_langs(original.as_deref()));
    Ok(Choice {
        key,
        title,
        langs,
        picked,
        original,
        signed_in: crate::subs::status(app).await.ready,
        languages: LANGS.iter().map(|x| (x.code.to_string(), x.name.to_string())).collect(),
    })
}

/// Saves a title's languages (empty: None) and looks for them at once.
pub fn set(app: &AppHandle, key: &str, title: &str, langs: Vec<String>) -> Result<(), String> {
    if !(key.starts_with("tmdb:") || key.starts_with("tvdb:")) {
        return Err("Chillarr can't tell which title that is.".into());
    }
    let mut clean: Vec<String> = Vec::new();
    for x in &langs {
        let code = lang(x).ok_or_else(|| format!("Chillarr doesn't know the language {x}."))?.code.to_string();
        if !clean.contains(&code) {
            clean.push(code);
        }
    }
    let st = app.state::<SubsWanted>();
    st.edit(|s| {
        // Languages taken off stop being looked for.
        s.jobs.retain(|_, j| j.key != key || j.done || clean.contains(&j.lang));
        s.titles.insert(key.to_string(), Want { title: title.to_string(), langs: clean });
    });
    st.nudge.notify_one();
    Ok(())
}

/* ---------- Deletes ---------- */

/// The title's key, for `forget_title`.
pub fn title_key(movie: bool, tmdb: Option<i64>, tvdb: Option<i64>) -> Option<String> {
    if movie { tmdb.map(|t| format!("tmdb:{t}")) } else { tvdb.map(|t| format!("tvdb:{t}")) }
}

/// A deleted title isn't looked for again (its language choice stays, for a
/// later Download).
pub fn forget_title(app: &AppHandle, key: Option<String>) {
    let Some(key) = key else { return };
    app.state::<SubsWanted>().edit(|s| s.jobs.retain(|_, j| j.key != key));
}

/// Deletes the subtitle files beside a video that's gone, and its jobs.
pub fn drop_sidecars(app: &AppHandle, video: &Path) {
    let Some(dir) = video.parent() else { return };
    let names: Vec<String> = std::fs::read_dir(dir).into_iter().flatten().flatten().filter(|e| e.path().is_file()).filter_map(|e| e.file_name().into_string().ok()).collect();
    for n in sidecars_of(video, &names) {
        if let Err(e) = std::fs::remove_file(dir.join(&n)) {
            log(app, Level::Amber, format!("Couldn't delete the subtitle file {n}: {e}"));
        }
    }
    let p = video.to_string_lossy().to_lowercase();
    app.state::<SubsWanted>().edit(|s| s.jobs.retain(|_, j| j.path.to_lowercase() != p));
}

/// Deletes one Sonarr episode file, then the subtitles beside it (Sonarr
/// only takes the ones it imported itself).
pub async fn delete_episode_file(app: &AppHandle, a: &crate::arr::Arr, file_id: i64) -> bool {
    let path = a.get_one(&format!("episodefile/{file_id}")).await.ok().and_then(|v| v["path"].as_str().map(String::from));
    if a.delete(&format!("episodefile/{file_id}")).await.is_err() {
        return false;
    }
    if let Some(p) = path {
        drop_sidecars(app, Path::new(&p));
    }
    true
}

/// A Jellyfin item's video file, read before Jellyfin deletes it.
pub async fn video_path(j: &Jf, id: &str) -> Option<PathBuf> {
    let uid = j.user_id.as_deref()?;
    let v = j.get_json(&format!("/Items?userId={uid}&ids={id}&fields=Path")).await.ok()?;
    let p = PathBuf::from(v["Items"][0]["Path"].as_str()?);
    ext_of(&p.to_string_lossy()).filter(|e| VIDEO_EXTS.contains(&e.as_str())).map(|_| p)
}

/* ---------- The fetcher ---------- */

/// One video in Jellyfin that wants a subtitle.
struct Item {
    id: String,
    key: String,
    label: String,
    path: PathBuf,
    streams: Vec<Value>,
    imdb: Option<String>,
    /// Episodes: (show's TVDB id, season, episode).
    episode: Option<(i64, i64, i64)>,
}

pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        // Let Jellyfin and the arrs come up first.
        tokio::time::sleep(Duration::from_secs(60)).await;
        loop {
            cycle(&app).await;
            let st = app.state::<SubsWanted>();
            let _ = tokio::time::timeout(CYCLE, st.nudge.notified()).await;
        }
    });
}

/// Titles added to Radarr or Sonarr since this first ran with no choice
/// saved (the Discover shelf, the phone, a Download whose choice wasn't
/// loaded yet) get the default rule.
async fn apply_default_rule(app: &AppHandle) {
    let since = app.state::<SubsWanted>().saved.lock().unwrap().since as i64;
    let added_after = |v: &Value| v["added"].as_str().and_then(|a| chrono::DateTime::parse_from_rfc3339(a).ok()).is_some_and(|d| d.timestamp() > since);
    let mut new: Vec<(String, Want)> = Vec::new();
    for (owner, path, field, prefix) in [(AppId::Radarr, "movie", "tmdbId", "tmdb"), (AppId::Sonarr, "series", "tvdbId", "tvdb")] {
        let Some(a) = arr(app, owner) else { continue };
        for v in a.list(path).await.unwrap_or_default().iter().filter(|v| added_after(v)) {
            let Some(id) = v[field].as_i64() else { continue };
            new.push((format!("{prefix}:{id}"), Want { title: v["title"].as_str().unwrap_or("").to_string(), langs: default_langs(v["originalLanguage"]["name"].as_str()) }));
        }
    }
    if new.is_empty() {
        return;
    }
    let st = app.state::<SubsWanted>();
    let changed = {
        let mut s = st.saved.lock().unwrap();
        let mut changed = false;
        for (k, w) in new {
            if !s.titles.contains_key(&k) {
                s.titles.insert(k, w);
                changed = true;
            }
        }
        changed
    };
    if changed {
        st.save(&st.saved.lock().unwrap());
    }
}

fn is_real_video(path: &str) -> bool {
    let p = Path::new(path);
    ext_of(path).is_some_and(|e| VIDEO_EXTS.contains(&e.as_str())) && p.is_file()
}

/// The library's videos for the titles that want subtitles.
async fn wanted_items(j: &Jf, titles: &BTreeMap<String, Want>) -> Result<Vec<Item>, String> {
    let wanted = |k: &str| titles.get(k).is_some_and(|w| !w.langs.is_empty());
    let id_of = |v: &Value, p: &str| v["ProviderIds"][p].as_str().map(String::from);
    let mut out = Vec::new();
    if titles.iter().any(|(k, w)| k.starts_with("tmdb:") && !w.langs.is_empty()) {
        let v = j.get_json("/Items?Recursive=true&IncludeItemTypes=Movie&Fields=ProviderIds,Path,MediaStreams").await.map_err(|e| format!("{e:?}"))?;
        for m in v["Items"].as_array().into_iter().flatten() {
            let Some(key) = id_of(m, "Tmdb").map(|t| format!("tmdb:{t}")).filter(|k| wanted(k)) else { continue };
            let Some(path) = m["Path"].as_str().filter(|p| is_real_video(p)) else { continue };
            out.push(Item {
                id: m["Id"].as_str().unwrap_or("").to_string(),
                key,
                label: m["Name"].as_str().unwrap_or("").to_string(),
                path: PathBuf::from(path),
                streams: m["MediaStreams"].as_array().cloned().unwrap_or_default(),
                imdb: id_of(m, "Imdb"),
                episode: None,
            });
        }
    }
    if titles.iter().any(|(k, w)| k.starts_with("tvdb:") && !w.langs.is_empty()) {
        let v = j.get_json("/Items?Recursive=true&IncludeItemTypes=Series&Fields=ProviderIds").await.map_err(|e| format!("{e:?}"))?;
        for s in v["Items"].as_array().into_iter().flatten() {
            let Some(tvdb) = id_of(s, "Tvdb").and_then(|t| t.parse::<i64>().ok()) else { continue };
            let key = format!("tvdb:{tvdb}");
            if !wanted(&key) {
                continue;
            }
            let Some(sid) = s["Id"].as_str() else { continue };
            let show = s["Name"].as_str().unwrap_or("");
            let eps = j.get_json(&format!("/Shows/{sid}/Episodes?Fields=ProviderIds,Path,MediaStreams")).await.map_err(|e| format!("{e:?}"))?;
            for e in eps["Items"].as_array().into_iter().flatten() {
                let Some(path) = e["Path"].as_str().filter(|p| is_real_video(p)) else { continue };
                let (Some(sn), Some(en)) = (e["ParentIndexNumber"].as_i64(), e["IndexNumber"].as_i64()) else { continue };
                out.push(Item {
                    id: e["Id"].as_str().unwrap_or("").to_string(),
                    key: key.clone(),
                    label: format!("{show} S{sn:02}E{en:02}"),
                    path: PathBuf::from(path),
                    streams: e["MediaStreams"].as_array().cloned().unwrap_or_default(),
                    imdb: id_of(s, "Imdb"),
                    episode: Some((tvdb, sn, en)),
                });
            }
        }
    }
    Ok(out)
}

async fn cycle(app: &AppHandle) {
    apply_default_rule(app).await;
    let Some(j) = jf_admin(app).filter(|j| j.user_id.is_some()) else { return };
    let st = app.state::<SubsWanted>();
    let titles = st.saved.lock().unwrap().titles.clone();
    let items = match wanted_items(&j, &titles).await {
        Ok(i) => i,
        // Jellyfin down or restarting: the next cycle tries again.
        Err(_) => return,
    };
    let t = now();
    let mut due: Vec<(String, usize, &'static Lang)> = Vec::new();
    {
        let mut s = st.saved.lock().unwrap();
        let live: HashSet<String> = items.iter().flat_map(|i| titles[&i.key].langs.iter().map(move |c| format!("{}:{c}", i.id))).collect();
        // Items gone from the library (deleted, replaced by a better copy) stop being looked for.
        s.jobs.retain(|k, _| live.contains(k));
        for (n, item) in items.iter().enumerate() {
            for code in &titles[&item.key].langs {
                let Some(want) = lang(code) else { continue };
                let k = format!("{}:{code}", item.id);
                let had = has_language(&item.streams, want) || sidecar_path(&item.path, want, "srt").is_some_and(|p| p.exists());
                let job = s.jobs.entry(k.clone()).or_insert_with(|| Job {
                    key: item.key.clone(),
                    path: item.path.to_string_lossy().into_owned(),
                    label: item.label.clone(),
                    lang: code.clone(),
                    first: t,
                    next: t,
                    done: had,
                    ..Default::default()
                });
                if had {
                    job.done = true;
                }
                if !job.done && !job.gave_up && job.next <= t {
                    due.push((k, n, want));
                }
            }
        }
        st.save(&s);
    }
    let signed_in = !due.is_empty() && crate::subs::status(app).await.ready;
    for (k, n, want) in due.into_iter().take(PER_CYCLE) {
        let item = &items[n];
        let outcome = fetch(app, &j, item, want, signed_in).await;
        let t = now();
        let name = item.label.as_str();
        st.edit(|s| {
            let Some(job) = s.jobs.get_mut(&k) else { return };
            match &outcome {
                Outcome::Added(_) => job.done = true,
                Outcome::NotFound => {
                    job.tries += 1;
                    match next_try(job.first, job.tries, t) {
                        Some(at) => job.next = at,
                        None => job.gave_up = true,
                    }
                }
                Outcome::Later(at) => {
                    job.next = *at;
                    if t > job.first + GIVE_UP_AFTER {
                        job.gave_up = true;
                    }
                }
            }
        });
        match outcome {
            Outcome::Added(source) => log(app, Level::Ok, format!("Added {} subtitles to {name} (from {source}).", want.name)),
            _ if st.saved.lock().unwrap().jobs.get(&k).is_some_and(|j| j.gave_up) => {
                log(app, Level::Amber, format!("Couldn't find {} subtitles for {name} in two weeks of looking. Stopped looking.", want.name))
            }
            _ => {}
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

enum Outcome {
    Added(&'static str),
    NotFound,
    /// Couldn't ask (limit used up, a source down): look again then.
    Later(u64),
}

/// A source's answer for one subtitle.
enum Got {
    File(Vec<u8>, String),
    /// Saved by Jellyfin itself (OpenSubtitles.com through the plugin).
    InJellyfin,
    None,
    Blocked(u64),
    Failed,
}

async fn fetch(app: &AppHandle, j: &Jf, item: &Item, want: &'static Lang, signed_in: bool) -> Outcome {
    let st = app.state::<SubsWanted>();
    let t = now();
    let mut later: Option<u64> = None;
    let mut push_later = |at: u64| later = Some(later.map_or(at, |l: u64| l.min(at)));
    let (com_until, org_until) = {
        let s = st.saved.lock().unwrap();
        (s.com_blocked_until, s.org_blocked_until)
    };
    let mut sources: Vec<(&'static str, u8)> = Vec::new();
    if signed_in {
        if com_until <= t { sources.push(("OpenSubtitles", 0)) } else { push_later(com_until) }
    }
    if item.imdb.is_some() {
        if org_until <= t { sources.push(("OpenSubtitles.org", 1)) } else { push_later(org_until) }
    }
    if item.episode.is_some() {
        sources.push(("Addic7ed", 2));
    }
    for (name, which) in sources {
        let got = match which {
            0 => from_com(app, item, want).await,
            1 => from_org(item, want).await,
            _ => from_gestdown(item, want).await,
        };
        match got {
            Got::InJellyfin => return Outcome::Added(name),
            Got::File(bytes, ext) => match write_sidecar(j, item, want, &bytes, &ext).await {
                Ok(()) => return Outcome::Added(name),
                Err(e) => {
                    log(app, Level::Amber, format!("Couldn't save {} subtitles for {}: {e}", want.name, item.label));
                    push_later(t + ERROR_RETRY);
                }
            },
            Got::None => {}
            Got::Blocked(until) => {
                let first = st.edit(|s| {
                    let slot = if which == 0 { &mut s.com_blocked_until } else { &mut s.org_blocked_until };
                    let first = *slot < until;
                    *slot = until;
                    first
                });
                if first {
                    log(app, Level::Amber, format!("{name}'s daily download limit is used up. Using the other sources until tomorrow."));
                }
                push_later(until);
            }
            Got::Failed => push_later(t + ERROR_RETRY),
        }
    }
    match later {
        Some(at) => Outcome::Later(at),
        None => Outcome::NotFound,
    }
}

async fn from_com(app: &AppHandle, item: &Item, want: &Lang) -> Got {
    let Ok(found) = crate::subs::search(app, &item.id, want.code).await else { return Got::Failed };
    // People's own uploads only: a machine translation is worse than none.
    let Some(best) = found.into_iter().find(|f| !f.machine) else { return Got::None };
    match crate::subs::download(app, &item.id, &best.id).await {
        Ok(()) => Got::InJellyfin,
        Err(e) if e == crate::subs::LIMIT => Got::Blocked(tomorrow(now())),
        Err(_) => Got::Failed,
    }
}

fn http() -> reqwest::Client {
    reqwest::Client::builder().user_agent(USER_AGENT).timeout(Duration::from_secs(30)).build().unwrap_or_default()
}

/// Reads a body, refusing anything over the download cap.
async fn capped(resp: reqwest::Response) -> Option<Vec<u8>> {
    if resp.content_length().is_some_and(|n| n > MAX_DOWNLOAD) {
        return None;
    }
    let b = resp.bytes().await.ok()?;
    (b.len() as u64 <= MAX_DOWNLOAD).then(|| b.to_vec())
}

/// Picks the best OpenSubtitles.org result: timed to this file (hash), then
/// the closest release name, then not for the hard of hearing, then the most downloaded.
fn pick_org<'a>(list: &'a [Value], video_name: &str, want: &Lang) -> Option<&'a Value> {
    let s = |v: &Value, k: &str| v[k].as_str().unwrap_or("").to_string();
    list.iter()
        .filter(|v| s(v, "SubLanguageID") == want.code || s(v, "ISO639") == want.two)
        .filter(|v| s(v, "SubAutoTranslation") != "1" && s(v, "SubBad") != "1")
        .filter(|v| SUB_EXTS.contains(&s(v, "SubFormat").to_lowercase().as_str()))
        .max_by_key(|v| {
            let release = format!("{} {}", s(v, "MovieReleaseName"), s(v, "SubFileName"));
            (
                s(v, "MatchedBy") == "moviehash",
                s(v, "SubFormat") == "srt",
                release_score(video_name, &release),
                s(v, "SubHearingImpaired") != "1",
                s(v, "SubDownloadsCnt").parse::<i64>().unwrap_or(0),
            )
        })
}

async fn from_org(item: &Item, want: &Lang) -> Got {
    let Some(imdb) = item.imdb.as_deref().map(|i| i.trim_start_matches("tt").to_string()).filter(|i| !i.is_empty()) else { return Got::None };
    let c = http();
    let lang = format!("sublanguageid-{}", want.code);
    let mut urls = Vec::new();
    if let Some((size, hash)) = movie_hash(&item.path) {
        urls.push(format!("{ORG}/search/moviebytesize-{size}/moviehash-{hash}/{lang}"));
    }
    urls.push(match item.episode {
        Some((_, s, e)) => format!("{ORG}/search/episode-{e}/imdbid-{imdb}/season-{s}/{lang}"),
        None => format!("{ORG}/search/imdbid-{imdb}/{lang}"),
    });
    let mut all: Vec<Value> = Vec::new();
    let mut answered = false;
    for u in urls {
        let Ok(r) = c.get(&u).send().await else { continue };
        if !r.status().is_success() {
            continue;
        }
        answered = true;
        if let Ok(Value::Array(list)) = r.json::<Value>().await {
            // A hash hit for some other title (hash collisions happen) doesn't count.
            all.extend(list.into_iter().filter(|v| {
                let id = v["IDMovieImdb"].as_str().unwrap_or("").trim_start_matches('0');
                let parent = v["SeriesIMDBParent"].as_str().unwrap_or("").trim_start_matches('0');
                let want = imdb.trim_start_matches('0');
                id == want || parent == want
            }));
        }
    }
    if !answered {
        return Got::Failed;
    }
    let name = item.path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let Some(best) = pick_org(&all, name, want) else { return Got::None };
    let Some(link) = best["SubDownloadLink"].as_str().filter(|l| org_link_ok(l)) else { return Got::None };
    // No redirects either: the checked host is the one answering.
    let direct = reqwest::Client::builder().user_agent(USER_AGENT).timeout(Duration::from_secs(30)).redirect(reqwest::redirect::Policy::none()).build().unwrap_or_default();
    let Ok(r) = direct.get(link).send().await else { return Got::Failed };
    let quota = r.headers().get("download-quota").and_then(|v| v.to_str().ok()?.trim().parse::<i64>().ok());
    if r.status().as_u16() == 407 || r.status().as_u16() == 429 || quota.is_some_and(|q| q < 0) {
        return Got::Blocked(tomorrow(now()));
    }
    if !r.status().is_success() {
        return Got::Failed;
    }
    let Some(gz) = capped(r).await else { return Got::Failed };
    match gunzip(&gz) {
        Ok(bytes) => {
            let ext = best["SubFormat"].as_str().unwrap_or("srt").to_lowercase();
            let bytes = if ext == "srt" { strip_ads(&bytes) } else { bytes };
            Got::File(bytes, ext)
        }
        // Over the limit it answers with a page instead of the file.
        Err(_) if quota == Some(0) => Got::Blocked(tomorrow(now())),
        Err(_) => Got::Failed,
    }
}

/// The search answer is data from the network, so its download link is only
/// followed to OpenSubtitles' download host (the only one it gave, measured
/// 2026-10-19), never to a LAN or local address.
fn org_link_ok(link: &str) -> bool {
    reqwest::Url::parse(link).is_ok_and(|u| {
        u.scheme() == "https" && u.host_str() == Some("dl.opensubtitles.org") && u.port().is_none() && u.username().is_empty() && u.password().is_none()
    })
}

async fn from_gestdown(item: &Item, want: &Lang) -> Got {
    let Some((tvdb, s, e)) = item.episode else { return Got::None };
    let c = http();
    let Ok(r) = c.get(format!("{GESTDOWN}/shows/external/tvdb/{tvdb}")).send().await else { return Got::Failed };
    if r.status().as_u16() == 404 {
        return Got::None;
    }
    let Ok(v) = r.json::<Value>().await else { return Got::Failed };
    let Some(show) = v["shows"][0]["id"].as_str() else { return Got::None };
    let Ok(r) = c.get(format!("{GESTDOWN}/subtitles/get/{show}/{s}/{e}/{}", urlenc(want.name))).send().await else { return Got::Failed };
    match r.status().as_u16() {
        404 => return Got::None,
        429 => return Got::Failed,
        c if !(200..300).contains(&c) => return Got::Failed,
        _ => {}
    }
    let Ok(v) = r.json::<Value>().await else { return Got::Failed };
    let name = item.path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let best = v["matchingSubtitles"].as_array().into_iter().flatten().filter(|x| x["completed"] == true).max_by_key(|x| {
        (release_score(name, x["version"].as_str().unwrap_or("")), x["hearingImpaired"] != true, x["downloadCount"].as_i64().unwrap_or(0))
    });
    let Some(uri) = best.and_then(|b| b["downloadUri"].as_str()) else { return Got::None };
    if !uri.starts_with("/subtitles/download/") {
        return Got::None;
    }
    let Ok(r) = c.get(format!("{GESTDOWN}{uri}")).send().await else { return Got::Failed };
    if !r.status().is_success() {
        return Got::Failed;
    }
    match capped(r).await {
        Some(b) if !b.is_empty() => Got::File(b, "srt".into()),
        _ => Got::Failed,
    }
}

/// Saves the file beside the video and has Jellyfin read the folder again.
async fn write_sidecar(j: &Jf, item: &Item, want: &Lang, bytes: &[u8], ext: &str) -> Result<(), String> {
    let path = sidecar_path(&item.path, want, ext).ok_or("not a subtitle format Jellyfin reads")?;
    // Never anywhere but the video's own folder.
    if path.parent() != item.path.parent() {
        return Err("the file name went outside the video's folder".into());
    }
    if path.exists() {
        return Err(format!("{} is already there", path.display()));
    }
    // Timed to some other copy of the film? Fixed before Jellyfin ever sees it.
    let (bytes, _) = crate::subsync::fix_bytes(&item.path, bytes, ext).await;
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    let _ = j
        .post_json(&format!("/Items/{}/Refresh?metadataRefreshMode=Default&imageRefreshMode=None&replaceAllMetadata=false&replaceAllImages=false", item.id), &serde_json::json!({}))
        .await;
    Ok(())
}

fn urlenc(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn org_download_links_only_go_to_opensubtitles() {
        assert!(org_link_ok("https://dl.opensubtitles.org/en/download/src-api/vrf-19d50c5e/filead/1951976249.gz"));
        assert!(org_link_ok("https://dl.opensubtitles.org:443/en/download/x.gz"));
        assert!(!org_link_ok("http://dl.opensubtitles.org/en/download/x.gz"));
        assert!(!org_link_ok("https://127.0.0.1:8096/System/Shutdown"));
        assert!(!org_link_ok("https://192.168.1.1/x"));
        assert!(!org_link_ok("https://dl.opensubtitles.org.evil.example/x"));
        assert!(!org_link_ok("https://dl.opensubtitles.org@127.0.0.1/x"));
        assert!(!org_link_ok("https://dl.opensubtitles.org:8080/x"));
        assert!(!org_link_ok("file:///C:/Windows/win.ini"));
        assert!(!org_link_ok(""));
    }
    use serde_json::json;

    #[test]
    fn english_is_picked_for_anything_not_in_english() {
        assert_eq!(default_langs(Some("Turkish")), vec!["eng"]);
        assert_eq!(default_langs(Some("Japanese")), vec!["eng"]);
        // A language Chillarr has no row for is still not English.
        assert_eq!(default_langs(Some("Malayalam")), vec!["eng"]);
        assert!(default_langs(Some("English")).is_empty());
        assert!(default_langs(Some("Unknown")).is_empty());
        assert!(default_langs(None).is_empty());
    }

    #[test]
    fn languages_match_by_any_code_or_name() {
        assert_eq!(lang("eng").unwrap().name, "English");
        assert_eq!(lang("EN").unwrap().code, "eng");
        assert_eq!(lang("fra").unwrap().code, "fre");
        assert_eq!(lang("deu").unwrap(), lang("German").unwrap());
        assert!(lang("xx").is_none());
    }

    #[test]
    fn already_has_language_counts_embedded_and_external_but_not_forced() {
        let eng = lang("eng").unwrap();
        let streams = vec![json!({ "Type": "Audio", "Language": "eng" }), json!({ "Type": "Subtitle", "Language": "tur" })];
        assert!(!has_language(&streams, eng), "English audio isn't an English subtitle");
        assert!(has_language(&[json!({ "Type": "Subtitle", "Language": "eng", "IsExternal": true })], eng));
        assert!(has_language(&[json!({ "Type": "Subtitle", "Language": "en" })], eng));
        assert!(!has_language(&[json!({ "Type": "Subtitle", "Language": "eng", "IsForced": true })], eng));
        assert!(has_language(&[json!({ "Type": "Subtitle", "Language": "ger" })], lang("deu").unwrap()));
        assert!(!has_language(&[json!({ "Type": "Subtitle" })], eng));
    }

    #[test]
    fn back_off_is_six_hours_then_daily_for_two_weeks() {
        let first = 1_000_000;
        assert_eq!(next_try(first, 1, first), Some(first + 6 * HOUR));
        assert_eq!(next_try(first, 2, first + 6 * HOUR), Some(first + 30 * HOUR));
        assert_eq!(next_try(first, 3, first + 30 * HOUR), Some(first + 54 * HOUR));
        assert_eq!(next_try(first, 9, first + 13 * DAY), Some(first + 14 * DAY));
        assert_eq!(next_try(first, 10, first + 13 * DAY + HOUR), None);
    }

    #[test]
    fn subtitle_goes_beside_the_video_with_its_language() {
        let v = Path::new(r"C:\Movies\The Miracle (2015)\Mucize 2015 1080p WEB-DL x264 [TGx].mkv");
        let p = sidecar_path(v, lang("eng").unwrap(), "SRT").unwrap();
        assert_eq!(p, Path::new(r"C:\Movies\The Miracle (2015)\Mucize 2015 1080p WEB-DL x264 [TGx].eng.srt"));
        assert!(sidecar_path(v, lang("eng").unwrap(), "exe").is_none());
        assert!(sidecar_path(v, lang("eng").unwrap(), "../srt").is_none());
    }

    #[test]
    fn deleting_a_video_takes_only_its_own_subtitles() {
        let names: Vec<String> = [
            "Show S01E01.mkv",
            "Show S01E01.eng.srt",
            "Show S01E01.en.forced.ass",
            "show s01e01.TUR.SRT",
            "Show S01E01.srt",
            "Show S01E01.nfo",
            "Show S01E01-thumb.jpg",
            "Show S01E010.eng.srt",
            "Show S01E01x.srt",
            "Show S01E02.eng.srt",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let mut got = sidecars_of(Path::new(r"D:\TV\Show\Season 01\Show S01E01.mkv"), &names);
        got.sort();
        assert_eq!(got, vec!["Show S01E01.en.forced.ass", "Show S01E01.eng.srt", "Show S01E01.srt", "show s01e01.TUR.SRT"]);
    }

    #[test]
    fn a_longer_named_video_keeps_its_subtitles() {
        let names: Vec<String> = ["Movie.mkv", "Movie.eng.srt", "Movie.Part2.mkv", "Movie.Part2.eng.srt"].iter().map(|s| s.to_string()).collect();
        assert_eq!(sidecars_of(Path::new(r"D:\Movies\Movie.mkv"), &names), vec!["Movie.eng.srt"]);
        assert_eq!(sidecars_of(Path::new(r"D:\Movies\Movie.Part2.mkv"), &names), vec!["Movie.Part2.eng.srt"]);
    }

    #[test]
    fn release_names_rank_by_shared_words() {
        let v = "Mucize 2015 1080p WEB-DL x264 [TGx]";
        let web = release_score(v, "Mucize.2015.1080p.WEB-DL.H.264.DD5.1-LTRG-eng.srt");
        let blu = release_score(v, "Mucize.2015.720p.BluRay.x264.DTS-LTRG.srt");
        assert!(web > blu);
    }

    #[test]
    fn org_pick_prefers_hash_then_release_and_skips_machine() {
        let mk = |name: &str, by: &str, auto: &str, dl: &str| json!({ "SubFileName": name, "MatchedBy": by, "SubAutoTranslation": auto, "SubDownloadsCnt": dl, "SubLanguageID": "eng", "SubFormat": "srt" });
        let list = vec![
            mk("Film.2015.720p.BluRay.srt", "imdbid", "0", "50000"),
            mk("Film.2015.1080p.WEB-DL.srt", "imdbid", "0", "10"),
            mk("Film.2015.1080p.WEB-DL.x264.srt", "imdbid", "1", "99999"),
        ];
        assert_eq!(pick_org(&list, "Film 2015 1080p WEB-DL", lang("eng").unwrap()).unwrap()["SubDownloadsCnt"], "10");
        let mut with_hash = list.clone();
        with_hash.push(mk("whatever.srt", "moviehash", "0", "1"));
        assert_eq!(pick_org(&with_hash, "Film 2015 1080p WEB-DL", lang("eng").unwrap()).unwrap()["SubFileName"], "whatever.srt");
        assert!(pick_org(&list, "x", lang("tur").unwrap()).is_none());
    }

    #[test]
    fn ads_are_stripped_and_cues_renumbered() {
        let srt = b"1\r\n00:00:06,000 --> 00:00:12,074\r\nWatch Online Movies and Series for FREE\r\nwww.osdb.link/lm\r\n\r\n2\r\n00:00:19,251 --> 00:00:22,665\r\nThe Miracle\r\n\r\n3\r\n00:00:23,001 --> 00:00:26,039\r\nCaf\xe9\r\n";
        let out = strip_ads(srt);
        assert_eq!(out, b"1\r\n00:00:19,251 --> 00:00:22,665\r\nThe Miracle\r\n\r\n2\r\n00:00:23,001 --> 00:00:26,039\r\nCaf\xe9\r\n\r\n".to_vec());
        // Nothing to strip: byte for byte the same.
        let clean = b"1\n00:00:01,000 --> 00:00:02,000\nHi\n";
        assert_eq!(strip_ads(clean), clean.to_vec());
    }

    #[test]
    fn gzip_is_capped() {
        use flate2::write::GzEncoder;
        use std::io::Write;
        let pack = |data: &[u8]| {
            let mut e = GzEncoder::new(Vec::new(), flate2::Compression::default());
            e.write_all(data).unwrap();
            e.finish().unwrap()
        };
        assert_eq!(gunzip(&pack(b"1\n00:00:01,000 --> 00:00:02,000\nHi\n")).unwrap().len(), 35);
        // A small bomb: 9 MB of zeros packs to a few KB and is refused.
        assert!(gunzip(&pack(&vec![0u8; 9 << 20])).is_err());
        assert!(gunzip(b"<html>limit</html>").is_err());
    }

    #[test]
    fn movie_hash_sums_the_size_and_both_ends() {
        // Size + every u64 of the first and last 64 KB: all bytes 1 gives
        // 2 * 8192 words of 0x0101010101010101.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("v.mkv");
        std::fs::write(&p, vec![1u8; 200_000]).unwrap();
        let want = 200_000u64.wrapping_add(0x0101010101010101u64.wrapping_mul(16384));
        assert_eq!(movie_hash(&p), Some((200_000, format!("{want:016x}"))));
        std::fs::write(&p, vec![1u8; 1000]).unwrap();
        assert!(movie_hash(&p).is_none());
    }
}
