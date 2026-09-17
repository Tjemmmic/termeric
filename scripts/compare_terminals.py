#!/usr/bin/env python3
"""Record fail-closed, opt-in cross-terminal Linux/X11 benchmark evidence."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import re
import shutil
import signal
import statistics
import subprocess
import sys
import time
import traceback
import uuid
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
FIXTURE = ROOT / "scripts" / "terminal_benchmark_fixture.py"
SUPPORTED_TERMINALS = ("termeric", "alacritty", "kitty", "wezterm", "ghostty", "konsole")

# A comparison launched from a Wayland desktop commonly inherits both DISPLAY
# (through Xwayland) and WAYLAND_DISPLAY. Merely setting XDG_SESSION_TYPE does
# not force toolkits to use DISPLAY, so a participant could otherwise escape a
# private Xvfb server and open on the operator's real desktop. Remove every
# selector used by our supported adapters before applying one explicit X11
# control below.
DISPLAY_BACKEND_SELECTORS = (
    "WAYLAND_DISPLAY",
    "WAYLAND_SOCKET",
    "EGL_PLATFORM",
    "WINIT_UNIX_BACKEND",
    "GDK_BACKEND",
    "QT_QPA_PLATFORM",
)
SESSION_BUS_SELECTORS = (
    "DBUS_SESSION_BUS_ADDRESS",
    "DBUS_STARTER_ADDRESS",
    "DBUS_STARTER_BUS_TYPE",
)
PRIVATE_SESSION_BUS_MARKER = "TERMERIC_COMPARISON_PRIVATE_SESSION_BUS"
DISABLED_WAYLAND_DISPLAY = ""


@dataclass(frozen=True)
class BenchmarkSettings:
    font_family: str
    font_size: float
    columns: int
    rows: int
    scrollback_lines: int
    window_width: int
    window_height: int


@dataclass(frozen=True)
class LaunchSpec:
    command: list[str]
    environment: dict[str, str]
    controls: dict[str, Any]


def x11_environment(inherited_environment: dict[str, str]) -> dict[str, str]:
    environment = dict(inherited_environment)
    for variable in DISPLAY_BACKEND_SELECTORS:
        environment.pop(variable, None)
    if environment.get(PRIVATE_SESSION_BUS_MARKER) != "1":
        for variable in SESSION_BUS_SELECTORS:
            environment.pop(variable, None)
    environment["WAYLAND_DISPLAY"] = DISABLED_WAYLAND_DISPLAY
    environment["WAYLAND_SOCKET"] = ""
    environment["XDG_SESSION_TYPE"] = "x11"
    environment["EGL_PLATFORM"] = "x11"
    return environment


def enter_private_session_bus(parser: argparse.ArgumentParser) -> None:
    if os.environ.get(PRIVATE_SESSION_BUS_MARKER) == "1":
        if not os.environ.get("DBUS_SESSION_BUS_ADDRESS"):
            parser.error("private comparison session bus has no address")
        return
    launcher = shutil.which("dbus-run-session")
    if launcher is None:
        parser.error("cross-terminal benchmarking requires dbus-run-session for isolation")
    environment = x11_environment(dict(os.environ))
    environment[PRIVATE_SESSION_BUS_MARKER] = "1"
    os.execvpe(
        launcher,
        [launcher, "--", sys.executable, str(Path(__file__).resolve()), *sys.argv[1:]],
        environment,
    )


def quote_toml(value: str) -> str:
    return json.dumps(value, ensure_ascii=False)


def quote_lua(value: str) -> str:
    return json.dumps(value, ensure_ascii=False)


def write_text(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")


def executable_for(name: str, termeric_binary: Path) -> str | None:
    if name == "termeric":
        return str(termeric_binary.resolve()) if termeric_binary.is_file() else None
    return shutil.which(name)


def file_sha256(path: str) -> str:
    digest = hashlib.sha256()
    with Path(path).resolve().open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def command_capture(command: list[str], environment: dict[str, str]) -> dict[str, Any]:
    executable = shutil.which(command[0])
    if executable is None:
        return {"status": "unavailable", "command": command}
    try:
        completed = subprocess.run(
            [executable, *command[1:]],
            cwd=ROOT,
            env=environment,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            check=False,
            timeout=10,
        )
    except subprocess.TimeoutExpired as error:
        return {
            "status": "failed",
            "command": command,
            "error": f"timed out after {error.timeout} seconds",
        }
    return {
        "status": "complete" if completed.returncode == 0 else "failed",
        "command": command,
        "exit_status": completed.returncode,
        "output": completed.stdout.strip(),
    }


def cpu_model() -> str:
    try:
        for line in Path("/proc/cpuinfo").read_text(encoding="utf-8").splitlines():
            if line.startswith("model name"):
                return line.split(":", 1)[1].strip()
    except OSError:
        pass
    return "unknown"


def version_command(name: str, executable: str) -> list[str]:
    if name == "ghostty":
        return [executable, "+version"]
    return [executable, "--version"]


def fixture_command(run_dir: Path, title: str, payload_bytes: int, timeout: float) -> list[str]:
    return [
        sys.executable,
        str(FIXTURE),
        "--ready",
        str(run_dir / "ready.json"),
        "--gate",
        str(run_dir / "gate"),
        "--result",
        str(run_dir / "fixture.json"),
        "--title",
        title,
        "--payload-bytes",
        str(payload_bytes),
        "--timeout-seconds",
        str(timeout),
    ]


def build_launch_spec(
    name: str,
    executable: str,
    run_dir: Path,
    title: str,
    fixture: list[str],
    settings: BenchmarkSettings,
    inherited_environment: dict[str, str],
) -> LaunchSpec:
    environment = x11_environment(inherited_environment)
    environment.update(
        {
            "XDG_CONFIG_HOME": str(run_dir / "xdg-config"),
            "XDG_CACHE_HOME": str(run_dir / "xdg-cache"),
            "XDG_DATA_HOME": str(run_dir / "xdg-data"),
            "XDG_STATE_HOME": str(run_dir / "xdg-state"),
            "NO_AT_BRIDGE": "1",
        }
    )
    controls: dict[str, Any] = {
        "fresh_process": True,
        "isolated_configuration": True,
        "font_family_requested": settings.font_family,
        "font_size_requested": settings.font_size,
        "columns_requested": settings.columns,
        "rows_requested": settings.rows,
        "scrollback_lines_requested": settings.scrollback_lines,
        "ligatures_disabled": True,
        "transparency_disabled": True,
        "display_backend_requested": "x11",
        "desktop_session_bus": "private",
    }
    font = settings.font_family
    size = settings.font_size
    cols = settings.columns
    rows = settings.rows
    history = settings.scrollback_lines

    if name == "termeric":
        controls["display_backend_control"] = "empty Wayland selectors; DISPLAY selected"
        controls["font_size_unit"] = "Termeric logical pixels before display scaling"
        config = run_dir / "xdg-config" / "termeric" / "config.toml"
        write_text(
            config,
            f"""[font]
