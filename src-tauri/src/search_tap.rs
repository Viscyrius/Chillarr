//! Titles searched for in Jellyfin join the Discover shelf. Jellyfin never
//! tells Chillarr what was searched, but its search plugin asks Chillarr's
//! Meilisearch (meili.rs), so a small proxy sits in front of it: every byte
//! goes through unchanged both ways, and the search requests are read on the
//! side (measured 2026-10-19: the plugin sends `POST /indexes/Chillarr/search`
//! with a chunked JSON body holding `"q"`, one per keystroke on a TV).
//!
//! After a term has been quiet for a moment, Radarr and Sonarr are asked for
//! it and the close matches you don't have go on the shelf (discover_shelf.rs),
//! so the next search finds them. The proxy never makes search worse: when
//! Meilisearch can't be reached it answers 502, as if it were down, and
//! Jellyfin falls back to its own search; a failed lookup only logs.
//!
//! Only searches carrying Meilisearch's key count: any web page can send a
//! POST to 127.0.0.1, so without that check a page could fill the shelf.

use crate::log::log;
use crate::model::Level;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::{Duration, Instant};
use tauri::AppHandle;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

const PORTS: [u16; 3] = [7701, 7702, 7703];
/// A term is looked up once nobody has typed for this long.
const QUIET: Duration = Duration::from_millis(1500);
/// Shorter terms (letters and digits) match too much to look up.
const MIN_CHARS: usize = 3;
/// The same term again within this is not looked up again.
const REPEAT_AFTER: Duration = Duration::from_secs(10 * 60);
/// Request heads and search bodies are small; anything bigger isn't read.
const MAX_HEAD: usize = 32 * 1024;
const MAX_BODY: usize = 64 * 1024;

static PORT: AtomicU16 = AtomicU16::new(0);

/// The proxy's port, once it's listening.
pub fn port() -> Option<u16> {
    Some(PORT.load(Ordering::Relaxed)).filter(|p| *p != 0)
}

/// Starts the proxy, points the search plugin at it, and turns searches into shelf titles.
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut found = None;
        for p in PORTS {
            if let Ok(l) = tokio::net::TcpListener::bind(("127.0.0.1", p)).await {
                found = Some((l, p));
                break;
            }
        }
        let Some((listener, p)) = found else {
            log(&app, Level::Amber, "Couldn't open a port for Jellyfin's search (7701-7703 are taken), so titles searched for there won't join the Discover shelf.");
            return;
        };
        PORT.store(p, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel::<String>(256);
        tauri::async_runtime::spawn(debounce(app.clone(), rx));
        let h = app.clone();
        tauri::async_runtime::spawn(async move {
            // Jellyfin may still be starting: try for about 20 minutes.
            for _ in 0..40 {
                if crate::meili::sync_url(&h).await {
                    return;
                }
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        });
        loop {
            let Ok((sock, _)) = listener.accept().await else { continue };
            tauri::async_runtime::spawn(forward(sock, tx.clone(), meili_key(&app)));
        }
    });
}

/// Meilisearch's key (meili.rs keeps it in `meili\key.txt`), read once it exists.
/// Only a request carrying it is the search plugin's: any web page can reach
/// 127.0.0.1, but none knows the key.
fn meili_key(app: &AppHandle) -> Option<String> {
    use tauri::Manager;
    static KEY: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    let mut k = KEY.lock().unwrap_or_else(|e| e.into_inner());
    if k.is_none() {
        let file = app.path().app_data_dir().ok()?.join("meili").join("key.txt");
        *k = std::fs::read_to_string(file).ok().map(|t| t.trim().to_string()).filter(|t| t.len() >= 32);
    }
    k.clone()
}

/// Whether a request head carries `Authorization: Bearer <key>`.
fn has_key(head: &str, key: Option<&str>) -> bool {
    let Some(key) = key.filter(|k| !k.is_empty()) else { return false };
    let want = format!("Bearer {key}");
    head.lines().skip(1).any(|l| l.split_once(':').is_some_and(|(k, v)| k.trim().eq_ignore_ascii_case("authorization") && crate::remote::ct_eq(v.trim().as_bytes(), want.as_bytes())))
}

