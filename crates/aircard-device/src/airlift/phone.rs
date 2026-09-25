//! A phone that fits in memory.
//!
//! The escape is a conversation between three things that cannot be tested apart:
//! AFC, the phone's own unzip, and AirTraffic. This module is all three, small
//! enough to read, so the choreography in [`super::escape`] can be run against
//! something that behaves the way the real thing does:
//!
//! * AFC sees `var/mobile/Media` and nothing else. A path that climbs out, or
//!   that resolves out through the escaping symlink, is simply not there as far
//!   as it is concerned -- that restriction is the reason the escape exists, so
//!   the fake has to have it too or the tests prove nothing.
//! * the zip conduit runs a real unzip, of the real archive [`super::payload`]
//!   builds, including turning the link entry into an actual symlink.
//! * AirTraffic resolves asset identifiers against the Airlock root, refuses
//!   anything the `Books` ledger does not name, and *moves* what it finds.
//!
//! Every test in here is a statement about the real phone that a person could
//! check by hand, in the form of an assertion instead of a cable.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::io::{Cursor, Read};
use std::rc::Rc;

use super::escape::{
    Airlift, AirliftError, ArchiveUpload, AssetMover, Kind, Media, MediaSource, Result,
};

/// Where AFC's root is on the device.
const MEDIA: &str = "var/mobile/Media";

/// Where AirTraffic's asset identifiers start.
const AIRLOCK: &str = "var/mobile/Media/Airlock/Book";

/// The longest chain of symlinks resolved before giving up.
const SYMLINK_LIMIT: u32 = 40;

/// The card every fixture uses.
const CARD: &str = "/var/mobile/Library/Passes/Cards/TestCard.pkpass";

/// The three asset names a card's artwork is written as. Wallet renders the PDF
/// in preference to the PNGs, which is why a flash has to move all three.
const ARTWORK: [&str; 3] = [
    "cardBackgroundCombined@2x.png",
    "cardBackgroundCombined@3x.png",
    "cardBackgroundCombined.pdf",
];

#[derive(Debug, Clone, PartialEq, Eq)]
enum Node {
    File(Vec<u8>),
    Directory,
    /// The target text, exactly as the archive carried it.
    Symlink(String),
}

/// A whole device tree, keyed by path without the leading slash.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Fs {
    nodes: BTreeMap<String, Node>,
}

impl Fs {
    /// Resolves `path` -- absolute, or relative to `base` -- into a key.
    ///
    /// Intermediate symlinks are followed; the final component is followed only
    /// when `follow_last`, because renaming a symlink moves the link itself and
    /// the escape depends on that. A missing component ends the walk.
    fn resolve(&self, base: &str, path: &str, follow_last: bool) -> Option<String> {
        let mut resolved: Vec<String> = if path.starts_with('/') {
            Vec::new()
        } else {
            components(base)
        };
        let mut pending: VecDeque<String> = path.split('/').map(str::to_owned).collect();
        let mut hops = 0;

        while let Some(component) = pending.pop_front() {
            match component.as_str() {
                "" | "." => {}
                ".." => {
                    resolved.pop();
                }
                _ => {
                    let mut candidate = resolved.clone();
                    candidate.push(component.clone());
                    match self.nodes.get(&candidate.join("/")) {
                        Some(Node::Symlink(target)) if follow_last || !pending.is_empty() => {
                            hops += 1;
                            if hops > SYMLINK_LIMIT {
                                return None;
                            }
                            if target.starts_with('/') {
                                resolved.clear();
                            }
                            for part in target.split('/').rev() {
                                pending.push_front(part.to_owned());
                            }
                        }
                        Some(_) => resolved.push(component),
                        None => return None,
                    }
                }
            }
        }
        Some(resolved.join("/"))
    }

    /// The children of a directory, which is what `list` answers with.
    fn children(&self, directory: &str) -> Vec<String> {
        let prefix = format!("{directory}/");
        let mut names: Vec<String> = self
            .nodes
            .keys()
            .filter_map(|other| {
                other
                    .strip_prefix(&prefix)
                    .filter(|rest| !rest.is_empty() && !rest.contains('/'))
                    .map(str::to_owned)
            })
            .collect();
        names.sort();
        names
    }

    fn file(&self, path: &str) -> Option<Vec<u8>> {
        match self.nodes.get(path) {
            Some(Node::File(bytes)) => Some(bytes.clone()),
            _ => None,
        }
    }
}

