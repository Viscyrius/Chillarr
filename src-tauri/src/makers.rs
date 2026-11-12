//! "Everything this person made": the page a director's, creator's or anime
//! staff member's name opens from the detail panel. Keyless sources, by the
//! id the detail panel found the person under:
//! - `tmdb:<id>`: a movie director. Wikidata lists what they directed or
//!   created (TMDb movie ids and TVDB show ids); Radarr's metadata server
//!   fills in the movies, TVmaze the shows.
//! - `tvmaze:<id>`: a series creator. TVmaze lists the shows they created.
//! - `anilist:<id>`: anime staff. AniList lists their anime.

use crate::media::{http, Media};
use crate::tiles::Tile;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

const RADARR_META: &str = "https://api.radarr.video/v1/movie";
const WIKIDATA: &str = "https://query.wikidata.org/sparql";
/// Up to this many titles on a page.
const MAX: usize = 80;
/// A person's list is asked again after this.
const FRESH: Duration = Duration::from_secs(60 * 60);

static CACHE: std::sync::Mutex<BTreeMap<String, (Instant, Vec<Tile>)>> = std::sync::Mutex::new(BTreeMap::new());

/// A person's id as the page takes it: (source, number).
pub fn parse(id: &str) -> Option<(&str, i64)> {
    let (src, n) = id.split_once(':')?;
    let n = n.parse::<i64>().ok().filter(|n| *n > 0)?;
    matches!(src, "tmdb" | "tvmaze" | "anilist").then_some((src, n))
}

/// Everything the person made, newest first, your copies swapped in.
/// `adult`: R and NC-17 films and ecchi anime too (the profile's 18+ is on).
pub async fn works(app: &AppHandle, id: &str, adult: bool) -> Result<Vec<Tile>, String> {
    let adult = adult && !crate::profiles::viewer_kids(app);
    let (src, n) = parse(id).ok_or("That person can't be looked up.")?;
    let ck = format!("{id}:{adult}");
    let cached = CACHE.lock().unwrap().get(&ck).filter(|(at, _)| at.elapsed() < FRESH).map(|(_, t)| t.clone());
    let list = match cached {
        Some(t) => t,
        None => {
            let mut t = match src {
                "tmdb" => from_wikidata(app, n, adult).await?,
                "tvmaze" => tvmaze_shows(n).await?,
                _ => anilist(n, adult).await?,
            };
            newest_first(&mut t);
            CACHE.lock().unwrap().insert(ck, (Instant::now(), t.clone()));
            t
        }
    };
    if list.is_empty() {
        return Ok(list);
    }
    let library = crate::media::explore_library(app).await;
    let tracked = crate::media::tracked(app).await;
    let owned = crate::catalog::merge_owned(list, &library);
    let mut seen = HashSet::new();
    let owned: Vec<Tile> = owned.into_iter().filter(|t| seen.insert(t.jf_id.clone().unwrap_or_else(|| t.key.clone()))).collect();
    Ok(crate::media::mark_tracked(owned, &tracked))
}

/// Newest first; titles with no year go last. Each title once.
pub fn newest_first(t: &mut Vec<Tile>) {
    let mut seen = HashSet::new();
    t.retain(|x| !x.title.is_empty() && seen.insert(x.key.clone()));
    t.sort_by(|a, b| b.year.unwrap_or(0).cmp(&a.year.unwrap_or(0)));
    t.truncate(MAX);
}

/// The TMDb movie ids and TVDB show ids in a Wikidata answer.
pub fn wikidata_ids(v: &Value) -> (Vec<i64>, Vec<i64>) {
    let (mut movies, mut shows) = (Vec::new(), Vec::new());
    for b in v["results"]["bindings"].as_array().into_iter().flatten() {
        if let Some(n) = b["tm"]["value"].as_str().and_then(|s| s.parse::<i64>().ok()) {
            if !movies.contains(&n) {
                movies.push(n);
            }
        } else if let Some(n) = b["tvdb"]["value"].as_str().and_then(|s| s.parse::<i64>().ok()) {
            if !shows.contains(&n) {
                shows.push(n);
            }
        }
    }
    (movies, shows)
}

async fn from_wikidata(app: &AppHandle, tmdb_person: i64, adult: bool) -> Result<Vec<Tile>, String> {
    // Directed (P57) or created (P170); P4985 is the person's TMDb id.
    let q = format!(
        "SELECT DISTINCT ?tm ?tvdb WHERE {{ ?p wdt:P4985 \"{tmdb_person}\" . {{ ?w wdt:P57 ?p }} UNION {{ ?w wdt:P170 ?p }} OPTIONAL {{ ?w wdt:P4947 ?tm }} OPTIONAL {{ ?w wdt:P4835 ?tvdb }} }}"
    );
    let v: Value = http()
        .get(WIKIDATA)
        .query(&[("query", q.as_str())])
        .header("Accept", "application/sparql-results+json")
        // Wikidata asks for a name it can reach.
        .header("User-Agent", "Chillarr (https://github.com/Viscyrius/Chillarr)")
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|_| "Wikidata isn't answering. Try again in a minute.".to_string())?
        .json()
        .await
        .map_err(|_| "Wikidata sent something odd. Try again in a minute.".to_string())?;
    let (movies, shows) = wikidata_ids(&v);
    let mut out = Vec::new();
    if !movies.is_empty() {
        if let Ok(r) = http().post(format!("{RADARR_META}/bulk")).json(&movies.iter().take(MAX).collect::<Vec<_>>()).send().await {
            let list: Vec<Value> = r.json().await.unwrap_or_default();
            out.extend(list.iter().filter(|m| adult || !crate::catalog::adult_cert(m["certification"].as_str().unwrap_or_default())).map(crate::catalog::from_meta));
        }
    }
    // Shows not known here are looked up all at once, not one after another.
    let mut lookups = tokio::task::JoinSet::new();
    for tvdb in shows.into_iter().take(20) {
        let known = app.state::<Media>().shows.lock().unwrap().by_tvdb(tvdb);
        match known {
            Some(t) => out.push(t),
            None => {
                lookups.spawn(async move { get(&format!("https://api.tvmaze.com/lookup/shows?thetvdb={tvdb}")).await });
            }
        }
    }
    while let Some(r) = lookups.join_next().await {
        if let Ok(Some(v)) = r {
            out.push(crate::tiles::from_tvmaze(&v));
        }
    }
    Ok(out)
}

