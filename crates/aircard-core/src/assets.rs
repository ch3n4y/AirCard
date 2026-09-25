//! The three files a flash overwrites, and the render caches that make Wallet
//! rebuild a card face.
//!
//! One module owns these names. The ported CLI had them in three places at once
//! and two of those copies left the combined PDF out, which meant a flash
//! replaced the PNGs and Wallet kept rendering the old skin from the PDF.

/// Raster card faces. Written under both names with the same bytes.
pub const PNG_ASSET_NAMES: [&str; 2] = [
    "cardBackgroundCombined@3x.png",
    "cardBackgroundCombined@2x.png",
];

/// The vector card face. Wallet prefers this over the PNGs when both are present.
pub const PDF_ASSET_NAME: &str = "cardBackgroundCombined.pdf";

/// Everything a flash overwrites, which is what a saved original has to cover.
pub const BACKED_UP_ASSETS: [&str; 3] = [
    "cardBackgroundCombined@3x.png",
    "cardBackgroundCombined@2x.png",
    "cardBackgroundCombined.pdf",
];

/// Rendered faces Wallet keeps per card. Removing them forces a rebuild.
pub const CACHE_FILES: [&str; 3] = ["FrontFace", "PlaceHolder", "Preview"];

/// Does this name belong to the set a backup has to hold?
pub fn is_backed_up(name: &str) -> bool {
    BACKED_UP_ASSETS.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_backed_up_set_is_the_pngs_plus_the_pdf() {
        let mut expected: Vec<&str> = PNG_ASSET_NAMES.to_vec();
        expected.push(PDF_ASSET_NAME);
        assert_eq!(BACKED_UP_ASSETS.to_vec(), expected);
    }

    #[test]
    fn the_names_are_plain_file_names() {
        // They are passed to the device as leaves, so a slash would be rejected.
        for name in BACKED_UP_ASSETS.iter().chain(CACHE_FILES.iter()) {
            assert!(!name.contains('/'), "{name} must be a plain file name");
        }
    }
}
