import base64
import io
import json
import subprocess
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import Mock, patch

import aircard
import aircard_backend
import apply_card_skin


PNG_1X1 = base64.b64decode(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII="
)


class CardFlashTests(unittest.TestCase):
    def test_cache_removal_moves_link_and_required_companion_payload(self) -> None:
        successful = {
            "exitCode": 0,
            "targetGatePassed": True,
            "operation": {"ok": True},
        }
        with (
            patch.object(apply_card_skin, "native", return_value=successful),
            patch.object(apply_card_skin, "run_json", return_value={"exitCode": 0, "ok": True}) as transfer,
        ):
            result = apply_card_skin.remove_files(
                "device", "/protected/card.cache", ["FrontFace"], retries=1
            )

        self.assertTrue(result)
        command = transfer.call_args.args[0]
        self.assertEqual(len(command), 6)
        self.assertIn("/airlift-link-", command[4])
        self.assertTrue(command[5].endswith("/removed-0"))

    def test_flash_writes_pdf_and_removes_rendered_cache(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            image_path = Path(temporary) / "card.png"
            image_path.write_bytes(PNG_1X1)
            write_file = Mock(return_value=True)
            remove_files = Mock(return_value=True)

            with (
                patch.object(aircard_backend, "write_file", write_file),
                patch.object(aircard_backend, "write_files_batch", Mock(return_value=False)),
                patch.object(aircard_backend, "remove_files", remove_files),
                redirect_stdout(io.StringIO()),
            ):
                result = aircard_backend.cmd_flash("device", "card", str(image_path))

        self.assertTrue(result)

        writes = [call.args for call in write_file.call_args_list]
        pass_assets = {
            leaf: payload
            for _, target, leaf, payload in writes
            if target.endswith(".pkpass")
        }
        self.assertEqual(
            set(pass_assets),
            {
                "cardBackgroundCombined@3x.png",
                "cardBackgroundCombined@2x.png",
                "cardBackgroundCombined.pdf",
            },
        )
        self.assertTrue(pass_assets["cardBackgroundCombined.pdf"].startswith(b"%PDF-"))

        removals = [call.args for call in remove_files.call_args_list]
        for extension in (".cache", ".pkcache"):
            self.assertIn(("device", f"/var/mobile/Library/Passes/Cards/card{extension}", list(aircard_backend.CACHE_FILES)), removals)

    def test_flash_fails_when_wallet_cache_cannot_be_removed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            image_path = Path(temporary) / "card.png"
            image_path.write_bytes(PNG_1X1)
            output = io.StringIO()
            with (
                patch.object(aircard_backend, "write_files_batch", return_value=True),
                patch.object(aircard_backend, "remove_files", side_effect=[True, False]),
                redirect_stdout(output),
            ):
                result = aircard_backend.cmd_flash("device", "card", str(image_path))
        messages = [json.loads(line) for line in output.getvalue().splitlines()]
        self.assertFalse(result)
        self.assertEqual(messages[-1]["type"], "error")

    def test_flash_reports_failure_when_an_asset_write_fails(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            image_path = Path(temporary) / "card.png"
            image_path.write_bytes(PNG_1X1)
            write_file = Mock(
                side_effect=[True, True, False, True, True, True, True, True, True]
            )
            output = io.StringIO()

            with (
                patch.object(aircard_backend, "write_files_batch", Mock(return_value=False)),
                patch.object(aircard_backend, "write_file", write_file),
                patch.object(aircard_backend, "remove_files", Mock(return_value=True)),
                redirect_stdout(output),
            ):
                result = aircard_backend.cmd_flash("device", "card", str(image_path))

        messages = [json.loads(line) for line in output.getvalue().splitlines()]
        self.assertFalse(result)
        self.assertEqual(messages[-1]["type"], "error")
        self.assertFalse(any(message["type"] == "success" for message in messages))

    def test_flash_reports_failure_when_pdf_conversion_fails(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            image_path = Path(temporary) / "card.png"
            image_path.write_bytes(PNG_1X1)
            write_file = Mock(return_value=True)
            output = io.StringIO()

            with (
                patch.object(aircard_backend, "write_file", write_file),
                patch.object(
                    aircard_backend,
                    "build_card_assets",
                    side_effect=subprocess.CalledProcessError(1, ["sips"]),
                ),
                redirect_stdout(output),
            ):
                result = aircard_backend.cmd_flash("device", "card", str(image_path))

        messages = [json.loads(line) for line in output.getvalue().splitlines()]
        self.assertFalse(result)
        self.assertEqual(messages[-1]["type"], "error")
        write_file.assert_not_called()


class CliFlashTests(unittest.TestCase):
    """The standalone CLI has to write what the app writes.

    It used to write the PNG bytes to two hardcoded PNG names and never touch
    cardBackgroundCombined.pdf, which Wallet renders in preference to them. A
    card skinned from the app therefore kept its old skin while every file the
    CLI listed reported OK.
    """

    def _flash(self, **patches):
        for target, replacement in patches.items():
            patcher = patch.object(aircard, target, replacement)
            patcher.start()
            self.addCleanup(patcher.stop)
        with redirect_stdout(io.StringIO()):
            return aircard.flash_skin_assets("device", ["card"], PNG_1X1)

    def test_cli_flash_writes_the_assets_the_app_writes(self) -> None:
        write_file = Mock(return_value=True)
        ok = self._flash(write_file=write_file, invalidate_cache=Mock(return_value=True))

        self.assertTrue(ok)
        written = {call.args[2]: call.args[3] for call in write_file.call_args_list}
        self.assertEqual(set(written), set(aircard.BACKED_UP_ASSETS))
        self.assertTrue(written["cardBackgroundCombined.pdf"].startswith(b"%PDF-"))
        self.assertEqual(written["cardBackgroundCombined@2x.png"], PNG_1X1)

    def test_cli_flash_reports_failure_when_the_artwork_cannot_be_prepared(self) -> None:
        write_file = Mock(return_value=True)
        ok = self._flash(
            write_file=write_file,
            build_card_assets=Mock(side_effect=subprocess.CalledProcessError(1, ["sips"])),
        )

        self.assertFalse(ok)
        write_file.assert_not_called()

    def test_cli_flash_admits_a_write_that_failed(self) -> None:
        ok = self._flash(
            write_file=Mock(side_effect=[True, True, False]),
            invalidate_cache=Mock(return_value=True),
        )

        self.assertFalse(ok)


class FlashAssetSourceTests(unittest.TestCase):
    """One module decides which files a flash writes: card_assets.

    Three flash paths had each grown their own PNG-only list -- the app's
    backend, the standalone CLI in aircard.py and the manual script at the
    bottom of apply_card_skin.py -- and every one of them silently failed to
    change a card that had been skinned from the app, because Wallet renders the
    combined PDF in preference to the PNGs.
    """

    REPO = Path(__file__).resolve().parent.parent

    def test_no_flash_path_names_the_assets_itself(self):
        offenders = []
        for path in sorted(self.REPO.glob("*.py")):
            if path.name == "card_assets.py":
                continue
            for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
                if "cardBackgroundCombined" in line and not line.strip().startswith("#"):
                    offenders.append(f"{path.name}:{number}")
        self.assertEqual(
            offenders, [],
            f"{offenders} name the card assets directly. Import them from "
            f"card_assets instead, or the list will drift out of step again.",
        )


if __name__ == "__main__":
    unittest.main()
