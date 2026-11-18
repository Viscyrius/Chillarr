//! Recommend: one profile sends a title to others ("you should see this"),
//! with a short note, and it shows on their home under "Recommended for you"
//! until they open or dismiss it. Plus "Your profiles are into": what the
//! other profiles on this Chillarr watched and read lately, and on a title's
//! page which of them watched or are reading it.
//!
//! Recommendations live in recommend.json, sealed for this Windows user like
//! the Hidden list (seal.rs), one inbox per profile. A card is kept as the
//! page sent it (cut to size), so it opens the same way it was sent.
//!
//! What others are into never shows a title that profile has hidden, nor a
//! series it keeps out of Your week, and a kids' profile sees none of it
//! (another profile's watching can be 18+). Sending to a kids' profile is
//! refused for an 18+ title.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

/// Recommendations kept per profile: the oldest go first past this.
const MAX_INBOX: usize = 60;
const MAX_NOTE: usize = 280;
/// A card is at most this much JSON.
const MAX_CARD: usize = 16 * 1024;
/// Titles in "Your profiles are into".
const MAX_INTO: usize = 24;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Rec {
    pub id: String,
    /// "media" (a film or show, opened as a tile) or "read" (a series to read, a card).
    pub kind: String,
    pub card: Value,
    pub from: String,
    pub from_name: String,
    pub from_color: String,
    pub note: Option<String>,
    pub at: i64,
}

#[derive(Serialize, Deserialize, Default)]
struct Store {
    inbox: HashMap<String, Vec<Rec>>,
}

#[derive(Default)]
pub struct Recommend {
    io: Mutex<()>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Who {
    pub id: String,
    pub name: String,
    pub color: String,
    /// What they're at: "watched", "S2 E4", "chapter 87"...
    pub what: String,
}

#[derive(Serialize)]
pub struct Target {
    pub id: String,
    pub name: String,
    pub color: String,
    pub kids: bool,
}

#[derive(Serialize)]
pub struct IntoItem {
    pub kind: String,
    pub card: Value,
    pub who: Vec<Who>,
    #[serde(skip)]
    at: i64,
}

fn file(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app.path().app_data_dir().map_err(|e| e.to_string())?.join("recommend.json"))
}

