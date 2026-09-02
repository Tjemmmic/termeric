"""Pure regression tests for native GUI acceptance record analysis."""

from __future__ import annotations

import base64
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock


sys.path.insert(0, str(Path(__file__).resolve().parent))
import gui_acceptance  # noqa: E402


FIXTURE_PATH = (
    Path(__file__).resolve().parents[1] / "termeric" / "tests" / "gui" / "terminal_fixture.py"
)
FIXTURE_SPEC = importlib.util.spec_from_file_location("termeric_terminal_fixture", FIXTURE_PATH)
assert FIXTURE_SPEC is not None and FIXTURE_SPEC.loader is not None
terminal_fixture = importlib.util.module_from_spec(FIXTURE_SPEC)
FIXTURE_SPEC.loader.exec_module(terminal_fixture)


class GuiAcceptanceRecordTests(unittest.TestCase):
    def test_isolated_config_disables_provider_plugins(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            config = Path(directory) / "config.toml"
            gui_acceptance.write_config(config, "/bin/cat", [])
            document = config.read_text(encoding="utf-8")
            self.assertIn("[plugins.claude_code]\nenabled = false", document)
            self.assertIn("[plugins.codex]\nenabled = false", document)

    def test_app_process_replaces_user_home_and_tmux_runtime(self) -> None:
        with tempfile.TemporaryDirectory() as directory, mock.patch.object(
            gui_acceptance.subprocess, "Popen", return_value=mock.Mock()
        ):
            run_dir = Path(directory) / "run"
            app = gui_acceptance.AppProcess(
                Path("/fixture/termeric"),
                run_dir,
                "x11",
                "/bin/sh",
                [],
                {"HOME": "/real/home", "TMUX_TMPDIR": "/real/tmux"},
            )
            try:
                self.assertEqual(app.env["HOME"], str(run_dir / "home"))
                self.assertEqual(app.env["TMUX_TMPDIR"], str(run_dir / "tmux-runtime"))
                self.assertEqual((run_dir / "tmux-runtime").stat().st_mode & 0o777, 0o700)
            finally:
                app.finish()

    def test_soak_graphics_frame_requests_an_observable_success_reply(self) -> None:
        with mock.patch.object(terminal_fixture, "emit") as emit:
            terminal_fixture.render_soak_frame(0)
        payload = emit.call_args.args[0]
        self.assertIn(b"i=91,q=0", payload)

    def test_vim_scenario_requires_zero_exit_and_normal_native_close(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            run_dir = Path(directory)
            marker = run_dir / "vim-exit.txt"
            document = run_dir / "vim-fixture.txt"
            screen_capture = run_dir / "vim-screen.bin"
            app = mock.Mock()

            def type_text(value: str) -> None:
                if value == "VIM_INPUT_17":
                    document.write_text(
                        document.read_text(encoding="utf-8") + value + "\n",
                        encoding="utf-8",
                    )
                elif value == ":wq":
                    marker.write_text("0", encoding="utf-8")
                    screen_capture.write_bytes(b"\x1b[?1049hvim\x1b[?1049l")

            app.type.side_effect = type_text
            binary = run_dir / "termeric"
            with mock.patch.object(
                gui_acceptance.shutil,
                "which",
                side_effect=lambda name: f"/fixture/bin/{name}",
            ), mock.patch.object(
                gui_acceptance, "AppProcess", return_value=app
            ) as app_process, mock.patch.object(
                gui_acceptance, "assert_no_runtime_errors"
            ) as assert_clean:
                gui_acceptance.run_vim_scenario(binary, run_dir, {}, False)

            launch = app_process.call_args.args
            self.assertEqual(launch[3], "/bin/sh")
            self.assertIn("/fixture/bin/vim -Nu NONE -n -i NONE", launch[4][1])
            self.assertIn("/fixture/bin/script --quiet --return", launch[4][1])
            self.assertIn("vim-exit.txt", launch[4][1])
            self.assertEqual(marker.read_text(encoding="utf-8"), "0")
            self.assertIn("VIM_INPUT_17", document.read_text(encoding="utf-8"))
            app.close_normally.assert_called_once_with("x11")
            assert_clean.assert_called_once_with(app)
            app.stop_after_failure.assert_called_once_with()
            app.finish.assert_called_once_with()

    def test_vim_scenario_rejects_nonzero_exit_before_native_close(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            run_dir = Path(directory)
            marker = run_dir / "vim-exit.txt"
            app = mock.Mock()

            def type_text(value: str) -> None:
                if value == ":wq":
                    marker.write_text("7", encoding="utf-8")

            app.type.side_effect = type_text
            with mock.patch.object(
                gui_acceptance.shutil, "which", return_value="/fixture/bin/vim"
            ), mock.patch.object(
                gui_acceptance, "AppProcess", return_value=app
            ), mock.patch.object(gui_acceptance, "assert_no_runtime_errors") as assert_clean:
                with self.assertRaisesRegex(
                    gui_acceptance.AcceptanceError, "Vim exited with 7"
                ):
                    gui_acceptance.run_vim_scenario(
                        run_dir / "termeric", run_dir, {}, False
                    )

            app.close_normally.assert_not_called()
            assert_clean.assert_not_called()
            app.stop_after_failure.assert_called_once_with()
            app.finish.assert_called_once_with()

    def test_external_graphics_clients_execute_and_emit_both_protocols(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            transcript = root / "transcript.jsonl"
            fake_bin = root / "bin"
            fake_bin.mkdir()
            clients = {
                "img2sixel": "#!/bin/sh\nprintf '\\033Pq~\\033\\\\'\n",
                "timg": "#!/bin/sh\nprintf '\\033]1337;File=inline=1:AA==\\007'\n",
            }
            for name, script in clients.items():
                executable = fake_bin / name
                executable.write_text(script, encoding="utf-8")
                executable.chmod(0o755)

            with mock.patch.dict(
                os.environ,
                {"PATH": f"{fake_bin}{os.pathsep}{os.environ.get('PATH', '')}"},
            ), mock.patch.object(terminal_fixture, "emit") as emit:
                terminal_fixture.render_external_graphics_clients(str(transcript))

            records = [json.loads(line) for line in transcript.read_text().splitlines()]
            self.assertEqual(
                {record["tool"] for record in records},
                {"img2sixel", "timg-iterm2"},
            )
            self.assertEqual(emit.call_count, 2)
            emitted = [call.args[0] for call in emit.call_args_list]
            self.assertTrue(any(payload.startswith(b"\x1bPq") for payload in emitted))
            self.assertTrue(any(payload.startswith(b"\x1b]1337;") for payload in emitted))

    def test_alternate_screen_capture_requires_entry_before_exit(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            capture = Path(directory) / "screen.bin"
            capture.write_bytes(b"prefix\x1b[?1049hcontent\x1b[?1049lsuffix")
            gui_acceptance.assert_alternate_screen_round_trip(capture, "fixture")

            capture.write_bytes(b"\x1b[?1049lexit only")
            with self.assertRaisesRegex(
                gui_acceptance.AcceptanceError, "alternate-screen round trip"
            ):
                gui_acceptance.assert_alternate_screen_round_trip(capture, "fixture")

            capture.write_bytes(b"\x1b[?47hmismatched\x1b[?1049l")
            with self.assertRaisesRegex(
                gui_acceptance.AcceptanceError, "alternate-screen round trip"
            ):
                gui_acceptance.assert_alternate_screen_round_trip(capture, "fixture")

    def test_recorded_full_screen_command_is_bounded_and_returns_status(self) -> None:
        with mock.patch.object(
            gui_acceptance.shutil, "which", return_value="/fixture/bin/script"
        ):
            command = gui_acceptance.recorded_full_screen_command(
                "/fixture/bin/less document.txt", Path("screen output.bin")
            )
        self.assertIn("/fixture/bin/script", command)
        self.assertIn("--return", command)
        self.assertIn("--output-limit 4MiB", command)
        self.assertIn("--echo never", command)
        self.assertIn("'screen output.bin'", command)

    def test_terminal_geometry_uses_character_and_pixel_fields_in_order(self) -> None:
        record = {
            "rows": 24,
            "cols": 80,
            "pixel_width": 640,
            "pixel_height": 384,
        }
        self.assertEqual(gui_acceptance.terminal_geometry(record), (24, 80, 640, 384))

    def test_latest_geometry_is_scoped_to_the_exact_fixture_pid(self) -> None:
        records = [
            {
                "event": "started",
                "pid": 10,
                "rows": 24,
                "cols": 80,
                "pixel_width": 640,
                "pixel_height": 384,
            },
            {
                "event": "winsize",
                "pid": 11,
                "rows": 30,
                "cols": 100,
                "pixel_width": 800,
                "pixel_height": 480,
            },
            {
                "event": "winsize",
                "pid": 10,
                "rows": 40,
                "cols": 120,
                "pixel_width": 960,
                "pixel_height": 640,
            },
        ]
        self.assertEqual(
            gui_acceptance.latest_terminal_geometry(records, 10),
            (40, 120, 960, 640),
        )
        self.assertEqual(
            gui_acceptance.latest_terminal_geometry(records, 11),
            (30, 100, 800, 480),
        )

    def test_input_token_maps_a_native_window_to_only_its_fixture_pid(self) -> None:
        records = [
            {
                "event": "input",
                "pid": 41,
                "base64": base64.b64encode(b"FIRST_WINDOW").decode(),
            },
            {
                "event": "input",
                "pid": 73,
                "base64": base64.b64encode(b"FULLSCREEN_TARGET_17").decode(),
            },
        ]
        self.assertEqual(
            gui_acceptance.pid_receiving_token(records, b"FULLSCREEN_TARGET_17"),
            73,
        )
        self.assertIsNone(gui_acceptance.pid_receiving_token(records, b"missing"))

    def test_token_recipient_set_exposes_accidental_cross_pane_delivery(self) -> None:
        duplicate = base64.b64encode(b"DUPLICATE_TOKEN").decode()
        records = [
            {"event": "input", "pid": 5, "base64": duplicate},
            {"event": "input", "pid": 9, "base64": duplicate},
        ]
        self.assertEqual(
            gui_acceptance.pids_receiving_token(records, b"DUPLICATE_TOKEN"),
            {5, 9},
        )

    def test_wayland_launch_and_fullscreen_dependencies_are_separate(self) -> None:
        launch = gui_acceptance.required_tools(
            "wayland", ["launch"], start_weston=True, start_sway=False
        )
        fullscreen = gui_acceptance.required_tools(
            "wayland", ["fullscreen"], start_weston=False, start_sway=True
        )
        self.assertIn("weston", launch)
        self.assertNotIn("wtype", launch)
        self.assertNotIn("sway", launch)
        self.assertNotIn("img2sixel", launch)
        self.assertNotIn("timg", launch)
        self.assertIn("sway", fullscreen)
        self.assertIn("wtype", fullscreen)
        self.assertNotIn("weston", fullscreen)
        self.assertNotIn("img2sixel", fullscreen)
        self.assertNotIn("timg", fullscreen)

    def test_external_graphics_and_clipboard_tools_are_workspace_scoped(self) -> None:
        workspace = gui_acceptance.required_tools(
            "x11",
            ["workspace"],
            start_weston=False,
            start_sway=False,
        )
        vim = gui_acceptance.required_tools(
            "x11",
            ["vim"],
            start_weston=False,
            start_sway=False,
        )
        self.assertTrue({"img2sixel", "timg", "xclip"}.issubset(workspace))
        self.assertTrue({"img2sixel", "timg", "xclip"}.isdisjoint(vim))

    def test_capture_tool_is_required_only_by_scenarios_that_take_snapshots(self) -> None:
        with mock.patch.dict(
            os.environ, {"TERMERIC_GUI_CAPTURE": "ffmpeg"}, clear=False
        ):
            fullscreen = gui_acceptance.required_tools(
                "x11",
                ["fullscreen"],
                start_weston=False,
                start_sway=False,
            )
            vim = gui_acceptance.required_tools(
                "x11",
                ["vim"],
                start_weston=False,
                start_sway=False,
            )
        self.assertNotIn("ffmpeg", fullscreen)
        self.assertIn("ffmpeg", vim)
        self.assertIn("script", vim)

    def test_fixture_runs_external_graphics_only_when_explicitly_requested(self) -> None:
        _, isolated = gui_acceptance.fixture_command(Path("transcript.jsonl"))
        _, graphics = gui_acceptance.fixture_command(
            Path("transcript.jsonl"), external_graphics=True
        )
        self.assertIn("--skip-external-graphics", isolated)
        self.assertNotIn("--skip-external-graphics", graphics)

    def test_x11_fullscreen_declares_ewmh_window_manager_dependency(self) -> None:
        tools = gui_acceptance.required_tools(
            "x11",
            ["fullscreen"],
            start_weston=False,
            start_sway=False,
            start_window_manager=True,
        )
        self.assertIn("openbox", tools)

    def test_wayland_interaction_requires_virtual_keyboard_clipboard_and_sway(self) -> None:
        tools = gui_acceptance.required_tools(
            "wayland",
            ["interaction"],
            start_weston=False,
            start_sway=True,
        )
        self.assertIn("wtype", tools)
        self.assertIn("wl-copy", tools)
        self.assertIn("wl-paste", tools)
        self.assertIn("sway", tools)
        self.assertIn("swaymsg", tools)
        self.assertNotIn("weston", tools)

    def test_wtype_chord_holds_and_releases_modifiers_in_stack_order(self) -> None:
        self.assertEqual(
            gui_acceptance.wtype_chord_arguments(("ctrl", "shift"), "v"),
            [
                "-M",
                "ctrl",
                "-M",
                "shift",
                "-P",
                "v",
                "-p",
                "v",
                "-m",
                "shift",
                "-m",
                "ctrl",
            ],
        )

    def test_sway_output_resize_targets_the_private_headless_output(self) -> None:
        self.assertEqual(
            gui_acceptance.sway_output_resize_command(1000, 740),
            ["swaymsg", "output", "HEADLESS-1", "resolution", "1000x740"],
        )
        with self.assertRaises(gui_acceptance.AcceptanceError):
            gui_acceptance.sway_output_resize_command(0, 740)

    def test_result_document_binds_recovery_flags_and_binary_digest(self) -> None:
        import tempfile

        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / "termeric"
            binary.write_bytes(b"tested native binary")
            document = gui_acceptance.result_document(
                backend="wayland",
                binary=binary,
                scenarios=[{"scenario": "interaction", "status": "passed"}],
                start_weston=False,
                start_sway=True,
                start_window_manager=False,
                force_surface_loss=True,
                force_surface_outdated=True,
                force_device_loss=True,
            )
        self.assertEqual(document["schema"], 3)
        self.assertEqual(document["compositor"], "sway")
        self.assertEqual(len(document["binary_sha256"]), 64)
        self.assertEqual(
            document["renderer_injections"],
            {
                "surface_loss": True,
                "surface_outdated": True,
                "device_loss": True,
            },
        )


if __name__ == "__main__":
    unittest.main()
