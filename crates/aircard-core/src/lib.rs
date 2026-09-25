//! Card assets, saved originals and the paths AirCard keeps on this machine.
//!
//! Everything here is pure logic and file IO: no device, no UI. That keeps the
//! rules that took the longest to get right -- what a backup has to contain, what
//! it means for a restore to have worked -- testable without a phone attached.

pub mod assets;
pub mod backup;
pub mod paths;

pub use assets::{BACKED_UP_ASSETS, CACHE_FILES, PDF_ASSET_NAME, PNG_ASSET_NAMES};
pub use backup::{is_complete, BackupStore, SaveError};
pub use paths::{artwork_cache_root, backups_root, home, log_file, slug, unslug};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_roots_live_under_the_user_home() {
        let home = home();
        assert!(backups_root().starts_with(&home));
        assert!(artwork_cache_root().starts_with(&home));
        assert!(log_file().starts_with(&home));
    }
}
