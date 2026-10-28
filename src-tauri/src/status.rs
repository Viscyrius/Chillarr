//! Turns API answers into what the window shows: one card per app and one row
//! per indexer. The rules for red/amber/grey are the spec's "What counts as broken".

pub use crate::arr::{ArrError, Health, Indexer};
use crate::model::{AppId, Level};
use serde::Serialize;

#[derive(Serialize, Clone, Debug)]
pub struct CardState {
    pub id: AppId,
    pub level: Level,
    pub line: String,
    pub notes: Vec<String>,
    pub open_url: Option<String>,
    /// Whether "Fix everything" can do something about it (the guard ignores the rest).
    pub fixable: bool,
    /// Not installed: the window offers to install it.
    #[serde(default)]
    pub missing: bool,
    /// A newer version Chillarr can update to.
    #[serde(default)]
    pub update: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Cell {
    pub level: Level,
    pub reason: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct IndexerRow {
    pub name: String,
    pub prowlarr: Option<Cell>,
    pub sonarr: Option<Cell>,
    pub radarr: Option<Cell>,
}

const INDEXER_SOURCES: [&str; 2] = ["IndexerStatusCheck", "IndexerLongTermStatusCheck"];

pub fn classify(h: &Health) -> Level {
    let error = h.kind == "error";
    match h.source.as_str() {
        "ApplicationStatusCheck" | "ApplicationLongTermStatusCheck" => Level::Red,
        "DownloadClientCheck" if error => Level::Red,
        // Finished downloads can't be imported: the chain is broken further down.
        "RemotePathMappingCheck" | "DownloadClientRootFolderCheck" => Level::Amber,
        "IndexerProxyStatusCheck" | "DownloadClientStatusCheck" | "IndexerStatusCheck" | "IndexerLongTermStatusCheck"
        | "IndexerProxyCheck" => {
            if error {
                Level::Red
            } else {
                Level::Amber
            }
        }
        _ => Level::Ok,
    }
}

/// Problems that starting apps, testing and re-syncing can clear. Folder
/// permissions and the like need the user.
fn fixable(h: &Health) -> bool {
    !matches!(h.source.as_str(), "RemotePathMappingCheck" | "DownloadClientRootFolderCheck")
}

/// Names after the last ": " in "...unavailable due to failures: a, b".
fn listed_names(msg: &str) -> Vec<String> {
    msg.rsplit_once(": ")
        .map(|(_, list)| list.split(", ").map(|s| s.trim().to_string()).collect())
        .unwrap_or_default()
}

/// The health message rewritten short and plain for a card.
fn plain(h: &Health, id: AppId) -> String {
    let all = h.message.starts_with("All ");
    match h.source.as_str() {
        "IndexerProxyStatusCheck" | "IndexerProxyCheck" => {
            if all {
                "Can't reach FlareSolverr".into()
            } else {
                format!("Can't reach proxy {}", listed_names(&h.message).join(", "))
            }
        }
        "DownloadClientStatusCheck" | "DownloadClientCheck" => "Can't reach the download client".into(),
        "RemotePathMappingCheck" | "DownloadClientRootFolderCheck" => "Can't see the download folder".into(),
        "AllowedHostsCheck" => format!("Allowed Hosts isn't set (optional, in {}'s General settings)", id.label()),
        "ApplicationStatusCheck" | "ApplicationLongTermStatusCheck" => {
            format!("Can't reach {}", listed_names(&h.message).join(" and "))
        }
        s if INDEXER_SOURCES.contains(&s) => {
            if all {
                "Every indexer failing".into()
            } else {
                let names: Vec<String> = listed_names(&h.message).iter().map(|n| canonical_name(n).to_string()).collect();
                format!("{} failing", names.join(", "))
            }
        }
        _ => h.message.clone(),
    }
}

pub fn card_from(id: AppId, reach: Result<(), ArrError>, health: &[Health]) -> CardState {
    let mut card = CardState { id, level: Level::Ok, line: "Running".into(), notes: Vec::new(), open_url: None, fixable: false, missing: false, update: None };
    if let Err(e) = reach {
        card.level = Level::Red;
        card.fixable = !matches!(e, ArrError::BadKey);
        card.line = match e {
            ArrError::Down => "Not running".into(),
            ArrError::BadKey => "API key rejected. Press Find again in Settings.".into(),
            ArrError::Http(code) => format!("Answered with error {code}"),
            ArrError::Rejected(m) | ArrError::Other(m) => m,
        };
        return card;
    }
    card.fixable = health.iter().any(|h| classify(h) != Level::Ok && fixable(h));
    let mut items: Vec<(Level, String)> = health.iter().map(|h| (classify(h), plain(h, id))).collect();
    // Worst first; the same plain line from two checks is shown once.
    items.sort_by_key(|(l, _)| std::cmp::Reverse(l.rank()));
    items.dedup_by(|a, b| a.1 == b.1);
    let mut it = items.into_iter();
    if let Some((level, line)) = it.next() {
        if level != Level::Ok {
            card.level = level;
            card.line = line;
        } else {
            card.notes.push(line);
        }
    }
    card.notes.extend(it.map(|(_, l)| l));
    // Say which folder, so the user knows what to change.
    for h in health.iter().filter(|h| h.source == "RemotePathMappingCheck") {
        if let Some(path) = h.message.split_once(" places downloads in ").and_then(|(_, r)| r.split_once(" but ")).map(|(p, _)| p) {
            card.notes.insert(0, format!("Finished downloads in {path} stay there: {} can't open that folder.", id.label()));
        }
    }
    card
}

/// Indexers an app's health names as failing, without the " (Prowlarr)" suffix.
/// An "All indexers..." message names none, so it adds nothing here.
pub fn failing_names(health: &[Health]) -> Vec<String> {
    health
        .iter()
        .filter(|h| INDEXER_SOURCES.contains(&h.source.as_str()) && !h.message.starts_with("All "))
        .flat_map(|h| listed_names(&h.message))
        .map(|n| canonical_name(&n).to_string())
        .collect()
}

pub fn canonical_name(n: &str) -> &str {
    n.strip_suffix(" (Prowlarr)").unwrap_or(n)
}

/// One `(name, cell)` per indexer in one app, failing ones taken from its health.
pub fn indexer_cells(indexers: &[Indexer], health: &[Health]) -> Vec<(String, Cell)> {
    let mut failing: Vec<String> = Vec::new();
    let mut every = false;
    for h in health.iter().filter(|h| INDEXER_SOURCES.contains(&h.source.as_str())) {
        if h.message.starts_with("All ") {
            every = true;
        } else {
            failing.extend(listed_names(&h.message));
        }
    }
    indexers
        .iter()
        .map(|i| {
            let bad = every || failing.iter().any(|f| f == &i.name);
            let cell = if bad {
                Cell { level: Level::Red, reason: "Marked failed".into() }
            } else {
                Cell { level: Level::Ok, reason: "Working".into() }
            };
            (canonical_name(&i.name).to_string(), cell)
        })
        .collect()
}

pub fn indexer_table(p: &[(String, Cell)], s: &[(String, Cell)], r: &[(String, Cell)]) -> Vec<IndexerRow> {
    let mut rows: Vec<IndexerRow> = Vec::new();
    let mut put = |name: &str, cell: &Cell, col: usize| {
        let i = match rows.iter().position(|x| x.name == name) {
            Some(i) => i,
            None => {
                rows.push(IndexerRow { name: name.to_string(), prowlarr: None, sonarr: None, radarr: None });
                rows.len() - 1
            }
        };
        let slot = match col {
            0 => &mut rows[i].prowlarr,
            1 => &mut rows[i].sonarr,
            _ => &mut rows[i].radarr,
        };
        *slot = Some(cell.clone());
    };
    for (col, list) in [p, s, r].into_iter().enumerate() {
        for (name, cell) in list {
            put(name, cell, col);
        }
    }
    rows.sort_by_key(|r| r.name.to_lowercase());
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load<T: serde::de::DeserializeOwned>(s: &str) -> T {
        serde_json::from_str(s).unwrap()
    }
    const P_HEALTH: &str = include_str!("../tests/fixtures/prowlarr-health.json");
    const R_HEALTH: &str = include_str!("../tests/fixtures/radarr-health.json");
    const P_IDX: &str = include_str!("../tests/fixtures/prowlarr-indexers.json");
    const R_IDX: &str = include_str!("../tests/fixtures/radarr-indexers.json");

    fn by_source<'a>(h: &'a [Health], s: &str) -> &'a Health {
        h.iter().find(|x| x.source == s).unwrap()
    }

