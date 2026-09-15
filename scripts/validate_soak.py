#!/usr/bin/env python3
"""Validate a serialized Termeric core resource-soak evidence record."""

from __future__ import annotations

import argparse
from datetime import datetime
import json
import math
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from scripts.run_soak import (
    CORE_SOAK_RECORD_KIND,
    CORE_SOAK_SCHEMA_VERSION,
    DIFF_PROGRESS_FIELDS,
    DIFF_SOAK_THRESHOLDS,
    FILE_DESCRIPTOR_KINDS,
    INVENTORY_DIMENSIONS,
    MAX_INVENTORY_LABELS,
    MAX_PROCESS_LABEL_CHARACTERS,
    TRANSCRIPT_ENTRIES_PER_ITERATION,
    bounded_process_label,
    build_core_soak_checks,
    file_sha256,
    minimum_sample_count,
    parse_progress,
    summarize_resource_inventories,
)
from scripts.soak_provenance import provenance_checks, validate_record_binding


SHA256_PATTERN = re.compile(r"[0-9a-f]{64}")
COMMIT_PATTERN = re.compile(r"[0-9a-f]{40}")


class SoakValidationError(RuntimeError):
    """Raised when core soak evidence is not trustworthy."""


def require_exact_keys(
    value: object, expected: set[str], context: str
) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise SoakValidationError(f"{context} must be an object")
    actual = set(value)
    if actual != expected:
        raise SoakValidationError(
            f"{context} fields differ: missing={sorted(expected - actual)}, "
            f"extra={sorted(actual - expected)}"
        )
    return value


def require_integer(
    value: object,
    context: str,
    *,
    minimum: int = 0,
    maximum: int | None = None,
) -> int:
    if (
        type(value) is not int
        or value < minimum
        or (maximum is not None and value > maximum)
    ):
        maximum_text = f" and <= {maximum}" if maximum is not None else ""
        raise SoakValidationError(
            f"{context} must be an integer >= {minimum}{maximum_text}"
        )
    return value


def require_number(
    value: object, context: str, *, minimum: float = 0.0, strict: bool = False
) -> float | int:
    if type(value) not in (int, float) or not math.isfinite(value):
        raise SoakValidationError(f"{context} must be a finite number")
    if (strict and value <= minimum) or (not strict and value < minimum):
        comparison = ">" if strict else ">="
        raise SoakValidationError(f"{context} must be {comparison} {minimum}")
    return value


def safe_relative_file(root: Path, raw_path: object, context: str) -> Path:
    if not isinstance(raw_path, str) or not raw_path:
        raise SoakValidationError(f"{context} path must be a non-empty string")
    pure = PurePosixPath(raw_path)
    if pure.is_absolute() or any(part in ("", ".", "..") for part in pure.parts):
        raise SoakValidationError(f"{context} path is unsafe: {raw_path!r}")
    root = root.resolve()
    candidate = root.joinpath(*pure.parts)
    current = root
    for part in pure.parts:
        current = current / part
        if current.is_symlink():
            raise SoakValidationError(f"{context} path contains a symlink: {raw_path!r}")
    try:
        resolved = candidate.resolve(strict=True)
        resolved.relative_to(root)
    except (FileNotFoundError, ValueError) as error:
        raise SoakValidationError(f"{context} path is missing or escapes its root") from error
    if not resolved.is_file():
        raise SoakValidationError(f"{context} path is not a regular file")
    return resolved


def validate_file_evidence(
    evidence: object, root: Path, context: str
) -> Path:
    item = require_exact_keys(evidence, {"path", "bytes", "sha256"}, context)
    path = safe_relative_file(root, item["path"], context)
    if require_integer(item["bytes"], f"{context}.bytes") != path.stat().st_size:
        raise SoakValidationError(f"{context} byte length changed")
    digest = item["sha256"]
    if (
        not isinstance(digest, str)
        or SHA256_PATTERN.fullmatch(digest) is None
        or digest != file_sha256(path)
    ):
        raise SoakValidationError(f"{context} SHA-256 digest changed")
    return path


def git_identity(source: Path) -> tuple[str, bool]:
    try:
        commit = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=source,
            text=True,
            capture_output=True,
            check=True,
        ).stdout.strip()
        status = subprocess.run(
            ["git", "status", "--porcelain=v1", "--untracked-files=all"],
            cwd=source,
            text=True,
            capture_output=True,
            check=True,
        ).stdout.strip()
    except subprocess.CalledProcessError as error:
        detail = error.stderr.strip() or error.stdout.strip()
        raise SoakValidationError(f"cannot inspect source candidate: {detail}") from error
    if COMMIT_PATTERN.fullmatch(commit) is None:
        raise SoakValidationError("source candidate commit is invalid")
    return commit, not bool(status)


