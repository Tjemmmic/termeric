"""Pure regression tests for native GUI acceptance record analysis."""

from __future__ import annotations

import base64
import importlib.util
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import tomllib
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
    def test_scrollback_fixture_is_bounded_owner_tagged_and_ends_at_marker(self):
        with mock.patch.object(terminal_fixture, "emit") as emit:
            terminal_fixture.render_scrollback_fixture(42)
        payload = emit.call_args.args[0]
        self.assertLess(len(payload), 4096)
        self.assertEqual(payload.split(b"\r\n")[1:-1],
                         [f"SCROLL_42_{row:03d}".encode() for row in range(100)])
        self.assertTrue(payload.endswith(b"SCROLL_END_42"))
        self.assertNotIn(b"\x1b", payload)

    def test_isolated_config_has_distinct_fonts_and_disables_provider_plugins(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            config = Path(directory) / "config.toml"
            gui_acceptance.write_config(config, "/bin/cat", [])
            document = config.read_text(encoding="utf-8")
            parsed = tomllib.loads(document)
            primary = parsed["font"]["family"].casefold()
            fallbacks = [family.casefold() for family in parsed["font"]["fallback"]]
            self.assertNotIn(primary, fallbacks)
            self.assertEqual(len(fallbacks), len(set(fallbacks)))
            self.assertEqual(fallbacks, ["noto color emoji"])
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

    def test_app_process_sequences_requested_renderer_faults_with_trigger_files(self) -> None:
        requested = {
            legacy: "1"
            for legacy, _ in gui_acceptance.RENDERER_INJECTION_ENV.values()
        }
        with tempfile.TemporaryDirectory() as directory, mock.patch.object(
            gui_acceptance.subprocess, "Popen", return_value=mock.Mock()
        ):
            app = gui_acceptance.AppProcess(
                Path("/fixture/termeric"),
                Path(directory) / "run",
                "x11",
                "/bin/sh",
                [],
                requested,
            )
            try:
                for name, (legacy, trigger_variable) in (
                    gui_acceptance.RENDERER_INJECTION_ENV.items()
                ):
                    self.assertNotIn(legacy, app.env)
                    self.assertEqual(
                        app.env[trigger_variable], str(app.renderer_triggers[name])
                    )
                    self.assertFalse(app.renderer_triggers[name].exists())

                app.arm_renderer_injection("surface_loss")
                self.assertTrue(app.renderer_triggers["surface_loss"].is_file())
                self.assertFalse(app.renderer_triggers["surface_outdated"].exists())
                self.assertFalse(app.renderer_triggers["device_loss"].exists())
            finally:
                app.finish()

    def test_private_compositor_runtime_is_short_and_cleanup_is_owned(self) -> None:
        runtime = gui_acceptance.private_runtime_directory("test")
        process = mock.Mock()
        process._termeric_runtime = runtime
        try:
            self.assertTrue(runtime.is_dir())
            self.assertEqual(runtime.stat().st_mode & 0o777, 0o700)
            self.assertEqual(runtime.parent, Path(tempfile.gettempdir()))
            gui_acceptance.cleanup_private_runtime(process)
            self.assertFalse(runtime.exists())
        finally:
            if runtime.exists():
                runtime.rmdir()

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

    def test_external_graphics_clients_execute_every_configured_protocol(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            transcript = root / "transcript.jsonl"
            kitten_args = root / "kitten-args.txt"
            kitten_stdin = root / "kitten-stdin.txt"
            timg_args = root / "timg-args.txt"
            fake_bin = root / "bin"
            fake_bin.mkdir()
            clients = {
                "kitten": """#!/bin/sh
if [ "$1" = "--version" ]; then
  printf 'kitten fixture 1.0\\n'
  exit 0
fi
printf '%s\\n' "$*" >> "$TERMERIC_KITTEN_ARGS"
case " $* " in
  *" --transfer-mode=file "*)
    wc -c > "$TERMERIC_KITTEN_STDIN"
    printf '\\033_Ga=T,q=2,f=100,t=t,s=64,v=64,S=1234,i=1703;L3RtcC9raXR0eS10dHktZ3JhcGhpY3MtcHJvdG9jb2wtZmFrZQ==\\033\\\\'
    ;;
  *" --transfer-mode=memory "*)
    printf '\\033_Ga=T,q=2,f=100,t=s,s=64,v=64,S=1234,i=1704;aWNhdC1mYWtl\\033\\\\'
    ;;
  *" --unicode-placeholder "*)
    printf '\\033_Ga=T,q=2,U=1,f=32,s=1,v=1,i=1702;AQIDBA==\\033\\\\'
    ;;
  *)
    printf '\\033_Ga=T,q=2,f=32,s=1,v=1,i=1701;AQIDBA==\\033\\\\'
    ;;
esac
""",
                "img2sixel": """#!/bin/sh
if [ "$1" = "--version" ]; then
  printf 'img2sixel fixture 1.0\\n'
else
  printf '\\033Pq~\\033\\\\'
fi
""",
                "timg": """#!/bin/sh
if [ "$1" = "--version" ]; then
  printf 'timg fixture 1.0\\n'
  exit 0
fi
printf '%s\\n' "$*" >> "$TERMERIC_TIMG_ARGS"
case " $* " in
  *" -pk "*)
    printf '\\033[?25l\\033_Ga=T,i=2701,q=2,f=100,m=0;iVBORw0KGgo=\\033\\\\\\033[?25h'
    ;;
  *" -ps "*)
    printf '\\033Pq~\\033\\\\'
    ;;
  *)
    printf '\\033]1337;File=inline=1:AA==\\007'
    ;;
esac
""",
            }
            for name, script in clients.items():
                executable = fake_bin / name
                executable.write_text(script, encoding="utf-8")
                executable.chmod(0o755)

            with mock.patch.dict(
                os.environ,
                {
                    "PATH": f"{fake_bin}{os.pathsep}{os.environ.get('PATH', '')}",
                    "TERMERIC_KITTEN_ARGS": str(kitten_args),
                    "TERMERIC_KITTEN_STDIN": str(kitten_stdin),
                    "TERMERIC_TIMG_ARGS": str(timg_args),
                },
            ), mock.patch.object(
                terminal_fixture, "require_media_target"
            ) as require_target, mock.patch.object(
                terminal_fixture, "wait_for_media_cleanup"
            ) as wait_for_cleanup, mock.patch.object(terminal_fixture, "emit") as emit:
                terminal_fixture.render_external_graphics_clients(
                    str(transcript), (40, 100, 800, 800)
                )

            records = [json.loads(line) for line in transcript.read_text().splitlines()]
            self.assertEqual(records[-1]["event"], "graphics_clients_complete")
            self.assertEqual((records[-1]["attempted"], records[-1]["failed"]), (8, 0))
            records = records[:-1]
            self.assertEqual(
                {record["tool"] for record in records},
                {
                    "kitten-stream",
                    "kitten-unicode-placeholder",
                    "kitten-file",
                    "kitten-memory",
                    "timg-kitty",
                    "img2sixel",
                    "timg-iterm2",
                    "timg-sixel",
                },
            )
            self.assertEqual(
                {record["version"] for record in records},
                {
                    "kitten fixture 1.0",
                    "img2sixel fixture 1.0",
                    "timg fixture 1.0",
                },
            )
            self.assertEqual(emit.call_count, 8)
            emitted = [call.args[0] for call in emit.call_args_list]
            self.assertTrue(any(b"i=1701" in payload for payload in emitted))
            self.assertTrue(any(b"U=1" in payload for payload in emitted))
            self.assertTrue(
                any(b"t=t" in payload and b"i=1703" in payload for payload in emitted)
            )
            self.assertTrue(
                any(b"t=s" in payload and b"i=1704" in payload for payload in emitted)
            )
            self.assertTrue(any(b"i=2701" in payload for payload in emitted))
            self.assertTrue(any(b"\x1bPq" in payload for payload in emitted))
            self.assertTrue(any(b"\x1b]1337;" in payload for payload in emitted))
            arguments = kitten_args.read_text(encoding="utf-8").splitlines()
            self.assertEqual(len(arguments), 4)
            self.assertEqual(
                sum("--transfer-mode=stream" in line for line in arguments), 2
            )
            self.assertEqual(sum("--transfer-mode=file" in line for line in arguments), 1)
            self.assertEqual(sum("--transfer-mode=memory" in line for line in arguments), 1)
            self.assertEqual(
                sum("--unicode-placeholder" in line for line in arguments), 1
            )
            self.assertEqual(sum("--stdin=yes" in line for line in arguments), 1)
            self.assertEqual(sum("--stdin=no" in line for line in arguments), 3)
            file_arguments = next(
                line for line in arguments if "--transfer-mode=file" in line
            )
            self.assertNotIn("graphics-client-", file_arguments)
            timg_arguments = timg_args.read_text(encoding="utf-8").splitlines()
            self.assertEqual(len(timg_arguments), 3)
            self.assertEqual(sum("-pk" in line for line in timg_arguments), 1)
            self.assertEqual(sum("-pi" in line for line in timg_arguments), 1)
            self.assertEqual(sum("-ps" in line for line in timg_arguments), 1)
            self.assertTrue(any("-ps --frames=1" in line for line in timg_arguments))
            self.assertTrue(any("--compress=0" in line for line in timg_arguments))
            self.assertTrue(all("-g2x1" in line for line in timg_arguments))
            records_by_tool = {record["tool"]: record for record in records}
            self.assertEqual(records_by_tool["kitten-stream"]["image_id"], 1701)
            self.assertEqual(
                records_by_tool["kitten-unicode-placeholder"]["image_id"], 1702
            )
            self.assertEqual(records_by_tool["kitten-file"]["image_id"], 1703)
            self.assertEqual(records_by_tool["kitten-memory"]["image_id"], 1704)
            self.assertTrue(records_by_tool["kitten-file"]["cleanup_observed"])
            self.assertTrue(records_by_tool["kitten-memory"]["cleanup_observed"])
            self.assertEqual(records_by_tool["timg-kitty"]["image_id"], 2701)
            self.assertEqual(
                require_target.call_args_list,
                [
                    mock.call(b"/tmp/kitty-tty-graphics-protocol-fake", b"t"),
                    mock.call(b"icat-fake", b"s"),
                ],
            )
            self.assertEqual(
                wait_for_cleanup.call_args_list,
                [
                    mock.call(b"/tmp/kitty-tty-graphics-protocol-fake", b"t"),
                    mock.call(b"icat-fake", b"s"),
                ],
            )
            fixture = next(root.glob("graphics-client-*.png"))
            fixture_bytes = fixture.read_bytes()
            self.assertEqual(fixture_bytes[:8], b"\x89PNG\r\n\x1a\n")
            self.assertEqual(struct.unpack(">II", fixture_bytes[16:24]), (64, 64))
            self.assertEqual(int(kitten_stdin.read_text()), len(fixture_bytes))

    def test_graphics_client_failures_record_stage_and_preserve_original_exception(self):
        payload = b"\x1b_Ga=T,i=1701;AAAA\x1b\\"
        cases = [
            ("empty", subprocess.CompletedProcess([], 0, b"", b""), "validate-output", 0, 0),
            ("invalid", subprocess.CompletedProcess([], 0, b"bad", b""), "validate-output", 0, 3),
            ("exit", subprocess.CalledProcessError(2, ["kitten"], output=payload,
                stderr=b"\x1b[31merror\n" + b"x" * 2048), "execute", 2, len(payload)),
            ("timeout", subprocess.TimeoutExpired(["kitten"], 15, output=payload,
                stderr=b"\xff\n"), "execute", None, len(payload)),
            ("missing", FileNotFoundError("not installed"), "execute", None, 0),
            ("version", subprocess.CalledProcessError(3, ["kitten", "--version"],
                stderr=b"no version"), "version", 3, 0),
        ]
        for name, result, stage, returncode, output_bytes in cases:
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                transcript = Path(directory) / "transcript.jsonl"
                version = mock.Mock(return_value="fixture 1.0")
                run = mock.Mock(return_value=result)
                if isinstance(result, Exception):
                    (version if name == "version" else run).side_effect = result
                with mock.patch.object(terminal_fixture, "command_version", version), \
                        mock.patch.object(terminal_fixture.subprocess, "run", run), \
                        mock.patch.object(terminal_fixture, "emit") as emit:
                    with self.assertRaises(Exception) as caught:
                        terminal_fixture.render_external_graphics_clients(str(transcript), (40, 100, 800, 800))
                    if isinstance(result, Exception):
                        self.assertIs(caught.exception, result)
                    emit.assert_not_called()
                    self.assertEqual(run.call_count, 0 if name == "version" else 8)
                records = [json.loads(line) for line in transcript.read_text().splitlines()]
                self.assertEqual(len(records), 9)
                self.assertEqual(records[-1]["event"], "graphics_clients_complete")
                self.assertEqual((records[-1]["attempted"], records[-1]["failed"]), (8, 8))
                record = records[0]
                self.assertEqual(record["event"], "graphics_client_error")
                self.assertIs(record["continuing"], True)
                self.assertEqual(record["tool"], "kitten-stream")
                self.assertEqual(record["protocol"], "kitty")
                self.assertEqual(record["stage"], stage)
                self.assertEqual(record["returncode"], returncode)
                self.assertEqual(record["output_bytes"], output_bytes)
                self.assertEqual(record["version"], None if name == "version" else "fixture 1.0")
                self.assertLessEqual(len(record["stderr"]), 1024)
                self.assertTrue(all(c.isprintable() for c in record["stderr"]))
                self.assertEqual(record["stderr_truncated"], name == "exit")
                self.assertEqual(record["stderr_bytes"], len(getattr(result, "stderr", None) or b""))

    def test_graphics_cleanup_failure_retains_prior_successes_but_not_failed_transfer(self):
        payloads = [b"\x1b_Ga=T,i=1701;AAAA\x1b\\", b"\x1b_Ga=T,i=1702;AAAA\x1b\\",
                    b"\x1b_Ga=T,i=1703,t=t;L3RtcC9raXR0eS10dHktZ3JhcGhpY3MtcHJvdG9jb2wtZmFrZQ==\x1b\\"]
        failure = RuntimeError("media target was not removed")
        with tempfile.TemporaryDirectory() as directory:
            transcript = Path(directory) / "transcript.jsonl"
            with mock.patch.object(terminal_fixture, "command_version", return_value="fixture 1") as version, \
                    mock.patch.object(terminal_fixture.subprocess, "run", side_effect=[
                        subprocess.CompletedProcess([], 0, data, b"") for data in payloads]) as run, \
                    mock.patch.object(terminal_fixture, "emit") as emit, \
                    mock.patch.object(terminal_fixture, "require_media_target"), \
                    mock.patch.object(terminal_fixture, "wait_for_media_cleanup", side_effect=failure):
                with self.assertRaises(RuntimeError) as caught:
                    terminal_fixture.render_external_graphics_clients(str(transcript), (40, 100, 800, 800))
                self.assertIs(caught.exception, failure)
                version.assert_called_once_with(["kitten", "--version"])
                self.assertEqual(run.call_count, 3)
                self.assertEqual(emit.call_count, 3)
            records = [json.loads(line) for line in transcript.read_text().splitlines()]
            self.assertEqual([r["event"] for r in records], ["graphics_client", "graphics_client", "graphics_client_error"])
            self.assertEqual(records[-1]["tool"], "kitten-file")
            self.assertEqual(records[-1]["stage"], "cleanup")
            self.assertIs(records[-1]["continuing"], False)
            self.assertNotIn("cleanup_observed", records[-1])

    def test_pre_emission_sixel_failure_still_attempts_iterm2_and_independent_sixel_but_fails_overall(self):
        payloads = [
            b"\x1b_Ga=T,i=1701;AAAA\x1b\\", b"\x1b_Ga=T,i=1702;AAAA\x1b\\",
            b"\x1b_Ga=T,i=1703,t=t;L3RtcC9raXR0eS10dHktZ3JhcGhpY3MtcHJvdG9jb2wtZmFrZQ==\x1b\\",
            b"\x1b_Ga=T,i=1704,t=s;aWNhdC1mYWtl\x1b\\",
            b"\x1b_Ga=T,i=2701;AAAA\x1b\\", b"", b"\x1b]1337;File=inline=1:AAAA\x07",
            b"\x1bPq~\x1b\\",
        ]
        with tempfile.TemporaryDirectory() as directory:
            transcript = Path(directory) / "transcript.jsonl"
            with mock.patch.object(terminal_fixture, "command_version", return_value="fixture 1"), \
                    mock.patch.object(terminal_fixture.subprocess, "run", side_effect=[
                        subprocess.CompletedProcess([], 0, data, b"") for data in payloads]) as run, \
                    mock.patch.object(terminal_fixture, "emit") as emit, \
                    mock.patch.object(terminal_fixture, "require_media_target"), \
                    mock.patch.object(terminal_fixture, "wait_for_media_cleanup"), \
                    mock.patch.object(terminal_fixture, "wait_for_graphics_drain") as drain:
                def hold():
                    drain.assert_called_once_with(str(transcript))
                    held = [json.loads(line) for line in transcript.read_text().splitlines()]
                    self.assertEqual(held[-2]["event"], "graphics_failure_hold")
                    self.assertEqual(held[-2]["timeout_seconds"], 10)
                    self.assertEqual(held[-1]["event"], "graphics_clients_complete")
                    self.assertIs(held[-1]["drained"], True)
                hold_callback = mock.Mock(side_effect=hold)
                with self.assertRaisesRegex(RuntimeError, "img2sixel produced no terminal output"):
                    terminal_fixture.render_external_graphics_clients(str(transcript), (40, 100, 800, 800), drain_output=True, hold_failure=hold_callback)
                hold_callback.assert_called_once_with()
                drain.assert_called_once_with(str(transcript))
                self.assertEqual(run.call_count, 8)
                self.assertEqual(emit.call_count, 7)
                self.assertTrue(emit.call_args.args[0].endswith(payloads[-1]))
            records = [json.loads(line) for line in transcript.read_text().splitlines()]
            records = [record for record in records if record["event"] != "graphics_failure_hold"]
            self.assertEqual(records[-3]["tool"], "timg-iterm2")
            self.assertEqual(records[-2]["tool"], "timg-sixel")
            self.assertEqual(records[-2]["event"], "graphics_client")
            self.assertEqual((records[-1]["attempted"], records[-1]["failed"]), (8, 1))
            self.assertIs(records[-1]["drained"], True)
            errors = [record for record in records if record["event"] == "graphics_client_error"]
            self.assertEqual([record["tool"] for record in errors], ["img2sixel"])

    def test_graphics_failure_hold_is_bounded_signal_interruptible_and_requires_drain(self):
        with mock.patch.object(terminal_fixture.time, "monotonic", side_effect=[0, 0, 9.99, 10]), \
                mock.patch.object(terminal_fixture.time, "sleep") as sleep:
            terminal_fixture.hold_graphics_failure(lambda: True)
            self.assertEqual(sleep.call_args_list, [mock.call(0.05), mock.call(0.05)])
        with mock.patch.object(terminal_fixture.time, "monotonic", return_value=0), \
                mock.patch.object(terminal_fixture.time, "sleep") as sleep:
            terminal_fixture.hold_graphics_failure(mock.Mock(side_effect=[True, False]))
            sleep.assert_called_once_with(0.05)
        with self.assertRaisesRegex(ValueError, "requires parser drain"):
            terminal_fixture.render_external_graphics_clients("unused", hold_failure=lambda: None)

    def test_graphics_drain_preserves_earlier_replies_and_accepts_fragmented_status(self):
        chunks = [b"\x1b_Gi=17;OK\x1b\\", b"\x1b[", b"0", b"n"]
        with tempfile.TemporaryDirectory() as directory:
            transcript = Path(directory) / "transcript.jsonl"
            with mock.patch.object(terminal_fixture, "emit") as emit, \
                    mock.patch.object(terminal_fixture.sys, "stdin") as stdin, \
                    mock.patch.object(terminal_fixture.select, "select", return_value=([99], [], [])), \
                    mock.patch.object(terminal_fixture.os, "read", side_effect=chunks):
                stdin.fileno.return_value = 99
                terminal_fixture.wait_for_graphics_drain(str(transcript))
                emit.assert_called_once_with(b"\x1b[5n")
            self.assertEqual(gui_acceptance.transcript_bytes(transcript), b"".join(chunks))

    def test_graphics_drain_fails_on_timeout_eof_and_bounded_input_exhaustion(self):
        for case, ready, data in [("timed out", False, b""), ("closed", True, b""),
                                  ("input limit", True, b"x" * 4096)]:
            with self.subTest(case=case), tempfile.TemporaryDirectory() as directory, \
                    mock.patch.object(terminal_fixture, "emit"), \
                    mock.patch.object(terminal_fixture.sys, "stdin") as stdin, \
                    mock.patch.object(terminal_fixture.select, "select", return_value=([99] if ready else [], [], [])), \
                    mock.patch.object(terminal_fixture.os, "read", return_value=data) as read:
                stdin.fileno.return_value = 99
                with self.assertRaisesRegex(RuntimeError, case):
                    terminal_fixture.wait_for_graphics_drain(str(Path(directory) / "transcript.jsonl"))
                self.assertLessEqual(read.call_count, 16)

    def test_emit_failure_aborts_without_attempting_later_clients(self):
        failure = OSError("partial PTY write")
        with tempfile.TemporaryDirectory() as directory:
            transcript = Path(directory) / "transcript.jsonl"
            with mock.patch.object(terminal_fixture, "command_version", return_value="fixture 1"), \
                    mock.patch.object(terminal_fixture.subprocess, "run", return_value=
                        subprocess.CompletedProcess([], 0, b"\x1b_Gi=1701;AAAA\x1b\\", b"")) as run, \
                    mock.patch.object(terminal_fixture, "emit", side_effect=failure):
                with self.assertRaises(OSError) as caught:
                    terminal_fixture.render_external_graphics_clients(str(transcript), (40, 100, 800, 800))
                self.assertIs(caught.exception, failure)
                self.assertEqual(run.call_count, 1)
            records = [json.loads(line) for line in transcript.read_text().splitlines()]
            self.assertEqual(len(records), 1)
            self.assertEqual(records[0]["stage"], "emit")
            self.assertIs(records[0]["continuing"], False)

    def test_graphics_fixture_extracts_only_explicit_nonzero_kitty_image_ids(self) -> None:
        payload = (
            b"\x1b[?25l"
            b"\x1b_Ga=T,i=0,q=2;ignored\x1b\\"
            b"\x1b_Ga=T,i=2701,q=2,f=100;payload\x1b\\"
        )
        self.assertEqual(terminal_fixture.kitty_image_id(payload), 2701)
        for invalid in (
            b"plain text",
            b"\x1b_Ga=T,q=2;missing\x1b\\",
            b"\x1b_Ga=T,i=not-a-number;bad\x1b\\",
            b"\x1b_Ga=T,i=4294967296;too-large\x1b\\",
        ):
            with self.subTest(invalid=invalid), self.assertRaisesRegex(
                RuntimeError, "no explicit nonzero image ID"
            ):
                terminal_fixture.kitty_image_id(invalid)

    def test_graphics_fixture_validates_local_media_targets_and_cleanup_paths(self) -> None:
        temporary = b"/dev/shm/kitty-tty-graphics-protocol-123"
        shared = b"icat-ABC123"
        payload = (
            b"\x1b_Ga=T,t=t,i=1703;"
            + base64.b64encode(temporary).rstrip(b"=")
            + b"\x1b\\"
            + b"\x1b_Ga=T,t=s,i=1704;"
            + base64.b64encode(shared)
            + b"\x1b\\"
        )
        self.assertEqual(terminal_fixture.kitty_media_target(payload, b"t"), temporary)
        self.assertEqual(terminal_fixture.kitty_media_target(payload, b"s"), shared)
        self.assertEqual(terminal_fixture.media_target_path(temporary, b"t"), temporary)
        self.assertEqual(
            terminal_fixture.media_target_path(b"/" + shared, b"s"),
            b"/dev/shm/" + shared,
        )

        for invalid_payload in (
            b"\x1b_Ga=T,t=t;iA===\x1b\\",
            b"\x1b_Ga=T,t=t;%%%\x1b\\",
            b"\x1b_Ga=T,t=t;\x1b\\",
        ):
            with self.subTest(invalid_payload=invalid_payload), self.assertRaises(
                RuntimeError
            ):
                terminal_fixture.kitty_media_target(invalid_payload, b"t")
        for invalid_target, medium in (
            (b"relative-kitty-tty-graphics-protocol", b"t"),
            (b"/tmp/unmarked", b"t"),
            (b"one/two", b"s"),
            (b"/", b"s"),
        ):
            with self.subTest(target=invalid_target), self.assertRaises(RuntimeError):
                terminal_fixture.media_target_path(invalid_target, medium)

    def test_graphics_fixture_requires_local_media_cleanup(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            target = os.fsencode(Path(directory) / "tty-graphics-protocol-pending")
            with self.assertRaisesRegex(RuntimeError, "is missing"):
                terminal_fixture.require_media_target(target, b"t")

            Path(os.fsdecode(target)).write_bytes(b"pending")
            terminal_fixture.require_media_target(target, b"t")
            with self.assertRaisesRegex(RuntimeError, "was not removed"):
                terminal_fixture.wait_for_media_cleanup(target, b"t", timeout=0)

            Path(os.fsdecode(target)).unlink()
            terminal_fixture.wait_for_media_cleanup(target, b"t", timeout=0)

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

    def test_fullscreen_geometry_may_expand_only_one_axis_but_never_shrink(self) -> None:
        initial = (24, 80, 640, 384)
        self.assertTrue(
            gui_acceptance.terminal_geometry_expanded((24, 120, 960, 384), initial)
        )
        self.assertTrue(
            gui_acceptance.terminal_geometry_expanded((36, 80, 640, 576), initial)
        )
        self.assertFalse(
            gui_acceptance.terminal_geometry_expanded((24, 80, 640, 384), initial)
        )
        self.assertFalse(
            gui_acceptance.terminal_geometry_expanded((36, 70, 560, 576), initial)
        )

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

    def test_input_token_matching_reassembles_each_pid_byte_stream(self) -> None:
        records = [
            {
                "event": "input",
                "pid": 41,
                "base64": base64.b64encode(b"WAYLAND_").decode(),
            },
            {
                "event": "input",
                "pid": 73,
                "base64": base64.b64encode(b"unrelated").decode(),
            },
            {
                "event": "input",
                "pid": 41,
                "base64": base64.b64encode(b"INPUT_32").decode(),
            },
        ]
        self.assertEqual(
            gui_acceptance.pids_receiving_token(records, b"WAYLAND_INPUT_32"),
            {41},
        )
        self.assertEqual(
            gui_acceptance.pid_receiving_token(records, b"WAYLAND_INPUT_32"),
            41,
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
        self.assertNotIn("kitten", launch)
        self.assertIn("sway", fullscreen)
        self.assertIn("wtype", fullscreen)
        self.assertNotIn("weston", fullscreen)
        self.assertNotIn("img2sixel", fullscreen)
        self.assertNotIn("timg", fullscreen)
        self.assertNotIn("kitten", fullscreen)

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
        self.assertTrue({"kitten", "img2sixel", "timg", "xclip"}.issubset(workspace))
        self.assertTrue({"kitten", "img2sixel", "timg", "xclip"}.isdisjoint(vim))

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

    def test_wtype_command_waits_for_virtual_keyboard_registration(self) -> None:
        self.assertEqual(
            gui_acceptance.wtype_command("WAYLAND_INPUT_32"),
            [
                "wtype",
                "-P",
                "Shift_L",
                "-p",
                "Shift_L",
                "-s",
                "100",
                "WAYLAND_INPUT_32",
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
