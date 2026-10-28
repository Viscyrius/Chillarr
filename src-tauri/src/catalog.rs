//! A movie catalog that grows: seeded from Radarr's own metadata lists
//! (TMDb trending/popular, IMDb popular/top 250) and extended by following
//! each movie's recommendations a few dozen at a time. Saved to disk, so it
//! keeps getting bigger the longer Chillarr runs. No API key is involved.

use crate::tiles::Tile;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

/// Votes a movie needs before it can be "top rated".
const MIN_VOTES: i64 = 200;

/// Popularity is live data: a title the rows can show is fetched again once
/// its numbers are older than this.
pub const REFRESH_AFTER: u64 = 24 * 60 * 60;
/// How many of the most popular movies are kept fresh. Every row (popular,
/// new, genres, Top 10) draws from the top of the list.
const HOT_MOVIES: usize = 1000;
/// The same for shows.
const HOT_SHOWS: usize = 300;

pub(crate) fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Entry {
    tile: Tile,
    popularity: f64,
    votes: i64,
    recs: Vec<i64>,
    /// When these numbers were fetched (unix seconds). 0 = before this was kept.
    #[serde(default)]
    seen_at: u64,
    /// TMDB id of its collection (the Iron Man films), so the shelf can bring a
    /// film's sequels along.
    #[serde(default)]
    collection: Option<i64>,
    /// Fetched since the collection was kept. Entries saved before it are
    /// fetched again once, so `collection` fills in.
    #[serde(default)]
    collection_known: bool,
    /// Length, age rating, language, studio and release dates, for Filters.
    #[serde(default, skip_serializing_if = "Facts::is_empty")]
    facts: Facts,
    /// Fetched since `facts` was kept. Older entries are fetched again in
    /// bulk (`facts_missing`), so the Filters fields fill in.
    #[serde(default)]
    facts_known: bool,
}

/// What Filters can ask of a movie or show beyond its tile. Empty fields are
/// left out of the saved catalogs, so they cost little on disk.
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Facts {
    /// ISO 639-1 code ("ko"), or the lowercased name when it has none here.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub lang: String,
    /// Shows: the network's or streaming service's country ("KR").
    #[serde(skip_serializing_if = "String::is_empty")]
    pub country: String,
    /// Minutes: a movie's length, a show's usual episode length.
    #[serde(skip_serializing_if = "is_zero16")]
    pub runtime: u16,
    /// Movies: the US age rating ("PG-13").
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cert: String,
    /// Movies: the studio. Shows: the network or streaming service.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub studio: String,
    /// Shows: TVmaze's type (Scripted, Animation, Reality...).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub show_type: String,
    /// Shows: Running, Ended, To Be Determined, In Development.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub status: String,
    /// Movies: first in cinemas, and first out to watch at home (yyyymmdd, 0 = unknown).
    #[serde(skip_serializing_if = "is_zero32")]
    pub cinema: u32,
    #[serde(skip_serializing_if = "is_zero32")]
    pub home: u32,
}

fn is_zero16(n: &u16) -> bool {
    *n == 0
}

fn is_zero32(n: &u32) -> bool {
    *n == 0
}

impl Facts {
    fn is_empty(&self) -> bool {
        *self == Facts::default()
    }

    /// A movie's, from Radarr's metadata server.
    pub fn of_movie(v: &Value) -> Facts {
        let date = |k: &str| v[k].as_str().map(yyyymmdd).unwrap_or(0);
        let home = [date("DigitalRelease"), date("PhysicalRelease")].into_iter().filter(|d| *d > 0).min().unwrap_or(0);
        let cert = v["Certifications"].as_array().and_then(|a| a.iter().find(|c| c["Country"] == "US")).and_then(|c| c["Certification"].as_str()).unwrap_or_default();
        Facts {
            lang: lang_code(v["OriginalLanguage"].as_str().unwrap_or_default()),
            runtime: v["Runtime"].as_u64().unwrap_or(0).min(u16::MAX as u64) as u16,
            cert: cert.trim().to_string(),
            studio: v["Studio"].as_str().unwrap_or_default().trim().to_string(),
            cinema: date("InCinema"),
            home,
            ..Default::default()
        }
    }

    /// A show's, from TVmaze.
    pub fn of_show(v: &Value) -> Facts {
        // A network (BBC One) or, for streaming shows, a web channel (Netflix).
        let by = if v["network"].is_object() { &v["network"] } else { &v["webChannel"] };
        let s = |x: &Value| x.as_str().unwrap_or_default().trim().to_string();
        Facts {
            lang: lang_code(v["language"].as_str().unwrap_or_default()),
            country: s(&by["country"]["code"]),
            runtime: v["averageRuntime"].as_u64().or(v["runtime"].as_u64()).unwrap_or(0).min(u16::MAX as u64) as u16,
            studio: s(&by["name"]),
            show_type: s(&v["type"]),
            status: s(&v["status"]),
            ..Default::default()
        }
    }
}

/// "2026-07-29T00:00:00Z" -> 20260729.
fn yyyymmdd(d: &str) -> u32 {
    d.get(..10).map(|x| x.replace('-', "")).and_then(|x| x.parse().ok()).unwrap_or(0)
}

/// Today as yyyymmdd (UTC).
pub fn today() -> u32 {
    use chrono::Datelike;
    let d = chrono::Utc::now().date_naive();
    d.year() as u32 * 10000 + d.month() * 100 + d.day()
}

/// One code per language: TMDb's codes and TVmaze's English names meet here.
/// Cantonese files under Chinese, Bokmal under Norwegian.
fn lang_code(s: &str) -> String {
    const NAMES: [(&str, &str); 26] = [
        ("english", "en"), ("korean", "ko"), ("japanese", "ja"), ("chinese", "zh"), ("cantonese", "zh"), ("mandarin", "zh"), ("spanish", "es"),
        ("french", "fr"), ("hindi", "hi"), ("german", "de"), ("italian", "it"), ("turkish", "tr"), ("thai", "th"), ("portuguese", "pt"),
        ("russian", "ru"), ("swedish", "sv"), ("danish", "da"), ("norwegian", "no"), ("dutch", "nl"), ("polish", "pl"), ("arabic", "ar"),
        ("tagalog", "tl"), ("indonesian", "id"), ("finnish", "fi"), ("hebrew", "he"), ("icelandic", "is"),
    ];
    let s = s.trim().to_ascii_lowercase();
    match s.as_str() {
        "cn" => "zh".into(),
        "nb" | "nn" => "no".into(),
        _ => NAMES.iter().find(|(n, _)| *n == s).map(|(_, c)| c.to_string()).unwrap_or(s),
    }
}

/// Shows in these languages are also found by their network's country: a
/// Korean drama TVmaze files under English still comes from KR.
fn lang_countries(code: &str) -> &'static [&'static str] {
    match code {
        "ko" => &["KR"],
        "ja" => &["JP"],
        "zh" => &["CN", "TW", "HK"],
        "th" => &["TH"],
        "tr" => &["TR"],
        _ => &[],
    }
}

/// Where a movie is: "released" (out to watch at home), "cinema" (in cinemas
/// only) or "soon". A film a year past cinemas, or an old one with no dates,
/// counts as out.
pub fn movie_stage(f: &Facts, year: Option<i64>, today: u32) -> &'static str {
    let year_now = (today / 10000) as i64;
    let out = (f.home > 0 && f.home <= today) || (f.cinema > 0 && f.cinema + 10000 <= today) || (f.home == 0 && f.cinema == 0 && year.is_some_and(|y| y < year_now));
    if out {
        "released"
    } else if f.cinema > 0 && f.cinema <= today {
        "cinema"
    } else {
        "soon"
    }
}

/// What the Filters panel asks for. Every part is optional.
#[derive(Deserialize, Default, Debug, Clone)]
#[serde(default)]
pub struct Filter {
    /// movie | series | anime
    pub kind: String,
    /// One genre, from links made before genres could be combined.
    pub genre: Option<String>,
    /// Every one of these.
    pub genres: Vec<String>,
    /// None of these.
    pub not_genres: Vec<String>,
    pub year_from: Option<i64>,
    pub year_to: Option<i64>,
    /// Out of 10.
    pub min_rating: Option<f64>,
    /// popular | top | new | title
    pub sort: String,
    /// Only what's in the library.
    pub owned: bool,
    // Anime only (AniList answers these; the saved catalogs have no such data).
    /// AniList tags: every one of these, none of `not_tags`.
    pub tags: Vec<String>,
    pub not_tags: Vec<String>,
    /// TV, TV_SHORT, MOVIE, OVA, ONA, SPECIAL: any of these.
    pub formats: Vec<String>,
    /// RELEASING, FINISHED, NOT_YET_RELEASED.
    pub status: Option<String>,
    /// WINTER, SPRING, SUMMER, FALL.
    pub season: Option<String>,
    pub episodes_min: Option<i64>,
    pub episodes_max: Option<i64>,
    // Movies and series (the saved catalogs' `Facts`). `status` above also
    // takes released | cinema | soon (movies) and TVmaze's status (series).
    /// Original language codes ("ko"): any of these, none of `not_languages`.
    pub languages: Vec<String>,
    pub not_languages: Vec<String>,
    /// Series: the network's country ("GB"), any of these.
    pub countries: Vec<String>,
    /// Minutes: a movie's length, a show's episode length.
    pub runtime_min: Option<i64>,
    pub runtime_max: Option<i64>,
    /// Movies: US age ratings, any of these.
    pub certs: Vec<String>,
    /// Movies: studios. Series: networks or streaming services. Any of these.
    pub studios: Vec<String>,
    /// Series: TVmaze's types, any of these and none of `not_show_types`.
    pub show_types: Vec<String>,
    pub not_show_types: Vec<String>,
}

