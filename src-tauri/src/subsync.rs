//! Puts subtitles back in time with the film, the way ffsubsync/alass do:
//! the film's audio says when people talk, the subtitle says when lines are
//! up, and the shift (and speed, for a file timed to a 25 fps release) that
//! lines them up best wins. A file beside the video is rewritten in place, so
//! Jellyfin, VLC and every TV get the fixed timing; a track inside the video
//! file can't be, so the player shifts that one itself.
//!
//! Measured on Replicas (2018), 2026-10-20: three English .srt files for the
//! same YIFY copy came out as 35.7 s late, right, and 25 fps (x1.0427), each
//! with a z-score of 7.8 or more against 4 or less for any other fit.
use crate::jellyfin::Jf;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::AppHandle;
use tokio::io::AsyncReadExt;

/// One step of the speech track: 10 ms.
const FRAME: f64 = 0.01;
/// Audio is read at 8 kHz, so 80 samples a step.
const RATE: u32 = 8000;
const PER_FRAME: usize = (RATE as f64 * FRAME) as usize;
/// "Louder than the 10 s around it" is what counts as talking. Measured: 1 s
/// windows fooled it on a quiet scene, 10 s didn't on any of the three files.
const LOCAL: usize = 1000;
/// How far a subtitle may be off: 3 minutes either way.
const MAX_SHIFT: f64 = 180.0;
/// Frame-rate mix-ups: a file timed for 25, 24 or 23.976 fps played at another.
const RATIOS: [f64; 7] = [1.0, 25.0 / 23.976, 23.976 / 25.0, 24.0 / 23.976, 23.976 / 24.0, 25.0 / 24.0, 24.0 / 25.0];
/// How clearly the best fit must stand out before anything is changed.
const SURE_Z: f64 = 6.0;
const SURE_LEAD: f64 = 1.5;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;

/// The answer: subtitle time `t` belongs at `t * ratio + offset` in the film.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Fit {
    pub ratio: f64,
    pub offset: f64,
    /// The fit stood out clearly; without that nothing is changed.
    pub sure: bool,
    /// The file beside the video was rewritten with it.
    #[serde(default)]
    pub written: bool,
}

impl Fit {
    const AS_IS: Fit = Fit { ratio: 1.0, offset: 0.0, sure: true, written: false };
    /// Off by enough to see: a quarter of a second, or any speed change.
    /// (A right file reads up to ~0.15 s off: lips and lines don't start together.)
    fn needs_change(&self) -> bool {
        self.sure && (self.ratio != 1.0 || self.offset.abs() >= 0.25)
    }
}

/* ---------- The film's speech ---------- */

/// From 10 ms loudness steps to the running total of "talking" (+1) and
/// "not" (-1), so any stretch is scored with two lookups.
pub fn speech_track(loud: &[f32]) -> Vec<i32> {
    let n = loud.len();
    let mut sum = vec![0f64; n + 1];
    for (i, &e) in loud.iter().enumerate() {
        sum[i + 1] = sum[i] + e as f64;
    }
    let mut out = Vec::with_capacity(n + 1);
    out.push(0i32);
    let mut acc = 0i32;
    for (i, &e) in loud.iter().enumerate() {
        let (a, b) = (i.saturating_sub(LOCAL / 2), (i + LOCAL / 2).min(n));
        let local = (sum[b] - sum[a]) / (b - a) as f64;
        acc += if e as f64 > local { 1 } else { -1 };
        out.push(acc);
    }
    out
}

/// Where Jellyfin's ffmpeg is (it's installed beside Jellyfin), else one on PATH.
fn ffmpeg() -> PathBuf {
    let pf = std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into());
    let jf = Path::new(&pf).join(r"Jellyfin\Server\ffmpeg.exe");
    if jf.is_file() { jf } else { PathBuf::from("ffmpeg.exe") }
}

