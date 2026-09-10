#!/usr/bin/env python3
"""Black-box native GUI acceptance for Termeric on Linux.

X11 runs semantic PTY assertions plus targeted visual snapshots. Wayland uses a
headless Weston compositor for launch, first render, and normal close. The
harness never reads or writes the user's normal configuration or session data.
"""

from __future__ import annotations

import argparse
import base64
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from typing import Callable

try:
    from PIL import Image, ImageChops
except ImportError:  # Reported as an explicit missing harness dependency in main().
    Image = None  # type: ignore[assignment]
    ImageChops = None  # type: ignore[assignment]


ROOT = Path(__file__).resolve().parents[1]
FIXTURE = ROOT / "termeric" / "tests" / "gui" / "terminal_fixture.py"
BASELINES = ROOT / "termeric" / "tests" / "gui" / "baselines"
DEFAULT_BINARY = ROOT / "target" / "debug" / "termeric"
WINDOW_SIZE = (960, 640)
ALTERNATE_SCREEN_ENTRIES = (b"\x1b[?47h", b"\x1b[?1047h", b"\x1b[?1049h")
ALTERNATE_SCREEN_EXITS = (b"\x1b[?47l", b"\x1b[?1047l", b"\x1b[?1049l")


class AcceptanceError(RuntimeError):
    pass


def run(
    command: list[str],
    *,
    env: dict[str, str] | None = None,
    input_text: str | None = None,
    timeout: float = 15,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        command,
        env=env,
        input=input_text,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=timeout,
        check=False,
    )
    if check and result.returncode != 0:
        rendered = " ".join(shlex.quote(part) for part in command)
        raise AcceptanceError(
            f"command failed ({result.returncode}): {rendered}\n"
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )
    return result


def wait_until(description: str, predicate: Callable[[], bool], timeout: float = 10) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.05)
    raise AcceptanceError(f"timed out waiting for {description}")


def toml_string(value: str) -> str:
    """JSON strings are valid TOML basic strings and escape paths safely."""
    return json.dumps(value)


def write_config(path: Path, program: str, arguments: list[str]) -> None:
    args = ", ".join(toml_string(argument) for argument in arguments)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        f"""[shell]
program = {toml_string(program)}
args = [{args}]

[font]
family = "DejaVu Sans Mono"
fallback = ["Noto Color Emoji"]
size = 14.0
line_height = 1.2
ligatures = false

[window]
width = {WINDOW_SIZE[0]}
height = {WINDOW_SIZE[1]}
padding = 8
decorations = false
opacity = 1.0

[session]
enabled = false
auto_restore = false
auto_save = false
save_scrollback = false

[splash]
mode = "never"

[plugins.claude_code]
enabled = false

[plugins.codex]
enabled = false

[bell]
visual = true
audio = true
visual_duration_ms = 1000

[security.osc52]
allow_read = true
allow_write = true
max_bytes = 1048576
""",
        encoding="utf-8",
    )


def read_records(path: Path) -> list[dict[str, object]]:
    if not path.exists():
        return []
    records = []
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.strip():
            records.append(json.loads(line))
    return records


def transcript_bytes(path: Path) -> bytes:
    chunks = []
    for record in read_records(path):
        if record.get("event") == "input":
            chunks.append(base64.b64decode(str(record["base64"])))
    return b"".join(chunks)


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def result_document(
    *,
    backend: str,
    binary: Path,
    scenarios: list[dict[str, str]],
    start_weston: bool,
    start_sway: bool,
    start_window_manager: bool,
    force_surface_loss: bool,
    force_surface_outdated: bool,
    force_device_loss: bool,
) -> dict[str, object]:
    compositor = (
        "weston"
        if start_weston
        else "sway"
        if start_sway
        else "openbox"
        if start_window_manager
        else "external"
    )
    return {
        "schema": 3,
        "recorded_at_utc": datetime.now(timezone.utc).isoformat(),
        "backend": backend,
        "compositor": compositor,
        "binary": str(binary),
        "binary_sha256": file_sha256(binary),
        "renderer_injections": {
            "surface_loss": force_surface_loss,
            "surface_outdated": force_surface_outdated,
            "device_loss": force_device_loss,
        },
        "scenarios": scenarios,
    }


def make_desktop_spies(directory: Path, event_log: Path) -> Path:
    binary_dir = directory / "bin"
    binary_dir.mkdir(parents=True, exist_ok=True)
    for name in ("xdg-open", "notify-send", "paplay"):
        target = binary_dir / name
        target.write_text(
            "#!/bin/sh\n"
            f"printf '%s\\t%s\\n' {shlex.quote(name)} \"$*\" >> "
            f"{shlex.quote(str(event_log))}\n",
            encoding="utf-8",
        )
        target.chmod(0o755)
    return binary_dir


