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
//! Not here yet: AFC (reading and writing files on the device) and the AirTraffic
//! escape. `legacy/Sources/device_helper.m` is the specification for both, and
//! says which calls pair up -- for example `AFCConnectionOpen` needs the socket
//! from `AMDServiceConnectionGetSocket`, and the secure IO context has to be handed
//! to AFC or the connection silently refuses to transfer anything.

#[cfg(target_os = "macos")]
mod macos {
    use std::cell::RefCell;
    use std::ffi::{c_char, c_int, c_void, CStr, CString};
    use std::sync::OnceLock;

    use serde::Serialize;
    use thiserror::Error;

    /// Where the private framework lives on every macOS install.
    const FRAMEWORK: &str = "/System/Library/PrivateFrameworks/MobileDevice.framework/MobileDevice";

    /// `kCFStringEncodingUTF8`
    const UTF8: u32 = 0x0800_0100;

    /// `message` value that means "device attached".
    const ATTACHED: u32 = 1;

    type CfType = *const c_void;
    type CfString = *const c_void;
    type CfDictionary = *const c_void;
    type CfAllocator = *const c_void;
    type CfIndex = isize;

    /// The layout the framework passes to a notification callback.
    #[repr(C)]
    struct CallbackInfo {
        device: *const c_void,
        message: u32,
    }

    #[derive(Debug, Error)]
    pub enum DeviceError {
        #[error("could not load {FRAMEWORK}: {detail}")]
        Framework { detail: String },
        #[error("{name} is missing from the framework, so this macOS version cannot be used")]
        MissingSymbol { name: String },
        #[error("device discovery could not start (status {status})")]
        Subscribe { status: c_int },
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
        /// "usb", "network" or "unknown".
        pub connection: String,
        pub bold_text: Option<bool>,
    }

    #[allow(clippy::type_complexity)]
    struct Symbols {
        cf_string_create: unsafe extern "C" fn(CfAllocator, *const c_char, u32) -> CfString,
        cf_string_read: unsafe extern "C" fn(CfString, *mut c_char, CfIndex, u32) -> u8,
        cf_dictionary_create: unsafe extern "C" fn(
            CfAllocator,
            *const CfType,
            *const CfType,
            CfIndex,
            *const c_void,
            *const c_void,
        ) -> CfDictionary,
        cf_release: unsafe extern "C" fn(CfType),
        cf_run_loop_run: unsafe extern "C" fn(CfType, f64, u8) -> c_int,
        k_cf_true: CfType,
        k_cf_false: CfType,
        k_run_loop_default_mode: CfType,
        k_dictionary_key_callbacks: CfType,
        k_dictionary_value_callbacks: CfType,

        subscribe: unsafe extern "C" fn(
            extern "C" fn(*mut CallbackInfo, *mut c_void),
            c_int,
            u32,
            *mut c_void,
            *mut CfType,
            CfDictionary,
        ) -> c_int,
        unsubscribe: unsafe extern "C" fn(CfType) -> c_int,
        device_identifier: unsafe extern "C" fn(*const c_void) -> CfString,
        interface_type: unsafe extern "C" fn(*const c_void) -> c_int,
        copy_value: unsafe extern "C" fn(*const c_void, CfString, CfString) -> CfType,
        connect: unsafe extern "C" fn(*const c_void) -> c_int,
        disconnect: unsafe extern "C" fn(*const c_void) -> c_int,
        is_paired: unsafe extern "C" fn(*const c_void) -> c_int,
        pair: unsafe extern "C" fn(*const c_void) -> c_int,
        validate_pairing: unsafe extern "C" fn(*const c_void) -> c_int,
        start_session: unsafe extern "C" fn(*const c_void) -> c_int,
        stop_session: unsafe extern "C" fn(*const c_void) -> c_int,
    }

    // Written once and never mutated; the CF singletons are process-wide constants,
    // so sharing this table between threads is sound. Whether a given framework
    // call may run concurrently is Apple's business, and iTunes does exactly that.
    unsafe impl Send for Symbols {}
    unsafe impl Sync for Symbols {}

    static SYMBOLS: OnceLock<Result<Symbols, DeviceError>> = OnceLock::new();

    thread_local! {
        /// Filled by the notification callback, which the framework calls from the
        /// run loop on this thread -- so this never needs to be shared.
        static FOUND: RefCell<Vec<DeviceInfo>> = const { RefCell::new(Vec::new()) };
    }

