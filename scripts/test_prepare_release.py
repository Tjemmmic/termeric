import tomllib
import unittest

from scripts.check_release import ROOT, ReleaseError, changelog_section, metainfo_release_date, workspace_package
from scripts.prepare_release import (
    CARGO_TOML,
    CHANGELOG,
    FILES,
    LOCK_FILES,
    METAINFO,
    add_metainfo_release,
    date_changelog,
    prepare,
    replace_once,
    version_key,
)


# Later than any version the repository will name while these tests run.
VERSION = "999.0.0"
DATE = "2026-12-24"
UNRELEASED = "# Changelog\n\n## [Unreleased]\n\n### Added\n\n- Roots.\n\n## [1.0.0] - 2026-01-01\n\n- Soil.\n"


class PrepareReleaseTests(unittest.TestCase):
    def setUp(self):
        self.files = {path: (ROOT / path).read_text(encoding="utf-8") for path in FILES}
        # Right after a release the section is empty, which prepare refuses.
        self.files[CHANGELOG] = self.files[CHANGELOG].replace(
            "## [Unreleased]\n", "## [Unreleased]\n\n### Added\n\n- Roots.\n", 1
        )

    def test_repository_files_all_take_the_new_version(self):
        current = workspace_package(self.files[CARGO_TOML], "version")
        prepared = prepare(self.files, VERSION, DATE)
        self.assertEqual(prepared.keys(), self.files.keys())
        for path, text in prepared.items():
            with self.subTest(path=path):
                self.assertNotEqual(text, self.files[path])
                self.assertIn(VERSION, text)
                # Only history and other crates may still name the old version.
                if path not in {CHANGELOG, METAINFO, *LOCK_FILES}:
                    self.assertNotIn(current, text)
        for path, crates in LOCK_FILES.items():
            locked = {
                package["name"]: package["version"]
                for package in tomllib.loads(prepared[path])["package"]
            }
            self.assertEqual({locked[crate] for crate in crates}, {VERSION})
        self.assertEqual(metainfo_release_date(prepared[METAINFO], VERSION), DATE)

    def test_prepared_changelog_keeps_an_empty_unreleased_section(self):
        changelog = date_changelog(UNRELEASED, "1.1.0", DATE)
        self.assertIn(f"## [Unreleased]\n\n## [1.1.0] - {DATE}\n\n### Added\n", changelog)
        self.assertEqual(changelog_section(changelog, "1.1.0"), (DATE, "### Added\n\n- Roots."))
        with self.assertRaisesRegex(ReleaseError, "section is empty"):
            date_changelog(changelog, "1.2.0", DATE)
        with self.assertRaisesRegex(ReleaseError, "no \\[Unreleased\\] section"):
            date_changelog("# Changelog\n", "1.2.0", DATE)

    def test_metainfo_release_comes_first(self):
        metainfo = add_metainfo_release(
            '<component>\n  <releases>\n    <release version="1.0.0" date="2026-01-01"/>\n  </releases>\n</component>\n',
            "1.1.0",
            DATE,
            "https://example.org/termeric",
        )
        self.assertLess(metainfo.index('version="1.1.0"'), metainfo.index('version="1.0.0"'))
        self.assertIn("https://example.org/termeric/releases/tag/v1.1.0", metainfo)

    def test_version_must_be_newer_and_plain(self):
        current = workspace_package(self.files[CARGO_TOML], "version")
        with self.assertRaisesRegex(ReleaseError, "is not newer than"):
            prepare(self.files, current, DATE)
        for version in ["1.2", "v1.2.0", "1.2.0-rc1", "1.2.0\n"]:
            with self.subTest(version=version), self.assertRaisesRegex(ReleaseError, "not a version"):
                prepare(self.files, version, DATE)
        self.assertLess(version_key("0.9.0"), version_key("0.10.0"))

    def test_a_missing_or_repeated_place_stops_the_release(self):
        for text, count in [("", 0), ("pkgver=1\npkgver=2\n", 2)]:
            with self.subTest(count=count), self.assertRaisesRegex(ReleaseError, f"one pkgver, found {count}"):
                replace_once(text, r"^pkgver=.*$", "pkgver=3", "pkgver")


if __name__ == "__main__":
    unittest.main()
