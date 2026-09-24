#!/usr/bin/env python3
"""Record the exact hermetic fake-provider integration acceptance subset."""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import platform
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from scripts.integration_acceptance_record import (
    CASE_DEFINITIONS,
    RecordError,
    atomic_write_json,
    create_template,
    file_sha256,
    seal_artifacts,
    validate_record,
)
from scripts.native_acceptance_record import require_clean_candidate


FAKE_PROVIDER_TEST = (
    "app::tests::fake_agent_clis_exercise_every_agent_command_end_to_end"
)
FAKE_PROVIDER_CASES = ("fake.claude_code", "fake.codex")
PROVIDER_OPT_IN_ENVIRONMENT = ("TERMERIC_REAL_CODEX_CONTRACT",)


def safe_test_environment(source: dict[str, str]) -> dict[str, str]:
    environment = dict(source)
    for variable in PROVIDER_OPT_IN_ENVIRONMENT:
        environment.pop(variable, None)
    return environment


def test_binary_from_cargo_messages(output: str) -> Path:
    executables: set[Path] = set()
    for line in output.splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not isinstance(message, dict) or message.get("reason") != "compiler-artifact":
            continue
        target = message.get("target")
        profile = message.get("profile")
        executable = message.get("executable")
        kind = target.get("kind") if isinstance(target, dict) else None
        if (
            isinstance(target, dict)
            and target.get("name") == "termeric"
            and isinstance(kind, list)
            and "bin" in kind
            and isinstance(profile, dict)
            and profile.get("test") is True
            and isinstance(executable, str)
        ):
            reported = Path(executable)
            executables.add(
                (reported if reported.is_absolute() else ROOT / reported).resolve()
            )
    if len(executables) != 1:
        raise RecordError(
            "expected one Termeric test executable from Cargo, found "
            f"{len(executables)}"
        )
    binary = executables.pop()
    if binary.is_symlink() or not binary.is_file():
        raise RecordError(f"Cargo reported a missing test executable: {binary}")
    return binary


def run_fake_provider_test(
    artifact_directory: Path,
    *,
    candidate_sha: str,
    environment: dict[str, str] | None = None,
) -> dict[str, object]:
    artifact_directory.mkdir(parents=True, exist_ok=True)
    log_path = artifact_directory / "fake-provider-test.log"
    result_path = artifact_directory / "fake-provider-result.json"
    for path in (log_path, result_path):
        if path.exists() or path.is_symlink():
            raise RecordError(f"refusing to overwrite existing artifact: {path}")
    test_environment = safe_test_environment(
        dict(os.environ) if environment is None else environment
    )
    build_command = [
        "cargo",
        "test",
        "-p",
        "termeric",
        "--locked",
        "--no-run",
        "--message-format=json",
    ]
    build = subprocess.run(
        build_command,
        cwd=ROOT,
        env=test_environment,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=900,
    )
    log_sections = [
        f"build_command={json.dumps(build_command)}",
        f"removed_provider_opt_ins={json.dumps(PROVIDER_OPT_IN_ENVIRONMENT)}",
        "build_stderr:",
        build.stderr,
    ]
    if build.returncode != 0:
        log_path.write_text("\n".join(log_sections), encoding="utf-8")
        raise RecordError(
            f"failed to build the hermetic integration test ({build.returncode}); "
            f"see {log_path}"
        )

    test_binary = test_binary_from_cargo_messages(build.stdout)
    test_command = [str(test_binary), FAKE_PROVIDER_TEST, "--exact", "--nocapture"]
    test = subprocess.run(
        test_command,
        cwd=ROOT,
        env=test_environment,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=300,
    )
    log_sections.extend(
        [
            f"test_command={json.dumps(test_command)}",
            "test_stdout:",
            test.stdout,
            "test_stderr:",
            test.stderr,
        ]
    )
    log_path.write_text("\n".join(log_sections), encoding="utf-8")
    expected_result = f"test {FAKE_PROVIDER_TEST} ... ok"
    if (
        test.returncode != 0
        or expected_result not in test.stdout
        or "1 passed; 0 failed" not in test.stdout
    ):
        raise RecordError(
            "the exact fake-provider integration test did not produce one passing result; "
            f"see {log_path}"
        )

    result: dict[str, object] = {
        "schema": 1,
        "recorded_at_utc": datetime.now(timezone.utc).isoformat(),
        "candidate_sha": candidate_sha,
        "test": FAKE_PROVIDER_TEST,
        "status": "pass",
        "test_binary": str(test_binary),
        "test_binary_sha256": file_sha256(test_binary),
        "removed_provider_opt_ins": list(PROVIDER_OPT_IN_ENVIRONMENT),
        "log_sha256": file_sha256(log_path),
    }
    atomic_write_json(result_path, result)
    result["log_path"] = log_path
    result["result_path"] = result_path
    return result


