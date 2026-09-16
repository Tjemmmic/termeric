"""Native top/left tab geometry and keyboard routing; no external clients."""
from __future__ import annotations

import json
import math
from pathlib import Path

from scripts.gui_wayland_pointer import VirtualPointer


def configuration_for_position(config: str, position: str) -> str:
    if position not in {"top", "left"}:
        raise ValueError("invalid tab placement")
    lines = ['tab_bar_position = "top"', 'tab_bar_position = "left"']
    matches = [line for line in lines if config.splitlines().count(line)]
    if len(matches) != 1 or config.splitlines().count(matches[0]) != 1:
        raise ValueError("fixture must have exactly one explicit tab placement")
    return "".join(
        line.replace(matches[0], f'tab_bar_position = "{position}"', 1)
        if line.rstrip("\r\n") == matches[0] else line
        for line in config.splitlines(keepends=True)
    )


def native_chord(app, backend, modifiers, key):
    if backend == "wayland":
        app.wayland_chord(modifiers, key)
    elif backend == "x11":
        app.key("+".join((*modifiers, key)))
    else:
        raise ValueError("unsupported tab-placement backend")


def native_text(app, backend, text):
    if backend == "wayland":
        app.wayland_type(text)
    elif backend == "x11":
        app.type(text)
    else:
        raise ValueError("unsupported tab-placement backend")


def rail_reduced_width(current: tuple[int, ...], top: tuple[int, ...]) -> bool:
    return (len(current) == len(top) == 4 and min(*current, *top) > 0
            and current[1] < top[1] and current[2] < top[2]
            and current[0] == top[0] and current[3] == top[3])


def compact_row_height(geometry: tuple[int, ...]) -> int:
    # This fixture uses a 960x128 scale-1 surface and the default 220px rail.
    # Derive row height from the actual PTY cell metrics, not the host font name.
    rows, cols, width, height = geometry
    if min(rows, cols, width, height) <= 0 or not 220 <= 960 - width < 250:
        raise ValueError("compact fixture did not retain its expected left rail")
    row_height = max(18, math.floor(height / rows * 1.1 + 0.5)) + 10
    if not 3 * row_height <= 128 < 5 * row_height:
        raise ValueError("fixture font does not force a compact rail with a full tab")
    return row_height


def compact_pointer_targets(geometry: tuple[int, ...]) -> dict[str, tuple[int, int]]:
    row_height = compact_row_height(geometry)
    control_y = math.floor(128 - row_height / 2)
    return {"tab": (20, math.floor(row_height * 1.5)),
            "previous": (36, control_y), "next": (110, control_y),
            "new": (183, control_y)}


def native_click(gui, app, backend, point):
    if backend == "x11":
        app.click(*point)
    elif backend == "wayland" and app.env.get("SWAYSOCK"):
        # The private single-output Sway fixture has no borders or gaps, and
        # its single tiled client starts at (0, 0). Never use the user's seat.
        with VirtualPointer(app.env) as pointer:
            pointer.click(*point, 960, 128)
        gui.time.sleep(0.12)
    else:
        raise ValueError("compact pointer fixture requires X11 or private Sway")


