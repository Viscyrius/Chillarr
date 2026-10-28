//! Getting part of a show (whole seasons or single episodes) and finding a
//! trailer for anything that has one.

use crate::library;
use crate::log::log;
use crate::media::{follow, http, jf, series_lookup, Media};
use crate::model::{AppId, Level};
use crate::tiles::Tile;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::Duration;
use tauri::{AppHandle, Manager};

/// Sonarr's own metadata server (keyless): every episode of a show by its TVDB id.
pub(crate) const SKYHOOK: &str = "https://skyhook.sonarr.tv/v1/tvdb/shows/en";
/// KinoCheck: trailers by TMDB id, keyless (1000 calls a day, measured 2026-10-17).
const KINOCHECK: &str = "https://api.kinocheck.com";

#[derive(Serialize, Clone, Debug)]
pub struct Episode {
    pub number: i64,
    pub title: String,
    pub air_date: Option<String>,
    pub aired: bool,
    /// Sonarr already has the file.
    pub has_file: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct Season {
    pub number: i64,
    pub episodes: Vec<Episode>,
}

#[derive(Serialize, Debug)]
pub struct Seasons {
    pub title: String,
    pub seasons: Vec<Season>,
    /// The season a later-season anime entry ("X Season 3") points at.
    pub suggested: Option<i64>,
}

/// What to get: whole seasons, plus single episodes as (season, episode).
#[derive(Deserialize, Debug, Default)]
pub struct Pick {
    #[serde(default)]
    pub seasons: Vec<i64>,
    #[serde(default)]
    pub episodes: Vec<(i64, i64)>,
}

/// Wants every season and missing episode of a show in Sonarr, specials left
/// out (as Sonarr's own "All episodes" does).
async fn whole_show(a: &crate::arr::Arr, id: i64) -> Result<(), String> {
    let mut s = a.get_one(&format!("series/{id}")).await.map_err(|e| e.to_string())?;
    s["monitored"] = json!(true);
    s["monitorNewItems"] = json!("all");
    for season in s["seasons"].as_array_mut().into_iter().flatten() {
        if season["seasonNumber"].as_i64().unwrap_or(0) > 0 {
            season["monitored"] = json!(true);
        }
    }
    a.update(&format!("series/{id}"), &s).await.map_err(|e| e.to_string())?;
    let eps = a.list(&format!("episode?seriesId={id}")).await.map_err(|e| e.to_string())?;
    let missing = missing_regular(&eps);
    if !missing.is_empty() {
        a.update("episode/monitor", &json!({ "episodeIds": missing, "monitored": true })).await.map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Sonarr's and Radarr's "unmonitor deleted" switches, on. They're off by
/// default, so a file deleted outside Chillarr (by hand, by another tool) is
/// wanted again and downloads again: Mushoku Tensei S1 E1-12 sat in Sonarr's
/// missing list after being deleted (measured 2026-10-19). On, what you
/// delete stays gone until you ask for it.
pub async fn keep_deleted_gone(app: &AppHandle) {
    for (id, key) in [(AppId::Sonarr, "autoUnmonitorPreviouslyDownloadedEpisodes"), (AppId::Radarr, "autoUnmonitorPreviouslyDownloadedMovies")] {
        let Some(a) = crate::media::arr(app, id) else { continue };
        let Ok(mut cfg) = a.get_one("config/mediamanagement").await else { continue };
        if cfg[key] == true {
            continue;
        }
        cfg[key] = json!(true);
        match a.update("config/mediamanagement", &cfg).await {
            Ok(()) => log(app, Level::Ok, format!("{id:?} no longer downloads again what you deleted.")),
            Err(e) => log(app, Level::Amber, format!("Couldn't stop {id:?} downloading deleted files again: {e:?}")),
        }
    }
}

/// A disc copy: a remux or a whole Blu-ray. 25 to 80 GB for one film, for a
/// picture that looks the same on a TV as a 3 to 5 GB copy. Godzilla (26 GB)
/// and Replicas (31 GB) came in like that because "HD-1080p" allows remuxes
/// (2026-10-19).
fn disc_copy(quality: &str) -> bool {
    quality.contains("Remux") || quality.contains("BR-DISK") || quality.contains("Raw-HD")
}

/// Takes disc copies out of a quality profile. Returns true when it changed.
/// The cutoff (the quality it stops upgrading at) must stay an allowed one,
/// so it moves to the best quality still allowed.
pub(crate) fn drop_disc_copies(p: &mut Value) -> bool {
    let mut changed = false;
    let mut best = None;
    let Some(items) = p["items"].as_array_mut() else { return false };
    for i in items.iter_mut() {
        let is_group = i["items"].as_array().is_some_and(|x| !x.is_empty());
        if is_group {
            for x in i["items"].as_array_mut().into_iter().flatten() {
                if x["allowed"] == true && x["quality"]["name"].as_str().is_some_and(disc_copy) {
                    x["allowed"] = json!(false);
                    changed = true;
                }
            }
            let any = i["items"].as_array().is_some_and(|x| x.iter().any(|q| q["allowed"] == true));
            if i["allowed"] == true && !any {
                i["allowed"] = json!(false);
                changed = true;
            }
            if i["allowed"] == true {
                best = i["id"].as_i64().or(best);
            }
        } else {
            if i["allowed"] == true && i["quality"]["name"].as_str().is_some_and(disc_copy) {
                i["allowed"] = json!(false);
                changed = true;
            }
            if i["allowed"] == true {
                best = i["quality"]["id"].as_i64().or(best);
            }
        }
    }
    if changed {
        if let Some(b) = best {
            p["cutoff"] = json!(b);
        }
    }
    changed
}

/// Puts remuxes back (Settings switch off): a resolution's remux is allowed
/// again wherever a normal copy of that resolution is. Whole discs stay out;
/// nothing plays them. Returns true when it changed.
pub(crate) fn allow_remux(p: &mut Value) -> bool {
    fn names(i: &Value) -> Vec<(String, bool)> {
        match i["items"].as_array().filter(|x| !x.is_empty()) {
            Some(g) => g.iter().map(|x| (x["quality"]["name"].as_str().unwrap_or_default().to_string(), x["allowed"] == true)).collect(),
            None => vec![(i["quality"]["name"].as_str().unwrap_or_default().to_string(), i["allowed"] == true)],
        }
    }
    let allowed: Vec<String> = p["items"].as_array().into_iter().flatten().flat_map(names).filter(|(n, on)| *on && !disc_copy(n)).map(|(n, _)| n).collect();
    let wants = |name: &str| ["1080p", "2160p", "720p"].iter().any(|r| name == format!("Remux-{r}") && allowed.iter().any(|n| n.ends_with(r)));
    let mut changed = false;
    for i in p["items"].as_array_mut().into_iter().flatten() {
        if i["items"].as_array().is_some_and(|x| !x.is_empty()) {
            continue;
        }
        if i["allowed"] != true && i["quality"]["name"].as_str().is_some_and(|n| wants(n)) {
            i["allowed"] = json!(true);
            changed = true;
        }
    }
    changed
}

/// Every Radarr and Sonarr quality profile without disc copies (the Settings
/// switch, on by default), so a Download, Watch now or a TV request never
/// brings in a 30 GB file. Turned off, remuxes are allowed again. Films
/// already here stay either way.
pub async fn no_disc_copies(app: &AppHandle) {
    let skip = app.state::<crate::SettingsState>().current.lock().unwrap().skip_remux;
    if skip {
        movie_sizes(app).await;
    }
    for id in [AppId::Radarr, AppId::Sonarr] {
        let Some(a) = crate::media::arr(app, id) else { continue };
        let Ok(profiles) = a.list("qualityprofile").await else { continue };
        for mut p in profiles {
            let changed = if skip { drop_disc_copies(&mut p) } else { allow_remux(&mut p) };
            if !changed {
                continue;
            }
            if !skip {
                let name = p["name"].as_str().unwrap_or("a quality").to_string();
                if let Some(pid) = p["id"].as_i64() {
                    match a.update(&format!("qualityprofile/{pid}"), &p).await {
                        Ok(()) => log(app, Level::Ok, format!("{id:?}'s {name} setting takes full-disc copies (remuxes) again.")),
                        Err(e) => log(app, Level::Amber, format!("Couldn't change {id:?}'s {name} setting: {e:?}")),
                    }
                }
                continue;
            }
            let (Some(pid), name) = (p["id"].as_i64(), p["name"].as_str().unwrap_or("a quality").to_string()) else { continue };
            match a.update(&format!("qualityprofile/{pid}"), &p).await {
                Ok(()) => log(app, Level::Ok, format!("{id:?}'s {name} setting no longer downloads full-disc copies (25 GB and up).")),
                Err(e) => log(app, Level::Amber, format!("Couldn't change {id:?}'s {name} setting: {e:?}")),
            }
        }
    }
}

/// Dubs that get added to other films' releases ("Dual Audio Hindi English"),
/// as release-name words. Japanese and English aren't here: anime's dual
/// audio is Japanese + English and is usually wanted; so is an English track
/// on a foreign film. Short tags that collide with group names (PL, POR) are
/// left out.
const DUB_WORDS: &str = r"(hindi|tamil|telugu|malayalam|kannada|bengali|ita|italian|french|truefrench|vff|vfq|german|ger|spanish|latino|castellano|portuguese|russian|rus|ukrainian|ukr|polish|czech|hungarian|turkish|thai|vietnamese|persian)";
const DUB_FORMAT: &str = "Chillarr: extra dub";
/// Enough to take a release below any profile's minimum score.
const DUB_SCORE: i64 = -10000;

/// The custom format: the release NAME mentions a dub AND its languages
/// include one besides the film's own. The two are different kinds of
/// condition on purpose: Radarr and Sonarr group conditions by kind, and a
/// required one makes the rest of its group not count (measured 2026-10-19:
/// with every condition a language one, plain English Godzilla matched). So
/// Godzilla "Dual Audio Hindi English" matches, a French film in French
/// doesn't, anime in Japanese + English doesn't. `_options` (the app's
/// language list) isn't needed for this shape; kept for the caller.
pub(crate) fn dub_format(_options: &[Value]) -> Value {
    json!({ "name": DUB_FORMAT, "includeCustomFormatWhenRenaming": false, "specifications": [
        { "name": "A dub in the name", "implementation": "ReleaseTitleSpecification", "negate": false, "required": true,
          "fields": [{ "name": "value", "value": format!("(?i){DUB_WORDS}") }] },
        { "name": "Not only the film's own language", "implementation": "LanguageSpecification", "negate": false, "required": true,
          "fields": [{ "name": "value", "value": -2 }, { "name": "exceptLanguage", "value": true }] }
    ]})
}

/// Gives a profile the dub format at `score`. True when it changed.
pub(crate) fn score_format(p: &mut Value, format: i64, score: i64) -> bool {
    if !p["formatItems"].is_array() {
        p["formatItems"] = json!([]);
    }
    let items = p["formatItems"].as_array_mut().unwrap();
    match items.iter_mut().find(|i| i["format"].as_i64() == Some(format)) {
        Some(i) if i["score"].as_i64() == Some(score) => false,
        Some(i) => {
            i["score"] = json!(score);
            true
        }
        None => {
            items.push(json!({ "format": format, "name": DUB_FORMAT, "score": score }));
            true
        }
    }
}

/// The "Skip dubbed copies" switch (on by default) in Radarr and Sonarr: the
/// dub format exists and every quality profile scores it far below its
/// minimum, so such a release is never taken. Off, its score is 0.
pub async fn no_dubs(app: &AppHandle) {
    let skip = app.state::<crate::SettingsState>().current.lock().unwrap().skip_dubs;
    for id in [AppId::Radarr, AppId::Sonarr] {
        let Some(a) = crate::media::arr(app, id) else { continue };
        let Ok(schema) = a.list("customformat/schema").await else { continue };
        let options: Vec<Value> = schema
            .iter()
            .find(|s| s["implementation"] == "LanguageSpecification")
            .and_then(|s| s["fields"].as_array()?.iter().find(|f| f["name"] == "value")?["selectOptions"].as_array().cloned())
            .unwrap_or_default();
        if options.is_empty() {
            continue;
        }
        let want = dub_format(&options);
        let existing = a.list("customformat").await.unwrap_or_default().into_iter().find(|c| c["name"] == DUB_FORMAT);
        let format = match existing {
            Some(mut c) => {
                let fid = c["id"].as_i64();
                let names = |v: &Value| v["specifications"].as_array().map(|s| s.iter().map(|x| (x["implementation"].clone(), x["fields"].clone())).collect::<Vec<_>>());
                if names(&c) != names(&want) {
                    c["specifications"] = want["specifications"].clone();
                    if let Some(f) = fid {
                        let _ = a.update(&format!("customformat/{f}"), &c).await;
                    }
                }
                fid
            }
            None => match a.create("customformat", &want).await {
                Ok(v) => v["id"].as_i64(),
                Err(e) => {
                    log(app, Level::Amber, format!("Couldn't set up dub skipping in {id:?}: {e:?}"));
                    None
                }
            },
        };
        let Some(format) = format else { continue };
        let score = if skip { DUB_SCORE } else { 0 };
        for mut p in a.list("qualityprofile").await.unwrap_or_default() {
            if !score_format(&mut p, format, score) {
                continue;
            }
            let (Some(pid), name) = (p["id"].as_i64(), p["name"].as_str().unwrap_or("a quality").to_string()) else { continue };
            if let Err(e) = a.update(&format!("qualityprofile/{pid}"), &p).await {
                log(app, Level::Amber, format!("Couldn't change {id:?}'s {name} setting for dubs: {e:?}"));
            }
        }
    }
}

/// Size limits for movies, in MB per minute of film (Radarr's unit), as
/// (preferred, max). Radarr ships with no max on Blu-ray and 4K, so a 1080p
/// "normal" copy of Godzilla came in at 17 GB (2026-10-19). 70 MB/min is about
/// 8.4 GB for two hours. Sonarr keeps its own limits: it judges a season pack
/// per episode, and good 1080p packs run 3 to 7 GB an episode.
pub(crate) fn movie_size_cap(quality: &str) -> Option<(f64, f64)> {
    if disc_copy(quality) {
        return None;
    }
    [("2160", (120.0, 170.0)), ("1080", (45.0, 70.0)), ("720", (25.0, 40.0))].iter().find(|(r, _)| quality.contains(r)).map(|(_, c)| *c)
}

/// Radarr's movie size limits, only ever tightened (with Skip full-disc
/// copies on): a limit already lower is left alone.
async fn movie_sizes(app: &AppHandle) {
    let Some(a) = crate::media::arr(app, AppId::Radarr) else { return };
    let Ok(defs) = a.list("qualitydefinition").await else { return };
    let mut changed = 0;
    for mut d in defs {
        let Some((pref, max)) = d["quality"]["name"].as_str().and_then(movie_size_cap) else { continue };
        if d["maxSize"].as_f64().is_some_and(|m| m <= max) {
            continue;
        }
        d["preferredSize"] = json!(pref);
        d["maxSize"] = json!(max);
        // Radarr's bulk save answers 202 and changes nothing; one at a time works (measured).
        if let Some(id) = d["id"].as_i64() {
            if a.update(&format!("qualitydefinition/{id}"), &d).await.is_ok() {
                changed += 1;
            }
        }
    }
    if changed > 0 {
        log(app, Level::Ok, "Radarr now skips oversized movie copies (about 8 GB for a two-hour film in 1080p).");
    }
}

/// Episodes without a file, specials left out.
fn missing_regular(eps: &[Value]) -> Vec<i64> {
    eps.iter().filter(|e| e["hasFile"] != true && e["seasonNumber"].as_i64().unwrap_or(0) > 0).filter_map(|e| e["id"].as_i64()).collect()
}

fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// Episodes grouped into seasons, specials last.
fn group(eps: impl Iterator<Item = (i64, Episode)>) -> Vec<Season> {
    let mut by: BTreeMap<i64, Vec<Episode>> = BTreeMap::new();
    for (s, e) in eps {
        by.entry(s).or_default().push(e);
    }
    let mut out: Vec<Season> = by.into_iter().map(|(number, mut episodes)| {
        episodes.sort_by_key(|e| e.number);
        Season { number, episodes }
    }).collect();
    out.sort_by_key(|s| if s.number == 0 { i64::MAX } else { s.number });
    out
}

/// The seasons and episodes of a show, from Sonarr when it already has it
/// (so finished episodes show as such), otherwise from Sonarr's metadata server.
pub async fn seasons(app: &AppHandle, tile: &Tile) -> Result<Seasons, String> {
    let (a, lookup, _, season_entry) = series_lookup(app, tile).await?;
    let title = lookup["title"].as_str().unwrap_or(&tile.title).to_string();
    let now = today();
    let aired = |d: &Option<String>| d.as_deref().is_some_and(|d| d <= now.as_str());
    let list: Vec<Season> = match lookup["id"].as_i64().filter(|i| *i > 0) {
        Some(id) => {
            let eps = a.list(&format!("episode?seriesId={id}")).await.map_err(|e| format!("Sonarr couldn't list the episodes: {e}"))?;
            group(eps.iter().map(|e| {
                let air_date = e["airDate"].as_str().map(String::from);
                (e["seasonNumber"].as_i64().unwrap_or(0), Episode {
                    number: e["episodeNumber"].as_i64().unwrap_or(0),
                    title: e["title"].as_str().unwrap_or_default().to_string(),
                    aired: aired(&air_date),
                    air_date,
                    has_file: e["hasFile"] == true,
                })
            }))
        }
        None => {
            let tvdb = lookup["tvdbId"].as_i64().ok_or("Sonarr has no TVDB id for this show")?;
            let show: Value = http().get(format!("{SKYHOOK}/{tvdb}")).send().await.map_err(|e| e.to_string())?.json().await.map_err(|e| e.to_string())?;
            group(show["episodes"].as_array().into_iter().flatten().map(|e| {
                let air_date = e["airDate"].as_str().map(String::from);
                (e["seasonNumber"].as_i64().unwrap_or(0), Episode {
                    number: e["episodeNumber"].as_i64().unwrap_or(0),
                    title: e["title"].as_str().unwrap_or_default().to_string(),
                    aired: aired(&air_date),
                    air_date,
                    has_file: false,
                })
            }))
        }
    };
    let suggested = season_entry.then(|| list.iter().filter(|s| s.number > 0).map(|s| s.number).max()).flatten();
    Ok(Seasons { title, seasons: list, suggested })
}

/// "Season 1 and 3", "Season 2, 4 episodes", "5 episodes".
fn describe(pick: &Pick) -> String {
    let seasons = match pick.seasons.as_slice() {
        [] => String::new(),
        [one] => format!("season {one}"),
        many => {
            let (last, rest) = many.split_last().unwrap();
            format!("seasons {} and {last}", rest.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(", "))
        }
    };
    let eps = match pick.episodes.len() {
        0 => String::new(),
        1 => "1 episode".into(),
        n => format!("{n} episodes"),
    };
    match (seasons.is_empty(), eps.is_empty()) {
        (false, false) => format!("{seasons} and {eps}"),
        (false, true) => seasons,
        _ => eps,
    }
}

/// Gets only what was picked. A show Sonarr doesn't have yet is added with
/// nothing monitored first, so it doesn't start on everything.
pub async fn get_picked(app: &AppHandle, tile: Tile, quality: &str, pick: Pick) -> Result<String, String> {
    let whole = pick.seasons.is_empty() && pick.episodes.is_empty();
    let (a, lookup, anime, _) = series_lookup(app, &tile).await?;
    if whole {
        // A new show: added whole. One already in Sonarr (got through the picker
        // before, so only some seasons are wanted) has every season and missing
        // episode marked first, since SeriesSearch only looks for wanted ones.
        let Some(id) = lookup["id"].as_i64().filter(|i| *i > 0) else {
            let added = crate::media::add(app, &tile, quality).await?;
            // Anime is added with only its first season wanted; Whole show means all of it.
            if anime && added.id > 0 {
                for _ in 0..30 {
                    if !a.list(&format!("episode?seriesId={}", added.id)).await.unwrap_or_default().is_empty() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                whole_show(&a, added.id).await.map_err(|e| format!("Added it, but Sonarr couldn't mark every season: {e}"))?;
                let _ = a.command_with(json!({ "name": "SeriesSearch", "seriesId": added.id })).await;
            }
            follow(app.clone(), added.app, added.id, tile.key.clone(), tile.title.clone());
            return Ok(added.msg);
        };
        let title = lookup["title"].as_str().unwrap_or(&tile.title).to_string();
        whole_show(&a, id).await.map_err(|e| format!("Sonarr couldn't mark {title}: {e}"))?;
        let _ = a.command_with(json!({ "name": "SeriesSearch", "seriesId": id })).await;
        let msg = format!("Getting the rest of {title}.");
        log(app, Level::Ok, &msg);
        follow(app.clone(), AppId::Sonarr, id, tile.key.clone(), title);
        return Ok(msg);
    }
    let title = lookup["title"].as_str().unwrap_or(&tile.title).to_string();
    // A show already in Sonarr but switched off was stopped: what it wanted
    // then is void, so only the pick is wanted once it's switched back on.
    let mut was_off = false;
    let id = match lookup["id"].as_i64().filter(|i| *i > 0) {
        Some(id) => {
            let series = a.get_one(&format!("series/{id}")).await.map_err(|e| format!("Sonarr couldn't read {title}: {e}"))?;
            was_off = series["monitored"] == false;
            id
        }
        None => {
            let root = library::pick_root(&a.list("rootfolder").await.unwrap_or_default(), anime).ok_or("Sonarr has no series folder yet. Add one in Settings > Library & storage.")?;
            let profile = library::profile_for(&a.list("qualityprofile").await.unwrap_or_default(), quality).ok_or("Sonarr has no quality profile")?;
            let mut payload = library::series_payload(lookup, profile, &root, anime);
            payload["addOptions"] = json!({ "searchForMissingEpisodes": false, "monitor": "none" });
            // Only what was picked, now and later: new seasons aren't fetched by themselves.
            payload["monitorNewItems"] = json!("none");
            let made = a.create("series", &payload).await.map_err(|e| format!("Sonarr couldn't add {title}: {e}"))?;
            made["id"].as_i64().ok_or_else(|| format!("Sonarr couldn't add {title}"))?
        }
    };
    // A show just added has no episodes until Sonarr has read its metadata.
    let mut eps: Vec<Value> = Vec::new();
    for _ in 0..30 {
        eps = a.list(&format!("episode?seriesId={id}")).await.unwrap_or_default();
        if !eps.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    if eps.is_empty() {
        return Err(format!("Sonarr added {title} but hasn't listed its episodes yet. Try again in a minute."));
    }
    let chosen: Vec<i64> = eps
        .iter()
        .filter(|e| {
            let (s, n) = (e["seasonNumber"].as_i64().unwrap_or(-1), e["episodeNumber"].as_i64().unwrap_or(-1));
            pick.seasons.contains(&s) || pick.episodes.contains(&(s, n))
        })
        .filter_map(|e| e["id"].as_i64())
        .collect();
    if chosen.is_empty() {
        return Err("None of those episodes are listed in Sonarr.".into());
    }
    // Old wants go first, while the show is still off, so nothing else is
    // searched for or grabbed from the feed in between.
    if was_off {
        let stale = stale_wants(&eps, &pick);
        if !stale.is_empty() {
            a.update("episode/monitor", &json!({ "episodeIds": stale, "monitored": false })).await.map_err(|e| format!("Sonarr couldn't drop what {title} wanted before: {e}"))?;
        }
    }
    // Whole seasons stay monitored, so episodes that haven't aired yet come too.
    if !pick.seasons.is_empty() {
        let mut series = a.get_one(&format!("series/{id}")).await.map_err(|e| e.to_string())?;
        series["monitored"] = json!(true);
        if let Some(list) = series["seasons"].as_array_mut() {
            for s in list.iter_mut().filter(|s| s["seasonNumber"].as_i64().is_some_and(|n| pick.seasons.contains(&n))) {
                s["monitored"] = json!(true);
            }
        }
        a.update(&format!("series/{id}"), &series).await.map_err(|e| format!("Sonarr couldn't save the seasons: {e}"))?;
    }
    a.update("episode/monitor", &json!({ "episodeIds": chosen, "monitored": true })).await.map_err(|e| format!("Sonarr couldn't mark the episodes: {e}"))?;
    // Switched on, so the search schedule and the feed keep looking for the
    // pick; seasons that come out later aren't wanted by themselves.
    if was_off {
        let mut series = a.get_one(&format!("series/{id}")).await.map_err(|e| e.to_string())?;
        series["monitored"] = json!(true);
        series["monitorNewItems"] = json!("none");
        a.update(&format!("series/{id}"), &series).await.map_err(|e| format!("Sonarr couldn't switch {title} back on: {e}"))?;
    }
    for s in &pick.seasons {
        let _ = a.command_with(json!({ "name": "SeasonSearch", "seriesId": id, "seasonNumber": s })).await;
    }
    let singles: Vec<i64> = eps
        .iter()
        .filter(|e| {
            let s = e["seasonNumber"].as_i64().unwrap_or(-1);
            !pick.seasons.contains(&s) && pick.episodes.contains(&(s, e["episodeNumber"].as_i64().unwrap_or(-1)))
        })
        .filter_map(|e| e["id"].as_i64())
        .collect();
    if !singles.is_empty() {
        let _ = a.command_with(json!({ "name": "EpisodeSearch", "episodeIds": singles })).await;
    }
    let msg = format!("Getting {} of {title}.", describe(&pick));
    log(app, Level::Ok, &msg);
    follow(app.clone(), AppId::Sonarr, id, tile.key.clone(), title);
    Ok(msg)
}

/// Episodes a switched-off show still marks as wanted that the pick
/// doesn't hold: no file, monitored, not in a picked season or episode.
fn stale_wants(eps: &[Value], pick: &Pick) -> Vec<i64> {
    eps.iter()
        .filter(|e| e["hasFile"] != true && e["monitored"] == true)
        .filter(|e| {
            let (s, n) = (e["seasonNumber"].as_i64().unwrap_or(-1), e["episodeNumber"].as_i64().unwrap_or(-1));
            !pick.seasons.contains(&s) && !pick.episodes.contains(&(s, n))
        })
        .filter_map(|e| e["id"].as_i64())
        .collect()
}

/// A YouTube id out of a watch/embed/short link.
fn youtube_id(url: &str) -> Option<String> {
    let id = if let Some((_, rest)) = url.split_once("v=") {
        rest
    } else if let Some((_, rest)) = url.split_once("youtu.be/") {
        rest
    } else {
        url.split_once("/embed/")?.1
    };
    let id: String = id.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    (id.len() >= 6).then_some(id)
}

async fn kinocheck(kind: &str, tmdb: i64) -> Option<String> {
    let v: Value = http().get(format!("{KINOCHECK}/{kind}?tmdb_id={tmdb}&language=en")).send().await.ok()?.json().await.ok()?;
    v["trailer"]["youtube_video_id"].as_str().or_else(|| v["videos"][0]["youtube_video_id"].as_str()).map(String::from)
}

async fn show_tmdb(tvdb: i64) -> Option<i64> {
    let v: Value = http().get(format!("{SKYHOOK}/{tvdb}")).send().await.ok()?.json().await.ok()?;
    v["tmdbId"].as_i64()
}

async fn anilist_trailer(id: i64) -> Option<String> {
    let q = json!({ "query": format!("{{ Media(id: {id}) {{ trailer {{ id site }} }} }}") });
    let v: Value = http().post("https://graphql.anilist.co").json(&q).send().await.ok()?.json().await.ok()?;
    (v["data"]["Media"]["trailer"]["site"] == "youtube").then(|| v["data"]["Media"]["trailer"]["id"].as_str().map(String::from)).flatten()
}

/// The trailer's YouTube id, if anything knows one. Answers are remembered
/// for the session, a miss included.
pub async fn trailer(app: &AppHandle, tile: &Tile) -> Option<String> {
    if tile.trailer.is_some() {
        return tile.trailer.clone();
    }
    if let Some(hit) = app.state::<Media>().trailers.lock().unwrap().get(&tile.key) {
        return hit.clone();
    }
    let (mut tmdb, mut tvdb) = (tile.tmdb, tile.tvdb);
    let mut found = None;
    if let Some(id) = &tile.jf_id {
        if let Some(j) = jf(app).filter(|j| j.user_id.is_some()) {
            if let Ok(v) = j.trailer_info(id).await {
                found = v["RemoteTrailers"].as_array().into_iter().flatten().find_map(|t| youtube_id(t["Url"].as_str()?));
                tmdb = tmdb.or_else(|| v["ProviderIds"]["Tmdb"].as_str()?.parse().ok());
                tvdb = tvdb.or_else(|| v["ProviderIds"]["Tvdb"].as_str()?.parse().ok());
            }
        }
    }
    let is_show = matches!(tile.kind.as_str(), "series" | "episode") || (tile.kind == "anime" && tile.format.as_deref() != Some("MOVIE"));
    if found.is_none() {
        if let Some(id) = tile.key.strip_prefix("anilist:").and_then(|s| s.parse().ok()) {
            found = anilist_trailer(id).await;
        }
    }
    if found.is_none() && is_show {
        if tmdb.is_none() || tile.kind == "series" {
            if let Some(tv) = tvdb {
                tmdb = show_tmdb(tv).await.or(tmdb);
            }
        }
        if let Some(t) = tmdb {
            found = kinocheck("shows", t).await;
        }
    } else if found.is_none() {
        if let Some(t) = tmdb {
            found = kinocheck("movies", t).await;
        }
    }
    app.state::<Media>().trailers.lock().unwrap().insert(tile.key.clone(), found.clone());
    found
}

// ---------- Every episode of a show, and where you're at ----------

/// One episode as the detail panel lists it.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct EpisodeRow {
    pub season: i64,
    pub episode: i64,
    pub title: String,
    pub overview: Option<String>,
    pub still: Option<String>,
    pub jf_id: Option<String>,
    /// Finished, by Jellyfin or by this profile's watch log (deleted since).
    pub played: bool,
    /// How far into it, while it's partly watched.
    pub progress: Option<f64>,
    /// "downloaded" | "downloading" | "half" | "missing"
    pub state: String,
    /// Downloading: how far, 0..1.
    pub pct: Option<f64>,
    pub air_date: Option<String>,
    pub aired: bool,
    /// Jellyfin's tile, to play it.
    pub tile: Option<Tile>,
    /// A Watch now copy (half watched or downloading): its key, so the row resumes it.
    pub watch_key: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct ShowEpisodes {
    pub seasons: Vec<i64>,
    pub spot: Option<crate::marker::Spot>,
    pub episodes: Vec<EpisodeRow>,
    pub tvdb: Option<i64>,
    /// The show itself, for Download (the panel may have been opened from an episode).
    pub show: Option<Tile>,
}

/// An episode from Sonarr or its metadata server (same field names).
#[derive(Clone, Debug, Default)]
pub struct BaseEp {
    pub season: i64,
    pub episode: i64,
    pub title: String,
    pub overview: Option<String>,
    pub still: Option<String>,
    pub air_date: Option<String>,
}

fn base_ep(v: &Value) -> Option<BaseEp> {
    let still = v["images"].as_array().and_then(|i| i.iter().find(|x| x["coverType"] == "screenshot")).and_then(|x| x["remoteUrl"].as_str()).or(v["image"].as_str()).map(String::from);
    Some(BaseEp {
        season: v["seasonNumber"].as_i64()?,
        episode: v["episodeNumber"].as_i64()?,
        title: v["title"].as_str().unwrap_or_default().to_string(),
        overview: v["overview"].as_str().filter(|s| !s.is_empty()).map(String::from),
        still,
        air_date: v["airDate"].as_str().map(String::from),
    })
}

/// Sonarr's (or skyhook's) episodes joined with Jellyfin's by season and
/// episode number. A Jellyfin episode the list doesn't have still gets a row.
/// Specials go last.
pub fn build_rows(
    base: Vec<BaseEp>,
    jf_eps: &[Value],
    jf_base: &str,
    logged: &std::collections::HashSet<(i64, i64)>,
    downloading: &std::collections::HashMap<(i64, i64), f64>,
    today: &str,
    watch_now: &dyn Fn(i64, i64) -> Option<crate::watchnow::NowState>,
) -> Vec<EpisodeRow> {
    let mut jf: BTreeMap<(i64, i64), &Value> = BTreeMap::new();
    for v in jf_eps.iter().filter(|v| v["LocationType"] != "Virtual") {
        if let (Some(s), Some(e)) = (v["ParentIndexNumber"].as_i64(), v["IndexNumber"].as_i64()) {
            jf.insert((s, e), v);
        }
    }
    let mut base = base;
    for (s, e) in jf.keys() {
        if !base.iter().any(|b| b.season == *s && b.episode == *e) {
            base.push(BaseEp { season: *s, episode: *e, ..Default::default() });
        }
    }
    let mut rows: Vec<EpisodeRow> = base
        .into_iter()
        .map(|b| {
            let v = jf.get(&(b.season, b.episode)).copied();
            let jf_id = v.and_then(|v| v["Id"].as_str()).map(String::from);
            let now = if jf_id.is_some() { None } else { watch_now(b.season, b.episode) };
            // Finished through Watch now counts at once, not only once the file is the library's.
            let played = v.is_some_and(|v| v["UserData"]["Played"] == true) || logged.contains(&(b.season, b.episode)) || now.as_ref().is_some_and(|n| n.finished);
            let mut progress = v.and_then(|v| v["UserData"]["PlayedPercentage"].as_f64()).filter(|p| *p > 0.0 && !played).map(|p| p / 100.0);
            let jf_still = v.and_then(|v| Some(format!("{jf_base}/Items/{}/Images/Primary?fillWidth=480&quality=85&tag={}", v["Id"].as_str()?, v["ImageTags"]["Primary"].as_str()?)));
            let (state, pct) = if jf_id.is_some() {
                ("downloaded", None)
            } else if let Some(n) = &now {
                // Watch now's own word over Sonarr's queue: it knows where you stopped.
                progress = n.progress.or(progress);
                (if n.half { "half" } else { "downloading" }, Some(n.pct))
            } else if let Some(p) = downloading.get(&(b.season, b.episode)) {
                ("downloading", Some(*p))
            } else {
                ("missing", None)
            };
            EpisodeRow {
                season: b.season,
                episode: b.episode,
                title: if b.title.is_empty() { v.and_then(|v| v["Name"].as_str()).unwrap_or_default().to_string() } else { b.title },
                overview: b.overview.or_else(|| v.and_then(|v| v["Overview"].as_str()).filter(|s| !s.is_empty()).map(String::from)),
                still: jf_still.or(b.still),
                aired: v.is_some() || b.air_date.as_deref().is_some_and(|d| d <= today),
                air_date: b.air_date,
                jf_id,
                played,
                progress,
                state: state.into(),
                pct,
                tile: v.map(|v| crate::tiles::from_jf(v, jf_base)),
                watch_key: now.map(|n| n.key),
            }
        })
        .collect();
    rows.sort_by_key(|r| (if r.season == 0 { i64::MAX } else { r.season }, r.episode));
    rows
}

/// Where you're at when nothing's been saved yet: the episode after the last
/// one finished (or that one, when it's the last). Nothing finished: none.
pub fn initial_spot(rows: &[EpisodeRow]) -> Option<crate::marker::Spot> {
    let regular: Vec<&EpisodeRow> = rows.iter().filter(|r| r.season > 0).collect();
    let last = regular.iter().rposition(|r| r.played)?;
    let r = regular.get(last + 1).unwrap_or(&regular[last]);
    Some(crate::marker::Spot { season: r.season, episode: r.episode })
}

fn tvdb_of(v: &Value) -> Option<i64> {
    v["ProviderIds"]["Tvdb"].as_str().and_then(|s| s.parse().ok())
}

/// The library's show with this TVDB id, never a Watch now placeholder's.
pub fn series_of(shows: &[Value], tvdb: i64) -> Option<&Value> {
    shows.iter().find(|v| !crate::jellyfin::on_shelf(v) && tvdb_of(v) == Some(tvdb))
}

/// Where a show's episode list came from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Source {
    /// Sonarr's or skyhook's, numbered like Jellyfin.
    Full,
    /// Jellyfin's own: the others number the show differently (anime in
    /// absolute order), so the list and the spot both go by Jellyfin's.
    Jellyfin,
    /// Sonarr and skyhook couldn't be had: what Jellyfin has, shown as is.
    Partial,
}

/// More Jellyfin episodes than this missing from Sonarr's list: the two number the show differently.
const NUMBERING_SLACK: usize = 2;

/// Jellyfin and Sonarr (or skyhook) disagree on the numbering: more than a
/// couple of Jellyfin's episodes have no match in the list.
pub fn numbering_disagrees(base: &[BaseEp], jf_eps: &[Value]) -> bool {
    if base.is_empty() {
        return false;
    }
    let unmatched = jf_eps
        .iter()
        .filter(|v| v["LocationType"] != "Virtual")
        .filter_map(|v| Some((v["ParentIndexNumber"].as_i64()?, v["IndexNumber"].as_i64()?)))
        .filter(|(s, e)| *s > 0 && !base.iter().any(|b| b.season == *s && b.episode == *e))
        .count();
    unmatched > NUMBERING_SLACK
}

/// Where you're at to show, and what (if anything) to save. A saved spot is
/// settled onto a real episode, and written back only from a full list (one
/// with episodes missing would pull it back). No spot yet: worked out from
/// what's finished and kept, except from a partial list.
pub fn place(saved: Option<crate::marker::Mark>, rows: &[EpisodeRow], source: Source, now: i64) -> (Option<crate::marker::Spot>, Option<crate::marker::Mark>) {
    use crate::marker::{settle, Mark, Spot};
    let order: Vec<Spot> = rows.iter().filter(|r| r.season > 0).map(|r| Spot { season: r.season, episode: r.episode }).collect();
    match saved {
        Some(m) => {
            let shown = settle(m.spot, &order);
            let save = shown.filter(|s| source == Source::Full && *s != m.spot).map(|spot| Mark { spot, set_at: m.set_at });
            (shown, save)
        }
        None => match source {
            Source::Full => {
                let first = initial_spot(rows);
                (first, first.map(|spot| Mark { spot, set_at: now }))
            }
            // Only what's downloaded is listed: kept as "the one after the last
            // finished" (as the sync counts), not as whatever is listed next.
            Source::Jellyfin => {
                let after = rows.iter().filter(|r| r.season > 0 && r.played).last().map(|r| Spot { season: r.season, episode: r.episode + 1 });
                (after.and_then(|s| settle(s, &order)), after.map(|spot| Mark { spot, set_at: now }))
            }
            Source::Partial => (initial_spot(rows), None),
        },
    }
}

/// What starting over touches: the episodes (Jellyfin numbering, as the log
/// keeps them) of the season, or of the whole show without specials, both on
/// this PC and only in the log; and the Jellyfin ids to mark unwatched (only
/// ones played or started).
pub fn restart_range(jf_eps: &[Value], logged: &[(i64, i64)], season: Option<i64>) -> (Vec<(i64, i64)>, Vec<String>) {
    let inside = |s: i64| match season {
        Some(n) => s == n,
        None => s > 0,
    };
    let mut which: Vec<(i64, i64)> = Vec::new();
    let mut unmark = Vec::new();
    for e in jf_eps {
        let (Some(s), Some(n), Some(id)) = (e["ParentIndexNumber"].as_i64(), e["IndexNumber"].as_i64(), e["Id"].as_str()) else { continue };
        if !inside(s) {
            continue;
        }
        if !which.contains(&(s, n)) {
            which.push((s, n));
        }
        if e["UserData"]["Played"] == true || e["UserData"]["PlaybackPositionTicks"].as_i64().unwrap_or(0) > 0 {
            unmark.push(id.to_string());
        }
    }
    for (s, n) in logged {
        if inside(*s) && !which.contains(&(*s, *n)) {
            which.push((*s, *n));
        }
    }
    which.sort();
    (which, unmark)
}

/// Every episode of a show (Sonarr's list, or skyhook's when Sonarr doesn't
/// have the show), with what's on this PC, what you've finished, and where
/// you're at. A handful of requests, never one per episode.
pub async fn show_episodes(app: &AppHandle, tile: &Tile) -> Result<ShowEpisodes, String> {
    let j = jf(app).filter(|j| j.user_id.is_some()).ok_or("Sign in to Jellyfin first.")?;
    let mut series_id = if tile.kind == "episode" { tile.series_id.clone() } else { tile.jf_id.clone() };
    let mut series_item = match &series_id {
        Some(id) => j.items(&format!("ids={id}")).await.ok().and_then(|v| v.into_iter().next()),
        None => None,
    };
    let mut tile_tvdb = tile.tvdb.filter(|_| tile.kind != "episode");
    // An AniList anime carries no TVDB id: Sonarr's match by title (the same
    // one Download picks) gives it, or Bleach would list no episodes at all.
    if series_id.is_none() && tile_tvdb.is_none() && tile.kind == "anime" && tile.format.as_deref() != Some("MOVIE") {
        tile_tvdb = crate::media::series_lookup(app, tile).await.ok().and_then(|(_, l, _, _)| l["tvdbId"].as_i64()).filter(|&t| t > 0);
    }
    // A show known only by its TVDB id (a Watch now episode, a Completed show,
    // an anime matched above): the library's copy if there is one, else the list alone.
    if let (None, Some(t)) = (&series_id, tile_tvdb) {
        let shows = j.items("recursive=true&includeItemTypes=Series&limit=5000").await.map_err(|e| format!("Jellyfin couldn't be asked: {e}"))?;
        series_item = series_of(&shows, t).cloned();
        series_id = series_item.as_ref().and_then(|v| v["Id"].as_str()).map(String::from);
    }
    let tvdb = series_item.as_ref().and_then(tvdb_of).or(tile_tvdb);
    let show = series_item.as_ref().map(|v| crate::tiles::from_jf(v, &j.base)).or_else(|| (tile.kind != "episode").then(|| tile.clone()));
    let sonarr = crate::media::arr(app, AppId::Sonarr);
    let (jf_eps, in_sonarr) = tokio::join!(
        async {
            match &series_id {
                Some(id) => j.episodes(id).await.map_err(|e| e.to_string()).map(Some),
                None => Ok(None),
            }
        },
        async {
            let (a, t) = (sonarr.as_ref()?, tvdb?);
            a.list(&format!("series?tvdbId={t}")).await.ok()?.into_iter().find(|s| s["tvdbId"].as_i64() == Some(t))?["id"].as_i64()
        },
    );
    // The panel was opened on something in Jellyfin: without its episodes the page would lie.
    let jf_eps = jf_eps.map_err(|e| format!("Jellyfin couldn't list the episodes: {e}"))?.unwrap_or_default();

    let mut base = Vec::new();
    let mut downloading = std::collections::HashMap::new();
    let mut source = Source::Partial;
    if let (Some(a), Some(sid)) = (&sonarr, in_sonarr) {
        let (eps_path, queue_path) = (format!("episode?seriesId={sid}&includeImages=true"), format!("queue?pageSize=1000&seriesIds={sid}"));
        let (eps, queue) = tokio::join!(a.list(&eps_path), a.get_one(&queue_path));
        let eps = eps.map_err(|e| format!("Sonarr couldn't list the episodes: {e}"))?;
        let by_id: std::collections::HashMap<i64, (i64, i64)> = eps.iter().filter_map(|e| Some((e["id"].as_i64()?, (e["seasonNumber"].as_i64()?, e["episodeNumber"].as_i64()?)))).collect();
        // The queue only says how far a download is; without it rows just say "Not downloaded".
        for r in queue.ok().and_then(|q| q["records"].as_array().cloned()).unwrap_or_default().iter().filter(|r| r["seriesId"].as_i64() == Some(sid)) {
            let (Some(ep), Some(size)) = (r["episodeId"].as_i64().and_then(|id| by_id.get(&id)), r["size"].as_f64().filter(|s| *s > 0.0)) else { continue };
            downloading.insert(*ep, (1.0 - r["sizeleft"].as_f64().unwrap_or(size) / size).clamp(0.0, 1.0));
        }
        base = eps.iter().filter_map(base_ep).collect();
        source = Source::Full;
    } else if let Some(t) = tvdb {
        // Not in Sonarr (or Sonarr didn't answer): its metadata server. If that's
        // down too, Jellyfin's own episodes still show, and nothing is saved from them.
        if let Ok(r) = http().get(format!("{SKYHOOK}/{t}")).send().await {
            if let Ok(show) = r.json::<Value>().await {
                base = show["episodes"].as_array().into_iter().flatten().filter_map(base_ep).collect();
                source = if base.is_empty() { Source::Partial } else { Source::Full };
            }
        }
    }
    if source == Source::Full && numbering_disagrees(&base, &jf_eps) {
        base.clear();
        downloading.clear();
        source = Source::Jellyfin;
    }

    let logged: std::collections::HashSet<(i64, i64)> = crate::history::active(app)
        .series
        .values()
        .filter(|w| tvdb.is_some() && w.tvdb == tvdb)
        .flat_map(|w| w.episodes.keys())
        .filter_map(|k| k.split_once(':').and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?))))
        .collect();
    // This profile's Watch now copies: "Half watched" or "Downloading 42%", and the row resumes them.
    let (now_entries, now_pct) = crate::watchnow::show_states(app, tvdb).await;
    let rows = build_rows(base, &jf_eps, &j.base, &logged, &downloading, &today(), &|s, e| crate::watchnow::episode_state(&now_entries, &now_pct, tvdb, s, e));

    let saved = tvdb.and_then(|t| crate::marker::get(app, t));
    let (spot, save) = place(saved, &rows, source, chrono::Utc::now().timestamp());
    if let (Some(t), Some(m)) = (tvdb, save) {
        if let Err(e) = crate::marker::put(app, t, m) {
            log(app, Level::Amber, e);
        }
    }
    let mut seasons: Vec<i64> = Vec::new();
    for r in &rows {
        if !seasons.contains(&r.season) {
            seasons.push(r.season);
        }
    }
    Ok(ShowEpisodes { seasons, spot, episodes: rows, tvdb, show })
}

/// Start the show (None) or one season over, for the watching profile only:
/// those episodes go back to unwatched in Jellyfin, leave the watch log, and
/// where you're at goes to the start of it. The spot is set first: anything
/// a sync reads from before this moment (or that Jellyfin won't unmark) then
/// can't move it.
pub async fn restart(app: &AppHandle, tvdb: i64, season: Option<i64>) -> Result<String, String> {
    let j = jf(app).filter(|j| j.user_id.is_some()).ok_or("Sign in to Jellyfin first.")?;
    crate::marker::set(app, tvdb, crate::marker::Spot { season: season.unwrap_or(1), episode: 0 })?;
    let shows = j.items("recursive=true&includeItemTypes=Series&limit=5000").await.map_err(|e| format!("Jellyfin couldn't be asked: {e}"))?;
    let mut jf_eps = Vec::new();
    for id in shows.iter().filter(|v| tvdb_of(v) == Some(tvdb)).filter_map(|v| v["Id"].as_str()) {
        jf_eps.extend(j.episodes(id).await.map_err(|e| format!("Jellyfin couldn't list the episodes: {e}"))?);
    }
    // Streamed from the Discover shelf or Watch now: their placeholders are
    // Played too, and the watch log reads them, so they're unmarked as well.
    let placeholders = j.watch_items_with_placeholders("includeItemTypes=Series").await.map_err(|e| format!("Jellyfin couldn't be asked: {e}"))?;
    for id in placeholders.iter().filter(|v| crate::jellyfin::on_shelf(v) && tvdb_of(v) == Some(tvdb)).filter_map(|v| v["Id"].as_str()) {
        jf_eps.extend(j.episodes(id).await.map_err(|e| format!("Jellyfin couldn't list the episodes: {e}"))?);
    }
    // Deleted ones too: they're only in the log.
    let logged: Vec<(i64, i64)> = crate::history::active(app)
        .series
        .values()
        .filter(|w| w.tvdb == Some(tvdb))
        .flat_map(|w| w.episodes.keys())
        .filter_map(|k| k.split_once(':').and_then(|(a, b)| Some((a.parse::<i64>().ok()?, b.parse::<i64>().ok()?))))
        .collect();
    let (which, unmark) = restart_range(&jf_eps, &logged, season);
    let mut failed = 0;
    for id in &unmark {
        if j.set_played(id, false).await.is_err() {
            failed += 1;
        }
    }
    crate::history::forget(app, tvdb, &which);
    // Watch now's own memory of those episodes too, so none resumes midway.
    if let Some(uid) = crate::media::active_user(app) {
        if let Err(e) = app.state::<crate::watchnow::WatchNow>().reset(&uid, tvdb, season) {
            log(app, Level::Amber, e);
        }
    }
    if failed > 0 {
        return Err(format!("Jellyfin wouldn't mark {failed} episode{} as unwatched. Try again in a moment.", if failed == 1 { "" } else { "s" }));
    }
    Ok(match season {
        Some(n) => format!("Season {n} starts over."),
        None => "The show starts over.".into(),
    })
}

#[cfg(test)]
mod tests {

