//! Getting bytes across the boundary AFC cannot cross.
//!
//! A Wallet card lives at `/var/mobile/Library/Passes/Cards/<hash>.pkpass`, and
//! its rendered faces at `<hash>.cache`. AFC reaches `/var/mobile/Media` and
//! nothing else: it refuses `..`, it will not follow the one symlink that could
//! get out, and it has no `MAKE_LINK` opcode with which to plant another -- on
//! iOS 27 that opcode answers "operation not supported", so the symlink route is
//! closed. What is left is the phone's own sync service: hand it a zip that the
//! phone's unzip expands into a symlink, ask AirTraffic to *move* that symlink
//! out into Media, and from then on the card's directory is reachable through it.
//!
//! The pieces, in the order they happen:
//!
//! `snapshot`  read the `Books` ledger the sync service keeps, so it can be put
//!             back byte for byte. The phone treats that ledger as authoritative
//!             and will not hand over an asset the ledger does not mention.
//! `stage`     upload the archive (see [`super::payload`]) and write a ledger
//!             naming the assets to move.
//! `move`      AirTraffic resolves each asset and *moves* it. Move, not copy: the
//!             source stops existing afterwards, so anything that reads a moved
//!             asset has to put it back.
//! `finish`    take the link and the staging tree away again, restore the ledger,
//!             and verify every one of them is gone -- whatever went wrong.
//!
//! Two rules this module exists to keep:
//!
//! * **One session at a time.** An AFC session and an AirTraffic connection to
//!   the same phone are never held at once: the previous implementation shipped
//!   them as separate short-lived processes, and two concurrent AFC sessions to
//!   one device abort the process outright. That is why the phases below open a
//!   media session, drop it, and only then reach for AirTraffic.
//! * **Never delete something whose contents are unknown.** A move the phone
//!   half-finished leaves a card's only copy sitting in Media, so cleanup keeps
//!   it and names it rather than tidying it away.

use std::thread::sleep;
use std::time::Duration;

use aircard_apple_ffi::DeviceError;

use super::payload::{
    self, build_archive, build_archive_multi, build_books, is_safe_relative_path, AIRLOCK_ROOT,
    TRACKED_BOOKS_DIRECTORIES, TRACKED_BOOKS_FILES,
};

/// The largest card file this app will pull out of a phone. Card artwork is a few
/// hundred kilobytes; the bound is here so that a path that surprises us cannot
/// fill memory.
const EXPORT_LIMIT: u64 = 32 * 1024 * 1024;

/// What the ledger snapshot reads under, per file and in total. The ledger is a
/// few kilobytes.
const LEDGER_FILE_LIMIT: u64 = 128 * 1024 * 1024;
const LEDGER_TOTAL_LIMIT: u64 = 256 * 1024 * 1024;

/// How long the phone is given to settle before the ledger is put back, matching
/// the previous implementation. Restoring too eagerly makes the phone rewrite the
/// ledger from its own in-memory copy and lose the restore.
const SETTLE: Duration = Duration::from_secs(2);

/// The pause between retries, multiplied by the attempt number.
const PACE: Duration = Duration::from_millis(300);

/// The payload a read puts in the archive. Its contents never reach the phone's
/// card directory -- the archive exists for its symlink -- but a zip entry needs
/// bytes, and naming them makes a staging tree on a phone legible.
const READ_PLACEHOLDER: &[u8] = b"aircard-backup-staging";

/// The same, for the archive whose only job is to relocate the link so files can
/// be moved out through it.
const REMOVE_PLACEHOLDER: &[u8] = b"aircard-v2";

/// The deepest staging tree that will be walked during cleanup.
const TREE_LIMIT: u32 = 32;

/// What a path is, as far as the phone will say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Directory,
    /// Never followed: the one symlink in play points at a Wallet card.
    Symlink,
    Other,
}

impl Kind {
    /// Maps AFC's `st_ifmt` spelling. Anything unrecognised is `Other` rather
    /// than absent, because "there is something there" and "there is nothing
    /// there" lead to different decisions.
    pub fn from_afc(spelling: &str) -> Kind {
        match spelling {
            "S_IFREG" => Kind::File,
            "S_IFDIR" => Kind::Directory,
            "S_IFLNK" => Kind::Symlink,
            _ => Kind::Other,
        }
    }
}

