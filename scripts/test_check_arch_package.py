import os
from pathlib import Path
import stat
import tempfile
import unittest

from scripts.test_check_debian_package import write_package_tree
from scripts.check_arch_package import (
    CheckError,
    METADATA_FILES,
    PACKAGE_NAMES,
    REQUIRED_DEPENDENCIES,
    REQUIRED_LICENSES,
    normalize_member_name,
    parse_pkginfo,
    parse_os_release,
    parse_verbose_types,
    SUPPORTED_ARCHITECTURES,
    expected_files,
    validate_archive_inventory,
    validate_archive_modes,
    validate_layout,
    validate_package_metadata,
)

REPOSITORY = Path(__file__).resolve().parent.parent
# The development package; the tests that differ by package say so.
PACKAGE = "termeric-git"
EXPECTED_FILES = expected_files(PACKAGE)


def recipe_srcinfos() -> dict[str, dict[str, list[str]]]:
    """Each AUR recipe's .SRCINFO, by package name."""
    return {
        name: parse_pkginfo((REPOSITORY / "packaging/arch" / name / ".SRCINFO").read_text(encoding="utf-8"))
        for name in PACKAGE_NAMES
    }


def valid_inventory(package: str = PACKAGE) -> tuple[list[str], list[str]]:
    files = expected_files(package)
    directories = {
        str(parent)
        for name in files
        for parent in Path(name).parents
        if str(parent) != "."
    }
    names = sorted(set(files) | METADATA_FILES | directories)
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
        for package in PACKAGE_NAMES:
            with self.subTest(package=package):
                names, types = valid_inventory(package)
                self.assertIn(f"usr/share/licenses/{package}/LICENSE", names)
                self.assertEqual(validate_archive_inventory(names, types, package), names)

    def test_inventory_rejects_another_packages_license_path(self):
        names, types = valid_inventory("termeric")
        with self.assertRaisesRegex(CheckError, "unexpected package directory: usr/share/licenses/termeric$"):
            validate_archive_inventory(names, types, "termeric-git")

    def test_inventory_expects_the_manual_page_that_zipman_compressed(self):
        names, types = valid_inventory()
        self.assertIn("usr/share/man/man1/termeric.1.gz", names)
        self.assertNotIn("usr/share/man/man1/termeric.1", EXPECTED_FILES)
        self.assertEqual(validate_archive_inventory(names, types, PACKAGE), names)
        uncompressed = [
            name.removesuffix(".gz") if name.startswith("usr/share/man/") else name
            for name in names
        ]
        with self.assertRaisesRegex(CheckError, "unexpected package file: usr/share/man/man1/termeric.1"):
            validate_archive_inventory(uncompressed, types, PACKAGE)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_package_tree(root, EXPECTED_FILES)
            self.assertTrue((root / "usr/share/man/man1/termeric.1.gz").is_file())
            validate_layout(root, PACKAGE, exercise_native_tools=False)

    def test_inventory_rejects_missing_duplicate_and_unexpected_members(self):
        names, types = valid_inventory()
        with self.assertRaisesRegex(CheckError, "missing members"):
            validate_archive_inventory(names[:-1], types[:-1], PACKAGE)
        with self.assertRaisesRegex(CheckError, "duplicate package member"):
            validate_archive_inventory(names + [names[0]], types + ["-"], PACKAGE)
        with self.assertRaisesRegex(CheckError, "unexpected package file"):
            validate_archive_inventory(names + ["usr/bin/surprise"], types + ["-"], PACKAGE)

    def test_inventory_rejects_traversal_absolute_and_symlink_members(self):
        for name in ["../escape", "/usr/bin/termeric", "usr/../escape", "usr\\escape"]:
            with self.subTest(name=name), self.assertRaisesRegex(CheckError, "unsafe"):
                normalize_member_name(name)
        names, types = valid_inventory()
        types[names.index("usr/bin/termeric")] = "l"
        with self.assertRaisesRegex(CheckError, "unsafe type"):
            validate_archive_inventory(names, types, PACKAGE)

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
            "provides = termeric",
            "conflict = termeric",
        ]
        lines.extend(f"license = {license}" for license in sorted(REQUIRED_LICENSES))
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
        with self.assertRaisesRegex(CheckError, "expected 'termeric' or 'termeric-git'"):
            validate_package_metadata(fields)

    def test_release_package_needs_no_provides_or_conflicts(self):
        fields = valid_metadata()
        fields["pkgname"] = ["termeric"]
        fields["pkgver"] = ["0.1.0-1"]
        del fields["provides"], fields["conflict"]
        self.assertEqual(validate_package_metadata(fields)["name"], "termeric")

    def test_development_package_must_stand_in_for_the_release_package(self):
        for field, message in [("provides", "does not provide termeric"), ("conflict", "does not conflict with termeric")]:
            fields = valid_metadata()
            del fields[field]
            with self.subTest(field=field), self.assertRaisesRegex(CheckError, message):
                validate_package_metadata(fields)
        fields = valid_metadata()
        fields["provides"] = ["termeric=0.1.0"]
        self.assertEqual(validate_package_metadata(fields)["name"], "termeric-git")
        fields["license"].remove("Unicode-3.0")
        with self.assertRaisesRegex(CheckError, "Unicode-3.0"):
            validate_package_metadata(fields)

    def test_metadata_accepts_each_listed_architecture_only(self):
        for architecture in ("x86_64", "aarch64"):
            fields = valid_metadata()
            fields["arch"] = [architecture]
            self.assertEqual(validate_package_metadata(fields)["architecture"], architecture)
        fields = valid_metadata()
        fields["arch"] = ["i686"]
        with self.assertRaisesRegex(CheckError, "expected 'x86_64' or 'aarch64'"):
            validate_package_metadata(fields)

    def test_each_srcinfo_names_its_own_package(self):
        for package, fields in recipe_srcinfos().items():
            with self.subTest(package=package):
                self.assertEqual(fields["pkgname"], [package])

    def test_each_srcinfo_declares_the_architectures_the_checker_accepts(self):
        for package, fields in recipe_srcinfos().items():
            with self.subTest(package=package):
                self.assertEqual(fields["arch"], list(SUPPORTED_ARCHITECTURES))

    def test_each_srcinfo_declares_the_licenses_the_checker_requires(self):
        for package, fields in recipe_srcinfos().items():
            with self.subTest(package=package):
                self.assertEqual(sorted(fields["license"]), sorted(REQUIRED_LICENSES))

    def test_each_srcinfo_declares_the_dependencies_the_checker_requires(self):
        for package, fields in recipe_srcinfos().items():
            with self.subTest(package=package):
                self.assertEqual(sorted(fields["depends"]), sorted(REQUIRED_DEPENDENCIES))

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
            write_package_tree(root, EXPECTED_FILES)
            validate_layout(root, PACKAGE, exercise_native_tools=False)

            notice = root / "usr/share/doc/termeric/THIRD_PARTY_NOTICES.md"
            notice.write_text("missing license")
            with self.assertRaisesRegex(CheckError, "Unicode License v3"):
                validate_layout(root, PACKAGE, exercise_native_tools=False)

    def test_layout_rejects_mode_and_file_type_regressions(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_package_tree(root, EXPECTED_FILES)
            (root / "usr/bin/termeric").chmod(0o777)
            with self.assertRaisesRegex(CheckError, "mode 777, expected 755"):
                validate_layout(root, PACKAGE, exercise_native_tools=False)

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
            validate_archive_modes(root, PACKAGE)
            (root / ".PKGINFO").chmod(0o666)
            with self.assertRaisesRegex(CheckError, "mode 666, expected 644"):
                validate_archive_modes(root, PACKAGE)


if __name__ == "__main__":
    unittest.main()
