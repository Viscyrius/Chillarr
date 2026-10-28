//! The search schedule: whatever you're waiting for that is out but has no copy
//! yet gets searched for, again and again on a back-off, until one is
//! downloading. Radarr and Sonarr on their own only watch their indexers' new
//! uploads (RSS), so a copy uploaded before a title counted as out, or while an
//! indexer was down, is never found without a search. "Hope" (2026-10-19) was
//! out six days with 134 copies on the indexers and never searched, because
//! Chillarr wasn't running the moment it came out. The schedule is kept on disk
//! so a restart neither forgets it nor searches everything again at once.

use crate::log::log;
use crate::media::{arr, data_dir};
use crate::model::{AppId, Level};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::AppHandle;

/// The wait after each search before the next: 1 h, 3 h, 6 h, 12 h, then daily.
const BACKOFF: [i64; 5] = [3600, 3 * 3600, 6 * 3600, 12 * 3600, 24 * 3600];
/// Searches started per check (one every 2 minutes). Every search asks every
/// indexer, and some behind Prowlarr rate-limit (Uindex, TorrentsCSV).
const BUDGET: usize = 4;
/// An episode is first searched this long after it airs: uploads take a while.
const GRACE: i64 = 90 * 60;
/// After this many searches without a copy, Radarr's full list of copies is
/// read once to say why. That list is a live ask of every indexer, so never more.
const EXPLAIN_AFTER: u32 = 3;

/// Where one title stands in the schedule.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Default)]
pub struct Entry {
    /// When it was first seen out with no copy (unix seconds).
    pub since: i64,
    /// Searches started for it.
    pub tries: u32,
    /// When the last search (or the look at why) went out.
    pub last: i64,
    /// Whether the once-per-title look at why nothing was grabbed has been done.
    #[serde(default)]
    pub explained: bool,
}

/// Every title being searched for: "m:<Radarr id>" or "e:<Sonarr episode id>".
/// An episode has its own id, so a new one starts a fresh schedule.
#[derive(Serialize, Deserialize, Default, Debug, Clone, PartialEq)]
pub struct Schedule {
    entries: HashMap<String, Entry>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Movie(i64),
    Episode { series: i64, season: i64, id: i64 },
}

/// Something wanted, out (or about to be) and without a file.
#[derive(Clone, Debug)]
pub struct Want {
    pub key: String,
    /// The movie, or the show an episode belongs to.
    pub title: String,
    /// "S01E05" for an episode.
    pub label: String,
    pub kind: Kind,
    /// Already downloading: nothing to search for.
    pub queued: bool,
    /// When it may first be searched (unix seconds).
    pub ready_at: i64,
}

#[derive(Debug, PartialEq)]
pub enum Job {
    Movies(Vec<i64>),
    /// Two or more episodes of one season: Sonarr looks for a season pack and
    /// the episodes, the same as its own "search all missing" does.
    Season { series: i64, season: i64 },
    Episodes { series: i64, ids: Vec<i64> },
    /// Read Radarr's list of copies to say why none was grabbed.
    Explain(i64),
}

impl Job {
    fn cost(&self) -> usize {
        match self {
            Job::Movies(ids) | Job::Episodes { ids, .. } => ids.len(),
            Job::Season { .. } | Job::Explain(_) => 1,
        }
    }
}

fn is_due(e: &Entry, now: i64) -> bool {
    (e.tries == 0 && e.last == 0) || now - e.last >= BACKOFF[(e.tries.max(1) as usize - 1).min(BACKOFF.len() - 1)]
}

