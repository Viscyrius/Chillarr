//! MangaUpdates, for reading search (issue #78). AniList is where every card
//! comes from, but it doesn't know every series (Sword God High School: The
//! Talentless Underachiever Becomes the Strongest, a 2026 manga, isn't on it
//! at all) and doesn't list every name a series goes by (it has Chronicle of
//! Runes only as Emblem Wars). MangaUpdates lists both, with all their names.
//!
//! So a search also asks MangaUpdates. A series it finds by the name typed is
//! looked up on AniList again by its other names; when AniList still has
//! nothing, the MangaUpdates series becomes a card of its own. Following that
//! card works like any other: sources are searched by its names.

use serde_json::{json, Value};
use std::time::Duration;

const API: &str = "https://api.mangaupdates.com/v1";
/// MangaUpdates series looked at for one search.
const MAX_HITS: usize = 3;
/// A slow MangaUpdates never holds up AniList's results for longer than this.
const WAIT: Duration = Duration::from_secs(6);

/// A MangaUpdates series that matches what was typed.
pub struct Hit {
    pub id: i64,
    /// The name it matched by: what the card is called.
    pub title: String,
    /// Every name it goes by, `title` first.
    pub names: Vec<String>,
    pub record: Value,
}

/// Lowercase words, punctuation dropped: "Sword God High School: The..." and
/// "sword god high school the" compare equal.
pub fn norm(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() { c.to_lowercase().next().unwrap_or(c) } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Words with a plural "s" taken off, so "Chronicles" finds "Chronicle".
fn stem(s: &str) -> String {
    norm(s)
        .split(' ')
        .map(|w| if w.chars().count() > 3 && w.ends_with('s') && !w.ends_with("ss") { &w[..w.len() - 1] } else { w })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The name is what was typed, or starts with it (a long title typed in part),
/// plurals aside; or, for a long name, nearly the same (a letter off).
/// MangaUpdates' own search is loose, so anything else is left out.
pub fn matches(term: &str, name: &str) -> bool {
    let t = stem(term);
    let n = stem(name);
    if t.chars().count() < 4 {
        return false;
    }
    n == t || n.starts_with(&format!("{t} ")) || (t.chars().count() >= 12 && letters_apart(&t, &n) * 10 <= t.chars().count().max(n.chars().count()))
}

/// How many letters differ (Levenshtein): "swordborn" and "swordborne" are 1.
fn letters_apart(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut prev = row[0];
        row[0] = i;
        for j in 1..=b.len() {
            let here = row[j];
            row[j] = (row[j] + 1).min(row[j - 1] + 1).min(prev + usize::from(a[i - 1] != b[j - 1]));
            prev = here;
        }
    }
    row[b.len()]
}

/// MangaUpdates' series matching the term, with all their names. Empty when it
/// doesn't answer in time.
pub async fn search(term: &str) -> Vec<Hit> {
    tokio::time::timeout(WAIT, search_now(term)).await.unwrap_or_default()
}

async fn search_now(term: &str) -> Vec<Hit> {
    let http = crate::media::http();
    let Ok(r) = http.post(format!("{API}/series/search")).json(&json!({ "search": term, "perpage": 10 })).send().await else { return vec![] };
    let Ok(v) = r.json::<Value>().await else { return vec![] };
    let mut found: Vec<(i64, String, Value)> = Vec::new();
    for res in v["results"].as_array().into_iter().flatten() {
        let rec = &res["record"];
        let Some(id) = rec["series_id"].as_i64() else { continue };
        let by = [res["hit_title"].as_str(), rec["title"].as_str()].into_iter().flatten().find(|n| matches(term, n));
        if let Some(by) = by {
            if !found.iter().any(|(i, ..)| *i == id) {
                found.push((id, by.to_string(), rec.clone()));
            }
        }
        if found.len() == MAX_HITS {
            break;
        }
    }
    // Their other names are only on each series' own page.
    let mut set = tokio::task::JoinSet::new();
    for (i, (id, ..)) in found.iter().enumerate() {
        let (http, id) = (http.clone(), *id);
        set.spawn(async move {
            let names: Vec<String> = match http.get(format!("{API}/series/{id}")).send().await {
                Ok(r) => r.json::<Value>().await.ok().map(|v| {
                    v["associated"].as_array().into_iter().flatten().filter_map(|a| a["title"].as_str().map(String::from)).collect()
                }),
                Err(_) => None,
            }
            .unwrap_or_default();
            (i, names)
        });
    }
    let mut more = vec![Vec::new(); found.len()];
    while let Some(Ok((i, names))) = set.join_next().await {
        more[i] = names;
    }
    found
        .into_iter()
        .zip(more)
        .map(|((id, title, record), more)| {
            let mut names = vec![title.clone()];
            for n in record["title"].as_str().map(String::from).into_iter().chain(more) {
                if !n.trim().is_empty() && !names.iter().any(|x| norm(x) == norm(&n)) {
                    names.push(n);
                }
            }
            Hit { id, title, names, record }
        })
        .collect()
}

/// Genres MangaUpdates puts on porn (AniList's isAdult) and on ecchi.
pub fn porn(h: &Hit) -> bool {
    genres(h).iter().any(|g| matches!(g.as_str(), "Hentai" | "Adult" | "Smut"))
}
pub fn ecchi(h: &Hit) -> bool {
    genres(h).iter().any(|g| g == "Ecchi")
}

fn genres(h: &Hit) -> Vec<String> {
    h.record["genres"].as_array().into_iter().flatten().filter_map(|g| g["genre"].as_str().map(String::from)).collect()
}

/// A reading card made from MangaUpdates, shaped like `read::card`'s. It has
/// no AniList id (`anilist` is null), so Similar and the makers line don't show.
pub fn card(h: &Hit) -> Value {
    let r = &h.record;
    let kind = r["type"].as_str().unwrap_or("");
    let country = match kind {
        "Manhwa" => Some("KR"),
        "Manhua" => Some("CN"),
        "Manga" | "Doujinshi" => Some("JP"),
        _ => None,
    };
    let format = if kind.contains("Novel") { "NOVEL" } else { "MANGA" };
    let names: Vec<&String> = h.names.iter().filter(|n| *n != &h.title).collect();
    json!({
        "anilist": null,
        "mu": h.id,
        "title": h.title,
        "names": names,
        "cover": r["image"]["url"]["original"],
        "color": null,
        "banner": null,
        "country": country,
        "format": format,
        "status": null,
        "chapters": null,
        "score": r["bayesian_rating"].as_f64().filter(|s| *s > 0.0).map(|s| (s * 10.0).round() as i64),
        "genres": genres(h),
        "description": r["description"].as_str().map(crate::read::clean_text),
        "year": r["year"].as_str().and_then(|y| y.parse::<i64>().ok()),
        "adult": porn(h),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plural_s_dropped() {
        assert!(matches("Chronicles of Runes", "Chronicle of Runes"));
        assert!(matches("chronicle of rune", "Chronicle of Runes"));
        assert!(!matches("Chronicles of Ruins", "Chronicle of Runes"), "a different word isn't a plural");
    }

    #[test]
    fn small_differences_in_a_long_name_match() {
        assert!(matches("The Bastard of Swordborn", "The Bastard of Swordborne"));
        assert!(!matches("Sword God", "Sword Art Online"), "short and different stays out");
    }

    #[test]
    fn a_long_title_typed_in_part_matches() {
        let full = "Sword God High School: The Talentless Underachiever Becomes the Strongest";
        assert!(matches("Sword God High School", full));
        assert!(matches("sword god high school the talentless", full));
        assert!(matches("Chronicle of Runes", "Chronicle of Runes"));
    }

    #[test]
    fn loose_hits_are_left_out() {
        // MangaUpdates answers "Chronicles of Ruins" with Ruins of Summer and the like.
        assert!(!matches("Chronicles of Ruins", "Ruins of Summer"));
        assert!(!matches("Sword God", "Sword Goddess"));
        assert!(!matches("abc", "abc"));
    }

    #[test]
    fn a_card_reads_mangaupdates_plainly() {
        let h = Hit {
            id: 1406460414,
            title: "Sword God High School".into(),
            names: vec!["Sword God High School".into(), "Toujin Haisukuuru".into()],
            record: json!({ "type": "Manga", "year": "2026", "bayesian_rating": 6.19, "genres": [{ "genre": "Action" }], "image": { "url": { "original": "https://cdn/x.jpg" } }, "description": "A <i>cursed</i> outcast." }),
        };
        let c = card(&h);
        assert_eq!(c["anilist"], Value::Null);
        assert_eq!(c["mu"], 1406460414);
        assert_eq!(c["names"], json!(["Toujin Haisukuuru"]));
        assert_eq!(c["country"], "JP");
        assert_eq!(c["year"], 2026);
        assert_eq!(c["score"], 62);
        assert_eq!(c["description"], "A cursed outcast.");
        assert_eq!(c["adult"], false);
    }
}

#[cfg(test)]
mod live {
    /// Real MangaUpdates: `cargo test --lib read_mu::live -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn finds_both_series_from_issue_78() {
        let sword = super::search("Sword God High School").await;
        println!("{:?}", sword.iter().map(|h| (&h.title, &h.names)).collect::<Vec<_>>());
        assert!(sword.iter().any(|h| h.id == 1406460414));
        let runes = super::search("Chronicle of Runes").await;
        println!("{:?}", runes.iter().map(|h| (&h.title, &h.names)).collect::<Vec<_>>());
        assert!(runes.iter().any(|h| h.names.iter().any(|n| n == "Emblem Wars")));
    }
}
