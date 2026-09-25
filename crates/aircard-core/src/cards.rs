//! The cards the window knows about, and what the last scan saw.
//!
//! Two files, deliberately apart. The card list is what a person chose to keep:
//! it survives the phone being unplugged, and a card the phone has stopped
//! rendering stays in it. The scan record is only what one scan happened to see,
//! which is a different kind of fact -- a card Wallet is not showing logs nothing,
//! so "not seen" is a hint and never a reason to delete anything on its own.
//!
//! The card list is a plain JSON array at `~/.aircard_cards.json`, which is the
//! file the previous implementation kept: a list built up over months reads as it
//! stands, and the old app keeps working if it is opened again.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::paths;

/// A card number, as the device spells it: base64 with `+`, `/` and `=` in it.
pub type CardHash = String;

/// The cards on this Mac, and what each scan saw.
pub struct CardStore {
    cards: PathBuf,
    seen: PathBuf,
}

impl CardStore {
    pub fn new(cards: impl Into<PathBuf>, seen: impl Into<PathBuf>) -> Self {
        Self {
            cards: cards.into(),
            seen: seen.into(),
        }
    }

    /// Where the app keeps both files.
    pub fn standard() -> Self {
        Self::new(paths::cards_file(), paths::scan_record_file())
    }

    pub fn cards_path(&self) -> &PathBuf {
        &self.cards
    }

    /// The list, in the order it was saved.
    ///
    /// A file that is missing or unreadable is an empty list rather than an
    /// error: a fresh install has no file, and a list somebody edited by hand
    /// into invalid JSON must not stop the app from starting.
    pub fn cards(&self) -> Vec<CardHash> {
        let Ok(text) = fs::read_to_string(&self.cards) else {
            return Vec::new();
        };
        let Ok(saved) = serde_json::from_str::<Vec<CardHash>>(&text) else {
            return Vec::new();
        };
        clean(saved)
    }

    pub fn save(&self, cards: &[CardHash]) -> std::io::Result<()> {
        if let Some(parent) = self.cards.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut text = serde_json::to_string_pretty(&clean(cards.to_vec()))
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        text.push('\n');
        fs::write(&self.cards, text)
    }

    /// Adds a card if it is not there, and answers with the whole list.
    pub fn add(&self, hash: &str) -> std::io::Result<Vec<CardHash>> {
        let mut cards = self.cards();
        if !hash.is_empty() && !cards.iter().any(|card| card == hash) {
            cards.push(hash.to_owned());
            self.save(&cards)?;
        }
        Ok(cards)
    }

    /// Takes cards out of the list. The saved originals stay on this Mac: a card
    /// can be added back and restored.
    pub fn forget(&self, hashes: &[CardHash]) -> std::io::Result<Vec<CardHash>> {
        let cards: Vec<CardHash> = self
            .cards()
            .into_iter()
            .filter(|card| !hashes.iter().any(|drop| drop == card))
            .collect();
        self.save(&cards)?;
        Ok(cards)
    }

    /// Which cards the most recent scan of this phone saw.
    pub fn seen(&self, udid: &str) -> Vec<CardHash> {
        self.records().remove(udid).map(clean).unwrap_or_default()
    }

    pub fn remember(&self, udid: &str, hashes: &[CardHash]) -> std::io::Result<()> {
        let mut records = self.records();
        records.insert(udid.to_owned(), clean(hashes.to_vec()));
        self.write_records(&records)
    }

    pub fn forget_scan(&self, udid: &str) -> std::io::Result<()> {
        let mut records = self.records();
        records.remove(udid);
        self.write_records(&records)
    }

    fn records(&self) -> BTreeMap<String, Vec<CardHash>> {
        let Ok(text) = fs::read_to_string(&self.seen) else {
            return BTreeMap::new();
        };
        serde_json::from_str(&text).unwrap_or_default()
    }

    fn write_records(&self, records: &BTreeMap<String, Vec<CardHash>>) -> std::io::Result<()> {
        if let Some(parent) = self.seen.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut text = serde_json::to_string_pretty(records)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        text.push('\n');
        fs::write(&self.seen, text)
    }
}

/// Drops empties and repeats, and leaves the order alone: the order is the order
/// cards were added, which is the only order a person recognises.
fn clean(cards: Vec<CardHash>) -> Vec<CardHash> {
    let mut kept: Vec<CardHash> = Vec::new();
    for card in cards {
        let card = card.trim().to_owned();
        if !card.is_empty() && !kept.iter().any(|seen| seen == &card) {
            kept.push(card);
        }
    }
    kept
}

