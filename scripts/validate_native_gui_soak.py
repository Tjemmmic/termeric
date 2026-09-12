#!/usr/bin/env python3
"""Validate a serialized Termeric native resource-soak evidence bundle."""

from __future__ import annotations

import argparse
from datetime import datetime
import json
from pathlib import Path
import subprocess
import sys
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from scripts.gui_acceptance import AcceptanceError, read_records
from scripts.drm_memory import (
    DRM_MEMORY_INTERFACE,
    MAX_DRM_AGGREGATE_BYTES,
    DrmMemoryError,
    summarize_drm_memory,
    validate_drm_memory_snapshot,
)
from scripts.run_native_gui_soak import (
    MAX_DRIVER_ERROR_CHARACTERS,
    MAX_NATIVE_SOAK_ARTIFACTS,
    MAX_NATIVE_SOAK_ARTIFACT_BYTES,
    MAX_NATIVE_SOAK_TOTAL_ARTIFACT_BYTES,
    NATIVE_SOAK_RECORD_KIND,
    NATIVE_SOAK_SCHEMA_VERSION,
    bounded_driver_error,
    build_native_soak_checks,
    kitty_image_ack_count,
    native_artifact_manifest,
    progress_count,
    runtime_error_count,
)
from scripts.run_soak import (
    MAX_INVENTORY_LABELS,
    MAX_PROCESS_LABEL_CHARACTERS,
    minimum_sample_count,
    summarize_resource_inventories,
)
from scripts.validate_soak import (
    COMMIT_PATTERN,
    SoakValidationError,
    git_identity,
    require_exact_keys,
    require_integer,
    require_number,
    validate_file_evidence,
    validate_samples,
)


def validate_environment(value: object, source: Path) -> bool:
    environment = require_exact_keys(
        value,
        {
            "platform",
            "python",
            "commit",
            "worktree_dirty",
            "backend",
            "compositor",
            "wgpu_backend",
            "gpu_memory_interface",
        },
        "environment",
    )
    for field in ("platform", "python"):
        item = environment[field]
        if not isinstance(item, str) or not item or len(item) > 512:
            raise SoakValidationError(f"environment.{field} is invalid")
    if environment["backend"] != "wayland":
        raise SoakValidationError("environment.backend must be wayland")
    if environment["compositor"] != "sway-headless":
        raise SoakValidationError("environment.compositor must be sway-headless")
    backend = environment["wgpu_backend"]
    if not isinstance(backend, str) or not backend or len(backend) > 128:
        raise SoakValidationError("environment.wgpu_backend is invalid")
    if environment["gpu_memory_interface"] != DRM_MEMORY_INTERFACE:
        raise SoakValidationError("environment.gpu_memory_interface is invalid")
    commit = environment["commit"]
    if not isinstance(commit, str) or COMMIT_PATTERN.fullmatch(commit) is None:
        raise SoakValidationError("environment.commit is invalid")
    if type(environment["worktree_dirty"]) is not bool:
        raise SoakValidationError("environment.worktree_dirty must be a boolean")
    current_commit, current_clean = git_identity(source)
    if commit != current_commit:
        raise SoakValidationError("record candidate commit does not match the source tree")
    expected_dirty = not current_clean
    if environment["worktree_dirty"] is not expected_dirty:
        raise SoakValidationError("record worktree state does not match the source tree")
    return current_clean


def validate_artifact_policy(value: object) -> None:
    policy = require_exact_keys(
        value, {"max_files", "max_file_bytes", "max_total_bytes"}, "artifact_policy"
    )
    expected = {
        "max_files": MAX_NATIVE_SOAK_ARTIFACTS,
        "max_file_bytes": MAX_NATIVE_SOAK_ARTIFACT_BYTES,
        "max_total_bytes": MAX_NATIVE_SOAK_TOTAL_ARTIFACT_BYTES,
    }
    if policy != expected:
        raise SoakValidationError("artifact policy differs from the validator")