fn components(path: &str) -> Vec<String> {
    path.split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

fn split_last(path: &str) -> Option<(&str, &str)> {
    let trimmed = path.trim_end_matches('/');
    let (parent, leaf) = trimmed
        .rfind('/')
        .map(|index| (&trimmed[..index], &trimmed[index + 1..]))
        .unwrap_or(("", trimmed));
    (!leaf.is_empty()).then_some((parent, leaf))
}

/// What the phone does with one `move_assets` call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Plan {
    /// Every asset arrives.
    All,
    /// Nothing arrives: the sync refuses the whole request.
    Refused,
    /// This many assets arrive and then the sync drops out. Interrupting a real
    /// move is the case that costs data when it is handled wrong, so it has to be
    /// reachable from a test.
    Interrupted(usize),
}

/// One device, shared by the three fake roles and by the test.
#[derive(Clone)]
struct Phone {
    fs: Rc<RefCell<Fs>>,
    /// One plan per move the app is expected to make, in order; an empty queue
    /// is a phone that just works.
    plans: Rc<RefCell<VecDeque<Plan>>>,
}

impl Phone {
    /// A phone with Media, an empty book library, and nothing else.
    fn new() -> Self {
        let phone = Self {
            fs: Rc::new(RefCell::new(Fs::default())),
            plans: Rc::new(RefCell::new(VecDeque::new())),
        };
        {
            let mut fs = phone.fs.borrow_mut();
            for directory in [
                "var",
                "var/mobile",
                MEDIA,
                AIRLOCK,
                "var/mobile/Library/Passes/Cards",
            ] {
                fs.nodes.insert(directory.to_owned(), Node::Directory);
            }
            // An existing ledger entry, so that restoring the preimage has
            // something to prove: a file that was there before must be there
            // afterwards, byte for byte.
            fs.nodes.insert(
                format!("{MEDIA}/Books/Books.plist"),
                Node::File(b"the library the phone had".to_vec()),
            );
            fs.nodes.insert(format!("{MEDIA}/Books"), Node::Directory);
        }
        phone
    }

    fn airlift(&self) -> Airlift<Phone, Phone, Phone> {
        Airlift::new(self.clone(), self.clone(), self.clone())
    }

    /// Adds a file, making any directories on the way.
    fn add_file(&self, path: &str, bytes: &[u8]) -> &Self {
        let mut fs = self.fs.borrow_mut();
        let mut walk = String::new();
        for part in components(path) {
            walk = if walk.is_empty() {
                part.clone()
            } else {
                format!("{walk}/{part}")
            };
            fs.nodes.entry(walk.clone()).or_insert(Node::Directory);
        }
        let key = path.trim_start_matches('/').to_owned();
        fs.nodes.insert(key, Node::File(bytes.to_vec()));
        self
    }

    /// A card with artwork and rendered faces, the way a real one looks.
    fn add_card(&self, artwork: &[u8]) -> &Self {
        for leaf in ARTWORK {
            self.add_file(&format!("{CARD}/{leaf}"), artwork);
        }
        for leaf in ["FrontFace", "PlaceHolder", "Preview"] {
            self.add_file(
                &format!("/var/mobile/Library/Passes/Cards/TestCard.cache/{leaf}"),
                artwork,
            );
        }
        self
    }

    fn card_artwork(&self) -> Option<Vec<u8>> {
        self.fs
            .borrow()
            .file(&format!("{}/{}", CARD.trim_start_matches('/'), ARTWORK[0]))
    }

    fn cache_names(&self) -> Vec<String> {
        let fs = self.fs.borrow();
        fs.children("var/mobile/Library/Passes/Cards/TestCard.cache")
    }

    fn media_names(&self) -> Vec<String> {
        self.fs.borrow().children(MEDIA)
    }

    fn device(&self) -> Fs {
        self.fs.borrow().clone()
    }

    /// What the phone does with each move it is asked for, in order.
    fn plans(&self, plans: &[Plan]) -> &Self {
        self.plans.borrow_mut().extend(plans);
        self
    }

