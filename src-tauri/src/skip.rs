//! Skip intro and the next-episode countdown. Jellyfin keeps "media segments"
//! (intro, recap, preview, credits) per episode but finds none by itself: the
//! Intro Skipper plugin fills them by comparing a season's episodes. Fix
//! everything installs it; the player asks for an episode's segments when it
//! starts.

use crate::jellyfin::{ticks_to_seconds, Jf};
use crate::log::log;
use crate::media::{jf, jf_admin};
use crate::model::Level;
use serde::Serialize;
use serde_json::{json, Value};
use std::time::Duration;
use tauri::AppHandle;

const PLUGIN_NAME: &str = "Intro Skipper";
const PLUGIN_GUID: &str = "c83d86bba1e04c35a113e2101cf4ee6b";
/// Built for Jellyfin 12.1 (targetAbi 12.1.0.0); what intro-skipper.org
/// sent a 12.1 server on 2026-10-18. Its zip is a GitHub release asset of
/// intro-skipper/intro-skipper, and the manifest's MD5 (which Jellyfin
/// checks) matched the download on 2026-10-19.
const PLUGIN_VERSION: &str = "12.0.5.0";
/// The official manifest (intro-skipper/manifest, `12/`) at one commit, not
/// intro-skipper.org: that host answers with whatever it likes, and a plugin
/// is a DLL Jellyfin runs as the user. Pinned, the versions and checksums
/// can't change under us and Jellyfin's auto-update finds nothing newer.
const REPO_URL: &str = "https://raw.githubusercontent.com/intro-skipper/manifest/6796f4c622e9ec4fa4d28d4fe7e07a6f703338f3/12/manifest.json";

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Segment {
    /// Intro, Outro (credits), Recap, Preview or Commercial.
    pub kind: String,
    pub start: f64,
    pub end: f64,
}

/// An episode's segments, in order. Empty when there are none (not analysed yet).
pub async fn segments(app: &AppHandle, item_id: &str) -> Vec<Segment> {
    let Some(j) = jf(app).filter(|j| j.user_id.is_some()) else { return vec![] };
    match j.get_json(&format!("/MediaSegments/{item_id}")).await {
        Ok(v) => parse(&v),
        Err(_) => vec![],
    }
}

fn parse(v: &Value) -> Vec<Segment> {
    let mut out: Vec<Segment> = v["Items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            let start = ticks_to_seconds(s["StartTicks"].as_i64()?);
            let end = ticks_to_seconds(s["EndTicks"].as_i64()?);
            (end > start).then(|| Segment { kind: s["Type"].as_str().unwrap_or_default().to_string(), start, end })
        })
        .collect();
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    out
}

/// The installed plugin's version, if it's there.
async fn installed(j: &Jf) -> Option<String> {
    let v = j.get_json("/Plugins").await.ok()?;
    v.as_array()?.iter().find(|p| p["Name"] == PLUGIN_NAME).map(|p| p["Version"].as_str().unwrap_or_default().to_string())
}

/// `a.b.c.d` as numbers, so 12.0.10.0 sorts above 12.0.5.0.
fn version_parts(v: &str) -> Vec<u64> {
    v.split('.').map(|n| n.trim().parse().unwrap_or(0)).collect()
}

fn at_least_pinned(v: &str) -> bool {
    version_parts(v) >= version_parts(PLUGIN_VERSION)
}

/// Another source of this plugin than the pinned one: intro-skipper.org
/// (any manifest it serves) or a branch-following copy of the manifest repo.
fn other_source(url: &str) -> bool {
    let u = url.trim().to_ascii_lowercase();
    let host = u.split("://").nth(1).unwrap_or(&u).split(['/', ':', '?']).next().unwrap_or_default();
    u != REPO_URL.to_ascii_lowercase() && (host == "intro-skipper.org" || host.ends_with(".intro-skipper.org") || u.contains("intro-skipper/manifest"))
}

/// Jellyfin's plugin sources with every other Intro Skipper source taken out
/// and the pinned one in. None when they're already that way.
fn fixed_repos(repos: &[Value]) -> Option<Vec<Value>> {
    let mut out: Vec<Value> = repos.iter().filter(|r| !other_source(r["Url"].as_str().unwrap_or_default())).cloned().collect();
    if !out.iter().any(|r| r["Url"] == REPO_URL) {
        out.push(json!({ "Name": PLUGIN_NAME, "Url": REPO_URL, "Enabled": true }));
    }
    (out.len() != repos.len() || out.iter().zip(repos).any(|(a, b)| a != b)).then_some(out)
}