/// The phone's Media tree, which is all AFC can see.
///
/// Every path here is relative to `/var/mobile/Media` and none of them contains
/// `..`, because AFC refuses such a path. Implementations are sessions: opening
/// one is the caller's decision, and dropping it must close it.
pub trait Media {
    fn exists(&mut self, path: &str) -> bool;
    fn kind(&mut self, path: &str) -> Option<Kind>;
    fn read(&mut self, path: &str, limit: u64) -> Result<Vec<u8>, AirliftError>;
    fn write(&mut self, path: &str, data: &[u8]) -> Result<(), AirliftError>;
    /// Creates the directory and any missing parent; succeeds when it exists.
    fn create_directory(&mut self, path: &str) -> Result<(), AirliftError>;
    fn remove(&mut self, path: &str) -> Result<(), AirliftError>;
    fn list(&mut self, path: &str) -> Result<Vec<String>, AirliftError>;
}

/// Opens a media session when a phase needs one, and only then.
pub trait MediaSource {
    fn open(&self) -> Result<Box<dyn Media + '_>, AirliftError>;
}

/// AirTraffic, which is the only thing that can move a file across the boundary.
pub trait AssetMover {
    /// Moves each `(asset identifier, destination)` pair.
    ///
    /// AirTraffic resolves the identifier against the Airlock root and *moves*
    /// the thing it finds: the destination ends up with the bytes and the source
    /// stops existing. A call that returns an error may still have moved some of
    /// the list, so nothing is ever removed on the strength of this result alone.
    fn move_assets(&self, assets: &[(String, String)]) -> Result<(), AirliftError>;
}

/// The zip conduit, which is how a staging tree appears in Media in one shot.
pub trait ArchiveUpload {
    fn upload(&self, media_subdir: &str, archive: &[u8]) -> Result<(), AirliftError>;
}

#[derive(Debug, thiserror::Error)]
pub enum AirliftError {
    #[error("{0}")]
    Device(#[from] DeviceError),
    #[error("could not put the staging archive on the phone: {detail}")]
    Upload { detail: String },
    #[error("the phone did not move the staged assets: {detail}")]
    Move { detail: String },
    #[error("{path} is not a plain file name")]
    Leaf { path: String },
    #[error("{path} is not a path this app will send")]
    Path { path: String },
    #[error("{path} on the phone is not a regular file")]
    UnexpectedKind { path: String },
    #[error("{path} on the phone could not be read")]
    Unreadable { path: String },
    #[error("{path} on the phone was refused: {detail}")]
    Refused { path: String, detail: String },
    #[error("the phone moved the staged assets and then failed: {detail}, and the only copy of what it moved is {kept}")]
    Interrupted { detail: String, kept: String },
    #[error("the Books ledger on the phone changed while it was being staged")]
    BooksChanged,
    #[error("something from an earlier run is still in the way: {path}")]
    NotFresh { path: String },
    #[error("cleanup left {} thing(s) on the phone: {}", .failures.len(), .failures.join(", "))]
    Cleanup { failures: Vec<String> },
}

impl AirliftError {
    /// Whether trying the whole operation again could plausibly go differently.
    ///
    /// Only the two failures that are about a session talking to the phone are
    /// worth repeating. A `NotFresh` path will still be there, a ledger that
    /// changed will not match on a second look either, and a cleanup that failed
    /// needs a person: retrying it would march on to the next phase over the
    /// wreckage of the last one.
    fn is_retryable(&self) -> bool {
        matches!(
            self,
            AirliftError::Upload { .. } | AirliftError::Move { .. }
        )
    }
}

pub type Result<T, E = AirliftError> = std::result::Result<T, E>;

/// What a move left behind, once everything that could be put back was.
#[derive(Debug, Default, Clone)]
pub struct Cleanup {
    pub removed: Vec<String>,
    /// Things left in Media because they may hold the only copy of something.
    /// Kept ones make a cleanup incomplete: they need a person.
    pub kept: Vec<String>,
    pub failures: Vec<String>,
}

impl Cleanup {
    pub fn is_complete(&self) -> bool {
        self.failures.is_empty() && self.kept.is_empty()
    }
}

/// What a read brought back.
#[derive(Debug, Clone)]
pub struct ReadBack {
    pub data: Vec<u8>,
    /// Set when the card did not get these bytes back, so Wallet will keep
    /// showing whatever it showed before until they are written again. `data` is
    /// good either way: this is the difference between "saved" and "saved and
    /// the phone is as it was".
    pub card_needs_repair: bool,
    /// Where the phone still holds a copy, when it does. Cleanup keeps that file
    /// exactly because the card directory did not take the bytes back, which
    /// makes it the only copy for as long as the card is not repaired.
    pub copy_at: Option<String>,
}

/// The `Books` ledger as it was, so it can be put back exactly.
///
/// Only a complete snapshot is worth having: a half-read one would restore a
/// ledger the phone never had, and would be indistinguishable from a good one.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Ledger {
    /// Each tracked file with its bytes, or `None` when it was not there.
    files: Vec<(&'static str, Option<Vec<u8>>)>,
    /// Each tracked directory and whether it was there.
    directories: Vec<(&'static str, bool)>,
}

impl Ledger {
    /// Whether the phone still looks exactly like this snapshot. Checked before
    /// staging, because a ledger that moved under us means someone else's sync
    /// is in flight and our names would land in the middle of it.
    fn matches(&self, media: &mut dyn Media) -> bool {
        for (path, expected) in &self.files {
            if media.exists(path) != expected.is_some() {
                return false;
            }
            let Some(bytes) = expected else { continue };
            match media.read(path, LEDGER_FILE_LIMIT) {
                Ok(observed) if observed == *bytes => {}
                _ => return false,
            }
        }
        for (path, expected) in &self.directories {
            if media.exists(path) != *expected {
                return false;
            }
            if *expected && media.kind(path) != Some(Kind::Directory) {
                return false;
            }
        }
        true
    }
}

/// The three names one attempt uses, all carrying the same token.
///
/// The token is why an interrupted run can never be mistaken for the current one:
/// a staging tree is ours only if its name ends in the token we generated.
struct Staging {
    token: String,
}

impl Staging {
    fn new() -> Self {
        Self {
            token: payload::token(),
        }
    }

    fn source(&self) -> String {
        payload::source_name(&self.token)
    }

    fn link(&self) -> String {
        payload::link_name(&self.token)
    }

    fn recovered(&self) -> String {
        payload::recovered_name(&self.token)
    }

    /// Where the archive parks its symlink, named the way AirTraffic wants it:
    /// relative to the Airlock root, which is two levels below Media.
    fn link_asset(&self) -> String {
        format!("../../{}/p0/p1/p2/link", self.source())
    }

    /// Everything this attempt may have created in Media, for the sweep that
    /// clears up after an interrupted run.
    fn owned(&self) -> [String; 3] {
        [self.source(), self.link(), self.recovered()]
    }
}

/// Something left in Media by an interrupted run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leftover {
    pub name: String,
    pub kind: Kind,
    /// Whether this name carries the 20-hex token the app generates. A name that
    /// does not could belong to another tool, and is listed but never swept
    /// without being asked for by name.
    pub ours: bool,
}

/// The escape, wired to one phone.
pub struct Airlift<S, A, U> {
    source: S,
    mover: A,
    upload: U,
}

impl<S: MediaSource, A: AssetMover, U: ArchiveUpload> Airlift<S, A, U> {
    pub fn new(source: S, mover: A, upload: U) -> Self {
        Self {
            source,
            mover,
            upload,
        }
    }

