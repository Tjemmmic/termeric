#!/usr/bin/env python3
"""Deterministic PTY peer for Termeric's native GUI acceptance harness."""

from __future__ import annotations

import argparse
import base64
import binascii
import fcntl
import json
import os
import re
import select
import signal
import struct
import subprocess
import sys
import termios
import time
import tty
import zlib


def write_record(path: str, event: str, **fields: object) -> None:
    record = {
        "event": event,
        "pid": os.getpid(),
        "time_ns": time.monotonic_ns(),
        **fields,
    }
    payload = (json.dumps(record, sort_keys=True) + "\n").encode()
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
    try:
        os.write(descriptor, payload)
    finally:
        os.close(descriptor)


def winsize() -> tuple[int, int, int, int]:
    packed = fcntl.ioctl(sys.stdin.fileno(), termios.TIOCGWINSZ, b"\0" * 8)
    return struct.unpack("HHHH", packed)


def emit(data: bytes) -> None:
    os.write(sys.stdout.fileno(), data)


def render_fixture(instance: int) -> None:
    tiny_rgba = "/wAA/wD/AP8AAP////8A/w=="
    lines = [
        b"\x1b[1;33mTERMERIC GUI ACCEPTANCE\x1b[0m",
        b"\x1b[31mred\x1b[0m \x1b[3mitalic\x1b[0m \x1b[4munderline\x1b[0m",
        "Unicode: cafe\u0301 \u754c \U0001f642 \U0001f469\u200d\U0001f4bb ligature: != ->".encode(),
        b"LINK \x1b]8;;https://example.test/gui\x07example.test/gui\x1b]8;;\x07",
        b"COPY_TOKEN_17",
        f"fixture instance {instance}".encode(),
    ]
    emit(b"\x1b[2J\x1b[H" + b"\r\n".join(lines) + b"\r\n")
    emit(
        f"\x1b_Ga=T,f=32,s=2,v=2,i=17,q=0,c=2,r=2,C=1;{tiny_rgba}\x1b\\".encode()
    )
    emit(b"\x1b]2;Termeric GUI Acceptance\x07")


def render_scrollback_fixture(pid: int) -> None:
    """Finite, owner-tagged history for native scrollbar isolation checks."""
    lines = [f"SCROLL_{pid}_{row:03d}" for row in range(100)]
    lines.append(f"SCROLL_END_{pid}")
    emit(("\r\n" + "\r\n".join(lines)).encode("ascii"))


def png_chunk(kind: bytes, data: bytes) -> bytes:
    payload = kind + data
    return (
        struct.pack(">I", len(data))
        + payload
        + struct.pack(">I", binascii.crc32(payload))
    )


def write_graphics_fixture(path: str) -> None:
    """Write a bounded multi-row RGBA fixture without an image-library dependency."""
    width = 64
    height = 64
    scanlines = bytearray()
    for row in range(height):
        scanlines.append(0)
        for col in range(width):
            scanlines.extend((col * 4, row * 4, (col ^ row) * 4, 255))
    payload = b"\x89PNG\r\n\x1a\n"
    payload += png_chunk(
        b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    )
    payload += png_chunk(b"IDAT", zlib.compress(bytes(scanlines), 9))
    payload += png_chunk(b"IEND", b"")
    with open(path, "wb") as fixture:
        fixture.write(payload)


def command_version(command: list[str]) -> str:
    result = subprocess.run(command, check=True, capture_output=True, timeout=5)
    output = (result.stdout + result.stderr).decode("utf-8", "replace")
    first_line = next((line.strip() for line in output.splitlines() if line.strip()), "")
    if not first_line:
        raise RuntimeError(f"{' '.join(command)} produced no version output")
    return "".join(character if character.isprintable() else "�" for character in first_line)[
        :256
    ]


def kitty_image_id(payload: bytes) -> int:
    """Extract the first explicit nonzero Kitty image ID from client output."""
    offset = 0
    while True:
        start = payload.find(b"\x1b_G", offset)
        if start < 0:
            break
        separator = payload.find(b";", start + 3)
        if separator < 0:
            break
        for field in payload[start + 3 : separator].split(b","):
            if not field.startswith(b"i="):
                continue
            encoded = field[2:]
            if encoded.isdigit():
                image_id = int(encoded)
                if 0 < image_id <= 0xFFFFFFFF:
                    return image_id
        offset = separator + 1
    raise RuntimeError("Kitty client output contains no explicit nonzero image ID")


