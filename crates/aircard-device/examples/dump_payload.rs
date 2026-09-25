//! Writes the single-file archive to disk so another `zipfile` can read it.
//!
//! The escape's payload is built entirely in memory and only exists on a phone
//! for the length of one run, so this is the only way to see one. It exists to
//! cross-check the Rust zip against the ported Python one: `zipfile` and
//! `plistlib` should report the same entries, modes and extra fields.
//!
//!   cargo run --example dump_payload -- [path]
//!
//! Defaults to `/tmp/aircard-payload.zip`.

use std::path::PathBuf;

use aircard_device::airlift::build_archive;

fn main() {
    let path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/aircard-payload.zip"));

    let archive = build_archive(
        "/var/mobile/Library/Passes/Cards/EXAMPLE.pkpass",
        b"aircard-test-payload",
    );
    std::fs::write(&path, &archive).expect("writing the archive");
    println!("wrote {} bytes to {}", archive.len(), path.display());
}
