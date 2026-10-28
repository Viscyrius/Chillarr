//! One shape for everything the window shows as a poster: items from
//! Jellyfin (playable), and recommendations from Radarr, TVmaze and AniList
//! (not downloaded yet, offered with "Download").

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct Tile {
    /// Unique per source: "jf:<id>", "tmdb:<id>", "tvdb:<id>", "anilist:<id>".
    pub key: String,
    /// movie | series | episode | anime
    pub kind: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub year: Option<i64>,
    pub overview: Option<String>,
    pub poster: Option<String>,
    pub backdrop: Option<String>,
    pub rating: Option<f64>,
    /// 0..1: how far you watched, or how far a download is.
    pub progress: Option<f64>,
    pub resume_seconds: Option<f64>,
    /// Set when Jellyfin has it, so it can be played.
    pub jf_id: Option<String>,
    pub series_id: Option<String>,
    pub tmdb: Option<i64>,
    pub tvdb: Option<i64>,
    pub status: Option<String>,
    /// AniList's format: TV, MOVIE, OVA...
    pub format: Option<String>,
    pub genres: Vec<String>,
    /// YouTube id of the trailer.
    pub trailer: Option<String>,
    /// My List > Completed: what's completed ("Seasons 1-2 completed").
    pub completed: Option<String>,
    /// My List > Completed, on the poster: "Deleted", "Deletes in 2 days"
    /// (the clean-up's countdown) or "Watched".
    pub mark: Option<String>,
    /// Jellyfin's Played for this profile, when Jellyfin said.
    pub played: Option<bool>,
    /// Watch now: how much of the download is there, 0..1.
    pub downloaded: Option<f64>,
    /// Watch now: the Jellyfin profile it was started for, passed back when
    /// the player closes so a profile switch meanwhile can't misfile it.
    pub profile: Option<String>,
}

impl Tile {
    pub fn new(key: &str, kind: &str, title: &str) -> Tile {
        Tile { key: key.into(), kind: kind.into(), title: title.into(), ..Default::default() }
    }
}

pub fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&").replace("&quot;", "\"").replace("&#39;", "'").replace("&nbsp;", " ").trim().to_string()
}

fn opt_str(v: &Value) -> Option<String> {
    v.as_str().map(String::from).filter(|s| !s.is_empty())
}

