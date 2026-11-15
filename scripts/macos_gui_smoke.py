#!/usr/bin/env python3
"""Launch Termeric.app on a macOS session, drive it, quit it, and check it.

For CI on a Mac runner, which has a window server. The app runs with its own
temporary home, configuration, data, and runtime directories. The check:

1. the app starts and its control socket answers with a window whose shell
   tab has a pane;
2. a tab launched over the socket shows its program's output;
3. quitting through Apple Events (`terminate:`, as the menu, the Dock, and
   logging out do) ends the process and saves the window's workspace; if
   macOS refuses the Apple Event to a process without the permission, the
   windows are closed over the socket instead, which ends the app the other
   way, and the check says so;
4. the log has no error or panic.

Every wait is bounded, so a hang fails the step rather than the runner.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time


BUNDLE_ID = "org.termeric.Termeric"
MARKER = "termeric-gui-smoke-ok"


class SmokeError(RuntimeError):
    pass


def find_key(value, key):
    """The first value named `key` anywhere in decoded JSON."""
    if isinstance(value, dict):
        if key in value:
            return value[key]
        value = list(value.values())
    if isinstance(value, list):
        for item in value:
            found = find_key(item, key)
            if found is not None:
                return found
    return None


def wait_for(description: str, timeout: float, attempt):
    deadline = time.monotonic() + timeout
    last_error = None
    while time.monotonic() < deadline:
        try:
            result = attempt()
            if result:
                return result
        except (subprocess.SubprocessError, ValueError, KeyError, IndexError) as error:
            last_error = error
        time.sleep(0.5)
    raise SmokeError(f"timed out waiting for {description}; last error: {last_error}")


class Smoke:
    def __init__(self, app: Path, root: Path) -> None:
        self.binary = app / "Contents/MacOS/termeric"
        if not self.binary.is_file():
            raise SmokeError(f"no executable in {app}")
        self.root = root
        home = root / "home"
        runtime = root / "run"
        for directory in (home, runtime):
            directory.mkdir(mode=0o700)
        self.env = dict(os.environ)
        self.env.update(
            {
                "HOME": str(home),
                "XDG_CONFIG_HOME": str(home / ".config"),
                "XDG_DATA_HOME": str(home / ".local/share"),
                "XDG_RUNTIME_DIR": str(runtime),
                "RUST_LOG": "info",
                "RUST_BACKTRACE": "1",
            }
        )
        self.log_path = root / "termeric.log"
        self.process: subprocess.Popen[bytes] | None = None

    def control(self, *arguments: str) -> str:
        result = subprocess.run(
            [str(self.binary), "control", *arguments],
            env=self.env,
            capture_output=True,
            timeout=20,
        )
        if result.returncode != 0:
            raise subprocess.SubprocessError(result.stderr.decode(errors="replace"))
        return result.stdout.decode()

    def inventory(self) -> dict:
        return json.loads(self.control("list", "--json"))

    def start(self) -> None:
        log = self.log_path.open("wb")
        self.process = subprocess.Popen(
            [str(self.binary)],
            env=self.env,
            stdin=subprocess.DEVNULL,
            stdout=log,
            stderr=subprocess.STDOUT,
        )

    def first_window(self) -> tuple[int, dict]:
        def ready():
            processes = self.inventory()["processes"]
            if not processes or not processes[0]["windows"]:
                return None
            window = processes[0]["windows"][0]
            if not window["tabs"] or not window["tabs"][0]["panes"]:
                return None
            return processes[0]["process_id"], window
        return wait_for("the first window", 90, ready)

    def pane_text(self, process: int, window: int, tab: int, pane: int) -> str:
        return self.control("get-text", str(process), str(window), str(tab), str(pane))

    def run(self) -> None:
        self.start()
        process_id, window = self.first_window()
        print(f"ok the app started: process {process_id}, window {window['id']}")
        shell_tab = window["tabs"][0]
        wait_for(
            "the shell to draw its prompt",
            60,
            lambda: self.pane_text(
                process_id, window["id"], shell_tab["id"], shell_tab["panes"][0]["id"]
            ).strip(),
        )
        print("ok the shell tab shows a prompt")

        launched = json.loads(
            self.control(
                "launch-tab",
                str(process_id),
                str(window["id"]),
                "--title",
                "smoke",
                "--json",
                "--",
                "/bin/sh",
                "-c",
                f"echo {MARKER}; sleep 60",
            )
        )
        tab_id = find_key(launched, "tab_id")
        if tab_id is None:
            raise SmokeError(f"launch-tab named no tab: {launched}")
        seen = {}

        def launched_output():
            for tab in self.inventory()["processes"][0]["windows"][0]["tabs"]:
                if tab["id"] == tab_id and tab["panes"]:
                    seen["tab"] = tab
                    seen["text"] = self.pane_text(
                        process_id, window["id"], tab["id"], tab["panes"][0]["id"]
                    )
                    return MARKER in seen["text"]
            seen["tabs"] = self.inventory()
            return False

        try:
            wait_for("the launched tab's output", 30, launched_output)
        except SmokeError as error:
            raise SmokeError(f"{error}; saw {seen}") from None
        print("ok a launched tab shows its program's output")

        self.quit(process_id, window["id"])
        self.check_saved()
        self.check_log()

    def quit(self, process_id: int, window_id: int) -> None:
        assert self.process is not None
        try:
            quit_by_apple_event = subprocess.run(
                ["osascript", "-e", f'tell application id "{BUNDLE_ID}" to quit'],
                capture_output=True,
                timeout=30,
            )
            refusal = (
                None
                if quit_by_apple_event.returncode == 0
                else quit_by_apple_event.stderr.decode(errors="replace").strip()
            )
        except subprocess.TimeoutExpired:
            refusal = "osascript did not answer within 30 seconds"
        if refusal is None:
            how = "an Apple Event (terminate:)"
        else:
            # Quitting through terminate: went untested; say so where CI shows
            # it, then end the app the other way so the rest still runs.
            print(f"::warning::the GUI smoke could not quit through Apple Events ({refusal}); terminate: is untested in this run")
            self.control("close-window", str(process_id), str(window_id))
            how = "closing its last window"
        try:
            code = self.process.wait(timeout=30)
        except subprocess.TimeoutExpired:
            self.process.kill()
            raise SmokeError(f"the app did not exit within 30 seconds of {how}")
        if code != 0:
            raise SmokeError(f"the app exited with {code} after {how}")
        print(f"ok the app quit through {how}")

    def check_saved(self) -> None:
        state = Path(self.env["XDG_DATA_HOME"]) / "termeric/state.json"
        if not state.is_file():
            raise SmokeError("quitting saved no state.json")
        saved = json.loads(state.read_text())
        if not saved.get("last_workspace_ids"):
            raise SmokeError(f"quitting kept no workspace to restore: {saved}")
        print("ok quitting kept the window's workspace for the next start")

    def check_log(self) -> None:
        log = self.log_path.read_text(errors="replace")
        problems = [
            line
            for line in log.splitlines()
            if " ERROR " in line or "panicked" in line or "objc2" in line.lower() and "error" in line.lower()
        ]
        if problems:
            raise SmokeError("the log reports problems:\n" + "\n".join(problems[:40]))
        print("ok the log has no errors")

    def stop(self) -> None:
        if self.process is not None and self.process.poll() is None:
            self.process.kill()
            self.process.wait(timeout=10)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--app", type=Path, required=True, help="the Termeric.app to run")
    parser.add_argument("--keep-log", type=Path, help="copy the app's log here afterwards")
    arguments = parser.parse_args(argv)
    # A short root: socket paths must fit macOS's 104 bytes.
    root = Path(tempfile.mkdtemp(prefix="tgs-", dir="/tmp"))
    smoke = Smoke(arguments.app.resolve(), root)
    try:
        smoke.run()
    except (SmokeError, OSError, subprocess.SubprocessError) as error:
        print(f"GUI smoke failed: {error}", file=sys.stderr)
        if smoke.log_path.is_file():
            print("--- app log (last 80 lines) ---", file=sys.stderr)
            print("\n".join(smoke.log_path.read_text(errors="replace").splitlines()[-80:]), file=sys.stderr)
        return 1
    finally:
        smoke.stop()
        if arguments.keep_log and smoke.log_path.is_file():
            shutil.copyfile(smoke.log_path, arguments.keep_log)
        shutil.rmtree(root, ignore_errors=True)
    print("GUI smoke passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
