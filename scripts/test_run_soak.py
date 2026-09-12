import unittest
import hashlib
from pathlib import Path
from tempfile import TemporaryDirectory

from scripts.run_soak import (
    COLLAPSED_INVENTORY_LABEL,
    CORE_SOAK_SCHEMA_VERSION,
    MAX_INVENTORY_LABELS,
    MAX_PROCESS_LABEL_CHARACTERS,
    bounded_process_label,
    classify_file_descriptor,
    counted_labels,
    evaluate_progress_ceilings,
    evaluate_resource_ceilings,
    file_sha256,
    inventory_coverage_check,
    minimum_sample_count,
    parse_progress,
    process_children,
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
    def test_report_schema_is_versioned_for_resource_inventories(self):
        self.assertEqual(CORE_SOAK_SCHEMA_VERSION, 2)

    def test_binary_digest_is_exact_and_content_bound(self):
        with TemporaryDirectory() as directory:
            binary = Path(directory) / "fixture"
            binary.write_bytes(b"termeric-soak-fixture")
            self.assertEqual(
                file_sha256(binary),
                hashlib.sha256(b"termeric-soak-fixture").hexdigest(),
            )

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


if __name__ == "__main__":
    unittest.main()
