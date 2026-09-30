import contextlib
import io
import json
import subprocess
import sys
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import patch

from scripts import run_soak
from scripts.run_soak import (
    COLLAPSED_INVENTORY_LABEL,
    MAX_INVENTORY_LABELS,
    MAX_PROCESS_LABEL_CHARACTERS,
    bounded_process_label,
    classify_file_descriptor,
    counted_labels,
    evaluate_outcome_checks,
    evaluate_diff_history_checks,
    evaluate_progress_ceilings,
    evaluate_resource_ceilings,
    inventory_coverage_check,
    minimum_sample_count,
    parse_progress,
    process_children,
    process_snapshot,
    summarize_resource_inventories,
)


def sample(index: int, *, rss: float = 100_000, threads: int = 4, fds: int = 12) -> dict:
    return {
        "elapsed_seconds": float(index),
        "rss_kib": rss,
        "threads": threads,
        "fds": fds,
        "children": [],
        "cpu_ticks": index,
        "inventory": {
            "fd_kinds": {"filesystem": fds},
            "thread_names": {"termeric": threads},
            "child_names": {},
        },
    }


class SoakCeilingTests(unittest.TestCase):
    def test_exited_process_without_rss_is_an_unavailable_sample_not_zero_memory(self):
        for state in ("Z (zombie)", "X (dead)", "x (dead)"):
            with patch("pathlib.Path.read_text", return_value=f"Name:\tfixture\nState:\t{state}\n"):
                with self.assertRaises(ProcessLookupError):
                    process_snapshot(12345, 0)

    def test_live_process_missing_rss_is_not_silently_accepted(self):
        with patch("pathlib.Path.read_text", return_value="Name:\tfixture\nState:\tS (sleeping)\n"):
            with self.assertRaises(KeyError):
                process_snapshot(12345, 0)

    def test_diff_history_requires_pressure_eviction_and_owned_capacity_bounds(self):
        valid = {"iteration": 1, "diff_completed": 80, "diff_evicted": 72,
                 "diff_retained": 8, "diff_history_peak_count": 64,
                 "diff_history_peak_bytes": 3_900_000}
        second = {**valid, "iteration": 2, "diff_evicted": 80}
        self.assertTrue(all(check["passed"] for check in evaluate_diff_history_checks([valid, second])))
        for field, value in [
            ("diff_completed", 79), ("diff_evicted", 0), ("diff_evicted", 71),
            ("diff_retained", 65), ("diff_history_peak_count", 65),
            ("diff_history_peak_count", 63), ("diff_history_peak_bytes", 0),
            ("diff_history_peak_bytes", 4 * 1024 * 1024 + 1),
            ("diff_completed", True), ("diff_completed", -1),
        ]:
            with self.subTest(field=field, value=value):
                self.assertFalse(all(check["passed"] for check in evaluate_diff_history_checks([{**valid, field: value}])))
        for field in valid.keys() - {"iteration"}:
            missing = {key: value for key, value in valid.items() if key != field}
            self.assertFalse(all(check["passed"] for check in evaluate_diff_history_checks([missing])))
        self.assertFalse(all(check["passed"] for check in evaluate_diff_history_checks([])))
        # Each record must reconcile against its predecessor, not just totals.
        self.assertFalse(all(check["passed"] for check in evaluate_diff_history_checks([valid, valid])))

    def test_progress_iterations_must_be_contiguous_and_end_with_one_completion(self):
        def record(iteration):
            return {"iteration": iteration, "transcript_queue_bytes": 1024,
                    "transcript_queue_bytes_after_finish": 0,
                    "transcript_entries_accepted": 68, "transcript_entries_dropped": 0}

        def contiguous(progress):
            checks = evaluate_progress_ceilings(progress, max_transcript_queue_bytes=4096)
            return next(check for check in checks if check["name"].startswith("iterations are contiguous"))["passed"]

        done = {"complete": True, "iterations": 2}
        self.assertTrue(contiguous([record(1), record(2), done]))
        for name, progress in {
            "gap": [record(1), record(3), {"complete": True, "iterations": 2}],
            "duplicate": [record(1), record(1), done],
            "no completion": [record(1), record(2)],
            "completion mid-stream": [record(1), done, record(2)],
            "two completions": [record(1), record(2), done, done],
            "wrong count": [record(1), record(2), {"complete": True, "iterations": 3}],
            "empty": [],
        }.items():
            with self.subTest(name=name):
                self.assertFalse(contiguous(progress))

    def evaluate(self, samples: list[dict]) -> list[dict]:
        return evaluate_resource_ceilings(
            samples,
            max_rss_kib=500_000,
            max_rss_growth_kib=10_000,
            max_thread_growth=2,
            max_fd_growth=4,
            max_children=2,
        )

    def test_flat_resources_pass(self):
        self.assertTrue(all(check["passed"] for check in self.evaluate([sample(i) for i in range(12)])))

    def test_one_sample_cannot_claim_growth_stability(self):
        checks = {check["name"]: check for check in self.evaluate([sample(0)])}
        self.assertFalse(checks["resource sample count"]["passed"])

    def test_minimum_sample_count_requires_half_cadence_and_two_endpoints(self):
        self.assertEqual(minimum_sample_count(30, 0.5), 30)
        self.assertEqual(minimum_sample_count(1, 10), 2)
        with self.assertRaises(ValueError):
            minimum_sample_count(30, 0)

    def test_deliberate_fd_leak_double_fails_growth_ceiling(self):
        samples = [sample(i, fds=12 + i) for i in range(12)]
        checks = {check["name"]: check for check in self.evaluate(samples)}
        self.assertFalse(checks["first-to-last fd growth"]["passed"])

    def test_deliberate_rss_and_thread_leak_doubles_fail(self):
        samples = [sample(i, rss=100_000 + i * 4_000, threads=4 + i) for i in range(12)]
        checks = {check["name"]: check for check in self.evaluate(samples)}
        self.assertFalse(checks["first-to-last RSS growth"]["passed"])
        self.assertFalse(checks["first-to-last thread growth"]["passed"])

    def test_children_from_every_process_thread_are_counted_once(self):
        with TemporaryDirectory() as directory:
            proc = Path(directory)
            for task, children in {
                "100": "201 202\n",
                "101": "202 203\n",
                "102": "\n",
            }.items():
                task_dir = proc / "task" / task
                task_dir.mkdir(parents=True)
                (task_dir / "children").write_text(children, encoding="utf-8")

            self.assertEqual(process_children(proc), [201, 202, 203])

    def test_descriptor_classes_do_not_retain_targets(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            regular = root / "workspace-secret.txt"
            regular.write_text("content", encoding="utf-8")
            regular_fd = root / "regular-fd"
            regular_fd.symlink_to(regular)
            socket_fd = root / "socket-fd"
            socket_fd.symlink_to("socket:[123456]")
            event_fd = root / "event-fd"
            event_fd.symlink_to("anon_inode:[eventfd]")
            unknown_anon_fd = root / "unknown-anon-fd"
            unknown_anon_fd.symlink_to("anon_inode:[future-secret-kind]")
            device_fd = root / "device-fd"
            device_fd.symlink_to("/dev/null")
            memory_fd = root / "memory-fd"
            memory_fd.symlink_to("memfd:renderer-cache")

            self.assertEqual(classify_file_descriptor(regular_fd), "filesystem")
            self.assertEqual(classify_file_descriptor(socket_fd), "socket")
            self.assertEqual(classify_file_descriptor(event_fd), "anon-eventfd")
            self.assertEqual(classify_file_descriptor(unknown_anon_fd), "anon-other")
            self.assertEqual(classify_file_descriptor(device_fd), "device-path")
            self.assertEqual(classify_file_descriptor(memory_fd), "memory-file")
            self.assertNotIn("workspace-secret", classify_file_descriptor(regular_fd))

    def test_process_labels_are_sanitized_bounded_and_nonempty(self):
        self.assertEqual(bounded_process_label("worker\nname\x00"), "worker?name?")
        self.assertEqual(bounded_process_label("\n"), "<empty>")
        self.assertEqual(
            len(bounded_process_label("x" * (MAX_PROCESS_LABEL_CHARACTERS + 20))),
            MAX_PROCESS_LABEL_CHARACTERS,
        )

    def test_inventory_label_cardinality_is_bounded_and_deterministic(self):
        labels = [
            COLLAPSED_INVENTORY_LABEL,
            *[f"worker-{index:03}" for index in range(MAX_INVENTORY_LABELS + 20)],
        ]
        counts = counted_labels(list(reversed(labels)))
        self.assertEqual(len(counts), MAX_INVENTORY_LABELS)
        self.assertEqual(counts[COLLAPSED_INVENTORY_LABEL], 22)
        self.assertEqual(sum(counts.values()), len(labels))
        self.assertEqual(list(counts), sorted(counts))

    def test_inventory_coverage_rejects_missing_malformed_and_mismatched_counts(self):
        valid = sample(0)
        self.assertTrue(inventory_coverage_check([valid])["passed"])

        missing = sample(0)
        del missing["inventory"]
        malformed = sample(0)
        malformed["inventory"]["fd_kinds"] = {"filesystem": True}
        negative = sample(0)
        negative["inventory"]["fd_kinds"] = {"filesystem": -12}
        mismatch = sample(0)
        mismatch["inventory"]["thread_names"] = {"termeric": 3}
        oversized = sample(0)
        oversized["inventory"]["thread_names"] = {
            "x" * (MAX_PROCESS_LABEL_CHARACTERS + 1): 4
        }
        unsafe = sample(0)
        unsafe["inventory"]["thread_names"] = {"worker\nname": 4}
        path_like_fd_kind = sample(0)
        path_like_fd_kind["inventory"]["fd_kinds"] = {"workspace-secret": 12}

        for rejected in (
            missing,
            malformed,
            negative,
            mismatch,
            oversized,
            unsafe,
            path_like_fd_kind,
        ):
            self.assertFalse(inventory_coverage_check([rejected])["passed"])

    def test_inventory_summary_exposes_quartile_growth_and_peak_by_category(self):
        samples = [sample(index, fds=10 + index) for index in range(8)]
        for index, item in enumerate(samples):
            item["inventory"]["fd_kinds"] = {
                "pipe": 4,
                "filesystem": 6 + index,
            }
        summary = summarize_resource_inventories(samples)
        self.assertEqual(
            summary["fd_kinds"]["filesystem"],
            {
                "first_quartile_median": 6.5,
                "last_quartile_median": 12.5,
                "growth": 6.0,
                "peak": 13.0,
            },
        )
        self.assertEqual(summary["fd_kinds"]["pipe"]["growth"], 0.0)
        self.assertEqual(summary["child_names"], {})

    def test_inventory_summary_cardinality_remains_bounded_across_name_churn(self):
        samples = []
        for index in range(MAX_INVENTORY_LABELS + 20):
            item = sample(index, threads=1)
            item["inventory"]["thread_names"] = {f"worker-{index:03}": 1}
            samples.append(item)
        summary = summarize_resource_inventories(samples)["thread_names"]
        self.assertEqual(len(summary), MAX_INVENTORY_LABELS)
        self.assertIn(COLLAPSED_INVENTORY_LABEL, summary)
        self.assertEqual(summary[COLLAPSED_INVENTORY_LABEL]["peak"], 1.0)

    def test_bounded_and_drained_transcript_queue_passes(self):
        checks = evaluate_progress_ceilings(
            [
                {
                    "iteration": 1,
                    "transcript_queue_bytes": 2_048,
                    "transcript_queue_bytes_after_finish": 0,
                    "transcript_entries_accepted": 68,
                    "transcript_entries_dropped": 0,
                },
                {"complete": True, "iterations": 1},
            ],
            max_transcript_queue_bytes=4_096,
        )
        self.assertTrue(all(check["passed"] for check in checks))

    def test_deliberate_queue_backlog_and_failed_drain_are_rejected(self):
        checks = {
            check["name"]: check
            for check in evaluate_progress_ceilings(
                [
                    {
                        "iteration": 1,
                        "transcript_queue_bytes": 8_192,
                        "transcript_queue_bytes_after_finish": 512,
                        "transcript_entries_accepted": 67,
                        "transcript_entries_dropped": 3,
                    }
                ],
                max_transcript_queue_bytes=4_096,
            )
        }
        self.assertFalse(checks["maximum transcript queue backlog"]["passed"])
        self.assertFalse(
            checks["transcript queue drained after every iteration"]["passed"]
        )
        self.assertFalse(checks["transcript workload entries accepted"]["passed"])
        self.assertFalse(checks["transcript entries dropped"]["passed"])

    def test_missing_queue_metric_cannot_pass_as_zero_backlog(self):
        checks = {
            check["name"]: check
            for check in evaluate_progress_ceilings(
                [{"iteration": 1, "scrollback_bytes": 1_024}],
                max_transcript_queue_bytes=4_096,
            )
        }
        self.assertFalse(checks["transcript queue metric coverage"]["passed"])
        self.assertFalse(checks["maximum transcript queue backlog"]["passed"])

    def test_malformed_progress_is_retained_as_a_failing_record(self):
        with TemporaryDirectory() as directory:
            log = Path(directory) / "soak.log"
            log.write_text(
                'TERMERIC_SOAK not-json\nTERMERIC_SOAK ["not", "an", "object"]\n',
                encoding="utf-8",
            )
            progress = parse_progress(log)
        self.assertEqual(len(progress), 2)
        checks = {
            check["name"]: check
            for check in evaluate_progress_ceilings(
                progress,
                max_transcript_queue_bytes=4_096,
            )
        }
        self.assertFalse(checks["malformed scenario progress records"]["passed"])

    def test_outcome_checks_require_exactly_one_completion_marker(self):
        checks = {
            check["name"]: check
            for check in evaluate_outcome_checks(
                gpu_smoke_requested=False,
                gpu_exit_status=None,
                scenario_exit_status=0,
                completion_markers=2,
                remaining_observed_children=0,
                gpu_error_count=0,
            )
        }
        self.assertFalse(checks["scenario emitted completion marker"]["passed"])


class CoreRunnerMainTests(unittest.TestCase):
    """Drive run_soak.main() with the test binary and /proc sampling mocked."""

    PROGRESS = [
        {"iteration": 1, "scrollback_bytes": 1024, "image_bytes": 4096,
         "transcript_queue_bytes": 2048, "transcript_queue_bytes_after_finish": 0,
         "transcript_entries_accepted": 68, "transcript_entries_dropped": 0,
         "diff_completed": 80, "diff_evicted": 72, "diff_retained": 8,
         "diff_history_peak_count": 64, "diff_history_peak_bytes": 3_900_000},
        {"complete": True, "iterations": 1},
    ]

    def invoke(self, mode="pass", *, gpu=False):
        state = {"launches": []}
        progress = self.PROGRESS

        class FakeProcess:
            pid = 12345
            returncode = 0

            def __init__(self, command, **kwargs):
                self.polls = 0
                state["launches"].append("scenario")
                if mode == "launch-failure":
                    raise OSError("injected launch failure")
                for record in progress:
                    kwargs["stdout"].write("TERMERIC_SOAK " + json.dumps(record) + "\n")
                kwargs["stdout"].flush()

            def poll(self):
                self.polls += 1
                if mode == "timeout":
                    return self.returncode if state.get("killed") else None
                return None if self.polls <= (3 if mode == "transient-sample" else 2) else 0

            def kill(self):
                state["killed"] = True
                self.returncode = -9

            def wait(self, **kwargs):
                return self.returncode

        def fake_command(command, **kwargs):
            if command[1:2] != ["test_support::linux_headless_gpu_render_smoke_when_requested"]:
                return subprocess.CompletedProcess(command, 0, stdout="fixture\n")
            state["launches"].append("gpu-smoke")
            if mode == "gpu-timeout":
                raise subprocess.TimeoutExpired(command, 60)
            return subprocess.CompletedProcess(command, 0, stdout="")

        snapshots = (
            KeyboardInterrupt() if mode == "interrupt"
            else ([FileNotFoundError()] if mode == "transient-sample" else [])
            + [sample(1), sample(2)]
        )
        with TemporaryDirectory() as temporary:
            report_path = Path(temporary) / "results.json"
            log_path = Path(temporary) / "scenario.log"
            argv = ["run_soak.py", "--duration-seconds", "1", "--sample-interval", "0.5",
                    "--output", str(report_path), "--log", str(log_path),
                    *(["--gpu-smoke"] if gpu else [])]
            with (
                patch.object(run_soak, "build_test_binary", return_value=Path("/fixture/test-binary")),
                patch.object(run_soak.subprocess, "Popen", FakeProcess),
                patch.object(run_soak.subprocess, "run", side_effect=fake_command),
                patch.object(run_soak, "process_snapshot", side_effect=snapshots) as snapshot,
                patch.object(run_soak.time, "sleep"),
                (patch.object(run_soak.time, "monotonic", side_effect=[0.0, 100.0])
                 if mode == "timeout" else contextlib.nullcontext()),
                patch.object(sys, "argv", argv),
                contextlib.redirect_stdout(io.StringIO()),
            ):
                try:
                    run_soak.main()
                    exited = False
                except SystemExit:
                    exited = True
            state["snapshot_calls"] = snapshot.call_count
            return json.loads(report_path.read_text()), state, exited

    def test_clean_run_passes_and_writes_the_report(self):
        report, state, exited = self.invoke(gpu=True)
        self.assertFalse(exited)
        self.assertEqual(report["status"], "pass")
        self.assertEqual(state["launches"], ["gpu-smoke", "scenario"])

    def test_launch_failure_interruption_and_timeout_fail_the_run(self):
        for mode in ("launch-failure", "interrupt", "timeout"):
            with self.subTest(mode=mode):
                report, state, exited = self.invoke(mode)
                self.assertTrue(exited)
                self.assertEqual(report["status"], "fail")
                if mode in {"interrupt", "timeout"}:
                    self.assertTrue(state["killed"])
                if mode == "timeout":
                    # The deadline passes before the first sample; the scenario is killed for it.
                    self.assertEqual(state["snapshot_calls"], 0)

    def test_gpu_timeout_does_not_launch_the_scenario(self):
        report, state, exited = self.invoke("gpu-timeout", gpu=True)
        self.assertTrue(exited)
        self.assertEqual(report["status"], "fail")
        self.assertEqual(state["launches"], ["gpu-smoke"])

    def test_transient_proc_sample_does_not_kill_the_scenario(self):
        report, state, exited = self.invoke("transient-sample")
        self.assertFalse(exited)
        self.assertEqual(report["status"], "pass")
        self.assertNotIn("killed", state)


if __name__ == "__main__":
    unittest.main()