def run_tab_placement_scenario(gui, binary: Path, run_dir: Path, env: dict,
                              backend: str) -> None:
    run_dir.mkdir(parents=True, exist_ok=True)
    transcript = run_dir / "pty-transcript.jsonl"
    program, arguments = gui.fixture_command(transcript, instance=173)
    app = gui.AppProcess(binary, run_dir, backend, program, arguments, env)

    def chord(modifiers, key):
        native_chord(app, backend, modifiers, key)

    def starts():
        return [int(row["pid"]) for row in gui.read_records(transcript)
                if row.get("event") == "started"]

    def expect_starts(count):
        gui.wait_until(f"tab-placement fixture count {count}", lambda: len(starts()) == count)
        return starts()[-1]

    def geometries(pids):
        records = gui.read_records(transcript)
        return {pid: gui.latest_terminal_geometry(records, pid) for pid in pids}

    def select(index):
        before = gui.transcript_bytes(transcript)
        chord((), "F6")
        chord((), "Home")
        for _ in range(index * 2):  # each tab has separate body and close targets
            chord((), "Down")
        chord((), "Return")
        chord((), "Escape")
        if gui.transcript_bytes(transcript) != before:
            raise gui.AcceptanceError("tab-bar keyboard navigation leaked into the PTY")

    tokens = {}

    def assert_input(token, pid):
        native_text(app, backend, token)
        gui.wait_until(token, lambda: gui.pids_receiving_token(
            gui.read_records(transcript), token.encode()) == {pid})
        tokens[token] = pid

    config_path = Path(app.env["XDG_CONFIG_HOME"]) / "termeric/config.toml"

    def placement(position):
        config_path.write_text(configuration_for_position(
            config_path.read_text(encoding="utf-8"), position), encoding="utf-8")

    try:
        app.wait_ready(backend)
        original = expect_starts(1)
        chord(("ctrl", "shift"), "t")
        second = expect_starts(2)
        chord(("alt", "shift"), "Right")
        split = expect_starts(3)
        pids = [original, second, split]
        gui.wait_until("split-pane geometry", lambda: (
            geometries(pids)[second][2] < geometries(pids)[original][2]
            and geometries(pids)[split][2] < geometries(pids)[original][2]))
        top = geometries(pids)
        placement("left")
        gui.wait_until("left rail resizes every existing PTY", lambda: all(
            rail_reduced_width(value, top[pid]) for pid, value in geometries(pids).items()))
        left = geometries(pids)

        select(0)
        assert_input("RAIL_FIRST_TAB_173", original)
        select(1)
        assert_input("RAIL_SPLIT_TAB_173", split)

        # NewTab always precedes Commands/Workspaces/Settings at the end of the
        # F6 sequence, independent of whether overflow controls are present.
        chord((), "F6")
        chord((), "End")
        for _ in range(3):
            chord((), "Up")
        chord((), "Return")
        added = expect_starts(4)
        assert_input("RAIL_NEW_TAB_173", added)
        chord((), "F6")
        chord((), "Home")
        for _ in range(5):
            chord((), "Down")
        chord((), "Return")  # close tab 2, not its split-tab sibling
        gui.wait_until("F6-created tab closes", lambda: not Path(f"/proc/{added}").exists())

        select(0)
        placement("top")
        gui.wait_until("top placement restores every existing PTY", lambda: geometries(pids) == top)
        restored = geometries(pids)
        assert_input("TOP_FIRST_TAB_173", original)
        select(1)
        assert_input("TOP_SPLIT_TAB_173", split)
        if starts() != [*pids, added]:
            raise gui.AcceptanceError("tab placement restarted or added an unexpected PTY")
        app.close_normally(backend)
        gui.wait_until("tab-placement child cleanup", lambda: all(
            not Path(f"/proc/{pid}").exists() for pid in [*pids, added]))
        gui.assert_no_runtime_errors(app)
        (run_dir / "tab-placement.json").write_text(json.dumps({
            "schema": 1, "backend": backend, "top": top, "left": left,
            "restored": restored, "input_owners": tokens, "closed_new_tab_pid": added,
            "coverage": "geometry and keyboard routing only; no overflow/drag/IME/visual acceptance",
        }, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    finally:
        app.stop_after_failure()
        app.finish()


def overflow_navigation_keys(tab_count: int) -> tuple[str, ...]:
    if not 2 <= tab_count <= 32:
        raise ValueError("overflow fixture tab count must be between 2 and 32")
    # Last close target -> Previous arrow -> NewTab. At the last viewport,
    # Next is absent. Returning to the active first tab would remove Previous
    # from the focus sequence and cause the second Down to abandon chrome.
    return ("F6", "Home", *("Down",) * (tab_count * 2 - 1), "Down", "Down", "Return")


def run_tab_overflow_scenario(gui, binary: Path, run_dir: Path, env: dict,
                             backend: str) -> None:
    run_dir.mkdir(parents=True, exist_ok=True)
    transcript = run_dir / "pty-transcript.jsonl"
    program, arguments = gui.fixture_command(transcript, instance=174)
    app = gui.AppProcess(binary, run_dir, backend, program, arguments, env)
    count = 12

    def starts():
        return [int(row["pid"]) for row in gui.read_records(transcript)
                if row.get("event") == "started"]

    def chord(modifiers, key):
        native_chord(app, backend, modifiers, key)

    try:
        app.wait_ready(backend)
        gui.wait_until("overflow fixture startup", lambda: len(starts()) == 1)
        original = starts()[0]
        before_width = gui.latest_terminal_geometry(gui.read_records(transcript), original)[2]
        config_path = Path(app.env["XDG_CONFIG_HOME"]) / "termeric/config.toml"
        config_path.write_text(configuration_for_position(
            config_path.read_text(encoding="utf-8"), "left"), encoding="utf-8")
        app.renderer_resize(backend, 960, 240)
        gui.wait_until("compact left rail geometry", lambda: (
            gui.latest_terminal_geometry(gui.read_records(transcript), original)[2] < before_width
            and gui.latest_terminal_geometry(gui.read_records(transcript), original)[3] <= 240))
        # With the renderer's >=28px rail rows, twelve tabs cannot fit in this
        # <=240px content-height fixture, even before reserving its controls.
        for expected in range(2, count + 1):
            chord(("ctrl", "shift"), "t")
            gui.wait_until(f"overflow tab {expected}", lambda: len(starts()) == expected)
        pids = starts()
        for key in ("F6", "Home", "Return", "Escape"):
            chord((), key)
        native_text(app, backend, "OVERFLOW_ORIGINAL_174")
        gui.wait_until("original tab selected", lambda: gui.pids_receiving_token(
            gui.read_records(transcript), b"OVERFLOW_ORIGINAL_174") == {original})
        before = gui.input_streams(gui.read_records(transcript))
        for key in overflow_navigation_keys(count):
            chord((), key)
        gui.wait_until("overflow F6 traversal creates a tab", lambda: len(starts()) == count + 1)
        after = gui.input_streams(gui.read_records(transcript))
        if any(after.get(pid, b"") != before.get(pid, b"") for pid in pids):
            raise gui.AcceptanceError("overflow F6 navigation leaked into an existing PTY")
        added = starts()[-1]
        native_text(app, backend, "OVERFLOW_CREATED_174")
        gui.wait_until("overflow-created tab receives input", lambda: gui.pids_receiving_token(
            gui.read_records(transcript), b"OVERFLOW_CREATED_174") == {added})

        app.renderer_resize(backend, 960, 128)
        gui.wait_until("short rail PTY resize", lambda: (
            gui.latest_terminal_geometry(gui.read_records(transcript), original)[3] < 128))
        compact_geometry = gui.latest_terminal_geometry(gui.read_records(transcript), original)
        points = compact_pointer_targets(compact_geometry)
        if backend == "wayland":
            for kind in ("get_seats", "get_tree"):
                response = gui.run(["swaymsg", "-t", kind], env=app.env)
                (run_dir / f"compact-{kind}.json").write_text(response.stdout, encoding="utf-8")
        for key in ("F6", "Home", "Return", "Escape"):
            chord((), key)
        pointer_owners = {}
        for name, owner in (("tab", original), ("next", pids[1]), ("previous", original)):
            native_click(gui, app, backend, points[name])
            token = f"COMPACT_{name.upper()}_174"
            native_text(app, backend, token)
            gui.wait_until(token, lambda: gui.pids_receiving_token(
                gui.read_records(transcript), token.encode()) == {owner})
            if len(starts()) != count + 1:
                raise gui.AcceptanceError("compact tab/arrow click unexpectedly created a tab")
            pointer_owners[token] = owner
        native_click(gui, app, backend, points["new"])
        gui.wait_until("compact New Tab click", lambda: len(starts()) == count + 2)
        compact_added = starts()[-1]
        native_text(app, backend, "COMPACT_NEW_174")
        gui.wait_until("compact new tab receives input", lambda: gui.pids_receiving_token(
            gui.read_records(transcript), b"COMPACT_NEW_174") == {compact_added})
        pointer_owners["COMPACT_NEW_174"] = compact_added

        # F6 changes chrome focus, not the active PTY. A terminal click must
        # return typing to that owner without requiring an extra Escape.
        focus_owners = {}

        def assert_focus_input(token, owner):
            native_text(app, backend, token)
            gui.wait_until(token, lambda: gui.pids_receiving_token(
                gui.read_records(transcript), token.encode()) == {owner})
            focus_owners[token] = owner

        for key in ("F6", "Home"):
            chord((), key)
        native_click(gui, app, backend, (300, 70))
        assert_focus_input("POINTER_TERMINAL_FOCUS_174", compact_added)

        # Click the first visible row of the distant, keyboard-scrolled tab
        # viewport. Clearing chrome focus before hit testing would select tab
        # zero instead of the distant visible tab. Derive its index from the
        # observed cell height because the compact rail can fit one or two tabs.
        visible_tabs = (128 - 2 * compact_row_height(compact_geometry)) // compact_row_height(compact_geometry)
        distant_owner = starts()[-visible_tabs]
        for key in ("F6", "Home", "Return", "Escape", "F6", "Home"):
            chord((), key)
        for _ in range((count + 1) * 2):
            chord((), "Down")
        native_click(gui, app, backend, points["tab"])
        assert_focus_input("POINTER_DISTANT_TAB_174", distant_owner)

        chord(("alt", "shift"), "Right")
        gui.wait_until("focus fixture split starts", lambda: len(starts()) == count + 3)
        split_pid = starts()[-1]
        for point, owner, token in (
                ((300, 70), distant_owner, "POINTER_LEFT_PANE_174"),
                ((700, 70), split_pid, "POINTER_RIGHT_PANE_174")):
            for key in ("F6", "Home"):
                chord((), key)
            native_click(gui, app, backend, point)
            assert_focus_input(token, owner)
        app.close_normally(backend)
        gui.wait_until("overflow fixture child cleanup", lambda: all(
            not Path(f"/proc/{pid}").exists() for pid in starts()))
        gui.assert_no_runtime_errors(app)
        (run_dir / "tab-overflow.json").write_text(json.dumps({
            "schema": 3, "backend": backend, "original_pids": pids,
            "created_pid": added, "keyboard_sequence": overflow_navigation_keys(count),
            "compact_pointer_targets": points, "compact_input_owners": pointer_owners,
            "compact_created_pid": compact_added,
            "pointer_focus_input_owners": focus_owners, "focus_split_pid": split_pid,
            "coverage": "overflow keyboard, compact pointer, and F6-to-pointer tab/pane focus routing; not screenshot, drag, or screen-reader acceptance",
        }, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    finally:
        app.stop_after_failure()
        app.finish()