    fn ledger(&self) -> Vec<String> {
        let fs = self.fs.borrow();
        let Some(bytes) = fs.file(&format!("{MEDIA}/Books/Sync/Books.plist")) else {
            return Vec::new();
        };
        let Ok(value) = plist::Value::from_reader(Cursor::new(bytes)) else {
            return Vec::new();
        };
        value
            .as_dictionary()
            .and_then(|root| root.get("Books"))
            .and_then(|books| books.as_array())
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| {
                        row.as_dictionary()
                            .and_then(|row| row.get("Persistent ID"))
                            .and_then(|identifier| identifier.as_string())
                            .map(str::to_owned)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// What AFC sees. Everything a Media method is asked about lands here, and
/// anything outside `var/mobile/Media` is refused the way AFC refuses it.
struct MediaView<'a> {
    fs: &'a RefCell<Fs>,
}

impl MediaView<'_> {
    fn visible(&self, path: &str, follow_last: bool) -> Option<String> {
        let fs = self.fs.borrow();
        let full = format!("{MEDIA}/{}", path.trim_start_matches('/'));
        let key = fs.resolve("", &full, follow_last)?;
        let inside = key == MEDIA || key.starts_with(&format!("{MEDIA}/"));
        inside.then_some(key)
    }

    fn refused(path: &str, detail: &str) -> AirliftError {
        AirliftError::Refused {
            path: path.to_owned(),
            detail: detail.to_owned(),
        }
    }
}

impl Media for MediaView<'_> {
    fn exists(&mut self, path: &str) -> bool {
        self.kind(path).is_some()
    }

    fn kind(&mut self, path: &str) -> Option<Kind> {
        let key = self.visible(path, false)?;
        match self.fs.borrow().nodes.get(&key)? {
            Node::File(_) => Some(Kind::File),
            Node::Directory => Some(Kind::Directory),
            Node::Symlink(_) => Some(Kind::Symlink),
        }
    }

    fn read(&mut self, path: &str, limit: u64) -> Result<Vec<u8>> {
        let key = self
            .visible(path, true)
            .ok_or_else(|| AirliftError::Unreadable {
                path: path.to_owned(),
            })?;
        match self.fs.borrow().nodes.get(&key) {
            Some(Node::File(bytes)) if bytes.len() as u64 <= limit => Ok(bytes.clone()),
            _ => Err(AirliftError::Unreadable {
                path: path.to_owned(),
            }),
        }
    }

    fn write(&mut self, path: &str, data: &[u8]) -> Result<()> {
        let (parent, leaf) = split_last(path).ok_or_else(|| Self::refused(path, "no file name"))?;
        let parent = self
            .visible(parent, true)
            .ok_or_else(|| Self::refused(path, "the directory is not there"))?;
        let mut fs = self.fs.borrow_mut();
        if fs.nodes.get(&parent) != Some(&Node::Directory) {
            return Err(Self::refused(path, "the directory is not there"));
        }
        fs.nodes
            .insert(format!("{parent}/{leaf}"), Node::File(data.to_vec()));
        Ok(())
    }

    fn create_directory(&mut self, path: &str) -> Result<()> {
        // AFC makes one level and wants the parent there, so the parent is what
        // gets resolved: the leaf itself is the thing about to come into being.
        let (parent, leaf) = split_last(path).ok_or_else(|| Self::refused(path, "no name"))?;
        let parent = self
            .visible(parent, true)
            .ok_or_else(|| Self::refused(path, "the parent is not there"))?;
        let mut fs = self.fs.borrow_mut();
        if fs.nodes.get(&parent) != Some(&Node::Directory) {
            return Err(Self::refused(path, "the parent is not there"));
        }
        fs.nodes.insert(format!("{parent}/{leaf}"), Node::Directory);
        Ok(())
    }

    fn remove(&mut self, path: &str) -> Result<()> {
        let key = self
            .visible(path, false)
            .ok_or_else(|| Self::refused(path, "not a path AFC can see"))?;
        let mut fs = self.fs.borrow_mut();
        if fs.nodes.get(&key) == Some(&Node::Directory) && !fs.children(&key).is_empty() {
            return Err(Self::refused(path, "the directory is not empty"));
        }
        fs.nodes.remove(&key);
        Ok(())
    }

    fn list(&mut self, path: &str) -> Result<Vec<String>> {
        let key = self
            .visible(path, true)
            .ok_or_else(|| Self::refused(path, "not a directory AFC can see"))?;
        let fs = self.fs.borrow();
        if fs.nodes.get(&key) != Some(&Node::Directory) {
            return Err(Self::refused(path, "not a directory"));
        }
        Ok(fs.children(&key))
    }
}

