//! The escape, against an iPhone on the end of the cable.
//!
//! Ignored by default, like every other test that needs hardware, and for the
//! same reason it has to run with `--test-threads=1`: two sessions to one phone
//! at once abort the process.
//!
//!   AIRCARD_TEST_UDID=<udid> cargo test -p aircard-device --test escape_device \
//!       -- --ignored --test-threads=1 --nocapture
//!
//! The first test rehearses the whole pipeline -- zip, symlink, ledger, move,
//! read, write, remove, clean up -- on a file this app creates itself, in a
//! directory that exists and is nobody's business. The second reads a real
//! card's artwork twice and compares. The last two are the acceptance test for
//! changing a card's face, in two runs so that the change can be looked at
//! before it is put back:
//!
//!   a_card_face_is_saved_then_changed      saves the original, flashes a face,
//!                                          reads it back, clears the cache
//!   a_card_face_is_put_back_from_the_saved_copy
//!                                          writes the saved original back and
//!                                          checks it byte for byte
//!
//! The card's bytes are printed and, where a copy is asked for, written out to be
//! compared against a known one. Comparing is the point: "it did not error" is
//! not the same answer as "the artwork is the artwork".
//!
//! Where the rehearsal puts its file matters. A first attempt used a file inside
//! `Media` and the phone moved the symlink and left the file alone, while
//! reporting that the move had happened -- measured on iOS 27. An asset the phone
//! already holds in its own sync root is not an asset to fetch. Cards are not in
//! there, and neither is anything else worth this trouble: every real target is
//! outside `Media`, so the rehearsal is too.

use std::env;
use std::fs;
use std::path::Path;

use aircard_core::assets::BACKED_UP_ASSETS;
use aircard_device::airlift::payload::token;
use aircard_device::{card_directory, Device};

/// A directory that has to exist, is outside Media, and will not miss one file.
///
/// Every card lives under `/var/mobile/Library/Passes`, so it is there; and
/// `aircard-probe-<token>` in it is a file nothing reads, which this test removes
/// again by the same escape that made it.
const PROBE_DIRECTORY: &str = "/var/mobile/Library";

/// The phone to work on. Never guessed: a test that picks a device by itself can
/// pick the wrong one.
fn device_udid() -> String {
    env::var("AIRCARD_TEST_UDID").expect("AIRCARD_TEST_UDID must name the phone to use")
}

fn probe_leaf() -> String {
    format!("aircard-probe-{}", token())
}

#[test]
#[ignore]
fn the_whole_pipeline_runs_on_a_file_of_its_own() {
    let device = Device::new(device_udid());
    let airlift = device.airlift();
    let leaf = probe_leaf();
    let first = b"an aircard probe, not a card".to_vec();
    let second = b"and this is the second face it wears".to_vec();

    // Nothing can create a file out there -- that is the whole reason the escape
    // exists -- so the probe is written by the escape itself.
    airlift
        .write_files(PROBE_DIRECTORY, &[(leaf.clone(), first.clone())], 3)
        .expect("the escape should have written the probe file");

    let read = airlift
        .read_file(PROBE_DIRECTORY, &leaf, 3)
        .expect("the escape should have read the probe file");
    assert_eq!(
        read.data, first,
        "the bytes that came back are not the ones that went in"
    );
    assert!(
        !read.card_needs_repair,
        "the file did not get its bytes back, and the only copy is at {:?}",
        read.copy_at
    );

    // Overwrite it and read that back too: writing and reading are the two
    // halves the app uses, and each has to survive the other.
    airlift
        .write_files(PROBE_DIRECTORY, &[(leaf.clone(), second.clone())], 3)
        .expect("the escape should have taken the second face");
    let again = airlift
        .read_file(PROBE_DIRECTORY, &leaf, 3)
        .expect("the second face should have read back");
    assert_eq!(again.data, second);

    // Removing it is how a rendered card face is invalidated, and it has to be a
    // real unlink: overwriting a face leaves Wallet rendering the old one.
    airlift
        .remove_files(PROBE_DIRECTORY, std::slice::from_ref(&leaf), 3)
        .expect("the escape should have removed the probe file");
    assert!(
        airlift.read_file(PROBE_DIRECTORY, &leaf, 1).is_err(),
        "the probe file came back"
    );

    let leftovers = airlift.leftovers().expect("leftovers should list");
    assert!(
        leftovers.is_empty(),
        "the rehearsal left {} thing(s) behind: {leftovers:?}",
        leftovers.len()
    );
}