/// Reads the video's first audio track as speech-band loudness. About 16 s
/// for a 1080p film; nothing is written to disk.
async fn loudness(video: &Path) -> Result<Vec<f32>, String> {
    let mut child = tokio::process::Command::new(ffmpeg())
        .args(["-nostdin", "-v", "error", "-i"])
        .arg(video)
        .args(["-map", "0:a:0", "-vn", "-sn", "-dn", "-ac", "1", "-af", "highpass=f=300,lowpass=f=3400", "-ar", &RATE.to_string(), "-f", "s16le", "-"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS)
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("couldn't start ffmpeg: {e}"))?;
    let mut out = child.stdout.take().ok_or("ffmpeg gave no output")?;
    let mut loud = Vec::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut carry: Vec<u8> = Vec::new();
    let step = PER_FRAME * 2;
    loop {
        let got = out.read(&mut buf).await.map_err(|e| e.to_string())?;
        if got == 0 {
            break;
        }
        carry.extend_from_slice(&buf[..got]);
        let whole = carry.len() / step * step;
        for f in carry[..whole].chunks_exact(step) {
            let sq: f64 = f.chunks_exact(2).map(|s| (i16::from_le_bytes([s[0], s[1]]) as f64).powi(2)).sum();
            loud.push((1.0 + (sq / PER_FRAME as f64).sqrt()).ln() as f32);
        }
        carry.drain(..whole);
    }
    let _ = child.wait().await;
    if loud.len() < 6000 {
        return Err("the video's sound couldn't be read".into());
    }
    Ok(loud)
}

/* ---------- Lining up ---------- */

fn score(cues: &[(f64, f64)], track: &[i32], ratio: f64, offset: f64) -> i64 {
    let n = track.len() as i64 - 1;
    let at = |t: f64| ((t * ratio + offset) / FRAME).round().clamp(0.0, n as f64) as usize;
    cues.iter().map(|&(s, e)| (track[at(e)] - track[at(s)]) as i64).sum()
}

/// The best (ratio, offset) and how far it stands out (z) and what the best
/// other speed reached, so a lucky peak isn't taken for the answer.
pub fn align(cues: &[(f64, f64)], track: &[i32]) -> Fit {
    if cues.len() < 20 || track.len() < 6000 {
        return Fit { sure: false, ..Fit::AS_IS };
    }
    let steps = (MAX_SHIFT / 0.05) as i64;
    let mut fits: Vec<(f64, f64, f64)> = RATIOS
        .iter()
        .map(|&r| {
            let scores: Vec<(f64, i64)> = (-steps..=steps).map(|k| k as f64 * 0.05).map(|o| (o, score(cues, track, r, o))).collect();
            let mean = scores.iter().map(|s| s.1 as f64).sum::<f64>() / scores.len() as f64;
            let sd = (scores.iter().map(|s| (s.1 as f64 - mean).powi(2)).sum::<f64>() / scores.len() as f64).sqrt().max(1.0);
            let best = scores.iter().max_by_key(|s| s.1).unwrap();
            (r, best.0, (best.1 as f64 - mean) / sd)
        })
        .collect();
    fits.sort_by(|a, b| b.2.total_cmp(&a.2));
    let (ratio, coarse, z) = fits[0];
    // A near speed (24 vs 23.976 fps) partly agrees with the right answer and
    // scores well for it; only a fit that puts the film's middle somewhere
    // else is a rival.
    let mid = cues[cues.len() / 2].0;
    let other = fits[1..].iter().filter(|f| (mid * (ratio - f.0) + coarse - f.1).abs() > 3.0).map(|f| f.2).fold(0.1, f64::max);
    // To the nearest 10 ms around the coarse answer.
    let offset = (-5..=5).map(|k| coarse + k as f64 * 0.01).max_by_key(|&o| score(cues, track, ratio, o)).unwrap();
    let offset = (offset * 100.0).round() / 100.0;
    Fit { ratio, offset, sure: z >= SURE_Z && z >= SURE_LEAD * other, written: false }
}

/* ---------- Subtitle files ---------- */

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    /// SRT and WebVTT: `00:01:02,345 --> 00:01:04,000`.
    Arrow,
    /// ASS/SSA: `Dialogue: 0,0:01:02.34,0:01:04.00,...`.
    Ass,
}

