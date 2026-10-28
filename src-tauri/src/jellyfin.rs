//! Jellyfin: the media server that plays and streams the library. Chillarr
//! finishes its first-time setup, signs in, and reads the library, Continue
//! Watching and streams from it.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

const VERSION: &str = "0.2.0";
const TIMEOUT: Duration = Duration::from_secs(20);

pub fn auth_header(device_id: &str, token: Option<&str>) -> String {
    let mut h = format!(r#"MediaBrowser Client="Chillarr", Device="Windows", DeviceId="{device_id}", Version="{VERSION}""#);
    if let Some(t) = token {
        h.push_str(&format!(r#", Token="{t}""#));
    }
    h
}

/// Jellyfin counts time in 100-nanosecond ticks.
pub fn seconds_to_ticks(s: f64) -> i64 {
    (s * 10_000_000.0).round() as i64
}

pub fn ticks_to_seconds(t: i64) -> f64 {
    t as f64 / 10_000_000.0
}

/// What the app's web view can play as-is. Everything else (MKV, HEVC, DTS...)
/// Jellyfin turns into an HLS stream of H.264/AAC on the fly.
pub fn device_profile() -> Value {
    json!({
        "Name": "Chillarr",
        "MaxStreamingBitrate": 120_000_000,
        "DirectPlayProfiles": [
            { "Container": "mp4,m4v", "Type": "Video", "VideoCodec": "h264", "AudioCodec": "aac,mp3" }
        ],
        "TranscodingProfiles": [
            { "Container": "ts", "Type": "Video", "Protocol": "hls", "VideoCodec": "h264", "AudioCodec": "aac",
              "Context": "Streaming", "MaxAudioChannels": "2", "BreakOnNonKeyFrames": true }
        ],
        "SubtitleProfiles": [
            { "Format": "vtt", "Method": "External" },
            { "Format": "srt", "Method": "External" },
            { "Format": "ass", "Method": "External" },
            { "Format": "ssa", "Method": "External" },
            { "Format": "pgssub", "Method": "Encode" },
            { "Format": "dvdsub", "Method": "Encode" }
        ],
        "CodecProfiles": [],
        "ContainerProfiles": []
    })
}

/// An item that's only a placeholder, not something in the library: a
/// Discover shelf title, or a Watch now title still downloading.
pub fn on_shelf(item: &Value) -> bool {
    let Some(p) = item["Path"].as_str() else { return false };
    let p = p.replace('/', "\\").to_ascii_lowercase();
    [crate::discover_shelf::FOLDER, crate::watchnow::LIBRARY].iter().any(|f| p.contains(&format!("\\{}\\", f.to_ascii_lowercase())))
}

/// The sign-in Chillarr keeps in its app-data folder. Never logged.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Session {
    pub device_id: String,
    pub user: String,
    pub user_id: String,
    pub token: String,
}

#[derive(Debug, PartialEq)]
pub enum JfError {
    Down,
    Rejected,
    Other(String),
}

impl std::fmt::Display for JfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JfError::Down => write!(f, "Jellyfin isn't answering"),
            JfError::Rejected => write!(f, "Jellyfin turned the sign-in down"),
            JfError::Other(m) => write!(f, "{m}"),
        }
    }
}

pub struct Jf {
    http: reqwest::Client,
    pub base: String,
    device_id: String,
    token: Option<String>,
    pub user_id: Option<String>,
    /// Someone else's sign-in (a phone, browser or TV through bridge.rs):
    /// requests carry only its token, because Jellyfin renames a device to
    /// whatever app and device a request names (the iPhone showed up as
    /// "Windows" after Chillarr spoke for it).
    on_behalf: bool,
}

impl Jf {
    pub fn new(base: &str, device_id: &str, session: Option<&Session>) -> Jf {
        Jf {
            http: reqwest::Client::builder().connect_timeout(Duration::from_secs(2)).build().unwrap_or_default(),
            base: base.trim_end_matches('/').to_string(),
            device_id: device_id.to_string(),
            token: session.map(|s| s.token.clone()).filter(|t| !t.is_empty()),
            user_id: session.map(|s| s.user_id.clone()).filter(|u| !u.is_empty()),
            on_behalf: false,
        }
    }

    /// Jellyfin as another device's own sign-in, leaving that device's name alone.
    pub fn on_behalf(base: &str, session: &Session) -> Jf {
        Jf { on_behalf: true, ..Jf::new(base, &session.device_id, Some(session)) }
    }