fn id_num(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

/// A JSON list of genre names ("genres" in Radarr, Sonarr and TVmaze).
pub fn genre_list(v: &Value) -> Vec<String> {
    v.as_array().map(|g| g.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}

pub fn from_radarr(v: &Value) -> Tile {
    let img = |kind: &str| {
        v["images"].as_array().and_then(|a| a.iter().find(|i| i["coverType"] == kind)).and_then(|i| opt_str(&i["remoteUrl"]))
    };
    let tmdb = id_num(&v["tmdbId"]);
    Tile {
        key: format!("tmdb:{}", tmdb.unwrap_or_default()),
        kind: "movie".into(),
        title: v["title"].as_str().unwrap_or_default().into(),
        year: v["year"].as_i64(),
        overview: opt_str(&v["overview"]),
        poster: img("poster").map(|u| u.replace("/original/", "/w500/")),
        backdrop: img("fanart").map(|u| u.replace("/original/", "/w1280/")),
        rating: v["ratings"]["tmdb"]["value"].as_f64(),
        tmdb,
        // A lookup of something Radarr already has comes back with its id.
        status: (v["id"].as_i64().unwrap_or(0) > 0).then(|| "Already added".to_string()),
        genres: genre_list(&v["genres"]),
        ..Default::default()
    }
}

/// A Sonarr lookup (series/lookup) as a tile.
pub fn from_sonarr(v: &Value) -> Tile {
    let img = |kind: &str| {
        v["images"].as_array().and_then(|a| a.iter().find(|i| i["coverType"] == kind)).and_then(|i| opt_str(&i["remoteUrl"]))
    };
    let tvdb = id_num(&v["tvdbId"]);
    Tile {
        key: format!("tvdb:{}", tvdb.unwrap_or_default()),
        kind: "series".into(),
        title: v["title"].as_str().unwrap_or_default().into(),
        year: v["year"].as_i64().filter(|y| *y > 0),
        overview: opt_str(&v["overview"]),
        poster: img("poster"),
        backdrop: img("fanart"),
        rating: v["ratings"]["value"].as_f64().filter(|r| *r > 0.0),
        tvdb,
        tmdb: id_num(&v["tmdbId"]).filter(|id| *id > 0),
        status: (v["id"].as_i64().unwrap_or(0) > 0).then(|| "Already added".to_string()),
        genres: genre_list(&v["genres"]),
        ..Default::default()
    }
}

pub fn from_tvmaze(v: &Value) -> Tile {
    let tvdb = id_num(&v["externals"]["thetvdb"]);
    Tile {
        key: format!("tvdb:{}", tvdb.unwrap_or_default()),
        kind: "series".into(),
        title: v["name"].as_str().unwrap_or_default().into(),
        year: v["premiered"].as_str().and_then(|d| d.get(..4)).and_then(|y| y.parse().ok()),
        overview: opt_str(&v["summary"]).map(|s| strip_html(&s)),
        poster: opt_str(&v["image"]["medium"]).or_else(|| opt_str(&v["image"]["original"])),
        rating: v["rating"]["average"].as_f64(),
        tvdb,
        genres: genre_list(&v["genres"]),
        ..Default::default()
    }
}

pub fn from_anilist(v: &Value) -> Tile {
    let title = opt_str(&v["title"]["english"]).or_else(|| opt_str(&v["title"]["romaji"])).unwrap_or_default();
    Tile {
        key: format!("anilist:{}", v["id"].as_i64().unwrap_or_default()),
        kind: "anime".into(),
        title,
        year: v["seasonYear"].as_i64(),
        overview: opt_str(&v["description"]).map(|s| strip_html(&s)),
        poster: opt_str(&v["coverImage"]["extraLarge"]),
        backdrop: opt_str(&v["bannerImage"]),
        rating: v["averageScore"].as_f64().map(|s| s / 10.0),
        format: opt_str(&v["format"]),
        genres: v["genres"].as_array().map(|g| g.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
        trailer: (v["trailer"]["site"] == "youtube").then(|| opt_str(&v["trailer"]["id"])).flatten(),
        ..Default::default()
    }
}

pub fn from_jf(v: &Value, base: &str) -> Tile {
    let id = v["Id"].as_str().unwrap_or_default();
    let kind = match v["Type"].as_str() {
        Some("Movie") => "movie",
        Some("Series") => "series",
        Some("Episode") => "episode",
        _ => "movie",
    };
    let primary = |item: &str, tag: &str| format!("{base}/Items/{item}/Images/Primary?fillHeight=480&quality=90&tag={tag}");
    let backdrop = |item: &str, tag: &str| format!("{base}/Items/{item}/Images/Backdrop/0?fillWidth=1600&quality=85&tag={tag}");
    let (title, subtitle, poster, bd) = if kind == "episode" {
        let series = v["SeriesId"].as_str().unwrap_or(id);
        let sub = format!(
            "S{} E{} · {}",
            v["ParentIndexNumber"].as_i64().unwrap_or(0),
            v["IndexNumber"].as_i64().unwrap_or(0),
            v["Name"].as_str().unwrap_or_default()
        );
        let poster = v["SeriesPrimaryImageTag"].as_str().map(|t| primary(series, t));
        let bd = match (v["ParentBackdropItemId"].as_str(), v["ParentBackdropImageTags"][0].as_str()) {
            (Some(item), Some(tag)) => Some(backdrop(item, tag)),
            _ => None,
        };
        (v["SeriesName"].as_str().unwrap_or_default().to_string(), Some(sub), poster, bd)
    } else {
        let poster = v["ImageTags"]["Primary"].as_str().map(|t| primary(id, t));
        let bd = v["BackdropImageTags"][0].as_str().map(|t| backdrop(id, t));
        (v["Name"].as_str().unwrap_or_default().to_string(), None, poster, bd)
    };
    let played = v["UserData"]["PlayedPercentage"].as_f64().map(|p| (p / 100.0 * 1000.0).round() / 1000.0);
    let resume = v["UserData"]["PlaybackPositionTicks"].as_i64().filter(|t| *t > 0).map(crate::jellyfin::ticks_to_seconds);
    Tile {
        key: format!("jf:{id}"),
        kind: kind.into(),
        title,
        subtitle,
        year: v["ProductionYear"].as_i64(),
        overview: opt_str(&v["Overview"]),
        poster,
        backdrop: bd,
        rating: v["CommunityRating"].as_f64(),
        progress: played,
        resume_seconds: resume,
        jf_id: Some(id.to_string()),
        series_id: opt_str(&v["SeriesId"]).or_else(|| (kind == "series").then(|| id.to_string())),
        tmdb: id_num(&v["ProviderIds"]["Tmdb"]),
        tvdb: id_num(&v["ProviderIds"]["Tvdb"]),
        status: None,
        format: None,
        genres: v["Genres"].as_array().map(|g| g.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
        trailer: None,
        completed: None,
        mark: None,
        played: v["UserData"]["Played"].as_bool(),
        downloaded: None,
        profile: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn load(s: &str) -> Value {
        serde_json::from_str(s.trim_start_matches('\u{feff}')).unwrap()
    }
    const RADARR: &str = include_str!("../tests/fixtures/radarr-recommendations.json");
    const TVMAZE: &str = include_str!("../tests/fixtures/tvmaze-shows.json");
    const ANILIST: &str = include_str!("../tests/fixtures/anilist-trending.json");

    #[test]
    fn radarr_recommendation_becomes_a_movie_tile() {
        let v = load(RADARR);
        let t = from_radarr(&v[0]);
        assert_eq!(t.kind, "movie");
        assert_eq!(t.title, "War of the Worlds");
        assert_eq!(t.year, Some(2005));
        assert_eq!(t.tmdb, Some(74));
        assert_eq!(t.key, "tmdb:74");
        assert!(t.poster.as_deref().unwrap().starts_with("https://image.tmdb.org/"));
        assert!(t.backdrop.is_some(), "fanart becomes the backdrop");
        assert!(t.jf_id.is_none(), "not downloaded, so not playable");
        assert!(!t.genres.is_empty(), "Radarr's genres come along");
    }

    #[test]
    fn sonarr_lookup_becomes_a_series_tile_with_genres() {
        let v = json!({
            "title": "High School DxD", "year": 2012, "tvdbId": 252506, "tmdbId": 34723, "id": 0,
            "genres": ["Action", "Animation", "Anime", "Comedy"],
            "ratings": { "votes": 120, "value": 7.4 },
            "images": [{ "coverType": "poster", "remoteUrl": "https://artworks.thetvdb.com/p.jpg" }, { "coverType": "fanart", "remoteUrl": "https://artworks.thetvdb.com/f.jpg" }]
        });
        let t = from_sonarr(&v);
        assert_eq!((t.kind.as_str(), t.key.as_str(), t.year), ("series", "tvdb:252506", Some(2012)));
        assert_eq!(t.genres, ["Action", "Animation", "Anime", "Comedy"]);
        assert_eq!(t.poster.as_deref(), Some("https://artworks.thetvdb.com/p.jpg"));
        assert!(t.backdrop.is_some() && t.status.is_none());
    }

    #[test]
    fn a_lookup_you_already_have_says_so() {
        let mut v = load(RADARR)[0].clone();
        v["id"] = serde_json::json!(12);
        assert_eq!(from_radarr(&v).status.as_deref(), Some("Already added"));
        assert_eq!(from_radarr(&load(RADARR)[0]).status, None);
    }

    #[test]
    fn tvmaze_show_becomes_a_series_tile() {
        let v = load(TVMAZE);
        let t = from_tvmaze(&v[1]);
        assert_eq!((t.kind.as_str(), t.title.as_str(), t.year), ("series", "Peaky Blinders", Some(2013)));
        assert_eq!(t.tvdb, Some(270915));
        assert_eq!(t.rating, Some(8.5));
        assert!(!t.overview.as_deref().unwrap_or("").contains('<'), "HTML is stripped");
        assert!(!t.genres.is_empty(), "TVmaze's genres come along");
    }

    #[test]
    fn anilist_entry_becomes_an_anime_tile() {
        let v = load(ANILIST);
        let t = from_anilist(&v["data"]["Page"]["media"][0]);
        assert_eq!(t.kind, "anime");
        assert_eq!(t.title, "The Apothecary Diaries Season 3");
        assert_eq!(t.rating, Some(8.5));
        assert!(t.backdrop.as_deref().unwrap().contains("banner"));
        assert!(t.key.starts_with("anilist:"));
    }

    #[test]
    fn jellyfin_episode_shows_its_series_and_resume_point() {
        let item = json!({
            "Id": "ep1", "Type": "Episode", "Name": "Pilot", "SeriesName": "Peaky Blinders", "SeriesId": "s1",
            "ParentIndexNumber": 1, "IndexNumber": 2, "ProductionYear": 2013,
            "SeriesPrimaryImageTag": "abc", "ParentBackdropItemId": "s1", "ParentBackdropImageTags": ["bd"],
            "UserData": { "PlayedPercentage": 42.5, "PlaybackPositionTicks": 12_000_000_000i64 },
            "ProviderIds": { "Tvdb": "270915" }
        });
        let t = from_jf(&item, "http://localhost:8096");
        assert_eq!(t.kind, "episode");
        assert_eq!(t.title, "Peaky Blinders");
        assert_eq!(t.subtitle.as_deref(), Some("S1 E2 · Pilot"));
        assert_eq!(t.progress, Some(0.425));
        assert_eq!(t.resume_seconds, Some(1200.0));
        assert_eq!(t.jf_id.as_deref(), Some("ep1"));
        assert_eq!(t.series_id.as_deref(), Some("s1"));
        assert_eq!(t.poster.as_deref(), Some("http://localhost:8096/Items/s1/Images/Primary?fillHeight=480&quality=90&tag=abc"));
        assert_eq!(t.backdrop.as_deref(), Some("http://localhost:8096/Items/s1/Images/Backdrop/0?fillWidth=1600&quality=85&tag=bd"));
    }

    #[test]
    fn jellyfin_movie_keeps_its_ids() {
        let item = json!({ "Id": "m1", "Type": "Movie", "Name": "Replicas", "ProductionYear": 2018,
            "ImageTags": { "Primary": "p" }, "BackdropImageTags": ["b"], "ProviderIds": { "Tmdb": "300681" }, "CommunityRating": 5.9 });
        let t = from_jf(&item, "http://localhost:8096");
        assert_eq!((t.kind.as_str(), t.tmdb, t.rating), ("movie", Some(300681), Some(5.9)));
        assert_eq!(t.key, "jf:m1");
    }

    #[test]
    fn played_is_mapped() {
        let ep = |played: Value| from_jf(&json!({ "Id": "e", "Type": "Episode", "Name": "x", "SeriesId": "s", "UserData": played }), "http://localhost:8096");
        assert_eq!(ep(json!({ "Played": true })).played, Some(true));
        assert_eq!(ep(json!({ "Played": false })).played, Some(false));
        assert_eq!(ep(json!({})).played, None);
    }
}
