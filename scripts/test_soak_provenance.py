"""Runner-path provenance regressions; all sources/processes are private fakes."""
import contextlib
import io
import json
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import sys
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

from scripts import run_soak
from scripts.test_validate_soak import resource_sample
from scripts.soak_provenance import MAX_EXECUTABLE_BYTES, PinnedExecutable, provenance_checks


@unittest.skipUnless(sys.platform == "linux", "Linux runner-path fixtures")
class CoreRunnerProvenanceTests(unittest.TestCase):
    def invoke(self, mode="run-drift", *, gpu=False):
        with TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = root / "target/debug/deps/fixture"
            binary.parent.mkdir(parents=True)
            binary.write_bytes(b"binary built from source A")
            binary.chmod(0o700)
            bundle = root / "evidence"
            report_path = bundle / "results.json"
            log_path = bundle / "scenario.log"
            state = {"commit": "a" * 40, "dirty": mode in {"dirty", "dirty-to-clean"}, "launches": []}
            progress = [
                {"iteration": 1, "scrollback_bytes": 1024, "image_bytes": 4096,
                 "transcript_queue_bytes": 2048, "transcript_queue_bytes_after_finish": 0,
                 "transcript_entries_accepted": 68, "transcript_entries_dropped": 0},
                {"complete": True, "iterations": 1},
            ]

            class FakeProcess:
                pid = 12345
                returncode = 0

                def __init__(self, command, **kwargs):
                    self.polls = 0
                    state["fd"] = int(Path(command[0]).name)
                    state["launches"].append("scenario")
                    state["launched_bytes"] = Path(command[0]).read_bytes()
                    if mode == "launch-failure":
                        raise OSError("injected launch failure")
                    if mode == "run-drift":
                        state["commit"] = "b" * 40
                    if mode == "run-dirty":
                        state["dirty"] = True
                    if mode in {"run-drift", "binary-only", "restored-path"}:
                        replacement = binary.with_suffix(".new")
                        replacement.write_bytes(b"binary built from source B")
                        replacement.chmod(0o700)
                        replacement.replace(binary)
                        state["pinned_after_replace"] = Path(command[0]).read_bytes()
                        if mode == "restored-path":
                            replacement.write_bytes(b"binary built from source A")
                            replacement.chmod(0o700)
                            replacement.replace(binary)
                    for record in progress:
                        kwargs["stdout"].write("TERMERIC_SOAK " + json.dumps(record) + "\n")
                    kwargs["stdout"].flush()

                def poll(self):
                    self.polls += 1
                    if mode == "timeout":
                        return self.returncode if state.get("killed") else None
                    return None if self.polls <= (3 if mode == "transient-sample" else 2) else 0

                def kill(self):
                    state["killed"] = True
                    self.returncode = -9

                def wait(self, **kwargs):
                    return self.returncode

            def build():
                if mode == "build-drift":
                    state["commit"] = "b" * 40
                if mode == "dirty-to-clean":
                    state["dirty"] = False
                return binary

            def fake_command(command, **kwargs):
                if command[:2] == ["git", "rev-parse"]:
                    output = state["commit"] + "\n"
                elif command[:2] == ["git", "status"]:
                    output = " M fixture\n" if state["dirty"] else ""
                elif command == ["rustc", "--version"]:
                    output = "rustc fixture\n"
                elif len(command) > 1 and command[1] == "test_support::linux_headless_gpu_render_smoke_when_requested":
                    state["launches"].append("gpu-smoke")
                    state["fd"] = int(Path(command[0]).name)
                    state["gpu_bytes"] = Path(command[0]).read_bytes()
                    if mode == "gpu-drift":
                        state["commit"] = "b" * 40
                    if mode == "gpu-timeout":
                        raise subprocess.TimeoutExpired(command, 60)
                    output = ""
                else:
                    raise AssertionError(command)
                return subprocess.CompletedProcess(command, 0, stdout=output)

            with (
                patch.object(run_soak, "ROOT", root),
                patch.object(run_soak.platform, "platform", return_value="Linux-fixture"),
                patch.object(run_soak, "build_test_binary", side_effect=build),
                patch.object(run_soak.subprocess, "Popen", FakeProcess),
                patch.object(run_soak.subprocess, "run", side_effect=fake_command),
                patch.object(run_soak, "process_snapshot", side_effect=KeyboardInterrupt() if mode == "interrupt" else ([FileNotFoundError()] if mode == "transient-sample" else []) + [resource_sample(0), resource_sample(1)]),
                patch.object(run_soak.time, "sleep"),
                (patch.object(run_soak.time, "monotonic", side_effect=[0.0, 100.0]) if mode == "timeout" else contextlib.nullcontext()),
                patch.object(sys, "argv", ["run_soak.py", "--duration-seconds", "1",
                    "--sample-interval", "0.5", "--output", str(report_path), "--log", str(log_path), *(["--gpu-smoke"] if gpu else [])]),
                contextlib.redirect_stdout(io.StringIO()),
            ):
                if mode in {"unchanged", "dirty", "transient-sample"}:
                    run_soak.main()
                else:
                    with self.assertRaises(SystemExit):
                        run_soak.main()
            report = json.loads(report_path.read_text())
            self.assertEqual(report["environment"]["commit"], "a" * 40)
            self.assertEqual(report["test_binary"]["sha256"], hashlib.sha256(b"binary built from source A").hexdigest())
            if "fd" in state:
                with self.assertRaises(OSError):
                    os.fstat(state["fd"])
            return report, state

    def test_checkout_and_rebuild_during_run_cannot_be_relabelled_as_a_candidate(self):
        report, state = self.invoke()
        self.assertFalse(report["candidate_eligible"])
        self.assertEqual(state["launched_bytes"], b"binary built from source A")
        self.assertEqual(state["pinned_after_replace"], b"binary built from source A")

    def test_unchanged_source_uses_one_measured_object_for_both_launches(self):
        report, state = self.invoke("unchanged", gpu=True)
        self.assertTrue(report["candidate_eligible"])
        self.assertEqual(state["launches"], ["gpu-smoke", "scenario"])
        self.assertEqual(state["gpu_bytes"], state["launched_bytes"])

    def test_source_changes_during_build_prevent_any_launch(self):
        for mode in ("build-drift", "dirty-to-clean"):
            report, state = self.invoke(mode)
            self.assertFalse(report["candidate_eligible"])
            self.assertEqual(state["launches"], [])

    def test_source_changes_after_gpu_smoke_prevent_scenario_launch(self):
        report, state = self.invoke("gpu-drift", gpu=True)
        self.assertFalse(report["candidate_eligible"])
        self.assertEqual(state["launches"], ["gpu-smoke"])

    def test_binary_replacement_restoration_and_dirty_source_are_not_promoted(self):
        for mode in ("binary-only", "restored-path", "run-dirty", "dirty"):
            report, _ = self.invoke(mode)
            self.assertFalse(report["candidate_eligible"])

    def test_launch_failure_and_interruption_close_the_pinned_descriptor(self):
        for mode in ("launch-failure", "interrupt", "timeout"):
            report, state = self.invoke(mode)
            self.assertEqual(report["status"], "fail")
            if mode in {"interrupt", "timeout"}:
                self.assertTrue(state["killed"])

    def test_gpu_timeout_does_not_launch_a_scenario_or_leak_the_descriptor(self):
        report, state = self.invoke("gpu-timeout", gpu=True)
        self.assertFalse(report["candidate_eligible"])
        self.assertEqual(state["launches"], ["gpu-smoke"])

    def test_transient_proc_observation_does_not_kill_the_live_scenario(self):
        report, state = self.invoke("transient-sample")
        self.assertTrue(report["candidate_eligible"])
        self.assertNotIn("killed", state)


