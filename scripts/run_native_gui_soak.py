#!/usr/bin/env python3
"""Run a displayed Wayland Termeric soak and enforce process-growth ceilings."""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import time
from typing import Any, Callable

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from scripts.gui_acceptance import (
    AcceptanceError,
    AppProcess,
    assert_no_runtime_errors,
    cleanup_private_runtime,
    file_sha256,
    pids_receiving_token,
    read_records,
    run,
    sway_environment,
    transcript_bytes,
    wait_until,
)
from scripts.run_soak import (
    MAX_INVENTORY_LABELS,
    MAX_PROCESS_LABEL_CHARACTERS,
    child_still_exists,
    evaluate_resource_ceilings,
    minimum_sample_count,
    process_snapshot,
    relative_file_path,
    summarize_resource_inventories,
    write_json_atomic,
)


FIXTURE = ROOT / "termeric" / "tests" / "gui" / "terminal_fixture.py"
NATIVE_SOAK_SCHEMA_VERSION = 4
NATIVE_SOAK_RECORD_KIND = "termeric-native-resource-soak"
MAX_NATIVE_SOAK_ARTIFACTS = 512
MAX_NATIVE_SOAK_ARTIFACT_BYTES = 128 * 1024 * 1024
MAX_NATIVE_SOAK_TOTAL_ARTIFACT_BYTES = 256 * 1024 * 1024
MAX_DRIVER_ERROR_CHARACTERS = 1024


def fixture_arguments(transcript: Path, instance: int) -> list[str]:
    return [
        str(FIXTURE),
        "--transcript",
        str(transcript),
        "--instance",
        str(instance),
        "--soak-output",
        "--skip-external-graphics",
    ]


def progress_count(records: list[dict[str, object]]) -> int:
    return sum(record.get("event") == "soak_progress" for record in records)


def kitty_image_ack_count(transcript: Path) -> int:
    return transcript_bytes(transcript).count(b"\x1b_Gi=91;OK\x1b\\")


def runtime_error_count(log_text: str) -> int:
    lowered = log_text.casefold()
    return sum(
        lowered.count(pattern)
        for pattern in (
            "panicked at",
            "validation error",
            "outofmemory",
            "out of gpu memory",
            "device lost",
            "device removed",
            "renderer recovery failed",
        )
    )


def bounded_driver_error(error: object) -> str:
    sanitized = "".join(
        character if character.isprintable() else "?" for character in str(error)
    )
    text = " ".join(sanitized.split())
    return text[:MAX_DRIVER_ERROR_CHARACTERS] or "unspecified native soak error"


def prepare_artifact_directory(path: Path) -> None:
    if path.is_symlink():
        raise AcceptanceError(f"artifact directory must not be a symlink: {path}")
    if path.exists():
        if not path.is_dir():
            raise AcceptanceError(f"artifact destination is not a directory: {path}")
        if any(path.iterdir()):
            raise AcceptanceError(
                f"refusing to reuse non-empty artifact directory: {path}"
            )
        return
    path.mkdir(parents=True)


def native_artifact_manifest(root: Path) -> list[dict[str, object]]:
    entries: list[dict[str, object]] = []
    total_bytes = 0
    for path in sorted(root.rglob("*"), key=lambda item: item.relative_to(root).as_posix()):
        relative = path.relative_to(root).as_posix()
        if path.is_symlink():
            raise AcceptanceError(f"native soak artifact is a symlink: {relative}")
        if path.is_dir():
            continue
        if not path.is_file():
            raise AcceptanceError(f"native soak artifact is not a regular file: {relative}")
        if relative == "results.json":
            continue
        size = path.stat().st_size
        if size > MAX_NATIVE_SOAK_ARTIFACT_BYTES:
            raise AcceptanceError(f"native soak artifact exceeds its byte limit: {relative}")
        total_bytes += size
        if total_bytes > MAX_NATIVE_SOAK_TOTAL_ARTIFACT_BYTES:
            raise AcceptanceError("native soak artifact bundle exceeds its byte limit")
        entries.append(
            {"path": relative, "bytes": size, "sha256": file_sha256(path)}
        )
        if len(entries) > MAX_NATIVE_SOAK_ARTIFACTS:
            raise AcceptanceError("native soak artifact bundle has too many files")
    return entries