impl Filter {
    /// The genres asked for, old single-genre links included.
    pub fn wanted_genres(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.genres.iter().map(String::as_str).chain(self.genre.as_deref()).filter(|g| !g.is_empty()).collect();
        v.dedup();
        v
    }

    pub fn keeps(&self, t: &Tile) -> bool {
        let has = |g: &str| t.genres.iter().any(|x| x.eq_ignore_ascii_case(g));
        if !self.wanted_genres().into_iter().all(has) || self.not_genres.iter().any(|g| has(g)) {
            return false;
        }
        if self.year_from.is_some() || self.year_to.is_some() {
            let Some(y) = t.year else { return false };
            if self.year_from.is_some_and(|f| y < f) || self.year_to.is_some_and(|to| y > to) {
                return false;
            }
        }
        if let Some(r) = self.min_rating.filter(|r| *r > 0.0) {
            if t.rating.unwrap_or(0.0) < r {
                return false;
            }
        }
        true
    }

    /// The parts `keeps` can't see, checked against a catalog title's facts.
    /// A title missing a fact a filter asks about is left out.
    pub fn keeps_facts(&self, x: &Facts, t: &Tile, today: u32) -> bool {
        let series = self.kind == "series";
        let speaks = |code: &String| {
            let code = code.to_ascii_lowercase();
            x.lang == code || (series && !x.country.is_empty() && lang_countries(&code).contains(&x.country.as_str()))
        };
        if (!self.languages.is_empty() && !self.languages.iter().any(speaks)) || self.not_languages.iter().any(speaks) {
            return false;
        }
        let any_of = |want: &[String], have: &str| want.is_empty() || want.iter().any(|w| w.eq_ignore_ascii_case(have));
        if !any_of(&self.countries, &x.country) || !any_of(&self.certs, &x.cert) || !any_of(&self.studios, &x.studio) || !any_of(&self.show_types, &x.show_type) {
            return false;
        }
        if self.not_show_types.iter().any(|w| w.eq_ignore_ascii_case(&x.show_type)) {
            return false;
        }
        if self.runtime_min.is_some() || self.runtime_max.is_some() {
            let r = x.runtime as i64;
            if r == 0 || self.runtime_min.is_some_and(|m| r < m) || self.runtime_max.is_some_and(|m| r > m) {
                return false;
            }
        }
        match self.status.as_deref().filter(|s| !s.is_empty()) {
            Some(want) if series => x.status.eq_ignore_ascii_case(want),
            Some(want @ ("released" | "cinema" | "soon")) => movie_stage(x, t.year, today) == want,
            _ => true,
        }
    }
}

/// The most common studios (or networks), the biggest first.
fn top_studios<'a>(facts: impl Iterator<Item = &'a Facts>, n: usize) -> Vec<String> {
    let mut count: HashMap<&str, usize> = HashMap::new();
    for f in facts.filter(|f| !f.studio.is_empty()) {
        *count.entry(f.studio.as_str()).or_default() += 1;
    }
    let mut v: Vec<(&str, usize)> = count.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    v.into_iter().take(n).map(|(s, _)| s.to_string()).collect()
}

/// Sorts filter results; `weight` is popularity (movies) or followers (shows).
fn sort_filtered(v: &mut [(Tile, f64)], sort: &str) {
    let rating = |t: &Tile| t.rating.unwrap_or(0.0);
    match sort {
        "top" => v.sort_by(|a, b| rating(&b.0).partial_cmp(&rating(&a.0)).unwrap_or(std::cmp::Ordering::Equal).then(b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal))),
        "new" => v.sort_by(|a, b| b.0.year.cmp(&a.0.year).then(b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal))),
        "title" => v.sort_by(|a, b| a.0.title.to_lowercase().cmp(&b.0.title.to_lowercase())),
        _ => v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)),
    }
}

/// AniList arguments for a filter, used as the "q:" category of an anime row.
pub fn anilist_filter(f: &Filter) -> String {
    let mut parts = vec![format!(
        "sort: {}",
        match f.sort.as_str() {
            "top" => "SCORE_DESC",
            "new" => "START_DATE_DESC",
            "title" => "TITLE_ROMAJI",
            _ => "POPULARITY_DESC",
        }
    )];
    let genres = f.wanted_genres();
    if !genres.is_empty() {
        parts.push(format!("genre_in: {}", gql_list(genres)));
    }
    if !f.not_genres.is_empty() {
        parts.push(format!("genre_not_in: {}", gql_list(f.not_genres.iter().map(String::as_str))));
    }
    if !f.tags.is_empty() {
        parts.push(format!("tag_in: {}", gql_list(f.tags.iter().map(String::as_str))));
    }
    if !f.not_tags.is_empty() {
        parts.push(format!("tag_not_in: {}", gql_list(f.not_tags.iter().map(String::as_str))));
    }
    // Enum values go in bare, so only known ones get through.
    let formats: Vec<&str> = f.formats.iter().map(String::as_str).filter(|x| ["TV", "TV_SHORT", "MOVIE", "OVA", "ONA", "SPECIAL"].contains(x)).collect();
    if !formats.is_empty() {
        parts.push(format!("format_in: [{}]", formats.join(", ")));
    }
    if let Some(st) = f.status.as_deref().filter(|x| ["RELEASING", "FINISHED", "NOT_YET_RELEASED"].contains(x)) {
        parts.push(format!("status: {st}"));
    }
    let season = f.season.as_deref().filter(|x| ["WINTER", "SPRING", "SUMMER", "FALL"].contains(x));
    if let Some(se) = season {
        parts.push(format!("season: {se}"));
    }
    // A season with one year is AniList's own "Fall 2025" (seasonYear). Its
    // Winter starts in December, so a start-date range of that year would
    // miss shows that began the December before. Alone, a season is that
    // season in any year.
    let season_year = season.and(f.year_from).filter(|y| Some(*y) == f.year_to);
    if let Some(n) = f.episodes_min.filter(|n| *n > 0) {
        parts.push(format!("episodes_greater: {}", n - 1));
    }
    if let Some(n) = f.episodes_max.filter(|n| *n > 0) {
        parts.push(format!("episodes_lesser: {}", n + 1));
    }
    if let Some(y) = season_year {
        parts.push(format!("seasonYear: {y}"));
    } else {
        if let Some(y) = f.year_from {
            parts.push(format!("startDate_greater: {}", y * 10000 - 1));
        }
        if let Some(y) = f.year_to {
            parts.push(format!("startDate_lesser: {}", (y + 1) * 10000));
        }
    }
    if let Some(r) = f.min_rating.filter(|r| *r > 0.0) {
        parts.push(format!("averageScore_greater: {}", (r * 10.0) as i64 - 1));
    }
    if f.sort == "top" {
        // Scores from a handful of fans aren't a ranking.
        parts.push("popularity_greater: 2000".into());
    }
    format!("q:{}", parts.join(", "))
}

/// A GraphQL list of strings, keeping only characters AniList names use.
fn gql_list<'a>(items: impl IntoIterator<Item = &'a str>) -> String {
    let safe: Vec<String> = items
        .into_iter()
        .map(|g| g.chars().filter(|c| c.is_alphanumeric() || " -'&".contains(*c)).collect::<String>())
        .filter(|g| !g.is_empty())
        .map(|g| format!("\"{g}\""))
        .collect();
    format!("[{}]", safe.join(", "))
}

/// The catalog stops growing here (a few MB on disk).
pub const CAP: usize = 30_000;

