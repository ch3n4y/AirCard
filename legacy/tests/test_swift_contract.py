"""Checks that what the backend prints is what the app can actually decode.

Swift's synthesised Decodable requires a key for every non-optional property.
A field the backend stops emitting, or never emitted, makes the whole decode
throw — the app then behaves as though nothing is connected at all, while every
Python-side test stays green because it only ever looks at the dict Python
built. That is the gap this file exists to close.
"""

import json
import re
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
SWIFT = REPO / "AirCardApp.swift"

import aircard

# "var name: Type" inside a struct body
PROPERTY = re.compile(r"^\s*var\s+(\w+)\s*:\s*([^\n=]+?)\s*$", re.M)


def struct_body(name: str):
    """The struct's contents, or None when the app does not define it."""
    src = SWIFT.read_text(encoding="utf-8")
    m = re.search(rf"^struct {name}\s*:[^{{]*\{{(.*?)^\}}", src, re.M | re.S)
    return m.group(1) if m else None


def required_keys(struct_name: str):
    """Properties Swift will refuse to decode without, or None if no such struct."""
    body = struct_body(struct_name)
    if body is None:
        return None
    out = set()
    for prop, typ in PROPERTY.findall(body):
        typ = typ.strip()
        if typ.endswith("?"):
            continue  # optional, may be absent
        out.add(prop)
    return out


def swift_function(name: str):
    """The body of a class-level Swift function in the app, or None if absent."""
    src = SWIFT.read_text(encoding="utf-8")
    m = re.search(
        rf"^    (?:private )?func {re.escape(name)}\([^\n]*\{{$\n(.*?)^    \}}$",
        src, re.M | re.S,
    )
    return m.group(1) if m else None


class SwiftDecodeContractTests(unittest.TestCase):
    def _normalized(self):
        return aircard._normalize_device({
            "udid": "00008150-000000000000001E",
            "product": "iPhone17,1",
            "name": "Test iPhone",
            "version": "27.0",
            "connection": "usb",
        })

    def _required(self, name):
        keys = required_keys(name)
        if keys is None:
            self.skipTest(f"{name} is not defined in this build of the app")
        return keys

    def test_device_entries_carry_every_key_swift_demands(self):
        missing = self._required("DeviceInfo") - set(self._normalized())
        self.assertEqual(
            missing, set(),
            f"aircard._normalize_device omits {sorted(missing)}, which DeviceInfo "
            f"declares non-optional. JSONDecoder throws keyNotFound and the app "
            f"reports no device at all.",
        )

    def test_device_list_response_shape(self):
        payload = {"connected": True, "devices": [self._normalized()]}
        missing = self._required("DeviceListResponse") - set(payload)
        self.assertEqual(missing, set(), f"--devices payload omits {sorted(missing)}")

    def test_saved_cards_response_shape(self):
        payload = {"ok": True, "cards": []}
        missing = self._required("SavedCardsResponse") - set(payload)
        self.assertEqual(missing, set(), f"--backups payload omits {sorted(missing)}")

    def test_payload_is_json_serialisable(self):
        """A value Python can hold but JSON cannot would break decoding too."""
        json.dumps({"connected": True, "devices": [self._normalized()]})

    def test_the_check_can_see_a_missing_key(self):
        """Guards the guard: prove this file notices an omission."""
        broken = dict(self._normalized())
        broken.pop("connected", None)
        self.assertIn("connected", self._required("DeviceInfo"))
        self.assertTrue(self._required("DeviceInfo") - set(broken))


class FlashLedgerContractTests(unittest.TestCase):
    """Nothing runs Swift in this suite, so the restore path is pinned by shape.

    The app remembers, per device and card, the signature of the skin it last put
    on the phone. A restore makes that record false. Left in place, re-picking
    the same image reads as "already on iPhone" and the skin is never written
    again.
    """

    def _body(self, name):
        body = swift_function(name)
        if body is None:
            self.skipTest(f"{name} is not defined in this build of the app")
        return body

    def test_the_source_extraction_finds_a_real_body(self):
        """Guards the guard: a regex that stopped matching would skip everything."""
        body = swift_function("applySkin")
        self.assertIsNotNone(body, "swift_function can no longer read a function body")
        self.assertIn("no_iphone_connected", body)

    def test_a_restore_forgets_the_skin_the_card_used_to_carry(self):
        body = self._body("restoreCard")
        call = "forgetFlashedSkin(udid: udid, cardId: id)"
        self.assertIn("if restored {", body, "restoreCard has no success branch to check")
        self.assertIn(
            call, body,
            "restoreCard leaves the flashed-skin ledger alone, so re-picking the same "
            "image reads as 'already on iPhone' and is skipped",
        )
        self.assertLess(
            body.index("if restored {"), body.index(call),
            "the ledger only stops being true once the restore has succeeded",
        )

    def test_a_missing_iphone_is_reported_rather_than_ignored(self):
        for name in ("backupCard", "restoreCard"):
            with self.subTest(function=name):
                self.assertIn(
                    'L("error.no_iphone_connected"', self._body(name),
                    f"{name} returns silently when no iPhone is connected, while its "
                    f"menu item stays enabled, so the click does nothing at all",
                )


class ScanRecordContractTests(unittest.TestCase):
    """The card list only ever grew, so a card taken out of Wallet stayed listed.

    Nothing runs Swift in this suite, so these pin the shape of the fix in the
    source, the same way the ledger tests above do.
    """

    def _body(self, name):
        body = swift_function(name)
        if body is None:
            self.skipTest(f"{name} is not defined in this build of the app")
        return body

    def test_a_scan_records_the_cards_it_actually_saw(self):
        body = self._body("startCardScanning")
        self.assertIn("scanSeen = []", body,
                      "a scan must start from an empty record, or it inherits the last one")
        self.assertIn(
            "self.scanSeen.insert(candidate)", body,
            "cards the scan found are not recorded, so a remembered row cannot be told "
            "apart from one that was actually seen",
        )

    def test_the_record_is_kept_but_not_from_an_empty_scan(self):
        self.assertIn("rememberScanResult()", self._body("stopCardScanning"),
                      "the scan result is thrown away, so the rows are never marked")
        remember = self._body("rememberScanResult")
        self.assertIn(
            "guard !scanSeen.isEmpty", remember,
            "a scan that observed nothing would mark every card as unseen",
        )
        self.assertIn("lastScanSeenKey", remember,
                      "the record would not survive a relaunch")

    def test_removing_unseen_cards_deletes_nothing_on_disk(self):
        """The rows go; the saved artwork behind them stays."""
        body = self._body("removeCardsNotSeenInLastScan")
        self.assertIn("cards.removeAll", body)
        for forbidden in ("forget_card_artwork", "--forget", "FileManager", "removeItem"):
            self.assertNotIn(
                forbidden, body,
                f"removing a row calls {forbidden}, which could destroy a saved original",
            )


if __name__ == "__main__":
    unittest.main()