fn load(app: &AppHandle) -> Store {
    file(app).ok().and_then(|p| crate::seal::read(&p)).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save(app: &AppHandle, s: &Store) -> Result<(), String> {
    let text = serde_json::to_string(s).map_err(|e| e.to_string())?;
    crate::seal::write(&file(app)?, &text).map_err(|e| format!("Couldn't save it: {e}"))
}

fn me(app: &AppHandle) -> Result<String, String> {
    crate::media::active_user(app).ok_or_else(|| "Pick a profile first.".to_string())
}

/// A note as kept: trimmed, one paragraph, cut to MAX_NOTE characters.
pub fn clean_note(n: Option<&str>) -> Option<String> {
    let t: String = n?.split_whitespace().collect::<Vec<_>>().join(" ");
    let t: String = t.chars().take(MAX_NOTE).collect();
    (!t.is_empty()).then_some(t)
}

/// A card's 18+ flag, whichever way the page spelled it.
fn adult(card: &Value) -> bool {
    card["adult"] == true || card["isAdult"] == true
}

/// The ids a card could be hidden by (hidden.rs's shapes).
pub fn card_keys(kind: &str, c: &Value) -> Vec<String> {
    let mut k = Vec::new();
    let s = |v: &Value| v.as_str().map(String::from).or_else(|| v.as_i64().map(|n| n.to_string()));
    if kind == "read" {
        if let Some(a) = s(&c["anilist"]) {
            k.push(format!("al:{a}"));
        }
        if let Some(r) = s(&c["key"]) {
            k.push(format!("r:{r}"));
        }
    } else {
        for (f, p) in [("jf_id", "jf"), ("series_id", "jf"), ("tmdb", "tmdb"), ("tvdb", "tvdb")] {
            if let Some(v) = s(&c[f]) {
                k.push(format!("{p}:{v}"));
            }
        }
    }
    k
}

/// The same title, as far as two cards can tell.
fn same(kind: &str, a: &Value, b: &Value) -> bool {
    let (x, y) = (card_keys(kind, a), card_keys(kind, b));
    x.iter().any(|k| y.contains(k)) || (!a["title"].is_null() && a["title"] == b["title"] && a["kind"] == b["kind"])
}

// ---------- commands ----------

/// The profiles this one can send to: every other one (kids' ones not for 18+).
pub fn targets(app: &AppHandle, adult_title: bool) -> Result<Vec<Target>, String> {
    let me = me(app)?;
    Ok(crate::profiles::list(app)
        .into_iter()
        .filter(|p| p.id != me && !(adult_title && p.kids))
        .map(|p| Target { id: p.id, name: p.name, color: p.color, kids: p.kids })
        .collect())
}

pub fn send(app: &AppHandle, kind: &str, card: Value, to: &[String], note: Option<&str>) -> Result<String, String> {
    if kind != "media" && kind != "read" {
        return Err("That can't be recommended.".into());
    }
    if to.is_empty() {
        return Err("Pick who to send it to.".into());
    }
    if serde_json::to_string(&card).map(|t| t.len()).unwrap_or(usize::MAX) > MAX_CARD || !card.is_object() {
        return Err("That title can't be sent.".into());
    }
    let me = me(app)?;
    let all = crate::profiles::list(app);
    let sender = all.iter().find(|p| p.id == me).ok_or("Pick a profile first.")?;
    let mut names = Vec::new();
    let note = clean_note(note);
    let now = chrono::Utc::now().timestamp();
    let st = app.state::<Recommend>();
    let _io = st.io.lock().unwrap_or_else(|e| e.into_inner());
    let mut s = load(app);
    for id in to {
        let Some(p) = all.iter().find(|p| &p.id == id && p.id != me) else { continue };
        if p.kids && adult(&card) {
            continue;
        }
        let inbox = s.inbox.entry(p.id.clone()).or_default();
        // Sent again: the newer one (and its note) replaces the older.
        inbox.retain(|r| !(r.from == me && r.kind == kind && same(kind, &r.card, &card)));
        let mut rnd = [0u8; 8];
        getrandom::getrandom(&mut rnd).map_err(|_| "Couldn't send it right now.".to_string())?;
        inbox.push(Rec {
            id: rnd.iter().map(|b| format!("{b:02x}")).collect(),
            kind: kind.into(),
            card: card.clone(),
            from: me.clone(),
            from_name: sender.name.clone(),
            from_color: sender.color.clone(),
            note: note.clone(),
            at: now,
        });
        if inbox.len() > MAX_INBOX {
            let extra = inbox.len() - MAX_INBOX;
            inbox.drain(..extra);
        }
        names.push(p.name.clone());
    }
    if names.is_empty() {
        return Err("None of those profiles can get this one.".into());
    }
    save(app, &s)?;
    let title = card["title"].as_str().unwrap_or("It");
    Ok(format!("Sent {title} to {}.", join_names(&names)))
}

fn join_names(n: &[String]) -> String {
    match n.len() {
        0 => String::new(),
        1 => n[0].clone(),
        _ => format!("{} and {}", n[..n.len() - 1].join(", "), n[n.len() - 1]),
    }
}

/// This profile's recommendations, newest first.
pub fn inbox(app: &AppHandle) -> Result<Vec<Rec>, String> {
    let me = me(app)?;
    let mut list = load(app).inbox.remove(&me).unwrap_or_default();
    // A profile removed since keeps what it sent; one now kids gets no 18+.
    if crate::profiles::is_kids(app, &me) {
        list.retain(|r| !adult(&r.card));
    }
    list.reverse();
    Ok(list)
}

pub fn dismiss(app: &AppHandle, id: &str) -> Result<(), String> {
    let me = me(app)?;
    let st = app.state::<Recommend>();
    let _io = st.io.lock().unwrap_or_else(|e| e.into_inner());
    let mut s = load(app);
    if let Some(list) = s.inbox.get_mut(&me) {
        list.retain(|r| r.id != id);
    }
    save(app, &s)
}

/// Every other profile with a sign-in, and its hidden ids. None for a kids' viewer.
fn others(app: &AppHandle) -> Option<(String, Vec<(String, crate::jellyfin::Jf, Who, Vec<String>)>)> {
    let me = crate::media::active_user(app)?;
    if crate::profiles::viewer_kids(app) {
        return None;
    }
    let views = crate::profiles::list(app);
    let mut out = Vec::new();
    for (id, j) in crate::profiles::all_jf(app) {
        if id == me {
            continue;
        }
        let Some(v) = views.iter().find(|v| v.id == id) else { continue };
        let who = Who { id: id.clone(), name: v.name.clone(), color: v.color.clone(), what: String::new() };
        out.push((id.clone(), j, who, crate::hidden::keys_of(app, &id)));
    }
    Some((me, out))
}

fn rfc_secs(v: &Value) -> i64 {
    v.as_str().and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()).map(|t| t.timestamp()).unwrap_or(0)
}