/// Passes one connection through to Meilisearch, reading search terms on the way.
async fn forward(client: TcpStream, tx: mpsc::Sender<String>, key: Option<String>) {
    let _ = client.set_nodelay(true);
    let upstream = match tokio::time::timeout(Duration::from_secs(3), TcpStream::connect(crate::meili::ADDR)).await {
        Ok(Ok(u)) => u,
        _ => return bad_gateway(client).await,
    };
    let _ = upstream.set_nodelay(true);
    let (mut cr, mut cw) = client.into_split();
    let (mut ur, mut uw) = upstream.into_split();
    let up = async {
        let mut tap = Tap { key, ..Tap::default() };
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = match cr.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            if uw.write_all(&buf[..n]).await.is_err() {
                break;
            }
            for body in tap.feed(&buf[..n]) {
                for t in search_terms(&body) {
                    let _ = tx.try_send(t);
                }
            }
        }
        let _ = uw.shutdown().await;
    };
    let down = async {
        let _ = tokio::io::copy(&mut ur, &mut cw).await;
        let _ = cw.shutdown().await;
    };
    tokio::join!(up, down);
}

/// What Meilisearch being down looks like through a proxy.
async fn bad_gateway(mut sock: TcpStream) {
    let mut buf = [0u8; 4096];
    let _ = tokio::time::timeout(Duration::from_secs(2), sock.read(&mut buf)).await;
    let body = r#"{"message":"Meilisearch isn't running","code":"bad_gateway","type":"system"}"#;
    let head = format!("HTTP/1.1 502 Bad Gateway\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    let _ = sock.write_all(head.as_bytes()).await;
    let _ = sock.write_all(body.as_bytes()).await;
    let _ = sock.shutdown().await;
}

/// `POST /indexes/<index>/search` and `POST /multi-search`.
fn is_search(method: &str, path: &str) -> bool {
    let path = path.split('?').next().unwrap_or_default();
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    method == "POST" && (parts == ["multi-search"] || matches!(parts.as_slice(), ["indexes", _, "search"]))
}

/// The terms a search body asks for: its `q`, or each query's `q` in a multi-search.
fn search_terms(body: &[u8]) -> Vec<String> {
    let Ok(v) = serde_json::from_slice::<Value>(body) else { return vec![] };
    let mut out: Vec<String> = Vec::new();
    for q in std::iter::once(&v).chain(v["queries"].as_array().into_iter().flatten()) {
        if let Some(t) = q["q"].as_str().map(|t| t.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|t| !t.is_empty()) {
            if !out.contains(&t) {
                out.push(t);
            }
        }
    }
    out
}

#[derive(Default, Debug, PartialEq)]
enum State {
    #[default]
    Head,
    /// Body bytes left (Content-Length).
    Fixed(u64),
    /// A chunk's size line.
    Size,
    /// Chunk bytes left.
    Data(u64),
    /// The CRLF after a chunk's data, bytes left.
    DataEnd(u8),
    /// Trailer lines after the last chunk, up to an empty one.
    Trailer,
    /// Something it can't follow: stop reading (bytes still go through).
    Lost,
}

/// Follows the HTTP/1.1 requests in a byte stream and hands back the bodies
/// of search requests that carry Meilisearch's key. Only reads; what it can't
/// follow it stops reading.
#[derive(Default)]
struct Tap {
    state: State,
    line: Vec<u8>,
    keep: bool,
    body: Vec<u8>,
    /// Meilisearch's key; without it no search is taken.
    key: Option<String>,
}

impl Tap {
    fn feed(&mut self, mut buf: &[u8]) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        while !buf.is_empty() {
            match self.state {
                State::Lost => break,
                State::Head | State::Size | State::Trailer => {
                    let b = buf[0];
                    buf = &buf[1..];
                    self.line.push(b);
                    if self.line.len() > MAX_HEAD {
                        self.state = State::Lost;
                    } else if b == b'\n' {
                        self.line_done(&mut out);
                    }
                }
                State::Fixed(n) | State::Data(n) => {
                    let k = (n as usize).min(buf.len());
                    self.take(&buf[..k]);
                    buf = &buf[k..];
                    let left = n - k as u64;
                    let fixed = matches!(self.state, State::Fixed(_));
                    self.state = match (fixed, left) {
                        (true, 0) => {
                            self.finish(&mut out);
                            State::Head
                        }
                        (true, l) => State::Fixed(l),
                        (false, 0) => State::DataEnd(2),
                        (false, l) => State::Data(l),
                    };
                }
                State::DataEnd(k) => {
                    buf = &buf[1..];
                    self.state = if k <= 1 { State::Size } else { State::DataEnd(k - 1) };
                }
            }
        }
        out
    }

    /// A line ended (the head's last, a chunk size, a trailer).
    fn line_done(&mut self, out: &mut Vec<Vec<u8>>) {
        match self.state {
            State::Head => {
                if self.line == b"\r\n" || self.line == b"\n" {
                    // A blank line between requests.
                    self.line.clear();
                } else if self.line.ends_with(b"\r\n\r\n") || self.line.ends_with(b"\n\n") {
                    self.start_body(out);
                }
            }
            State::Size => {
                let text = String::from_utf8_lossy(&self.line).to_string();
                self.line.clear();
                let hex = text.split(';').next().unwrap_or_default().trim();
                self.state = match u64::from_str_radix(hex, 16) {
                    Ok(0) => State::Trailer,
                    Ok(n) => State::Data(n),
                    Err(_) => State::Lost,
                };
            }
            State::Trailer => {
                let blank = self.line == b"\r\n" || self.line == b"\n";
                self.line.clear();
                if blank {
                    self.finish(out);
                    self.state = State::Head;
                }
            }
            _ => {}
        }
    }

    fn start_body(&mut self, out: &mut Vec<Vec<u8>>) {
        let head = String::from_utf8_lossy(&self.line).to_string();
        self.line.clear();
        let mut lines = head.lines();
        let mut first = lines.next().unwrap_or_default().split_whitespace();
        let (method, path) = (first.next().unwrap_or_default(), first.next().unwrap_or_default());
        self.keep = is_search(method, path) && has_key(&head, self.key.as_deref());
        self.body.clear();
        let (mut length, mut chunked) = (0u64, false);
        for l in lines {
            let Some((k, v)) = l.split_once(':') else { continue };
            match k.trim().to_ascii_lowercase().as_str() {
                "content-length" => match v.trim().parse() {
                    Ok(n) => length = n,
                    Err(_) => {
                        self.state = State::Lost;
                        return;
                    }
                },
                "transfer-encoding" => chunked = v.to_ascii_lowercase().contains("chunked"),
                _ => {}
            }
        }
        self.state = if chunked {
            State::Size
        } else if length > 0 {
            State::Fixed(length)
        } else {
            self.finish(out);
            State::Head
        };
    }

    fn take(&mut self, bytes: &[u8]) {
        if !self.keep {
            return;
        }
        if self.body.len() + bytes.len() > MAX_BODY {
            self.keep = false;
            self.body = Vec::new();
        } else {
            self.body.extend_from_slice(bytes);
        }
    }

    fn finish(&mut self, out: &mut Vec<Vec<u8>>) {
        if self.keep && !self.body.is_empty() {
            out.push(std::mem::take(&mut self.body));
        }
        self.keep = false;
        self.body.clear();
    }
}

