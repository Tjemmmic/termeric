"""Native top/left tab geometry and keyboard routing; no external clients."""
from __future__ import annotations

import json
import math
from contextlib import contextmanager
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


# The renderer's TAB_BAR_HEIGHT at scale 1. Chrome rows use the desktop UI
# face, so their height no longer follows the terminal font.
TAB_ROW_HEIGHT = 34
# The renderer's MIN_TAB_WIDTH at scale 1: the top bar overflows below it.
MIN_TOP_TAB_WIDTH = 96
# The panel-dismissal fixture: wide enough that the split's divider, halfway
# across the terminal right of the 220px rail, lies left of a side panel (at
# most 484px wide at the fixture's 11px cells), so a click on it is outside.
PANEL_SURFACE = (1280, 720)
PANEL_DIVIDER_X = 220 + (PANEL_SURFACE[0] - 220) // 2
PANEL_POINTER_Y = 680


def compact_row_height(geometry: tuple[int, ...]) -> int:
    # This fixture uses a 960x128 scale-1 surface and the default 220px rail.
    rows, cols, width, height = geometry
    if min(rows, cols, width, height) <= 0 or not 220 <= 960 - width < 250:
        raise ValueError("compact fixture did not retain its expected left rail")
    return TAB_ROW_HEIGHT


def compact_pointer_targets(geometry: tuple[int, ...]) -> dict[str, tuple[int, int]]:
    row_height = compact_row_height(geometry)
    control_y = math.floor(128 - row_height / 2)
    return {"tab": (20, math.floor(row_height * 1.5)),
            "previous": (36, control_y), "next": (110, control_y),
            "new": (183, control_y)}


