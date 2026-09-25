//! The escape out of `Media`: AirTraffic, and the conduit that carries a zip in.
//!
//! Two private frameworks, `dlopen`ed the same way `MobileDevice.framework` is
//! and for the same reason -- never linked at build time, so a missing symbol is
//! a clear error rather than a build that cannot start.
//!
//! Why an escape is needed at all: AFC's root is `/var/mobile/Media`, it refuses
//! `..`, and it has no `MAKE_LINK`, so a card's files in
//! `/var/mobile/Library/Passes` are out of its reach in every direction. So the
//! phone is asked to move assets with the same AirTraffic protocol iTunes speaks,
//! and the staging directory those assets are moved *into* is filled by uploading
//! one zip to `com.apple.streaming_zip_conduit`. The phone unpacks that zip
//! itself, which is where the symlink inside it comes from: the phone's `unzip`
//! can create a link that AFC cannot.
//!
//! Two rules for callers, both learned from the previous implementation:
//!
//! * One device connection at a time. The original shipped AirTraffic and the AFC
//!   helper as separate short-lived processes, so each step had the phone to
//!   itself; an AFC session and an AirTraffic connection at the same time has
//!   never been tested. Serialise AFC, the log stream, `AirTraffic` and
//!   `ZipConduit` against each other. (Two sessions to one device abort the
//!   process rather than returning an error, which is why the device tests run
//!   with `--test-threads=1`.)
//! * `AirTraffic::move_assets` moves, it does not copy. `AssetCompleted` tells the
//!   phone the asset now lives at that path, so the bytes end up at the
//!   destination and are gone from where they were -- and the exchange gives up
//!   after the first asset arrives, which is exactly why several files are made to
//!   land inside one staging directory instead of being moved one at a time. Read
//!   the result back afterwards; never assume it arrived.

use super::*;

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Where the private framework lives on every macOS install.
const AIRTRAFFIC: &str =
    "/System/Library/PrivateFrameworks/AirTrafficHost.framework/AirTrafficHost";

/// How long one exchange may take. The original ran under `alarm(300)` with a
/// handler that `_exit`ed the process; a library has no business killing its
/// caller, so the same window is a deadline checked between reads.
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(300);

/// How long to wait before looking for a message again. The original's
/// `usleep(100000)`.
const WAIT: Duration = Duration::from_millis(100);

/// The original's `usleep(200000)` between the host info and the sync request.
const SYNC_REQUEST_GAP: Duration = Duration::from_millis(200);

/// Reads allowed for each stage of the handshake, from the original's loops.
const ALLOWED_READS: usize = 8;
const READY_READS: usize = 12;
const MANIFEST_READS: usize = 20;

/// The original's `usleep(400000)` after the first asset and `usleep(60000)`
/// between the rest: the first is the one the phone acts on.
const FIRST_ASSET_GAP: Duration = Duration::from_millis(400);
const ASSET_GAP: Duration = Duration::from_millis(60);

/// The original's `sleep(2)` before it released the connection: the phone is
/// still moving the bytes, and it should not be left talking to nobody.
const SETTLE: Duration = Duration::from_secs(2);

/// The original set this on the conduit's socket after sending an archive: the
/// phone unpacks before it answers, so the read has to wait.
const RECEIVE_TIMEOUT: Duration = Duration::from_secs(30);

/// The `AirTrafficHost` symbols. CoreFoundation is not bound here: the table in
/// the parent module already has the CF calls this needs, `CFDictionaryGetValue`
/// included, and one place for them is one place to get them right.
struct AirSymbols {
    create: unsafe extern "C" fn(CfString) -> *mut c_void,
    release: unsafe extern "C" fn(*mut c_void),
    send_host_info: unsafe extern "C" fn(*mut c_void, CfDictionary),
    send_sync_request: unsafe extern "C" fn(*mut c_void, CfType, CfDictionary, CfDictionary),
    send_metadata_sync_finished: unsafe extern "C" fn(*mut c_void, CfDictionary, CfDictionary),
    send_asset_completed: unsafe extern "C" fn(*mut c_void, CfString, CfString, CfString),
    read_message: unsafe extern "C" fn(*mut c_void) -> CfDictionary,
    message_name: unsafe extern "C" fn(CfDictionary) -> CfString,
    message_param: unsafe extern "C" fn(CfDictionary, CfString) -> CfType,
}

// Same reasoning as the parent module's table: written once, never mutated, and
// the function pointers are process-wide constants.
unsafe impl Send for AirSymbols {}
unsafe impl Sync for AirSymbols {}

static AIR_SYMBOLS: OnceLock<Result<AirSymbols, DeviceError>> = OnceLock::new();