def validate_environment(value: object, source: Path) -> tuple[dict[str, Any], bool]:
    environment = require_exact_keys(
        value,
        {"platform", "python", "rustc", "commit", "worktree_dirty", "wgpu_backend"},
        "environment",
    )
    for field in ("platform", "python", "rustc"):
        item = environment[field]
        if not isinstance(item, str) or not item or len(item) > 512:
            raise SoakValidationError(f"environment.{field} is invalid")
    backend = environment["wgpu_backend"]
    if backend is not None and (
        not isinstance(backend, str) or not backend or len(backend) > 128
    ):
        raise SoakValidationError("environment.wgpu_backend is invalid")
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
    return environment, current_clean


def validate_thresholds(value: object) -> dict[str, Any]:
    thresholds = require_exact_keys(
        value,
        {
            "max_rss_kib",
            "max_rss_growth_kib",
            "max_thread_growth",
            "max_fd_growth",
            "max_children",
            "max_transcript_queue_bytes",
            "minimum_resource_samples",
            "transcript_entries_per_iteration",
            "max_inventory_labels_per_dimension",
            "max_process_label_characters",
            *DIFF_SOAK_THRESHOLDS,
        },
        "thresholds",
    )
    require_number(thresholds["max_rss_kib"], "thresholds.max_rss_kib", strict=True)
    for field in ("max_rss_growth_kib", "max_thread_growth", "max_fd_growth"):
        require_number(thresholds[field], f"thresholds.{field}")
    require_integer(thresholds["max_children"], "thresholds.max_children")
    require_integer(
        thresholds["max_transcript_queue_bytes"],
        "thresholds.max_transcript_queue_bytes",
        minimum=1,
    )
    require_integer(
        thresholds["minimum_resource_samples"],
        "thresholds.minimum_resource_samples",
        minimum=2,
    )
    fixed = {
        **DIFF_SOAK_THRESHOLDS,
        "transcript_entries_per_iteration": TRANSCRIPT_ENTRIES_PER_ITERATION,
        "max_inventory_labels_per_dimension": MAX_INVENTORY_LABELS,
        "max_process_label_characters": MAX_PROCESS_LABEL_CHARACTERS,
    }
    for field, expected in fixed.items():
        if thresholds[field] != expected or type(thresholds[field]) is not int:
            raise SoakValidationError(f"thresholds.{field} differs from the evaluator")
    return thresholds


def validate_inventory(value: object, sample: dict[str, Any], index: int) -> None:
    inventory = require_exact_keys(
        value,
        {dimension for dimension, _ in INVENTORY_DIMENSIONS},
        f"samples[{index}].inventory",
    )
    for dimension, aggregate in INVENTORY_DIMENSIONS:
        counts = inventory[dimension]
        if not isinstance(counts, dict) or len(counts) > MAX_INVENTORY_LABELS:
            raise SoakValidationError(f"samples[{index}].inventory.{dimension} is invalid")
        for label, count in counts.items():
            if (
                not isinstance(label, str)
                or not label
                or len(label) > MAX_PROCESS_LABEL_CHARACTERS
                or label != bounded_process_label(label)
                or (dimension == "fd_kinds" and label not in FILE_DESCRIPTOR_KINDS)
            ):
                raise SoakValidationError(
                    f"samples[{index}].inventory.{dimension} has an unsafe label"
                )
            require_integer(count, f"samples[{index}].inventory.{dimension}.{label}")
        expected = (
            len(sample[aggregate]) if aggregate == "children" else sample[aggregate]
        )
        if sum(counts.values()) != expected:
            raise SoakValidationError(
                f"samples[{index}].inventory.{dimension} does not reconcile"
            )


