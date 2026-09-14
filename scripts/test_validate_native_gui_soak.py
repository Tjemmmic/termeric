from __future__ import annotations

import base64
import copy
import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

from scripts.drm_memory import aggregate_drm_fdinfos, summarize_drm_memory

from scripts.run_native_gui_soak import (
    MAX_NATIVE_SOAK_ARTIFACTS,
    MAX_NATIVE_SOAK_ARTIFACT_BYTES,
    MAX_NATIVE_SOAK_TOTAL_ARTIFACT_BYTES,
    NATIVE_SOAK_RECORD_KIND,
    NATIVE_SOAK_SCHEMA_VERSION,
    build_native_soak_checks,
    native_artifact_manifest,
)
from scripts.run_soak import (
    MAX_INVENTORY_LABELS,
    MAX_PROCESS_LABEL_CHARACTERS,
    file_sha256,
    summarize_resource_inventories,
)
from scripts.validate_native_gui_soak import SoakValidationError, validate_record
from scripts.soak_provenance import provenance_checks
from scripts.test_validate_soak import provenance_fixture, mark_provenance_dirty


COMMIT = "c" * 40
KITTY_ACK = b"\x1b_Gi=91;OK\x1b\\"


def resource_sample(index: int) -> dict[str, object]:
    return {
        "elapsed_seconds": float(index + 1),
        "rss_kib": 200_000.0,
        "threads": 8,
        "fds": 30,
        "children": [100],
        "cpu_ticks": 20 + index,
        "inventory": {
            "fd_kinds": {"socket": 30},
            "thread_names": {"termeric": 8},
            "child_names": {"fixture": 1},
        },
        "gpu_memory": aggregate_drm_fdinfos([]),
    }


class NativeSoakRecordValidationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = TemporaryDirectory()
        self.source = Path(self.temporary.name) / "source"
        self.bundle = self.source / "evidence"
        self.binary = self.source / "target" / "debug" / "termeric"
        self.record_path = self.bundle / "results.json"
        self.binary.parent.mkdir(parents=True)
        self.bundle.mkdir()
        self.binary.write_bytes(b"native-test-binary")
        application = self.bundle / "application"
        application.mkdir()
        (application / "termeric.log").write_text(
            "ordinary native renderer output\n", encoding="utf-8"
        )
        (application / "termeric.stdout").write_text("", encoding="utf-8")
        records = [
            {"event": "started", "pid": 100},
            {"event": "soak_progress", "iteration": 1},
            {"event": "soak_progress", "iteration": 2},
            {
                "event": "input",
                "base64": base64.b64encode(KITTY_ACK).decode("ascii"),
            },
        ]
        (self.bundle / "pty-transcript.jsonl").write_text(
            "\n".join(json.dumps(record) for record in records) + "\n",
            encoding="utf-8",
        )
        (self.bundle / "sway.log").write_text("sway exited cleanly\n", encoding="utf-8")
        (self.bundle / "sway-config").write_text("output HEADLESS-1\n", encoding="utf-8")
        terminfo = self.bundle / "terminfo" / "x"
        terminfo.mkdir(parents=True)
        (terminfo / "xterm-termeric").write_bytes(b"compiled-terminfo")

        samples = [resource_sample(index) for index in range(6)]
        thresholds = {
            "max_rss_kib": 786_432.0,
            "max_rss_growth_kib": 196_608.0,
            "max_thread_growth": 8.0,
            "max_fd_growth": 16.0,
            "max_children": 8,
            "require_gpu_memory": False,
            "max_drm_resident_bytes": 1024 * 1024 * 1024,
            "max_drm_resident_growth_bytes": 256 * 1024 * 1024,
            "minimum_resource_samples": 3,
            "minimum_progress_records": 2,
            "minimum_resize_actions": 2,
            "minimum_pane_actions": 2,
            "minimum_graphics_acknowledgements": 1,
            "max_inventory_labels_per_dimension": MAX_INVENTORY_LABELS,
            "max_process_label_characters": MAX_PROCESS_LABEL_CHARACTERS,
        }
        outcomes = {
            "exit_status": 0,
            "progress_records": 2,
            "resize_actions": 2,
            "pane_actions": 2,
            "graphics_acknowledgements": 1,
            "remaining_children": 0,
            "runtime_errors": 0,
            "driver_error": None,
        }
        provenance = provenance_fixture(self.source, self.binary, COMMIT, ["displayed"])
        checks = build_native_soak_checks(samples, thresholds, outcomes)
        checks.extend(provenance_checks(provenance, ["displayed"]))
        self.record = {
            "schema": NATIVE_SOAK_SCHEMA_VERSION,
            "kind": NATIVE_SOAK_RECORD_KIND,
            "status": "pass",
            "candidate_eligible": True,
            "eligibility": "candidate",
            "recorded_at_utc": "2026-09-11T15:00:00+00:00",
            "environment": {
                "platform": "Linux-test",
                "python": "3.13.0",
                "commit": COMMIT,
                "worktree_dirty": False,
                "backend": "wayland",
                "compositor": "sway-headless",
                "wgpu_backend": "gl",
                "gpu_memory_interface": "linux-drm-fdinfo-v1",
            },
            "provenance": provenance,
            "binary": {
                "path": "target/debug/termeric",
                "bytes": self.binary.stat().st_size,
                "sha256": file_sha256(self.binary),
            },
            "artifact_policy": {
                "max_files": MAX_NATIVE_SOAK_ARTIFACTS,
                "max_file_bytes": MAX_NATIVE_SOAK_ARTIFACT_BYTES,
                "max_total_bytes": MAX_NATIVE_SOAK_TOTAL_ARTIFACT_BYTES,
            },
            "artifacts": native_artifact_manifest(self.bundle),
            "duration_seconds": 6,
            "sample_interval_seconds": 1.0,
            "action_interval_seconds": 1.0,
            "thresholds": thresholds,
            "outcomes": outcomes,
            "samples": samples,
            "resource_inventory_summary": summarize_resource_inventories(samples),
            "drm_memory_summary": summarize_drm_memory(samples),
            "observed_children": [100],
            "checks": checks,
        }
        self.write_record()

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def write_record(self) -> None:
        self.record_path.write_text(
            json.dumps(self.record, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )

    def validate(self, *, clean: bool = True, allow_ineligible: bool = False) -> dict:
        with patch(
            "scripts.validate_native_gui_soak.git_identity",
            return_value=(COMMIT, clean),
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

    def test_authentic_clean_bundle_validates(self) -> None:
        self.assertEqual(self.validate()["status"], "pass")

    def test_missing_tampered_or_old_launch_provenance_is_rejected(self):
        original = copy.deepcopy(self.record)
        mutations = [
            lambda record: record.pop("provenance"),
            lambda record: record.update(schema=5),
            lambda record: record.update(schema=6.0),
            lambda record: record["provenance"].update(pinned_final=None),
            lambda record: record["provenance"]["source_final"].update(commit="d" * 40),
            lambda record: record["provenance"]["binary_initial"].update(sha256="0" * 64),
            lambda record: record["provenance"]["launches"].clear(),
            lambda record: record["provenance"]["launches"][0].update(role="scenario"),
        ]
        for mutation in mutations:
            self.record = copy.deepcopy(original)
            self.assert_mutation_rejected(mutation)

    def test_missing_final_identity_is_diagnostic_not_a_successful_candidate(self):
        self.record["provenance"]["binary_final"] = None
        self.record["checks"] = build_native_soak_checks(
            self.record["samples"], self.record["thresholds"], self.record["outcomes"],
        ) + provenance_checks(self.record["provenance"], ["displayed"])
        self.write_record()
        with self.assertRaises(SoakValidationError):
            self.validate(allow_ineligible=True)
        self.record.update(status="fail", candidate_eligible=False, eligibility="failed")
        self.write_record()
        with self.assertRaises(SoakValidationError):
            self.validate()
        self.assertFalse(self.validate(allow_ineligible=True)["candidate_eligible"])

    def test_required_measured_drm_memory_bundle_validates(self) -> None:
        for index, sample in enumerate(self.record["samples"]):
            sample["gpu_memory"] = aggregate_drm_fdinfos(
                [
                    "drm-driver: amdgpu\n"
                    "drm-pdev: 0000:03:00.0\n"
                    "drm-client-id: 7\n"
                    f"drm-total-vram: {256 + index} MiB\n"
                    f"drm-resident-vram: {128 + index} MiB\n"
                ]
            )
        self.record["thresholds"]["require_gpu_memory"] = True
        self.record["drm_memory_summary"] = summarize_drm_memory(
            self.record["samples"]
        )
        self.record["checks"] = build_native_soak_checks(
            self.record["samples"],
            self.record["thresholds"],
            self.record["outcomes"],
        )
        self.record["checks"].extend(provenance_checks(self.record["provenance"], ["displayed"]))
        self.write_record()
        self.assertEqual(self.validate()["status"], "pass")

    def test_unknown_field_and_policy_drift_are_rejected(self) -> None:
        original = copy.deepcopy(self.record)
        self.assert_mutation_rejected(lambda record: record.update({"surprise": True}))

        self.record = original
        self.assert_mutation_rejected(
            lambda record: record["artifact_policy"].update({"max_files": 1})
        )

    def test_changed_sample_threshold_outcome_check_and_summary_are_rejected(self) -> None:
        mutations = [
            lambda record: record["samples"][0].update({"rss_kib": 900_000.0}),
            lambda record: record["thresholds"].update({"max_rss_kib": 1.0}),
            lambda record: record["thresholds"].update(
                {"require_gpu_memory": "yes"}
            ),
            lambda record: record["thresholds"].update(
                {"minimum_progress_records": 3}
            ),
            lambda record: record["outcomes"].update({"resize_actions": 0}),
            lambda record: record["outcomes"].update({"progress_records": 3}),
            lambda record: record["outcomes"].update({"runtime_errors": 1}),
            lambda record: record["checks"][0].update({"passed": False}),
            lambda record: record["resource_inventory_summary"].update(
                {"fd_kinds": {}}
            ),
            lambda record: record["drm_memory_summary"].update(
                {"measured_samples": 1}
            ),
            lambda record: record["samples"][0]["gpu_memory"].update(
                {"client_count": 1}
            ),
        ]
        for mutation in mutations:
            with self.subTest(mutation=mutation):
                original = copy.deepcopy(self.record)
                mutation(self.record)
                self.write_record()
                with self.assertRaises(SoakValidationError):
                    self.validate()
                self.record = original

    def test_changed_binary_and_artifact_bytes_are_rejected(self) -> None:
        self.binary.write_bytes(b"changed")
        with self.assertRaises(SoakValidationError):
            self.validate()

        self.binary.write_bytes(b"native-test-binary")
        (self.bundle / "sway.log").write_text("changed\n", encoding="utf-8")
        with self.assertRaises(SoakValidationError):
            self.validate()

    def test_manifest_removal_unsafe_path_and_stale_extra_file_are_rejected(self) -> None:
        original = copy.deepcopy(self.record)
        self.record["artifacts"] = self.record["artifacts"][1:]
        self.write_record()
        with self.assertRaises(SoakValidationError):
            self.validate()

        self.record = copy.deepcopy(original)
        self.record["artifacts"][0]["path"] = "../escaped"
        self.write_record()
        with self.assertRaises(SoakValidationError):
            self.validate()

        self.record = copy.deepcopy(original)
        self.record["artifacts"].insert(1, copy.deepcopy(self.record["artifacts"][0]))
        self.write_record()
        with self.assertRaises(SoakValidationError):
            self.validate()

        self.record = original
        (self.bundle / "stale-extra.txt").write_text("stale", encoding="utf-8")
        self.write_record()
        with self.assertRaises(SoakValidationError):
            self.validate()

    def test_passing_bundle_requires_canonical_runtime_artifacts(self) -> None:
        (self.bundle / "sway-config").unlink()
        self.record["artifacts"] = native_artifact_manifest(self.bundle)
        self.write_record()
        with self.assertRaisesRegex(SoakValidationError, "required artifacts"):
            self.validate()

    def test_non_finite_and_boolean_numbers_are_rejected(self) -> None:
        self.record["samples"][0]["rss_kib"] = float("inf")
        self.write_record()
        with self.assertRaises(SoakValidationError):
            self.validate()

        self.record["samples"][0]["rss_kib"] = True
        self.write_record()
        with self.assertRaises(SoakValidationError):
            self.validate()

    def test_dirty_bundle_is_only_accepted_as_diagnostic(self) -> None:
        mark_provenance_dirty(self.record["provenance"])
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

    def test_consistent_failed_bundle_is_only_accepted_as_diagnostic(self) -> None:
        self.record["outcomes"]["exit_status"] = 1
        self.record["outcomes"]["driver_error"] = "native close failed"
        self.record["checks"] = build_native_soak_checks(
            self.record["samples"],
            self.record["thresholds"],
            self.record["outcomes"],
        )
        self.record["checks"].extend(provenance_checks(self.record["provenance"], ["displayed"]))
        self.record["status"] = "fail"
        self.record["candidate_eligible"] = False
        self.record["eligibility"] = "failed"
        self.write_record()
        with self.assertRaises(SoakValidationError):
            self.validate()
        self.assertEqual(
            self.validate(allow_ineligible=True)["eligibility"], "failed"
        )

    def test_wrong_source_identity_is_rejected(self) -> None:
        with patch(
            "scripts.validate_native_gui_soak.git_identity",
            return_value=("d" * 40, True),
        ):
            with self.assertRaises(SoakValidationError):
                validate_record(self.record_path, self.source)


if __name__ == "__main__":
    unittest.main()
