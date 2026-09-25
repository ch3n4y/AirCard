//! Reads the backups this machine already has.
//!
//! Ignored by default: it depends on the user's real data, so it is a check to run
//! on purpose rather than part of the suite. It is worth running after a port,
//! because the saved originals were written by the previous implementation -- if
//! they read back here, the layout and the percent-encoded names survived intact.

use aircard_core::{BackupStore, PDF_ASSET_NAME, PNG_ASSET_NAMES};

#[test]
#[ignore = "reads this machine's real ~/.aircard_backups"]
fn real_backups_on_this_machine_read_back() {
    let store = BackupStore::new(aircard_core::backups_root());
    let devices = store.devices();
    println!("devices with saved originals: {devices:?}");
    assert!(
        !devices.is_empty(),
        "expected at least one device in {}",
        store.root().display()
    );

    let mut expected: Vec<String> = vec![
        PDF_ASSET_NAME.to_owned(),
        PNG_ASSET_NAMES[1].to_owned(),
        PNG_ASSET_NAMES[0].to_owned(),
    ];
    expected.sort();

    for udid in &devices {
        let cards = store.list(udid);
        println!("  {udid}: {} card(s)", cards.len());
        for card in &cards {
            let mut names: Vec<String> = store
                .read(udid, card)
                .into_iter()
                .map(|(name, data)| {
                    assert!(!data.is_empty(), "{name} is empty");
                    name
                })
                .collect();
            names.sort();
            println!("    {card} -> {names:?}");
            assert_eq!(names, expected, "a saved original holds the whole set");
        }
    }
}