    #[test]
    fn whole_show_wants_missing_regular_episodes_only() {
        let ep = |id: i64, season: i64, has: bool| json!({ "id": id, "seasonNumber": season, "hasFile": has });
        assert_eq!(missing_regular(&[ep(1, 0, false), ep(2, 1, true), ep(3, 1, false), ep(4, 2, false)]), vec![3, 4]);
    }
    use super::*;

    #[test]
    fn a_stopped_show_wants_only_the_pick() {
        let ep = |id: i64, s: i64, e: i64, monitored: bool, has: bool| json!({ "id": id, "seasonNumber": s, "episodeNumber": e, "monitored": monitored, "hasFile": has });
        let eps = vec![ep(1, 1, 1, true, false), ep(2, 1, 2, true, false), ep(3, 1, 3, true, true), ep(4, 2, 1, true, false), ep(5, 2, 2, false, false), ep(6, 0, 1, true, false)];
        // Watch now or the shelf picks S1E1: every other old want goes, files stay as they are.
        assert_eq!(stale_wants(&eps, &Pick { seasons: vec![], episodes: vec![(1, 1)] }), vec![2, 4, 6]);
        // A picked season keeps its episodes.
        assert_eq!(stale_wants(&eps, &Pick { seasons: vec![2], episodes: vec![(1, 2)] }), vec![1, 6]);
        assert!(stale_wants(&[ep(9, 1, 1, false, false)], &Pick { seasons: vec![], episodes: vec![(1, 2)] }).is_empty(), "already unwanted: nothing to change");
    }