    /// Copies a file out of a card directory.
    ///
    /// The phone's copy is *moved* into Media to be read, so this writes the same
    /// bytes back before it returns: on the success path the card is left exactly
    /// as it was, and when it is not the bytes still come back and
    /// `card_needs_repair` says the card has to be written again.
    pub fn read_file(&self, target: &str, leaf: &str, retries: usize) -> Result<ReadBack> {
        check_target(target)?;
        check_leaf(leaf)?;
        let path = join_path(target, leaf);
        self.attempt(retries, || self.read_once(target, leaf, &path))
    }

    fn read_once(&self, target: &str, leaf: &str, path: &str) -> Result<ReadBack> {
        let plan = Staging::new();
        let ledger = self.snapshot()?;
        let assets = vec![plan.link_asset(), airlock_relative(path)];
        let destinations = vec![plan.link(), plan.recovered()];
        let archive = build_archive(target, READ_PLACEHOLDER);

        if let Err(error) = self.stage(&plan, &ledger, &archive, &assets) {
            self.finish(&plan, &ledger, true);
            return Err(error);
        }
        if let Err(error) = self.move_assets(&plan, &assets, &destinations) {
            // The phone may have moved the file and then failed, or nothing at
            // all. Cleanup takes the link and the staging tree back -- both of
            // which belong to this app -- and keeps whatever is at the recovered
            // name, because if the move did happen that is the only copy left of
            // something a person cares about.
            let cleanup = self.finish(&plan, &ledger, false);
            return Err(match cleanup.kept.first() {
                Some(kept) => AirliftError::Interrupted {
                    detail: error.to_string(),
                    kept: kept.clone(),
                },
                None => error,
            });
        }

        let data = self.read_moved(&plan.recovered())?;
        // The original is out of the card directory and this is the only copy of
        // it anywhere else, so it goes back before anything else is attempted.
        let restored = self.write_file(target, leaf, &data, 3);
        // Only once the card has its bytes back is the copy in Media expendable.
        // When it does not, that copy is the only one left on the phone, so
        // cleanup keeps it and hands its name back rather than tidying away the
        // last of something a person cares about.
        let cleanup = self.finish(&plan, &ledger, restored.is_ok());
        Ok(ReadBack {
            data,
            card_needs_repair: restored.is_err() || !cleanup.is_complete(),
            copy_at: cleanup.kept.first().cloned(),
        })
    }