def git_provenance() -> dict[str, object]:
    commit = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=ROOT,
        text=True,
        stdout=subprocess.PIPE,
        check=True,
    ).stdout.strip()
    dirty = bool(
        subprocess.run(
            ["git", "status", "--porcelain=v1", "--untracked-files=all"],
            cwd=ROOT,
            text=True,
            stdout=subprocess.PIPE,
            check=True,
        ).stdout.strip()
    )
    return {"commit": commit, "worktree_dirty": dirty}


def require_clean_candidate(provenance: dict[str, object], required: bool) -> None:
    if required and provenance["worktree_dirty"] is not False:
        raise AcceptanceError(
            "refusing clean-candidate native soak from a dirty worktree"
        )


def perform_until(
    description: str,
    action: Callable[[], None],
    predicate: Callable[[], bool],
    *,
    attempts: int = 3,
    attempt_timeout: float = 2,
) -> None:
    for attempt in range(attempts):
        action()
        try:
            wait_until(description, predicate, attempt_timeout)
            return
        except AcceptanceError:
            if attempt + 1 == attempts:
                raise AcceptanceError(
                    f"timed out waiting for {description} after {attempts} attempts"
                )


def evaluate_native_checks(
    samples: list[dict[str, Any]],
    *,
    max_rss_kib: float,
    max_rss_growth_kib: float,
    max_thread_growth: float,
    max_fd_growth: float,
    max_children: int,
    minimum_samples: int,
    exit_status: int | None,
    progress_records: int,
    minimum_progress: int,
    resize_actions: int,
    minimum_resize_actions: int,
    pane_actions: int,
    minimum_pane_actions: int,
    graphics_acks: int,
    minimum_graphics_acks: int,
    remaining_children: int,
    runtime_errors: int,
) -> list[dict[str, Any]]:
    checks = evaluate_resource_ceilings(
        samples,
        max_rss_kib=max_rss_kib,
        max_rss_growth_kib=max_rss_growth_kib,
        max_thread_growth=max_thread_growth,
        max_fd_growth=max_fd_growth,
        max_children=max_children,
        minimum_samples=minimum_samples,
    )
    checks.extend(
        [
            {
                "name": "native application exit status",
                "actual": exit_status,
                "max": 0,
                "passed": exit_status == 0,
            },
            {
                "name": "continuous PTY progress records",
                "actual": progress_records,
                "min": minimum_progress,
                "passed": progress_records >= minimum_progress,
            },
            {
                "name": "native output resize actions",
                "actual": resize_actions,
                "min": minimum_resize_actions,
                "passed": resize_actions >= minimum_resize_actions,
            },
            {
                "name": "native pane-churn actions",
                "actual": pane_actions,
                "min": minimum_pane_actions,
                "passed": pane_actions >= minimum_pane_actions,
            },
            {
                "name": "accepted Kitty image replacements",
                "actual": graphics_acks,
                "min": minimum_graphics_acks,
                "passed": graphics_acks >= minimum_graphics_acks,
            },
            {
                "name": "observed child processes cleaned up",
                "actual": remaining_children,
                "max": 0,
                "passed": remaining_children == 0,
            },
            {
                "name": "native renderer/runtime errors",
                "actual": runtime_errors,
                "max": 0,
                "passed": runtime_errors == 0,
            },
        ]
    )
    return checks