/// Installs Intro Skipper when it's missing or older than the pinned version:
/// points Jellyfin at the pinned source only, installs it, restarts Jellyfin
/// to load it, and starts the first analysis. Returns what it did, for the
/// Fix everything report; None when there was nothing to do.
pub async fn ensure_plugin(app: &AppHandle) -> Option<String> {
    let j = jf_admin(app).filter(|j| j.user_id.is_some())?;
    // Done even with the plugin there: an intro-skipper.org entry left in
    // Jellyfin would hand it its next auto-update.
    let repos: Vec<Value> = j.get_json("/Repositories").await.ok()?.as_array().cloned().unwrap_or_default();
    if let Some(repos) = fixed_repos(&repos) {
        if let Err(e) = j.post_json("/Repositories", &Value::Array(repos)).await {
            log(app, Level::Amber, format!("Couldn't set Jellyfin's source for Intro Skipper: {e}"));
            return None;
        }
    }
    if installed(&j).await.is_some_and(|v| at_least_pinned(&v)) {
        return None;
    }
    let path = format!(
        "/Packages/Installed/{}?assemblyGuid={PLUGIN_GUID}&version={PLUGIN_VERSION}&repositoryUrl={}",
        PLUGIN_NAME.replace(' ', "%20"),
        REPO_URL.replace(':', "%3A").replace('/', "%2F")
    );
    if let Err(e) = j.post_json(&path, &json!({})).await {
        log(app, Level::Amber, format!("Jellyfin couldn't install Intro Skipper, so Skip intro stays off: {e}"));
        return None;
    }
    // The install finishes in the background; then a restart loads it.
    tokio::time::sleep(Duration::from_secs(6)).await;
    let _ = j.post_json("/System/Restart", &json!({})).await;
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_secs(2)).await;
        if installed(&j).await.is_some_and(|v| at_least_pinned(&v)) {
            analyse(&j).await;
            let msg = "Installed Intro Skipper in Jellyfin. Skip intro and the next-episode countdown work once it has looked at your shows (it runs in the background).".to_string();
            log(app, Level::Ok, &msg);
            return Some(msg);
        }
    }
    log(app, Level::Amber, "Installed Intro Skipper, but Jellyfin didn't come back with it loaded. Restart Jellyfin and press Fix everything.");
    None
}

/// Starts Intro Skipper's scan of the library now instead of at its next scheduled time.
async fn analyse(j: &Jf) {
    let Ok(tasks) = j.get_json("/ScheduledTasks").await else { return };
    let task = tasks.as_array().into_iter().flatten().find(|t| {
        let key = t["Key"].as_str().unwrap_or_default();
        let cat = t["Category"].as_str().unwrap_or_default();
        cat.contains(PLUGIN_NAME) || key.to_ascii_lowercase().contains("introskipper") || key.contains("CPBIntroSkipper")
    });
    if let Some(id) = task.and_then(|t| t["Id"].as_str()) {
        let _ = j.post_json(&format!("/ScheduledTasks/Running/{id}"), &json!({})).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_come_back_in_seconds_and_in_order() {
        let v = json!({ "Items": [
            { "Type": "Outro", "StartTicks": 25_000_000_000i64, "EndTicks": 26_000_000_000i64 },
            { "Type": "Intro", "StartTicks": 300_000_000i64, "EndTicks": 1_200_000_000i64 },
            { "Type": "Recap", "StartTicks": 10, "EndTicks": 10 },
        ]});
        assert_eq!(parse(&v), vec![
            Segment { kind: "Intro".into(), start: 30.0, end: 120.0 },
            Segment { kind: "Outro".into(), start: 2500.0, end: 2600.0 },
        ]);
    }

    #[test]
    fn only_intro_skippers_other_sources_are_dropped() {
        let repos = vec![
            json!({ "Name": "Jellyfin Stable", "Url": "https://repo.jellyfin.org/files/plugin/manifest.json", "Enabled": true }),
            json!({ "Name": "Intro Skipper", "Url": "https://intro-skipper.org/manifest.json", "Enabled": true }),
            json!({ "Name": "pre", "Url": "https://intro-skipper.org/manifest-prerelease.json", "Enabled": true }),
            json!({ "Name": "cdn", "Url": "https://cdn.jsdelivr.net/gh/intro-skipper/manifest@main/12/manifest.json", "Enabled": true }),
            json!({ "Name": "Meilisearch", "Url": "https://raw.githubusercontent.com/arnesacnussem/jellyfin-plugin-meilisearch/x/manifest.json", "Enabled": true }),
            json!({ "Name": "lookalike", "Url": "https://notintro-skipper.org.example.com/manifest.json", "Enabled": true }),
        ];
        let out = fixed_repos(&repos).unwrap();
        let urls: Vec<&str> = out.iter().map(|r| r["Url"].as_str().unwrap()).collect();
        assert_eq!(urls, vec![
            "https://repo.jellyfin.org/files/plugin/manifest.json",
            "https://raw.githubusercontent.com/arnesacnussem/jellyfin-plugin-meilisearch/x/manifest.json",
            "https://notintro-skipper.org.example.com/manifest.json",
            REPO_URL,
        ]);
        // Already right: nothing to send.
        assert!(fixed_repos(&out).is_none());
    }

    #[test]
    fn versions_compare_as_numbers() {
        assert!(at_least_pinned("12.0.5.0"));
        assert!(at_least_pinned("12.0.10.0"));
        assert!(!at_least_pinned("12.0.4.0"));
        assert!(!at_least_pinned("1.12.0.1"));
        assert!(!at_least_pinned(""));
    }
}