#[test]
#[ignore]
fn a_cards_artwork_comes_back_byte_for_byte() {
    let device = Device::new(device_udid());
    let airlift = device.airlift();
    let hash = env::var("AIRCARD_TEST_CARD").expect("AIRCARD_TEST_CARD must name a card");
    let leaf = env::var("AIRCARD_TEST_LEAF")
        .unwrap_or_else(|_| "cardBackgroundCombined@3x.png".to_owned());
    let directory = card_directory(&hash);

    let first = airlift
        .read_file(&directory, &leaf, 3)
        .expect("the read should have worked");
    assert!(
        !first.card_needs_repair,
        "the card did not get its bytes back, and the only copy is at {:?}",
        first.copy_at
    );

    // Reading it a second time is the check that the write-back put the same
    // bytes back where they came from: an escape over an unchanged file has to
    // answer with the same bytes, and it has to do it twice running.
    let second = airlift
        .read_file(&directory, &leaf, 3)
        .expect("the second read should have worked");
    assert_eq!(first.data, second.data, "two reads of one file disagree");

    println!(
        "{hash}/{leaf}: {} bytes, read twice, both reads identical",
        first.data.len()
    );
    if let Ok(path) = env::var("AIRCARD_TEST_OUT") {
        fs::write(&path, &first.data).expect("the copy could not be written out");
        println!("a copy is at {path}");
    }

    let leftovers = airlift.leftovers().expect("leftovers should list");
    assert!(
        leftovers.is_empty(),
        "the read left {} thing(s) behind: {leftovers:?}",
        leftovers.len()
    );
}