fn default_cap() -> usize {
    CAP
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Catalog {
    movies: HashMap<i64, Entry>,
    frontier: VecDeque<i64>,
    /// Ids already in `frontier`, so checking is instant.
    #[serde(default)]
    queued: HashSet<i64>,
    /// Ids the server has no movie for (404). Never asked again.
    #[serde(default, alias = "handed")]
    dead: HashSet<i64>,
    /// TMDb's trending list, in its order: the Top 10.
    #[serde(default)]
    trending: Vec<i64>,
    /// When the seed lists were last fetched (unix seconds).
    #[serde(default)]
    pub seeded_at: u64,
    /// IMDb's top 250 is big (13 MB), so it's fetched weekly.
    #[serde(default)]
    pub top250_at: u64,
    #[serde(default = "default_cap")]
    pub cap: usize,
    /// When each collection's films were last fetched (unix seconds).
    #[serde(default)]
    collections_at: HashMap<i64, u64>,
}

impl Default for Catalog {
    fn default() -> Self {
        Catalog {
            movies: HashMap::new(),
            frontier: VecDeque::new(),
            queued: HashSet::new(),
            dead: HashSet::new(),
            trending: Vec::new(),
            seeded_at: 0,
            top250_at: 0,
            cap: CAP,
            collections_at: HashMap::new(),
        }
    }
}

fn image(v: &Value, kind: &str, size: &str) -> Option<String> {
    v["Images"]
        .as_array()?
        .iter()
        .find(|i| i["CoverType"].as_str().is_some_and(|c| c.eq_ignore_ascii_case(kind)))
        .and_then(|i| i["Url"].as_str())
        .map(|u| u.replace("/original/", &format!("/{size}/")))
}

/// A movie from Radarr's metadata server as a tile.
pub fn from_meta(v: &Value) -> Tile {
    let tmdb = v["TmdbId"].as_i64();
    Tile {
        key: format!("tmdb:{}", tmdb.unwrap_or_default()),
        kind: "movie".into(),
        title: v["Title"].as_str().unwrap_or_default().into(),
        year: v["Year"].as_i64().filter(|y| *y > 0),
        overview: v["Overview"].as_str().map(String::from).filter(|s| !s.is_empty()),
        poster: image(v, "poster", "w500"),
        backdrop: image(v, "fanart", "w1280"),
        rating: v["MovieRatings"]["Tmdb"]["Value"].as_f64().filter(|r| *r > 0.0).map(|r| (r * 10.0).round() / 10.0),
        tmdb,
        genres: v["Genres"].as_array().map(|g| g.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
        trailer: v["YoutubeTrailerId"].as_str().map(String::from).filter(|s| !s.is_empty()),
        ..Default::default()
    }
}

impl Catalog {
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.movies.len()
    }

    /// Adds (or refreshes) one movie and queues its recommendations to fetch.
    pub fn add(&mut self, v: &Value) {
        self.add_at(v, now_secs());
    }

    fn add_at(&mut self, v: &Value, now: u64) {
        let Some(id) = v["TmdbId"].as_i64() else { return };
        let tile = from_meta(v);
        if tile.title.is_empty() || tile.poster.is_none() {
            return;
        }
        let recs: Vec<i64> = v["Recommendations"].as_array().map(|r| r.iter().filter_map(|x| x["TmdbId"].as_i64()).collect()).unwrap_or_default();
        for r in &recs {
            if !self.movies.contains_key(r) && !self.dead.contains(r) && self.queued.insert(*r) {
                self.frontier.push_back(*r);
            }
        }
        let entry = Entry {
            tile,
            popularity: v["Popularity"].as_f64().unwrap_or(0.0),
            votes: v["MovieRatings"]["Tmdb"]["Count"].as_i64().unwrap_or(0),
            recs,
            seen_at: now,
            collection: v["Collection"]["TmdbId"].as_i64().filter(|c| *c > 0),
            collection_known: true,
            facts: Facts::of_movie(v),
            // A collection's parts may come back short; only a full record counts.
            facts_known: v.get("Runtime").is_some(),
        };
        self.movies.insert(id, entry);
    }

    /// Takes in a collection from Radarr's metadata server (`/movie/collection/<id>`):
    /// every film in it, each marked as part of it.
    pub fn add_collection(&mut self, v: &Value) {
        self.add_collection_at(v, now_secs());
    }

    fn add_collection_at(&mut self, v: &Value, now: u64) {
        let Some(cid) = v["TmdbId"].as_i64() else { return };
        for part in v["Parts"].as_array().into_iter().flatten() {
            self.add_at(part, now);
            if let Some(e) = part["TmdbId"].as_i64().and_then(|id| self.movies.get_mut(&id)) {
                e.collection = Some(cid);
            }
        }
        self.collections_at.insert(cid, now);
    }

    /// The collections of these films whose members haven't been fetched in a
    /// week (a new sequel joins its collection), at most `n`.
    pub fn collections_to_fetch(&self, ids: &[i64], now: u64, n: usize) -> Vec<i64> {
        let mut out = Vec::new();
        for c in ids.iter().filter_map(|id| self.movies.get(id)?.collection) {
            let fresh = self.collections_at.get(&c).is_some_and(|at| now.saturating_sub(*at) < 7 * 24 * 3600);
            if !fresh && !out.contains(&c) {
                out.push(c);
                if out.len() >= n {
                    break;
                }
            }
        }
        out
    }

    pub fn collection_of(&self, id: i64) -> Option<i64> {
        self.movies.get(&id)?.collection
    }

    /// The known films of each of these collections, in release order.
    pub fn collection_members(&self, collections: &HashSet<i64>) -> HashMap<i64, Vec<Tile>> {
        let mut out: HashMap<i64, Vec<&Entry>> = HashMap::new();
        if collections.is_empty() {
            return HashMap::new();
        }
        for e in self.movies.values() {
            if let Some(c) = e.collection.filter(|c| collections.contains(c)) {
                out.entry(c).or_default().push(e);
            }
        }
        out.into_iter()
            .map(|(c, mut v)| {
                v.sort_by_key(|e| (e.tile.year.unwrap_or(i64::MAX), e.tile.tmdb));
                (c, v.into_iter().map(|e| e.tile.clone()).collect())
            })
            .collect()
    }

    /// Up to `n` movies the rows can show whose numbers have gone stale,
    /// the stalest first: the trending list and the most popular movies.
    pub fn stale(&self, now: u64, n: usize) -> Vec<i64> {
        let mut hot: Vec<(&i64, &Entry)> = self.movies.iter().collect();
        hot.sort_by(|a, b| b.1.popularity.partial_cmp(&a.1.popularity).unwrap_or(std::cmp::Ordering::Equal));
        hot.truncate(HOT_MOVIES);
        hot.extend(self.trending.iter().filter_map(|id| self.movies.get_key_value(id)));
        hot.retain(|(_, e)| !e.collection_known || now.saturating_sub(e.seen_at) > REFRESH_AFTER);
        hot.sort_by_key(|(id, e)| (e.seen_at, **id));
        hot.dedup_by_key(|(id, _)| **id);
        hot.into_iter().take(n).map(|(id, _)| *id).collect()
    }

    /// Up to `n` recommended movies not fetched yet. Call `retry` for any that
    /// couldn't be fetched (offline) and `dead` for ones that don't exist.
    pub fn next_to_fetch(&mut self, n: usize) -> Vec<i64> {
        let mut out = Vec::new();
        if self.movies.len() >= self.cap {
            return out;
        }
        while out.len() < n {
            let Some(id) = self.frontier.pop_front() else { break };
            self.queued.remove(&id);
            if self.movies.contains_key(&id) || self.dead.contains(&id) {
                continue;
            }
            out.push(id);
        }
        out
    }

    /// Puts ids back at the front of the queue (they couldn't be fetched this time).
    pub fn retry(&mut self, ids: &[i64]) {
        for id in ids.iter().rev() {
            if !self.movies.contains_key(id) && !self.dead.contains(id) && self.queued.insert(*id) {
                self.frontier.push_front(*id);
            }
        }
    }

    pub fn dead(&mut self, id: i64) {
        self.dead.insert(id);
    }

    /// Moves ids to the front of the queue (your own movies' recommendations).
    pub fn prioritize(&mut self, ids: &[i64]) {
        let ids: Vec<i64> = ids.iter().copied().filter(|id| !self.movies.contains_key(id) && !self.dead.contains(id)).collect();
        self.frontier.retain(|x| !ids.contains(x));
        for id in ids.iter().rev() {
            self.queued.insert(*id);
            self.frontier.push_front(*id);
        }
    }

    pub fn set_trending(&mut self, ids: Vec<i64>) {
        self.trending = ids;
    }

    /// The Top 10: TMDb's trending movies, most popular first. The server hands
    /// the list back sorted by TMDb id, not by rank, so its order means nothing.
    pub fn top10(&self) -> Vec<Tile> {
        let mut v: Vec<&Entry> = self.trending.iter().filter_map(|id| self.movies.get(id)).collect();
        v.sort_by(|a, b| b.popularity.partial_cmp(&a.popularity).unwrap_or(std::cmp::Ordering::Equal));
        let t = Self::take(v, 10);
        if t.is_empty() { self.popular(10) } else { t }
    }

    pub fn has(&self, id: i64) -> bool {
        self.movies.contains_key(&id)
    }

    pub fn recommendations(&self, id: i64) -> Vec<i64> {
        self.movies.get(&id).map(|e| e.recs.clone()).unwrap_or_default()
    }

    pub fn get(&self, id: i64) -> Option<Tile> {
        self.movies.get(&id).map(|e| e.tile.clone())
    }

    /// Movies like one, best first: TMDb's recommendations for it in TMDb's
    /// order, then the movies those recommend most often (ties: the more
    /// popular). Only movies the catalog has are listed; `similar_missing`
    /// names the rest, so they can be fetched.
    pub fn similar_movies(&self, id: i64, n: usize) -> Vec<Tile> {
        let (first, second) = self.similar_ids(id);
        first.into_iter().chain(second).filter_map(|x| self.movies.get(&x)).take(n).map(|e| e.tile.clone()).collect()
    }

    /// What `similar_movies` would list but hasn't fetched yet, nearest first:
    /// (TMDb's own recommendations, the ones after them).
    pub fn similar_missing(&self, id: i64) -> (Vec<i64>, Vec<i64>) {
        let (first, second) = self.similar_ids(id);
        let gone = |x: &i64| !self.movies.contains_key(x) && !self.dead.contains(x);
        (first.into_iter().filter(gone).collect(), second.into_iter().filter(gone).collect())
    }

    fn similar_ids(&self, id: i64) -> (Vec<i64>, Vec<i64>) {
        let mut first: Vec<i64> = Vec::new();
        for r in self.recommendations(id) {
            if r != id && !first.contains(&r) {
                first.push(r);
            }
        }
        let mut count: HashMap<i64, usize> = HashMap::new();
        for r in &first {
            for x in self.recommendations(*r) {
                if x != id && !first.contains(&x) {
                    *count.entry(x).or_default() += 1;
                }
            }
        }
        let pop = |x: i64| self.movies.get(&x).map_or(-1.0, |e| e.popularity);
        let mut second: Vec<(i64, usize)> = count.into_iter().collect();
        second.sort_by(|a, b| b.1.cmp(&a.1).then(pop(b.0).partial_cmp(&pop(a.0)).unwrap_or(std::cmp::Ordering::Equal)).then(a.0.cmp(&b.0)));
        (first, second.into_iter().map(|(x, _)| x).collect())
    }

    fn sorted<F: Fn(&Entry) -> bool>(&self, keep: F, by_rating: bool) -> Vec<&Entry> {
        let mut v: Vec<&Entry> = self.movies.values().filter(|e| keep(e)).collect();
        if by_rating {
            v.sort_by(|a, b| b.tile.rating.partial_cmp(&a.tile.rating).unwrap_or(std::cmp::Ordering::Equal));
        } else {
            v.sort_by(|a, b| b.popularity.partial_cmp(&a.popularity).unwrap_or(std::cmp::Ordering::Equal));
        }
        v
    }

    fn take(v: Vec<&Entry>, n: usize) -> Vec<Tile> {
        v.into_iter().take(n).map(|e| e.tile.clone()).collect()
    }

    pub fn popular(&self, n: usize) -> Vec<Tile> {
        Self::take(self.sorted(|_| true, false), n)
    }

    pub fn top_rated(&self, n: usize) -> Vec<Tile> {
        Self::take(self.sorted(|e| e.votes >= MIN_VOTES, true), n)
    }

    /// This year's and last year's movies, most popular first.
    pub fn new_releases(&self, this_year: i64, n: usize) -> Vec<Tile> {
        Self::take(self.sorted(|e| e.tile.year.is_some_and(|y| y >= this_year - 1), false), n)
    }

    pub fn by_genre(&self, genre: &str, n: usize) -> Vec<Tile> {
        Self::take(self.sorted(|e| e.tile.genres.iter().any(|g| g == genre), false), n)
    }

    /// Genres with at least `min` movies, the biggest first.
    pub fn genres(&self, min: usize) -> Vec<String> {
        let mut count: HashMap<&str, usize> = HashMap::new();
        for e in self.movies.values() {
            for g in &e.tile.genres {
                *count.entry(g.as_str()).or_default() += 1;
            }
        }
        let mut v: Vec<(&str, usize)> = count.into_iter().filter(|(_, c)| *c >= min).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        v.into_iter().map(|(g, _)| g.to_string()).collect()
    }

    /// One page of a category: "popular", "top", "new:<year>", "genre:<name>".
    pub fn page(&self, category: &str, page: usize, size: usize) -> Vec<Tile> {
        let list: Vec<&Entry> = if let Some(g) = category.strip_prefix("genre:") {
            self.sorted(|e| e.tile.genres.iter().any(|x| x == g), false)
        } else if let Some(y) = category.strip_prefix("new:") {
            let y: i64 = y.parse().unwrap_or(0);
            self.sorted(|e| e.tile.year.is_some_and(|x| x >= y - 1), false)
        } else if category == "top" {
            self.sorted(|e| e.votes >= MIN_VOTES, true)
        } else {
            self.sorted(|_| true, false)
        };
        list.into_iter().skip(page * size).take(size).map(|e| e.tile.clone()).collect()
    }

    /// Movies whose title matches a search: (tile, how close, TMDb popularity),
    /// the closest and then the most popular first. No network: the saved catalog.
    pub fn matching(&self, term: &str, n: usize) -> Vec<(Tile, u8, f64)> {
        let mut v: Vec<(&Entry, u8)> = self.movies.values().filter_map(|e| crate::media::match_rank(&e.tile.title, term).map(|r| (e, r))).collect();
        v.sort_by(|a, b| a.1.cmp(&b.1).then(b.0.popularity.partial_cmp(&a.0.popularity).unwrap_or(std::cmp::Ordering::Equal)));
        v.into_iter().take(n).map(|(e, r)| (e.tile.clone(), r, e.popularity)).collect()
    }

    /// Up to `n` movies saved before `facts` was kept, the most popular first,
    /// to fetch again in bulk.
    pub fn facts_missing(&self, n: usize) -> Vec<i64> {
        let mut v: Vec<(&i64, &Entry)> = self.movies.iter().filter(|(_, e)| !e.facts_known).collect();
        v.sort_by(|a, b| b.1.popularity.partial_cmp(&a.1.popularity).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(b.0)));
        v.into_iter().take(n).map(|(id, _)| *id).collect()
    }

    /// Asked for again and not sent back: don't ask for them again.
    pub fn facts_gone(&mut self, ids: &[i64]) {
        for id in ids {
            if let Some(e) = self.movies.get_mut(id) {
                e.facts_known = true;
            }
        }
    }

    /// The studios Filters offers, the biggest first.
    pub fn studios(&self, n: usize) -> Vec<String> {
        top_studios(self.movies.values().map(|e| &e.facts), n)
    }

    /// One page of what a filter keeps, sorted as asked.
    pub fn filtered(&self, f: &Filter, page: usize, size: usize) -> Vec<Tile> {
        let today = today();
        let mut v: Vec<(Tile, f64)> = self
            .movies
            .values()
            .filter(|e| f.keeps(&e.tile) && (f.sort != "top" || e.votes >= MIN_VOTES) && f.keeps_facts(&e.facts, &e.tile, today))
            .map(|e| (e.tile.clone(), e.popularity))
            .collect();
        sort_filtered(&mut v, &f.sort);
        v.into_iter().skip(page * size).take(size).map(|(t, _)| t).collect()
    }

    pub fn load(path: &Path) -> Catalog {
        std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        save_atomic(path, &serde_json::to_string(self).map_err(|e| e.to_string())?)
    }
}

