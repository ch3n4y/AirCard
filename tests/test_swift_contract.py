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


if __name__ == "__main__":
    unittest.main()