family = {quote_toml(font)}
size = {size}
line_height = 1.0
ligatures = false

[scrollback]
lines = {history}
max_memory_mib = 128

[window]
width = {settings.window_width}
height = {settings.window_height}
padding = 0
decorations = false
opacity = 1.0

[cursor]
blink = false

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
""",
        )
        command = [
            executable,
            "--new-window",
            "--geometry",
            f"{settings.window_width}x{settings.window_height}",
            "--class",
            title,
            "--",
            *fixture,
        ]
        controls["initial_size_control"] = "Termeric pixel geometry; actual PTY grid is verified"
    elif name == "alacritty":
        controls["display_backend_control"] = "empty Wayland selectors; DISPLAY selected"
        controls["font_size_unit"] = "points"
        config = run_dir / "alacritty.toml"
        write_text(
            config,
            f"""[window]
dimensions = {{ columns = {cols}, lines = {rows} }}
decorations = "None"
dynamic_title = true
opacity = 1.0
padding = {{ x = 0, y = 0 }}

[scrolling]
history = {history}

[font]
size = {size}
builtin_box_drawing = true

[font.normal]
family = {quote_toml(font)}

[cursor]
style = {{ shape = "Block", blinking = "Never" }}
""",
        )
        command = [executable, "--config-file", str(config), "--class", title, "-e", *fixture]
        controls["initial_size_control"] = "configured terminal grid cells"
    elif name == "kitty":
        controls["display_backend_control"] = "linux_display_server x11"
        controls["font_size_unit"] = "points"
        config = run_dir / "kitty.conf"
        write_text(
            config,
            f"""font_family {font}
font_size {size}
disable_ligatures always
scrollback_lines {history}
initial_window_width {cols}c
initial_window_height {rows}c
remember_window_size no
window_padding_width 0
hide_window_decorations yes
tab_bar_style hidden
background_opacity 1.0
cursor_blink_interval 0
confirm_os_window_close 0
linux_display_server x11
""",
        )
        command = [
            executable,
            "--config",
            str(config),
            "--class",
            title,
            "--session",
            "none",
            *fixture,
        ]
        controls["initial_size_control"] = "configured terminal grid cells"
    elif name == "wezterm":
        controls["display_backend_control"] = "enable_wayland = false"
        controls["font_size_unit"] = "points"
        config = run_dir / "wezterm.lua"
        write_text(
            config,
            f"""local wezterm = require 'wezterm'
