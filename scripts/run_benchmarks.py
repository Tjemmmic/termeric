#!/usr/bin/env python3
"""Run Termeric release benchmarks, write JSON, and enforce stable budgets."""

from __future__ import annotations

import argparse
import json
import math
import os
import platform
import shutil
import statistics
import subprocess
import tempfile
import time
import tomllib
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
PREFIX = "TERMERIC_BENCH "
METRIC_CONTRACT = ROOT / "benchmarks" / "metrics.toml"


def command_output(command: list[str]) -> str:
    try:
        return subprocess.check_output(command, cwd=ROOT, text=True, stderr=subprocess.STDOUT).strip()
    except (OSError, subprocess.CalledProcessError) as error:
        return f"unavailable: {error}"


def percentile(samples: list[float], fraction: float) -> float:
    if not samples:
        raise ValueError("cannot calculate a percentile without samples")
    ordered = sorted(samples)
    return ordered[max(0, math.ceil(len(ordered) * fraction) - 1)]


def summarize(samples: list[float]) -> dict[str, float | int]:
    if not samples:
        return {"count": 0}
    mean = statistics.fmean(samples)
    deviation = statistics.pstdev(samples) if len(samples) > 1 else 0.0
    return {
        "count": len(samples),
        "min": min(samples),
        "median": statistics.median(samples),
        "p95": percentile(samples, 0.95),
        "max": max(samples),
        "mean": mean,
        "coefficient_of_variation": deviation / mean if mean else 0.0,
    }


