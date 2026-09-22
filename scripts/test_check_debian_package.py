import os
from pathlib import Path
import json
import stat
import tempfile
import unittest
from unittest import mock

from scripts.check_debian_package import (
    CheckError,
    EXPECTED_FILES,
    ICON_FILES,
    LIFECYCLE_OPT_IN,
    parse_desktop_entry,
    parse_os_release,
    require_lifecycle_host,
    validate_package_metadata,
    validate_desktop_entry,
    validate_layout,
    write_report,
)


DESKTOP_ENTRY = """\
[Desktop Entry]
Type=Application
Name=Termeric
Exec=termeric
Icon=termeric
Terminal=false
StartupWMClass=termeric
Actions=NewWindow;

[Desktop Action NewWindow]
Name=New Window
Exec=termeric --new-window
"""


def png_fixture(relative: str) -> bytes:
    """A PNG signature and IHDR prefix sized for the icon's directory."""
    size = ICON_FILES[relative].to_bytes(4, "big")
    return b"\x89PNG\r\n\x1a\n" + (13).to_bytes(4, "big") + b"IHDR" + size + size


class DebianPackageCheckTests(unittest.TestCase):
    def test_layout_contract_accepts_expected_files_and_modes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for relative, mode in EXPECTED_FILES.items():
                path = root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                if relative == "usr/bin/termeric":
                    contents = b"\x7fELFfixture"
                elif relative.endswith(".png"):
                    contents = png_fixture(relative)
                elif relative.endswith(".desktop"):
                    contents = DESKTOP_ENTRY.encode()
                elif relative.endswith("THIRD_PARTY_NOTICES.md"):
                    contents = b"Unicode License v3\n"
                else:
                    contents = b"fixture"
                path.write_bytes(contents)
                path.chmod(mode)

            validate_layout(root, exercise_native_tools=False)

            notice = root / "usr/share/doc/termeric/THIRD_PARTY_NOTICES.md"
            notice.write_text("missing license")
            with self.assertRaisesRegex(CheckError, "Unicode License v3"):
                validate_layout(root, exercise_native_tools=False)

    def test_layout_contract_rejects_an_icon_in_the_wrong_size_directory(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for relative, mode in EXPECTED_FILES.items():
                path = root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                if relative == "usr/bin/termeric":
                    contents = b"\x7fELFfixture"
                elif relative.endswith(".png"):
                    contents = png_fixture(relative)
                elif relative.endswith(".desktop"):
                    contents = DESKTOP_ENTRY.encode()
                elif relative.endswith("THIRD_PARTY_NOTICES.md"):
                    contents = b"Unicode License v3\n"
                else:
                    contents = b"fixture"
                path.write_bytes(contents)
                path.chmod(mode)

            misplaced = "usr/share/icons/hicolor/48x48/apps/termeric.png"
            (root / misplaced).write_bytes(
                png_fixture("usr/share/icons/hicolor/512x512/apps/termeric.png")
            )
            with self.assertRaisesRegex(CheckError, "is 512x512, expected 48x48"):
                validate_layout(root, exercise_native_tools=False)

    def test_layout_contract_rejects_an_executable_mode_regression(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for relative, mode in EXPECTED_FILES.items():
                path = root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                contents = DESKTOP_ENTRY.encode() if relative.endswith(".desktop") else b"fixture"
                if relative == "usr/bin/termeric":
                    contents = b"\x7fELFfixture"
                elif relative.endswith(".png"):
                    contents = png_fixture(relative)
                elif relative.endswith("THIRD_PARTY_NOTICES.md"):
                    contents = b"Unicode License v3\n"
                path.write_bytes(contents)
                path.chmod(0o644 if relative == "usr/bin/termeric" else mode)

            with self.assertRaisesRegex(CheckError, "mode 644, expected 755"):
                validate_layout(root, exercise_native_tools=False)

    def test_desktop_contract_rejects_a_gui_launch_mismatch(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "termeric.desktop"
            path.write_text(DESKTOP_ENTRY.replace("Exec=termeric\n", "Exec=wrong\n", 1))
            with self.assertRaisesRegex(CheckError, "expected 'termeric'"):
                validate_desktop_entry(path)

    def test_desktop_parser_rejects_key_value_before_a_section(self):
        with self.assertRaisesRegex(CheckError, "malformed desktop entry"):
            parse_desktop_entry("Type=Application\n")

    def test_os_release_recognizes_debian_derivatives(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "os-release"
            path.write_text('ID="ubuntu"\nID_LIKE="debian linux"\n')
            self.assertEqual(parse_os_release(path), {"ubuntu", "debian", "linux"})

    def test_lifecycle_mode_fails_closed_without_explicit_opt_in(self):
        with mock.patch.dict(os.environ, {}, clear=True):
            with self.assertRaisesRegex(CheckError, LIFECYCLE_OPT_IN):
                require_lifecycle_host()

    def test_expected_modes_are_not_group_or_world_writable(self):
        for mode in EXPECTED_FILES.values():
            self.assertFalse(mode & (stat.S_IWGRP | stat.S_IWOTH))

    @mock.patch("scripts.check_debian_package.package_field")
    def test_metadata_requires_the_documented_software_renderer(self, package_field):
        values = {
            "Package": "termeric",
            "Version": "1.2.3-1",
            "Architecture": "amd64",
            "Depends": "libc6",
            "Recommends": "mesa-vulkan-drivers",
        }
        package_field.side_effect = lambda _package, field: values[field]
        metadata = validate_package_metadata(Path("termeric.deb"))
        self.assertEqual(metadata["recommends"], "mesa-vulkan-drivers")

        values["Recommends"] = ""
        with self.assertRaisesRegex(CheckError, "software renderer"):
            validate_package_metadata(Path("termeric.deb"))

    def test_report_records_a_hash_and_explicit_unperformed_lifecycle(self):
        with tempfile.TemporaryDirectory() as directory:
            temporary = Path(directory)
            package = temporary / "termeric.deb"
            package.write_bytes(b"package fixture")
            report = temporary / "report.json"
            write_report(
                report,
                package,
                status="pass",
                metadata={
                    "name": "termeric",
                    "version": "1.2.3-1",
                    "architecture": "amd64",
                    "depends": "libc6",
                    "recommends": "mesa-vulkan-drivers",
                },
                lifecycle=None,
            )

            parsed = json.loads(report.read_text())
            self.assertEqual(parsed["schema"], 1)
            self.assertEqual(parsed["status"], "pass")
            self.assertEqual(parsed["package"]["bytes"], len(b"package fixture"))
            self.assertEqual(len(parsed["package"]["sha256"]), 64)
            self.assertEqual(parsed["lifecycle"], {"performed": False})


if __name__ == "__main__":
    unittest.main()