return {{
  font = wezterm.font({quote_lua(font)}),
  font_size = {size},
  harfbuzz_features = {{ 'calt=0', 'liga=0', 'dlig=0' }},
  initial_cols = {cols},
  initial_rows = {rows},
  scrollback_lines = {history},
  enable_tab_bar = false,
  window_decorations = 'NONE',
  window_background_opacity = 1.0,
  text_background_opacity = 1.0,
  cursor_blink_rate = 0,
  window_padding = {{ left = 0, right = 0, top = 0, bottom = 0 }},
  enable_wayland = false,
  check_for_updates = false,
}}
""",
        )
        command = [
            executable,
            "--config-file",
            str(config),
            "start",
            "--always-new-process",
            "--no-auto-connect",
            "--class",
            title,
            "--",
            *fixture,
        ]
        controls["initial_size_control"] = "configured terminal grid cells"
    elif name == "ghostty":
        environment["GDK_BACKEND"] = "x11"
        controls["display_backend_control"] = "GDK_BACKEND=x11"
        controls["font_size_unit"] = "points"
        config = run_dir / "ghostty.conf"
        # Ghostty scrollback is byte-bounded rather than line-bounded. The
        # deliberately conservative conversion is recorded and comparability
        # remains fail closed on this semantic difference.
        scrollback_bytes = history * 1024
        write_text(
            config,
            f"""font-family = {font}