class AppProcess:
    def __init__(
        self,
        binary: Path,
        run_dir: Path,
        backend: str,
        program: str,
        arguments: list[str],
        inherited_env: dict[str, str],
        termeric_arguments: list[str] | None = None,
    ) -> None:
        self.run_dir = run_dir
        self.run_dir.mkdir(parents=True, exist_ok=True)
        self.log_path = run_dir / "termeric.log"
        self.stdout_path = run_dir / "termeric.stdout"
        self.desktop_events = run_dir / "desktop-events.tsv"
        xdg_config = run_dir / "xdg-config"
        xdg_data = run_dir / "xdg-data"
        xdg_cache = run_dir / "xdg-cache"
        isolated_home = run_dir / "home"
        tmux_runtime = run_dir / "tmux-runtime"
        isolated_home.mkdir(parents=True, exist_ok=True)
        tmux_runtime.mkdir(parents=True, exist_ok=True, mode=0o700)
        tmux_runtime.chmod(0o700)
        write_config(xdg_config / "termeric" / "config.toml", program, arguments)
        spy_bin = make_desktop_spies(run_dir, self.desktop_events)

        self.env = dict(inherited_env)
        self.env.update(
            {
                "XDG_CONFIG_HOME": str(xdg_config),
                "XDG_DATA_HOME": str(xdg_data),
                "XDG_CACHE_HOME": str(xdg_cache),
                "HOME": str(isolated_home),
                "TMUX_TMPDIR": str(tmux_runtime),
                "RUST_LOG": "termeric=trace,wgpu_core=warn,wgpu_hal=warn",
                "WGPU_BACKEND": self.env.get("WGPU_BACKEND", "vulkan"),
                "TERMERIC_DESKTOP_EVENTS": str(self.desktop_events),
                "PATH": f"{spy_bin}{os.pathsep}{self.env.get('PATH', '')}",
            }
        )
        if backend == "x11":
            self.env["WINIT_UNIX_BACKEND"] = "x11"
            self.env.setdefault("EGL_PLATFORM", "x11")
            # wgpu-hal probes Wayland before X11 even for an Xlib window. Point
            # the probe at a guaranteed-absent socket so GLES chooses X11.
            self.env["WAYLAND_DISPLAY"] = ""
        else:
            self.env["WINIT_UNIX_BACKEND"] = "wayland"
            self.env.pop("DISPLAY", None)

        self._stdout = self.stdout_path.open("w", encoding="utf-8")
        self._stderr = self.log_path.open("w", encoding="utf-8")
        self.process = subprocess.Popen(
            [str(binary), *(termeric_arguments or [])],
            env=self.env,
            cwd=ROOT,
            stdout=self._stdout,
            stderr=self._stderr,
            text=True,
        )
        self.window_id: str | None = None

    def log_contains(self, text: str) -> bool:
        self._stderr.flush()
        return text in self.log_path.read_text(encoding="utf-8", errors="replace")

    def log_count(self, text: str) -> int:
        self._stderr.flush()
        return self.log_path.read_text(encoding="utf-8", errors="replace").count(text)

    def wait_ready(self, backend: str, resize_to: tuple[int, int] | None = WINDOW_SIZE) -> None:
        def initialized() -> bool:
            if self.process.poll() is not None:
                raise AcceptanceError(
                    f"Termeric exited during startup ({self.process.returncode}); "
                    f"see {self.log_path}"
                )
            return self.log_contains("Tab manager initialized")

        wait_until("Termeric's first terminal and render", initialized, 20)
        wait_until(
            "renderer adapter diagnostics",
            lambda: self.log_contains("preference=")
            and self.log_contains("device_type=")
            and self.log_contains("software="),
        )
        if backend == "x11":
            result: list[str] = []

            def find_window() -> bool:
                found = run(
                    [
                        "xdotool",
                        "search",
                        "--onlyvisible",
                        "--pid",
                        str(self.process.pid),
                        "--name",
                        "Termeric",
                    ],
                    env=self.env,
                    check=False,
                )
                if found.returncode == 0 and found.stdout.strip():
                    result.append(found.stdout.splitlines()[-1])
                    return True
                return False

            wait_until("the native X11 window", find_window)
            self.window_id = result[-1]
            if resize_to is not None:
                self.xdo("windowsize", "--sync", self.window_id, *map(str, resize_to))
            self.activate()
        if self.env.get("TERMERIC_GUI_FORCE_SURFACE_LOSS_ONCE") == "1":
            wait_until(
                "injected surface loss",
                lambda: self.log_contains("GPU surface lost"),
            )
            if backend == "x11":
                assert self.window_id is not None
                self.xdo("windowsize", "--sync", self.window_id, "959", "639")
                self.xdo("windowsize", "--sync", self.window_id, *map(str, WINDOW_SIZE))
            elif self.env.get("SWAYSOCK"):
                self.wayland_output_resize(1000, 740)
                self.wayland_output_resize(1024, 768)
            wait_until(
                f"{backend} surface-loss recovery",
                lambda: self.log_contains("surface-recovery"),
                20,
            )
            if self.process.poll() is not None:
                raise AcceptanceError(f"Termeric exited during {backend} surface recovery")
        if self.env.get("TERMERIC_GUI_FORCE_SURFACE_OUTDATED_ONCE") == "1":
            wait_until(
                "injected outdated surface",
                lambda: self.log_contains("GPU surface outdated"),
            )
            recovery_count = self.log_count("surface-recovery")
            if backend == "x11":
                assert self.window_id is not None
                self.xdo("windowsize", "--sync", self.window_id, "958", "638")
                self.xdo("windowsize", "--sync", self.window_id, *map(str, WINDOW_SIZE))
            elif self.env.get("SWAYSOCK"):
                self.wayland_output_resize(1002, 742)
                self.wayland_output_resize(1024, 768)
            wait_until(
                f"{backend} outdated-surface recovery",
                lambda: self.log_count("surface-recovery") > recovery_count,
                20,
            )
            if self.process.poll() is not None:
                raise AcceptanceError(
                    f"Termeric exited during {backend} outdated-surface recovery"
                )
        if self.env.get("TERMERIC_GUI_FORCE_DEVICE_LOSS_ONCE") == "1":
            wait_until(
                "injected device loss",
                lambda: self.log_contains("Injecting one device-loss event"),
            )
            wait_until(
                f"{backend} device-loss renderer recreation",
                lambda: self.log_contains("Renderer recovery complete"),
                20,
            )
            if self.process.poll() is not None:
                raise AcceptanceError(f"Termeric exited during {backend} device-loss recovery")
        time.sleep(0.25)

    def xdo(self, *arguments: str) -> str:
        return run(["xdotool", *arguments], env=self.env).stdout

    def visible_x11_windows(self) -> list[str]:
        result = run(
            [
                "xdotool",
                "search",
                "--onlyvisible",
                "--pid",
                str(self.process.pid),
                "--name",
                "Termeric",
            ],
            env=self.env,
            check=False,
        )
        return result.stdout.splitlines() if result.returncode == 0 else []

    def x11_geometry(self, window_id: str) -> dict[str, int]:
        geometry = self.xdo("getwindowgeometry", "--shell", window_id)
        return {
            key: int(value)
            for line in geometry.splitlines()
            if "=" in line
            for key, value in [line.split("=", 1)]
        }

    def wayland_key(self, *arguments: str) -> None:
        run(wtype_command(*arguments), env=self.env)
        time.sleep(0.12)

    def wayland_chord(self, modifiers: tuple[str, ...], key: str) -> None:
        self.wayland_key(*wtype_chord_arguments(modifiers, key))

    def wayland_type(self, text: str) -> None:
        run(wtype_command(text), env=self.env)
        time.sleep(0.12)

    def wayland_output_resize(self, width: int, height: int) -> None:
        run(sway_output_resize_command(width, height), env=self.env)
        time.sleep(0.12)

    def activate(self) -> None:
        assert self.window_id is not None
        # Xvfb intentionally runs without a window manager, so
        # _NET_ACTIVE_WINDOW may be unavailable; XSetInputFocus still works.
        run(
            ["xdotool", "windowactivate", "--sync", self.window_id],
            env=self.env,
            check=False,
        )
        self.xdo("windowfocus", "--sync", self.window_id)

    def key(self, chord: str) -> None:
        assert self.window_id is not None
        self.activate()
        self.xdo("key", "--clearmodifiers", chord)
        time.sleep(0.12)

    def type(self, text: str) -> None:
        assert self.window_id is not None
        self.activate()
        self.xdo(
            "type",
            "--clearmodifiers",
            "--delay",
            "3",
            text,
        )
        time.sleep(0.12)

    def click(self, x: int, y: int, button: int = 1) -> None:
        assert self.window_id is not None
        self.xdo("mousemove", "--window", self.window_id, str(x), str(y))
        self.xdo("click", str(button))
        time.sleep(0.12)

    def drag(self, start: tuple[int, int], end: tuple[int, int]) -> None:
        assert self.window_id is not None
        self.activate()
        self.xdo("mousemove", "--window", self.window_id, str(start[0]), str(start[1]))
        self.xdo("mousedown", "1")
        time.sleep(0.08)
        self.xdo("mousemove", "--window", self.window_id, str(end[0]), str(end[1]))
        time.sleep(0.08)
        self.xdo("mouseup", "1")
        time.sleep(0.12)

    def snapshot(self, name: str, update: bool, *, review_only: bool = False) -> Path:
        assert self.window_id is not None
        capture = self.run_dir / f"{name}.png"
        normalized = self.run_dir / f"{name}.normalized.png"
        capture_backend = self.env.get("TERMERIC_GUI_CAPTURE", "auto")
        if capture_backend in ("auto", "imagemagick") and shutil.which("import") is not None:
            run(["import", "-window", self.window_id, str(capture)], env=self.env)
        elif capture_backend in ("auto", "spectacle") and shutil.which("spectacle") is not None:
            run(
                ["spectacle", "--background", "--activewindow", "--nonotify", "--output", str(capture)],
                env=self.env,
            )
        else:
            geometry = self.xdo("getwindowgeometry", "--shell", self.window_id)
            values = dict(
                line.split("=", 1) for line in geometry.splitlines() if "=" in line
            )
            screen = self.env.get("DISPLAY", "")
            source = f"{screen}+{values['X']},{values['Y']}"
            run(
                [
                    "ffmpeg",
                    "-loglevel",
                    "error",
                    "-y",
                    "-f",
                    "x11grab",
                    "-video_size",
                    f"{values['WIDTH']}x{values['HEIGHT']}",
                    "-i",
                    source,
                    "-frames:v",
                    "1",
                    str(capture),
                ],
                env=self.env,
            )

        if self.env.get("TERMERIC_GUI_SKIP_VISUAL") == "1":
            return capture

        assert Image is not None
        with Image.open(capture) as image:
            image = image.convert("RGB")
            if image.size != WINDOW_SIZE:
                raise AcceptanceError(f"unexpected snapshot geometry: {image.size}")
            colors = image.getcolors(maxcolors=WINDOW_SIZE[0] * WINDOW_SIZE[1])
            if colors is not None and len(colors) < 8:
                raise AcceptanceError(f"snapshot {name} has too few colors: {len(colors)}")
            resized = image.resize((160, 100), Image.Resampling.BILINEAR)
            resized.save(normalized)

        if review_only:
            return capture

        reference = BASELINES / "x11" / f"{name}.png"
        if update:
            reference.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(normalized, reference)
        elif not reference.exists():
            raise AcceptanceError(
                f"missing visual baseline {reference}; review artifacts then use "
                "--update-baselines"
            )
        else:
            assert ImageChops is not None
            with Image.open(reference).convert("RGB") as expected, Image.open(
                normalized
            ).convert("RGB") as actual:
                histogram = ImageChops.difference(expected, actual).histogram()
            squared = sum((index % 256) ** 2 * count for index, count in enumerate(histogram))
            error = (squared / (160 * 100 * 3)) ** 0.5 / 255.0
            if error > 0.12:
                raise AcceptanceError(
                    f"snapshot {name} differs from its baseline (normalized RMSE {error:.4f} "
                    f"> 0.1200); inspect {capture}"
                )
        return capture

    def close_normally(self, backend: str) -> None:
        if self.process.poll() is not None:
            return
        if backend == "x11":
            assert self.window_id is not None
            self.activate()
            self.xdo("key", "--clearmodifiers", "alt+F4")
            close_attempts = 1
            close_timeout = 8
        else:
            close_attempts = 3
            close_timeout = 3
        return_code: int | None = None
        for _ in range(close_attempts):
            if backend != "x11":
                self.wayland_chord(("alt",), "F4")
            try:
                return_code = self.process.wait(timeout=close_timeout)
                break
            except subprocess.TimeoutExpired:
                continue
        if return_code is None:
            raise AcceptanceError("Termeric did not honor the native close request")
        if return_code != 0:
            raise AcceptanceError(f"Termeric exited with status {return_code}")
        if not self.log_contains("Close requested"):
            raise AcceptanceError("native close did not reach WindowEvent::CloseRequested")

    def stop_after_failure(self) -> None:
        if self.process.poll() is None:
            self.process.send_signal(signal.SIGTERM)
            try:
                self.process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=3)

    def finish(self) -> None:
        self._stdout.close()
        self._stderr.close()


def fixture_command(
    transcript: Path,
    instance: int = 0,
    exit_after: float | None = None,
    *,
    external_graphics: bool = False,
) -> tuple[str, list[str]]:
    arguments = [
        str(FIXTURE),
        "--transcript",
        str(transcript),
        "--instance",
        str(instance),
    ]
    if exit_after is not None:
        arguments.extend(["--exit-after", str(exit_after)])
    if not external_graphics:
        arguments.append("--skip-external-graphics")
    return sys.executable, arguments


def assert_no_runtime_errors(app: AppProcess) -> None:
    app._stderr.flush()
    log = app.log_path.read_text(encoding="utf-8", errors="replace")
    forbidden = (
        "panicked at",
        "Validation Error",
        "OutOfMemory",
        "Failed to initialize renderer",
        "Failed to initialize tabs",
    )
    found = [needle for needle in forbidden if needle in log]
    if found:
        raise AcceptanceError(f"runtime log contains errors {found}; see {app.log_path}")


def terminal_geometry(record: dict[str, object]) -> tuple[int, int, int, int]:
    return tuple(
        int(record[key]) for key in ("rows", "cols", "pixel_width", "pixel_height")
    )  # type: ignore[return-value]