def run_rust_suite(package: str, test_target: str | None = None) -> list[dict[str, Any]]:
    command = ["cargo", "test", "-p", package, "--release", "--locked"]
    if test_target:
        command.extend(["--test", test_target])
    command.extend(["release_benchmark_suite", "--", "--ignored", "--nocapture", "--test-threads=1"])
    completed = subprocess.run(command, cwd=ROOT, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    print(completed.stdout, end="")
    if completed.returncode:
        raise SystemExit(f"benchmark command failed with status {completed.returncode}: {' '.join(command)}")
    metrics = []
    for line in completed.stdout.splitlines():
        marker = line.find(PREFIX)
        if marker >= 0:
            metrics.append(json.loads(line[marker + len(PREFIX) :]))
    if not metrics:
        raise SystemExit(f"benchmark command emitted no metrics: {' '.join(command)}")
    return metrics


def process_ticks(pid: int) -> int:
    fields = (Path("/proc") / str(pid) / "stat").read_text(encoding="utf-8").split()
    return int(fields[13]) + int(fields[14])


def parse_proc_kib_fields(
    text: str,
    *,
    required: tuple[str, ...],
    optional: tuple[str, ...] = (),
) -> dict[str, float]:
    """Parse exact, unique Linux `/proc` fields whose unit is binary KiB."""
    expected = set(required) | set(optional)
    values: dict[str, float] = {}
    for raw_line in text.splitlines():
        label, separator, remainder = raw_line.partition(":")
        if not separator or label not in expected:
            continue
        if label in values:
            raise ValueError(f"duplicate /proc memory field {label}")
        parts = remainder.split()
        if len(parts) != 2 or not parts[0].isascii() or not parts[0].isdigit():
            raise ValueError(f"invalid /proc memory value for {label}")
        if parts[1] != "kB":
            raise ValueError(f"invalid /proc memory unit for {label}: {parts[1]!r}")
        values[label] = float(int(parts[0]))

    missing = [label for label in required if label not in values]
    if missing:
        raise ValueError(f"missing /proc memory field(s): {', '.join(missing)}")
    for label in optional:
        values.setdefault(label, 0.0)
    return values


def parse_process_rss_kib(status: str) -> float:
    return parse_proc_kib_fields(status, required=("VmRSS",))["VmRSS"]


def parse_smaps_rollup_kib(smaps_rollup: str) -> dict[str, float]:
    values = parse_proc_kib_fields(
        smaps_rollup,
        required=("Rss", "Pss", "Private_Clean", "Private_Dirty"),
        optional=("Private_Hugetlb",),
    )
    return {
        "rss_kib": values["Rss"],
        "pss_kib": values["Pss"],
        "private_kib": (
            values["Private_Clean"]
            + values["Private_Dirty"]
            + values["Private_Hugetlb"]
        ),
    }


def process_rss_kib(pid: int) -> float:
    status = (Path("/proc") / str(pid) / "status").read_text(encoding="utf-8")
    return parse_process_rss_kib(status)


def process_smaps_rollup_kib(pid: int) -> dict[str, float]:
    text = (Path("/proc") / str(pid) / "smaps_rollup").read_text(encoding="utf-8")
    return parse_smaps_rollup_kib(text)


def presentation_latency_samples(path: Path) -> list[float]:
    if not path.exists():
        return []
    return [float(line) for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]


def rendered_frame_count(path: Path) -> int:
    return int(path.read_text(encoding="utf-8").strip())


def isolate_display_backend(environment: dict[str, str]) -> str:
    """Keep the displayed probe on the session backend selected by its caller."""
    session_type = environment.get("XDG_SESSION_TYPE", "").lower()
    has_x11 = bool(environment.get("DISPLAY"))
    has_wayland = bool(environment.get("WAYLAND_DISPLAY"))

    if session_type == "x11" or (has_x11 and not has_wayland):
        environment["XDG_SESSION_TYPE"] = "x11"
        environment["EGL_PLATFORM"] = "x11"
        environment["WAYLAND_DISPLAY"] = ""
        environment["WAYLAND_SOCKET"] = ""
        environment["WINIT_UNIX_BACKEND"] = "x11"
        return "x11"

    if session_type == "wayland" or has_wayland:
        environment["XDG_SESSION_TYPE"] = "wayland"
        environment["EGL_PLATFORM"] = "wayland"
        environment["DISPLAY"] = ""
        environment["WINIT_UNIX_BACKEND"] = "wayland"
        return "wayland"

    raise RuntimeError("displayed benchmark could not select an X11 or Wayland backend")


def skipped_display_metrics(reason: str) -> list[dict[str, Any]]:
    return [
        {
            "name": name,
            "unit": unit,
            "samples": [],
            "workload": reason,
        }
        for name, unit in (
            ("idle.application_cpu", "percent_of_one_core"),
            ("idle.application_rendered_frames", "frames"),
            ("memory.application_idle_rss", "KiB"),
            ("memory.application_idle_pss", "KiB"),
            ("memory.application_idle_private", "KiB"),
            ("latency.displayed_key_event_to_present", "ms"),
        )
    ]


def display_memory_metrics(
    *,
    display_backend: str,
    rss_samples: list[float],
    pss_samples: list[float],
    private_samples: list[float],
    smaps_skip_reason: str | None,
) -> list[dict[str, Any]]:
    if display_backend not in ("x11", "wayland"):
        raise ValueError(f"unsupported displayed-memory backend {display_backend!r}")
    if not rss_samples:
        raise ValueError("displayed-memory rows require RSS samples")
    has_smaps_samples = bool(pss_samples) and bool(private_samples)
    if bool(pss_samples) != bool(private_samples):
        raise ValueError("PSS and private-memory availability differs")
    if has_smaps_samples == (smaps_skip_reason is not None):
        raise ValueError("smaps samples require exactly one sampled or skipped disposition")
    if has_smaps_samples and (
        len(pss_samples) != len(rss_samples) or len(private_samples) != len(rss_samples)
    ):
        raise ValueError("displayed-memory sample counts differ")
    if smaps_skip_reason is not None and not smaps_skip_reason.startswith("skipped:"):
        raise ValueError("smaps skip reason must be explicit")
    rss_context = {
        "display_backend": display_backend,
        "measurement": "linux-proc-status-vmrss",
    }
    smaps_context = {
        "display_backend": display_backend,
        "measurement": "linux-proc-smaps-rollup",
    }
    smaps_workload = (
        smaps_skip_reason
        if smaps_skip_reason is not None
        else "actual displayed application proportional/private memory during idle sampling"
    )
    return [
        {
            "name": "memory.application_idle_rss",
            "unit": "KiB",
            "samples": rss_samples,
            "workload": "actual displayed application RSS during idle sampling",
            "context": rss_context,
        },
        {
            "name": "memory.application_idle_pss",
            "unit": "KiB",
            "samples": pss_samples,
            "workload": smaps_workload,
            "context": smaps_context if has_smaps_samples else None,
        },
        {
            "name": "memory.application_idle_private",
            "unit": "KiB",
            "samples": private_samples,
            "workload": smaps_workload,
            "context": smaps_context if has_smaps_samples else None,
        },
    ]


def load_metric_contract(path: Path = METRIC_CONTRACT) -> dict[str, Any]:
    with path.open("rb") as stream:
        contract = tomllib.load(stream)
    if contract.get("schema_version") != 1:
        raise ValueError(f"unsupported benchmark metric contract schema in {path}")
    entries = contract.get("metric", [])
    names = [entry.get("name") for entry in entries]
    if not entries or any(not isinstance(name, str) or not name for name in names):
        raise ValueError(f"benchmark metric contract has an invalid or empty name in {path}")
    if len(names) != len(set(names)):
        raise ValueError(f"benchmark metric contract contains duplicate names in {path}")
    for entry in entries:
        if entry.get("producer") not in {"core", "application", "display"}:
            raise ValueError(f"invalid producer for benchmark metric {entry['name']}")
        if not isinstance(entry.get("unit"), str) or not entry["unit"]:
            raise ValueError(f"invalid unit for benchmark metric {entry['name']}")
        if not isinstance(entry.get("samples"), int) or entry["samples"] < 1:
            raise ValueError(f"invalid sample count for benchmark metric {entry['name']}")
    return contract


def validate_metric_batch(
    metrics: list[dict[str, Any]],
    contract: dict[str, Any],
    producer: str,
) -> list[str]:
    """Return fail-closed contract errors for one producer invocation."""
    expected = {
        entry["name"]: entry
        for entry in contract["metric"]
        if entry["producer"] == producer
    }
    actual: dict[str, list[dict[str, Any]]] = {}
    for metric in metrics:
        name = metric.get("name")
        if not isinstance(name, str):
            return [f"{producer}: emitted a metric without a string name"]
        actual.setdefault(name, []).append(metric)

    errors = [
        f"{producer}: missing metric {name}" for name in sorted(expected.keys() - actual.keys())
    ]
    errors.extend(
        f"{producer}: unexpected metric {name}" for name in sorted(actual.keys() - expected.keys())
    )
    for name in sorted(expected.keys() & actual.keys()):
        records = actual[name]
        entry = expected[name]
        if len(records) != 1:
            errors.append(f"{producer}: metric {name} was emitted {len(records)} times")
            continue
        metric = records[0]
        if metric.get("unit") != entry["unit"]:
            errors.append(
                f"{producer}: metric {name} unit {metric.get('unit')!r} != {entry['unit']!r}"
            )
        samples = metric.get("samples")
        if not isinstance(samples, list):
            errors.append(f"{producer}: metric {name} samples are not a list")
            continue
        if not samples:
            explicitly_skipped = str(metric.get("workload", "")).startswith("skipped:")
            if not entry.get("allow_skipped") or not explicitly_skipped:
                errors.append(f"{producer}: metric {name} has no samples without an allowed explicit skip")
            continue
        if len(samples) != entry["samples"]:
            errors.append(
                f"{producer}: metric {name} has {len(samples)} samples; expected {entry['samples']}"
            )
        if any(
            not isinstance(sample, (int, float))
            or isinstance(sample, bool)
            or not math.isfinite(float(sample))
            or float(sample) < 0.0
            for sample in samples
        ):
            errors.append(f"{producer}: metric {name} contains an invalid sample")
        if entry.get("context_required") and not metric.get("context"):
            errors.append(f"{producer}: metric {name} is missing required context")
    return errors


def validate_repeat_consistency(
    metrics: list[dict[str, Any]], contract: dict[str, Any], repeat_count: int
) -> list[str]:
    """Reject repeated measurements that cannot truthfully be merged."""
    if repeat_count < 2:
        return []
    errors = []
    for entry in contract["metric"]:
        if entry["producer"] == "display":
            continue
        records = [metric for metric in metrics if metric.get("name") == entry["name"]]
        if len(records) != repeat_count:
            errors.append(
                f"{entry['producer']}: metric {entry['name']} has {len(records)} "
                f"repeat records; expected {repeat_count}"
            )
            continue
        sampled = sum(bool(metric.get("samples")) for metric in records)
        if 0 < sampled < len(records):
            errors.append(
                f"{entry['producer']}: metric {entry['name']} changed availability "
                f"across repeats ({sampled} sampled, {len(records) - sampled} skipped)"
            )
        contexts = {
            json.dumps(metric.get("context"), sort_keys=True, separators=(",", ":"))
            for metric in records
        }
        if len(contexts) > 1:
            errors.append(
                f"{entry['producer']}: metric {entry['name']} changed context across repeats"
            )
        workloads = {metric.get("workload") for metric in records}
        if len(workloads) > 1:
            errors.append(
                f"{entry['producer']}: metric {entry['name']} changed workload across repeats"
            )
    return errors


def actual_display_metrics() -> list[dict[str, Any]]:
    if platform.system() != "Linux" or not (os.environ.get("WAYLAND_DISPLAY") or os.environ.get("DISPLAY")):
        return skipped_display_metrics("skipped: Linux display session unavailable")

    subprocess.run(["cargo", "build", "-p", "termeric", "--release", "--locked"], cwd=ROOT, check=True)
    with tempfile.TemporaryDirectory(prefix="termeric-benchmark-config-") as config_home:
        config_dir = Path(config_home) / "termeric"
        config_dir.mkdir()
        (config_dir / "config.toml").write_text(
            '[shell]\nprogram = "/bin/cat"\nargs = []\n\n'
            '[cursor]\nblink = false\n\n'
            '[session]\nenabled = false\nauto_restore = false\nauto_save = false\n\n'
            '[splash]\nmode = "never"\n\n'
            '[plugins.claude_code]\nenabled = false\n\n'
            '[plugins.codex]\nenabled = false\n',
            encoding="utf-8",
        )
        latency_path = Path(config_home) / "key-to-present.samples"
        frame_stats_path = Path(config_home) / "rendered-frames"
        environment = os.environ.copy()
        environment["XDG_CONFIG_HOME"] = config_home
        environment.setdefault("RUST_LOG", "warn")
        environment["TERMERIC_BENCHMARK_KEY_TO_PRESENT_PATH"] = str(latency_path)
        environment["TERMERIC_BENCHMARK_FRAME_STATS_PATH"] = str(frame_stats_path)
        display_backend = isolate_display_backend(environment)
        process = subprocess.Popen(
            [ROOT / "target" / "release" / "termeric"],
            cwd=ROOT,
            env=environment,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            text=True,
        )
        try:
            time.sleep(1.0)
            if process.poll() is not None:
                diagnostics = process.stderr.read().strip() if process.stderr else ""
                suffix = f"\n{diagnostics[-4_000:]}" if diagnostics else ""
                raise RuntimeError(
                    f"Termeric exited before idle sampling (status {process.returncode}){suffix}"
                )
            samples = []
            frame_samples = []
            rss_samples = []
            pss_samples = []
            private_samples = []
            smaps_skip_reason = None
            try:
                process_smaps_rollup_kib(process.pid)
            except (OSError, ValueError) as error:
                smaps_skip_reason = (
                    "skipped: Linux smaps_rollup unavailable "
                    f"({type(error).__name__})"
                )
            clock_ticks = os.sysconf("SC_CLK_TCK")
            for _ in range(5):
                before_frames = rendered_frame_count(frame_stats_path)
                before_ticks = process_ticks(process.pid)
                started = time.monotonic()
                time.sleep(0.4)
                elapsed = time.monotonic() - started
                after_ticks = process_ticks(process.pid)
                after_frames = rendered_frame_count(frame_stats_path)
                samples.append((after_ticks - before_ticks) / clock_ticks / elapsed * 100.0)
                frame_samples.append(float(after_frames - before_frames))
                rss_samples.append(process_rss_kib(process.pid))
                if smaps_skip_reason is None:
                    memory = process_smaps_rollup_kib(process.pid)
                    pss_samples.append(memory["pss_kib"])
                    private_samples.append(memory["private_kib"])

            latency_samples = []
            latency_workload = "skipped: native X11 input injection unavailable"
            xdotool = shutil.which("xdotool")
            if environment.get("DISPLAY") and xdotool:
                window_id = None
                deadline = time.monotonic() + 8.0
                while time.monotonic() < deadline and window_id is None:
                    found = subprocess.run(
                        [
                            xdotool,
                            "search",
                            "--onlyvisible",
                            "--pid",
                            str(process.pid),
                            "--name",
                            "Termeric",
                        ],
                        env=environment,
                        text=True,
                        stdout=subprocess.PIPE,
                        stderr=subprocess.DEVNULL,
                        check=False,
                    )
                    if found.returncode == 0 and found.stdout.strip():
                        window_id = found.stdout.splitlines()[-1]
                    else:
                        time.sleep(0.05)
                if window_id is None:
                    raise RuntimeError("displayed latency probe could not find Termeric's X11 window")
                subprocess.run(
                    [xdotool, "windowfocus", "--sync", window_id],
                    env=environment,
                    check=True,
                )
                for key in ("a", "b", "c", "d", "e"):
                    expected = len(latency_samples) + 1
                    subprocess.run(
                        [xdotool, "key", "--window", window_id, "--clearmodifiers", key],
                        env=environment,
                        check=True,
                    )
                    deadline = time.monotonic() + 5.0
                    while time.monotonic() < deadline:
                        observed = presentation_latency_samples(latency_path)
                        if len(observed) >= expected:
                            latency_samples = observed
                            break
                        time.sleep(0.01)
                    else:
                        raise RuntimeError(
                            f"displayed latency probe timed out after {len(latency_samples)} samples"
                        )
                latency_workload = (
                    "native X11 key callback through PTY echo, parser, GPU submission, "
                    "and SurfaceTexture::present; excludes compositor scanout"
                )
        finally:
            process.terminate()
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=3)
            if process.stderr:
                process.stderr.close()

    return [
        {
            "name": "idle.application_cpu",
            "unit": "percent_of_one_core",
            "samples": samples,
            "workload": "actual displayed application left untouched for five 400 ms windows",
        },
        {
            "name": "idle.application_rendered_frames",
            "unit": "frames",
            "samples": frame_samples,
            "workload": "rendered-frame deltas during the five displayed idle sampling windows",
        },
        {
            "name": "latency.displayed_key_event_to_present",
            "unit": "ms",
            "samples": latency_samples,
            "workload": latency_workload,
            "context": {"input_backend": "xdotool-x11" if latency_samples else None},
        },
        *display_memory_metrics(
            display_backend=display_backend,
            rss_samples=rss_samples,
            pss_samples=pss_samples,
            private_samples=private_samples,
            smaps_skip_reason=smaps_skip_reason,
        ),
    ]


