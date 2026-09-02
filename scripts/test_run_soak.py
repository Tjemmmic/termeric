import unittest
import hashlib
from pathlib import Path
from tempfile import TemporaryDirectory

from scripts.run_soak import (
    evaluate_progress_ceilings,
    evaluate_resource_ceilings,
    file_sha256,
    minimum_sample_count,
    parse_progress,
    process_children,
)


def sample(index: int, *, rss: float = 100_000, threads: int = 4, fds: int = 12) -> dict:
    return {
        "elapsed_seconds": float(index),
        "rss_kib": rss,
        "threads": threads,
        "fds": fds,
        "children": [],
        "cpu_ticks": index,
    }


class SoakCeilingTests(unittest.TestCase):
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
