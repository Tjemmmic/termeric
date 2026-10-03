import os
from pathlib import Path
import stat
import tempfile
import unittest

from scripts.test_check_debian_package import png_fixture
from scripts.check_arch_package import (
    CheckError,
    EXPECTED_FILES,
    METADATA_FILES,
    REQUIRED_DEPENDENCIES,
    REQUIRED_LICENSES,
    normalize_member_name,
    parse_pkginfo,
    parse_os_release,
    parse_verbose_types,
    validate_archive_inventory,
    validate_archive_modes,
    validate_layout,
    validate_package_metadata,
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


def valid_inventory() -> tuple[list[str], list[str]]:
    directories = {
        str(parent)
        for name in EXPECTED_FILES
        for parent in Path(name).parents
        if str(parent) != "."
    }
    names = sorted(set(EXPECTED_FILES) | METADATA_FILES | directories)
    return names, ["d" if name in directories else "-" for name in names]


def valid_metadata() -> dict[str, list[str]]:
    return {
        "pkgname": ["termeric-git"],
        "pkgver": ["0.1.0.r42.gabcdef0-1"],
        "arch": ["x86_64"],
        "license": sorted(REQUIRED_LICENSES),
        "provides": ["termeric"],
        "conflict": ["termeric"],
        "depend": sorted(REQUIRED_DEPENDENCIES),
    }


class ArchPackageCheckTests(unittest.TestCase):
    def test_inventory_accepts_only_expected_regular_files(self):
        names, types = valid_inventory()
        self.assertEqual(validate_archive_inventory(names, types), names)

    def test_inventory_rejects_missing_duplicate_and_unexpected_members(self):
        names, types = valid_inventory()
        with self.assertRaisesRegex(CheckError, "missing members"):
            validate_archive_inventory(names[:-1], types[:-1])
        with self.assertRaisesRegex(CheckError, "duplicate package member"):
            validate_archive_inventory(names + [names[0]], types + ["-"])
        with self.assertRaisesRegex(CheckError, "unexpected package file"):
            validate_archive_inventory(names + ["usr/bin/surprise"], types + ["-"])

    def test_inventory_rejects_traversal_absolute_and_symlink_members(self):
        for name in ["../escape", "/usr/bin/termeric", "usr/../escape", "usr\\escape"]:
            with self.subTest(name=name), self.assertRaisesRegex(CheckError, "unsafe"):
                normalize_member_name(name)
        names, types = valid_inventory()
        types[names.index("usr/bin/termeric")] = "l"
        with self.assertRaisesRegex(CheckError, "unsafe type"):
            validate_archive_inventory(names, types)

    def test_verbose_type_parser_fails_closed(self):
        self.assertEqual(
            parse_verbose_types(
                "-rw-r--r-- 0 0 0 1 Jan 1 00:00 .PKGINFO\n"
                "drwxr-xr-x 0 0 0 0 Jan 1 00:00 usr/\n"
            ),
            ["-", "d"],
        )
        with self.assertRaisesRegex(CheckError, "could not parse"):
            parse_verbose_types("? unknown")
        with self.assertRaisesRegex(CheckError, "not owned by root:root"):
            parse_verbose_types(
                "-rw-r--r-- 0 1000 1000 1 Jan 1 00:00 .PKGINFO",
                require_root_ownership=True,
            )

    def test_pkginfo_parser_and_metadata_contract(self):
        lines = [
            "pkgname = termeric-git",
            "pkgver = 0.1.0.r42.gabcdef0-1",
            "arch = x86_64",
            "license = MIT",
            "license = Unicode-3.0",
            "provides = termeric",
            "conflict = termeric",
        ]
        lines.extend(f"depend = {dependency}" for dependency in sorted(REQUIRED_DEPENDENCIES))
        fields = parse_pkginfo("\n".join(lines))
        metadata = validate_package_metadata(fields)
        self.assertEqual(metadata["name"], "termeric-git")
        self.assertEqual(metadata["architecture"], "x86_64")

    def test_metadata_rejects_missing_dependency_and_wrong_identity(self):
        fields = valid_metadata()
        fields["depend"].remove("fontconfig")
        with self.assertRaisesRegex(CheckError, "fontconfig"):
            validate_package_metadata(fields)
        fields = valid_metadata()
        fields["pkgname"] = ["wrong"]
        with self.assertRaisesRegex(CheckError, "expected 'termeric-git'"):
            validate_package_metadata(fields)
        fields = valid_metadata()
        fields["license"].remove("Unicode-3.0")
        with self.assertRaisesRegex(CheckError, "Unicode-3.0"):
            validate_package_metadata(fields)

    def test_metadata_rejects_pacman_invalid_tilde_version(self):
        fields = valid_metadata()
        fields["pkgver"] = ["0~termeric-lifecycle-1"]
        with self.assertRaisesRegex(CheckError, "package version is invalid"):
            validate_package_metadata(fields)

    def test_os_release_recognizes_arch_derivatives(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "os-release"
            path.write_text('ID="cachyos"\nID_LIKE="arch linux"\n')
            self.assertEqual(parse_os_release(path), {"cachyos", "arch", "linux"})

    def test_layout_accepts_expected_files_and_modes_without_native_tools(self):
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

    def test_layout_rejects_mode_and_file_type_regressions(self):
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
                path.chmod(0o777 if relative == "usr/bin/termeric" else mode)
            with self.assertRaisesRegex(CheckError, "mode 777, expected 755"):
                validate_layout(root, exercise_native_tools=False)

    def test_expected_modes_are_not_group_or_world_writable(self):
        for mode in EXPECTED_FILES.values():
            self.assertFalse(mode & (stat.S_IWGRP | stat.S_IWOTH))

    def test_archive_metadata_and_directories_have_exact_safe_modes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            directories = {
                parent
                for name in EXPECTED_FILES
                for parent in Path(name).parents
                if str(parent) != "."
            }
            for relative in directories:
                (root / relative).mkdir(parents=True, exist_ok=True)
                (root / relative).chmod(0o755)
            for name in METADATA_FILES:
                (root / name).write_text("metadata")
                (root / name).chmod(0o644)
            validate_archive_modes(root)
            (root / ".PKGINFO").chmod(0o666)
            with self.assertRaisesRegex(CheckError, "mode 666, expected 644"):
                validate_archive_modes(root)


if __name__ == "__main__":
    unittest.main()
