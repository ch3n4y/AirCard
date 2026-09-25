//! Talking to an iPhone.
//!
//! Three jobs, in the order the app needs them:
//!
//! 1. [`hash`] decides which log lines name a Wallet card. That is the whole
//!    discovery mechanism: a card's files sit outside everything AFC will reach,
//!    so the only announcement that a card exists is iOS logging its path when
//!    Wallet renders it.
//! 2. [`AfcSession`] reaches `Media` -- for the card list, for the sync ledger,
//!    and for everything the app saves on the phone's behalf.
//! 3. [`airlift`] is the escape: the only way to touch a card's own files, which
//!    AFC cannot list, refuses to climb to, and will not let a symlink point at.
//!
//! Nothing here needs anything installed. The frameworks are private and already
//! on the Mac, and they are opened at run time, so a Mac without them gets a
//! clear error instead of a build that cannot start.

pub mod airlift;
pub mod hash;

pub use hash::{hashes_in, is_rejected, is_wallet_line, PLACEHOLDER_HASHES};

/// Device discovery, AFC and the log stream, through Apple's own framework.
///
/// Re-exported rather than wrapped: on everything but a Mac this resolves to a
/// clear "not supported" error, which the window shows as such instead of
/// claiming no phone is plugged in.
pub use aircard_apple_ffi::{list_devices, AfcSession, DeviceError, DeviceInfo, LogStream};

pub use airlift::{
    card_cache_directories, card_directory, Airlift, AirliftError, ArchiveUpload, AssetMover,
    Device, Kind, Leftover, Media, MediaSource, ReadBack,
};