def environment_metadata() -> dict[str, Any]:
    cpu_model = "unknown"
    cpuinfo = Path("/proc/cpuinfo")
    if cpuinfo.exists():
        for line in cpuinfo.read_text(encoding="utf-8", errors="replace").splitlines():
            if line.startswith("model name"):
                cpu_model = line.split(":", 1)[1].strip()
                break
    return {
        "captured_at": datetime.now(timezone.utc).isoformat(),
        "git_commit": command_output(["git", "rev-parse", "HEAD"]),
        "git_dirty": bool(command_output(["git", "status", "--porcelain"])),
        "rustc": command_output(["rustc", "--version", "--verbose"]),
        "cargo": command_output(["cargo", "--version"]),
        "os": platform.platform(),
        "kernel": platform.release(),
        "architecture": platform.machine(),
        "cpu_model": cpu_model,
        "logical_cpus": os.cpu_count(),
        "wgpu_backend": os.environ.get("WGPU_BACKEND", "auto"),
        "display": {
            "session": os.environ.get("XDG_SESSION_TYPE"),
            "wayland": os.environ.get("WAYLAND_DISPLAY"),
            "x11": os.environ.get("DISPLAY"),
        },
    }


def merge_metrics(raw_metrics: list[dict[str, Any]]) -> list[dict[str, Any]]:
    merged: dict[str, dict[str, Any]] = {}
    for metric in raw_metrics:
        current = merged.setdefault(metric["name"], {**metric, "samples": []})
        if current["unit"] != metric["unit"]:
            raise ValueError(f"unit changed within run for {metric['name']}")
        if current.get("workload") != metric.get("workload"):
            raise ValueError(f"workload changed within run for {metric['name']}")
        if current.get("context") != metric.get("context"):
            raise ValueError(f"context changed within run for {metric['name']}")
        current["samples"].extend(float(sample) for sample in metric.get("samples", []))
    for metric in merged.values():
        metric["statistics"] = summarize(metric["samples"])
    return sorted(merged.values(), key=lambda metric: metric["name"])