impl Schedule {
    /// Brings the entries under `prefix` in line with `wants` (one app's full
    /// list): a title newly out gets an entry, one that got its file, stopped
    /// being wanted or went back to "not out" loses it. Returns the new ones
    /// that aren't already downloading, to announce.
    pub fn sync(&mut self, prefix: &str, wants: &[Want], now: i64) -> Vec<Want> {
        let ready: Vec<&Want> = wants.iter().filter(|w| w.key.starts_with(prefix) && w.ready_at <= now).collect();
        let live: HashSet<&str> = ready.iter().map(|w| w.key.as_str()).collect();
        self.entries.retain(|k, _| !k.starts_with(prefix) || live.contains(k.as_str()));
        let mut fresh = Vec::new();
        for w in ready {
            if !self.entries.contains_key(&w.key) {
                self.entries.insert(w.key.clone(), Entry { since: now, ..Entry::default() });
                if !w.queued {
                    fresh.push(w.clone());
                }
            }
        }
        fresh
    }

    /// What to search for now, at most `budget` searches: those searched
    /// least first, then those waiting longest. Each job comes with its keys.
    pub fn plan(&self, wants: &[Want], now: i64, budget: usize) -> Vec<(Job, Vec<String>)> {
        let mut due: Vec<(&Want, &Entry)> = wants
            .iter()
            .filter(|w| !w.queued && w.ready_at <= now)
            .filter_map(|w| Some((w, self.entries.get(&w.key)?)))
            .filter(|(_, e)| is_due(e, now))
            .collect();
        due.sort_by(|a, b| (a.1.tries, a.1.last, &a.0.key).cmp(&(b.1.tries, b.1.last, &b.0.key)));
        // One unit costs one search: a movie, a lone episode, or a season's worth.
        enum Unit<'a> {
            Movie(i64, &'a str, bool),
            Group(i64, i64, Vec<(i64, &'a str)>),
        }
        let mut units: Vec<Unit> = Vec::new();
        for (w, e) in &due {
            match w.kind {
                Kind::Movie(id) => units.push(Unit::Movie(id, &w.key, e.tries >= EXPLAIN_AFTER && !e.explained)),
                Kind::Episode { series, season, id } => {
                    let at = units.iter().position(|u| matches!(u, Unit::Group(s, n, _) if *s == series && *n == season));
                    match at {
                        Some(i) => {
                            if let Unit::Group(_, _, eps) = &mut units[i] {
                                eps.push((id, &w.key));
                            }
                        }
                        None => units.push(Unit::Group(series, season, vec![(id, &w.key)])),
                    }
                }
            }
        }
        let mut jobs: Vec<(Job, Vec<String>)> = Vec::new();
        let (mut movies, mut movie_keys) = (Vec::new(), Vec::new());
        // series -> index in `jobs` of its EpisodeSearch, so lone episodes of a show go in one command.
        let mut lone: HashMap<i64, usize> = HashMap::new();
        for u in units.into_iter().take(budget) {
            match u {
                Unit::Movie(id, key, true) => jobs.push((Job::Explain(id), vec![key.to_string()])),
                Unit::Movie(id, key, false) => {
                    movies.push(id);
                    movie_keys.push(key.to_string());
                }
                Unit::Group(series, season, eps) if eps.len() > 1 => {
                    jobs.push((Job::Season { series, season }, eps.iter().map(|(_, k)| k.to_string()).collect()));
                }
                Unit::Group(series, _, eps) => {
                    let (id, key) = eps[0];
                    match lone.get(&series) {
                        Some(&i) => {
                            if let (Job::Episodes { ids, .. }, keys) = &mut jobs[i] {
                                ids.push(id);
                                keys.push(key.to_string());
                            }
                        }
                        None => {
                            lone.insert(series, jobs.len());
                            jobs.push((Job::Episodes { series, ids: vec![id] }, vec![key.to_string()]));
                        }
                    }
                }
            }
        }
        if !movies.is_empty() {
            jobs.insert(0, (Job::Movies(movies), movie_keys));
        }
        jobs
    }

    /// A search went out for `keys`.
    pub fn searched(&mut self, keys: &[String], now: i64) {
        for k in keys {
            if let Some(e) = self.entries.get_mut(k) {
                e.tries += 1;
                e.last = now;
            }
        }
    }

    /// The look at why went out; it takes that turn, so the next search waits as usual.
    pub fn explaining(&mut self, key: &str, now: i64) {
        if let Some(e) = self.entries.get_mut(key) {
            e.explained = true;
            e.last = now;
        }
    }

    #[cfg(test)]
    pub fn entry(&self, key: &str) -> Option<&Entry> {
        self.entries.get(key)
    }
}

pub fn file(dir: &Path) -> PathBuf {
    dir.join("search-schedule.json")
}

/// A file that won't parse is set aside as `.json.bad` and the schedule starts
/// over (everything out is searched once more, which is safe).
pub fn load(dir: &Path) -> Schedule {
    let path = file(dir);
    let Ok(text) = std::fs::read_to_string(&path) else { return Schedule::default() };
    serde_json::from_str(&text).unwrap_or_else(|_| {
        let _ = std::fs::rename(&path, path.with_extension("json.bad"));
        Schedule::default()
    })
}

fn save_text(dir: &Path, text: &str) -> Result<(), String> {
    let path = file(dir);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, &path)).map_err(|e| e.to_string())
}

