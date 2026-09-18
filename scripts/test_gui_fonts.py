"""Hermetic font prerequisite tests; no installed fonts or GUI required."""
import json
from pathlib import Path
import subprocess
import tempfile
import tomllib
import unittest
from unittest import mock

from scripts import gui_acceptance as gui, gui_fonts as fonts


class GuiFontTests(unittest.TestCase):
    def probe(self, result, primary="Noto Sans Mono", fails=False):
        with tempfile.TemporaryDirectory() as directory:
            environment = {"HOME": directory, "XDG_CONFIG_HOME": directory + "/config",
                           "XDG_DATA_HOME": directory + "/data", "PATH": "/fixture/bin"}
            kwargs = {"side_effect": result} if isinstance(result, Exception) else {"return_value": result}
            with mock.patch.object(fonts.subprocess, "run", **kwargs) as run:
                if fails:
                    with self.assertRaises(ValueError):
                        fonts.check_font_profile(Path(directory), environment, primary)
                else:
                    fonts.check_font_profile(Path(directory), environment, primary)
                self.assertEqual(run.call_args.args[0], ["fc-list", "--format=%{family}\\n"])
                self.assertIs(run.call_args.kwargs["env"], environment)
                self.assertEqual(run.call_args.kwargs["timeout"], 10)
            return json.loads((Path(directory) / "font-profile.json").read_text())

    def test_installed_aliases_are_exact_case_insensitive_and_not_coverage_claims(self):
        record = self.probe(subprocess.CompletedProcess([], 0,
            "Alias,NOTO SANS MONO\nNoto Color Emoji\n", ""))
        self.assertEqual(record["status"], "passed")
        self.assertEqual(record["missing"], [])
        self.assertEqual(record["renderer_face"], "not_observed")
        self.assertEqual(record["glyph_coverage"], "not_observed")

    def test_missing_primary_and_fallback_fail_without_substring_or_substitution(self):
        for inventory, missing in (
            ("Noto Sans Mono Other\nNoto Color Emoji\n", ["Noto Sans Mono"]),
            ("Noto Sans Mono\n", ["Noto Color Emoji"]),
            ("", ["Noto Sans Mono", "Noto Color Emoji"]),
        ):
            record = self.probe(subprocess.CompletedProcess([], 0, inventory, ""), fails=True)
            self.assertEqual(record["missing"], missing)
            self.assertEqual(record["status"], "failed")

    def test_failed_missing_timed_out_and_oversize_probes_retain_failure(self):
        for result in (FileNotFoundError("no fc-list"), subprocess.TimeoutExpired("fc-list", 10),
                       subprocess.CompletedProcess([], 1, "", "broken inventory"),
                       subprocess.CompletedProcess([], 0, "x" * (1024 * 1024 + 1), "")):
            record = self.probe(result, fails=True)
            self.assertEqual(record["status"], "failed")
            self.assertTrue(record["error"])
            self.assertLess(len(json.dumps(record)), 8192)

    def test_custom_profiles_cannot_compare_or_update_default_visual_baselines(self):
        self.assertEqual(fonts.validate_profile_options(" Noto Sans Mono ", True, False), "Noto Sans Mono")
        self.assertEqual(fonts.validate_profile_options(fonts.DEFAULT_PRIMARY, False, True), fonts.DEFAULT_PRIMARY)
        for skip, update in ((False, False), (False, True), (True, True)):
            with self.assertRaises(ValueError):
                fonts.validate_profile_options("Noto Sans Mono", skip, update)
        for family in ("", "a\nb", "a\x7fb", "a,b", "é" * 65, "noto COLOR emoji"):
            with self.assertRaises(ValueError):
                fonts.validate_primary(family)

    def test_child_config_and_probe_share_explicit_family_and_isolated_environment(self):
        with tempfile.TemporaryDirectory() as directory, mock.patch.object(
            gui, "check_font_profile"
        ) as probe, mock.patch.object(gui.subprocess, "Popen", return_value=mock.Mock()):
            app = gui.AppProcess(Path("/fixture/app"), Path(directory), "wayland", "/bin/cat", [],
                                 {fonts.PRIMARY_ENV: 'Fixture "Mono"', "HOME": "/operator/home"})
            try:
                probe.assert_called_once_with(Path(directory), app.env, 'Fixture "Mono"')
                self.assertEqual(app.env["HOME"], directory + "/home")
                config = tomllib.loads((Path(directory) / "xdg-config/termeric/config.toml").read_text())
                self.assertEqual(config["font"]["family"], 'Fixture "Mono"')
                self.assertEqual(config["font"]["fallback"], [fonts.FALLBACK])
            finally:
                app.finish()

    def test_missing_fonts_prevent_app_launch(self):
        with tempfile.TemporaryDirectory() as directory, mock.patch.object(
            gui, "check_font_profile", side_effect=ValueError("missing font")
        ), mock.patch.object(gui.subprocess, "Popen") as launch:
            with self.assertRaisesRegex(ValueError, "missing font"):
                gui.AppProcess(Path("/fixture/app"), Path(directory), "wayland", "/bin/cat", [], {})
            launch.assert_not_called()


if __name__ == "__main__":
    unittest.main()
