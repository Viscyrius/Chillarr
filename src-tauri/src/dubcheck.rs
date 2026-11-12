//! Dubbed means the dub is really in the file. A release name is only a
//! claim: "[DUAL JAP PT-BR] [SUB PT-BR ENG]" came in as an English dub and
//! held Japanese + Portuguese (11 Mushoku Tensei episodes, measured
//! 2026-11-12 from Sonarr's own scan, `mediaInfo.audioLanguages`).
//!
//! With "Dubbed" chosen, every few hours each file's real audio languages
//! are read from Sonarr and Radarr (they scan every file they import). A file
//! without the dub language gets a search: only when a release that names
//! the dub is found is the old file deleted and that release taken. Nothing
//! found: the file stays and it's tried again a day later, at most 7 times.
//! Profiles don't allow upgrades, so this is the only way a copy is replaced.

use crate::log::log;
use crate::model::{AppId, Level};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Duration;
use tauri::{AppHandle, Manager};

const EVERY: Duration = Duration::from_secs(6 * 3600);
const RETRY_SECS: i64 = 24 * 3600;
const MAX_TRIES: u32 = 7;
/// Searches in one pass, so turning Dubbed on over a big library doesn't
/// flood the indexers: the rest wait for the next pass.
const SEARCHES_PER_PASS: usize = 12;
/// Times one episode or film is replaced, at most: a release that claims the
/// dub but lacks it is never taken twice, and after two it's left alone.
const MAX_REPLACE: u32 = 2;

/// Other ISO 639-2 spellings the scanners use for the same language.
fn same_lang(code: &str, want: &str) -> bool {
    let norm = |c: &str| match c {
        "fra" => "fre",
        "deu" => "ger",
        "chi" | "cmn" | "yue" => "zho",
        other => other,
    }
    .to_string();
    norm(&code.trim().to_ascii_lowercase()) == norm(want)
}

/// True when a scanned file's languages ("jpn/por") are known and lack `want`.
/// Unknown ("" or missing) is never acted on.
/// An untagged track ("und", "mul", "zxx") may be the dub, so it counts as unknown too.
pub fn lacks(audio_languages: &str, want: &str) -> bool {
    let langs: Vec<&str> = audio_languages.split('/').map(str::trim).filter(|l| !l.is_empty()).collect();
    let untagged = langs.iter().any(|l| matches!(l.to_ascii_lowercase().as_str(), "und" | "mul" | "zxx"));
    !langs.is_empty() && !untagged && !langs.iter().any(|l| same_lang(l, want))
}

/// A release from an interactive search that may be taken: it carries the
/// "has a dub" format and is only refused for not being an upgrade (profiles
/// don't allow upgrades, so with the old file there every release says so).
pub fn usable_release(r: &Value) -> bool {
    let named = r["customFormats"].as_array().into_iter().flatten().any(|f| f["name"] == "Chillarr: has a dub");
    let only_upgrade = r["rejections"].as_array().into_iter().flatten().all(|x| {
        let t = x.as_str().map(String::from).unwrap_or_else(|| x["reason"].as_str().unwrap_or_default().to_string()).to_ascii_lowercase();
        t.contains("upgrade") || t.contains("cutoff") || t.contains("existing file")
    });
    named && only_upgrade
}

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
pub struct Tried {
    pub tries: u32,
    /// Unix seconds before which it isn't looked at again.
    pub next: i64,
    /// The file has the dub: never looked at again.
    pub done: bool,
    /// Releases already taken for it (by guid): never taken again.
    #[serde(default)]
    pub grabbed: Vec<String>,
    #[serde(default)]
    pub replaced: u32,
}

/// Whether `guid` may be taken for this episode or film.
pub fn may_grab(t: &Tried, guid: &str) -> bool {
    t.replaced < MAX_REPLACE && !t.grabbed.iter().any(|g| g == guid)
}

/// A release was taken: its new file is looked at again a day later.
pub fn took(t: &mut Tried, guid: &str, now: i64) {
    t.grabbed.push(guid.to_string());
    t.replaced += 1;
    t.tries = 0;
    t.next = now + RETRY_SECS;
    t.done = false;
}