def latest_terminal_geometry(
    records: list[dict[str, object]], pid: int
) -> tuple[int, int, int, int]:
    candidates = [
        record
        for record in records
        if int(record.get("pid", -1)) == pid
        and record.get("event") in ("started", "winsize")
    ]
    if not candidates:
        raise AcceptanceError(f"no PTY geometry recorded for fixture pid {pid}")
    return terminal_geometry(candidates[-1])


def pid_receiving_token(records: list[dict[str, object]], token: bytes) -> int | None:
    for pid, stream in input_streams(records).items():
        if token in stream:
            return pid
    return None


def pids_receiving_token(records: list[dict[str, object]], token: bytes) -> set[int]:
    return {pid for pid, stream in input_streams(records).items() if token in stream}


def input_streams(records: list[dict[str, object]]) -> dict[int, bytearray]:
    streams: dict[int, bytearray] = {}
    for record in records:
        if record.get("event") != "input":
            continue
        pid = int(record["pid"])
        streams.setdefault(pid, bytearray()).extend(
            base64.b64decode(str(record.get("base64", "")))
        )
    return streams


def wtype_chord_arguments(modifiers: tuple[str, ...], key: str) -> list[str]:
    arguments: list[str] = []
    for modifier in modifiers:
        arguments.extend(["-M", modifier])
    arguments.extend(["-P", key, "-p", key])
    for modifier in reversed(modifiers):
        arguments.extend(["-m", modifier])
    return arguments


def wtype_command(*arguments: str) -> list[str]:
    # A newly connected virtual keyboard can otherwise lose the beginning of its
    # first input sequence while wlroots registers it with the seat. A Shift key
    # cycle activates the keyboard without emitting PTY bytes; the following
    # delay keeps the requested input intact even while rendering is saturated.
    return [
        "wtype",
        "-P",
        "Shift_L",
        "-p",
        "Shift_L",
        "-s",
        "100",
        *arguments,
    ]


def sway_output_resize_command(width: int, height: int) -> list[str]:
    if width <= 0 or height <= 0:
        raise AcceptanceError("Sway acceptance output dimensions must be positive")
    return ["swaymsg", "output", "HEADLESS-1", "resolution", f"{width}x{height}"]


def run_domain_scenario(
    binary: Path, run_dir: Path, env: dict[str, str], update: bool
) -> None:
    run_dir.mkdir(parents=True, exist_ok=True)
    domain_env = dict(env)
    runtime = run_dir / "runtime"
    runtime.mkdir(mode=0o700)
    runtime.chmod(0o700)
    domain_env["XDG_RUNTIME_DIR"] = str(runtime)
    created = run(
        [str(binary), "domain", "create", "GUI acceptance", "--json"],
        env=domain_env,
    )
    domain_id = str(json.loads(created.stdout)["domain"]["id"])
    first: AppProcess | None = None
    second: AppProcess | None = None

    def pane_text() -> str:
        return run(
            [str(binary), "domain", "get-text", domain_id, "0"],
            env=domain_env,
        ).stdout

    try:
        first = AppProcess(
            binary,
            run_dir / "first-gui",
            "x11",
            "/bin/sh",
            [],
            domain_env,
            ["domain", "attach", domain_id],
        )
        first.wait_ready("x11")
        wait_until(
            "first authenticated domain attachment",
            lambda: first is not None
            and first.log_contains(f"Attached window to persistent domain {domain_id}"),
        )
        first.type("printf 'domain-before-detach\\n'")
        first.key("Return")
        wait_until(
            "domain output before GUI detach",
            lambda: "domain-before-detach" in pane_text(),
        )
        first.type(
            "printf '\\033_Ga=T,f=32,s=1,v=1,i=73,q=2,c=2,r=1,C=1;/wAA/w==\\033\\\\'"
        )
        first.key("Return")
        first.type("printf 'domain-image-stored\\n'")
        first.key("Return")
        wait_until(
            "domain image command completed before detach",
            lambda: "domain-image-stored" in pane_text(),
        )
        first.close_normally("x11")
        assert domain_id in run(
            [str(binary), "domain", "list"], env=domain_env
        ).stdout

        second = AppProcess(
            binary,
            run_dir / "second-gui",
            "x11",
            "/bin/sh",
            [],
            domain_env,
            ["domain", "attach", domain_id],
        )
        second.wait_ready("x11")
        wait_until(
            "reattached domain relay",
            lambda: '"attachments": 1'
            in run(
                [str(binary), "domain", "panes", domain_id, "--json"],
                env=domain_env,
            ).stdout,
        )
        wait_until(
            "decoded domain image restored before live output",
            lambda: second is not None
            and second.log_contains(
                "Restored live-domain graphics snapshot: 1 images, 1 placements"
            ),
        )
        second.snapshot("domain-image-reattach", update, review_only=True)
        second.type("printf 'domain-after-reattach\\n'")
        second.key("Return")
        wait_until(
            "domain output after GUI reattach",
            lambda: "domain-after-reattach" in pane_text(),
        )
        second.close_normally("x11")
        assert_no_runtime_errors(first)
        assert_no_runtime_errors(second)
    finally:
        for app in (first, second):
            if app is not None:
                app.stop_after_failure()
                app.finish()
        run([str(binary), "domain", "kill", domain_id], env=domain_env, check=False)
        run([str(binary), "domain", "stop-server"], env=domain_env, check=False)