    #[test]
    fn classify_live_prowlarr_health() {
        let h: Vec<Health> = load(P_HEALTH);
        assert_eq!(classify(by_source(&h, "IndexerProxyStatusCheck")), Level::Red);
        assert_eq!(classify(by_source(&h, "ApplicationLongTermStatusCheck")), Level::Red);
        assert_eq!(classify(by_source(&h, "IndexerLongTermStatusCheck")), Level::Amber);
    }

    #[test]
    fn classify_live_radarr_health() {
        let h: Vec<Health> = load(R_HEALTH);
        assert_eq!(classify(by_source(&h, "DownloadClientStatusCheck")), Level::Red);
        assert_eq!(classify(by_source(&h, "AllowedHostsCheck")), Level::Ok);
    }

    #[test]
    fn card_red_when_down() {
        let c = card_from(AppId::Sonarr, Err(ArrError::Down), &[]);
        assert_eq!(c.level, Level::Red);
        assert_eq!(c.line, "Not running");
        assert!(c.fixable);
    }

    #[test]
    fn card_bad_key_is_not_down() {
        // Review Focus 3.
        let c = card_from(AppId::Radarr, Err(ArrError::BadKey), &[]);
        assert_eq!(c.level, Level::Red);
        assert!(c.line.starts_with("API key rejected"), "{}", c.line);
    }

