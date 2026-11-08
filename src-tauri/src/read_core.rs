//! The parts of reading that are plain logic: which archive entries are pages
//! and in what order, image sizes from their first bytes, chapter numbers from
//! file names, matching a source's title to AniList's, and judging a source's
//! page quality. No I/O here, so all of it is unit tested.

use std::cmp::Ordering;

const IMAGE_EXT: [&str; 7] = ["jpg", "jpeg", "png", "webp", "gif", "avif", "jxl"];

fn ext(name: &str) -> String {
    name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
}

/// The content type a page is served with.
pub fn mime(name: &str) -> &'static str {
    match ext(name).as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "avif" => "image/avif",
        "jxl" => "image/jxl",
        "xhtml" | "html" | "htm" => "application/xhtml+xml",
        "css" => "text/css",
        "svg" => "image/svg+xml",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

/// A page of a comic archive: an image that isn't a macOS leftover or hidden.
pub fn is_page(name: &str) -> bool {
    let base = name.rsplit('/').next().unwrap_or(name);
    IMAGE_EXT.contains(&ext(name).as_str()) && !name.starts_with("__MACOSX/") && !base.starts_with('.')
}

/// "page 2" before "page 10"; case ignored.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.to_ascii_lowercase(), b.to_ascii_lowercase());
    let (mut x, mut y) = (a.as_bytes(), b.as_bytes());
    loop {
        match (x.first(), y.first()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let n = x.iter().take_while(|c| c.is_ascii_digit()).count();
                let m = y.iter().take_while(|c| c.is_ascii_digit()).count();
                let (p, q) = (std::str::from_utf8(&x[..n]).unwrap(), std::str::from_utf8(&y[..m]).unwrap());
                let (pt, qt) = (p.trim_start_matches('0'), q.trim_start_matches('0'));
                let o = pt.len().cmp(&qt.len()).then_with(|| pt.cmp(qt)).then_with(|| n.cmp(&m));
                if o != Ordering::Equal {
                    return o;
                }
                x = &x[n..];
                y = &y[m..];
            }
            (Some(c), Some(d)) => {
                if c != d {
                    return c.cmp(d);
                }
                x = &x[1..];
                y = &y[1..];
            }
        }
    }
}

/// Indexes of the page entries in reading order.
pub fn page_order(names: &[String]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..names.len()).filter(|&i| is_page(&names[i])).collect();
    idx.sort_by(|&a, &b| natural_cmp(&names[a], &names[b]));
    idx
}

/// Width and height from an image's first bytes (JPEG, PNG, WebP, GIF).
pub fn dims(b: &[u8]) -> Option<(u32, u32)> {
    let be16 = |i: usize| b.get(i..i + 2).map(|s| u16::from_be_bytes([s[0], s[1]]) as u32);
    let le16 = |i: usize| b.get(i..i + 2).map(|s| u16::from_le_bytes([s[0], s[1]]) as u32);
    let le24 = |i: usize| b.get(i..i + 3).map(|s| s[0] as u32 | (s[1] as u32) << 8 | (s[2] as u32) << 16);
    if b.starts_with(b"\x89PNG\r\n\x1a\n") && b.len() >= 24 {
        return Some((u32::from_be_bytes(b[16..20].try_into().ok()?), u32::from_be_bytes(b[20..24].try_into().ok()?)));
    }
    if b.starts_with(b"GIF8") {
        return Some((le16(6)?, le16(8)?));
    }
    if b.len() >= 30 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        return match &b[12..16] {
            b"VP8X" => Some((1 + le24(24)?, 1 + le24(27)?)),
            b"VP8L" => {
                let v = u32::from_le_bytes(b[21..25].try_into().ok()?);
                Some(((v & 0x3FFF) + 1, ((v >> 14) & 0x3FFF) + 1))
            }
            b"VP8 " => Some((le16(26)? & 0x3FFF, le16(28)? & 0x3FFF)),
            _ => None,
        };
    }
    if b.starts_with(&[0xFF, 0xD8]) {
        let mut i = 2;
        while i + 9 < b.len() {
            if b[i] != 0xFF {
                i += 1;
                continue;
            }
            let m = b[i + 1];
            // SOF0-SOF15 except DHT (C4), JPG (C8) and DAC (CC).
            if (0xC0..=0xCF).contains(&m) && ![0xC4, 0xC8, 0xCC].contains(&m) {
                return Some((be16(i + 7)?, be16(i + 5)?));
            }
            if m == 0xD8 || m == 0x01 || (0xD0..=0xD7).contains(&m) {
                i += 2;
                continue;
            }
            i += 2 + be16(i + 2)? as usize;
        }
    }
    None
}