def run_workspace_scenario(
    binary: Path, run_dir: Path, env: dict[str, str], update: bool
) -> None:
    run_dir.mkdir(parents=True, exist_ok=True)
    transcript = run_dir / "pty-transcript.jsonl"
    transcript.unlink(missing_ok=True)
    program, arguments = fixture_command(transcript, external_graphics=True)
    app = AppProcess(binary, run_dir, "x11", program, arguments, env)
    try:
        app.wait_ready("x11")
        wait_until("fixture startup record", lambda: len(read_records(transcript)) >= 1)
        wait_until(
            "settled fixture render",
            lambda: app.log_contains("frame_hits: 35"),
            15,
        )
        wait_until(
            "Kitty image acceptance reply through the native PTY route",
            lambda: b"\x1b_Gi=17;OK\x1b\\" in transcript_bytes(transcript),
            15,
        )
        wait_until(
            "img2sixel and timg iTerm2 client output accepted by the terminal",
            lambda: app.log_contains("Accepted Sixel image")
            and app.log_contains("Accepted iTerm2 inline image")
            and {
                record.get("tool")
                for record in read_records(transcript)
                if record.get("event") == "graphics_client"
            }
            == {"img2sixel", "timg-iterm2"},
            15,
        )
        input_before_chrome = transcript_bytes(transcript)
        app.key("F6")
        app.key("Right")
        app.key("Tab")
        app.snapshot("keyboard-focus", update, review_only=True)
        app.key("Escape")
        time.sleep(0.1)
        chrome_delta = transcript_bytes(transcript)[len(input_before_chrome) :]
        if b"\x1b[17~" in chrome_delta or b"\t" in chrome_delta:
            raise AcceptanceError("F6 chrome navigation leaked bytes into the PTY")
        time.sleep(0.2)
        starts = [r for r in read_records(transcript) if r.get("event") == "started"]
        first = starts[0]
        if min(int(first[key]) for key in ("rows", "cols", "pixel_width", "pixel_height")) <= 0:
            raise AcceptanceError(f"fixture received invalid PTY geometry: {first}")
        if first.get("term") != "xterm-termeric" or first.get("colorterm") != "truecolor":
            raise AcceptanceError(f"fixture received incorrect terminal identity: {first}")
        if "terminfo" not in str(first.get("terminfo_dirs", "")):
            raise AcceptanceError(f"fixture cannot discover the terminfo database: {first}")
        app.snapshot("terminal", update)

        app.type("INPUT_TOKEN_17")
        app.key("Return")
        wait_until("typed PTY input", lambda: b"INPUT_TOKEN_17" in transcript_bytes(transcript))

        app.key("ctrl+shift+f")
        app.type("COPY_TOKEN_17")
        app.snapshot("search", update)
        # The right-side search indicator is an actual mouse hit target.
        app.click(850, 55)
        app.key("Escape")

        # Keyboard-only copy mode shares the search grid, moves its cursor to
        # the confirmed match, selects to end-of-line, and yanks without ever
        # emitting modal keys to the PTY.
        before_copy_mode = transcript_bytes(transcript)
        app.key("ctrl+shift+x")
        app.key("slash")
        app.type("COPY_TOKEN_17")
        app.key("Return")
        app.key("v")
        app.key("End")
        app.snapshot("copy-mode-selection", update, review_only=True)
        app.key("y")
        copy_mode_clipboard = run(
            ["xclip", "-selection", "clipboard", "-o"], env=app.env
        ).stdout
        if "COPY_TOKEN_17" not in copy_mode_clipboard:
            raise AcceptanceError(
                f"keyboard copy-mode clipboard mismatch: {copy_mode_clipboard!r}"
            )
        if transcript_bytes(transcript) != before_copy_mode:
            raise AcceptanceError("copy-mode navigation leaked modal keys to the PTY")

        # Quick-select's first stable label targets the fixture's OSC 8 link.
        # The xdg-open spy proves a keyboard-only activation reached the same
        # configured desktop action as Ctrl+click.
        app.key("ctrl+shift+u")
        app.snapshot("quick-select-hints", update, review_only=True)
        app.key("a")
        wait_until(
            "keyboard quick-select URL opener",
            lambda: app.desktop_events.exists()
            and "https://example.test/gui" in app.desktop_events.read_text(),
        )

        # Select the deterministic fifth fixture row and copy through Termeric.
        assert app.window_id is not None
        app.xdo("mousemove", "--window", app.window_id, "10", "114")
        app.xdo("mousedown", "1")
        app.xdo("mousemove", "--window", app.window_id, "125", "114")
        app.xdo("mouseup", "1")
        app.key("ctrl+shift+c")
        clipboard = run(["xclip", "-selection", "clipboard", "-o"], env=app.env).stdout
        if "COPY_TOKEN_17" not in clipboard:
            raise AcceptanceError(f"selection clipboard mismatch: {clipboard!r}")

        # Ctrl+click the OSC 8 link; PATH points xdg-open at an observable spy.
        app.xdo("keydown", "ctrl")
        app.click(90, 96)
        app.xdo("keyup", "ctrl")
        wait_until(
            "OSC 8 URL opener",
            lambda: app.desktop_events.exists()
            and "https://example.test/gui" in app.desktop_events.read_text(),
        )

        # Ask the PTY fixture to exercise the remaining desktop bridges. The
        # command spies make audio and notifications observable without
        # contacting a real desktop service, while xclip proves OSC 52 reached
        # the native clipboard and its read reply returned to the same PTY.
        app.key("F4")
        wait_until(
            "desktop bell and notification bridges",
            lambda: app.desktop_events.exists()
            and "paplay\t" in app.desktop_events.read_text()
            and "notify-send\t" in app.desktop_events.read_text()
            and "Desktop integration acceptance" in app.desktop_events.read_text(),
        )
        wait_until(
            "OSC 52 clipboard write",
            lambda: run(
                ["xclip", "-selection", "clipboard", "-o"],
                env=app.env,
                check=False,
            ).stdout
            == "OSC52_TOKEN_26",
        )
        wait_until(
            "OSC 52 clipboard read reply through the native PTY route",
            lambda: b"\x1b]52;c;T1NDNTJfVE9LRU5fMjY=\x1b\\"
            in transcript_bytes(transcript),
        )
        app.snapshot("desktop-integrations", update)

        # Enable mouse/focus/bracketed-paste modes from the PTY fixture.
        app.key("F2")
        wait_until(
            "terminal input modes",
            lambda: any(r.get("event") == "modes_enabled" for r in read_records(transcript)),
        )
        app.click(450, 300)
        clipboard_owner = subprocess.Popen(
            ["xclip", "-selection", "clipboard"],
            env=app.env,
            stdin=subprocess.PIPE,
            text=True,
        )
        assert clipboard_owner.stdin is not None
        clipboard_owner.stdin.write("PASTE_TOKEN_17")
        clipboard_owner.stdin.close()
        try:
            app.key("ctrl+shift+v")
            wait_until(
                "mouse and bracketed paste reports",
                lambda: b"\x1b[<0;" in transcript_bytes(transcript)
                and b"\x1b[200~PASTE_TOKEN_17\x1b[201~" in transcript_bytes(transcript),
            )
        finally:
            clipboard_owner.terminate()
            clipboard_owner.wait(timeout=3)
        app.key("F3")

        app.key("ctrl+shift+t")
        wait_until(
            "second tab fixture",
            lambda: sum(r.get("event") == "started" for r in read_records(transcript)) >= 2,
        )
        started_pids = [
            int(record["pid"])
            for record in read_records(transcript)
            if record.get("event") == "started"
        ]
        original_pid = started_pids[0]
        # The active second tab occupies x=200..400 at fixed acceptance
        # geometry. Dragging it over x=0..200 must make the original tab #2.
        app.drag((300, 15), (100, 15))
        app.key("ctrl+2")
        app.type("TAB_DRAG_REORDER_35")

        def drag_reordered_tabs() -> bool:
            payload = b"".join(
                base64.b64decode(str(record.get("base64", "")))
                for record in read_records(transcript)
                if record.get("event") == "input"
                and int(record.get("pid", -1)) == original_pid
            )
            return b"TAB_DRAG_REORDER_35" in payload

        wait_until("native tab drag reorder", drag_reordered_tabs)
        app.key("ctrl+1")
        app.key("alt+shift+Right")
        wait_until(
            "split-pane fixture",
            lambda: sum(r.get("event") == "started" for r in read_records(transcript)) >= 3,
        )
        app.key("alt+shift+Down")
        wait_until(
            "nested split-pane fixture",
            lambda: sum(r.get("event") == "started" for r in read_records(transcript)) >= 4,
        )
        app.type("NESTED_PANE_TOKEN_35")
        app.key("alt+Up")
        app.type("RIGHT_PANE_TOKEN_17")
        app.key("alt+Left")
        app.type("LEFT_PANE_TOKEN_17")
        app.snapshot("panes", update)
        app.key("alt+Right")
        app.key("alt+shift+w")

        # Persist the complete two-tab workspace through the user-facing
        # command, then exercise manager rename and restore controls. The
        # archive lives under this scenario's disposable XDG data directory.
        app.key("ctrl+shift+p")
        app.type("Save Workspace")
        app.key("Return")
        workspace_files: list[Path] = []

        def workspace_saved() -> bool:
            workspace_files[:] = list((run_dir / "xdg-data").rglob("workspace.toml"))
            return len(workspace_files) == 1

        wait_until("complete workspace archive", workspace_saved)
        app.key("ctrl+shift+p")
        app.type("Workspace Manager")
        app.key("Return")
        # Exact coordinates mirror the renderer's 320px minimum panel and
        # Load/Rename/Delete button geometry for the fixed 960x640 fixture.
        app.click(830, 91)
        app.xdo("key", "--repeat", "40", "BackSpace")
        app.type("Native workspace")
        app.key("Return")
        wait_until(
            "workspace rename",
            lambda: 'name = "Native workspace"' in workspace_files[0].read_text(),
        )
        starts_before_restore = sum(
            record.get("event") == "started" for record in read_records(transcript)
        )
        app.click(770, 91)
        wait_until(
            "workspace tab and pane relaunch",
            lambda: sum(
                record.get("event") == "started" for record in read_records(transcript)
            )
            >= starts_before_restore + 2,
        )
        app.type("RESTORED_WORKSPACE_FOCUS_27")
        wait_until(
            "restored workspace focused-terminal input",
            lambda: b"RESTORED_WORKSPACE_FOCUS_27" in transcript_bytes(transcript),
        )

        # Create a second native window in the same process, route input to it,
        # close it, and prove the original renderer/PTY remain alive.
        original_window = app.window_id
        app.key("ctrl+shift+p")
        app.type("New Window")
        app.key("Return")
        window_ids: list[str] = []

        def visible_window_ids() -> list[str]:
            result = run(
                [
                    "xdotool",
                    "search",
                    "--onlyvisible",
                    "--pid",
                    str(app.process.pid),
                    "--name",
                    "Termeric",
                ],
                env=app.env,
                check=False,
            )
            return result.stdout.splitlines() if result.returncode == 0 else []

        def two_windows() -> bool:
            window_ids[:] = visible_window_ids()
            return len(set(window_ids)) == 2

        wait_until("second independent native window", two_windows)
        second_window = next(identifier for identifier in window_ids if identifier != original_window)
        app.window_id = second_window
        app.type("SECOND_WINDOW_TOKEN_28")
        wait_until(
            "second-window PTY input",
            lambda: b"SECOND_WINDOW_TOKEN_28" in transcript_bytes(transcript),
        )
        app.key("alt+F4")
        wait_until(
            "second window close without process exit",
            lambda: app.process.poll() is None
            and set(visible_window_ids()) == {original_window},
        )
        app.window_id = original_window
        app.type("FIRST_WINDOW_SURVIVES_28")
        wait_until(
            "surviving first-window PTY input",
            lambda: b"FIRST_WINDOW_SURVIVES_28" in transcript_bytes(transcript),
        )

        app.key("ctrl+comma")
        app.snapshot("settings", update)
        app.click(95, 112)  # settings navigation is a modal mouse hit target
        app.key("Escape")

        # Resize down and back up. Transcript SIGWINCH records must retain pixels.
        app.xdo("windowsize", "--sync", app.window_id, "811", "533")
        app.xdo("windowsize", "--sync", app.window_id, *map(str, WINDOW_SIZE))
        wait_until(
            "PTY pixel resize record",
            lambda: sum(r.get("event") == "winsize" for r in read_records(transcript)) >= 2,
        )
        for record in read_records(transcript):
            if record.get("event") == "winsize" and min(
                int(record[key]) for key in ("rows", "cols", "pixel_width", "pixel_height")
            ) <= 0:
                raise AcceptanceError(f"resize lost PTY pixel geometry: {record}")

        app.close_normally("x11")
        assert_no_runtime_errors(app)
    finally:
        app.stop_after_failure()
        app.finish()