def kitty_media_target(payload: bytes, expected_medium: bytes) -> bytes:
    """Extract one local-media target from current Kitten output."""
    if expected_medium not in (b"t", b"s"):
        raise ValueError(
            "expected_medium must be temporary-file (t) or shared-memory (s)"
        )
    offset = 0
    while True:
        start = payload.find(b"\x1b_G", offset)
        if start < 0:
            break
        separator = payload.find(b";", start + 3)
        terminator = payload.find(b"\x1b\\", separator + 1)
        if separator < 0 or terminator < 0:
            break
        controls = {}
        for field in payload[start + 3 : separator].split(b","):
            key, delimiter, value = field.partition(b"=")
            if delimiter and key not in controls:
                controls[key] = value
        if controls.get(b"t") == expected_medium:
            encoded = payload[separator + 1 : terminator]
            if len(encoded) % 4 == 1:
                raise RuntimeError("Kitty local-media target has invalid base64 data")
            try:
                target = base64.b64decode(
                    encoded + b"=" * (-len(encoded) % 4), validate=True
                )
            except binascii.Error as error:
                raise RuntimeError(
                    "Kitty local-media target has invalid base64 data"
                ) from error
            if not target or b"\0" in target:
                raise RuntimeError("Kitty local-media target is empty or contains NUL")
            return target
        offset = terminator + 2
    medium = expected_medium.decode("ascii")
    raise RuntimeError(f"Kitty client output contains no t={medium} media target")


def media_target_path(target: bytes, medium: bytes) -> bytes:
    """Map a validated current-Kitten target to its Linux filesystem object."""
    if medium == b"t":
        if not os.path.isabs(target) or b"tty-graphics-protocol" not in target:
            raise RuntimeError("Kitty temporary-file target is not a safe client path")
        return target
    if medium == b"s":
        name = target.removeprefix(b"/")
        if not name or b"/" in name or len(name) > 255:
            raise RuntimeError("Kitty shared-memory target is not one POSIX name")
        return os.path.join(b"/dev/shm", name)
    raise ValueError("medium must be temporary-file (t) or shared-memory (s)")


def wait_for_media_cleanup(target: bytes, medium: bytes, timeout: float = 5) -> None:
    """Require Termeric to consume and remove a client-owned local-media object."""
    path = media_target_path(target, medium)
    deadline = time.monotonic() + timeout
    while os.path.lexists(path):
        if time.monotonic() >= deadline:
            name = os.path.basename(path).decode("utf-8", "replace")
            raise RuntimeError(f"Kitty {medium.decode()} media target was not removed: {name}")
        time.sleep(0.01)


def require_media_target(target: bytes, medium: bytes) -> None:
    """Require the client-owned object to exist before PTY delivery."""
    path = media_target_path(target, medium)
    if not os.path.lexists(path):
        name = os.path.basename(path).decode("utf-8", "replace")
        raise RuntimeError(f"Kitty {medium.decode()} media target is missing: {name}")


def graphics_client_bands(geometry, client_tools):
    """Non-overlapping vertical review slots, independent of client CR output.

    Keep the header and a final cursor/newline row outside the image bands.
    Kitty/img2sixel receive the unscaled 64px fixture; timg's 2x1 fit is given
    two rows, also allowing its protocol-specific cursor advancement.
    """
    rows, cols, pixel_width, pixel_height = geometry
    if any(type(value) is not int or not 1 <= value <= 65535 for value in geometry):
        raise ValueError("graphics review requires positive bounded terminal geometry")
    cell_width, cell_height = pixel_width // cols, pixel_height // rows
    if cell_width < 1 or cell_height < 1 or (cols - 1) * cell_width < 64:
        raise ValueError("graphics review requires pixel cell metrics and space for a 64px image")
    small = {"timg-kitty", "timg-iterm2", "timg-sixel"}
    large = {"kitten-stream", "kitten-unicode-placeholder", "kitten-file", "kitten-memory", "img2sixel"}
    bands = {}
    row = 8
    for tool in client_tools:
        if tool in bands or tool not in small | large:
            raise ValueError("graphics review requires distinct known clients")
        height = 2 if tool in small else (64 + cell_height - 1) // cell_height
        bands[tool] = (row, 2, height)
        row += height
    if row > rows:
        raise ValueError(f"graphics review needs at least {row} rows at this cell height; got {rows}")
    return bands