/// Writes to a temp file and moves it into place, so a crash mid-write never
/// leaves a broken catalog behind.
fn save_atomic(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// Shows that grow page by page from TVmaze (keyless). Only shows lots of
/// people watch are kept.
#[derive(Serialize, Deserialize, Default, Debug)]
pub struct Shows {
    shows: HashMap<i64, (Tile, i64)>,
    pub next_page: u32,
    /// When each kept show's weight was fetched (unix seconds).
    #[serde(default)]
    seen: HashMap<i64, u64>,
    /// When the crawl last reached TVmaze's last page. A week later it starts
    /// over, so every show's weight is read again. 0 = still crawling.
    #[serde(default)]
    pub crawled_at: u64,
    /// Language, network, type, status and episode length, for Filters.
    #[serde(default)]
    facts: HashMap<i64, Facts>,
    /// The crawl has run since `facts` was kept. A catalog saved before that
    /// starts its crawl over once, so every show's facts fill in.
    #[serde(default)]
    facts_crawl: bool,
}

/// A finished crawl starts over after this.
pub const RECRAWL_AFTER: u64 = 7 * 24 * 60 * 60;

const MIN_WEIGHT: i64 = 85;

impl Shows {
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.shows.len()
    }

    pub fn add_page(&mut self, page: &[Value]) {
        self.add_shows(page, now_secs());
        self.next_page += 1;
    }

    /// Adds or refreshes shows without moving the crawl on (a page asked
    /// again, or single shows from `/shows/<id>`). A show that has dropped
    /// below the bar leaves the catalog.
    pub fn add_shows(&mut self, page: &[Value], now: u64) {
        for v in page {
            let weight = v["weight"].as_i64().unwrap_or(0);
            let Some(id) = v["id"].as_i64() else { continue };
            if weight < MIN_WEIGHT || !v["externals"]["thetvdb"].is_number() || v["image"].is_null() {
                self.forget(id);
                continue;
            }
            self.seen.insert(id, now);
            let mut t = crate::tiles::from_tvmaze(v);
            t.genres = v["genres"].as_array().map(|g| g.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
            t.poster = v["image"]["original"].as_str().or(v["image"]["medium"].as_str()).map(String::from);
            self.shows.insert(id, (t, weight));
            self.facts.insert(id, Facts::of_show(v));
        }
    }

    /// The show with this TVDB id, if the catalog has it.
    pub fn by_tvdb(&self, tvdb: i64) -> Option<Tile> {
        self.shows.values().find(|(t, _)| t.tvdb == Some(tvdb)).map(|(t, _)| t.clone())
    }

    /// A show TVmaze no longer has, or that too few people watch now.
    pub fn forget(&mut self, id: i64) {
        self.shows.remove(&id);
        self.seen.remove(&id);
        self.facts.remove(&id);
    }

    /// Up to `n` of the most watched shows whose weight has gone stale, the
    /// stalest first.
    pub fn stale(&self, now: u64, n: usize) -> Vec<i64> {
        let mut hot: Vec<(i64, i64)> = self.shows.iter().map(|(id, (_, w))| (*id, *w)).collect();
        hot.sort_by(|a, b| b.1.cmp(&a.1));
        hot.truncate(HOT_SHOWS);
        let seen = |id: i64| self.seen.get(&id).copied().unwrap_or(0);
        let mut v: Vec<i64> = hot.into_iter().map(|(id, _)| id).filter(|id| now.saturating_sub(seen(*id)) > REFRESH_AFTER).collect();
        v.sort_by_key(|id| (seen(*id), *id));
        v.truncate(n);
        v
    }

    /// TVmaze answered an empty page: the crawl has caught up. New shows land
    /// on the last page, so that one is asked again each time (`next_page - 1`),
    /// and a week after the crawl first caught up it starts over.
    pub fn reached_end(&mut self, now: u64) {
        if self.crawled_at == 0 {
            self.crawled_at = now;
        } else if now.saturating_sub(self.crawled_at) > RECRAWL_AFTER {
            self.next_page = 0;
            self.crawled_at = 0;
        }
    }

    fn sorted<F: Fn(&Tile) -> bool>(&self, keep: F, by_rating: bool) -> Vec<Tile> {
        let mut v: Vec<&(Tile, i64)> = self.shows.values().filter(|(t, _)| keep(t)).collect();
        let r = |t: &Tile| (t.rating.unwrap_or(0.0) * 100.0) as i64;
        if by_rating {
            v.sort_by(|a, b| r(&b.0).cmp(&r(&a.0)).then(b.1.cmp(&a.1)));
        } else {
            v.sort_by(|a, b| b.1.cmp(&a.1).then(r(&b.0).cmp(&r(&a.0))));
        }
        v.into_iter().map(|(t, _)| t.clone()).collect()
    }

    pub fn popular(&self, n: usize) -> Vec<Tile> {
        self.sorted(|_| true, false).into_iter().take(n).collect()
    }

    pub fn top_rated(&self, n: usize) -> Vec<Tile> {
        self.sorted(|_| true, true).into_iter().take(n).collect()
    }

    pub fn by_genre(&self, genre: &str, n: usize) -> Vec<Tile> {
        self.sorted(|t| t.genres.iter().any(|g| g == genre), false).into_iter().take(n).collect()
    }

    /// Shows like one you watched: the most genres in common first, then the
    /// most watched. TVmaze has no recommendations, so genres stand in.
    /// Jellyfin and TVmaze name a few genres differently ("Sci-Fi & Fantasy"),
    /// so a genre matches on any word they share ("Sci" matches "Science").
    pub fn similar(&self, genres: &[String], not_tvdb: Option<i64>, n: usize) -> Vec<Tile> {
        let words_of = |g: &str| -> Vec<String> { g.split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() > 2 && !w.eq_ignore_ascii_case("and")).map(|w| w.to_lowercase()).collect() };
        let words: HashSet<String> = genres.iter().flat_map(|g| words_of(g)).collect();
        let hit = |w: &String| words.iter().any(|x| w.starts_with(x.as_str()) || x.starts_with(w.as_str()));
        let shared = |t: &Tile| t.genres.iter().filter(|g| words_of(g).iter().any(hit)).count();
        let mut v: Vec<(&Tile, usize, i64)> = self.shows.values().filter(|(t, _)| not_tvdb.is_none() || t.tvdb != not_tvdb).map(|(t, w)| (t, shared(t), *w)).filter(|(_, s, _)| *s > 0).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)));
        v.into_iter().take(n).map(|(t, _, _)| t.clone()).collect()
    }

    pub fn genres(&self, min: usize) -> Vec<String> {
        let mut count: HashMap<&str, usize> = HashMap::new();
        for (t, _) in self.shows.values() {
            for g in &t.genres {
                *count.entry(g.as_str()).or_default() += 1;
            }
        }
        let mut v: Vec<(&str, usize)> = count.into_iter().filter(|(_, c)| *c >= min).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        v.into_iter().map(|(g, _)| g.to_string()).collect()
    }

    pub fn page(&self, category: &str, page: usize, size: usize) -> Vec<Tile> {
        let list = if let Some(g) = category.strip_prefix("genre:") {
            self.sorted(|t| t.genres.iter().any(|x| x == g), false)
        } else {
            self.sorted(|_| true, category == "top")
        };
        list.into_iter().skip(page * size).take(size).collect()
    }

    /// Shows whose title matches a search: (tile, how close, TVmaze weight 0..100).
    pub fn matching(&self, term: &str, n: usize) -> Vec<(Tile, u8, f64)> {
        let mut v: Vec<(&Tile, u8, i64)> = self.shows.values().filter_map(|(t, w)| crate::media::match_rank(&t.title, term).map(|r| (t, r, *w))).collect();
        v.sort_by(|a, b| a.1.cmp(&b.1).then(b.2.cmp(&a.2)));
        v.into_iter().take(n).map(|(t, r, w)| (t.clone(), r, w as f64)).collect()
    }

    /// One page of what a filter keeps, sorted as asked.
    pub fn filtered(&self, f: &Filter, page: usize, size: usize) -> Vec<Tile> {
        let (today, none) = (today(), Facts::default());
        let mut v: Vec<(Tile, f64)> = self
            .shows
            .iter()
            .filter(|(id, (t, _))| f.keeps(t) && f.keeps_facts(self.facts.get(id).unwrap_or(&none), t, today))
            .map(|(_, (t, w))| (t.clone(), *w as f64))
            .collect();
        sort_filtered(&mut v, &f.sort);
        v.into_iter().skip(page * size).take(size).map(|(t, _)| t).collect()
    }

    /// The networks and streaming services Filters offers, the biggest first.
    pub fn studios(&self, n: usize) -> Vec<String> {
        top_studios(self.facts.values(), n)
    }

    pub fn load(path: &Path) -> Shows {
        let mut s: Shows = std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        s.start_facts_crawl();
        s
    }

    /// Saved before facts were kept: the crawl starts over from the first page.
    fn start_facts_crawl(&mut self) {
        if !self.facts_crawl {
            if !self.shows.is_empty() {
                self.next_page = 0;
                self.crawled_at = 0;
            }
            self.facts_crawl = true;
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        save_atomic(path, &serde_json::to_string(self).map_err(|e| e.to_string())?)
    }
}

