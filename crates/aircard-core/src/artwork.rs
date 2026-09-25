//! Turning a picture into the three files a card's face is made of.
//!
//! A card's face is two copies of one PNG plus a PDF of the same picture, and
//! Wallet renders the PDF in preference to the PNGs -- which is why a flash that
//! replaced only the PNGs once left a card showing its old face while every step
//! reported success. The three have to be built together and written together.
//!
//! The conversion is macOS's own `sips`: it ships with every Mac, it reads every
//! format a person is likely to pick, and using it means this app carries no
//! image library of its own to disagree with the phone about colour profiles.
//! That is also why nothing here runs anywhere but a Mac, and says so.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::assets::{PDF_ASSET_NAME, PNG_ASSET_NAMES};
use crate::{paths, slug};

/// The size of a card's face, in pixels. A real card's artwork is 1536×969, and
/// a picture is fitted to it rather than letterboxed: a card face has no margin
/// to hide bars in.
pub const FACE_WIDTH: u32 = 1536;
pub const FACE_HEIGHT: u32 = 969;

/// Where the conversions live on a Mac.
const SIPS: &str = "/usr/bin/sips";

#[derive(Debug, thiserror::Error)]
pub enum ArtworkError {
    #[error("{} could not be read", .path.display())]
    Unreadable { path: PathBuf },
    #[error("the size of {} could not be read", .path.display())]
    NoSize { path: PathBuf },
    #[error("{} could not be written", .path.display())]
    Unwritable { path: PathBuf },
    #[error("sips failed on {}: {detail}", .path.display())]
    Sips { path: PathBuf, detail: String },
    #[error("this needs macOS, where sips lives")]
    Unsupported,
}

pub type Result<T, E = ArtworkError> = std::result::Result<T, E>;

/// The three files a flash writes, in the order [`crate::assets`] names them.
///
/// Built from one picture: the PNG is used for both raster faces as it stands,
/// which is what the phone expects and what keeps the two identical, and the PDF
/// is the same picture converted.
pub fn build_assets(png: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    supported()?;
    let scratch = Scratch::new()?;
    let source = scratch.join("card.png");
    let converted = scratch.join("card.pdf");
    fs::write(&source, png).map_err(|_| ArtworkError::Unwritable {
        path: source.clone(),
    })?;

    sips(&[
        "-s".into(),
        "format".into(),
        "pdf".into(),
        source.clone().into_os_string(),
        "--out".into(),
        converted.clone().into_os_string(),
    ])
    .map_err(|detail| ArtworkError::Sips {
        path: source.clone(),
        detail,
    })?;
    let pdf = fs::read(&converted).map_err(|_| ArtworkError::Unreadable {
        path: converted.clone(),
    })?;

    let mut assets: Vec<(String, Vec<u8>)> = PNG_ASSET_NAMES
        .iter()
        .map(|name| ((*name).to_owned(), png.to_vec()))
        .collect();
    assets.push((PDF_ASSET_NAME.to_owned(), pdf));
    Ok(assets)
}

/// Fits a picture to a card face and writes it out as PNG.
///
/// The picture is scaled up until it covers the face and the middle is cropped
/// out of it. Scaling to fit instead would leave bars along two edges, and a card
/// face shows them.
pub fn prepare_png(source: &Path, destination: &Path) -> Result<()> {
    supported()?;
    if !source.is_file() {
        return Err(ArtworkError::Unreadable {
            path: source.to_owned(),
        });
    }
    let (width, height) = size(source)?;
    let (scaled_width, scaled_height) = cover_size(width, height);

    let scratch = Scratch::new()?;
    let scaled = scratch.join("scaled.png");
    sips(&[
        "-s".into(),
        "format".into(),
        "png".into(),
        "-z".into(),
        scaled_height.to_string().into(),
        scaled_width.to_string().into(),
        source.into(),
        "--out".into(),
        scaled.clone().into_os_string(),
    ])
    .map_err(|detail| ArtworkError::Sips {
        path: source.to_owned(),
        detail,
    })?;
    sips(&[
        "-c".into(),
        FACE_HEIGHT.to_string().into(),
        FACE_WIDTH.to_string().into(),
        scaled.clone().into_os_string(),
        "--out".into(),
        destination.into(),
    ])
    .map_err(|detail| ArtworkError::Sips {
        path: scaled.clone(),
        detail,
    })?;
    Ok(())
}

