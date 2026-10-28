//! The phone remote: a small page Chillarr serves on the home network. Open
//! the address on a phone on the same Wi-Fi, type the 10-character code once,
//! and the phone becomes a D-pad, back button, play/pause and search box.
//! Off until turned on in Settings. Every command needs the code. Wrong codes
//! lock out the address that sent them, for longer each time, so a stranger
//! can neither guess the code nor lock out the real phone. Requests must name
//! the PC by its IP or name (no DNS rebinding from a web page).
//!
//! There is one code for the Chillarr window on this PC and one for each TV,
//! phone or browser that runs Chillarr inside Jellyfin's pages (bridge.rs).
//! A device's code is shown on that device, under Settings > Phone remote,
//! and stays the same until someone asks for a new one there or forgets the
//! device on the PC. Commands typed under a device's code go to that device
//! only, over its event stream.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Tried in order; the first free one is used.
const PORTS: [u16; 3] = [8686, 8687, 8688];
const MAX_REQUEST: usize = 32 * 1024;
/// The whole request (head and body) must arrive within this.
const REQUEST_WAIT: Duration = Duration::from_secs(10);
/// Connections served at once; more are dropped.
const MAX_CONNECTIONS: usize = 64;
/// Wrong codes from one address before it's locked out.
const LOCK_AFTER: u32 = 8;
/// The first lockout; each one after doubles, up to `LOCK_MAX`.
const LOCK_SECS: u64 = 60;
const LOCK_MAX: u64 = 24 * 3600;
/// Addresses remembered for lockouts; the stalest goes first past this.
const MAX_TRACKED: usize = 4096;
/// No digits or letters that look alike (0/O, 1/I/L).
const CODE_CHARS: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";
const CODE_LEN: usize = 10;

/// Devices with their own code. Past this, one unseen for `STALE_SECS` makes
/// room; while every one was seen lately, a new device is turned away.
const MAX_DEVICES: usize = 64;
const STALE_SECS: u64 = 30 * 24 * 3600;
/// A device's key: random bytes, sent as hex.
const KEY_BYTES: usize = 16;
/// A device's name as shown on the PC and the phone.
const NAME_LEN: usize = 40;
/// How often the PC's Tailscale name is looked up again while the remote runs.
const TAILSCALE_EVERY: Duration = Duration::from_secs(60);

/// The actions the page may send; anything else is ignored.
const ACTIONS: [&str; 15] = ["up", "down", "left", "right", "select", "back", "play", "tab-prev", "tab-next", "hero-prev", "hero-next", "search", "seek-back", "seek-fwd", "type"];

#[derive(Serialize, Deserialize, Clone, Default)]
struct Saved {
    on: bool,
    /// The code for the Chillarr window on this PC.
    pin: String,
    #[serde(default)]
    devices: Vec<Device>,
}

/// A TV or browser with its own code.
///
/// It is known by a key Chillarr gave it, kept in that browser, never by its
/// Jellyfin device id: Jellyfin takes any DeviceId a caller names next to a
/// good token (measured on 12.1), so anyone signed in could have claimed the
/// TV's id and read, renewed or listened in on its code.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Device {
    /// Chillarr's own name for it: what the window forgets it by, and what
    /// its event stream is tagged with. Not a secret.
    id: String,
    /// The secret the device proves itself with. Only the device ever has it.
    /// (Missing in a test build's file: such an entry is dropped on load.)
    #[serde(default)]
    key: String,
    name: String,
    code: String,
    /// Last time it asked for its code or opened its event stream (seconds).
    #[serde(default)]
    seen: u64,
}

/// A device as the window lists it (no key).
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct DeviceView {
    pub id: String,
    pub name: String,
    pub code: String,
}

/// What a code drives.
#[derive(Clone, Debug, PartialEq)]
enum Target {
    Window,
    /// A device's id and name.
    Device(String, String),
}

/// Wrong codes from one address.
#[derive(Clone, Copy, Default)]
struct Tries {
    failures: u32,
    /// Lockouts so far; each doubles the next.
    strikes: u32,
    locked_until: u64,
    last: u64,
}

pub struct Remote {
    path: PathBuf,
    saved: Mutex<Saved>,
    running: AtomicBool,
    port: Mutex<Option<u16>>,
    /// Bumped to stop the running server.
    generation: AtomicU64,
    tries: Mutex<HashMap<IpAddr, Tries>>,
    /// This PC's Tailscale name (like pc.tailnet.ts.net), when Tailscale is connected.
    tailscale: Mutex<Option<String>>,
    /// And its Tailscale IP, for the addresses shown.
    tailscale_ip: Mutex<Option<String>>,
}

