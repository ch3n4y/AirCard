import io
import json
import pathlib
import tempfile
import unittest
from contextlib import redirect_stdout
from unittest.mock import Mock, patch

import aircard
import aircard_backend

CARD = "M6nDwZrkYbFlsodLgCbvyFZQ1cc="
UDID = "00008150-001405803C47801C"


def canonical_assets():
    return [(name, f"original:{name}".encode()) for name in aircard.BACKED_UP_ASSETS]


class ArtworkCommandTests(unittest.TestCase):
    """The picture the app shows for a card.

    Resolved from this Mac whenever it can be: reading it off the phone moves the
    file out of the card and writes it back, which is only worth doing when the
    user asks.
    """

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        root = pathlib.Path(self.tmp.name)
        for target in ("BACKUPS_ROOT", "ARTWORK_CACHE_ROOT"):
            p = patch.object(aircard, target, root / target.lower())
            p.start()
            self.addCleanup(p.stop)

    def _run(self, **kwargs):
        buf = io.StringIO()
        with redirect_stdout(buf):
            result = aircard_backend.cmd_artwork(UDID, CARD, **kwargs)
        events = [json.loads(line) for line in buf.getvalue().strip().splitlines() if line.strip()]
        return result, events

    def _cache(self):
        return aircard.card_artwork_cache_dir(UDID, CARD)

    def test_an_unknown_card_offers_nothing_and_touches_no_device(self):
        reader = Mock(return_value=b"x")
        with patch.object(aircard_backend, "read_file", reader):
            ok, events = self._run()

        self.assertFalse(ok)
        self.assertEqual(events[-1]["code"], "artwork.not_saved")
        reader.assert_not_called()

    def test_the_saved_original_is_used_without_touching_the_device(self):
        assets = canonical_assets()
        self.assertTrue(aircard.save_card_backup(UDID, CARD, assets))
        reader = Mock()
        with patch.object(aircard_backend, "read_file", reader):
            ok, events = self._run()

        self.assertTrue(ok)
        self.assertEqual(events[-1]["source"], "backup")
        self.assertTrue(events[-1]["path"].endswith(aircard.PNG_ASSET_NAMES[0]))
        reader.assert_not_called()

    def test_a_backup_beats_a_copy_left_by_an_earlier_read(self):
        """The backup is the original; a fetched copy is whatever was on the card."""
        self.assertTrue(aircard.save_card_backup(UDID, CARD, canonical_assets()))
        self._cache().mkdir(parents=True)
        (self._cache() / aircard.PDF_ASSET_NAME).write_bytes(b"%PDF-stale")

        ok, events = self._run()

        self.assertTrue(ok)
        self.assertEqual(events[-1]["source"], "backup")

    def test_a_previous_read_is_reused_instead_of_reading_again(self):
        self._cache().mkdir(parents=True)
        (self._cache() / aircard.PDF_ASSET_NAME).write_bytes(b"%PDF-cached")
        reader = Mock()
        with patch.object(aircard_backend, "read_file", reader):
            ok, events = self._run()

        self.assertTrue(ok)
        self.assertEqual(events[-1]["source"], "cache")
        self.assertTrue(events[-1]["path"].endswith(aircard.PDF_ASSET_NAME))
        reader.assert_not_called()

    def test_fetch_reads_the_card_and_keeps_a_copy(self):
        reader = Mock(side_effect=[payload for _, payload in canonical_assets()])
        with patch.object(aircard_backend, "read_file", reader):
            ok, events = self._run(fetch=True)

        self.assertTrue(ok)
        self.assertEqual(events[-1]["source"], "device")
        self.assertEqual(reader.call_count, len(aircard.BACKED_UP_ASSETS))
        self.assertEqual(
            sorted(p.name for p in self._cache().iterdir()),
            sorted(aircard.BACKED_UP_ASSETS),
        )

        # Second time round it costs the phone nothing.
        with patch.object(aircard_backend, "read_file", Mock()) as again:
            ok, events = self._run()
        self.assertTrue(ok)
        self.assertEqual(events[-1]["source"], "cache")
        again.assert_not_called()

    def test_a_partial_read_is_still_a_picture(self):
        """One drawable file is enough to tell two rows apart."""
        reader = Mock(side_effect=[b"png-bytes", None, None])
        with patch.object(aircard_backend, "read_file", reader):
            ok, events = self._run(fetch=True)

        self.assertTrue(ok)
        self.assertEqual(events[-1]["source"], "device")
        self.assertEqual(events[-1]["path"], str(self._cache() / aircard.PNG_ASSET_NAMES[0]))
        self.assertIn("artwork.partial", [event.get("code") for event in events])

    def test_an_unreadable_card_is_reported_and_leaves_nothing_behind(self):
        with patch.object(aircard_backend, "read_file", Mock(side_effect=RuntimeError("boom"))):
            ok, events = self._run(fetch=True)

        self.assertFalse(ok)
        self.assertEqual(events[-1]["code"], "artwork.read_failed")
        # An empty directory would make the next look-up think there is a picture.
        self.assertFalse(self._cache().exists())

    def test_forget_drops_the_copy(self):
        self._cache().mkdir(parents=True)
        (self._cache() / aircard.PDF_ASSET_NAME).write_bytes(b"%PDF-cached")

        ok, events = self._run(forget=True)

        self.assertFalse(ok)
        self.assertEqual(events[-1]["code"], "artwork.not_saved")
        self.assertFalse(self._cache().exists())

    def test_the_cache_is_kept_per_device(self):
        other = "00008130-000000000000000A"
        self._cache().mkdir(parents=True)
        (self._cache() / aircard.PDF_ASSET_NAME).write_bytes(b"%PDF-cached")

        self.assertIsNotNone(aircard.card_artwork_file(aircard.card_artwork_cache_dir(UDID, CARD)))
        self.assertIsNone(aircard.card_artwork_file(aircard.card_artwork_cache_dir(other, CARD)))


if __name__ == "__main__":
    unittest.main()