/// Saves the face a card is wearing, then puts another one on it.
///
/// This one leaves the phone wearing the new face so a person can look at Wallet
/// and see it, and the original is on disk before anything is written -- a flash
/// with no saved original is the one mistake this app cannot undo.
///
///   AIRCARD_TEST_CARD=<hash> \
///   AIRCARD_TEST_FACE=<directory holding the three asset files to wear> \
///   AIRCARD_TEST_KEEP=<directory to save the original into> \
///   cargo test -p aircard-device --test escape_device \
///       -- --ignored --test-threads=1 --nocapture a_card_face_is_saved_then_changed
#[test]
#[ignore]
fn a_card_face_is_saved_then_changed() {
    let udid = device_udid();
    let hash = env::var("AIRCARD_TEST_CARD").expect("AIRCARD_TEST_CARD must name the card");
    let face = env::var("AIRCARD_TEST_FACE").expect("AIRCARD_TEST_FACE must hold the new face");
    let keep = env::var("AIRCARD_TEST_KEEP")
        .expect("AIRCARD_TEST_KEEP must name where to save the old one");
    let directory = card_directory(&hash);
    let airlift = Device::new(&udid).airlift();

    // What the card is wearing now. A read writes the same bytes back, so after
    // this loop the card is still exactly as it was.
    let mut original: Vec<(String, Vec<u8>)> = Vec::new();
    for name in BACKED_UP_ASSETS {
        let read = airlift
            .read_file(&directory, name, 3)
            .unwrap_or_else(|error| panic!("could not read {name}: {error}"));
        assert!(
            !read.card_needs_repair,
            "{name} did not get its bytes back, and the only copy is at {:?}",
            read.copy_at
        );
        println!("read {name}: {} bytes", read.data.len());
        original.push((name.to_owned(), read.data));
    }

    fs::create_dir_all(&keep).expect("the original could not be saved");
    for (name, bytes) in &original {
        let path = Path::new(&keep).join(name);
        fs::write(&path, bytes).expect("the original could not be saved");
        println!("saved {name} to {}", path.display());
    }

    let new_face: Vec<(String, Vec<u8>)> = BACKED_UP_ASSETS
        .iter()
        .map(|name| {
            let path = Path::new(&face).join(name);
            let bytes = fs::read(&path)
                .unwrap_or_else(|error| panic!("{} could not be read: {error}", path.display()));
            println!("flashing {name}: {} bytes", bytes.len());
            ((*name).to_owned(), bytes)
        })
        .collect();

    airlift
        .write_files(&directory, &new_face, 3)
        .expect("the flash should have written the new face");

    // The phone is the only one that has seen the result, so read it back.
    for (name, expected) in &new_face {
        let read = airlift
            .read_file(&directory, name, 3)
            .unwrap_or_else(|error| panic!("could not read {name} back: {error}"));
        assert_eq!(
            &read.data, expected,
            "{name} on the card is not what was flashed"
        );
        println!("{name} reads back as flashed");
    }

    // Wallet renders a card face once and keeps it, so the cache has to go
    // before the new artwork is what a person actually sees.
    let invalidated = airlift
        .invalidate_cache(&hash)
        .expect("the cache could not be invalidated");
    println!("cache invalidated: {invalidated}");

    let leftovers = airlift.leftovers().expect("leftovers should list");
    assert!(leftovers.is_empty(), "the flash left {leftovers:?} behind");
    println!("the card is wearing the new face -- open Wallet, then run the restore");
}

/// Puts the saved original back on the card, and checks it byte for byte.
///
///   AIRCARD_TEST_CARD=<hash> \
///   AIRCARD_TEST_RESTORE=<the directory saved earlier> \
///   cargo test -p aircard-device --test escape_device \
///       -- --ignored --test-threads=1 --nocapture a_card_face_is_put_back_from_the_saved_copy
#[test]
#[ignore]
fn a_card_face_is_put_back_from_the_saved_copy() {
    let udid = device_udid();
    let hash = env::var("AIRCARD_TEST_CARD").expect("AIRCARD_TEST_CARD must name the card");
    let keep = env::var("AIRCARD_TEST_RESTORE")
        .expect("AIRCARD_TEST_RESTORE must hold the saved original");
    let directory = card_directory(&hash);
    let airlift = Device::new(&udid).airlift();

    let original: Vec<(String, Vec<u8>)> = BACKED_UP_ASSETS
        .iter()
        .map(|name| {
            let path = Path::new(&keep).join(name);
            let bytes = fs::read(&path)
                .unwrap_or_else(|error| panic!("{} could not be read: {error}", path.display()));
            ((*name).to_owned(), bytes)
        })
        .collect();

    airlift
        .write_files(&directory, &original, 3)
        .expect("the original should have gone back");

    let mut all_back = true;
    for (name, expected) in &original {
        let read = airlift
            .read_file(&directory, name, 3)
            .unwrap_or_else(|error| panic!("could not read {name} back: {error}"));
        let same = read.data == *expected;
        all_back &= same;
        println!(
            "{name}: {} bytes, {}",
            read.data.len(),
            if same { "identical" } else { "DIFFERENT" }
        );
    }
    assert!(
        all_back,
        "the card does not read back as the original it was saved from"
    );

    let invalidated = airlift
        .invalidate_cache(&hash)
        .expect("the cache could not be invalidated");
    println!("cache invalidated: {invalidated}");
    let leftovers = airlift.leftovers().expect("leftovers should list");
    assert!(
        leftovers.is_empty(),
        "the restore left {leftovers:?} behind"
    );
    println!("the card is back to the face it was saved from");
}
