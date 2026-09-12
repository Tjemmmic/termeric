import os
from pathlib import Path
import json
import stat
import subprocess
import tempfile
import unittest
from unittest import mock

from scripts.check_arch_package import (
    CheckError,
    EXPECTED_FILES,
    LIFECYCLE_OPT_IN,
    METADATA_FILES,
    REQUIRED_DEPENDENCIES,
    REQUIRED_LICENSES,
    SYNTHETIC_PREDECESSOR_VERSION,
    exercise_system_lifecycle,
    normalize_member_name,
    parse_pkginfo,
    parse_os_release,
    parse_verbose_types,
    replace_metadata_version,
    require_lifecycle_host,
    running_package_processes,
    validate_archive_inventory,
    validate_archive_modes,
    validate_layout,
    validate_package_metadata,
    verify_removed,
    verify_user_state,
    write_lifecycle_pacman_config,
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
        fields["depend"].remove("openssl")
        with self.assertRaisesRegex(CheckError, "openssl"):
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

    def test_synthetic_predecessor_version_passes_archive_validation(self):
        fields = valid_metadata()
        fields["pkgver"] = [SYNTHETIC_PREDECESSOR_VERSION]
        metadata = validate_package_metadata(fields)
        self.assertEqual(metadata["version"], SYNTHETIC_PREDECESSOR_VERSION)

    def test_os_release_recognizes_arch_derivatives(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "os-release"
            path.write_text('ID="cachyos"\nID_LIKE="arch linux"\n')
            self.assertEqual(parse_os_release(path), {"cachyos", "arch", "linux"})

    def test_lifecycle_mode_fails_closed_without_explicit_opt_in(self):
        with mock.patch.dict(os.environ, {}, clear=True):
            with self.assertRaisesRegex(CheckError, LIFECYCLE_OPT_IN):
                require_lifecycle_host()

    def test_lifecycle_host_rejects_rootless_missing_tools_and_existing_package(self):
        with mock.patch.dict(os.environ, {LIFECYCLE_OPT_IN: "1"}, clear=True):
            with mock.patch("scripts.check_arch_package.os.geteuid", return_value=1000):
                with self.assertRaisesRegex(CheckError, "must run as root"):
                    require_lifecycle_host()

            common = [
                mock.patch("scripts.check_arch_package.os.geteuid", return_value=0),
                mock.patch(
                    "scripts.check_arch_package.parse_os_release", return_value={"arch"}
                ),
            ]
            with common[0], common[1], mock.patch(
                "scripts.check_arch_package.require_tool",
                side_effect=CheckError("required tool is unavailable: pacman"),
            ):
                with self.assertRaisesRegex(CheckError, "unavailable: pacman"):
                    require_lifecycle_host()

            with mock.patch(
                "scripts.check_arch_package.os.geteuid", return_value=0
            ), mock.patch(
                "scripts.check_arch_package.parse_os_release", return_value={"arch"}
            ), mock.patch(
                "scripts.check_arch_package.require_tool", return_value="/usr/bin/tool"
            ), mock.patch(
                "scripts.check_arch_package.installed_version",
                side_effect=lambda name: "1.2.3-1" if name == "termeric" else None,
            ):
                with self.assertRaisesRegex(CheckError, "pre-existing termeric package"):
                    require_lifecycle_host()

    def test_lifecycle_command_failures_attempt_cleanup_only_after_install(self):
        successful = subprocess.CompletedProcess([], 0, "", "")
        scenarios = {
            "install": [CheckError("install failed")],
            "upgrade": [successful, CheckError("upgrade failed")],
            "remove": [successful, successful, CheckError("remove failed")],
        }
        for stage, outcomes in scenarios.items():
            with self.subTest(stage=stage), mock.patch(
                "scripts.check_arch_package.require_lifecycle_host"
            ), mock.patch(
                "scripts.check_arch_package.build_synthetic_predecessor",
                return_value=Path("/tmp/predecessor.pkg.tar.zst"),
            ), mock.patch(
                "scripts.check_arch_package.require_tool", return_value="/usr/bin/pacman"
            ), mock.patch(
                "scripts.check_arch_package.run", side_effect=outcomes
            ), mock.patch(
                "scripts.check_arch_package.verify_installed"
            ), mock.patch(
                "scripts.check_arch_package.installed_version",
                return_value=SYNTHETIC_PREDECESSOR_VERSION,
            ), mock.patch(
                "scripts.check_arch_package.subprocess.run"
            ) as cleanup, mock.patch("builtins.print"):
                with self.assertRaisesRegex(CheckError, f"{stage} failed"):
                    exercise_system_lifecycle(
                        Path("candidate.pkg.tar.zst"), "1.2.3-1"
                    )
                self.assertEqual(cleanup.call_count, 0 if stage == "install" else 1)

    def test_synthetic_version_replacement_is_exact(self):
        with tempfile.TemporaryDirectory() as directory:
            metadata = Path(directory) / ".PKGINFO"
            metadata.write_text("pkgname = termeric-git\npkgver = 1.2.3-1\n")
            replace_metadata_version(metadata, "0~test-1")
            self.assertEqual(
                metadata.read_text(),
                "pkgname = termeric-git\npkgver = 0~test-1\n",
            )

            metadata.write_text("pkgname = termeric-git\n")
            with self.assertRaisesRegex(CheckError, "no unique pkgver"):
                replace_metadata_version(metadata, "0~test-1")
            metadata.write_text("pkgver = 1-1\npkgver = 2-1\n")
            with self.assertRaisesRegex(CheckError, "no unique pkgver"):
                replace_metadata_version(metadata, "0~test-1")

    def test_lifecycle_pacman_config_removes_only_active_noextract_rules(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "pacman.conf"
            destination = root / "lifecycle.conf"
            source.write_text(
                "[options]\n"
                "#NoExtract = documented-default\n"
                "NoExtract = usr/share/doc/*\n"
                "  noextract=usr/share/man/*\n"
                "Architecture = auto\n"
                "[core]\n"
                "Include = /etc/pacman.d/mirrorlist\n"
            )

            write_lifecycle_pacman_config(source, destination)

            self.assertEqual(
                destination.read_text(),
                "[options]\n"
                "#NoExtract = documented-default\n"
                "Architecture = auto\n"
                "[core]\n"
                "Include = /etc/pacman.d/mirrorlist\n",
            )

    def test_layout_accepts_expected_files_and_modes_without_native_tools(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for relative, mode in EXPECTED_FILES.items():
                path = root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                if relative == "usr/bin/termeric":
                    contents = b"\x7fELFfixture"
                elif relative.endswith(".png"):
                    contents = b"\x89PNG\r\n\x1a\nfixture"
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
                    contents = b"\x89PNG\r\n\x1a\nfixture"
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

    def test_removal_rejects_owned_residue_and_live_deleted_executable(self):
        with tempfile.TemporaryDirectory() as directory:
            temporary = Path(directory)
            root = temporary / "root"
            proc = temporary / "proc"
            root.mkdir()
            proc.mkdir()
            with mock.patch(
                "scripts.check_arch_package.installed_version", return_value=None
            ):
                (root / "usr/bin").mkdir(parents=True)
                (root / "usr/bin/termeric").write_text("residue")
                with self.assertRaisesRegex(CheckError, "owned files remain"):
                    verify_removed(root, proc)
                (root / "usr/bin/termeric").unlink()

                (proc / "42").mkdir()
                (proc / "42/exe").symlink_to("/usr/bin/termeric (deleted)")
                self.assertEqual(running_package_processes(proc), [42])
                with self.assertRaisesRegex(CheckError, "PIDs: 42"):
                    verify_removed(root, proc)

    def test_user_state_retention_rejects_changed_or_deleted_sentinel(self):
        with tempfile.TemporaryDirectory() as directory:
            sentinel = Path(directory) / "lifecycle-sentinel"
            sentinel.write_text("retain user state\n")
            verify_user_state(sentinel)
            sentinel.write_text("changed\n")
            with self.assertRaisesRegex(CheckError, "changed synthetic"):
                verify_user_state(sentinel)
            sentinel.unlink()
            with self.assertRaisesRegex(CheckError, "changed synthetic"):
                verify_user_state(sentinel)

    def test_report_hashes_archive_and_marks_lifecycle_unperformed(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            package = root / "termeric.pkg.tar.zst"
            package.write_bytes(b"package fixture")
            report = root / "report.json"
            write_report(
                report,
                package,
                status="pass",
                metadata={"name": "termeric-git", "version": "1-1"},
            )
            payload = json.loads(report.read_text())
            self.assertEqual(payload["schema"], 1)
            self.assertEqual(payload["archive_contract"], "pass")
            self.assertFalse(payload["lifecycle"]["performed"])
            self.assertRegex(payload["package"]["sha256"], r"^[0-9a-f]{64}$")
            self.assertNotIn(str(root), report.read_text())


if __name__ == "__main__":
    unittest.main()
    replace_metadata_version,
    require_lifecycle_host,
    exercise_system_lifecycle,