def run_x11_fullscreen_scenario(
    binary: Path, run_dir: Path, env: dict[str, str], _update: bool
) -> None:
    """Prove fullscreen is native, reversible, and isolated to one window."""
    run_dir.mkdir(parents=True, exist_ok=True)
    transcript = run_dir / "pty-transcript.jsonl"
    transcript.unlink(missing_ok=True)
    program, arguments = fixture_command(transcript, instance=170)
    app = AppProcess(
        binary,
        run_dir,
        "x11",
        program,
        arguments,
        env,
        termeric_arguments=["--new-window", "--new-window"],
    )
    try:
        app.wait_ready("x11", resize_to=None)
        window_ids: list[str] = []

        def two_windows() -> bool:
            window_ids[:] = app.visible_x11_windows()
            return len(set(window_ids)) == 2

        wait_until("two native X11 windows for fullscreen isolation", two_windows)
        wait_until(
            "two fullscreen fixture PTYs",
            lambda: sum(
                record.get("event") == "started" for record in read_records(transcript)
            )
            >= 2,
        )
        target_window, sibling_window = sorted(set(window_ids))
        app.window_id = target_window
        app.type("FULLSCREEN_TARGET_17")
        wait_until(
            "target-window PTY identity",
            lambda: pid_receiving_token(read_records(transcript), b"FULLSCREEN_TARGET_17")
            is not None,
        )
        target_pid = pid_receiving_token(
            read_records(transcript), b"FULLSCREEN_TARGET_17"
        )
        assert target_pid is not None

        app.window_id = sibling_window
        app.type("FULLSCREEN_SIBLING_17")
        wait_until(
            "sibling-window PTY identity",
            lambda: pid_receiving_token(read_records(transcript), b"FULLSCREEN_SIBLING_17")
            is not None,
        )
        sibling_pid = pid_receiving_token(
            read_records(transcript), b"FULLSCREEN_SIBLING_17"
        )
        assert sibling_pid is not None and sibling_pid != target_pid

        normal_window = app.x11_geometry(target_window)
        sibling_before = app.x11_geometry(sibling_window)
        before = read_records(transcript)
        baseline_count = len(before)
        normal_terminal = latest_terminal_geometry(before, target_pid)

        app.window_id = target_window
        app.key("F11")
        wait_until(
            "X11 compositor fullscreen geometry",
            lambda: app.x11_geometry(target_window)["WIDTH"] > normal_window["WIDTH"]
            and app.x11_geometry(target_window)["HEIGHT"] > normal_window["HEIGHT"],
        )
        wait_until(
            "fullscreen PTY pixel geometry",
            lambda: any(
                record.get("event") == "winsize"
                and int(record.get("pid", -1)) == target_pid
                and terminal_geometry(record)[2] > normal_terminal[2]
                and terminal_geometry(record)[3] > normal_terminal[3]
                for record in read_records(transcript)[baseline_count:]
            ),
        )
        if app.x11_geometry(sibling_window) != sibling_before:
            raise AcceptanceError("fullscreen changed the sibling X11 window geometry")
        changed_pids = {
            int(record["pid"])
            for record in read_records(transcript)[baseline_count:]
            if record.get("event") == "winsize"
        }
        if changed_pids != {target_pid}:
            raise AcceptanceError(
                f"fullscreen resize escaped the target X11 PTY: {changed_pids}"
            )

        exit_baseline = len(read_records(transcript))
        app.key("F11")
        wait_until(
            "X11 compositor fullscreen exit",
            lambda: (
                app.x11_geometry(target_window)["WIDTH"],
                app.x11_geometry(target_window)["HEIGHT"],
            )
            == (normal_window["WIDTH"], normal_window["HEIGHT"]),
        )
        wait_until(
            "restored X11 PTY pixel geometry",
            lambda: any(
                record.get("event") == "winsize"
                and int(record.get("pid", -1)) == target_pid
                and terminal_geometry(record) == normal_terminal
                for record in read_records(transcript)[exit_baseline:]
            ),
        )
        if app.x11_geometry(sibling_window) != sibling_before:
            raise AcceptanceError("fullscreen exit changed the sibling X11 window geometry")
        if any(
            record.get("event") == "winsize"
            and int(record.get("pid", -1)) == sibling_pid
            for record in read_records(transcript)[baseline_count:]
        ):
            raise AcceptanceError("fullscreen changed the sibling X11 PTY geometry")

        app.window_id = target_window
        app.key("alt+F4")
        wait_until(
            "target fullscreen window close",
            lambda: app.process.poll() is None
            and set(app.visible_x11_windows()) == {sibling_window},
        )
        app.window_id = sibling_window
        app.close_normally("x11")
        if app.log_path.read_text(encoding="utf-8", errors="replace").count(
            "Fullscreen request"
        ) < 2:
            raise AcceptanceError("fullscreen requests were not recorded in diagnostics")
        assert_no_runtime_errors(app)
    finally:
        app.stop_after_failure()
        app.finish()


def run_wayland_fullscreen_scenario(
    binary: Path, run_dir: Path, env: dict[str, str]
) -> None:
    """Drive native Wayland fullscreen and prove only one sibling PTY resizes."""
    run_dir.mkdir(parents=True, exist_ok=True)
    transcript = run_dir / "pty-transcript.jsonl"
    transcript.unlink(missing_ok=True)
    program, arguments = fixture_command(transcript, instance=171)
    app = AppProcess(
        binary,
        run_dir,
        "wayland",
        program,
        arguments,
        env,
        termeric_arguments=["--new-window", "--new-window"],
    )
    try:
        app.wait_ready("wayland")
        wait_until(
            "two native Wayland fullscreen fixture PTYs",
            lambda: sum(
                record.get("event") == "started" for record in read_records(transcript)
            )
            >= 2,
        )
        starts = [
            record
            for record in read_records(transcript)
            if record.get("event") == "started"
        ][-2:]
        settled = read_records(transcript)
        initial = {
            int(record["pid"]): latest_terminal_geometry(settled, int(record["pid"]))
            for record in starts
        }
        baseline_count = len(settled)

        app.wayland_key("-P", "F11", "-p", "F11")

        target_pid: int | None = None

        def one_wayland_target_grew() -> bool:
            nonlocal target_pid
            events = [
                record
                for record in read_records(transcript)[baseline_count:]
                if record.get("event") == "winsize"
            ]
            grown = {
                int(record["pid"])
                for record in events
                if int(record.get("pid", -1)) in initial
                and terminal_geometry(record)[2] > initial[int(record["pid"])][2]
                and terminal_geometry(record)[3] > initial[int(record["pid"])][3]
            }
            if len(grown) == 1:
                target_pid = next(iter(grown))
                return True
            return False

        wait_until("Wayland compositor fullscreen PTY geometry", one_wayland_target_grew)
        assert target_pid is not None
        sibling_pid = next(pid for pid in initial if pid != target_pid)
        changed_pids = {
            int(record["pid"])
            for record in read_records(transcript)[baseline_count:]
            if record.get("event") == "winsize"
        }
        if changed_pids != {target_pid}:
            raise AcceptanceError(
                f"fullscreen resize escaped the target Wayland PTY: {changed_pids}"
            )

        exit_baseline = len(read_records(transcript))
        app.wayland_key("-P", "F11", "-p", "F11")
        wait_until(
            "restored Wayland PTY pixel geometry",
            lambda: any(
                record.get("event") == "winsize"
                and int(record.get("pid", -1)) == target_pid
                and terminal_geometry(record) == initial[target_pid]
                for record in read_records(transcript)[exit_baseline:]
            ),
        )
        if any(
            record.get("event") == "winsize"
            and int(record.get("pid", -1)) == sibling_pid
            for record in read_records(transcript)[baseline_count:]
        ):
            raise AcceptanceError("fullscreen changed the sibling Wayland PTY geometry")

        app.wayland_key("-M", "alt", "-P", "F4", "-p", "F4", "-m", "alt")

        def one_wayland_sibling_remains() -> bool:
            app._stderr.flush()
            log = app.log_path.read_text(encoding="utf-8", errors="replace")
            return app.process.poll() is None and log.count("Close requested") == 1

        wait_until("one Wayland sibling remains", one_wayland_sibling_remains)
        app.wayland_key("-M", "alt", "-P", "F4", "-p", "F4", "-m", "alt")
        try:
            return_code = app.process.wait(timeout=8)
        except subprocess.TimeoutExpired as error:
            raise AcceptanceError("Wayland sibling windows did not close") from error
        if return_code != 0:
            raise AcceptanceError(f"Wayland fullscreen scenario exited with {return_code}")
        log = app.log_path.read_text(encoding="utf-8", errors="replace")
        if log.count("Fullscreen request") < 2 or log.count("Close requested") < 2:
            raise AcceptanceError("Wayland fullscreen/close lifecycle was not fully observed")
        assert_no_runtime_errors(app)
    finally:
        app.stop_after_failure()
        app.finish()


