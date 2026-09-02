#!/usr/bin/env python3
"""Deterministic PTY peer for Termeric's native GUI acceptance harness."""

from __future__ import annotations

import argparse
import base64
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


def render_external_graphics_clients(transcript: str) -> None:
    """Emit output from independent Sixel and iTerm2 client implementations."""
    image = os.path.join(
        os.path.dirname(transcript), f"graphics-client-{os.getpid()}.png"
    )
    with open(image, "wb") as fixture:
        fixture.write(
            base64.b64decode(
                "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgYPgPAAEDAQAIicLsAAAAAElFTkSuQmCC"
            )
        )

    for tool, command in (
        ("img2sixel", ["img2sixel", image]),
        ("timg-iterm2", ["timg", "-pi", "-g2x1", image]),
    ):
        result = subprocess.run(command, check=True, capture_output=True)
        if not result.stdout:
            raise RuntimeError(f"{tool} produced no terminal output")
        emit(result.stdout)
        write_record(
            transcript,
            "graphics_client",
            tool=tool,
            output_bytes=len(result.stdout),
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
