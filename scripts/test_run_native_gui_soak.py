import base64
import hashlib
import os
import unittest
from unittest import mock
from pathlib import Path
from tempfile import TemporaryDirectory

from scripts.run_native_gui_soak import (
    AcceptanceError,
    MAX_DRIVER_ERROR_CHARACTERS,
    NATIVE_SOAK_SCHEMA_VERSION,
    bounded_driver_error,
    build_native_soak_checks,
    evaluate_native_checks,
    fixture_arguments,
    git_provenance,
    kitty_image_ack_count,
    native_artifact_manifest,
    perform_until,
    prepare_artifact_directory,
    progress_count,
    require_clean_candidate,
    runtime_error_count,
)
from scripts.run_soak import summarize_resource_inventories


def sample(index: int, *, rss_kib: float = 200_000, fds: int = 30) -> dict:
    return {
        "elapsed_seconds": float(index),
        "rss_kib": rss_kib,
        "threads": 8,
        "fds": fds,
        "children": [100],
        "cpu_ticks": index,
        "inventory": {
            "fd_kinds": {"socket": fds},
            "thread_names": {"termeric": 8},
            "child_names": {"fixture": 1},
        },
    }


class NativeGuiSoakTests(unittest.TestCase):
    def test_report_schema_is_versioned_for_resource_inventories(self) -> None:
        self.assertEqual(NATIVE_SOAK_SCHEMA_VERSION, 4)

    def evaluate(self, samples: list[dict], **overrides: object) -> list[dict]:
        arguments = {
            "max_rss_kib": 500_000,
            "max_rss_growth_kib": 50_000,
            "max_thread_growth": 2,
            "max_fd_growth": 4,
            "max_children": 2,
            "minimum_samples": 6,
            "exit_status": 0,
            "progress_records": 30,
            "minimum_progress": 10,
            "resize_actions": 6,
            "minimum_resize_actions": 4,
            "pane_actions": 6,
            "minimum_pane_actions": 4,
            "graphics_acks": 8,
            "minimum_graphics_acks": 3,
            "remaining_children": 0,
            "runtime_errors": 0,
        }
        arguments.update(overrides)
        return evaluate_native_checks(samples, **arguments)

    def test_flat_displayed_process_evidence_passes(self) -> None:
        self.assertTrue(
            all(check["passed"] for check in self.evaluate([sample(i) for i in range(12)]))
        )

    def test_growth_missing_progress_and_child_leak_fail_independently(self) -> None:
        samples = [sample(i, rss_kib=200_000 + i * 10_000, fds=30 + i) for i in range(12)]
        checks = {
            check["name"]: check
            for check in self.evaluate(
                samples,
                progress_records=0,
                remaining_children=1,
            )
        }
        self.assertFalse(checks["first-to-last RSS growth"]["passed"])
        self.assertFalse(checks["first-to-last fd growth"]["passed"])
        self.assertFalse(checks["continuous PTY progress records"]["passed"])
        self.assertFalse(checks["observed child processes cleaned up"]["passed"])

    def test_sparse_native_sampling_fails_before_claiming_growth_stability(self) -> None:
        checks = {
            check["name"]: check
            for check in self.evaluate([sample(0)], minimum_samples=6)
        }
        self.assertFalse(checks["resource sample count"]["passed"])

    def test_native_samples_require_reconciled_diagnostic_inventories(self) -> None:
        samples = [sample(i) for i in range(12)]
        samples[4]["inventory"]["child_names"] = {}
        checks = {check["name"]: check for check in self.evaluate(samples)}
        self.assertFalse(checks["resource inventory coverage"]["passed"])

    def test_native_inventory_summary_uses_the_shared_quartile_contract(self) -> None:
        samples = [sample(i, fds=30 + i) for i in range(8)]
        for index, item in enumerate(samples):
            item["inventory"]["fd_kinds"] = {"socket": 30 + index}
        summary = summarize_resource_inventories(samples)
        self.assertEqual(summary["fd_kinds"]["socket"]["growth"], 6.0)
        self.assertEqual(summary["fd_kinds"]["socket"]["peak"], 37.0)

    def test_resize_and_pane_churn_are_required_independently(self) -> None:
        missing_resize = {
            check["name"]: check
            for check in self.evaluate(
                [sample(i) for i in range(12)], resize_actions=0
            )
        }
        self.assertFalse(missing_resize["native output resize actions"]["passed"])
        self.assertTrue(missing_resize["native pane-churn actions"]["passed"])

        missing_panes = {
            check["name"]: check
            for check in self.evaluate(
                [sample(i) for i in range(12)], pane_actions=0
            )
        }
        self.assertTrue(missing_panes["native output resize actions"]["passed"])
        self.assertFalse(missing_panes["native pane-churn actions"]["passed"])

    def test_missing_graphics_acknowledgements_fail_independently(self) -> None:
        checks = {
            check["name"]: check
            for check in self.evaluate(
                [sample(i) for i in range(12)], graphics_acks=0
            )
        }
        self.assertFalse(checks["accepted Kitty image replacements"]["passed"])
        self.assertTrue(checks["continuous PTY progress records"]["passed"])

    def test_exact_kitty_acknowledgements_are_counted_from_fragmented_input(self) -> None:
        acknowledgement = b"\x1b_Gi=91;OK\x1b\\"
        with TemporaryDirectory() as directory:
            transcript = Path(directory) / "transcript.jsonl"
            chunks = [acknowledgement[:7], acknowledgement[7:] + acknowledgement]
            transcript.write_text(
                "".join(
                    '{"event":"input","base64":"'
                    + base64.b64encode(chunk).decode()
                    + '"}\n'
                    for chunk in chunks
                ),
                encoding="utf-8",
            )
            self.assertEqual(kitty_image_ack_count(transcript), 2)

    def test_runtime_error_scanner_finds_gpu_and_panic_failures(self) -> None:
        self.assertEqual(
            runtime_error_count("Validation Error\npanicked at source\ndevice lost\n"),
            3,
        )
        self.assertEqual(runtime_error_count("ordinary warning\n"), 0)

    def test_git_provenance_records_commit_and_dirty_state(self) -> None:
        clean = mock.Mock(stdout="a" * 40 + "\n")
        dirty = mock.Mock(stdout=" M scripts/run_native_gui_soak.py\n")
        with mock.patch(
            "scripts.run_native_gui_soak.subprocess.run",
            side_effect=[clean, dirty],
        ) as run:
            self.assertEqual(
                git_provenance(),
                {"commit": "a" * 40, "worktree_dirty": True},
            )
        self.assertEqual(run.call_count, 2)
        self.assertEqual(
            run.call_args_list[1].args[0],
            ["git", "status", "--porcelain=v1", "--untracked-files=all"],
        )

    def test_clean_candidate_mode_rejects_only_dirty_evidence(self) -> None:
        require_clean_candidate({"worktree_dirty": False}, True)
        require_clean_candidate({"worktree_dirty": True}, False)
        with self.assertRaisesRegex(AcceptanceError, "dirty worktree"):
            require_clean_candidate({"worktree_dirty": True}, True)

    def test_progress_and_fixture_contract(self) -> None:
        records = [
            {"event": "started"},
            {"event": "soak_progress"},
            {"event": "soak_progress"},
        ]
        self.assertEqual(progress_count(records), 2)
        arguments = fixture_arguments(Path("transcript.jsonl"), 7)
        self.assertIn("--soak-output", arguments)
        self.assertIn("--skip-external-graphics", arguments)

    def test_native_action_retries_only_until_observable_effect(self) -> None:
        action = mock.Mock()
        with mock.patch(
            "scripts.run_native_gui_soak.wait_until",
            side_effect=[AcceptanceError("first attempt"), None],
        ) as wait:
            perform_until("pane startup", action, lambda: False)
        self.assertEqual(action.call_count, 2)
        self.assertEqual(wait.call_count, 2)

    def test_driver_error_is_single_line_bounded_and_reconstructable(self) -> None:
        error = bounded_driver_error(" first\nsecond\x00 " + "x" * 2_000)
        self.assertEqual(len(error), MAX_DRIVER_ERROR_CHARACTERS)
        self.assertNotIn("\n", error)
        self.assertNotIn("\x00", error)
        checks = build_native_soak_checks(
            [sample(index) for index in range(6)],
            {
                "max_rss_kib": 500_000,
                "max_rss_growth_kib": 50_000,
                "max_thread_growth": 2,
                "max_fd_growth": 4,
                "max_children": 2,
                "minimum_resource_samples": 3,
                "minimum_progress_records": 1,
                "minimum_resize_actions": 1,
                "minimum_pane_actions": 1,
                "minimum_graphics_acknowledgements": 1,
            },
            {
                "exit_status": 1,
                "progress_records": 1,
                "resize_actions": 1,
                "pane_actions": 1,
                "graphics_acknowledgements": 1,
                "remaining_children": 0,
                "runtime_errors": 0,
                "driver_error": error,
            },
        )
        self.assertFalse(checks[-1]["passed"])
        self.assertEqual(checks[-1]["actual"], error)

    def test_artifact_destination_must_be_empty_and_manifest_is_exact(self) -> None:
        with TemporaryDirectory() as directory:
            root = Path(directory) / "bundle"
            prepare_artifact_directory(root)
            (root / "nested").mkdir()
            artifact = root / "nested" / "log.txt"
            artifact.write_text("evidence", encoding="utf-8")
            self.assertEqual(
                native_artifact_manifest(root),
                [
                    {
                        "path": "nested/log.txt",
                        "bytes": 8,
                        "sha256": hashlib.sha256(b"evidence").hexdigest(),
                    }
                ],
            )
            with self.assertRaisesRegex(AcceptanceError, "non-empty"):
                prepare_artifact_directory(root)
            symlink = root / "linked.log"
            symlink.symlink_to(artifact)
            with self.assertRaisesRegex(AcceptanceError, "symlink"):
                native_artifact_manifest(root)
            symlink.unlink()
            if hasattr(os, "mkfifo"):
                os.mkfifo(root / "special-pipe")
                with self.assertRaisesRegex(AcceptanceError, "regular file"):
                    native_artifact_manifest(root)

    def test_artifact_manifest_enforces_file_count_and_byte_limits(self) -> None:
        with TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "one").write_bytes(b"1234")
            (root / "two").write_bytes(b"5678")
            with mock.patch(
                "scripts.run_native_gui_soak.MAX_NATIVE_SOAK_ARTIFACT_BYTES", 3
            ):
                with self.assertRaisesRegex(AcceptanceError, "byte limit"):
                    native_artifact_manifest(root)
            with mock.patch(
                "scripts.run_native_gui_soak.MAX_NATIVE_SOAK_TOTAL_ARTIFACT_BYTES",
                7,
            ):
                with self.assertRaisesRegex(AcceptanceError, "bundle exceeds"):
                    native_artifact_manifest(root)
            with mock.patch(
                "scripts.run_native_gui_soak.MAX_NATIVE_SOAK_ARTIFACTS", 1
            ):
                with self.assertRaisesRegex(AcceptanceError, "too many"):
                    native_artifact_manifest(root)


if __name__ == "__main__":
    unittest.main()