fn kind_of(path: &Path) -> Option<Kind> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "srt" | "vtt" => Some(Kind::Arrow),
        "ass" | "ssa" => Some(Kind::Ass),
        _ => None,
    }
}

/// A timestamp at the start of `b`: (seconds, bytes used). Hours optional
/// (WebVTT), milliseconds or centiseconds after `,` or `.`.
fn read_time(b: &[u8]) -> Option<(f64, usize)> {
    let mut parts = [0u64; 3];
    let (mut n, mut i, mut digits) = (0usize, 0usize, 0usize);
    loop {
        match b.get(i) {
            Some(c) if c.is_ascii_digit() => { parts[n] = parts[n] * 10 + (c - b'0') as u64; digits += 1; }
            Some(b':') if digits > 0 && n < 2 => { n += 1; digits = 0; }
            Some(b',') | Some(b'.') if digits == 2 && n >= 1 => break,
            _ => return None,
        }
        i += 1;
    }
    i += 1;
    let (mut frac, mut fd) = (0u64, 0u32);
    while let Some(c) = b.get(i).filter(|c| c.is_ascii_digit()) {
        if fd < 3 { frac = frac * 10 + (c - b'0') as u64; fd += 1; }
        i += 1;
    }
    if fd == 0 {
        return None;
    }
    let (h, m, s) = if n == 2 { (parts[0], parts[1], parts[2]) } else { (0, parts[0], parts[1]) };
    Some(((h * 3600 + m * 60 + s) as f64 + frac as f64 / 10f64.powi(fd as i32), i))
}

fn write_time(t: f64, kind: Kind, comma: bool) -> String {
    let t = t.max(0.0);
    if kind == Kind::Ass {
        let cs = (t * 100.0).round() as u64;
        return format!("{}:{:02}:{:02}.{:02}", cs / 360_000, cs / 6000 % 60, cs / 100 % 60, cs % 100);
    }
    let ms = (t * 1000.0).round() as u64;
    format!("{:02}:{:02}:{:02}{}{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, if comma { ',' } else { '.' }, ms % 1000)
}

/// Every timestamp in a cue line: (start byte, end byte, seconds).
fn times_in(line: &[u8], kind: Kind) -> Vec<(usize, usize, f64)> {
    let mut out = Vec::new();
    match kind {
        Kind::Arrow => {
            if !line.windows(3).any(|w| w == b"-->") {
                return out;
            }
            let mut i = 0;
            while i < line.len() {
                let fresh = i == 0 || !line[i - 1].is_ascii_digit() && line[i - 1] != b':';
                if let Some((t, used)) = fresh.then(|| read_time(&line[i..])).flatten() {
                    out.push((i, i + used, t));
                    i += used;
                } else {
                    i += 1;
                }
            }
            out.truncate(2);
        }
        Kind::Ass => {
            let Some(rest) = line.strip_prefix(b"Dialogue:") else { return out };
            let base = line.len() - rest.len();
            let commas: Vec<usize> = rest.iter().enumerate().filter(|(_, &c)| c == b',').map(|(i, _)| i).take(3).collect();
            if commas.len() < 3 {
                return out;
            }
            for (a, b) in [(commas[0] + 1, commas[1]), (commas[1] + 1, commas[2])] {
                let field = &rest[a..b];
                let pad = field.iter().take_while(|c| c.is_ascii_whitespace()).count();
                if let Some((t, used)) = read_time(&field[pad..]) {
                    out.push((base + a + pad, base + a + pad + used, t));
                }
            }
            if out.len() != 2 {
                out.clear();
            }
        }
    }
    out
}

fn lines(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    bytes.split_inclusive(|&c| c == b'\n')
}

/// Each line's (start, end), in the file's own time.
fn cues_of(bytes: &[u8], kind: Kind) -> Vec<(f64, f64)> {
    lines(bytes).filter_map(|l| match times_in(l, kind).as_slice() { [a, b] => Some((a.2, b.2)), _ => None }).collect()
}

/// The same file with every cue moved by `fit`; everything else byte for byte
/// (encoding, BOM, styling), since only ASCII timestamps are touched.
fn shifted(bytes: &[u8], kind: Kind, fit: Fit) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 64);
    for line in lines(bytes) {
        let mut at = 0;
        for (a, b, t) in times_in(line, kind) {
            out.extend_from_slice(&line[at..a]);
            let comma = line[a..b].contains(&b',');
            out.extend_from_slice(write_time(t * fit.ratio + fit.offset, kind, comma).as_bytes());
            at = b;
        }
        out.extend_from_slice(&line[at..]);
    }
    out
}

