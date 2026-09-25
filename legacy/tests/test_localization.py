"""Checks every shipped translation against the English source.

A .strings file that is missing keys still loads, and the app silently falls
back to English for whatever is absent, so nothing fails loudly on its own.
A translation that drops a %@ is worse: it crashes at the moment the user hits
that screen. Both are caught here instead.
"""

import re
import subprocess
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
LOCALES = REPO / "locales"
BASE = "en"

# "key" = "value";  — value may contain escaped quotes
ENTRY = re.compile(r'^\s*"((?:[^"\\]|\\.)*)"\s*=\s*"((?:[^"\\]|\\.)*)"\s*;\s*$')
# %@ %d %1$@ %2$d %.1f %% ...
SPECIFIER = re.compile(r'%(?:(\d+)\$)?[-+ 0#]*[\d.]*([@dioux%fFeEgGsS])')
BREAK = re.compile(r"(\\+)n")


def parse(path):
    out = {}
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("//") or line.startswith("/*") or line.startswith("*"):
            continue
        m = ENTRY.match(line)
        if m:
            out[m.group(1)] = m.group(2)
    return out


def specifiers(text):
    """Multiset of specifier types, ignoring position so word order can change."""
    return sorted(t for _, t in SPECIFIER.findall(text) if t != "%")


def strings_files():
    return sorted(LOCALES.glob("*.lproj/Localizable.strings"))


def break_escapes(text):
    """Backslashes before each `n`: one is a newline, two or more is literal text."""
    return [len(before) for before in BREAK.findall(text)]


class LocalizationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.base_path = LOCALES / f"{BASE}.lproj" / "Localizable.strings"
        assert cls.base_path.is_file(), f"missing base strings file: {cls.base_path}"
        cls.base = parse(cls.base_path)

    def test_base_is_not_empty(self):
        self.assertGreater(len(self.base), 50, "English source looks truncated")

    def test_every_language_has_every_key(self):
        for path in strings_files():
            lang = path.parent.name
            if lang == f"{BASE}.lproj":
                continue
            entries = parse(path)
            missing = sorted(set(self.base) - set(entries))
            orphan = sorted(set(entries) - set(self.base))
            self.assertEqual(missing, [], f"{lang} is missing keys (would fall back to English): {missing[:8]}")
            self.assertEqual(orphan, [], f"{lang} has keys the app never asks for: {orphan[:8]}")

    def test_format_specifiers_survive_translation(self):
        for path in strings_files():
            lang = path.parent.name
            entries = parse(path)
            for key, english in self.base.items():
                want = specifiers(english)
                if not want:
                    continue
                if key not in entries:
                    continue  # reported by the key-parity test
                got = specifiers(entries[key])
                self.assertEqual(
                    got, want,
                    f"{lang} / {key}: format specifiers changed "
                    f"({want} -> {got}). This crashes at runtime.",
                )

    def test_line_breaks_are_not_double_escaped(self):
        """A twice-escaped newline renders as a literal backslash-backed 'n'."""
        for path in strings_files():
            lang = path.parent.name
            entries = parse(path)
            for key, english in self.base.items():
                want = break_escapes(english)
                if not want or key not in entries:
                    continue
                got = break_escapes(entries[key])
                self.assertEqual(
                    got, want,
                    f"{lang} / {key}: line breaks were escaped differently "
                    f"({want} -> {got}). Wallet users in this language would see "
                    f"the escape as text.",
                )

    def test_no_translation_left_as_english_placeholder(self):
        """A file full of untouched English means the language never got done."""
        for path in strings_files():
            lang = path.parent.name
            if lang == f"{BASE}.lproj":
                continue
            entries = parse(path)
            if not entries:
                continue
            # Short labels legitimately match (OK, Export, AirCard, Log...).
            long_keys = [k for k, v in self.base.items() if len(v) > 25]
            identical = [k for k in long_keys if entries.get(k) == self.base[k]]
            self.assertLess(
                len(identical), max(3, len(long_keys) // 4),
                f"{lang}: {len(identical)}/{len(long_keys)} long strings are still the English text",
            )

    def test_files_are_well_formed(self):
        for path in strings_files():
            result = subprocess.run(["plutil", "-lint", str(path)], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, f"{path.parent.name} is malformed: {result.stdout}{result.stderr}")



if __name__ == "__main__":
    unittest.main()