/// Whether a file is due a search now, and the record after the try.
pub fn due(t: Option<&Tried>, now: i64) -> bool {
    match t {
        None => true,
        Some(t) => !t.done && t.tries < MAX_TRIES && t.replaced < MAX_REPLACE && now >= t.next,
    }
}

pub fn after_miss(t: Option<&Tried>, now: i64) -> Tried {
    let mut t = t.cloned().unwrap_or_default();
    t.tries += 1;
    t.next = now + RETRY_SECS;
    t
}

type Book = HashMap<String, Tried>;

fn path(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("dubcheck.json"))
}

fn load(app: &AppHandle) -> Book {
    path(app).and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save(app: &AppHandle, b: &Book) {
    if let (Some(p), Ok(t)) = (path(app), serde_json::to_string(b)) {
        let _ = std::fs::write(p, t);
    }
}

/// One file to look at: what it is, how to search for it and how to remove it.
struct Item {
    key: String,
    name: String,
    langs: String,
    search: String,
    remove: String,
    /// The command that searches for it as missing.
    missing: Value,
}

async fn items(a: &crate::arr::Arr, id: AppId) -> Vec<Item> {
    let mut out = Vec::new();
    match id {
        AppId::Sonarr => {
            let series = a.list("series").await.unwrap_or_default();
            for s in series {
                let (Some(sid), title) = (s["id"].as_i64(), s["title"].as_str().unwrap_or("")) else { continue };
                if s["statistics"]["episodeFileCount"].as_i64() == Some(0) {
                    continue;
                }
                let files = a.list(&format!("episodefile?seriesId={sid}")).await.unwrap_or_default();
                let eps = a.list(&format!("episode?seriesId={sid}")).await.unwrap_or_default();
                for f in files {
                    let Some(fid) = f["id"].as_i64() else { continue };
                    let Some(ep) = eps.iter().find(|e| e["episodeFileId"].as_i64() == Some(fid)) else { continue };
                    let Some(eid) = ep["id"].as_i64() else { continue };
                    out.push(Item {
                        // By episode: a replaced file gets a new id, the episode keeps its own.
                        key: format!("sonarr:ep:{eid}"),
                        name: format!("{title} S{:02}E{:02}", ep["seasonNumber"].as_i64().unwrap_or(0), ep["episodeNumber"].as_i64().unwrap_or(0)),
                        langs: f["mediaInfo"]["audioLanguages"].as_str().unwrap_or("").to_string(),
                        search: format!("release?episodeId={eid}"),
                        remove: format!("episodefile/{fid}"),
                        missing: json!({ "name": "EpisodeSearch", "episodeIds": [eid] }),
                    });
                }
            }
        }
        AppId::Radarr => {
            for m in a.list("movie").await.unwrap_or_default() {
                let f = &m["movieFile"];
                let (Some(mid), Some(fid)) = (m["id"].as_i64(), f["id"].as_i64()) else { continue };
                out.push(Item {
                    key: format!("radarr:movie:{mid}"),
                    name: m["title"].as_str().unwrap_or("").to_string(),
                    langs: f["mediaInfo"]["audioLanguages"].as_str().unwrap_or("").to_string(),
                    search: format!("release?movieId={mid}"),
                    remove: format!("moviefile/{fid}"),
                    missing: json!({ "name": "MoviesSearch", "movieIds": [mid] }),
                });
            }
        }
        _ => {}
    }
    out
}

/// One pass over Sonarr and Radarr. Does nothing unless "Dubbed" is chosen.
pub async fn pass(app: &AppHandle) {
    // One at a time (the timer and a settings change can both start one).
    use std::sync::atomic::{AtomicBool, Ordering};
    static BUSY: AtomicBool = AtomicBool::new(false);
    if BUSY.swap(true, Ordering::AcqRel) {
        return;
    }
    struct Free;
    impl Drop for Free {
        fn drop(&mut self) {
            BUSY.store(false, Ordering::Release);
        }
    }
    let _free = Free;
    let (pref, want) = {
        let st = app.state::<crate::SettingsState>();
        let s = st.current.lock().unwrap();
        (s.audio_pref.clone(), s.audio_lang.clone())
    };
    if pref != "dub" {
        return;
    }
    let mut book = load(app);
    let now = chrono::Utc::now().timestamp();
    let mut searches = 0;
    for id in [AppId::Sonarr, AppId::Radarr] {
        let Some(a) = crate::media::arr(app, id) else { continue };
        for it in items(&a, id).await {
            if !lacks(&it.langs, &want) {
                if let Some(t) = book.get_mut(&it.key) {
                    t.done = true;
                }
                continue;
            }
            if !due(book.get(&it.key), now) || searches >= SEARCHES_PER_PASS {
                continue;
            }
            searches += 1;
            let mut t = book.get(&it.key).cloned().unwrap_or_default();
            let releases: Vec<Value> = a.list(&it.search).await.unwrap_or_default();
            let best = releases
                .iter()
                .filter(|r| usable_release(r) && r["guid"].as_str().is_some_and(|g| may_grab(&t, g)))
                .max_by_key(|r| r["customFormatScore"].as_i64().unwrap_or(0));
            let Some(r) = best else {
                if t.tries == 0 && t.replaced == 0 {
                    log(app, Level::Amber, format!("{} has no {want} voices ({}). No copy with them yet; trying again tomorrow.", it.name, it.langs));
                }
                book.insert(it.key, after_miss(Some(&t), now));
                continue;
            };
            let guid = r["guid"].as_str().unwrap_or_default().to_string();
            // The old file goes first: with upgrades off the new one would be refused next to it.
            if let Err(e) = a.delete(&it.remove).await {
                log(app, Level::Amber, format!("Couldn't remove {} to replace it: {e:?}", it.name));
                book.insert(it.key, after_miss(Some(&t), now));
                continue;
            }
            let grab = json!({ "guid": guid, "indexerId": r["indexerId"] });
            took(&mut t, &guid, now);
            match a.create("release", &grab).await {
                Ok(_) => log(app, Level::Ok, format!("{} had no {want} voices ({}). Getting a copy that has them: {}", it.name, it.langs, r["title"].as_str().unwrap_or(""))),
                Err(e) => {
                    // Never left without a file: it's searched for as missing right away.
                    let searched = a.create("command", &it.missing).await.is_ok();
                    let next = if searched { "Searching for it again now." } else { "Search for it in Sonarr or Radarr." };
                    log(app, Level::Amber, format!("Removed {} but couldn't take the dubbed copy ({e:?}). {next}", it.name));
                }
            }
            book.insert(it.key, t);
        }
    }
    save(app, &book);
}

/// Sets Jellyfin's audio choice and runs a pass a minute after start, then every 6 hours.
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(60)).await;
        sync_jellyfin(&app).await;
        loop {
            pass(&app).await;
            tokio::time::sleep(EVERY).await;
        }
    });
}

