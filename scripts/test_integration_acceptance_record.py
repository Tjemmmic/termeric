from copy import deepcopy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from scripts.integration_acceptance_record import (
    CASE_DEFINITIONS,
    create_template,
    disposition_for_checks,
    file_sha256,
    main,
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
                environment[field] = (
                    "b" * 64 if str(field).endswith("_sha256") else f"recorded-{field}"
                )
        checks = case["checks"]
        assert isinstance(checks, dict)
        for check in checks:
            checks[check] = "pass"
        artifact: dict[str, object] = {
            "path": f"artifacts/{case['id']}.txt",
            "description": "redacted integration acceptance evidence",
            "sha256": "0" * 64,
        }
        if case["evidence_mode"] == "provider_manual":
            artifact["reviewed_for_secrets"] = True
            approval = case["approval"]
            assert isinstance(approval, dict)
            approval.update(
                {
                    "status": "granted",
                    "approved_by": "Release Operator",
                    "approved_at_utc": "2026-09-01T12:00:00+00:00",
                    "account_scope": "disposable-redacted-account",
                    "spending_limit": "USD 1 maximum",
                    "allowed_external_effects": [],
                    "credential_cleanup": "not_applicable",
                }
            )
        case["artifacts"] = [artifact]
    return record


class IntegrationAcceptanceRecordTests(unittest.TestCase):
    def test_template_cli_requires_clean_candidate_before_writing(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "record.json"
            with (
                patch(
                    "sys.argv",
                    [
                        "integration_acceptance_record.py",
                        "template",
                        "--output",
                        str(output),
                        "--candidate-sha",
                        CANDIDATE_SHA,
                        "--operator",
                        "Operator",
                    ],
                ),
                patch(
                    "scripts.integration_acceptance_record.require_clean_candidate"
                ) as require_clean,
            ):
                self.assertEqual(main(), 0)
            require_clean.assert_called_once_with(CANDIDATE_SHA)
            self.assertTrue(output.is_file())

    def test_template_has_every_exact_case_check_mode_and_provider_approval(self) -> None:
        record = create_template(CANDIDATE_SHA, "Operator")
        cases = record["cases"]
        assert isinstance(cases, list)
        by_id = {case["id"]: case for case in cases if isinstance(case, dict)}
        self.assertEqual(set(by_id), set(CASE_DEFINITIONS))
        for case_id, definition in CASE_DEFINITIONS.items():
            case = by_id[case_id]
            self.assertEqual(set(case["checks"]), set(definition.checks))
            self.assertEqual(case["evidence_mode"], definition.evidence_mode)
            self.assertEqual(case["disposition"], "unrecorded")
            self.assertEqual("approval" in case, definition.provider_consent)

    def test_incomplete_template_is_accepted_only_when_requested(self) -> None:
        record = create_template(CANDIDATE_SHA, "Operator")
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "record.json"
            failures = validate_record(path, record)
            self.assertTrue(any("remains unrecorded" in item for item in failures))
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
            self.assertEqual(
                validate_record(
                    Path(directory) / "record.json",
                    record,
                    require_artifact_files=False,
                ),
                [],
            )

    def test_missing_case_unknown_check_and_wrong_mode_fail_closed(self) -> None:
        record = completed_record()
        cases = record["cases"]
        assert isinstance(cases, list)
        removed = cases.pop()
        first = cases[0]
        assert isinstance(first, dict)
        first["evidence_mode"] = "provider_manual"
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
        self.assertTrue(any("evidence_mode must be" in item for item in failures))

    def test_disposition_must_agree_with_exact_check_results(self) -> None:
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

    def test_provider_result_requires_explicit_complete_approval(self) -> None:
        record = completed_record()
        cases = record["cases"]
        assert isinstance(cases, list)
        provider = next(
            case
            for case in cases
            if isinstance(case, dict) and case.get("id") == "authenticated.codex"
        )
        approval = provider["approval"]
        assert isinstance(approval, dict)
        approval["status"] = "not_granted"
        with tempfile.TemporaryDirectory() as directory:
            failures = validate_record(
                Path(directory) / "record.json",
                record,
                require_artifact_files=False,
            )
        self.assertTrue(any("require explicit granted approval" in item for item in failures))

        approval["status"] = "granted"
        approval["spending_limit"] = "RECORD_ME"
        with tempfile.TemporaryDirectory() as directory:
            failures = validate_record(
                Path(directory) / "record.json",
                record,
                require_artifact_files=False,
            )
        self.assertTrue(any("approval.spending_limit must be recorded" in item for item in failures))

    def test_not_run_provider_can_be_an_explicit_accepted_limitation(self) -> None:
        record = completed_record()
        cases = record["cases"]
        assert isinstance(cases, list)
        provider = next(
            case
            for case in cases
            if isinstance(case, dict)
            and case.get("id") == "authenticated.claude_code"
        )
        provider["disposition"] = "accepted_limitation"
        checks = provider["checks"]
        assert isinstance(checks, dict)
        for check in checks:
            checks[check] = "accepted_limitation"
        provider["artifacts"] = []
        provider["notes"] = "Not run; provider consent was not granted for this beta."
        provider["approval"] = {
            "status": "not_granted",
            "approved_by": "RECORD_ME",
            "approved_at_utc": "RECORD_ME",
            "account_scope": "RECORD_ME",
            "spending_limit": "RECORD_ME",
            "allowed_external_effects": [],
            "credential_cleanup": "RECORD_ME",
        }
        with tempfile.TemporaryDirectory() as directory:
            self.assertEqual(
                validate_record(
                    Path(directory) / "record.json",
                    record,
                    require_artifact_files=False,
                ),
                [],
            )

    def test_provider_artifact_requires_secret_review_and_artifacts_are_confined(self) -> None:
        record = completed_record()
        cases = record["cases"]
        assert isinstance(cases, list)
        provider = next(
            case
            for case in cases
            if isinstance(case, dict) and case.get("id") == "authenticated.codex"
        )
        artifacts = provider["artifacts"]
        assert isinstance(artifacts, list)
        artifact = artifacts[0]
        assert isinstance(artifact, dict)
        artifact["reviewed_for_secrets"] = False
        artifact["path"] = "../outside.txt"
        with tempfile.TemporaryDirectory() as directory:
            failures = validate_record(
                Path(directory) / "record.json",
                record,
                require_artifact_files=False,
            )
        self.assertTrue(any("escapes the record directory" in item for item in failures))
        self.assertTrue(any("reviewed_for_secrets=true" in item for item in failures))

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
                evidence.write_text(f"evidence for {case['id']}\n", encoding="utf-8")
            path.write_text(json.dumps(record), encoding="utf-8")
            self.assertEqual(seal_artifacts(path, record), len(CASE_DEFINITIONS))
            sealed = json.loads(path.read_text(encoding="utf-8"))
            self.assertEqual(validate_record(path, sealed), [])

            first = sealed["cases"][0]["artifacts"][0]
            evidence = root / first["path"]
            self.assertEqual(first["sha256"], file_sha256(evidence))
            evidence.write_text("tampered\n", encoding="utf-8")
            failures = validate_record(path, sealed)
            self.assertTrue(any("digest mismatch" in item for item in failures))

    def test_require_pass_rejects_an_explicit_limitation(self) -> None:
        record = deepcopy(completed_record())
        cases = record["cases"]
        assert isinstance(cases, list)
        first = cases[0]
        assert isinstance(first, dict)
        first["disposition"] = "accepted_limitation"
        first["notes"] = "Accepted for this candidate"
        checks = first["checks"]
        assert isinstance(checks, dict)
        for check in checks:
            checks[check] = "accepted_limitation"
        first["artifacts"] = []
        with tempfile.TemporaryDirectory() as directory:
            failures = validate_record(
                Path(directory) / "record.json",
                record,
                require_pass=True,
                require_artifact_files=False,
            )
        self.assertTrue(any("release-ready validation requires pass" in item for item in failures))


if __name__ == "__main__":
    unittest.main()