def metric_map(report: dict[str, Any]) -> dict[str, dict[str, Any]]:
    return {metric["name"]: metric for metric in report["metrics"]}


def evaluate_budgets(report: dict[str, Any], budgets: dict[str, Any]) -> list[dict[str, Any]]:
    metrics = metric_map(report)
    results = []
    for budget in budgets.get("ratio_budget", []):
        numerator_metric = metrics.get(budget["numerator"])
        denominator_metric = metrics.get(budget["denominator"])
        numerator = None if numerator_metric is None else numerator_metric["statistics"].get("median")
        denominator = (
            None if denominator_metric is None else denominator_metric["statistics"].get("median")
        )
        ratio = None if numerator is None or denominator is None else (
            math.inf if not denominator else numerator / denominator
        )
        results.append({**budget, "actual": ratio, "passed": ratio is not None and ratio <= budget["max_ratio"]})
    for budget in budgets.get("metric_budget", []):
        metric = metrics.get(budget["metric"])
        actual = None if metric is None else metric["statistics"].get(budget["statistic"])
        explicitly_skipped = (
            metric is not None
            and not metric.get("samples")
            and str(metric.get("workload", "")).startswith("skipped:")
        )
        passed = (
            bool(budget.get("allow_skipped")) and explicitly_skipped
            if actual is None
            else actual <= budget["max"]
        )
        results.append({**budget, "actual": actual, "passed": passed})
    for budget in budgets.get("variation_budget", []):
        metric = metrics.get(budget["metric"])
        actual = None if metric is None else metric["statistics"].get("coefficient_of_variation")
        results.append({
            **budget,
            "actual": actual,
            "passed": actual is not None and actual <= budget["max_coefficient_of_variation"],
        })
    return results


