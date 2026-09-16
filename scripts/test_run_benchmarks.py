import json
import unittest
from unittest.mock import patch
from pathlib import Path
import tempfile
import tomllib

from scripts.run_benchmarks import (
    compare_reports,
    comparison_diagnostics,
    display_memory_metrics,
    evaluate_budgets,
    isolate_display_backend,
    load_metric_contract,
    main,
    merge_metrics,
    parse_process_rss_kib,
    parse_smaps_rollup_kib,
    percentile,
    presentation_latency_samples,
    rendered_frame_count,
    skipped_display_metrics,
    summarize,
    validate_metric_batch,
    validate_repeat_consistency,
)


def report(value: float) -> dict:
    return {
        "metrics": [
            {
                "name": "work",
                "unit": "ms",
                "workload": "fixed work fixture v1",
                "samples": [value] * 5,
                "statistics": summarize([value] * 5),
            },
            {
                "name": "control",
                "unit": "ms",
                "workload": "fixed control fixture v1",
                "samples": [10.0] * 5,
                "statistics": summarize([10.0] * 5),
            },
        ]
    }


def comparable_report(value: float) -> dict:
    result = report(value)
    result.update(
        {
            "schema_version": 1,
            "profile": "release",
            "environment": {
                "architecture": "x86_64",
                "cpu_model": "fixture cpu",
                "logical_cpus": 8,
            },
        }
    )
    return result


