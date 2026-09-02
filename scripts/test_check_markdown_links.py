import unittest
from pathlib import Path

from scripts.check_markdown_links import (
    github_slug,
    link_destination,
    local_reference,
    markdown_anchors,
)


class MarkdownLinkContractTests(unittest.TestCase):
    def test_link_destination_strips_title_and_angle_brackets(self):
        self.assertEqual(link_destination("guide.md#install 'Install guide'"), "guide.md#install")
        self.assertEqual(link_destination("<guide with spaces.md#install> 'Guide'"), "guide with spaces.md#install")

    def test_local_reference_keeps_same_file_anchor(self):
        source = Path("/repo/docs/guide.md")
        self.assertEqual(
            local_reference(source, "#install"),
            (source, "install"),
        )

    def test_external_reference_is_ignored(self):
        self.assertIsNone(
            local_reference(Path("/repo/README.md"), "https://example.com/a#b")
        )

    def test_github_slug_removes_formatting_and_punctuation(self):
        self.assertEqual(github_slug("Build & `Test`"), "build--test")
        self.assertEqual(github_slug("API_v2: Safety"), "api_v2-safety")

    def test_markdown_anchors_ignore_fences_and_suffix_duplicates(self):
        anchors = markdown_anchors(
            "# Title\n\n## Repeat\n\n```sh\n# not-a-heading\n```\n\n## Repeat\n"
        )
        self.assertEqual(anchors, {"title", "repeat", "repeat-1"})


if __name__ == "__main__":
    unittest.main()