#[derive(Serialize)]
pub struct Status {
    pub on: bool,
    pub running: bool,
    /// The address on the home network.
    pub url: Option<String>,
    /// Every address that reaches it: home network first, then Tailscale.
    pub urls: Vec<String>,
    pub pin: String,
    /// Devices with their own code (the window's view only).
    pub devices: Vec<DeviceView>,
}

/// What a device is shown about its own code. `key` is what it keeps to
/// prove itself next time (new when it had none, or a key nobody knows).
#[derive(Serialize)]
pub struct DeviceStatus {
    pub on: bool,
    pub urls: Vec<String>,
    pub code: String,
    pub key: String,
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// A new code from the OS's random source. None if that source fails.
fn new_pin() -> Option<String> {
    let mut out = String::with_capacity(CODE_LEN);
    // Bytes past the last whole multiple of the alphabet are skipped, so every
    // character is equally likely.
    let limit = (256 / CODE_CHARS.len() * CODE_CHARS.len()) as u8;
    while out.len() < CODE_LEN {
        let mut b = [0u8; 32];
        getrandom::getrandom(&mut b).ok()?;
        for x in b.into_iter().filter(|x| *x < limit) {
            if out.len() < CODE_LEN {
                out.push(CODE_CHARS[x as usize % CODE_CHARS.len()] as char);
            }
        }
    }
    Some(out)
}

/// Random bytes as hex (a device's key or id). None if the OS's source fails.
fn new_key(bytes: usize) -> Option<String> {
    let mut b = vec![0u8; bytes];
    getrandom::getrandom(&mut b).ok()?;
    Some(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// A key of the shape Chillarr makes.
fn key_valid(key: &str) -> bool {
    key.len() == KEY_BYTES * 2 && key.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A code of the current kind (older versions made 6 digits).
fn pin_valid(pin: &str) -> bool {
    pin.len() == CODE_LEN && pin.bytes().all(|b| CODE_CHARS.contains(&b))
}

/// What the phone typed, as the code is stored: capitals, no spaces or dashes.
fn normalize(pin: &str) -> String {
    pin.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect()
}

/// Equal, taking the same time wherever the first difference is.
pub(crate) fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    std::hint::black_box(a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y))) == 0
}

/// Whether a Host header names this PC: an IP, localhost, or the PC's own
/// name. A web page using DNS rebinding sends its own domain and is turned away.
fn host_ok(host: &str, own_name: &str, tailscale: Option<&str>) -> bool {
    let host = host.trim();
    let port_ok = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    if let Some(rest) = host.strip_prefix('[') {
        // [v6] or [v6]:port
        let Some((inner, after)) = rest.split_once(']') else { return false };
        return (after.is_empty() || after.strip_prefix(':').is_some_and(port_ok)) && inner.parse::<std::net::Ipv6Addr>().is_ok();
    }
    let name = match host.rsplit_once(':') {
        Some((h, p)) if port_ok(p) => h,
        Some(_) => return false,
        None => host,
    };
    if name.is_empty() {
        return false;
    }
    if name.parse::<std::net::Ipv4Addr>().is_ok() {
        return true;
    }
    let name = name.to_ascii_lowercase();
    let own = own_name.trim().to_ascii_lowercase();
    // Tailscale's name for this PC, whole or its first part (MagicDNS answers both).
    let ts = tailscale.map(|t| t.trim().trim_end_matches('.').to_ascii_lowercase()).filter(|t| !t.is_empty());
    // A page can't rebind a tailnet's ts.net name to itself. (Pointing
    // Tailscale Funnel at this port would make the name public: don't.)
    let ts_ok = ts.is_some_and(|t| name == t || t.split('.').next() == Some(name.as_str()));
    name == "localhost" || ts_ok || (!own.is_empty() && (name == own || name == format!("{own}.local")))
}

/// A header's value (case-insensitive name).
/// A name a device gave itself, made safe to show: printable, trimmed, short.
fn clean_name(name: &str) -> String {
    let n: String = name.chars().filter(|c| !c.is_control()).take(NAME_LEN).collect();
    let n = n.trim();
    if n.is_empty() { "TV".to_string() } else { n.to_string() }
}

fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().skip(1).find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim().eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

impl Remote {
    pub fn load(dir: PathBuf) -> Remote {
        let path = dir.join("remote.json");
        let mut saved: Saved = crate::seal::read(&path).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        // A device saved without a key (a test build's) can't prove itself: dropped.
        let before = saved.devices.len();
        saved.devices.retain(|d| key_valid(&d.key) && pin_valid(&d.code));
        // An old 6-digit code (or none) is replaced; phones pair again.
        let replace = !pin_valid(&saved.pin);
        if replace {
            saved.pin = new_pin().unwrap_or_default();
        }
        let r = Remote {
            path,
            saved: Mutex::new(saved),
            running: AtomicBool::new(false),
            port: Mutex::new(None),
            generation: AtomicU64::new(0),
            tries: Mutex::new(HashMap::new()),
            tailscale: Mutex::new(None),
            tailscale_ip: Mutex::new(None),
        };
        if (replace || r.saved.lock().unwrap().devices.len() != before) && r.path.exists() {
            r.store();
        }
        r
    }

