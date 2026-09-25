import io
import json
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import Mock, patch

import aircard
import aircard_backend

CARD = "M6nDwZrkYbFlsodLgCbvyFZQ1cc="
UDID = "00008150-001405803C47801C"


def canonical_assets():
    """Every file a flash overwrites, which is what a backup has to hold."""
    return [(name, f"original:{name}".encode()) for name in aircard.BACKED_UP_ASSETS]


def png_assets():
    """The artwork a card can have without the combined PDF a flash adds."""
    return [n for n in aircard.BACKED_UP_ASSETS if n.endswith(".png")]


def write_legacy_backup(names):
    """Lays down a backup directory holding only ``names``.

    save_card_backup refuses to create one of these, but an older build stored
    whatever it managed to read, so a directory like this can already be on a
    user's disk.
    """
    d = aircard.card_backup_dir(UDID, CARD)
    d.mkdir(parents=True, exist_ok=True)
    for name in names:
        (d / name).write_bytes(b"original")
    return d


class BackupCoverageTests(unittest.TestCase):
    def test_backup_covers_every_file_a_flash_writes(self):
        """A file left behind puts the skin straight back on a restored card."""
        import card_assets
        written = {name for name, _ in [
            *[(n, b"") for n in card_assets.PNG_ASSET_NAMES],
            (card_assets.PDF_ASSET_NAME, b""),
        ]}
        missing = written - set(aircard.BACKED_UP_ASSETS)
        self.assertEqual(
            missing, set(),
            f"a flash writes {sorted(missing)} but backup/restore does not cover it",
        )


class BackupStorageTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        p = patch.object(aircard, "BACKUPS_ROOT", Path(self.tmp.name))
        p.start()
        self.addCleanup(p.stop)

    def test_round_trip(self):
        assets = canonical_assets()
        self.assertFalse(aircard.has_card_backup(UDID, CARD))
        self.assertTrue(aircard.save_card_backup(UDID, CARD, assets))
        self.assertTrue(aircard.has_card_backup(UDID, CARD))
        self.assertEqual(aircard.read_card_backup(UDID, CARD), sorted(assets))

    def test_hashes_with_slashes_survive_the_filesystem(self):
        # Real card hashes contain / and +, neither of which is a legal folder name.
        awkward = "ab/cd+ef=="
        self.assertTrue(aircard.save_card_backup(UDID, awkward, canonical_assets()))
        self.assertTrue(aircard.has_card_backup(UDID, awkward))
        self.assertEqual(aircard.list_backed_up_cards(UDID), [awkward])

    def test_empty_payloads_are_not_a_backup(self):
        self.assertFalse(aircard.save_card_backup(UDID, CARD, []))
        self.assertFalse(aircard.save_card_backup(UDID, CARD, [("x.png", b"")]))
        self.assertFalse(aircard.has_card_backup(UDID, CARD))

    def test_artwork_missing_one_file_is_refused_outright(self):
        """Half a backup looks restorable and then restores the card halfway."""
        partial = canonical_assets()[:-1]
        self.assertFalse(aircard.save_card_backup(UDID, CARD, partial))
        self.assertFalse(aircard.has_card_backup(UDID, CARD))
        self.assertEqual(aircard.read_card_backup(UDID, CARD), [])

    def test_a_refused_backup_does_not_leave_an_earlier_one_behind(self):
        """A leftover file would make the next has_card_backup say yes."""
        partial = canonical_assets()[:-1]
        write_legacy_backup([partial[0][0]])
        self.assertFalse(aircard.save_card_backup(UDID, CARD, partial))
        self.assertFalse(aircard.card_backup_dir(UDID, CARD).exists())
        self.assertFalse(aircard.has_card_backup(UDID, CARD))

    def test_an_incomplete_backup_on_disk_is_not_offered(self):
        """The state an older build could leave behind must not look usable."""
        write_legacy_backup(png_assets())
        self.assertFalse(aircard.has_card_backup(UDID, CARD))
        self.assertEqual(aircard.list_backed_up_cards(UDID), [])

    def test_backups_are_kept_per_device(self):
        self.assertTrue(aircard.save_card_backup(UDID, CARD, canonical_assets()))
        other = "00008130-000000000000000A"
        self.assertFalse(aircard.has_card_backup(other, CARD))
        self.assertEqual(aircard.list_backed_up_cards(other), [])


class RestoreCommandTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        p = patch.object(aircard, "BACKUPS_ROOT", Path(self.tmp.name))
        p.start()
        self.addCleanup(p.stop)

    def _events(self, fn, *args):
        buf = io.StringIO()
        with redirect_stdout(buf):
            result = fn(*args)
        events = [json.loads(line) for line in buf.getvalue().strip().splitlines() if line.strip()]
        return result, events

    def test_restore_without_a_backup_touches_nothing(self):
        """The dangerous case: nothing saved, so nothing may be written."""
        batch, single, remove = Mock(), Mock(), Mock()
        with patch.object(aircard_backend, "write_files_batch", batch), \
                patch.object(aircard_backend, "write_file", single), \
                patch.object(aircard_backend, "remove_files", remove):
            ok, events = self._events(aircard_backend.cmd_restore, UDID, CARD)
        self.assertFalse(ok)
        self.assertEqual(events[-1]["code"], "restore.no_backup")
        batch.assert_not_called()
        single.assert_not_called()
        remove.assert_not_called()

    def test_restore_writes_originals_and_clears_caches(self):
        assets = canonical_assets()
        self.assertTrue(aircard.save_card_backup(UDID, CARD, assets))
        batch = Mock(return_value=True)
        remove = Mock(return_value=True)
        with patch.object(aircard_backend, "write_files_batch", batch), \
                patch.object(aircard_backend, "remove_files", remove):
            ok, events = self._events(aircard_backend.cmd_restore, UDID, CARD)
        self.assertTrue(ok)
        self.assertEqual(events[-1]["code"], "restore.done")
        target, payload = batch.call_args[0][1], batch.call_args[0][2]
        self.assertIn(CARD, target)
        self.assertEqual([n for n, _ in payload], sorted(n for n, _ in assets))
        # A complete backup leaves nothing to take off the card, so only the two
        # rendered-face directories are cleared.
        self.assertEqual(remove.call_count, 2)

    def test_restore_removes_artwork_the_backup_never_held(self):
        """A PNG-only original: the flash's PDF has to go, or Wallet renders it."""
        write_legacy_backup(png_assets())
        remove = Mock(return_value=True)
        with patch.object(aircard_backend, "write_files_batch", Mock(return_value=True)), \
                patch.object(aircard_backend, "remove_files", remove):
            ok, events = self._events(aircard_backend.cmd_restore, UDID, CARD)
        self.assertTrue(ok)
        self.assertEqual(events[-1]["code"], "restore.done")
        leaves = [call.args[2] for call in remove.call_args_list]
        self.assertIn([aircard.PDF_ASSET_NAME], leaves)
        self.assertEqual(remove.call_count, 3)

    def test_restore_fails_when_a_leftover_cannot_be_removed(self):
        """Reporting success would hide a skin that is still being rendered."""
        write_legacy_backup(png_assets())
        remove = Mock(side_effect=[False, True, True])
        with patch.object(aircard_backend, "write_files_batch", Mock(return_value=True)), \
                patch.object(aircard_backend, "remove_files", remove):
            ok, events = self._events(aircard_backend.cmd_restore, UDID, CARD)
        self.assertFalse(ok)
        self.assertEqual(events[-1]["code"], "restore.failed")
        self.assertIn(aircard.PDF_ASSET_NAME, events[-1]["message"])

    def test_restore_reports_failure_when_the_write_fails(self):
        self.assertTrue(aircard.save_card_backup(UDID, CARD, canonical_assets()))
        with patch.object(aircard_backend, "write_files_batch", Mock(return_value=False)), \
                patch.object(aircard_backend, "write_file", Mock(return_value=False)), \
                patch.object(aircard_backend, "remove_files", Mock(return_value=True)):
            ok, events = self._events(aircard_backend.cmd_restore, UDID, CARD)
        self.assertFalse(ok)
        self.assertEqual(events[-1]["code"], "restore.failed")

    def test_restore_fails_when_the_cache_cannot_be_cleared(self):
        """Artwork back but caches stale means Wallet still shows the skin."""
        self.assertTrue(aircard.save_card_backup(UDID, CARD, canonical_assets()))
        with patch.object(aircard_backend, "write_files_batch", Mock(return_value=True)), \
                patch.object(aircard_backend, "remove_files", Mock(side_effect=[True, False])):
            ok, events = self._events(aircard_backend.cmd_restore, UDID, CARD)
        self.assertFalse(ok)
        self.assertEqual(events[-1]["code"], "restore.failed")


class BackupCommandTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        p = patch.object(aircard, "BACKUPS_ROOT", Path(self.tmp.name))
        p.start()
        self.addCleanup(p.stop)

    def _events(self, fn, *args):
        buf = io.StringIO()
        with redirect_stdout(buf):
            result = fn(*args)
        return result, [json.loads(l) for l in buf.getvalue().strip().splitlines() if l.strip()]

    def test_backup_reads_and_stores_the_current_artwork(self):
        reader = Mock(side_effect=[b"three-x", b"two-x", b"one-x"])
        with patch.object(aircard_backend, "read_file", reader):
            ok, events = self._events(aircard_backend.cmd_backup, UDID, CARD)
        self.assertTrue(ok)
        self.assertEqual(events[-1]["code"], "backup.done")
        self.assertEqual(reader.call_count, len(aircard.BACKED_UP_ASSETS))
        self.assertTrue(aircard.has_card_backup(UDID, CARD))
        self.assertEqual(
            sorted(name for name, _ in aircard.read_card_backup(UDID, CARD)),
            sorted(aircard.BACKED_UP_ASSETS),
        )

    def test_one_unreadable_file_means_no_backup_at_all(self):
        """A backup short of one file could not put the whole card back."""
        reader = Mock(side_effect=[b"three-x", None, b"one-x"])
        with patch.object(aircard_backend, "read_file", reader):
            ok, events = self._events(aircard_backend.cmd_backup, UDID, CARD)
        self.assertFalse(ok)
        self.assertEqual(events[-1]["code"], "backup.failed")
        self.assertIn(aircard.BACKED_UP_ASSETS[1], events[-1]["message"])
        self.assertFalse(aircard.has_card_backup(UDID, CARD))
        self.assertEqual(aircard.read_card_backup(UDID, CARD), [])

    def test_an_existing_backup_is_never_overwritten(self):
        """Otherwise a second backup would save the skin as the 'original'."""
        assets = canonical_assets()
        self.assertTrue(aircard.save_card_backup(UDID, CARD, assets))
        reader = Mock()
        with patch.object(aircard_backend, "read_file", reader):
            ok, events = self._events(aircard_backend.cmd_backup, UDID, CARD)
        self.assertTrue(ok)
        self.assertEqual(events[-1]["code"], "backup.exists")
        reader.assert_not_called()
        self.assertEqual(aircard.read_card_backup(UDID, CARD), sorted(assets))

    def test_unreadable_artwork_reports_failure_and_claims_no_backup(self):
        with patch.object(aircard_backend, "read_file", Mock(return_value=None)):
            ok, events = self._events(aircard_backend.cmd_backup, UDID, CARD)
        self.assertFalse(ok)
        self.assertEqual(events[-1]["code"], "backup.failed")
        self.assertFalse(aircard.has_card_backup(UDID, CARD))

    def test_backup_survives_a_raising_reader(self):
        with patch.object(aircard_backend, "read_file", Mock(side_effect=RuntimeError("boom"))):
            ok, events = self._events(aircard_backend.cmd_backup, UDID, CARD)
        self.assertFalse(ok)
        self.assertEqual(events[-1]["code"], "backup.failed")


if __name__ == "__main__":
    unittest.main()
