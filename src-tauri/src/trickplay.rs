//! The little pictures over the player's progress bar. Jellyfin makes them
//! ("trickplay"), but ships with them off for every library, so Chillarr
//! turns them on once for the real Movies / Series / Anime libraries and
//! starts the job. Never for the Discover or Watch now libraries: those are
//! placeholders, and making pictures would stream every film just to look.
//!
//! Jellyfin's own defaults are kept (below-normal priority, one thread, one
//! picture every 10 s, 320 px wide); only key-frame extraction is turned on,
//! which makes a film's pictures several times faster for about the same look.
use crate::log::log;
use crate::media::jf_admin;
use crate::model::Level;
use serde_json::{json, Value};
use std::path::Path;
use tauri::{AppHandle, Manager};

const DONE_FILE: &str = "trickplay-on";

/// A library of real files: movies or shows, nowhere in Chillarr's own folder.
pub fn is_real_library(lib: &Value, app_dir: &Path) -> bool {
    let kind = lib["CollectionType"].as_str().unwrap_or_default();
    let own = app_dir.to_string_lossy().to_lowercase();
    matches!(kind, "movies" | "tvshows")
        && lib["Locations"].as_array().is_some_and(|l| !l.is_empty() && l.iter().all(|p| !p.as_str().unwrap_or_default().to_lowercase().starts_with(&own)))
}

/// Once per PC: switched off by hand afterwards stays off.
pub async fn ensure(app: &AppHandle) {
    let Some(data) = crate::media::data_dir(app) else { return };
    if data.join(DONE_FILE).exists() {
        return;
    }
    let Some(j) = jf_admin(app).filter(|j| j.user_id.is_some()) else { return };
    let Ok(app_dir) = app.path().app_data_dir() else { return };
    let Ok(libs) = j.libraries().await else { return };
    let mut turned_on = Vec::new();
    for l in libs.iter().filter(|l| is_real_library(l, &app_dir)) {
        let mut opts = l["LibraryOptions"].clone();
        if opts["EnableTrickplayImageExtraction"] == true && opts["ExtractTrickplayImagesDuringLibraryScan"] == true {
            continue;
        }
        opts["EnableTrickplayImageExtraction"] = json!(true);
        opts["ExtractTrickplayImagesDuringLibraryScan"] = json!(true);
        if j.post_json("/Library/VirtualFolders/LibraryOptions", &json!({ "Id": l["ItemId"], "LibraryOptions": opts })).await.is_err() {
            // Tried again on the next check.
            return;
        }
        turned_on.push(l["Name"].as_str().unwrap_or("a library").to_string());
    }
    if let Ok(mut cfg) = j.get_json("/System/Configuration").await {
        if cfg["TrickplayOptions"].is_object() && cfg["TrickplayOptions"]["EnableKeyFrameOnlyExtraction"] != true {
            cfg["TrickplayOptions"]["EnableKeyFrameOnlyExtraction"] = json!(true);
            let _ = j.post_json("/System/Configuration", &cfg).await;
        }
    }
    if !turned_on.is_empty() {
        if let Ok(tasks) = j.get_json("/ScheduledTasks").await {
            if let Some(id) = tasks.as_array().into_iter().flatten().find(|t| t["Key"] == "RefreshTrickplayImages").and_then(|t| t["Id"].as_str()) {
                let _ = j.post_json(&format!("/ScheduledTasks/Running/{id}"), &json!({})).await;
            }
        }
        log(app, Level::Ok, format!("Jellyfin now makes preview pictures for the progress bar ({}). It works through your films in the background.", turned_on.join(", ")));
    }
    let _ = std::fs::write(data.join(DONE_FILE), b"");
}

/// The player's view of an item's preview pictures: one sheet holds
/// `cols` x `rows` pictures, `interval` ms apart.
pub fn info(item: &Value, source_id: &str) -> Option<Value> {
    let by_width = item["Trickplay"][source_id].as_object().or_else(|| item["Trickplay"].as_object()?.values().next()?.as_object())?;
    // The smallest is plenty for a picture over the bar.
    let (width, t) = by_width.iter().filter_map(|(w, t)| Some((w.parse::<u32>().ok()?, t))).min_by_key(|(w, _)| *w)?;
    Some(json!({
        "width": width,
        "w": t["Width"], "h": t["Height"],
        "cols": t["TileWidth"], "rows": t["TileHeight"],
        "count": t["ThumbnailCount"], "interval": t["Interval"],
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_real_movie_and_show_libraries() {
        let app = Path::new(r"C:\Users\anime\AppData\Roaming\com.viscyrius.chillarr");
        let lib = |kind: &str, path: &str| json!({ "CollectionType": kind, "Locations": [path] });
        assert!(is_real_library(&lib("movies", r"C:\Users\anime\Desktop\Desktop\Movies\Normal"), app));
        assert!(is_real_library(&lib("tvshows", r"D:\Series\Anime"), app));
        assert!(!is_real_library(&lib("movies", r"C:\Users\anime\AppData\Roaming\com.viscyrius.chillarr\Chillarr Discover\Movies"), app));
        assert!(!is_real_library(&lib("tvshows", r"c:\users\anime\appdata\roaming\com.viscyrius.chillarr\Chillarr Watch now\Series"), app));
        assert!(!is_real_library(&lib("music", r"D:\Music"), app));
        assert!(!is_real_library(&json!({ "CollectionType": "movies", "Locations": [] }), app));
    }

    #[test]
    fn picks_the_smallest_sheet_for_the_source() {
        let item = json!({ "Trickplay": { "abc": {
            "640": { "Width": 640, "Height": 360, "TileWidth": 10, "TileHeight": 10, "ThumbnailCount": 643, "Interval": 10000 },
            "320": { "Width": 320, "Height": 180, "TileWidth": 10, "TileHeight": 10, "ThumbnailCount": 643, "Interval": 10000 } } } });
        let t = info(&item, "abc").unwrap();
        assert_eq!(t["width"], 320);
        assert_eq!(t["count"], 643);
        assert!(info(&json!({}), "abc").is_none());
    }
}