def run_wayland_interaction_scenario(
    binary: Path, run_dir: Path, env: dict[str, str]
) -> None:
    """Exercise Wayland keyboard, clipboard, bracketed paste, and pane routing."""
    run_dir.mkdir(parents=True, exist_ok=True)
    transcript = run_dir / "pty-transcript.jsonl"
    transcript.unlink(missing_ok=True)
    program, arguments = fixture_command(transcript, instance=172)
    app = AppProcess(binary, run_dir, "wayland", program, arguments, env)
    try:
        app.wait_ready("wayland")
        wait_until(
            "Wayland interaction fixture startup",
            lambda: any(
                record.get("event") == "started"
                for record in read_records(transcript)
            ),
        )
        starts = [
            record
            for record in read_records(transcript)
            if record.get("event") == "started"
        ]
        original_pid = int(starts[0]["pid"])
        if min(
            int(starts[0][key])
            for key in ("rows", "cols", "pixel_width", "pixel_height")
        ) <= 0:
            raise AcceptanceError(
                f"Wayland fixture received invalid PTY geometry: {starts[0]}"
            )
        if starts[0].get("term") != "xterm-termeric":
            raise AcceptanceError(
                f"Wayland fixture received incorrect terminal identity: {starts[0]}"
            )

        app.wayland_type("WAYLAND_INPUT_32")
        app.wayland_chord((), "Return")
        wait_until(
            "Wayland native keyboard input",
            lambda: pids_receiving_token(
                read_records(transcript), b"WAYLAND_INPUT_32"
            )
            == {original_pid},
        )

        # Copy mode is entirely keyboard driven. wl-paste proves the app owns
        # the real compositor clipboard rather than only an internal buffer.
        before_copy_mode = transcript_bytes(transcript)
        app.wayland_chord(("ctrl", "shift"), "x")
        app.wayland_type("/")
        app.wayland_type("COPY_TOKEN_17")
        app.wayland_chord((), "Return")
        app.wayland_type("v")
        app.wayland_chord((), "End")
        app.wayland_type("y")
        copied = run(["wl-paste", "--no-newline"], env=app.env).stdout
        if "COPY_TOKEN_17" not in copied:
            raise AcceptanceError(f"Wayland copy-mode clipboard mismatch: {copied!r}")
        if transcript_bytes(transcript) != before_copy_mode:
            raise AcceptanceError("Wayland copy-mode navigation leaked bytes into the PTY")

        # Use a compositor-owned client as the source, then enter the app's
        # configured Paste action. Bracketed mode must frame exactly one write.
        app.wayland_chord((), "F2")
        wait_until(
            "Wayland terminal input modes",
            lambda: any(
                record.get("event") == "modes_enabled"
                for record in read_records(transcript)
            ),
        )
        clipboard_owner = subprocess.Popen(
            ["wl-copy", "--paste-once", "WAYLAND_BRACKETED_32"],
            env=app.env,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            text=True,
        )
        try:
            time.sleep(0.12)
            app.wayland_chord(("ctrl", "shift"), "v")
            wait_until(
                "Wayland compositor clipboard bracketed paste",
                lambda: b"\x1b[200~WAYLAND_BRACKETED_32\x1b[201~"
                in transcript_bytes(transcript),
            )
            return_code = clipboard_owner.wait(timeout=3)
            if return_code != 0:
                assert clipboard_owner.stderr is not None
                raise AcceptanceError(
                    "Wayland clipboard source failed: "
                    f"{clipboard_owner.stderr.read().strip()}"
                )
        finally:
            if clipboard_owner.poll() is None:
                clipboard_owner.terminate()
                clipboard_owner.wait(timeout=3)
        bracketed = b"\x1b[200~WAYLAND_BRACKETED_32\x1b[201~"
        if transcript_bytes(transcript).count(bracketed) != 1:
            raise AcceptanceError("Wayland bracketed paste was not delivered exactly once")
        app.wayland_chord((), "F3")

        # Split focuses the new right pane. Navigate back and forth with the
        # configured bindings and prove every token reaches only the exact PID.
        app.wayland_chord(("alt", "shift"), "Right")
        wait_until(
            "Wayland split-pane PTY",
            lambda: sum(
                record.get("event") == "started"
                for record in read_records(transcript)
            )
            >= 2,
        )
        split_pid = next(
            int(record["pid"])
            for record in read_records(transcript)
            if record.get("event") == "started"
            and int(record.get("pid", -1)) != original_pid
        )
        app.wayland_type("WAYLAND_RIGHT_PANE_32")
        wait_until(
            "Wayland input routed to new pane",
            lambda: pids_receiving_token(
                read_records(transcript), b"WAYLAND_RIGHT_PANE_32"
            )
            == {split_pid},
        )
        app.wayland_chord(("alt",), "Left")
        app.wayland_type("WAYLAND_LEFT_PANE_32")
        wait_until(
            "Wayland input routed back to original pane",
            lambda: pids_receiving_token(
                read_records(transcript), b"WAYLAND_LEFT_PANE_32"
            )
            == {original_pid},
        )
        app.wayland_chord(("alt",), "Right")
        app.wayland_chord(("alt", "shift"), "w")
        wait_until(
            "Wayland pane close",
            lambda: app.log_contains("Closed pane")
            and not Path(f"/proc/{split_pid}").exists(),
        )
        app.wayland_type("WAYLAND_SURVIVING_PANE_32")
        wait_until(
            "Wayland surviving-pane input",
            lambda: pids_receiving_token(
                read_records(transcript), b"WAYLAND_SURVIVING_PANE_32"
            )
            == {original_pid},
        )

        app.wayland_chord(("alt",), "F4")
        try:
            return_code = app.process.wait(timeout=8)
        except subprocess.TimeoutExpired as error:
            raise AcceptanceError(
                "Wayland interaction window did not honor native close"
            ) from error
        if return_code != 0:
            raise AcceptanceError(
                f"Wayland interaction scenario exited with {return_code}"
            )
        if not app.log_contains("Close requested"):
            raise AcceptanceError("Wayland interaction close was not recorded")
        assert_no_runtime_errors(app)
    finally:
        app.stop_after_failure()
        app.finish()


def wrapper_after(command: str, marker: Path, transcript: Path) -> tuple[str, list[str]]:
    fixture = " ".join(shlex.quote(part) for part in fixture_command(transcript)[1])
    wrapped = f"{command}; status=$?; printf '%s' \"$status\" > {shlex.quote(str(marker))}; exec {shlex.quote(sys.executable)} {fixture}"
    return "/bin/sh", ["-c", wrapped]


def recorded_full_screen_command(command: str, capture: Path) -> str:
    """Wrap a full-screen client in a bounded PTY output recording."""
    script = shutil.which("script") or "script"
    return " ".join(
        shlex.quote(part)
        for part in (
            script,
            "--quiet",
            "--return",
            "--flush",
            "--echo",
            "never",
            "--output-limit",
            "4MiB",
            "--log-out",
            str(capture),
            "--command",
            command,
        )
    )


def assert_alternate_screen_round_trip(capture: Path, application: str) -> None:
    if not capture.is_file():
        raise AcceptanceError(f"{application} PTY output capture is missing: {capture}")
    payload = capture.read_bytes()
    if not any(
        (entry := payload.find(entry_sequence)) >= 0
        and payload.find(exit_sequence, entry + len(entry_sequence)) >= 0
        for entry_sequence, exit_sequence in zip(
            ALTERNATE_SCREEN_ENTRIES, ALTERNATE_SCREEN_EXITS
        )
    ):
        raise AcceptanceError(
            f"{application} did not complete an observable alternate-screen round trip"
        )


def run_vim_scenario(binary: Path, run_dir: Path, env: dict[str, str], update: bool) -> None:
    run_dir.mkdir(parents=True, exist_ok=True)
    document = run_dir / "vim-fixture.txt"
    document.write_text("first line\nsecond line\n", encoding="utf-8")
    marker = run_dir / "vim-exit.txt"
    transcript = run_dir / "after-vim.jsonl"
    screen_capture = run_dir / "vim-screen.bin"
    marker.unlink(missing_ok=True)
    transcript.unlink(missing_ok=True)
    screen_capture.unlink(missing_ok=True)
    vim = shutil.which("vim") or "vim"
    program, arguments = wrapper_after(
        recorded_full_screen_command(
            " ".join(
                shlex.quote(part)
                for part in (
                    vim,
                    "-Nu",
                    "NONE",
                    "-n",
                    "-i",
                    "NONE",
                    str(document),
                )
            ),
            screen_capture,
        ),
        marker,
        transcript,
    )
    app = AppProcess(
        binary,
        run_dir,
        "x11",
        program,
        arguments,
        env,
    )
    try:
        app.wait_ready("x11")
        app.key("G")
        app.key("o")
        app.type("VIM_INPUT_17")
        app.snapshot("vim", update)
        app.key("Escape")
        app.type(":wq")
        app.key("Return")
        wait_until("Vim normal exit", marker.exists)
        if marker.read_text() != "0":
            raise AcceptanceError(f"Vim exited with {marker.read_text()}")
        wait_until("Vim write", lambda: "VIM_INPUT_17" in document.read_text())
        assert_alternate_screen_round_trip(screen_capture, "Vim")
        app.close_normally("x11")
        assert_no_runtime_errors(app)
    finally:
        app.stop_after_failure()
        app.finish()


def run_less_scenario(binary: Path, run_dir: Path, env: dict[str, str], update: bool) -> None:
    run_dir.mkdir(parents=True, exist_ok=True)
    document = run_dir / "less-fixture.txt"
    document.write_text(
        "".join(f"less fixture line {index:03d}\n" for index in range(1, 201)),
        encoding="utf-8",
    )
    marker = run_dir / "less-exit.txt"
    transcript = run_dir / "after-less.jsonl"
    screen_capture = run_dir / "less-screen.bin"
    marker.unlink(missing_ok=True)
    transcript.unlink(missing_ok=True)
    screen_capture.unlink(missing_ok=True)
    less = shutil.which("less") or "less"
    program, arguments = wrapper_after(
        recorded_full_screen_command(
            " ".join(
                shlex.quote(part)
                for part in (
                    "env",
                    "LESS=",
                    "LESSOPEN=",
                    "LESSCLOSE=",
                    "LESSHISTFILE=-",
                    less,
                    str(document),
                )
            ),
            screen_capture,
        ),
        marker,
        transcript,
    )
    app = AppProcess(binary, run_dir, "x11", program, arguments, env)
    try:
        app.wait_ready("x11")
        app.key("End")
        app.snapshot("less", update)
        app.key("q")
        wait_until("less normal exit", marker.exists)
        if marker.read_text() != "0":
            raise AcceptanceError(f"less exited with {marker.read_text()}")
        assert_alternate_screen_round_trip(screen_capture, "less")
        app.close_normally("x11")
        assert_no_runtime_errors(app)
    finally:
        app.stop_after_failure()
        app.finish()


