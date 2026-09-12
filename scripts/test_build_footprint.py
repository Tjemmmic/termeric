"""Hermetic tests for Cargo target-directory footprint accounting."""

from __future__ import annotations

from contextlib import redirect_stdout
import io
import json
import os
from pathlib import Path
import tempfile
import tomllib
import unittest
from unittest import mock

from scripts import build_footprint


ROOT = Path(__file__).resolve().parents[1]


def sized_file(root: Path, relative: str, size: int) -> None:
    path = root / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"x" * size)


class BuildProfileContractTests(unittest.TestCase):
    def test_default_and_full_debug_profiles_are_explicit(self) -> None:
        manifest = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
        self.assertEqual(manifest["profile"]["dev"]["debug"], "line-tables-only")
        self.assertEqual(manifest["profile"]["debugging"]["inherits"], "dev")
        self.assertEqual(manifest["profile"]["debugging"]["debug"], 2)
        self.assertNotIn("incremental", manifest["profile"]["dev"])


class BuildFootprintTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.target = Path(self.temporary.name)
        for relative, size in (
            ("debug/deps/libexample.rlib", 3),
            ("debug/incremental/cache.bin", 5),
            ("debug/build/example/out/generated", 7),
            ("debug/.fingerprint/example/state", 11),
            ("debug/termeric", 13),
            ("release/examples/example", 17),
            ("doc/index.html", 19),
            ("release-preflight/report.json", 23),
            ("x86_64-unknown-linux-gnu/debug/deps/cross.rlib", 29),
        ):
            sized_file(self.target, relative, size)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def test_exact_deterministic_accounting_and_classification(self) -> None:
        first = build_footprint.build_report(
            self.target, large_file_bytes=10, large_file_limit=20
        )
        second = build_footprint.build_report(
            self.target, large_file_bytes=10, large_file_limit=20
        )
        self.assertEqual(first, second)
        self.assertEqual(first["totals"], {"bytes": 127, "files": 9})
        self.assertEqual(first["profiles"]["debug"], {"bytes": 39, "files": 5})
        self.assertEqual(first["profiles"]["release"], {"bytes": 17, "files": 1})
        self.assertEqual(first["profiles"]["doc"], {"bytes": 19, "files": 1})
        self.assertEqual(first["profiles"]["<root>"], {"bytes": 23, "files": 1})
        self.assertEqual(
            first["profiles"]["x86_64-unknown-linux-gnu/debug"],
            {"bytes": 29, "files": 1},
        )
        self.assertEqual(first["classes"]["dependencies"], {"bytes": 32, "files": 2})
        self.assertEqual(first["classes"]["incremental"], {"bytes": 5, "files": 1})
        self.assertEqual(first["classes"]["build-scripts"], {"bytes": 7, "files": 1})
        self.assertEqual(first["classes"]["metadata"], {"bytes": 11, "files": 1})
        self.assertEqual(first["classes"]["top-level-products"], {"bytes": 13, "files": 1})
        self.assertEqual(first["classes"]["examples-benches"], {"bytes": 17, "files": 1})
        self.assertEqual(first["classes"]["documentation"], {"bytes": 19, "files": 1})
        self.assertEqual(first["classes"]["other"], {"bytes": 23, "files": 1})
        self.assertEqual(
            [item["bytes"] for item in first["large_files"]],
            [29, 23, 19, 17, 13, 11],
        )

    def test_limits_are_opt_in_and_fail_independently(self) -> None:
        unrestricted = build_footprint.build_report(self.target)
        self.assertEqual(unrestricted["status"], "pass")
        self.assertEqual(unrestricted["warnings"], [])
        self.assertEqual(unrestricted["violations"], [])

        warned = build_footprint.build_report(
            self.target, warn_bytes=126, warn_files=8
        )
        self.assertEqual(warned["status"], "warning")
        self.assertEqual(len(warned["warnings"]), 2)
        self.assertEqual(warned["violations"], [])

        limited = build_footprint.build_report(
            self.target,
            warn_bytes=100,
            warn_files=7,
            max_bytes=126,
            max_files=8,
        )
        self.assertEqual(limited["status"], "fail")
        self.assertEqual(limited["warnings"], [])
        self.assertEqual(len(limited["violations"]), 2)

    def test_warning_must_not_exceed_failure_limit(self) -> None:
        with self.assertRaisesRegex(build_footprint.FootprintError, "warning bytes"):
            build_footprint.build_report(self.target, warn_bytes=128, max_bytes=127)
        with self.assertRaisesRegex(build_footprint.FootprintError, "warning files"):
            build_footprint.build_report(self.target, warn_files=10, max_files=9)

    def test_json_cli_is_versioned_and_limit_failure_returns_one(self) -> None:
        output = io.StringIO()
        with redirect_stdout(output):
            result = build_footprint.main(
                [os.fspath(self.target), "--json", "--max-bytes", "126"]
            )
        report = json.loads(output.getvalue())
        self.assertEqual(result, 1)
        self.assertEqual(report["schema_version"], 1)
        self.assertEqual(report["kind"], "termeric-cargo-build-footprint")
        self.assertEqual(report["status"], "fail")

    def test_warning_cli_returns_success(self) -> None:
        output = io.StringIO()
        with redirect_stdout(output):
            result = build_footprint.main(
                [os.fspath(self.target), "--json", "--warn-files", "8"]
            )
        report = json.loads(output.getvalue())
        self.assertEqual(result, 0)
        self.assertEqual(report["status"], "warning")

    @unittest.skipIf(os.name == "nt", "symlink creation is not generally permitted")
    def test_symlinks_are_reported_but_never_followed(self) -> None:
        (self.target / "debug" / "linked-product").symlink_to(
            self.target / "debug" / "termeric"
        )
        (self.target / "linked-directory").symlink_to(
            self.target / "debug", target_is_directory=True
        )
        report = build_footprint.build_report(self.target)
        self.assertEqual(report["totals"], {"bytes": 127, "files": 9})
        self.assertEqual(report["skipped"]["symlinks"], 2)

    @unittest.skipIf(os.name == "nt", "symlink creation is not generally permitted")
    def test_symlink_target_directory_is_rejected(self) -> None:
        link = self.target.parent / f"{self.target.name}-link"
        link.symlink_to(self.target, target_is_directory=True)
        self.addCleanup(link.unlink)
        with self.assertRaisesRegex(build_footprint.FootprintError, "must not be a symlink"):
            build_footprint.build_report(link)

    def test_missing_and_non_directory_targets_are_rejected(self) -> None:
        with self.assertRaisesRegex(build_footprint.FootprintError, "cannot inspect"):
            build_footprint.build_report(self.target / "missing")
        regular = self.target / "regular-file"
        regular.write_bytes(b"data")
        with self.assertRaisesRegex(build_footprint.FootprintError, "not a directory"):
            build_footprint.build_report(regular)

    def test_unreadable_directory_cannot_become_zero_size_success(self) -> None:
        with mock.patch.object(
            build_footprint.os,
            "scandir",
            side_effect=PermissionError("denied"),
        ):
            with self.assertRaisesRegex(build_footprint.FootprintError, "cannot read"):
                build_footprint.build_report(self.target)


if __name__ == "__main__":
    unittest.main()