    /// Writes one file into a card directory, overwriting whatever is there.
    pub fn write_file(
        &self,
        target: &str,
        leaf: &str,
        payload: &[u8],
        retries: usize,
    ) -> Result<()> {
        self.write_files(target, &[(leaf.to_owned(), payload.to_vec())], retries)
    }

    /// Writes several files into one card directory in a single move.
    ///
    /// A card's artwork is three files that Wallet renders as a set; writing them
    /// one at a time leaves it briefly holding a mixture of two cards.
    pub fn write_files(
        &self,
        target: &str,
        files: &[(String, Vec<u8>)],
        retries: usize,
    ) -> Result<()> {
        check_target(target)?;
        if files.is_empty() {
            return Ok(());
        }
        for (leaf, _) in files {
            check_leaf(leaf)?;
        }
        self.attempt(retries, || self.write_once(target, files))
    }

    fn write_once(&self, target: &str, files: &[(String, Vec<u8>)]) -> Result<()> {
        let plan = Staging::new();
        let ledger = self.snapshot()?;
        let mut assets = vec![plan.link_asset()];
        let mut destinations = vec![plan.link()];
        for (index, (leaf, _)) in files.iter().enumerate() {
            // The first payload also goes out under the plain `payload` name,
            // and that is the one this asks for: it is the name the archive has
            // always carried, so an older phone reading the compatibility entry
            // sees the same single-file staging it used to.
            assets.push(format!(
                "../../{}/{name}",
                plan.source(),
                name = if index == 0 {
                    "payload".to_owned()
                } else {
                    format!("payload_{index}")
                }
            ));
            destinations.push(join_path(&plan.link(), leaf));
        }
        let archive = build_archive_multi(target, files);

        if let Err(error) = self.stage(&plan, &ledger, &archive, &assets) {
            self.finish(&plan, &ledger, true);
            return Err(error);
        }
        self.move_and_clean(&plan, &ledger, &assets, &destinations)?;
        let cleanup = self.finish(&plan, &ledger, true);
        if !cleanup.is_complete() {
            return Err(AirliftError::Cleanup {
                failures: cleanup
                    .failures
                    .iter()
                    .chain(cleanup.kept.iter())
                    .cloned()
                    .collect(),
            });
        }
        Ok(())
    }

    /// Deletes files out of a card directory, which is the only way to make
    /// Wallet render a card again.
    ///
    /// Overwriting a rendered face with arbitrary bytes is not enough -- recent
    /// iOS keeps serving the old one -- so the face has to be unlinked, and
    /// unlinking it means moving it out through the link first.
    pub fn remove_files(&self, target: &str, leaves: &[String], retries: usize) -> Result<()> {
        check_target(target)?;
        if leaves.is_empty() {
            return Ok(());
        }
        for leaf in leaves {
            check_leaf(leaf)?;
        }
        self.attempt(retries, || self.remove_once(target, leaves))
    }

    fn remove_once(&self, target: &str, leaves: &[String]) -> Result<()> {
        let plan = Staging::new();
        let ledger = self.snapshot()?;
        let archive = build_archive(target, REMOVE_PLACEHOLDER);

        let mut assets = vec![plan.link_asset()];
        let mut destinations = vec![plan.link()];
        for (index, leaf) in leaves.iter().enumerate() {
            assets.push(format!("../../{}/{}", plan.link(), leaf));
            destinations.push(format!("{}/removed-{index}", plan.source()));
        }

        if let Err(error) = self.stage(&plan, &ledger, &archive, &assets) {
            self.finish(&plan, &ledger, true);
            return Err(error);
        }
        self.move_and_clean(&plan, &ledger, &assets, &destinations)?;

        // The moved files now sit inside the staging tree, where AFC can reach
        // them, so deleting them for real is what actually invalidates the cache.
        // A face that was not there needs no invalidating either, so how many
        // arrived is not checked: the cache is gone either way, which is all the
        // caller asked for.
        let cleanup = self.finish(&plan, &ledger, true);
        if !cleanup.is_complete() {
            return Err(AirliftError::Cleanup {
                failures: cleanup
                    .failures
                    .iter()
                    .chain(cleanup.kept.iter())
                    .cloned()
                    .collect(),
            });
        }
        Ok(())
    }