fn air_symbols() -> Result<&'static AirSymbols, DeviceError> {
    AIR_SYMBOLS
        .get_or_init(|| {
            let path = CString::new(AIRTRAFFIC).expect("a fixed path");
            let handle = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_NOW) };
            if handle.is_null() {
                let detail = unsafe {
                    CStr::from_ptr(libc::dlerror())
                        .to_string_lossy()
                        .into_owned()
                };
                return Err(DeviceError::Airtraffic {
                    detail: format!("could not load {AIRTRAFFIC}: {detail}"),
                });
            }
            Ok(AirSymbols {
                create: function(handle, "ATHostConnectionCreate")?,
                release: function(handle, "ATHostConnectionRelease")?,
                send_host_info: function(handle, "ATHostConnectionSendHostInfo")?,
                send_sync_request: function(handle, "ATHostConnectionSendSyncRequest")?,
                send_metadata_sync_finished: function(
                    handle,
                    "ATHostConnectionSendMetadataSyncFinished",
                )?,
                send_asset_completed: function(handle, "ATHostConnectionSendAssetCompleted")?,
                read_message: function(handle, "ATHostConnectionReadMessage")?,
                message_name: function(handle, "ATCFMessageGetName")?,
                message_param: function(handle, "ATCFMessageGetParam")?,
            })
        })
        .as_ref()
        .map_err(Clone::clone)
}

/// A `CFDictionary` under construction.
///
/// `CFDictionaryCreate` retains its keys and values, so anything built here is
/// released again once the dictionary has been taken. What the framework already
/// owns -- `kCFBooleanFalse` and the like -- is handed over with `shared` instead,
/// because releasing a shared constant would end its life for the whole process.
struct Dictionary<'a> {
    symbols: &'a Symbols,
    keys: Vec<CfString>,
    values: Vec<CfType>,
    /// Values this code created, and so must release.
    built: Vec<CfType>,
    /// The C strings the keys and values were made from. `CFStringCreateWithCString`
    /// copies, but holding them until the dictionary is taken costs nothing and
    /// keeps that assumption out of the rest of the module.
    backing: Vec<CString>,
}

impl<'a> Dictionary<'a> {
    fn new(symbols: &'a Symbols) -> Self {
        Self {
            symbols,
            keys: Vec::new(),
            values: Vec::new(),
            built: Vec::new(),
            backing: Vec::new(),
        }
    }

    /// Adds a string value, built from `text`.
    fn text(&mut self, key: &str, text: &str) -> Option<()> {
        let (value, value_backing) = cf_string(self.symbols, text)?;
        self.built.push(value);
        self.backing.push(value_backing);
        self.key(key)?;
        self.values.push(value);
        Some(())
    }

    /// Adds a value this code created. The dictionary takes its own reference, so
    /// this one is released with the dictionary.
    fn owned(&mut self, key: &str, value: CfType) -> Option<()> {
        if value.is_null() {
            return None;
        }
        self.built.push(value);
        self.key(key)?;
        self.values.push(value);
        Some(())
    }

    /// Adds a value the framework owns, which is never released here.
    fn shared(&mut self, key: &str, value: CfType) -> Option<()> {
        if value.is_null() {
            return None;
        }
        self.key(key)?;
        self.values.push(value);
        Some(())
    }

    fn key(&mut self, key: &str) -> Option<()> {
        let (reference, key_backing) = cf_string(self.symbols, key)?;
        self.keys.push(reference);
        self.backing.push(key_backing);
        Some(())
    }

    /// The dictionary, with every reference this builder held dropped.
    fn take(self) -> Option<CfType> {
        let dictionary = unsafe {
            (self.symbols.cf_dictionary_create)(
                std::ptr::null(),
                self.keys.as_ptr(),
                self.values.as_ptr(),
                self.keys.len() as CfIndex,
                self.symbols.k_dictionary_key_callbacks,
                self.symbols.k_dictionary_value_callbacks,
            )
        };
        for reference in &self.keys {
            unsafe { (self.symbols.cf_release)(*reference) };
        }
        for value in &self.built {
            unsafe { (self.symbols.cf_release)(*value) };
        }
        drop(self.backing);
        if dictionary.is_null() {
            None
        } else {
            Some(dictionary)
        }
    }
}

/// A `CFArray` holding `items`, as an owned reference.
///
/// The type callbacks matter: without them the array would not retain what it is
/// given, and the values would be released while the phone still needed them.
fn cf_array(symbols: &Symbols, items: &[CfType]) -> Option<CfType> {
    let array = unsafe {
        (symbols.cf_array_create)(
            std::ptr::null(),
            items.as_ptr(),
            items.len() as CfIndex,
            symbols.k_array_callbacks,
        )
    };
    if array.is_null() {
        None
    } else {
        Some(array)
    }
}

