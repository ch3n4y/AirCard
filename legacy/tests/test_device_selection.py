import unittest
from unittest.mock import patch

import aircard


def _raw(udid, product="iPhone15,2", name="iPhone", connection=None, **extra):
    entry = {"udid": udid, "product": product, "name": name}
    if connection is not None:
        entry["connection"] = connection
    entry.update(extra)
    return entry


class DeviceSelectionTests(unittest.TestCase):
    def _with_devices(self, devices):
        return patch.object(aircard, "list_devices", return_value=devices)

    def test_prefers_usb_over_wifi(self):
        devices = [
            _raw("wifi-udid", connection="network", name="Wi-Fi iPhone"),
            _raw("usb-udid", connection="usb", name="Cabled iPhone"),
        ]
        with self._with_devices(devices):
            self.assertEqual(aircard.get_connected_device()["udid"], "usb-udid")

    def test_prefers_iphone_over_ipad(self):
        devices = [
            _raw("ipad-udid", product="iPad13,1", name="iPad", connection="usb"),
            _raw("iphone-udid", product="iPhone16,1", name="iPhone", connection="network"),
        ]
        with self._with_devices(devices):
            # iPhone wins even though the iPad is on USB.
            self.assertEqual(aircard.get_connected_device()["udid"], "iphone-udid")

    def test_ordering_is_stable_regardless_of_enumeration_order(self):
        a = [_raw("b", connection="usb", name="B"), _raw("a", connection="usb", name="A")]
        b = list(reversed(a))
        with self._with_devices(a):
            first = [d["udid"] for d in aircard.list_connected_devices()]
        with self._with_devices(b):
            second = [d["udid"] for d in aircard.list_connected_devices()]
        self.assertEqual(first, second)
        self.assertEqual(first[0], "a")  # udid of the device named "A"; tie broken by name

    def test_preferred_udid_is_honored_when_present(self):
        devices = [
            _raw("usb-udid", connection="usb"),
            _raw("wifi-udid", connection="network"),
        ]
        with self._with_devices(devices):
            picked = aircard.get_connected_device(preferred_udid="wifi-udid")
            self.assertEqual(picked["udid"], "wifi-udid")

    def test_preferred_udid_absent_returns_none_not_substitute(self):
        # A chosen device that has dropped out must NOT be silently replaced by
        # another iPhone — that would flash onto the wrong device.
        devices = [_raw("usb-udid", connection="usb")]
        with self._with_devices(devices):
            self.assertIsNone(aircard.get_connected_device(preferred_udid="ghost-udid"))

    def test_automatic_selection_still_falls_back_to_best(self):
        devices = [_raw("usb-udid", connection="usb")]
        with self._with_devices(devices):
            # No explicit target -> best device is fine.
            self.assertEqual(aircard.get_connected_device()["udid"], "usb-udid")

    def test_missing_connection_defaults_to_unknown_and_beats_wifi(self):
        devices = [
            _raw("legacy-udid", name="Legacy"),  # no connection field
            _raw("wifi-udid", connection="network", name="Wi-Fi"),
        ]
        with self._with_devices(devices):
            listed = aircard.list_connected_devices()
            self.assertEqual(listed[0]["udid"], "legacy-udid")
            self.assertEqual(listed[0]["connection"], "unknown")

    def test_entries_without_udid_or_product_are_dropped(self):
        devices = [
            {"udid": "", "product": "iPhone15,2"},
            {"udid": "no-product"},
            _raw("good-udid", connection="usb"),
        ]
        with self._with_devices(devices):
            listed = aircard.list_connected_devices()
            self.assertEqual([d["udid"] for d in listed], ["good-udid"])

    def test_no_devices_returns_none(self):
        with self._with_devices([]):
            self.assertIsNone(aircard.get_connected_device())
            self.assertEqual(aircard.list_connected_devices(), [])

    def test_normalized_fields_have_defaults(self):
        with self._with_devices([_raw("u", connection="usb")]):
            dev = aircard.get_connected_device()
        for key in ("udid", "name", "version", "product", "language", "locale", "connection"):
            self.assertIn(key, dev)
        self.assertEqual(dev["name"], "iPhone")
        self.assertEqual(dev["version"], "Unknown")


if __name__ == "__main__":
    unittest.main()
