//! Saved originals of card artwork.
//!
//! Two rules here were learned the hard way and are the reason this is a module
//! rather than a few lines of file IO:
//!
//! * a backup holds every file a flash overwrites, or it does not exist. One
//!   transient read failure used to produce a backup that looked complete, could
//!   never be completed, and restored the card only halfway.
//! * a partial capture is thrown away, directory and all. A directory left on
//!   disk would make the next look-up report the card as restorable.

use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::assets::BACKED_UP_ASSETS;
use crate::paths::{file_non_empty, slug, unslug};

#[derive(Debug, Error)]
pub enum SaveError {
    /// Not one file of the set could be left out, so nothing was kept.
    #[error("could not read {missing:?}, so nothing was saved")]
    Incomplete { missing: Vec<String> },
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// A backup directory counts only when the whole set is in it.
pub fn is_complete(dir: &Path) -> bool {
    BACKED_UP_ASSETS.iter().all(|name| file_non_empty(&dir.join(name)))
}

fn discard_dir(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}

/// Saved originals, kept per device: a backup taken from one iPhone says nothing
/// about another.
pub struct BackupStore {
    root: PathBuf,
}

impl BackupStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn dir(&self, udid: &str, card_hash: &str) -> PathBuf {
        self.root.join(slug(udid)).join(slug(card_hash))
    }

    /// True only when the whole original is on disk, so restore is real.
    pub fn contains(&self, udid: &str, card_hash: &str) -> bool {
        let dir = self.dir(udid, card_hash);
        dir.is_dir() && is_complete(&dir)
    }

