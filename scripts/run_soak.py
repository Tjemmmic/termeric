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
SOAK_PREFIX = "TERMERIC_SOAK "
TRANSCRIPT_ENTRIES_PER_ITERATION = 68


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


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


def process_children(proc: Path) -> list[int]:
    """Return direct children spawned by any thread in a Linux process."""
    children: set[int] = set()
    try:
        tasks = list((proc / "task").iterdir())
    except FileNotFoundError:
        return []
    for task in tasks:
        try:
            values = (task / "children").read_text().split()
        except FileNotFoundError:
            # Threads may exit while /proc is being sampled.
            continue
        children.update(int(value) for value in values)
    return sorted(children)


def process_snapshot(pid: int, started: float) -> dict[str, Any]:
    proc = Path("/proc") / str(pid)
    status = (proc / "status").read_text(encoding="utf-8")
    values = {
        line.split(":", 1)[0]: line.split(":", 1)[1].strip()
        for line in status.splitlines()
        if ":" in line
    }
    rss_kib = float(values["VmRSS"].split()[0])
    threads = int(values["Threads"])
    fds = len(list((proc / "fd").iterdir()))
    children = process_children(proc)
    stat = (proc / "stat").read_text().split()
    return {
        "elapsed_seconds": time.monotonic() - started,
        "rss_kib": rss_kib,
        "threads": threads,
        "fds": fds,
        "children": children,
        "cpu_ticks": int(stat[13]) + int(stat[14]),
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
    if not samples:
        return [sample_check]
    checks = [
        sample_check,
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
    ):
        parser.error("duration, sample interval, and queue ceiling must be positive")

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
    minimum_samples = minimum_sample_count(
        args.duration_seconds, args.sample_interval
    )
    checks = evaluate_resource_ceilings(
        samples,
        max_rss_kib=args.max_rss_mib * 1024,
        max_rss_growth_kib=args.max_rss_growth_mib * 1024,
        max_thread_growth=args.max_thread_growth,
        max_fd_growth=args.max_fd_growth,
        max_children=args.max_children,
        minimum_samples=minimum_samples,
    )
    max_transcript_queue_bytes = int(args.max_transcript_queue_mib * 1024 * 1024)
    checks.extend(
        evaluate_progress_ceilings(
            progress,
            max_transcript_queue_bytes=max_transcript_queue_bytes,
        )
    )
    checks.extend(
        [
            {
                "name": "headless GPU smoke exit status",
                "actual": gpu_exit_status if args.gpu_smoke else "not requested",
                "max": 0 if args.gpu_smoke else "not requested",
                "passed": not args.gpu_smoke or gpu_exit_status == 0,
            },
            {
                "name": "scenario exit status",
                "actual": exit_status,
                "max": 0,
                "passed": exit_status == 0,
            },
            {
                "name": "scenario emitted completion marker",
                "actual": sum(1 for item in progress if item.get("complete")),
                "max": 1,
                "passed": any(item.get("complete") for item in progress),
            },
            {
                "name": "child processes cleaned up after scenario",
                "actual": sum(child_still_exists(pid) for pid in observed_children),
                "max": 0,
                "passed": not any(child_still_exists(pid) for pid in observed_children),
            },
        ]
    )
    log_text = args.log.read_text(encoding="utf-8", errors="replace").lower()
    gpu_errors = sum(
        log_text.count(pattern)
        for pattern in ["validation error", "device lost", "device removed"]
    )
    checks.append(
        {
            "name": "GPU validation/device errors",
            "actual": gpu_errors,
            "max": 0,
            "passed": gpu_errors == 0,
        }
    )
    report = {
        "schema_version": 1,
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
            "commit": subprocess.run(
                ["git", "rev-parse", "HEAD"],
                cwd=ROOT,
                text=True,
                stdout=subprocess.PIPE,
                check=True,
            ).stdout.strip(),
            "worktree_dirty": bool(
                subprocess.run(
                    ["git", "status", "--porcelain"],
                    cwd=ROOT,
                    text=True,
                    stdout=subprocess.PIPE,
                    check=True,
                ).stdout.strip()
            ),
            "wgpu_backend": environment.get("WGPU_BACKEND"),
        },
        "duration_seconds": args.duration_seconds,
        "sample_interval_seconds": args.sample_interval,
        "minimum_resource_samples": minimum_samples,
        "maximum_transcript_queue_bytes": max_transcript_queue_bytes,
        "gpu_smoke_requested": args.gpu_smoke,
        "test_binary": str(binary),
        "test_binary_sha256": file_sha256(binary),
        "thresholds": {
            "max_rss_kib": args.max_rss_mib * 1024,
            "max_rss_growth_kib": args.max_rss_growth_mib * 1024,
            "max_thread_growth": args.max_thread_growth,
            "max_fd_growth": args.max_fd_growth,
            "max_children": args.max_children,
            "max_transcript_queue_bytes": max_transcript_queue_bytes,
            "minimum_resource_samples": minimum_samples,
            "transcript_entries_per_iteration": TRANSCRIPT_ENTRIES_PER_ITERATION,
        },
        "samples": samples,
        "progress": progress,
        "checks": checks,
    }
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    for check in checks:
        label = "PASS" if check["passed"] else "FAIL"
        boundary = check.get("max", check.get("min"))
        print(f"{label}: {check['name']} (actual={check['actual']}, boundary={boundary})")
    if not all(check["passed"] for check in checks):
        raise SystemExit("resource soak ceilings failed")


if __name__ == "__main__":
    main()