/// A one-element array holding `Book`: the only dataclass this escape syncs, and
/// what both `SyncedDataclasses` and `SyncedAssetTypes` carry.
fn book_array(symbols: &Symbols) -> Option<CfType> {
    let (book, _book_backing) = cf_string(symbols, "Book")?;
    let array = cf_array(symbols, &[book]);
    unsafe { (symbols.cf_release)(book) };
    array
}

/// An empty dictionary: the anchors of a first sync, and the metadata of a
/// dataclass whose assets are already all on the phone.
fn empty_dictionary(symbols: &Symbols) -> Option<CfType> {
    Dictionary::new(symbols).take()
}

/// The real macOS product version, such as `15.1`.
///
/// The original sent `NSProcessInfo.operatingSystemVersionString`, which is
/// `"Version 15.1.1 (Build 24B91)"`; the marketing version is what a phone
/// matches on, and it is one sysctl away. `"unknown"` rather than a panic if the
/// sysctl is ever absent: the host info is not worth failing the escape over.
fn macos_version() -> String {
    let name = b"kern.osproductversion\0";
    let mut buffer = [0_u8; 64];
    let mut size = buffer.len();
    // SAFETY: `name` is NUL-terminated, and `size` starts as the real length of
    // the buffer `sysctlbyname` is allowed to fill.
    let status = unsafe {
        libc::sysctlbyname(
            name.as_ptr() as *const c_char,
            buffer.as_mut_ptr() as *mut c_void,
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if status != 0 {
        return "unknown".to_owned();
    }
    let end = buffer.iter().position(|byte| *byte == 0).unwrap_or(size);
    String::from_utf8_lossy(&buffer[..end.min(buffer.len())]).into_owned()
}

/// A fresh library identifier, formatted the way `NSUUID.UUIDString` is.
///
/// It has to be new for every exchange: the phone reads it as the identity of the
/// library that is syncing, and seeing one it already knows makes it think the
/// previous sync is still running.
fn library_id() -> String {
    let mut bytes = [0_u8; 16];
    // SAFETY: `getentropy` fills exactly the buffer it is given, or fails.
    let status = unsafe { libc::getentropy(bytes.as_mut_ptr() as *mut c_void, bytes.len()) };
    if status != 0 {
        // Only oversized requests can fail; a timestamp still tells two exchanges
        // apart, which is all the identifier is for.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos() as u64)
            .unwrap_or(0);
        bytes[..8].copy_from_slice(&nanos.to_le_bytes());
        bytes[8..].copy_from_slice(&std::process::id().to_le_bytes().repeat(2));
    }
    // Version 4, variant 1, the way a UUID is meant to look.
    bytes[6] = (bytes[6] & 0x0F) | 0x40;
    bytes[8] = (bytes[8] & 0x3F) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02X}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// The `HostInfo` an iTunes-style host sends.
///
/// The version strings are the ones the original sent and are what makes the
/// phone treat this as an iTunes 13.7 host rather than something it does not
/// recognise.
fn host_info(symbols: &Symbols) -> Option<CfType> {
    let version = macos_version();
    let library = library_id();
    let mut host = Dictionary::new(symbols);
    host.text("Type", "iTunes")?;
    host.text("Version", "13.7.0.161")?;
    host.text("MacOSVersion", version.as_str())?;
    host.text("SyncHostName", "airlift")?;
    host.text("LibraryID", library.as_str())?;
    host.owned("SyncedDataclasses", book_array(symbols)?)?;
    host.owned("SyncedAssetTypes", book_array(symbols)?)?;
    host.shared("Wakeable", symbols.k_cf_false)?;
    host.take()
}

/// `{"Book": 1}`: the metadata for the one dataclass is complete.
fn sync_types(symbols: &Symbols) -> Option<CfType> {
    let number = cf_number(symbols, 1);
    if number.is_null() {
        return None;
    }
    let mut types = Dictionary::new(symbols);
    types.owned("Book", number)?;
    types.take()
}

/// `{"MediaSubdir": …}`: the one parameter the streaming zip conduit takes.
fn zip_request(symbols: &Symbols, media_subdir: &str) -> Option<CfType> {
    let mut request = Dictionary::new(symbols);
    request.text("MediaSubdir", media_subdir)?;
    request.take()
}

/// The message's name, which is what every stage here switches on.
fn message_name(air: &AirSymbols, symbols: &Symbols, message: CfDictionary) -> String {
    read_cf_string(symbols, unsafe { (air.message_name)(message) })
}

/// A parameter of the message, borrowed from it and so only valid while it lives.
fn message_param(
    air: &AirSymbols,
    symbols: &Symbols,
    message: CfDictionary,
    key: &str,
) -> Option<CfType> {
    let (key, _key_backing) = cf_string(symbols, key)?;
    let value = unsafe { (air.message_param)(message, key) };
    unsafe { (symbols.cf_release)(key) };
    if value.is_null() {
        None
    } else {
        Some(value)
    }
}

/// The value under `key`, borrowed from `dictionary`.
fn dictionary_value(symbols: &Symbols, dictionary: CfType, key: &str) -> CfType {
    let Some((key, _key_backing)) = cf_string(symbols, key) else {
        return std::ptr::null();
    };
    let value = unsafe { (symbols.cf_dictionary_get_value)(dictionary, key) };
    unsafe { (symbols.cf_release)(key) };
    value
}

/// Whether a value the phone sent is of a kind that may be walked. Calling a
/// `CFArray` function on a `CFString` is undefined behaviour rather than an
/// error, and the phone is the one that wrote this dictionary.
fn is_kind(symbols: &Symbols, value: CfType, kind: unsafe extern "C" fn() -> usize) -> bool {
    if value.is_null() {
        return false;
    }
    unsafe { (symbols.cf_get_type_id)(value) == kind() }
}

/// Whether the manifest lists `identifier` as a `Book` that may be downloaded.
fn manifest_contains(symbols: &Symbols, manifest: CfType, identifier: &str) -> bool {
    let books = dictionary_value(symbols, manifest, "Book");
    if !is_kind(symbols, books, symbols.cf_array_type_id) {
        return false;
    }
    let count = unsafe { (symbols.cf_array_get_count)(books) };
    for index in 0..count {
        let entry = unsafe { (symbols.cf_array_get_value)(books, index) };
        if !is_kind(symbols, entry, symbols.cf_dictionary_type_id) {
            continue;
        }
        let asset = dictionary_value(symbols, entry, "AssetID");
        if asset.is_null() || read_cf_string(symbols, asset) != identifier {
            continue;
        }
        // A plist boolean comes back as the shared `kCFBooleanTrue`, so identity
        // is the test -- and it is the test that only accepts a real "yes".
        if dictionary_value(symbols, entry, "IsDownload") == symbols.k_cf_true {
            return true;
        }
    }
    false
}

/// True once this exchange has had its 300 seconds.
fn spent(deadline: Instant) -> bool {
    deadline.elapsed() >= EXCHANGE_TIMEOUT
}

/// Reads messages until one named `wanted` arrives, dropping the rest.
///
/// A read that returns nothing means the phone is still thinking, which is the
/// original's `usleep(100000)`; a message with any other name is neither an error
/// nor an answer, so it is dropped and the next one read. `Ok(false)` means the
/// reads ran out.
fn read_until(
    air: &AirSymbols,
    symbols: &Symbols,
    connection: *mut c_void,
    wanted: &'static str,
    reads: usize,
    deadline: Instant,
) -> Result<bool, DeviceError> {
    for _ in 0..reads {
        if spent(deadline) {
            return Err(DeviceError::Handshake { stage: wanted });
        }
        let message = unsafe { (air.read_message)(connection) };
        if message.is_null() {
            std::thread::sleep(WAIT);
            continue;
        }
        let name = message_name(air, symbols, message);
        unsafe { (symbols.cf_release)(message) };
        if name == wanted {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Reads until the phone sends its asset manifest, or says why it will not.
///
/// `SyncFailed` and `SyncFinished` end the exchange, the same way they stop the
/// original's loop: there is no manifest after either. The returned dictionary is
/// owned by the caller.
fn read_manifest(
    air: &AirSymbols,
    symbols: &Symbols,
    connection: *mut c_void,
    deadline: Instant,
) -> Result<Option<CfType>, DeviceError> {
    for _ in 0..MANIFEST_READS {
        if spent(deadline) {
            return Err(DeviceError::Handshake {
                stage: "AssetManifest",
            });
        }
        let message = unsafe { (air.read_message)(connection) };
        if message.is_null() {
            std::thread::sleep(WAIT);
            continue;
        }
        let name = message_name(air, symbols, message);
        let manifest = if name == "AssetManifest" {
            message_param(air, symbols, message, "AssetManifest")
                // Retained because the message owns it and is about to go.
                .map(|value| unsafe { (symbols.cf_retain)(value) })
        } else {
            None
        };
        let stopped = name == "SyncFailed" || name == "SyncFinished";
        unsafe { (symbols.cf_release)(message) };
        if let Some(manifest) = manifest {
            return Ok(Some(manifest));
        }
        if stopped {
            break;
        }
    }
    Ok(None)
}

/// Tells the phone one asset has been dealt with, and where it now lives.
fn send_asset_completed(
    air: &AirSymbols,
    symbols: &Symbols,
    connection: *mut c_void,
    identifier: &str,
    destination: &str,
) -> Result<(), DeviceError> {
    let (asset, _asset_backing) =
        cf_string(symbols, identifier).ok_or_else(|| DeviceError::Request {
            detail: format!("{identifier} cannot be named to AirTraffic"),
        })?;
    let (dataclass, _dataclass_backing) =
        cf_string(symbols, "Book").ok_or_else(|| DeviceError::Request {
            detail: "Book cannot be named to AirTraffic".to_owned(),
        })?;
    let (path, _path_backing) =
        cf_string(symbols, destination).ok_or_else(|| DeviceError::Request {
            detail: format!("{destination} cannot be named to AirTraffic"),
        })?;
    unsafe { (air.send_asset_completed)(connection, asset, dataclass, path) };
    unsafe {
        (symbols.cf_release)(asset);
        (symbols.cf_release)(dataclass);
        (symbols.cf_release)(path);
    }
    Ok(())
}

/// A live `AirTrafficHost` connection to one iPhone.
///
/// The connection is the exchange: the phone offers it `SyncAllowed` when it
/// accepts, and it is released once the assets have been named. See the module
/// notes for what that means about serialising against AFC, and for the read-back
/// rule after a move.
pub struct AirTraffic {
    /// Null once the exchange has finished: the original releases the connection
    /// at the end of the move rather than at the end of the process, and
    /// `move_assets` takes `&self`, so the slot has to be replaceable.
    connection: Cell<*mut c_void>,
    /// When this connection's 300 seconds began.
    deadline: Instant,
}

impl AirTraffic {
    /// Opens AirTraffic on one iPhone and waits for `SyncAllowed`.
    ///
    /// The phone sends `SyncAllowed` unprompted once it has taken the connection,
    /// and everything after that is the host's turn. Up to eight reads, a tenth
    /// of a second apart, is what the original allowed -- it is also the whole of
    /// what can be checked without touching an asset, so it is where a smoke test
    /// stops.
    pub fn open(udid: &str) -> Result<Self, DeviceError> {
        let air = air_symbols()?;
        let symbols = symbols()?;
        let Some((identifier, _identifier_backing)) = cf_string(symbols, udid) else {
            return Err(DeviceError::Airtraffic {
                detail: format!("{udid} cannot be named to AirTraffic"),
            });
        };
        let connection = unsafe { (air.create)(identifier) };
        unsafe { (symbols.cf_release)(identifier) };
        if connection.is_null() {
            return Err(DeviceError::Airtraffic {
                detail: format!("AirTraffic would not open {udid}"),
            });
        }

        let this = Self {
            connection: Cell::new(connection),
            deadline: Instant::now(),
        };
        match read_until(
            air,
            symbols,
            connection,
            "SyncAllowed",
            ALLOWED_READS,
            this.deadline,
        ) {
            Ok(true) => Ok(this),
            // Every way out of a failed open releases the connection, so a phone
            // is never left holding a sync session for an object nobody has.
            Ok(false) => {
                this.close();
                Err(DeviceError::Handshake {
                    stage: "SyncAllowed",
                })
            }
            Err(error) => {
                this.close();
                Err(error)
            }
        }
    }

    /// Moves the named assets and reports each one the phone accepts.
    ///
    /// Each pair is `(identifier, destination)`: the identifier the phone knows
    /// the asset by, and the path it should end up at. `progress` is called after
    /// every asset but the first, with the index, the number of assets after the
    /// first and the destination's leaf name -- the first asset is the one that
    /// changes the phone's state, and the original counted progress against the
    /// rest. The connection is released on the way out, so a second call reports
    /// that it is closed rather than talking to a released object.
    pub fn move_assets(
        &self,
        assets: &[(String, String)],
        progress: &mut dyn FnMut(usize, usize, &str),
    ) -> Result<(), DeviceError> {
        match self.exchange(assets, progress) {
            Ok(()) => {
                std::thread::sleep(SETTLE);
                self.close();
                Ok(())
            }
            Err(error) => {
                self.close();
                Err(error)
            }
        }
    }

    /// The handshake, the manifest check and one message per asset.
    fn exchange(
        &self,
        assets: &[(String, String)],
        progress: &mut dyn FnMut(usize, usize, &str),
    ) -> Result<(), DeviceError> {
        let connection = self.live()?;
        let air = air_symbols()?;
        let symbols = symbols()?;
        self.handshake(air, symbols, connection, assets)?;

        let total = assets.len().saturating_sub(1);
        for (index, (identifier, destination)) in assets.iter().enumerate() {
            send_asset_completed(air, symbols, connection, identifier, destination)?;
            if index > 0 {
                progress(index, total, last_component(destination));
            }
            if index + 1 < assets.len() {
                std::thread::sleep(if index == 0 {
                    FIRST_ASSET_GAP
                } else {
                    ASSET_GAP
                });
            }
        }
        Ok(())
    }

    /// Everything the original did between `SyncAllowed` and the first asset.
    ///
    /// The phone reads these in order and answers one message at a time, so the
    /// order is the protocol and not a preference: host info, sync request,
    /// `ReadyForSync`, metadata finished, manifest.
    fn handshake(
        &self,
        air: &AirSymbols,
        symbols: &Symbols,
        connection: *mut c_void,
        assets: &[(String, String)],
    ) -> Result<(), DeviceError> {
        let Some(host) = host_info(symbols) else {
            return Err(DeviceError::Request {
                detail: "the host info could not be built".to_owned(),
            });
        };
        unsafe { (air.send_host_info)(connection, host) };
        std::thread::sleep(SYNC_REQUEST_GAP);

        let outcome = self.request_sync(air, symbols, connection, host, assets);
        unsafe { (symbols.cf_release)(host) };
        outcome
    }

    /// Asks for the one dataclass this escape uses, then walks the phone to its
    /// manifest and checks the assets asked for are in it.
    fn request_sync(
        &self,
        air: &AirSymbols,
        symbols: &Symbols,
        connection: *mut c_void,
        host: CfType,
        assets: &[(String, String)],
    ) -> Result<(), DeviceError> {
        let Some(dataclasses) = book_array(symbols) else {
            return Err(DeviceError::Request {
                detail: "the dataclass array could not be built".to_owned(),
            });
        };
        let Some(anchors) = empty_dictionary(symbols) else {
            unsafe { (symbols.cf_release)(dataclasses) };
            return Err(DeviceError::Request {
                detail: "the anchor dictionary could not be built".to_owned(),
            });
        };
        unsafe { (air.send_sync_request)(connection, dataclasses, anchors, host) };
        unsafe {
            (symbols.cf_release)(dataclasses);
            (symbols.cf_release)(anchors);
        }

        if !read_until(
            air,
            symbols,
            connection,
            "ReadyForSync",
            READY_READS,
            self.deadline,
        )? {
            return Err(DeviceError::Handshake {
                stage: "ReadyForSync",
            });
        }

        // `Book` is done: the phone reads this as the moment to send its manifest
        // of what it has, and a manifest that lists the assets as downloadable is
        // the permission this whole exchange exists to get.
        let Some(metadata) = sync_types(symbols) else {
            return Err(DeviceError::Request {
                detail: "the metadata sync reply could not be built".to_owned(),
            });
        };
        let Some(finished_anchors) = empty_dictionary(symbols) else {
            unsafe { (symbols.cf_release)(metadata) };
            return Err(DeviceError::Request {
                detail: "the anchor dictionary could not be built".to_owned(),
            });
        };
        unsafe { (air.send_metadata_sync_finished)(connection, metadata, finished_anchors) };
        unsafe {
            (symbols.cf_release)(metadata);
            (symbols.cf_release)(finished_anchors);
        }

        let manifest = read_manifest(air, symbols, connection, self.deadline)?;
        let missing = assets
            .iter()
            .filter(|(identifier, _)| {
                !manifest_contains(symbols, manifest.unwrap_or(std::ptr::null()), identifier)
            })
            .count();
        if let Some(manifest) = manifest {
            unsafe { (symbols.cf_release)(manifest) };
        }
        if missing > 0 {
            return Err(DeviceError::AssetsMissing { count: missing });
        }
        Ok(())
    }

    /// The live connection, or a clear error when the exchange let it go.
    fn live(&self) -> Result<*mut c_void, DeviceError> {
        let connection = self.connection.get();
        if connection.is_null() {
            return Err(DeviceError::Airtraffic {
                detail: "the connection has already been closed".to_owned(),
            });
        }
        Ok(connection)
    }

    /// Releases the phone's side of the connection once; a second call does
    /// nothing.
    fn close(&self) {
        let connection = self.connection.replace(std::ptr::null_mut());
        if connection.is_null() {
            return;
        }
        if let Ok(air) = air_symbols() {
            unsafe { (air.release)(connection) };
        }
    }
}

impl Drop for AirTraffic {
    fn drop(&mut self) {
        // A connection that was opened and never used still holds a sync session
        // on the phone, so it is released here as well as at the end of a move.
        self.close();
    }
}

/// A live `com.apple.streaming_zip_conduit` connection to one iPhone.
///
/// This is how a directory under `Media` is filled in one piece: the phone
/// unpacks the zip itself, so the archive can carry things AFC cannot create --
/// a symlink above all. Same one-device-connection-at-a-time rule as the rest of
/// this module.
pub struct ZipConduit {
    device: DeviceHandle,
    service: CfType,
}

impl ZipConduit {
    /// Opens the zip conduit on one iPhone.
    pub fn open(udid: &str) -> Result<Self, DeviceError> {
        let (device, service) = open_service(udid, "com.apple.streaming_zip_conduit")?;
        Ok(Self { device, service })
    }

    /// Uploads one zip into a directory under `Media`, for the phone to unpack.
    ///
    /// `media_subdir` is relative to `Media` and is the directory that will hold
    /// what the archive contains -- the staging name this crate generated, which
    /// nothing here validates: the caller owns the naming, as the original's
    /// `GeneratedNamesMatch` did. The archive goes as bytes, in whatever form the
    /// caller zipped it; nothing is re-encoded.
    pub fn upload(&self, media_subdir: &str, archive: &[u8]) -> Result<(), DeviceError> {
        let symbols = symbols()?;

        let Some(request) = zip_request(symbols, media_subdir) else {
            return Err(DeviceError::Request {
                detail: "the zip request could not be built".to_owned(),
            });
        };
        let status = unsafe { (symbols.service_send_message)(self.service, request, BINARY_PLIST) };
        unsafe { (symbols.cf_release)(request) };
        if status != 0 {
            return Err(DeviceError::ZipConduit { status });
        }

        // The original's `SendAll`: a service connection writes as much as it
        // likes and reports how much, so a short write is normal and the rest
        // goes in the next call.
        let mut sent = 0_usize;
        while sent < archive.len() {
            let count = unsafe {
                (symbols.service_send)(
                    self.service,
                    archive[sent..].as_ptr() as *const c_void,
                    archive.len() - sent,
                )
            };
            if count <= 0 {
                return Err(DeviceError::ZipConduit { status: count });
            }
            sent += count as usize;
        }

        // The phone unpacks the archive before it answers, which can take longer
        // than a socket read waits by default -- and an empty read here would look
        // like a refusal. Thirty seconds is what the original allowed.
        let timeout = libc::timeval {
            tv_sec: RECEIVE_TIMEOUT.as_secs() as libc::time_t,
            tv_usec: 0,
        };
        // SAFETY: a live socket from the service connection, and a `timeval` of
        // the length that is passed with it.
        unsafe {
            libc::setsockopt(
                (symbols.service_socket)(self.service),
                libc::SOL_SOCKET,
                libc::SO_RCVTIMEO,
                &timeout as *const libc::timeval as *const c_void,
                std::mem::size_of::<libc::timeval>() as libc::socklen_t,
            );
        }

        // One reply, whatever it says: the original read exactly one message and
        // reported its status.
        let mut reply: CfType = std::ptr::null();
        let mut format: CfIndex = BINARY_PLIST as CfIndex;
        let status =
            unsafe { (symbols.service_receive_message)(self.service, &mut reply, &mut format) };
        if reply.is_null() && status == 0 {
            return Err(DeviceError::ZipConduitReply);
        }
        if !reply.is_null() {
            unsafe { (symbols.cf_release)(reply) };
        }
        if status != 0 {
            return Err(DeviceError::ZipConduit { status });
        }
        Ok(())
    }
}

impl Drop for ZipConduit {
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
mod tests {
    use super::*;

    #[test]
    fn the_airtraffic_framework_and_its_core_foundation_helps_are_there() {
        // AirTrafficHost is private but part of macOS, and `air_symbols` fails on
        // the first symbol that is missing -- so resolving at all is the check. A
        // symbol that moved would otherwise first show up on a phone, in the
        // middle of a sync.
        air_symbols().expect("AirTrafficHost.framework is part of macOS");

        // The CoreFoundation calls this module added to the parent's table, the way
        // the rest of the handshake uses them: an array of nothing, of the right
        // kind, that can be counted.
        let symbols = symbols().expect("MobileDevice.framework is part of macOS");
        let array = cf_array(symbols, &[]).expect("CFArrayCreate resolves");
        assert!(is_kind(symbols, array, symbols.cf_array_type_id));
        assert_eq!(unsafe { (symbols.cf_array_get_count)(array) }, 0);
        unsafe { (symbols.cf_release)(array) };
    }

    #[test]
    fn a_manifest_is_read_the_way_the_phone_sends_it() {
        // The manifest is the phone's permission to move an asset, so it is built
        // here the way the phone builds it: an array of dictionaries, each with an
        // identifier and an `IsDownload` boolean.
        let symbols = symbols().expect("MobileDevice.framework is part of macOS");

        let mut wanted = Dictionary::new(symbols);
        wanted
            .text("AssetID", "downloadable")
            .expect("the fixture builds");
        wanted
            .shared("IsDownload", symbols.k_cf_true)
            .expect("the fixture builds");
        let wanted = wanted.take().expect("the fixture builds");

        let mut held = Dictionary::new(symbols);
        held.text("AssetID", "not-downloadable")
            .expect("the fixture builds");
        held.shared("IsDownload", symbols.k_cf_false)
            .expect("the fixture builds");
        let held = held.take().expect("the fixture builds");

        let books = cf_array(symbols, &[wanted, held]).expect("the fixture builds");
        unsafe {
            (symbols.cf_release)(wanted);
            (symbols.cf_release)(held);
        }

        let mut manifest = Dictionary::new(symbols);
        manifest.owned("Book", books).expect("the fixture builds");
        let manifest = manifest.take().expect("the fixture builds");

        assert!(manifest_contains(symbols, manifest, "downloadable"));
        assert!(
            !manifest_contains(symbols, manifest, "not-downloadable"),
            "an asset the phone will not download is not an asset to move"
        );
        assert!(!manifest_contains(symbols, manifest, "absent"));
        // A manifest of the wrong shape is refused rather than walked.
        assert!(!manifest_contains(symbols, manifest, ""));

        let mut wrong = Dictionary::new(symbols);
        wrong
            .text("Book", "not an array")
            .expect("the fixture builds");
        let wrong = wrong.take().expect("the fixture builds");
        assert!(!manifest_contains(symbols, wrong, "downloadable"));

        unsafe {
            (symbols.cf_release)(manifest);
            (symbols.cf_release)(wrong);
        }
    }

    #[test]
    fn the_host_info_carries_the_names_it_has_to() {
        let symbols = symbols().expect("MobileDevice.framework is part of macOS");
        let host = host_info(symbols).expect("the host info builds");
        for (key, expected) in [("Type", "iTunes"), ("SyncHostName", "airlift")] {
            let value = dictionary_value(symbols, host, key);
            assert_eq!(read_cf_string(symbols, value), expected, "{key}");
        }
        let dataclasses = dictionary_value(symbols, host, "SyncedDataclasses");
        assert!(is_kind(symbols, dataclasses, symbols.cf_array_type_id));
        assert_eq!(unsafe { (symbols.cf_array_get_count)(dataclasses) }, 1);
        assert_eq!(
            unsafe { (symbols.cf_array_get_count)(cf_books(symbols, host)) },
            1
        );
        assert!(dictionary_value(symbols, host, "Wakeable") == symbols.k_cf_false);
        assert!(!dictionary_value(symbols, host, "LibraryID").is_null());
        unsafe { (symbols.cf_release)(host) };
    }

    /// The `Book` array inside a host info dictionary, for the test above.
    fn cf_books(symbols: &Symbols, host: CfType) -> CfType {
        dictionary_value(symbols, host, "SyncedAssetTypes")
    }

    #[test]
    fn the_library_identifier_is_a_fresh_uuid() {
        let first = library_id();
        let second = library_id();
        assert_ne!(
            first, second,
            "the phone reads it as the library's identity"
        );
        assert_eq!(first.len(), 36, "{first}");
        let groups: Vec<usize> = first.split('-').map(str::len).collect();
        assert_eq!(groups, vec![8, 4, 4, 4, 12], "{first}");
        assert!(
            first
                .chars()
                .all(|c| c.is_ascii_hexdigit() || c == '-' || c.is_ascii_uppercase()),
            "{first}"
        );
        let version = macos_version();
        assert!(
            version.chars().next().is_some_and(|c| c.is_ascii_digit()),
            "the product version has to be the real one, not {version}"
        );
    }

    #[test]
    #[ignore = "needs an iPhone attached"]
    fn the_airtraffic_handshake_starts() {
        // Only the connection and the phone's own `SyncAllowed`: nothing is asked
        // of the phone and no asset is named, so this moves nothing. `move_assets`
        // is deliberately not called here -- it moves real assets, and the phone
        // gives up after the first one arrives.
        let udid = super::super::afc_tests::test_device();
        let connection = AirTraffic::open(&udid).expect("AirTraffic takes the connection");
        println!("SyncAllowed observed from {udid}");
        drop(connection);
    }
}