    /// Removes the rendered faces of one card so Wallet rebuilds them.
    ///
    /// Returns whether anything was demonstrably invalidated. A `false` is not a
    /// failure and must not fail a flash: whether the card has a cache at all is
    /// not knowable from here -- the directory is one AFC cannot see, so there is
    /// no way to look before trying -- and a phone that will not move a face is
    /// also what an absent cache looks like.
    pub fn invalidate_cache(&self, card_hash: &str) -> Result<bool> {
        if card_hash.is_empty()
            || card_hash.contains('/')
            || card_hash.contains('\0')
            || card_hash == "."
            || card_hash == ".."
        {
            return Err(AirliftError::Path {
                path: card_hash.to_owned(),
            });
        }
        let leaves: Vec<String> = aircard_core::assets::CACHE_FILES
            .iter()
            .map(|leaf| (*leaf).to_owned())
            .collect();
        let mut invalidated = false;
        let mut failures = Vec::new();
        for target in card_cache_directories(card_hash) {
            match self.remove_files(&target, &leaves, 3) {
                Ok(()) => invalidated = true,
                Err(AirliftError::Move { .. }) => {}
                Err(error) => failures.push(format!("{target}: {error}")),
            }
        }
        if failures.is_empty() {
            Ok(invalidated)
        } else {
            Err(AirliftError::Cleanup { failures })
        }
    }

    /// Lists what an interrupted run left in Media.
    ///
    /// These are the phone's own bytes: a `recovered` file may hold the only copy
    /// of a card's artwork. Listing is safe; removing is the caller's call.
    pub fn leftovers(&self) -> Result<Vec<Leftover>> {
        let mut media = self.source.open()?;
        let mut found = Vec::new();
        for name in media.list(".")? {
            let Some(prefix) = [
                payload::SOURCE_PREFIX,
                payload::LINK_PREFIX,
                payload::RECOVERED_PREFIX,
            ]
            .into_iter()
            .find(|prefix| name.starts_with(prefix)) else {
                continue;
            };
            // A name is ours when what follows the prefix is a token this app
            // generates. Anything else with the right prefix came from somewhere
            // else and is listed, but not as ours.
            let ours = payload::generated_token(&name, prefix).is_some();
            let Some(kind) = media.kind(&name) else {
                continue;
            };
            found.push(Leftover { name, kind, ours });
        }
        found.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(found)
    }

    /// Removes one named leftover, after its contents have been dealt with.
    ///
    /// Only a name that starts with one of this app's prefixes and carries no
    /// path separator is accepted: a sweep is exactly the place where a wrong
    /// argument would delete a phone's data.
    pub fn remove_leftover(&self, name: &str) -> Result<()> {
        let known = [
            payload::SOURCE_PREFIX,
            payload::LINK_PREFIX,
            payload::RECOVERED_PREFIX,
        ]
        .iter()
        .any(|prefix| name.starts_with(prefix));
        if !known || name.contains('/') || name == "." || name == ".." {
            return Err(AirliftError::Path {
                path: name.to_owned(),
            });
        }
        let mut media = self.source.open()?;
        let mut failures = Vec::new();
        remove_tree(&mut *media, name, 0, &mut failures);
        if failures.is_empty() {
            Ok(())
        } else {
            Err(AirliftError::Cleanup { failures })
        }
    }

    /// Reads the `Books` ledger into a snapshot that can be restored from.
    fn snapshot(&self) -> Result<Ledger> {
        let mut media = self.source.open()?;
        let mut files = Vec::new();
        let mut total = 0_u64;
        for path in TRACKED_BOOKS_FILES {
            let present = media.exists(path);
            if present && media.kind(path) != Some(Kind::File) {
                return Err(AirliftError::UnexpectedKind {
                    path: path.to_owned(),
                });
            }
            let bytes = if present {
                let bytes =
                    media
                        .read(path, LEDGER_FILE_LIMIT)
                        .map_err(|_| AirliftError::Unreadable {
                            path: path.to_owned(),
                        })?;
                total += bytes.len() as u64;
                if total > LEDGER_TOTAL_LIMIT {
                    return Err(AirliftError::Unreadable {
                        path: path.to_owned(),
                    });
                }
                Some(bytes)
            } else {
                None
            };
            files.push((path, bytes));
        }
        let mut directories = Vec::new();
        for path in TRACKED_BOOKS_DIRECTORIES {
            let present = media.exists(path);
            if present && media.kind(path) != Some(Kind::Directory) {
                return Err(AirliftError::UnexpectedKind {
                    path: path.to_owned(),
                });
            }
            directories.push((path, present));
        }
        Ok(Ledger { files, directories })
    }