def fill_fake_provider_cases(
    record: dict[str, object],
    *,
    platform_name: str,
    rustc_version: str,
    test_binary_sha256: str,
) -> None:
    cases = record.get("cases")
    if not isinstance(cases, list):
        raise RecordError("integration record cases must be an array")
    by_id = {
        case.get("id"): case
        for case in cases
        if isinstance(case, dict) and isinstance(case.get("id"), str)
    }
    for case_id in FAKE_PROVIDER_CASES:
        case = by_id.get(case_id)
        if not isinstance(case, dict):
            raise RecordError(f"integration record is missing {case_id}")
        definition = CASE_DEFINITIONS[case_id]
        case["disposition"] = "pass"
        case["environment"] = {
            "platform": platform_name,
            "rustc_version": rustc_version,
            "test_binary_sha256": test_binary_sha256,
        }
        case["checks"] = {check: "pass" for check in definition.checks}
        case["artifacts"] = [
            {
                "path": "artifacts/fake-provider-test.log",
                "description": (
                    "exact fake Claude Code/Codex real-PTY integration test output"
                ),
                "sha256": "0" * 64,
            },
            {
                "path": "artifacts/fake-provider-result.json",
                "description": (
                    "machine-readable exact-test, opt-in removal, and binary identity"
                ),
                "sha256": "0" * 64,
            },
        ]
        case["notes"] = (
            "Hermetic fake executable only; no provider CLI, account, credential, "
            "network request, or AI-service token was used."
        )


def command_output(command: list[str]) -> str:
    return subprocess.run(
        command,
        cwd=ROOT,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=30,
        check=True,
    ).stdout.strip()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--operator", required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    try:
        if not args.operator.strip():
            raise RecordError("operator must not be empty")
        if output.exists() or output.is_symlink():
            raise RecordError(f"refusing to overwrite existing record: {output}")
        candidate_sha = command_output(["git", "rev-parse", "HEAD"])
        require_clean_candidate(candidate_sha)
        evidence = run_fake_provider_test(
            output.parent / "artifacts", candidate_sha=candidate_sha
        )
        test_binary_sha256 = evidence.get("test_binary_sha256")
        if not isinstance(test_binary_sha256, str):
            raise RecordError("fake-provider evidence omitted its test binary digest")
        record = create_template(candidate_sha, args.operator)
        fill_fake_provider_cases(
            record,
            platform_name=platform.platform(),
            rustc_version=command_output(["rustc", "--version"]),
            test_binary_sha256=test_binary_sha256,
        )
        atomic_write_json(output, record)
        seal_artifacts(output, record)
        sealed = json.loads(output.read_text(encoding="utf-8"))
        failures = validate_record(output, sealed, allow_incomplete=True)
        if failures:
            raise RecordError("; ".join(failures))
        print(f"Recorded hermetic fake-provider acceptance: {output}")
        print("Authenticated and non-fake integration rows remain unrecorded.")
        return 0
    except (OSError, RecordError, subprocess.SubprocessError) as error:
        print(f"Fake-provider acceptance failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