def run_tmux_scenario(binary: Path, run_dir: Path, env: dict[str, str], update: bool) -> None:
    run_dir.mkdir(parents=True, exist_ok=True)
    socket = f"termeric-gui-{os.getpid()}"
    marker = run_dir / "tmux-exit.txt"
    transcript = run_dir / "after-tmux.jsonl"
    screen_capture = run_dir / "tmux-screen.bin"
    marker.unlink(missing_ok=True)
    transcript.unlink(missing_ok=True)
    screen_capture.unlink(missing_ok=True)
    tmux = shutil.which("tmux") or "tmux"
    program, arguments = wrapper_after(
        recorded_full_screen_command(
            f"{shlex.quote(tmux)} -f /dev/null -L {shlex.quote(socket)} new-session /bin/sh",
            screen_capture,
        ),
        marker,
        transcript,
    )
    app = AppProcess(binary, run_dir, "x11", program, arguments, env)
    try:
        app.wait_ready("x11")
        app.type("printf TMUX_INPUT_17")
        app.key("Return")

        captured = ""

        def pane_contains_token() -> bool:
            nonlocal captured
            result = run(
                [tmux, "-L", socket, "capture-pane", "-p"], env=app.env, check=False
            )
            captured = result.stdout
            return "TMUX_INPUT_17" in captured

        wait_until("tmux pane transcript", pane_contains_token)
        app.snapshot("tmux", update)
        app.type("exit")
        app.key("Return")
        wait_until("tmux normal exit", marker.exists)
        if marker.read_text() != "0":
            raise AcceptanceError(f"tmux exited with {marker.read_text()}")
        assert_alternate_screen_round_trip(screen_capture, "tmux")
        app.close_normally("x11")
        assert_no_runtime_errors(app)
    finally:
        run([tmux, "-L", socket, "kill-server"], env=app.env, check=False)
        app.stop_after_failure()
        app.finish()


def run_wayland_launch(binary: Path, run_dir: Path, env: dict[str, str]) -> None:
    run_dir.mkdir(parents=True, exist_ok=True)
    transcript = run_dir / "pty-transcript.jsonl"
    transcript.unlink(missing_ok=True)
    program, arguments = fixture_command(transcript, exit_after=1.5)
    app = AppProcess(binary, run_dir, "wayland", program, arguments, env)
    try:
        app.wait_ready("wayland")
        wait_until(
            "Wayland fixture startup",
            lambda: any(
                record.get("event") == "started" for record in read_records(transcript)
            ),
        )
        started = next(
            record for record in read_records(transcript) if record.get("event") == "started"
        )
        if started.get("term") != "xterm-termeric":
            raise AcceptanceError(f"Wayland fixture received incorrect TERM: {started}")
        try:
            return_code = app.process.wait(timeout=8)
        except subprocess.TimeoutExpired as error:
            raise AcceptanceError("Wayland app did not close after its PTY exited") from error
        if return_code != 0:
            raise AcceptanceError(f"Wayland app exited with status {return_code}")
        if not app.log_contains("All tabs closed, exiting"):
            raise AcceptanceError("Wayland close did not complete the terminal lifecycle")
        assert_no_runtime_errors(app)
    finally:
        app.stop_after_failure()
        app.finish()


def run_cli_launch_scenario(
    binary: Path, run_dir: Path, env: dict[str, str], _update: bool
) -> None:
    run_dir.mkdir(parents=True, exist_ok=True)
    unicode_cwd = run_dir / "unicode-根"
    unicode_cwd.mkdir()
    transcript = run_dir / "cli-transcript.jsonl"
    program, arguments = fixture_command(transcript, instance=28)
    app = AppProcess(
        binary,
        run_dir,
        "x11",
        program,
        arguments,
        env,
        termeric_arguments=[
            "--working-directory",
            str(unicode_cwd),
            "--split",
            "vertical",
            "--tab",
            "--working-directory",
            str(unicode_cwd),
            "--title",
            "CLI fixture",
            "--geometry",
            "820x540",
            "--app-id",
            "termeric.cli-acceptance",
            "--",
            program,
            *arguments,
        ],
    )
    try:
        app.wait_ready("x11", resize_to=None)
        wait_until(
            "CLI fixture startup",
            lambda: sum(
                record.get("event") == "started" for record in read_records(transcript)
            )
            >= 3,
        )
        starts = [
            record for record in read_records(transcript) if record.get("event") == "started"
        ]
        if len(starts) < 3 or any(
            record.get("cwd") != str(unicode_cwd) for record in starts
        ):
            raise AcceptanceError(f"CLI tab/pane/cwd launch was not applied: {starts}")
        assert app.window_id is not None
        geometry = dict(
            line.split("=", 1)
            for line in app.xdo("getwindowgeometry", "--shell", app.window_id).splitlines()
            if "=" in line
        )
        if (geometry.get("WIDTH"), geometry.get("HEIGHT")) != ("820", "540"):
            raise AcceptanceError(f"CLI geometry was not applied: {geometry}")
        window_class = app.xdo("getwindowclassname", app.window_id).strip()
        if window_class != "termeric.cli-acceptance":
            raise AcceptanceError(f"CLI X11 identity mismatch: {window_class!r}")
        app.close_normally("x11")
        assert_no_runtime_errors(app)
    finally:
        app.stop_after_failure()
        app.finish()


def private_runtime_directory(prefix: str) -> Path:
    runtime = Path(tempfile.mkdtemp(prefix=f"termeric-{prefix}-"))
    runtime.chmod(0o700)
    return runtime


def cleanup_private_runtime(process: subprocess.Popen[str]) -> None:
    runtime = getattr(process, "_termeric_runtime", None)
    if runtime is not None:
        shutil.rmtree(runtime, ignore_errors=True)


def weston_environment(base_env: dict[str, str], run_dir: Path) -> tuple[dict[str, str], subprocess.Popen[str]]:
    runtime = private_runtime_directory("weston")
    socket = "wayland-termeric-acceptance"
    env = dict(base_env)
    env.update({"XDG_RUNTIME_DIR": str(runtime), "WAYLAND_DISPLAY": socket})
    log = (run_dir / "weston.log").open("w", encoding="utf-8")
    process = subprocess.Popen(
        [
            "weston",
            f"--backend={env.get('TERMERIC_WESTON_BACKEND', 'headless-backend.so')}",
            "--shell=kiosk-shell.so",
            f"--socket={socket}",
            "--idle-time=0",
            "--width=1024",
            "--height=768",
        ],
        env=env,
        stdout=log,
        stderr=subprocess.STDOUT,
        text=True,
    )
    wait_until(
        "Weston Wayland socket",
        lambda: (runtime / socket).exists() or process.poll() is not None,
        10,
    )
    if process.poll() is not None:
        log.close()
        shutil.rmtree(runtime, ignore_errors=True)
        raise AcceptanceError(f"Weston exited during startup; see {run_dir / 'weston.log'}")
    process._termeric_log = log  # type: ignore[attr-defined]
    process._termeric_runtime = runtime  # type: ignore[attr-defined]
    return env, process


def sway_environment(
    base_env: dict[str, str], run_dir: Path
) -> tuple[dict[str, str], subprocess.Popen[str]]:
    runtime = private_runtime_directory("sway")
    env = dict(base_env)
    env.update(
        {
            "XDG_RUNTIME_DIR": str(runtime),
            "WLR_BACKENDS": "headless",
            "WLR_HEADLESS_OUTPUTS": "1",
            "WLR_LIBINPUT_NO_DEVICES": "1",
        }
    )
    config = run_dir / "sway-config"
    config.write_text(
        """output HEADLESS-1 resolution 1024x768
default_border none
default_floating_border none
focus_follows_mouse no
bindsym Mod1+F4 kill
""",
        encoding="utf-8",
    )
    log = (run_dir / "sway.log").open("w", encoding="utf-8")
    process = subprocess.Popen(
        ["sway", "--unsupported-gpu", "--config", str(config)],
        env=env,
        stdout=log,
        stderr=subprocess.STDOUT,
        text=True,
    )
    sockets: list[Path] = []
    ipc_sockets: list[Path] = []

    def sway_socket_ready() -> bool:
        sockets[:] = [
            path
            for path in runtime.glob("wayland-*")
            if not path.name.endswith(".lock")
        ]
        ipc_sockets[:] = list(runtime.glob("sway-ipc.*.sock"))
        return (bool(sockets) and bool(ipc_sockets)) or process.poll() is not None

    wait_until("Sway Wayland socket", sway_socket_ready, 10)
    if process.poll() is not None or not sockets or not ipc_sockets:
        log.close()
        shutil.rmtree(runtime, ignore_errors=True)
        raise AcceptanceError(f"Sway exited during startup; see {run_dir / 'sway.log'}")
    env["WAYLAND_DISPLAY"] = sockets[0].name
    env["SWAYSOCK"] = str(ipc_sockets[0])
    process._termeric_log = log  # type: ignore[attr-defined]
    process._termeric_runtime = runtime  # type: ignore[attr-defined]
    return env, process


