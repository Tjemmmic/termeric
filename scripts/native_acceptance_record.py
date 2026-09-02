#!/usr/bin/env python3
"""Create, seal, and validate Termeric's native Linux acceptance record."""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys


ROOT = Path(__file__).resolve().parents[1]
SCHEMA_VERSION = 2
DISPOSITIONS = {"pass", "fail", "unavailable", "accepted_limitation", "unrecorded"}
FINAL_DISPOSITIONS = DISPOSITIONS - {"unrecorded"}
SHA256 = re.compile(r"[0-9a-f]{64}")
COMMIT_SHA = re.compile(r"[0-9a-f]{40,64}")
PLACEHOLDER = "RECORD_ME"

COMMON_ENVIRONMENT = (
    "distro",
    "kernel",
    "desktop_session",
    "backend",
    "compositor",
    "compositor_version",
    "gpu",
    "gpu_driver",
    "scale_factor",
    "locale",
    "termeric_version_output",
    "termeric_binary_sha256",
)

VISUAL_CHECKS = (
    "window_launch_and_close",
    "fractional_scale_transition",
    "resize_quality_and_pty_geometry",
    "wide_combining_and_fallback_text",
    "color_emoji_single_and_zwj",
    "ligatures_selection_and_cursor_geometry",
    "fullscreen_enter_exit",
    "sustained_presentation_without_errors",
)
DESKTOP_POLICY_CHECKS = (
    "clipboard_copy_paste",
    "primary_selection",
    "osc52_permission_and_round_trip",
    "external_url_policy",
    "notification_policy_and_presentation",
    "audible_bell_device_output",
)
VENDOR_GPU_CHECKS = (
    "hardware_adapter_selected",
    "surface_loss_recovery",
    "surface_outdated_recovery",
    "device_loss_recovery",
    "pty_and_panes_survive_recovery",
    "idle_cpu_and_redraw_behavior",
    "gpu_memory_observation",
)
IME_CHECKS = (
    "single_pane_preedit_not_sent",
    "single_pane_cjk_commit_once",
    "split_pane_preedit_not_sent",
    "split_pane_cjk_commit_once",
    "dead_key_commit_once",
    "focus_change_cancels_preedit",
    "cancelled_preedit_never_reaches_other_pane",
)
ORCA_CHECKS = (
    "window_name",
    "selected_tab_and_focused_pane",
    "visible_text_cursor_and_selection",
    "polite_live_output_updates",
    "named_tab_bar_controls",
    "f6_chrome_focus",
    "command_palette_search_and_activation",
    "settings_filter_and_adjustment",
    "tab_pane_and_control_actions",
    "modal_background_isolation",
    "inactive_scrollback_and_hidden_text_excluded",
    "toast_and_error_announcement_priority",
)

CASE_DEFINITIONS: dict[str, tuple[str, ...]] = {
    "x11.visual_scale_fonts": VISUAL_CHECKS,
    "wayland.visual_scale_fonts": VISUAL_CHECKS,
    "x11.desktop_policy": DESKTOP_POLICY_CHECKS,
    "wayland.desktop_policy": DESKTOP_POLICY_CHECKS,
    "x11.vendor_gpu_recovery": VENDOR_GPU_CHECKS,
    "wayland.vendor_gpu_recovery": VENDOR_GPU_CHECKS,
    "x11.ibus_ime": IME_CHECKS,
    "x11.fcitx5_ime": IME_CHECKS,
    "wayland.ibus_ime": IME_CHECKS,
    "wayland.fcitx5_ime": IME_CHECKS,
    "x11.orca": ORCA_CHECKS,
    "wayland.orca": ORCA_CHECKS,
}


class RecordError(RuntimeError):
    pass


def read_json(path: Path) -> dict[str, object]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise RecordError(f"cannot read {path}: {error}") from error
    if not isinstance(value, dict):
        raise RecordError("native acceptance record root must be an object")
    return value


def atomic_write_json(path: Path, value: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.tmp")
    temporary.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")
    os.replace(temporary, path)


def workspace_version() -> str:
    cargo = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    match = re.search(r'^version\s*=\s*"([^"]+)"', cargo, re.MULTILINE)
    if match is None:
        raise RecordError("workspace Cargo.toml has no version")
    return match.group(1)


def current_commit() -> str:
    result = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        timeout=15,
    )
    if result.returncode != 0:
        raise RecordError(f"cannot determine candidate commit: {result.stderr.strip()}")
    return result.stdout.strip()