/// What the other profiles watched and read lately, the newest first, each
/// title once with everyone who's into it.
pub async fn into(app: &AppHandle) -> Result<Vec<IntoItem>, String> {
    let Some((_, people)) = others(app) else { return Ok(vec![]) };
    let mut items: Vec<IntoItem> = Vec::new();
    let add = |kind: &str, card: Value, who: Who, at: i64, items: &mut Vec<IntoItem>| {
        if let Some(it) = items.iter_mut().find(|i| i.kind == kind && same(kind, &i.card, &card)) {
            if !it.who.iter().any(|w| w.id == who.id) {
                it.who.push(who);
            }
            it.at = it.at.max(at);
        } else {
            items.push(IntoItem { kind: kind.into(), card, who: vec![who], at });
        }
    };
    let followed = crate::read::followed(app);
    for (id, j, who, hidden) in &people {
        let hid = |kind: &str, c: &Value| card_keys(kind, c).iter().any(|k| hidden.contains(k));
        // Films and episodes this profile played lately; an episode counts for its show.
        let played = j
            .items("recursive=true&includeItemTypes=Movie,Episode&sortBy=DatePlayed&sortOrder=Descending&limit=20&enableUserData=true")
            .await
            .unwrap_or_default();
        let mut shows: Vec<(String, i64, String)> = Vec::new();
        for i in played.iter() {
            let at = rfc_secs(&i["UserData"]["LastPlayedDate"]);
            if at == 0 {
                continue;
            }
            if i["Type"] == "Episode" {
                if let Some(sid) = i["SeriesId"].as_str() {
                    let what = match (i["ParentIndexNumber"].as_i64(), i["IndexNumber"].as_i64()) {
                        (Some(s), Some(e)) => format!("S{s} E{e}"),
                        _ => "watching".into(),
                    };
                    if !shows.iter().any(|s| s.0 == sid) {
                        shows.push((sid.to_string(), at, what));
                    }
                }
            } else {
                let t = crate::tiles::from_jf(i, &j.base);
                let card = serde_json::to_value(&t).unwrap_or_default();
                if !hid("media", &card) {
                    let what = if i["UserData"]["Played"] == true { "watched" } else { "watching" };
                    add("media", card, Who { what: what.into(), ..who.clone() }, at, &mut items);
                }
            }
        }
        if !shows.is_empty() {
            let ids: Vec<&str> = shows.iter().map(|s| s.0.as_str()).collect();
            let series = j.items(&format!("ids={}", ids.join(","))).await.unwrap_or_default();
            for s in series {
                let Some((_, at, what)) = s["Id"].as_str().and_then(|sid| shows.iter().find(|x| x.0 == sid)) else { continue };
                let card = serde_json::to_value(crate::tiles::from_jf(&s, &j.base)).unwrap_or_default();
                if !hid("media", &card) {
                    add("media", card, Who { what: what.clone(), ..who.clone() }, *at, &mut items);
                }
            }
        }
        // Reading: the series this profile is in, unless kept out of its week.
        for (key, spot) in crate::read::spots(app, id) {
            if spot.hide_week {
                continue;
            }
            let Some(f) = followed.iter().find(|f| f.key == key) else { continue };
            let card = read_card(f);
            if hid("read", &card) {
                continue;
            }
            add("read", card, Who { what: "reading".into(), ..who.clone() }, spot.at, &mut items);
        }
    }
    items.sort_by_key(|i| std::cmp::Reverse(i.at));
    items.truncate(MAX_INTO);
    Ok(items)
}

