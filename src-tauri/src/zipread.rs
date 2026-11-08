//! Reads single files out of a zip (CBZ chapters, EPUB books) without
//! unpacking the archive: the central directory is read once, then each
//! page is one seek and one inflate. Stored (0) and deflate (8) entries,
//! zip64 sizes and offsets. Nothing is written to disk.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Nothing in a reading archive comes near this; a bigger "page" is refused
/// rather than inflated into memory.
pub const MAX_ENTRY: u64 = 256 * 1024 * 1024;
/// A central directory bigger than this isn't a chapter or a book.
const MAX_DIRECTORY: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub method: u16,
    pub compressed: u64,
    pub size: u64,
    /// Where the entry's local header starts.
    pub header_at: u64,
}

fn u16le(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}
fn u32le(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}
fn u64le(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

/// Every file entry in the archive (folders left out), in directory order.
pub fn entries(path: &Path) -> Result<Vec<Entry>, String> {
    let mut f = File::open(path).map_err(|e| format!("Can't open it: {e}"))?;
    let len = f.metadata().map_err(|e| e.to_string())?.len();
    // The end record sits in the last 22 bytes plus a comment of up to 64 KB.
    let tail_len = len.min(22 + 65_535);
    let mut tail = vec![0u8; tail_len as usize];
    f.seek(SeekFrom::Start(len - tail_len)).map_err(|e| e.to_string())?;
    f.read_exact(&mut tail).map_err(|e| e.to_string())?;
    let eocd = (0..tail.len().saturating_sub(21)).rev().find(|&i| u32le(&tail, i) == 0x0605_4b50).ok_or("Not a zip archive")?;
    let mut count = u16le(&tail, eocd + 10) as u64;
    let mut dir_size = u32le(&tail, eocd + 12) as u64;
    let mut dir_at = u32le(&tail, eocd + 16) as u64;
    // Zip64: the locator just before the end record points at the real one.
    if (count == 0xFFFF || dir_size == 0xFFFF_FFFF || dir_at == 0xFFFF_FFFF) && eocd >= 20 && u32le(&tail, eocd - 20) == 0x0706_4b50 {
        let rec_at = u64le(&tail, eocd - 20 + 8);
        let mut rec = [0u8; 56];
        f.seek(SeekFrom::Start(rec_at)).map_err(|e| e.to_string())?;
        f.read_exact(&mut rec).map_err(|e| e.to_string())?;
        if u32le(&rec, 0) != 0x0606_4b50 {
            return Err("Broken zip64 record".into());
        }
        count = u64le(&rec, 32);
        dir_size = u64le(&rec, 40);
        dir_at = u64le(&rec, 48);
    }
    if dir_size > MAX_DIRECTORY || dir_at.checked_add(dir_size).is_none_or(|end| end > len) {
        return Err("The archive's directory is damaged".into());
    }
    let mut dir = vec![0u8; dir_size as usize];
    f.seek(SeekFrom::Start(dir_at)).map_err(|e| e.to_string())?;
    f.read_exact(&mut dir).map_err(|e| e.to_string())?;
    parse_directory(&dir, count)
}

fn parse_directory(dir: &[u8], count: u64) -> Result<Vec<Entry>, String> {
    let mut out = Vec::with_capacity(count.min(10_000) as usize);
    let mut i = 0usize;
    while i + 46 <= dir.len() && u32le(dir, i) == 0x0201_4b50 {
        let method = u16le(dir, i + 10);
        let mut compressed = u32le(dir, i + 20) as u64;
        let mut size = u32le(dir, i + 24) as u64;
        let name_len = u16le(dir, i + 28) as usize;
        let extra_len = u16le(dir, i + 30) as usize;
        let comment_len = u16le(dir, i + 32) as usize;
        let mut header_at = u32le(dir, i + 42) as u64;
        let end = i + 46 + name_len + extra_len + comment_len;
        if end > dir.len() {
            return Err("The archive's directory is cut short".into());
        }
        let name_bytes = &dir[i + 46..i + 46 + name_len];
        // Bit 11: UTF-8 names. Older tools wrote CP437; those bytes are kept lossily.
        let name = String::from_utf8_lossy(name_bytes).replace('\\', "/");
        // Zip64 extra field: the 0xFFFFFFFF values, in this order.
        let mut x = i + 46 + name_len;
        let extra_end = x + extra_len;
        while x + 4 <= extra_end {
            let (id, n) = (u16le(dir, x), u16le(dir, x + 2) as usize);
            if id == 0x0001 {
                let mut p = x + 4;
                for v in [&mut size, &mut compressed, &mut header_at] {
                    if *v == 0xFFFF_FFFF && p + 8 <= x + 4 + n {
                        *v = u64le(dir, p);
                        p += 8;
                    }
                }
            }
            x += 4 + n;
        }
        if !name.ends_with('/') {
            out.push(Entry { name, method, compressed, size, header_at });
        }
        i = end;
    }
    Ok(out)
}

/// One entry's bytes, inflated.
pub fn read(path: &Path, e: &Entry) -> Result<Vec<u8>, String> {
    if e.size > MAX_ENTRY || e.compressed > MAX_ENTRY {
        return Err("That file is too big to show".into());
    }
    let mut f = File::open(path).map_err(|e| format!("Can't open it: {e}"))?;
    let mut head = [0u8; 30];
    f.seek(SeekFrom::Start(e.header_at)).map_err(|e| e.to_string())?;
    f.read_exact(&mut head).map_err(|e| e.to_string())?;
    if u32le(&head, 0) != 0x0403_4b50 {
        return Err("The archive is damaged".into());
    }
    // The local header's own name/extra lengths can differ from the directory's.
    let skip = u16le(&head, 26) as i64 + u16le(&head, 28) as i64;
    f.seek(SeekFrom::Current(skip)).map_err(|e| e.to_string())?;
    let mut raw = vec![0u8; e.compressed as usize];
    f.read_exact(&mut raw).map_err(|e| e.to_string())?;
    match e.method {
        0 => Ok(raw),
        8 => {
            let mut out = Vec::with_capacity(e.size as usize);
            flate2::read::DeflateDecoder::new(&raw[..]).take(MAX_ENTRY).read_to_end(&mut out).map_err(|e| format!("Couldn't unpack it: {e}"))?;
            Ok(out)
        }
        m => Err(format!("Unsupported packing ({m})")),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;

    /// A small zip with one stored and one deflated entry, built by hand.
    pub(crate) fn make_zip(files: &[(&str, &[u8], bool)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data, deflate) in files {
            let body = if *deflate {
                let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                enc.write_all(data).unwrap();
                enc.finish().unwrap()
            } else {
                data.to_vec()
            };
            let method: u16 = if *deflate { 8 } else { 0 };
            let at = out.len() as u32;
            out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            out.extend_from_slice(&[20, 0, 0, 0]);
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&[0; 8]); // time, date, crc
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&3u16.to_le_bytes()); // a local extra the directory doesn't have
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&[0, 0, 0]);
            out.extend_from_slice(&body);
            central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            central.extend_from_slice(&[20, 0, 20, 0, 0, 0]);
            central.extend_from_slice(&method.to_le_bytes());
            central.extend_from_slice(&[0; 8]);
            central.extend_from_slice(&(body.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&[0; 12]);
            central.extend_from_slice(&at.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let dir_at = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&dir_at.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out
    }

    #[test]
    fn reads_stored_and_deflated_entries() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("c.cbz");
        let big: Vec<u8> = (0..50_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&p, make_zip(&[("001.jpg", b"stored bytes", false), ("sub\\002.png", &big, true)])).unwrap();
        let es = entries(&p).unwrap();
        assert_eq!(es.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), vec!["001.jpg", "sub/002.png"]);
        assert_eq!(read(&p, &es[0]).unwrap(), b"stored bytes");
        assert_eq!(read(&p, &es[1]).unwrap(), big);
    }

    #[test]
    fn junk_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.cbz");
        std::fs::write(&p, b"definitely not a zip file at all, just text").unwrap();
        assert!(entries(&p).is_err());
    }
}

#[cfg(test)]
mod real {
    /// `CHILLARR_REAL_CBZ=<a chapter> cargo test --lib real_cbz -- --ignored`:
    /// every page must match Python's zipfile byte count and parse as an image.
    #[test]
    #[ignore]
    fn real_cbz() {
        let p = std::path::PathBuf::from(std::env::var("CHILLARR_REAL_CBZ").unwrap());
        let es = super::entries(&p).unwrap();
        let names: Vec<String> = es.iter().map(|e| e.name.clone()).collect();
        let order = crate::read_core::page_order(&names);
        let t = std::time::Instant::now();
        for i in &order {
            let b = super::read(&p, &es[*i]).unwrap();
            assert_eq!(b.len() as u64, es[*i].size);
            println!("{} {} {:?}", es[*i].name, b.len(), crate::read_core::dims(&b));
        }
        println!("{} pages in {:?}", order.len(), t.elapsed());
    }
}