def render_external_graphics_clients(
    transcript: str,
    terminal_geometry: tuple[int, int, int, int] | None = None,
    *,
    drain_output: bool = False,
    hold_failure=None,
) -> None:
    """Emit current Kitty, independent Sixel, and iTerm2 client output."""
    if hold_failure is not None and not drain_output:
        raise ValueError("graphics failure hold requires parser drain")
    image = os.path.join(
        os.path.dirname(transcript), f"graphics-client-{os.getpid()}.png"
    )
    write_graphics_fixture(image)
    rows, cols, pixel_width, pixel_height = terminal_geometry or winsize()
    window_size = f"{cols},{rows},{pixel_width},{pixel_height}"
    versions: dict[str, str] = {}

    with open(image, "rb") as fixture:
        image_bytes = fixture.read()
    clients = (
        (
            "kitten-stream",
            "kitten",
            "kitty",
            [
                "kitten",
                "icat",
                "--stdin=no",
                "--transfer-mode=stream",
                f"--use-window-size={window_size}",
                "--fit=none",
                "--align=left",
                "--image-id=1701",
                image,
            ],
            None,
        ),
        (
            "kitten-unicode-placeholder",
            "kitten",
            "kitty-unicode-placeholder",
            [
                "kitten",
                "icat",
                "--stdin=no",
                "--unicode-placeholder",
                "--transfer-mode=stream",
                f"--use-window-size={window_size}",
                "--fit=none",
                "--align=left",
                "--image-id=1702",
                image,
            ],
            None,
        ),
        (
            "kitten-file",
            "kitten",
            "kitty-temporary-file",
            [
                "kitten",
                "icat",
                "--stdin=yes",
                "--transfer-mode=file",
                f"--use-window-size={window_size}",
                "--fit=none",
                "--align=left",
                "--image-id=1703",
            ],
            image_bytes,
        ),
        (
            "kitten-memory",
            "kitten",
            "kitty-shared-memory",
            [
                "kitten",
                "icat",
                "--stdin=no",
                "--transfer-mode=memory",
                f"--use-window-size={window_size}",
                "--fit=none",
                "--align=left",
                "--image-id=1704",
                image,
            ],
            None,
        ),
        (
            "timg-kitty",
            "timg",
            "kitty",
            ["timg", "-I", "-pk", "--compress=0", "-g2x1", image],
            None,
        ),
        (
            "img2sixel",
            "img2sixel",
            "sixel",
            ["img2sixel", image],
            None,
        ),
        (
            "timg-iterm2",
            "timg",
            "iterm2",
            ["timg", "-pi", "-g2x1", image],
            None,
        ),
        (
            "timg-sixel",
            "timg",
            "sixel",
            ["timg", "-I", "-ps", "--frames=1", "-g2x1", image],
            None,
        ),
    )
    first_error = None
    failed = 0
    bands = graphics_client_bands((rows, cols, pixel_width, pixel_height), [client[0] for client in clients])
    for tool, version_key, protocol, command, stdin_data in clients:
        cursor_row, cursor_col, reserved_rows = bands[tool]
        result = None
        stage = "version"
        try:
            if version_key not in versions:
                versions[version_key] = command_version([version_key, "--version"])
            stage = "execute"
            result = subprocess.run(
                command,
                input=stdin_data,
                check=True,
                capture_output=True,
                timeout=15,
            )
            stage = "validate-output"
            if not result.stdout:
                raise RuntimeError(f"{tool} produced no terminal output")
            protocol_fields: dict[str, object] = {}
            if protocol.startswith("kitty"):
                protocol_fields["image_id"] = kitty_image_id(result.stdout)
            elif protocol == "sixel" and not re.search(rb"\x1bP[0-9;]*q[^\x1b]*\x1b\\", result.stdout):
                raise RuntimeError(f"{tool} produced no complete Sixel frame")
            elif protocol == "iterm2" and not re.search(rb"\x1b\]1337;File=[^\x07\x1b]*:[A-Za-z0-9+/=\r\n]+(?:\x07|\x1b\\)", result.stdout):
                raise RuntimeError(f"{tool} produced no complete iTerm2 frame")
            target = None
            medium = None
            if protocol in ("kitty-temporary-file", "kitty-shared-memory"):
                medium = b"t" if protocol == "kitty-temporary-file" else b"s"
                target = kitty_media_target(result.stdout, medium)
                require_media_target(target, medium)
            stage = "emit"
            emit(f"\x1b[{cursor_row};{cursor_col}H".encode() + result.stdout)
            if target is not None and medium is not None:
                stage = "cleanup"
                wait_for_media_cleanup(target, medium)
                protocol_fields["cleanup_observed"] = True
                protocol_fields["cleanup_target"] = os.path.basename(target).decode(
                    "utf-8", "replace"
                )[:256]
        except Exception as error:
            # Keep subprocess diagnostics separate from protocol bytes. Never
            # emit subprocess-error output into the terminal or call a failed
            # transfer successful. Only pre-emission failures are independent:
            # partial writes or unconfirmed media cleanup abort the run.
            continuing = stage in {"version", "execute", "validate-output"}
            diagnostic = result if result is not None else error
            stderr = getattr(diagnostic, "stderr", None) or b""
            write_record(
                transcript, "graphics_client_error", tool=tool, protocol=protocol,
                version=versions.get(version_key), stage=stage,
                error_type=type(error).__name__, error=graphics_diagnostic(str(error)),
                returncode=getattr(diagnostic, "returncode", None),
                cursor_row=cursor_row, cursor_col=cursor_col, reserved_rows=reserved_rows,
                placement_policy="vertical-bands-v1",
                output_bytes=len(getattr(diagnostic, "stdout", None) or b""),
                stderr=graphics_diagnostic(stderr),
                stderr_bytes=len(stderr), stderr_truncated=len(stderr) > 1024,
                continuing=continuing,
            )
            if not continuing:
                raise
            failed += 1
            if first_error is None:
                first_error = error
            continue
        write_record(
            transcript,
            "graphics_client",
            tool=tool,
            version=versions[version_key],
            protocol=protocol,
            output_bytes=len(result.stdout),
            cursor_row=cursor_row,
            cursor_col=cursor_col,
            reserved_rows=reserved_rows,
            placement_policy="vertical-bands-v1",
            fixture_width=64,
            fixture_height=64,
            terminal_rows=rows,
            terminal_cols=cols,
            terminal_pixel_width=pixel_width,
            terminal_pixel_height=pixel_height,
            **protocol_fields,
        )
    if drain_output:
        wait_for_graphics_drain(transcript)
    if first_error is not None and hold_failure is not None:
        write_record(transcript, "graphics_failure_hold", timeout_seconds=10)
    write_record(transcript, "graphics_clients_complete", attempted=len(clients), failed=failed,
                 drained=drain_output)
    if first_error is not None:
        try:
            if hold_failure is not None:
                hold_failure()
        finally:
            raise first_error


