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
//! read back, write back, clean up -- on a file this app makes inside `Media`,
//! where nothing of anyone's is at stake. The second does the same to a real
//! card's artwork, and is only worth trying once the first one passes:
//!
//!   AIRCARD_TEST_CARD=<card hash> AIRCARD_TEST_OUT=<path> cargo test ...
//!
//! The card's bytes are printed and, with `AIRCARD_TEST_OUT`, written out to be
//! compared against a known copy. Comparing is the point: these runs are the
//! first time any of this has touched a card, and "it did not error" is not the
//! same answer as "the artwork is the artwork".

use std::env;
use std::fs;

use aircard_device::airlift::payload::token;
use aircard_device::{card_directory, Device, MediaSource};

/// The phone to work on. Never guessed: a test that picks a device by itself can
/// pick the wrong one.
fn device_udid() -> String {
    env::var("AIRCARD_TEST_UDID").expect("AIRCARD_TEST_UDID must name the phone to use")
}

/// A file of our own inside Media.
///
/// Deliberately outside the names the escape generates, so it can never be
/// mistaken for staging. What it proves is that the escape takes *its* names back
/// down, which the test checks separately by listing them.
fn probe_directory() -> String {
    format!("/var/mobile/Media/aircard-probe-{}", token())
}

#[test]
#[ignore]
fn the_whole_pipeline_runs_on_a_file_of_its_own() {
    let device = Device::new(device_udid());
    let airlift = device.airlift();
    let directory = probe_directory();
    let leaf = "probe.bin";
    let before = b"an aircard probe, not a card".to_vec();

    {
        let mut media = device.open().expect("AFC should have opened");
        media
            .create_directory(&directory)
            .expect("the probe directory should have been made");
        media
            .write(&format!("{directory}/{leaf}"), &before)
            .expect("the probe file should have been written");
    }

    let read = airlift
        .read_file(&directory, leaf, 3)
        .expect("the rehearsal should have read the file");
    assert_eq!(
        read.data, before,
        "the bytes that came back are not the ones that went in"
    );
    assert!(
        !read.card_needs_repair,
        "the file did not get its bytes back, and a copy is at {:?}",
        read.copy_at
    );

    {
        let mut media = device.open().expect("AFC should have opened");
        let observed = media
            .read(&format!("{directory}/{leaf}"), 1 << 20)
            .expect("the file should still be there");
        assert_eq!(observed, before, "the file has to be exactly as it was");
        media
            .remove(&format!("{directory}/{leaf}"))
            .expect("the probe file should have been removed");
        media
            .remove(&directory)
            .expect("the probe directory should have been removed");
    }

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
