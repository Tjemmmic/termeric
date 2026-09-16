"""Native palette editing, fenced against exact local PTY owners."""
import json

from scripts.gui_tab_placement import native_keyboard_lifetime


CASES = (
    ("middle-delete", "U+26X3", (("key", "Home"), ("key", "Right"), ("key", "Right"),
                               ("key", "Right"), ("key", "Right"),
                               ("key", "Delete"), ("text", "0"), ("key", "End"))),
    ("left-insert", "U+263", (("key", "Left"), ("text", "0"), ("key", "End"))),
    ("home-right", "U+603", (("key", "Home"), ("key", "Right"),
                            ("key", "Right"), ("text", "2"), ("key", "End"))),
    ("end-insert", "U+260", (("key", "Home"), ("key", "End"), ("text", "3"))),
    ("combining-backspace", "U+2603e\u0301", (("key", "BackSpace"),)),
    ("zwj-backspace", "U+2603👩\u200d💻", (("key", "BackSpace"),)),
    ("flag-backspace", "U+2603🇺🇸", (("key", "BackSpace"),)),
)


def verify_palette_delivery(streams, expected):
    """Join fragments before calling; refuse duplicates, leakage, and wrong PTYs."""
    actual = {pid: bytes(data) for pid, data in streams.items() if data}
    if len(expected) != 2 or any(not data for data in expected.values()):
        raise ValueError("palette evidence requires two exact fenced PTY owners")
    if actual != expected:
        raise ValueError(f"palette input escaped its modal or exact destination: {actual!r} != {expected!r}")


def run_palette_input_checks(gui, app, transcript, original_pid):
    """Use only the private seat and synthetic PTYs; never a provider or shell."""
    def records():
        return gui.read_records(transcript)

    def started():
        return {int(r["pid"]) for r in records() if r.get("event") == "started"}

    def token(value, owner):
        app.wayland_type(value)
        gui.wait_until(value, lambda: gui.pids_receiving_token(records(), value.encode()) == {owner})

    report = {
        "schema": 1, "backend": "wayland", "cases": [],
        "coverage": "private Sway native query editing and exact local-pane bytes; not native IME, screen-reader, or visual acceptance",
    }
    try:
        with native_keyboard_lifetime(gui, app, "wayland", duration_ms=120000):
            old = started()
            app.wayland_chord(("alt", "shift"), "Right")
            gui.wait_until("palette sibling fixture starts", lambda: len(started() - old) == 1)
            sibling = (started() - old).pop()

            def select(owner):
                app.wayland_chord(("alt",), "Left" if owner == original_pid else "Right")

            for owner in (original_pid, sibling):
                select(owner)
                app.wayland_chord((), "F3")
                token(f"PALETTE_READY_{owner}", owner)

            for index, (name, query, edits) in enumerate(CASES):
                owner = (original_pid, sibling)[index % 2]
                other = sibling if owner == original_pid else original_pid
                select(owner)
                token(f"PALETTE_BEGIN_{name}", owner)
                before = gui.input_streams(records())
                lost = app.log_count("Window focus changed: false")
                app.wayland_chord(("ctrl", "shift"), "p")
                app.wayland_type("QQQunicode")
                app.wayland_chord((), "Home")
                for _ in range(3):
                    app.wayland_chord((), "Delete")
                app.wayland_chord((), "End")
                app.wayland_chord((), "Return")
                app.wayland_type(query)
                for kind, value in edits:
                    if kind == "key":
                        app.wayland_chord((), value)
                    else:
                        app.wayland_type(value)
                app.wayland_chord((), "Return")
                fences = {pid: f"PALETTE_END_{name}_{pid}".encode() for pid in (owner, other)}
                # Each pane's post-selection fence drains fragmented writes
                # before we compare exact bytes, not just a substring match.
                for pid in (owner, other):
                    select(pid)
                    token(fences[pid].decode(), pid)
                after = gui.input_streams(records())
                delta = {pid: bytes(data)[len(before.get(pid, b"")):] for pid, data in after.items()}
                expected = {owner: "☃".encode() + fences[owner], other: fences[other]}
                case = {"case": name, "owner_pid": owner, "sibling_pid": other,
                        "expected_hex": {str(pid): data.hex() for pid, data in expected.items()},
                        "actual_hex": {str(pid): data.hex() for pid, data in delta.items() if data},
                        "unrelated_focus_losses": app.log_count("Window focus changed: false") - lost}
                report["cases"].append(case)
                verify_palette_delivery(delta, expected)
                if case["unrelated_focus_losses"] != 0:
                    raise gui.AcceptanceError("unrelated focus loss invalidated palette editing evidence")
            select(sibling)
            app.wayland_chord(("alt", "shift"), "w")
            gui.wait_until("palette sibling closes", lambda: not gui.Path(f"/proc/{sibling}").exists())
            token("PALETTE_ORIGINAL_RESTORED", original_pid)
    except Exception as error:
        report["error"] = str(error)
        raise
    finally:
        (app.run_dir / "palette-input.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