@unittest.skipUnless(sys.platform == "linux", "Linux descriptor execution contract")
class ExecutablePinTests(unittest.TestCase):
    def test_real_exec_uses_pinned_bytes_after_path_replacement_without_fd_inheritance(self):
        initial = {"commit": "a" * 40, "worktree_dirty": False}
        with TemporaryDirectory() as temporary, patch("scripts.soak_provenance.source_identity", side_effect=lambda _: dict(initial)):
            root = Path(temporary)
            binary = root / "interpreter"
            shutil.copyfile(Path(sys.executable).resolve(), binary)
            binary.chmod(0o700)
            with PinnedExecutable(root, binary, initial) as executable:
                command = executable.launch("scenario")
                replacement = root / "replacement"
                replacement.write_bytes(b"not executable interpreter bytes")
                replacement.chmod(0o700)
                replacement.replace(binary)
                result = subprocess.run(
                    [str(command), "-I", "-S", "-c", f"import os; print(os.path.exists('/proc/self/fd/{executable.fd}'))"],
                    capture_output=True, text=True, check=True, timeout=5, close_fds=True,
                )
                self.assertEqual(result.stdout.strip(), "False")
                proof = executable.finish()
                self.assertFalse(all(item["passed"] for item in provenance_checks(proof, ["scenario"])))
                descriptor = executable.fd
            with self.assertRaises(OSError):
                os.fstat(descriptor)

    def test_symlink_fifo_directory_oversize_and_unstable_measurement_fail_closed(self):
        initial = {"commit": "a" * 40, "worktree_dirty": False}
        with TemporaryDirectory() as temporary:
            root = Path(temporary)
            regular = root / "regular"
            regular.write_bytes(b"fixture")
            regular.chmod(0o700)
            symlink = root / "symlink"
            symlink.symlink_to(regular)
            fifo = root / "fifo"
            os.mkfifo(fifo, 0o700)
            directory = root / "directory"
            directory.mkdir()
            oversized = root / "oversized"
            with oversized.open("wb") as file:
                file.truncate(MAX_EXECUTABLE_BYTES + 1)
            oversized.chmod(0o700)
            before = len(list(Path("/proc/self/fd").iterdir()))
            for path in (symlink, fifo, directory, oversized, root / "../outside"):
                with self.assertRaises((ValueError, OSError)):
                    PinnedExecutable(root, path, initial)
            with patch("scripts.soak_provenance.os.pread", return_value=b""):
                with self.assertRaises(ValueError):
                    PinnedExecutable(root, regular, initial)
            self.assertEqual(len(list(Path("/proc/self/fd").iterdir())), before)