    fn store(&self) {
        let text = serde_json::to_string(&*self.saved.lock().unwrap()).unwrap_or_default();
        let _ = crate::seal::write(&self.path, &text);
    }

    /// The addresses a phone can open, once the server is up.
    fn urls(&self) -> Vec<String> {
        let running = self.running.load(Ordering::SeqCst);
        let Some(p) = (*self.port.lock().unwrap()).filter(|_| running) else { return Vec::new() };
        let mut out: Vec<String> = crate::arr::lan_ip().into_iter().collect();
        out.extend(self.tailscale.lock().unwrap().clone());
        out.extend(self.tailscale_ip.lock().unwrap().clone());
        out.dedup();
        out.into_iter().map(|h| format!("http://{h}:{p}")).collect()
    }

    pub fn status(&self) -> Status {
        let s = self.saved.lock().unwrap().clone();
        let running = self.running.load(Ordering::SeqCst);
        let urls = self.urls();
        let devices = s.devices.into_iter().map(|d| DeviceView { id: d.id, name: d.name, code: d.code }).collect();
        Status { on: s.on, running, url: urls.first().cloned(), urls, pin: s.pin, devices }
    }

    /// The device holding `key`, as (its code, its key). A key nobody holds
    /// (or none) makes a new device with a new key: a page can never pick the
    /// key it's known by. `renew` gives that device a new code. Its name is
    /// updated each time, so a renamed TV shows its new name.
    fn device_code(&self, key: Option<&str>, name: &str, renew: bool, at: u64) -> Result<(String, String), String> {
        const FAILED: &str = "Couldn't make a code. Try again.";
        let mut s = self.saved.lock().unwrap();
        let name = clean_name(name);
        let taken: Vec<String> = std::iter::once(s.pin.clone()).chain(s.devices.iter().map(|d| d.code.clone())).collect();
        let fresh = || (0..8).find_map(|_| new_pin().filter(|c| !taken.contains(c)));
        let key = key.filter(|k| key_valid(k));
        if let Some(d) = key.and_then(|k| s.devices.iter_mut().find(|d| ct_eq(d.key.as_bytes(), k.as_bytes()))) {
            if renew || !pin_valid(&d.code) {
                d.code = fresh().ok_or(FAILED)?;
            }
            d.name = name;
            d.seen = at;
            return Ok((d.code.clone(), d.key.clone()));
        }
        if s.devices.len() >= MAX_DEVICES {
            // Only one unseen for a month makes room, so a flood of new ones
            // can't push out the TV the family uses.
            let stale = s.devices.iter().enumerate().filter(|(_, d)| at.saturating_sub(d.seen) > STALE_SECS).min_by_key(|(_, d)| d.seen).map(|(i, _)| i);
            match stale {
                Some(i) => {
                    s.devices.remove(i);
                }
                None => return Err("Too many devices have a code. Forget some in Chillarr on the PC, under Settings > General > Phone remote.".into()),
            }
        }
        let (code, key, id) = (fresh().ok_or(FAILED)?, new_key(KEY_BYTES).ok_or(FAILED)?, new_key(8).ok_or(FAILED)?);
        s.devices.push(Device { id, key: key.clone(), name, code: code.clone(), seen: at });
        Ok((code, key))
    }

    /// The id of the device holding `key`, marking it seen; None for a key
    /// nobody holds. `seen` only moves (and the file is only written) once an
    /// hour: it just has to tell a month-old device apart.
    fn device_by_key(&self, key: &str, at: u64) -> Option<(String, bool)> {
        if !key_valid(key) {
            return None;
        }
        let mut s = self.saved.lock().unwrap();
        let d = s.devices.iter_mut().find(|d| ct_eq(d.key.as_bytes(), key.as_bytes()))?;
        let moved = at.saturating_sub(d.seen) >= 3600;
        if moved {
            d.seen = at;
        }
        Some((d.id.clone(), moved))
    }