def require_clean_candidate(candidate_sha: str) -> None:
    head = current_commit()
    if candidate_sha != head:
        raise RecordError(
            f"candidate SHA {candidate_sha} does not match checked-out HEAD {head}"
        )
    result = subprocess.run(
        ["git", "status", "--porcelain", "--untracked-files=all"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        timeout=30,
    )
    if result.returncode != 0:
        raise RecordError(f"cannot inspect candidate worktree: {result.stderr.strip()}")
    if result.stdout.strip():
        raise RecordError(
            "refusing to create candidate evidence from a dirty worktree; "
            "commit or remove every tracked and untracked source change first"
        )


def backend_for(case_id: str) -> str:
    return case_id.split(".", 1)[0]


def case_template(case_id: str, checks: tuple[str, ...]) -> dict[str, object]:
    environment = {field: PLACEHOLDER for field in COMMON_ENVIRONMENT}
    environment["backend"] = backend_for(case_id)
    if "ibus_ime" in case_id:
        environment["input_method"] = "IBus"
        environment["input_method_version"] = PLACEHOLDER
    elif "fcitx5_ime" in case_id:
        environment["input_method"] = "Fcitx 5"
        environment["input_method_version"] = PLACEHOLDER
    if case_id.endswith(".orca"):
        environment["screen_reader"] = "Orca"
        environment["screen_reader_version"] = PLACEHOLDER
        environment["at_spi_bus"] = PLACEHOLDER
    return {
        "id": case_id,
        "disposition": "unrecorded",
        "environment": environment,
        "checks": {check: "unrecorded" for check in checks},
        "artifacts": [],
        "notes": "",
    }


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
            case_template(case_id, checks)
            for case_id, checks in CASE_DEFINITIONS.items()
        ],
        "notes": (
            "Artifacts may contain terminal content. Review and redact them before sharing."
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


def safe_artifact_path(record_path: Path, raw_path: object) -> Path:
    if not isinstance(raw_path, str) or not raw_path.strip():
        raise RecordError("artifact path must be a non-empty relative path")
    relative = Path(raw_path)
    if relative.is_absolute() or ".." in relative.parts:
        raise RecordError(f"artifact path escapes the record directory: {raw_path!r}")
    base = record_path.resolve().parent
    resolved = (base / relative).resolve()
    try:
        resolved.relative_to(base)
    except ValueError as error:
        raise RecordError(f"artifact path escapes the record directory: {raw_path!r}") from error
    return resolved


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def artifact_failures(
    record_path: Path,
    case_id: str,
    artifacts: object,
    *,
    require_files: bool,
) -> list[str]:
    failures: list[str] = []
    if not isinstance(artifacts, list):
        return [f"{case_id}: artifacts must be an array"]
    for index, artifact in enumerate(artifacts):
        prefix = f"{case_id}: artifact {index}"
        if not isinstance(artifact, dict):
            failures.append(f"{prefix} must be an object")
            continue
        try:
            path = safe_artifact_path(record_path, artifact.get("path"))
        except RecordError as error:
            failures.append(f"{prefix}: {error}")
            continue
        description = artifact.get("description")
        if not isinstance(description, str) or not description.strip():
            failures.append(f"{prefix} needs a non-empty description")
        digest = artifact.get("sha256")
        if not isinstance(digest, str) or SHA256.fullmatch(digest) is None:
            failures.append(f"{prefix} needs a lowercase SHA-256 digest")
        if not require_files:
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
        failures.append(
            f"schema must be {SCHEMA_VERSION}, got {record.get('schema')!r}"
        )
    candidate = record.get("candidate")
    if not isinstance(candidate, dict):
        failures.append("candidate must be an object")
    else:
        sha = candidate.get("sha")
        if not isinstance(sha, str) or COMMIT_SHA.fullmatch(sha) is None:
            failures.append("candidate.sha must be 40-64 lowercase hexadecimal characters")
        elif expected_sha is not None and sha != expected_sha:
            failures.append(f"candidate.sha is {sha}, expected {expected_sha}")
        version = candidate.get("termeric_version")
        expected_version = workspace_version()
        if version != expected_version:
            failures.append(
                f"candidate.termeric_version is {version!r}, expected {expected_version!r}"
            )
        for field in ("recorded_at_utc", "operator"):
            value = candidate.get(field)
            if not isinstance(value, str) or not value.strip() or value == PLACEHOLDER:
                failures.append(f"candidate.{field} must be recorded")
        recorded_at = candidate.get("recorded_at_utc")
        if isinstance(recorded_at, str) and recorded_at != PLACEHOLDER:
            try:
                timestamp = datetime.fromisoformat(recorded_at)
            except ValueError:
                failures.append("candidate.recorded_at_utc must be an ISO-8601 timestamp")
            else:
                if timestamp.utcoffset() != timezone.utc.utcoffset(timestamp):
                    failures.append("candidate.recorded_at_utc must carry a UTC offset")

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
    missing = sorted(CASE_DEFINITIONS.keys() - by_id.keys())
    extra = sorted(by_id.keys() - CASE_DEFINITIONS.keys())
    for case_id in missing:
        failures.append(f"missing required case {case_id}")
    for case_id in extra:
        failures.append(f"unknown case {case_id}")

    for case_id, expected_checks in CASE_DEFINITIONS.items():
        case = by_id.get(case_id)
        if case is None:
            continue
        disposition = case.get("disposition")
        if disposition not in DISPOSITIONS:
            failures.append(f"{case_id}: invalid disposition {disposition!r}")
        elif not allow_incomplete and disposition not in FINAL_DISPOSITIONS:
            failures.append(f"{case_id}: disposition remains unrecorded")
        elif require_pass and disposition != "pass":
            failures.append(f"{case_id}: release-ready validation requires pass")

        environment = case.get("environment")
        if not isinstance(environment, dict):
            failures.append(f"{case_id}: environment must be an object")
        else:
            for field in COMMON_ENVIRONMENT:
                value = environment.get(field)
                if not isinstance(value, str) or not value.strip() or value == PLACEHOLDER:
                    if not allow_incomplete:
                        failures.append(f"{case_id}: environment.{field} must be recorded")
            expected_backend = backend_for(case_id)
            if environment.get("backend") != expected_backend:
                failures.append(
                    f"{case_id}: environment.backend must be {expected_backend!r}"
                )
            binary_digest = environment.get("termeric_binary_sha256")
            if (
                isinstance(binary_digest, str)
                and binary_digest != PLACEHOLDER
                and SHA256.fullmatch(binary_digest) is None
            ):
                failures.append(
                    f"{case_id}: environment.termeric_binary_sha256 must be a lowercase SHA-256 digest"
                )
            conditional_fields: tuple[str, ...] = ()
            if "_ime" in case_id:
                conditional_fields = ("input_method", "input_method_version")
            elif case_id.endswith(".orca"):
                conditional_fields = (
                    "screen_reader",
                    "screen_reader_version",
                    "at_spi_bus",
                )
            for field in conditional_fields:
                value = environment.get(field)
                if not isinstance(value, str) or not value.strip() or value == PLACEHOLDER:
                    if not allow_incomplete:
                        failures.append(f"{case_id}: environment.{field} must be recorded")
            expected_tool: tuple[str, str] | None = None
            if "ibus_ime" in case_id:
                expected_tool = ("input_method", "IBus")
            elif "fcitx5_ime" in case_id:
                expected_tool = ("input_method", "Fcitx 5")
            elif case_id.endswith(".orca"):
                expected_tool = ("screen_reader", "Orca")
            if expected_tool is not None and environment.get(expected_tool[0]) != expected_tool[1]:
                failures.append(
                    f"{case_id}: environment.{expected_tool[0]} must be {expected_tool[1]!r}"
                )

        raw_checks = case.get("checks")
        if not isinstance(raw_checks, dict):
            failures.append(f"{case_id}: checks must be an object")
            continue
        checks = {str(key): str(value) for key, value in raw_checks.items()}
        missing_checks = sorted(set(expected_checks) - checks.keys())
        extra_checks = sorted(checks.keys() - set(expected_checks))
        for check in missing_checks:
            failures.append(f"{case_id}: missing check {check}")
        for check in extra_checks:
            failures.append(f"{case_id}: unknown check {check}")
        for check, value in checks.items():
            if value not in DISPOSITIONS:
                failures.append(f"{case_id}: check {check} has invalid result {value!r}")
            elif not allow_incomplete and value == "unrecorded":
                failures.append(f"{case_id}: check {check} remains unrecorded")
        if not missing_checks and not extra_checks and all(
            value in DISPOSITIONS for value in checks.values()
        ):
            derived = disposition_for_checks(checks)
            if disposition != derived:
                failures.append(
                    f"{case_id}: disposition {disposition!r} disagrees with checks ({derived!r})"
                )

        artifacts = case.get("artifacts")
        failures.extend(
            artifact_failures(
                record_path,
                case_id,
                artifacts,
                require_files=require_artifact_files,
            )
        )
        evidence_required = any(
            result in {"pass", "fail"} for result in checks.values()
        )
        if evidence_required and isinstance(artifacts, list) and not artifacts:
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
            candidate_sha = args.candidate_sha or current_commit()
            require_clean_candidate(candidate_sha)
            record = create_template(candidate_sha, args.operator)
            atomic_write_json(output, record)
            print(f"Wrote native acceptance template: {output}")
            return 0
        record = read_json(args.record)
        if args.command == "seal":
            count = seal_artifacts(args.record, record)
            print(f"Sealed {count} native acceptance artifact(s)")
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
            print("Native acceptance record failures:", file=sys.stderr)
            print("\n".join(failures), file=sys.stderr)
            return 1
        print(f"Validated native acceptance record: {args.record}")
        return 0
    except (OSError, RecordError, subprocess.TimeoutExpired) as error:
        print(f"Native acceptance record failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