def required_tools(
    backend: str,
    scenarios: list[str],
    start_weston: bool,
    start_sway: bool,
    start_window_manager: bool = False,
) -> list[str]:
    tools = ["tic"]
    if backend == "x11":
        tools.append("xdotool")
        if "workspace" in scenarios:
            tools.extend(["img2sixel", "timg", "xclip"])
        if set(scenarios) & {"workspace", "domain", "vim", "less", "tmux"}:
            capture_backend = os.environ.get("TERMERIC_GUI_CAPTURE", "auto")
            if capture_backend == "ffmpeg" or (
                shutil.which("import") is None and shutil.which("spectacle") is None
            ):
                tools.append("ffmpeg")
        if "vim" in scenarios:
            tools.append("vim")
        if "less" in scenarios:
            tools.append("less")
        if "tmux" in scenarios:
            tools.append("tmux")
        if set(scenarios) & {"vim", "less", "tmux"}:
            tools.append("script")
        if start_window_manager:
            tools.append("openbox")
    else:
        if "fullscreen" in scenarios or "interaction" in scenarios:
            tools.append("wtype")
        if "interaction" in scenarios:
            tools.extend(["wl-copy", "wl-paste"])
        if start_weston:
            tools.append("weston")
        if start_sway:
            tools.extend(["sway", "swaymsg"])
    return sorted(set(tools))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--backend", choices=("x11", "wayland"), required=True)
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--artifacts", type=Path, default=ROOT / "target" / "gui-acceptance")
    parser.add_argument(
        "--scenario",
        action="append",
        choices=(
            "launch",
            "workspace",
            "interaction",
            "fullscreen",
            "cli",
            "domain",
            "vim",
            "less",
            "tmux",
        ),
        help="scenario to run; repeat the option (default: all X11, launch-only Wayland)",
    )
    parser.add_argument("--update-baselines", action="store_true")
    parser.add_argument(
        "--skip-visual-checks",
        action="store_true",
        help="capture artifacts but run semantic assertions only",
    )
    parser.add_argument("--start-weston", action="store_true")
    parser.add_argument("--start-sway", action="store_true")
    parser.add_argument(
        "--start-window-manager",
        action="store_true",
        help="start Openbox for EWMH fullscreen, activation, and WM_DELETE handling",
    )
    parser.add_argument(
        "--force-surface-loss",
        action="store_true",
        help="inject one renderer SurfaceError::Lost and prove resize recovery",
    )
    parser.add_argument(
        "--force-surface-outdated",
        action="store_true",
        help="inject one renderer SurfaceError::Outdated and prove resize recovery",
    )
    parser.add_argument(
        "--force-device-loss",
        action="store_true",
        help="inject one device-loss event and prove whole-renderer recreation",
    )
    args = parser.parse_args()

    binary = args.binary.resolve()
    if not binary.is_file():
        raise AcceptanceError(f"Termeric binary not found: {binary}; run cargo build first")
    if sys.platform != "linux":
        raise AcceptanceError("native GUI acceptance currently supports Linux only")
    if Image is None or ImageChops is None:
        raise AcceptanceError("Pillow is required for reviewable visual baselines")

    scenarios = args.scenario or [
        "workspace",
        "fullscreen",
        "cli",
        "domain",
        "vim",
        "less",
        "tmux",
    ]
    if args.backend == "x11":
        if "launch" in scenarios:
            raise AcceptanceError("the launch scenario is Wayland-only")
        if "fullscreen" in scenarios and not args.start_window_manager:
            raise AcceptanceError("X11 fullscreen requires --start-window-manager")
        if args.start_weston or args.start_sway:
            raise AcceptanceError("Wayland compositor launchers require --backend wayland")
    else:
        scenarios = args.scenario or ["launch"]
        invalid = sorted(set(scenarios) - {"launch", "interaction", "fullscreen"})
        if invalid:
            raise AcceptanceError(
                "Wayland supports only launch/interaction/fullscreen scenarios, got "
                + ", ".join(invalid)
            )
        if args.start_weston and args.start_sway:
            raise AcceptanceError("choose only one Wayland compositor launcher")
        if args.start_window_manager:
            raise AcceptanceError("--start-window-manager requires --backend x11")
        sway_scenarios = sorted(set(scenarios) & {"interaction", "fullscreen"})
        if sway_scenarios and not args.start_sway:
            raise AcceptanceError(
                "Wayland " + ", ".join(sway_scenarios) + " requires --start-sway"
            )
    missing = [
        tool
        for tool in required_tools(
            args.backend,
            scenarios,
            args.start_weston,
            args.start_sway,
            args.start_window_manager,
        )
        if shutil.which(tool) is None
    ]
    if missing:
        raise AcceptanceError(
            "required GUI acceptance tools are unavailable: " + ", ".join(missing)
        )

    artifacts = args.artifacts.resolve()
    artifacts.mkdir(parents=True, exist_ok=True)
    environment = dict(os.environ)
    terminfo = artifacts / "terminfo"
    terminfo.mkdir(parents=True, exist_ok=True)
    run(
        [
            "tic",
            "-x",
            "-o",
            str(terminfo),
            str(ROOT / "assets" / "terminfo" / "xterm-termeric.terminfo"),
        ],
        env=environment,
    )
    environment["TERMERIC_TERM"] = "auto"
    environment["TERMERIC_TERMINFO_DIR"] = str(terminfo)
    if args.skip_visual_checks:
        environment["TERMERIC_GUI_SKIP_VISUAL"] = "1"
    if args.force_surface_loss:
        environment["TERMERIC_GUI_FORCE_SURFACE_LOSS_ONCE"] = "1"
    if args.force_surface_outdated:
        environment["TERMERIC_GUI_FORCE_SURFACE_OUTDATED_ONCE"] = "1"
    if args.force_device_loss:
        environment["TERMERIC_GUI_FORCE_DEVICE_LOSS_ONCE"] = "1"
    weston: subprocess.Popen[str] | None = None
    sway: subprocess.Popen[str] | None = None
    window_manager: subprocess.Popen[str] | None = None
    results: list[dict[str, str]] = []
    try:
        if args.backend == "wayland" and args.start_weston:
            environment, weston = weston_environment(environment, artifacts)
        if args.backend == "wayland" and args.start_sway:
            environment, sway = sway_environment(environment, artifacts)
        if args.backend == "x11" and args.start_window_manager:
            wm_log = (artifacts / "window-manager.log").open("w", encoding="utf-8")
            wm_config = artifacts / "openbox.xml"
            wm_config.write_text(
                """<?xml version="1.0" encoding="UTF-8"?>
<openbox_config xmlns="http://openbox.org/3.4/rc">
  <focus><focusNew>yes</focusNew><followMouse>no</followMouse></focus>
  <keyboard>
    <keybind key="A-F4"><action name="Close" /></keybind>
  </keyboard>
  <applications>
    <application class="*"><decor>no</decor></application>
  </applications>
</openbox_config>
""",
                encoding="utf-8",
            )
            window_manager = subprocess.Popen(
                ["openbox", "--sm-disable", "--config-file", str(wm_config)],
                env=environment,
                stdout=wm_log,
                stderr=subprocess.STDOUT,
                text=True,
            )
            time.sleep(0.4)
            if window_manager.poll() is not None:
                wm_log.close()
                raise AcceptanceError(
                    f"Openbox exited during startup; see {artifacts / 'window-manager.log'}"
                )
            window_manager._termeric_log = wm_log  # type: ignore[attr-defined]

        runners = {
            "workspace": run_workspace_scenario,
            "fullscreen": run_x11_fullscreen_scenario,
            "cli": run_cli_launch_scenario,
            "domain": run_domain_scenario,
            "vim": run_vim_scenario,
            "less": run_less_scenario,
            "tmux": run_tmux_scenario,
        }
        if args.backend == "wayland":
            for scenario in scenarios:
                try:
                    if scenario == "launch":
                        run_wayland_launch(binary, artifacts / scenario, environment)
                    elif scenario == "interaction":
                        run_wayland_interaction_scenario(
                            binary, artifacts / scenario, environment
                        )
                    else:
                        run_wayland_fullscreen_scenario(
                            binary, artifacts / scenario, environment
                        )
                except Exception as error:
                    results.append(
                        {"scenario": scenario, "status": "failed", "error": str(error)}
                    )
                    raise
                else:
                    results.append({"scenario": scenario, "status": "passed"})
        else:
            for scenario in scenarios:
                scenario_dir = artifacts / scenario
                try:
                    runners[scenario](binary, scenario_dir, environment, args.update_baselines)
                except Exception as error:
                    results.append(
                        {"scenario": scenario, "status": "failed", "error": str(error)}
                    )
                    raise
                else:
                    results.append({"scenario": scenario, "status": "passed"})
    finally:
        if window_manager is not None:
            window_manager.terminate()
            try:
                window_manager.wait(timeout=3)
            except subprocess.TimeoutExpired:
                window_manager.kill()
                window_manager.wait(timeout=3)
            wm_log = getattr(window_manager, "_termeric_log", None)
            if wm_log is not None:
                wm_log.close()
        if weston is not None:
            weston.terminate()
            try:
                weston.wait(timeout=3)
            except subprocess.TimeoutExpired:
                weston.kill()
                weston.wait(timeout=3)
            weston_log = getattr(weston, "_termeric_log", None)
            if weston_log is not None:
                weston_log.close()
            cleanup_private_runtime(weston)
        if sway is not None:
            sway.terminate()
            try:
                sway.wait(timeout=3)
            except subprocess.TimeoutExpired:
                sway.kill()
                sway.wait(timeout=3)
            sway_log = getattr(sway, "_termeric_log", None)
            if sway_log is not None:
                sway_log.close()
            cleanup_private_runtime(sway)
        (artifacts / "results.json").write_text(
            json.dumps(
                result_document(
                    backend=args.backend,
                    binary=binary,
                    scenarios=results,
                    start_weston=args.start_weston,
                    start_sway=args.start_sway,
                    start_window_manager=args.start_window_manager,
                    force_surface_loss=args.force_surface_loss,
                    force_surface_outdated=args.force_surface_outdated,
                    force_device_loss=args.force_device_loss,
                ),
                indent=2,
                sort_keys=True,
            )
            + "\n",
            encoding="utf-8",
        )

    print(f"Termeric {args.backend} GUI acceptance passed: {', '.join(scenarios)}")
    print(f"Artifacts: {artifacts}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (AcceptanceError, subprocess.TimeoutExpired) as error:
        print(f"GUI acceptance failed: {error}", file=sys.stderr)
        raise SystemExit(1)