    /// Cards on this device that can be restored, sorted.
    pub fn list(&self, udid: &str) -> Vec<String> {
        let Ok(entries) = fs::read_dir(self.root.join(slug(udid))) else {
            return Vec::new();
        };
        let mut found: Vec<String> = entries
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_dir())
            .map(|entry| unslug(&entry.file_name().to_string_lossy()))
            .filter(|card| self.contains(udid, card))
            .collect();
        found.sort();
        found.dedup();
        found
    }

    /// Every stored file, ordered by name so callers get a stable sequence.
    pub fn read(&self, udid: &str, card_hash: &str) -> Vec<(String, Vec<u8>)> {
        let Ok(entries) = fs::read_dir(self.dir(udid, card_hash)) else {
            return Vec::new();
        };
        let mut files: Vec<(String, Vec<u8>)> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let path = entry.path();
                if !path.is_file() {
                    return None;
                }
                let name = path.file_name()?.to_string_lossy().into_owned();
                let data = fs::read(&path).ok()?;
                (!data.is_empty()).then_some((name, data))
            })
            .collect();
        files.sort_by(|left, right| left.0.cmp(&right.0));
        files
    }

    /// Devices with at least one saved original, sorted. Lets the app show what
    /// is already protected without a phone attached.
    pub fn devices(&self) -> Vec<String> {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut found: Vec<String> = entries
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_dir())
            .map(|entry| unslug(&entry.file_name().to_string_lossy()))
            .filter(|udid| !self.list(udid).is_empty())
            .collect();
        found.sort();
        found.dedup();
        found
    }

    /// Stores the original artwork, once. A later flash must not overwrite it.
    pub fn save(
        &self,
        udid: &str,
        card_hash: &str,
        assets: &[(String, Vec<u8>)],
    ) -> Result<(), SaveError> {
        let dir = self.dir(udid, card_hash);

        // A file that failed to read is missing, not empty: zero bytes would
        // otherwise pass for artwork.
        let missing: Vec<String> = BACKED_UP_ASSETS
            .iter()
            .filter(|name| {
                !assets
                    .iter()
                    .any(|(candidate, data)| candidate == *name && !data.is_empty())
            })
            .map(|name| (*name).to_owned())
            .collect();

        if !missing.is_empty() {
            discard_dir(&dir);
            return Err(SaveError::Incomplete { missing });
        }

        // Start from an empty directory: files from an earlier partial attempt
        // would otherwise sit alongside the fresh ones.
        discard_dir(&dir);
        let write = || -> std::io::Result<()> {
            fs::create_dir_all(&dir)?;
            for (name, data) in assets {
                fs::write(dir.join(name), data)?;
            }
            Ok(())
        };
        if let Err(error) = write() {
            // Do not leave something that looks like a backup but is not.
            discard_dir(&dir);
            return Err(SaveError::Io(error));
        }
        Ok(())
    }

    /// Drops a saved original, for when the user asks for the row to go.
    pub fn discard(&self, udid: &str, card_hash: &str) {
        discard_dir(&self.dir(udid, card_hash));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UDID: &str = "00008150-000E218A26A0C01C";
    const CARD: &str = "2Do5+0cj+vG1zMfmFbPt0D4GPKQ=";

    fn store() -> (tempfile::TempDir, BackupStore) {
        let tmp = tempfile::tempdir().unwrap();
        let store = BackupStore::new(tmp.path());
        (tmp, store)
    }

    fn full_set() -> Vec<(String, Vec<u8>)> {
        BACKED_UP_ASSETS
            .iter()
            .map(|name| ((*name).to_owned(), format!("original:{name}").into_bytes()))
            .collect()
    }

    #[test]
    fn round_trip() {
        let (_tmp, store) = store();
        assert!(!store.contains(UDID, CARD));

        store.save(UDID, CARD, &full_set()).unwrap();

        assert!(store.contains(UDID, CARD));
        let mut expected = full_set();
        expected.sort_by(|left, right| left.0.cmp(&right.0));
        assert_eq!(store.read(UDID, CARD), expected);
        assert_eq!(store.list(UDID), vec![CARD.to_owned()]);
    }

    #[test]
    fn artwork_missing_one_file_is_refused_and_leaves_nothing_behind() {
        let (_tmp, store) = store();
        let mut partial = full_set();
        partial.pop();

        let error = store.save(UDID, CARD, &partial).unwrap_err();

        assert!(matches!(error, SaveError::Incomplete { .. }));
        assert!(!store.contains(UDID, CARD));
        assert!(!store.dir(UDID, CARD).exists());
        assert!(store.list(UDID).is_empty());
    }

    #[test]
    fn an_empty_file_is_not_a_captured_file() {
        let (_tmp, store) = store();
        let mut assets = full_set();
        assets[0].1.clear();

        assert!(store.save(UDID, CARD, &assets).is_err());
        assert!(!store.dir(UDID, CARD).exists());
    }

    #[test]
    fn a_refused_backup_does_not_leave_an_earlier_one_behind() {
        let (_tmp, store) = store();
        let dir = store.dir(UDID, CARD);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(BACKED_UP_ASSETS[0]), b"half").unwrap();
        assert!(!store.contains(UDID, CARD), "a partial set must not look restorable");

        let mut partial = full_set();
        partial.pop();
        assert!(store.save(UDID, CARD, &partial).is_err());

        assert!(!dir.exists());
    }

    #[test]
    fn an_incomplete_backup_on_disk_is_never_offered() {
        let (_tmp, store) = store();
        let dir = store.dir(UDID, CARD);
        fs::create_dir_all(&dir).unwrap();
        for name in BACKED_UP_ASSETS.iter().take(2) {
            fs::write(dir.join(name), b"original").unwrap();
        }

        assert!(!store.contains(UDID, CARD));
        assert!(store.list(UDID).is_empty());
    }

    #[test]
    fn backups_are_kept_per_device() {
        let (_tmp, store) = store();
        store.save(UDID, CARD, &full_set()).unwrap();

        let other = "00008130-000000000000000A";
        assert!(!store.contains(other, CARD));
        assert!(store.list(other).is_empty());
    }

    #[test]
    fn saving_over_a_partial_directory_leaves_only_the_full_set() {
        let (_tmp, store) = store();
        let dir = store.dir(UDID, CARD);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("stale-file-from-an-older-build"), b"junk").unwrap();

        store.save(UDID, CARD, &full_set()).unwrap();

        let names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), BACKED_UP_ASSETS.len(), "unexpected leftovers: {names:?}");
    }

    #[test]
    fn discarding_removes_the_card_but_not_its_neighbours() {
        let (_tmp, store) = store();
        let other = "ql2MjCQ86Xc6xp7nPee-xDWNSRI=";
        store.save(UDID, CARD, &full_set()).unwrap();
        store.save(UDID, other, &full_set()).unwrap();

        store.discard(UDID, CARD);

        assert!(!store.contains(UDID, CARD));
        assert!(store.contains(UDID, other));
    }

    #[test]
    fn a_device_whose_only_backup_is_partial_is_not_listed() {
        let (_tmp, store) = store();
        store.save(UDID, CARD, &full_set()).unwrap();

        let other = "00008130-000000000000000A";
        let dir = store.dir(other, CARD);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(BACKED_UP_ASSETS[0]), b"half").unwrap();

        assert_eq!(store.devices(), vec![UDID.to_owned()]);
    }
}
