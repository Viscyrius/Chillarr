//! Your week while watching (issue #44): your series and anime by the day
//! their episodes air, from Sonarr's calendar. "Yours" is what this profile
//! has watched, or has on My List (a Jellyfin favourite or kept by Chillarr).

use crate::model::AppId;
use crate::tiles::{self, Tile};
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use tauri::{AppHandle, Manager};

/// Sonarr's calendar is asked this far either side of now: a week back for
/// the last episode, a week on for a show that hasn't aired this week yet.
const DAYS: i64 = 7;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct WeekShow {
    pub tile: Tile,
    /// When its latest episode aired (or the next one airs, if none did this
    /// week), in milliseconds since 1970. The page reads the weekday from it
    /// where you are.
    pub air: i64,
    /// True when `air` has already passed.
    pub aired: bool,
    /// Sonarr files it as anime (the Anime tab shows these, Series the rest).
    pub anime: bool,
}

fn air_ms(ep: &Value) -> Option<i64> {
    let s = ep["airDateUtc"].as_str()?;
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|d| d.timestamp_millis())
}

/// One show per series of yours on the calendar: its latest aired episode,
/// or the next one when none aired in the window. Newest first.
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
            best.insert(tvdb, (ep["series"].clone(), at, aired));
        }
    }
    let mut out: Vec<_> = best.into_iter().map(|(id, (s, at, aired))| (id, s, at, aired)).collect();
    out.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
    out
}

/// The tvdb ids of what this profile watches or has on My List, and the
/// Jellyfin tiles for the ones in the library (so a card can play).
async fn yours(app: &AppHandle) -> (HashSet<i64>, HashMap<i64, Tile>) {
    let mut ids: HashSet<i64> = crate::history::active(app).series.values().filter_map(|w| w.tvdb).collect();
    for k in app.state::<crate::media::Media>().saved_keys() {
        if let Some(id) = k.strip_prefix("tvdb:").and_then(|n| n.parse().ok()) {
            ids.insert(id);
        }
    }
    let mut lib = HashMap::new();
    if let Some(j) = crate::media::jf(app).filter(|j| j.user_id.is_some()) {
        for v in j.favourites().await.unwrap_or_default() {
            let t = tiles::from_jf(&v, &j.base);
            if let (Some(id), "series") = (t.tvdb, t.kind.as_str()) {
                ids.insert(id);
                lib.insert(id, t);
            }
        }
    }
    (ids, lib)
}

/// Your series and anime on Sonarr's calendar this week. Err when Sonarr
/// isn't set up or didn't answer, so the page can say so.
pub async fn list(app: &AppHandle) -> Result<Vec<WeekShow>, String> {
    let a = crate::media::arr(app, AppId::Sonarr).ok_or("Sonarr isn't set up yet.")?;
    let now = chrono::Utc::now();
    let fmt = |d: chrono::DateTime<chrono::Utc>| d.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let path = format!(
        "calendar?start={}&end={}&includeSeries=true&unmonitored=true",
        fmt(now - chrono::Duration::days(DAYS)),
        fmt(now + chrono::Duration::days(DAYS))
    );
    let (cal, (ids, lib)) = tokio::join!(a.list(&path), yours(app));
    let cal = cal.map_err(|e| format!("Sonarr didn't answer: {e}"))?;
    Ok(pick(&cal, &ids, now.timestamp_millis())
        .into_iter()
        .map(|(id, s, air, aired)| {
            let tile = lib.get(&id).cloned().unwrap_or_else(|| tiles::from_sonarr(&s));
            WeekShow { tile, air, aired, anime: s["seriesType"] == "anime" }
        })
        .collect())
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
    fn an_episode_without_an_air_time_is_skipped() {
        let cal = vec![json!({ "series": { "tvdbId": 1 } })];
        assert!(pick(&cal, &[1].into_iter().collect(), 0).is_empty());
    }
}