fn ts(v: &Value) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(v.as_str()?).ok().map(|d| d.timestamp())
}

fn ids_in(queue: &Value, field: &str) -> HashSet<i64> {
    queue["records"].as_array().into_iter().flatten().filter_map(|r| r[field].as_i64()).collect()
}

/// Radarr's monitored movies that are out (by its own minimum availability) and have no file.
pub fn movie_wants(movies: &[Value], queued: &HashSet<i64>) -> Vec<Want> {
    movies
        .iter()
        .filter(|m| m["monitored"] == true && m["hasFile"] == false && m["isAvailable"] == true)
        .filter_map(|m| {
            let id = m["id"].as_i64()?;
            Some(Want {
                key: format!("m:{id}"),
                title: m["title"].as_str().unwrap_or("A movie").to_string(),
                label: String::new(),
                kind: Kind::Movie(id),
                queued: queued.contains(&id),
                ready_at: 0,
            })
        })
        .collect()
}

/// Sonarr's monitored episodes that have aired with no file (`wanted/missing`
/// records), limited to what you're actually waiting for: every episode of a
/// show with nothing on disk yet (it's on the waiting list), and any episode
/// that aired after its show was added. Older gaps in a show you already have
/// some of (episodes that were out when you asked and weren't found then) are
/// left to Sonarr: those may be leftovers of a pick, and searching them would
/// pull whole seasons nobody is waiting for.
pub fn episode_wants(missing: &[Value], series: &[Value], queued: &HashSet<i64>) -> Vec<Want> {
    let shows: HashMap<i64, &Value> = series.iter().filter_map(|s| Some((s["id"].as_i64()?, s))).collect();
    missing
        .iter()
        .filter(|e| e["monitored"] == true && e["hasFile"] == false)
        .filter_map(|e| {
            let (id, sid) = (e["id"].as_i64()?, e["seriesId"].as_i64()?);
            let show = shows.get(&sid).filter(|s| s["monitored"] == true)?;
            let aired = ts(&e["airDateUtc"])?;
            let none_yet = show["statistics"]["episodeFileCount"].as_i64() == Some(0);
            let after_added = ts(&show["added"]).is_some_and(|added| aired >= added);
            if !none_yet && !after_added {
                return None;
            }
            let (season, number) = (e["seasonNumber"].as_i64()?, e["episodeNumber"].as_i64()?);
            Some(Want {
                key: format!("e:{id}"),
                title: show["title"].as_str().unwrap_or("A show").to_string(),
                label: format!("S{season:02}E{number:02}"),
                kind: Kind::Episode { series: sid, season, id },
                queued: queued.contains(&id),
                ready_at: aired + GRACE,
            })
        })
        .collect()
}