/// Holds terms until typing stops. A TV sends a search per letter ("ir",
/// "iro", "iron"), so a term replaces any waiting one it extends or shortens,
/// and goes out once it's been quiet for `QUIET`.
#[derive(Default)]
struct Debounce {
    /// (squashed key, term as typed, last seen)
    waiting: Vec<(String, String, Instant)>,
    /// Looked up lately, by key.
    done: HashMap<String, Instant>,
}

impl Debounce {
    fn note(&mut self, term: &str, now: Instant) {
        let key = crate::media::squash(term);
        if key.chars().count() < MIN_CHARS {
            return;
        }
        self.waiting.retain(|(k, _, _)| !(k.starts_with(&key) || key.starts_with(k.as_str())));
        self.waiting.push((key, term.trim().to_string(), now));
    }

    fn due(&mut self, now: Instant) -> Vec<String> {
        let Debounce { waiting, done } = self;
        done.retain(|_, at| now.duration_since(*at) < REPEAT_AFTER);
        let mut out = Vec::new();
        waiting.retain(|(k, t, at)| {
            if now.duration_since(*at) < QUIET {
                return true;
            }
            if !done.contains_key(k) {
                done.insert(k.clone(), now);
                out.push(t.clone());
            }
            false
        });
        out
    }
}