def build_native_soak_checks(
    samples: list[dict[str, Any]],
    thresholds: dict[str, Any],
    outcomes: dict[str, Any],
) -> list[dict[str, Any]]:
    checks = evaluate_native_checks(
        samples,
        max_rss_kib=thresholds["max_rss_kib"],
        max_rss_growth_kib=thresholds["max_rss_growth_kib"],
        max_thread_growth=thresholds["max_thread_growth"],
        max_fd_growth=thresholds["max_fd_growth"],
        max_children=thresholds["max_children"],
        minimum_samples=thresholds["minimum_resource_samples"],
        exit_status=outcomes["exit_status"],
        progress_records=outcomes["progress_records"],
        minimum_progress=thresholds["minimum_progress_records"],
        resize_actions=outcomes["resize_actions"],
        minimum_resize_actions=thresholds["minimum_resize_actions"],
        pane_actions=outcomes["pane_actions"],
        minimum_pane_actions=thresholds["minimum_pane_actions"],
        graphics_acks=outcomes["graphics_acknowledgements"],
        minimum_graphics_acks=thresholds["minimum_graphics_acknowledgements"],
        remaining_children=outcomes["remaining_children"],
        runtime_errors=outcomes["runtime_errors"],
    )
    driver_error = outcomes["driver_error"]
    checks.append(
        {
            "name": "native soak driver completed",
            "actual": driver_error if driver_error is not None else "complete",
            "max": "no error" if driver_error is not None else "complete",
            "passed": driver_error is None,
        }
    )
    return checks