def overflow_drag_layout(name: str, row_height: int, tab_count: int) -> dict:
    """Scale-1 fixture geometry and independent tab-order checkpoints.

    Top/normal controls disappear at the first/last visible viewport. Compact
    controls instead follow the active index, all the way to the end tabs.
    """
    if not 28 <= row_height <= 42 or not 8 <= tab_count <= 16:
        raise ValueError("overflow drag fixture metrics exceed its bounded hold")
    if name == "top":
        width, height, position = 480, 240, "top"
        viewport_width = width - 6 * row_height
        visible = viewport_width // MIN_TOP_TAB_WIDTH
        slot_width = viewport_width / visible
        point = lambda offset: (math.floor(row_height + offset * slot_width + 20), row_height // 2)
        previous, next_ = (row_height // 2, row_height // 2), (math.floor(width - 4.5 * row_height), row_height // 2)
    elif name in ("normal", "compact"):
        width, height, position = 960, row_height * 6 if name == "normal" else 128, "left"
        visible = 2 if name == "normal" else (height - 2 * row_height) // row_height
        first_y = row_height * (2 if name == "normal" else 1)
        point = lambda offset: (20, math.floor(first_y + (offset + 0.5) * row_height))
        if name == "normal":
            previous, next_ = (20, math.floor(1.5 * row_height)), (20, math.floor(4.5 * row_height))
        else:
            previous, next_ = (36, math.floor(height - row_height / 2)), (110, math.floor(height - row_height / 2))
    else:
        raise ValueError("unknown overflow drag layout")
    forward = tab_count - 1 if name == "compact" else tab_count - visible + visible // 2
    reverse = 0 if name == "compact" else visible // 2
    return {"name": name, "position": position, "surface": (width, height),
            "visible": visible, "forward_index": forward, "reverse_index": reverse,
            "start": point(0), "forward_start": point(forward - (tab_count - visible)),
            "reverse_start": point(reverse), "previous": previous, "next": next_}


def native_click(gui, app, backend, point, surface=(960, 128)):
    if backend == "x11":
        app.click(*point)
    elif backend == "wayland" and app.env.get("SWAYSOCK"):
        # The private single-output Sway fixture has no borders or gaps, and
        # its single tiled client starts at (0, 0). Never use the user's seat.
        with VirtualPointer(app.env) as pointer:
            pointer.click(*point, *surface)
        gui.time.sleep(0.12)
    else:
        raise ValueError("compact pointer fixture requires X11 or private Sway")


def native_drag(gui, app, backend, start, end, surface, during_drag=None, *, end_surface=None,
                hold_seconds=0.12):
    if not 0.12 <= hold_seconds <= 10:
        raise ValueError("native drag hold must be bounded")
    if backend == "wayland":
        with VirtualPointer(app.env) as pointer:
            pointer.move(*start, *surface)
            pointer.set_left_button(True)
            try:
                gui.time.sleep(0.12)
                if during_drag is not None:
                    during_drag()
                pointer.move(*end, *(end_surface or surface))
                gui.time.sleep(hold_seconds)
            finally:
                pointer.set_left_button(False)
    elif backend == "x11":
        app.activate()
        app.xdo("mousemove", "--window", app.window_id, *(str(value) for value in start))
        app.xdo("mousedown", "1")
        try:
            gui.time.sleep(0.12)
            if during_drag is not None:
                during_drag()
            app.xdo("mousemove", "--window", app.window_id, *(str(value) for value in end))
            gui.time.sleep(hold_seconds)
        finally:
            app.xdo("mouseup", "1")
    else:
        raise ValueError("unsupported drag fixture backend")


def native_interrupted_drag(gui, app, backend, start, end, surface, *, release_while_hidden=True):
    with native_keyboard_lifetime(gui, app, backend):
        return _native_interrupted_drag(gui, app, backend, start, end, surface, release_while_hidden=release_while_hidden)


@contextmanager
def native_keyboard_lifetime(gui, app, backend, *, duration_ms=30000):
    if backend != "wayland":
        yield
        return
    # A real desktop retains its keyboard while switching windows. Individual
    # wtype invocations disconnect theirs, generating unrelated focus losses.
    if not 1 <= duration_ms <= 600000:
        raise ValueError("private keyboard lifetime must be bounded to ten minutes")
    keyboard = gui.subprocess.Popen(gui.wtype_command("-s", str(duration_ms)), env=app.env,
                                    stdout=gui.subprocess.DEVNULL, stderr=gui.subprocess.DEVNULL)
    try:
        def focused():
            log = app.log_path.read_text(encoding="utf-8", errors="replace")
            # App starts focused; duplicate initial true notifications are not
            # logged. Only an observed loss requires a subsequent regain.
            lost = log.rfind("Window focus changed: false")
            return keyboard.poll() is None and (lost < 0 or log.rfind("Window focus changed: true") > lost)
        gui.wait_until("persistent private keyboard focus", focused)
        yield
    finally:
        keyboard.terminate()
        try:
            keyboard.wait(timeout=5)
        except gui.subprocess.TimeoutExpired:
            keyboard.kill()
            keyboard.wait(timeout=5)


def _native_interrupted_drag(gui, app, backend, start, end, surface, *, release_while_hidden=True):
    lost = app.log_count("Window focus changed: false")
    gained = app.log_count("Window focus changed: true")

    def hide():
        if backend == "wayland":
            gui.run(["swaymsg", "workspace", "number", "2"], env=app.env)
        else:
            app.xdo("windowminimize", "--sync", app.window_id)
        gui.wait_until("drag window loses focus", lambda: app.log_count("Window focus changed: false") > lost)

    def show():
        if backend == "wayland":
            gui.run(["swaymsg", "workspace", "number", "1"], env=app.env)
        else:
            app.xdo("windowmap", "--sync", app.window_id)
            app.activate()
        gui.wait_until("drag window regains focus", lambda: app.log_count("Window focus changed: true") > gained)

    def interrupt(release, move):
        try:
            try:
                gui.time.sleep(0.12)
                hide()
            finally:
                try:
                    if release_while_hidden:
                        release()
                finally:
                    show()
            move()
            gui.time.sleep(0.12)
        finally:
            if not release_while_hidden:
                release()

    if backend == "wayland":
        with VirtualPointer(app.env) as pointer:
            pointer.move(*start, *surface)
            pointer.set_left_button(True)
            interrupt(lambda: pointer.set_left_button(False), lambda: pointer.move(*end, *surface))
    elif backend == "x11":
        app.activate()
        app.xdo("mousemove", "--window", app.window_id, *(str(value) for value in start))
        app.xdo("mousedown", "1")
        interrupt(lambda: app.xdo("mouseup", "1"),
                  lambda: app.xdo("mousemove", "--window", app.window_id, *(str(value) for value in end)))
    else:
        raise ValueError("unsupported interrupted-drag backend")
    return {"focus_lost": app.log_count("Window focus changed: false") - lost,
            "focus_regained": app.log_count("Window focus changed: true") - gained}


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
    arguments.append("--scrollback-fixture")
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
        with native_keyboard_lifetime(gui, app, backend):
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
        # Tabs in order, and where the distant tab is: the last one when the
        # compact rail shows one tab, the one before it when it shows two.
        tab_order = [*pids, added, compact_added]
        distant_index = tab_order.index(distant_owner)
        after_distant = tab_order[(distant_index + 1) % len(tab_order)]
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

        # A six-row rail has exactly two normal-height visible tab slots.
        # Resize from the compact fixture so slot metrics are font-derived.
        row_height = compact_row_height(compact_geometry)
        drag_height = row_height * 6
        old_height = gui.latest_terminal_geometry(gui.read_records(transcript), original)[3]
        app.renderer_resize(backend, 960, drag_height)
        gui.wait_until("drag rail resize", lambda: gui.latest_terminal_geometry(
            gui.read_records(transcript), original)[3] > old_height)
        source = (20, math.floor(row_height * 2.5))
        destination = (20, math.floor(row_height * 3.5))
        drag_owners = {}

        def select_and_check(index, token, owner):
            for key in ("F6", "Home", *("Down",) * (index * 2), "Return", "Escape"):
                chord((), key)
            native_text(app, backend, token)
            gui.wait_until(token, lambda: gui.pids_receiving_token(
                gui.read_records(transcript), token.encode()) == {owner})
            drag_owners[token] = owner

        select_and_check(0, "DRAG_INITIAL_174", original)
        native_drag(gui, app, backend, source, destination, (960, drag_height))
        select_and_check(0, "DRAG_FORWARD_FIRST_174", pids[1])
        select_and_check(1, "DRAG_FORWARD_SECOND_174", original)
        native_drag(gui, app, backend, destination, source, (960, drag_height))
        select_and_check(0, "DRAG_RESTORED_FIRST_174", original)
        select_and_check(1, "DRAG_RESTORED_SECOND_174", pids[1])
        def set_drag_layout(layout):
            before = app.log_count("Config hot-reloaded")
            config_path.write_text(configuration_for_position(config_path.read_text(), layout["position"]))
            gui.wait_until("drag placement reload", lambda: app.log_count("Config hot-reloaded") > before)
            width, height = layout["surface"]
            app.renderer_resize(backend, width, height)
            cell_width = compact_geometry[2] / compact_geometry[1]
            cell_height = compact_geometry[3] / compact_geometry[0]
            expected_width = width - (220 if layout["position"] == "left" else 0) - 16
            expected_height = height - row_height - 16
            def sized():
                geometry = gui.latest_terminal_geometry(gui.read_records(transcript), original)
                return (expected_width - cell_width < geometry[2] <= expected_width
                        and expected_height - cell_height < geometry[3] <= expected_height)
            gui.wait_until("drag layout PTY geometry", sized)

        # Hold without further motion in every distinct layout. Observe exact
        # ownership immediately after each hold, before keyboard reselection.
        overflow_layouts = []
        for name in ("normal", "compact", "top"):
            layout = overflow_drag_layout(name, row_height, count + 2)
            set_drag_layout(layout)
            prefix = f"OVERFLOW_{name.upper()}"
            with native_keyboard_lifetime(gui, app, backend, duration_ms=60000):
                losses = app.log_count("Window focus changed: false")
                select_and_check(0, f"{prefix}_START_174", original)
                native_drag(gui, app, backend, layout["start"], layout["next"],
                            layout["surface"], hold_seconds=4)
                assert_focus_input(f"{prefix}_HELD_OWNER_174", original)
                select_and_check(layout["forward_index"], f"{prefix}_LAST_VIEWPORT_174", original)
                native_drag(gui, app, backend, layout["forward_start"], layout["previous"],
                            layout["surface"], hold_seconds=4)
                assert_focus_input(f"{prefix}_REVERSE_OWNER_174", original)
                select_and_check(layout["reverse_index"], f"{prefix}_FIRST_VIEWPORT_174", original)
                if layout["reverse_index"]:
                    native_drag(gui, app, backend, layout["reverse_start"], layout["start"], layout["surface"])
                select_and_check(0, f"{prefix}_RESTORED_174", original)
                select_and_check(1, f"{prefix}_SIBLING_174", pids[1])
                if app.log_count("Window focus changed: false") != losses:
                    raise gui.AcceptanceError(f"{name} overflow traversal had unrelated focus loss")
            overflow_layouts.append(layout)
        set_drag_layout(overflow_layouts[0])
        # Holding tab zero must never start moving tab one after Ctrl+Tab.
        native_drag(gui, app, backend, source, (40, source[1]), (960, drag_height),
                    during_drag=lambda: chord(("ctrl",), "Tab"))
        select_and_check(0, "DRAG_OWNER_FIRST_174", original)
        select_and_check(1, "DRAG_OWNER_SECOND_174", pids[1])
        focus_transitions = native_interrupted_drag(gui, app, backend, source, destination, (960, drag_height))
        select_and_check(0, "DRAG_BLUR_FIRST_174", original)
        select_and_check(1, "DRAG_BLUR_SECOND_174", pids[1])
        held_focus_transitions = native_interrupted_drag(
            gui, app, backend, source, destination, (960, drag_height), release_while_hidden=False)
        select_and_check(0, "DRAG_HELD_RETURN_FIRST_174", original)
        select_and_check(1, "DRAG_HELD_RETURN_SECOND_174", pids[1])
        with native_keyboard_lifetime(gui, app, backend):
            lost_before_modal = app.log_count("Window focus changed: false")
            def open_palette():
                chord(("ctrl", "shift"), "p")
                native_text(app, backend, "MODAL_CAPTURE_174")
                gui.time.sleep(0.15)
            native_drag(gui, app, backend, source, destination, (960, drag_height),
                        during_drag=open_palette)
            chord((), "Escape")
            gui.time.sleep(0.15)
            if app.log_count("Window focus changed: false") != lost_before_modal:
                raise gui.AcceptanceError("modal drag was interrupted by unrelated focus loss")
            if gui.pids_receiving_token(gui.read_records(transcript), b"MODAL_CAPTURE_174"):
                raise gui.AcceptanceError("modal drag query leaked into a PTY")
        select_and_check(0, "DRAG_MODAL_FIRST_174", original)
        select_and_check(1, "DRAG_MODAL_SECOND_174", pids[1])
        with native_keyboard_lifetime(gui, app, backend):
            lost_before_resize = app.log_count("Window focus changed: false")
            old_width = gui.latest_terminal_geometry(gui.read_records(transcript), original)[2]
            def resize_during_drag():
                app.renderer_resize(backend, 1040, drag_height)
                gui.wait_until("held drag PTY resize", lambda: gui.latest_terminal_geometry(
                    gui.read_records(transcript), original)[2] > old_width)
            native_drag(gui, app, backend, source, destination, (960, drag_height),
                        during_drag=resize_during_drag, end_surface=(1040, drag_height))
            if app.log_count("Window focus changed: false") != lost_before_resize:
                raise gui.AcceptanceError("resize drag was interrupted by unrelated focus loss")
            app.renderer_resize(backend, 960, drag_height)
            gui.wait_until("held drag geometry restored", lambda: gui.latest_terminal_geometry(
                gui.read_records(transcript), original)[2] == old_width)
        select_and_check(0, "DRAG_RESIZE_FIRST_174", original)
        select_and_check(1, "DRAG_RESIZE_SECOND_174", pids[1])
        # Keep the original split active. A click outside an open panel, but
        # directly on its background divider, must dismiss the panel rather
        # than begin resizing. Only the existing focused PTY receives typing.
        select_and_check(distant_index, "PANEL_SPLIT_SELECTED_174", split_pid)
        old_geometry = gui.latest_terminal_geometry(gui.read_records(transcript), split_pid)
        app.renderer_resize(backend, *PANEL_SURFACE)
        gui.wait_until("panel pointer fixture resize", lambda: gui.latest_terminal_geometry(
            gui.read_records(transcript), split_pid)[3] > old_geometry[3])
        panel_geometry = {pid: gui.latest_terminal_geometry(gui.read_records(transcript), pid)
                          for pid in (distant_owner, split_pid)}
        panel_owners = {}
        for name, steps in (("SETTINGS", 0), ("SESSION_DROPDOWN", 1), ("COMMANDS", 2)):
            for key in ("F6", "End", *("Up",) * steps, "Return"):
                chord((), key)
            gui.time.sleep(0.25)
            native_click(gui, app, backend, (PANEL_DIVIDER_X, PANEL_POINTER_Y), PANEL_SURFACE)
            gui.time.sleep(0.25)
            token = f"PANEL_{name}_DISMISSED_174"
            assert_focus_input(token, split_pid)
            panel_owners[token] = split_pid
            if any(gui.latest_terminal_geometry(gui.read_records(transcript), pid) != geometry
                   for pid, geometry in panel_geometry.items()):
                raise gui.AcceptanceError("panel dismissal resized a background pane")
        if len(starts()) != count + 3:
            raise gui.AcceptanceError("tab dragging unexpectedly created another PTY")
        # Prove this is a working divider hit before testing interruption.
        # Both panes must receive changed PTY geometry, then keep that exact
        # preview when Ctrl+Tab hides their tab during the next held drag.
        with native_keyboard_lifetime(gui, app, backend):
            lost_before_divider = app.log_count("Window focus changed: false")
            native_drag(gui, app, backend, (PANEL_DIVIDER_X, PANEL_POINTER_Y),
                        (PANEL_DIVIDER_X + 90, PANEL_POINTER_Y), PANEL_SURFACE)
            gui.wait_until("positive divider resize", lambda: all(
                gui.latest_terminal_geometry(gui.read_records(transcript), pid) != geometry
                for pid, geometry in panel_geometry.items()))
            divider_geometry = {pid: gui.latest_terminal_geometry(gui.read_records(transcript), pid)
                                for pid in panel_geometry}
            sibling_geometry = gui.latest_terminal_geometry(gui.read_records(transcript), after_distant)
            native_drag(gui, app, backend, (PANEL_DIVIDER_X + 90, PANEL_POINTER_Y),
                        (PANEL_DIVIDER_X + 160, PANEL_POINTER_Y), PANEL_SURFACE,
                        during_drag=lambda: chord(("ctrl",), "Tab"))
            assert_focus_input("DIVIDER_SWITCH_TARGET_174", after_distant)
            if app.log_count("Window focus changed: false") != lost_before_divider:
                raise gui.AcceptanceError("divider drag was interrupted by unrelated focus loss")
            if any(gui.latest_terminal_geometry(gui.read_records(transcript), pid) != geometry
                   for pid, geometry in divider_geometry.items()):
                raise gui.AcceptanceError("divider drag resized its hidden source tab")
            if gui.latest_terminal_geometry(gui.read_records(transcript), after_distant) != sibling_geometry:
                raise gui.AcceptanceError("divider drag resized the newly selected tab")
        select_and_check(1, "SCROLL_TARGET_SELECTED_174", pids[1])
        chord(("ctrl", "shift"), "End")
        select_and_check(0, "SCROLL_SOURCE_SELECTED_174", original)
        with native_keyboard_lifetime(gui, app, backend):
            lost_before_scroll = app.log_count("Window focus changed: false")
            # The right pane's scrollbar, 12px inside the window's edge.
            scrollbar_x = PANEL_SURFACE[0] - 12
            native_drag(gui, app, backend, (scrollbar_x, PANEL_POINTER_Y), (scrollbar_x, 45),
                        PANEL_SURFACE,
                        during_drag=lambda: chord(("ctrl",), "Tab"))
            if app.log_count("Window focus changed: false") != lost_before_scroll:
                raise gui.AcceptanceError("scrollbar drag was interrupted by unrelated focus loss")
        # Copy the cursor's visible line without searching or resetting scroll.
        # At the bottom this is the owner marker; a stolen scroll reads history.
        chord(("ctrl", "shift"), "x")
        chord((), "Home")
        native_text(app, backend, "v")
        chord((), "End")
        native_text(app, backend, "y")
        clipboard_command = (["wl-paste", "--no-newline"] if backend == "wayland"
                             else ["xclip", "-selection", "clipboard", "-o"])
        copied_scroll_line = gui.run(clipboard_command, env=app.env).stdout
        if copied_scroll_line.strip() != f"SCROLL_END_{pids[1]}":
            raise gui.AcceptanceError(f"scrollbar drag retargeted the selected tab: {copied_scroll_line!r}")
        app.close_normally(backend)
        gui.wait_until("overflow fixture child cleanup", lambda: all(
            not Path(f"/proc/{pid}").exists() for pid in starts()))
        gui.assert_no_runtime_errors(app)
        (run_dir / "tab-overflow.json").write_text(json.dumps({
            "schema": 12, "backend": backend, "original_pids": pids,
            "created_pid": added, "keyboard_sequence": overflow_navigation_keys(count),
            "compact_pointer_targets": points, "compact_input_owners": pointer_owners,
            "compact_created_pid": compact_added,
            "pointer_focus_input_owners": focus_owners, "focus_split_pid": split_pid,
            "drag_input_owners": drag_owners, "drag_surface": [960, drag_height],
            "overflow_drag_layouts": overflow_layouts, "overflow_drag_focus_losses": 0,
            "drag_focus_transitions": focus_transitions,
            "drag_held_focus_transitions": held_focus_transitions,
            "modal_drag_focus_losses": 0,
            "resize_drag_focus_losses": 0,
            "resize_drag_widths": [960, 1040, 960],
            "scrollbar_target_visible_line": copied_scroll_line,
            "scrollbar_drag_focus_losses": 0,
            "divider_drag_focus_losses": 0,
            "divider_source_geometry_after_switch": divider_geometry,
            "divider_target_geometry_after_switch": sibling_geometry,
            "panel_dismiss_input_owners": panel_owners,
            "panel_background_divider_point": [PANEL_DIVIDER_X, PANEL_POINTER_Y],
            "panel_geometry": panel_geometry,
            "drag_points": {"source": source, "destination": destination},
            "coverage": "overflow keyboard, pointer focus, forward/reverse drag, normal/compact/top held-control traversal with exact ownership, drag ownership, window-focus/palette/resize interruption, panel outside-divider dismissal, and cross-tab scrollbar/divider isolation; not screenshot, arbitrary lost-release, pinned/live-domain traversal, or screen-reader acceptance",
        }, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    finally:
        app.stop_after_failure()
        app.finish()