/// Turns the stream of search terms into lookups, one at a time.
async fn debounce(app: AppHandle, mut rx: mpsc::Receiver<String>) {
    let (wtx, mut wrx) = mpsc::channel::<String>(32);
    let h = app.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(t) = wrx.recv().await {
            crate::discover_shelf::add_searched(&h, &t).await;
        }
    });
    let mut d = Debounce::default();
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    loop {
        tokio::select! {
            m = rx.recv() => match m {
                Some(t) => d.note(&t, Instant::now()),
                None => break,
            },
            _ = tick.tick() => {
                for t in d.due(Instant::now()) {
                    let _ = wtx.try_send(t);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The plugin's request, as captured on 2026-10-19 (key shortened).
    const CAPTURED: &[u8] = b"POST /indexes/Chillarr/search HTTP/1.1\r\nHost: 127.0.0.1:7701\r\nAuthorization: Bearer x\r\nUser-Agent: Meilisearch .NET (v0.20.0)\r\nTransfer-Encoding: chunked\r\nContent-Type: application/json; charset=utf-8\r\n\r\n114\r\n{\"limit\":60,\"retrieveVectors\":false,\"q\":\"iron man\",\"filter\":\"(type = \\u0022MediaBrowser.Controller.Entities.Movies.Movie\\u0022 OR type = \\u0022MediaBrowser.Controller.Entities.TV.Series\\u0022)\",\"attributesToRetrieve\":[\"guid\"],\"matchingStrategy\":\"last\",\"showRankingScore\":true}\r\n0\r\n\r\n";

    fn terms(tap: &mut Tap, bytes: &[u8]) -> Vec<String> {
        tap.feed(bytes).iter().flat_map(|b| search_terms(b)).collect()
    }

    /// A tap that knows the key CAPTURED carries.
    fn keyed() -> Tap {
        Tap { key: Some("x".into()), ..Tap::default() }
    }

    #[test]
    fn only_searches_with_the_key_are_taken() {
        // Any web page can POST here; without the key nothing joins the shelf.
        let page = "POST /indexes/Chillarr/search HTTP/1.1\r\nContent-Type: text/plain\r\nContent-Length: 16\r\n\r\n{\"q\":\"stuffing\"}";
        assert!(terms(&mut keyed(), page.as_bytes()).is_empty());
        let wrong = String::from_utf8_lossy(CAPTURED).replace("Bearer x", "Bearer y");
        assert!(terms(&mut keyed(), wrong.as_bytes()).is_empty());
        // Meilisearch's key not made yet: nothing is taken, even with a header.
        assert!(terms(&mut Tap::default(), CAPTURED).is_empty());
        // The plugin's own request still is, and the stream is still followed after.
        let mut tap = keyed();
        assert!(terms(&mut tap, page.as_bytes()).is_empty());
        assert_eq!(terms(&mut tap, CAPTURED), vec!["iron man"]);
        assert!(!has_key("POST / HTTP/1.1\r\nAuthorization: Bearer x", Some("")));
        assert!(!has_key("Authorization: Bearer x\r\n", Some("x")), "the request line isn't a header");
    }

    #[test]
    fn the_plugins_search_is_read_however_it_arrives() {
        assert_eq!(terms(&mut keyed(), CAPTURED), vec!["iron man"]);
        // Byte by byte, as a slow socket might hand it over.
        let mut tap = keyed();
        let got: Vec<String> = CAPTURED.iter().flat_map(|b| terms(&mut tap, std::slice::from_ref(b))).collect();
        assert_eq!(got, vec!["iron man"]);
    }

    #[test]
    fn requests_on_one_connection_are_followed() {
        let docs = b"POST /indexes/Chillarr/documents?primaryKey=guid HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n5\r\n[{\"q\"\r\n3\r\n:1}\r\n0\r\n\r\n";
        let health = b"GET /health HTTP/1.1\r\nHost: x\r\n\r\n";
        let multi = r#"{"queries":[{"q":"dune"},{"q":"dune"},{"q":""}]}"#;
        let fixed = format!("POST /multi-search HTTP/1.1\r\nauthorization:  Bearer x \r\nContent-Length: {}\r\n\r\n{multi}", multi.len());
        let mut stream = Vec::new();
        stream.extend_from_slice(docs);
        stream.extend_from_slice(health);
        stream.extend_from_slice(CAPTURED);
        stream.extend_from_slice(fixed.as_bytes());
        let mut tap = keyed();
        let (a, b) = stream.split_at(150);
        let mut got = terms(&mut tap, a);
        got.extend(terms(&mut tap, b));
        assert_eq!(got, vec!["iron man", "dune"], "documents and health aren't searches");
    }

    #[test]
    fn what_it_cant_follow_it_stops_reading() {
        let mut tap = keyed();
        assert!(tap.feed(b"POST /indexes/x/search HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n").is_empty());
        assert_eq!(tap.state, State::Lost);
        assert!(tap.feed(CAPTURED).is_empty());
        // A huge body isn't kept.
        let mut tap = keyed();
        let big = format!("POST /indexes/x/search HTTP/1.1\r\nContent-Length: {}\r\n\r\n{{\"q\":\"{}\"}}", MAX_BODY + 9, "a".repeat(MAX_BODY + 1));
        assert!(tap.feed(big.as_bytes()).is_empty());
        assert_eq!(tap.state, State::Head);
    }

    #[test]
    fn only_search_paths_count() {
        assert!(is_search("POST", "/indexes/Chillarr/search"));
        assert!(is_search("POST", "/multi-search"));
        assert!(!is_search("GET", "/indexes/Chillarr/search"));
        assert!(!is_search("POST", "/indexes/Chillarr/documents"));
        assert!(!is_search("POST", "/indexes/Chillarr/facet-search"));
        assert_eq!(search_terms(br#"{"q":"  iron   man "}"#), vec!["iron man"]);
        assert!(search_terms(b"not json").is_empty());
    }

    #[test]
    fn typing_waits_for_quiet_and_sends_the_last_term_once() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut d = Debounce::default();
        for (i, t) in ["i", "ir", "iro", "iron", "iron ", "iron m", "iron ma", "iron man"].iter().enumerate() {
            d.note(t, ms(i as u64 * 300));
        }
        assert!(d.due(ms(2200)).is_empty(), "still typing");
        assert_eq!(d.due(ms(3700)), vec!["iron man"]);
        assert!(d.due(ms(9000)).is_empty());
        // The same search again soon isn't looked up again; much later it is.
        d.note("Iron Man", ms(10_000));
        assert!(d.due(ms(12_000)).is_empty());
        d.note("iron man", ms(11 * 60_000));
        assert_eq!(d.due(ms(11 * 60_000 + 1600)), vec!["iron man"]);
    }

    #[test]
    fn short_and_unrelated_terms() {
        let t0 = Instant::now();
        let mut d = Debounce::default();
        d.note("x2", t0);
        d.note("dune", t0);
        d.note("matrix", t0);
        // Deleting back to a shorter term replaces the longer one.
        d.note("matri", t0 + Duration::from_millis(100));
        let mut got = d.due(t0 + Duration::from_secs(2));
        got.sort();
        assert_eq!(got, vec!["dune", "matri"]);
    }
}