    #[test]
    fn youtube_ids_come_out_of_any_link() {
        assert_eq!(youtube_id("https://www.youtube.com/watch?v=l91Km49W9qI").as_deref(), Some("l91Km49W9qI"));
        assert_eq!(youtube_id("https://youtu.be/l91Km49W9qI?t=3").as_deref(), Some("l91Km49W9qI"));
        assert_eq!(youtube_id("https://www.youtube.com/embed/l91Km49W9qI").as_deref(), Some("l91Km49W9qI"));
        assert_eq!(youtube_id("https://vimeo.com/123"), None);
    }

    #[test]
    fn picks_read_as_a_sentence() {
        assert_eq!(describe(&Pick { seasons: vec![1], episodes: vec![] }), "season 1");
        assert_eq!(describe(&Pick { seasons: vec![1, 2, 4], episodes: vec![(3, 1)] }), "seasons 1, 2 and 4 and 1 episode");
        assert_eq!(describe(&Pick { seasons: vec![], episodes: vec![(1, 1), (1, 2)] }), "2 episodes");
    }

    #[test]
    fn specials_go_last() {
        let ep = |n| Episode { number: n, title: String::new(), air_date: None, aired: true, has_file: false };
        let s = group(vec![(0, ep(1)), (2, ep(2)), (2, ep(1)), (1, ep(1))].into_iter());
        assert_eq!(s.iter().map(|x| x.number).collect::<Vec<_>>(), vec![1, 2, 0]);
        assert_eq!(s[1].episodes.iter().map(|e| e.number).collect::<Vec<_>>(), vec![1, 2]);
    }