    fn pin_ok(&self, ip: IpAddr, pin: &str) -> Option<Target> {
        self.pin_ok_at(ip, pin, now())
    }

    /// Checks a code from `ip` at time `at` (seconds), counting wrong ones.
    /// Every code is compared in full, so the time taken doesn't tell which matched.
    fn pin_ok_at(&self, ip: IpAddr, pin: &str, at: u64) -> Option<Target> {
        let mut tries = self.tries.lock().unwrap();
        if tries.get(&ip).is_some_and(|t| at < t.locked_until) {
            return None;
        }
        let typed = normalize(pin);
        let found = {
            let s = self.saved.lock().unwrap();
            let mut found = None;
            if pin_valid(&s.pin) && ct_eq(typed.as_bytes(), s.pin.as_bytes()) {
                found = Some(Target::Window);
            }
            for d in &s.devices {
                if pin_valid(&d.code) && ct_eq(typed.as_bytes(), d.code.as_bytes()) {
                    found = Some(Target::Device(d.id.clone(), d.name.clone()));
                }
            }
            found
        };
        if found.is_some() {
            tries.remove(&ip);
            return found;
        }
        if !tries.contains_key(&ip) && tries.len() >= MAX_TRACKED {
            // Forget what's no longer locked and quiet for a day, then the stalest.
            tries.retain(|_, t| at < t.locked_until || at.saturating_sub(t.last) < 24 * 3600);
            if tries.len() >= MAX_TRACKED {
                if let Some(old) = tries.iter().min_by_key(|(_, t)| t.last).map(|(k, _)| *k) {
                    tries.remove(&old);
                }
            }
        }
        let t = tries.entry(ip).or_default();
        t.last = at;
        t.failures += 1;
        if t.failures >= LOCK_AFTER {
            t.failures = 0;
            t.strikes = t.strikes.saturating_add(1);
            let secs = LOCK_SECS.saturating_mul(1u64 << (t.strikes - 1).min(20)).min(LOCK_MAX);
            t.locked_until = at + secs;
        }
        None
    }
}

/// Turns the remote on or off (and remembers it). A new code disconnects phones.
pub fn set(app: &AppHandle, on: bool, new_code: bool) -> Status {
    let r = app.state::<Remote>();
    {
        let mut s = r.saved.lock().unwrap();
        s.on = on;
        if new_code {
            if let Some(p) = new_pin() {
                s.pin = p;
            }
        }
    }
    r.store();
    r.generation.fetch_add(1, Ordering::SeqCst);
    if on {
        start(app.clone());
    }
    // Give a new server a moment to bind before reporting the address.
    std::thread::sleep(std::time::Duration::from_millis(150));
    r.status()
}

/// The code for the device holding `key` (a bridge command, for any signed-in
/// page). With the remote off nothing is made: the page says it's off.
pub fn for_device(app: &AppHandle, key: Option<&str>, name: &str, renew: bool) -> Result<DeviceStatus, String> {
    let r = app.state::<Remote>();
    if !r.saved.lock().unwrap().on {
        return Ok(DeviceStatus { on: false, urls: Vec::new(), code: String::new(), key: key.unwrap_or_default().to_string() });
    }
    let (code, key) = r.device_code(key, name, renew, now())?;
    r.store();
    Ok(DeviceStatus { on: true, urls: r.urls(), code, key })
}

/// The device an event stream belongs to, by the key its page sent; None
/// for no key or one nobody holds (that stream hears no remote).
pub fn stream_device(app: &AppHandle, key: &str) -> Option<String> {
    let r = app.state::<Remote>();
    let (id, moved) = r.device_by_key(key, now())?;
    if moved {
        r.store();
    }
    Some(id)
}

/// Forgets a device: its code stops working until it asks for one again.
pub fn forget(app: &AppHandle, id: &str) -> Status {
    let r = app.state::<Remote>();
    r.saved.lock().unwrap().devices.retain(|d| d.id != id);
    r.store();
    r.status()
}

/// Looks up this PC's Tailscale name and IP again (a process call: off the async threads).
async fn refresh_tailscale(app: &AppHandle) {
    let got = tokio::task::spawn_blocking(crate::media::tailscale_self).await.ok().flatten();
    let r = app.state::<Remote>();
    let (name, ip) = got.unwrap_or((None, None));
    *r.tailscale.lock().unwrap() = name;
    *r.tailscale_ip.lock().unwrap() = ip;
}

/// Starts the server when the remote is on (at launch, or when switched on).
pub fn start(app: AppHandle) {
    let r = app.state::<Remote>();
    if !r.saved.lock().unwrap().on {
        return;
    }
    let my_gen = r.generation.load(Ordering::SeqCst);
    tauri::async_runtime::spawn(async move {
        // The previous server notices the new generation and lets go of the port.
        for _ in 0..20 {
            if !app.state::<Remote>().running.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let mut listener = None;
        for p in PORTS {
            if let Ok(l) = TcpListener::bind(("0.0.0.0", p)).await {
                listener = Some((l, p));
                break;
            }
        }
        let Some((listener, port)) = listener else {
            crate::log::log(&app, crate::model::Level::Red, "The phone remote couldn't open a port (8686-8688 are taken).");
            return;
        };
        let r = app.state::<Remote>();
        *r.port.lock().unwrap() = Some(port);
        r.running.store(true, Ordering::SeqCst);
        refresh_tailscale(&app).await;
        let mut looked = std::time::Instant::now();
        let slots = Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS));
        loop {
            let accepted = tokio::time::timeout(std::time::Duration::from_millis(500), listener.accept()).await;
            if r.generation.load(Ordering::SeqCst) != my_gen {
                break;
            }
            if looked.elapsed() > TAILSCALE_EVERY {
                // Tailscale connected or dropped since: its name is let in, or no longer.
                let h = app.clone();
                tauri::async_runtime::spawn(async move { refresh_tailscale(&h).await });
                looked = std::time::Instant::now();
            }
            if let Ok(Ok((sock, peer))) = accepted {
                // Full: the connection is dropped rather than queued.
                let Ok(slot) = slots.clone().try_acquire_owned() else { continue };
                let h = app.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = serve(h, sock, peer.ip()).await;
                    drop(slot);
                });
            }
        }
        r.running.store(false, Ordering::SeqCst);
    });
}