/// One entry of the card list as the window shows it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Card {
    pub hash: CardHash,
    /// Its original artwork is saved on this Mac.
    pub has_original: bool,
    /// A face has been read off the phone and kept, so the list can show it.
    pub has_artwork: bool,
    /// It appeared in the most recent scan of this phone.
    pub seen_in_last_scan: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch {
        path: PathBuf,
    }

    impl Scratch {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "aircard-cards-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_nanos())
                    .unwrap_or_default()
            ));
            fs::create_dir_all(&path).expect("a scratch directory");
            Self { path }
        }

        fn store(&self) -> CardStore {
            CardStore::new(self.path.join("cards.json"), self.path.join("seen.json"))
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    const REAL: &str = "2Do5+0cj+vG1zMfmFbPt0D4GPKQ=";

    #[test]
    fn a_list_that_is_saved_is_the_list_that_comes_back() {
        let scratch = Scratch::new();
        let store = scratch.store();
        assert!(store.cards().is_empty(), "a fresh install has no list");

        store
            .save(&["a".into(), REAL.into(), "c".into()])
            .expect("saving");
        assert_eq!(store.cards(), vec!["a", REAL, "c"], "order is kept");
        assert!(scratch.path.join("cards.json").is_file());
    }

    #[test]
    fn the_file_is_the_one_the_previous_implementation_wrote() {
        // A list written by the old app, read by this one, as it stands.
        let scratch = Scratch::new();
        let store = scratch.store();
        fs::write(
            scratch.path.join("cards.json"),
            "[\n  \"ql2MjCQ86Xc6xp7nPee-xDWNSRI=\",\n  \"2Do5+0cj+vG1zMfmFbPt0D4GPKQ=\"\n]\n",
        )
        .expect("the old file");
        assert_eq!(store.cards(), vec!["ql2MjCQ86Xc6xp7nPee-xDWNSRI=", REAL]);
        // And what this app writes is a file the old app can read: a JSON array
        // of strings, one per line.
        store.add("HuPplYeKY753l5EqMBiwHbP3aos=").expect("adding");
        let written = fs::read_to_string(scratch.path.join("cards.json")).expect("the new file");
        assert!(written.starts_with("[\n  \""));
        let value: serde_json::Value = serde_json::from_str(&written).expect("still JSON");
        assert_eq!(value.as_array().map(Vec::len), Some(3));
    }

    #[test]
    fn adding_a_card_twice_leaves_one_of_it() {
        let scratch = Scratch::new();
        let store = scratch.store();
        store.add(REAL).expect("adding");
        store.add(REAL).expect("adding again");
        assert_eq!(store.cards(), vec![REAL]);
        // Whitespace around a pasted hash is not part of the hash.
        store.add("  spaced  ").expect("adding a pasted one");
        assert_eq!(store.cards(), vec![REAL, "spaced"]);
        store.add("").expect("an empty paste is ignored");
        assert_eq!(store.cards(), vec![REAL, "spaced"]);
    }

    #[test]
    fn forgetting_takes_the_card_and_leaves_the_rest() {
        let scratch = Scratch::new();
        let store = scratch.store();
        store
            .save(&["a".into(), "b".into(), "c".into()])
            .expect("saving");
        let left = store.forget(&["b".into()]).expect("forgetting");
        assert_eq!(left, vec!["a", "c"]);
        assert_eq!(store.cards(), vec!["a", "c"]);
        store
            .forget(&["a".into(), "c".into()])
            .expect("forgetting the rest");
        assert!(store.cards().is_empty());
    }

    #[test]
    fn a_file_somebody_broke_does_not_stop_the_app() {
        let scratch = Scratch::new();
        let store = scratch.store();
        fs::write(scratch.path.join("cards.json"), "{ not json at all").expect("a broken file");
        assert!(store.cards().is_empty(), "a broken list reads as no list");
        // And it can be written again, which is what lets the person recover.
        store.add(REAL).expect("adding over a broken file");
        assert_eq!(store.cards(), vec![REAL]);
    }

    #[test]
    fn the_scan_record_is_per_phone_and_replaces_what_was_there() {
        let scratch = Scratch::new();
        let store = scratch.store();
        assert!(store.seen("phone-a").is_empty());

        store
            .remember("phone-a", &["one".into(), "two".into()])
            .expect("remembering");
        store
            .remember("phone-b", &["three".into()])
            .expect("remembering");
        assert_eq!(store.seen("phone-a"), vec!["one", "two"]);
        assert_eq!(store.seen("phone-b"), vec!["three"]);
        assert!(store.seen("phone-c").is_empty());

        // A second scan of the same phone is the record, not a second record.
        store
            .remember("phone-a", &["two".into()])
            .expect("remembering");
        assert_eq!(store.seen("phone-a"), vec!["two"]);
        assert_eq!(
            store.seen("phone-b"),
            vec!["three"],
            "the other phone is untouched"
        );

        store.forget_scan("phone-a").expect("forgetting the record");
        assert!(store.seen("phone-a").is_empty());
        assert_eq!(store.seen("phone-b"), vec!["three"]);
    }

    #[test]
    fn a_broken_scan_record_reads_as_nothing_seen_and_can_be_written_again() {
        let scratch = Scratch::new();
        let store = scratch.store();
        fs::write(scratch.path.join("seen.json"), "[[[").expect("a broken file");
        assert!(store.seen("phone-a").is_empty());
        store
            .remember("phone-a", &["one".into()])
            .expect("remembering");
        assert_eq!(store.seen("phone-a"), vec!["one"]);
    }
}