    #[test]
    fn rows_join_sonarr_and_jellyfin_and_say_what_each_is() {
        let base = |s: i64, e: i64, aired: &str| BaseEp { season: s, episode: e, title: format!("Ep {e}"), air_date: Some(aired.into()), ..Default::default() };
        let jf = vec![
            json!({ "Id": "j1", "Type": "Episode", "Name": "One", "SeriesId": "s", "ParentIndexNumber": 1, "IndexNumber": 1, "ImageTags": { "Primary": "t" }, "UserData": { "Played": true } }),
            json!({ "Id": "j2", "Type": "Episode", "Name": "Two", "SeriesId": "s", "ParentIndexNumber": 1, "IndexNumber": 2, "UserData": { "Played": false, "PlayedPercentage": 40.0 } }),
            json!({ "Id": "v", "Type": "Episode", "ParentIndexNumber": 1, "IndexNumber": 5, "LocationType": "Virtual", "UserData": {} }),
            json!({ "Id": "sp", "Type": "Episode", "Name": "Special", "SeriesId": "s", "ParentIndexNumber": 0, "IndexNumber": 1, "UserData": {} }),
        ];
        let logged: std::collections::HashSet<(i64, i64)> = [(1, 3)].into();
        let downloading: std::collections::HashMap<(i64, i64), f64> = [((1, 4), 0.42)].into();
        let rows = build_rows(vec![base(1, 1, "2020-01-01"), base(1, 2, "2020-01-08"), base(1, 3, "2020-01-15"), base(1, 4, "2020-01-22"), base(1, 5, "2099-01-01")], &jf, "http://localhost:8096", &logged, &downloading, "2026-10-19", &|_, _| None);
        let got: Vec<String> = rows.iter().map(|r| format!("S{}E{} {} {} {:?} {:?} {}", r.season, r.episode, r.state, r.played, r.progress, r.pct, r.aired)).collect();
        assert_eq!(got, vec![
            "S1E1 downloaded true None None true",
            "S1E2 downloaded false Some(0.4) None true",
            "S1E3 missing true None None true",
            "S1E4 downloading false None Some(0.42) true",
            "S1E5 missing false None None false",
            "S0E1 downloaded false None None true",
        ], "deleted but logged: ticked; a virtual Jellyfin episode isn't a file; specials last, even when only Jellyfin has them");
        assert_eq!(rows[0].still.as_deref(), Some("http://localhost:8096/Items/j1/Images/Primary?fillWidth=480&quality=85&tag=t"));
        assert_eq!(rows[0].title, "Ep 1", "Sonarr's title");
        assert_eq!(rows[0].tile.as_ref().and_then(|t| t.jf_id.as_deref()), Some("j1"), "playable");
        let now = |half: bool| move |s: i64, e: i64| Some(crate::watchnow::NowState { key: format!("ep:1:{s}:{e}"), half, pct: 0.42, progress: half.then_some(0.25), finished: false });
        let queued: std::collections::HashMap<(i64, i64), f64> = [((2, 1), 0.9)].into();
        let half = build_rows(vec![base(2, 1, "2020-01-01")], &[], "", &Default::default(), &queued, "2026-10-19", &now(true));
        assert_eq!((half[0].state.as_str(), half[0].pct, half[0].progress, half[0].watch_key.as_deref()), ("half", Some(0.42), Some(0.25), Some("ep:1:2:1")), "Watch now's own state, over Sonarr's queue");
        let going = build_rows(vec![base(2, 1, "2020-01-01")], &[], "", &Default::default(), &Default::default(), "2026-10-19", &now(false));
        assert_eq!((going[0].state.as_str(), going[0].pct, going[0].progress), ("downloading", Some(0.42), None), "pressed, not started yet");
        let here = build_rows(vec![base(1, 1, "2020-01-01")], &jf, "http://localhost:8096", &Default::default(), &Default::default(), "2026-10-19", &now(true));
        assert_eq!((here[0].state.as_str(), here[0].watch_key.as_deref()), ("downloaded", None), "the library copy wins");
    }

