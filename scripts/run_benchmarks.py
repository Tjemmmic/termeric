#!/usr/bin/env python3
"""Run Termeric's release benchmarks, write their results, and enforce budgets."""

from __future__ import annotations

import argparse
import json
import math
import statistics
import subprocess
import tomllib
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
BUDGETS = ROOT / "benchmarks" / "budgets.toml"
PREFIX = "TERMERIC_BENCH "
SUITES = (("termeric-core", "performance_benchmarks"), ("termeric", None))


def percentile(samples: list[float], fraction: float) -> float:
    ordered = sorted(samples)
    return ordered[max(0, math.ceil(len(ordered) * fraction) - 1)]


def summarize(samples: list[float]) -> dict[str, float | int]:
    if not samples:
        return {"count": 0}
    return {
        "count": len(samples),
        "min": min(samples),
        "median": statistics.median(samples),
        "p95": percentile(samples, 0.95),
        "max": max(samples),
        "mean": statistics.fmean(samples),
    }


def parse_metrics(output: str) -> list[dict[str, Any]]:
    """The metric records a benchmark run printed, one per marked line."""
    return [
        json.loads(line[line.find(PREFIX) + len(PREFIX):])
        for line in output.splitlines()
        if PREFIX in line
    ]


def run_suite(package: str, test_target: str | None) -> list[dict[str, Any]]:
    command = ["cargo", "test", "-p", package, "--release", "--locked"]
    if test_target:
        command += ["--test", test_target]
    command += ["release_benchmark_suite", "--", "--ignored", "--nocapture", "--test-threads=1"]
    completed = subprocess.run(command, cwd=ROOT, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    print(completed.stdout, end="")
    if completed.returncode:
        raise SystemExit(f"benchmark run failed with status {completed.returncode}: {' '.join(command)}")
    metrics = parse_metrics(completed.stdout)
    if not metrics:
        raise SystemExit(f"benchmark run printed no metrics: {' '.join(command)}")
    return metrics


def merge_metrics(records: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Pool each metric's samples across repeats and summarize them."""
    merged: dict[str, dict[str, Any]] = {}
    for record in records:
        metric = merged.setdefault(record["name"], {**record, "samples": []})
        if metric["unit"] != record["unit"]:
            raise ValueError(f"{record['name']} changed unit between repeats")
        metric["samples"].extend(float(sample) for sample in record.get("samples", []))
    for metric in merged.values():
        metric["statistics"] = summarize(metric["samples"])
    return sorted(merged.values(), key=lambda metric: metric["name"])


def evaluate_budgets(metrics: list[dict[str, Any]], budgets: dict[str, Any]) -> list[dict[str, Any]]:
    """Check every budget. A missing metric fails, unless the budget allows a
    run that reported the metric as skipped (no GPU, for instance)."""
    by_name = {metric["name"]: metric for metric in metrics}

    def statistic(name: str, key: str) -> float | None:
        metric = by_name.get(name)
        return None if metric is None else metric["statistics"].get(key)

    results = []
    for budget in budgets.get("metric_budget", []):
        actual = statistic(budget["metric"], budget["statistic"])
        metric = by_name.get(budget["metric"])
        skipped = metric is not None and not metric["samples"] and str(metric.get("workload", "")).startswith("skipped:")
        passed = actual <= budget["max"] if actual is not None else bool(budget.get("allow_skipped")) and skipped
        results.append({**budget, "actual": actual, "passed": passed})
    for budget in budgets.get("ratio_budget", []):
        numerator = statistic(budget["numerator"], "median")
        denominator = statistic(budget["denominator"], "median")
        ratio = None if numerator is None or denominator is None else (numerator / denominator if denominator else math.inf)
        results.append({**budget, "actual": ratio, "passed": ratio is not None and ratio <= budget["max_ratio"]})
    return results


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / "target" / "benchmarks" / "latest.json")
    parser.add_argument("--repeat", type=int, default=1, help="run the suites this many times")
    parser.add_argument("--no-check", action="store_true", help="record results without enforcing budgets")
    args = parser.parse_args()
    if args.repeat < 1:
        parser.error("--repeat must be positive")

    records = []
    for repeat in range(args.repeat):
        for package, target in SUITES:
            print(f"Benchmarks {repeat + 1}/{args.repeat}: {package}")
            records += run_suite(package, target)
    metrics = merge_metrics(records)
    with BUDGETS.open("rb") as stream:
        budgets = evaluate_budgets(metrics, tomllib.load(stream))

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps({"metrics": metrics, "budgets": budgets}, indent=2) + "\n", encoding="utf-8")
    print(f"Wrote {args.output}")
    for budget in budgets:
        print(f"{'PASS' if budget['passed'] else 'FAIL'}: {budget['name']} (actual={budget['actual']})")
    failures = [budget for budget in budgets if not budget["passed"]]
    if failures and not args.no_check:
        raise SystemExit(f"{len(failures)} benchmark budget(s) failed")


if __name__ == "__main__":
    main()