    async fn call(&self, method: reqwest::Method, path: &str, body: Option<&Value>) -> Result<Value, JfError> {
        let mut req = self
            .http
            .request(method, format!("{}{}", self.base, path))
            .header("Authorization", match (&self.token, self.on_behalf) {
                (Some(t), true) => format!(r#"MediaBrowser Token="{t}""#),
                _ => auth_header(&self.device_id, self.token.as_deref()),
            })
            .timeout(TIMEOUT);
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().await.map_err(|e| if e.is_connect() || e.is_timeout() { JfError::Down } else { JfError::Other(e.to_string()) })?;
        match resp.status().as_u16() {
            200..=299 => {
                let text = resp.text().await.unwrap_or_default();
                Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
            }
            401 | 403 => Err(JfError::Rejected),
            c => Err(JfError::Other(format!("Jellyfin answered with error {c}"))),
        }
    }

    async fn get(&self, path: &str) -> Result<Value, JfError> {
        self.call(reqwest::Method::GET, path, None).await
    }

    /// Any GET as JSON (plugins, remote subtitle search).
    pub async fn get_json(&self, path: &str) -> Result<Value, JfError> {
        self.get(path).await
    }

    /// Any POST with a JSON body (plugin install and settings, subtitle download, restart).
    pub async fn post_json(&self, path: &str, body: &Value) -> Result<Value, JfError> {
        self.post(path, body).await
    }

    /// Deletes an item and its files from disk (Jellyfin needs the user to be allowed to delete).
    pub async fn delete_item(&self, id: &str) -> Result<(), JfError> {
        self.call(reqwest::Method::DELETE, &format!("/Items/{id}"), None).await.map(|_| ())
    }

    async fn post(&self, path: &str, body: &Value) -> Result<Value, JfError> {
        self.call(reqwest::Method::POST, path, Some(body)).await
    }

    /// The token and device id this client signs in with (SyncPlay's socket
    /// must use the same pair as its REST calls).
    pub fn credentials(&self) -> Option<(String, String)> {
        Some((self.token.clone()?, self.device_id.clone()))
    }

    fn uid(&self) -> Result<&str, JfError> {
        self.user_id.as_deref().ok_or(JfError::Rejected)
    }

    /// `(reachable, setup_done)` without signing in.
    pub async fn public_info(&self) -> Result<bool, JfError> {
        let v = self.get("/System/Info/Public").await?;
        Ok(v["StartupWizardCompleted"].as_bool().unwrap_or(false))
    }

    /// Finishes Jellyfin's first-time setup with the user's chosen account.
    pub async fn complete_setup(&self, user: &str, password: &str) -> Result<(), JfError> {
        self.post("/Startup/Configuration", &json!({ "UICulture": "en-US", "MetadataCountryCode": "US", "PreferredMetadataLanguage": "en" }))
            .await?;
        self.get("/Startup/User").await?;
        self.post("/Startup/User", &json!({ "Name": user, "Password": password })).await?;
        self.post("/Startup/RemoteAccess", &json!({ "EnableRemoteAccess": true, "EnableAutomaticPortMapping": false }))
            .await?;
        self.post("/Startup/Complete", &json!({})).await?;
        Ok(())
    }

    pub async fn sign_in(&self, user: &str, password: &str) -> Result<Session, JfError> {
        let v = self.post("/Users/AuthenticateByName", &json!({ "Username": user, "Pw": password })).await?;
        let token = v["AccessToken"].as_str().ok_or(JfError::Rejected)?.to_string();
        let user_id = v["User"]["Id"].as_str().ok_or(JfError::Rejected)?.to_string();
        Ok(Session { device_id: self.device_id.clone(), user: user.to_string(), user_id, token })
    }

    /// A new Jellyfin account (admin only). Returns its id.
    pub async fn create_user(&self, name: &str, password: &str) -> Result<String, JfError> {
        let v = self.post("/Users/New", &json!({ "Name": name, "Password": password })).await?;
        v["Id"].as_str().map(String::from).ok_or_else(|| JfError::Other("Jellyfin didn't say which account it made".into()))
    }

    /// Sets an account's password (admin only). Measured on 12.1: this signs
    /// that account out everywhere, so whoever kept its token signs in again.
    pub async fn set_password(&self, id: &str, password: &str) -> Result<(), JfError> {
        self.post(&format!("/Users/{id}/Password"), &json!({ "NewPw": password })).await.map(|_| ())
    }

    /// Shows or hides an account on the sign-in screen of Jellyfin's own apps
    /// (admin only). Measured on 12.1: /Users/New makes every account hidden.
    /// Any DELETE (removing an account half made).
    pub async fn call_delete(&self, path: &str) -> Result<Value, JfError> {
        self.call(reqwest::Method::DELETE, path, None).await
    }

    pub async fn set_hidden(&self, id: &str, hidden: bool) -> Result<(), JfError> {
        let mut policy = self.get(&format!("/Users/{id}")).await?["Policy"].take();
        if !policy.is_object() {
            return Err(JfError::Other("Jellyfin didn't send the account's settings".into()));
        }
        policy["IsHidden"] = json!(hidden);
        self.post(&format!("/Users/{id}/Policy"), &policy).await.map(|_| ())
    }

    /// Renames an account (admin only).
    pub async fn rename_user(&self, id: &str, name: &str) -> Result<(), JfError> {
        let mut user = self.get(&format!("/Users/{id}")).await?;
        user["Name"] = json!(name);
        self.post(&format!("/Users/{id}"), &user).await.map(|_| ())
    }

    pub async fn libraries(&self) -> Result<Vec<Value>, JfError> {
        Ok(self.get("/Library/VirtualFolders").await?.as_array().cloned().unwrap_or_default())
    }

    /// Adds a library for a folder unless one already points there.
    pub async fn ensure_library(&self, name: &str, kind: &str, path: &str) -> Result<bool, JfError> {
        self.ensure_library_with(name, kind, path, json!({})).await
    }

    /// `ensure_library`, with these options on a library it creates (so they
    /// hold from the first scan, before Jellyfin has given it an id).
    pub async fn ensure_library_with(&self, name: &str, kind: &str, path: &str, options: Value) -> Result<bool, JfError> {
        let libs = self.libraries().await?;
        let norm = |p: &str| p.trim_end_matches(['\\', '/']).to_ascii_lowercase();
        let have = libs
            .iter()
            .any(|l| l["Locations"].as_array().is_some_and(|ls| ls.iter().any(|x| x.as_str().map(norm) == Some(norm(path)))));
        if have {
            return Ok(false);
        }
        // A library with that name already exists (a second Anime folder, or one the
        // user made): add the folder to it instead of creating a clashing one.
        if libs.iter().any(|l| l["Name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(name))) {
            self.post("/Library/VirtualFolders/Paths?refreshLibrary=true", &json!({ "Name": name, "PathInfo": { "Path": path } })).await?;
            return Ok(true);
        }
        let q = format!("/Library/VirtualFolders?name={}&collectionType={kind}&refreshLibrary=true", urlencode(name));
        let mut opts = if options.is_object() { options } else { json!({}) };
        opts["PathInfos"] = json!([{ "Path": path }]);
        self.post(&q, &json!({ "LibraryOptions": opts })).await?;
        Ok(true)
    }

    /// Rescans every library folder for new files.
    pub async fn scan(&self) -> Result<(), JfError> {
        self.post("/Library/Refresh", &json!({})).await.map(|_| ())
    }

    /// The user's libraries (works for any account, not just admins).
    pub async fn views(&self) -> Result<Vec<Value>, JfError> {
        let uid = self.uid()?;
        Ok(self.get(&format!("/UserViews?userId={uid}")).await?["Items"].as_array().cloned().unwrap_or_default())
    }

    /// One item's trailer links and outside ids.
    pub async fn trailer_info(&self, id: &str) -> Result<Value, JfError> {
        let uid = self.uid()?;
        let v = self.get(&format!("/Items?userId={uid}&ids={id}&fields=RemoteTrailers,ProviderIds")).await?;
        Ok(v["Items"][0].clone())
    }

    /// Library items, minus the Discover shelf's placeholders (titles you don't have).
    pub async fn items(&self, query: &str) -> Result<Vec<Value>, JfError> {
        let uid = self.uid()?;
        let v = self.get(&format!("/Items?userId={uid}&{query}&fields=Overview,ProviderIds,Genres,DateCreated,Path&enableImageTypes=Primary,Backdrop,Thumb")).await?;
        Ok(v["Items"].as_array().cloned().unwrap_or_default().into_iter().filter(|i| !on_shelf(i)).collect())
    }

    /// A lean list for the watch log: ids, numbers, provider ids, genres and
    /// this user's play data. No images or overviews, so it stays cheap to
    /// ask every couple of minutes.
    pub async fn watch_items(&self, query: &str) -> Result<Vec<Value>, JfError> {
        Ok(self.watch_items_with_placeholders(query).await?.into_iter().filter(|i| !on_shelf(i)).collect())
    }

    /// `watch_items` with the Discover shelf's and Watch now's placeholders
    /// left in: what was streamed from them is still something you watched.
    /// For the watch log (and Start over, which unmarks them), never for
    /// finding a title's real copy.
    pub async fn watch_items_with_placeholders(&self, query: &str) -> Result<Vec<Value>, JfError> {
        let uid = self.uid()?;
        let v = self.get(&format!("/Items?userId={uid}&recursive=true&{query}&fields=ProviderIds,Genres,Path&enableImages=false&enableUserData=true")).await?;
        Ok(v["Items"].as_array().cloned().unwrap_or_default())
    }

    /// Any DELETE (a library, an item).
    pub async fn delete_path(&self, path: &str) -> Result<(), JfError> {
        self.call(reqwest::Method::DELETE, path, None).await.map(|_| ())
    }

    /// Puts an item on, or takes it off, this user's favourites. Chillarr's
    /// My List is the favourites for anything in the library, so it shows the
    /// same in Jellyfin's own apps.
    pub async fn set_favourite(&self, id: &str, on: bool) -> Result<(), JfError> {
        let path = format!("/Users/{}/FavoriteItems/{id}", self.uid()?);
        if on {
            self.post(&path, &json!({})).await.map(|_| ())
        } else {
            self.delete_path(&path).await
        }
    }

    /// This user's favourite movies and shows, newest first. `items` leaves out
    /// the Discover shelf, where a heart means "get it", not My List.
    pub async fn favourites(&self) -> Result<Vec<Value>, JfError> {
        self.items("recursive=true&isFavorite=true&includeItemTypes=Movie,Series&sortBy=DateCreated&sortOrder=Descending&limit=200").await
    }

    pub async fn resume(&self) -> Result<Vec<Value>, JfError> {
        self.items("recursive=true&filters=IsResumable&includeItemTypes=Movie,Episode&sortBy=DatePlayed&sortOrder=Descending&limit=20").await
    }

    pub async fn next_up(&self) -> Result<Vec<Value>, JfError> {
        let uid = self.uid()?;
        let v = self.get(&format!("/Shows/NextUp?userId={uid}&limit=20&fields=Overview,Path")).await?;
        Ok(v["Items"].as_array().cloned().unwrap_or_default().into_iter().filter(|i| !on_shelf(i)).collect())
    }

    pub async fn latest(&self) -> Result<Vec<Value>, JfError> {
        self.items("recursive=true&includeItemTypes=Movie,Series&sortBy=DateCreated&sortOrder=Descending&limit=24").await
    }

    /// Where this user is in an item, as Jellyfin's own apps resume it.
    pub async fn set_position(&self, item_id: &str, secs: f64) -> Result<(), JfError> {
        self.set_user_data(item_id, &json!({ "PlaybackPositionTicks": seconds_to_ticks(secs) })).await
    }

    /// Changes some of this user's data on an item (position, played).
    /// Newer Jellyfin takes /UserItems; older ones only /Users/{id}/Items.
    pub async fn set_user_data(&self, item_id: &str, data: &Value) -> Result<(), JfError> {
        let uid = self.uid()?.to_string();
        match self.post(&format!("/UserItems/{item_id}/UserData?userId={uid}"), data).await {
            Err(e) if not_found(&e) => self.post(&format!("/Users/{uid}/Items/{item_id}/UserData"), data).await.map(|_| ()),
            r => r.map(|_| ()),
        }
    }

    pub async fn episodes(&self, series_id: &str) -> Result<Vec<Value>, JfError> {
        let uid = self.uid()?;
        let v = self.get(&format!("/Shows/{series_id}/Episodes?userId={uid}&fields=Overview,Path")).await?;
        Ok(v["Items"].as_array().cloned().unwrap_or_default())
    }

    /// Marks an item watched or unwatched for this user. Newer Jellyfin takes
    /// /UserPlayedItems; older ones only /Users/{id}/PlayedItems.
    pub async fn set_played(&self, item_id: &str, played: bool) -> Result<(), JfError> {
        let uid = self.uid()?.to_string();
        let method = if played { reqwest::Method::POST } else { reqwest::Method::DELETE };
        match self.call(method.clone(), &format!("/UserPlayedItems/{item_id}?userId={uid}"), None).await {
            Ok(_) => Ok(()),
            Err(JfError::Other(_)) => self.call(method, &format!("/Users/{uid}/PlayedItems/{item_id}"), None).await.map(|_| ()),
            Err(e) => Err(e),
        }
    }

    /// How to play an item: a direct file or an HLS stream, plus its tracks.
    /// `subtitle_index`: a picture-based track (PGS, DVD) to draw into the video;
    /// otherwise none is, and text tracks are shown by the player.
    pub async fn playback(&self, item_id: &str, start_seconds: f64, audio_index: Option<i64>, subtitle_index: Option<i64>) -> Result<Value, JfError> {
        let uid = self.uid()?;
        let mut body = json!({
            "DeviceProfile": device_profile(),
            "StartTimeTicks": seconds_to_ticks(start_seconds),
            "AutoOpenLiveStream": true,
            "MaxStreamingBitrate": 120_000_000,
            "EnableDirectPlay": true,
            "EnableDirectStream": true,
            "EnableTranscoding": true
        });
        if let Some(a) = audio_index {
            body["AudioStreamIndex"] = json!(a);
        }
        // -1: no track drawn in, or Jellyfin picks the file's default (often the wrong language) and burns it.
        body["SubtitleStreamIndex"] = json!(subtitle_index.unwrap_or(-1));
        self.post(&format!("/Items/{item_id}/PlaybackInfo?userId={uid}"), &body).await
    }

    /// Start, progress and stop reports, so Continue Watching knows where you are.
    pub async fn report(&self, what: &str, body: &Value) -> Result<(), JfError> {
        let path = match what {
            "start" => "/Sessions/Playing",
            "stop" => "/Sessions/Playing/Stopped",
            _ => "/Sessions/Playing/Progress",
        };
        self.post(path, body).await.map(|_| ())
    }

    /// Ends the transcode a play session started, so Jellyfin deletes its
    /// segments now instead of leaving them in its cache.
    pub async fn kill_encoding(&self, play_session_id: &str) -> Result<(), JfError> {
        let path = format!("/Videos/ActiveEncodings?deviceId={}&playSessionId={}", urlencode(&self.device_id), urlencode(play_session_id));
        self.call(reqwest::Method::DELETE, &path, None).await.map(|_| ())
    }

    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }
}

/// Jellyfin has no such route (`call` reports any other status as Other).
fn not_found(e: &JfError) -> bool {
    matches!(e, JfError::Other(m) if m.ends_with("error 404"))
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_header_names_chillarr_and_carries_the_token() {
        assert_eq!(
            auth_header("dev-1", None),
            r#"MediaBrowser Client="Chillarr", Device="Windows", DeviceId="dev-1", Version="0.2.0""#
        );
        assert_eq!(
            auth_header("dev-1", Some("tok")),
            r#"MediaBrowser Client="Chillarr", Device="Windows", DeviceId="dev-1", Version="0.2.0", Token="tok""#
        );
    }

    #[test]
    fn on_shelf_covers_watch_now_library() {
        let wn = json!({ "Path": r"C:\Users\x\AppData\Roaming\Chillarr\Chillarr Watch now\Movies\Inception [tmdbid-27205]\Inception [tmdbid-27205].strm" });
        let shelf = json!({ "Path": r"C:\Users\x\AppData\Roaming\Chillarr\Chillarr Discover\Movies\A\A.strm" });
        let real = json!({ "Path": r"D:\Media\Movies\Inception (2010)\Inception.mkv" });
        let slashes = json!({ "Path": "C:/data/chillarr watch now/Series/X/Season 01/X S01E01.strm" });
        assert!(on_shelf(&wn));
        assert!(on_shelf(&shelf));
        assert!(on_shelf(&slashes));
        assert!(!on_shelf(&real));
        assert!(!on_shelf(&json!({})));
    }

    #[test]
    fn ticks_and_seconds() {
        assert_eq!(seconds_to_ticks(1.5), 15_000_000);
        assert_eq!(ticks_to_seconds(36_000_000_000), 3600.0);
    }

    #[test]
    fn device_profile_plays_only_what_the_webview_can() {
        let p = device_profile();
        let direct = &p["DirectPlayProfiles"][0];
        assert_eq!(direct["Container"], "mp4,m4v");
        assert_eq!(direct["AudioCodec"], "aac,mp3");
        let t = &p["TranscodingProfiles"][0];
        assert_eq!(t["Protocol"], "hls");
        assert_eq!(t["VideoCodec"], "h264");
    }

    #[test]
    fn only_a_404_falls_back() {
        assert!(not_found(&JfError::Other("Jellyfin answered with error 404".into())));
        assert!(!not_found(&JfError::Other("Jellyfin answered with error 500".into())));
        assert!(!not_found(&JfError::Down));
    }
}