/// The AniList GraphQL query for one anime row ("trending", "season",
/// "popular", "top", "genre:<name>") and page.
pub fn anilist_args(category: &str, page: u32, year: i32, month: u32) -> String {
    let filter = if let Some(q) = category.strip_prefix("q:") {
        q.to_string()
    } else if let Some(g) = category.strip_prefix("genre:") {
        let safe: String = g.chars().filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-').collect();
        format!("sort: POPULARITY_DESC, genre: \"{safe}\"")
    } else if let Some(t) = category.strip_prefix("tag:") {
        // AniList files some kinds of show (Harem, School) as tags, not genres.
        // Rank 70+: a tag on any show it barely touches put Toradora and Code
        // Geass at the top of Harem.
        let safe: String = t.chars().filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-' || *c == '\'').collect();
        format!("sort: POPULARITY_DESC, tag: \"{safe}\", minimumTagRank: 70")
    } else {
        match category {
            "season" => {
                let season = match month {
                    1..=3 => "WINTER",
                    4..=6 => "SPRING",
                    7..=9 => "SUMMER",
                    _ => "FALL",
                };
                format!("sort: POPULARITY_DESC, season: {season}, seasonYear: {year}")
            }
            "popular" => "sort: POPULARITY_DESC".into(),
            "top" => "sort: SCORE_DESC".into(),
            _ => "sort: TRENDING_DESC".into(),
        }
    };
    format!(
        "{{ Page(page: {page}, perPage: 30) {{ media(type: ANIME, isAdult: false, {filter}) {{ id idMal format genres title {{ english romaji }} coverImage {{ extraLarge }} bannerImage averageScore description(asHtml: false) seasonYear trailer {{ id site }} }} }} }}"
    )
}

