//! The airlift escape: the payloads, the naming, and the staged commands.
//!
//! A Wallet card's files are outside everything AFC will reach, so replacing a
//! card's artwork means handing the phone a zip whose symlink leads out of
//! `Media` and asking its own sync service to move the files involved. Three
//! layers, each usable without the next:
//!
//! [`payload`]  the zip, the plists and the naming, which is all pure data.
//! [`escape`]   the phases -- snapshot, stage, move, finish -- over three traits.
//! [`device`]   those traits, implemented against a real phone.

pub mod device;
pub mod escape;
pub mod payload;

/// A phone that fits in memory, for testing the phases without a cable.
#[cfg(test)]
mod phone;

pub use device::Device;
pub use escape::{
    card_cache_directories, card_directory, Airlift, AirliftError, ArchiveUpload, AssetMover, Kind,
    Leftover, Media, MediaSource, ReadBack,
};
pub use payload::{
    build_archive, build_archive_multi, build_books, generated_names_match, generated_token,
    is_canary_leaf, is_safe_relative_path, link_name, recovered_name, source_name, token,
    AIRLOCK_ROOT, CANARY_PREFIX, LINK_PREFIX, RECOVERED_PREFIX, SOURCE_PREFIX,
    TRACKED_BOOKS_DIRECTORIES, TRACKED_BOOKS_FILES,
};
