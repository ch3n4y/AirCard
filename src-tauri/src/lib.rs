//! The commands the window calls.
//!
//! Deliberately thin: the rules live in `aircard-core` (saved originals, card
//! artwork, the card list) and `aircard-device` (the phone), and both are tested
//! without a window or a device attached. What lives here is the wiring, plus the
//! two things only the app can know: which operation is running, and that only
//! one of them may hold a conversation with the phone at a time.
//!
//! Every message that reaches a person is Chinese. Every message that reaches the
//! log is English. The reason behind a failure is worth keeping -- and worth
//! keeping out of the window, which is not where anybody debugs a phone.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aircard_core::artwork::{self, ArtworkCache};
use aircard_core::assets::BACKED_UP_ASSETS;
use aircard_core::{BackupStore, CardStore};
use aircard_device::{card_directory, Device};
use base64::Engine;
use serde::Serialize;
use tauri::State;

/// What to say when the phone is already busy.
const BUSY: &str = "有操作正在进行中。请等它结束再试。";

/// How often a running scan publishes what it has read.
const SCAN_PUBLISH: Duration = Duration::from_millis(250);

/// How many lines of the log the window shows.
const LOG_TAIL_LINES: usize = 200;

/// The size of a thumbnail, in pixels along its longest side.
const THUMBNAIL: u32 = 480;

/// The size of the picture shown in the flash panel, which is looked at closely.
const PREVIEW: u32 = 900;

#[derive(Serialize)]
struct AppPaths {
    backups: String,
    artwork_cache: String,
    log_file: String,
    cards_file: String,
}

/// One card as the window shows it.
#[derive(Serialize)]
struct CardView {
    hash: String,
    has_original: bool,
    has_artwork: bool,
    seen_in_last_scan: bool,
}

/// What happened to one card in a flash.
#[derive(Serialize)]
struct FlashResult {
    card: String,
    ok: bool,
    message: String,
}

/// Something an interrupted run left on the phone.
#[derive(Serialize)]
struct LeftoverView {
    name: String,
    ours: bool,
}

#[derive(Default, Serialize)]
struct ScanView {
    running: bool,
    lines_read: u64,
    found: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    problem: Option<String>,
}

/// What the window and the scan share.
struct App {
    /// Raised while any operation is holding the phone.
    ///
    /// Not politeness: two sessions to one device abort the process, and the
    /// escape moves real files across a security boundary, so the app asks the
    /// phone for one conversation at a time and refuses the rest with [`BUSY`].
    phone: Arc<AtomicBool>,
    /// Raised to ask a running scan to stop.
    stop: Arc<AtomicBool>,
    /// What a scan has read and found. Shared with the thread running it, which
    /// is why it is behind an `Arc`: the thread outlives the command that
    /// started it.
    scan: Arc<Mutex<ScanView>>,
}

impl App {
    fn new() -> Self {
        Self {
            phone: Arc::new(AtomicBool::new(false)),
            stop: Arc::new(AtomicBool::new(false)),
            scan: Arc::new(Mutex::new(ScanView::default())),
        }
    }
}

impl App {
    /// The shared handles, so device work can move to a thread of its own.
    fn shared(&self) -> Self {
        Self {
            phone: Arc::clone(&self.phone),
            stop: Arc::clone(&self.stop),
            scan: Arc::clone(&self.scan),
        }
    }
}

/// Runs device work on a thread of its own.
///
/// Not a detail. The framework delivers device notifications to the run loop of
/// the thread that subscribed, and a synchronous Tauri command runs on the main
/// thread -- whose run loop belongs to AppKit and is already being run by it. Ask
/// from there and the run loop comes back at once without waiting for anything:
/// an empty device list, and "no iPhone is reachable" for a phone sitting on the
/// cable. The same call from a thread of its own works, which is why the scan --
/// the only device operation that spawns a thread -- was the only one that ever
/// succeeded.
fn off_the_main_thread<T, F>(work: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    match std::thread::spawn(work).join() {
        Ok(result) => result,
        Err(_) => {
            log_line("a device operation panicked on its own thread");
            Err("操作未能完成。请查看日志。".to_owned())
        }
    }
}

/// Holds the phone for as long as it lives.
#[derive(Debug)]
struct Turn(Arc<AtomicBool>);

