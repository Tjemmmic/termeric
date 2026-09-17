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
    "timg-sixel": "sixel",
}
FIXED_IDS = {"kitten-stream": 1701, "kitten-unicode-placeholder": 1702,
             "kitten-file": 1703, "kitten-memory": 1704}
# Eight bounded version/execute pairs, two cleanup waits, and delivery margin.
CLIENT_TIMEOUT = 200


def clients_complete(records):
    completed = [row for row in records if row.get("event") == "graphics_clients_complete"]
    if not completed:
        return False
    outcomes = [row for row in records if row.get("event") in {"graphics_client", "graphics_client_error"}]
    failed = sum(row["event"] == "graphics_client_error" for row in outcomes)
    if (len(completed) != 1 or len(outcomes) != len(PROTOCOLS)
            or [(row.get("tool"), row.get("protocol")) for row in outcomes] != list(PROTOCOLS.items())
            or type(completed[0].get("attempted")) is not int or completed[0]["attempted"] != len(PROTOCOLS)
            or type(completed[0].get("failed")) is not int or completed[0]["failed"] != failed
            or completed[0].get("drained") is not True
            or records.index(completed[0]) <= max(records.index(row) for row in outcomes)):
        raise ValueError("invalid graphics client completion record")
    return True


def graphics_clients_accepted(records, log_contains, log_count):
    try:
        if not clients_complete(records):
            return False
    except ValueError:
        return False
    if any(row.get("event") == "graphics_client_error" for row in records):
        return False
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
    # Both independent static Sixel producers must reach the terminal parser.
    # A single acceptance line cannot stand in for both emitted streams.
    sixel_count = log_count("Accepted Sixel image:")
    return type(sixel_count) is int and sixel_count == 2 and all(log_contains(message) for message in messages)


def graphics_clients_ready(records, log_contains, log_count):
    """Collect independent pre-emission failures; abort unsafe/legacy failures."""
    errors = [row for row in records if row.get("event") == "graphics_client_error"]
    complete = clients_complete(records)
    for row in errors:
        if complete or row.get("continuing") is not True or row.get("stage") not in {"version", "execute", "validate-output"}:
            raise ValueError(f"graphics client {row.get('tool')} failed during "
                             f"{row.get('stage')}: {row.get('error')}")
    return graphics_clients_accepted(records, log_contains, log_count)


def failed_capture_ready(records):
    """Only drained, single-owner runs with independent pre-emission failures.

    This admits diagnostic capture, never client or visual acceptance.
    Incomplete/unsafe transfers must still abort without another capture wait.
    """
    try:
        if not clients_complete(records):
            return False
    except ValueError:
        return False
    starts = [row for row in records if row.get("event") == "started"]
    errors = [row for row in records if row.get("event") == "graphics_client_error"]
    return (len(starts) == 1 and type(starts[0].get("pid")) is int
            and starts[0]["pid"] > 0 and bool(errors)
            and all(row.get("continuing") is True
                    and row.get("stage") in {"version", "execute", "validate-output"}
                    for row in errors))


def capture_review_output(run_dir, env, name):
    capture = run_dir / name
    snapshot = capture_private_output(env)
    snapshot.save(capture)
    return {"path": capture.name, "size": list(snapshot.size),
            "sha256": hashlib.sha256(capture.read_bytes()).hexdigest(),
            "review_only": True}


def run_wayland_graphics(gui, binary, run_dir, env):
    run_dir.mkdir(parents=True, exist_ok=True)
    transcript = run_dir / "pty-transcript.jsonl"
    program, arguments = gui.fixture_command(transcript, external_graphics=True)
    app = gui.AppProcess(binary, run_dir, "wayland", program, arguments, env)
    report = {"schema": 4, "backend": "wayland", "clients": [],
              "coverage": "native client acceptance, local-media cleanup, and review-only capture; not pixel or real-desktop acceptance"}
    try:
        app.wait_ready("wayland")
        gui.wait_until("all eight native graphics clients accepted", lambda:
            graphics_clients_ready(gui.read_records(transcript), app.log_contains, app.log_count), CLIENT_TIMEOUT)
        gui.wait_until("native Kitty reply", lambda:
            b"\x1b_Gi=17;OK\x1b\\" in gui.transcript_bytes(transcript), 15)
        starts = [row for row in gui.read_records(transcript) if row.get("event") == "started"]
        if len(starts) != 1:
            raise gui.AcceptanceError("graphics fixture requires exactly one PTY owner")
        # Capture before injecting keys or scrolling. This is retained for
        # review, not treated as proof that every accepted image is visible.
        gui.time.sleep(0.25)
        report["capture"] = capture_review_output(run_dir, app.env, "graphics-clients.png")
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
        try:
            if failed_capture_ready(gui.read_records(transcript)) and app.process.poll() is None:
                # No keys/scrolling are injected on this path. Preserve the
                # pre-input private-compositor frame for review only; the
                # original client failure remains the scenario outcome.
                gui.time.sleep(0.25)
                report["diagnostic_capture"] = {
                    **capture_review_output(run_dir, app.env, "graphics-clients-failed.png"),
                    "failed_run": True,
                    "acceptance": False,
                }
        except Exception as capture_error:
            # A secondary capture failure must not replace the client error
            # or prevent the existing failure-report/cleanup path.
            report["diagnostic_capture_error"] = str(capture_error)
        raise
    finally:
        try:
            records = gui.read_records(transcript)
            report["clients"] = [row for row in records if row.get("event") == "graphics_client"]
            report["client_errors"] = [row for row in records if row.get("event") == "graphics_client_error"]
            report["completion"] = [row for row in records if row.get("event") == "graphics_clients_complete"]
            (run_dir / "graphics-input.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        finally:
            try:
                app.stop_after_failure()
            finally:
                app.finish()