/// One line on why searches found nothing to grab, from Radarr's full list of
/// copies. None when there are no copies at all (nothing to explain yet).
pub fn explain(title: &str, releases: &[Value]) -> Option<String> {
    if releases.is_empty() {
        return None;
    }
    let n = releases.len();
    let fine = releases.iter().filter(|r| r["rejected"] != true).count();
    let copies = |k: usize| if k == 1 { "1 copy".to_string() } else { format!("{k} copies") };
    if fine > 0 {
        return Some(format!("Radarr sees {} of {title} it would take, but its searches haven't grabbed one. Chillarr keeps trying.", copies(fine)));
    }
    // Each copy's first reason; the most common one says it best.
    let mut counts: HashMap<String, usize> = HashMap::new();
    for r in releases {
        let first = r["rejections"].as_array().and_then(|a| a.first()).and_then(|x| x.as_str().or_else(|| x["reason"].as_str()).map(String::from));
        if let Some(reason) = first {
            *counts.entry(reason).or_default() += 1;
        }
    }
    let top = counts.into_iter().max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0))).map(|(r, _)| r);
    Some(match top {
        Some(r) => format!("Found {} of {title}, but Radarr turns them all down. Most often: {}.", copies(n), r.trim_end_matches('.')),
        None => format!("Found {} of {title}, but Radarr turns them all down.", copies(n)),
    })
}

/// The schedule plus what the check loop needs to run it. Lives in waiting::Seen.
#[derive(Default)]
pub struct Searcher {
    sched: Arc<Mutex<Schedule>>,
    loaded: bool,
    saved: String,
    /// Apps already reported as not answering, so it's said once per outage.
    down: Vec<AppId>,
}

impl Searcher {
    fn down_once(&mut self, app: &AppHandle, owner: AppId, why: String) {
        if !self.down.contains(&owner) {
            self.down.push(owner);
            let what = if owner == AppId::Radarr { "movies" } else { "episodes" };
            log(app, Level::Amber, format!("Chillarr can't look for the {what} you're waiting for until {} answers again: {why}", owner.label()));
        }
    }
}

/// One pass: Radarr's movie list and Sonarr's show list as the check loop
/// already read them (None when that read failed).
pub async fn tick(app: &AppHandle, s: &mut Searcher, movies: Option<&[Value]>, series: Option<&[Value]>) {
    let Some(dir) = data_dir(app) else { return };
    if !s.loaded {
        let loaded = load(&dir);
        s.saved = serde_json::to_string(&loaded).unwrap_or_default();
        *s.sched.lock().unwrap() = loaded;
        s.loaded = true;
    }
    let now = chrono::Utc::now().timestamp();
    let mut budget = BUDGET;
    for owner in [AppId::Radarr, AppId::Sonarr] {
        // Not set up: nothing to search, nothing to say.
        let Some(a) = arr(app, owner) else { continue };
        let wants = match wants_of(&a, owner, movies, series).await {
            Ok(w) => w,
            Err(why) => {
                s.down_once(app, owner, why);
                continue;
            }
        };
        s.down.retain(|o| *o != owner);
        let prefix = if owner == AppId::Radarr { "m:" } else { "e:" };
        let (fresh, jobs) = {
            let mut sched = s.sched.lock().unwrap();
            let fresh = sched.sync(prefix, &wants, now);
            (fresh, sched.plan(&wants, now, budget))
        };
        announce(app, &fresh);
        for (job, keys) in jobs {
            let cost = job.cost();
            let body = match &job {
                Job::Explain(id) => {
                    s.sched.lock().unwrap().explaining(&keys[0], now);
                    let title = wants.iter().find(|w| w.key == keys[0]).map(|w| w.title.clone()).unwrap_or_default();
                    spawn_explain(app.clone(), s.sched.clone(), *id, keys[0].clone(), title);
                    budget = budget.saturating_sub(cost);
                    continue;
                }
                Job::Movies(ids) => json!({ "name": "MoviesSearch", "movieIds": ids }),
                Job::Season { series, season } => json!({ "name": "SeasonSearch", "seriesId": series, "seasonNumber": season }),
                Job::Episodes { ids, .. } => json!({ "name": "EpisodeSearch", "episodeIds": ids }),
            };
            match a.command_with(body).await {
                Ok(_) => {
                    s.sched.lock().unwrap().searched(&keys, now);
                    budget = budget.saturating_sub(cost);
                }
                Err(e) => {
                    s.down_once(app, owner, format!("it wouldn't start a search ({e})"));
                    break;
                }
            }
        }
    }
    let text = serde_json::to_string(&*s.sched.lock().unwrap()).unwrap_or_default();
    if text != s.saved {
        match save_text(&dir, &text) {
            Ok(()) => s.saved = text,
            // Kept in memory and tried again next check; a lost file only means one extra search each.
            Err(e) => log(app, Level::Amber, format!("Couldn't save the search schedule: {e}")),
        }
    }
}