def validate_samples(
    value: object, *, extra_fields: set[str] | None = None
) -> list[dict[str, Any]]:
    if not isinstance(value, list):
        raise SoakValidationError("samples must be an array")
    previous_elapsed = -1.0
    previous_cpu_ticks = -1
    for index, raw_sample in enumerate(value):
        sample = require_exact_keys(
            raw_sample,
            {
                "elapsed_seconds",
                "rss_kib",
                "threads",
                "fds",
                "children",
                "cpu_ticks",
                "inventory",
            }
            | (extra_fields or set()),
            f"samples[{index}]",
        )
        elapsed = require_number(
            sample["elapsed_seconds"], f"samples[{index}].elapsed_seconds"
        )
        if elapsed <= previous_elapsed:
            raise SoakValidationError("sample elapsed times must be strictly increasing")
        previous_elapsed = float(elapsed)
        require_number(sample["rss_kib"], f"samples[{index}].rss_kib")
        require_integer(sample["threads"], f"samples[{index}].threads")
        require_integer(sample["fds"], f"samples[{index}].fds")
        cpu_ticks = require_integer(sample["cpu_ticks"], f"samples[{index}].cpu_ticks")
        if cpu_ticks < previous_cpu_ticks:
            raise SoakValidationError("sample CPU ticks must be non-decreasing")
        previous_cpu_ticks = cpu_ticks
        children = sample["children"]
        if (
            not isinstance(children, list)
            or any(type(pid) is not int or pid <= 0 for pid in children)
            or children != sorted(set(children))
        ):
            raise SoakValidationError(f"samples[{index}].children is invalid")
        validate_inventory(sample["inventory"], sample, index)
    return value


def validate_progress(value: object, log: Path) -> list[dict[str, Any]]:
    if not isinstance(value, list):
        raise SoakValidationError("progress must be an array")
    workload_fields = {
        *DIFF_PROGRESS_FIELDS,
        "iteration",
        "scrollback_bytes",
        "image_bytes",
        "transcript_queue_bytes",
        "transcript_queue_bytes_after_finish",
        "transcript_entries_accepted",
        "transcript_entries_dropped",
    }
    completion_fields = {"complete", "iterations"}
    expected_iteration = 1
    completion_count = 0
    for index, raw_item in enumerate(value):
        if isinstance(raw_item, dict) and "iteration" in raw_item:
            item = require_exact_keys(raw_item, workload_fields, f"progress[{index}]")
            for field in workload_fields:
                require_integer(item[field], f"progress[{index}].{field}")
            if item["iteration"] != expected_iteration:
                raise SoakValidationError("workload progress iterations are not contiguous")
            expected_iteration += 1
        else:
            item = require_exact_keys(raw_item, completion_fields, f"progress[{index}]")
            if item["complete"] is not True:
                raise SoakValidationError(f"progress[{index}].complete must be true")
            require_integer(
                item["iterations"], f"progress[{index}].iterations", minimum=1
            )
            completion_count += 1
            if index != len(value) - 1 or item["iterations"] != expected_iteration - 1:
                raise SoakValidationError("completion progress does not match the workload")
    if completion_count > 1:
        raise SoakValidationError("progress has more than one completion record")
    if parse_progress(log) != value:
        raise SoakValidationError("recorded progress differs from the sealed scenario log")
    return value


def validate_outcomes(
    value: object, progress: list[dict[str, Any]], log: Path, gpu_requested: bool
) -> dict[str, Any]:
    outcomes = require_exact_keys(
        value,
        {
            "gpu_exit_status",
            "scenario_exit_status",
            "completion_markers",
            "remaining_observed_children",
            "gpu_error_count",
        },
        "outcomes",
    )
    gpu_exit_status = outcomes["gpu_exit_status"]
    if gpu_requested:
        require_integer(
            gpu_exit_status,
            "outcomes.gpu_exit_status",
            minimum=-255,
            maximum=255,
        )
    elif gpu_exit_status is not None:
        raise SoakValidationError("unrequested GPU smoke must have a null exit status")
    require_integer(
        outcomes["scenario_exit_status"],
        "outcomes.scenario_exit_status",
        minimum=-255,
        maximum=255,
    )
    for field in (
        "completion_markers",
        "remaining_observed_children",
        "gpu_error_count",
    ):
        require_integer(outcomes[field], f"outcomes.{field}")
    completion_markers = sum(item.get("complete") is True for item in progress)
    if outcomes["completion_markers"] != completion_markers:
        raise SoakValidationError("outcomes.completion_markers differs from progress")
    log_text = log.read_text(encoding="utf-8", errors="replace").lower()
    gpu_errors = sum(
        log_text.count(pattern)
        for pattern in ("validation error", "device lost", "device removed")
    )
    if outcomes["gpu_error_count"] != gpu_errors:
        raise SoakValidationError("outcomes.gpu_error_count differs from the scenario log")
    return outcomes