def hold_graphics_failure(keep_running) -> None:
    """Keep the failed fixture's terminal frame alive for at most ten seconds.

    The private harness captures without injecting input, then terminates the
    app. The main fixture's signal handler can end this hold promptly too.
    """
    deadline = time.monotonic() + 10
    while keep_running() and time.monotonic() < deadline:
        time.sleep(0.05)


def wait_for_graphics_drain(transcript: str, timeout: float = 5.0) -> None:
    """Fence parser processing before publishing completion, including failures.

    This controlled fixture sends no earlier DSR 5 queries. Preserve preceding
    replies (including the initial Kitty probe) in the normal input transcript.
    A status reply proves parser progress, not GPU presentation or pixels.
    """
    emit(b"\x1b[5n")
    deadline = time.monotonic() + timeout
    received = bytearray()
    descriptor = sys.stdin.fileno()
    while len(received) < 64 * 1024:
        remaining = deadline - time.monotonic()
        if remaining <= 0 or not select.select([descriptor], [], [], remaining)[0]:
            raise RuntimeError("graphics parser-drain reply timed out")
        data = os.read(descriptor, min(4096, 64 * 1024 - len(received)))
        if not data:
            raise RuntimeError("graphics parser-drain input closed")
        write_record(transcript, "input", base64=base64.b64encode(data).decode(),
                     hex=data.hex(), text=data.decode("utf-8", "replace"))
        received.extend(data)
        if b"\x1b[0n" in received:
            return
    raise RuntimeError("graphics parser-drain reply exceeded input limit")


def graphics_diagnostic(value: str | bytes) -> str:
    """Bound retained diagnostics and neutralize terminal control characters."""
    if isinstance(value, bytes):
        value = value[:1024].decode("utf-8", "replace")
    return "".join(character if character.isprintable() else "�" for character in value[:1024])