async fn wants_of(a: &crate::arr::Arr, owner: AppId, movies: Option<&[Value]>, series: Option<&[Value]>) -> Result<Vec<Want>, String> {
    // Without the queue there's no telling what's already downloading, so no searching at all.
    let queue = a.get_one("queue?pageSize=2000").await.map_err(|e| e.to_string())?;
    if owner == AppId::Radarr {
        let movies = movies.ok_or("it didn't list its movies")?;
        return Ok(movie_wants(movies, &ids_in(&queue, "movieId")));
    }
    let series = series.ok_or("it didn't list its shows")?;
    let missing = a.get_one("wanted/missing?pageSize=2000&monitored=true&sortKey=airDateUtc&sortDirection=descending").await.map_err(|e| e.to_string())?;
    let records = missing["records"].as_array().ok_or("its missing list came back empty")?;
    Ok(episode_wants(records, series, &ids_in(&queue, "episodeId")))
}

fn announce(app: &AppHandle, fresh: &[Want]) {
    let mut shows: Vec<(&str, Vec<&str>)> = Vec::new();
    for w in fresh {
        match w.kind {
            Kind::Movie(_) => log(app, Level::Ok, format!("{} is out now. Looking for a copy.", w.title)),
            Kind::Episode { .. } => match shows.iter_mut().find(|(t, _)| *t == w.title) {
                Some((_, eps)) => eps.push(&w.label),
                None => shows.push((&w.title, vec![&w.label])),
            },
        }
    }
    for (title, eps) in shows {
        let line = if eps.len() == 1 { format!("{title} {} is out now. Looking for a copy.", eps[0]) } else { format!("{} episodes of {title} are out now. Looking for copies.", eps.len()) };
        log(app, Level::Ok, line);
    }
}

