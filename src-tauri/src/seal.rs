//! Sign-ins and keys on disk, sealed with Windows' per-user encryption
//! (DPAPI): another Windows account, or the file copied to another PC, can't
//! read them. A file saved before sealing existed is read as it is and sealed
//! the next time it's saved.

use base64::Engine;
use std::path::Path;
use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB};

const PREFIX: &str = "chillarr-sealed:";

fn run(data: &[u8], seal: bool) -> Option<Vec<u8>> {
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB::default();
    // SAFETY: `input` points at `data` for the call; `out` is filled by Windows
    // and freed with LocalFree once copied.
    unsafe {
        let done = if seal {
            CryptProtectData(&input, None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        } else {
            CryptUnprotectData(&input, None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        };
        done.ok()?;
        let bytes = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(out.pbData as _));
        Some(bytes)
    }
}

/// Seals `text` for this Windows user.
pub fn seal(text: &str) -> Option<String> {
    run(text.as_bytes(), true).map(|b| format!("{PREFIX}{}", base64::engine::general_purpose::STANDARD.encode(b)))
}

/// The plain text of what `seal` made, or of a file from before sealing.
pub fn open(stored: &str) -> Option<String> {
    let Some(b64) = stored.trim().strip_prefix(PREFIX) else { return Some(stored.to_string()) };
    let blob = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    String::from_utf8(run(&blob, false)?).ok()
}

pub fn read(path: &Path) -> Option<String> {
    open(&std::fs::read_to_string(path).ok()?)
}

/// How many times `read_raw` reads a file another program is holding.
const READ_TRIES: u32 = 4;

/// A file's text as it is on disk, sealed or not. Ok(None): there is none.
/// A read that fails (an antivirus scan or OneDrive holding the file) is tried
/// again a moment later; Err then means "couldn't read it this time", never
/// "damaged": only text that won't open or parse is that.
pub fn read_raw(path: &Path) -> std::io::Result<Option<String>> {
    let mut i = 0;
    loop {
        match std::fs::read(path) {
            Ok(b) => return Ok(Some(String::from_utf8_lossy(&b).into_owned())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) if i + 1 >= READ_TRIES => return Err(e),
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(250)),
        }
        i += 1;
    }
}

/// Never falls back to writing it in the clear. Written beside the file and
/// then swapped in, so a crash mid-write leaves the old file whole.
pub fn write(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let sealed = seal(text).ok_or_else(|| std::io::Error::other("Windows couldn't seal it"))?;
    let mut name = path.file_name().ok_or_else(|| std::io::Error::other("no file name"))?.to_os_string();
    name.push(".tmp");
    let tmp = path.with_file_name(name);
    let written = std::fs::File::create(&tmp).and_then(|mut f| {
        f.write_all(sealed.as_bytes())?;
        f.sync_all()
    });
    let swapped = written.and_then(|_| std::fs::rename(&tmp, path));
    if swapped.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    swapped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_hides_the_text() {
        let s = seal(r#"{"token":"abc123"}"#).unwrap();
        assert!(s.starts_with(PREFIX) && !s.contains("abc123"));
        assert_eq!(open(&s).unwrap(), r#"{"token":"abc123"}"#);
    }

    #[test]
    fn a_file_from_before_sealing_reads_as_is() {
        assert_eq!(open(r#"{"a":1}"#).unwrap(), r#"{"a":1}"#);
    }

    #[test]
    fn a_write_replaces_the_file_whole_and_leaves_nothing_beside_it() {
        let dir = std::env::temp_dir().join(format!("chillarr-seal-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("x.json");
        write(&f, "first").unwrap();
        write(&f, "second").unwrap();
        let back = read(&f);
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name()).collect();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(back.as_deref(), Some("second"));
        assert_eq!(left.len(), 1, "{left:?}");
    }

    #[test]
    fn a_damaged_seal_reads_as_nothing() {
        assert!(open(&format!("{PREFIX}AAAA")).is_none());
    }
}
