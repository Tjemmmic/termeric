import os
from pathlib import Path
import tempfile
import unittest

from scripts.compare_terminals import (
    BenchmarkSettings,
    DISABLED_WAYLAND_DISPLAY,
    PRIVATE_SESSION_BUS_MARKER,
    SUPPORTED_TERMINALS,
    build_launch_spec,
    comparison_contract,
    process_tree_sample,
    stat_fields,
    summarize,
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
        "fixture": {
            "columns": columns,
            "rows": rows,
            "output_lines": output_lines,
        },
        "fixture_ready": {"columns": ready_columns, "rows": ready_rows},
        "window_geometry": {"width": width, "height": height},
    }


def terminal(name: str, run_sample: dict) -> dict:
    return {
        "terminal": name,
        "status": "complete",
        "controls": {},
        "samples": [run_sample],
    }


class CrossTerminalBenchmarkTests(unittest.TestCase):
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
                        self.assertIn("[plugins.claude_code]\nenabled = false", config)
                        self.assertIn("[plugins.codex]\nenabled = false", config)
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
        self.assertEqual(snapshot["pids"], [os.getpid()])
        self.assertGreater(snapshot["rss_kib"], 0)
        self.assertGreaterEqual(snapshot["threads"], 1)


if __name__ == "__main__":
    unittest.main()
