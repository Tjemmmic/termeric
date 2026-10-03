import tomllib
import unittest

from scripts.run_benchmarks import BUDGETS, evaluate_budgets, merge_metrics, parse_metrics, percentile, summarize


def metric(name, samples, **extra):
    return {"name": name, "unit": "ms", "samples": samples, "statistics": summarize(samples), **extra}


class BenchmarkTests(unittest.TestCase):
    def test_percentiles_use_an_observed_sample(self):
        self.assertEqual(percentile([5.0, 1.0, 3.0, 2.0, 4.0], 0.95), 5.0)
        self.assertEqual(summarize([]), {"count": 0})

    def test_marked_lines_become_records_and_repeats_pool_their_samples(self):
        output = 'noise\nTERMERIC_BENCH {"name": "a", "unit": "ms", "samples": [1]}\n'
        records = parse_metrics(output) * 2
        self.assertEqual(merge_metrics(records)[0]["statistics"]["count"], 2)
        with self.assertRaises(ValueError):
            merge_metrics([records[0], {**records[0], "unit": "bytes"}])

    def test_metric_budgets_fail_over_the_limit_or_without_evidence(self):
        budget = {"name": "rail", "metric": "a", "statistic": "max", "max": 10.0}
        for samples, passes in [([10.0, 9.0], True), ([10.5], False)]:
            self.assertEqual(evaluate_budgets([metric("a", samples)], {"metric_budget": [budget]})[0]["passed"], passes)
        self.assertFalse(evaluate_budgets([], {"metric_budget": [budget]})[0]["passed"])
        skipped = metric("a", [], workload="skipped: no headless WGPU adapter")
        self.assertFalse(evaluate_budgets([skipped], {"metric_budget": [budget]})[0]["passed"])
        allowing = {**budget, "allow_skipped": True}
        self.assertTrue(evaluate_budgets([skipped], {"metric_budget": [allowing]})[0]["passed"])
        self.assertFalse(evaluate_budgets([], {"metric_budget": [allowing]})[0]["passed"])

    def test_ratio_budgets_compare_medians(self):
        budget = {"name": "ratio", "numerator": "big", "denominator": "small", "max_ratio": 4.0}
        for small, big, passes in [(10.0, 40.0, True), (10.0, 40.1, False), (0.0, 1.0, False)]:
            metrics = [metric("small", [small]), metric("big", [big])]
            self.assertEqual(evaluate_budgets(metrics, {"ratio_budget": [budget]})[0]["passed"], passes)
        self.assertFalse(evaluate_budgets([metric("big", [1.0])], {"ratio_budget": [budget]})[0]["passed"])

    def test_checked_in_budgets_are_well_formed(self):
        with BUDGETS.open("rb") as stream:
            budgets = tomllib.load(stream)
        self.assertEqual(set(budgets), {"schema_version", "metric_budget", "ratio_budget"})
        names = [budget["name"] for kind in ("metric_budget", "ratio_budget") for budget in budgets[kind]]
        self.assertEqual(len(names), len(set(names)))
        for budget in budgets["metric_budget"]:
            self.assertIn(budget["statistic"], {"min", "median", "p95", "max", "mean"})


if __name__ == "__main__":
    unittest.main()
