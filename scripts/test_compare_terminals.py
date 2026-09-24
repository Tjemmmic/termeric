import os
import io
import json
import subprocess
from dataclasses import replace
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from scripts.compare_terminals import (
    BenchmarkSettings,
    DISABLED_WAYLAND_DISPLAY,
    PRIVATE_SESSION_BUS_MARKER,
    SUPPORTED_TERMINALS,
    build_launch_spec,
    comparison_contract,
    direct_children,
    font_environment,
    idle_cpu_measurement,
    main,
    parse_resource_status,
    process_tree_sample,
    run_sample,
    stat_fields,
    summarize,
    terminal_resources,
    terminal_summary,
    window_geometry,
    x11_environment,
)
from scripts.terminal_benchmark_fixture import emit_workload


SETTINGS = BenchmarkSettings(
    font_family="DejaVu Sans Mono",
    font_size=12.0,
    columns=100,
    rows=30,
    scrollback_lines=10_000,
    window_width=1000,
    window_height=700,
)


def font_observation() -> dict:
    return {
        "requested_family": SETTINGS.font_family, "status": "exact", "errors": [],
        "requested_family_installed": True,
        "renderer_face": {"status": "not_observed"},
        "match": {"families": [SETTINGS.font_family], "file": "/fixture/font.ttf", "index": 0, "sha256": "a" * 64},
    }


def sample(
    *,
    columns: int = 100,
    rows: int = 30,
    ready_columns: int = 100,
    ready_rows: int = 30,
    width: int = 1000,
    height: int = 700,
    output_lines: int = 8_000,
) -> dict:
    return {
        "font_environment": font_observation(),
        "fixture": {
            "columns": columns,
            "rows": rows,
            "output_lines": output_lines,
            "pty_response_ms": 1.0,
        },
        "fixture_ready": {"columns": ready_columns, "rows": ready_rows},
        "window_geometry": {"width": width, "height": height},
        "startup_to_visible_ms": 2.0,
        "resources": {
            "rss_kib": 100, "idle_cpu_percent_one_core": 0.0,
            "measurement_status": {"rss_kib": "complete", "idle_cpu_percent_one_core": "complete"},
        },
    }


def terminal(name: str, run_sample: dict) -> dict:
    return {
        "terminal": name,
        "status": "complete",
        "controls": {},
        "samples": [run_sample],
    }