def validate_record(
    record_path: Path, source: Path, *, allow_ineligible: bool = False
) -> dict[str, Any]:
    if record_path.is_symlink() or not record_path.is_file():
        raise SoakValidationError("record is missing or is not a regular file")
    try:
        def reject_constant(value: str) -> None:
            raise SoakValidationError(f"invalid JSON numeric constant: {value}")

        payload = json.loads(
            record_path.read_text(encoding="utf-8"),
            parse_constant=reject_constant,
        )
    except (OSError, json.JSONDecodeError) as error:
        raise SoakValidationError(f"cannot parse core soak record: {error}") from error
    root = require_exact_keys(
        payload,
        {
            "schema_version",
            "kind",
            "status",
            "candidate_eligible",
            "eligibility",
            "recorded_at_utc",
            "environment",
            "duration_seconds",
            "sample_interval_seconds",
            "minimum_resource_samples",
            "maximum_transcript_queue_bytes",
            "gpu_smoke_requested",
            "test_binary",
            "provenance",
            "artifacts",
            "thresholds",
            "outcomes",
            "samples",
            "progress",
            "resource_inventory_summary",
            "checks",
        },
        "record",
    )
    if (
        type(root["schema_version"]) is not int
        or root["schema_version"] != CORE_SOAK_SCHEMA_VERSION
        or root["kind"] != CORE_SOAK_RECORD_KIND
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
    _, source_clean = validate_environment(root["environment"], source)
    duration = require_integer(root["duration_seconds"], "duration_seconds", minimum=1)
    interval = require_number(
        root["sample_interval_seconds"], "sample_interval_seconds", strict=True
    )
    minimum_samples = require_integer(
        root["minimum_resource_samples"], "minimum_resource_samples", minimum=2
    )
    maximum_queue = require_integer(
        root["maximum_transcript_queue_bytes"],
        "maximum_transcript_queue_bytes",
        minimum=1,
    )
    if type(root["gpu_smoke_requested"]) is not bool:
        raise SoakValidationError("gpu_smoke_requested must be a boolean")
    thresholds = validate_thresholds(root["thresholds"])
    if minimum_samples != minimum_sample_count(duration, interval):
        raise SoakValidationError("minimum_resource_samples is inconsistent with cadence")
    if thresholds["minimum_resource_samples"] != minimum_samples:
        raise SoakValidationError("threshold sample count differs from the record")
    if thresholds["max_transcript_queue_bytes"] != maximum_queue:
        raise SoakValidationError("transcript queue ceiling differs from the record")

    binary = validate_file_evidence(root["test_binary"], source, "test_binary")
    try:
        provenance_checks(root["provenance"], ["gpu-smoke", "scenario"] if root["gpu_smoke_requested"] else ["scenario"])
        validate_record_binding(root["provenance"], root["environment"], root["test_binary"])
    except ValueError as error:
        raise SoakValidationError(str(error)) from error
    artifacts = require_exact_keys(root["artifacts"], {"scenario_log"}, "artifacts")
    log = validate_file_evidence(
        artifacts["scenario_log"], record_path.parent, "artifacts.scenario_log"
    )
    if binary == log or binary == record_path.resolve():
        raise SoakValidationError("evidence paths must identify distinct files")
    samples = validate_samples(root["samples"])
    progress = validate_progress(root["progress"], log)
    outcomes = validate_outcomes(
        root["outcomes"], progress, log, root["gpu_smoke_requested"]
    )
    expected_summary = summarize_resource_inventories(samples)
    if root["resource_inventory_summary"] != expected_summary:
        raise SoakValidationError("resource inventory summary does not match samples")
    expected_checks = build_core_soak_checks(
        samples,
        progress,
        thresholds,
        outcomes,
        gpu_smoke_requested=root["gpu_smoke_requested"],
        provenance=root["provenance"],
    )
    if root["checks"] != expected_checks:
        raise SoakValidationError("reported checks do not match reconstructed checks")
    passed = all(check["passed"] is True for check in expected_checks)
    expected_status = "pass" if passed else "fail"
    expected_candidate_eligible = passed and source_clean
    expected_eligibility = (
        "candidate"
        if expected_candidate_eligible
        else "dirty-dry-run"
        if passed
        else "failed"
    )
    if root["status"] != expected_status:
        raise SoakValidationError("record status does not match reconstructed checks")
    if root["candidate_eligible"] is not expected_candidate_eligible:
        raise SoakValidationError("candidate eligibility is inconsistent")
    if root["eligibility"] != expected_eligibility:
        raise SoakValidationError("eligibility classification is inconsistent")
    if not expected_candidate_eligible and not allow_ineligible:
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
        print(f"Core soak validation failed: {error}", file=sys.stderr)
        return 1
    print(f"Validated core resource soak: {arguments.record}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