class BenchmarkBudgetTests(unittest.TestCase):
    def test_shaping_pressure_contract_and_deterministic_budgets(self):
        contract = load_metric_contract()
        rows = [row for row in contract["metric"] if ".shaping_pressure." in row["name"]]
        self.assertEqual(len(rows), 6)
        for row in rows:
            self.assertEqual(row["producer"], "application")
            self.assertEqual(row["samples"], 5)
            self.assertTrue(row["context_required"])
            self.assertTrue(row["allow_skipped"])
        with (Path(__file__).parents[1] / "benchmarks" / "budgets.toml").open("rb") as stream:
            budgets = tomllib.load(stream)
        selected = [row for row in budgets["metric_budget"] if ".shaping_pressure." in row["metric"]]
        self.assertEqual({row["metric"]: row["max"] for row in selected}, {
            "memory.shaping_pressure.retained_bytes": 8388608.0,
            "renderer.shaping_pressure.rasterizations": 0.0,
        })
        for budget in selected:
            for value, passes in [(budget["max"], True), (budget["max"] + 1, False)]:
                sampled = {"metrics": [{
                    "name": budget["metric"], "samples": [value] * 5,
                    "statistics": summarize([value] * 5),
                }]}
                self.assertEqual(evaluate_budgets(sampled, {"metric_budget": [budget]})[0]["passed"], passes)
            self.assertFalse(evaluate_budgets({"metrics": []}, {"metric_budget": [budget]})[0]["passed"])

    def test_streaming_diff_budgets_reject_regressions_and_missing_evidence(self):
        with (Path(__file__).parents[1] / "benchmarks" / "budgets.toml").open("rb") as stream:
            checked_in = tomllib.load(stream)
        metrics = [
            item for item in checked_in["metric_budget"]
            if ".diff_observer." in item["metric"]
        ]
        ratios = [
            item for item in checked_in["ratio_budget"]
            if ".diff_observer." in item["numerator"]
        ]
        self.assertEqual(len(metrics), 4)
        self.assertEqual(len(ratios), 1)
        for budget in metrics:
            with self.subTest(metric=budget["metric"]):
                maximum = 4194304.0 if budget["metric"].startswith("memory.") else 100.0
                self.assertEqual(budget["max"], maximum)
                self.assertFalse(budget.get("allow_skipped", False))
                selected = {"metric_budget": [budget]}
                for value, passes in [(maximum, True), (maximum + 1, False)]:
                    sampled = {"metrics": [{
                        "name": budget["metric"],
                        "samples": [value] * 5,
                        "statistics": summarize([value] * 5),
                    }]}
                    self.assertEqual(evaluate_budgets(sampled, selected)[0]["passed"], passes)
                self.assertFalse(evaluate_budgets({"metrics": []}, selected)[0]["passed"])
                skipped = {"metrics": [{
                    "name": budget["metric"], "samples": [], "statistics": {"count": 0},
                    "workload": "skipped: synthetic regression fixture",
                }]}
                self.assertFalse(evaluate_budgets(skipped, selected)[0]["passed"])

        ratio = ratios[0]
        self.assertEqual(ratio["max_ratio"], 4.0)
        for bulk, fragmented, passes in [(10.0, 40.0, True), (10.0, 40.1, False), (0.0, 1.0, False)]:
            sampled = {"metrics": [
                {"name": name, "statistics": summarize([value] * 5)}
                for name, value in [(ratio["denominator"], bulk), (ratio["numerator"], fragmented)]
            ]}
            selected = {"ratio_budget": [ratio]}
            self.assertEqual(evaluate_budgets(sampled, selected)[0]["passed"], passes)
            for missing in sampled["metrics"]:
                self.assertFalse(evaluate_budgets({"metrics": [missing]}, selected)[0]["passed"])

    def test_checked_in_metric_contract_is_valid_and_unique(self):
        contract = load_metric_contract()
        names = [entry["name"] for entry in contract["metric"]]
        self.assertEqual(len(names), len(set(names)))
        self.assertGreaterEqual(len(names), 50)

    def test_metric_contract_rejects_missing_unexpected_and_duplicate_rows(self):
        contract = {
            "metric": [
                {
                    "name": "expected_missing",
                    "unit": "ms",
                    "producer": "core",
                    "samples": 2,
                },
                {
                    "name": "expected_duplicate",
                    "unit": "ms",
                    "producer": "core",
                    "samples": 2,
                },
            ]
        }
        errors = validate_metric_batch(
            [
                {"name": "expected_duplicate", "unit": "ms", "samples": [1.0, 2.0]},
                {"name": "expected_duplicate", "unit": "ms", "samples": [1.0, 2.0]},
                {"name": "unexpected", "unit": "ms", "samples": [1.0, 2.0]},
            ],
            contract,
            "core",
        )
        self.assertTrue(any("missing metric expected_missing" in error for error in errors))
        self.assertTrue(any("unexpected metric unexpected" in error for error in errors))
        self.assertTrue(any("expected_duplicate was emitted 2 times" in error for error in errors))

    def test_metric_contract_requires_exact_unit_sample_count_and_context(self):
        contract = {
            "metric": [
                {
                    "name": "work",
                    "unit": "ms",
                    "producer": "application",
                    "samples": 2,
                    "context_required": True,
                }
            ]
        }
        errors = validate_metric_batch(
            [{"name": "work", "unit": "seconds", "samples": [1.0]}],
            contract,
            "application",
        )
        self.assertTrue(any("unit" in error for error in errors))
        self.assertTrue(any("1 samples; expected 2" in error for error in errors))
        self.assertTrue(any("missing required context" in error for error in errors))

    def test_metric_contract_accepts_only_declared_explicit_skips(self):
        contract = {
            "metric": [
                {
                    "name": "optional",
                    "unit": "ms",
                    "producer": "display",
                    "samples": 5,
                    "allow_skipped": True,
                }
            ]
        }
        self.assertEqual(
            validate_metric_batch(
                [
                    {
                        "name": "optional",
                        "unit": "ms",
                        "samples": [],
                        "workload": "skipped: fixture unavailable",
                    }
                ],
                contract,
                "display",
            ),
            [],
        )
        errors = validate_metric_batch(
            [{"name": "optional", "unit": "ms", "samples": [], "workload": "forgotten"}],
            contract,
            "display",
        )
        self.assertTrue(any("without an allowed explicit skip" in error for error in errors))

    def test_metric_contract_rejects_probe_availability_that_changes_between_repeats(self):
        contract = {
            "metric": [
                {
                    "name": "renderer.optional",
                    "unit": "ms",
                    "producer": "application",
                    "samples": 1,
                    "allow_skipped": True,
                }
            ]
        }
        errors = validate_repeat_consistency(
            [
                {"name": "renderer.optional", "samples": [1.0]},
                {"name": "renderer.optional", "samples": [], "workload": "skipped: fixture"},
            ],
            contract,
            2,
        )
        self.assertTrue(any("changed availability across repeats" in error for error in errors))

    def test_repeat_consistency_rejects_adapter_and_workload_drift(self):
        contract = {
            "metric": [
                {
                    "name": "renderer.work",
                    "unit": "ms",
                    "producer": "application",
                    "samples": 1,
                    "context_required": True,
                }
            ]
        }
        errors = validate_repeat_consistency(
            [
                {
                    "name": "renderer.work",
                    "samples": [1.0],
                    "context": "llvmpipe",
                    "workload": "first fixture",
                },
                {
                    "name": "renderer.work",
                    "samples": [2.0],
                    "context": "NVIDIA",
                    "workload": "second fixture",
                },
            ],
            contract,
            2,
        )
        self.assertTrue(any("changed context across repeats" in error for error in errors))
        self.assertTrue(any("changed workload across repeats" in error for error in errors))

    def test_repeat_consistency_rejects_missing_required_repeat_record(self):
        contract = {
            "metric": [
                {
                    "name": "work",
                    "unit": "ms",
                    "producer": "core",
                    "samples": 1,
                }
            ]
        }
        errors = validate_repeat_consistency(
            [{"name": "work", "samples": [1.0], "workload": "fixture"}],
            contract,
            2,
        )
        self.assertTrue(any("has 1 repeat records; expected 2" in error for error in errors))

    def test_metric_merge_rejects_context_or_workload_drift(self):
        base = {
            "name": "renderer.work",
            "unit": "ms",
            "samples": [1.0],
            "workload": "fixture",
            "context": "llvmpipe",
        }
        with self.assertRaisesRegex(ValueError, "context changed"):
            merge_metrics([base, {**base, "context": "NVIDIA"}])
        with self.assertRaisesRegex(ValueError, "workload changed"):
            merge_metrics([base, {**base, "workload": "different fixture"}])

    def test_skipped_display_contract_keeps_every_metric_in_the_artifact(self):
        metrics = skipped_display_metrics("skipped: fixture")
        self.assertEqual(
            [metric["name"] for metric in metrics],
            [
                "idle.application_cpu",
                "idle.application_rendered_frames",
                "memory.application_idle_rss",
                "memory.application_idle_pss",
                "memory.application_idle_private",
                "latency.displayed_key_event_to_present",
            ],
        )
        self.assertTrue(all(metric["samples"] == [] for metric in metrics))

    def test_proc_memory_parsers_require_exact_unique_kib_fields(self):
        self.assertEqual(
            parse_process_rss_kib("Name:\tterm\nVmRSS:\t12345 kB\nThreads:\t8\n"),
            12345.0,
        )
        for invalid_status in (
            "Name:\tterm\n",
            "VmRSS: 1 kB\nVmRSS: 2 kB\n",
            "VmRSS: 1 KB\n",
        ):
            with self.subTest(invalid_status=invalid_status):
                with self.assertRaises(ValueError):
                    parse_process_rss_kib(invalid_status)
        self.assertEqual(
            parse_smaps_rollup_kib(
                "00400000-7fffffff ---p 00000000 00:00 0 [rollup]\n"
                "Rss:                 200 kB\n"
                "Pss:                 150 kB\n"
                "Private_Clean:        20 kB\n"
                "Private_Dirty:        70 kB\n"
                "Private_Hugetlb:      10 kB\n"
                "Shared_Clean:         80 kB\n"
            ),
            {"rss_kib": 200.0, "pss_kib": 150.0, "private_kib": 100.0},
        )
        self.assertEqual(
            parse_smaps_rollup_kib(
                "Rss: 20 kB\nPss: 15 kB\nPrivate_Clean: 2 kB\nPrivate_Dirty: 7 kB\n"
            )["private_kib"],
            9.0,
        )

        for invalid in (
            "Rss: 20 kB\nPrivate_Clean: 2 kB\nPrivate_Dirty: 7 kB\n",
            "Rss: 20 kB\nRss: 21 kB\nPss: 15 kB\nPrivate_Clean: 2 kB\nPrivate_Dirty: 7 kB\n",
            "Rss: -20 kB\nPss: 15 kB\nPrivate_Clean: 2 kB\nPrivate_Dirty: 7 kB\n",
            "Rss: 20 KB\nPss: 15 kB\nPrivate_Clean: 2 kB\nPrivate_Dirty: 7 kB\n",
            "Rss: 20.5 kB\nPss: 15 kB\nPrivate_Clean: 2 kB\nPrivate_Dirty: 7 kB\n",
        ):
            with self.subTest(invalid=invalid):
                with self.assertRaises(ValueError):
                    parse_smaps_rollup_kib(invalid)

    def test_display_memory_rows_bind_context_and_explicit_smaps_skip(self):
        sampled = display_memory_metrics(
            display_backend="wayland",
            rss_samples=[180_000.0] * 5,
            pss_samples=[140_000.0] * 5,
            private_samples=[110_000.0] * 5,
            smaps_skip_reason=None,
        )
        self.assertEqual(
            [metric["name"] for metric in sampled],
            [
                "memory.application_idle_rss",
                "memory.application_idle_pss",
                "memory.application_idle_private",
            ],
        )
        self.assertEqual(sampled[0]["context"]["display_backend"], "wayland")
        self.assertEqual(sampled[1]["context"]["measurement"], "linux-proc-smaps-rollup")
        self.assertEqual(sampled[2]["samples"], [110_000.0] * 5)
        display_batch = [
            metric
            for metric in skipped_display_metrics("skipped: fixture")
            if not metric["name"].startswith("memory.application_idle_")
        ] + sampled
        self.assertEqual(
            validate_metric_batch(display_batch, load_metric_contract(), "display"),
            [],
        )

        skipped = display_memory_metrics(
            display_backend="x11",
            rss_samples=[180_000.0] * 5,
            pss_samples=[],
            private_samples=[],
            smaps_skip_reason="skipped: Linux smaps_rollup unavailable (PermissionError)",
        )
        self.assertEqual(skipped[1]["samples"], [])
        self.assertIsNone(skipped[1]["context"])
        self.assertTrue(skipped[1]["workload"].startswith("skipped:"))
        smaps_unavailable_batch = [
            metric
            for metric in skipped_display_metrics("skipped: fixture")
            if not metric["name"].startswith("memory.application_idle_")
        ] + skipped
        self.assertEqual(
            validate_metric_batch(
                smaps_unavailable_batch, load_metric_contract(), "display"
            ),
            [],
        )

        with self.assertRaisesRegex(ValueError, "availability differs"):
            display_memory_metrics(
                display_backend="x11",
                rss_samples=[1.0] * 5,
                pss_samples=[1.0] * 5,
                private_samples=[],
                smaps_skip_reason=None,
            )
        with self.assertRaisesRegex(ValueError, "sample counts differ"):
            display_memory_metrics(
                display_backend="x11",
                rss_samples=[1.0] * 5,
                pss_samples=[1.0] * 4,
                private_samples=[1.0] * 4,
                smaps_skip_reason=None,
            )
        with self.assertRaisesRegex(ValueError, "unsupported displayed-memory backend"):
            display_memory_metrics(
                display_backend="unknown",
                rss_samples=[1.0] * 5,
                pss_samples=[1.0] * 5,
                private_samples=[1.0] * 5,
                smaps_skip_reason=None,
            )
        with self.assertRaisesRegex(ValueError, "skip reason must be explicit"):
            display_memory_metrics(
                display_backend="x11",
                rss_samples=[1.0] * 5,
                pss_samples=[],
                private_samples=[],
                smaps_skip_reason="unavailable",
            )

    def test_checked_in_idle_rss_budget_is_broad_and_fail_closed(self):
        with (Path(__file__).resolve().parents[1] / "benchmarks" / "budgets.toml").open(
            "rb"
        ) as stream:
            budgets = tomllib.load(stream)
        budget = next(
            item
            for item in budgets["metric_budget"]
            if item["metric"] == "memory.application_idle_rss"
        )
        self.assertEqual(budget["statistic"], "p95")
        self.assertEqual(budget["max"], 512 * 1024)
        self.assertTrue(budget["allow_skipped"])

        sampled = {
            "metrics": [
                {
                    "name": "memory.application_idle_rss",
                    "samples": [budget["max"] + 1] * 5,
                    "statistics": summarize([budget["max"] + 1] * 5),
                    "workload": "fixture",
                }
            ]
        }
        self.assertFalse(evaluate_budgets(sampled, {"metric_budget": [budget]})[0]["passed"])

    def test_presentation_latency_samples_are_ordered_and_ignore_blank_lines(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "samples"
            path.write_text("3.25\n\n1.5\n", encoding="utf-8")
            self.assertEqual(presentation_latency_samples(path), [3.25, 1.5])

    def test_rendered_frame_count_reads_one_complete_counter(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "frames"
            path.write_text("17\n", encoding="utf-8")
            self.assertEqual(rendered_frame_count(path), 17)

    def test_display_backend_isolation_removes_wayland_from_x11_probe(self):
        environment = {
            "DISPLAY": ":98",
            "WAYLAND_DISPLAY": "wayland-0",
            "WAYLAND_SOCKET": "7",
            "XDG_SESSION_TYPE": "x11",
            "WINIT_UNIX_BACKEND": "wayland",
        }
        self.assertEqual(isolate_display_backend(environment), "x11")
        self.assertEqual(environment["DISPLAY"], ":98")
        self.assertEqual(environment["EGL_PLATFORM"], "x11")
        self.assertEqual(environment["WAYLAND_DISPLAY"], "")
        self.assertEqual(environment["WAYLAND_SOCKET"], "")
        self.assertEqual(environment["WINIT_UNIX_BACKEND"], "x11")

    def test_display_backend_isolation_removes_x11_from_wayland_probe(self):
        environment = {
            "DISPLAY": ":1",
            "WAYLAND_DISPLAY": "wayland-0",
            "XDG_SESSION_TYPE": "wayland",
            "WINIT_UNIX_BACKEND": "x11",
        }
        self.assertEqual(isolate_display_backend(environment), "wayland")
        self.assertEqual(environment["WAYLAND_DISPLAY"], "wayland-0")
        self.assertEqual(environment["EGL_PLATFORM"], "wayland")
        self.assertEqual(environment["DISPLAY"], "")
        self.assertEqual(environment["WINIT_UNIX_BACKEND"], "wayland")

    def test_percentile_uses_observed_sample(self):
        self.assertEqual(percentile([5.0, 1.0, 3.0, 2.0, 4.0], 0.95), 5.0)

    def test_deliberate_twenty_one_percent_slowdown_fails_twenty_percent_budget(self):
        budgets = {
            "ratio_budget": [
                {
                    "name": "deliberate slowdown",
                    "numerator": "work",
                    "denominator": "control",
                    "max_ratio": 1.2,
                }
            ]
        }
        self.assertFalse(evaluate_budgets(report(12.1), budgets)[0]["passed"])

    def test_absolute_interaction_rail_catches_linear_but_unusable_search(self):
        budgets = {
            "metric_budget": [
                {
                    "name": "100k incremental search interaction rail",
                    "metric": "work",
                    "statistic": "p95",
                    "max": 500.0,
                }
            ]
        }
        self.assertFalse(evaluate_budgets(report(11_800.0), budgets)[0]["passed"])

    def test_allow_skipped_budget_does_not_treat_absent_metric_as_a_pass(self):
        budgets = {
            "metric_budget": [
                {
                    "name": "optional probe",
                    "metric": "missing",
                    "statistic": "p95",
                    "max": 10.0,
                    "allow_skipped": True,
                }
            ]
        }
        self.assertFalse(evaluate_budgets({"metrics": []}, budgets)[0]["passed"])

    def test_allow_skipped_budget_accepts_present_explicit_skip_row(self):
        skipped = {
            "name": "optional",
            "unit": "ms",
            "samples": [],
            "statistics": summarize([]),
            "workload": "skipped: fixture unavailable",
        }
        budgets = {
            "metric_budget": [
                {
                    "name": "optional probe",
                    "metric": "optional",
                    "statistic": "p95",
                    "max": 10.0,
                    "allow_skipped": True,
                }
            ]
        }
        self.assertTrue(evaluate_budgets({"metrics": [skipped]}, budgets)[0]["passed"])

    def test_cross_commit_comparison_reports_direction_and_percent(self):
        comparison = compare_reports(report(10.0), report(12.0))[0]
        self.assertEqual(comparison["metric"], "work")
        self.assertAlmostEqual(comparison["change_percent"], 20.0)

    def test_baseline_comparison_skips_changed_workload_without_losing_control(self):
        baseline = comparable_report(10.0)
        current = comparable_report(1.0)
        current["metrics"][0]["workload"] = "smaller work fixture v2"
        diagnostics, comparable = comparison_diagnostics(baseline, current)
        self.assertTrue(diagnostics["passed"])
        self.assertEqual(comparable, {"control"})
        self.assertEqual(diagnostics["skipped_metrics"], [{
            "metric": "work",
            "reason": "metric workload is missing, invalid, or changed",
            "baseline_workload": "fixed work fixture v1",
            "current_workload": "smaller work fixture v2",
        }])
        for selected in [None, comparable, {"work", "control"}]:
            comparisons = compare_reports(baseline, current, selected)
            self.assertEqual([item["metric"] for item in comparisons], ["control"])

    def test_baseline_comparison_rejects_missing_or_invalid_workload_on_either_side(self):
        for invalid in [None, "", " \t", 17, False, [], {}, "skipped: unavailable"]:
            for sides in [(0,), (1,), (0, 1)]:
                with self.subTest(invalid=invalid, sides=sides):
                    reports = [comparable_report(10.0), comparable_report(1.0)]
                    for side in sides:
                        if invalid is None:
                            reports[side]["metrics"][0].pop("workload")
                        else:
                            reports[side]["metrics"][0]["workload"] = invalid
                    diagnostics, comparable = comparison_diagnostics(*reports)
                    self.assertTrue(diagnostics["passed"])
                    self.assertEqual(comparable, {"control"})
                    self.assertEqual(len(diagnostics["skipped_metrics"]), 1)
                    self.assertEqual([item["metric"] for item in compare_reports(*reports)], ["control"])

    def test_baseline_comparison_with_no_matching_workloads_has_no_percentages(self):
        baseline = comparable_report(10.0)
        current = comparable_report(1.0)
        for row in current["metrics"]:
            row["workload"] += " changed"
        diagnostics, comparable = comparison_diagnostics(baseline, current)
        self.assertFalse(diagnostics["passed"])
        self.assertEqual(comparable, set())
        self.assertEqual(len(diagnostics["skipped_metrics"]), 2)
        self.assertTrue(any("no metrics comparable" in error for error in diagnostics["errors"]))
        self.assertEqual(compare_reports(baseline, current), [])

    def test_cli_artifact_records_workload_skip_without_false_percentage(self):
        baseline = comparable_report(10.0)
        current = comparable_report(1.0)
        current["metrics"][0]["workload"] = "smaller work fixture v2"
        contract = {"schema_version": 1, "metric": [
            {"name": "work", "unit": "ms", "producer": "application", "samples": 5},
            {"name": "control", "unit": "ms", "producer": "core", "samples": 5},
        ]}
        # Exercise artifact publication without launching Rust, a display, or
        # any external client. The unrelated real-suite budget inventory is
        # outside this two-row comparison fixture.
        with tempfile.TemporaryDirectory() as directory:
            baseline_path = Path(directory) / "baseline.json"
            output_path = Path(directory) / "result.json"
            baseline_path.write_text(json.dumps(baseline), encoding="utf-8")
            with (
                patch("sys.argv", ["run_benchmarks.py", "--skip-displayed", "--baseline", str(baseline_path), "--output", str(output_path)]),
                patch("scripts.run_benchmarks.load_metric_contract", return_value=contract),
                patch("scripts.run_benchmarks.run_rust_suite", side_effect=[[current["metrics"][1]], [current["metrics"][0]]]) as producer,
                patch("scripts.run_benchmarks.skipped_display_metrics", return_value=[]),
                patch("scripts.run_benchmarks.environment_metadata", return_value=current["environment"]),
                patch("scripts.run_benchmarks.evaluate_budgets", return_value=[]),
                patch("builtins.print"),
            ):
                main()
            self.assertEqual(producer.call_count, 2)
            result = json.loads(output_path.read_text(encoding="utf-8"))
            self.assertTrue(result["metric_contract"]["passed"])
            self.assertTrue(result["comparison_contract"]["passed"])
            self.assertEqual(result["comparison_contract"]["comparable_metrics"], ["control"])
            self.assertEqual(result["comparison_contract"]["skipped_metrics"][0]["metric"], "work")
            self.assertEqual([row["metric"] for row in result["comparison"]], ["control"])

    def test_baseline_comparison_rejects_a_different_machine(self):
        baseline = comparable_report(10.0)
        current = comparable_report(12.0)
        current["environment"]["cpu_model"] = "different cpu"

        diagnostics, comparable = comparison_diagnostics(baseline, current)

        self.assertFalse(diagnostics["passed"])
        self.assertEqual(comparable, set())
        self.assertTrue(any("cpu_model" in error for error in diagnostics["errors"]))
        self.assertEqual(compare_reports(baseline, current, comparable), [])

    def test_baseline_comparison_skips_only_context_incompatible_metrics(self):
        baseline = comparable_report(10.0)
        current = comparable_report(12.0)
        baseline["metrics"][0]["context"] = "llvmpipe"
        current["metrics"][0]["context"] = "NVIDIA"

        diagnostics, comparable = comparison_diagnostics(baseline, current)

        self.assertTrue(diagnostics["passed"])
        self.assertEqual(comparable, {"control"})
        self.assertEqual(
            diagnostics["skipped_metrics"],
            [
                {
                    "metric": "work",
                    "reason": "metric context is missing or changed",
                    "baseline_context": "llvmpipe",
                    "current_context": "NVIDIA",
                }
            ],
        )
        self.assertEqual(
            [item["metric"] for item in compare_reports(baseline, current, comparable)],
            ["control"],
        )

    def test_cross_commit_comparison_does_not_compare_different_units(self):
        baseline = report(10.0)
        current = report(12.0)
        current["metrics"][0]["unit"] = "seconds"
        self.assertEqual(
            [item["metric"] for item in compare_reports(baseline, current)],
            ["control"],
        )


if __name__ == "__main__":
    unittest.main()
