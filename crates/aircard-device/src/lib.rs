//! Talking to an iPhone.
//!
//! Only the part that needs no device is here so far: deciding which log lines
//! name a card. That is the whole discovery mechanism, because a card's files sit
//! outside everything AFC will reach.
//!
//! Still to come, in order:
//!
//! 1. transport: discovery, pairing and AFC, over `MobileDevice.framework` on
//!    macOS so the app keeps needing nothing installed.
//! 2. the escape itself: `AirTrafficHost`'s `ATHostConnection*` on macOS, which is
//!    the only way to move a file across the `Media` boundary -- AFC cannot list
//!    the card directory, refuses `..`, and does not support `MAKE_LINK` at all.
//! 3. staged commands, one per operation, each of which must clean up after itself
//!    even when it is interrupted. The ported implementation leaked staging
//!    directories on the phone when a run was cut short.

pub mod hash;

pub use hash::{hashes_in, is_rejected, is_wallet_line, PLACEHOLDER_HASHES};

/// Device discovery, AFC and the log stream, through Apple's own framework.
///
/// Re-exported rather than wrapped: on everything but a Mac this resolves to a
/// clear "not supported" error, which the window shows as such instead of
/// claiming no phone is plugged in.
pub use aircard_apple_ffi::{list_devices, AfcSession, DeviceError, DeviceInfo, LogStream};
