//! The commands the window calls.
//!
//! Deliberately thin: the rules live in `aircard-core` (saved originals, asset
//! names) and `aircard-device` (the phone), and both are tested without a window
//! or a device attached.

use aircard_core::BackupStore;
use serde::Serialize;

#[derive(Serialize)]
struct AppPaths {
    backups: String,
    artwork_cache: String,
    log_file: String,
}

fn as_text(path: std::path::PathBuf) -> String {
    path.to_string_lossy().into_owned()
}

/// Where AirCard keeps things, so the window can show it and a report can name it.
#[tauri::command]
fn app_paths() -> AppPaths {
    AppPaths {
        backups: as_text(aircard_core::backups_root()),
        artwork_cache: as_text(aircard_core::artwork_cache_root()),
        log_file: as_text(aircard_core::log_file()),
    }
}

/// Devices with at least one saved original on this Mac.
#[tauri::command]
fn devices_with_saved_originals() -> Vec<String> {
    BackupStore::new(aircard_core::backups_root()).devices()
}

/// Cards on a device whose original artwork is saved, so a restore is real.
#[tauri::command]
fn saved_originals(udid: String) -> Vec<String> {
    BackupStore::new(aircard_core::backups_root()).list(&udid)
}

/// iPhones this Mac can reach right now.
///
/// An error is worth surfacing as an error: "nothing is plugged in" and "the
/// framework would not talk to us" mean very different things to whoever is
/// holding the phone.
#[tauri::command]
fn list_devices() -> Result<Vec<aircard_device::DeviceInfo>, String> {
    aircard_device::list_devices().map_err(|error| error.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            app_paths,
            devices_with_saved_originals,
            saved_originals,
            list_devices
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
