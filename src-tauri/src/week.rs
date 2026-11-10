//! Your week while watching (issue #44): your series and anime by the day
//! their episodes air, like reading's Your week. "Yours" is every show in
//! Sonarr, what this profile has watched, its My List and its Jellyfin
//! favourites. Every one still running sits on its airing day: with the
//! episode out this week or the next one announced, or "between seasons" on
//! the day it last aired. Sonarr's calendar first, then TVmaze (by TVDB id)
//! and AniList (anime known only by its AniList id) for the rest.

use crate::model::AppId;
use crate::tiles::{self, Tile};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

/// Sonarr's calendar is asked this far either side of now: a week back for
/// the last episode, a week on for a show that hasn't aired this week yet.
const DAYS: i64 = 7;
/// TVmaze's answer for a show is kept this long.
const TVMAZE_LIFE: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct WeekShow {
    pub tile: Tile,
    /// When its episode aired (or airs), in milliseconds since 1970. The page
    /// reads the weekday from it where you are.
    pub air: i64,
    /// True when `air` has already passed.
    pub aired: bool,
    /// The Anime tab shows these, Series the rest.
    pub anime: bool,
    /// Which episode that is, "S2 E7", and its name when known.
    pub episode: String,
    pub episode_title: String,
    /// Still running, but no episode this week and none announced: `air` is
    /// when the last one aired, so it sits on its usual day.
    pub between: bool,
}

/// "S2 E7" for a calendar episode ("E7" for specials and absolute numbering gaps).
fn ep_label(ep: &Value) -> String {
    match (ep["seasonNumber"].as_i64().or_else(|| ep["season"].as_i64()), ep["episodeNumber"].as_i64().or_else(|| ep["number"].as_i64())) {
        (Some(s), Some(e)) if s > 0 => format!("S{s} E{e}"),
        (_, Some(e)) => format!("E{e}"),
        _ => String::new(),
    }
}

fn clean_title(t: Option<&str>) -> String {
    t.filter(|t| !t.eq_ignore_ascii_case("TBA") && !t.eq_ignore_ascii_case("TBD")).unwrap_or_default().to_string()
}

fn air_ms(ep: &Value) -> Option<i64> {
    let s = ep["airDateUtc"].as_str().or_else(|| ep["airstamp"].as_str())?;
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|d| d.timestamp_millis())
}

/// One show per series of yours on the calendar: its latest aired episode,
/// or the next one when none aired in the window. Newest first. Each comes
/// with the calendar episode it was picked for (its series is `ep["series"]`).
pub fn pick(cal: &[Value], yours: &HashSet<i64>, now_ms: i64) -> Vec<(i64, Value, i64, bool)> {
    let mut best: HashMap<i64, (Value, i64, bool)> = HashMap::new();
    for ep in cal {
        let Some(tvdb) = ep["series"]["tvdbId"].as_i64().filter(|id| *id > 0 && yours.contains(id)) else { continue };
        let Some(at) = air_ms(ep) else { continue };
        let aired = at <= now_ms;
        let better = match best.get(&tvdb) {
            None => true,
            // An aired one beats one still to come; among aired the latest, among to come the soonest.
            Some((_, was, was_aired)) => match (aired, *was_aired) {
                (true, false) => true,
                (false, true) => false,
                (true, true) => at > *was,
                (false, false) => at < *was,
            },
        };
        if better {
            best.insert(tvdb, (ep.clone(), at, aired));
        }
    }
    let mut out: Vec<_> = best.into_iter().map(|(id, (s, at, aired))| (id, s, at, aired)).collect();
    out.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
    out
}

/// Where a running show sits from TVmaze's answer (with `nextepisode` and
/// `previousepisode` embedded): (air, aired, episode, between), or None when
/// it has ended or never aired.
pub fn tvmaze_slot(show: &Value, now_ms: i64) -> Option<(i64, bool, Value, bool)> {
    let status = show["status"].as_str().unwrap_or_default();
    let next = &show["_embedded"]["nextepisode"];
    let prev = &show["_embedded"]["previousepisode"];
    let week = DAYS * 86_400_000;
    let prev_at = air_ms(prev);
    // Out within the last week: that one, like the calendar.
    if let Some(at) = prev_at.filter(|at| now_ms - at <= week && *at <= now_ms) {
        return Some((at, true, prev.clone(), false));
    }
    if let Some(at) = air_ms(next) {
        return Some((at, at <= now_ms, next.clone(), false));
    }
    if status != "Running" {
        return None;
    }
    prev_at.map(|at| (at, true, prev.clone(), true))
}

