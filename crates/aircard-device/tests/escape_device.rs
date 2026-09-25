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
//! directory that exists and is nobody's business. The second does the same to a
//! real card's artwork, and is only worth trying once the first one passes:
//!
//!   AIRCARD_TEST_CARD=<card hash> AIRCARD_TEST_OUT=<path> cargo test ...
//!
//! The card's bytes are printed and, with `AIRCARD_TEST_OUT`, written out to be
//! compared against a known copy. Comparing is the point: these runs are the
//! first time any of this has touched a card, and "it did not error" is not the
//! same answer as "the artwork is the artwork".
//!
//! Where the rehearsal puts its file matters. A first attempt used a file inside
//! `Media` and the phone moved the symlink and left the file alone, while
//! reporting that the move had happened -- measured on iOS 27. An asset the
//! phone already holds in its own sync root is not an asset to fetch. Cards are
//! not in there, and neither is anything else worth this trouble: every real
//! target is outside `Media`, so the rehearsal is too.

use std::env;
use std::fs;

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
