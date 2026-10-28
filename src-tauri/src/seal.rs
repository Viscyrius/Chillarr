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

/// Never falls back to writing it in the clear.
pub fn write(path: &Path, text: &str) -> std::io::Result<()> {
    let sealed = seal(text).ok_or_else(|| std::io::Error::other("Windows couldn't seal it"))?;
    std::fs::write(path, sealed)
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
    fn a_damaged_seal_reads_as_nothing() {
        assert!(open(&format!("{PREFIX}AAAA")).is_none());
    }
}