/// Jellyfin's own audio choice for a user, from Chillarr's: dubbed asks for
/// the dub language first; original voices leave Jellyfin to the file's
/// default track. True when `cfg` changed. Before this every user had none,
/// so Jellyfin's own pages always played the file's default (Japanese).
/// Original voices only undo what Dubbed set (a user's own choices stay).
pub fn audio_config(cfg: &mut Value, pref: &str, lang: &str) -> bool {
    if pref != "dub" {
        if cfg["AudioLanguagePreference"].is_null() {
            return false;
        }
        cfg["AudioLanguagePreference"] = Value::Null;
        cfg["PlayDefaultAudioTrack"] = json!(true);
        return true;
    }
    let changed = cfg["AudioLanguagePreference"] != json!(lang) || cfg["PlayDefaultAudioTrack"] != json!(false);
    cfg["AudioLanguagePreference"] = json!(lang);
    cfg["PlayDefaultAudioTrack"] = json!(false);
    changed
}

/// Every Jellyfin user gets the audio choice (it's one setting for the house).
pub async fn sync_jellyfin(app: &AppHandle) {
    let (pref, lang) = {
        let st = app.state::<crate::SettingsState>();
        let s = st.current.lock().unwrap();
        (s.audio_pref.clone(), s.audio_lang.clone())
    };
    let Some(j) = crate::media::jf_admin(app) else { return };
    let Ok(users) = j.get_json("/Users").await else { return };
    for u in users.as_array().into_iter().flatten() {
        let Some(uid) = u["Id"].as_str() else { continue };
        let mut cfg = u["Configuration"].clone();
        if !cfg.is_object() || !audio_config(&mut cfg, &pref, &lang) {
            continue;
        }
        if let Err(e) = j.post_json(&format!("/Users/{uid}/Configuration"), &cfg).await {
            log(app, Level::Amber, format!("Couldn't set {}'s audio language in Jellyfin: {e}", u["Name"].as_str().unwrap_or("a profile")));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portuguese_and_japanese_lack_english() {
        assert!(lacks("jpn/por", "eng"));
        assert!(!lacks("jpn/eng", "eng"));
        assert!(!lacks("eng/jpn", "eng"));
        assert!(!lacks("", "eng"), "unknown is left alone");
        assert!(lacks("jpn", "eng"));
        assert!(!lacks("fra / jpn", "fre"), "fra and fre are the same");
    }

    #[test]
    fn only_a_named_dub_refused_for_not_upgrading_is_taken() {
        let ok = json!({ "customFormats": [{ "name": "Chillarr: has a dub" }], "rejections": ["Existing file on disk is of equal or higher preference", "Upgrades are not allowed"] });
        assert!(usable_release(&ok));
        let other = json!({ "customFormats": [{ "name": "Chillarr: maybe a dub" }], "rejections": [] });
        assert!(!usable_release(&other), "an unnamed Dual Audio could be Portuguese again");
        let bad = json!({ "customFormats": [{ "name": "Chillarr: has a dub" }], "rejections": ["Not enough seeders"] });
        assert!(!usable_release(&bad));
    }

    #[test]
    fn failed_once_per_file_then_daily_up_to_seven() {
        let now = 1_000_000;
        assert!(due(None, now));
        let t = after_miss(None, now);
        assert_eq!(t.tries, 1);
        assert!(!due(Some(&t), now + 60), "not again the same day");
        assert!(due(Some(&t), now + RETRY_SECS));
        let mut t = t;
        for _ in 0..6 {
            t = after_miss(Some(&t), t.next);
        }
        assert_eq!(t.tries, 7);
        assert!(!due(Some(&t), t.next + 1), "seven tries, then left alone");
        assert!(!due(Some(&Tried { tries: 1, next: 0, done: true, ..Default::default() }), now));
    }

    #[test]
    fn untagged_audio_is_unknown() {
        assert!(!lacks("jpn/und", "eng"), "an untagged track may be the English one");
        assert!(!lacks("und", "eng"));
        assert!(!lacks("mul", "eng"));
    }

    #[test]
    fn a_release_is_taken_once_and_an_item_twice_at_most() {
        let mut t = Tried::default();
        assert!(may_grab(&t, "guid-a"));
        took(&mut t, "guid-a", 0);
        assert!(!may_grab(&t, "guid-a"), "the same release never again (it may lack the dub too)");
        assert!(may_grab(&t, "guid-b"));
        took(&mut t, "guid-b", 0);
        assert!(!may_grab(&t, "guid-c"), "two replacements, then left alone");
    }

    #[test]
    fn a_replacement_is_judged_again_later_not_never() {
        let mut t = Tried::default();
        took(&mut t, "guid-a", 0);
        assert!(!t.done, "the new file is checked again: it may lack the dub too");
        assert!(!due(Some(&t), 0) && due(Some(&t), RETRY_SECS + 1));
    }

    #[test]
    fn jellyfin_audio_pref_follows_dub() {
        let mut cfg = json!({ "AudioLanguagePreference": null, "PlayDefaultAudioTrack": true });
        assert!(audio_config(&mut cfg, "dub", "eng"));
        assert_eq!(cfg["AudioLanguagePreference"], "eng");
        assert_eq!(cfg["PlayDefaultAudioTrack"], false);
        assert!(!audio_config(&mut cfg, "dub", "eng"), "already so");
        assert!(audio_config(&mut cfg, "sub", "eng"));
        assert_eq!(cfg["AudioLanguagePreference"], Value::Null);
        assert_eq!(cfg["PlayDefaultAudioTrack"], true);
    }
}
