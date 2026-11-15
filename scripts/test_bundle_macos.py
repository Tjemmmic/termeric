"""Tests for bundle_macos.py."""

from __future__ import annotations

from pathlib import Path
import plistlib
import struct
import tempfile
import unittest

from scripts import bundle_macos


def fake_binary(directory: Path) -> Path:
    binary = directory / "termeric"
    binary.write_bytes(b"\xcf\xfa\xed\xfe" + b"\0" * 64)
    return binary


class BundleTests(unittest.TestCase):
    def test_the_bundle_holds_the_binary_plist_icon_and_terminfo(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            app = bundle_macos.bundle(fake_binary(directory), directory / "Termeric.app")
            contents = app / "Contents"
            executable = contents / "MacOS/termeric"
            self.assertEqual(executable.stat().st_mode & 0o777, 0o755)
            with (contents / "Info.plist").open("rb") as plist:
                info = plistlib.load(plist)
            self.assertEqual(info["CFBundleIdentifier"], "org.termeric.Termeric")
            self.assertEqual(info["CFBundleExecutable"], "termeric")
            self.assertEqual(info["CFBundlePackageType"], "APPL")
            self.assertEqual(info["CFBundleShortVersionString"], bundle_macos.workspace_version())
            self.assertEqual(info["LSMinimumSystemVersion"], "11.0")
            self.assertNotIn("NSSupportsSuddenTermination", info)
            for key in bundle_macos.USAGE_DESCRIPTIONS:
                self.assertTrue(info[key].startswith("Termeric asks because"), key)
            for layout in ("x", "78"):
                self.assertEqual(
                    (contents / "Resources/terminfo" / layout / "xterm-termeric").read_bytes(),
                    (bundle_macos.ROOT / "assets/terminfo/x/xterm-termeric").read_bytes(),
                )
            self.assertEqual((contents / "PkgInfo").read_bytes(), b"APPL????")
            for notice in ("LICENSE", "THIRD_PARTY_NOTICES.md"):
                self.assertTrue((contents / "Resources" / notice).is_file(), notice)

    def test_the_icon_has_every_slot_at_its_size(self) -> None:
        data = bundle_macos.icns()
        self.assertEqual(data[:4], b"icns")
        self.assertEqual(struct.unpack(">I", data[4:8])[0], len(data))
        offset, found = 8, {}
        while offset < len(data):
            kind = data[offset : offset + 4]
            length = struct.unpack(">I", data[offset + 4 : offset + 8])[0]
            found[kind] = bundle_macos.png_size(data[offset + 8 : offset + length])
            offset += length
        self.assertEqual(offset, len(data))
        self.assertEqual(
            found, {kind: (size, size) for kind, size in bundle_macos.ICNS_ENTRIES}
        )

    def test_only_a_new_app_from_a_mac_binary_is_written(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            elf = directory / "linux"
            elf.write_bytes(b"\x7fELF" + b"\0" * 64)
            with self.assertRaises(bundle_macos.BundleError):
                bundle_macos.bundle(elf, directory / "Termeric.app")
            with self.assertRaises(bundle_macos.BundleError):
                bundle_macos.bundle(fake_binary(directory), directory / "Termeric")
            (directory / "Taken.app").mkdir()
            with self.assertRaises(bundle_macos.BundleError):
                bundle_macos.bundle(fake_binary(directory), directory / "Taken.app")


if __name__ == "__main__":
    unittest.main()