/// A smaller copy of a picture, for showing in a list.
pub fn thumbnail_png(png: &[u8], longest_side: u32) -> Result<Vec<u8>> {
    supported()?;
    let scratch = Scratch::new()?;
    let source = scratch.join("full.png");
    let small = scratch.join("small.png");
    fs::write(&source, png).map_err(|_| ArtworkError::Unwritable {
        path: source.clone(),
    })?;
    sips(&[
        "-Z".into(),
        longest_side.to_string().into(),
        source.clone().into_os_string(),
        "--out".into(),
        small.clone().into_os_string(),
    ])
    .map_err(|detail| ArtworkError::Sips {
        path: source.clone(),
        detail,
    })?;
    fs::read(&small).map_err(|_| ArtworkError::Unreadable { path: small })
}

/// The size to scale a picture to before the middle is cropped out of it: the
/// smallest one that still covers the face on both sides.
pub fn cover_size(width: u32, height: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (FACE_WIDTH, FACE_HEIGHT);
    }
    let across = FACE_WIDTH as f64 / width as f64;
    let down = FACE_HEIGHT as f64 / height as f64;
    let cover = if across > down { across } else { down };
    let scaled_width = (width as f64 * cover).ceil() as u32;
    let scaled_height = (height as f64 * cover).ceil() as u32;
    (scaled_width.max(FACE_WIDTH), scaled_height.max(FACE_HEIGHT))
}

/// A card's face as it was read off a phone, kept so the list can show it.
///
/// Reading one costs a trip across the security boundary and takes about a
/// minute, so it is kept until somebody asks for it to go.
pub struct ArtworkCache {
    root: PathBuf,
}

impl ArtworkCache {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The default place: `~/.aircard_cache`, the same directory the previous
    /// implementation used, so a face read by either is visible to both.
    pub fn default_root() -> Self {
        Self::new(paths::artwork_cache_root())
    }

    pub fn file(&self, udid: &str, card_hash: &str) -> PathBuf {
        self.root
            .join(slug(udid))
            .join(format!("{}.png", slug(card_hash)))
    }

    pub fn contains(&self, udid: &str, card_hash: &str) -> bool {
        paths::file_non_empty(&self.file(udid, card_hash))
    }

    pub fn read(&self, udid: &str, card_hash: &str) -> Option<Vec<u8>> {
        let path = self.file(udid, card_hash);
        let bytes = fs::read(&path).ok()?;
        (!bytes.is_empty()).then_some(bytes)
    }

    pub fn save(&self, udid: &str, card_hash: &str, png: &[u8]) -> std::io::Result<()> {
        let path = self.file(udid, card_hash);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, png)
    }

    pub fn forget(&self, udid: &str, card_hash: &str) {
        let _ = fs::remove_file(self.file(udid, card_hash));
    }
}

/// The pixel size of a picture, as `sips` reports it.
fn size(path: &Path) -> Result<(u32, u32)> {
    let output = Command::new(SIPS)
        .arg("-g")
        .arg("pixelWidth")
        .arg("-g")
        .arg("pixelHeight")
        .arg(path)
        .output()
        .map_err(|_| ArtworkError::NoSize {
            path: path.to_owned(),
        })?;
    let text = String::from_utf8_lossy(&output.stdout);
    let value = |key: &str| -> Option<u32> {
        text.lines()
            .find_map(|line| line.trim().strip_prefix(key))
            .and_then(|rest| rest.trim().trim_start_matches(':').trim().parse().ok())
    };
    match (value("pixelWidth"), value("pixelHeight")) {
        (Some(width), Some(height)) => Ok((width, height)),
        _ => Err(ArtworkError::NoSize {
            path: path.to_owned(),
        }),
    }
}

