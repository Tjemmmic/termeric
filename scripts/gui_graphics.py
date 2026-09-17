"""Native graphics-client acceptance in a private compositor; no providers."""
import hashlib
import json

from scripts.gui_tab_placement import native_keyboard_lifetime
from scripts.gui_wayland_capture import capture_private_output


PROTOCOLS = {
    "kitten-stream": "kitty",
    "kitten-unicode-placeholder": "kitty-unicode-placeholder",
    "kitten-file": "kitty-temporary-file",
    "kitten-memory": "kitty-shared-memory",
    "timg-kitty": "kitty",
    "img2sixel": "sixel",
    "timg-iterm2": "iterm2",
}
FIXED_IDS = {"kitten-stream": 1701, "kitten-unicode-placeholder": 1702,
             "kitten-file": 1703, "kitten-memory": 1704}


def graphics_clients_accepted(records, log_contains):
    clients = [row for row in records if row.get("event") == "graphics_client"]
    if len(clients) != len(PROTOCOLS) or {row.get("tool"): row.get("protocol") for row in clients} != PROTOCOLS:
        return False
    by_tool = {row["tool"]: row for row in clients}
    for tool, row in by_tool.items():
        if not isinstance(row.get("version"), str) or not row["version"].strip():
            return False
        if type(row.get("output_bytes")) is not int or row["output_bytes"] <= 0:
            return False
        if tool in FIXED_IDS and (type(row.get("image_id")) is not int or row["image_id"] != FIXED_IDS[tool]):
            return False
    timg_id = by_tool["timg-kitty"].get("image_id")
    if type(timg_id) is not int or not 0 < timg_id <= 0xffffffff or timg_id in {17, *FIXED_IDS.values()}:
        return False
    if any(by_tool[tool].get("cleanup_observed") is not True for tool in ("kitten-file", "kitten-memory")):
        return False
    messages = [f"Accepted Kitty image: id={image_id}," for image_id in (1701, 1703, 1704, timg_id)]
    messages += ["Accepted Kitty Unicode placeholder image: id=1702,",
                 "Accepted Sixel image:", "Accepted iTerm2 inline image:"]
    return all(log_contains(message) for message in messages)


def run_wayland_graphics(gui, binary, run_dir, env):
    run_dir.mkdir(parents=True, exist_ok=True)
    transcript = run_dir / "pty-transcript.jsonl"
    program, arguments = gui.fixture_command(transcript, external_graphics=True)
    app = gui.AppProcess(binary, run_dir, "wayland", program, arguments, env)
    report = {"schema": 1, "backend": "wayland", "clients": [],
              "coverage": "native client acceptance, local-media cleanup, and review-only capture; not pixel or real-desktop acceptance"}
    try:
        app.wait_ready("wayland")
        gui.wait_until("all seven native graphics clients accepted", lambda:
            graphics_clients_accepted(gui.read_records(transcript), app.log_contains), 30)
        gui.wait_until("native Kitty reply", lambda:
            b"\x1b_Gi=17;OK\x1b\\" in gui.transcript_bytes(transcript), 15)
        report["clients"] = [row for row in gui.read_records(transcript) if row.get("event") == "graphics_client"]
        starts = [row for row in gui.read_records(transcript) if row.get("event") == "started"]
        if len(starts) != 1:
            raise gui.AcceptanceError("graphics fixture requires exactly one PTY owner")
        # Capture before injecting keys or scrolling. This is retained for
        # review, not treated as proof that every accepted image is visible.
        gui.time.sleep(0.25)
        capture = run_dir / "graphics-clients.png"
        snapshot = capture_private_output(app.env)
        snapshot.save(capture)
        report["capture"] = {"path": capture.name, "size": list(snapshot.size),
                             "sha256": hashlib.sha256(capture.read_bytes()).hexdigest(),
                             "review_only": True}
        with native_keyboard_lifetime(gui, app, "wayland", duration_ms=30000):
            token = b"WAYLAND_GRAPHICS_READY"
            app.wayland_type(token.decode())
            gui.wait_until("input after graphics acceptance", lambda:
                gui.pids_receiving_token(gui.read_records(transcript), token) == {int(starts[0]["pid"])})
            app.wayland_chord(("alt",), "F4")
            if app.process.wait(timeout=8) != 0 or not app.log_contains("Close requested"):
                raise gui.AcceptanceError("graphics fixture failed native close")
        gui.assert_no_runtime_errors(app)
    except Exception as error:
        report["error"] = str(error)
        raise
    finally:
        (run_dir / "graphics-input.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        app.stop_after_failure()
        app.finish()
