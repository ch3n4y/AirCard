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
    use std::cell::{Cell, RefCell};
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

    #[derive(Debug, Clone, Error)]
    pub enum DeviceError {
        #[error("could not load {FRAMEWORK}: {detail}")]
        Framework { detail: String },
        #[error("{name} is missing from the framework, so this macOS version cannot be used")]
        MissingSymbol { name: String },
        #[error("device discovery could not start (status {status})")]
        Subscribe { status: c_int },
        #[error("no iPhone with udid {udid} is reachable")]
        NotFound { udid: String },
        #[error("the iPhone could not be opened (status {status})")]
        Connect { status: c_int },
        #[error("pairing with the iPhone was refused (status {status})")]
        Pairing { status: c_int },
        #[error("the device session could not start (status {status})")]
        Session { status: c_int },
        #[error("the afc service could not start (status {status})")]
        Service { status: c_int },
        #[error("afc refused the operation (status {status})")]
        Afc { status: c_int },
        #[error("{path} is not a path this code will send to the device")]
        Path { path: String },
        #[error("{path} is not there")]
        AfcPath { path: String },
        #[error("{path} is {size} bytes, over the {limit} byte limit")]
        TooLarge { path: String, size: u64, limit: u64 },
        #[error("{path} is still there after removing it")]
        StillThere { path: String },
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
        // Sessions and AFC. The socket comes from the service connection, and the
        // secure IO context has to be handed to AFC, or it quietly transfers nothing.
        secure_start_service:
            unsafe extern "C" fn(*const c_void, CfString, *const c_void, *mut CfType) -> c_int,
        service_socket: unsafe extern "C" fn(CfType) -> c_int,
        service_secure_context: unsafe extern "C" fn(CfType) -> *mut c_void,
        service_invalidate: unsafe extern "C" fn(CfType) -> c_int,
        cf_retain: unsafe extern "C" fn(CfType) -> CfType,
        cf_run_loop_stop: unsafe extern "C" fn(CfType),
        cf_run_loop_current: unsafe extern "C" fn() -> CfType,
        afc_open: unsafe extern "C" fn(c_int, u32, *mut CfType) -> c_int,
        afc_close: unsafe extern "C" fn(CfType) -> c_int,
        afc_set_secure_context: unsafe extern "C" fn(CfType, *mut c_void) -> c_int,
        afc_set_dispose_secure_context: unsafe extern "C" fn(CfType, c_int) -> c_int,
        afc_set_io_timeout: unsafe extern "C" fn(CfType, u32) -> c_int,
        afc_file_info_open: unsafe extern "C" fn(CfType, *const c_char, *mut CfType) -> c_int,
        afc_key_value_read:
            unsafe extern "C" fn(CfType, *mut *mut c_char, *mut *mut c_char) -> c_int,
        afc_key_value_close: unsafe extern "C" fn(CfType) -> c_int,
        afc_file_ref_open: unsafe extern "C" fn(CfType, *const c_char, u64, *mut CfType) -> c_int,
        afc_file_ref_read: unsafe extern "C" fn(CfType, CfType, *mut c_void, *mut i64) -> c_int,
        afc_file_ref_write: unsafe extern "C" fn(CfType, CfType, *const c_void, i64) -> c_int,
        afc_file_ref_close: unsafe extern "C" fn(CfType, CfType) -> c_int,
        afc_directory_create: unsafe extern "C" fn(CfType, *const c_char) -> c_int,
        afc_remove_path: unsafe extern "C" fn(CfType, *const c_char) -> c_int,
        afc_directory_open: unsafe extern "C" fn(CfType, *const c_char, *mut CfType) -> c_int,
        afc_directory_read: unsafe extern "C" fn(CfType, CfType, *mut *mut c_char) -> c_int,
        afc_directory_close: unsafe extern "C" fn(CfType, CfType) -> c_int,
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
                    secure_start_service: function(handle, "AMDeviceSecureStartService")?,
                    service_socket: function(handle, "AMDServiceConnectionGetSocket")?,
                    service_secure_context: function(handle, "AMDServiceConnectionGetSecureIOContext")?,
                    service_invalidate: function(handle, "AMDServiceConnectionInvalidate")?,
                    cf_retain: function(handle, "CFRetain")?,
                    cf_run_loop_stop: function(handle, "CFRunLoopStop")?,
                    cf_run_loop_current: function(handle, "CFRunLoopGetCurrent")?,
                    afc_open: function(handle, "AFCConnectionOpen")?,
                    afc_close: function(handle, "AFCConnectionClose")?,
                    afc_set_secure_context: function(handle, "AFCConnectionSetSecureContext")?,
                    afc_set_dispose_secure_context: function(handle, "AFCConnectionSetDisposeSecureContextOnInvalidate")?,
                    afc_set_io_timeout: function(handle, "AFCConnectionSetIOTimeout")?,
                    afc_file_info_open: function(handle, "AFCFileInfoOpen")?,
                    afc_key_value_read: function(handle, "AFCKeyValueRead")?,
                    afc_key_value_close: function(handle, "AFCKeyValueClose")?,
                    afc_file_ref_open: function(handle, "AFCFileRefOpen")?,
                    afc_file_ref_read: function(handle, "AFCFileRefRead")?,
                    afc_file_ref_write: function(handle, "AFCFileRefWrite")?,
                    afc_file_ref_close: function(handle, "AFCFileRefClose")?,
                    afc_directory_create: function(handle, "AFCDirectoryCreate")?,
                    afc_remove_path: function(handle, "AFCRemovePath")?,
                    afc_directory_open: function(handle, "AFCDirectoryOpen")?,
                    afc_directory_read: function(handle, "AFCDirectoryRead")?,
                    afc_directory_close: function(handle, "AFCDirectoryClose")?,
                })
            })
            .as_ref()
            .map_err(Clone::clone)
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

    /// A device the framework handed us, retained so it stays alive while we use it.
    struct DeviceHandle(*const c_void);

    impl Drop for DeviceHandle {
        fn drop(&mut self) {
            if let Ok(symbols) = symbols() {
                unsafe { (symbols.cf_release)(self.0) };
            }
        }
    }

    thread_local! {
        /// The udid `find_device` is waiting for, and what it found.
        static WANTED: RefCell<Option<String>> = const { RefCell::new(None) };
        static TARGET: Cell<*const c_void> = const { Cell::new(std::ptr::null()) };
    }

    extern "C" fn on_target(info: *mut CallbackInfo, _context: *mut c_void) {
        // Same rules as the discovery callback: framework owned, so no panics.
        let Ok(symbols) = symbols() else { return };
        if info.is_null() || !TARGET.with(|slot| slot.get().is_null()) {
            return;
        }
        let (device, message) = unsafe { ((*info).device, (*info).message) };
        if device.is_null() || message != ATTACHED {
            return;
        }
        let identifier = unsafe { (symbols.device_identifier)(device) };
        if identifier.is_null() {
            return;
        }
        let udid = read_cf_string(symbols, identifier);
        unsafe { (symbols.cf_release)(identifier) };

        let wanted = WANTED.with(|wanted| wanted.borrow().clone());
        if wanted.as_deref() != Some(udid.as_str()) {
            return;
        }
        TARGET.with(|slot| slot.set(unsafe { (symbols.cf_retain)(device) }));
        // Stop waiting the moment the phone that was asked for shows up.
        unsafe { (symbols.cf_run_loop_stop)((symbols.cf_run_loop_current)()) };
    }

    /// Finds one iPhone by udid and retains it, waiting up to `seconds` for it.
    fn find_device(udid: &str, seconds: f64) -> Result<DeviceHandle, DeviceError> {
        let symbols = symbols()?;
        let Some(dictionary) = options(symbols) else {
            return Err(DeviceError::Framework {
                detail: "could not build the notification options".to_owned(),
            });
        };

        WANTED.with(|wanted| *wanted.borrow_mut() = Some(udid.to_owned()));
        TARGET.with(|slot| slot.set(std::ptr::null()));

        let mut subscription: CfType = std::ptr::null();
        let status = unsafe {
            (symbols.subscribe)(
                on_target,
                0,
                0,
                std::ptr::null_mut(),
                &mut subscription,
                dictionary,
            )
        };
        if status == 0 {
            unsafe { (symbols.cf_run_loop_run)(symbols.k_run_loop_default_mode, seconds, 0) };
        }
        if !subscription.is_null() {
            unsafe { (symbols.unsubscribe)(subscription) };
        }
        unsafe { (symbols.cf_release)(dictionary) };
        WANTED.with(|wanted| *wanted.borrow_mut() = None);

        if status != 0 {
            return Err(DeviceError::Subscribe { status });
        }
        let target = TARGET.with(|slot| slot.get());
        if target.is_null() {
            return Err(DeviceError::NotFound {
                udid: udid.to_owned(),
            });
        }
        Ok(DeviceHandle(target))
    }

    /// A live AFC connection to one iPhone.
    ///
    /// This reaches `/var/mobile/Media` and nothing else. A card's files live in
    /// `/var/mobile/Library/Passes`, which is why the AirTraffic escape exists:
    /// AFC will not list that directory, refuses `..`, and does not support
    /// `MAKE_LINK` at all, so it cannot even plant a symlink to get there.
    ///
    /// One session at a time. Two sessions to the same device at once abort the
    /// process -- a crash, not an error: the framework is not built for it.
    /// Callers serialise: the window keeps one operation in flight, and the device
    /// tests run with `--test-threads=1`.
    pub struct AfcSession {
        device: DeviceHandle,
        service: CfType,
        connection: CfType,
    }

    impl AfcSession {
        /// Opens `com.apple.afc` on one iPhone.
        pub fn open(udid: &str) -> Result<Self, DeviceError> {
            let symbols = symbols()?;
            let device = find_device(udid, 30.0)?;

            let status = unsafe { (symbols.connect)(device.0) };
            if status != 0 {
                return Err(DeviceError::Connect { status });
            }
            if unsafe { (symbols.is_paired)(device.0) } == 0 {
                unsafe { (symbols.pair)(device.0) };
            }
            let mut validated = unsafe { (symbols.validate_pairing)(device.0) };
            if validated != 0 {
                // Pair again and retry once, the way the previous helper did.
                unsafe { (symbols.pair)(device.0) };
                validated = unsafe { (symbols.validate_pairing)(device.0) };
            }
            if validated != 0 {
                unsafe { (symbols.disconnect)(device.0) };
                return Err(DeviceError::Pairing { status: validated });
            }
            let status = unsafe { (symbols.start_session)(device.0) };
            if status != 0 {
                unsafe { (symbols.disconnect)(device.0) };
                return Err(DeviceError::Session { status });
            }

            let Some((service_name, _name_owned)) = cf_string(symbols, "com.apple.afc") else {
                unsafe { (symbols.stop_session)(device.0) };
                unsafe { (symbols.disconnect)(device.0) };
                return Err(DeviceError::Framework {
                    detail: "could not name the afc service".to_owned(),
                });
            };
            let mut service: CfType = std::ptr::null();
            let status = unsafe {
                (symbols.secure_start_service)(
                    device.0,
                    service_name,
                    std::ptr::null(),
                    &mut service,
                )
            };
            unsafe { (symbols.cf_release)(service_name) };
            if status != 0 || service.is_null() {
                unsafe { (symbols.stop_session)(device.0) };
                unsafe { (symbols.disconnect)(device.0) };
                return Err(DeviceError::Service { status });
            }

            let mut connection: CfType = std::ptr::null();
            let status = unsafe {
                (symbols.afc_open)(
                    (symbols.service_socket)(service),
                    0,
                    &mut connection,
                )
            };
            if status != 0 || connection.is_null() {
                unsafe { (symbols.service_invalidate)(service) };
                unsafe { (symbols.stop_session)(device.0) };
                unsafe { (symbols.disconnect)(device.0) };
                return Err(DeviceError::Afc { status });
            }

            // Hand AFC the secure IO context, or the connection answers and moves
            // no bytes. The previous helper carries the same note.
            let secure = unsafe { (symbols.service_secure_context)(service) };
            if !secure.is_null() {
                unsafe {
                    (symbols.afc_set_secure_context)(connection, secure);
                    (symbols.afc_set_dispose_secure_context)(connection, 0);
                    (symbols.afc_set_io_timeout)(connection, 30);
                }
            }

            Ok(Self {
                device,
                service,
                connection,
            })
        }

        /// A path is only sent when it is relative to Media and cannot climb out.
        fn checked(name: &str) -> Result<CString, DeviceError> {
            if name.is_empty() || name.split('/').any(|part| part == "..") {
                return Err(DeviceError::Path {
                    path: name.to_owned(),
                });
            }
            CString::new(name).map_err(|_| DeviceError::Path {
                path: name.to_owned(),
            })
        }

        /// What AFC says about a path: its size and kind, or None when it is absent.
        pub fn stat(&self, path: &str) -> Option<(u64, String)> {
            let symbols = symbols().ok()?;
            let name = Self::checked(path).ok()?;
            let mut info: CfType = std::ptr::null();
            let status =
                unsafe { (symbols.afc_file_info_open)(self.connection, name.as_ptr(), &mut info) };
            if status != 0 || info.is_null() {
                return None;
            }
            let mut size = 0_u64;
            let mut kind = String::new();
            loop {
                let mut key: *mut c_char = std::ptr::null_mut();
                let mut value: *mut c_char = std::ptr::null_mut();
                let status = unsafe { (symbols.afc_key_value_read)(info, &mut key, &mut value) };
                if status != 0 || key.is_null() || value.is_null() {
                    break;
                }
                let key_text = unsafe { CStr::from_ptr(key) }.to_string_lossy();
                let value_text = unsafe { CStr::from_ptr(value) }.to_string_lossy();
                match key_text.as_ref() {
                    "st_size" => size = value_text.parse().unwrap_or(0),
                    "st_ifmt" => kind = value_text.into_owned(),
                    _ => {}
                }
            }
            unsafe { (symbols.afc_key_value_close)(info) };
            Some((size, kind))
        }

        pub fn exists(&self, path: &str) -> bool {
            self.stat(path).is_some()
        }

        /// Reads a file whole, refusing anything over `limit`.
        pub fn read(&self, path: &str, limit: u64) -> Result<Vec<u8>, DeviceError> {
            let symbols = symbols()?;
            let (size, _kind) = self.stat(path).ok_or_else(|| DeviceError::AfcPath {
                path: path.to_owned(),
            })?;
            if size > limit {
                return Err(DeviceError::TooLarge {
                    path: path.to_owned(),
                    size,
                    limit,
                });
            }
            let name = Self::checked(path)?;
            let mut file: CfType = std::ptr::null();
            let status = unsafe {
                (symbols.afc_file_ref_open)(self.connection, name.as_ptr(), 1, &mut file)
            };
            if status != 0 || file.is_null() {
                return Err(DeviceError::Afc { status });
            }

            let mut data = vec![0_u8; size as usize];
            let mut offset = 0_usize;
            let mut failure = None;
            while offset < data.len() {
                let mut chunk = (data.len() - offset) as i64;
                let status = unsafe {
                    (symbols.afc_file_ref_read)(
                        self.connection,
                        file,
                        data[offset..].as_mut_ptr() as *mut c_void,
                        &mut chunk,
                    )
                };
                if status != 0 || chunk <= 0 || chunk as usize > data.len() - offset {
                    failure = Some(DeviceError::Afc { status });
                    break;
                }
                offset += chunk as usize;
            }
            let closed = unsafe { (symbols.afc_file_ref_close)(self.connection, file) };
            if let Some(error) = failure {
                return Err(error);
            }
            if closed != 0 {
                return Err(DeviceError::Afc { status: closed });
            }
            Ok(data)
        }

        /// Writes a file whole, creating or truncating it.
        pub fn write(&self, path: &str, data: &[u8]) -> Result<(), DeviceError> {
            let symbols = symbols()?;
            let name = Self::checked(path)?;
            let mut file: CfType = std::ptr::null();
            let status = unsafe {
                (symbols.afc_file_ref_open)(self.connection, name.as_ptr(), 3, &mut file)
            };
            if status != 0 || file.is_null() {
                return Err(DeviceError::Afc { status });
            }
            let written = if data.is_empty() {
                0
            } else {
                unsafe {
                    (symbols.afc_file_ref_write)(
                        self.connection,
                        file,
                        data.as_ptr() as *const c_void,
                        data.len() as i64,
                    )
                }
            };
            let closed = unsafe { (symbols.afc_file_ref_close)(self.connection, file) };
            if written != 0 {
                return Err(DeviceError::Afc { status: written });
            }
            if closed != 0 {
                return Err(DeviceError::Afc { status: closed });
            }
            Ok(())
        }

        pub fn create_directory(&self, path: &str) -> Result<(), DeviceError> {
            let symbols = symbols()?;
            let name = Self::checked(path)?;
            let status =
                unsafe { (symbols.afc_directory_create)(self.connection, name.as_ptr()) };
            if status != 0 {
                return Err(DeviceError::Afc { status });
            }
            Ok(())
        }

        /// Removes a file, or a directory and everything under it.
        ///
        /// Verifies afterwards: an AFC remove that reports success and leaves the
        /// file behind would be worse than a failure, since callers rely on this to
        /// prove a skin is gone from a card.
        pub fn remove(&self, path: &str) -> Result<(), DeviceError> {
            let symbols = symbols()?;
            let name = Self::checked(path)?;
            let status = unsafe { (symbols.afc_remove_path)(self.connection, name.as_ptr()) };
            if status != 0 {
                return Err(DeviceError::Afc { status });
            }
            if self.exists(path) {
                return Err(DeviceError::StillThere {
                    path: path.to_owned(),
                });
            }
            Ok(())
        }

        /// Directory contents, with `.` and `..` left out.
        pub fn list(&self, path: &str) -> Result<Vec<String>, DeviceError> {
            let symbols = symbols()?;
            let name = Self::checked(path)?;
            let mut directory: CfType = std::ptr::null();
            let status = unsafe {
                (symbols.afc_directory_open)(self.connection, name.as_ptr(), &mut directory)
            };
            if status != 0 || directory.is_null() {
                return Err(DeviceError::Afc { status });
            }
            let mut entries = Vec::new();
            loop {
                let mut entry: *mut c_char = std::ptr::null_mut();
                let status =
                    unsafe { (symbols.afc_directory_read)(self.connection, directory, &mut entry) };
                if status != 0 || entry.is_null() {
                    break;
                }
                let text = unsafe { CStr::from_ptr(entry) }
                    .to_string_lossy()
                    .into_owned();
                if text != "." && text != ".." {
                    entries.push(text);
                }
            }
            unsafe { (symbols.afc_directory_close)(self.connection, directory) };
            entries.sort();
            Ok(entries)
        }
    }

    impl Drop for AfcSession {
        fn drop(&mut self) {
            let Ok(symbols) = symbols() else { return };
            unsafe {
                if !self.connection.is_null() {
                    (symbols.afc_close)(self.connection);
                }
                if !self.service.is_null() {
                    (symbols.service_invalidate)(self.service);
                }
                (symbols.stop_session)(self.device.0);
                (symbols.disconnect)(self.device.0);
            }
            // The retained device is released by DeviceHandle's own Drop.
        }
    }

    #[cfg(test)]
    mod afc_tests {
        use super::*;

        /// The device to test against: AIRCARD_TEST_UDID, or the only one plugged in.
        pub fn test_device() -> String {
            if let Ok(udid) = std::env::var("AIRCARD_TEST_UDID") {
                return udid;
            }
            let devices = list_devices().expect("discovery runs");
            assert_eq!(
                devices.len(),
                1,
                "set AIRCARD_TEST_UDID to choose between {devices:#?}"
            );
            devices[0].udid.clone()
        }

        #[test]
        #[ignore = "needs an iPhone attached"]
        fn media_is_listed_the_way_the_device_reported_it() {
            let session = AfcSession::open(&test_device()).expect("afc opens");
            let entries = session.list("/").expect("media lists");
            println!("{entries:#?}");
            for expected in ["DCIM", "Airlock", "Books"] {
                assert!(
                    entries.iter().any(|name| name == expected),
                    "{expected} is missing from {entries:?}"
                );
            }
        }

        #[test]
        #[ignore = "needs an iPhone attached"]
        fn a_file_can_be_written_read_and_removed() {
            let session = AfcSession::open(&test_device()).expect("afc opens");
            let path = "/aircard-afc-probe.txt";
            let payload = b"aircard probe: written over afc\n";
            let _ = session.remove(path);

            session.write(path, payload).expect("write");
            assert!(session.exists(path), "the file should be there after writing");
            assert_eq!(session.read(path, 4096).expect("read"), payload);

            session.remove(path).expect("remove");
            assert!(!session.exists(path), "the probe must not be left behind");
        }

        #[test]
        #[ignore = "needs an iPhone attached"]
        fn a_path_cannot_climb_out_of_media() {
            // This is why the escape exists. AFC's root is Media, and it says so.
            let session = AfcSession::open(&test_device()).expect("afc opens");
            let outside = "/../Library/Passes/Cards";
            assert!(session.list(outside).is_err(), "traversal must be refused");
            assert!(!session.exists(outside));
        }
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
pub use macos::{list_devices, AfcSession, DeviceError, DeviceInfo};

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
}

#[cfg(not(target_os = "macos"))]
pub use elsewhere::{list_devices, DeviceError, DeviceInfo};
