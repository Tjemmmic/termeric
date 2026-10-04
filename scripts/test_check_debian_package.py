import os
from pathlib import Path
import stat
import tempfile
import unittest
from unittest import mock

from scripts.check_debian_package import (
    CARGO_LOCK,
    ROOT,
    CheckError,
    DESKTOP_ENTRIES,
    EXPECTED_FILES,
    ICON_FILES,
    locked_libgit2_version,
    parse_desktop_entry,
    parse_os_release,
    validate_package_metadata,
    validate_desktop_entry,
    validate_layout,
    validate_notice,
)


LOCK_FIXTURE = """\
[[package]]
name = "libgit2-sys"
version = "0.17.0+1.8.1"
"""

# A notice that passes validate_notice against the repository's Cargo.lock.
NOTICE_FIXTURE = (
    "Unicode License v3\nstatically links libgit2 "
    f"{locked_libgit2_version(CARGO_LOCK.read_text(encoding='utf-8'))}, built from\n"
).encode()


def png_fixture(relative: str) -> bytes:
    """A PNG signature and IHDR prefix sized for the icon's directory."""
    size = ICON_FILES[relative].to_bytes(4, "big")
    return b"\x89PNG\r\n\x1a\n" + (13).to_bytes(4, "big") + b"IHDR" + size + size


def write_package_tree(root: Path, expected_files: dict[str, int]) -> None:
    """Write every expected file with contents that pass the layout checks.

    Desktop entries are the repository's own, so the checks cover them too.
    """
    for relative, mode in expected_files.items():
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        if relative == "usr/bin/termeric":
            contents = b"\x7fELFfixture"
        elif relative.endswith(".png"):
            contents = png_fixture(relative)
        elif relative.endswith(".desktop"):
            contents = (ROOT / "assets" / Path(relative).name).read_bytes()
        elif relative.endswith("THIRD_PARTY_NOTICES.md"):
            contents = NOTICE_FIXTURE
        else:
            contents = b"fixture"
        path.write_bytes(contents)
        path.chmod(mode)


class DebianPackageCheckTests(unittest.TestCase):
    def test_layout_contract_accepts_expected_files_and_modes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_package_tree(root, EXPECTED_FILES)

            validate_layout(root, exercise_native_tools=False)

            notice = root / "usr/share/doc/termeric/THIRD_PARTY_NOTICES.md"
            notice.write_text("missing license")
            with self.assertRaisesRegex(CheckError, "Unicode License v3"):
                validate_layout(root, exercise_native_tools=False)

    def test_notice_must_name_the_libgit2_that_cargo_lock_bundles(self):
        self.assertEqual(locked_libgit2_version(LOCK_FIXTURE), "1.8.1")
        validate_notice("Unicode License v3\nstatically links libgit2 1.8.1, built", LOCK_FIXTURE)

        stale = LOCK_FIXTURE.replace("0.17.0+1.8.1", "0.18.0+1.9.0")
        with self.assertRaisesRegex(CheckError, "names libgit2 1.8.1, but Cargo.lock bundles libgit2 1.9.0"):
            validate_notice("Unicode License v3\nstatically links libgit2 1.8.1, built", stale)
        with self.assertRaisesRegex(CheckError, "names libgit2 nowhere"):
            validate_notice("Unicode License v3\n", LOCK_FIXTURE)
        with self.assertRaisesRegex(CheckError, "found 0"):
            locked_libgit2_version('name = "git2"\nversion = "0.19.0"\n')

    def test_notice_must_list_a_crate_under_every_license(self):
        notice = "Unicode License v3\nstatically links libgit2 1.8.1, built\n"
        listed = "### MIT License\n\nUsed by:\n\n- [dpi 0.1.2](https://example.invalid)\n\n```text\nMIT\n```\n"
        validate_notice(notice + listed, LOCK_FIXTURE)
        empty = "### MIT License\n\nUsed by:\n\n\n```text\nMIT\n```\n"
        with self.assertRaisesRegex(CheckError, "lists no crates"):
            validate_notice(notice + empty, LOCK_FIXTURE)

    def test_repository_notices_name_the_libgit2_that_cargo_lock_bundles(self):
        lock_text = CARGO_LOCK.read_text(encoding="utf-8")
        for notice in (ROOT / "packaging/notices/notices.hbs", ROOT / "termeric/THIRD_PARTY_NOTICES.md"):
            with self.subTest(notice=notice.name):
                validate_notice(notice.read_text(encoding="utf-8"), lock_text)

    def test_layout_contract_rejects_an_icon_in_the_wrong_size_directory(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_package_tree(root, EXPECTED_FILES)

            misplaced = "usr/share/icons/hicolor/48x48/apps/org.termeric.Termeric.png"
            (root / misplaced).write_bytes(
                png_fixture("usr/share/icons/hicolor/512x512/apps/org.termeric.Termeric.png")
            )
            with self.assertRaisesRegex(CheckError, "is 512x512, expected 48x48"):
                validate_layout(root, exercise_native_tools=False)

    def test_layout_contract_rejects_an_executable_mode_regression(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_package_tree(root, EXPECTED_FILES)
            (root / "usr/bin/termeric").chmod(0o644)

            with self.assertRaisesRegex(CheckError, "mode 644, expected 755"):
                validate_layout(root, exercise_native_tools=False)

    def test_desktop_contract_rejects_a_launch_or_identity_mismatch(self):
        required = DESKTOP_ENTRIES["usr/share/applications/org.termeric.Termeric.desktop"]
        entry = (ROOT / "assets/org.termeric.Termeric.desktop").read_text()
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "org.termeric.Termeric.desktop"
            path.write_text(entry.replace("Exec=termeric\n", "Exec=wrong\n", 1))
            with self.assertRaisesRegex(CheckError, "expected 'termeric'"):
                validate_desktop_entry(path, required)
            path.write_text(entry.replace("Actions=NewWindow;", "Actions=", 1))
            with self.assertRaisesRegex(CheckError, "NewWindow action"):
                validate_desktop_entry(path, required)
            path.write_text(entry.replace("StartupWMClass=org.termeric.Termeric", "StartupWMClass=termeric"))
            with self.assertRaisesRegex(CheckError, "StartupWMClass is 'termeric'"):
                validate_desktop_entry(path, required)

    def test_desktop_parser_rejects_key_value_before_a_section(self):
        with self.assertRaisesRegex(CheckError, "malformed desktop entry"):
            parse_desktop_entry("Type=Application\n")

    def test_os_release_recognizes_debian_derivatives(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "os-release"
            path.write_text('ID="ubuntu"\nID_LIKE="debian linux"\n')
            self.assertEqual(parse_os_release(path), {"ubuntu", "debian", "linux"})

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


if __name__ == "__main__":
    unittest.main()