/* ---------- Remembering ---------- */

/// The last film's speech track, so trying another subtitle is instant.
static TRACK: tokio::sync::Mutex<Option<(String, Arc<Vec<i32>>)>> = tokio::sync::Mutex::const_new(None);

fn stamp(p: &Path) -> String {
    let m = std::fs::metadata(p).ok();
    let mtime = m.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    format!("{}|{}|{mtime}", p.display(), m.map_or(0, |m| m.len()))
}

async fn track_for(video: &Path) -> Result<Arc<Vec<i32>>, String> {
    let key = stamp(video);
    // Held while ffmpeg runs: a second ask for the same film waits for this one.
    let mut slot = TRACK.lock().await;
    if let Some((k, t)) = slot.as_ref().filter(|(k, _)| *k == key) {
        let _ = k;
        return Ok(t.clone());
    }
    let t = Arc::new(speech_track(&loudness(video).await?));
    *slot = Some((key, t.clone()));
    Ok(t)
}

/// subsync.json: answers already worked out, by video + subtitle stamp.
fn memo_path(app: &AppHandle) -> Option<PathBuf> {
    crate::media::data_dir(app).map(|d| d.join("subsync.json"))
}
fn memo_get(app: &AppHandle, key: &str) -> Option<Fit> {
    let m: BTreeMap<String, Fit> = serde_json::from_str(&std::fs::read_to_string(memo_path(app)?).ok()?).ok()?;
    m.get(key).copied()
}
fn memo_put(app: &AppHandle, key: String, fit: Fit) {
    let Some(p) = memo_path(app) else { return };
    let mut m: BTreeMap<String, Fit> = std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    m.insert(key, fit);
    // Old answers are only ever a re-check away; keep the file small.
    while m.len() > 400 {
        let first = m.keys().next().cloned().unwrap();
        m.remove(&first);
    }
    let _ = std::fs::write(&p, serde_json::to_string(&m).unwrap_or_default());
}

/// A subtitle file beside `video`, fixed in place when it's clearly off.
/// Shared with subfetch.rs, which runs it on every file it saves.
pub async fn fix_bytes(video: &Path, bytes: &[u8], ext: &str) -> (Vec<u8>, Option<Fit>) {
    let Some(kind) = kind_of(Path::new(&format!("x.{ext}"))) else { return (bytes.to_vec(), None) };
    let cues = cues_of(bytes, kind);
    let Ok(track) = track_for(video).await else { return (bytes.to_vec(), None) };
    let fit = align(&cues, &track);
    if fit.needs_change() { (shifted(bytes, kind, fit), Some(fit)) } else { (bytes.to_vec(), Some(fit)) }
}