    /// Puts the ledger back as it was and reports what could not be put back.
    fn restore_ledger(&self, ledger: &Ledger) -> Vec<String> {
        let mut failures = Vec::new();
        let Ok(mut media) = self.source.open() else {
            failures.push("Books: the phone could not be reached".to_owned());
            return failures;
        };
        for (path, expected) in &ledger.files {
            match expected {
                Some(bytes) => {
                    if !ensure_directory(&mut *media, parent(path)) {
                        failures.push(format!("{path}: parent directory"));
                        continue;
                    }
                    if media.write(path, bytes).is_err() || !media.exists(path) {
                        failures.push((*path).to_owned());
                    }
                }
                None => remove_if_present(&mut *media, path, &mut failures),
            }
        }
        // Deepest first, so a directory is empty by the time its own turn comes.
        for (path, expected) in ledger.directories.iter().rev() {
            if !*expected {
                remove_if_present(&mut *media, path, &mut failures);
            }
        }
        if failures.is_empty() && !ledger.matches(&mut *media) {
            failures.push("Books: it does not match the snapshot".to_owned());
        }
        failures
    }

    /// Checks the ledger is still what was snapshotted, then uploads the archive
    /// and writes the ledger that names the assets to move.
    ///
    /// The checks come first and they are strict: staging over a ledger that has
    /// moved would interleave this app's names with another sync's, and staging
    /// over a leftover would mean moving something this app did not create.
    fn stage(
        &self,
        plan: &Staging,
        ledger: &Ledger,
        archive: &[u8],
        assets: &[String],
    ) -> Result<()> {
        if !payload::generated_names_match(&plan.source(), &plan.link(), &plan.recovered()) {
            return Err(AirliftError::Path {
                path: plan.source(),
            });
        }
        {
            let mut media = self.source.open()?;
            if !ledger.matches(&mut *media) {
                return Err(AirliftError::BooksChanged);
            }
            for path in plan.owned() {
                if media.exists(&path) {
                    return Err(AirliftError::NotFresh { path });
                }
            }
        }

        self.upload.upload(&plan.source(), archive)?;

        let books = build_books(assets);
        let mut media = self.source.open()?;
        if !media.exists(&plan.source())
            || !media.exists(&format!("{}/p0/p1/p2/link", plan.source()))
            || !(media.exists(&format!("{}/payload", plan.source()))
                || media.exists(&format!("{}/payload_0", plan.source())))
        {
            return Err(AirliftError::Upload {
                detail: format!("{} did not come back from the phone", plan.source()),
            });
        }
        if !ensure_directory(&mut *media, "Books") || !ensure_directory(&mut *media, "Books/Sync") {
            return Err(AirliftError::Upload {
                detail: "the Books directory could not be made".to_owned(),
            });
        }
        media.write("Books/Sync/Books.plist", &books)?;
        Ok(())
    }

    fn move_assets(
        &self,
        _plan: &Staging,
        assets: &[String],
        destinations: &[String],
    ) -> Result<()> {
        debug_assert_eq!(assets.len(), destinations.len());
        let pairs: Vec<(String, String)> = assets
            .iter()
            .cloned()
            .zip(destinations.iter().cloned())
            .collect();
        self.mover.move_assets(&pairs)
    }

    /// Moves the staged assets and, when the move fails, takes the staging back
    /// down before returning the error.
    ///
    /// Nothing of the phone's is at stake on these paths: every destination is
    /// either a name this app generated or the file it was about to write over,
    /// and the bytes to write it again are in hand. Leaving the staging tree
    /// behind instead is how an interrupted run used to fill a phone with
    /// directories nobody would ever remove.
    fn move_and_clean(
        &self,
        plan: &Staging,
        ledger: &Ledger,
        assets: &[String],
        destinations: &[String],
    ) -> Result<()> {
        match self.move_assets(plan, assets, destinations) {
            Ok(()) => Ok(()),
            Err(error) => {
                let cleanup = self.finish(plan, ledger, true);
                Err(with_cleanup(error, cleanup))
            }
        }
    }

    fn read_moved(&self, recovered: &str) -> Result<Vec<u8>> {
        let mut media = self.source.open()?;
        if !media.exists(recovered) {
            return Err(AirliftError::Unreadable {
                path: recovered.to_owned(),
            });
        }
        media
            .read(recovered, EXPORT_LIMIT)
            .map_err(|_| AirliftError::Unreadable {
                path: recovered.to_owned(),
            })
    }

