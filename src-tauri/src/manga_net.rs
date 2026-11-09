//! The manga downloader's way out to the internet, so Limit downloads covers
//! manga too (user, 2026-11-08). Suwayomi has no speed limit of its own, but
//! it can use a SOCKS5 proxy: this is one, on 127.0.0.1 only, with a random
//! username and password per start, passing everything through a shared
//! token bucket. limit.rs sets the rate each round from the same 20% budget
//! qBittorrent shares; off, it's unlimited.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Bytes a second each way; 0 is no limit.
static RATE_DOWN: AtomicU64 = AtomicU64::new(0);
static RATE_UP: AtomicU64 = AtomicU64::new(0);
/// Bytes moved each way since the last `take_moved`.
static MOVED_DOWN: AtomicU64 = AtomicU64::new(0);
static MOVED_UP: AtomicU64 = AtomicU64::new(0);
const CHUNK: usize = 16 * 1024;
/// Connections at once; Suwayomi uses a handful.
const MAX_CONN: u64 = 128;
static OPEN: AtomicU64 = AtomicU64::new(0);

struct Bucket {
    tokens: f64,
    at: Instant,
}

static DOWN: Mutex<Option<Bucket>> = Mutex::new(None);
static UP: Mutex<Option<Bucket>> = Mutex::new(None);

/// (port, user, password) of the running relay.
static RELAY: OnceLock<(u16, String, String)> = OnceLock::new();

pub fn set_rate(down: u64, up: u64) {
    RATE_DOWN.store(down, Ordering::Relaxed);
    RATE_UP.store(up, Ordering::Relaxed);
}

/// Bytes moved (down, up) since the last call.
pub fn take_moved() -> (u64, u64) {
    (MOVED_DOWN.swap(0, Ordering::Relaxed), MOVED_UP.swap(0, Ordering::Relaxed))
}

fn random(n: usize) -> String {
    let mut b = vec![0u8; n];
    getrandom::getrandom(&mut b).expect("random");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The relay, started once: (port, user, password) for Suwayomi's settings.
pub fn ensure() -> Result<(u16, String, String), String> {
    if let Some(r) = RELAY.get() {
        return Ok(r.clone());
    }
    let std_l = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| format!("Couldn't open the manga downloader's connection: {e}"))?;
    std_l.set_nonblocking(true).map_err(|e| e.to_string())?;
    let port = std_l.local_addr().map_err(|e| e.to_string())?.port();
    let creds = (port, random(8), random(16));
    if RELAY.set(creds.clone()).is_err() {
        return Ok(RELAY.get().cloned().unwrap());
    }
    let (user, pass) = (creds.1.clone(), creds.2.clone());
    tauri::async_runtime::spawn(async move {
        let Ok(l) = TcpListener::from_std(std_l) else { return };
        serve(l, user, pass).await;
    });
    Ok(creds)
}

async fn serve(l: TcpListener, user: String, pass: String) {
    loop {
        let Ok((sock, peer)) = l.accept().await else { continue };
        if !peer.ip().is_loopback() || OPEN.load(Ordering::Relaxed) >= MAX_CONN {
            continue;
        }
        let (u, p) = (user.clone(), pass.clone());
        OPEN.fetch_add(1, Ordering::Relaxed);
        tokio::spawn(async move {
            let _ = tokio::time::timeout(Duration::from_secs(3600), client(sock, &u, &p)).await;
            OPEN.fetch_sub(1, Ordering::Relaxed);
        });
    }
}

/// One SOCKS5 connection: user/password, CONNECT only, then the throttled pipe.
async fn client(mut s: TcpStream, user: &str, pass: &str) -> std::io::Result<()> {
    let to = Duration::from_secs(10);
    let mut h = [0u8; 2];
    tokio::time::timeout(to, s.read_exact(&mut h)).await??;
    if h[0] != 5 {
        return Ok(());
    }
    let mut methods = vec![0u8; h[1] as usize];
    s.read_exact(&mut methods).await?;
    if !methods.contains(&2) {
        s.write_all(&[5, 0xFF]).await?;
        return Ok(());
    }
    s.write_all(&[5, 2]).await?;
    // RFC 1929: ver, ulen, user, plen, pass.
    let mut v = [0u8; 2];
    s.read_exact(&mut v).await?;
    let mut u = vec![0u8; v[1] as usize];
    s.read_exact(&mut u).await?;
    let mut pl = [0u8; 1];
    s.read_exact(&mut pl).await?;
    let mut p = vec![0u8; pl[0] as usize];
    s.read_exact(&mut p).await?;
    let ok = crate::remote::ct_eq(&u, user.as_bytes()) & crate::remote::ct_eq(&p, pass.as_bytes());
    s.write_all(&[1, if ok { 0 } else { 1 }]).await?;
    if !ok {
        return Ok(());
    }
    let mut r = [0u8; 4];
    s.read_exact(&mut r).await?;
    if r[0] != 5 || r[1] != 1 {
        s.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
        return Ok(());
    }
    let host = match r[3] {
        1 => {
            let mut a = [0u8; 4];
            s.read_exact(&mut a).await?;
            std::net::Ipv4Addr::from(a).to_string()
        }
        3 => {
            let mut n = [0u8; 1];
            s.read_exact(&mut n).await?;
            let mut d = vec![0u8; n[0] as usize];
            s.read_exact(&mut d).await?;
            String::from_utf8_lossy(&d).into_owned()
        }
        4 => {
            let mut a = [0u8; 16];
            s.read_exact(&mut a).await?;
            format!("[{}]", std::net::Ipv6Addr::from(a))
        }
        _ => return Ok(()),
    };
    let mut port = [0u8; 2];
    s.read_exact(&mut port).await?;
    let port = u16::from_be_bytes(port);
    let remote = match tokio::time::timeout(Duration::from_secs(20), TcpStream::connect(format!("{host}:{port}"))).await {
        Ok(Ok(r)) => r,
        _ => {
            s.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
            return Ok(());
        }
    };
    s.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
    let (mut cr, mut cw) = s.into_split();
    let (mut rr, mut rw) = remote.into_split();
    let up = async {
        let r = pipe(&mut cr, &mut rw, &UP, &RATE_UP, &MOVED_UP).await;
        let _ = rw.shutdown().await;
        r
    };
    let down = async {
        let r = pipe(&mut rr, &mut cw, &DOWN, &RATE_DOWN, &MOVED_DOWN).await;
        let _ = cw.shutdown().await;
        r
    };
    let _ = tokio::join!(up, down);
    Ok(())
}