def validate_artifacts(value: object, record_path: Path) -> dict[str, Path]:
    if not isinstance(value, list):
        raise SoakValidationError("artifacts must be an array")
    if len(value) > MAX_NATIVE_SOAK_ARTIFACTS:
        raise SoakValidationError("artifact manifest has too many files")
    paths: dict[str, Path] = {}
    previous = ""
    total_bytes = 0
    for index, raw_entry in enumerate(value):
        entry = require_exact_keys(
            raw_entry, {"path", "bytes", "sha256"}, f"artifacts[{index}]"
        )
        raw_path = entry["path"]
        if not isinstance(raw_path, str) or raw_path <= previous:
            raise SoakValidationError("artifact paths must be unique and sorted")
        previous = raw_path
        size = require_integer(entry["bytes"], f"artifacts[{index}].bytes")
        if size > MAX_NATIVE_SOAK_ARTIFACT_BYTES:
            raise SoakValidationError(f"artifacts[{index}] exceeds its byte limit")
        total_bytes += size
        if total_bytes > MAX_NATIVE_SOAK_TOTAL_ARTIFACT_BYTES:
            raise SoakValidationError("artifact manifest exceeds its total byte limit")
        paths[raw_path] = validate_file_evidence(
            entry, record_path.parent, f"artifacts[{index}]"
        )
    try:
        actual = native_artifact_manifest(record_path.parent)
    except AcceptanceError as error:
        raise SoakValidationError(str(error)) from error
    if value != actual:
        raise SoakValidationError("artifact manifest differs from the exact bundle inventory")
    return paths


