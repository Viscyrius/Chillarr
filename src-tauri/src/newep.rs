//! New episodes (#72): like reading's New chapters, for watching. One card per
//! show this profile has watched that has episodes out since it last watched
//! it: the newest one (S2 E7), how many are new, and how long ago it came out.
//! It shows at the top of Home (all), Series and Anime, and in My List's New
//! for you for the shows on My List.
//!
//! "Watched" is this profile's own watch log (history.rs); "out" is Sonarr's
//! air dates. Episodes already watched never count, nor specials.

use crate::model::AppId;
use crate::tiles::{self, Tile};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

/// Kept per profile this long: each show is one Sonarr ask.
const LIFE: Duration = Duration::from_secs(10 * 60);
/// Shows looked at, the most recently watched first.
const MAX_SHOWS: usize = 40;

#[derive(Clone, Debug, PartialEq)]
pub struct Fresh {
    pub count: usize,
    /// The newest new episode: (season, episode, its name, aired at ms).
    pub newest: (i64, i64, String, i64),
}

fn air_ms(e: &Value) -> Option<i64> {
    e["airDateUtc"].as_str().and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()).map(|t| t.timestamp_millis())
}

/// What's new in a show's Sonarr episodes for someone who last watched it at
/// `since` (unix seconds) and has watched `watched` ("season:episode").
pub fn fresh(eps: &[Value], watched: &BTreeMap<String, i64>, since: i64, now_ms: i64) -> Option<Fresh> {
    let mut new: Vec<(i64, i64, String, i64)> = eps
        .iter()
        .filter_map(|e| {
            let (s, n, at) = (e["seasonNumber"].as_i64()?, e["episodeNumber"].as_i64()?, air_ms(e)?);
            (s > 0 && at <= now_ms && at > since * 1000 && !watched.contains_key(&crate::history::ep_key(s, n))).then(|| (s, n, e["title"].as_str().unwrap_or("").to_string(), at))
        })
        .collect();
    new.sort_by_key(|x| (x.0, x.1));
    let newest = new.last()?.clone();
    Some(Fresh { count: new.len(), newest })
}

/// "today", "yesterday", "3 days ago", "2 weeks ago".
pub fn ago(at_ms: i64, now_ms: i64) -> String {
    let days = ((now_ms - at_ms).max(0) / 86_400_000) as i64;
    match days {
        0 => "today".into(),
        1 => "yesterday".into(),
        2..=13 => format!("{days} days ago"),
        _ => format!("{} weeks ago", days / 7),
    }
}

/// The card's line under the title: "S2 E7 · 3 new · yesterday".
pub fn line(f: &Fresh, now_ms: i64) -> String {
    let (s, e, _, at) = &f.newest;
    let n = if f.count > 1 { format!(" · {} new", f.count) } else { String::new() };
    format!("S{s} E{e}{n} · {}", ago(*at, now_ms))
}

#[derive(Default)]
pub struct Cache(Mutex<HashMap<String, (Instant, Vec<(Tile, bool)>)>>);

/// The shows with new episodes for the profile watching now: (card, is anime),
/// newest first.
pub async fn shows(app: &AppHandle) -> Vec<(Tile, bool)> {
    let Some(uid) = crate::media::active_user(app) else { return vec![] };
    if let Some((at, v)) = app.state::<Cache>().0.lock().unwrap().get(&uid) {
        if at.elapsed() < LIFE {
            return v.clone();
        }
    }
    let got = shows_now(app).await;
    app.state::<Cache>().0.lock().unwrap().insert(uid, (Instant::now(), got.clone()));
    got
}