    /// Watch now S1E1 of a show never opened, finish it, then open the page:
    /// the row is watched and where you're at is S1E2, saved from the full list.
    #[test]
    fn a_watch_now_finish_on_an_unopened_show_puts_you_on_the_next_one() {
        let base = |s: i64, e: i64| BaseEp { season: s, episode: e, air_date: Some("2020-01-01".into()), ..Default::default() };
        let entries = vec![crate::watchnow::Entry { key: "ep:7:1:1".into(), kind: "episode".into(), tvdb: Some(7), season: Some(1), episode: Some(1), position: 1150.0, duration: Some(1200.0), ..Default::default() }];
        let pct = std::collections::HashMap::new();
        let state = |s: i64, e: i64| crate::watchnow::episode_state(&entries, &pct, Some(7), s, e);
        let rows = build_rows(vec![base(1, 1), base(1, 2), base(1, 3)], &[], "", &Default::default(), &Default::default(), "2026-10-19", &state);
        assert!(rows[0].played && rows[0].state != "half" && rows[0].progress.is_none(), "S1E1 is watched");
        let spot = crate::marker::Spot { season: 1, episode: 2 };
        assert_eq!(place(None, &rows, Source::Full, 50), (Some(spot), Some(crate::marker::Mark { spot, set_at: 50 })));
        // Forward only: a show already further on stays there.
        let later = crate::marker::Mark { spot: crate::marker::Spot { season: 1, episode: 3 }, set_at: 10 };
        assert_eq!(place(Some(later), &rows, Source::Full, 50).0, Some(later.spot));
        // Half way through it: not watched, and still the spot.
        let half = vec![crate::watchnow::Entry { position: 300.0, ..entries[0].clone() }];
        let state = |s: i64, e: i64| crate::watchnow::episode_state(&half, &pct, Some(7), s, e);
        let rows = build_rows(vec![base(1, 1), base(1, 2)], &[], "", &Default::default(), &Default::default(), "2026-10-19", &state);
        assert_eq!((rows[0].played, rows[0].state.as_str()), (false, "half"));
        assert_eq!(place(None, &rows, Source::Full, 50).0, None, "nothing finished yet");
    }

