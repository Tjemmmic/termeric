import unittest

from scripts.check_feature_support import ROOT, validate, validate_release_dry_run


class FeatureSupportContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.readme = (ROOT / "README.md").read_text(encoding="utf-8")
        cls.matrix = (ROOT / "docs/feature-support.md").read_text(encoding="utf-8")
        cls.release = (ROOT / "docs/release-acceptance.md").read_text(encoding="utf-8")

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


if __name__ == "__main__":
    unittest.main()