/// Recommendations with anything you already own swapped for your copy, so it
/// shows as playable instead of "Download".
pub fn merge_owned(recs: Vec<Tile>, library: &[Tile]) -> Vec<Tile> {
    recs.into_iter()
        .map(|r| {
            library
                .iter()
                .find(|l| (r.tmdb.is_some() && l.tmdb == r.tmdb) || (r.tvdb.is_some() && l.tvdb == r.tvdb))
                .map(|l| Tile { genres: r.genres.clone(), trailer: r.trailer.clone(), ..l.clone() })
                .unwrap_or(r)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_keep_what_matches() {
        let t = Tile { year: Some(2014), rating: Some(7.4), genres: vec!["Action".into(), "Science Fiction".into()], ..Tile::new("tmdb:1", "movie", "Godzilla") };
        let f = |genre: Option<&str>, from: Option<i64>, to: Option<i64>, min: Option<f64>| Filter { genre: genre.map(String::from), year_from: from, year_to: to, min_rating: min, ..Default::default() };
        assert!(f(None, None, None, None).keeps(&t));
        assert!(f(Some("action"), Some(2010), Some(2019), Some(7.0)).keeps(&t));
        assert!(!f(Some("Comedy"), None, None, None).keeps(&t));
        assert!(!f(None, Some(2015), None, None).keeps(&t));
        assert!(!f(None, None, Some(2013), None).keeps(&t));
        assert!(!f(None, None, None, Some(8.0)).keeps(&t));
    }

    #[test]
    fn anime_filters_become_anilist_arguments() {
        let q = anilist_filter(&Filter { genre: Some("Sci-Fi".into()), year_from: Some(2020), year_to: Some(2023), min_rating: Some(7.5), sort: "top".into(), ..Default::default() });
        assert!(q.starts_with("q:sort: SCORE_DESC"));
        assert!(q.contains("genre_in: [\"Sci-Fi\"]") && q.contains("startDate_greater: 20199999") && q.contains("startDate_lesser: 20240000") && q.contains("averageScore_greater: 74"));
    }

    #[test]
    fn a_season_with_one_year_is_that_season() {
        let q = anilist_filter(&Filter { season: Some("FALL".into()), year_from: Some(2025), year_to: Some(2025), ..Default::default() });
        assert!(q.contains("season: FALL") && q.contains("seasonYear: 2025") && !q.contains("startDate"));
        let any = anilist_filter(&Filter { season: Some("FALL".into()), ..Default::default() });
        assert!(any.contains("season: FALL") && !any.contains("seasonYear"));
        let range = anilist_filter(&Filter { season: Some("WINTER".into()), year_from: Some(2020), year_to: Some(2023), ..Default::default() });
        assert!(!range.contains("seasonYear") && range.contains("startDate_greater: 20199999"));
    }

    #[test]
    fn genres_combine_and_exclude() {
        let t = Tile { genres: vec!["Action".into(), "Science Fiction".into()], ..Tile::new("tmdb:1", "movie", "Godzilla") };
        let f = |g: &[&str], not: &[&str]| Filter { genres: g.iter().map(|x| x.to_string()).collect(), not_genres: not.iter().map(|x| x.to_string()).collect(), ..Default::default() };
        assert!(f(&["action", "Science Fiction"], &[]).keeps(&t));
        assert!(!f(&["Action", "Comedy"], &[]).keeps(&t));
        assert!(!f(&[], &["Science Fiction"]).keeps(&t));
        assert!(f(&["Action"], &["Comedy"]).keeps(&t));
    }

    #[test]
    fn anime_rows_ask_by_genre_or_tag() {
        assert!(anilist_args("genre:Ecchi", 1, 2026, 10).contains("genre: \"Ecchi\""));
        let q = anilist_args("tag:Female Harem", 2, 2026, 10);
        assert!(q.contains("tag: \"Female Harem\"") && q.contains("page: 2") && q.contains("isAdult: false"), "{q}");
        assert!(!anilist_args("tag:x\") { y", 1, 2026, 10).contains("\")"));
    }

    #[test]
    fn advanced_anime_filters_reach_anilist() {
        let q = anilist_filter(&Filter {
            genres: vec!["Action".into(), "Ecchi".into()],
            not_genres: vec!["Horror".into()],
            tags: vec!["Isekai".into(), "Boys' Love".into()],
            formats: vec!["TV".into(), "bogus) { x".into()],
            status: Some("RELEASING".into()),
            season: Some("FALL".into()),
            episodes_min: Some(13),
            episodes_max: Some(26),
            sort: "popular".into(),
            ..Default::default()
        });
        for want in ["genre_in: [\"Action\", \"Ecchi\"]", "genre_not_in: [\"Horror\"]", "tag_in: [\"Isekai\", \"Boys' Love\"]", "format_in: [TV]", "status: RELEASING", "season: FALL", "episodes_greater: 12", "episodes_lesser: 27"] {
            assert!(q.contains(want), "{want} missing from {q}");
        }
        assert!(!q.contains("bogus"));
    }
    use serde_json::json;

    const MOVIE: &str = include_str!("../tests/fixtures/radarr-meta-movie.json");
    const LIST: &str = include_str!("../tests/fixtures/radarr-meta-list.json");

    fn meta(id: i64, title: &str, year: i64, genres: &[&str], pop: f64, rating: f64, recs: &[i64]) -> Value {
        json!({
            "TmdbId": id, "Title": title, "Year": year, "Genres": genres, "Popularity": pop,
            "MovieRatings": { "Tmdb": { "Value": rating, "Count": 500 } },
            "Images": [{ "CoverType": "Poster", "Url": format!("https://image.tmdb.org/t/p/original/{id}.jpg") }],
            "Recommendations": recs.iter().map(|r| json!({ "TmdbId": r, "Title": "x" })).collect::<Vec<_>>(),
            "Overview": "o"
        })
    }

    #[test]
    fn metadata_becomes_a_movie_tile_with_genres_and_trailer() {
        let v: Value = serde_json::from_str(MOVIE).unwrap();
        let t = from_meta(&v);
        assert_eq!(t.title, "Harry Potter and the Philosopher's Stone");
        assert_eq!((t.kind.as_str(), t.year, t.tmdb), ("movie", Some(2001), Some(671)));
        assert_eq!(t.genres, vec!["Adventure", "Fantasy"]);
        assert_eq!(t.trailer.as_deref(), Some("l91Km49W9qI"));
        assert!(t.poster.as_deref().unwrap().contains("/w500/"));
        assert!(t.backdrop.as_deref().unwrap().contains("/w1280/"));
        assert!(t.rating.unwrap() > 7.0);
    }

    #[test]
    fn list_entries_parse_too() {
        let v: Vec<Value> = serde_json::from_str(LIST).unwrap();
        assert!(v.iter().map(from_meta).all(|t| t.tmdb.is_some() && !t.title.is_empty()));
    }

    #[test]
    fn the_catalog_grows_through_recommendations() {
        let mut c = Catalog::default();
        c.add(&meta(1, "A", 2020, &["Action"], 50.0, 7.0, &[2, 3]));
        c.add(&meta(2, "B", 2021, &["Action"], 40.0, 6.0, &[1, 4]));
        assert_eq!(c.len(), 2);
        let mut next = c.next_to_fetch(10);
        next.sort();
        assert_eq!(next, vec![3, 4], "1 and 2 are already here");
        assert_eq!(c.recommendations(1), vec![2, 3]);
    }

    #[test]
    fn a_failed_fetch_is_tried_again_and_a_dead_id_is_dropped() {
        let mut c = Catalog::default();
        c.add(&meta(1, "A", 2020, &["Action"], 50.0, 7.0, &[2, 3]));
        let ids = c.next_to_fetch(10);
        assert_eq!(ids.len(), 2);
        // Offline: nothing was fetched, so both come back next time.
        c.retry(&ids);
        assert_eq!(c.next_to_fetch(10).len(), 2);
        // A 404 (no such movie) is dropped for good.
        c.dead(2);
        c.retry(&[3]);
        assert_eq!(c.next_to_fetch(10), vec![3]);
    }

    #[test]
    fn your_movies_jump_the_queue() {
        let mut c = Catalog::default();
        c.add(&meta(1, "A", 2020, &["Action"], 50.0, 7.0, &[2, 3, 4]));
        c.prioritize(&[4]);
        assert_eq!(c.next_to_fetch(1), vec![4]);
    }

    #[test]
    fn the_catalog_stops_growing_at_its_cap() {
        let mut c = Catalog::default();
        c.cap = 2;
        c.add(&meta(1, "A", 2020, &["Action"], 50.0, 7.0, &[3]));
        c.add(&meta(2, "B", 2020, &["Action"], 50.0, 7.0, &[4]));
        assert!(c.next_to_fetch(10).is_empty(), "full: nothing more to fetch");
    }

    #[test]
    fn top_10_ranks_the_trending_list_by_popularity() {
        let mut c = Catalog::default();
        c.add(&meta(1, "Old spike", 2020, &["Action"], 999.0, 7.0, &[]));
        c.add(&meta(2, "Trending 2", 2026, &["Action"], 10.0, 7.0, &[]));
        c.add(&meta(3, "Trending 1", 2026, &["Action"], 20.0, 7.0, &[]));
        // The server sends the list in id order; the rank comes from popularity.
        c.set_trending(vec![2, 3]);
        assert_eq!(c.top10().into_iter().map(|t| t.title).collect::<Vec<_>>(), vec!["Trending 1", "Trending 2"]);
    }

    #[test]
    fn ranked_movies_with_old_numbers_are_fetched_again() {
        let now = 10 * REFRESH_AFTER;
        let mut c = Catalog::default();
        c.add_at(&meta(1, "Popular, old numbers", 2026, &["Action"], 500.0, 7.0, &[]), now - 2 * REFRESH_AFTER);
        c.add_at(&meta(2, "Popular, fresh", 2026, &["Action"], 400.0, 7.0, &[]), now - 60);
        c.add_at(&meta(3, "Trending, older still", 2026, &["Action"], 0.0, 7.0, &[]), now - 3 * REFRESH_AFTER);
        for i in 10..(10 + HOT_MOVIES as i64) {
            c.add_at(&meta(i, "Filler", 2000, &["Drama"], 100.0, 6.0, &[]), now);
        }
        c.set_trending(vec![3]);
        assert_eq!(c.stale(now, 10), vec![3, 1], "stalest first; trending counts however unpopular");
        assert_eq!(c.stale(now, 1), vec![3]);
        c.add_at(&meta(1, "Popular, old numbers", 2026, &["Action"], 90.0, 7.0, &[]), now);
        assert_eq!(c.stale(now, 10), vec![3], "a refresh takes the new numbers and the new time");
        assert_eq!(c.popular(1)[0].title, "Popular, fresh", "and the ranking follows them");
    }

    #[test]
    fn collections_are_kept_and_their_films_fetched_once_a_week() {
        let now = 100 * REFRESH_AFTER;
        let mut c = Catalog::default();
        let mut im = meta(1726, "Iron Man", 2008, &["Action"], 40.0, 7.6, &[]);
        im["Collection"] = json!({ "TmdbId": 131292, "Name": "Iron Man Collection", "Parts": null });
        c.add_at(&im, now);
        c.add_at(&meta(2, "Alone", 2010, &["Drama"], 5.0, 6.0, &[]), now);
        assert_eq!(c.collection_of(1726), Some(131292));
        assert_eq!(c.collection_of(2), None);
        assert_eq!(c.collections_to_fetch(&[1726, 2, 1726], now, 10), vec![131292]);
        // The collection's own answer (Radarr's /movie/collection/131292): full films.
        let mut two = meta(10138, "Iron Man 2", 2010, &["Action"], 50.0, 6.9, &[]);
        two["Collection"] = Value::Null;
        let col = json!({ "TmdbId": 131292, "Name": "Iron Man Collection", "Parts": [meta(68721, "Iron Man 3", 2013, &["Action"], 28.0, 6.9, &[]), two, im] });
        c.add_collection_at(&col, now);
        assert!(c.collections_to_fetch(&[1726], now + 3600, 10).is_empty(), "fresh for a week");
        assert_eq!(c.collections_to_fetch(&[1726], now + 8 * 24 * 3600, 10), vec![131292]);
        let members = c.collection_members(&[131292].into());
        let titles: Vec<&str> = members[&131292].iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, vec!["Iron Man", "Iron Man 2", "Iron Man 3"], "release order");
        assert!(c.collection_members(&HashSet::new()).is_empty());
    }

    #[test]
    fn movies_saved_before_collections_were_kept_are_fetched_again() {
        let now = 10 * REFRESH_AFTER;
        let mut c = Catalog::default();
        c.add_at(&meta(1, "Fresh", 2026, &["Action"], 500.0, 7.0, &[]), now - 60);
        c.movies.get_mut(&1).unwrap().collection_known = false;
        assert_eq!(c.stale(now, 10), vec![1]);
        let old: Catalog = serde_json::from_str(r#"{"movies":{},"frontier":[]}"#).unwrap();
        assert!(old.collections_at.is_empty());
    }

    #[test]
    fn genre_rows_are_ordered_by_popularity() {
        let mut c = Catalog::default();
        c.add(&meta(1, "Low", 2010, &["Horror"], 5.0, 6.0, &[]));
        c.add(&meta(2, "High", 2012, &["Horror", "Thriller"], 90.0, 6.5, &[]));
        c.add(&meta(3, "Comedy", 2012, &["Comedy"], 99.0, 7.0, &[]));
        let h: Vec<String> = c.by_genre("Horror", 10).into_iter().map(|t| t.title).collect();
        assert_eq!(h, vec!["High", "Low"]);
        let genres = c.genres(1);
        assert_eq!(genres[0], "Horror", "most movies first");
        assert!(genres.contains(&"Comedy".to_string()));
    }

    #[test]
    fn top_rated_needs_enough_votes_and_new_means_recent() {
        let mut c = Catalog::default();
        c.add(&meta(1, "Classic", 1994, &["Drama"], 30.0, 8.7, &[]));
        c.add(&meta(2, "Fresh", 2026, &["Drama"], 60.0, 7.1, &[]));
        let mut obscure = meta(3, "Obscure", 2001, &["Drama"], 1.0, 9.9, &[]);
        obscure["MovieRatings"]["Tmdb"]["Count"] = json!(3);
        c.add(&obscure);
        assert_eq!(c.top_rated(10)[0].title, "Classic");
        assert!(!c.top_rated(10).iter().any(|t| t.title == "Obscure"), "3 votes don't make a top movie");
        assert_eq!(c.new_releases(2026, 10).into_iter().map(|t| t.title).collect::<Vec<_>>(), vec!["Fresh"]);
        assert_eq!(c.popular(1)[0].title, "Fresh");
    }

    #[test]
    fn explore_pages_walk_through_everything() {
        let mut c = Catalog::default();
        for i in 0..45 {
            c.add(&meta(i, &format!("M{i}"), 2000, &["Action"], i as f64, 6.0, &[]));
        }
        assert_eq!(c.page("genre:Action", 0, 20).len(), 20);
        assert_eq!(c.page("genre:Action", 2, 20).len(), 5);
        assert_eq!(c.page("popular", 0, 20)[0].title, "M44");
        assert!(c.page("genre:Nope", 0, 20).is_empty());
    }

    fn show(id: i64, name: &str, weight: i64, rating: f64, genres: &[&str]) -> Value {
        json!({ "id": id, "name": name, "weight": weight, "rating": { "average": rating }, "genres": genres,
                "premiered": "2015-01-01", "externals": { "thetvdb": id + 1000 },
                "image": { "medium": "https://static.tvmaze.com/m.jpg", "original": "https://static.tvmaze.com/o.jpg" } })
    }

    #[test]
    fn shows_like_one_you_watched_share_its_genres() {
        let mut s = Shows::default();
        s.add_page(&[show(1, "Watched", 99, 8.0, &["Drama", "Crime"]), show(2, "Both", 90, 8.0, &["Crime", "Drama"]), show(3, "One, popular", 99, 8.0, &["Drama"]), show(4, "None", 99, 8.0, &["Comedy"]), show(5, "Sci", 88, 8.0, &["Science-Fiction"])]);
        let got: Vec<String> = s.similar(&["Drama".into(), "Crime".into()], Some(1001), 10).into_iter().map(|t| t.title).collect();
        assert_eq!(got, vec!["Both", "One, popular"], "most genres in common first, never the show itself or no match");
        assert_eq!(s.similar(&["Sci-Fi & Fantasy".into()], None, 10)[0].title, "Sci", "Jellyfin's genre names still match TVmaze's");
    }

    #[test]
    fn series_catalog_keeps_popular_shows_and_grows_by_page() {
        let mut s = Shows::default();
        assert_eq!(s.next_page, 0);
        s.add_page(&[show(1, "Big", 99, 8.5, &["Drama"]), show(2, "Niche", 40, 9.0, &["Drama"]), show(3, "Hit", 95, 7.9, &["Comedy", "Drama"])]);
        assert_eq!(s.next_page, 1, "the next refresh fetches the next page");
        assert_eq!(s.len(), 2, "barely watched shows are left out");
        assert_eq!(s.popular(10).iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), vec!["Big", "Hit"]);
        assert_eq!(s.by_genre("Comedy", 10)[0].title, "Hit");
        assert_eq!(s.top_rated(10)[0].title, "Big");
        assert_eq!(s.page("genre:Drama", 0, 1).len(), 1);
        assert!(s.genres(1).contains(&"Drama".to_string()));
    }

    #[test]
    fn show_weights_are_kept_live() {
        let now = 10 * REFRESH_AFTER;
        let mut s = Shows::default();
        s.add_shows(&[show(1, "Old", 99, 8.0, &["Drama"]), show(2, "Rising", 90, 8.0, &["Drama"])], now - 2 * REFRESH_AFTER);
        s.add_shows(&[show(3, "Fresh", 95, 8.0, &["Drama"])], now);
        assert_eq!(s.next_page, 0, "single shows don't move the crawl on");
        assert_eq!(s.stale(now, 10), vec![1, 2]);
        s.add_shows(&[show(2, "Rising", 120, 8.0, &["Drama"]), show(1, "Old", 10, 8.0, &["Drama"])], now);
        assert_eq!(s.popular(10).iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), vec!["Rising", "Fresh"], "new weights rank, and a show that fell below the bar leaves");
        assert!(s.stale(now, 10).is_empty());
    }

    #[test]
    fn a_finished_crawl_starts_over_after_a_week() {
        let mut s = Shows { next_page: 300, ..Default::default() };
        s.reached_end(1000);
        assert_eq!((s.next_page, s.crawled_at), (300, 1000), "caught up: keep asking the last page");
        s.reached_end(1000 + RECRAWL_AFTER);
        assert_eq!(s.next_page, 300, "not a week yet");
        s.reached_end(1001 + RECRAWL_AFTER);
        assert_eq!((s.next_page, s.crawled_at), (0, 0), "a week on: every show is read again");
    }

    #[test]
    fn anilist_rows_ask_for_the_right_thing() {
        assert!(anilist_args("trending", 1, 2026, 10).contains("sort: TRENDING_DESC"));
        assert!(anilist_args("popular", 1, 2026, 10).contains("sort: POPULARITY_DESC"));
        assert!(anilist_args("top", 1, 2026, 10).contains("sort: SCORE_DESC"));
        let season = anilist_args("season", 2, 2026, 10);
        assert!(season.contains("season: FALL") && season.contains("seasonYear: 2026") && season.contains("page: 2"));
        assert!(anilist_args("season", 1, 2026, 1).contains("season: WINTER"));
        let g = anilist_args("genre:Slice of Life", 1, 2026, 10);
        assert!(g.contains(r#"genre: "Slice of Life""#));
        let evil = anilist_args("genre:x\") { evil", 1, 2026, 10);
        assert!(evil.contains(r#"genre: "x  evil""#) && !evil.contains("x\")"), "quotes and braces can't break out of the query");
    }

    #[test]
    fn owned_titles_show_up_playable() {
        let rec = vec![Tile { tmdb: Some(5), ..Tile::new("tmdb:5", "movie", "Owned") }, Tile { tmdb: Some(6), ..Tile::new("tmdb:6", "movie", "New") }];
        let lib = vec![Tile { tmdb: Some(5), jf_id: Some("jf5".into()), ..Tile::new("jf:jf5", "movie", "Owned") }];
        let out = merge_owned(rec, &lib);
        assert_eq!(out[0].jf_id.as_deref(), Some("jf5"));
        assert!(out[1].jf_id.is_none());
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn similar_movies_put_tmdbs_picks_first_then_what_they_recommend() {
        let mut c = Catalog::default();
        // 1 recommends 2 and 3 (and itself, which TMDb never should but mustn't show).
        c.add(&meta(1, "Seed", 2008, &["Action"], 50.0, 7.9, &[2, 3, 1, 2]));
        c.add(&meta(2, "Two", 2010, &["Action"], 10.0, 7.0, &[4, 5, 1]));
        c.add(&meta(3, "Three", 2012, &["Action"], 20.0, 7.0, &[5, 6]));
        c.add(&meta(4, "Four", 2013, &["Action"], 90.0, 7.0, &[]));
        c.add(&meta(5, "Five", 2014, &["Action"], 1.0, 7.0, &[]));
        let ids: Vec<i64> = c.similar_movies(1, 20).iter().filter_map(|t| t.tmdb).collect();
        // 5 is recommended by both picks, so it beats the more popular 4; 6 isn't fetched yet.
        assert_eq!(ids, vec![2, 3, 5, 4]);
        assert_eq!(c.similar_missing(1), (vec![], vec![6]));
        assert_eq!(c.similar_movies(1, 2).len(), 2);
        assert!(c.similar_movies(99, 20).is_empty(), "a movie the catalog doesn't know has nothing yet");
    }

    #[test]
    fn a_show_is_found_by_its_tvdb_id() {
        let mut s = Shows::default();
        s.add_shows(&[json!({ "id": 7, "name": "Found", "weight": 99, "genres": ["Drama"], "image": { "original": "p.jpg" }, "externals": { "thetvdb": 70 } })], 0);
        assert_eq!(s.by_tvdb(70).map(|t| t.title), Some("Found".into()));
        assert!(s.by_tvdb(71).is_none());
    }

    fn kshow(id: i64, name: &str, lang: &str, by: Value, kind: &str, status: &str, runtime: i64) -> Value {
        let mut v = show(id, name, 90, 8.0, &["Drama"]);
        v["language"] = json!(lang);
        v["type"] = json!(kind);
        v["status"] = json!(status);
        v["averageRuntime"] = json!(runtime);
        if by["kind"] == "web" {
            v["webChannel"] = json!({ "name": by["name"], "country": by["country"] });
        } else {
            v["network"] = json!({ "name": by["name"], "country": { "code": by["code"] } });
        }
        v
    }

    fn titles(v: Vec<Tile>) -> Vec<String> {
        let mut t: Vec<String> = v.into_iter().map(|t| t.title).collect();
        t.sort();
        t
    }

    #[test]
    fn tvmaze_facts_come_from_the_network_or_web_channel() {
        let v: Value = serde_json::from_str(include_str!("../tests/fixtures/tvmaze-shows.json").trim_start_matches('\u{feff}')).unwrap();
        let f = Facts::of_show(&v[1]);
        assert_eq!((f.lang.as_str(), f.country.as_str(), f.studio.as_str()), ("en", "GB", "BBC One"));
        assert_eq!((f.show_type.as_str(), f.status.as_str(), f.runtime), ("Scripted", "Running", 61));
        let web = Facts::of_show(&json!({ "language": "Korean", "webChannel": { "name": "Netflix", "country": null }, "network": null }));
        assert_eq!((web.lang.as_str(), web.studio.as_str(), web.country.as_str()), ("ko", "Netflix", ""));
    }

    #[test]
    fn korean_series_match_by_language_or_network_country() {
        let mut s = Shows::default();
        s.add_shows(
            &[
                kshow(1, "Squid Game", "Korean", json!({ "kind": "web", "name": "Netflix", "country": null }), "Scripted", "Ended", 55),
                kshow(2, "Dubbed Drama", "English", json!({ "name": "tvN", "code": "KR" }), "Scripted", "Running", 70),
                kshow(3, "Peaky", "English", json!({ "name": "BBC One", "code": "GB" }), "Scripted", "Ended", 60),
                kshow(4, "Running Man", "Korean", json!({ "name": "SBS", "code": "KR" }), "Variety", "Running", 80),
                kshow(5, "Bake Off", "English", json!({ "name": "Channel 4", "code": "GB" }), "Reality", "Running", 25),
            ],
            0,
        );
        let f = |mut f: Filter| {
            f.kind = "series".into();
            titles(s.filtered(&f, 0, 40))
        };
        assert_eq!(f(Filter { languages: vec!["ko".into()], ..Default::default() }), ["Dubbed Drama", "Running Man", "Squid Game"]);
        assert_eq!(f(Filter { languages: vec!["ko".into()], not_show_types: vec!["Variety".into()], ..Default::default() }), ["Dubbed Drama", "Squid Game"]);
        assert_eq!(f(Filter { not_languages: vec!["ko".into()], ..Default::default() }), ["Bake Off", "Peaky"]);
        assert_eq!(f(Filter { countries: vec!["GB".into()], show_types: vec!["Reality".into()], ..Default::default() }), ["Bake Off"]);
        assert_eq!(f(Filter { status: Some("Running".into()), runtime_min: Some(31), runtime_max: Some(75), ..Default::default() }), ["Dubbed Drama"]);
        assert_eq!(f(Filter { studios: vec!["netflix".into()], ..Default::default() }), ["Squid Game"]);
        assert_eq!(s.studios(10).len(), 5);
        // Gone from TVmaze: its facts go too.
        s.forget(1);
        assert!(!s.facts.contains_key(&1));
    }

    #[test]
    fn a_series_catalog_from_before_facts_crawls_again_once() {
        let old: Shows = serde_json::from_str(r#"{"shows":{"1":[{"key":"tvdb:1","kind":"series","title":"x"},90]},"next_page":300,"crawled_at":5}"#).unwrap();
        let mut s = old;
        s.start_facts_crawl();
        assert_eq!((s.next_page, s.crawled_at, s.facts_crawl), (0, 0, true));
        s.next_page = 40;
        s.start_facts_crawl();
        assert_eq!(s.next_page, 40, "only once");
    }

    fn mmeta(id: i64, title: &str, year: i64, extra: Value) -> Value {
        let mut v = meta(id, title, year, &["Drama"], 10.0, 7.0, &[]);
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        v
    }

    #[test]
    fn movie_facts_and_their_filters() {
        let v: Value = serde_json::from_str(MOVIE).unwrap();
        let f = Facts::of_movie(&v);
        assert_eq!((f.lang.as_str(), f.runtime, f.cert.as_str(), f.studio.as_str()), ("en", 152, "PG", "Warner Bros. Pictures"));
        assert_eq!((f.cinema, f.home), (20011116, 20020429));

        let mut c = Catalog::default();
        c.add_at(&mmeta(1, "Parasite", 2019, json!({ "OriginalLanguage": "ko", "Runtime": 132, "Studio": "Barunson E&A", "Certifications": [{ "Country": "US", "Certification": "R" }], "InCinema": "2019-05-30T00:00:00Z", "DigitalRelease": "2020-01-28T00:00:00Z" })), 0);
        c.add_at(&mmeta(2, "Short Film", 2018, json!({ "OriginalLanguage": "en", "Runtime": 80, "Studio": "Pixar", "Certifications": [{ "Country": "US", "Certification": "G" }] })), 0);
        c.add_at(&mmeta(3, "Hong Kong", 1994, json!({ "OriginalLanguage": "cn", "Runtime": 102 })), 0);
        c.add_at(&mmeta(4, "Next Summer", 2099, json!({ "OriginalLanguage": "en", "Runtime": 0, "InCinema": "2099-07-01T00:00:00Z" })), 0);
        let f = |f: Filter| titles(c.filtered(&Filter { kind: "movie".into(), ..f }, 0, 40));
        assert_eq!(f(Filter { languages: vec!["ko".into()], ..Default::default() }), ["Parasite"]);
        assert_eq!(f(Filter { languages: vec!["zh".into()], ..Default::default() }), ["Hong Kong"], "TMDb's cn (Cantonese) files under Chinese");
        assert_eq!(f(Filter { not_languages: vec!["en".into()], ..Default::default() }), ["Hong Kong", "Parasite"]);
        assert_eq!(f(Filter { certs: vec!["R".into(), "PG-13".into()], ..Default::default() }), ["Parasite"]);
        assert_eq!(f(Filter { runtime_max: Some(89), ..Default::default() }), ["Short Film"]);
        assert_eq!(f(Filter { runtime_min: Some(90), runtime_max: Some(120), ..Default::default() }), ["Hong Kong"], "unknown length is left out");
        assert_eq!(f(Filter { studios: vec!["Pixar".into()], ..Default::default() }), ["Short Film"]);
        assert_eq!(f(Filter { status: Some("soon".into()), ..Default::default() }), ["Next Summer"]);
        assert_eq!(f(Filter { status: Some("released".into()), ..Default::default() }), ["Hong Kong", "Parasite", "Short Film"]);
    }

    #[test]
    fn movie_stages() {
        let at = |cinema: u32, home: u32| Facts { cinema, home, ..Default::default() };
        let today = 20261019;
        assert_eq!(movie_stage(&at(20261001, 0), Some(2026), today), "cinema");
        assert_eq!(movie_stage(&at(20261001, 20261015), Some(2026), today), "released");
        assert_eq!(movie_stage(&at(20261201, 0), Some(2026), today), "soon");
        assert_eq!(movie_stage(&at(20240101, 0), Some(2024), today), "released", "a year past cinemas");
        assert_eq!(movie_stage(&at(0, 0), Some(2001), today), "released");
        assert_eq!(movie_stage(&at(0, 0), Some(2027), today), "soon");
    }

    #[test]
    fn movies_saved_before_facts_are_fetched_again_in_bulk() {
        let mut c = Catalog::default();
        c.add_at(&meta(1, "Old", 2001, &["Action"], 5.0, 7.0, &[]), 0);
        c.add_at(&meta(2, "Older", 2000, &["Action"], 50.0, 7.0, &[]), 0);
        assert_eq!(c.facts_missing(10), vec![2, 1], "no Runtime: a short record, most popular first");
        c.add_at(&mmeta(2, "Older", 2000, json!({ "Runtime": 100 })), 0);
        assert_eq!(c.facts_missing(10), vec![1]);
        c.facts_gone(&[1]);
        assert!(c.facts_missing(10).is_empty());
        // Saved files without facts still load, and empty facts aren't written.
        let text = serde_json::to_string(&c).unwrap();
        assert!(!text.contains("\"facts\":{}"));
        let back: Catalog = serde_json::from_str(&text).unwrap();
        assert_eq!(back.movies[&2].facts.runtime, 100);
    }

    #[test]
    fn the_catalog_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let mut c = Catalog::default();
        c.add(&meta(1, "Kept", 2020, &["Action"], 1.0, 6.0, &[9]));
        c.save(&path).unwrap();
        assert!(!dir.path().join("catalog.json.tmp").exists(), "written to a temp file, then moved into place");
        let back = Catalog::load(&path);
        assert_eq!(back.len(), 1);
        assert_eq!(back.recommendations(1), vec![9]);
    }
}