def comparison_diagnostics(
    baseline: dict[str, Any], current: dict[str, Any]
) -> tuple[dict[str, Any], set[str]]:
    """Describe which baseline rows are comparable without mixing environments."""
    errors = []
    skipped = []
    for field in ("schema_version", "profile"):
        if baseline.get(field) != current.get(field):
            errors.append(
                f"baseline {field} {baseline.get(field)!r} != current {current.get(field)!r}"
            )

    baseline_environment = baseline.get("environment")
    current_environment = current.get("environment")
    if not isinstance(baseline_environment, dict) or not isinstance(current_environment, dict):
        errors.append("baseline and current reports must contain environment objects")
    else:
        for field in ("architecture", "cpu_model", "logical_cpus"):
            before = baseline_environment.get(field)
            after = current_environment.get(field)
            if before is None or after is None:
                errors.append(f"comparison environment is missing {field}")
            elif before != after:
                errors.append(f"comparison environment changed {field}: {before!r} != {after!r}")

    old = metric_map(baseline)
    comparable = set()
    for name, metric in metric_map(current).items():
        previous = old.get(name)
        if previous is None:
            continue
        if previous.get("unit") != metric.get("unit"):
            skipped.append(
                {
                    "metric": name,
                    "reason": f"unit changed: {previous.get('unit')!r} != {metric.get('unit')!r}",
                }
            )
            continue
        previous_median = previous.get("statistics", {}).get("median")
        current_median = metric.get("statistics", {}).get("median")
        if previous_median is None or current_median is None:
            skipped.append({"metric": name, "reason": "one report has no median"})
            continue
        if previous.get("context") != metric.get("context"):
            skipped.append(
                {
                    "metric": name,
                    "reason": "metric context is missing or changed",
                    "baseline_context": previous.get("context"),
                    "current_context": metric.get("context"),
                }
            )
            continue
        comparable.add(name)

    if errors:
        comparable.clear()
    if not comparable:
        errors.append("baseline has no metrics comparable in the current environment")
    diagnostics = {
        "passed": not errors,
        "errors": errors,
        "comparable_metrics": sorted(comparable),
        "skipped_metrics": sorted(skipped, key=lambda item: item["metric"]),
    }
    return diagnostics, comparable


