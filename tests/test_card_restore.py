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


class BackupStorageTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        p = patch.object(aircard, "BACKUPS_ROOT", Path(self.tmp.name))
        p.start()
        self.addCleanup(p.stop)

    def test_round_trip(self):
        self.assertFalse(aircard.has_card_backup(UDID, CARD))
        ok = aircard.save_card_backup(UDID, CARD, [("a@2x.png", b"one"), ("a@3x.png", b"two")])
        self.assertTrue(ok)
        self.assertTrue(aircard.has_card_backup(UDID, CARD))
        self.assertEqual(
            aircard.read_card_backup(UDID, CARD),
            [("a@2x.png", b"one"), ("a@3x.png", b"two")],
        )

    def test_hashes_with_slashes_survive_the_filesystem(self):
        # Real card hashes contain / and +, neither of which is a legal folder name.
        awkward = "ab/cd+ef=="
        self.assertTrue(aircard.save_card_backup(UDID, awkward, [("x.png", b"z")]))
        self.assertTrue(aircard.has_card_backup(UDID, awkward))
        self.assertEqual(aircard.list_backed_up_cards(UDID), [awkward])

    def test_empty_payloads_are_not_a_backup(self):
        self.assertFalse(aircard.save_card_backup(UDID, CARD, []))
        self.assertFalse(aircard.save_card_backup(UDID, CARD, [("x.png", b"")]))
        self.assertFalse(aircard.has_card_backup(UDID, CARD))

    def test_backups_are_kept_per_device(self):
        aircard.save_card_backup(UDID, CARD, [("x.png", b"a")])
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
        aircard.save_card_backup(UDID, CARD, [("a@2x.png", b"one"), ("a@3x.png", b"two")])
        batch = Mock(return_value=True)
        remove = Mock(return_value=True)
        with patch.object(aircard_backend, "write_files_batch", batch), \
                patch.object(aircard_backend, "remove_files", remove):
            ok, events = self._events(aircard_backend.cmd_restore, UDID, CARD)
        self.assertTrue(ok)
        self.assertEqual(events[-1]["code"], "restore.done")
        target, payload = batch.call_args[0][1], batch.call_args[0][2]
        self.assertIn(CARD, target)
        self.assertEqual([n for n, _ in payload], ["a@2x.png", "a@3x.png"])
        # Both rendered-face directories get cleared, same as a flash does.
        self.assertEqual(remove.call_count, 2)

    def test_restore_reports_failure_when_the_write_fails(self):
        aircard.save_card_backup(UDID, CARD, [("a@2x.png", b"one")])
        with patch.object(aircard_backend, "write_files_batch", Mock(return_value=False)), \
                patch.object(aircard_backend, "write_file", Mock(return_value=False)), \
                patch.object(aircard_backend, "remove_files", Mock(return_value=True)):
            ok, events = self._events(aircard_backend.cmd_restore, UDID, CARD)
        self.assertFalse(ok)
        self.assertEqual(events[-1]["code"], "restore.failed")

    def test_restore_fails_when_the_cache_cannot_be_cleared(self):
        """Artwork back but caches stale means Wallet still shows the skin."""
        aircard.save_card_backup(UDID, CARD, [("a@2x.png", b"one")])
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
        reader = Mock(side_effect=[b"three-x", b"two-x"])
        with patch.object(aircard_backend, "read_file", reader):
            ok, events = self._events(aircard_backend.cmd_backup, UDID, CARD)
        self.assertTrue(ok)
        self.assertEqual(events[-1]["code"], "backup.done")
        self.assertTrue(aircard.has_card_backup(UDID, CARD))

    def test_an_existing_backup_is_never_overwritten(self):
        """Otherwise a second backup would save the skin as the 'original'."""
        aircard.save_card_backup(UDID, CARD, [("a@2x.png", b"pristine")])
        reader = Mock()
        with patch.object(aircard_backend, "read_file", reader):
            ok, events = self._events(aircard_backend.cmd_backup, UDID, CARD)
        self.assertTrue(ok)
        self.assertEqual(events[-1]["code"], "backup.exists")
        reader.assert_not_called()
        self.assertEqual(aircard.read_card_backup(UDID, CARD), [("a@2x.png", b"pristine")])

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
