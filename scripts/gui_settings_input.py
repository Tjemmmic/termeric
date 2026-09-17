"""Private Wayland Settings-filter routing, with observable setting effects."""
import json
import math
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
)


def settings_pointer_point(geometry, column):
    """Independent settled SettingsModalLayout math, from actual PTY cells."""
    rows, cols, width, height = geometry
    if min(rows, cols, width, height) <= 0 or not math.isfinite(column) or column < 0:
        raise ValueError("invalid native Settings pointer geometry")
    cw, ch = width / cols, height / rows
    panel_width = min(max(cw * 60, 500), SURFACE[0] * 0.7)
    panel_x, padding = (SURFACE[0] - panel_width) / 2, cw * 1.5
    text_x = panel_x + padding + cw * 2.2
    right = panel_x + panel_width - padding - cw * 0.8
    if column >= math.floor((right - text_x) / cw):
        raise ValueError("Settings click is outside the visible filter")
    return (math.floor(text_x + column * cw),
            math.floor(SURFACE[1] * 0.1 + padding + ch * 2.4 + 6))


def verify_settings_delivery(streams, owner, fence):
    actual = {pid: bytes(data) for pid, data in streams.items() if data}
    if type(owner) is not int or owner <= 0 or not fence or actual != {owner: fence}:
        raise ValueError("Settings editing leaked bytes or lost its exact PTY owner")


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


def run_settings_input_checks(gui, app, transcript, owner):
    report = {"schema": 1, "backend": "wayland", "cases": [],
              "coverage": "private Settings clipboard/Unicode/pointer routing and font-size effect; not visual, native IME, or independently sealed evidence"}
    records = lambda: gui.read_records(transcript)
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
                loss = app.log_count("Window focus changed: false")
                larger_before = app.log_count("Font size set to 15,")
                restored_before = app.log_count("Font size set to 14,")
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
                    elif kind == "click":
                        gui.time.sleep(0.25)
                        native_click(gui, app, "wayland", settings_pointer_point(geometry, value), SURFACE)
                    else:
                        raise ValueError("unknown native Settings edit")
                app.wayland_chord((), "Right")
                gui.wait_until(f"{name}: filtered Size action changes native geometry", lambda:
                    app.log_count("Font size set to 15,") == larger_before + 1
                    and gui.latest_terminal_geometry(records(), owner) != geometry)
                larger = gui.latest_terminal_geometry(records(), owner)
                if not (larger[0] < geometry[0] and larger[1] < geometry[1]):
                    raise gui.AcceptanceError("larger font did not reduce PTY rows and columns")
                app.wayland_chord((), "Left")
                gui.wait_until(f"{name}: filtered Size action restores native geometry", lambda:
                    app.log_count("Font size set to 14,") == restored_before + 1
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
                        "unrelated_focus_losses": app.log_count("Window focus changed: false") - loss}
                report["cases"].append(case)
                verify_settings_delivery(delta, owner, fence)
                if (app.log_count("Font size set to 15,") != larger_before + 1
                        or app.log_count("Font size set to 14,") != restored_before + 1):
                    raise gui.AcceptanceError("Settings case applied an extra font-size change")
                if case["unrelated_focus_losses"] != 0:
                    raise gui.AcceptanceError("unrelated focus loss invalidated Settings evidence")
            report.pop("active_case")
    except Exception as error:
        report["error"] = str(error)
        raise
    finally:
        (app.run_dir / "settings-input.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