/// What this profile calls its shows: tiles (from My List and favourites,
/// which have their pictures) by key, TVDB ids, AniList ids, and which are anime.
struct Yours {
    tiles: HashMap<String, Tile>,
    tvdb: HashSet<i64>,
    anilist: HashSet<i64>,
    anime_tvdb: HashSet<i64>,
}

async fn yours(app: &AppHandle) -> Yours {
    let mut y = Yours { tiles: HashMap::new(), tvdb: HashSet::new(), anilist: HashSet::new(), anime_tvdb: HashSet::new() };
    for w in crate::history::active(app).series.values() {
        if let Some(id) = w.tvdb {
            y.tvdb.insert(id);
            if w.anime {
                y.anime_tvdb.insert(id);
            }
        } else if let Some(id) = w.anilist {
            y.anilist.insert(id);
        }
    }
    for t in app.state::<crate::media::Media>().saved_list() {
        if t.kind != "series" && t.kind != "anime" {
            continue;
        }
        if let Some(id) = t.tvdb.or_else(|| t.key.strip_prefix("tvdb:").and_then(|n| n.parse().ok())) {
            y.tvdb.insert(id);
            if t.kind == "anime" {
                y.anime_tvdb.insert(id);
            }
            y.tiles.insert(format!("tvdb:{id}"), t);
        } else if let Some(id) = t.key.strip_prefix("anilist:").and_then(|n| n.parse::<i64>().ok()) {
            y.anilist.insert(id);
            y.tiles.insert(t.key.clone(), t);
        }
    }
    if let Some(j) = crate::media::jf(app).filter(|j| j.user_id.is_some()) {
        for v in j.favourites().await.unwrap_or_default() {
            let t = tiles::from_jf(&v, &j.base);
            if let (Some(id), "series") = (t.tvdb, t.kind.as_str()) {
                y.tvdb.insert(id);
                y.tiles.insert(format!("tvdb:{id}"), t);
            }
        }
    }
    y
}

/// TVmaze's show for a TVDB id, with its next and previous episodes, kept a while.
async fn tvmaze(tvdb: i64) -> Option<Value> {
    static SEEN: Mutex<Option<HashMap<i64, (Instant, Option<Value>)>>> = Mutex::new(None);
    if let Some((at, v)) = SEEN.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).get(&tvdb) {
        if at.elapsed() < TVMAZE_LIFE {
            return v.clone();
        }
    }
    let http = crate::media::http();
    let found = async {
        let r = http.get(format!("https://api.tvmaze.com/lookup/shows?thetvdb={tvdb}")).send().await.ok()?;
        if r.status() == reqwest::StatusCode::NOT_FOUND {
            return Some(Value::Null);
        }
        let show: Value = r.error_for_status().ok()?.json().await.ok()?;
        let id = show["id"].as_i64()?;
        http.get(format!("https://api.tvmaze.com/shows/{id}?embed[]=nextepisode&embed[]=previousepisode")).send().await.ok()?.error_for_status().ok()?.json::<Value>().await.ok()
    }
    .await;
    // A show TVmaze doesn't know is remembered as such; a failed ask isn't.
    if let Some(v) = &found {
        let v = (!v.is_null()).then(|| v.clone());
        SEEN.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(tvdb, (Instant::now(), v));
    }
    found.filter(|v| !v.is_null())
}

/// AniList's airing anime among `ids`: (tile, air, aired, episode number).
async fn anilist_airing(ids: &[i64], now_ms: i64) -> Vec<(Tile, i64, bool, i64)> {
    if ids.is_empty() {
        return vec![];
    }
    let q = "query($ids:[Int]){Page(perPage:50){media(id_in:$ids,type:ANIME){id status format genres title{english romaji} coverImage{extraLarge} bannerImage averageScore description(asHtml:false) seasonYear nextAiringEpisode{airingAt episode}}}}";
    let Ok(v) = crate::media::anilist(&json!({ "query": q, "variables": { "ids": ids } })).await else { return vec![] };
    let mut out = Vec::new();
    for m in v["data"]["Page"]["media"].as_array().into_iter().flatten() {
        let next = &m["nextAiringEpisode"];
        let (Some(at), Some(n)) = (next["airingAt"].as_i64(), next["episode"].as_i64()) else { continue };
        let at = at * 1000;
        // A weekly show whose next one is under a week off aired its last one a week before.
        let (air, aired, ep) = if n > 1 && at - now_ms < DAYS * 86_400_000 && at > now_ms && m["status"] == "RELEASING" {
            let last = at - DAYS * 86_400_000;
            if now_ms - last < 86_400_000 { (last, true, n - 1) } else { (at, false, n) }
        } else {
            (at, at <= now_ms, n)
        };
        out.push((tiles::from_anilist(m), air, aired, ep));
    }
    out
}