def close_sway(process: subprocess.Popen[str] | None) -> None:
    if process is None:
        return
    process.terminate()
    try:
        process.wait(timeout=3)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=3)
    log = getattr(process, "_termeric_log", None)
    if log is not None:
        log.close()
    cleanup_private_runtime(process)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/termeric")
    parser.add_argument("--duration-seconds", type=int, default=30)
    parser.add_argument("--sample-interval", type=float, default=0.5)
    parser.add_argument("--action-interval", type=float, default=2.0)
    parser.add_argument(
        "--artifacts", type=Path, default=ROOT / "target/native-gui-soak"
    )
    parser.add_argument("--max-rss-mib", type=float, default=768.0)
    parser.add_argument("--max-rss-growth-mib", type=float, default=192.0)
    parser.add_argument("--max-thread-growth", type=float, default=8.0)
    parser.add_argument("--max-fd-growth", type=float, default=16.0)
    parser.add_argument("--max-children", type=int, default=8)
    parser.add_argument(
        "--require-clean",
        action="store_true",
        help="refuse to start unless the Git worktree is clean",
    )
    args = parser.parse_args()
    if os.name != "posix" or not Path("/proc/self/status").is_file():
        parser.error("native GUI soak requires Linux /proc")
    if (
        args.duration_seconds < 5
        or args.sample_interval <= 0
        or args.action_interval <= 0
        or args.max_rss_mib <= 0
        or args.max_rss_growth_mib < 0
        or args.max_thread_growth < 0
        or args.max_fd_growth < 0
        or args.max_children < 0
    ):
        parser.error(
            "duration must be at least five seconds, intervals and absolute ceilings "
            "must be positive, and growth ceilings must be non-negative"
        )
    binary = args.binary.resolve()
    if args.binary.is_symlink() or not binary.is_file():
        parser.error(f"Termeric binary does not exist: {binary}")
    try:
        binary_relative = relative_file_path(ROOT, binary, "Termeric binary")
    except ValueError as error:
        parser.error(str(error))
    for tool in ("sway", "swaymsg", "tic", "wtype"):
        if not shutil.which(tool):
            parser.error(f"required native soak tool is unavailable: {tool}")
    provenance = git_provenance()
    try:
        require_clean_candidate(provenance, args.require_clean)
    except AcceptanceError as error:
        parser.error(str(error))

    if args.artifacts.is_symlink():
        parser.error("native soak artifact directory must not be a symlink")
    artifacts = args.artifacts.resolve()
    try:
        prepare_artifact_directory(artifacts)
    except AcceptanceError as error:
        parser.error(str(error))
    transcript = artifacts / "pty-transcript.jsonl"
    environment = dict(os.environ)
    environment.setdefault("WGPU_BACKEND", "gl")
    terminfo = artifacts / "terminfo"
    terminfo.mkdir(parents=True, exist_ok=True)
    run(
        [
            "tic",
            "-x",
            "-o",
            str(terminfo),
            str(ROOT / "assets/terminfo/xterm-termeric.terminfo"),
        ],
        env=environment,
    )
    environment["TERMERIC_TERM"] = "auto"
    environment["TERMERIC_TERMINFO_DIR"] = str(terminfo)

    sway: subprocess.Popen[str] | None = None
    app: AppProcess | None = None
    samples: list[dict[str, Any]] = []
    observed_children: set[int] = set()
    resize_actions = 0
    pane_actions = 0
    split_open = False
    action_generation = 0
    failure: str | None = None
    try:
        environment, sway = sway_environment(environment, artifacts)
        app = AppProcess(
            binary,
            artifacts / "application",
            "wayland",
            sys.executable,
            fixture_arguments(transcript, 230),
            environment,
        )
        app.wait_ready("wayland")
        wait_until(
            "native soak fixture startup",
            lambda: any(
                record.get("event") == "started"
                for record in read_records(transcript)
            ),
        )
        started = time.monotonic()
        next_sample = started
        next_action = started + args.action_interval
        while time.monotonic() - started < args.duration_seconds:
            now = time.monotonic()
            if app.process.poll() is not None:
                raise AcceptanceError(
                    f"Termeric exited during native soak ({app.process.returncode})"
                )
            if now >= next_sample:
                snapshot = process_snapshot(app.process.pid, started)
                samples.append(snapshot)
                observed_children.update(snapshot["children"])
                next_sample = now + args.sample_interval
            if now >= next_action:
                app.wayland_output_resize(
                    1000 if action_generation % 2 == 0 else 1024,
                    740 if action_generation % 2 == 0 else 768,
                )
                resize_actions += 1
                if split_open:
                    starts = [
                        record
                        for record in read_records(transcript)
                        if record.get("event") == "started"
                    ]
                    focused_pid = int(starts[-1]["pid"])
                    perform_until(
                        "native soak split cleanup",
                        lambda: app.wayland_chord(("alt", "shift"), "w"),
                        lambda: not Path(f"/proc/{focused_pid}").exists(),
                    )
                    split_open = False
                else:
                    starts_before = sum(
                        record.get("event") == "started"
                        for record in read_records(transcript)
                    )
                    perform_until(
                        "native soak split startup",
                        lambda: app.wayland_chord(("alt", "shift"), "Right"),
                        lambda: sum(
                            record.get("event") == "started"
                            for record in read_records(transcript)
                        )
                        > starts_before,
                    )
                    token = f"NATIVE_SOAK_ROUTE_{action_generation}".encode()
                    focused_pid = int(
                        [
                            record
                            for record in read_records(transcript)
                            if record.get("event") == "started"
                        ][-1]["pid"]
                    )
                    perform_until(
                        "native soak exact pane route",
                        lambda: app.wayland_type(token.decode()),
                        lambda: pids_receiving_token(read_records(transcript), token)
                        == {focused_pid},
                    )
                    split_open = True
                pane_actions += 1
                action_generation += 1
                next_action = time.monotonic() + args.action_interval
            time.sleep(min(0.05, args.sample_interval))

        app.close_normally("wayland")
        assert_no_runtime_errors(app)
    except Exception as error:
        failure = bounded_driver_error(error)
        if app is not None:
            app.stop_after_failure()
    finally:
        exit_status = app.process.returncode if app is not None else None
        if app is not None:
            observed_children.update(
                int(record["pid"])
                for record in read_records(transcript)
                if record.get("event") == "started"
            )
            app.finish()
        close_sway(sway)

    cleanup_deadline = time.monotonic() + 5
    while (
        any(child_still_exists(pid) for pid in observed_children)
        and time.monotonic() < cleanup_deadline
    ):
        time.sleep(0.05)
    remaining_children = sum(child_still_exists(pid) for pid in observed_children)
    records = read_records(transcript)
    application_log = artifacts / "application/termeric.log"
    log_text = (
        application_log.read_text(encoding="utf-8", errors="replace")
        if application_log.is_file()
        else ""
    )
    minimum_progress = max(1, args.duration_seconds // 3)
    minimum_action_kind = max(
        1, args.duration_seconds // max(1, int(args.action_interval * 3))
    )
    minimum_graphics_acks = max(1, args.duration_seconds // 6)
    graphics_acks = kitty_image_ack_count(transcript)
    minimum_samples = minimum_sample_count(
        args.duration_seconds, args.sample_interval
    )
    thresholds = {
        "max_rss_kib": args.max_rss_mib * 1024,
        "max_rss_growth_kib": args.max_rss_growth_mib * 1024,
        "max_thread_growth": args.max_thread_growth,
        "max_fd_growth": args.max_fd_growth,
        "max_children": args.max_children,
        "minimum_resource_samples": minimum_samples,
        "minimum_progress_records": minimum_progress,
        "minimum_resize_actions": minimum_action_kind,
        "minimum_pane_actions": minimum_action_kind,
        "minimum_graphics_acknowledgements": minimum_graphics_acks,
        "max_inventory_labels_per_dimension": MAX_INVENTORY_LABELS,
        "max_process_label_characters": MAX_PROCESS_LABEL_CHARACTERS,
    }
    outcomes = {
        "exit_status": exit_status,
        "progress_records": progress_count(records),
        "resize_actions": resize_actions,
        "pane_actions": pane_actions,
        "graphics_acknowledgements": graphics_acks,
        "remaining_children": remaining_children,
        "runtime_errors": runtime_error_count(log_text),
        "driver_error": failure,
    }
    checks = build_native_soak_checks(samples, thresholds, outcomes)
    passed = all(check["passed"] for check in checks)
    candidate_eligible = passed and provenance["worktree_dirty"] is False
    eligibility = (
        "candidate"
        if candidate_eligible
        else "dirty-dry-run"
        if passed
        else "failed"
    )
    manifest = native_artifact_manifest(artifacts)
    report = {
        "schema": NATIVE_SOAK_SCHEMA_VERSION,
        "kind": NATIVE_SOAK_RECORD_KIND,
        "status": "pass" if passed else "fail",
        "candidate_eligible": candidate_eligible,
        "eligibility": eligibility,
        "recorded_at_utc": datetime.now(timezone.utc).isoformat(),
        "environment": {
            "platform": platform.platform(),
            "python": sys.version.split()[0],
            **provenance,
            "backend": "wayland",
            "compositor": "sway-headless",
            "wgpu_backend": environment.get("WGPU_BACKEND"),
        },
        "binary": {
            "path": binary_relative,
            "bytes": binary.stat().st_size,
            "sha256": file_sha256(binary),
        },
        "artifact_policy": {
            "max_files": MAX_NATIVE_SOAK_ARTIFACTS,
            "max_file_bytes": MAX_NATIVE_SOAK_ARTIFACT_BYTES,
            "max_total_bytes": MAX_NATIVE_SOAK_TOTAL_ARTIFACT_BYTES,
        },
        "artifacts": manifest,
        "duration_seconds": args.duration_seconds,
        "sample_interval_seconds": args.sample_interval,
        "action_interval_seconds": args.action_interval,
        "thresholds": thresholds,
        "outcomes": outcomes,
        "samples": samples,
        "resource_inventory_summary": summarize_resource_inventories(samples),
        "observed_children": sorted(observed_children),
        "checks": checks,
    }
    output = artifacts / "results.json"
    write_json_atomic(output, report)
    for check in checks:
        label = "PASS" if check["passed"] else "FAIL"
        boundary = check.get("max", check.get("min"))
        print(f"{label}: {check['name']} (actual={check['actual']}, boundary={boundary})")
    print(f"Artifacts: {artifacts}")
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