async fn get(url: &str) -> Option<Value> {
    let r = http().get(url).send().await.ok()?;
    if !r.status().is_success() {
        return None;
    }
    r.json().await.ok()
}

/// The jobs on TVmaze that mean "made this show".
pub fn made_it(job: &str) -> bool {
    matches!(job, "Creator" | "Developer")
}

async fn tvmaze_shows(person: i64) -> Result<Vec<Tile>, String> {
    let v = get(&format!("https://api.tvmaze.com/people/{person}/crewcredits?embed=show")).await.ok_or("TVmaze isn't answering. Try again in a minute.")?;
    Ok(v.as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["type"].as_str().is_some_and(made_it))
        .map(|c| crate::tiles::from_tvmaze(&c["_embedded"]["show"]))
        .filter(|t| t.tvdb.is_some())
        .collect())
}

async fn anilist(staff: i64, adult: bool) -> Result<Vec<Tile>, String> {
    let q = format!(
        "{{ Staff(id: {staff}) {{ staffMedia(type: ANIME, sort: POPULARITY_DESC, perPage: 50) {{ nodes {{ id isAdult format genres title {{ english romaji }} coverImage {{ extraLarge }} bannerImage averageScore description(asHtml: false) seasonYear trailer {{ id site }} }} }} }} }}"
    );
    let v = crate::media::anilist(&json!({ "query": q })).await?;
    Ok(v["data"]["Staff"]["staffMedia"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["isAdult"] != true)
        .filter(|m| adult || !m["genres"].as_array().is_some_and(|g| g.iter().any(|g| g == "Ecchi")))
        .map(crate::tiles::from_anilist)
        .collect())
}

/// A series' creators on TVmaze, by its TVDB id: (name, TVmaze person id).
pub async fn show_creators(tvdb: i64) -> Vec<(String, i64)> {
    let Some(show) = get(&format!("https://api.tvmaze.com/lookup/shows?thetvdb={tvdb}")).await else { return vec![] };
    let Some(id) = show["id"].as_i64() else { return vec![] };
    let Some(crew) = get(&format!("https://api.tvmaze.com/shows/{id}/crew")).await else { return vec![] };
    creators(&crew)
}

pub fn creators(crew: &Value) -> Vec<(String, i64)> {
    let mut out: Vec<(String, i64)> = Vec::new();
    for c in crew.as_array().into_iter().flatten().filter(|c| c["type"].as_str().is_some_and(made_it)) {
        if let (Some(n), Some(id)) = (c["person"]["name"].as_str(), c["person"]["id"].as_i64()) {
            if !out.iter().any(|(_, i)| *i == id) {
                out.push((n.to_string(), id));
            }
        }
    }
    out.truncate(4);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_known_people_ids_are_looked_up() {
        assert_eq!(parse("tmdb:525"), Some(("tmdb", 525)));
        assert_eq!(parse("anilist:96879"), Some(("anilist", 96879)));
        assert_eq!(parse("tvmaze:0"), None);
        assert_eq!(parse("imdb:5"), None);
        assert_eq!(parse("tmdb"), None);
    }

    #[test]
    fn wikidata_answer_splits_movies_and_shows() {
        let v = json!({ "results": { "bindings": [
            { "tm": { "value": "27205" } }, { "tm": { "value": "27205" } }, { "tvdb": { "value": "81189" } }, {}
        ] } });
        assert_eq!(wikidata_ids(&v), (vec![27205], vec![81189]));
    }

    #[test]
    fn creators_are_the_people_who_made_the_show() {
        let crew = json!([
            { "type": "Creator", "person": { "id": 1, "name": "A" } },
            { "type": "Executive Producer", "person": { "id": 2, "name": "B" } },
            { "type": "Developer", "person": { "id": 3, "name": "C" } },
            { "type": "Creator", "person": { "id": 1, "name": "A" } }
        ]);
        assert_eq!(creators(&crew), vec![("A".to_string(), 1), ("C".to_string(), 3)]);
    }

    #[test]
    fn newest_first_without_repeats() {
        let t = |k: &str, y: Option<i64>| Tile { year: y, ..Tile::new(k, "movie", k) };
        let mut v = vec![t("a", Some(2001)), t("b", None), t("c", Some(2020)), t("a", Some(2001))];
        newest_first(&mut v);
        assert_eq!(v.iter().map(|x| x.key.as_str()).collect::<Vec<_>>(), ["c", "a", "b"]);
    }
}