/// Reads one request: its head, then as much body as Content-Length says
/// (small, capped). None when the phone hung up or it was too big (answered).
async fn read_request(sock: &mut tokio::net::TcpStream) -> std::io::Result<Option<(String, Vec<u8>)>> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    let (head_end, len) = loop {
        let n = sock.read(&mut chunk).await?;
        if n == 0 {
            return Ok(None);
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > MAX_REQUEST {
            reply(sock, "413 Payload Too Large", "text/plain", b"too big").await?;
            return Ok(None);
        }
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..i]).to_string();
            let len = header(&head, "content-length").and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
            break (i + 4, len.min(MAX_REQUEST));
        }
    };
    while buf.len() < head_end + len {
        let n = sock.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let body = buf[head_end..buf.len().min(head_end + len)].to_vec();
    Ok(Some((head, body)))
}

async fn serve(app: AppHandle, mut sock: tokio::net::TcpStream, ip: IpAddr) -> std::io::Result<()> {
    // One deadline for the whole request, so a slow sender can't hold a slot.
    let Ok(read) = tokio::time::timeout(REQUEST_WAIT, read_request(&mut sock)).await else { return Ok(()) };
    let Some((head, body)) = read? else { return Ok(()) };
    let body = body.as_slice();
    let own_name = std::env::var("COMPUTERNAME").unwrap_or_default();
    let tailscale = app.state::<Remote>().tailscale.lock().unwrap().clone();
    if !header(&head, "host").is_some_and(|h| host_ok(h, &own_name, tailscale.as_deref())) {
        return reply(&mut sock, "403 Forbidden", "text/plain", b"open the remote by this PC's address").await;
    }
    let mut first = head.lines().next().unwrap_or_default().split_whitespace();
    let (method, path) = (first.next().unwrap_or_default(), first.next().unwrap_or_default());
    let pin = header(&head, "x-pin").unwrap_or_default().to_string();
    let r = app.state::<Remote>();
    match (method, path) {
        ("GET", "/") | ("GET", "/index.html") => reply(&mut sock, "200 OK", "text/html; charset=utf-8", PAGE.as_bytes()).await,
        ("POST", "/check") => match r.pin_ok(ip, &pin) {
            // Which screen this code drives, for the phone to show.
            Some(t) => {
                let (to, online) = match &t {
                    Target::Window => ("Chillarr on the PC".to_string(), true),
                    Target::Device(id, name) => (name.clone(), crate::bridge::device_online(id)),
                };
                let json = serde_json::to_vec(&serde_json::json!({ "to": to, "online": online })).unwrap_or_default();
                reply(&mut sock, "200 OK", "application/json", &json).await
            }
            None => reply(&mut sock, "401 Unauthorized", "text/plain", b"wrong code").await,
        },
        ("POST", "/search") => {
            if r.pin_ok(ip, &pin).is_none() {
                return reply(&mut sock, "401 Unauthorized", "text/plain", b"wrong code").await;
            }
            let v: Value = serde_json::from_slice(body).unwrap_or_default();
            let term: String = v["q"].as_str().unwrap_or_default().chars().take(80).collect();
            let found: Vec<Value> = crate::media::search(&app, &term)
                .await
                .tiles
                .into_iter()
                .take(24)
                .map(|t| serde_json::json!({
                    "tile": t,
                    "title": t.title,
                    "year": t.year,
                    "kind": t.kind,
                    "poster": t.poster,
                    "status": if t.jf_id.is_some() { Some("In your library".to_string()) } else { t.status.clone() },
                }))
                .collect();
            let json = serde_json::to_vec(&found).unwrap_or_default();
            reply(&mut sock, "200 OK", "application/json", &json).await
        }
        ("POST", "/get") => {
            if r.pin_ok(ip, &pin).is_none() {
                return reply(&mut sock, "401 Unauthorized", "text/plain", b"wrong code").await;
            }
            let v: Value = serde_json::from_slice(body).unwrap_or_default();
            let Ok(tile) = serde_json::from_value::<crate::tiles::Tile>(v["tile"].clone()) else {
                return reply(&mut sock, "400 Bad Request", "text/plain", b"bad title").await;
            };
            let msg = crate::discover_shelf::get_for_phone(&app, tile).await;
            let json = serde_json::to_vec(&match msg { Ok(m) => serde_json::json!({ "ok": true, "msg": m }), Err(e) => serde_json::json!({ "ok": false, "msg": e }) }).unwrap_or_default();
            reply(&mut sock, "200 OK", "application/json", &json).await
        }
        ("POST", "/cmd") => {
            let Some(target) = r.pin_ok(ip, &pin) else {
                return reply(&mut sock, "401 Unauthorized", "text/plain", b"wrong code").await;
            };
            let v: Value = serde_json::from_slice(body).unwrap_or_default();
            let action = v["action"].as_str().unwrap_or_default();
            if !ACTIONS.contains(&action) {
                return reply(&mut sock, "204 No Content", "text/plain", b"").await;
            }
            let text: String = v["text"].as_str().unwrap_or_default().chars().take(80).collect();
            let payload = serde_json::json!({ "action": action, "text": text });
            match target {
                Target::Window => {
                    let _ = app.emit("remote", payload);
                    if let Some(w) = app.get_webview_window("main") {
                        // Typing on the phone brings Chillarr up if it sits in the tray.
                        if action == "type" || action == "search" {
                            let _ = w.show();
                        }
                    }
                }
                Target::Device(id, _) => {
                    // Nobody to hear it: the phone says so instead of pressing into nothing.
                    if !crate::bridge::device_online(&id) {
                        return reply(&mut sock, "409 Conflict", "text/plain", b"not showing Chillarr").await;
                    }
                    crate::bridge::emit_device(&id, "remote", &payload);
                }
            }
            reply(&mut sock, "204 No Content", "text/plain", b"").await
        }
        _ => reply(&mut sock, "404 Not Found", "text/plain", b"not here").await,
    }
}