    #[test]
    fn the_first_spot_is_after_the_last_finished() {
        let row = |s: i64, e: i64, played: bool| EpisodeRow { season: s, episode: e, title: String::new(), overview: None, still: None, jf_id: None, played, progress: None, state: "missing".into(), pct: None, air_date: None, aired: true, tile: None, watch_key: None };
        let spot = |season, episode| Some(crate::marker::Spot { season, episode });
        // Rewatched S1E1 after finishing S1E3: still after S1E3, the furthest.
        assert_eq!(initial_spot(&[row(1, 1, true), row(1, 2, false), row(1, 3, true), row(2, 1, false), row(0, 1, true)]), spot(2, 1));
        assert_eq!(initial_spot(&[row(1, 1, true), row(1, 2, true)]), spot(1, 2), "all finished: the last");
        assert_eq!(initial_spot(&[row(1, 1, false), row(0, 1, true)]), None, "nothing finished (specials don't count)");
    }

    fn jf_ep(s: i64, e: i64, played: bool) -> Value {
        json!({ "Id": format!("j{s}x{e}"), "Type": "Episode", "Name": "x", "SeriesId": "s", "ParentIndexNumber": s, "IndexNumber": e, "UserData": { "Played": played } })
    }

    fn base_eps(list: &[(i64, i64)]) -> Vec<BaseEp> {
        list.iter().map(|(s, e)| BaseEp { season: *s, episode: *e, ..Default::default() }).collect()
    }