    #[test]
    fn card_uses_worst_item_and_keeps_rest_as_notes() {
        let h: Vec<Health> = load(R_HEALTH);
        let c = card_from(AppId::Radarr, Ok(()), &h);
        assert_eq!(c.level, Level::Red);
        assert_eq!(c.line, "Can't reach the download client");
        assert!(c.fixable);
        assert!(c.notes.iter().any(|n| n.contains("Allowed Hosts")));
        assert!(c.notes.iter().any(|n| n == "1337x failing"));
    }

    #[test]
    fn import_path_problem_is_amber() {
        // Live 2026-10-16: Radarr (a LocalService) can't see the user's Downloads,
        // so finished downloads never reach the library.
        let h = Health {
            source: "RemotePathMappingCheck".into(),
            kind: "error".into(),
            message: "Download client qBittorrent places downloads in C:\\Users\\anime\\Downloads but Radarr cannot see this directory.".into(),
        };
        assert_eq!(classify(&h), Level::Amber);
        let c = card_from(AppId::Radarr, Ok(()), &[h]);
        assert_eq!(c.line, "Can't see the download folder");
        assert!(!c.fixable, "a Test can't fix folder permissions");
        assert!(c.notes.iter().any(|n| n.contains(r"C:\Users\anime\Downloads")), "{:?}", c.notes);
        assert!(!c.notes.iter().any(|n| n.contains("Fix everything")), "the cause may be a path mapping Chillarr can't fix");
    }

    #[test]
    fn allowed_hosts_note_is_one_short_line() {
        let h: Vec<Health> = load(R_HEALTH);
        let c = card_from(AppId::Radarr, Ok(()), &h);
        assert!(c.notes.contains(&"Allowed Hosts isn't set (optional, in Radarr's General settings)".to_string()), "{:?}", c.notes);
    }

    #[test]
    fn healthy_card_says_running() {
        let c = card_from(AppId::Radarr, Ok(()), &[]);
        assert_eq!((c.level, c.line.as_str()), (Level::Ok, "Running"));
    }

    #[test]
    fn prowlarr_suffix_merges_rows() {
        // Review Focus 2.
        let p = indexer_cells(&load::<Vec<Indexer>>(P_IDX), &load::<Vec<Health>>(P_HEALTH));
        let r = indexer_cells(&load::<Vec<Indexer>>(R_IDX), &load::<Vec<Health>>(R_HEALTH));
        let rows = indexer_table(&p, &[], &r);
        let row = |n: &str| rows.iter().find(|x| x.name == n).unwrap_or_else(|| panic!("no row {n}"));
        assert_eq!(row("1337x").prowlarr.as_ref().unwrap().level, Level::Red);
        assert_eq!(row("1337x").radarr.as_ref().unwrap().level, Level::Red);
        assert!(row("Nyaa").prowlarr.is_none());
        assert_eq!(row("Nyaa").radarr.as_ref().unwrap().level, Level::Ok);
        assert_eq!(row("Nyaa.si").radarr.as_ref().unwrap().level, Level::Ok);
        assert!(row("EZTV").radarr.is_none());
        assert!(!rows.iter().any(|x| x.name.contains("(Prowlarr)")));
    }

    #[test]
    fn all_indexers_failing_marks_every_cell() {
        let idx = vec![Indexer { id: 1, name: "A".into() }, Indexer { id: 2, name: "B".into() }];
        let h = vec![Health {
            source: "IndexerStatusCheck".into(),
            kind: "error".into(),
            message: "All indexers are unavailable due to failures".into(),
        }];
        assert!(indexer_cells(&idx, &h).iter().all(|(_, c)| c.level == Level::Red));
    }
}
