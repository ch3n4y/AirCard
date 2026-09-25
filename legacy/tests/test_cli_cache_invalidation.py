"""The CLI entry-point in aircard.py must invalidate every Wallet cache leaf.

Before this fix, aircard.py defined its own CACHE_FILES = ["FrontFace", "Preview"],
missing the "PlaceHolder" entry present in the canonical card_assets.CACHE_FILES.
Skipping PlaceHolder left stale rendered card artwork visible after a skin change,
because Wallet only rebuilds faces whose cache entries are absent.

Additionally, aircard.py still overwrote cache files with b"corrupted" bytes instead
of unlinking them via remove_files / invalidate_cache, which stopped working
correctly on iOS 27 (fixed in aircard_backend.py by commit 7e8979b but never
propagated to the standalone CLI).
"""

import unittest

import aircard
import card_assets


class CLICacheFilesTest(unittest.TestCase):
    """aircard.CACHE_FILES must match card_assets.CACHE_FILES exactly."""

    def test_cli_cache_files_includes_placeholder(self):
        """PlaceHolder must be in the cache list used by the CLI."""
        self.assertIn("PlaceHolder", aircard.CACHE_FILES)

    def test_cli_cache_files_matches_canonical(self):
        """aircard.CACHE_FILES must be identical to card_assets.CACHE_FILES."""
        self.assertEqual(set(aircard.CACHE_FILES), set(card_assets.CACHE_FILES))


if __name__ == "__main__":
    unittest.main()
