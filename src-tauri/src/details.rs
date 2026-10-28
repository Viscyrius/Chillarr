//! Everything about a title for the detail panel: ratings, runtime, age
//! rating, studio, dates, who made it, and the cast with photos. Keyless
//! sources: Radarr's metadata server (movies), Sonarr's (shows), AniList
//! (anime), and Jellyfin for what's in the library.

use crate::media::{http, jf, Media};
use crate::tiles::Tile;
use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

const RADARR_META: &str = "https://api.radarr.video/v1/movie";
const SKYHOOK: &str = "https://skyhook.sonarr.tv/v1/tvdb/shows/en";

#[derive(Serialize, Clone, Debug, Default)]
pub struct Person {
    pub name: String,
    pub role: Option<String>,
    pub photo: Option<String>,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Details {
    /// (label, value) in display order: "Rating" "8.5 / 10 (5,594 votes)".
    pub facts: Vec<(String, String)>,
    /// (job, names): "Director" "Jon Watts".
    pub crew: Vec<(String, String)>,
    pub cast: Vec<Person>,
}

fn runtime(minutes: i64) -> Option<String> {
    match minutes {
        m if m <= 0 => None,
        m if m < 60 => Some(format!("{m}m")),
        m => Some(format!("{}h {}m", m / 60, m % 60)),
    }
}

fn votes(n: i64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn date(v: &Value) -> Option<String> {
    let d = v.as_str()?.get(..10)?;
    let parsed = chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()?;
    Some(parsed.format("%-d %B %Y").to_string())
}

fn push(facts: &mut Vec<(String, String)>, label: &str, value: Option<String>) {
    if let Some(v) = value.filter(|v| !v.trim().is_empty()) {
        facts.push((label.into(), v));
    }
}

async fn get(url: String) -> Option<Value> {
    http().get(url).send().await.ok()?.json().await.ok()
}

async fn movie(tmdb: i64) -> Option<Details> {
    let m = get(format!("{RADARR_META}/{tmdb}")).await?;
    let mut d = Details::default();
    if let Some(r) = m["Ratings"].as_array().and_then(|r| r.iter().find(|x| x["Origin"] == "Tmdb")) {
        push(&mut d.facts, "Rating", r["Value"].as_f64().map(|v| format!("{v:.1} / 10 ({} votes)", votes(r["Count"].as_i64().unwrap_or(0)))));
    }
    push(&mut d.facts, "Runtime", m["Runtime"].as_i64().and_then(runtime));
    let certs = m["Certifications"].as_array().cloned().unwrap_or_default();
    let cert = certs.iter().find(|c| c["Country"] == "US").or_else(|| certs.first()).and_then(|c| c["Certification"].as_str()).map(String::from);
    push(&mut d.facts, "Rated", cert);
    push(&mut d.facts, "In cinemas", date(&m["InCinema"]));
    push(&mut d.facts, "Digital release", date(&m["DigitalRelease"]));
    push(&mut d.facts, "Studio", m["Studio"].as_str().map(String::from));
    push(&mut d.facts, "Language", m["OriginalLanguage"]["Name"].as_str().map(String::from));
    push(&mut d.facts, "Collection", m["Collection"]["Name"].as_str().map(String::from));
    let crew = m["Credits"]["Crew"].as_array().cloned().unwrap_or_default();
    let names = |f: &dyn Fn(&Value) -> bool| {
        let mut v: Vec<String> = crew.iter().filter(|c| f(c)).filter_map(|c| c["Name"].as_str().map(String::from)).collect();
        v.dedup();
        v.truncate(4);
        (!v.is_empty()).then(|| v.join(", "))
    };
    if let Some(n) = names(&|c| c["Job"] == "Director") {
        d.crew.push(("Director".into(), n));
    }
    if let Some(n) = names(&|c| c["Department"] == "Writing") {
        d.crew.push(("Writers".into(), n));
    }
    d.cast = m["Credits"]["Cast"]
        .as_array()
        .into_iter()
        .flatten()
        .take(24)
        .map(|c| Person {
            name: c["Name"].as_str().unwrap_or_default().into(),
            role: c["Character"].as_str().filter(|s| !s.is_empty()).map(String::from),
            photo: c["Images"][0]["Url"].as_str().map(|u| u.replace("/original/", "/w185/")),
        })
        .collect();
    Some(d)
}

async fn show(tvdb: i64) -> Option<Details> {
    let s = get(format!("{SKYHOOK}/{tvdb}")).await?;
    let mut d = Details::default();
    push(&mut d.facts, "Rating", s["rating"]["value"].as_str().and_then(|v| v.parse::<f64>().ok()).or(s["rating"]["value"].as_f64()).filter(|v| *v > 0.0).map(|v| format!("{v:.1} / 10 ({} votes)", votes(s["rating"]["count"].as_i64().unwrap_or(0)))));
    let eps = s["episodes"].as_array().cloned().unwrap_or_default();
    let seasons = s["seasons"].as_array().map(|a| a.iter().filter(|x| x["seasonNumber"].as_i64().unwrap_or(0) > 0).count()).unwrap_or(0);
    let regular = eps.iter().filter(|e| e["seasonNumber"].as_i64().unwrap_or(0) > 0).count();
    if seasons > 0 {
        push(&mut d.facts, "Seasons", Some(format!("{seasons} ({regular} episodes)")));
    }
    push(&mut d.facts, "Episode length", s["runtime"].as_i64().and_then(runtime));
    push(&mut d.facts, "Rated", s["contentRating"].as_str().map(String::from));
    push(&mut d.facts, "Network", s["network"].as_str().or(s["originalNetwork"].as_str()).map(String::from));
    push(&mut d.facts, "Status", s["status"].as_str().map(|x| {
        let mut c = x.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
    }));
    push(&mut d.facts, "First aired", date(&s["firstAired"]));
    push(&mut d.facts, "Last aired", date(&s["lastAired"]));
    d.cast = s["actors"]
        .as_array()
        .into_iter()
        .flatten()
        .take(24)
        .map(|a| Person { name: a["name"].as_str().unwrap_or_default().into(), role: a["character"].as_str().filter(|s| !s.is_empty()).map(String::from), photo: a["image"].as_str().map(String::from) })
        .collect();
    Some(d)
}

async fn anime(id: i64) -> Option<Details> {
    let q = format!(
        "{{ Media(id: {id}) {{ format episodes duration status season seasonYear averageScore popularity source startDate {{ year month day }} endDate {{ year month day }} studios(isMain: true) {{ nodes {{ name }} }} staff(perPage: 6, sort: RELEVANCE) {{ edges {{ role node {{ name {{ full }} }} }} }} characters(perPage: 24, sort: [ROLE, RELEVANCE]) {{ edges {{ role node {{ name {{ full }} image {{ medium }} }} voiceActors(language: JAPANESE) {{ name {{ full }} }} }} }} }} }}"
    );
    let v: Value = http().post("https://graphql.anilist.co").json(&json!({ "query": q })).send().await.ok()?.json().await.ok()?;
    let m = &v["data"]["Media"];
    if m.is_null() {
        return None;
    }
    let mut d = Details::default();
    push(&mut d.facts, "Rating", m["averageScore"].as_i64().map(|s| format!("{:.1} / 10 ({} fans)", s as f64 / 10.0, votes(m["popularity"].as_i64().unwrap_or(0)))));
    push(&mut d.facts, "Episodes", m["episodes"].as_i64().map(|n| n.to_string()));
    push(&mut d.facts, "Episode length", m["duration"].as_i64().and_then(runtime));
    let title_case = |s: &str| s.split('_').map(|w| { let mut c = w.chars(); c.next().map(|f| f.to_string() + &c.as_str().to_lowercase()).unwrap_or_default() }).collect::<Vec<_>>().join(" ");
    push(&mut d.facts, "Status", m["status"].as_str().map(title_case));
    if let (Some(s), Some(y)) = (m["season"].as_str(), m["seasonYear"].as_i64()) {
        push(&mut d.facts, "Season", Some(format!("{} {y}", title_case(s))));
    }
    push(&mut d.facts, "Studio", m["studios"]["nodes"][0]["name"].as_str().map(String::from));
    push(&mut d.facts, "Based on", m["source"].as_str().map(title_case));
    for e in m["staff"]["edges"].as_array().into_iter().flatten().take(4) {
        if let (Some(role), Some(name)) = (e["role"].as_str(), e["node"]["name"]["full"].as_str()) {
            d.crew.push((role.into(), name.into()));
        }
    }
    d.cast = m["characters"]["edges"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| {
            let va = e["voiceActors"][0]["name"]["full"].as_str();
            Person {
                name: e["node"]["name"]["full"].as_str().unwrap_or_default().into(),
                role: Some(match va { Some(v) => format!("Voiced by {v}"), None => e["role"].as_str().map(title_case).unwrap_or_default() }),
                photo: e["node"]["image"]["medium"].as_str().map(String::from),
            }
        })
        .collect();
    Some(d)
}

/// From Jellyfin, for library titles the outside sources don't match.
async fn library(app: &AppHandle, id: &str) -> Option<(Details, Option<i64>, Option<i64>)> {
    let j = jf(app).filter(|j| j.user_id.is_some())?;
    let v = j.get_json(&format!("/Items?userId={}&ids={id}&fields=People,Studios,ProviderIds,OfficialRating,Genres,PremiereDate", j.user_id.clone()?)).await.ok()?;
    let it = &v["Items"][0];
    let mut d = Details::default();
    push(&mut d.facts, "Rating", it["CommunityRating"].as_f64().map(|r| format!("{r:.1} / 10")));
    push(&mut d.facts, "Runtime", it["RunTimeTicks"].as_i64().and_then(|t| runtime(t / 600_000_000)));
    push(&mut d.facts, "Rated", it["OfficialRating"].as_str().map(String::from));
    push(&mut d.facts, "Released", date(&it["PremiereDate"]));
    push(&mut d.facts, "Studio", it["Studios"][0]["Name"].as_str().map(String::from));
    let people = it["People"].as_array().cloned().unwrap_or_default();
    let directors: Vec<String> = people.iter().filter(|p| p["Type"] == "Director").filter_map(|p| p["Name"].as_str().map(String::from)).collect();
    if !directors.is_empty() {
        d.crew.push(("Director".into(), directors.join(", ")));
    }
    d.cast = people
        .iter()
        .filter(|p| p["Type"] == "Actor")
        .take(24)
        .map(|p| Person {
            name: p["Name"].as_str().unwrap_or_default().into(),
            role: p["Role"].as_str().filter(|s| !s.is_empty()).map(String::from),
            photo: p["PrimaryImageTag"].as_str().and_then(|tag| Some(format!("{}/Items/{}/Images/Primary?fillHeight=278&quality=85&tag={tag}", j.base, p["Id"].as_str()?))),
        })
        .collect();
    let tmdb = it["ProviderIds"]["Tmdb"].as_str().and_then(|s| s.parse().ok());
    let tvdb = it["ProviderIds"]["Tvdb"].as_str().and_then(|s| s.parse().ok());
    Some((d, tmdb, tvdb))
}

/// The details for a tile, remembered for the session.
pub async fn details(app: &AppHandle, tile: &Tile) -> Details {
    let key = tile.series_id.as_ref().map(|s| format!("jf:{s}")).unwrap_or_else(|| tile.key.clone());
    if let Some(d) = app.state::<Media>().details.lock().unwrap().get(&key) {
        return d.clone();
    }
    let (mut tmdb, mut tvdb) = (tile.tmdb, tile.tvdb);
    let mut lib = None;
    // An episode shows its show; a library title gets its outside ids from Jellyfin.
    if let Some(id) = tile.series_id.as_deref().or(tile.jf_id.as_deref()) {
        if let Some((d, tm, tv)) = library(app, id).await {
            tmdb = tmdb.or(tm);
            tvdb = tvdb.or(tv);
            lib = Some(d);
        }
    }
    let is_movie = tile.kind == "movie" || tile.format.as_deref() == Some("MOVIE");
    let anilist = tile.key.strip_prefix("anilist:").and_then(|s| s.parse::<i64>().ok());
    let found = match (anilist, is_movie, tmdb, tvdb) {
        (Some(id), _, _, _) => anime(id).await,
        (None, true, Some(t), _) => movie(t).await,
        (None, false, _, Some(t)) => show(t).await,
        _ => None,
    };
    let mut d = match (found, lib) {
        (Some(mut f), Some(l)) => {
            // Outside data is richer; Jellyfin fills in a cast it lacks.
            if f.cast.is_empty() {
                f.cast = l.cast;
            }
            f
        }
        (Some(f), None) => f,
        (None, Some(l)) => l,
        (None, None) => Details::default(),
    };
    d.cast.retain(|p| !p.name.is_empty());
    app.state::<Media>().details.lock().unwrap().insert(key, d.clone());
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_read_like_people_write_them() {
        assert_eq!(runtime(145).as_deref(), Some("2h 25m"));
        assert_eq!(runtime(24).as_deref(), Some("24m"));
        assert_eq!(runtime(0), None);
        assert_eq!(votes(5594), "5,594");
        assert_eq!(votes(1234567), "1,234,567");
        assert_eq!(date(&json!("2026-07-31T00:00:00Z")).as_deref(), Some("31 July 2026"));
    }
}
