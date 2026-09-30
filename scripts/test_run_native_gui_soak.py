import base64
import unittest
from unittest import mock
from pathlib import Path
from tempfile import TemporaryDirectory

from scripts.run_native_gui_soak import (
    AcceptanceError,
    MAX_DRIVER_ERROR_CHARACTERS,
    bounded_driver_error,
    bounded_process_tree_pids,
    application_log_check,
    build_native_soak_checks,
    evaluate_native_checks,
    fixture_arguments,
    kitty_image_ack_count,
    perform_until,
    prepare_artifact_directory,
    progress_count,
    runtime_error_count,
)
from scripts.drm_memory import aggregate_drm_fdinfos
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
        "gpu_memory": aggregate_drm_fdinfos([]),
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
            "require_gpu_memory": False,
            "max_drm_resident_bytes": 1024 * 1024 * 1024,
            "max_drm_resident_growth_bytes": 256 * 1024 * 1024,
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

    def test_required_gpu_memory_rejects_explicit_unavailability(self) -> None:
        checks = {
            check["name"]: check
            for check in self.evaluate(
                [sample(i) for i in range(12)], require_gpu_memory=True
            )
        }
        self.assertFalse(checks["required DRM memory sample coverage"]["passed"])
        self.assertTrue(checks["DRM memory disposition consistency"]["passed"])

    def test_process_tree_sampling_is_recursive_unique_and_bounded(self) -> None:
        with TemporaryDirectory() as directory:
            root = Path(directory)
            for pid, children in {10: "11 12", 11: "12 13", 12: "", 13: ""}.items():
                task = root / str(pid) / "task" / str(pid)
                task.mkdir(parents=True)
                (task / "children").write_text(children)
            self.assertEqual(bounded_process_tree_pids(root, 10), [10, 11, 12, 13])
            with mock.patch("scripts.run_native_gui_soak.MAX_DRM_PROCESSES", 3):
                with self.assertRaisesRegex(AcceptanceError, "sampling ceiling"):
                    bounded_process_tree_pids(root, 10)

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
                "require_gpu_memory": False,
                "max_drm_resident_bytes": 1024 * 1024 * 1024,
                "max_drm_resident_growth_bytes": 256 * 1024 * 1024,
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


    def test_missing_application_log_fails_its_check(self) -> None:
        with TemporaryDirectory() as directory:
            log = Path(directory) / "termeric.log"
            self.assertFalse(application_log_check(log)["passed"])
            log.write_text("", encoding="utf-8")
            self.assertTrue(application_log_check(log)["passed"])

    def test_artifact_destination_must_be_new_or_empty(self) -> None:
        with TemporaryDirectory() as directory:
            root = Path(directory) / "bundle"
            prepare_artifact_directory(root)
            (root / "stale.log").write_text("old run", encoding="utf-8")
            with self.assertRaisesRegex(AcceptanceError, "non-empty"):
                prepare_artifact_directory(root)


if __name__ == "__main__":
    unittest.main()
