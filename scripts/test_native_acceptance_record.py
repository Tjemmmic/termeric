from copy import deepcopy
import json
from pathlib import Path
import tempfile
import unittest

from scripts.native_acceptance_record import (
    CASE_DEFINITIONS,
    create_template,
    disposition_for_checks,
    file_sha256,
    seal_artifacts,
    validate_record,
)


CANDIDATE_SHA = "a" * 40


def completed_record() -> dict[str, object]:
    record = create_template(CANDIDATE_SHA, "Release Operator")
    cases = record["cases"]
    assert isinstance(cases, list)
    for case in cases:
        assert isinstance(case, dict)
        case["disposition"] = "pass"
        environment = case["environment"]
        assert isinstance(environment, dict)
        for field, value in environment.items():
            if value == "RECORD_ME":
                environment[field] = f"recorded-{field}"
        environment["termeric_binary_sha256"] = "b" * 64
        checks = case["checks"]
        assert isinstance(checks, dict)
        for check in checks:
            checks[check] = "pass"
        case["artifacts"] = [
            {
                "path": f"artifacts/{case['id']}.txt",
                "description": "native acceptance notes",
                "sha256": "0" * 64,
            }
        ]
    return record


class NativeAcceptanceRecordTests(unittest.TestCase):
    def test_template_has_every_exact_case_and_check(self) -> None:
        record = create_template(CANDIDATE_SHA, "Operator")
        cases = record["cases"]
        assert isinstance(cases, list)
        by_id = {case["id"]: case for case in cases if isinstance(case, dict)}
        self.assertEqual(set(by_id), set(CASE_DEFINITIONS))
        for case_id, expected_checks in CASE_DEFINITIONS.items():
            self.assertEqual(set(by_id[case_id]["checks"]), set(expected_checks))
            self.assertEqual(by_id[case_id]["disposition"], "unrecorded")
        self.assertIn(
            "surface_outdated_recovery",
            by_id["x11.vendor_gpu_recovery"]["checks"],
        )
        self.assertIn(
            "surface_outdated_recovery",
            by_id["wayland.vendor_gpu_recovery"]["checks"],
        )

    def test_incomplete_template_is_accepted_only_when_requested(self) -> None:
        record = create_template(CANDIDATE_SHA, "Operator")
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "record.json"
            failures = validate_record(path, record)
            self.assertTrue(any("remains unrecorded" in failure for failure in failures))
            self.assertEqual(
                validate_record(
                    path,
                    record,
                    allow_incomplete=True,
                    require_artifact_files=False,
                ),
                [],
            )

    def test_complete_structure_passes_without_local_artifact_resolution(self) -> None:
        record = completed_record()
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "record.json"
            self.assertEqual(
                validate_record(path, record, require_artifact_files=False), []
            )

    def test_missing_case_and_unknown_check_fail_closed(self) -> None:
        record = completed_record()
        cases = record["cases"]
        assert isinstance(cases, list)
        removed = cases.pop()
        first = cases[0]
        assert isinstance(first, dict)
        checks = first["checks"]
        assert isinstance(checks, dict)
        checks["invented_check"] = "pass"
        with tempfile.TemporaryDirectory() as directory:
            failures = validate_record(
                Path(directory) / "record.json",
                record,
                require_artifact_files=False,
            )
        self.assertTrue(any(f"missing required case {removed['id']}" in item for item in failures))
        self.assertTrue(any("unknown check invented_check" in item for item in failures))

    def test_disposition_must_agree_with_check_results(self) -> None:
        record = completed_record()
        cases = record["cases"]
        assert isinstance(cases, list)
        first = cases[0]
        assert isinstance(first, dict)
        checks = first["checks"]
        assert isinstance(checks, dict)
        checks[next(iter(checks))] = "fail"
        with tempfile.TemporaryDirectory() as directory:
            failures = validate_record(
                Path(directory) / "record.json",
                record,
                require_artifact_files=False,
            )
        self.assertTrue(any("disagrees with checks ('fail')" in item for item in failures))
        self.assertEqual(disposition_for_checks(checks), "fail")

    def test_non_pass_disposition_requires_notes(self) -> None:
        record = completed_record()
        cases = record["cases"]
        assert isinstance(cases, list)
        first = cases[0]
        assert isinstance(first, dict)
        first["disposition"] = "unavailable"
        checks = first["checks"]
        assert isinstance(checks, dict)
        for check in checks:
            checks[check] = "unavailable"
        first["artifacts"] = []
        first["notes"] = ""
        with tempfile.TemporaryDirectory() as directory:
            failures = validate_record(
                Path(directory) / "record.json",
                record,
                require_artifact_files=False,
            )
        self.assertTrue(any("requires explanatory notes" in item for item in failures))

    def test_artifact_path_traversal_is_rejected(self) -> None:
        record = completed_record()
        cases = record["cases"]
        assert isinstance(cases, list)
        first = cases[0]
        assert isinstance(first, dict)
        artifacts = first["artifacts"]
        assert isinstance(artifacts, list)
        artifact = artifacts[0]
        assert isinstance(artifact, dict)
        artifact["path"] = "../outside.txt"
        with tempfile.TemporaryDirectory() as directory:
            failures = validate_record(
                Path(directory) / "record.json",
                record,
                require_artifact_files=False,
            )
        self.assertTrue(any("escapes the record directory" in item for item in failures))

    def test_seal_hashes_artifacts_and_digest_validation_detects_changes(self) -> None:
        record = completed_record()
        cases = record["cases"]
        assert isinstance(cases, list)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "record.json"
            for case in cases:
                assert isinstance(case, dict)
                artifacts = case["artifacts"]
                assert isinstance(artifacts, list)
                artifact = artifacts[0]
                assert isinstance(artifact, dict)
                evidence = root / str(artifact["path"])
                evidence.parent.mkdir(parents=True, exist_ok=True)
                evidence.write_text(f"evidence for {case['id']}\n")
            path.write_text(json.dumps(record))
            self.assertEqual(seal_artifacts(path, record), len(CASE_DEFINITIONS))
            sealed = json.loads(path.read_text())
            self.assertEqual(validate_record(path, sealed), [])

            first = sealed["cases"][0]["artifacts"][0]
            evidence = root / first["path"]
            self.assertEqual(first["sha256"], file_sha256(evidence))
            evidence.write_text("tampered\n")
            failures = validate_record(path, sealed)
            self.assertTrue(any("digest mismatch" in item for item in failures))

    def test_require_pass_rejects_an_explicit_accepted_limitation(self) -> None:
        modified = deepcopy(completed_record())
        cases = modified["cases"]
        assert isinstance(cases, list)
        first = cases[0]
        assert isinstance(first, dict)
        first["disposition"] = "accepted_limitation"
        first["notes"] = "Accepted for this candidate"
        checks = first["checks"]
        assert isinstance(checks, dict)
        for check in checks:
            checks[check] = "accepted_limitation"
        with tempfile.TemporaryDirectory() as directory:
            failures = validate_record(
                Path(directory) / "record.json",
                modified,
                require_pass=True,
                require_artifact_files=False,
            )
        self.assertTrue(any("release-ready validation requires pass" in item for item in failures))

    def test_invalid_timestamp_binary_digest_and_tool_identity_fail(self) -> None:
        record = completed_record()
        candidate = record["candidate"]
        assert isinstance(candidate, dict)
        candidate["recorded_at_utc"] = "yesterday"
        cases = record["cases"]
        assert isinstance(cases, list)
        first = cases[0]
        assert isinstance(first, dict)
        environment = first["environment"]
        assert isinstance(environment, dict)
        environment["termeric_binary_sha256"] = "not-a-digest"
        ibus = next(
            case
            for case in cases
            if isinstance(case, dict) and case.get("id") == "x11.ibus_ime"
        )
        ibus_environment = ibus["environment"]
        assert isinstance(ibus_environment, dict)
        ibus_environment["input_method"] = "Different IME"
        with tempfile.TemporaryDirectory() as directory:
            failures = validate_record(
                Path(directory) / "record.json",
                record,
                require_artifact_files=False,
            )
        self.assertTrue(any("ISO-8601" in item for item in failures))
        self.assertTrue(any("binary_sha256" in item for item in failures))
        self.assertTrue(any("must be 'IBus'" in item for item in failures))


if __name__ == "__main__":
    unittest.main()
