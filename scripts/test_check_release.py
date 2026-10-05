import unittest

from scripts.check_release import (
    METAINFO,
    ROOT,
    ReleaseError,
    absolute_links,
    changelog_section,
    check_release,
    metainfo_release_date,
    stamp_release,
)


CARGO_TOML = """\
[workspace.package]
version = "1.2.0"
repository = "https://example.org/termeric"
"""

CHANGELOG = """\
# Changelog

## [Unreleased]

- Not yet.

## [1.2.0] - 2026-11-02

The second release. See [Status](docs/src/status.md) or [the site](https://example.org/#x).

### Added

- Things.

## [1.1.0] - 2026-10-01

- Older things.

[1.2.0]: https://example.org/compare/v1.1.0...v1.2.0
"""

METAINFO_XML = """\
<component type="desktop-application">
  <releases>
    <release version="1.2.0" date="2026-11-02"/>
    <release version="1.1.0" date="2026-10-01"/>
  </releases>
</component>
"""


class ReleaseCheckTests(unittest.TestCase):
    def test_section_ends_at_the_next_version(self):
        date, body = changelog_section(CHANGELOG, "1.2.0")
        self.assertEqual(date, "2026-11-02")
        self.assertTrue(body.startswith("The second release."))
        self.assertTrue(body.endswith("- Things."))
        _, body = changelog_section(CHANGELOG, "1.1.0")
        self.assertEqual(body, "- Older things.")
        with self.assertRaisesRegex(ReleaseError, "no section for 9.9.9"):
            changelog_section(CHANGELOG, "9.9.9")

    def test_unreleased_section_has_no_date(self):
        date, _ = changelog_section(CHANGELOG.replace("[1.2.0] - 2026-11-02", "[1.2.0] - Unreleased"), "1.2.0")
        self.assertIsNone(date)

    def test_release_passes_with_absolute_links_in_its_notes(self):
        version, notes = check_release(CARGO_TOML, CHANGELOG, METAINFO_XML, tag="v1.2.0", publish=True)
        self.assertEqual(version, "1.2.0")
        self.assertIn("[Status](https://example.org/termeric/blob/v1.2.0/docs/src/status.md)", notes)
        self.assertIn("[the site](https://example.org/#x)", notes)

    def test_publishing_rejects_a_mismatch(self):
        cases = [
            ({"tag": "v1.2.1"}, "does not match the workspace version 1.2.0"),
            ({"tag": None}, "needs the release tag"),
            ({"changelog": CHANGELOG.replace("[1.2.0] - 2026-11-02", "[1.2.0] - Unreleased")}, "still Unreleased"),
            ({"metainfo": METAINFO_XML.replace('date="2026-11-02"', 'date="2026-11-03"')}, "dated 2026-11-03"),
            ({"metainfo": METAINFO_XML.replace('version="1.2.0" ', "")}, "no release entry for 1.2.0"),
        ]
        for change, message in cases:
            arguments = {"changelog": CHANGELOG, "metainfo": METAINFO_XML, "tag": "v1.2.0", **change}
            with self.subTest(message=message), self.assertRaisesRegex(ReleaseError, message):
                check_release(
                    CARGO_TOML,
                    arguments["changelog"],
                    arguments["metainfo"],
                    tag=arguments["tag"],
                    publish=True,
                )

    def test_dry_run_accepts_an_unreleased_section_without_a_tag(self):
        changelog = CHANGELOG.replace("[1.2.0] - 2026-11-02", "[1.2.0] - Unreleased")
        version, notes = check_release(CARGO_TOML, changelog, METAINFO_XML, tag=None, publish=False)
        self.assertEqual(version, "1.2.0")
        self.assertIn("/blob/main/docs/src/status.md", notes)

    def test_relative_links_only_are_rewritten(self):
        self.assertEqual(
            absolute_links("[a](b.md) [c](#d) [e](mailto:x@y)", "https://h/"),
            "[a](https://h/b.md) [c](#d) [e](mailto:x@y)",
        )

    def test_repository_metadata_has_the_current_version(self):
        cargo_toml = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
        version, _ = check_release(
            cargo_toml,
            (ROOT / "CHANGELOG.md").read_text(encoding="utf-8"),
            METAINFO.read_text(encoding="utf-8"),
            tag=None,
            publish=False,
        )
        self.assertRegex(metainfo_release_date(METAINFO.read_text(encoding="utf-8"), version), r"^\d{4}-\d{2}-\d{2}$")


class ReleaseStampTests(unittest.TestCase):
    def test_stamping_dates_both_files_for_one_version_only(self):
        changelog = CHANGELOG.replace("## [1.2.0] - 2026-11-02", "## [1.2.0] - Unreleased")
        changelog, metainfo = stamp_release(changelog, METAINFO_XML, "1.2.0", "2026-12-24")
        self.assertIn("## [1.2.0] - 2026-12-24", changelog)
        self.assertIn("## [1.1.0] - 2026-10-01", changelog)
        self.assertIn('<release version="1.2.0" date="2026-12-24"/>', metainfo)
        self.assertIn('<release version="1.1.0" date="2026-10-01"/>', metainfo)
        self.assertEqual(changelog_section(changelog, "1.2.0")[0], "2026-12-24")
        self.assertEqual(metainfo_release_date(metainfo, "1.2.0"), "2026-12-24")

    def test_stamping_refuses_a_bad_date_or_a_missing_version(self):
        with self.assertRaisesRegex(ReleaseError, "not a YYYY-MM-DD date"):
            stamp_release(CHANGELOG, METAINFO_XML, "1.2.0", "2026-13-01")
        with self.assertRaisesRegex(ReleaseError, "no section for 9.9.9"):
            stamp_release(CHANGELOG, METAINFO_XML, "9.9.9", "2026-12-24")
        with self.assertRaisesRegex(ReleaseError, "no release entry for 1.0.0"):
            stamp_release(
                CHANGELOG.replace("[1.1.0] - 2026-10-01", "[1.0.0] - 2026-10-01"),
                METAINFO_XML,
                "1.0.0",
                "2026-12-24",
            )


if __name__ == "__main__":
    unittest.main()