/// What a chapter file's name says: Suwayomi writes `<scanlator>_<chapter name>.cbz`.
#[derive(Debug, PartialEq, Clone)]
pub struct ChapterName {
    pub scanlator: Option<String>,
    pub name: String,
    pub number: Option<f64>,
}

pub fn chapter_name(file_stem: &str) -> ChapterName {
    let (scanlator, name) = match file_stem.split_once('_') {
        Some((s, n)) if !s.is_empty() && !n.is_empty() => (Some(s.to_string()), n.to_string()),
        _ => (None, file_stem.to_string()),
    };
    let number = chapter_number(&name);
    ChapterName { scanlator, name, number }
}

/// The number after "chapter", "ch", "episode", "ep" or "#"; else the last
/// number in the name ("Vol. 2 Ch. 15.5" is 15.5, "Volume 3" is 3).
pub fn chapter_number(name: &str) -> Option<f64> {
    let low = name.to_ascii_lowercase();
    let nums = numbers(&low);
    for key in ["chapter", "episode", "ch.", "ch ", "ep.", "ep ", "#"] {
        if let Some(at) = low.find(key) {
            if let Some((_, n)) = nums.iter().find(|(i, _)| *i >= at + key.len()) {
                return Some(*n);
            }
        }
    }
    nums.last().map(|(_, n)| *n)
}

fn numbers(s: &str) -> Vec<(usize, f64)> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() {
            let start = i;
            while i < b.len() && (b[i].is_ascii_digit() || (b[i] == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit))) {
                i += 1;
            }
            if let Ok(n) = s[start..i].parse::<f64>() {
                out.push((start, n));
            }
        } else {
            i += 1;
        }
    }
    out
}

/// A title reduced for comparing: lower case, letters and digits only,
/// "the"/"a" dropped, roman numerals and "season" words left alone.
pub fn norm_title(t: &str) -> Vec<String> {
    t.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty() && !matches!(*w, "the" | "a" | "an" | "official"))
        .map(String::from)
        .collect()
}

/// How alike two titles are, 0..1: 1 when the words are the same, else the
/// share of words in common (Jaccard). "Solo Leveling" against "Solo
/// Leveling: Ragnarok" is 0.67, so a sequel never passes for the original.
pub fn title_score(a: &str, b: &str) -> f64 {
    let (x, y) = (norm_title(a), norm_title(b));
    if x.is_empty() || y.is_empty() {
        return 0.0;
    }
    if x == y {
        return 1.0;
    }
    let inter = x.iter().filter(|w| y.contains(w)).count() as f64;
    let union = (x.len() + y.len()) as f64 - inter;
    inter / union
}

/// The best score of `found` against any of the names a title goes by.
pub fn best_title_score(found: &str, names: &[String]) -> f64 {
    names.iter().map(|n| title_score(found, n)).fold(0.0, f64::max)
}

/// Titles under this aren't the same series.
pub const SAME_TITLE: f64 = 0.85;

/// One sampled page: width, height, file size.
#[derive(Clone, Copy, Debug)]
pub struct Sample {
    pub w: u32,
    pub h: u32,
    pub bytes: u64,
    pub webp: bool,
}

/// How good a source's pages look, from a few sampled pages. Width is what
/// you see (capped at 2000: wider is no sharper on any screen here), then a
/// copy squeezed so hard it's blocky is pushed down. Measured 2026-11-08 on
/// Solo Leveling: MangaFire 760 px at 0.03 bytes/px (blocky) vs Weeb Central
/// 720 px at 0.22. WebP packs about 1.6x tighter than JPEG at the same look.
pub fn quality(samples: &[Sample]) -> f64 {
    let mut s: Vec<f64> = samples
        .iter()
        .filter(|p| p.w > 0 && p.h > 0)
        .map(|p| {
            let bpp = p.bytes as f64 / (p.w as f64 * p.h as f64) * if p.webp { 1.6 } else { 1.0 };
            let squeeze = if bpp < 0.05 { 0.6 } else if bpp < 0.08 { 0.85 } else { 1.0 };
            p.w.min(2000) as f64 * squeeze
        })
        .collect();
    if s.is_empty() {
        return 0.0;
    }
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    s[s.len() / 2]
}