/// Reads Radarr's full list of copies for one movie (a slow, live ask of every
/// indexer) off the check loop, and says why none was grabbed. If the ask
/// fails, the look is due again next time instead of being lost.
fn spawn_explain(app: AppHandle, sched: Arc<Mutex<Schedule>>, id: i64, key: String, title: String) {
    tauri::async_runtime::spawn(async move {
        let Some(a) = arr(&app, AppId::Radarr) else { return };
        match a.list_slow(&format!("release?movieId={id}")).await {
            Ok(releases) => {
                if let Some(line) = explain(&title, &releases) {
                    log(&app, Level::Amber, line);
                }
            }
            Err(_) => {
                if let Some(e) = sched.lock().unwrap().entries.get_mut(&key) {
                    e.explained = false;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn movie(id: i64) -> Want {
        Want { key: format!("m:{id}"), title: format!("Movie {id}"), label: String::new(), kind: Kind::Movie(id), queued: false, ready_at: 0 }
    }

    fn ep(id: i64, series: i64, season: i64) -> Want {
        Want { key: format!("e:{id}"), title: "Show".into(), label: format!("S{season:02}E{id:02}"), kind: Kind::Episode { series, season, id }, queued: false, ready_at: 0 }
    }

    fn keys(jobs: &[(Job, Vec<String>)]) -> Vec<String> {
        jobs.iter().flat_map(|(_, k)| k.clone()).collect()
    }

    #[test]
    fn searches_back_off_then_go_daily() {
        let mut s = Schedule::default();
        let w = vec![movie(23)];
        let t0 = 1_000_000;
        assert_eq!(s.sync("m:", &w, t0).len(), 1, "announced once");
        assert_eq!(s.plan(&w, t0, 4)[0].0, Job::Movies(vec![23]), "searched as soon as it's out");
        s.searched(&["m:23".into()], t0);
        let mut at = t0;
        for gap in [3600, 3 * 3600, 6 * 3600, 12 * 3600, 24 * 3600, 24 * 3600] {
            assert!(s.plan(&w, at + gap - 1, 4).is_empty(), "not before {gap}s");
            at += gap;
            let jobs = s.plan(&w, at, 4);
            assert_eq!(keys(&jobs), vec!["m:23"]);
            // The look at why takes one turn after the third search, then searching carries on.
            if let Job::Explain(23) = jobs[0].0 {
                s.explaining("m:23", at);
                assert_eq!(s.entry("m:23").unwrap().tries, 3);
                at += 24 * 3600;
                assert_eq!(s.plan(&w, at, 4)[0].0, Job::Movies(vec![23]));
            }
            s.searched(&["m:23".into()], at);
        }
        assert!(s.entry("m:23").unwrap().explained);
        assert!(s.sync("m:", &w, at).is_empty(), "never announced twice");
    }

    #[test]
    fn the_schedule_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Schedule::default();
        s.sync("m:", &[movie(1)], 50);
        s.searched(&["m:1".into()], 60);
        save_text(dir.path(), &serde_json::to_string(&s).unwrap()).unwrap();
        let back = load(dir.path());
        assert_eq!(back, s);
        assert!(back.plan(&[movie(1)], 61, 4).is_empty(), "a restart doesn't search it again straight away");
        // A damaged file is set aside, not overwritten.
        std::fs::write(file(dir.path()), "{oops").unwrap();
        assert_eq!(load(dir.path()), Schedule::default());
        assert!(dir.path().join("search-schedule.json.bad").exists());
    }

    #[test]
    fn a_new_episode_gets_a_fresh_schedule() {
        let mut s = Schedule::default();
        let mut w = vec![ep(1, 7, 1)];
        s.sync("e:", &w, 0);
        s.searched(&["e:1".into()], 0);
        s.searched(&["e:1".into()], 10);
        w.push(ep(2, 7, 1));
        let fresh = s.sync("e:", &w, 20);
        assert_eq!(fresh.len(), 1);
        assert_eq!(s.entry("e:2").unwrap().tries, 0);
        assert_eq!(keys(&s.plan(&w, 30, 4)), vec!["e:2"], "the new one goes now; the old one waits its turn");
        // Not aired long enough yet: no entry, no search.
        let mut late = ep(3, 7, 1);
        late.ready_at = 100;
        w.push(late);
        s.sync("e:", &w, 40);
        assert!(s.entry("e:3").is_none());
    }

    #[test]
    fn downloading_or_done_titles_are_left_alone() {
        let mut s = Schedule::default();
        let mut q = movie(5);
        q.queued = true;
        let w = vec![q, movie(6)];
        assert_eq!(s.sync("m:", &w, 0).len(), 1, "a movie already downloading isn't announced");
        assert_eq!(keys(&s.plan(&w, 0, 4)), vec!["m:6"]);
        // Movie 6 got its file: it's no longer wanted, and its entry goes. Sonarr's entries stay.
        s.sync("e:", &[ep(9, 1, 1)], 0);
        s.sync("m:", &w[..1], 1);
        assert!(s.entry("m:6").is_none());
        assert!(s.entry("e:9").is_some());
        let radarr = vec![json!({ "id": 1, "title": "A", "monitored": true, "hasFile": false, "isAvailable": true }),
            json!({ "id": 2, "title": "B", "monitored": true, "hasFile": true, "isAvailable": true }),
            json!({ "id": 3, "title": "C", "monitored": false, "hasFile": false, "isAvailable": true }),
            json!({ "id": 4, "title": "D", "monitored": true, "hasFile": false, "isAvailable": false })];
        let got: Vec<String> = movie_wants(&radarr, &[1].into()).into_iter().map(|w| format!("{}{}", w.key, if w.queued { " queued" } else { "" })).collect();
        assert_eq!(got, vec!["m:1 queued"]);
    }

    #[test]
    fn searches_per_check_are_capped() {
        let s0 = Schedule::default();
        let w: Vec<Want> = (1..=9).map(movie).collect();
        let mut s = s0.clone();
        s.sync("m:", &w, 0);
        let jobs = s.plan(&w, 0, 4);
        assert_eq!(jobs.iter().map(|(j, _)| j.cost()).sum::<usize>(), 4);
        s.searched(&keys(&jobs), 0);
        assert_eq!(keys(&s.plan(&w, 0, 4)), vec!["m:5", "m:6", "m:7", "m:8"], "the rest go next check");
        // Episodes: a season's worth is one search, lone ones share a command per show.
        let eps = vec![ep(1, 7, 1), ep(2, 7, 1), ep(3, 7, 1), ep(4, 7, 2), ep(5, 8, 1), ep(6, 7, 3)];
        let mut s = Schedule::default();
        s.sync("e:", &eps, 0);
        let jobs: Vec<Job> = s.plan(&eps, 0, 3).into_iter().map(|(j, _)| j).collect();
        assert_eq!(jobs, vec![Job::Season { series: 7, season: 1 }, Job::Episodes { series: 7, ids: vec![4] }, Job::Episodes { series: 8, ids: vec![5] }]);
    }

    #[test]
    fn only_episodes_youre_waiting_for() {
        let show = |id: i64, files: i64| json!({ "id": id, "title": "Show", "monitored": true, "added": "2026-07-25T19:42:09Z", "statistics": { "episodeFileCount": files } });
        let missing = |id: i64, sid: i64, aired: &str| json!({ "id": id, "seriesId": sid, "seasonNumber": 1, "episodeNumber": id, "monitored": true, "hasFile": false, "airDateUtc": aired });
        let series = vec![show(1, 11), show(2, 0)];
        let rows = vec![
            missing(70, 1, "2021-01-10T15:00:00Z"), // an old gap in a show you have: left to Sonarr
            missing(71, 1, "2026-10-12T15:00:00Z"), // aired after it was added: searched
            missing(72, 2, "2021-01-10T15:00:00Z"), // a show with nothing yet: searched
            missing(73, 3, "2026-10-12T15:00:00Z"), // a show Sonarr isn't watching
        ];
        let got: Vec<String> = episode_wants(&rows, &series, &[72].into()).into_iter().map(|w| format!("{} {}", w.key, w.queued)).collect();
        assert_eq!(got, vec!["e:71 false", "e:72 true"]);
        let w = &episode_wants(&rows, &series, &HashSet::new())[0];
        assert_eq!(w.ready_at, ts(&json!("2026-10-12T15:00:00Z")).unwrap() + GRACE, "searched a while after it airs");
    }

    #[test]
    fn says_why_nothing_was_grabbed() {
        let r = |rejected: bool, why: &str| json!({ "rejected": rejected, "rejections": [why] });
        assert_eq!(explain("Hope", &[]), None);
        assert_eq!(
            explain("Hope", &[r(true, "Language is not wanted"), r(true, "Language is not wanted"), r(true, "Quality not wanted.")]).unwrap(),
            "Found 3 copies of Hope, but Radarr turns them all down. Most often: Language is not wanted."
        );
        assert!(explain("Hope", &[r(true, "x"), r(false, "")]).unwrap().starts_with("Radarr sees 1 copy of Hope it would take"));
        // Newer Radarr gives each reason as an object.
        let obj = json!({ "rejected": true, "rejections": [{ "reason": "Size too big", "type": "permanent" }] });
        assert!(explain("Hope", &[obj]).unwrap().ends_with("Most often: Size too big."));
    }
}
