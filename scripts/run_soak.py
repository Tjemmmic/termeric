#!/usr/bin/env python3
"""Run Termeric's Linux resource soak and enforce bounded growth ceilings."""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
import platform
import statistics
import subprocess
import sys
import time
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
CORE_SOAK_SCHEMA_VERSION = 3
CORE_SOAK_RECORD_KIND = "termeric-core-resource-soak"
SOAK_PREFIX = "TERMERIC_SOAK "
TRANSCRIPT_ENTRIES_PER_ITERATION = 68
MAX_PROCESS_LABEL_CHARACTERS = 32
MAX_INVENTORY_LABELS = 64
COLLAPSED_INVENTORY_LABEL = "<other-labels>"
FILE_DESCRIPTOR_KINDS = frozenset(
    {
        "socket",
        "pipe",
        "anon-eventpoll",
        "anon-eventfd",
        "anon-timerfd",
        "anon-signalfd",
        "anon-inotify",
        "anon-pidfd",
        "anon-other",
        "filesystem",
        "device-path",
        "memory-file",
        "other",
        "unreadable",
    }
)
INVENTORY_DIMENSIONS = (
    ("fd_kinds", "fds"),
    ("thread_names", "threads"),
    ("child_names", "children"),
)


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def relative_file_path(root: Path, path: Path, context: str) -> str:
    """Return a regular file path confined beneath ``root``."""
    root = root.resolve()
    resolved = path.resolve()
    try:
        relative = resolved.relative_to(root)
    except ValueError as error:
        raise ValueError(f"{context} must be inside {root}: {path}") from error
    if path.is_symlink() or not resolved.is_file():
        raise ValueError(f"{context} must be a regular non-symlink file: {path}")
    return relative.as_posix()


def write_json_atomic(path: Path, payload: dict[str, Any]) -> None:
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    try:
        temporary.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def build_test_binary() -> Path:
    command = [
        "cargo",
        "test",
        "-p",
        "termeric",
        "--locked",
        "--no-run",
        "--message-format=json",
    ]
    completed = subprocess.run(
        command,
        cwd=ROOT,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=True,
    )
    candidates: list[Path] = []
    for line in completed.stdout.splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        target = message.get("target", {})
        executable = message.get("executable")
        if (
            message.get("reason") == "compiler-artifact"
            and executable
            and target.get("name") == "termeric"
            and "bin" in target.get("kind", [])
            and message.get("profile", {}).get("test")
        ):
            candidates.append(Path(executable))
    if not candidates:
        raise RuntimeError(f"Cargo did not report the Termeric test binary:\n{completed.stderr}")
    return candidates[-1]


def process_tasks(proc: Path) -> list[Path]:
    """Enumerate one bounded process task snapshot in deterministic order."""
    try:
        return sorted((proc / "task").iterdir(), key=lambda path: int(path.name))
    except FileNotFoundError:
        return []


def process_children_from_tasks(tasks: list[Path]) -> list[int]:
    """Return direct children reported by one already-enumerated task snapshot."""
    children: set[int] = set()
    for task in tasks:
        try:
            values = (task / "children").read_text().split()
        except FileNotFoundError:
            # Threads may exit while /proc is being sampled.
            continue
        children.update(int(value) for value in values)
    return sorted(children)


def process_children(proc: Path) -> list[int]:
    """Return direct children spawned by any thread in a Linux process."""
    return process_children_from_tasks(process_tasks(proc))


def bounded_process_label(value: str) -> str:
    """Return a privacy-safe bounded `/proc/*/comm` diagnostic label."""
    sanitized = "".join(
        character if character.isprintable() and character not in "\r\n" else "?"
        for character in value.strip()
    )
    return sanitized[:MAX_PROCESS_LABEL_CHARACTERS] or "<empty>"


def counted_labels(labels: list[str]) -> dict[str, int]:
    counts: dict[str, int] = {}
    for label in labels:
        counts[label] = counts.get(label, 0) + 1
    existing_collapsed = counts.pop(COLLAPSED_INVENTORY_LABEL, 0)
    if len(counts) + bool(existing_collapsed) > MAX_INVENTORY_LABELS:
        ordered = sorted(counts)
        retained = ordered[: MAX_INVENTORY_LABELS - 1]
        collapsed = existing_collapsed + sum(
            counts[label] for label in ordered[MAX_INVENTORY_LABELS - 1 :]
        )
        counts = {label: counts[label] for label in retained}
        counts[COLLAPSED_INVENTORY_LABEL] = collapsed
    elif existing_collapsed:
        counts[COLLAPSED_INVENTORY_LABEL] = existing_collapsed
    return dict(sorted(counts.items()))


