"""Native held-button ownership checks on the private Wayland seat."""
import json
import re

from scripts.gui_tab_placement import native_keyboard_lifetime
from scripts.gui_wayland_pointer import VirtualPointer


def mouse_reports(data):
    """Parse complete SGR reports after joining all PTY read fragments."""
    matches = list(re.finditer(rb"\x1b\[<(\d+);(\d+);(\d+)([Mm])", data))
    if data.count(b"\x1b[<") != len(matches):
        raise ValueError("incomplete or malformed SGR mouse report")
    result = [(int(m[1]), int(m[2]), int(m[3]), m[4].decode()) for m in matches]
    if any(x < 1 or y < 1 for _, x, y, _ in result):
        raise ValueError("mouse report contains non-positive coordinates")
    return result


def verify_mouse_owners(streams, source, destination, *, switched, prior_motion=False):
    if source == destination or set(streams) != {source, destination}:
        raise ValueError("mouse ownership requires two exact, distinct PTYs")
    reports = {pid: mouse_reports(data) for pid, data in streams.items()}
    actual = reports[source]
    if reports[destination]:
        raise ValueError(f"mouse reports leaked to replacement owner: {reports}")
    kinds = [(code, final) for code, _, _, final in actual]
    if switched:
        if (not kinds or kinds[0] != (0, "M")
                or (kinds[1:] and not prior_motion)
                or any(kind != (32, "M") for kind in kinds[1:])):
            raise ValueError(f"interrupted mouse gesture was not canceled: {reports}")
    elif (len(kinds) < 3 or kinds[0] != (0, "M") or kinds[-1] != (0, "m")
          or any(kind != (32, "M") for kind in kinds[1:-1])):
        raise ValueError(f"ordinary mouse drag lost its exact press/motion/release: {reports}")
    return reports


def run_mouse_ownership_checks(gui, app, transcript, original_pid):
    """Start/end with the original single-pane tab; never use a desktop seat."""
    def records():
        return gui.read_records(transcript)

    def starts():
        return [int(r["pid"]) for r in records() if r.get("event") == "started"]

    def token(text, owner):
        app.wayland_type(text)
        gui.wait_until(text, lambda: gui.pids_receiving_token(records(), text.encode()) == {owner})

    cases = []
    surface = (1024, 768)
    # Normalize geometry after optional renderer recovery resizes.
    app.renderer_resize("wayland", *surface)
    with native_keyboard_lifetime(gui, app, "wayland", duration_ms=120000):
        with VirtualPointer(app.env) as pointer:
            for topology in ("tab", "pane"):
                existing = set(starts())
                if topology == "tab":
                    app.wayland_chord(("ctrl", "shift"), "t")
                else:
                    app.wayland_chord(("alt", "shift"), "Right")
                gui.wait_until("new mouse capture PTY", lambda: len(set(starts()) - existing) == 1)
                other = (set(starts()) - existing).pop()

                def select(source):
                    if topology == "tab":
                        app.wayland_chord(("ctrl",), "1" if source else "2")
                    else:
                        app.wayland_chord(("alt",), "Left" if source else "Right")

                for source, pid in ((True, original_pid), (False, other)):
                    select(source)
                    app.wayland_chord((), "F7")
                    gui.wait_until("mouse tracking enabled for exact PTY", lambda: any(
                        r.get("event") == "drag_modes_enabled" and int(r["pid"]) == pid for r in records()))

                for kind in ("ordinary", "switch-release", "switch-motion"):
                    name = f"{topology}-{kind}"
                    select(True)
                    token(f"MOUSE_BEGIN_{name}", original_pid)
                    baseline = gui.input_streams(records())
                    lost = app.log_count("Window focus changed: false")
                    start = (200, 300)
                    end = (240, 340) if kind == "ordinary" else (800, 340)
                    pointer.move(*start, *surface)
                    pointer.set_left_button(True)
                    try:
                        gui.wait_until("native mouse press delivered to original PTY", lambda:
                            b"\x1b[<0;" in bytes(gui.input_streams(records()).get(original_pid, b""))[len(baseline.get(original_pid, b"")):])
                        if kind == "switch-release":
                            # Move outside the old pane before switching: release
                            # occurs inside the new pane, without post-switch motion.
                            pointer.move(*end, *surface)
                        if kind != "ordinary":
                            select(False)
                            token(f"MOUSE_SWITCH_{name}", other)
                        if kind != "switch-release":
                            pointer.move(*end, *surface)
                        gui.time.sleep(0.12)
                    finally:
                        pointer.set_left_button(False)
                    gui.time.sleep(0.12)
                    # Fence each PTY after native release, then inspect complete
                    # per-owner streams (a report can straddle read records).
                    for source, pid in ((True, original_pid), (False, other)):
                        select(source)
                        token(f"MOUSE_END_{name}_{pid}", pid)
                    streams = gui.input_streams(records())
                    delta = {pid: bytes(streams.get(pid, b""))[len(baseline.get(pid, b"")):] for pid in (original_pid, other)}
                    if app.log_count("Window focus changed: false") != lost:
                        raise gui.AcceptanceError("unrelated focus loss invalidated mouse ownership evidence")
                    # Moving before a tab switch is still inside the old full
                    # grid and may legitimately emit motion before cancellation.
                    try:
                        reports = verify_mouse_owners(delta, original_pid, other, switched=kind != "ordinary",
                                                      prior_motion=topology == "tab" and kind == "switch-release")
                    except ValueError as error:
                        raise gui.AcceptanceError(f"{name}: {error}") from error
                    cases.append({"case": name, "source_pid": original_pid, "destination_pid": other,
                                  "reports": reports, "unrelated_focus_losses": 0})
                for source in (True, False):
                    select(source)
                    app.wayland_chord((), "F3")
                if topology == "tab":
                    app.wayland_chord(("ctrl", "shift"), "w")
                else:
                    app.wayland_chord(("alt", "shift"), "w")
                gui.wait_until("mouse capture sibling closes", lambda: not gui.Path(f"/proc/{other}").exists())
                token(f"MOUSE_RESTORED_{topology}", original_pid)
    (app.run_dir / "mouse-ownership.json").write_text(json.dumps({
        "schema": 1, "backend": "wayland", "cases": cases,
        "coverage": "private Sway local tab/pane ownership; no X11, remote-domain, or desktop-policy acceptance",
    }, indent=2) + "\n", encoding="utf-8")