font-size = {size}
font-feature = -calt,-liga,-dlig
window-width = {cols}
window-height = {rows}
window-padding-x = 0
window-padding-y = 0
window-decoration = false
window-theme = ghostty
background-opacity = 1.0
cursor-style-blink = false
scrollback-limit = {scrollback_bytes}
gtk-single-instance = false
""",
        )
        command = [executable, f"--config-file={config}", "-e", *fixture]
        controls.update(
            {
                "initial_size_control": "configured terminal grid cells",
                "scrollback_control": f"{scrollback_bytes} byte cap (not an exact line cap)",
                "comparison_note": "Ghostty exposes a byte scrollback cap; the contract requires the whole workload to fit",
            }
        )
    elif name == "konsole":
        environment["QT_QPA_PLATFORM"] = "xcb"
        controls["display_backend_control"] = "QT_QPA_PLATFORM=xcb"
        controls["font_size_unit"] = "Qt font point size"
        qfont = f"{font},{size},-1,5,50,0,0,0,0,0"
        command = [
            executable,
            "--separate",
            "--builtin-profile",
            "--hide-menubar",
            "--hide-tabbar",
            "--notransparency",
            "-p",
            f"Font={qfont}",
            "-p",
            f"HistorySize={history}",
            "-p",
            "HistoryMode=1",
            "-e",
            *fixture,
        ]
        controls.update(
            {
                "initial_size_control": "X11 window resized to the common pixel size; actual PTY grid is verified",
                "ligatures_disabled": False,
                "comparison_note": "Konsole has no documented launch flag for disabling shaping ligatures; the fixture avoids common programming-ligature sequences",
            }
        )
    else:
        raise ValueError(f"unsupported terminal adapter: {name}")
    return LaunchSpec(command, environment, controls)


def read_json(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def wait_for_file(path: Path, process: subprocess.Popen[bytes], timeout: float) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    last_error: Exception | None = None
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"launcher exited with status {process.returncode} before {path.name}")
        if path.is_file():
            try:
                return read_json(path)
            except (json.JSONDecodeError, OSError) as error:
                last_error = error
        time.sleep(0.01)
    detail = f": {last_error}" if last_error else ""
    raise TimeoutError(f"timed out waiting for {path}{detail}")


def find_window(title: str, environment: dict[str, str], timeout: float) -> tuple[str, int, float]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        found = subprocess.run(
            ["xdotool", "search", "--onlyvisible", "--name", re.escape(title)],
            env=environment,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            check=False,
        )
        if found.returncode == 0 and found.stdout.strip():
            window = found.stdout.splitlines()[-1]
            pid = subprocess.run(
                ["xdotool", "getwindowpid", window],
                env=environment,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
                check=False,
            )
            return window, int(pid.stdout.strip() or 0), time.monotonic()
        time.sleep(0.01)
    raise TimeoutError(f"timed out finding visible X11 window titled {title!r}")


def window_geometry(window: str, environment: dict[str, str]) -> dict[str, Any]:
    # Geometry is required for resource comparisons, not for valid PTY/startup
    # timings. Retain a failed observation instead of losing the whole sample.
    try:
        result = subprocess.run(
            ["xdotool", "getwindowgeometry", "--shell", window],
            env=environment,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=True,
            timeout=5,
        )
    except (OSError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        return {"errors": [f"window geometry query failed: {type(error).__name__}"]}
    values: dict[str, Any] = {}
    dimensions_seen = set()
    errors = []
    for line in result.stdout.splitlines():
        key, separator, value = line.partition("=")
        key = key.lower()
        if not separator or key not in ("x", "y", "width", "height"):
            continue
        pattern = r"[0-9]+" if key in ("width", "height") else r"-?[0-9]+"
        number = None
        if re.fullmatch(pattern, value):
            try:
                number = int(value)
            except ValueError:
                # Includes Python's limit on oversized integer text.
                pass
        if key in ("width", "height"):
            if key in dimensions_seen:
                errors.append(f"duplicate native-window {key}")
            dimensions_seen.add(key)
            if number is None or number <= 0:
                errors.append(f"invalid native-window {key}")
                continue
        elif number is None:
            # Optional coordinates do not establish resource comparability.
            continue
        values[key] = number
    for key in ("width", "height"):
        if key not in dimensions_seen:
            errors.append(f"missing native-window {key}")
    if errors:
        values["errors"] = errors
    return values


def stat_fields(pid: int) -> tuple[int, int, int, int] | None:
    try:
        text = (Path("/proc") / str(pid) / "stat").read_text(encoding="utf-8")
        fields = text[text.rfind(")") + 2 :].split()
        return int(fields[2]), int(fields[3]), int(fields[11]) + int(fields[12]), int(fields[19])
    except (OSError, IndexError, ValueError):
        return None


def direct_children(pid: int) -> set[int]:
    children: set[int] = set()
    task_dir = Path("/proc") / str(pid) / "task"
    tasks = list(task_dir.iterdir())
    for task in tasks:
        try:
            children.update(int(value) for value in (task / "children").read_text().split())
        except FileNotFoundError:
            # A vanished thread is normal; a still-present thread with an
            # unreadable children file is not evidence of an empty subtree.
            if task.exists():
                raise
    return children


def descendants(roots: set[int]) -> set[int]:
    observed = set(roots)
    pending = list(roots)
    while pending:
        for child in direct_children(pending.pop()):
            if child not in observed:
                observed.add(child)
                pending.append(child)
    return observed


def benchmark_pids(launcher_pid: int, window_pid: int, fixture_pid: int) -> set[int]:
    fixture_tree = descendants({fixture_pid})
    roots = {launcher_pid}
    if window_pid > 0:
        roots.add(window_pid)
    owned = descendants(roots)
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        parsed = stat_fields(int(entry.name))
        if parsed is not None and parsed[1] == launcher_pid:
            owned.add(int(entry.name))
    return owned - fixture_tree


def unavailable_resources(reason: str) -> dict[str, Any]:
    return {
        "status": "unavailable", "pids": [], "processes": [], "identities": [],
        "rss_kib": None, "threads": None, "fds": None, "cpu_ticks": None,
        "errors": [reason],
    }


def parse_resource_status(text: str) -> tuple[int, int]:
    values: dict[str, int] = {}
    for line in text.splitlines():
        label, separator, value = line.partition(":")
        if not separator or label not in {"VmRSS", "Threads"}:
            continue
        pattern = r"([0-9]+)\s+kB" if label == "VmRSS" else r"([0-9]+)"
        match = re.fullmatch(pattern, value.strip())
        if label in values or match is None:
            raise ValueError(f"invalid or duplicate {label}")
        values[label] = int(match[1])
    if set(values) != {"VmRSS", "Threads"} or values["Threads"] < 1:
        raise ValueError("missing RSS or positive thread count")
    return values["VmRSS"], values["Threads"]


def process_tree_sample(pids: set[int]) -> dict[str, Any]:
    if not pids:
        return unavailable_resources("no terminal processes were observed")
    processes = []
    identities = []
    errors = []
    for pid in sorted(pids):
        proc = Path("/proc") / str(pid)
        try:
            before = stat_fields(pid)
            if before is None or before[2] < 0 or before[3] < 0:
                raise ValueError("process stat unavailable")
            # Cleanup must not depend on successful RSS or descriptor reads.
            # Its existing PID/start-time revalidation still prevents signaling
            # a replacement process if this identity subsequently disappears.
            identities.append({"pid": pid, "start_time_ticks": before[3]})
            rss, threads = parse_resource_status((proc / "status").read_text(encoding="utf-8"))
            fds = len(list((proc / "fd").iterdir()))
            after = stat_fields(pid)
            if after is None or before[3] != after[3] or after[2] < before[2]:
                raise ValueError("process identity or CPU counter changed during observation")
        except (OSError, ValueError) as error:
            errors.append(f"pid {pid}: resource observation unavailable ({type(error).__name__})")
            continue
        processes.append({
            "pid": pid, "start_time_ticks": after[3], "cpu_ticks": after[2],
            "rss_kib": rss, "threads": threads, "fds": fds,
        })
    result = {
        "status": "unavailable" if errors else "complete",
        "pids": [process["pid"] for process in processes],
        "requested_pids": sorted(pids), "processes": processes,
        "identities": identities, "errors": errors,
    }
    for metric in ("rss_kib", "threads", "fds", "cpu_ticks"):
        # Retain per-process diagnostics, but never rank a partial tree total.
        result[metric] = None if errors else sum(process[metric] for process in processes)
    return result


def terminal_resources(launcher_pid: int, window_pid: int, fixture_pid: int) -> dict[str, Any]:
    try:
        return process_tree_sample(benchmark_pids(launcher_pid, window_pid, fixture_pid))
    except (OSError, ValueError) as error:
        return unavailable_resources(f"process-tree discovery unavailable ({type(error).__name__})")


def idle_cpu_measurement(
    before: dict[str, Any], after: dict[str, Any], elapsed: float, clock_ticks: int,
) -> tuple[float | None, list[str]]:
    if before["status"] != "complete" or after["status"] != "complete":
        return None, ["idle interval has an incomplete process observation"]
    old = {process["pid"]: process for process in before["processes"]}
    new = {process["pid"]: process for process in after["processes"]}
    if not old or old.keys() != new.keys():
        return None, ["terminal process membership changed during idle interval"]
    delta = 0
    for pid, process in new.items():
        previous = old[pid]
        if process["start_time_ticks"] != previous["start_time_ticks"]:
            return None, ["terminal process identity changed during idle interval"]
        change = process["cpu_ticks"] - previous["cpu_ticks"]
        if change < 0:
            return None, ["terminal CPU counter decreased during idle interval"]
        delta += change
    if not math.isfinite(elapsed) or elapsed <= 0 or clock_ticks <= 0:
        return None, ["invalid idle sampling duration or clock frequency"]
    return delta / clock_ticks / elapsed * 100.0, []


def metric_available(resources: dict[str, Any], name: str) -> bool:
    value = resources.get(name)
    statuses = resources.get("measurement_status")
    return (
        isinstance(statuses, dict) and statuses.get(name) == "complete"
        and type(value) in (int, float) and math.isfinite(value) and value >= 0
    )


def terminate_run(
    process: subprocess.Popen[bytes],
    window: str | None,
    environment: dict[str, str],
    tracked: dict[int, int],
) -> None:
    # Recover every process that still belongs to the private benchmark
    # session even when launch failed before the normal ownership snapshot.
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        pid = int(entry.name)
        parsed = stat_fields(pid)
        if parsed is not None and parsed[1] == process.pid:
            tracked.setdefault(pid, parsed[3])
    if window:
        subprocess.run(
            ["xdotool", "windowclose", window],
            env=environment,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )
    try:
        process.wait(timeout=2)
    except subprocess.TimeoutExpired:
        pass
    for requested_signal in (signal.SIGTERM, signal.SIGKILL):
        for pid, start_time in tracked.items():
            parsed = stat_fields(pid)
            if parsed is not None and parsed[3] == start_time:
                try:
                    os.kill(pid, requested_signal)
                except (ProcessLookupError, PermissionError):
                    pass
        if process.poll() is None:
            try:
                process.send_signal(requested_signal)
            except ProcessLookupError:
                pass
        try:
            process.wait(timeout=2)
            break
        except subprocess.TimeoutExpired:
            continue


def summarize(samples: list[float]) -> dict[str, float | None]:
    if not samples:
        return {"minimum": None, "median": None, "p95": None, "maximum": None, "mean": None}
    ordered = sorted(samples)
    index = min(len(ordered) - 1, max(0, math.ceil(0.95 * len(ordered)) - 1))
    return {
        "minimum": ordered[0],
        "median": statistics.median(ordered),
        "p95": ordered[index],
        "maximum": ordered[-1],
        "mean": statistics.fmean(ordered),
    }


def comparison_contract(terminals: list[dict[str, Any]], settings: BenchmarkSettings) -> dict[str, Any]:
    common_reasons = []
    completed = [terminal for terminal in terminals if terminal.get("status") == "complete"]
    if len(completed) != len(terminals):
        common_reasons.append("one or more explicitly selected terminals did not complete")
    if len(completed) < 2:
        common_reasons.append("at least two completed terminals are required for a comparison")
    observed_grids = set()
    initial_grids = set()
    observed_pixels = set()
    pixel_reasons = []
    grid_reasons = []
    retention_reasons = []
    for terminal in completed:
        for sample in terminal["samples"]:
            grid = (sample["fixture"]["columns"], sample["fixture"]["rows"])
            observed_grids.add(grid)
            ready_grid = (
                sample["fixture_ready"]["columns"],
                sample["fixture_ready"]["rows"],
            )
            initial_grids.add(ready_grid)
            geometry = sample.get("window_geometry", {})
            if (
                not isinstance(geometry, dict)
                or geometry.get("errors", []) != []
                or any(
                    type(geometry.get(key)) is not int or geometry[key] <= 0
                    for key in ("width", "height")
                )
            ):
                pixel_reasons.append(
                    f"{terminal['terminal']}: missing, invalid, or failed native-window geometry observation"
                )
            else:
                observed_pixels.add((geometry["width"], geometry["height"]))
            if grid != (settings.columns, settings.rows):
                grid_reasons.append(
                    f"{terminal['terminal']}: observed grid {grid[0]}x{grid[1]} instead of "
                    f"{settings.columns}x{settings.rows}"
                )
            if sample["fixture"].get("output_lines", settings.scrollback_lines + 1) > settings.scrollback_lines:
                retention_reasons.append(
                    f"{terminal['terminal']}: fixture output exceeded the requested scrollback line capacity"
                )
    if len(observed_grids) > 1:
        grid_reasons.append("observed PTY grids differ across samples")
    startup_reasons = list(common_reasons)
    if len(initial_grids) > 1:
        startup_reasons.append("initial PTY grids differ across samples")
    if initial_grids and initial_grids != {(settings.columns, settings.rows)}:
        startup_reasons.append("one or more initial PTY grids differ from the requested grid")
    pty_reasons = common_reasons + grid_reasons + retention_reasons
    resource_reasons = pty_reasons + pixel_reasons
    if len(observed_pixels) > 1:
        resource_reasons.append("observed native-window pixel sizes differ across samples")
    resource_metric_reasons = {}
    for metric in ("rss_kib", "idle_cpu_percent_one_core"):
        reasons = list(resource_reasons)
        for terminal in completed:
            if not terminal["samples"] or any(
                not metric_available(sample.get("resources", {}), metric)
                for sample in terminal["samples"]
            ):
                reasons.append(f"{terminal['terminal']}: incomplete {metric} observation")
        resource_metric_reasons[metric] = reasons
    metric_contracts = {
        "startup_to_visible_ms": {
            "comparable": not startup_reasons,
            "reasons": sorted(set(startup_reasons)),
        },
        "pty_response_ms": {
            "comparable": not pty_reasons,
            "reasons": sorted(set(pty_reasons)),
        },
        "rss_kib": {
            "comparable": not resource_metric_reasons["rss_kib"],
            "reasons": sorted(set(resource_metric_reasons["rss_kib"])),
        },
        "idle_cpu_percent_one_core": {
            "comparable": not resource_metric_reasons["idle_cpu_percent_one_core"],
            "reasons": sorted(set(resource_metric_reasons["idle_cpu_percent_one_core"])),
        },
    }
    reasons = sorted(
        {
            reason
            for contract in metric_contracts.values()
            for reason in contract["reasons"]
        }
    )
    return {
        "comparable": all(contract["comparable"] for contract in metric_contracts.values()),
        "reasons": reasons,
        "metrics": metric_contracts,
        "ranking_permitted": {
            name: contract["comparable"] for name, contract in metric_contracts.items()
        },
        "initial_grids": [list(value) for value in sorted(initial_grids)],
        "observed_grids": [list(value) for value in sorted(observed_grids)],
        "observed_window_pixels": [list(value) for value in sorted(observed_pixels, key=str)],
    }


def run_sample(
    name: str,
    executable: str,
    run_dir: Path,
    settings: BenchmarkSettings,
    payload_bytes: int,
    timeout: float,
    idle_seconds: float,
    inherited_environment: dict[str, str],
) -> tuple[dict[str, Any], dict[str, Any]]:
    run_dir.mkdir(parents=True, exist_ok=False)
    title = f"termeric-benchmark-{uuid.uuid4().hex}"
    fixture = fixture_command(run_dir, title, payload_bytes, timeout)
    spec = build_launch_spec(
        name, executable, run_dir, title, fixture, settings, inherited_environment
    )
    stdout = (run_dir / "launcher.stdout").open("wb")
    stderr = (run_dir / "launcher.stderr").open("wb")
    started = time.monotonic()
    process = subprocess.Popen(
        spec.command,
        cwd=ROOT,
        env=spec.environment,
        stdout=stdout,
        stderr=stderr,
        start_new_session=True,
    )
    window: str | None = None
    tracked: dict[int, int] = {}
    try:
        ready = wait_for_file(run_dir / "ready.json", process, timeout)
        window, window_pid, visible_at = find_window(title, spec.environment, timeout)
        if name in {"termeric", "konsole"}:
            subprocess.run(
                [
                    "xdotool",
                    "windowsize",
                    "--sync",
                    window,
                    str(settings.window_width),
                    str(settings.window_height),
                ],
                env=spec.environment,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.PIPE,
                check=True,
            )
            time.sleep(0.1)
        (run_dir / "gate").touch()
        fixture_result = wait_for_file(run_dir / "fixture.json", process, timeout)
        before = terminal_resources(process.pid, window_pid, int(ready["pid"]))
        tracked = {
            item["pid"]: item["start_time_ticks"] for item in before["identities"]
        }
        fixture_stat = stat_fields(int(ready["pid"]))
        if fixture_stat is not None:
            tracked[int(ready["pid"])] = fixture_stat[3]
        idle_started = time.monotonic()
        time.sleep(idle_seconds)
        elapsed = time.monotonic() - idle_started
        after = terminal_resources(process.pid, window_pid, int(ready["pid"]))
        tracked.update({item["pid"]: item["start_time_ticks"] for item in after["identities"]})
        clock_ticks = os.sysconf("SC_CLK_TCK")
        idle_cpu, cpu_errors = idle_cpu_measurement(before, after, elapsed, clock_ticks)
        sample = {
            "startup_to_visible_ms": (visible_at - started) * 1000.0,
            "fixture": fixture_result,
            "fixture_ready": ready,
            "window_id": window,
            "window_pid": window_pid,
            "window_geometry": window_geometry(window, spec.environment),
            "resources": {
                "rss_kib": after["rss_kib"],
                "threads": after["threads"],
                "fds": after["fds"],
                "idle_cpu_percent_one_core": idle_cpu,
                "accounted_pids": after["pids"],
                "fixture_pid_excluded": int(ready["pid"]),
                "measurement_status": {
                    "rss_kib": after["status"],
                    "idle_cpu_percent_one_core": "complete" if idle_cpu is not None else "unavailable",
                },
                "unavailable_reasons": {"rss_kib": after["errors"], "idle_cpu_percent_one_core": cpu_errors},
                "observations": {"before": before, "after": after},
            },
        }
        return sample, spec.controls
    finally:
        terminate_run(process, window, spec.environment, tracked)
        stdout.close()
        stderr.close()


def terminal_summary(samples: list[dict[str, Any]]) -> dict[str, Any]:
    fields = {
        "startup_to_visible_ms": [float(sample["startup_to_visible_ms"]) for sample in samples],
        "pty_response_ms": [float(sample["fixture"]["pty_response_ms"]) for sample in samples],
    }
    result = {name: {"samples": values, "statistics": summarize(values)} for name, values in fields.items()}
    for name in ("rss_kib", "idle_cpu_percent_one_core"):
        values = [
            float(sample["resources"][name]) if metric_available(sample["resources"], name) else None
            for sample in samples
        ]
        complete = bool(values) and all(value is not None for value in values)
        result[name] = {
            "samples": values, "statistics": summarize(values if complete else []),
            "status": "complete" if complete else "unavailable",
        }
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--terminal",
        action="append",
        choices=SUPPORTED_TERMINALS,
        required=True,
        help="terminal to measure; repeat explicitly for every participant",
    )
    parser.add_argument("--termeric-binary", type=Path, default=ROOT / "target/release/termeric")
    parser.add_argument("--output", type=Path, default=ROOT / "target/terminal-comparison/results.json")
    parser.add_argument("--samples", type=int, default=5)
    parser.add_argument("--warmups", type=int, default=1)
    parser.add_argument("--payload-bytes", type=int, default=1024 * 1024)
    parser.add_argument("--idle-seconds", type=float, default=1.0)
    parser.add_argument("--timeout-seconds", type=float, default=20.0)
    parser.add_argument("--font-family", default="DejaVu Sans Mono")
    parser.add_argument("--font-size", type=float, default=12.0)
    parser.add_argument("--columns", type=int, default=100)
    parser.add_argument("--rows", type=int, default=30)
    parser.add_argument("--scrollback-lines", type=int, default=10_000)
    parser.add_argument("--window-width", type=int, default=1000)
    parser.add_argument("--window-height", type=int, default=700)
    args = parser.parse_args()
    if platform.system() != "Linux" or not Path("/proc/self/status").is_file():
        parser.error("cross-terminal benchmarking currently requires Linux /proc")
    if not os.environ.get("DISPLAY"):
        parser.error("cross-terminal benchmarking requires an X11 DISPLAY (use xvfb-run for isolation)")
    if not shutil.which("xdotool"):
        parser.error("cross-terminal benchmarking requires xdotool")
    if args.samples < 1 or args.warmups < 0:
        parser.error("samples must be positive and warmups must be nonnegative")
    if min(args.payload_bytes, args.columns, args.rows, args.scrollback_lines) < 1:
        parser.error("payload, grid, and scrollback values must be positive")
    if min(args.idle_seconds, args.timeout_seconds, args.font_size) <= 0:
        parser.error("idle duration, timeout, and font size must be positive")
    enter_private_session_bus(parser)
    selected = list(dict.fromkeys(args.terminal))
    output = args.output.resolve()
    artifact_root = output.parent
    artifact_root.mkdir(parents=True, exist_ok=True)
    run_id = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ") + "-" + uuid.uuid4().hex[:8]
    run_root = artifact_root / "runs" / run_id
    settings = BenchmarkSettings(
        args.font_family,
        args.font_size,
        args.columns,
        args.rows,
        args.scrollback_lines,
        args.window_width,
        args.window_height,
    )
    environment = x11_environment(dict(os.environ))
    records: dict[str, dict[str, Any]] = {}
    for name in selected:
        executable = executable_for(name, args.termeric_binary)
        record: dict[str, Any] = {"terminal": name, "executable": executable}
        if executable is None:
            record.update({"status": "unavailable", "error": "selected executable was not found"})
            records[name] = record
            continue
        record["executable_sha256"] = file_sha256(executable)
        try:
            version = subprocess.run(
                version_command(name, executable),
                env=environment,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                check=False,
                timeout=args.timeout_seconds,
            )
            record["version"] = version.stdout.strip()
            record["version_exit_status"] = version.returncode
        except subprocess.TimeoutExpired as error:
            record["version"] = ""
            record["version_exit_status"] = None
            record["version_error"] = f"timed out after {error.timeout} seconds"
        if record["version_exit_status"] != 0 or not record["version"]:
            record.update(
                {
                    "status": "failed",
                    "error": "version probe failed; terminal was not launched",
                }
            )
        else:
            record.update(
                {
                    "status": "ready",
                    "controls": {},
                    "warmups_excluded": args.warmups,
                    "samples": [],
                }
            )
        records[name] = record

    round_orders = []
    total_rounds = args.warmups + args.samples
    for index in range(total_rounds):
        kind = "warmup" if index < args.warmups else "sample"
        offset = index % len(selected)
        order = selected[offset:] + selected[:offset]
        round_orders.append({"round": index + 1, "kind": kind, "terminals": order})
        for name in order:
            record = records[name]
            if record["status"] != "ready":
                continue
            executable = str(record["executable"])
            run_dir = run_root / name / f"{index + 1:02d}-{kind}"
            try:
                run_result, controls = run_sample(
                    name,
                    executable,
                    run_dir,
                    settings,
                    args.payload_bytes,
                    args.timeout_seconds,
                    args.idle_seconds,
                    environment,
                )
                if kind == "sample":
                    record["samples"].append(run_result)
                record["controls"] = controls
            except Exception as error:
                failure = {
                    "error_type": type(error).__name__,
                    "error": str(error),
                    "traceback": traceback.format_exc(),
                }
                write_text(
                    run_dir / "failure.json",
                    json.dumps(failure, indent=2, sort_keys=True) + "\n",
                )
                record.update({"status": "failed", **failure})

    terminals = [records[name] for name in selected]
    for record in terminals:
        if record["status"] != "ready":
            continue
        if len(record["samples"]) != args.samples:
            record.update(
                {
                    "status": "failed",
                    "error": f"captured {len(record['samples'])} of {args.samples} required samples",
                }
            )
        else:
            record["status"] = "complete"
            record["metrics"] = terminal_summary(record["samples"])

    contract = comparison_contract(terminals, settings)
    report = {
        "schema": 2,
        "run_id": run_id,
        "run_artifacts": str(run_root),
        "recorded_at_utc": datetime.now(timezone.utc).isoformat(),
        "method": "fresh-process X11 end-to-end comparison; no internal Termeric metrics",
        "environment": {
            "platform": platform.platform(),
            "kernel": platform.release(),
            "architecture": platform.machine(),
            "cpu_model": cpu_model(),
            "logical_cpus": os.cpu_count(),
            "display": environment.get("DISPLAY"),
            "wayland_display": environment.get("WAYLAND_DISPLAY"),
            "x11_environment_enforced": True,
            "private_session_bus_enforced": (
                environment.get(PRIVATE_SESSION_BUS_MARKER) == "1"
                and bool(environment.get("DBUS_SESSION_BUS_ADDRESS"))
            ),
            "desktop": environment.get("XDG_CURRENT_DESKTOP"),
            "python": sys.version.split()[0],
            "x11_outputs": command_capture(["xrandr", "--current"], environment),
            "opengl": command_capture(["glxinfo", "-B"], environment),
        },
        "settings": {
            **settings.__dict__,
            "payload_bytes": args.payload_bytes,
            "idle_seconds": args.idle_seconds,
            "samples": args.samples,
            "warmups": args.warmups,
        },
        "source": {
            "git_commit": command_capture(["git", "rev-parse", "HEAD"], environment),
            "git_status": command_capture(["git", "status", "--porcelain"], environment),
        },
        "metric_boundaries": {
            "startup_to_visible_ms": "process launch through a visible X11 window containing the unique OSC 2 title marker; includes child startup and title parsing",
            "pty_response_ms": "child begins exact byte stream through receipt of CSI 6 n response; excludes proof of compositor presentation",
            "rss_kib": "sum of completely observed terminal process-tree RSS after workload, excluding the common fixture; incomplete observations are unavailable, never zero or partial totals",
            "idle_cpu_percent_one_core": "CPU delta of the same observed terminal PID/start-time identities across post-workload idle, excluding the common fixture; incomplete observations or process churn are unavailable",
        },
        "comparison_contract": contract,
        "round_orders": round_orders,
        "terminals": terminals,
    }
    output.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"Wrote cross-terminal evidence: {output}")
    if contract["comparable"]:
        print("PASS: selected results satisfy the exact comparison contract")
    else:
        for metric, status in contract["metrics"].items():
            if status["comparable"]:
                print(f"COMPARABLE: {metric}")
            for reason in status["reasons"]:
                print(f"NOT COMPARABLE: {metric}: {reason}")
    return 0 if all(record["status"] == "complete" for record in terminals) else 1


if __name__ == "__main__":
    raise SystemExit(main())
