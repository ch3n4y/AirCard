import io
import json
import unittest
from contextlib import redirect_stdout
from unittest.mock import patch

import aircard_backend


class DeviceBackendTests(unittest.TestCase):
    def _run(self, argv):
        buf = io.StringIO()
        with patch.object(aircard_backend.sys, "argv", ["aircard_backend.py", *argv]), \
                redirect_stdout(buf):
            try:
                aircard_backend.main()
            except SystemExit:
                pass
        return json.loads(buf.getvalue().strip().splitlines()[-1])

    def test_devices_lists_all_when_helper_present(self):
        sample = [
            {"udid": "usb-udid", "name": "Cabled", "product": "iPhone16,1",
             "version": "18.6", "language": "en", "locale": "", "bold_text": None,
             "connection": "usb", "connected": True},
            {"udid": "wifi-udid", "name": "Remote", "product": "iPhone15,2",
             "version": "18.5", "language": "en", "locale": "", "bold_text": None,
             "connection": "network", "connected": True},
        ]
        with patch.object(aircard_backend, "find_device_helper", return_value="/x/device_helper"), \
                patch.object(aircard_backend, "list_connected_devices", return_value=sample):
            out = self._run(["--devices"])
        self.assertTrue(out["connected"])
        self.assertEqual([d["udid"] for d in out["devices"]], ["usb-udid", "wifi-udid"])
        self.assertIn("connection", out["devices"][0])

    def test_devices_reports_missing_helper(self):
        with patch.object(aircard_backend, "find_device_helper", return_value=None):
            out = self._run(["--devices"])
        self.assertFalse(out["connected"])
        self.assertEqual(out["error"], "device_helper_missing")
        self.assertEqual(out["devices"], [])

    def test_device_passes_preferred_udid_through(self):
        captured = {}

        def fake_get(preferred_udid=None):
            captured["udid"] = preferred_udid
            return {"udid": preferred_udid or "default", "name": "iPhone",
                    "product": "iPhone16,1", "version": "18.6", "connection": "usb"}

        with patch.object(aircard_backend, "find_device_helper", return_value="/x/device_helper"), \
                patch.object(aircard_backend, "get_connected_device", side_effect=fake_get), \
                patch.object(aircard_backend, "native", return_value={"ok": True}), \
                patch.object(aircard_backend, "operation_ok", return_value=True):
            out = self._run(["--device", "wifi-udid"])
        self.assertEqual(captured["udid"], "wifi-udid")
        self.assertEqual(out["udid"], "wifi-udid")
        self.assertTrue(out["connected"])
        self.assertTrue(out["airlift_compatible"])

    def test_device_without_udid_uses_default(self):
        with patch.object(aircard_backend, "find_device_helper", return_value="/x/device_helper"), \
                patch.object(aircard_backend, "get_connected_device", return_value=None):
            out = self._run(["--device"])
        self.assertFalse(out["connected"])
        self.assertEqual(out["error"], "no_device")


if __name__ == "__main__":
    unittest.main()