    #[test]
    fn numbering_that_disagrees_is_spotted() {
        // Sonarr: two seasons of 12. Jellyfin numbers the same files 1-24 in one season.
        let sonarr = base_eps(&(1..=12).map(|e| (1, e)).chain((1..=12).map(|e| (2, e))).collect::<Vec<_>>());
        let absolute: Vec<Value> = (1..=24).map(|e| jf_ep(1, e, false)).collect();
        assert!(numbering_disagrees(&sonarr, &absolute));
        let same: Vec<Value> = (1..=12).map(|e| jf_ep(1, e, false)).chain((1..=12).map(|e| jf_ep(2, e, false))).collect();
        assert!(!numbering_disagrees(&sonarr, &same));
        // A couple of extras (a double episode split differently, a special) is still the same numbering.
        let few = [same.clone(), vec![jf_ep(1, 13, false), jf_ep(1, 14, false), jf_ep(0, 1, false)]].concat();
        assert!(!numbering_disagrees(&sonarr, &few));
        assert!(!numbering_disagrees(&[], &absolute), "no list to disagree with");
        let virtuals: Vec<Value> = (13..=24).map(|e| json!({ "ParentIndexNumber": 1, "IndexNumber": e, "LocationType": "Virtual" })).collect();
        assert!(!numbering_disagrees(&sonarr, &virtuals), "placeholders aren't files");
    }