impl MediaSource for Phone {
    fn open(&self) -> Result<Box<dyn Media + '_>> {
        Ok(Box::new(MediaView { fs: &self.fs }))
    }
}

impl ArchiveUpload for Phone {
    fn upload(&self, media_subdir: &str, archive: &[u8]) -> Result<()> {
        let upload = |detail: String| AirliftError::Upload { detail };
        let mut zip = zip::ZipArchive::new(Cursor::new(archive))
            .map_err(|error| upload(error.to_string()))?;
        let root = format!("{MEDIA}/{media_subdir}");
        let mut fs = self.fs.borrow_mut();
        // The conduit makes the directory it is told to unpack into.
        fs.nodes.insert(root.clone(), Node::Directory);

        for index in 0..zip.len() {
            let mut entry = zip
                .by_index(index)
                .map_err(|error| upload(error.to_string()))?;
            let name = entry.name().trim_end_matches('/').to_owned();
            let mode = entry.unix_mode().unwrap_or(0o100_644);
            let key = format!("{root}/{name}");
            match mode & 0o170_000 {
                0o040_000 => {
                    fs.nodes.insert(key, Node::Directory);
                }
                // What the archive carries as an entry, the phone's unzip turns
                // into a link with the entry's bytes as its target. Nothing
                // checks where that target points: this is the trick.
                0o120_000 => {
                    let mut target = String::new();
                    entry
                        .read_to_string(&mut target)
                        .map_err(|error| upload(error.to_string()))?;
                    fs.nodes.insert(key, Node::Symlink(target));
                }
                _ => {
                    let mut bytes = Vec::new();
                    entry
                        .read_to_end(&mut bytes)
                        .map_err(|error| upload(error.to_string()))?;
                    fs.nodes.insert(key, Node::File(bytes));
                }
            }
        }
        Ok(())
    }
}

impl AssetMover for Phone {
    fn move_assets(&self, assets: &[(String, String)]) -> Result<()> {
        let interrupted = |identifier: &str| AirliftError::Move {
            detail: format!("the sync dropped out moving {identifier}"),
        };

        // The ledger is what the phone goes by: it is asked for each asset and
        // answers for the ones it recognises. An asset it was never told about
        // is not there to move.
        let named = self.ledger();
        for (identifier, _) in assets {
            if !named.iter().any(|name| name == identifier) {
                return Err(interrupted(identifier));
            }
        }

        if assets.is_empty() {
            return Ok(());
        }
        // One plan per call, in the order the phases are expected to make them:
        // a test that lists its moves is also asserting how many there are.
        let plan = self.plans.borrow_mut().pop_front().unwrap_or(Plan::All);
        if plan == Plan::Refused {
            return Err(interrupted(&assets[0].0));
        }

        let mut fs = self.fs.borrow_mut();
        for (index, (identifier, destination)) in assets.iter().enumerate() {
            // An identifier is relative to the Airlock root; a destination is
            // relative to Media. That asymmetry is the phone's, not this app's.
            let (source_parent, source_leaf) =
                split_last(identifier).ok_or_else(|| interrupted(identifier))?;
            let source_parent = fs
                .resolve(AIRLOCK, source_parent, true)
                .ok_or_else(|| interrupted(identifier))?;
            let source = if source_parent.is_empty() {
                source_leaf.to_owned()
            } else {
                format!("{source_parent}/{source_leaf}")
            };

            let (target_parent, target_leaf) =
                split_last(destination).ok_or_else(|| interrupted(destination))?;
            let target_parent = fs
                .resolve(MEDIA, target_parent, true)
                .ok_or_else(|| interrupted(destination))?;
            let target = if target_parent.is_empty() {
                target_leaf.to_owned()
            } else {
                format!("{target_parent}/{target_leaf}")
            };

            // A move: the destination gets the node and the source stops
            // existing. Note that this unlinks the source whatever it was, so
            // moving the link moves the link and not what it points at.
            let node = fs
                .nodes
                .remove(&source)
                .ok_or_else(|| interrupted(identifier))?;
            fs.nodes.insert(target, node);

            if plan == Plan::Interrupted(index + 1) {
                return Err(interrupted(identifier));
            }
        }
        Ok(())
    }
}

/// The names the escape is allowed to leave in Media, in the order a run makes
/// them.
fn leftovers_of(phone: &Phone) -> Vec<String> {
    phone
        .media_names()
        .into_iter()
        .filter(|name| {
            ["airlift-src-", "airlift-link-", "airlift-recovered-"]
                .iter()
                .any(|prefix| name.starts_with(prefix))
        })
        .collect()
}