/// The player's ask: line up Jellyfin track `index` of item `id`. A file
/// beside the video is rewritten (`written`); a track inside the video comes
/// back as a fit for the player to apply. `cues` is what the player shows,
/// used for a track inside the video.
pub async fn sync(app: &AppHandle, j: &Jf, id: &str, index: i64, cues: Vec<(f64, f64)>) -> Result<Fit, String> {
    let uid = j.user_id.as_deref().ok_or("not signed in")?;
    let v = j.get_json(&format!("/Items?userId={uid}&ids={id}&fields=Path,MediaSources")).await.map_err(|e| e.to_string())?;
    let item = &v["Items"][0];
    let video = PathBuf::from(item["Path"].as_str().ok_or("no file for this title")?);
    // Watch now and Discover titles are placeholders; the film isn't all here yet.
    if !video.is_file() || video.extension().is_some_and(|e| e.eq_ignore_ascii_case("strm")) {
        return Err("this video isn't fully on this PC".into());
    }
    let stream = item["MediaSources"][0]["MediaStreams"].as_array().and_then(|s| s.iter().find(|s| s["Index"] == index && s["Type"] == "Subtitle")).cloned().ok_or("no such subtitle track")?;
    let external = stream["IsExternal"].as_bool().unwrap_or(false);
    let file = stream["Path"].as_str().map(PathBuf::from).filter(|p| external && p.is_file() && kind_of(p).is_some());
    let key = format!("{}#{}", stamp(&video), file.as_deref().map_or(format!("stream{index}"), stamp));
    if let Some(f) = memo_get(app, &key) {
        return Ok(Fit { written: false, ..f });
    }
    let track = track_for(&video).await?;
    let Some(file) = file else {
        let fit = align(&cues, &track);
        memo_put(app, key, fit);
        return Ok(fit);
    };
    // Only ever a subtitle file in the video's own folder.
    if file.parent() != video.parent() {
        return Err("the subtitle isn't in the video's folder".into());
    }
    let kind = kind_of(&file).unwrap();
    let bytes = std::fs::read(&file).map_err(|e| e.to_string())?;
    let fit = align(&cues_of(&bytes, kind), &track);
    if !fit.needs_change() {
        memo_put(app, key, fit);
        return Ok(fit);
    }
    std::fs::write(&file, shifted(&bytes, kind, fit)).map_err(|e| format!("couldn't save the fixed subtitles: {e}"))?;
    // The fixed file is right as it is now.
    memo_put(app, format!("{}#{}", stamp(&video), stamp(&file)), Fit::AS_IS);
    Ok(Fit { written: true, ..fit })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A film where people talk exactly during `lines`, with music between.
    fn film(lines: &[(f64, f64)], len: f64) -> Vec<i32> {
        let n = (len / FRAME) as usize;
        // Music and noise underneath, and talk that's only mostly louder,
        // so other speeds and shifts score something too, like in a real film.
        let mut seed = 0x9e37_79b9u64;
        let mut rand = move || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) as f32 / (1u64 << 31) as f32
        };
        let mut loud: Vec<f32> = (0..n).map(|i| 2.0 + ((i as f32) * 0.0013).sin() * 0.8 + rand() * 1.2).collect();
        for &(s, e) in lines {
            for l in &mut loud[(s / FRAME) as usize..((e / FRAME) as usize).min(n)] {
                if rand() < 0.6 {
                    *l += 0.9;
                }
            }
        }
        speech_track(&loud)
    }

    fn dialogue(count: usize) -> Vec<(f64, f64)> {
        // Uneven lines and gaps that never repeat, like real dialogue.
        let mut seed = 0x2545_f491u64;
        let mut rand = move || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) as f64 / (1u64 << 31) as f64
        };
        let mut t = 30.0;
        (0..count)
            .map(|_| {
                let len = 0.8 + rand() * 3.5;
                let line = (t, t + len);
                t += len + 0.3 + rand() * rand() * 25.0;
                line
            })
            .collect()
    }

    /// The real thing, on the PC it was measured on: `cargo test --lib replicas -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn replicas_files() {
        let dir = Path::new(r"C:\Users\anime\Desktop\Desktop\Movies\Normal\Replicas (2018)");
        let name = "Replicas (2018) [BluRay] [1080p] [YTS] [YIFY]";
        let track = speech_track(&loudness(&dir.join(format!("{name}.mp4"))).await.unwrap());
        for (sub, ratio, offset) in [("eng.srt", 1.0, -35.7), ("eng.0.srt", 1.0, 0.0), ("eng.1.srt", 25.0 / 23.976, -0.4)] {
            let bytes = std::fs::read(dir.join(format!("{name}.{sub}"))).unwrap();
            let fit = align(&cues_of(&bytes, Kind::Arrow), &track);
            println!("{sub}: {fit:?}");
            assert!(fit.sure && (fit.ratio - ratio).abs() < 1e-6 && (fit.offset - offset).abs() < 0.3, "{sub}: {fit:?}");
        }
    }

    #[test]
    fn finds_a_late_file() {
        let real = dialogue(300);
        let track = film(&real, real.last().unwrap().1 + 60.0);
        let late: Vec<_> = real.iter().map(|&(s, e)| (s + 35.7, e + 35.7)).collect();
        let fit = align(&late, &track);
        assert!(fit.sure, "{fit:?}");
        assert_eq!(fit.ratio, 1.0);
        assert!((fit.offset + 35.7).abs() < 0.02, "{fit:?}");
    }

    #[test]
    fn finds_a_25fps_file() {
        let real = dialogue(300);
        let track = film(&real, real.last().unwrap().1 + 60.0);
        let pal: Vec<_> = real.iter().map(|&(s, e)| (s * 23.976 / 25.0, e * 23.976 / 25.0)).collect();
        let fit = align(&pal, &track);
        assert!(fit.sure, "{fit:?}");
        assert!((fit.ratio - 25.0 / 23.976).abs() < 1e-9, "{fit:?}");
        assert!(fit.offset.abs() < 0.05, "{fit:?}");
    }

    #[test]
    fn leaves_a_right_file_alone() {
        let real = dialogue(300);
        let track = film(&real, real.last().unwrap().1 + 60.0);
        assert!(!align(&real, &track).needs_change());
    }

    #[test]
    fn unrelated_subtitles_are_not_sure() {
        let real = dialogue(300);
        let track = film(&real, real.last().unwrap().1 + 60.0);
        // Evenly spaced lines that match nothing in this film.
        let other: Vec<_> = (0..300).map(|i| (20.0 + i as f64 * 9.0, 22.5 + i as f64 * 9.0)).collect();
        assert!(!align(&other, &track).needs_change());
    }

    #[test]
    fn srt_moves_and_keeps_everything_else() {
        let srt = "\u{feff}1\r\n00:01:48,920 --> 00:01:50,274\r\n<i>We have touchdown.</i>\r\n\r\n2\r\n00:00:00,500 --> 00:00:01,000\r\nCaf\u{e9} 12:00:00,000 text\r\n".as_bytes();
        let fit = Fit { ratio: 1.0, offset: -35.7, sure: true, written: false };
        let out = String::from_utf8(shifted(srt, Kind::Arrow, fit)).unwrap();
        assert!(out.contains("00:01:13,220 --> 00:01:14,574\r\n<i>We have touchdown.</i>"), "{out}");
        // Before the film starts clamps to 0; text that looks like a time stays.
        assert!(out.contains("00:00:00,000 --> 00:00:00,000\r\nCaf\u{e9} 12:00:00,000 text"), "{out}");
        assert!(out.starts_with('\u{feff}'));
        assert_eq!(cues_of(srt, Kind::Arrow).len(), 2);
    }

    #[test]
    fn vtt_without_hours_and_settings() {
        let vtt = b"WEBVTT\n\n01:02.500 --> 01:04.000 line:10%\nHi\n";
        let fit = Fit { ratio: 1.0, offset: 1.0, sure: true, written: false };
        let out = String::from_utf8(shifted(vtt, Kind::Arrow, fit)).unwrap();
        assert!(out.contains("00:01:03.500 --> 00:01:05.000 line:10%"), "{out}");
    }

    #[test]
    fn ass_moves_only_dialogue_times() {
        let ass = b"[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:01:02.34,0:01:04.00,Default,,0,0,0,,Hi, at 0:00:01.00\n";
        let fit = Fit { ratio: 1.0, offset: 2.5, sure: true, written: false };
        let out = String::from_utf8(shifted(ass, Kind::Ass, fit)).unwrap();
        assert!(out.contains("Dialogue: 0,0:01:04.84,0:01:06.50,Default,,0,0,0,,Hi, at 0:00:01.00"), "{out}");
        assert_eq!(cues_of(ass, Kind::Ass), vec![(62.34, 64.0)]);
    }
}