def compare_reports(
    baseline: dict[str, Any],
    current: dict[str, Any],
    comparable_names: set[str] | None = None,
) -> list[dict[str, Any]]:
    old = metric_map(baseline)
    comparisons = []
    for name, metric in metric_map(current).items():
        if name not in old or (comparable_names is not None and name not in comparable_names):
            continue
        if old[name].get("unit") != metric.get("unit"):
            continue
        if old[name].get("context") != metric.get("context"):
            continue
        previous = old[name]["statistics"].get("median")
        present = metric["statistics"].get("median")
        if previous is None or present is None:
            continue
        comparisons.append({
            "metric": name,
            "baseline_median": previous,
            "current_median": present,
            "change_percent": None if previous == 0 else (present / previous - 1.0) * 100.0,
        })
    return comparisons


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / "target" / "benchmarks" / "latest.json")
    parser.add_argument("--repeat", type=int, default=1, help="repeat the complete Rust suites")
    parser.add_argument("--baseline", type=Path, help="prior JSON artifact to compare")
    parser.add_argument(
        "--skip-displayed",
        "--skip-gui-idle",
        dest="skip_displayed",
        action="store_true",
        help="do not launch the displayed idle and native key-to-present probes",
    )
    parser.add_argument("--no-check", action="store_true", help="record results without enforcing budgets")
    args = parser.parse_args()
    if args.repeat < 1:
        parser.error("--repeat must be positive")

    raw_metrics = []
    contract = load_metric_contract()
    contract_errors = []
    for run in range(args.repeat):
        print(f"Benchmark repeat {run + 1}/{args.repeat}: core")
        core_metrics = run_rust_suite("termeric-core", "performance_benchmarks")
        contract_errors.extend(
            f"repeat {run + 1}: {error}"
            for error in validate_metric_batch(core_metrics, contract, "core")
        )
        raw_metrics.extend(core_metrics)
        print(f"Benchmark repeat {run + 1}/{args.repeat}: application and renderer")
        application_metrics = run_rust_suite("termeric")
        contract_errors.extend(
            f"repeat {run + 1}: {error}"
            for error in validate_metric_batch(application_metrics, contract, "application")
        )
        raw_metrics.extend(application_metrics)
    if not args.skip_displayed:
        print("Benchmark: displayed application idle CPU/RSS and key-to-present latency")
        display_metrics = actual_display_metrics()
    else:
        display_metrics = skipped_display_metrics("skipped: --skip-displayed requested")
    contract_errors.extend(validate_metric_batch(display_metrics, contract, "display"))
    raw_metrics.extend(display_metrics)
    contract_errors.extend(validate_repeat_consistency(raw_metrics, contract, args.repeat))

    report = {
        "schema_version": 1,
        "profile": "release",
        "repeat_count": args.repeat,
        "environment": environment_metadata(),
        "metrics": merge_metrics(raw_metrics),
        "metric_contract": {
            "schema_version": contract["schema_version"],
            "path": str(METRIC_CONTRACT.relative_to(ROOT)),
            "passed": not contract_errors,
            "errors": contract_errors,
        },
    }
    with (ROOT / "benchmarks" / "budgets.toml").open("rb") as stream:
        budgets = tomllib.load(stream)
    report["budgets"] = evaluate_budgets(report, budgets)
    if args.baseline:
        baseline = json.loads(args.baseline.read_text())
        diagnostics, comparable = comparison_diagnostics(baseline, report)
        report["comparison_contract"] = {
            "baseline": str(args.baseline),
            **diagnostics,
        }
        report["comparison"] = compare_reports(baseline, report, comparable)

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"Wrote benchmark artifact: {args.output}")

    failures = [budget for budget in report["budgets"] if not budget["passed"]]
    for error in contract_errors:
        print(f"FAIL: metric contract ({error})")
    if args.baseline:
        comparison_contract = report["comparison_contract"]
        status = "PASS" if comparison_contract["passed"] else "SKIP"
        print(
            f"{status}: baseline comparison "
            f"({len(comparison_contract['comparable_metrics'])} comparable, "
            f"{len(comparison_contract['skipped_metrics'])} metric skips)"
        )
        for error in comparison_contract["errors"]:
            print(f"SKIP: baseline comparison ({error})")
    for budget in report["budgets"]:
        status = "PASS" if budget["passed"] else "FAIL"
        print(f"{status}: {budget['name']} (actual={budget['actual']})")
    if (failures or contract_errors) and not args.no_check:
        raise SystemExit(
            f"benchmark evidence failed: {len(contract_errors)} metric contract error(s), "
            f"{len(failures)} budget failure(s)"
        )


if __name__ == "__main__":
    main()