def process_name(proc: Path) -> str:
    try:
        return bounded_process_label((proc / "comm").read_text(encoding="utf-8"))
    except (OSError, UnicodeError):
        return "<unreadable>"


def classify_file_descriptor(fd: Path) -> str:
    """Classify an fd without retaining its potentially sensitive target."""
    try:
        target = os.readlink(fd)
    except OSError:
        return "unreadable"
    if target.startswith("socket:["):
        return "socket"
    if target.startswith("pipe:["):
        return "pipe"
    if target.startswith("anon_inode:"):
        for name in ("eventpoll", "eventfd", "timerfd", "signalfd", "inotify", "pidfd"):
            if name in target:
                return f"anon-{name}"
        return "anon-other"
    if target.startswith(("memfd:", "/memfd:")):
        return "memory-file"
    if target.startswith("/dev/"):
        return "device-path"
    if target.startswith("/"):
        return "filesystem"
    return "other"


def process_inventory(
    proc_root: Path,
    tasks: list[Path],
    fds: list[Path],
    children: list[int],
) -> dict[str, dict[str, int]]:
    return {
        "fd_kinds": counted_labels([classify_file_descriptor(fd) for fd in fds]),
        "thread_names": counted_labels([process_name(task) for task in tasks]),
        "child_names": counted_labels(
            [process_name(proc_root / str(child)) for child in children]
        ),
    }


def process_snapshot(pid: int, started: float) -> dict[str, Any]:
    proc_root = Path("/proc")
    proc = proc_root / str(pid)
    status = (proc / "status").read_text(encoding="utf-8")
    values = {
        line.split(":", 1)[0]: line.split(":", 1)[1].strip()
        for line in status.splitlines()
        if ":" in line
    }
    rss_kib = float(values["VmRSS"].split()[0])
    tasks = process_tasks(proc)
    fd_entries = sorted((proc / "fd").iterdir(), key=lambda path: int(path.name))
    children = process_children_from_tasks(tasks)
    stat_fields = (proc / "stat").read_text().split()
    return {
        "elapsed_seconds": time.monotonic() - started,
        "rss_kib": rss_kib,
        "threads": len(tasks),
        "fds": len(fd_entries),
        "children": children,
        "cpu_ticks": int(stat_fields[13]) + int(stat_fields[14]),
        "inventory": process_inventory(proc_root, tasks, fd_entries, children),
    }