    fn function<T: Copy>(handle: *mut c_void, name: &str) -> Result<T, DeviceError> {
        let symbol = CString::new(name).expect("symbol names have no interior nul");
        let pointer = unsafe { libc::dlsym(handle, symbol.as_ptr()) };
        if pointer.is_null() {
            return Err(DeviceError::MissingSymbol {
                name: name.to_owned(),
            });
        }
        Ok(unsafe { std::mem::transmute_copy::<*mut c_void, T>(&pointer) })
    }

    /// A data symbol whose value is itself a pointer, such as `kCFBooleanTrue`.
    fn data_pointer(handle: *mut c_void, name: &str) -> Result<CfType, DeviceError> {
        let symbol = CString::new(name).expect("symbol names have no interior nul");
        let pointer = unsafe { libc::dlsym(handle, symbol.as_ptr()) };
        if pointer.is_null() {
            return Err(DeviceError::MissingSymbol {
                name: name.to_owned(),
            });
        }
        Ok(unsafe { *(pointer as *const CfType) })
    }

    fn symbols() -> Result<&'static Symbols, DeviceError> {
        SYMBOLS
            .get_or_init(|| {
                let path = CString::new(FRAMEWORK).expect("a fixed path");
                let handle = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_NOW) };
                if handle.is_null() {
                    let detail = unsafe {
                        CStr::from_ptr(libc::dlerror())
                            .to_string_lossy()
                            .into_owned()
                    };
                    return Err(DeviceError::Framework { detail });
                }
                Ok(Symbols {
                    cf_string_create: function(handle, "CFStringCreateWithCString")?,
                    cf_string_read: function(handle, "CFStringGetCString")?,
                    cf_dictionary_create: function(handle, "CFDictionaryCreate")?,
                    cf_release: function(handle, "CFRelease")?,
                    cf_run_loop_run: function(handle, "CFRunLoopRunInMode")?,
                    k_cf_true: data_pointer(handle, "kCFBooleanTrue")?,
                    k_cf_false: data_pointer(handle, "kCFBooleanFalse")?,
                    k_run_loop_default_mode: data_pointer(handle, "kCFRunLoopDefaultMode")?,
                    k_dictionary_key_callbacks: function(handle, "kCFTypeDictionaryKeyCallBacks")?,
                    k_dictionary_value_callbacks: function(
                        handle,
                        "kCFTypeDictionaryValueCallBacks",
                    )?,
                    subscribe: function(handle, "AMDeviceNotificationSubscribeWithOptions")?,
                    unsubscribe: function(handle, "AMDeviceNotificationUnsubscribe")?,
                    device_identifier: function(handle, "AMDeviceCopyDeviceIdentifier")?,
                    interface_type: function(handle, "AMDeviceGetInterfaceType")?,
                    copy_value: function(handle, "AMDeviceCopyValue")?,
                    connect: function(handle, "AMDeviceConnect")?,
                    disconnect: function(handle, "AMDeviceDisconnect")?,
                    is_paired: function(handle, "AMDeviceIsPaired")?,
                    pair: function(handle, "AMDevicePair")?,
                    validate_pairing: function(handle, "AMDeviceValidatePairing")?,
                    start_session: function(handle, "AMDeviceStartSession")?,
                    stop_session: function(handle, "AMDeviceStopSession")?,
                })
            })
            .as_ref()
            .map_err(|error| match error {
                DeviceError::Framework { detail } => DeviceError::Framework {
                    detail: detail.clone(),
                },
                DeviceError::MissingSymbol { name } => {
                    DeviceError::MissingSymbol { name: name.clone() }
                }
                DeviceError::Subscribe { status } => DeviceError::Subscribe { status: *status },
            })
    }

    fn cf_string(symbols: &Symbols, text: &str) -> Option<(CfString, CString)> {
        let owned = CString::new(text).ok()?;
        let reference =
            unsafe { (symbols.cf_string_create)(std::ptr::null(), owned.as_ptr(), UTF8) };
        if reference.is_null() {
            return None;
        }
        Some((reference, owned))
    }

    fn read_cf_string(symbols: &Symbols, reference: CfString) -> String {
        if reference.is_null() {
            return String::new();
        }
        let mut buffer = [0 as c_char; 4096];
        let ok = unsafe {
            (symbols.cf_string_read)(
                reference,
                buffer.as_mut_ptr(),
                buffer.len() as CfIndex,
                UTF8,
            )
        };
        if ok == 0 {
            return String::new();
        }
        unsafe { CStr::from_ptr(buffer.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    }

    /// The notification scope. Several of these are the difference between an
    /// empty list and the phone that is plugged in.
    fn options(symbols: &Symbols) -> Option<CfDictionary> {
        let settings: [(&str, CfType); 5] = [
            (
                "NotificationOptionSearchForPairedDevices",
                symbols.k_cf_true,
            ),
            (
                "NotificationOptionSearchForPairedDevicesViaDirectConnectionsOnly",
                symbols.k_cf_false,
            ),
            (
                "NotificationOptionSearchForWiFiPairableDevices",
                symbols.k_cf_false,
            ),
            ("NotificationOptionEnableRemoteXPC", symbols.k_cf_true),
            ("NotificationOptionEnableUSBMux", symbols.k_cf_true),
        ];

        let mut keys: Vec<CfString> = Vec::with_capacity(settings.len());
        let mut values: Vec<CfType> = Vec::with_capacity(settings.len());
        let mut owned: Vec<CString> = Vec::with_capacity(settings.len());
        for (key, value) in &settings {
            let (reference, text) = cf_string(symbols, key)?;
            keys.push(reference);
            values.push(*value);
            owned.push(text);
        }

        let dictionary = unsafe {
            (symbols.cf_dictionary_create)(
                std::ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                keys.len() as CfIndex,
                symbols.k_dictionary_key_callbacks,
                symbols.k_dictionary_value_callbacks,
            )
        };
        for key in &keys {
            unsafe { (symbols.cf_release)(*key) };
        }
        drop(owned);
        if dictionary.is_null() {
            None
        } else {
            Some(dictionary)
        }
    }

    fn connection_name(kind: c_int) -> String {
        match kind {
            1 => "usb".to_owned(),
            2 => "network".to_owned(),
            _ => "unknown".to_owned(),
        }
    }

    fn value_of(
        symbols: &Symbols,
        device: *const c_void,
        domain: Option<&str>,
        key: &str,
    ) -> String {
        let Some((key_string, _key_owned)) = cf_string(symbols, key) else {
            return String::new();
        };
        let domain_string = domain.and_then(|name| cf_string(symbols, name));
        let value = unsafe {
            (symbols.copy_value)(
                device,
                domain_string
                    .as_ref()
                    .map(|(reference, _)| *reference)
                    .unwrap_or(std::ptr::null()),
                key_string,
            )
        };
        let text = read_cf_string(symbols, value);
        if !value.is_null() {
            unsafe { (symbols.cf_release)(value) };
        }
        unsafe { (symbols.cf_release)(key_string) };
        if let Some((reference, _)) = domain_string {
            unsafe { (symbols.cf_release)(reference) };
        }
        text
    }

    fn boolean_of(
        symbols: &Symbols,
        device: *const c_void,
        domain: &str,
        key: &str,
    ) -> Option<bool> {
        let (key_string, _owned) = cf_string(symbols, key)?;
        let (domain_string, _domain_owned) = cf_string(symbols, domain)?;
        let value = unsafe { (symbols.copy_value)(device, domain_string, key_string) };
        // Read it before releasing it: the singletons compare by identity.
        let result = if value.is_null() {
            None
        } else {
            Some(value == symbols.k_cf_true)
        };
        if !value.is_null() {
            unsafe { (symbols.cf_release)(value) };
        }
        unsafe { (symbols.cf_release)(key_string) };
        unsafe { (symbols.cf_release)(domain_string) };
        result
    }

    fn describe(symbols: &Symbols, device: *const c_void) -> Option<DeviceInfo> {
        let identifier = unsafe { (symbols.device_identifier)(device) };
        if identifier.is_null() {
            return None;
        }
        let udid = read_cf_string(symbols, identifier);
        unsafe { (symbols.cf_release)(identifier) };
        if udid.is_empty() {
            return None;
        }

        let mut info = DeviceInfo {
            udid,
            name: String::new(),
            product: String::new(),
            version: String::new(),
            build: String::new(),
            language: String::new(),
            locale: String::new(),
            connection: connection_name(unsafe { (symbols.interface_type)(device) }),
            bold_text: None,
        };

        if unsafe { (symbols.connect)(device) } != 0 {
            // Unreachable or a stale pairing. Say so rather than dropping it: the
            // previous implementation listed it and let the caller decide.
            return Some(info);
        }
        if unsafe { (symbols.is_paired)(device) } == 0 {
            unsafe { (symbols.pair)(device) };
        }
        if unsafe { (symbols.validate_pairing)(device) } == 0
            && unsafe { (symbols.start_session)(device) } == 0
        {
            info.name = value_of(symbols, device, None, "DeviceName");
            info.version = value_of(symbols, device, None, "ProductVersion");
            info.product = value_of(symbols, device, None, "ProductType");
            info.build = value_of(symbols, device, None, "BuildVersion");
            info.language = value_of(symbols, device, Some("com.apple.international"), "Language");
            info.locale = value_of(symbols, device, Some("com.apple.international"), "Locale");
            info.bold_text = boolean_of(
                symbols,
                device,
                "com.apple.Accessibility",
                "EnhancedTextLegibility",
            );
            unsafe { (symbols.stop_session)(device) };
        }
        unsafe { (symbols.disconnect)(device) };
        Some(info)
    }

    extern "C" fn on_device(info: *mut CallbackInfo, _context: *mut c_void) {
        // Called by the framework: no panics may cross this boundary, so every
        // step checks rather than unwraps.
        let Ok(symbols) = symbols() else { return };
        if info.is_null() {
            return;
        }
        let (device, message) = unsafe { ((*info).device, (*info).message) };
        if device.is_null() || message != ATTACHED {
            return;
        }
        let Some(entry) = describe(symbols, device) else {
            return;
        };
        // One entry per device: the callback fires again as devices settle.
        let _ = FOUND.try_with(|found| {
            if let Ok(mut devices) = found.try_borrow_mut() {
                if !devices.iter().any(|seen| seen.udid == entry.udid) {
                    devices.push(entry);
                }
            }
        });
    }

    /// Every iPhone this Mac can reach, cabled or over the network.
    ///
    /// Waits two seconds for the framework to report, the same window the previous
    /// helper used.
    pub fn list_devices() -> Result<Vec<DeviceInfo>, DeviceError> {
        let symbols = symbols()?;
        let Some(dictionary) = options(symbols) else {
            return Err(DeviceError::Framework {
                detail: "could not build the notification options".to_owned(),
            });
        };

        FOUND.with(|found| found.borrow_mut().clear());

        let mut subscription: CfType = std::ptr::null();
        let status = unsafe {
            (symbols.subscribe)(
                on_device,
                0,
                0,
                std::ptr::null_mut(),
                &mut subscription,
                dictionary,
            )
        };
        if status == 0 {
            unsafe { (symbols.cf_run_loop_run)(symbols.k_run_loop_default_mode, 2.0, 0) };
        }
        if !subscription.is_null() {
            unsafe { (symbols.unsubscribe)(subscription) };
        }
        unsafe { (symbols.cf_release)(dictionary) };

        if status != 0 {
            return Err(DeviceError::Subscribe { status });
        }
        Ok(FOUND.with(|found| found.borrow().clone()))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn interface_types_are_named() {
            assert_eq!(connection_name(1), "usb");
            assert_eq!(connection_name(2), "network");
            assert_eq!(connection_name(0), "unknown");
            assert_eq!(connection_name(99), "unknown");
        }

        #[test]
        fn the_framework_and_its_symbols_are_there() {
            let symbols = symbols().expect("MobileDevice.framework is part of macOS");
            assert!(!symbols.k_cf_true.is_null());
            assert!(!symbols.k_run_loop_default_mode.is_null());
        }

        #[test]
        #[ignore = "needs an iPhone attached"]
        fn a_connected_iphone_is_discovered() {
            let devices = list_devices().expect("discovery runs");
            println!("{devices:#?}");
            assert!(
                !devices.is_empty(),
                "plug an iPhone in, or expect this to fail"
            );
            for device in &devices {
                assert!(
                    !device.udid.is_empty(),
                    "a device without a udid is useless"
                );
                assert!(
                    device.connection == "usb" || device.connection == "network",
                    "unexpected connection kind: {}",
                    device.connection
                );
            }
        }
    }
}

#[cfg(target_os = "macos")]
pub use macos::{list_devices, DeviceError, DeviceInfo};

#[cfg(not(target_os = "macos"))]
mod elsewhere {
    use serde::Serialize;
    use thiserror::Error;

    #[derive(Debug, Error)]
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
}

#[cfg(not(target_os = "macos"))]
pub use elsewhere::{list_devices, DeviceError, DeviceInfo};