/// A followed series as the reading pages' card.
fn read_card(f: &crate::read::Followed) -> Value {
    serde_json::json!({
        "key": f.key, "anilist": f.anilist, "title": f.title, "names": f.names, "cover": f.cover, "banner": f.banner,
        "country": f.country, "format": f.format, "status": f.status, "description": f.description, "genres": f.genres,
    })
}

/// Which other profiles watched or are reading this title, and how far.
pub async fn who(app: &AppHandle, kind: &str, card: &Value) -> Result<Vec<Who>, String> {
    let Some((_, people)) = others(app) else { return Ok(vec![]) };
    let mut out = Vec::new();
    if kind == "read" {
        let followed = crate::read::followed(app);
        let keys: Vec<String> = followed.iter().filter(|f| same("read", &read_card(f), card)).map(|f| f.key.clone()).collect();
        for (id, _, w, hidden) in &people {
            if card_keys("read", card).iter().any(|k| hidden.contains(k)) {
                continue;
            }
            let spots = crate::read::spots(app, id);
            if keys.iter().filter_map(|k| spots.get(k)).any(|s| !s.hide_week) {
                out.push(Who { what: "reading".into(), ..w.clone() });
            }
        }
        return Ok(out);
    }
    let Some(jf_id) = card["series_id"].as_str().or(card["jf_id"].as_str()).map(String::from) else { return Ok(out) };
    let is_show = card["kind"] == "series" || card["series_id"].is_string();
    for (_, j, w, hidden) in &people {
        if card_keys("media", card).iter().any(|k| hidden.contains(k)) {
            continue;
        }
        if is_show {
            let eps = j
                .items(&format!("parentId={jf_id}&recursive=true&includeItemTypes=Episode&sortBy=DatePlayed&sortOrder=Descending&limit=1&enableUserData=true"))
                .await
                .unwrap_or_default();
            if let Some(e) = eps.first().filter(|e| rfc_secs(&e["UserData"]["LastPlayedDate"]) > 0) {
                let what = match (e["ParentIndexNumber"].as_i64(), e["IndexNumber"].as_i64()) {
                    (Some(s), Some(n)) => format!("on S{s} E{n}"),
                    _ => "watching".into(),
                };
                out.push(Who { what, ..w.clone() });
            }
        } else if let Some(i) = j.items(&format!("ids={jf_id}&enableUserData=true")).await.ok().and_then(|v| v.into_iter().next()) {
            let u = &i["UserData"];
            if u["Played"] == true {
                out.push(Who { what: "watched".into(), ..w.clone() });
            } else if u["PlaybackPositionTicks"].as_i64().unwrap_or(0) > 0 {
                out.push(Who { what: "watching".into(), ..w.clone() });
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn notes_are_one_trimmed_paragraph_cut_to_size() {
        assert_eq!(clean_note(Some("  hi \n\n there ")), Some("hi there".into()));
        assert_eq!(clean_note(Some("   ")), None);
        assert_eq!(clean_note(None), None);
        assert_eq!(clean_note(Some(&"x".repeat(1000))).unwrap().chars().count(), MAX_NOTE);
    }

    #[test]
    fn cards_match_by_any_shared_id() {
        let a = json!({"jf_id": "abc", "tmdb": 603, "title": "A", "kind": "movie"});
        let b = json!({"tmdb": 603, "title": "B", "kind": "movie"});
        let c = json!({"tmdb": 604, "title": "C", "kind": "movie"});
        assert!(same("media", &a, &b));
        assert!(!same("media", &a, &c));
        assert!(same("read", &json!({"anilist": 5}), &json!({"anilist": 5, "key": "m1"})));
    }

    #[test]
    fn hidden_keys_follow_hidden_rs_shapes() {
        assert_eq!(card_keys("read", &json!({"anilist": 30013, "key": "m12"})), vec!["al:30013", "r:m12"]);
        assert_eq!(card_keys("media", &json!({"jf_id": "x1", "tvdb": 81189})), vec!["jf:x1", "tvdb:81189"]);
    }

    #[test]
    fn names_join_like_a_sentence() {
        assert_eq!(join_names(&["A".into()]), "A");
        assert_eq!(join_names(&["A".into(), "B".into(), "C".into()]), "A, B and C");
    }
}