def edge_growth(samples: list[dict[str, Any]], field: str) -> float:
    width = max(1, len(samples) // 4)
    first = statistics.median(float(sample[field]) for sample in samples[:width])
    last = statistics.median(float(sample[field]) for sample in samples[-width:])
    return last - first


def minimum_sample_count(duration_seconds: float, sample_interval: float) -> int:
    """Require at least half of the nominal cadence and two growth endpoints."""
    if duration_seconds <= 0 or sample_interval <= 0:
        raise ValueError("duration and sample interval must be positive")
    return max(2, int((duration_seconds / sample_interval) * 0.5))


def inventory_coverage_check(samples: list[dict[str, Any]]) -> dict[str, Any]:
    complete = 0
    for sample in samples:
        inventory = sample.get("inventory")
        if not isinstance(inventory, dict):
            continue
        valid = True
        for dimension, aggregate in INVENTORY_DIMENSIONS:
            values = inventory.get(dimension)
            expected = (
                len(sample.get(aggregate, []))
                if aggregate == "children" and isinstance(sample.get(aggregate), list)
                else sample.get(aggregate)
            )
            if (
                not isinstance(values, dict)
                or len(values) > MAX_INVENTORY_LABELS
                or not isinstance(expected, int)
                or isinstance(expected, bool)
                or any(
                    not isinstance(label, str)
                    or not label
                    or len(label) > MAX_PROCESS_LABEL_CHARACTERS
                    or label != bounded_process_label(label)
                    or (dimension == "fd_kinds" and label not in FILE_DESCRIPTOR_KINDS)
                    or not isinstance(count, int)
                    or isinstance(count, bool)
                    or count < 0
                    for label, count in values.items()
                )
                or sum(values.values()) != expected
            ):
                valid = False
                break
        if valid:
            complete += 1
    return {
        "name": "resource inventory coverage",
        "actual": complete,
        "min": len(samples),
        "passed": bool(samples) and complete == len(samples),
    }


def summarize_resource_inventories(
    samples: list[dict[str, Any]],
) -> dict[str, dict[str, dict[str, float]]]:
    if not samples:
        return {dimension: {} for dimension, _ in INVENTORY_DIMENSIONS}
    width = max(1, len(samples) // 4)
    summary: dict[str, dict[str, dict[str, float]]] = {}
    for dimension, _ in INVENTORY_DIMENSIONS:
        valid_values = []
        for sample in samples:
            inventory = sample.get("inventory")
            values = inventory.get(dimension) if isinstance(inventory, dict) else None
            valid_values.append(
                {
                    label: count
                    for label, count in values.items()
                    if isinstance(values, dict)
                    and isinstance(label, str)
                    and label == bounded_process_label(label)
                    and isinstance(count, int)
                    and not isinstance(count, bool)
                    and count >= 0
                }
                if isinstance(values, dict)
                else {}
            )
        observed_labels = set(
            {
                label
                for values in valid_values
                for label in values
            }
        )
        non_collapsed = sorted(observed_labels - {COLLAPSED_INVENTORY_LABEL})
        if len(observed_labels) <= MAX_INVENTORY_LABELS:
            labels = non_collapsed
            collapsed_labels: set[str] = set()
            if COLLAPSED_INVENTORY_LABEL in observed_labels:
                labels.append(COLLAPSED_INVENTORY_LABEL)
        else:
            labels = non_collapsed[: MAX_INVENTORY_LABELS - 1]
            collapsed_labels = set(non_collapsed[MAX_INVENTORY_LABELS - 1 :])
            labels.append(COLLAPSED_INVENTORY_LABEL)
        dimension_summary = {}
        for label in labels:
            if label == COLLAPSED_INVENTORY_LABEL:
                values = [
                    float(
                        sample_values.get(COLLAPSED_INVENTORY_LABEL, 0)
                        + sum(sample_values.get(item, 0) for item in collapsed_labels)
                    )
                    for sample_values in valid_values
                ]
            else:
                values = [
                    float(sample_values.get(label, 0))
                    for sample_values in valid_values
                ]
            first = statistics.median(values[:width])
            last = statistics.median(values[-width:])
            dimension_summary[label] = {
                "first_quartile_median": first,
                "last_quartile_median": last,
                "growth": last - first,
                "peak": max(values),
            }
        summary[dimension] = dimension_summary
    return summary


def evaluate_resource_ceilings(
    samples: list[dict[str, Any]],
    *,
    max_rss_kib: float,
    max_rss_growth_kib: float,
    max_thread_growth: float,
    max_fd_growth: float,
    max_children: int,
    minimum_samples: int = 2,
) -> list[dict[str, Any]]:
    sample_check = {
        "name": "resource sample count",
        "actual": len(samples),
        "min": minimum_samples,
        "passed": len(samples) >= minimum_samples,
    }
    inventory_check = inventory_coverage_check(samples)
    if not samples:
        return [sample_check, inventory_check]
    checks = [
        sample_check,
        inventory_check,
        {
            "name": "maximum process RSS",
            "actual": max(sample["rss_kib"] for sample in samples),
            "max": max_rss_kib,
        },
        {
            "name": "first-to-last RSS growth",
            "actual": edge_growth(samples, "rss_kib"),
            "max": max_rss_growth_kib,
        },
        {
            "name": "first-to-last thread growth",
            "actual": edge_growth(samples, "threads"),
            "max": max_thread_growth,
        },
        {
            "name": "first-to-last fd growth",
            "actual": edge_growth(samples, "fds"),
            "max": max_fd_growth,
        },
        {
            "name": "maximum concurrent child processes",
            "actual": max(len(sample["children"]) for sample in samples),
            "max": max_children,
        },
    ]
    return [
        check
        if "passed" in check
        else {**check, "passed": check["actual"] <= check["max"]}
        for check in checks
    ]


def evaluate_progress_ceilings(
    progress: list[dict[str, Any]],
    *,
    max_transcript_queue_bytes: int,
) -> list[dict[str, Any]]:
    """Validate real per-iteration backlog and durable queue drainage."""
    invalid_records = sum("invalid_progress_record" in item for item in progress)
    workload = [item for item in progress if "iteration" in item]
    fields = (
        "transcript_queue_bytes",
        "transcript_queue_bytes_after_finish",
        "transcript_entries_accepted",
        "transcript_entries_dropped",
    )
    complete = [
        item
        for item in workload
        if all(
            isinstance(item.get(field), int)
            and not isinstance(item.get(field), bool)
            and item[field] >= 0
            for field in fields
        )
    ]
    backlog = [item["transcript_queue_bytes"] for item in complete]
    drained = [item["transcript_queue_bytes_after_finish"] for item in complete]
    accepted = [item["transcript_entries_accepted"] for item in complete]
    dropped = [item["transcript_entries_dropped"] for item in complete]
    return [
        {
            "name": "malformed scenario progress records",
            "actual": invalid_records,
            "max": 0,
            "passed": invalid_records == 0,
        },
        {
            "name": "scenario workload progress records",
            "actual": len(workload),
            "min": 1,
            "passed": bool(workload),
        },
        {
            "name": "transcript queue metric coverage",
            "actual": len(complete),
            "min": len(workload),
            "passed": bool(workload) and len(complete) == len(workload),
        },
        {
            "name": "maximum transcript queue backlog",
            "actual": max(backlog) if backlog else None,
            "max": max_transcript_queue_bytes,
            "passed": bool(backlog) and max(backlog) <= max_transcript_queue_bytes,
        },
        {
            "name": "transcript queue drained after every iteration",
            "actual": max(drained) if drained else None,
            "max": 0,
            "passed": bool(drained) and max(drained) == 0,
        },
        {
            "name": "transcript workload entries accepted",
            "actual": min(accepted) if accepted else None,
            "min": TRANSCRIPT_ENTRIES_PER_ITERATION,
            "passed": bool(accepted)
            and min(accepted) >= TRANSCRIPT_ENTRIES_PER_ITERATION,
        },
        {
            "name": "transcript entries dropped",
            "actual": sum(dropped) if dropped else None,
            "max": 0,
            "passed": bool(dropped) and sum(dropped) == 0,
        },
    ]


def evaluate_outcome_checks(
    *,
    gpu_smoke_requested: bool,
    gpu_exit_status: int | None,
    scenario_exit_status: int,
    completion_markers: int,
    remaining_observed_children: int,
    gpu_error_count: int,
) -> list[dict[str, Any]]:
    """Build checks from the terminal outcomes retained in the record."""
    return [
        {
            "name": "headless GPU smoke exit status",
            "actual": gpu_exit_status if gpu_smoke_requested else "not requested",
            "max": 0 if gpu_smoke_requested else "not requested",
            "passed": not gpu_smoke_requested or gpu_exit_status == 0,
        },
        {
            "name": "scenario exit status",
            "actual": scenario_exit_status,
            "max": 0,
            "passed": scenario_exit_status == 0,
        },
        {
            "name": "scenario emitted completion marker",
            "actual": completion_markers,
            "max": 1,
            "passed": completion_markers == 1,
        },
        {
            "name": "child processes cleaned up after scenario",
            "actual": remaining_observed_children,
            "max": 0,
            "passed": remaining_observed_children == 0,
        },
        {
            "name": "GPU validation/device errors",
            "actual": gpu_error_count,
            "max": 0,
            "passed": gpu_error_count == 0,
        },
    ]


def build_core_soak_checks(
    samples: list[dict[str, Any]],
    progress: list[dict[str, Any]],
    thresholds: dict[str, Any],
    outcomes: dict[str, Any],
    *,
    gpu_smoke_requested: bool,
) -> list[dict[str, Any]]:
    """Reconstruct every core-soak check from retained raw inputs."""
    checks = evaluate_resource_ceilings(
        samples,
        max_rss_kib=thresholds["max_rss_kib"],
        max_rss_growth_kib=thresholds["max_rss_growth_kib"],
        max_thread_growth=thresholds["max_thread_growth"],
        max_fd_growth=thresholds["max_fd_growth"],
        max_children=thresholds["max_children"],
        minimum_samples=thresholds["minimum_resource_samples"],
    )
    checks.extend(
        evaluate_progress_ceilings(
            progress,
            max_transcript_queue_bytes=thresholds["max_transcript_queue_bytes"],
        )
    )
    checks.extend(
        evaluate_outcome_checks(
            gpu_smoke_requested=gpu_smoke_requested,
            gpu_exit_status=outcomes["gpu_exit_status"],
            scenario_exit_status=outcomes["scenario_exit_status"],
            completion_markers=outcomes["completion_markers"],
            remaining_observed_children=outcomes["remaining_observed_children"],
            gpu_error_count=outcomes["gpu_error_count"],
        )
    )
    return checks


def parse_progress(log_path: Path) -> list[dict[str, Any]]:
    progress = []
    for line in log_path.read_text(encoding="utf-8", errors="replace").splitlines():
        marker = line.find(SOAK_PREFIX)
        if marker >= 0:
            try:
                record = json.loads(line[marker + len(SOAK_PREFIX) :])
            except json.JSONDecodeError:
                record = {"invalid_progress_record": "invalid_json"}
            if not isinstance(record, dict):
                record = {"invalid_progress_record": "non_object"}
            progress.append(record)
    return progress


def child_still_exists(pid: int) -> bool:
    return (Path("/proc") / str(pid)).exists()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--duration-seconds", type=int, default=30)
    parser.add_argument("--sample-interval", type=float, default=0.5)
    parser.add_argument("--output", type=Path, default=ROOT / "target/soak/results.json")
    parser.add_argument("--log", type=Path, default=ROOT / "target/soak/scenario.log")
    parser.add_argument("--max-rss-mib", type=float, default=512.0)
    parser.add_argument("--max-rss-growth-mib", type=float, default=128.0)
    parser.add_argument("--max-thread-growth", type=float, default=4.0)
    parser.add_argument("--max-fd-growth", type=float, default=8.0)
    parser.add_argument("--max-children", type=int, default=16)
    parser.add_argument("--max-transcript-queue-mib", type=float, default=2.0)
    parser.add_argument("--gpu-smoke", action="store_true")
    args = parser.parse_args()
    if os.name != "posix" or not Path("/proc/self/status").exists():
        raise SystemExit("run_soak.py requires Linux /proc")
    if (
        args.duration_seconds < 1
        or args.sample_interval <= 0
        or args.max_transcript_queue_mib <= 0
        or args.max_rss_mib <= 0
        or args.max_rss_growth_mib < 0
        or args.max_thread_growth < 0
        or args.max_fd_growth < 0
        or args.max_children < 0
    ):
        parser.error(
            "duration and absolute ceilings must be positive; "
            "growth ceilings must be non-negative"
        )

    output_path = args.output.resolve()
    log_path = args.log.resolve()
    if output_path == log_path:
        parser.error("the report and scenario log must use different paths")
    try:
        log_path.relative_to(output_path.parent)
    except ValueError:
        parser.error("the scenario log must be inside the report directory")

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.log.parent.mkdir(parents=True, exist_ok=True)
    binary = build_test_binary()
    environment = os.environ.copy()
    environment["TERMERIC_RUN_SOAK"] = "1"
    environment["TERMERIC_SOAK_DURATION_SECONDS"] = str(args.duration_seconds)
    environment.setdefault("RUST_LOG", "warn")

    gpu_exit_status: int | None = None
    with args.log.open("w", encoding="utf-8") as log:
        if args.gpu_smoke:
            gpu_environment = environment.copy()
            gpu_environment["TERMERIC_GPU_SMOKE"] = "1"
            gpu = subprocess.run(
                [
                    binary,
                    "test_support::linux_headless_gpu_render_smoke_when_requested",
                    "--exact",
                    "--nocapture",
                ],
                cwd=ROOT,
                env=gpu_environment,
                stdout=log,
                stderr=subprocess.STDOUT,
            )
            gpu_exit_status = gpu.returncode

        process = subprocess.Popen(
            [
                binary,
                "test_support::scheduled_resource_soak_scenario",
                "--exact",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ],
            cwd=ROOT,
            env=environment,
            stdout=log,
            stderr=subprocess.STDOUT,
        )
        started = time.monotonic()
        samples: list[dict[str, Any]] = []
        observed_children: set[int] = set()
        timeout = args.duration_seconds + 30
        while process.poll() is None and time.monotonic() - started < timeout:
            try:
                snapshot = process_snapshot(process.pid, started)
            except (FileNotFoundError, ProcessLookupError):
                break
            samples.append(snapshot)
            observed_children.update(snapshot["children"])
            time.sleep(args.sample_interval)
        if process.poll() is None:
            process.kill()
            process.wait(timeout=5)
        exit_status = process.returncode

    progress = parse_progress(args.log)
    minimum_samples = minimum_sample_count(args.duration_seconds, args.sample_interval)
    max_transcript_queue_bytes = int(args.max_transcript_queue_mib * 1024 * 1024)
    log_text = args.log.read_text(encoding="utf-8", errors="replace").lower()
    gpu_errors = sum(
        log_text.count(pattern)
        for pattern in ["validation error", "device lost", "device removed"]
    )
    thresholds = {
        "max_rss_kib": args.max_rss_mib * 1024,
        "max_rss_growth_kib": args.max_rss_growth_mib * 1024,
        "max_thread_growth": args.max_thread_growth,
        "max_fd_growth": args.max_fd_growth,
        "max_children": args.max_children,
        "max_transcript_queue_bytes": max_transcript_queue_bytes,
        "minimum_resource_samples": minimum_samples,
        "transcript_entries_per_iteration": TRANSCRIPT_ENTRIES_PER_ITERATION,
        "max_inventory_labels_per_dimension": MAX_INVENTORY_LABELS,
        "max_process_label_characters": MAX_PROCESS_LABEL_CHARACTERS,
    }
    outcomes = {
        "gpu_exit_status": gpu_exit_status,
        "scenario_exit_status": exit_status,
        "completion_markers": sum(1 for item in progress if item.get("complete") is True),
        "remaining_observed_children": sum(
            child_still_exists(pid) for pid in observed_children
        ),
        "gpu_error_count": gpu_errors,
    }
    checks = build_core_soak_checks(
        samples,
        progress,
        thresholds,
        outcomes,
        gpu_smoke_requested=args.gpu_smoke,
    )
    passed = all(check["passed"] for check in checks)
    commit = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=ROOT,
        text=True,
        stdout=subprocess.PIPE,
        check=True,
    ).stdout.strip()
    worktree_dirty = bool(
        subprocess.run(
            ["git", "status", "--porcelain=v1", "--untracked-files=all"],
            cwd=ROOT,
            text=True,
            stdout=subprocess.PIPE,
            check=True,
        ).stdout.strip()
    )
    candidate_eligible = passed and not worktree_dirty
    eligibility = (
        "candidate"
        if candidate_eligible
        else "dirty-dry-run"
        if passed
        else "failed"
    )
    binary_relative = relative_file_path(ROOT, binary, "test binary")
    log_relative = relative_file_path(args.output.parent, args.log, "scenario log")
    report = {
        "schema_version": CORE_SOAK_SCHEMA_VERSION,
        "kind": CORE_SOAK_RECORD_KIND,
        "status": "pass" if passed else "fail",
        "candidate_eligible": candidate_eligible,
        "eligibility": eligibility,
        "recorded_at_utc": datetime.now(timezone.utc).isoformat(),
        "environment": {
            "platform": platform.platform(),
            "python": sys.version.split()[0],
            "rustc": subprocess.run(
                ["rustc", "--version"],
                cwd=ROOT,
                text=True,
                stdout=subprocess.PIPE,
                check=True,
            ).stdout.strip(),
            "commit": commit,
            "worktree_dirty": worktree_dirty,
            "wgpu_backend": environment.get("WGPU_BACKEND"),
        },
        "duration_seconds": args.duration_seconds,
        "sample_interval_seconds": args.sample_interval,
        "minimum_resource_samples": minimum_samples,
        "maximum_transcript_queue_bytes": max_transcript_queue_bytes,
        "gpu_smoke_requested": args.gpu_smoke,
        "test_binary": {
            "path": binary_relative,
            "bytes": binary.stat().st_size,
            "sha256": file_sha256(binary),
        },
        "artifacts": {
            "scenario_log": {
                "path": log_relative,
                "bytes": args.log.stat().st_size,
                "sha256": file_sha256(args.log),
            }
        },
        "thresholds": thresholds,
        "outcomes": outcomes,
        "samples": samples,
        "progress": progress,
        "resource_inventory_summary": summarize_resource_inventories(samples),
        "checks": checks,
    }
    write_json_atomic(args.output, report)
    for check in checks:
        label = "PASS" if check["passed"] else "FAIL"
        boundary = check.get("max", check.get("min"))
        print(f"{label}: {check['name']} (actual={check['actual']}, boundary={boundary})")
    if not passed:
        raise SystemExit("resource soak ceilings failed")


if __name__ == "__main__":
    main()