async fn shows_now(app: &AppHandle) -> Vec<(Tile, bool)> {
    let Some(a) = crate::media::arr(app, AppId::Sonarr) else { return vec![] };
    let a = std::sync::Arc::new(a);
    let h = crate::history::active(app);
    // Each watched show by TVDB id: when it was last watched, and which episodes.
    let mut mine: Vec<(i64, i64, BTreeMap<String, i64>, bool)> = Vec::new();
    for s in h.series.values() {
        let Some(tvdb) = s.tvdb else { continue };
        let last = s.episodes.values().chain(s.files.values()).copied().max().unwrap_or(0);
        if last == 0 {
            continue;
        }
        match mine.iter_mut().find(|m| m.0 == tvdb) {
            Some(m) => {
                m.1 = m.1.max(last);
                m.2.extend(s.episodes.clone());
            }
            None => mine.push((tvdb, last, s.episodes.clone(), s.anime)),
        }
    }
    mine.sort_by_key(|m| std::cmp::Reverse(m.1));
    mine.truncate(MAX_SHOWS);
    let series = a.list("series").await.unwrap_or_default();
    let now_ms = chrono::Utc::now().timestamp_millis();
    let mut asks = tokio::task::JoinSet::new();
    for (tvdb, last, watched, anime) in mine {
        let Some(s) = series.iter().find(|s| s["tvdbId"].as_i64() == Some(tvdb)).cloned() else { continue };
        let Some(sid) = s["id"].as_i64() else { continue };
        let a = a.clone();
        asks.spawn(async move {
            let eps = a.list(&format!("episode?seriesId={sid}")).await.unwrap_or_default();
            let f = fresh(&eps, &watched, last, now_ms)?;
            let mut t = tiles::from_sonarr(&s);
            t.subtitle = Some(line(&f, now_ms));
            Some((t, anime || s["seriesType"] == "anime", f.newest.3))
        });
    }
    let mut out: Vec<(Tile, bool, i64)> = Vec::new();
    while let Some(r) = asks.join_next().await {
        if let Ok(Some(x)) = r {
            out.push(x);
        }
    }
    out.sort_by_key(|x| std::cmp::Reverse(x.2));
    out.into_iter().map(|(t, a, _)| (t, a)).collect()
}

/// A watch changed what's new: asked again next time.
pub fn forget(app: &AppHandle) {
    app.state::<Cache>().0.lock().unwrap().clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const DAY: i64 = 86_400_000;
    fn ep(s: i64, e: i64, day: i64) -> Value {
        let at = chrono::DateTime::from_timestamp_millis(day * DAY).unwrap().to_rfc3339();
        json!({ "seasonNumber": s, "episodeNumber": e, "title": format!("Ep {e}"), "airDateUtc": at })
    }

    #[test]
    fn new_episodes_are_the_ones_out_since_you_last_watched() {
        let eps = vec![ep(2, 5, 100), ep(2, 6, 107), ep(2, 7, 114), ep(2, 8, 121), ep(0, 1, 115)];
        let mut watched = BTreeMap::new();
        watched.insert("2:5".to_string(), 101 * 86_400);
        let now = 118 * DAY;
        let f = fresh(&eps, &watched, 101 * 86_400, now).unwrap();
        assert_eq!(f.count, 2, "6 and 7 are out; 8 airs later; the special doesn't count");
        assert_eq!((f.newest.0, f.newest.1), (2, 7));
        assert_eq!(line(&f, now), "S2 E7 · 2 new · 4 days ago");
    }

    #[test]
    fn nothing_new_when_caught_up() {
        let eps = vec![ep(1, 1, 10), ep(1, 2, 17)];
        let mut watched = BTreeMap::new();
        watched.insert("1:1".to_string(), 11 * 86_400);
        watched.insert("1:2".to_string(), 18 * 86_400);
        assert!(fresh(&eps, &watched, 18 * 86_400, 30 * DAY).is_none());
    }

    #[test]
    fn an_episode_watched_already_isnt_new_even_if_it_aired_late() {
        let eps = vec![ep(1, 3, 20)];
        let mut watched = BTreeMap::new();
        watched.insert("1:3".to_string(), 21 * 86_400);
        assert!(fresh(&eps, &watched, 5 * 86_400, 30 * DAY).is_none());
    }
}
