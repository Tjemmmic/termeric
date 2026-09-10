import base64
import unittest
from unittest import mock
from pathlib import Path
from tempfile import TemporaryDirectory

from scripts.run_native_gui_soak import (
    AcceptanceError,
    evaluate_native_checks,
    fixture_arguments,
    git_provenance,
    kitty_image_ack_count,
    perform_until,
    progress_count,
    require_clean_candidate,
    runtime_error_count,
)


def sample(index: int, *, rss_kib: float = 200_000, fds: int = 30) -> dict:
    return {
        "elapsed_seconds": float(index),
        "rss_kib": rss_kib,
        "threads": 8,
        "fds": fds,
        "children": [100],
        "cpu_ticks": index,
    }


class NativeGuiSoakTests(unittest.TestCase):
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
            run.call_args_list[1].args[0], ["git", "status", "--porcelain"]
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


if __name__ == "__main__":
    unittest.main()
