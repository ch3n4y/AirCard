//! The macOS half of the crate: everything that needs Apple's frameworks.
//!
//! A directory rather than one file because the escape out of `Media` grew its
//! own module. `MobileDevice.framework` is bound here, `AirTrafficHost.framework`
//! in `airtraffic`, and both are reached the same way -- `dlopen`, never a link
//! at build time.

mod airtraffic;

pub use airtraffic::{AirTraffic, ZipConduit};

use std::cell::{Cell, RefCell};
use std::ffi::{c_char, c_int, c_long, c_void, CStr, CString};
use std::sync::OnceLock;

use serde::Serialize;
use thiserror::Error;

/// Where the private framework lives on every macOS install.
const FRAMEWORK: &str = "/System/Library/PrivateFrameworks/MobileDevice.framework/MobileDevice";

/// `kCFStringEncodingUTF8`
const UTF8: u32 = 0x0800_0100;

/// `message` value that means "device attached".
const ATTACHED: u32 = 1;

/// `kCFPropertyListBinaryFormat_v1_0`: the only format `os_trace_relay`, and
/// every other MobileDevice service, accepts for a request plist.
const BINARY_PLIST: c_int = 200;

/// `kCFNumberSInt64Type`. The request carries `UINT32_MAX` as a pid, so the
/// number is built 64-bit wide rather than risking a sign flip.
const SINT64: isize = 4;

/// `com.apple.os_trace_relay` frame cap. Anything larger is corrupt, and is
/// rejected before the payload is allocated.
const MAX_FRAME_LENGTH: u32 = 16 * 1024 * 1024;