impl Turn {
    fn take(app: &App) -> Result<Self, String> {
        if app.phone.swap(true, Ordering::SeqCst) {
            return Err(BUSY.to_owned());
        }
        Ok(Self(Arc::clone(&app.phone)))
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

fn as_text(path: PathBuf) -> String {
    path.to_string_lossy().into_owned()
}

fn file_store() -> CardStore {
    CardStore::standard()
}

fn backup_store() -> BackupStore {
    BackupStore::new(aircard_core::backups_root())
}

fn artwork_cache() -> ArtworkCache {
    ArtworkCache::default_root()
}

fn data_url(png: &[u8]) -> String {
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    )
}

/// Appends one line to the app's log.
///
/// English on purpose: the log is what somebody reads with a terminal open, and
/// translating a failure into the language of the moment makes it harder to hand
/// on, not easier to read.
fn log_line(message: &str) {
    let path = aircard_core::log_file();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        use std::io::Write;
        let _ = writeln!(file, "{} {}", utc_stamp(), message);
    }
}

/// A failure a person can read, with the reason kept in the log.
fn failed(message: &str, detail: impl std::fmt::Display) -> String {
    log_line(&format!("failed: {message} | {detail}"));
    message.to_owned()
}

/// The whole list, with the two facts that need the phone to have been asked
/// before: whether the original is saved, and whether a face is kept.
fn list_cards(udid: &str) -> Vec<CardView> {
    let store = file_store();
    let backups = backup_store();
    let cache = artwork_cache();
    let seen = store.seen(udid);
    store
        .cards()
        .into_iter()
        .map(|hash| CardView {
            has_original: backups.contains(udid, &hash),
            has_artwork: cache.contains(udid, &hash),
            seen_in_last_scan: seen.iter().any(|found| found == &hash),
            hash,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Devices and paths
// ---------------------------------------------------------------------------

/// Where AirCard keeps things, so the window can show them and a report can name
/// them.
#[tauri::command]
fn app_paths() -> AppPaths {
    AppPaths {
        backups: as_text(aircard_core::backups_root()),
        artwork_cache: as_text(aircard_core::artwork_cache_root()),
        log_file: as_text(aircard_core::log_file()),
        cards_file: as_text(aircard_core::cards_file()),
    }
}

/// iPhones this Mac can reach right now.
#[tauri::command]
fn list_devices() -> Result<Vec<aircard_device::DeviceInfo>, String> {
    off_the_main_thread(|| {
        let devices = aircard_device::list_devices().map_err(|error| {
            failed(
                "无法读取设备列表。请确认 iPhone 已连接，并已在手机上信任此电脑。",
                error,
            )
        })?;
        // How many phones were seen is the first thing anybody asks, and the
        // answer is not otherwise written down anywhere.
        log_line(&format!("listed {} device(s)", devices.len()));
        Ok(devices)
    })
}

// ---------------------------------------------------------------------------
// The card list
// ---------------------------------------------------------------------------

#[tauri::command]
fn cards(udid: String) -> Vec<CardView> {
    list_cards(&udid)
}

#[tauri::command]
fn add_card(hash: String) -> Result<(), String> {
    file_store()
        .add(&hash)
        .map_err(|error| failed("无法写入卡片列表。", error))?;
    Ok(())
}

#[tauri::command]
fn forget_cards(hashes: Vec<String>) -> Result<(), String> {
    file_store()
        .forget(&hashes)
        .map_err(|error| failed("无法写入卡片列表。", error))?;
    Ok(())
}

/// A smaller copy of a kept face, as a data URL, for the list to show.
///
/// The full face is megabytes of PNG; a list of them would be a list of stalls.
#[tauri::command]
fn card_thumbnail(udid: String, hash: String) -> Option<String> {
    let png = artwork_cache().read(&udid, &hash)?;
    match artwork::thumbnail_png(&png, THUMBNAIL) {
        Ok(small) => Some(data_url(&small)),
        Err(error) => {
            log_line(&format!("thumbnail failed for {hash}: {error}"));
            None
        }
    }
}

#[tauri::command]
fn forget_artwork(udid: String, hash: String) {
    artwork_cache().forget(&udid, &hash);
}

/// The picture that is about to be flashed, fitted to a card face.
#[tauri::command]
fn image_preview(path: String) -> Result<Option<String>, String> {
    let fitted = fit_to_face(&path)?;
    match artwork::thumbnail_png(&fitted, PREVIEW) {
        Ok(small) => Ok(Some(data_url(&small))),
        Err(error) => Err(failed("无法读取这张图片。", error)),
    }
}

/// Fits a picture to a card face in a scratch file, and hands back the bytes.
fn fit_to_face(path: &str) -> Result<Vec<u8>, String> {
    let source = PathBuf::from(path);
    if !source.is_file() {
        return Err("找不到这张图片。".to_owned());
    }
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let fitted =
        std::env::temp_dir().join(format!("aircard-fit-{}-{unique}.png", std::process::id()));
    artwork::prepare_png(&source, &fitted)
        .map_err(|error| failed("AirCard 无法读取你选择的图片。", error))?;
    fs::read(&fitted).map_err(|error| failed("无法读取这张图片。", error))
}

// ---------------------------------------------------------------------------
// One card's face
// ---------------------------------------------------------------------------

/// Reads one of a card's files off the phone.
fn read_asset(device: &Device, hash: &str, name: &str) -> Result<Vec<u8>, String> {
    let directory = card_directory(hash);
    let airlift = device.airlift();
    let read = airlift
        .read_file(&directory, name, 3)
        .map_err(|error| failed("无法读取这张卡的卡面。请查看日志。", error))?;
    if read.card_needs_repair {
        // The bytes are in hand either way, and the caller keeps them: what went
        // wrong is that the phone did not get its own copy back, so the card
        // needs the same bytes written to it again.
        log_line(&format!(
            "read {hash}/{name} left the card needing repair; the copy is at {:?}",
            read.copy_at
        ));
    }
    Ok(read.data)
}

/// Reads a card's face off the phone and keeps it, so the list can show it.
#[tauri::command]
fn read_artwork(app: State<App>, udid: String, hash: String) -> Result<(), String> {
    let app = app.shared();
    off_the_main_thread(move || read_artwork_now(&app, udid, hash))
}

fn read_artwork_now(app: &App, udid: String, hash: String) -> Result<(), String> {
    let _turn = Turn::take(app)?;
    let device = Device::new(&udid);
    let png = read_asset(&device, &hash, BACKED_UP_ASSETS[0])?;
    artwork_cache()
        .save(&udid, &hash, &png)
        .map_err(|error| failed("无法保存卡面。", error))?;
    log_line(&format!("read artwork for {hash}: {} bytes", png.len()));
    Ok(())
}

/// Saves the original artwork of a card, once.
#[tauri::command]
fn save_original(app: State<App>, udid: String, hash: String) -> Result<(), String> {
    let app = app.shared();
    off_the_main_thread(move || {
        let _turn = Turn::take(&app)?;
        save_original_locked(&udid, &hash)
    })
}

fn save_original_locked(udid: &str, hash: &str) -> Result<(), String> {
    let device = Device::new(udid);
    let mut assets: Vec<(String, Vec<u8>)> = Vec::new();
    for name in BACKED_UP_ASSETS {
        assets.push((name.to_owned(), read_asset(&device, hash, name)?));
    }
    // The face that was just read is the one the list should show.
    if let Some((_, png)) = assets.iter().find(|(name, _)| name == BACKED_UP_ASSETS[0]) {
        let _ = artwork_cache().save(udid, hash, png);
    }
    backup_store()
        .save(udid, hash, &assets)
        .map_err(|error| failed("原始图像存储失败。", error))?;
    log_line(&format!("saved the original artwork of {hash}"));
    Ok(())
}

/// Puts a card's saved original back on the phone.
#[tauri::command]
fn restore_original(app: State<App>, udid: String, hash: String) -> Result<(), String> {
    let app = app.shared();
    off_the_main_thread(move || restore_original_now(&app, udid, hash))
}

fn restore_original_now(app: &App, udid: String, hash: String) -> Result<(), String> {
    let _turn = Turn::take(app)?;
    let store = backup_store();
    let assets = store.read(&udid, &hash);
    if assets.is_empty() {
        return Err("尚未存储这张卡片的原始图像，无法恢复。".to_owned());
    }
    let device = Device::new(&udid);
    let airlift = device.airlift();
    let directory = card_directory(&hash);
    airlift
        .write_files(&directory, &assets, 3)
        .map_err(|error| failed("卡片恢复失败。", error))?;
    airlift
        .invalidate_cache(&hash)
        .map_err(|error| failed("卡片恢复失败。", error))?;
    log_line(&format!("restored the original artwork of {hash}"));
    Ok(())
}

// ---------------------------------------------------------------------------
// Scanning the phone's log for cards
// ---------------------------------------------------------------------------

#[tauri::command]
fn start_scan(app: State<App>, udid: String) -> Result<(), String> {
    {
        let mut scan = app.scan.lock().map_err(|_| BUSY.to_owned())?;
        if scan.running {
            return Err("扫描已在进行中。".to_owned());
        }
        *scan = ScanView {
            running: true,
            lines_read: 0,
            found: Vec::new(),
            problem: None,
        };
    }
    app.stop.store(false, Ordering::SeqCst);

    let scan = Arc::clone(&app.scan);
    let phone = Arc::clone(&app.phone);
    let stop = Arc::clone(&app.stop);
    std::thread::spawn(move || {
        let app = App { phone, stop, scan };
        run_scan(&app, &udid);
    });
    Ok(())
}

/// The scan itself, on its own thread, holding the phone from start to finish.
fn run_scan(app: &App, udid: &str) {
    let finish = |problem: Option<String>| {
        if let Ok(mut scan) = app.scan.lock() {
            scan.running = false;
            scan.problem = problem;
        }
    };
    let turn = match Turn::take(app) {
        Ok(turn) => turn,
        Err(busy) => return finish(Some(busy)),
    };

    let mut stream = match aircard_device::LogStream::open(udid) {
        Ok(stream) => stream,
        Err(error) => {
            log_line(&format!("scan could not open the device log: {error}"));
            drop(turn);
            return finish(Some("无法开始扫描卡片。请查看日志。".to_owned()));
        }
    };

    let mut lines = 0_u64;
    let mut found: Vec<String> = Vec::new();
    let mut published = SystemTime::now();
    loop {
        if app.stop.load(Ordering::SeqCst) {
            log_line(&format!(
                "scan stopped after {lines} lines and {} card(s)",
                found.len()
            ));
            drop(turn);
            return finish(None);
        }
        match stream.next_line() {
            Ok(None) => continue,
            Ok(Some(line)) => {
                lines += 1;
                for hash in aircard_device::hashes_in(&line) {
                    if !found.iter().any(|seen| seen == &hash) {
                        log_line(&format!("scan saw card {hash}"));
                        found.push(hash);
                    }
                }
                if published.elapsed().unwrap_or_default() >= SCAN_PUBLISH {
                    if let Ok(mut scan) = app.scan.lock() {
                        scan.lines_read = lines;
                        scan.found = found.clone();
                    }
                    published = SystemTime::now();
                }
            }
            Err(error) => {
                log_line(&format!("scan: the log stream ended: {error}"));
                drop(turn);
                return finish(Some(
                    "卡片扫描已结束。请查看日志，重新连接 iPhone 后再试。".to_owned(),
                ));
            }
        }
    }
}

#[tauri::command]
fn scan_status(app: State<App>) -> ScanView {
    match app.scan.lock() {
        Ok(scan) => ScanView {
            running: scan.running,
            lines_read: scan.lines_read,
            found: scan.found.clone(),
            problem: scan.problem.clone(),
        },
        Err(_) => ScanView::default(),
    }
}

#[tauri::command]
fn stop_scan(app: State<App>) {
    app.stop.store(true, Ordering::SeqCst);
}

/// Takes what the scan found into the card list, and remembers it as the most
/// recent scan of this phone.
///
/// Cards already in the list keep their place: the list is what a person chose to
/// keep, and a scan only adds to it.
#[tauri::command]
fn fold_scan(app: State<App>, udid: String) -> Result<Vec<CardView>, String> {
    let found = match app.scan.lock() {
        Ok(scan) => scan.found.clone(),
        Err(_) => Vec::new(),
    };
    let store = file_store();
    store
        .remember(&udid, &found)
        .map_err(|error| failed("无法记录扫描结果。", error))?;
    let mut cards = store.cards();
    for hash in found {
        if !cards.iter().any(|kept| kept == &hash) {
            cards.push(hash);
        }
    }
    store
        .save(&cards)
        .map_err(|error| failed("无法写入卡片列表。", error))?;
    log_line(&format!("folded {} card(s) from the scan", cards.len()));
    Ok(list_cards(&udid))
}

#[tauri::command]
fn clear_scan_record(udid: String) -> Result<(), String> {
    file_store()
        .forget_scan(&udid)
        .map_err(|error| failed("无法清除扫描记录。", error))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Flashing
// ---------------------------------------------------------------------------

/// Puts one picture on one or more cards.
///
/// A card whose original is not saved is saved first: a flash is the one thing
/// this app does that cannot be undone from the phone, and the moment to make it
/// undoable is before it happens, not after.
#[tauri::command]
fn flash_cards(
    app: State<App>,
    udid: String,
    hashes: Vec<String>,
    image: String,
) -> Result<Vec<FlashResult>, String> {
    if hashes.is_empty() {
        return Err("请至少选择一张卡片。".to_owned());
    }
    if image.is_empty() {
        return Err("请先选择要刷入的图片。".to_owned());
    }
    // Fitting the picture is local work; everything that touches the phone
    // happens on a thread of its own.
    let png = fit_to_face(&image)?;
    let assets =
        artwork::build_assets(&png).map_err(|error| failed("无法准备卡面素材。", error))?;

    let app = app.shared();
    off_the_main_thread(move || flash_now(&app, udid, hashes, assets))
}

fn flash_now(
    app: &App,
    udid: String,
    hashes: Vec<String>,
    assets: Vec<(String, Vec<u8>)>,
) -> Result<Vec<FlashResult>, String> {
    let _turn = Turn::take(app)?;
    let device = Device::new(&udid);
    let airlift = device.airlift();
    let backups = backup_store();
    let mut results = Vec::new();

    for hash in &hashes {
        let directory = card_directory(hash);
        let mut saved = false;
        if !backups.contains(&udid, hash) {
            match save_original_locked(&udid, hash) {
                Ok(()) => saved = true,
                Err(message) => {
                    results.push(FlashResult {
                        card: hash.clone(),
                        ok: false,
                        message,
                    });
                    continue;
                }
            }
        }
        let written = airlift
            .write_files(&directory, &assets, 3)
            .map_err(|error| failed("卡片皮肤应用失败。", error));
        if let Err(message) = written {
            results.push(FlashResult {
                card: hash.clone(),
                ok: false,
                message,
            });
            continue;
        }
        match airlift.invalidate_cache(hash) {
            Ok(_) => {
                log_line(&format!("flashed {hash}"));
                results.push(FlashResult {
                    card: hash.clone(),
                    ok: true,
                    message: if saved {
                        "已存储原始图像，随后刷入。".to_owned()
                    } else {
                        "已刷入。".to_owned()
                    },
                });
            }
            Err(error) => results.push(FlashResult {
                card: hash.clone(),
                ok: false,
                message: failed("卡片已刷入，但渲染缓存未能清除。", error),
            }),
        }
    }
    Ok(results)
}

// ---------------------------------------------------------------------------
// What an interrupted run left behind
// ---------------------------------------------------------------------------

#[tauri::command]
fn leftovers(app: State<App>, udid: String) -> Result<Vec<LeftoverView>, String> {
    let app = app.shared();
    off_the_main_thread(move || leftovers_now(&app, udid))
}

fn leftovers_now(app: &App, udid: String) -> Result<Vec<LeftoverView>, String> {
    let _turn = Turn::take(app)?;
    let device = Device::new(&udid);
    let found = device
        .airlift()
        .leftovers()
        .map_err(|error| failed("无法读取设备上的残留。", error))?;
    Ok(found
        .into_iter()
        .map(|leftover| LeftoverView {
            name: leftover.name,
            ours: leftover.ours,
        })
        .collect())
}

#[tauri::command]
fn sweep_leftovers(app: State<App>, udid: String, names: Vec<String>) -> Result<(), String> {
    let app = app.shared();
    off_the_main_thread(move || sweep_leftovers_now(&app, udid, names))
}

fn sweep_leftovers_now(app: &App, udid: String, names: Vec<String>) -> Result<(), String> {
    let _turn = Turn::take(app)?;
    let device = Device::new(&udid);
    let airlift = device.airlift();
    let mut failures = Vec::new();
    for name in &names {
        if let Err(error) = airlift.remove_leftover(name) {
            log_line(&format!("could not sweep {name}: {error}"));
            failures.push(name.clone());
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!("有 {} 项未能清除。请查看日志。", failures.len()))
    }
}

// ---------------------------------------------------------------------------
// The log
// ---------------------------------------------------------------------------

#[tauri::command]
fn log_tail() -> String {
    let Ok(text) = fs::read_to_string(aircard_core::log_file()) else {
        return String::new();
    };
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(LOG_TAIL_LINES);
    lines[start..].join("\n")
}

#[tauri::command]
fn open_path(app: tauri::AppHandle, path: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_path(path, None::<&str>)
        .map_err(|error| failed("无法打开这个位置。", error))
}

/// The time, in UTC, as a log line wants it.
///
/// Written by hand because the standard library has no calendar and this is the
/// whole of what is needed: the days since the epoch, converted to a date by the
/// civil-from-days rule.
fn utc_stamp() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or_default();
    let (days, rest) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        (rest % 3600) / 60,
        rest % 60
    )
}

/// Howard Hinnant's `civil_from_days`: the date `days` days after 1970-01-01.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32;
    let month = (month_prime + if month_prime < 10 { 3 } else { -9 }) as u32;
    (year + i64::from(month <= 2), month, day)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(App::new())
        .invoke_handler(tauri::generate_handler![
            app_paths,
            list_devices,
            cards,
            add_card,
            forget_cards,
            card_thumbnail,
            forget_artwork,
            image_preview,
            read_artwork,
            save_original,
            restore_original,
            start_scan,
            scan_status,
            stop_scan,
            fold_scan,
            clear_scan_record,
            flash_cards,
            leftovers,
            sweep_leftovers,
            log_tail,
            open_path
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_stamp_counts_from_the_epoch_in_utc() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(1), (1970, 1, 2));
        // 2000-03-01 is day 11017, which is the interesting one for leap years.
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(11_017), (2000, 3, 1));
        let stamp = utc_stamp();
        assert!(stamp.ends_with('Z'), "{stamp}");
        assert_eq!(stamp.len(), 20, "{stamp}");
        assert!(stamp.starts_with("20"), "{stamp}");
    }

    #[test]
    fn only_one_operation_holds_the_phone_at_a_time() {
        let app = App::new();
        let first = Turn::take(&app).expect("the first turn");
        let refused = Turn::take(&app).expect_err("a second turn has to be refused");
        assert_eq!(refused, BUSY);
        drop(first);
        assert!(
            Turn::take(&app).is_ok(),
            "the phone is free again once the turn is over"
        );
    }

    #[test]
    fn the_picture_chosen_for_a_flash_becomes_a_preview() {
        // The one path in the window that can be checked without a phone: the
        // picture a person picks has to come back as something the panel can
        // draw, fitted to the shape of a card face first.
        let path = std::env::temp_dir().join(format!(
            "aircard-preview-{}-{}.png",
            std::process::id(),
            utc_stamp()
        ));
        fs::write(&path, TINY_PNG).expect("the fixture");
        let preview = image_preview(path.to_string_lossy().into_owned())
            .expect("a preview")
            .expect("a data url");
        assert!(preview.starts_with("data:image/png;base64,"), "{preview}");
        assert!(preview.len() > 200, "a real picture, not a stub");
        let _ = fs::remove_file(&path);

        // A path that is not there is a Chinese sentence, not a panic.
        let missing = image_preview("/nowhere/at/all.png".to_owned());
        assert_eq!(missing.expect_err("an error"), "找不到这张图片。");
    }

    /// An 8×8 PNG, so this test needs nothing from outside the repository.
    const TINY_PNG: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 8, 0, 0, 0, 8, 8, 2,
        0, 0, 0, 75, 109, 41, 220, 0, 0, 0, 108, 73, 68, 65, 84, 120, 218, 13, 201, 65, 1, 0, 48,
        8, 3, 49, 148, 160, 164, 74, 170, 132, 231, 169, 64, 9, 74, 170, 104, 203, 55, 85, 69, 23,
        42, 92, 76, 177, 197, 21, 41, 170, 154, 110, 212, 184, 153, 102, 155, 107, 210, 63, 68, 11,
        9, 139, 17, 43, 78, 68, 63, 76, 27, 25, 155, 49, 107, 206, 196, 63, 134, 30, 52, 120, 152,
        97, 135, 27, 50, 63, 150, 94, 180, 120, 153, 101, 151, 91, 178, 63, 142, 62, 116, 248, 152,
        99, 143, 59, 114, 63, 66, 7, 5, 135, 9, 27, 46, 36, 60, 176, 44, 84, 129, 249, 166, 11,
        121, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];

    #[test]
    fn a_turn_is_given_back_even_when_it_is_dropped_early() {
        let app = App::new();
        {
            let _turn = Turn::take(&app).expect("a turn");
        }
        assert!(!app.phone.load(Ordering::SeqCst));
    }
}
