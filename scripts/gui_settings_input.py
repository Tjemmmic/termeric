"""Private Wayland Settings-filter routing, with observable setting effects."""
import json
import math
import re
import subprocess

from scripts.gui_tab_placement import native_click, native_keyboard_lifetime

SURFACE = (1024, 768)
CASES = (
    ("paste", "size", ()),
    ("reject-multiline", "size", (("paste", "\nexecute\n"),)),
    ("reject-oversize", "size", (("paste", "x" * 257),)),
    ("combining-backspace", "sizee\u0301", (("key", "BackSpace"),)),
    ("zwj-backspace", "size👩\u200d💻", (("key", "BackSpace"),)),
    ("pointer-wide", "si界ze", (("click", 3.75), ("key", "BackSpace"))),
    ("pointer-combining", "sie\u0301ze", (("click", 2.75), ("key", "BackSpace"))),
    ("home-delete", "e\u0301size", (("key", "Home"), ("key", "Delete"))),
    ("end-word-backspace", "size draft", (("key", "Home"), ("key", "End"), ("ctrl", "BackSpace"), ("key", "BackSpace"))),
    ("word-left-backspace", "draft size", (("ctrl", "Left"), ("ctrl", "BackSpace"))),
    ("word-right-delete", "size draft", (("key", "Home"), ("ctrl", "Right"), ("ctrl", "Delete"))),
)


def settings_pointer_point(geometry, column):
    """Independent settled SettingsModalLayout math, from actual PTY cells:
    the dialog is centered, and its search field sits in the header, left of
    the close button."""
    rows, cols, width, height = geometry
    if min(rows, cols, width, height) <= 0 or not math.isfinite(column) or column < 0:
        raise ValueError("invalid native Settings pointer geometry")
    cw, ch = width / cols, height / rows
    panel_width = min(min(max(cw * 100, 560), 900), max(SURFACE[0] - cw * 4, 0))
    panel_height = min(min(max(SURFACE[1] * 0.82, 320), 640), max(SURFACE[1] - ch * 2, 0))
    panel_x = (SURFACE[0] - panel_width) / 2
    panel_y = (SURFACE[1] - panel_height) * 0.4
    padding, control, header = cw * 1.5, ch * 1.5, ch * 3.0
    close_x = panel_x + panel_width - padding * 0.5 - control
    input_width, input_height = min(panel_width * 0.36, cw * 36), ch * 1.7
    input_x = close_x - cw * 0.75 - input_width
    input_y = panel_y + (header - input_height) / 2
    # ui::constants::FILTER_TEXT_INSET_CELLS
    text_x = input_x + cw * 2.6
    right = input_x + input_width - cw * 0.8
    if column >= math.floor((right - text_x) / cw):
        raise ValueError("Settings click is outside the visible filter")
    return (math.floor(text_x + column * cw), math.floor(input_y + input_height / 2))


def verify_settings_delivery(streams, owner, fence):
    actual = {pid: bytes(data) for pid, data in streams.items() if data}
    if type(owner) is not int or owner <= 0 or not fence or actual != {owner: fence}:
        raise ValueError("Settings editing leaked bytes or lost its exact PTY owner")


def font_changes(log):
    """Retain every complete application diagnostic, including duplicates."""
    return [(float(size), int(cols), int(rows)) for size, cols, rows in
            re.findall(r"Font size set to ([0-9]+(?:\.[0-9]+)?), grid resized to ([0-9]+)x([0-9]+)\r?\n", log)]


def observed_font_changes(app, offset):
    with app.log_path.open("rb") as log:
        log.seek(offset)
        data = log.read(4 * 1024 * 1024 + 1)
    if len(data) > 4 * 1024 * 1024:
        raise ValueError("Settings case log exceeds its diagnostic budget")
    return font_changes(data.decode("utf-8", "replace"))


def font_state(size, geometry):
    rows, cols, _width, _height = geometry
    return size, cols, rows


def paste_private(gui, app, text):
    # Foreground ownership and --paste-once make successful transfer observable
    # and keep the source process within this scenario's cleanup boundary.
    source = subprocess.Popen(["wl-copy", "--foreground", "--paste-once", "--", text],
                              env=app.env, stdout=subprocess.DEVNULL,
                              stderr=subprocess.PIPE, text=True)
    try:
        gui.time.sleep(0.12)
        if source.poll() is not None:
            raise gui.AcceptanceError("private clipboard source exited before paste")
        app.wayland_chord(("ctrl", "shift"), "v")
        if source.wait(timeout=5) != 0:
            raise gui.AcceptanceError("private clipboard source failed during paste")
    finally:
        if source.poll() is None:
            source.terminate()
            source.wait(timeout=3)
        if source.stderr is not None:
            source.stderr.close()