/// Every activity record opens with this fixed header before its payload.
const RECORD_HEADER: usize = 129;

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
    #[error("the device log stream ended unexpectedly")]
    LogEnded,
    #[error("the device returned an invalid log frame: {detail}")]
    LogFrame { detail: String },
    #[error("the device refused to start log streaming")]
    LogRefused,
    #[error("the device's log reply could not be decoded")]
    LogReply,
    #[error("AirTraffic could not be used: {detail}")]
    Airtraffic { detail: String },
    #[error("the iPhone never sent {stage}, so the AirTraffic handshake stopped there")]
    Handshake { stage: &'static str },
    #[error("{count} of the assets asked for are not in the iPhone's manifest")]
    AssetsMissing { count: usize },
    #[error("the request for the iPhone could not be built: {detail}")]
    Request { detail: String },
    #[error("the streaming zip service refused the archive (status {status})")]
    ZipConduit { status: c_int },
    #[error("the streaming zip service took the archive and never answered")]
    ZipConduitReply,
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
    // Building the StartActivity plist request and decoding the plist reply.
    cf_number_create: unsafe extern "C" fn(CfAllocator, isize, *const c_void) -> CfType,
    cf_data_create: unsafe extern "C" fn(CfAllocator, *const u8, CfIndex) -> CfType,
    cf_property_list_create:
        unsafe extern "C" fn(CfAllocator, CfType, u64, *mut CfIndex, *mut CfType) -> CfType,
    cf_dictionary_get_value: unsafe extern "C" fn(CfDictionary, CfType) -> CfType,
    // The AirTraffic escape builds its own argument arrays, and it checks the kind
    // of what the phone sent back: a CFArray call on a CFString is undefined
    // behaviour rather than an error, so the type IDs are looked up as well.
    cf_array_create:
        unsafe extern "C" fn(CfAllocator, *const CfType, CfIndex, *const c_void) -> CfType,
    cf_array_get_count: unsafe extern "C" fn(CfType) -> CfIndex,
    cf_array_get_value: unsafe extern "C" fn(CfType, CfIndex) -> CfType,
    cf_get_type_id: unsafe extern "C" fn(CfType) -> usize,
    cf_array_type_id: unsafe extern "C" fn() -> usize,
    cf_dictionary_type_id: unsafe extern "C" fn() -> usize,
    k_cf_true: CfType,
    k_cf_false: CfType,
    k_run_loop_default_mode: CfType,
    k_dictionary_key_callbacks: CfType,
    k_dictionary_value_callbacks: CfType,
    k_array_callbacks: CfType,

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
    // os_trace_relay reads and writes whole plists over the service connection.
    service_receive: unsafe extern "C" fn(CfType, *mut c_void, c_long) -> c_long,
    service_send_message: unsafe extern "C" fn(CfType, CfType, c_int) -> c_int,
    // The streaming zip conduit sends the archive as bytes and then reads one
    // plist reply; the log stream's `service_receive` is the wrong shape for it.
    service_send: unsafe extern "C" fn(CfType, *const c_void, usize) -> c_int,
    service_receive_message: unsafe extern "C" fn(CfType, *mut CfType, *mut CfIndex) -> c_int,
    cf_retain: unsafe extern "C" fn(CfType) -> CfType,
    cf_run_loop_stop: unsafe extern "C" fn(CfType),
    cf_run_loop_current: unsafe extern "C" fn() -> CfType,
    afc_open: unsafe extern "C" fn(c_int, u32, *mut CfType) -> c_int,
    afc_close: unsafe extern "C" fn(CfType) -> c_int,
    afc_set_secure_context: unsafe extern "C" fn(CfType, *mut c_void) -> c_int,
    afc_set_dispose_secure_context: unsafe extern "C" fn(CfType, c_int) -> c_int,
    afc_set_io_timeout: unsafe extern "C" fn(CfType, u32) -> c_int,
    afc_file_info_open: unsafe extern "C" fn(CfType, *const c_char, *mut CfType) -> c_int,
    afc_key_value_read: unsafe extern "C" fn(CfType, *mut *mut c_char, *mut *mut c_char) -> c_int,
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
                cf_number_create: function(handle, "CFNumberCreate")?,
                cf_data_create: function(handle, "CFDataCreate")?,
                cf_property_list_create: function(handle, "CFPropertyListCreateWithData")?,
                cf_dictionary_get_value: function(handle, "CFDictionaryGetValue")?,
                cf_array_create: function(handle, "CFArrayCreate")?,
                cf_array_get_count: function(handle, "CFArrayGetCount")?,
                cf_array_get_value: function(handle, "CFArrayGetValueAtIndex")?,
                cf_get_type_id: function(handle, "CFGetTypeID")?,
                cf_array_type_id: function(handle, "CFArrayGetTypeID")?,
                cf_dictionary_type_id: function(handle, "CFDictionaryGetTypeID")?,
                k_cf_true: data_pointer(handle, "kCFBooleanTrue")?,
                k_cf_false: data_pointer(handle, "kCFBooleanFalse")?,
                k_run_loop_default_mode: data_pointer(handle, "kCFRunLoopDefaultMode")?,
                k_dictionary_key_callbacks: function(handle, "kCFTypeDictionaryKeyCallBacks")?,
                k_dictionary_value_callbacks: function(handle, "kCFTypeDictionaryValueCallBacks")?,
                k_array_callbacks: function(handle, "kCFTypeArrayCallBacks")?,
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
                service_receive: function(handle, "AMDServiceConnectionReceive")?,
                service_send_message: function(handle, "AMDServiceConnectionSendMessage")?,
                service_send: function(handle, "AMDServiceConnectionSend")?,
                service_receive_message: function(handle, "AMDServiceConnectionReceiveMessage")?,
                cf_retain: function(handle, "CFRetain")?,
                cf_run_loop_stop: function(handle, "CFRunLoopStop")?,
                cf_run_loop_current: function(handle, "CFRunLoopGetCurrent")?,
                afc_open: function(handle, "AFCConnectionOpen")?,
                afc_close: function(handle, "AFCConnectionClose")?,
                afc_set_secure_context: function(handle, "AFCConnectionSetSecureContext")?,
                afc_set_dispose_secure_context: function(
                    handle,
                    "AFCConnectionSetDisposeSecureContextOnInvalidate",
                )?,
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
    let reference = unsafe { (symbols.cf_string_create)(std::ptr::null(), owned.as_ptr(), UTF8) };
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

fn value_of(symbols: &Symbols, device: *const c_void, domain: Option<&str>, key: &str) -> String {
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

fn boolean_of(symbols: &Symbols, device: *const c_void, domain: &str, key: &str) -> Option<bool> {
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

/// Connects, pairs and starts a session, then opens one secure service.
///
/// AFC and the log stream share this dance -- connect, pair, validate, start
/// a session -- before either can name the service it wants. Kept private so
/// the public shapes stay `AfcSession` and `LogStream`.
fn open_service(udid: &str, service_name: &str) -> Result<(DeviceHandle, CfType), DeviceError> {
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

    let Some((name, _name_owned)) = cf_string(symbols, service_name) else {
        unsafe { (symbols.stop_session)(device.0) };
        unsafe { (symbols.disconnect)(device.0) };
        return Err(DeviceError::Framework {
            detail: format!("could not name the {service_name} service"),
        });
    };
    let mut service: CfType = std::ptr::null();
    let status =
        unsafe { (symbols.secure_start_service)(device.0, name, std::ptr::null(), &mut service) };
    unsafe { (symbols.cf_release)(name) };
    if status != 0 || service.is_null() {
        unsafe { (symbols.stop_session)(device.0) };
        unsafe { (symbols.disconnect)(device.0) };
        return Err(DeviceError::Service { status });
    }
    Ok((device, service))
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
        let (device, service) = open_service(udid, "com.apple.afc")?;

        let mut connection: CfType = std::ptr::null();
        let status =
            unsafe { (symbols.afc_open)((symbols.service_socket)(service), 0, &mut connection) };
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
        let status =
            unsafe { (symbols.afc_file_ref_open)(self.connection, name.as_ptr(), 1, &mut file) };
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
        let status =
            unsafe { (symbols.afc_file_ref_open)(self.connection, name.as_ptr(), 3, &mut file) };
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
        let status = unsafe { (symbols.afc_directory_create)(self.connection, name.as_ptr()) };
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
        let status =
            unsafe { (symbols.afc_directory_open)(self.connection, name.as_ptr(), &mut directory) };
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

/// One decoded frame: the type byte plus its payload.
struct Frame {
    kind: u8,
    payload: Vec<u8>,
}

/// A byte source the framing decoder reads from.
///
/// The decoder only ever needs exact-length reads, but a service connection
/// can hand back short reads, so `read_exact` loops over `read` and the tests
/// can make `read` return tiny fragments. Abstracted from the service pointer
/// so framing, endianness and the record layout are testable with no iPhone.
trait ByteSource {
    /// Returns up to `bytes.len()` bytes; fewer means the stream is ending.
    fn read(&mut self, bytes: &mut [u8]) -> Result<usize, DeviceError>;

    /// Fills `bytes` completely, looping over short reads.
    fn read_exact(&mut self, bytes: &mut [u8]) -> Result<(), DeviceError> {
        let mut offset = 0;
        while offset < bytes.len() {
            let count = self.read(&mut bytes[offset..])?;
            if count == 0 || count > bytes.len() - offset {
                return Err(DeviceError::LogEnded);
            }
            offset += count;
        }
        Ok(())
    }
}

/// The log service connection seen as a byte source.
struct ServiceSource<'a> {
    symbols: &'a Symbols,
    connection: CfType,
}

impl ByteSource for ServiceSource<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> Result<usize, DeviceError> {
        let count = unsafe {
            (self.symbols.service_receive)(
                self.connection,
                bytes.as_mut_ptr() as *mut c_void,
                bytes.len() as c_long,
            )
        };
        if count <= 0 || count as usize > bytes.len() {
            return Err(DeviceError::LogEnded);
        }
        Ok(count as usize)
    }
}

/// Reads one frame: a type byte, a 32-bit length and the payload.
///
/// Plist replies (type 1) carry their length big endian; activity records
/// (type 2) little endian. The split is not a mistake -- it is what
/// `os_trace_relay` sends, and libimobiledevice's `ostrace.c` reads it back
/// the same way. A corrupt length is rejected before anything is allocated.
fn read_frame(source: &mut impl ByteSource) -> Result<Frame, DeviceError> {
    let mut header = [0_u8; 5];
    source.read_exact(&mut header)?;
    let kind = header[0];
    let length = match kind {
        1 => u32::from_be_bytes([header[1], header[2], header[3], header[4]]),
        2 => u32::from_le_bytes([header[1], header[2], header[3], header[4]]),
        _ => {
            return Err(DeviceError::LogFrame {
                detail: format!("unsupported frame type {kind}"),
            })
        }
    };
    if length == 0 || length > MAX_FRAME_LENGTH {
        return Err(DeviceError::LogFrame {
            detail: format!("invalid frame length {length}"),
        });
    }
    let mut payload = vec![0_u8; length as usize];
    source.read_exact(&mut payload)?;
    Ok(Frame { kind, payload })
}

fn u16_le(bytes: &[u8], offset: usize) -> Option<u16> {
    let slice = bytes.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

fn u32_le(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// Drops the trailing NUL bytes the device pads its strings with.
fn trim_nul(bytes: &[u8]) -> &[u8] {
    let mut end = bytes.len();
    while end > 0 && bytes[end - 1] == 0 {
        end -= 1;
    }
    &bytes[..end]
}

/// The text after the last `/`, matching `NSString.lastPathComponent`.
fn last_component(text: &str) -> &str {
    match text.rfind('/') {
        Some(index) => &text[index + 1..],
        None => text,
    }
}

/// Decodes one activity record to `"{process}({image}): {message}\n"`.
///
/// The record opens with a 129-byte header and then carries three
/// length-delimited, NUL-padded UTF-8 strings: process, image, message. The
/// header length is at offset 5, the process length at 37, the image length
/// at 107 and the message length at 109 -- all little endian. Every length is
/// bounds-checked, so a truncated or corrupt record is refused rather than
/// indexed past its end. Multiline messages are kept whole: on iOS 18 the
/// Wallet card path shows up on a continuation line of CoreFoundation's
/// "Resource lookup" message.
fn decode_record(record: &[u8]) -> Option<String> {
    if record.len() < RECORD_HEADER || record[0] != 2 {
        return None;
    }
    let header_length = u32_le(record, 5)? as usize;
    let process_length = u16_le(record, 37)? as usize;
    let image_length = u16_le(record, 107)? as usize;
    let message_length = u32_le(record, 109)? as usize;
    let text_length = process_length
        .checked_add(image_length)?
        .checked_add(message_length)?;
    if header_length < RECORD_HEADER
        || header_length > record.len()
        || process_length == 0
        || message_length == 0
        || text_length > record.len() - header_length
    {
        return None;
    }

    let text = &record[header_length..];
    let process = trim_nul(&text[..process_length]);
    let image = trim_nul(&text[process_length..process_length + image_length]);
    let message_end = process_length + image_length + message_length;
    let message = trim_nul(&text[process_length + image_length..message_end]);
    Some(format!(
        "{}({}): {}\n",
        last_component(&String::from_utf8_lossy(process)),
        last_component(&String::from_utf8_lossy(image)),
        String::from_utf8_lossy(message),
    ))
}

/// `CFNumberCreate` for a 64-bit signed value.
fn cf_number(symbols: &Symbols, value: i64) -> CfType {
    unsafe {
        (symbols.cf_number_create)(
            std::ptr::null(),
            SINT64,
            &value as *const i64 as *const c_void,
        )
    }
}

/// The `StartActivity` request as a CFDictionary the service can send.
///
/// `Pid` `UINT32_MAX` streams every process. The flags ask for payload,
/// historical, callstack and debug events -- the same set the previous helper
/// used, and the reason `syslog_relay` is not enough: it omits the Info/Debug
/// resource lookups that name Wallet cards on iOS 18.
fn activity_request(symbols: &Symbols) -> Option<CfDictionary> {
    let mut keys: Vec<CfString> = Vec::with_capacity(4);
    let mut values: Vec<CfType> = Vec::with_capacity(4);
    // The CFStrings copy their bytes, but the CString backings must outlive
    // the CFStringCreateWithCString calls, so they are held until the end.
    let mut owned: Vec<CString> = Vec::with_capacity(4);

    let (request_key, request_key_owned) = cf_string(symbols, "Request")?;
    let (request_value, request_value_owned) = cf_string(symbols, "StartActivity")?;
    keys.push(request_key);
    values.push(request_value);
    owned.push(request_key_owned);
    owned.push(request_value_owned);

    for (name, value) in [
        ("Pid", u32::MAX as i64),
        ("MessageFilter", 0xFFFF),
        ("StreamFlags", 0x3C),
    ] {
        let (key, key_owned) = cf_string(symbols, name)?;
        let number = cf_number(symbols, value);
        if number.is_null() {
            unsafe { (symbols.cf_release)(key) };
            return None;
        }
        keys.push(key);
        values.push(number);
        owned.push(key_owned);
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
    // The dictionary retains its keys and values, so both vectors can go.
    for reference in &keys {
        unsafe { (symbols.cf_release)(*reference) };
    }
    for value in &values {
        unsafe { (symbols.cf_release)(*value) };
    }
    drop(owned);
    if dictionary.is_null() {
        None
    } else {
        Some(dictionary)
    }
}

/// Reads the plist reply that answers `StartActivity` and checks that it says
/// `RequestSuccessful`. Anything else means the relay refused to stream.
fn check_reply(symbols: &Symbols, service: CfType) -> Result<(), DeviceError> {
    let mut source = ServiceSource {
        symbols,
        connection: service,
    };
    let frame = read_frame(&mut source)?;
    if frame.kind != 1 {
        return Err(DeviceError::LogReply);
    }

    let data = unsafe {
        (symbols.cf_data_create)(
            std::ptr::null(),
            frame.payload.as_ptr(),
            frame.payload.len() as CfIndex,
        )
    };
    if data.is_null() {
        return Err(DeviceError::LogReply);
    }
    let plist = unsafe {
        (symbols.cf_property_list_create)(
            std::ptr::null(),
            data,
            0, // kCFPropertyListImmutable
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    unsafe { (symbols.cf_release)(data) };
    if plist.is_null() {
        return Err(DeviceError::LogReply);
    }

    let Some((key, _key_owned)) = cf_string(symbols, "Status") else {
        unsafe { (symbols.cf_release)(plist) };
        return Err(DeviceError::LogReply);
    };
    let value = unsafe { (symbols.cf_dictionary_get_value)(plist, key) };
    unsafe { (symbols.cf_release)(key) };
    let accepted = !value.is_null() && read_cf_string(symbols, value) == "RequestSuccessful";
    unsafe { (symbols.cf_release)(plist) };
    if accepted {
        Ok(())
    } else {
        Err(DeviceError::LogRefused)
    }
}

/// A live `com.apple.os_trace_relay` stream from one iPhone.
///
/// Same one-session-at-a-time rule as `AfcSession`: two sessions to the same
/// device at once abort the process, so callers serialise.
pub struct LogStream {
    device: DeviceHandle,
    service: CfType,
}

impl LogStream {
    /// Opens the unified activity log stream on one iPhone.
    pub fn open(udid: &str) -> Result<Self, DeviceError> {
        let symbols = symbols()?;
        let (device, service) = open_service(udid, "com.apple.os_trace_relay")?;

        // Every failure past this point has to undo the session: the caller
        // never receives a LogStream whose Drop would clean it up.
        let started = match activity_request(symbols) {
            Some(request) => {
                let status =
                    unsafe { (symbols.service_send_message)(service, request, BINARY_PLIST) };
                unsafe { (symbols.cf_release)(request) };
                if status != 0 {
                    Err(DeviceError::LogRefused)
                } else {
                    check_reply(symbols, service)
                }
            }
            None => Err(DeviceError::Framework {
                detail: "could not build the log request".to_owned(),
            }),
        };

        match started {
            Ok(()) => Ok(Self { device, service }),
            Err(error) => {
                unsafe {
                    (symbols.service_invalidate)(service);
                    (symbols.stop_session)(device.0);
                    (symbols.disconnect)(device.0);
                }
                Err(error)
            }
        }
    }

    /// Pulls the next decoded log line.
    ///
    /// `Ok(None)` means the frame was not an activity record: the stream also
    /// carries plist replies and other event kinds, and activity records that
    /// do not hold a complete log line. `Err` means the stream ended or the
    /// device sent something malformed.
    pub fn next_line(&mut self) -> Result<Option<String>, DeviceError> {
        let symbols = symbols()?;
        let mut source = ServiceSource {
            symbols,
            connection: self.service,
        };
        let frame = read_frame(&mut source)?;
        if frame.kind != 2 {
            return Ok(None);
        }
        Ok(decode_record(&frame.payload))
    }
}

impl Drop for LogStream {
    fn drop(&mut self) {
        let Ok(symbols) = symbols() else { return };
        unsafe {
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
        assert!(
            session.exists(path),
            "the file should be there after writing"
        );
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

    #[test]
    #[ignore = "needs an iPhone attached"]
    fn the_phone_is_found_on_a_different_thread_from_the_one_that_listed_it() {
        // The window's own shape: the device list is asked for while the page is
        // loading, on one thread of the pool, and every later command runs on
        // whichever thread the pool hands out -- which is almost never the same
        // one. Notification callbacks arrive on the run loop of the thread that
        // subscribed, so a subscription made somewhere else has to be able to
        // find the phone too.
        let udid = test_device();
        let listed = list_devices().expect("discovery runs");
        assert!(!listed.is_empty(), "a phone has to be plugged in");
        println!("this thread listed {} device(s)", listed.len());

        // On a thread that has never spoken to the framework before.
        let elsewhere = std::thread::spawn({
            let udid = udid.clone();
            move || -> Result<(), DeviceError> {
                let session = AfcSession::open(&udid)?;
                drop(session);
                Ok(())
            }
        })
        .join()
        .expect("the thread finishes");
        elsewhere.expect("a session on a thread that has not listed devices");

        // And once more here, after both.
        let session = AfcSession::open(&udid).expect("a session on the thread that listed");
        drop(session);
    }

    #[test]
    #[ignore = "needs an iPhone attached"]
    fn the_phone_is_found_after_a_scan_has_run() {
        // The window scans for cards and then reads one, one after the other, in
        // the same process. The scan holds a session of its own for as long as it
        // runs, and what it leaves behind when it is told to stop must not stop
        // the next thing from finding the phone -- which is what went wrong when
        // the window was first clicked through by hand.
        let udid = test_device();

        // On this thread first: open the log stream, read a little, close it.
        let mut stream = LogStream::open(&udid).expect("the log stream opens");
        let mut read = 0;
        while read < 20 {
            match stream.next_line() {
                Ok(Some(_)) => read += 1,
                Ok(None) => continue,
                Err(error) => panic!("the stream ended after {read} lines: {error}"),
            }
        }
        drop(stream);

        let session = AfcSession::open(&udid).expect("AFC opens after a scan on this thread");
        drop(session);

        // And then the other way round: the scan runs on a thread of its own,
        // exactly as `start_scan` spawns it, and the read happens afterwards on
        // this one.
        let scanned = std::thread::spawn({
            let udid = udid.clone();
            move || -> Result<(), DeviceError> {
                let mut stream = LogStream::open(&udid)?;
                let mut read = 0;
                while read < 20 {
                    match stream.next_line()? {
                        Some(_) => read += 1,
                        None => continue,
                    }
                }
                Ok(())
            }
        })
        .join()
        .expect("the thread finishes");
        scanned.expect("a scan on a thread of its own");

        let session = AfcSession::open(&udid).expect("AFC opens after a scan on another thread");
        drop(session);
    }

    #[test]
    #[ignore = "needs an iPhone attached"]
    fn the_phone_is_found_from_every_kind_of_thread() {
        // The window asks for the phone from whichever thread its pool hands out,
        // and a pool thread is reused all day -- so "found on this thread, once"
        // is not the property that matters. Each case below is a situation the
        // window is actually in.
        let udid = test_device();

        // The one the window is in first: list what is plugged in.
        let listed = list_devices().expect("discovery runs");
        assert!(!listed.is_empty(), "a phone has to be plugged in");

        // Twice on the thread that has just listed them.
        for attempt in 1..=2 {
            let session = AfcSession::open(&udid).unwrap_or_else(|error| {
                panic!("attempt {attempt} on a thread that listed devices: {error}")
            });
            drop(session);
        }

        // A session stays on the thread that opened it -- it is deliberately not
        // `Send`, because the framework's run loop is per thread -- so each case
        // opens and closes its own and reports only whether it worked.
        let attempt = |label: &'static str, before: bool| {
            let udid = udid.clone();
            std::thread::spawn(move || -> Result<(), DeviceError> {
                if before {
                    list_devices()?;
                }
                let session = AfcSession::open(&udid)?;
                drop(session);
                let again = AfcSession::open(&udid)?;
                drop(again);
                Ok(())
            })
            .join()
            .unwrap_or_else(|_| panic!("{label}: the thread panicked"))
            .unwrap_or_else(|error| panic!("{label}: {error}"))
        };

        // A thread of its own, twice over: two commands in a row.
        attempt("a thread of its own", false);

        // And a thread that lists the devices before asking for one, which is
        // what every pool thread has done by the time a person has clicked
        // anything.
        attempt("a thread that has listed devices", true);
    }
}

/// Framing and record decoding, driven from memory instead of from a device.
///
/// `legacy/tests/test_os_trace.m` drove the Objective-C version with this same
/// synthetic record, and the same cases are repeated here, so the port is
/// checked against that fixture rather than against whatever an iPhone happens
/// to log today.
#[cfg(test)]
mod log_tests {
    use super::*;

    /// The synthetic Info-level resource lookup: a 129-byte header, then the
    /// NUL-padded process, image and message strings.
    fn log_record() -> Vec<u8> {
        let process = b"/usr/libexec/passd\0";
        let image = b"/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation\0";
        let message = concat!(
            "Resource lookup\n",
            "\tResult        : file:///var/mobile/Library/Passes/Cards/",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAA=.pkpass/en.lproj/actions.strings\0"
        )
        .as_bytes();

        let mut record = vec![0_u8; RECORD_HEADER];
        record[0] = 2;
        record[1] = 8;
        record[5] = RECORD_HEADER as u8;
        record[37] = process.len() as u8;
        record[68] = 1; // Info
        record[107] = image.len() as u8;
        record[109..113].copy_from_slice(&(message.len() as u32).to_le_bytes());
        record.extend_from_slice(process);
        record.extend_from_slice(image);
        record.extend_from_slice(message);
        record
    }

    /// One frame: the type byte, a length in that type's own byte order, payload.
    fn frame(kind: u8, payload: &[u8]) -> Vec<u8> {
        let length = payload.len() as u32;
        let coded = if kind == 1 {
            length.to_be_bytes()
        } else {
            length.to_le_bytes()
        };
        let mut wire = Vec::with_capacity(5 + payload.len());
        wire.push(kind);
        wire.extend_from_slice(&coded);
        wire.extend_from_slice(payload);
        wire
    }

    /// Wire bytes in memory, handed out in fragments so the short-read loop in
    /// `read_exact` is exercised the way a service connection exercises it.
    struct MemorySource {
        bytes: Vec<u8>,
        offset: usize,
        chunk: usize,
    }

    impl MemorySource {
        fn new(bytes: Vec<u8>, chunk: usize) -> Self {
            Self {
                bytes,
                offset: 0,
                chunk,
            }
        }
    }

    impl ByteSource for MemorySource {
        fn read(&mut self, bytes: &mut [u8]) -> Result<usize, DeviceError> {
            let available = self.bytes.len() - self.offset;
            let count = bytes.len().min(available).min(self.chunk);
            bytes[..count].copy_from_slice(&self.bytes[self.offset..self.offset + count]);
            self.offset += count;
            Ok(count)
        }
    }

    #[test]
    fn the_fixture_record_decodes_to_a_log_line() {
        let line = decode_record(&log_record()).expect("the fixture is a valid record");
        println!("{line}");
        assert!(
            line.starts_with("passd(CoreFoundation): Resource lookup\n"),
            "{line}"
        );
        assert!(
            line.contains("/Cards/AAAAAAAAAAAAAAAAAAAAAAAAAAA=.pkpass/"),
            "{line}"
        );
        assert!(line.ends_with("actions.strings\n"), "{line}");
        assert!(!line.contains('\0'), "the padding must be trimmed: {line}");
    }

    #[test]
    fn frames_use_big_endian_plists_and_little_endian_records() {
        // A 0x0102 byte reply, so the length bytes are 0x00 0x00 0x01 0x02.
        // Read little endian they would ask for 0x02010000 -- over the cap --
        // so the two byte orders cannot be confused with each other here.
        let mut reply = vec![1, 0, 0, 1, 2];
        reply.resize(5 + 0x0102, 0xAA);
        let mut source = MemorySource::new(reply, 4096);
        let plist = read_frame(&mut source).expect("a type 1 frame reads");
        assert_eq!(plist.kind, 1);
        assert_eq!(plist.payload.len(), 0x0102);

        let record = log_record();
        let wire = frame(2, &record);
        assert_eq!(wire[1], (record.len() & 0xFF) as u8);
        assert_eq!(wire[2], (record.len() >> 8) as u8);
        let mut source = MemorySource::new(wire, 4096);
        let event = read_frame(&mut source).expect("a type 2 frame reads");
        assert_eq!(event.kind, 2);
        assert_eq!(event.payload, record);
        assert!(decode_record(&event.payload).is_some());
    }

    #[test]
    fn a_fragmented_stream_decodes_both_frames() {
        // The device coalesces frames into one read and splits them inside a
        // field, so the same wire is read one byte at a time, three bytes at a
        // time and in one go.
        let reply = frame(1, &[0x5A; 64]);
        let record = log_record();
        let mut wire = reply.clone();
        wire.extend_from_slice(&frame(2, &record));

        for chunk in [1, 3, 65536] {
            let mut source = MemorySource::new(wire.clone(), chunk);
            let ack = read_frame(&mut source).expect("the reply frame reads");
            assert_eq!(ack.kind, 1);
            assert_eq!(ack.payload, reply[5..]);
            let event = read_frame(&mut source).expect("the record frame reads");
            assert_eq!(event.kind, 2);
            assert_eq!(event.payload, record);
            assert!(
                decode_record(&event.payload).is_some(),
                "chunk {chunk} must decode"
            );
            // Then the stream ends, which is an error rather than a frame.
            assert!(
                matches!(read_frame(&mut source), Err(DeviceError::LogEnded)),
                "chunk {chunk} must end"
            );
        }
    }

    #[test]
    fn invalid_frames_are_rejected() {
        // An unknown type byte, a zero length in either byte order, and a
        // length over the 16 MiB cap: each is refused before its payload is
        // allocated or waited for, and a header cut short is a truncated
        // stream rather than a frame.
        for wire in [
            vec![3, 1, 0, 0, 0],
            vec![2, 0, 0, 0, 0],
            vec![1, 0, 0, 0, 0],
            vec![2, 0xFF, 0xFF, 0xFF, 0x7F],
            vec![2, 0, 0, 0],
        ] {
            let mut source = MemorySource::new(wire.clone(), 2);
            assert!(read_frame(&mut source).is_err(), "{wire:?} must be refused");
        }

        // A payload that stops in the middle of the record.
        let mut truncated = frame(2, &log_record());
        truncated.truncate(10);
        let mut source = MemorySource::new(truncated, 1);
        assert!(matches!(
            read_frame(&mut source),
            Err(DeviceError::LogEnded)
        ));
    }

    #[test]
    fn malformed_records_are_rejected() {
        let record = log_record();
        assert!(
            decode_record(&record).is_some(),
            "the fixture has to be the reference"
        );

        // Shorter than the 129-byte header, and one byte short of the strings
        // it declares.
        assert!(decode_record(&record[..128]).is_none());
        assert!(decode_record(&record[..record.len() - 1]).is_none());

        // A header length or a message length of u32::MAX runs off the end.
        for offset in [5, 109] {
            let mut corrupt = record.clone();
            corrupt[offset..offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
            assert!(decode_record(&corrupt).is_none(), "offset {offset}");
        }

        // A header that does not claim to be an activity record.
        let mut corrupt = record.clone();
        corrupt[0] = 1;
        assert!(decode_record(&corrupt).is_none());

        // The process and message lengths are the two the format insists on.
        let mut corrupt = record.clone();
        corrupt[37..39].copy_from_slice(&0_u16.to_le_bytes());
        assert!(decode_record(&corrupt).is_none());
        let mut corrupt = record.clone();
        corrupt[109..113].copy_from_slice(&0_u32.to_le_bytes());
        assert!(decode_record(&corrupt).is_none());
    }

    #[test]
    #[ignore = "needs an iPhone attached"]
    fn the_device_log_stream_emits_lines() {
        // Two device sessions at once abort the process, so this runs with
        // `--test-threads=1` like the other device tests.
        let udid = super::afc_tests::test_device();
        let mut stream = LogStream::open(&udid).expect("the log stream opens");
        let mut lines = 0_usize;
        let mut cards = 0_usize;
        // The relay has no stop call: the stream ends when the service is
        // invalidated, and Drop does that. Read a bounded number of frames.
        for _ in 0..2000 {
            match stream.next_line() {
                Ok(Some(line)) => {
                    lines += 1;
                    if lines <= 5 {
                        println!("{line}");
                    }
                    if line.contains("/var/mobile/Library/Passes/Cards/") {
                        cards += 1;
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    println!("the stream ended after {lines} lines: {error}");
                    break;
                }
            }
            if lines >= 200 {
                break;
            }
        }
        println!("{lines} log lines read, {cards} naming a Wallet card path");
        assert!(
            lines > 0,
            "the device log stream produced no lines -- is the iPhone unlocked?"
        );
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