class CrossTerminalBenchmarkTests(unittest.TestCase):
    def test_font_probe_checks_inventory_and_hashes_exact_face_in_isolated_environment(self):
        environment = {"XDG_CONFIG_HOME": "/isolated/config", "XDG_CACHE_HOME": "/isolated/cache"}
        replies = [
            {"status": "complete", "output": "Other\nDejaVu Sans Mono,Alias"},
            {"status": "complete", "output": "DejaVu Sans Mono,Alias\n/fixture/font.ttf\n2"},
        ]
        with patch("scripts.compare_terminals.command_capture", side_effect=replies) as probe, \
                patch("scripts.compare_terminals.file_sha256", return_value="a" * 64) as digest:
            result = font_environment(SETTINGS.font_family, environment)
        self.assertEqual(result["status"], "exact")
        self.assertTrue(result["requested_family_installed"])
        self.assertEqual(result["match"]["index"], 2)
        self.assertEqual(result["match"]["sha256"], "a" * 64)
        self.assertEqual(result["renderer_face"], {"status": "not_observed"})
        self.assertEqual([call.args[0][0] for call in probe.call_args_list], ["fc-list", "fc-match"])
        self.assertTrue(all(call.args[1] == environment for call in probe.call_args_list))
        digest.assert_called_once_with("/fixture/font.ttf")

    def test_font_probe_does_not_accept_silent_substitution_or_inventory_alone(self):
        for inventory, match in [("Noto Sans Mono", "Noto Sans Mono"), (SETTINGS.font_family, "Noto Sans Mono"), ("Other", SETTINGS.font_family)]:
            with self.subTest(inventory=inventory, match=match), \
                    patch("scripts.compare_terminals.command_capture", side_effect=[
                        {"status": "complete", "output": inventory},
                        {"status": "complete", "output": f"{match}\n/fixture/font.ttf\n0"},
                    ]), patch("scripts.compare_terminals.file_sha256", return_value="a" * 64):
                result = font_environment(SETTINGS.font_family, {})
            self.assertEqual(result["status"], "substituted")
            self.assertTrue(result["errors"])

    def test_font_probe_failures_are_explicit_and_do_not_prevent_raw_measurements(self):
        cases = [
            [{"status": "unavailable"}, {"status": "complete", "output": ""}],
            [{"status": "failed"}, {"status": "failed"}],
        ]
        cases += [[{"status": "complete", "output": SETTINGS.font_family}, {"status": "complete", "output": value}]
                  for value in ("", "Family\nrelative.ttf\n0", "Family\n/font.ttf\n-1", "Family\n/font.ttf\nnan", "Family\n/font.ttf\n0\nextra")]
        for replies in cases:
            with self.subTest(replies=replies), patch("scripts.compare_terminals.command_capture", side_effect=replies):
                result = font_environment(SETTINGS.font_family, {})
            self.assertEqual(result["status"], "unavailable")
            self.assertTrue(result["errors"])
        for error in (FileNotFoundError("missing"), PermissionError("denied")):
            with self.subTest(error=error), patch("scripts.compare_terminals.command_capture", side_effect=error):
                self.assertEqual(font_environment(SETTINGS.font_family, {})["status"], "unavailable")
        with patch("scripts.compare_terminals.command_capture", side_effect=[
            {"status": "complete", "output": SETTINGS.font_family},
            {"status": "complete", "output": f"{SETTINGS.font_family}\n/font.ttf\n0"},
        ]), patch("scripts.compare_terminals.file_sha256", side_effect=PermissionError("denied")):
            self.assertEqual(font_environment(SETTINGS.font_family, {})["status"], "unavailable")

    def test_font_patterns_and_empty_names_are_not_misrepresented_as_literal_families(self):
        for name in ("", " ", "Missing:family=Installed", "One,Two", "Font\\Name", "Font\nName"):
            with self.subTest(name=name), patch("scripts.compare_terminals.command_capture") as probe:
                self.assertEqual(font_environment(name, {})["status"], "unavailable")
                probe.assert_not_called()

    def test_missing_substituted_and_malformed_font_observations_block_all_rankings(self):
        bad = [None, {}, {**font_observation(), "status": "substituted"},
               {**font_observation(), "requested_family_installed": 1},
               {**font_observation(), "requested_family": "Different"},
               {**font_observation(), "errors": ["failed"]}]
        bad += [{**font_observation(), "match": {**font_observation()["match"], key: value}}
                for key, value in (("families", []), ("families", [None]), ("families", ["Other"]),
                                   ("index", True), ("index", -1), ("sha256", "invalid"), ("file", "relative"))]
        for observation in bad:
            with self.subTest(observation=observation):
                first = sample(); first["font_environment"] = observation
                report = comparison_contract([terminal("termeric", first), terminal("kitty", sample())], SETTINGS)
                self.assertTrue(all(not value for value in report["ranking_permitted"].values()))
        old = sample(); del old["font_environment"]
        self.assertFalse(comparison_contract([terminal("termeric", old), terminal("kitty", old)], SETTINGS)["comparable"])

    def test_font_fingerprints_are_checked_for_every_sample_without_claiming_renderer_equivalence(self):
        for key, value in (("sha256", "b" * 64), ("index", 1)):
            with self.subTest(key=key):
                first = terminal("termeric", sample())
                later = sample(); later["font_environment"]["match"][key] = value
                first["samples"].append(later)
                report = comparison_contract([first, terminal("kitty", sample())], SETTINGS)
                self.assertTrue(all(not value for value in report["ranking_permitted"].values()))
        report = comparison_contract([terminal("termeric", sample()), terminal("kitty", sample())], SETTINGS)
        self.assertTrue(report["comparable"])
        self.assertEqual(report["font_control"]["renderer_face_equivalence"], "not_verified")

    def test_termeric_calibration_overrides_are_explicit_and_do_not_change_other_adapters(self):
        calibrated = replace(SETTINGS, termeric_font_size=16.0, termeric_line_height=1.3)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for name in SUPPORTED_TERMINALS:
                with self.subTest(terminal=name):
                    run_dir = root / name
                    spec = build_launch_spec(name, f"/fixture/{name}", run_dir, "title", ["fixture"], calibrated, {"DISPLAY": ":99"})
                    expected = 16.0 if name == "termeric" else 12.0
                    self.assertEqual(spec.controls["font_size_requested"], expected)
                    if name == "termeric":
                        config = (run_dir / "xdg-config/termeric/config.toml").read_text()
                        self.assertIn("\nsize = 16.0\nline_height = 1.3\n", config)
                        self.assertEqual(spec.controls["line_height_requested"], 1.3)
                        self.assertEqual(spec.controls["line_height_unit"], "font-size multiplier")
                    else:
                        self.assertNotIn("line_height_requested", spec.controls)
                        original_dir = root / f"original-{name}"
                        original = build_launch_spec(name, f"/fixture/{name}", original_dir, "title", ["fixture"], SETTINGS, {"DISPLAY": ":99"})
                        self.assertEqual(spec.controls, original.controls)
                        self.assertEqual(
                            [arg.replace(str(run_dir), "CONFIG") for arg in spec.command],
                            [arg.replace(str(original_dir), "CONFIG") for arg in original.command],
                        )
                        for path in run_dir.rglob("*"):
                            if path.is_file():
                                self.assertEqual(path.read_bytes(), (original_dir / path.relative_to(run_dir)).read_bytes())
            default_dir = root / "default-termeric"
            default = build_launch_spec("termeric", "/fixture/termeric", default_dir, "title", ["fixture"], SETTINGS, {})
            self.assertEqual(default.controls["font_size_requested"], 12.0)
            self.assertEqual(default.controls["line_height_requested"], 1.0)

    def test_invalid_calibration_values_fail_before_any_external_command(self):
        cases = [("--termeric-font-size", value) for value in ("7", "73", "nan", "inf", "-inf")]
        cases += [("--termeric-line-height", value) for value in ("0.7", "3.1", "nan", "inf", "-inf")]
        cases += [("--font-size", value) for value in ("7", "73", "nan", "inf", "-inf")]
        cases += [(flag, value) for flag in ("--idle-seconds", "--timeout-seconds") for value in ("0", "nan", "inf")]
        for flag, value in cases:
            with self.subTest(flag=flag, value=value), \
                    patch("sys.argv", ["compare_terminals.py", "--terminal", "termeric", f"{flag}={value}"]), \
                    patch("sys.stderr", new_callable=io.StringIO), \
                    patch("scripts.compare_terminals.enter_private_session_bus") as bus, \
                    patch("scripts.compare_terminals.subprocess.run") as command:
                with self.assertRaises(SystemExit) as error:
                    main()
                self.assertEqual(error.exception.code, 2)
                bus.assert_not_called()
                command.assert_not_called()

    def test_calibration_cli_records_settings_without_implicitly_selecting_terminals(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "report.json"
            with patch("sys.argv", ["compare_terminals.py", "--terminal", "termeric", "--termeric-font-size=16", "--termeric-line-height=1.3", "--output", str(output)]), \
                    patch.dict(os.environ, {"DISPLAY": ":99"}), \
                    patch("scripts.compare_terminals.platform.system", return_value="Linux"), \
                    patch("scripts.compare_terminals.Path.is_file", return_value=True), \
                    patch("scripts.compare_terminals.shutil.which", return_value="/fixture/xdotool"), \
                    patch("scripts.compare_terminals.enter_private_session_bus"), \
                    patch("scripts.compare_terminals.executable_for", return_value=None) as executable, \
                    patch("scripts.compare_terminals.command_capture", return_value={}), \
                    patch("scripts.compare_terminals.run_sample") as launch, \
                    patch("sys.stdout", new_callable=io.StringIO):
                self.assertEqual(main(), 1)  # Selected executable unavailable, not a false pass.
            report = json.loads(output.read_text())
            self.assertEqual(report["schema"], 3)
            self.assertEqual(report["comparison_contract"]["font_control"]["renderer_face_equivalence"], "not_verified")
            self.assertEqual(report["settings"]["font_size"], 12.0)
            self.assertEqual(report["settings"]["termeric_font_size"], 16.0)
            self.assertEqual(report["settings"]["termeric_line_height"], 1.3)
            self.assertEqual([row["terminal"] for row in report["terminals"]], ["termeric"])
            executable.assert_called_once()
            launch.assert_not_called()

    def test_x11_environment_removes_parent_wayland_and_toolkit_overrides(self):
        environment = x11_environment(
            {
                "DISPLAY": ":99",
                "WAYLAND_DISPLAY": "wayland-0",
                "WAYLAND_SOCKET": "42",
                "EGL_PLATFORM": "wayland",
                "XDG_SESSION_TYPE": "wayland",
                "WINIT_UNIX_BACKEND": "wayland",
                "GDK_BACKEND": "wayland",
                "QT_QPA_PLATFORM": "wayland",
                "DBUS_SESSION_BUS_ADDRESS": "unix:path=/run/user/1000/bus",
                "DBUS_STARTER_ADDRESS": "unix:path=/run/user/1000/bus",
                "DBUS_STARTER_BUS_TYPE": "session",
            }
        )
        self.assertEqual(environment["DISPLAY"], ":99")
        self.assertEqual(environment["XDG_SESSION_TYPE"], "x11")
        self.assertEqual(environment["EGL_PLATFORM"], "x11")
        self.assertEqual(environment["WAYLAND_DISPLAY"], DISABLED_WAYLAND_DISPLAY)
        for variable in (
            "WINIT_UNIX_BACKEND",
            "GDK_BACKEND",
            "QT_QPA_PLATFORM",
            "DBUS_SESSION_BUS_ADDRESS",
            "DBUS_STARTER_ADDRESS",
            "DBUS_STARTER_BUS_TYPE",
        ):
            self.assertNotIn(variable, environment)
        self.assertEqual(environment["WAYLAND_SOCKET"], "")

    def test_x11_environment_preserves_only_a_marked_private_session_bus(self):
        private = x11_environment(
            {
                "DISPLAY": ":99",
                PRIVATE_SESSION_BUS_MARKER: "1",
                "DBUS_SESSION_BUS_ADDRESS": "unix:path=/tmp/private-bus",
            }
        )
        self.assertEqual(private["DBUS_SESSION_BUS_ADDRESS"], "unix:path=/tmp/private-bus")
        self.assertEqual(private[PRIVATE_SESSION_BUS_MARKER], "1")

    def test_every_adapter_uses_an_isolated_explicit_configuration(self):
        fixture = ["python3", "fixture.py"]
        inherited = {
            "DISPLAY": ":99",
            "WAYLAND_DISPLAY": "wayland-0",
            "EGL_PLATFORM": "wayland",
            "XDG_SESSION_TYPE": "wayland",
            "WINIT_UNIX_BACKEND": "wayland",
            "GDK_BACKEND": "wayland",
            "QT_QPA_PLATFORM": "wayland",
            PRIVATE_SESSION_BUS_MARKER: "1",
            "DBUS_SESSION_BUS_ADDRESS": "unix:path=/tmp/private-bus",
        }
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for name in SUPPORTED_TERMINALS:
                with self.subTest(name=name):
                    run_dir = root / name
                    run_dir.mkdir()
                    spec = build_launch_spec(
                        name,
                        f"/fixture/{name}",
                        run_dir,
                        "unique-title",
                        fixture,
                        SETTINGS,
                        inherited,
                    )
                    self.assertTrue(spec.controls["fresh_process"])
                    self.assertTrue(spec.controls["isolated_configuration"])
                    self.assertEqual(spec.environment["XDG_CONFIG_HOME"], str(run_dir / "xdg-config"))
                    self.assertEqual(spec.environment["DISPLAY"], ":99")
                    self.assertEqual(spec.environment["XDG_SESSION_TYPE"], "x11")
                    self.assertEqual(spec.environment["EGL_PLATFORM"], "x11")
                    self.assertEqual(
                        spec.environment["WAYLAND_DISPLAY"], DISABLED_WAYLAND_DISPLAY
                    )
                    self.assertEqual(
                        spec.environment["DBUS_SESSION_BUS_ADDRESS"],
                        "unix:path=/tmp/private-bus",
                    )
                    self.assertEqual(spec.controls["display_backend_requested"], "x11")
                    self.assertEqual(spec.controls["desktop_session_bus"], "private")
                    self.assertTrue(spec.controls["display_backend_control"])
                    self.assertEqual(spec.controls["font_size_requested"], SETTINGS.font_size)
                    self.assertTrue(spec.controls["font_size_unit"])
                    self.assertNotIn("font_size_points_requested", spec.controls)
                    self.assertTrue(all(part not in ("~", "$HOME") for part in spec.command))
                    self.assertTrue(spec.command[-2:] == fixture or name == "termeric")
                    if name == "termeric":
                        config = (run_dir / "xdg-config" / "termeric" / "config.toml").read_text()
                        for agent in ("claude-code", "codex", "gemini", "opencode"):
                            self.assertIn(f"[agents.{agent}]\nenabled = false", config)
                        self.assertNotIn("WINIT_UNIX_BACKEND", spec.environment)
                    elif name == "alacritty":
                        self.assertNotIn("WINIT_UNIX_BACKEND", spec.environment)
                    elif name == "kitty":
                        self.assertIn("linux_display_server x11", (run_dir / "kitty.conf").read_text())
                    elif name == "wezterm":
                        self.assertIn("enable_wayland = false", (run_dir / "wezterm.lua").read_text())
                    elif name == "ghostty":
                        self.assertEqual(spec.environment["GDK_BACKEND"], "x11")
                    elif name == "konsole":
                        self.assertEqual(spec.environment["QT_QPA_PLATFORM"], "xcb")

    def test_fresh_process_controls_are_present_for_reusing_terminals(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            cases = {
                "kitty": ("--session", "none"),
                "wezterm": ("--always-new-process", "--no-auto-connect"),
                "ghostty": ("gtk-single-instance = false",),
                "konsole": ("--separate", "--builtin-profile"),
            }
            for name, expected in cases.items():
                with self.subTest(name=name):
                    run_dir = root / name
                    run_dir.mkdir()
                    spec = build_launch_spec(
                        name,
                        f"/fixture/{name}",
                        run_dir,
                        "title",
                        ["fixture"],
                        SETTINGS,
                        {"DISPLAY": ":99"},
                    )
                    combined = "\n".join(
                        [" ".join(spec.command)]
                        + [path.read_text() for path in run_dir.rglob("*") if path.is_file()]
                    )
                    for token in expected:
                        self.assertIn(token, combined)

    def test_comparison_contract_is_metric_specific_and_fail_closed(self):
        report = comparison_contract(
            [terminal("termeric", sample()), terminal("kitty", sample(width=1002))],
            SETTINGS,
        )
        self.assertTrue(report["metrics"]["startup_to_visible_ms"]["comparable"])
        self.assertTrue(report["metrics"]["pty_response_ms"]["comparable"])
        self.assertFalse(report["metrics"]["rss_kib"]["comparable"])
        self.assertFalse(report["metrics"]["idle_cpu_percent_one_core"]["comparable"])
        self.assertFalse(report["comparable"])

    def test_window_geometry_parses_positive_dimensions_and_optional_signed_coordinates(self):
        with patch("scripts.compare_terminals.subprocess.run") as run:
            run.return_value.stdout = "WINDOW=42\nX=-10\nY=20\nWIDTH=1000\nHEIGHT=700\nSCREEN=0\n"
            self.assertEqual(window_geometry("42", {}), {"x": -10, "y": 20, "width": 1000, "height": 700})
            self.assertEqual(run.call_args.kwargs["timeout"], 5)
            self.assertTrue(run.call_args.kwargs["check"])
            run.return_value.stdout = "X=--10\nY=unknown\nWIDTH=1000\nHEIGHT=700\n"
            self.assertEqual(window_geometry("42", {}), {"width": 1000, "height": 700})

    def test_window_geometry_rejects_missing_duplicate_and_invalid_dimensions(self):
        malformed = [
            "", "WIDTH=1000\n", "HEIGHT=700\n",
            "WIDTH=1000\nHEIGHT=700\nWIDTH=1000\n",
            "WIDTH=1000\nHEIGHT=700\nHEIGHT=701\n",
        ]
        for dimension in ("WIDTH", "HEIGHT"):
            other = "HEIGHT=700" if dimension == "WIDTH" else "WIDTH=1000"
            for value in ("", "0", "-1", "--2", "1.0", "nan", "+1000", "١٠٠٠", "9" * 5000):
                malformed.append(f"{dimension}={value}\n{other}\n")
        for output in malformed:
            with self.subTest(output=output[:80]), patch("scripts.compare_terminals.subprocess.run") as run:
                run.return_value.stdout = output
                geometry = window_geometry("42", {})
                self.assertTrue(geometry["errors"])
                first, second = sample(), sample()
                first["window_geometry"] = second["window_geometry"] = geometry
                report = comparison_contract([terminal("termeric", first), terminal("kitty", second)], SETTINGS)
                self.assertEqual(report["ranking_permitted"], {
                    "startup_to_visible_ms": True, "pty_response_ms": True,
                    "rss_kib": False, "idle_cpu_percent_one_core": False,
                })

    def test_failed_window_query_is_an_explicit_unavailable_observation(self):
        for error in (OSError("unreadable"), subprocess.CalledProcessError(1, ["xdotool"]), subprocess.TimeoutExpired(["xdotool"], 5)):
            with self.subTest(error=type(error).__name__), patch("scripts.compare_terminals.subprocess.run", side_effect=error):
                geometry = window_geometry("42", {})
                self.assertNotIn("width", geometry)
                self.assertNotIn("height", geometry)
                self.assertIn(type(error).__name__, geometry["errors"][0])

    def test_matching_missing_or_malformed_geometry_never_permits_resource_ranking(self):
        geometries = [None, {}, [], True, "1000x700", {"width": 1000}, {"height": 700}]
        for dimension in ("width", "height"):
            for invalid in (None, 0, -1, True, 1000.0, "1000", float("nan"), float("inf"), [], {}):
                geometries.append({**sample()["window_geometry"], dimension: invalid})
        for error_field in (["query failed"], None, False, "", {}):
            geometries.append({**sample()["window_geometry"], "errors": error_field})
        for geometry in geometries:
            with self.subTest(geometry=geometry):
                first, second = sample(), sample()
                first["window_geometry"] = second["window_geometry"] = geometry
                report = comparison_contract([terminal("termeric", first), terminal("kitty", second)], SETTINGS)
                self.assertEqual(report["ranking_permitted"], {
                    "startup_to_visible_ms": True, "pty_response_ms": True,
                    "rss_kib": False, "idle_cpu_percent_one_core": False,
                })
                self.assertEqual(report["observed_window_pixels"], [])
        first, second = sample(), sample()
        del first["window_geometry"], second["window_geometry"]
        self.assertFalse(comparison_contract([terminal("termeric", first), terminal("kitty", second)], SETTINGS)["comparable"])

    def test_one_bad_geometry_sample_cannot_be_hidden_by_matching_complete_samples(self):
        first = terminal("termeric", sample())
        bad = sample()
        bad["window_geometry"] = {"width": 1000, "height": 700, "errors": ["duplicate width"]}
        first["samples"].append(bad)
        report = comparison_contract([first, terminal("kitty", sample())], SETTINGS)
        self.assertEqual(report["observed_window_pixels"], [[1000, 700]])
        for metric in ("rss_kib", "idle_cpu_percent_one_core"):
            self.assertFalse(report["ranking_permitted"][metric])
            self.assertTrue(any("geometry observation" in reason for reason in report["metrics"][metric]["reasons"]))
        self.assertTrue(report["ranking_permitted"]["startup_to_visible_ms"])
        self.assertTrue(report["ranking_permitted"]["pty_response_ms"])

    def test_grid_and_retention_drift_block_pty_ranking(self):
        report = comparison_contract(
            [
                terminal("termeric", sample()),
                terminal("kitty", sample(columns=99, output_lines=10_001)),
            ],
            SETTINGS,
        )
        reasons = report["metrics"]["pty_response_ms"]["reasons"]
        self.assertTrue(any("observed grid" in reason for reason in reasons))
        self.assertTrue(any("scrollback line capacity" in reason for reason in reasons))
        self.assertFalse(report["ranking_permitted"]["pty_response_ms"])

    def test_initial_grid_drift_only_blocks_startup_when_final_grid_matches(self):
        report = comparison_contract(
            [
                terminal("termeric", sample(ready_columns=80, ready_rows=24)),
                terminal("kitty", sample()),
            ],
            SETTINGS,
        )
        self.assertFalse(report["metrics"]["startup_to_visible_ms"]["comparable"])
        self.assertTrue(report["metrics"]["pty_response_ms"]["comparable"])

    def test_unavailable_selected_terminal_blocks_every_comparison(self):
        report = comparison_contract(
            [terminal("termeric", sample()), {"terminal": "kitty", "status": "unavailable"}],
            SETTINGS,
        )
        self.assertTrue(
            all(not contract["comparable"] for contract in report["metrics"].values())
        )

    def test_fixture_emits_the_exact_requested_byte_count(self):
        with tempfile.TemporaryFile() as stream:
            written, lines = emit_workload(stream.fileno(), 12_345)
            stream.seek(0, os.SEEK_END)
            self.assertEqual(written, 12_345)
            self.assertEqual(stream.tell(), 12_345)
            self.assertGreater(lines, 1)

    def test_summary_and_proc_accounting_are_deterministic(self):
        self.assertEqual(summarize([5.0, 1.0, 3.0])["median"], 3.0)
        parsed = stat_fields(os.getpid())
        self.assertIsNotNone(parsed)
        snapshot = process_tree_sample({os.getpid()})
        self.assertEqual(snapshot["status"], "complete")
        self.assertEqual(snapshot["errors"], [])
        self.assertEqual(snapshot["pids"], [os.getpid()])
        self.assertGreater(snapshot["rss_kib"], 0)
        self.assertGreaterEqual(snapshot["threads"], 1)

    def test_resource_status_requires_unique_exact_fields_and_units(self):
        self.assertEqual(parse_resource_status("VmRSS:\t0 kB\nThreads:\t1\n"), (0, 1))
        for text in [
            "Threads: 1\n", "VmRSS: 100 kB\n", "VmRSS: 100 MB\nThreads: 1\n",
            "VmRSS: -1 kB\nThreads: 1\n", "VmRSS: 1.5 kB\nThreads: 1\n",
            "VmRSS: 10 kB\nVmRSS: 20 kB\nThreads: 1\n",
            "VmRSS: 10 kB\nThreads: 1\nThreads: 2\n",
            "VmRSS: 10 kB\nThreads: 0\n", "VmRSS: 10 kB\nThreads: true\n",
        ]:
            with self.subTest(text=text), self.assertRaises(ValueError):
                parse_resource_status(text)

    def test_partial_process_read_never_publishes_partial_tree_totals(self):
        def descriptors(path):
            if path.parent.name == "2":
                raise PermissionError("injected fd access denial")
            return iter([Path("0"), Path("1")])

        with patch("scripts.compare_terminals.stat_fields", return_value=(1, 1, 10, 100)), \
                patch.object(Path, "read_text", return_value="VmRSS: 100 kB\nThreads: 2\n"), \
                patch.object(Path, "iterdir", descriptors):
            observed = process_tree_sample({1, 2})
        self.assertEqual(observed["status"], "unavailable")
        self.assertEqual(observed["requested_pids"], [1, 2])
        self.assertEqual(observed["pids"], [1])
        self.assertEqual(observed["identities"], [
            {"pid": 1, "start_time_ticks": 100}, {"pid": 2, "start_time_ticks": 100},
        ])
        self.assertEqual(observed["processes"][0]["rss_kib"], 100)
        for name in ("rss_kib", "threads", "fds", "cpu_ticks"):
            self.assertIsNone(observed[name])
        self.assertIn("pid 2", observed["errors"][0])

    def test_process_identity_counter_and_status_failures_are_unavailable(self):
        for stat_values, status in [
            ([None], "VmRSS: 100 kB\nThreads: 1\n"),
            ([(1, 1, -1, 100)], "VmRSS: 100 kB\nThreads: 1\n"),
            ([(1, 1, 10, -1)], "VmRSS: 100 kB\nThreads: 1\n"),
            ([(1, 1, 10, 100), None], "VmRSS: 100 kB\nThreads: 1\n"),
            ([(1, 1, 10, 100), (1, 1, 10, 200)], "VmRSS: 100 kB\nThreads: 1\n"),
            ([(1, 1, 10, 100), (1, 1, 9, 100)], "VmRSS: 100 kB\nThreads: 1\n"),
            ([(1, 1, 10, 100)], "Threads: 1\n"),
        ]:
            with self.subTest(stat_values=stat_values, status=status), \
                    patch("scripts.compare_terminals.stat_fields", side_effect=stat_values), \
                    patch.object(Path, "read_text", return_value=status), \
                    patch.object(Path, "iterdir", return_value=iter([])):
                observed = process_tree_sample({1})
            self.assertEqual(observed["status"], "unavailable")
            self.assertEqual(observed["processes"], [])
            self.assertIsNone(observed["rss_kib"])
        self.assertEqual(process_tree_sample(set())["status"], "unavailable")

    def test_complete_resource_totals_are_recounted_from_all_observed_processes(self):
        with patch("scripts.compare_terminals.stat_fields", side_effect=lambda pid: (1, 1, pid * 10, pid * 100)), \
                patch.object(Path, "read_text", return_value="VmRSS: 100 kB\nThreads: 2\n"), \
                patch.object(Path, "iterdir", side_effect=lambda: iter([Path("0"), Path("1")])):
            observed = process_tree_sample({1, 2})
        self.assertEqual(observed["status"], "complete")
        self.assertEqual(observed["pids"], [1, 2])
        for name, total in [("rss_kib", 200), ("threads", 4), ("fds", 4), ("cpu_ticks", 30)]:
            self.assertEqual(observed[name], total)
            self.assertEqual(observed[name], sum(process[name] for process in observed["processes"]))

    def test_discovery_denial_is_explicit_not_an_empty_process_tree(self):
        with patch.object(Path, "iterdir", side_effect=PermissionError("denied")):
            with self.assertRaises(PermissionError):
                direct_children(1)
        with patch("scripts.compare_terminals.benchmark_pids", side_effect=PermissionError("denied")):
            observed = terminal_resources(1, 2, 3)
        self.assertEqual(observed["status"], "unavailable")
        self.assertIsNone(observed["rss_kib"])
        self.assertIn("discovery unavailable", observed["errors"][0])

    def test_idle_cpu_requires_the_same_complete_identities_and_monotonic_counters(self):
        def observation(*records):
            return {"status": "complete", "processes": [
                {"pid": pid, "start_time_ticks": start, "cpu_ticks": ticks}
                for pid, start, ticks in records
            ]}
        before = observation((1, 100, 10), (2, 200, 20))
        self.assertEqual(idle_cpu_measurement(before, before, 1.0, 100), (0.0, []))
        self.assertEqual(idle_cpu_measurement(before, observation((1, 100, 11), (2, 200, 23)), 2.0, 100), (2.0, []))
        for after in [
            observation(), observation((1, 100, 10)),
            observation((1, 100, 10), (2, 200, 20), (3, 300, 1)),
            observation((1, 101, 10), (2, 200, 20)),
            observation((1, 100, 9), (2, 200, 99)),
            {"status": "unavailable", "processes": []},
        ]:
            value, reasons = idle_cpu_measurement(before, after, 1.0, 100)
            self.assertIsNone(value)
            self.assertTrue(reasons)
        for elapsed, frequency in [(0, 100), (float("nan"), 100), (1, 0)]:
            self.assertIsNone(idle_cpu_measurement(before, before, elapsed, frequency)[0])

    def test_missing_resource_samples_block_only_the_affected_metric(self):
        for metric in ("rss_kib", "idle_cpu_percent_one_core"):
            other = "rss_kib" if metric != "rss_kib" else "idle_cpu_percent_one_core"
            for invalid in (None, -1, float("nan"), float("inf"), True):
                first = sample()
                first["resources"][metric] = invalid
                report = comparison_contract([terminal("termeric", first), terminal("kitty", sample())], SETTINGS)
                self.assertFalse(report["ranking_permitted"][metric])
                for unaffected in (other, "startup_to_visible_ms", "pty_response_ms"):
                    self.assertTrue(report["ranking_permitted"][unaffected])
            first = sample()
            del first["resources"]["measurement_status"]
            report = comparison_contract([terminal("termeric", first), terminal("kitty", sample())], SETTINGS)
            self.assertFalse(report["ranking_permitted"][metric])
        report = comparison_contract([terminal("termeric", sample()), terminal("kitty", sample())], SETTINGS)
        self.assertTrue(report["comparable"])

    def test_resource_summary_does_not_cherry_pick_complete_samples(self):
        first, second = sample(), sample()
        second["resources"]["rss_kib"] = None
        second["resources"]["measurement_status"]["rss_kib"] = "unavailable"
        summary = terminal_summary([first, second])
        self.assertEqual(summary["rss_kib"]["status"], "unavailable")
        self.assertEqual(summary["rss_kib"]["samples"], [100.0, None])
        self.assertTrue(all(value is None for value in summary["rss_kib"]["statistics"].values()))
        self.assertEqual(summary["idle_cpu_percent_one_core"]["statistics"]["median"], 0.0)
        self.assertEqual(summary["startup_to_visible_ms"]["statistics"]["median"], 2.0)

    def test_sample_keeps_valid_timings_and_cleanup_when_resource_read_is_unavailable(self):
        before = {
            "status": "unavailable", "processes": [], "errors": ["denied"],
            "identities": [{"pid": 122, "start_time_ticks": 9}],
        }
        after = {
            "status": "complete", "pids": [123], "errors": [],
            "rss_kib": 100, "threads": 1, "fds": 2, "cpu_ticks": 3,
            "processes": [{"pid": 123, "start_time_ticks": 10, "cpu_ticks": 3}],
            "identities": [{"pid": 123, "start_time_ticks": 10}],
        }
        ready = {**sample()["fixture_ready"], "pid": 124}
        with tempfile.TemporaryDirectory() as temporary, \
                patch("scripts.compare_terminals.font_environment", return_value={"status": "unavailable", "errors": ["missing font"]}) as fonts, \
                patch("scripts.compare_terminals.subprocess.Popen") as launch, \
                patch("scripts.compare_terminals.wait_for_file", side_effect=[ready, sample()["fixture"]]), \
                patch("scripts.compare_terminals.find_window", return_value=("42", 123, 10.5)), \
                patch("scripts.compare_terminals.window_geometry", return_value=sample()["window_geometry"]), \
                patch("scripts.compare_terminals.terminal_resources", side_effect=[before, after]), \
                patch("scripts.compare_terminals.stat_fields", return_value=(124, 124, 0, 20)), \
                patch("scripts.compare_terminals.time.monotonic", side_effect=[10.0, 11.0, 12.0]), \
                patch("scripts.compare_terminals.time.sleep"), \
                patch("scripts.compare_terminals.terminate_run") as cleanup:
            launch.return_value.pid = 123
            result, _ = run_sample(
                "kitty", "/fixture/kitty", Path(temporary) / "sample", SETTINGS,
                1024, 20.0, 1.0, {"DISPLAY": ":99"},
            )
        self.assertEqual(result["startup_to_visible_ms"], 500.0)
        self.assertEqual(result["font_environment"]["status"], "unavailable")
        self.assertIn("xdg-config", fonts.call_args.args[1]["XDG_CONFIG_HOME"])
        self.assertEqual(result["fixture"]["pty_response_ms"], 1.0)
        self.assertEqual(result["resources"]["rss_kib"], 100)
        self.assertIsNone(result["resources"]["idle_cpu_percent_one_core"])
        self.assertEqual(result["resources"]["measurement_status"]["idle_cpu_percent_one_core"], "unavailable")
        self.assertEqual(result["resources"]["observations"], {"before": before, "after": after})
        cleanup.assert_called_once()
        self.assertEqual(cleanup.call_args.args[3], {122: 9, 123: 10, 124: 20})

    def test_geometry_timeout_retains_timing_resource_observations_and_cleanup(self):
        observation = {
            "status": "complete", "pids": [123], "errors": [],
            "rss_kib": 100, "threads": 1, "fds": 2, "cpu_ticks": 3,
            "processes": [{"pid": 123, "start_time_ticks": 10, "cpu_ticks": 3}],
            "identities": [{"pid": 123, "start_time_ticks": 10}],
        }
        ready = {**sample()["fixture_ready"], "pid": 124}
        with tempfile.TemporaryDirectory() as temporary, \
                patch("scripts.compare_terminals.font_environment", return_value=font_observation()), \
                patch("scripts.compare_terminals.subprocess.Popen") as launch, \
                patch("scripts.compare_terminals.subprocess.run", side_effect=subprocess.TimeoutExpired(["xdotool"], 5)), \
                patch("scripts.compare_terminals.wait_for_file", side_effect=[ready, sample()["fixture"]]), \
                patch("scripts.compare_terminals.find_window", return_value=("42", 123, 10.5)), \
                patch("scripts.compare_terminals.terminal_resources", return_value=observation), \
                patch("scripts.compare_terminals.stat_fields", return_value=(124, 124, 0, 20)), \
                patch("scripts.compare_terminals.time.monotonic", side_effect=[10.0, 11.0, 12.0]), \
                patch("scripts.compare_terminals.time.sleep"), \
                patch("scripts.compare_terminals.terminate_run") as cleanup:
            launch.return_value.pid = 123
            result, _ = run_sample(
                "kitty", "/fixture/kitty", Path(temporary) / "sample", SETTINGS,
                1024, 20.0, 1.0, {"DISPLAY": ":99"},
            )
        self.assertEqual(result["startup_to_visible_ms"], 500.0)
        self.assertEqual(result["fixture"]["pty_response_ms"], 1.0)
        self.assertEqual(result["resources"]["rss_kib"], 100)
        self.assertEqual(result["resources"]["idle_cpu_percent_one_core"], 0.0)
        self.assertEqual(result["resources"]["observations"], {"before": observation, "after": observation})
        self.assertIn("TimeoutExpired", result["window_geometry"]["errors"][0])
        report = comparison_contract([terminal("termeric", result), terminal("kitty", sample())], SETTINGS)
        self.assertEqual(report["ranking_permitted"], {
            "startup_to_visible_ms": True, "pty_response_ms": True,
            "rss_kib": False, "idle_cpu_percent_one_core": False,
        })
        cleanup.assert_called_once()
        self.assertEqual(cleanup.call_args.args[3], {123: 10, 124: 20})


if __name__ == "__main__":
    unittest.main()