def geometry_transitions(records, owner, start):
    """Distinct PTY sizes the owner went through from record index `start` on."""
    sizes = []
    for record in records[start:]:
        if record.get("pid") == owner and record.get("event") in ("started", "winsize"):
            size = tuple(int(record[key]) for key in ("rows", "cols", "pixel_width", "pixel_height"))
            if not sizes or sizes[-1] != size:
                sizes.append(size)
    return sizes


def run_settings_input_checks(gui, app, transcript, owner):
    report = {"backend": "wayland", "cases": []}
    records = lambda: gui.read_records(transcript)
    baseline = None
    try:
        app.renderer_resize("wayland", *SURFACE)
        with native_keyboard_lifetime(gui, app, "wayland", duration_ms=120000):
            for name, query, edits in CASES:
                report["active_case"] = name
                begin = f"SETTINGS_BEGIN_{name}"
                app.wayland_type(begin)
                gui.wait_until(begin, lambda: gui.pids_receiving_token(records(), begin.encode()) == {owner})
                before = gui.input_streams(records())
                geometry = gui.latest_terminal_geometry(records(), owner)
                if baseline is None:
                    baseline = geometry
                elif geometry != baseline:
                    raise gui.AcceptanceError(f"{name}: Settings case started from a different geometry")
                start_index = len(records())
                loss = app.log_count("Window focus changed: false")
                log_offset = app.log_path.stat().st_size
                app.wayland_chord(("ctrl",), "comma")
                # Start on Family, not Size. Ignoring the paste must not let
                # the default selected Size row falsely satisfy the oracle.
                app.wayland_chord((), "Down")
                paste_private(gui, app, query)
                for kind, value in edits:
                    if kind == "paste":
                        paste_private(gui, app, value)
                    elif kind == "key":
                        app.wayland_chord((), value)
                    elif kind == "ctrl":
                        app.wayland_chord(("ctrl",), value)
                    elif kind == "click":
                        gui.time.sleep(0.25)
                        native_click(gui, app, "wayland", settings_pointer_point(geometry, value), SURFACE)
                    else:
                        raise ValueError("unknown native Settings edit")
                app.wayland_chord((), "Right")
                gui.wait_until(f"{name}: filtered Size action changes native geometry", lambda:
                    observed_font_changes(app, log_offset) == [font_state(15, gui.latest_terminal_geometry(records(), owner))]
                    and gui.latest_terminal_geometry(records(), owner) != geometry)
                larger = gui.latest_terminal_geometry(records(), owner)
                if not (larger[0] < geometry[0] and larger[1] < geometry[1]):
                    raise gui.AcceptanceError("larger font did not reduce PTY rows and columns")
                app.wayland_chord((), "Left")
                expected_changes = [font_state(15, larger), font_state(14, geometry)]
                gui.wait_until(f"{name}: filtered Size action restores native geometry", lambda:
                    observed_font_changes(app, log_offset) == expected_changes
                    and gui.latest_terminal_geometry(records(), owner) == geometry)
                app.wayland_chord((), "Escape")  # clear filter
                app.wayland_chord((), "Escape")  # close Settings
                fence = f"SETTINGS_END_{name}_{owner}".encode()
                app.wayland_type(fence.decode())
                gui.wait_until(fence.decode(), lambda: gui.pids_receiving_token(records(), fence) == {owner})
                after = gui.input_streams(records())
                delta = {pid: bytes(data)[len(before.get(pid, b"")):] for pid, data in after.items()}
                case = {"case": name, "owner_pid": owner, "before": geometry,
                        "larger": larger, "restored": gui.latest_terminal_geometry(records(), owner),
                        "actual_hex": {str(pid): data.hex() for pid, data in delta.items() if data},
                        "font_changes": observed_font_changes(app, log_offset),
                        "unrelated_focus_losses": app.log_count("Window focus changed: false") - loss}
                report["cases"].append(case)
                verify_settings_delivery(delta, owner, fence)
                if case["font_changes"] != expected_changes:
                    raise gui.AcceptanceError("Settings case applied an extra font-size change")
                if case["unrelated_focus_losses"] != 0:
                    raise gui.AcceptanceError("unrelated focus loss during a Settings case")
                if geometry_transitions(records(), owner, start_index) not in ([larger, geometry], [geometry, larger, geometry]):
                    raise gui.AcceptanceError(f"{name}: PTY went through intermediate sizes")
            report.pop("active_case")
    except Exception as error:
        report["error"] = str(error)
        raise
    finally:
        (app.run_dir / "settings-input.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
