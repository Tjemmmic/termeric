from __future__ import annotations

import copy
import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

from scripts.run_soak import (
    CORE_SOAK_RECORD_KIND,
    CORE_SOAK_SCHEMA_VERSION,
    MAX_INVENTORY_LABELS,
    MAX_PROCESS_LABEL_CHARACTERS,
    TRANSCRIPT_ENTRIES_PER_ITERATION,
    build_core_soak_checks,
    file_sha256,
    summarize_resource_inventories,
)
from scripts.validate_soak import SoakValidationError, validate_record


COMMIT = "a" * 40


def resource_sample(index: int) -> dict[str, object]:
    return {
        "elapsed_seconds": float(index + 1),
        "rss_kib": 100_000.0,
        "threads": 4,
        "fds": 12,
        "children": [],
        "cpu_ticks": 10 + index,
        "inventory": {
            "fd_kinds": {"filesystem": 12},
            "thread_names": {"termeric": 4},
            "child_names": {},
        },
    }


class CoreSoakRecordValidationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = TemporaryDirectory()
        self.source = Path(self.temporary.name) / "source"
        self.bundle = self.source / "evidence"
        self.binary = self.source / "target" / "debug" / "deps" / "termeric-test"
        self.log = self.bundle / "scenario.log"
        self.record_path = self.bundle / "results.json"
        self.binary.parent.mkdir(parents=True)
        self.bundle.mkdir()
        self.binary.write_bytes(b"test-binary")
        self.progress = [
            {
                "iteration": 1,
                "scrollback_bytes": 1024,
                "image_bytes": 4096,
                "transcript_queue_bytes": 2048,
                "transcript_queue_bytes_after_finish": 0,
                "transcript_entries_accepted": TRANSCRIPT_ENTRIES_PER_ITERATION,
                "transcript_entries_dropped": 0,
            },
            {"complete": True, "iterations": 1},
        ]
        self.log.write_text(
            "running soak\n"
            + "\n".join(
                f"TERMERIC_SOAK {json.dumps(item, separators=(',', ':'))}"
                for item in self.progress
            )
            + "\n",
            encoding="utf-8",
        )
        samples = [resource_sample(index) for index in range(4)]
        thresholds = {
            "max_rss_kib": 500_000.0,
            "max_rss_growth_kib": 10_000.0,
            "max_thread_growth": 2.0,
            "max_fd_growth": 4.0,
            "max_children": 2,
            "max_transcript_queue_bytes": 4096,
            "minimum_resource_samples": 2,
            "transcript_entries_per_iteration": TRANSCRIPT_ENTRIES_PER_ITERATION,
            "max_inventory_labels_per_dimension": MAX_INVENTORY_LABELS,
            "max_process_label_characters": MAX_PROCESS_LABEL_CHARACTERS,
        }
        outcomes = {
            "gpu_exit_status": None,
            "scenario_exit_status": 0,
            "completion_markers": 1,
            "remaining_observed_children": 0,
            "gpu_error_count": 0,
        }
        checks = build_core_soak_checks(
            samples,
            self.progress,
            thresholds,
            outcomes,
            gpu_smoke_requested=False,
        )
        self.record = {
            "schema_version": CORE_SOAK_SCHEMA_VERSION,
            "kind": CORE_SOAK_RECORD_KIND,
            "status": "pass",
            "candidate_eligible": True,
            "eligibility": "candidate",
            "recorded_at_utc": "2026-09-11T12:00:00+00:00",
            "environment": {
                "platform": "Linux-test",
                "python": "3.13.0",
                "rustc": "rustc 1.90.0",
                "commit": COMMIT,
                "worktree_dirty": False,
                "wgpu_backend": "vulkan",
            },
            "duration_seconds": 4,
            "sample_interval_seconds": 1.0,
            "minimum_resource_samples": 2,
            "maximum_transcript_queue_bytes": 4096,
            "gpu_smoke_requested": False,
            "test_binary": {
                "path": "target/debug/deps/termeric-test",
                "bytes": self.binary.stat().st_size,
                "sha256": file_sha256(self.binary),
            },
            "artifacts": {
                "scenario_log": {
                    "path": "scenario.log",
                    "bytes": self.log.stat().st_size,
                    "sha256": file_sha256(self.log),
                }
            },
            "thresholds": thresholds,
            "outcomes": outcomes,
            "samples": samples,
            "progress": self.progress,
            "resource_inventory_summary": summarize_resource_inventories(samples),
            "checks": checks,
        }
        self.write_record()

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def write_record(self) -> None:
        self.record_path.write_text(
            json.dumps(self.record, indent=2) + "\n", encoding="utf-8"
        )

    def validate(self, *, clean: bool = True, allow_ineligible: bool = False) -> dict:
        with patch(
            "scripts.validate_soak.git_identity", return_value=(COMMIT, clean)
        ):
            return validate_record(
                self.record_path,
                self.source,
                allow_ineligible=allow_ineligible,
            )

    def assert_mutation_rejected(self, mutate) -> None:
        mutate(self.record)
        self.write_record()
        with self.assertRaises(SoakValidationError):
            self.validate()

    def test_authentic_clean_record_validates(self) -> None:
        validated = self.validate()
        self.assertEqual(validated["status"], "pass")

    def test_unknown_field_is_rejected(self) -> None:
        self.assert_mutation_rejected(lambda record: record.update({"surprise": True}))

    def test_changed_sample_is_rejected(self) -> None:
        self.assert_mutation_rejected(
            lambda record: record["samples"][0].update({"rss_kib": 900_000.0})
        )

    def test_changed_threshold_is_rejected(self) -> None:
        self.assert_mutation_rejected(
            lambda record: record["thresholds"].update({"max_rss_kib": 99_000.0})
        )

    def test_changed_outcome_is_rejected(self) -> None:
        self.assert_mutation_rejected(
            lambda record: record["outcomes"].update({"scenario_exit_status": 1})
        )

    def test_changed_reported_check_is_rejected(self) -> None:
        self.assert_mutation_rejected(
            lambda record: record["checks"][0].update({"passed": False})
        )

    def test_changed_inventory_summary_is_rejected(self) -> None:
        self.assert_mutation_rejected(
            lambda record: record["resource_inventory_summary"].update({"fd_kinds": {}})
        )

    def test_changed_binary_is_rejected(self) -> None:
        self.binary.write_bytes(b"changed-binary")
        with self.assertRaises(SoakValidationError):
            self.validate()

    def test_changed_log_is_rejected(self) -> None:
        self.log.write_text("changed\n", encoding="utf-8")
        with self.assertRaises(SoakValidationError):
            self.validate()

    def test_unsafe_and_symlink_paths_are_rejected(self) -> None:
        unsafe = copy.deepcopy(self.record)
        unsafe["artifacts"]["scenario_log"]["path"] = "../scenario.log"
        self.record = unsafe
        self.write_record()
        with self.assertRaises(SoakValidationError):
            self.validate()

        self.record = copy.deepcopy(self.record)
        self.record["artifacts"]["scenario_log"]["path"] = "scenario-link.log"
        (self.bundle / "scenario-link.log").symlink_to(self.log)
        self.write_record()
        with self.assertRaises(SoakValidationError):
            self.validate()

    def test_non_finite_and_boolean_numbers_are_rejected(self) -> None:
        non_finite = copy.deepcopy(self.record)
        non_finite["samples"][0]["rss_kib"] = float("nan")
        self.record = non_finite
        self.write_record()
        with self.assertRaises(SoakValidationError):
            self.validate()

        self.record = copy.deepcopy(self.record)
        self.record["samples"][0]["rss_kib"] = True
        self.write_record()
        with self.assertRaises(SoakValidationError):
            self.validate()

    def test_dirty_record_is_only_accepted_as_ineligible_diagnostic(self) -> None:
        self.record["environment"]["worktree_dirty"] = True
        self.record["candidate_eligible"] = False
        self.record["eligibility"] = "dirty-dry-run"
        self.write_record()
        with self.assertRaises(SoakValidationError):
            self.validate(clean=False)
        self.assertEqual(
            self.validate(clean=False, allow_ineligible=True)["eligibility"],
            "dirty-dry-run",
        )

    def test_consistent_failed_record_is_only_accepted_as_diagnostic(self) -> None:
        self.progress = self.progress[:-1]
        self.log.write_text(
            "running soak\n"
            + "\n".join(
                f"TERMERIC_SOAK {json.dumps(item, separators=(',', ':'))}"
                for item in self.progress
            )
            + "\n",
            encoding="utf-8",
        )
        self.record["progress"] = self.progress
        self.record["artifacts"]["scenario_log"] = {
            "path": "scenario.log",
            "bytes": self.log.stat().st_size,
            "sha256": file_sha256(self.log),
        }
        self.record["outcomes"]["scenario_exit_status"] = 1
        self.record["outcomes"]["completion_markers"] = 0
        self.record["checks"] = build_core_soak_checks(
            self.record["samples"],
            self.progress,
            self.record["thresholds"],
            self.record["outcomes"],
            gpu_smoke_requested=False,
        )
        self.record["status"] = "fail"
        self.record["candidate_eligible"] = False
        self.record["eligibility"] = "failed"
        self.write_record()
        with self.assertRaises(SoakValidationError):
            self.validate()
        self.assertEqual(
            self.validate(allow_ineligible=True)["eligibility"], "failed"
        )

    def test_wrong_source_commit_is_rejected(self) -> None:
        with patch(
            "scripts.validate_soak.git_identity", return_value=("b" * 40, True)
        ):
            with self.assertRaises(SoakValidationError):
                validate_record(self.record_path, self.source)


if __name__ == "__main__":
    unittest.main()