    /// Takes the staging tree and the first two names away and restores the
    /// ledger, then verifies all of it is gone.
    ///
    /// `discard_recovered` says whether the file at the recovered name is known
    /// to be a copy. When it is not -- because a move may have been interrupted
    /// with the card's only copy in it -- it is kept and reported.
    fn finish(&self, plan: &Staging, ledger: &Ledger, discard_recovered: bool) -> Cleanup {
        let mut cleanup = Cleanup::default();
        let Ok(mut media) = self.source.open() else {
            cleanup
                .failures
                .push("the phone could not be reached for cleanup".to_owned());
            return cleanup;
        };

        let link = plan.link();
        let recovered = plan.recovered();
        let source = plan.source();

        remove_if_present(&mut *media, &link, &mut cleanup.failures);
        if media.exists(&recovered) {
            if discard_recovered {
                remove_if_present(&mut *media, &recovered, &mut cleanup.failures);
            } else {
                cleanup.kept.push(recovered.clone());
            }
        }
        remove_tree(&mut *media, &source, 0, &mut cleanup.failures);
        cleanup.removed.push(link.clone());
        cleanup.removed.push(source.clone());

        sleep(SETTLE);
        let failures = self.restore_ledger(ledger);
        cleanup.failures.extend(failures);

        for path in [link.as_str(), source.as_str()] {
            if media.exists(path) {
                cleanup.failures.push(format!("{path}: still there"));
            }
        }
        cleanup
    }