fn sips(arguments: &[std::ffi::OsString]) -> std::result::Result<(), String> {
    let output = Command::new(SIPS)
        .args(arguments)
        .output()
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    Err(if detail.is_empty() {
        format!("it exited with {}", output.status)
    } else {
        detail
    })
}

#[cfg(target_os = "macos")]
fn supported() -> Result<()> {
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn supported() -> Result<()> {
    Err(ArtworkError::Unsupported)
}

/// Scratch space for one conversion, removed when the conversion is over.
///
/// A named directory rather than a temporary-file library: the tools being used
/// take paths, and a directory that deletes itself is the whole of what is
/// needed.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new() -> Result<Self> {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let path =
            std::env::temp_dir().join(format!("aircard-artwork-{}-{unique}", std::process::id()));
        fs::create_dir_all(&path).map_err(|_| ArtworkError::Unwritable { path: path.clone() })?;
        Ok(Self { path })
    }

    fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An 8×8 PNG, so the tests need nothing from outside the repository.
    const TINY_PNG: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 8, 0, 0, 0, 8, 8, 2,
        0, 0, 0, 75, 109, 41, 220, 0, 0, 0, 108, 73, 68, 65, 84, 120, 218, 13, 201, 65, 1, 0, 48,
        8, 3, 49, 148, 160, 164, 74, 170, 132, 231, 169, 64, 9, 74, 170, 104, 203, 55, 85, 69, 23,
        42, 92, 76, 177, 197, 21, 41, 170, 154, 110, 212, 184, 153, 102, 155, 107, 210, 63, 68, 11,
        9, 139, 17, 43, 78, 68, 63, 76, 27, 25, 155, 49, 107, 206, 196, 63, 134, 30, 52, 120, 152,
        97, 135, 27, 50, 63, 150, 94, 180, 120, 153, 101, 151, 91, 178, 63, 142, 62, 116, 248, 152,
        99, 143, 59, 114, 63, 66, 7, 5, 135, 9, 27, 46, 36, 60, 176, 44, 84, 129, 249, 166, 11,
        121, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];

    #[test]
    fn a_picture_is_scaled_until_it_covers_the_face() {
        // A wide picture: the height is what has to reach the face, and the
        // width comes out past it and gets cropped.
        let (width, height) = cover_size(3072, 1000);
        assert_eq!(height, FACE_HEIGHT);
        assert!(width > FACE_WIDTH, "a wide picture has width to spare");

        // A tall one the other way round.
        let (width, height) = cover_size(1000, 2000);
        assert_eq!(width, FACE_WIDTH);
        assert!(height > FACE_HEIGHT, "a tall picture has height to spare");

        // Already the right shape: the cover is the face itself.
        assert_eq!(
            cover_size(FACE_WIDTH, FACE_HEIGHT),
            (FACE_WIDTH, FACE_HEIGHT)
        );
    }

    #[test]
    fn a_size_nobody_can_scale_is_still_a_face() {
        assert_eq!(cover_size(0, 0), (FACE_WIDTH, FACE_HEIGHT));
        assert_eq!(cover_size(FACE_WIDTH, 0), (FACE_WIDTH, FACE_HEIGHT));
    }

    #[test]
    fn the_cache_round_trips_and_forgets() {
        let scratch = Scratch::new().expect("a scratch directory");
        let cache = ArtworkCache::new(scratch.join("cache"));
        assert!(!cache.contains("00008150-000E0D060C04401C", "a/b+c="));
        cache
            .save("00008150-000E0D060C04401C", "a/b+c=", b"a face")
            .expect("saving");
        assert!(cache.contains("00008150-000E0D060C04401C", "a/b+c="));
        assert_eq!(
            cache.read("00008150-000E0D060C04401C", "a/b+c="),
            Some(b"a face".to_vec())
        );
        // A hash with a slash in it is a real card hash, and it has to land in
        // one file rather than two directories.
        let path = cache.file("00008150-000E0D060C04401C", "a/b+c=");
        // The hash is one name inside the device's directory, not a directory of
        // its own: a slash in a card number must not become a path.
        assert_eq!(
            path.parent(),
            Some(
                scratch
                    .join("cache")
                    .join("00008150-000E0D060C04401C")
                    .as_path()
            )
        );
        cache.forget("00008150-000E0D060C04401C", "a/b+c=");
        assert!(!cache.contains("00008150-000E0D060C04401C", "a/b+c="));
    }

    #[test]
    fn an_empty_file_is_not_a_kept_face() {
        let scratch = Scratch::new().expect("a scratch directory");
        let cache = ArtworkCache::new(scratch.join("cache"));
        cache
            .save("udid", "hash", b"")
            .expect("an empty save still writes");
        assert!(!cache.contains("udid", "hash"));
        assert_eq!(cache.read("udid", "hash"), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_three_files_are_two_copies_of_the_png_and_a_pdf() {
        let assets = build_assets(TINY_PNG).expect("the assets should have been built");
        let names: Vec<&str> = assets.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "cardBackgroundCombined@3x.png",
                "cardBackgroundCombined@2x.png",
                "cardBackgroundCombined.pdf",
            ]
        );
        assert_eq!(assets[0].1, TINY_PNG);
        assert_eq!(
            assets[1].1, TINY_PNG,
            "the two raster faces are the same picture"
        );
        assert!(
            assets[2].1.starts_with(b"%PDF"),
            "the third file has to be a PDF"
        );
        assert!(assets[2].1.len() > 1000, "and a real one, not a stub");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_picture_comes_out_exactly_the_size_of_a_face() {
        let scratch = Scratch::new().expect("a scratch directory");
        let source = scratch.join("source.png");
        fs::write(&source, TINY_PNG).expect("the fixture");
        let fitted = scratch.join("fitted.png");
        prepare_png(&source, &fitted).expect("the picture should have been fitted");
        assert_eq!(size(&fitted).expect("a size"), (FACE_WIDTH, FACE_HEIGHT));

        let small = thumbnail_png(&fs::read(&fitted).expect("the fitted picture"), 64)
            .expect("a thumbnail");
        assert!(small.starts_with(b"\x89PNG"));
        let (width, height) = size_of_bytes(&small).expect("a size");
        assert_eq!(
            width.max(height),
            64,
            "the longest side is the one asked for"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_missing_picture_is_an_error_and_not_an_empty_face() {
        let scratch = Scratch::new().expect("a scratch directory");
        let error = prepare_png(&scratch.join("nothing-here.png"), &scratch.join("out.png"))
            .expect_err("there is no picture to fit");
        assert!(matches!(error, ArtworkError::Unreadable { .. }), "{error}");
    }

    /// The size of a picture that is already in memory, by writing it out first.
    #[cfg(target_os = "macos")]
    fn size_of_bytes(png: &[u8]) -> Result<(u32, u32)> {
        let scratch = Scratch::new()?;
        let path = scratch.join("measure.png");
        fs::write(&path, png).map_err(|_| ArtworkError::Unwritable { path: path.clone() })?;
        size(&path)
    }

    #[test]
    fn a_failed_tool_run_says_which_file_it_was_working_on() {
        let error = ArtworkError::Sips {
            path: PathBuf::from("/tmp/fitted.png"),
            detail: "no such file".to_owned(),
        };
        assert_eq!(
            error.to_string(),
            "sips failed on /tmp/fitted.png: no such file"
        );
    }
}