/// Every running show of yours, on the day it airs. Err only when nothing
/// could be asked at all.
pub async fn list(app: &AppHandle) -> Result<Vec<WeekShow>, String> {
    let now = chrono::Utc::now();
    let now_ms = now.timestamp_millis();
    let sonarr = crate::media::arr(app, AppId::Sonarr);
    let fmt = |d: chrono::DateTime<chrono::Utc>| d.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let path = format!(
        "calendar?start={}&end={}&includeSeries=true&unmonitored=true",
        fmt(now - chrono::Duration::days(DAYS)),
        fmt(now + chrono::Duration::days(DAYS))
    );
    let (cal, all, mut y) = tokio::join!(
        async { match &sonarr { Some(a) => a.list(&path).await.unwrap_or_default(), None => vec![] } },
        async { match &sonarr { Some(a) => a.list("series").await.unwrap_or_default(), None => vec![] } },
        yours(app)
    );
    // Every show in Sonarr is one you get, so it's yours too.
    let mut sonarr_tiles: HashMap<i64, Tile> = HashMap::new();
    for s in &all {
        if let Some(id) = s["tvdbId"].as_i64().filter(|id| *id > 0) {
            y.tvdb.insert(id);
            if s["seriesType"] == "anime" {
                y.anime_tvdb.insert(id);
            }
            sonarr_tiles.insert(id, tiles::from_sonarr(s));
        }
    }
    let tile_for = |id: i64, fallback: Tile| y.tiles.get(&format!("tvdb:{id}")).cloned().or_else(|| sonarr_tiles.get(&id).cloned()).unwrap_or(fallback);
    let mut out: Vec<WeekShow> = Vec::new();
    let mut done: HashSet<i64> = HashSet::new();
    for (id, ep, air, aired) in pick(&cal, &y.tvdb, now_ms) {
        let s = &ep["series"];
        done.insert(id);
        let anime = s["seriesType"] == "anime" || y.anime_tvdb.contains(&id);
        out.push(WeekShow { tile: tile_for(id, tiles::from_sonarr(s)), air, aired, anime, episode: ep_label(&ep), episode_title: clean_title(ep["title"].as_str()), between: false });
    }
    // The rest from TVmaze, a few at a time.
    let rest: Vec<i64> = y.tvdb.iter().copied().filter(|id| !done.contains(id)).take(80).collect();
    let gate = std::sync::Arc::new(tokio::sync::Semaphore::new(6));
    let mut set = tokio::task::JoinSet::new();
    for id in rest {
        let gate = gate.clone();
        set.spawn(async move {
            let _turn = gate.acquire_owned().await;
            (id, tvmaze(id).await)
        });
    }
    let mut found = Vec::new();
    while let Some(done) = set.join_next().await {
        if let Ok(x) = done {
            found.push(x);
        }
    }
    for (id, show) in found {
        let Some(show) = show else { continue };
        let Some((air, aired, ep, between)) = tvmaze_slot(&show, now_ms) else { continue };
        let lang_jp = show["language"] == "Japanese" && show["type"] == "Animation";
        let anime = y.anime_tvdb.contains(&id) || lang_jp || show["genres"].as_array().is_some_and(|g| g.iter().any(|x| x == "Anime"));
        let mut fallback = tiles::from_tvmaze(&show);
        if anime {
            fallback.kind = "anime".into();
        }
        out.push(WeekShow { tile: tile_for(id, fallback), air, aired, anime, episode: if between { String::new() } else { ep_label(&ep) }, episode_title: if between { String::new() } else { clean_title(ep["name"].as_str()) }, between });
    }
    // Anime known only by AniList.
    let al: Vec<i64> = y.anilist.iter().copied().collect();
    for (t, air, aired, n) in anilist_airing(&al, now_ms).await {
        let t = y.tiles.get(&t.key).cloned().unwrap_or(t);
        out.push(WeekShow { tile: t, air, aired, anime: true, episode: format!("E{n}"), episode_title: String::new(), between: false });
    }
    // One card per show, even when it is known two ways.
    let mut seen = HashSet::new();
    out.retain(|w| seen.insert(crate::media::squash(&w.tile.title)));
    out.sort_by(|a, b| b.air.cmp(&a.air));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ep(tvdb: i64, when: &str) -> Value {
        json!({ "airDateUtc": when, "series": { "tvdbId": tvdb, "title": format!("S{tvdb}") } })
    }

    #[test]
    fn keeps_only_yours_and_the_latest_aired_episode() {
        let now = air_ms(&ep(0, "2026-11-08T12:00:00Z")).unwrap();
        let cal = vec![
            ep(1, "2026-11-02T01:00:00Z"),
            ep(1, "2026-11-07T01:00:00Z"),
            ep(1, "2026-11-12T01:00:00Z"),
            ep(2, "2026-11-10T01:00:00Z"),
            ep(2, "2026-11-13T01:00:00Z"),
            ep(3, "2026-11-07T01:00:00Z"),
        ];
        let yours: HashSet<i64> = [1, 2].into_iter().collect();
        let got = pick(&cal, &yours, now);
        let got: Vec<(i64, i64, bool)> = got.into_iter().map(|(id, _, at, aired)| (id, at, aired)).collect();
        assert_eq!(got, vec![
            (2, air_ms(&ep(0, "2026-11-10T01:00:00Z")).unwrap(), false),
            (1, air_ms(&ep(0, "2026-11-07T01:00:00Z")).unwrap(), true),
        ]);
    }

    #[test]
    fn episode_labels() {
        assert_eq!(ep_label(&json!({ "seasonNumber": 2, "episodeNumber": 7 })), "S2 E7");
        assert_eq!(ep_label(&json!({ "season": 3, "number": 1 })), "S3 E1");
        assert_eq!(ep_label(&json!({ "seasonNumber": 0, "episodeNumber": 3 })), "E3");
        assert_eq!(ep_label(&json!({})), "");
    }

    #[test]
    fn an_episode_without_an_air_time_is_skipped() {
        let cal = vec![json!({ "series": { "tvdbId": 1 } })];
        assert!(pick(&cal, &[1].into_iter().collect(), 0).is_empty());
    }

    #[test]
    fn tvmaze_running_shows_sit_on_their_day() {
        let now = air_ms(&json!({ "airstamp": "2026-11-10T12:00:00+00:00" })).unwrap();
        let prev = |s: &str| json!({ "airstamp": s, "season": 2, "number": 12 });
        // Between seasons: last aired long ago, nothing announced, still running.
        let show = json!({ "status": "Running", "_embedded": { "previousepisode": prev("2025-09-17T13:30:00+00:00") } });
        let (at, aired, _, between) = tvmaze_slot(&show, now).unwrap();
        assert!(aired && between);
        assert_eq!(at, air_ms(&prev("2025-09-17T13:30:00+00:00")).unwrap());
        // Out three days ago: that episode, not between.
        let show = json!({ "status": "Running", "_embedded": { "previousepisode": prev("2026-11-07T13:30:00+00:00") } });
        let (_, aired, ep, between) = tvmaze_slot(&show, now).unwrap();
        assert!(aired && !between);
        assert_eq!(ep_label(&ep), "S2 E12");
        // The next one announced: that one.
        let show = json!({ "status": "Running", "_embedded": { "previousepisode": prev("2025-01-01T00:00:00+00:00"), "nextepisode": { "airstamp": "2026-11-12T13:30:00+00:00", "season": 3, "number": 1 } } });
        let (_, aired, ep, between) = tvmaze_slot(&show, now).unwrap();
        assert!(!aired && !between);
        assert_eq!(ep_label(&ep), "S3 E1");
        // Ended with nothing coming: left out.
        let show = json!({ "status": "Ended", "_embedded": { "previousepisode": prev("2025-01-01T00:00:00+00:00") } });
        assert!(tvmaze_slot(&show, now).is_none());
    }
}
