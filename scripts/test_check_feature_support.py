import unittest

from scripts.check_feature_support import (
    ROOT,
    validate,
    validate_benchmark_metric_count,
    validate_release_dry_run,
)


class FeatureSupportContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.readme = (ROOT / "README.md").read_text(encoding="utf-8")
        cls.matrix = (ROOT / "docs/feature-support.md").read_text(encoding="utf-8")
        cls.release = (ROOT / "docs/release-acceptance.md").read_text(encoding="utf-8")
        cls.metrics = (ROOT / "benchmarks/metrics.toml").read_bytes()

    def test_repository_contract_passes(self):
        self.assertEqual(validate(self.readme, self.matrix), [])

    def test_unknown_status_fails(self):
        changed = self.matrix.replace(
            "| Scrollback memory | **Partial**",
            "| Scrollback memory | **Working**",
        )
        self.assertTrue(
            any(
                "unknown status" in failure
                for failure in validate(self.readme, changed)
            )
        )

    def test_new_unmapped_readme_highlight_fails(self):
        changed = self.readme.replace(
            "## Status", "- **Unreviewed capability** — unsupported claim\n\n## Status"
        )
        self.assertTrue(
            any("has no reviewed matrix mapping" in failure for failure in validate(changed, self.matrix))
        )

    def test_missing_mapped_capability_fails(self):
        changed = self.matrix.replace(
            "| Themes | **Supported**", "| Theme collection | **Supported**"
        )
        self.assertTrue(
            any(
                "references missing capability 'Themes'" in failure
                for failure in validate(self.readme, changed)
            )
        )

    def test_release_dry_run_contract_passes(self):
        self.assertEqual(validate_release_dry_run(self.release), [])

    def test_release_dry_run_requires_every_gate(self):
        changed = self.release.replace(
            "| 9 | Manual record validation |",
            "| 10 | Manual record validation |",
            1,
        )
        self.assertTrue(
            any(
                "gates 1 through 9" in failure
                for failure in validate_release_dry_run(changed)
            )
        )

    def test_benchmark_metric_count_matches_manifest(self):
        self.assertEqual(
            validate_benchmark_metric_count(self.matrix, self.metrics), []
        )

    def test_benchmark_metric_count_rejects_documentation_drift(self):
        changed = self.matrix.replace("62-metric contract", "61-metric contract")
        self.assertTrue(
            any(
                "manifest contains 62" in failure
                for failure in validate_benchmark_metric_count(changed, self.metrics)
            )
        )


if __name__ == "__main__":
    unittest.main()
