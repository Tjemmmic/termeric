"""Native top/left tab geometry and keyboard routing; no external clients."""
from __future__ import annotations

import json
from pathlib import Path


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