    fn rows(list: &[(i64, i64, bool)]) -> Vec<EpisodeRow> {
        list.iter().map(|(s, e, played)| EpisodeRow { season: *s, episode: *e, title: String::new(), overview: None, still: None, jf_id: None, played: *played, progress: None, state: "missing".into(), pct: None, air_date: None, aired: true, tile: None, watch_key: None }).collect()
    }

    #[test]
    fn a_settled_spot_is_saved_only_from_a_full_list() {
        use crate::marker::{Mark, Spot};
        let spot = |season, episode| Spot { season, episode };
        let saved = Some(Mark { spot: spot(1, 3), set_at: 100 });
        // S1E3 isn't listed: Sonarr's full list goes on to S2E1.
        let full = rows(&[(1, 1, true), (1, 2, true), (2, 1, false)]);
        assert_eq!(place(saved, &full, Source::Full, 999), (Some(spot(2, 1)), Some(Mark { spot: spot(2, 1), set_at: 100 })));
        // Only Jellyfin's (S1E3 not downloaded): shown on what's there, never saved.
        let partial = rows(&[(1, 1, true), (1, 2, true)]);
        assert_eq!(place(saved, &partial, Source::Partial, 999), (Some(spot(1, 2)), None));
        assert_eq!(place(saved, &partial, Source::Jellyfin, 999), (Some(spot(1, 2)), None));
        // Already on a real episode: nothing to save.
        assert_eq!(place(Some(Mark { spot: spot(1, 2), set_at: 100 }), &full, Source::Full, 999).1, None);
    }

    #[test]
    fn the_first_spot_is_worked_out_on_first_open() {
        use crate::marker::{Mark, Spot};
        let spot = |season, episode| Spot { season, episode };
        let list = rows(&[(1, 1, true), (1, 2, true), (1, 3, false), (2, 1, false)]);
        assert_eq!(place(None, &list, Source::Full, 500), (Some(spot(1, 3)), Some(Mark { spot: spot(1, 3), set_at: 500 })));
        assert_eq!(place(None, &list, Source::Partial, 500), (Some(spot(1, 3)), None), "partial: shown, not kept");
        // Jellyfin's own list, only S1E1-2 downloaded: kept as "after S1E2" so the sync agrees.
        let jf_only = rows(&[(1, 1, true), (1, 2, true)]);
        assert_eq!(place(None, &jf_only, Source::Jellyfin, 500), (Some(spot(1, 2)), Some(Mark { spot: spot(1, 3), set_at: 500 })));
        assert_eq!(place(None, &rows(&[(1, 1, false)]), Source::Full, 500), (None, None), "nothing finished: no spot yet");
    }

    #[test]
    fn a_show_known_by_tvdb_finds_the_librarys_copy() {
        let shows = vec![
            serde_json::json!({ "Id": "ws", "Path": r"C:\a\Chillarr Watch now\Series\BB [tvdbid-81189]", "ProviderIds": { "Tvdb": "81189" } }),
            serde_json::json!({ "Id": "x", "Path": r"D:\TV\Other", "ProviderIds": { "Tvdb": "5" } }),
            serde_json::json!({ "Id": "s", "Path": r"D:\TV\Breaking Bad", "ProviderIds": { "Tvdb": "81189" } }),
        ];
        assert_eq!(series_of(&shows, 81189).unwrap()["Id"], "s", "never the Watch now placeholder");
        assert!(series_of(&shows, 9).is_none(), "deleted: the list alone");
    }

    #[test]
    fn starting_over_picks_the_right_episodes() {
        let jf = vec![jf_ep(1, 1, true), jf_ep(1, 2, false), jf_ep(2, 1, true), jf_ep(2, 2, false), jf_ep(0, 1, true),
            json!({ "Id": "started", "ParentIndexNumber": 2, "IndexNumber": 3, "UserData": { "Played": false, "PlaybackPositionTicks": 50 } })];
        let logged = [(1, 9), (2, 7), (0, 2)];
        let (which, unmark) = restart_range(&jf, &logged, Some(2));
        assert_eq!(which, vec![(2, 1), (2, 2), (2, 3), (2, 7)], "the season on this PC and deleted ones from the log");
        assert_eq!(unmark, vec!["j2x1", "started"], "only played or started ones are unmarked");
        let (which, unmark) = restart_range(&jf, &logged, None);
        assert_eq!(which, vec![(1, 1), (1, 2), (1, 9), (2, 1), (2, 2), (2, 3), (2, 7)], "the whole show leaves specials alone");
        assert_eq!(unmark, vec!["j1x1", "j2x1", "started"]);
    }

    #[test]
    fn disc_copies_leave_every_profile() {
        let mut p = json!({ "cutoff": 31, "items": [
            { "quality": { "id": 7, "name": "Bluray-1080p" }, "items": [], "allowed": true },
            { "id": 1001, "name": "WEB 2160p", "items": [ { "quality": { "id": 18, "name": "WEBDL-2160p" }, "allowed": true } ], "allowed": true },
            { "quality": { "id": 19, "name": "Bluray-2160p" }, "items": [], "allowed": true },
            { "quality": { "id": 31, "name": "Remux-2160p" }, "items": [], "allowed": true },
            { "quality": { "id": 22, "name": "BR-DISK" }, "items": [], "allowed": true }
        ]});
        assert!(drop_disc_copies(&mut p));
        assert_eq!(p["items"][3]["allowed"], false);
        assert_eq!(p["items"][4]["allowed"], false);
        assert_eq!(p["items"][0]["allowed"], true);
        assert_eq!(p["cutoff"], 19, "the cutoff moves to the best quality still allowed");
        assert!(!drop_disc_copies(&mut p), "nothing to change the second time");
    }

    #[test]
    fn turning_the_switch_off_lets_remuxes_back_where_their_resolution_is() {
        let mut p = json!({ "cutoff": 7, "items": [
            { "quality": { "id": 7, "name": "Bluray-1080p" }, "items": [], "allowed": true },
            { "quality": { "id": 30, "name": "Remux-1080p" }, "items": [], "allowed": false },
            { "quality": { "id": 31, "name": "Remux-2160p" }, "items": [], "allowed": false },
            { "quality": { "id": 22, "name": "BR-DISK" }, "items": [], "allowed": false }
        ]});
        assert!(allow_remux(&mut p));
        assert_eq!(p["items"][1]["allowed"], true, "1080p is allowed, so its remux comes back");
        assert_eq!(p["items"][2]["allowed"], false, "no 2160p here, so no 4K remux");
        assert_eq!(p["items"][3]["allowed"], false, "whole discs stay out");
        assert!(!allow_remux(&mut p));
    }

    #[test]
    fn the_dub_format_needs_a_dub_and_a_language_besides_the_films_own() {
        let f = dub_format(&[]);
        let specs = f["specifications"].as_array().unwrap();
        assert_eq!(specs.len(), 2);
        assert!(specs.iter().all(|s| s["required"] == true));
        assert_ne!(specs[0]["implementation"], specs[1]["implementation"], "different kinds, or one would be ignored");
        let re = specs[0]["fields"][0]["value"].as_str().unwrap();
        assert!(re.contains("hindi") && !re.contains("japanese") && !re.contains("english"));
        assert_eq!(specs[1]["fields"][0]["value"], -2);
        let mut p = json!({ "formatItems": [] });
        assert!(score_format(&mut p, 7, -10000));
        assert!(!score_format(&mut p, 7, -10000));
        assert!(score_format(&mut p, 7, 0));
        assert_eq!(p["formatItems"][0]["score"], 0);
    }

    #[test]
    fn movie_sizes_are_capped_per_resolution_and_disc_copies_skipped() {
        assert_eq!(movie_size_cap("Bluray-1080p"), Some((45.0, 70.0)));
        assert_eq!(movie_size_cap("WEBDL-2160p"), Some((120.0, 170.0)));
        assert_eq!(movie_size_cap("HDTV-720p"), Some((25.0, 40.0)));
        assert_eq!(movie_size_cap("Remux-1080p"), None);
        assert_eq!(movie_size_cap("DVD"), None);
    }
}