/// Reading mode a series opens in, from AniList's country of origin.
pub fn default_mode(country: Option<&str>, format: Option<&str>) -> &'static str {
    match (country, format) {
        (_, Some("NOVEL")) => "book",
        (Some("KR") | Some("CN") | Some("TW"), _) => "webtoon",
        _ => "manga",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_sort_naturally_and_skip_junk() {
        let names: Vec<String> = ["10.jpg", "2.jpg", "1.JPG", "ComicInfo.xml", "__MACOSX/._1.jpg", ".thumb.png", "cover/001.webp"].iter().map(|s| s.to_string()).collect();
        let order: Vec<&str> = page_order(&names).into_iter().map(|i| names[i].as_str()).collect();
        assert_eq!(order, vec!["1.JPG", "2.jpg", "10.jpg", "cover/001.webp"]);
        assert_eq!(natural_cmp("p001", "p1"), Ordering::Greater, "same number, longer zero-padding after");
    }

    #[test]
    fn image_sizes_from_headers() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend_from_slice(&800u32.to_be_bytes());
        png.extend_from_slice(&13220u32.to_be_bytes());
        assert_eq!(dims(&png), Some((800, 13220)));
        // JPEG: SOI, an APP0 segment, then SOF0 with height 1116 and width 720.
        let mut jpg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 4, 0, 0, 0xFF, 0xC0, 0, 11, 8];
        jpg.extend_from_slice(&1116u16.to_be_bytes());
        jpg.extend_from_slice(&720u16.to_be_bytes());
        jpg.extend_from_slice(&[3, 0, 0, 0, 0]);
        assert_eq!(dims(&jpg), Some((720, 1116)));
        let mut webp = b"RIFF\0\0\0\0WEBPVP8X".to_vec();
        webp.extend_from_slice(&[0; 8]);
        webp.extend_from_slice(&[0x1F, 0x03, 0x00, 0x9F, 0x0F, 0x00]); // 800 x 4000
        assert_eq!(dims(&webp), Some((800, 4000)));
        assert_eq!(dims(b"nope"), None);
    }

    #[test]
    fn chapter_names_from_suwayomi_files() {
        assert_eq!(chapter_name("Official_Chapter 1"), ChapterName { scanlator: Some("Official".into()), name: "Chapter 1".into(), number: Some(1.0) });
        assert_eq!(chapter_name("Asura_Chapter 12.5 - The Gate").number, Some(12.5));
        assert_eq!(chapter_name("Vol. 2 Ch. 15").number, Some(15.0));
        assert_eq!(chapter_name("Episode 7").number, Some(7.0));
        assert_eq!(chapter_name("Volume 3").number, Some(3.0));
        assert_eq!(chapter_name("Oneshot").number, None);
    }

    #[test]
    fn titles_match_only_the_same_series() {
        let names = vec!["Na Honjaman Level Up".to_string(), "Solo Leveling".to_string()];
        assert!(best_title_score("Solo Leveling", &names) >= SAME_TITLE);
        assert!(best_title_score("solo leveling!", &names) >= SAME_TITLE);
        assert!(best_title_score("Solo Leveling: Ragnarok", &names) < SAME_TITLE, "a sequel isn't the series");
        assert!(best_title_score("Omniscient Reader", &names) < SAME_TITLE);
    }

    #[test]
    fn quality_prefers_sharp_over_squeezed() {
        let weeb = [Sample { w: 720, h: 4000, bytes: 645 * 1024, webp: false }, Sample { w: 720, h: 1116, bytes: 285 * 1024, webp: false }, Sample { w: 720, h: 4000, bytes: 905 * 1024, webp: false }];
        let fire = [Sample { w: 760, h: 972, bytes: 22 * 1024, webp: false }, Sample { w: 760, h: 972, bytes: 25 * 1024, webp: false }, Sample { w: 760, h: 972, bytes: 73 * 1024, webp: false }];
        assert!(quality(&weeb) > quality(&fire), "{} vs {}", quality(&weeb), quality(&fire));
        let wide = [Sample { w: 1600, h: 2400, bytes: 900 * 1024, webp: false }];
        assert!(quality(&wide) > quality(&weeb));
        assert_eq!(quality(&[]), 0.0);
    }

    #[test]
    fn modes_follow_the_country() {
        assert_eq!(default_mode(Some("KR"), Some("MANGA")), "webtoon");
        assert_eq!(default_mode(Some("CN"), None), "webtoon");
        assert_eq!(default_mode(Some("JP"), Some("MANGA")), "manga");
        assert_eq!(default_mode(Some("JP"), Some("NOVEL")), "book");
    }
}
