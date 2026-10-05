import os
from pathlib import Path
import stat
import subprocess
import tempfile
import unittest

from scripts.stage_install_tree import (
    APP_ID,
    INSTALLER_NAME,
    MANIFEST_NAME,
    QUICK_ACCESS_APP_ID,
    ROOT,
    StageError,
    format_manifest,
    manifest,
    parse_manifest,
    stage,
    tree_sources,
)


def staged_tree(directory: Path) -> Path:
    """A tree staged from the repository around a stand-in binary."""
    binary = directory / "termeric"
    binary.write_bytes(b"\x7fELFfixture")
    tree = directory / "tree"
    stage(binary, tree)
    return tree


def run_installer(tree: Path, home: Path, *arguments: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["sh", str(tree / INSTALLER_NAME), *arguments],
        env={**os.environ, "HOME": str(home)},
        capture_output=True,
        text=True,
        timeout=60,
    )


def files_below(root: Path) -> set[str]:
    return {str(path.relative_to(root)) for path in root.rglob("*") if not path.is_dir()}


class StageInstallTreeTests(unittest.TestCase):
    def test_every_source_is_a_file_in_the_repository(self):
        for path, source in tree_sources().items():
            with self.subTest(path=path):
                self.assertTrue((ROOT / source).is_file(), source)

    def test_manifest_round_trips_and_rejects_malformed_lines(self):
        entries = manifest()
        self.assertEqual(entries["bin/termeric"], 0o755)
        self.assertEqual(parse_manifest(format_manifest(entries)), entries)
        for line in ["644", "600 share/x", "644 share/x\n644 share/x"]:
            with self.subTest(line=line), self.assertRaises(StageError):
                parse_manifest(line)

    def test_stage_writes_exactly_the_manifest_with_its_modes(self):
        with tempfile.TemporaryDirectory() as directory:
            tree = staged_tree(Path(directory))
            entries = parse_manifest((tree / MANIFEST_NAME).read_text(encoding="utf-8"))
            self.assertEqual(entries, manifest())
            self.assertEqual(files_below(tree), set(entries) | {MANIFEST_NAME, INSTALLER_NAME})
            for path, mode in entries.items():
                self.assertEqual(stat.S_IMODE((tree / path).stat().st_mode), mode, path)
            self.assertEqual(stat.S_IMODE((tree / INSTALLER_NAME).stat().st_mode), 0o755)
            self.assertIn(f"share/applications/{APP_ID}.desktop", entries)
            self.assertIn(f"share/metainfo/{APP_ID}.metainfo.xml", entries)

    def test_stage_refuses_a_used_directory_and_a_non_executable(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            tree = staged_tree(root)
            with self.assertRaisesRegex(StageError, "new or empty directory"):
                stage(root / "termeric", tree)
            (root / "script").write_text("#!/bin/sh\n")
            with self.assertRaisesRegex(StageError, "not an ELF executable"):
                stage(root / "script", root / "other")


class InstallScriptTests(unittest.TestCase):
    def test_install_and_uninstall_under_a_user_prefix(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            tree = staged_tree(root)
            prefix = root / "home dir/.local"
            home = root / "home dir"
            result = run_installer(tree, home)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("fpath=(", result.stdout)

            entries = manifest()
            for path, mode in entries.items():
                installed = prefix / path
                self.assertTrue(installed.is_file(), path)
                self.assertEqual(stat.S_IMODE(installed.stat().st_mode), mode, path)
            binary = f'"{prefix}/bin/termeric"'
            desktop = (prefix / f"share/applications/{APP_ID}.desktop").read_text()
            self.assertIn(f"\nExec={binary}\n", desktop)
            self.assertIn(f"\nExec={binary} --new-window\n", desktop)
            quick = (prefix / f"share/applications/{QUICK_ACCESS_APP_ID}.desktop").read_text()
            self.assertIn(f"\nExec={binary} --quick-access\n", quick)
            record = prefix / "lib/termeric/installed-files"
            self.assertEqual(
                record.read_text().splitlines(), [f"{prefix}/{path}" for path in entries]
            )

            # Installing again over the same prefix is an upgrade.
            result = run_installer(tree, home, "--prefix", f"{prefix}/")
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(len(record.read_text().splitlines()), len(entries))

            result = run_installer(tree, home, "--uninstall")
            self.assertEqual(result.returncode, 0, result.stderr)
            # Only a desktop database the refresh may have written is left.
            self.assertLessEqual(files_below(prefix), {"share/applications/mimeinfo.cache"})
            self.assertFalse((prefix / "lib/termeric").exists())

            result = run_installer(tree, home, "--uninstall")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("no Termeric installation is recorded", result.stderr)

    def test_an_install_or_upgrade_that_fails_partway_can_still_be_uninstalled(self):
        missing = "share/terminfo/x/xterm-termeric"
        entries = list(manifest())
        # Files come before and after the one that will be missing.
        self.assertLess(0, entries.index(missing))
        self.assertLess(entries.index(missing), len(entries) - 1)
        for upgrade in [False, True]:
            with self.subTest(upgrade=upgrade), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                tree = staged_tree(root)
                home = root / "home"
                prefix = home / ".local"
                record = prefix / "lib/termeric/installed-files"
                if upgrade:
                    result = run_installer(tree, home)
                    self.assertEqual(result.returncode, 0, result.stderr)
                source = tree / missing
                contents = source.read_bytes()
                source.unlink()

                result = run_installer(tree, home)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(f"MANIFEST lists {missing}, which is missing", result.stderr)
                # Less the record and a desktop database the first install's
                # refresh may have written.
                written = files_below(prefix) - {
                    "lib/termeric/installed-files",
                    "share/applications/mimeinfo.cache",
                }
                self.assertIn("bin/termeric", written)
                self.assertNotIn(entries[-1], written)
                self.assertFalse(any(path.endswith(".install-tmp") for path in written))
                recorded = {
                    line.removeprefix(f"{prefix}/") for line in record.read_text().splitlines()
                }
                self.assertLessEqual(written, recorded)

                source.write_bytes(contents)
                result = run_installer(tree, home, "--uninstall")
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertLessEqual(files_below(prefix), {"share/applications/mimeinfo.cache"})
                self.assertFalse((prefix / "lib/termeric").exists())

    def test_finds_its_own_directory_whatever_cdpath_holds(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            staged_tree(root)
            # A decoy that cd would pick for the relative "tree" through CDPATH.
            decoy = root / "decoy"
            (decoy / "tree").mkdir(parents=True)
            package = root / "package"
            result = subprocess.run(
                ["sh", f"tree/{INSTALLER_NAME}", "--prefix", "/usr", "--destdir", str(package)],
                cwd=root,
                env={**os.environ, "HOME": str(root), "CDPATH": str(decoy)},
                capture_output=True,
                text=True,
                timeout=60,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(files_below(package), {f"usr/{path}" for path in manifest()})

    def test_destdir_stages_for_a_package_without_a_record(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            tree = staged_tree(root)
            package = root / "package"
            result = run_installer(tree, root, "--prefix", "/usr", "--destdir", str(package))
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(files_below(package), {f"usr/{path}" for path in manifest()})
            self.assertEqual(
                (package / f"usr/share/applications/{APP_ID}.desktop").read_bytes(),
                (ROOT / f"assets/{APP_ID}.desktop").read_bytes(),
            )

    def test_uninstall_refuses_a_recorded_path_outside_the_prefix(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            tree = staged_tree(root)
            victim = root / "victim"
            victim.write_text("keep")
            record = root / "prefix/lib/termeric/installed-files"
            record.parent.mkdir(parents=True)
            record.write_text(f"{victim}\n")
            result = run_installer(tree, root, "--prefix", str(root / "prefix"), "--uninstall")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("outside", result.stderr)
            self.assertTrue(victim.exists())

    def test_rejects_prefixes_it_cannot_write_into_a_desktop_entry(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            tree = staged_tree(root)
            for prefix in ["relative", "/", f"{root}/a$b", f'{root}/a"b']:
                with self.subTest(prefix=prefix):
                    result = run_installer(tree, root, "--prefix", prefix)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("--prefix", result.stderr)
            result = run_installer(tree, root, "--unknown")
            self.assertNotEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main()
