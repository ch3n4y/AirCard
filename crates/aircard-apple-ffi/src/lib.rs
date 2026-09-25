//! Talking to `MobileDevice.framework` directly.
//!
//! The framework is private and lives in the system, so nothing is installed for
//! this to work -- which is the point: the previous implementation shipped small
//! Objective-C helpers built the same way. The symbols are looked up at runtime
//! with `dlopen`/`dlsym` rather than linked at build time, so a missing symbol
//! becomes a clear error instead of a build that cannot even start.
//!
//! Discovery has to ask for the right notification scope. On modern macOS the
//! option dictionary is not optional: without `NotificationOptionEnableUSBMux` a
//! cabled iPhone is simply not reported, and the result is an empty list with no
//! error to explain it.
//!
//! The Mac side of all this lives in `macos`, with the escape that crosses the
//! `Media` boundary in `macos::airtraffic`. `legacy/Sources/device_helper.m` is
//! the specification for both, and says which calls pair up -- for example
//! `AFCConnectionOpen` needs the socket from `AMDServiceConnectionGetSocket`, and
//! the secure IO context has to be handed to AFC or the connection silently
//! refuses to transfer anything.

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::{
    list_devices, AfcSession, AirTraffic, DeviceError, DeviceInfo, LogStream, ZipConduit,
};

// A child module rather than `cfg` on every item, so each platform is one whole
// file and the two `pub use` lines keep the same shape: everything above and
// below reads as one crate, whichever platform it is built for.
#[cfg(not(target_os = "macos"))]
mod elsewhere {
    use serde::Serialize;
    use thiserror::Error;

    #[derive(Debug, Clone, Error)]
    pub enum DeviceError {
        #[error("device access needs a Mac: the escape goes through Apple's AirTraffic framework")]
        Unsupported,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize)]
    pub struct DeviceInfo {
        pub udid: String,
        pub name: String,
        pub product: String,
        pub version: String,
        pub build: String,
        pub language: String,
        pub locale: String,
        pub connection: String,
        pub bold_text: Option<bool>,
    }

    /// Windows can list devices only through Apple's own driver stack; until that
    /// is wired up, say so instead of pretending there is nothing plugged in.
    pub fn list_devices() -> Result<Vec<DeviceInfo>, DeviceError> {
        Err(DeviceError::Unsupported)
    }

    /// The log stream is the os_trace_relay service, reached through the private
    /// MobileDevice framework; away from macOS there is nothing to talk to.
    pub struct LogStream;

    impl LogStream {
        pub fn open(_udid: &str) -> Result<Self, DeviceError> {
            Err(DeviceError::Unsupported)
        }

        pub fn next_line(&mut self) -> Result<Option<String>, DeviceError> {
            Err(DeviceError::Unsupported)
        }
    }

    /// AirTraffic is `AirTrafficHost.framework`, which only exists inside macOS --
    /// and only macOS may open the private framework in the first place.
    pub struct AirTraffic;

    impl AirTraffic {
        pub fn open(_udid: &str) -> Result<Self, DeviceError> {
            Err(DeviceError::Unsupported)
        }

        /// The same signature as the Mac version, so a caller compiles everywhere
        /// and hears "not supported" from the error rather than from a `cfg`.
        pub fn move_assets(
            &self,
            _assets: &[(String, String)],
            _progress: &mut dyn FnMut(usize, usize, &str),
        ) -> Result<(), DeviceError> {
            Err(DeviceError::Unsupported)
        }
    }

    /// The streaming zip conduit is a service of the same private frameworks.
    pub struct ZipConduit;

    impl ZipConduit {
        pub fn open(_udid: &str) -> Result<Self, DeviceError> {
            Err(DeviceError::Unsupported)
        }

        pub fn upload(&self, _media_subdir: &str, _archive: &[u8]) -> Result<(), DeviceError> {
            Err(DeviceError::Unsupported)
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub use elsewhere::{list_devices, AirTraffic, DeviceError, DeviceInfo, LogStream, ZipConduit};
