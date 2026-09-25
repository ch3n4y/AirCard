//! Where AirCard keeps things on this machine.
//!
//! Card hashes are base64-ish and contain `/`, `+` and `=`, none of which survives
//! as a folder name, so every hash is percent-encoded. The encoding matches what
//! the ported implementation did (Python's `quote(value, safe="")`) and was
//! checked against a real iPhone: `2Do5+0cj+vG1zMfmFbPt0D4GPKQ=` is stored as
//! `2Do5%2B0cj%2BvG1zMfmFbPt0D4GPKQ%3D`.

use std::path::{Path, PathBuf};

/// Percent-encode everything that is not unreserved.
pub fn slug(value: &str) -> String {
    const UNRESERVED: &[u8] = b"-_.~";
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || UNRESERVED.contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The inverse of [`slug`]. Malformed escapes are left alone rather than dropped,
/// so a hand-made directory can still be read back.
pub fn unslug(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3])
                .ok()
                .and_then(|pair| u8::from_str_radix(pair, 16).ok());
            if let Some(byte) = hex {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The user's home directory, on either platform.
pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Saved originals of card artwork.
pub fn backups_root() -> PathBuf {
    home().join(".aircard_backups")
}

/// Pictures read off a phone, so the app can show one without asking again.
pub fn artwork_cache_root() -> PathBuf {
    home().join(".aircard_cache")
}

/// Where the app's own log lines are appended, next to the platform's other logs.
pub fn log_file() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        home().join("Library/Logs/AirCard.log")
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(home)
            .join("AirCard")
            .join("AirCard.log")
    }
}

/// The list of cards the window knows about.
///
/// A plain JSON array of card numbers, which is the file the previous
/// implementation kept, so a list built up over months is read as it stands.
pub fn cards_file() -> PathBuf {
    home().join(".aircard_cards.json")
}

/// Which cards the most recent scan of each phone saw.
///
/// Kept apart from the card list: the list is what a person chose to keep, and
/// this is only what a scan happened to see, which changes on every scan and is
/// wrong the moment a card is moved on the phone.
pub fn scan_record_file() -> PathBuf {
    home().join(".aircard_scan_seen.json")
}

/// Is there anything at `path` worth reading?
pub fn file_non_empty(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.len() > 0)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_card_hashes_become_the_names_seen_on_the_device() {
        // Both of these were checked against the folder names on a real iPhone.
        assert_eq!(
            slug("2Do5+0cj+vG1zMfmFbPt0D4GPKQ="),
            "2Do5%2B0cj%2BvG1zMfmFbPt0D4GPKQ%3D"
        );
        assert_eq!(
            slug("ql2MjCQ86Xc6xp7nPee-xDWNSRI="),
            "ql2MjCQ86Xc6xp7nPee-xDWNSRI%3D"
        );
    }

    #[test]
    fn a_slash_cannot_escape_the_directory() {
        assert_eq!(slug("ab/cd+ef=="), "ab%2Fcd%2Bef%3D%3D");
    }

    #[test]
    fn slugs_round_trip() {
        for hash in [
            "2Do5+0cj+vG1zMfmFbPt0D4GPKQ=",
            "ql2MjCQ86Xc6xp7nPee-xDWNSRI=",
            "ab/cd+ef==",
            "plain-name_1",
        ] {
            assert_eq!(unslug(&slug(hash)), hash);
            assert!(!slug(hash).contains('/'));
        }
    }
}