def validate_thresholds(
    value: object, duration: int, sample_interval: float | int, action_interval: float | int
) -> dict[str, Any]:
    thresholds = require_exact_keys(
        value,
        {
            "max_rss_kib",
            "max_rss_growth_kib",
            "max_thread_growth",
            "max_fd_growth",
            "max_children",
            "require_gpu_memory",
            "max_drm_resident_bytes",
            "max_drm_resident_growth_bytes",
            "minimum_resource_samples",
            "minimum_progress_records",
            "minimum_resize_actions",
            "minimum_pane_actions",
            "minimum_graphics_acknowledgements",
            "max_inventory_labels_per_dimension",
            "max_process_label_characters",
        },
        "thresholds",
    )
    require_number(thresholds["max_rss_kib"], "thresholds.max_rss_kib", strict=True)
    for field in ("max_rss_growth_kib", "max_thread_growth", "max_fd_growth"):
        require_number(thresholds[field], f"thresholds.{field}")
    require_integer(thresholds["max_children"], "thresholds.max_children")
    if type(thresholds["require_gpu_memory"]) is not bool:
        raise SoakValidationError("thresholds.require_gpu_memory must be a boolean")
    require_integer(
        thresholds["max_drm_resident_bytes"],
        "thresholds.max_drm_resident_bytes",
        minimum=1,
        maximum=MAX_DRM_AGGREGATE_BYTES,
    )
    require_integer(
        thresholds["max_drm_resident_growth_bytes"],
        "thresholds.max_drm_resident_growth_bytes",
        maximum=MAX_DRM_AGGREGATE_BYTES,
    )
    fixed = {
        "minimum_resource_samples": minimum_sample_count(duration, sample_interval),
        "minimum_progress_records": max(1, duration // 3),
        "minimum_resize_actions": max(
            1, duration // max(1, int(action_interval * 3))
        ),
        "minimum_pane_actions": max(
            1, duration // max(1, int(action_interval * 3))
        ),
        "minimum_graphics_acknowledgements": max(1, duration // 6),
        "max_inventory_labels_per_dimension": MAX_INVENTORY_LABELS,
        "max_process_label_characters": MAX_PROCESS_LABEL_CHARACTERS,
    }
    for field, expected in fixed.items():
        if type(thresholds[field]) is not int or thresholds[field] != expected:
            raise SoakValidationError(f"thresholds.{field} differs from its formula")
    return thresholds


def validate_outcomes(
    value: object,
    artifacts: dict[str, Path],
    observed_children: list[int],
) -> dict[str, Any]:
    outcomes = require_exact_keys(
        value,
        {
            "exit_status",
            "progress_records",
            "resize_actions",
            "pane_actions",
            "graphics_acknowledgements",
            "remaining_children",
            "runtime_errors",
            "driver_error",
        },
        "outcomes",
    )
    exit_status = outcomes["exit_status"]
    if exit_status is not None:
        require_integer(
            exit_status, "outcomes.exit_status", minimum=-255, maximum=255
        )
    for field in (
        "progress_records",
        "resize_actions",
        "pane_actions",
        "graphics_acknowledgements",
        "remaining_children",
        "runtime_errors",
    ):
        require_integer(outcomes[field], f"outcomes.{field}")
    if outcomes["remaining_children"] > len(observed_children):
        raise SoakValidationError("remaining child count exceeds observed children")
    driver_error = outcomes["driver_error"]
    if driver_error is not None and (
        not isinstance(driver_error, str)
        or not driver_error
        or len(driver_error) > MAX_DRIVER_ERROR_CHARACTERS
        or bounded_driver_error(driver_error) != driver_error
    ):
        raise SoakValidationError("outcomes.driver_error is invalid")

    transcript = artifacts.get("pty-transcript.jsonl")
    try:
        records = read_records(transcript) if transcript is not None else []
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise SoakValidationError(f"cannot parse sealed PTY transcript: {error}") from error
    if any(not isinstance(record, dict) for record in records):
        raise SoakValidationError("sealed PTY transcript contains a non-object record")
    derived_progress = progress_count(records)
    if outcomes["progress_records"] != derived_progress:
        raise SoakValidationError("progress count differs from the sealed transcript")
    try:
        derived_acks = kitty_image_ack_count(transcript) if transcript is not None else 0
    except (KeyError, ValueError, TypeError) as error:
        raise SoakValidationError(f"cannot decode sealed PTY transcript: {error}") from error
    if outcomes["graphics_acknowledgements"] != derived_acks:
        raise SoakValidationError(
            "graphics acknowledgement count differs from the sealed transcript"
        )

    application_log = artifacts.get("application/termeric.log")
    log_text = (
        application_log.read_text(encoding="utf-8", errors="replace")
        if application_log is not None
        else ""
    )
    if outcomes["runtime_errors"] != runtime_error_count(log_text):
        raise SoakValidationError("runtime error count differs from the sealed log")
    return outcomes


def validate_record(
    record_path: Path, source: Path, *, allow_ineligible: bool = False
) -> dict[str, Any]:
    if record_path.name != "results.json":
        raise SoakValidationError("native soak record must be named results.json")
    if record_path.is_symlink() or not record_path.is_file():
        raise SoakValidationError("record is missing or is not a regular file")

    def reject_constant(value: str) -> None:
        raise SoakValidationError(f"invalid JSON numeric constant: {value}")

    try:
        payload = json.loads(
            record_path.read_text(encoding="utf-8"), parse_constant=reject_constant
        )
    except (OSError, json.JSONDecodeError) as error:
        raise SoakValidationError(f"cannot parse native soak record: {error}") from error
    root = require_exact_keys(
        payload,
        {
            "schema",
            "kind",
            "status",
            "candidate_eligible",
            "eligibility",
            "recorded_at_utc",
            "environment",
            "binary",
            "artifact_policy",
            "artifacts",
            "duration_seconds",
            "sample_interval_seconds",
            "action_interval_seconds",
            "thresholds",
            "outcomes",
            "samples",
            "resource_inventory_summary",
            "drm_memory_summary",
            "observed_children",
            "checks",
        },
        "record",
    )
    if (
        root["schema"] != NATIVE_SOAK_SCHEMA_VERSION
        or root["kind"] != NATIVE_SOAK_RECORD_KIND
    ):
        raise SoakValidationError("record schema or kind is unsupported")
    timestamp = root["recorded_at_utc"]
    if not isinstance(timestamp, str):
        raise SoakValidationError("recorded_at_utc must be a string")
    try:
        parsed_timestamp = datetime.fromisoformat(timestamp)
    except ValueError as error:
        raise SoakValidationError("recorded_at_utc is invalid") from error
    if parsed_timestamp.tzinfo is None:
        raise SoakValidationError("recorded_at_utc must include a timezone")

    source = source.resolve()
    source_clean = validate_environment(root["environment"], source)
    binary = validate_file_evidence(root["binary"], source, "binary")
    validate_artifact_policy(root["artifact_policy"])
    artifacts = validate_artifacts(root["artifacts"], record_path)
    if binary == record_path.resolve() or binary in artifacts.values():
        raise SoakValidationError("binary and evidence paths must identify distinct files")

    duration = require_integer(root["duration_seconds"], "duration_seconds", minimum=5)
    sample_interval = require_number(
        root["sample_interval_seconds"], "sample_interval_seconds", strict=True
    )
    action_interval = require_number(
        root["action_interval_seconds"], "action_interval_seconds", strict=True
    )
    thresholds = validate_thresholds(
        root["thresholds"], duration, sample_interval, action_interval
    )
    samples = validate_samples(root["samples"], extra_fields={"gpu_memory"})
    try:
        for sample in samples:
            validate_drm_memory_snapshot(sample["gpu_memory"])
    except DrmMemoryError as error:
        raise SoakValidationError(f"invalid DRM memory sample: {error}") from error
    observed_children = root["observed_children"]
    if (
        not isinstance(observed_children, list)
        or any(type(pid) is not int or pid <= 0 for pid in observed_children)
        or observed_children != sorted(set(observed_children))
    ):
        raise SoakValidationError("observed_children must be sorted unique PIDs")
    sampled_children = {
        pid for sample in samples for pid in sample["children"]
    }
    if not sampled_children.issubset(observed_children):
        raise SoakValidationError("sampled children are missing from observed_children")
    outcomes = validate_outcomes(root["outcomes"], artifacts, observed_children)
    expected_summary = summarize_resource_inventories(samples)
    if root["resource_inventory_summary"] != expected_summary:
        raise SoakValidationError("resource inventory summary does not match samples")
    try:
        expected_drm_summary = summarize_drm_memory(samples)
    except DrmMemoryError as error:
        raise SoakValidationError(f"invalid DRM memory summary: {error}") from error
    if root["drm_memory_summary"] != expected_drm_summary:
        raise SoakValidationError("DRM memory summary does not match samples")
    expected_checks = build_native_soak_checks(samples, thresholds, outcomes)
    if root["checks"] != expected_checks:
        raise SoakValidationError("reported checks do not match reconstructed checks")
    passed = all(check["passed"] is True for check in expected_checks)
    required_passing_artifacts = {
        "application/termeric.log",
        "application/termeric.stdout",
        "pty-transcript.jsonl",
        "sway-config",
        "sway.log",
        "terminfo/x/xterm-termeric",
    }
    if passed and not required_passing_artifacts.issubset(artifacts):
        missing = sorted(required_passing_artifacts - set(artifacts))
        raise SoakValidationError(
            f"passing bundle is missing required artifacts: {missing}"
        )
    expected_status = "pass" if passed else "fail"
    candidate_eligible = passed and source_clean
    eligibility = (
        "candidate"
        if candidate_eligible
        else "dirty-dry-run"
        if passed
        else "failed"
    )
    if root["status"] != expected_status:
        raise SoakValidationError("record status does not match reconstructed checks")
    if root["candidate_eligible"] is not candidate_eligible:
        raise SoakValidationError("candidate eligibility is inconsistent")
    if root["eligibility"] != eligibility:
        raise SoakValidationError("eligibility classification is inconsistent")
    if not candidate_eligible and not allow_ineligible:
        raise SoakValidationError("record is not eligible candidate evidence")
    return root


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("record", type=Path)
    parser.add_argument("--source", type=Path, default=ROOT)
    parser.add_argument(
        "--allow-ineligible",
        action="store_true",
        help="accept an internally consistent failed or dirty diagnostic record",
    )
    arguments = parser.parse_args(argv)
    try:
        validate_record(
            arguments.record.resolve(),
            arguments.source.resolve(),
            allow_ineligible=arguments.allow_ineligible,
        )
    except (SoakValidationError, OSError, subprocess.SubprocessError) as error:
        print(f"Native soak validation failed: {error}", file=sys.stderr)
        return 1
    print(f"Validated native resource soak: {arguments.record}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
