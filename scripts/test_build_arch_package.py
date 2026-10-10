from pathlib import Path
import tempfile
import unittest

from scripts.build_arch_package import BuildError, copy_checkout, recipe_version


class ArchPackageBuildTests(unittest.TestCase):
    def test_copy_checkout_keeps_git_identity_and_excludes_build_trees(self):
        with tempfile.TemporaryDirectory() as directory:
            temporary = Path(directory)
            source = temporary / "source"
            source.mkdir()
            (source / "Cargo.toml").write_text("[workspace]\n")
            (source / ".git").mkdir()
            (source / ".git/HEAD").write_text("ref: refs/heads/test\n")
            (source / "src").mkdir()
            (source / "src/main.rs").write_text("fn main() {}\n")
            (source / "target").mkdir()
            (source / "target/artifact").write_text("large\n")
            (source / "nested/pkg").mkdir(parents=True)
            (source / "nested/pkg/kept").write_text("kept\n")

            destination = temporary / "destination"
            copy_checkout(source, destination)

            self.assertTrue((destination / ".git/HEAD").is_file())
            self.assertTrue((destination / "src/main.rs").is_file())
            self.assertFalse((destination / "target").exists())
            self.assertTrue((destination / "nested/pkg/kept").is_file())

    def test_copy_checkout_rejects_non_git_source(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory)
            (source / "Cargo.toml").write_text("[workspace]\n")
            with self.assertRaisesRegex(BuildError, "Git-backed"):
                copy_checkout(source, source / "output")

    def test_release_recipe_version_is_read_from_its_one_pkgver(self):
        self.assertEqual(recipe_version("pkgname=termeric\npkgver=0.1.0\npkgrel=1\n"), "0.1.0")
        for recipe in ["pkgname=termeric\n", "pkgver=0.1.0\npkgver=0.2.0\n"]:
            with self.subTest(recipe=recipe), self.assertRaisesRegex(BuildError, "exactly once"):
                recipe_version(recipe)


if __name__ == "__main__":
    unittest.main()
