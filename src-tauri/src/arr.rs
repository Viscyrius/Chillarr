//! Typed client for the Prowlarr (v1) and Sonarr/Radarr (v3) HTTP APIs.
//! Chillarr only talks to these apps over HTTP; none of their code is used.

use crate::model::{AppConfig, AppId};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
/// Indexer tests go out to the real sites (through FlareSolverr for 1337x), which is slow.
const TEST_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Debug)]
pub enum ArrError {
    Down,
    BadKey,
    Http(u16),
    /// The app said no, with its own reason ("already added", "not writable"...).
    Rejected(String),
    Other(String),
}

impl std::fmt::Display for ArrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArrError::Down => write!(f, "it isn't answering"),
            ArrError::BadKey => write!(f, "it rejected the API key"),
            ArrError::Http(c) => write!(f, "it answered with error {c}"),
            ArrError::Rejected(m) | ArrError::Other(m) => write!(f, "{m}"),
        }
    }
}

/// The reason in an *arr error body: a validation list or a {message} object.
pub fn reason(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let m = if let Some(list) = v.as_array() {
        list.iter().filter_map(|x| x["errorMessage"].as_str()).collect::<Vec<_>>().join(" ")
    } else {
        v["message"].as_str().unwrap_or_default().to_string()
    };
    (!m.is_empty()).then_some(m)
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Health {
    pub source: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub message: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Indexer {
    pub id: i64,
    pub name: String,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Failure {
    pub error_message: String,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TestResult {
    pub id: i64,
    pub is_valid: bool,
    #[serde(default)]
    pub validation_failures: Vec<Failure>,
}

/// Allowed Hosts covering every name this PC is reached by: itself, its
/// computer name and its LAN address (so a phone on the same network still works).
pub fn allowed_hosts_value(computer: &str, lan_ip: Option<&str>) -> String {
    let mut v = vec!["localhost", "127.0.0.1"];
    if !computer.is_empty() {
        v.push(computer);
    }
    if let Some(ip) = lan_ip {
        v.push(ip);
    }
    v.join(",")
}

/// This PC's LAN address. Connecting a UDP socket sends nothing; it only
/// asks Windows which interface it would use.
pub fn lan_ip() -> Option<String> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    let ip = s.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then(|| ip.to_string())
}

#[derive(Deserialize)]
struct CommandRef {
    id: i64,
    #[serde(default)]
    status: String,
}

pub struct Arr {
    http: reqwest::Client,
    base: String,
    key: String,
    ver: &'static str,
}

impl Arr {
    /// `None` for apps without an `*arr` API, or when discovery found no key.
    pub fn new(cfg: &AppConfig) -> Option<Arr> {
        let ver = match cfg.id {
            AppId::Prowlarr => "v1",
            AppId::Sonarr | AppId::Radarr => "v3",
            _ => return None,
        };
        let http = reqwest::Client::builder().connect_timeout(CONNECT_TIMEOUT).build().ok()?;
        Some(Arr { http, base: cfg.base_url.clone(), key: cfg.api_key.clone()?, ver })
    }

    fn url(&self, path: &str) -> String {
        format!("{}/api/{}/{}", self.base, self.ver, path)
    }

    async fn send(&self, req: reqwest::RequestBuilder, timeout: Duration, ok_400: bool) -> Result<String, ArrError> {
        let resp = req.header("X-Api-Key", &self.key).timeout(timeout).send().await.map_err(|e| {
            if e.is_connect() || e.is_timeout() {
                ArrError::Down
            } else {
                ArrError::Other(e.to_string())
            }
        })?;
        let code = resp.status().as_u16();
        match code {
            200..=299 => {}
            400 if ok_400 => {}
            401 | 403 => return Err(ArrError::BadKey),
            400 | 409 | 422 => {
                let body = resp.text().await.unwrap_or_default();
                return Err(reason(&body).map(ArrError::Rejected).unwrap_or(ArrError::Http(code)));
            }
            _ => return Err(ArrError::Http(code)),
        }
        resp.text().await.map_err(|e| ArrError::Other(e.to_string()))
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, ArrError> {
        let body = self.send(self.http.get(self.url(path)), REQUEST_TIMEOUT, false).await?;
        serde_json::from_str(&body).map_err(|e| ArrError::Other(format!("{path}: {e}")))
    }

    /// A test endpoint answers 200 when everything passed and 400 with the same
    /// list when something failed.
    async fn test_all(&self, path: &str) -> Result<Vec<TestResult>, ArrError> {
        let body = self.send(self.http.post(self.url(path)), TEST_TIMEOUT, true).await?;
        serde_json::from_str(&body).map_err(|e| ArrError::Other(format!("{path}: {e}")))
    }

    pub async fn status(&self) -> Result<(), ArrError> {
        self.get::<serde_json::Value>("system/status").await.map(|_| ())
    }

    pub async fn health(&self) -> Result<Vec<Health>, ArrError> {
        self.get("health").await
    }

    pub async fn indexers(&self) -> Result<Vec<Indexer>, ArrError> {
        self.get("indexer").await
    }

    /// `(id, name)` of anything listed at `path` (indexers, download clients, applications).
    pub async fn names(&self, path: &str) -> Result<Vec<Indexer>, ArrError> {
        self.get(path).await
    }

    pub async fn test_all_indexers(&self) -> Result<Vec<TestResult>, ArrError> {
        self.test_all("indexer/testall").await
    }

    /// Tests each indexer on its own, leaving out the ones `skip` names. A
    /// failed test counts against a site in Radarr (it sits the site out for a
    /// while, longer each time), so a test known to fail falsely isn't run.
    pub async fn test_indexers_skipping(&self, skip: impl Fn(&str) -> bool) -> Result<Vec<TestResult>, ArrError> {
        let list: Vec<serde_json::Value> = self.get("indexer").await?;
        let mut out = Vec::new();
        for i in list.into_iter().filter(|i| !skip(i["name"].as_str().unwrap_or_default())) {
            let Some(id) = i["id"].as_i64() else { continue };
            let body = self.send(self.http.post(self.url("indexer/test")).json(&i), TEST_TIMEOUT, true).await?;
            // Passing: an empty answer. Failing: a 400 with the reasons.
            let failures: Vec<Failure> = serde_json::from_str(&body).unwrap_or_default();
            out.push(TestResult { id, is_valid: failures.is_empty(), validation_failures: failures });
        }
        Ok(out)
    }

    pub async fn test_all_download_clients(&self) -> Result<Vec<TestResult>, ArrError> {
        self.test_all("downloadclient/testall").await
    }

    /// Prowlarr only: can it reach Sonarr and Radarr.
    pub async fn test_all_applications(&self) -> Result<Vec<TestResult>, ArrError> {
        self.test_all("applications/testall").await
    }

    /// Prowlarr only: tests each indexer proxy (FlareSolverr). `(name, error)` per proxy.
    pub async fn test_proxies(&self) -> Result<Vec<(String, Option<String>)>, ArrError> {
        let proxies: Vec<serde_json::Value> = self.get("indexerproxy").await?;
        let mut out = Vec::new();
        for p in proxies {
            let name = p["name"].as_str().unwrap_or("Proxy").to_string();
            let req = self.http.post(self.url("indexerproxy/test")).json(&p);
            let err = match self.send(req, TEST_TIMEOUT, true).await {
                Ok(body) if body.trim().is_empty() || body.trim() == "{}" => None,
                Ok(body) => {
                    let fails: Vec<Failure> = serde_json::from_str(&body).unwrap_or_default();
                    fails.first().map(|f| f.error_message.clone())
                }
                Err(e) => Some(format!("{e:?}")),
            };
            out.push((name, err));
        }
        Ok(out)
    }

    /// Fills Allowed Hosts when it is empty. `Ok(true)` when it changed it
    /// (the app needs a restart for that to apply).
    pub async fn set_allowed_hosts_if_empty(&self, value: &str) -> Result<bool, ArrError> {
        let mut cfg: serde_json::Value = self.get("config/host").await?;
        if cfg["allowedHosts"].as_str().is_some_and(|s| !s.trim().is_empty()) {
            return Ok(false);
        }
        cfg["allowedHosts"] = serde_json::Value::String(value.to_string());
        let req = self.http.put(self.url("config/host")).json(&cfg);
        self.send(req, REQUEST_TIMEOUT, false).await.map(|_| true)
    }

    /// Asks the app to restart itself. Works for service installs too, no admin needed.
    pub async fn restart(&self) -> Result<(), ArrError> {
        self.send(self.http.post(self.url("system/restart")), REQUEST_TIMEOUT, false).await.map(|_| ())
    }

    /// Any endpoint that returns one JSON object.
    pub async fn get_one(&self, path: &str) -> Result<serde_json::Value, ArrError> {
        self.get(path).await
    }

    /// A list endpoint that asks the indexers live (`release?movieId=`), which
    /// takes far longer than the usual 20 seconds.
    pub async fn list_slow(&self, path: &str) -> Result<Vec<serde_json::Value>, ArrError> {
        let body = self.send(self.http.get(self.url(path)), TEST_TIMEOUT, false).await?;
        serde_json::from_str(&body).map_err(|e| ArrError::Other(format!("{path}: {e}")))
    }

    /// Any list endpoint as raw JSON (schemas, links, tags).
    pub async fn list(&self, path: &str) -> Result<Vec<serde_json::Value>, ArrError> {
        self.get(path).await
    }

    /// Adds an item without the app testing it first (the other side may still be starting).
    pub async fn create(&self, path: &str, body: &serde_json::Value) -> Result<serde_json::Value, ArrError> {
        let req = self.http.post(self.url(&format!("{path}?forceSave=true"))).json(body);
        let text = self.send(req, REQUEST_TIMEOUT, false).await?;
        Ok(serde_json::from_str(&text).unwrap_or(serde_json::Value::Null))
    }

    /// Saves a changed item back (PUT), e.g. `series/12` or `episode/monitor`.
    pub async fn update(&self, path: &str, body: &serde_json::Value) -> Result<(), ArrError> {
        let req = self.http.put(self.url(path)).json(body);
        self.send(req, REQUEST_TIMEOUT, false).await.map(|_| ())
    }

    /// Removes several at once, e.g. `queue/bulk` with `{"ids": [...]}`.
    pub async fn delete_with(&self, path: &str, body: &serde_json::Value) -> Result<(), ArrError> {
        self.send(self.http.delete(self.url(path)).json(body), REQUEST_TIMEOUT, false).await.map(|_| ())
    }

    /// Removes an item, e.g. `movie/12?deleteFiles=false`.
    pub async fn delete(&self, path: &str) -> Result<(), ArrError> {
        self.send(self.http.delete(self.url(path)), REQUEST_TIMEOUT, false).await.map(|_| ())
    }

    /// Release titles this app grabbed (sent to the download client), newest first.
    pub async fn grabbed_titles(&self) -> Result<Vec<String>, ArrError> {
        let page: serde_json::Value = self.get("history?eventType=1&pageSize=250&sortKey=date&sortDirection=descending").await?;
        Ok(page["records"]
            .as_array()
            .map(|r| r.iter().filter_map(|x| x["sourceTitle"].as_str().map(String::from)).collect())
            .unwrap_or_default())
    }

    /// Asks the app to import a finished download at `path` it lost track of.
    /// "Copy" leaves the original for seeding; on the same drive the app makes
    /// a hardlink, so it takes no extra space.
    pub async fn import_from(&self, scan_command: &str, path: &str) -> Result<i64, ArrError> {
        self.command_body(serde_json::json!({ "name": scan_command, "path": path, "importMode": "Copy" })).await
    }

    pub async fn command(&self, name: &str) -> Result<i64, ArrError> {
        self.command_body(serde_json::json!({ "name": name })).await
    }

    pub async fn command_with(&self, body: serde_json::Value) -> Result<i64, ArrError> {
        self.command_body(body).await
    }

    async fn command_body(&self, body: serde_json::Value) -> Result<i64, ArrError> {
        let req = self.http.post(self.url("command")).json(&body);
        let body = self.send(req, REQUEST_TIMEOUT, false).await?;
        let c: CommandRef = serde_json::from_str(&body).map_err(|e| ArrError::Other(e.to_string()))?;
        Ok(c.id)
    }

    pub async fn command_done(&self, id: i64) -> Result<bool, ArrError> {
        let c: CommandRef = self.get(&format!("command/{id}")).await?;
        Ok(matches!(c.status.as_str(), "completed" | "failed" | "aborted" | "cancelled" | "orphaned"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AppConfig;

    fn cfg(port: u16) -> AppConfig {
        AppConfig {
            id: AppId::Radarr,
            base_url: format!("http://127.0.0.1:{port}"),
            api_key: Some("k".into()),
            exe: None,
            service: None,
            managed: true,
            problem: None,
        }
    }

    #[tokio::test]
    async fn arr_maps_401_to_bad_key() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (mut s, _) = l.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf).await;
            let _ = s
                .write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await;
        });
        let arr = Arr::new(&cfg(port)).unwrap();
        assert!(matches!(arr.health().await, Err(ArrError::BadKey)));
    }

    #[tokio::test]
    async fn arr_maps_refused_to_down() {
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let arr = Arr::new(&cfg(port)).unwrap();
        assert!(matches!(arr.health().await, Err(ArrError::Down)));
    }

    #[test]
    fn allowed_hosts_cover_this_pc() {
        assert_eq!(
            allowed_hosts_value("GAMING-PC", Some("192.168.1.5")),
            "localhost,127.0.0.1,GAMING-PC,192.168.1.5"
        );
        assert_eq!(allowed_hosts_value("PC", None), "localhost,127.0.0.1,PC");
        assert_eq!(allowed_hosts_value("", None), "localhost,127.0.0.1");
    }

    #[test]
    fn rejection_keeps_the_reason() {
        let list = r#"[{"propertyName":"TmdbId","errorMessage":"This movie has already been added"}]"#;
        assert_eq!(reason(list).as_deref(), Some("This movie has already been added"));
        let obj = r#"{"message":"Folder is not writable by user anime"}"#;
        assert_eq!(reason(obj).as_deref(), Some("Folder is not writable by user anime"));
        assert_eq!(reason("<html>"), None);
        assert_eq!(ArrError::Rejected("x".into()).to_string(), "x");
    }

    #[test]
    fn no_key_no_client() {
        let mut c = cfg(1);
        c.api_key = None;
        assert!(Arr::new(&c).is_none());
        c.api_key = Some("k".into());
        c.id = AppId::FlareSolverr;
        assert!(Arr::new(&c).is_none());
    }

    #[test]
    fn parses_failed_test_results() {
        let body = r#"[{"id":3,"isValid":false,"validationFailures":[{"propertyName":"","errorMessage":"Unable to connect to indexer"}]},{"id":1,"isValid":true,"validationFailures":[]}]"#;
        let r: Vec<TestResult> = serde_json::from_str(body).unwrap();
        assert!(!r[0].is_valid);
        assert_eq!(r[0].validation_failures[0].error_message, "Unable to connect to indexer");
        assert!(r[1].is_valid);
    }

    #[tokio::test]
    #[ignore]
    async fn live_health_reads() {
        for c in crate::discover::discover_all() {
            if matches!(c.id, AppId::Prowlarr | AppId::Radarr) {
                let a = Arr::new(&c).unwrap();
                assert!(a.health().await.is_ok(), "{:?}", c.id);
                assert!(!a.indexers().await.unwrap().is_empty());
            }
        }
    }
}