#[test]
fn a_read_returns_the_artwork_and_leaves_the_phone_exactly_as_it_was() {
    let phone = Phone::new();
    phone.add_card(b"the artwork the card came with");
    let before = phone.device();

    let read = phone
        .airlift()
        .read_file(CARD, ARTWORK[0], 1)
        .expect("the read should have worked");

    assert_eq!(read.data, b"the artwork the card came with");
    assert!(!read.card_needs_repair);
    assert_eq!(read.copy_at, None);
    // Not "the artwork is unchanged": the whole device is unchanged, down to the
    // ledger entry the sync had to be talked into handing over, and nothing of
    // this app's is left lying in Media.
    assert_eq!(phone.device(), before);
    assert!(leftovers_of(&phone).is_empty());
}

#[test]
fn a_write_replaces_the_artwork_and_leaves_the_phone_exactly_as_it_was() {
    let phone = Phone::new();
    phone.add_card(b"the artwork the card came with");
    let mut expected = phone.device();

    phone
        .airlift()
        .write_file(CARD, ARTWORK[0], b"a new face", 1)
        .expect("the write should have worked");

    expected.nodes.insert(
        format!("{}/{}", CARD.trim_start_matches('/'), ARTWORK[0]),
        Node::File(b"a new face".to_vec()),
    );
    assert_eq!(phone.card_artwork().as_deref(), Some(&b"a new face"[..]));
    assert_eq!(phone.device(), expected);
}

#[test]
fn all_three_artwork_files_go_out_in_one_move() {
    let phone = Phone::new();
    phone.add_card(b"the artwork the card came with");

    let files: Vec<(String, Vec<u8>)> = ARTWORK
        .iter()
        .map(|leaf| ((*leaf).to_owned(), format!("new {leaf}").into_bytes()))
        .collect();
    phone
        .airlift()
        .write_files(CARD, &files, 1)
        .expect("the batch should have worked");

    let fs = phone.device();
    for (leaf, bytes) in &files {
        assert_eq!(
            fs.file(&format!("{}/{}", CARD.trim_start_matches('/'), leaf))
                .as_ref(),
            Some(bytes),
            "{leaf} did not take the new bytes"
        );
    }
    assert!(leftovers_of(&phone).is_empty());
}

#[test]
fn a_move_the_phone_refuses_leaves_the_card_alone_and_nothing_in_media() {
    let phone = Phone::new();
    phone.add_card(b"the artwork the card came with");
    phone.plans(&[Plan::Refused]);
    let before = phone.device();

    let error = phone
        .airlift()
        .read_file(CARD, ARTWORK[0], 1)
        .expect_err("a refused move is not a read");

    assert!(matches!(error, AirliftError::Move { .. }), "{error}");
    assert_eq!(phone.device(), before);
    assert!(leftovers_of(&phone).is_empty());
}

#[test]
fn a_move_the_phone_half_finished_keeps_the_only_copy_and_names_it() {
    let phone = Phone::new();
    phone.add_card(b"the artwork the card came with");
    // Both assets land, and then the sync drops out. The card directory has been
    // emptied of that artwork by the move and the bytes are now in Media, so
    // this is the one case where cleanup must not tidy up.
    phone.plans(&[Plan::Interrupted(2)]);
    let before = phone.device();

    let error = phone
        .airlift()
        .read_file(CARD, ARTWORK[0], 1)
        .expect_err("an interrupted move is not a read");

    let AirliftError::Interrupted { kept, .. } = &error else {
        panic!("expected the copy to be named, got {error}");
    };
    assert!(kept.starts_with("airlift-recovered-"), "{kept}");

    let fs = phone.device();
    assert_eq!(
        fs.file(&format!("{MEDIA}/{kept}")).as_deref(),
        Some(&b"the artwork the card came with"[..]),
        "the only copy of the artwork should still be on the phone"
    );
    // Everything that belongs to this app and is not that copy is gone, and the
    // ledger is back the way the phone had it.
    assert_eq!(leftovers_of(&phone), vec![kept.clone()]);
    assert_eq!(
        fs.file(&format!("{MEDIA}/Books/Books.plist")),
        before
            .nodes
            .get(&format!("{MEDIA}/Books/Books.plist"))
            .and_then(|node| match node {
                Node::File(bytes) => Some(bytes.clone()),
                _ => None,
            })
    );
}