async fn pipe(
    from: &mut (impl AsyncReadExt + Unpin),
    to: &mut (impl AsyncWriteExt + Unpin),
    bucket: &'static Mutex<Option<Bucket>>,
    rate: &'static AtomicU64,
    moved: &'static AtomicU64,
) -> std::io::Result<()> {
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = from.read(&mut buf).await?;
        if n == 0 {
            return Ok(());
        }
        wait_for(bucket, rate, n as u64).await;
        to.write_all(&buf[..n]).await?;
        moved.fetch_add(n as u64, Ordering::Relaxed);
    }
}

/// Waits until `n` bytes may pass at the current rate (shared by every connection).
async fn wait_for(bucket: &'static Mutex<Option<Bucket>>, rate: &'static AtomicU64, n: u64) {
    loop {
        let r = rate.load(Ordering::Relaxed);
        if r == 0 {
            return;
        }
        let wait = {
            let mut g = bucket.lock().unwrap();
            let b = g.get_or_insert(Bucket { tokens: r as f64, at: Instant::now() });
            let now = Instant::now();
            // At most a quarter second banked, so a burst stays small.
            b.tokens = (b.tokens + now.duration_since(b.at).as_secs_f64() * r as f64).min(r as f64 / 4.0 + n as f64);
            b.at = now;
            if b.tokens >= n as f64 {
                b.tokens -= n as f64;
                None
            } else {
                Some(Duration::from_secs_f64(((n as f64 - b.tokens) / r as f64).min(1.0)))
            }
        };
        match wait {
            None => return,
            Some(d) => tokio::time::sleep(d).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A SOCKS5 client, as Suwayomi would be.
    async fn connect(port: u16, user: &str, pass: &str, to: std::net::SocketAddr) -> std::io::Result<TcpStream> {
        let mut s = TcpStream::connect(("127.0.0.1", port)).await?;
        s.write_all(&[5, 1, 2]).await?;
        let mut r = [0u8; 2];
        s.read_exact(&mut r).await?;
        assert_eq!(r, [5, 2]);
        let mut auth = vec![1, user.len() as u8];
        auth.extend_from_slice(user.as_bytes());
        auth.push(pass.len() as u8);
        auth.extend_from_slice(pass.as_bytes());
        s.write_all(&auth).await?;
        s.read_exact(&mut r).await?;
        if r[1] != 0 {
            return Err(std::io::Error::other("refused"));
        }
        let std::net::SocketAddr::V4(a) = to else { panic!() };
        let mut req = vec![5, 1, 0, 1];
        req.extend_from_slice(&a.ip().octets());
        req.extend_from_slice(&a.port().to_be_bytes());
        s.write_all(&req).await?;
        let mut rep = [0u8; 10];
        s.read_exact(&mut rep).await?;
        assert_eq!(rep[1], 0);
        Ok(s)
    }

    #[tokio::test]
    async fn relays_with_the_password_and_holds_the_rate() {
        // A server that sends 256 KB.
        let srv = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = srv.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut c, _)) = srv.accept().await {
                tokio::spawn(async move {
                    let _ = c.write_all(&vec![7u8; 256 * 1024]).await;
                });
            }
        });
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(serve(l, "u".into(), "secret".into()));
        assert!(connect(port, "u", "wrong", addr).await.is_err(), "a wrong password is refused");
        // No limit: all of it, quickly.
        set_rate(0, 0);
        let mut s = connect(port, "u", "secret", addr).await.unwrap();
        let mut got = Vec::new();
        s.read_to_end(&mut got).await.unwrap();
        assert_eq!(got.len(), 256 * 1024);
        // 256 KB/s: the first quarter second is banked, the rest takes about 0.75 s.
        set_rate(256 * 1024, 0);
        *DOWN.lock().unwrap() = None;
        let t = Instant::now();
        let mut s = connect(port, "u", "secret", addr).await.unwrap();
        let mut got = Vec::new();
        s.read_to_end(&mut got).await.unwrap();
        let took = t.elapsed().as_secs_f64();
        set_rate(0, 0);
        assert_eq!(got.len(), 256 * 1024);
        assert!(took > 0.5 && took < 2.0, "took {took}s");
    }

    /// Holds a relay on 127.0.0.1:4599 (user u, password p) for 100 s and
    /// prints what passes: `cargo test --lib live_relay -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn live_relay() {
        let l = TcpListener::bind("127.0.0.1:4599").await.unwrap();
        tokio::spawn(serve(l, "u".into(), "p".into()));
        for _ in 0..10 {
            tokio::time::sleep(Duration::from_secs(10)).await;
            let (d, u) = take_moved();
            println!("moved down {d} up {u}");
        }
    }
}