    /// Runs `run` up to `retries` times, and only again when the failure is one
    /// that a second try could change.
    fn attempt<T>(&self, retries: usize, mut run: impl FnMut() -> Result<T>) -> Result<T> {
        let attempts = retries.max(1);
        let mut last = None;
        for index in 1..=attempts {
            match run() {
                Ok(value) => return Ok(value),
                Err(error) => {
                    let retryable = error.is_retryable();
                    last = Some(error);
                    if !retryable || index == attempts {
                        break;
                    }
                    sleep(PACE * index as u32);
                }
            }
        }
        Err(last.expect("every path above records the error it is leaving with"))
    }
}

/// The path AirTraffic needs, which starts from the Airlock root rather than
/// from Media: two more levels up, and it is allowed to begin with `..` because
/// the phone resolves it, not AFC.
fn airlock_relative(path: &str) -> String {
    relative_path(AIRLOCK_ROOT, path)
}

/// `to` expressed relative to `from`, the way the phone's own sync does it.
fn relative_path(from: &str, to: &str) -> String {
    let from = components(from);
    let to = components(to);
    let shared = from
        .iter()
        .zip(to.iter())
        .take_while(|(left, right)| left == right)
        .count();
    let mut parts = vec![".."; from.len() - shared];
    parts.extend_from_slice(&to[shared..]);
    if parts.is_empty() {
        ".".to_owned()
    } else {
        parts.join("/")
    }
}

fn components(path: &str) -> Vec<&str> {
    path.split('/').filter(|part| !part.is_empty()).collect()
}

fn join_path(directory: &str, leaf: &str) -> String {
    format!("{}/{}", directory.trim_end_matches('/'), leaf)
}

/// The directory a path sits in, as a Media-relative path.
fn parent(path: &str) -> &str {
    match path.rfind('/') {
        Some(index) if index > 0 => &path[..index],
        _ => "",
    }
}

fn check_leaf(leaf: &str) -> Result<()> {
    if leaf.is_empty() || leaf.contains('/') || leaf == "." || leaf == ".." {
        return Err(AirliftError::Leaf {
            path: leaf.to_owned(),
        });
    }
    Ok(())
}

/// Where a card's own files live on the phone.
///
/// Outside Media, which is the entire problem: there is no AFC path to it, and
/// that is why everything else in this module exists.
pub fn card_directory(card_hash: &str) -> String {
    format!("/var/mobile/Library/Passes/Cards/{card_hash}.pkpass")
}

/// Where Wallet has rendered a card.
///
/// Both extensions turn up in the wild, and a face left behind in the one that
/// was not cleared keeps the old artwork on the card, so anything that removes
/// faces has to do both.
pub fn card_cache_directories(card_hash: &str) -> [String; 2] {
    [".cache", ".pkcache"]
        .map(|extension| format!("/var/mobile/Library/Passes/Cards/{card_hash}{extension}"))
}

fn check_target(target: &str) -> Result<()> {
    if !target.starts_with('/') || !is_safe_relative_path(&target[1..]) {
        return Err(AirliftError::Path {
            path: target.to_owned(),
        });
    }
    Ok(())
}

/// Creates a directory if it is not there. The phone answers "already exists"
/// with a failure, which is why the check comes first.
fn ensure_directory(media: &mut dyn Media, path: &str) -> bool {
    if path.is_empty() {
        return true;
    }
    media.exists(path) || media.create_directory(path).is_ok()
}

/// A failed move, plus whatever cleanup could not take back down.
///
/// The move's own reason comes first because it is the cause; the rest is what
/// is still on the phone and needs a person.
fn with_cleanup(error: AirliftError, cleanup: Cleanup) -> AirliftError {
    if cleanup.is_complete() {
        return error;
    }
    let mut failures = vec![error.to_string()];
    failures.extend(cleanup.failures);
    failures.extend(cleanup.kept);
    AirliftError::Cleanup { failures }
}

fn remove_if_present(media: &mut dyn Media, path: &str, failures: &mut Vec<String>) {
    if !media.exists(path) {
        return;
    }
    if media.remove(path).is_err() || media.exists(path) {
        failures.push(path.to_owned());
    }
}

/// Removes a staging tree, deepest child first.
///
/// A symlink is unlinked and never descended into: the one symlink in play points
/// at a Wallet card, and a recursive delete that followed it would take the card
/// this app exists to protect.
fn remove_tree(media: &mut dyn Media, path: &str, depth: u32, failures: &mut Vec<String>) {
    if depth > TREE_LIMIT {
        failures.push(format!("{path}: nested too deeply"));
        return;
    }
    let Some(kind) = media.kind(path) else {
        return;
    };
    if kind == Kind::Directory {
        let mut children = match media.list(path) {
            Ok(children) => children,
            Err(_) => {
                failures.push(format!("{path}: could not list"));
                return;
            }
        };
        children.sort();
        for child in children {
            remove_tree(media, &join_path(path, &child), depth + 1, failures);
        }
    }
    if media.remove(path).is_err() || media.exists(path) {
        failures.push(path.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_path_climbs_out_of_airlock_to_the_card_directory() {
        assert_eq!(
            relative_path(AIRLOCK_ROOT, "/var/mobile/Library/Passes/Cards/AbC=.pkpass"),
            "../../../Library/Passes/Cards/AbC=.pkpass"
        );
    }

    #[test]
    fn relative_path_handles_a_shared_prefix_and_a_path_that_is_already_relative() {
        assert_eq!(relative_path("/a/b/c", "/a/b/c"), ".");
        assert_eq!(relative_path("/a/b", "/a/b/d/e"), "d/e");
        assert_eq!(relative_path("/a/b/c", "/a"), "../..");
        assert_eq!(relative_path("/a", "/a/b/c"), "b/c");
        assert_eq!(relative_path("", "a/b"), "a/b");
    }

    #[test]
    fn airlock_relative_is_the_asset_identifier_the_phone_wants() {
        assert_eq!(
            airlock_relative("/var/mobile/Library/Passes/Cards/AbC=.pkpass"),
            "../../../Library/Passes/Cards/AbC=.pkpass"
        );
    }

    #[test]
    fn a_leaf_must_be_one_file_name() {
        assert!(check_leaf("FrontFace").is_ok());
        assert!(check_leaf("cardBackgroundCombined@3x.png").is_ok());
        for bad in ["", ".", "..", "a/b", "/etc/passwd"] {
            assert!(check_leaf(bad).is_err(), "{bad} was accepted");
        }
    }

    #[test]
    fn a_target_must_be_absolute_and_climb_free() {
        assert!(check_target("/var/mobile/Library/Passes/Cards/A.pkpass").is_ok());
        for bad in [
            "",
            "relative/path",
            "/var/../etc/passwd",
            "/var/mobile/Library/Passes/Cards/",
        ] {
            assert!(check_target(bad).is_err(), "{bad} was accepted");
        }
    }

    #[test]
    fn kinds_map_from_afc_spelling_and_an_unknown_one_is_not_absence() {
        assert_eq!(Kind::from_afc("S_IFREG"), Kind::File);
        assert_eq!(Kind::from_afc("S_IFDIR"), Kind::Directory);
        assert_eq!(Kind::from_afc("S_IFLNK"), Kind::Symlink);
        assert_eq!(Kind::from_afc("S_IFSOCK"), Kind::Other);
    }

    #[test]
    fn a_join_never_doubles_a_separator() {
        assert_eq!(join_path("/a/b", "c"), "/a/b/c");
        assert_eq!(join_path("/a/b/", "c"), "/a/b/c");
        assert_eq!(parent("/a/b/c"), "/a/b");
        assert_eq!(parent("Books"), "");
    }

    #[test]
    fn only_a_session_failure_is_worth_repeating() {
        assert!(AirliftError::Upload {
            detail: String::new()
        }
        .is_retryable());
        assert!(AirliftError::Move {
            detail: String::new()
        }
        .is_retryable());
        assert!(!AirliftError::BooksChanged.is_retryable());
        assert!(!AirliftError::NotFresh {
            path: String::new()
        }
        .is_retryable());
        assert!(!AirliftError::Cleanup {
            failures: Vec::new()
        }
        .is_retryable());
        assert!(!AirliftError::Unreadable {
            path: String::new()
        }
        .is_retryable());
    }
}
