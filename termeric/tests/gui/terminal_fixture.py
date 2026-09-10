#!/usr/bin/env python3
"""Deterministic PTY peer for Termeric's native GUI acceptance harness."""

from __future__ import annotations

import argparse
import base64
import binascii
import fcntl
import json
import os
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


def render_external_graphics_clients(
    transcript: str,
    terminal_geometry: tuple[int, int, int, int] | None = None,
) -> None:
    """Emit current Kitty, independent Sixel, and iTerm2 client output."""
    image = os.path.join(
        os.path.dirname(transcript), f"graphics-client-{os.getpid()}.png"
    )
    write_graphics_fixture(image)
    rows, cols, pixel_width, pixel_height = terminal_geometry or winsize()
    window_size = f"{cols},{rows},{pixel_width},{pixel_height}"
    versions = {
        "kitten": command_version(["kitten", "--version"]),
        "img2sixel": command_version(["img2sixel", "--version"]),
        "timg": command_version(["timg", "--version"]),
    }

    for tool, version_key, protocol, cursor_row, command in (
        (
            "kitten-stream",
            "kitten",
            "kitty",
            8,
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
        ),
        (
            "kitten-unicode-placeholder",
            "kitten",
            "kitty-unicode-placeholder",
            16,
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
        ),
        (
            "timg-kitty",
            "timg",
            "kitty",
            24,
            ["timg", "-I", "-pk", "--compress=0", "-g2x1", image],
        ),
        ("img2sixel", "img2sixel", "sixel", 27, ["img2sixel", image]),
        (
            "timg-iterm2",
            "timg",
            "iterm2",
            32,
            ["timg", "-pi", "-g2x1", image],
        ),
    ):
        result = subprocess.run(command, check=True, capture_output=True, timeout=15)
        if not result.stdout:
            raise RuntimeError(f"{tool} produced no terminal output")
        emit(f"\x1b[{cursor_row};2H".encode() + result.stdout)
        protocol_fields: dict[str, int] = {}
        if protocol.startswith("kitty"):
            protocol_fields["image_id"] = kitty_image_id(result.stdout)
        write_record(
            transcript,
            "graphics_client",
            tool=tool,
            version=versions[version_key],
            protocol=protocol,
            output_bytes=len(result.stdout),
            cursor_row=cursor_row,
            cursor_col=2,
            fixture_width=64,
            fixture_height=64,
            terminal_rows=rows,
            terminal_cols=cols,
            terminal_pixel_width=pixel_width,
            terminal_pixel_height=pixel_height,
            **protocol_fields,
        )


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
    render_fixture(args.instance)
    if not args.skip_external_graphics:
        render_external_graphics_clients(args.transcript)
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
                emit(b"\x1b[?1000l\x1b[?1006l\x1b[?1004l\x1b[?2004l")
                write_record(args.transcript, "modes_disabled")
            if b"\x1bOS" in data:  # F4: exercise native desktop bridges.
                emit(b"\x07")
                emit(b"\x1b]777;notify;Termeric;Desktop integration acceptance\x1b\\")
                emit(b"\x1b]52;c;T1NDNTJfVE9LRU5fMjY=\x1b\\")
                emit(b"\x1b]52;c;?\x1b\\")
                write_record(args.transcript, "desktop_integrations_requested")
    finally:
        emit(b"\x1b[?1000l\x1b[?1006l\x1b[?1004l\x1b[?2004l")
        termios.tcsetattr(sys.stdin.fileno(), termios.TCSANOW, old_settings)
        write_record(args.transcript, "stopped")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