#[test]
fn a_read_that_could_not_be_written_back_still_hands_the_bytes_over() {
    let phone = Phone::new();
    phone.add_card(b"the artwork the card came with");
    // The read's own move lands, and the move that puts the bytes back is refused
    // every time it is tried -- writing a card file is worth three attempts, so
    // that is how many refusals this has to be.
    phone.plans(&[Plan::All, Plan::Refused, Plan::Refused, Plan::Refused]);

    let read = phone
        .airlift()
        .read_file(CARD, ARTWORK[0], 1)
        .expect("the bytes were read, so they come back");

    assert_eq!(read.data, b"the artwork the card came with");
    assert!(
        read.card_needs_repair,
        "the card did not get its bytes back"
    );
    let copy = read.copy_at.expect("the phone still has a copy");
    assert_eq!(
        phone.device().file(&format!("{MEDIA}/{copy}")).as_deref(),
        Some(&b"the artwork the card came with"[..]),
    );

    // And the card can be repaired from those bytes, which is what the caller is
    // expected to do next.
    phone.plans(&[Plan::All]);
    phone
        .airlift()
        .write_file(CARD, ARTWORK[0], &read.data, 3)
        .expect("the repair should have worked");
    assert_eq!(phone.card_artwork(), Some(read.data));
}

#[test]
fn invalidation_takes_the_rendered_faces_off_the_phone() {
    let phone = Phone::new();
    phone.add_card(b"artwork");
    assert_eq!(phone.cache_names().len(), 3);

    let invalidated = phone
        .airlift()
        .invalidate_cache("TestCard")
        .expect("invalidation should not fail");

    assert!(invalidated, "the cache was there to invalidate");
    assert!(phone.cache_names().is_empty());
    assert_eq!(phone.card_artwork().as_deref(), Some(&b"artwork"[..]));
    assert!(leftovers_of(&phone).is_empty());
}

#[test]
fn a_card_with_no_cache_is_not_a_failure() {
    let phone = Phone::new();
    phone.add_file(&format!("{CARD}/{}", ARTWORK[0]), b"artwork");

    let invalidated = phone
        .airlift()
        .invalidate_cache("TestCard")
        .expect("there was nothing to invalidate, which is not an error");

    assert!(!invalidated);
    assert!(leftovers_of(&phone).is_empty());
}

#[test]
fn an_interrupted_run_can_be_listed_and_swept() {
    let phone = Phone::new();
    phone.add_file(
        &format!("{MEDIA}/airlift-src-00112233445566778899/payload"),
        b"x",
    );
    phone.add_file(
        &format!("{MEDIA}/airlift-recovered-00112233445566778899"),
        b"y",
    );
    phone.add_file(&format!("{MEDIA}/airlift-src-notatoken/payload"), b"z");
    let airlift = phone.airlift();

    let found = airlift.leftovers().expect("listing is safe");
    let names: Vec<&str> = found
        .iter()
        .map(|leftover| leftover.name.as_str())
        .collect();
    assert_eq!(
        names,
        vec![
            "airlift-recovered-00112233445566778899",
            "airlift-src-00112233445566778899",
            "airlift-src-notatoken",
        ]
    );
    assert!(found[0].ours && found[1].ours);
    assert!(
        !found[2].ours,
        "a name without a token is listed but is not one of ours"
    );

    airlift
        .remove_leftover("airlift-src-00112233445566778899")
        .expect("sweeping our own name");
    assert_eq!(airlift.leftovers().unwrap().len(), 2);
    // A sweep is exactly where a wrong argument would delete a phone's data.
    assert!(airlift.remove_leftover("Books").is_err());
    assert!(airlift.remove_leftover("airlift-src-../../Books").is_err());
    assert_eq!(airlift.leftovers().unwrap().len(), 2);
}

#[test]
fn the_phone_will_not_hand_over_an_asset_the_ledger_does_not_name() {
    let phone = Phone::new();
    phone.add_card(b"artwork");

    // This is the rule the staging ledger exists for: without it the phone has
    // nothing to resolve and the move fails -- which is why a lost ledger write
    // shows up as a move failure and not as silence.
    let refused = phone.move_assets(&[(
        "../../../Library/Passes/Cards/TestCard.pkpass/front".to_owned(),
        "airlift-recovered-00112233445566778899".to_owned(),
    )]);
    assert!(refused.is_err());
}