async fn reply(sock: &mut tokio::net::TcpStream, status: &str, kind: &str, body: &[u8]) -> std::io::Result<()> {
    let head = format!("HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n", body.len());
    sock.write_all(head.as_bytes()).await?;
    sock.write_all(body).await?;
    sock.shutdown().await
}

const PAGE: &str = include_str!("remote.html");


#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn remote() -> Remote {
        Remote::load(std::env::temp_dir().join("chillarr-remote-test-nonexistent"))
    }

    #[test]
    fn codes_are_long_and_unambiguous() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..50 {
            let p = new_pin().unwrap();
            assert_eq!(p.len(), CODE_LEN);
            assert!(pin_valid(&p), "{p}");
            assert!(!p.contains(['0', '1', 'O', 'I', 'L']), "{p}");
            seen.insert(p);
        }
        assert_eq!(seen.len(), 50, "codes don't repeat");
        assert!(!pin_valid("123456"), "an old 6-digit code is replaced");
        assert!(pin_valid(&remote().saved.lock().unwrap().pin));
    }

    #[test]
    fn a_typed_code_may_have_spaces_and_small_letters() {
        let r = remote();
        let good = r.saved.lock().unwrap().pin.clone();
        let typed = format!("{} {}", good[..5].to_lowercase(), &good[5..]);
        assert_eq!(r.pin_ok_at(ip("192.168.1.5"), &typed, 1000), Some(Target::Window));
        assert_eq!(r.pin_ok_at(ip("192.168.1.5"), "", 1000), None);
    }

    #[test]
    fn wrong_codes_lock_out_only_their_sender_and_for_longer_each_time() {
        let r = remote();
        let good = r.saved.lock().unwrap().pin.clone();
        let (bad, phone) = (ip("192.168.1.66"), ip("192.168.1.5"));
        let t = 1_000_000;
        for _ in 0..LOCK_AFTER {
            assert!(r.pin_ok_at(bad, "WRONGCODE2", t).is_none());
        }
        assert!(r.pin_ok_at(bad, &good, t + 1).is_none(), "locked after too many wrong codes");
        assert!(r.pin_ok_at(phone, &good, t + 1).is_some(), "the real phone still gets in");
        assert!(r.pin_ok_at(bad, &good, t + LOCK_SECS - 1).is_none());
        // After the first lockout another round locks for twice as long.
        let t2 = t + LOCK_SECS;
        for _ in 0..LOCK_AFTER {
            assert!(r.pin_ok_at(bad, "WRONGCODE2", t2).is_none());
        }
        assert!(r.pin_ok_at(bad, &good, t2 + LOCK_SECS + 1).is_none());
        assert!(r.pin_ok_at(bad, &good, t2 + 2 * LOCK_SECS).is_some());
        // A right code clears the record.
        assert!(!r.tries.lock().unwrap().contains_key(&bad));
    }

    #[test]
    fn the_lockout_record_stays_bounded() {
        let r = remote();
        for i in 0..(MAX_TRACKED as u32 + 50) {
            r.pin_ok_at(IpAddr::V4(std::net::Ipv4Addr::from(0x0a00_0000 + i)), "x", 10 + i as u64);
        }
        assert!(r.tries.lock().unwrap().len() <= MAX_TRACKED);
    }

    #[test]
    fn only_requests_naming_this_pc_are_served() {
        for h in ["192.168.1.20:8686", "192.168.1.20", "127.0.0.1:8686", "localhost:8686", "LOCALHOST", "[::1]:8686", "[fe80::1]", "desktop-ab12:8686", "Desktop-AB12.local"] {
            assert!(host_ok(h, "DESKTOP-AB12", None), "{h}");
        }
        for h in ["", "evil.example:8686", "desktop-ab12.evil.example", "192.168.1.20.nip.io", "[::1", "[::1]x", "localhost:", "localhost:80a", "evil.example"] {
            assert!(!host_ok(h, "DESKTOP-AB12", None), "{h}");
        }
        assert!(!host_ok("anything", "", None));
    }

    #[test]
    fn this_pcs_tailscale_name_is_let_in() {
        let ts = Some("my-pc.tail1234.ts.net.");
        for h in ["my-pc.tail1234.ts.net:8686", "MY-PC.tail1234.ts.net", "my-pc:8686", "100.101.102.103:8686"] {
            assert!(host_ok(h, "DESKTOP-AB12", ts), "{h}");
        }
        for h in ["other-pc.tail1234.ts.net:8686", "my-pc.tail1234.ts.net.evil.example", "tail1234.ts.net", "my-pc.evil.example"] {
            assert!(!host_ok(h, "DESKTOP-AB12", ts), "{h}");
        }
        assert!(!host_ok("my-pc.tail1234.ts.net", "DESKTOP-AB12", None), "only while Tailscale says that's this PC");
    }

    #[test]
    fn each_device_has_its_own_code_that_stays_until_renewed() {
        let r = remote();
        let window = r.saved.lock().unwrap().pin.clone();
        let (tv, tv_key) = r.device_code(None, "Living room TV", false, 10).unwrap();
        let (phone, phone_key) = r.device_code(None, "Phone", false, 11).unwrap();
        assert!(pin_valid(&tv) && tv != phone && tv != window);
        assert!(key_valid(&tv_key) && tv_key != phone_key);
        assert_eq!(r.device_code(Some(&tv_key), "Living room TV", false, 50).unwrap(), (tv.clone(), tv_key.clone()), "asking again keeps the code");
        let tv_id = r.device_by_key(&tv_key, 55).unwrap().0;
        let at = ip("192.168.1.9");
        assert_eq!(r.pin_ok_at(at, &tv, 60), Some(Target::Device(tv_id.clone(), "Living room TV".into())));
        assert!(matches!(r.pin_ok_at(at, &phone, 60), Some(Target::Device(id, _)) if id != tv_id));
        assert_eq!(r.pin_ok_at(at, &window, 60), Some(Target::Window));
        let (renewed, same_key) = r.device_code(Some(&tv_key), "Bedroom TV", true, 70).unwrap();
        assert!(renewed != tv && same_key == tv_key);
        assert!(r.pin_ok_at(at, &tv, 80).is_none(), "the old code stops working");
        assert_eq!(r.pin_ok_at(at, &renewed, 80), Some(Target::Device(tv_id, "Bedroom TV".into())));
        assert_eq!(r.saved.lock().unwrap().devices.len(), 2);
    }

    #[test]
    fn only_the_device_holding_the_key_gets_its_code() {
        let r = remote();
        let (tv, tv_key) = r.device_code(None, "TV", false, 10).unwrap();
        let tv_id = r.device_by_key(&tv_key, 10).unwrap().0;
        // Knowing the TV's id (it's no secret) or guessing a key gets a new device, never the TV.
        for guess in [tv_id.as_str(), "00000000000000000000000000000000", "not a key", ""] {
            let (code, key) = r.device_code(Some(guess), "Me", true, 20).unwrap();
            assert!(code != tv && key != tv_key && key != guess, "{guess}");
            assert!(r.device_by_key(guess, 20).is_none(), "{guess}");
        }
        assert_eq!(r.pin_ok_at(ip("10.0.0.1"), &tv, 30), Some(Target::Device(tv_id, "TV".into())), "the TV's code wasn't renewed");
        assert!(r.status().devices.iter().all(|d| !serde_json::to_string(d).unwrap().contains(&tv_key)), "the window never lists a key");
    }

    #[test]
    fn devices_stay_bounded_without_pushing_out_one_in_use() {
        let r = remote();
        let (_, tv_key) = r.device_code(None, "TV", false, 0).unwrap();
        for i in 1..MAX_DEVICES {
            r.device_code(None, "x", false, i as u64).unwrap();
        }
        // Full of devices seen this month: a new one is turned away.
        assert!(r.device_code(None, "new", false, 100).is_err());
        assert!(r.device_by_key(&tv_key, 100).is_some(), "the TV is still there");
        // A month on, with the TV still in use, the stalest unused one makes room.
        let later = 100 + STALE_SECS + 10;
        r.device_by_key(&tv_key, later);
        assert!(r.device_code(None, "new", false, later).is_ok());
        let s = r.saved.lock().unwrap();
        assert_eq!(s.devices.len(), MAX_DEVICES);
        drop(s);
        assert!(r.device_by_key(&tv_key, later).is_some(), "the TV in use stays");
    }

    #[test]
    fn a_device_without_a_key_is_dropped_and_the_rest_is_kept() {
        let dir = std::env::temp_dir().join(format!("chillarr-remote-old-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let old = r#"{"on":true,"pin":"ABCDE23456","devices":[{"id":"x","name":"E2E TV","code":"FGHJK23456","made":5}]}"#;
        crate::seal::write(&dir.join("remote.json"), old).unwrap();
        let r = Remote::load(dir.clone());
        let s = r.saved.lock().unwrap().clone();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(s.on && s.pin == "ABCDE23456", "the PC's code and switch survive");
        assert!(s.devices.is_empty());
    }

    #[test]
    fn device_names_are_cleaned() {
        assert_eq!(clean_name("  \u{7}Kitchen\nTV  "), "KitchenTV");
        assert_eq!(clean_name(""), "TV");
        assert_eq!(clean_name(&"a".repeat(100)).len(), NAME_LEN);
    }

    #[test]
    fn codes_compare_whole() {
        assert!(ct_eq(b"ABCDEFGHJK", b"ABCDEFGHJK"));
        assert!(!ct_eq(b"ABCDEFGHJK", b"ABCDEFGHJ2"));
        assert!(!ct_eq(b"ABC", b"ABCD"));
        let head = "POST /check HTTP/1.1\r\nHost: 10.0.0.2:8686\r\nx-PIN:  ABC \r\n";
        assert_eq!(header(head, "X-Pin"), Some("ABC"));
        assert_eq!(header(head, "host"), Some("10.0.0.2:8686"));
        assert_eq!(header(head, "POST /check HTTP/1.1"), None);
    }
}