def render_soak_frame(iteration: int) -> int:
    lines = []
    for row in range(64):
        color = 16 + ((iteration * 17 + row * 7) % 216)
        lines.append(
            f"\x1b[38;5;{color}msoak {iteration:06d}/{row:02d}\x1b[0m "
            "ASCII payload cafe\u0301 \u754c \U0001f469\u200d\U0001f4bb "
            + ("x" * 80)
            + "\r\n"
        )
    if iteration % 16 == 0:
        lines.append(
            "\x1b_Ga=T,f=32,s=1,v=1,i=91,q=0,c=2,r=1,C=1;"
            "/wAA/w==\x1b\\"
        )
    payload = "".join(lines).encode()
    emit(payload)
    return len(payload)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--transcript", required=True)
    parser.add_argument("--instance", type=int, default=0)
    parser.add_argument("--exit-after", type=float)
    parser.add_argument("--soak-output", action="store_true")
    parser.add_argument("--skip-external-graphics", action="store_true")
    parser.add_argument("--hold-graphics-failure", action="store_true")
    parser.add_argument("--scrollback-fixture", action="store_true")
    parser.add_argument("--background-fixture", action="store_true")
    args = parser.parse_args()

    old_settings = termios.tcgetattr(sys.stdin.fileno())
    tty.setraw(sys.stdin.fileno())
    running = True

    def stop(_signal: int, _frame: object) -> None:
        nonlocal running
        running = False

    def record_winsize(_signal: int, _frame: object) -> None:
        rows, cols, pixel_width, pixel_height = winsize()
        write_record(
            args.transcript,
            "winsize",
            rows=rows,
            cols=cols,
            pixel_width=pixel_width,
            pixel_height=pixel_height,
        )

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    signal.signal(signal.SIGWINCH, record_winsize)
    rows, cols, pixel_width, pixel_height = winsize()
    write_record(
        args.transcript,
        "started",
        rows=rows,
        cols=cols,
        pixel_width=pixel_width,
        pixel_height=pixel_height,
        term=os.environ.get("TERM"),
        colorterm=os.environ.get("COLORTERM"),
        terminfo_dirs=os.environ.get("TERMINFO_DIRS"),
        cwd=os.getcwd(),
    )
    if args.background_fixture:
        # Keep background sample locations free of text while preserving an
        # explicit cell background for native layer-order checks.
        emit(b"\x1b[2J\x1b[H\x1b[?25l\x1b[5;5H\x1b[48;2;255;0;255m" + b" " * 20 + b"\x1b[0m")
        emit(b"\x1b]2;Termeric GUI Acceptance\x07")
    else:
        render_fixture(args.instance)
    if args.scrollback_fixture:
        render_scrollback_fixture(os.getpid())
    if not args.skip_external_graphics:
        render_external_graphics_clients(
            args.transcript, drain_output=True,
            hold_failure=(lambda: hold_graphics_failure(lambda: running))
            if args.hold_graphics_failure else None,
        )
    exit_deadline = (
        time.monotonic() + args.exit_after if args.exit_after is not None else None
    )
    soak_iteration = 0
    next_soak_output = time.monotonic()

    try:
        while running:
            now = time.monotonic()
            if exit_deadline is not None and now >= exit_deadline:
                break
            if args.soak_output and now >= next_soak_output:
                emitted = render_soak_frame(soak_iteration)
                if soak_iteration % 10 == 0:
                    write_record(
                        args.transcript,
                        "soak_progress",
                        iteration=soak_iteration,
                        output_bytes=emitted,
                    )
                soak_iteration += 1
                next_soak_output = now + 0.1
            timeout = 0.05 if args.soak_output else 0.2
            readable, _, _ = select.select([sys.stdin], [], [], timeout)
            if not readable:
                continue
            data = os.read(sys.stdin.fileno(), 4096)
            if not data:
                break
            write_record(
                args.transcript,
                "input",
                base64=base64.b64encode(data).decode(),
                hex=data.hex(),
                text=data.decode("utf-8", "replace"),
            )
            if b"\x1bOQ" in data:  # F2: enable application input modes.
                emit(b"\x1b[?1000h\x1b[?1006h\x1b[?1004h\x1b[?2004h")
                write_record(args.transcript, "modes_enabled")
            if b"\x1bOR" in data:  # F3: restore ordinary selection input.
                emit(b"\x1b[?1000l\x1b[?1002l\x1b[?1006l\x1b[?1004l\x1b[?2004l")
                write_record(args.transcript, "modes_disabled")
            if b"\x1b[18~" in data:  # F7: isolated held-button mouse routing.
                emit(b"\x1b[?1002h\x1b[?1006h")
                write_record(args.transcript, "drag_modes_enabled")
            if b"\x1bOS" in data:  # F4: exercise native desktop bridges.
                emit(b"\x07")
                emit(b"\x1b]777;notify;Termeric;Desktop integration acceptance\x1b\\")
                emit(b"\x1b]52;c;T1NDNTJfVE9LRU5fMjY=\x1b\\")
                emit(b"\x1b]52;c;?\x1b\\")
                write_record(args.transcript, "desktop_integrations_requested")
    finally:
        emit(b"\x1b[?1000l\x1b[?1002l\x1b[?1006l\x1b[?1004l\x1b[?2004l")
        termios.tcsetattr(sys.stdin.fileno(), termios.TCSANOW, old_settings)
        write_record(args.transcript, "stopped")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