@unittest.skipUnless(sys.platform == "linux", "Linux runner-path fixtures")
class NativeRunnerProvenanceTests(unittest.TestCase):
    def test_native_runner_retains_initial_bytes_and_source_on_drift_without_a_gui(self):
        from scripts import run_native_gui_soak as native
        initial = {"commit": "c" * 40, "worktree_dirty": False}
        state = dict(initial)
        with TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = root / "target/termeric"
            binary.parent.mkdir()
            binary.write_bytes(b"native A")
            binary.chmod(0o700)
            artifacts = root / "artifacts"
            captured = {}

            def fake_app(executable, *_):
                captured["bytes"] = Path(executable).read_bytes()
                captured["fd"] = int(Path(executable).name)
                state["commit"] = "d" * 40
                replacement = binary.with_suffix(".new")
                replacement.write_bytes(b"native B")
                replacement.chmod(0o700)
                replacement.replace(binary)
                raise native.AcceptanceError("injected stop before any real GUI")

            with (
                patch.object(native, "ROOT", root),
                patch.object(native.shutil, "which", return_value="fixture-tool"),
                patch.object(native, "git_provenance", return_value=dict(initial)),
                patch("scripts.soak_provenance.source_identity", side_effect=lambda _: dict(state)),
                patch.object(native, "run"),
                patch.object(native, "sway_environment", side_effect=lambda env, _: (env, None)),
                patch.object(native, "AppProcess", side_effect=fake_app),
                patch.object(sys, "argv", ["native-soak", "--binary", str(binary), "--artifacts", str(artifacts), "--duration-seconds", "5"]),
                contextlib.redirect_stdout(io.StringIO()),
            ):
                self.assertEqual(native.main(), 1)
            report = json.loads((artifacts / "results.json").read_text())
            self.assertEqual(captured["bytes"], b"native A")
            self.assertEqual(report["binary"]["sha256"], hashlib.sha256(b"native A").hexdigest())
            self.assertEqual(report["environment"]["commit"], initial["commit"])
            self.assertFalse(report["candidate_eligible"])
            self.assertFalse(all(item["passed"] for item in provenance_checks(report["provenance"], ["displayed"])))
            with self.assertRaises(OSError):
                os.fstat(captured["fd"])


if __name__ == "__main__":
    unittest.main()
