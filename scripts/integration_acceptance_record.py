#!/usr/bin/env python3
"""Create, seal, and validate Termeric integration acceptance evidence."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
from datetime import datetime, timezone
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from scripts.native_acceptance_record import (
    COMMIT_SHA,
    PLACEHOLDER,
    SHA256,
    RecordError,
    atomic_write_json,
    file_sha256,
    require_clean_candidate,
    safe_artifact_path,
    workspace_version,
)


SCHEMA_VERSION = 1
DISPOSITIONS = {"pass", "fail", "unavailable", "accepted_limitation", "unrecorded"}
FINAL_DISPOSITIONS = DISPOSITIONS - {"unrecorded"}
APPROVAL_STATUSES = {"not_granted", "granted"}
CREDENTIAL_CLEANUP = {"revoked", "not_applicable", "retained_by_operator"}

AGENT_COMMAND_CHECKS = (
    "open",
    "resume",
    "new_conversation",
    "restart",
    "review_diff",
    "copy_diff",
    "insert_context",
    "toggle_transcript",
    "export_session",
)
FAKE_PROVIDER_CHECKS = (
    "launch_argv_environment_cwd_and_profile",
    "scripted_output_and_diff",
    *AGENT_COMMAND_CHECKS,
)
AUTHENTICATED_PROVIDER_CHECKS = (
    *AGENT_COMMAND_CHECKS,
    "transcript_and_export_redaction_review",
    "unapproved_external_effects_declined",
)
POSIX_SHELL_CHECKS = (
    "custom_user_rc_preserved",
    "osc7_working_directory",
    "multiline_success_region",
    "failing_exit_region",
    "capability_replies_return_to_origin",
)
DESKTOP_CHECKS = (
    "selection_copy_and_paste",
    "osc52_write_and_read",
    "link_opening",
    "visual_bell",
    "audio_bell",
    "notification",
    "helper_failure_is_visible",
)


@dataclass(frozen=True)
class CaseDefinition:
    evidence_mode: str
    checks: tuple[str, ...]
    environment_fields: tuple[str, ...]
    fixed_environment: tuple[tuple[str, str], ...] = ()
    provider_consent: bool = False


TEST_ENVIRONMENT = ("platform", "rustc_version", "test_binary_sha256")
SHELL_ENVIRONMENT = (
    "platform",
    "shell",
    "shell_path",
    "shell_version",
    "test_binary_sha256",
)
DESKTOP_ENVIRONMENT = (
    "distro",
    "kernel",
    "desktop_session",
    "backend",
    "compositor",
    "compositor_version",
    "termeric_version_output",
    "termeric_binary_sha256",
    "clipboard_tool_version",
    "url_opener",
    "audio_helper",
    "notification_helper",
)
PROVIDER_ENVIRONMENT = (
    "platform",
    "provider",
    "cli_version",
    "termeric_version_output",
    "termeric_binary_sha256",
    "selected_profile",
    "selected_model",
    "redacted_configuration_summary",
)


CASE_DEFINITIONS: dict[str, CaseDefinition] = {
    "fake.claude_code": CaseDefinition("hermetic", FAKE_PROVIDER_CHECKS, TEST_ENVIRONMENT),
    "fake.codex": CaseDefinition("hermetic", FAKE_PROVIDER_CHECKS, TEST_ENVIRONMENT),
    "shell.bash": CaseDefinition(
        "hermetic", POSIX_SHELL_CHECKS, SHELL_ENVIRONMENT, (("shell", "Bash"),)
    ),
    "shell.zsh": CaseDefinition(
        "hermetic", POSIX_SHELL_CHECKS, SHELL_ENVIRONMENT, (("shell", "Zsh"),)
    ),
    "shell.fish": CaseDefinition(
        "hermetic", POSIX_SHELL_CHECKS, SHELL_ENVIRONMENT, (("shell", "Fish"),)
    ),
    "shell.nushell": CaseDefinition(
        "hermetic",
        (
            "user_config_preserved",
            "native_osc7_working_directory",
            "native_osc133_regions",
            "script_and_special_modes_unchanged",
        ),
        SHELL_ENVIRONMENT,
        (("shell", "Nushell"),),
    ),
    "shell.elvish": CaseDefinition(
        "hermetic",
        (
            "user_config_preserved",
            "module_import",
            "osc7_working_directory",
            "osc133_regions",
            "special_modes_unchanged",
            "xdg_data_dirs_restored",
        ),
        SHELL_ENVIRONMENT,
        (("shell", "Elvish"),),
    ),
    "harvest.workflows": CaseDefinition(
        "hermetic",
        (
            "tab_and_pane_isolation",
            "history",
            "rerun",
            "copy",
            "search",
            "explain_routing_without_provider_execution",
            "unsupported_shell_explanation",
        ),
        TEST_ENVIRONMENT,
    ),
    "x11.desktop_integrations": CaseDefinition(
        "native_desktop", DESKTOP_CHECKS, DESKTOP_ENVIRONMENT, (("backend", "x11"),)
    ),
    "wayland.desktop_integrations": CaseDefinition(
        "native_desktop",
        DESKTOP_CHECKS,
        DESKTOP_ENVIRONMENT,
        (("backend", "wayland"),),
    ),
    "authenticated.claude_code": CaseDefinition(
        "provider_manual",
        AUTHENTICATED_PROVIDER_CHECKS,
        PROVIDER_ENVIRONMENT,
        (("provider", "Claude Code"),),
        provider_consent=True,
    ),
    "authenticated.codex": CaseDefinition(
        "provider_manual",
        AUTHENTICATED_PROVIDER_CHECKS,
        PROVIDER_ENVIRONMENT,
        (("provider", "Codex"),),
        provider_consent=True,
    ),
}


def read_json(path: Path) -> dict[str, object]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise RecordError(f"cannot read {path}: {error}") from error
    if not isinstance(value, dict):
        raise RecordError("integration acceptance record root must be an object")
    return value


def utc_timestamp_failure(value: object, field: str) -> str | None:
    if not isinstance(value, str) or not value.strip() or value == PLACEHOLDER:
        return f"{field} must be recorded"
    try:
        timestamp = datetime.fromisoformat(value)
    except ValueError:
        return f"{field} must be an ISO-8601 timestamp"
    if timestamp.utcoffset() != timezone.utc.utcoffset(timestamp):
        return f"{field} must carry a UTC offset"
    return None


def approval_template() -> dict[str, object]:
    return {
        "status": "not_granted",
        "approved_by": PLACEHOLDER,
        "approved_at_utc": PLACEHOLDER,
        "account_scope": PLACEHOLDER,
        "spending_limit": PLACEHOLDER,
        "allowed_external_effects": [],
        "credential_cleanup": PLACEHOLDER,
    }


def case_template(case_id: str, definition: CaseDefinition) -> dict[str, object]:
    environment = {field: PLACEHOLDER for field in definition.environment_fields}
    environment.update(dict(definition.fixed_environment))
    case: dict[str, object] = {
        "id": case_id,
        "evidence_mode": definition.evidence_mode,
        "disposition": "unrecorded",
        "environment": environment,
        "checks": {check: "unrecorded" for check in definition.checks},
        "artifacts": [],
        "notes": "",
    }
    if definition.provider_consent:
        case["approval"] = approval_template()
    return case


def create_template(candidate_sha: str, operator: str) -> dict[str, object]:
    if COMMIT_SHA.fullmatch(candidate_sha) is None:
        raise RecordError("candidate SHA must be 40-64 lowercase hexadecimal characters")
    return {
        "schema": SCHEMA_VERSION,
        "candidate": {
            "sha": candidate_sha,
            "termeric_version": workspace_version(),
            "recorded_at_utc": datetime.now(timezone.utc).isoformat(),
            "operator": operator or PLACEHOLDER,
        },
        "cases": [
            case_template(case_id, definition)
            for case_id, definition in CASE_DEFINITIONS.items()
        ],
        "notes": (
            "Provider artifacts must be redacted and marked reviewed_for_secrets. "
            "Never store credentials, account identifiers, or conversation identifiers."
        ),
    }


def disposition_for_checks(checks: dict[str, str]) -> str:
    values = set(checks.values())
    if "unrecorded" in values:
        return "unrecorded"
    if "fail" in values:
        return "fail"
    if values == {"pass"}:
        return "pass"
    if "accepted_limitation" in values:
        return "accepted_limitation"
    return "unavailable"


def artifact_failures(
    record_path: Path,
    case_id: str,
    artifacts: object,
    *,
    require_files: bool,
    provider_artifacts: bool,
) -> list[str]:
    failures: list[str] = []
    if not isinstance(artifacts, list):
        return [f"{case_id}: artifacts must be an array"]
    for index, artifact in enumerate(artifacts):
        prefix = f"{case_id}: artifact {index}"
        if not isinstance(artifact, dict):
            failures.append(f"{prefix} must be an object")
            continue
        path: Path | None
        try:
            path = safe_artifact_path(record_path, artifact.get("path"))
        except RecordError as error:
            failures.append(f"{prefix}: {error}")
            path = None
        description = artifact.get("description")
        if not isinstance(description, str) or not description.strip():
            failures.append(f"{prefix} needs a non-empty description")
        digest = artifact.get("sha256")
        if not isinstance(digest, str) or SHA256.fullmatch(digest) is None:
            failures.append(f"{prefix} needs a lowercase SHA-256 digest")
        if provider_artifacts and artifact.get("reviewed_for_secrets") is not True:
            failures.append(f"{prefix} must be marked reviewed_for_secrets=true")
        if not require_files or path is None:
            continue
        if path.is_symlink() or not path.is_file():
            failures.append(f"{prefix} is missing a regular file: {path}")
        elif isinstance(digest, str) and SHA256.fullmatch(digest):
            actual = file_sha256(path)
            if actual != digest:
                failures.append(
                    f"{prefix} digest mismatch: recorded {digest}, actual {actual}"
                )
    return failures


def approval_failures(case_id: str, approval: object, *, run_recorded: bool) -> list[str]:
    if not isinstance(approval, dict):
        return [f"{case_id}: approval must be an object"]
    failures: list[str] = []
    expected_fields = {
        "status",
        "approved_by",
        "approved_at_utc",
        "account_scope",
        "spending_limit",
        "allowed_external_effects",
        "credential_cleanup",
    }
    missing = sorted(expected_fields - approval.keys())
    extra = sorted(approval.keys() - expected_fields)
    failures.extend(f"{case_id}: approval missing {field}" for field in missing)
    failures.extend(f"{case_id}: approval has unknown field {field}" for field in extra)
    status = approval.get("status")
    if status not in APPROVAL_STATUSES:
        failures.append(f"{case_id}: invalid approval status {status!r}")
    if run_recorded and status != "granted":
        failures.append(f"{case_id}: pass/fail checks require explicit granted approval")
    if status == "granted":
        for field in ("approved_by", "account_scope", "spending_limit"):
            value = approval.get(field)
            if not isinstance(value, str) or not value.strip() or value == PLACEHOLDER:
                failures.append(f"{case_id}: approval.{field} must be recorded")
        timestamp_failure = utc_timestamp_failure(
            approval.get("approved_at_utc"), f"{case_id}: approval.approved_at_utc"
        )
        if timestamp_failure is not None:
            failures.append(timestamp_failure)
        effects = approval.get("allowed_external_effects")
        if not isinstance(effects, list) or any(
            not isinstance(effect, str) or not effect.strip() for effect in effects
        ):
            failures.append(
                f"{case_id}: approval.allowed_external_effects must be an array of strings"
            )
        cleanup = approval.get("credential_cleanup")
        if cleanup not in CREDENTIAL_CLEANUP:
            failures.append(
                f"{case_id}: approval.credential_cleanup must be one of "
                f"{sorted(CREDENTIAL_CLEANUP)}"
            )
    return failures


def validate_record(
    record_path: Path,
    record: dict[str, object],
    *,
    allow_incomplete: bool = False,
    require_pass: bool = False,
    require_artifact_files: bool = True,
    expected_sha: str | None = None,
) -> list[str]:
    failures: list[str] = []
    if record.get("schema") != SCHEMA_VERSION:
        failures.append(f"schema must be {SCHEMA_VERSION}, got {record.get('schema')!r}")
    candidate = record.get("candidate")
    if not isinstance(candidate, dict):
        failures.append("candidate must be an object")
    else:
        sha = candidate.get("sha")
        if not isinstance(sha, str) or COMMIT_SHA.fullmatch(sha) is None:
            failures.append("candidate.sha must be 40-64 lowercase hexadecimal characters")
        elif expected_sha is not None and sha != expected_sha:
            failures.append(f"candidate.sha is {sha}, expected {expected_sha}")
        if candidate.get("termeric_version") != workspace_version():
            failures.append(
                "candidate.termeric_version does not match the current workspace version"
            )
        operator = candidate.get("operator")
        if (
            not isinstance(operator, str)
            or not operator.strip()
            or operator == PLACEHOLDER
        ) and not allow_incomplete:
            failures.append("candidate.operator must be recorded")
        timestamp_failure = utc_timestamp_failure(
            candidate.get("recorded_at_utc"), "candidate.recorded_at_utc"
        )
        if timestamp_failure is not None:
            failures.append(timestamp_failure)

    cases = record.get("cases")
    if not isinstance(cases, list):
        return failures + ["cases must be an array"]
    by_id: dict[str, dict[str, object]] = {}
    for index, case in enumerate(cases):
        if not isinstance(case, dict):
            failures.append(f"case {index} must be an object")
            continue
        case_id = case.get("id")
        if not isinstance(case_id, str):
            failures.append(f"case {index} has no string id")
            continue
        if case_id in by_id:
            failures.append(f"duplicate case {case_id}")
        by_id[case_id] = case
    for case_id in sorted(CASE_DEFINITIONS.keys() - by_id.keys()):
        failures.append(f"missing required case {case_id}")
    for case_id in sorted(by_id.keys() - CASE_DEFINITIONS.keys()):
        failures.append(f"unknown case {case_id}")

    for case_id, definition in CASE_DEFINITIONS.items():
        case = by_id.get(case_id)
        if case is None:
            continue
        if case.get("evidence_mode") != definition.evidence_mode:
            failures.append(
                f"{case_id}: evidence_mode must be {definition.evidence_mode!r}"
            )
        disposition = case.get("disposition")
        if disposition not in DISPOSITIONS:
            failures.append(f"{case_id}: invalid disposition {disposition!r}")
        elif not allow_incomplete and disposition not in FINAL_DISPOSITIONS:
            failures.append(f"{case_id}: disposition remains unrecorded")
        elif require_pass and disposition != "pass":
            failures.append(f"{case_id}: release-ready validation requires pass")

        environment = case.get("environment")
        expected_environment = set(definition.environment_fields)
        if not isinstance(environment, dict):
            failures.append(f"{case_id}: environment must be an object")
        else:
            for field in sorted(expected_environment - environment.keys()):
                failures.append(f"{case_id}: environment missing {field}")
            for field in sorted(environment.keys() - expected_environment):
                failures.append(f"{case_id}: environment has unknown field {field}")
            fixed = dict(definition.fixed_environment)
            for field in definition.environment_fields:
                value = environment.get(field)
                if field in fixed and value != fixed[field]:
                    failures.append(
                        f"{case_id}: environment.{field} must be {fixed[field]!r}"
                    )
                if (
                    not isinstance(value, str)
                    or not value.strip()
                    or value == PLACEHOLDER
                ) and not allow_incomplete:
                    failures.append(f"{case_id}: environment.{field} must be recorded")
                if (
                    field.endswith("_sha256")
                    and isinstance(value, str)
                    and value != PLACEHOLDER
                    and SHA256.fullmatch(value) is None
                ):
                    failures.append(
                        f"{case_id}: environment.{field} must be a lowercase SHA-256 digest"
                    )

        raw_checks = case.get("checks")
        checks: dict[str, str] = {}
        if not isinstance(raw_checks, dict):
            failures.append(f"{case_id}: checks must be an object")
        else:
            checks = {str(key): str(value) for key, value in raw_checks.items()}
            expected_checks = set(definition.checks)
            for check in sorted(expected_checks - checks.keys()):
                failures.append(f"{case_id}: missing check {check}")
            for check in sorted(checks.keys() - expected_checks):
                failures.append(f"{case_id}: unknown check {check}")
            for check, value in checks.items():
                if value not in DISPOSITIONS:
                    failures.append(
                        f"{case_id}: check {check} has invalid result {value!r}"
                    )
                elif not allow_incomplete and value == "unrecorded":
                    failures.append(f"{case_id}: check {check} remains unrecorded")
            if set(checks) == expected_checks and all(
                value in DISPOSITIONS for value in checks.values()
            ):
                derived = disposition_for_checks(checks)
                if disposition != derived:
                    failures.append(
                        f"{case_id}: disposition {disposition!r} disagrees with checks "
                        f"({derived!r})"
                    )

        run_recorded = any(value in {"pass", "fail"} for value in checks.values())
        if definition.provider_consent:
            failures.extend(
                approval_failures(case_id, case.get("approval"), run_recorded=run_recorded)
            )
        elif "approval" in case:
            failures.append(f"{case_id}: only provider-manual cases may contain approval")

        artifacts = case.get("artifacts")
        failures.extend(
            artifact_failures(
                record_path,
                case_id,
                artifacts,
                require_files=require_artifact_files,
                provider_artifacts=definition.provider_consent,
            )
        )
        if run_recorded and isinstance(artifacts, list) and not artifacts:
            failures.append(f"{case_id}: pass/fail checks require at least one artifact")
        notes = case.get("notes")
        if disposition in {"fail", "unavailable", "accepted_limitation"} and (
            not isinstance(notes, str) or not notes.strip()
        ):
            failures.append(f"{case_id}: non-pass disposition requires explanatory notes")
    return failures


def seal_artifacts(record_path: Path, record: dict[str, object]) -> int:
    cases = record.get("cases")
    if not isinstance(cases, list):
        raise RecordError("cases must be an array")
    sealed = 0
    for case in cases:
        if not isinstance(case, dict):
            continue
        artifacts = case.get("artifacts")
        if not isinstance(artifacts, list):
            continue
        for artifact in artifacts:
            if not isinstance(artifact, dict):
                continue
            path = safe_artifact_path(record_path, artifact.get("path"))
            if path.is_symlink() or not path.is_file():
                raise RecordError(f"cannot seal missing regular artifact: {path}")
            artifact["sha256"] = file_sha256(path)
            sealed += 1
    atomic_write_json(record_path, record)
    return sealed


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)

    template = commands.add_parser("template", help="write a complete unrecorded template")
    template.add_argument("--output", type=Path, required=True)
    template.add_argument("--candidate-sha", default=None)
    template.add_argument("--operator", default="")

    seal = commands.add_parser("seal", help="hash every referenced local artifact")
    seal.add_argument("record", type=Path)

    validate = commands.add_parser("validate", help="validate structure and evidence")
    validate.add_argument("record", type=Path)
    validate.add_argument("--allow-incomplete", action="store_true")
    validate.add_argument("--require-pass", action="store_true")
    validate.add_argument("--skip-artifact-files", action="store_true")
    validate.add_argument("--expected-sha")
    args = parser.parse_args()

    try:
        if args.command == "template":
            output = args.output.resolve()
            if output.exists():
                raise RecordError(f"refusing to overwrite existing record: {output}")
            candidate_sha = args.candidate_sha
            if candidate_sha is None:
                result = subprocess.run(
                    ["git", "rev-parse", "HEAD"],
                    cwd=ROOT,
                    capture_output=True,
                    text=True,
                    timeout=15,
                    check=True,
                )
                candidate_sha = result.stdout.strip()
            require_clean_candidate(candidate_sha)
            record = create_template(candidate_sha, args.operator)
            atomic_write_json(output, record)
            print(f"Wrote integration acceptance template: {output}")
            return 0
        record = read_json(args.record)
        if args.command == "seal":
            count = seal_artifacts(args.record, record)
            print(f"Sealed {count} integration acceptance artifact(s)")
            return 0
        failures = validate_record(
            args.record,
            record,
            allow_incomplete=args.allow_incomplete,
            require_pass=args.require_pass,
            require_artifact_files=not args.skip_artifact_files,
            expected_sha=args.expected_sha,
        )
        if failures:
            print("Integration acceptance record failures:", file=sys.stderr)
            print("\n".join(failures), file=sys.stderr)
            return 1
        print(f"Validated integration acceptance record: {args.record}")
        return 0
    except (OSError, RecordError, subprocess.SubprocessError) as error:
        print(f"Integration acceptance record failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
