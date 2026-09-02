#!/usr/bin/env python3
"""Deterministic PTY workload used by the cross-terminal benchmark runner."""

from __future__ import annotations

import argparse
import fcntl
import json
import os
from pathlib import Path
import re
import select
import signal
import struct
import termios
import time
import tty


CURSOR_REPORT = re.compile(rb"\x1b\[(\d+);(\d+)R")


def write_all(fd: int, payload: bytes) -> None:
    view = memoryview(payload)
    while view:
        written = os.write(fd, view)
        view = view[written:]


def atomic_json(path: Path, value: dict[str, object]) -> None:
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, sort_keys=True) + "\n", encoding="utf-8")
    temporary.replace(path)


def terminal_size(fd: int) -> tuple[int, int]:
    rows, columns, _, _ = struct.unpack(
        "HHHH", fcntl.ioctl(fd, termios.TIOCGWINSZ, struct.pack("HHHH", 0, 0, 0, 0))
    )
    return columns, rows


def workload_chunk(sequence: int) -> bytes:
    line = (
        f"\x1b[38;2;{sequence % 255};{(sequence * 7) % 255};{(sequence * 13) % 255}m"
        f"frame={sequence:08d} fn main() {{ value + result; }} "
        "ASCII 0123456789 CJK=\u754c combining=e\u0301 emoji=\U0001f469\u200d\U0001f4bb "
        "\x1b[0m\r\n"
    )
    return line.encode("utf-8")


def emit_workload(fd: int, byte_count: int) -> tuple[int, int]:
    written = 0
    sequence = 0
    while written < byte_count:
        chunk = workload_chunk(sequence)
        remaining = byte_count - written
        if len(chunk) > remaining:
            chunk = (b"x" * max(0, remaining - 2) + b"\r\n")[:remaining]
        write_all(fd, chunk)
        written += len(chunk)
        sequence += 1
    return written, sequence


def read_cursor_report(fd: int, timeout_seconds: float) -> tuple[int, int]:
    deadline = time.monotonic() + timeout_seconds
    received = bytearray()
    while time.monotonic() < deadline:
        readable, _, _ = select.select([fd], [], [], max(0.0, deadline - time.monotonic()))
        if not readable:
            break
        received.extend(os.read(fd, 256))
        match = CURSOR_REPORT.search(received)
        if match:
            return int(match.group(2)), int(match.group(1))
    raise TimeoutError(f"terminal did not answer CSI 6 n; received {bytes(received)!r}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ready", type=Path, required=True)
    parser.add_argument("--gate", type=Path, required=True)
    parser.add_argument("--result", type=Path, required=True)
    parser.add_argument("--title", required=True)
    parser.add_argument("--payload-bytes", type=int, required=True)
    parser.add_argument("--timeout-seconds", type=float, default=20.0)
    args = parser.parse_args()
    if args.payload_bytes < 1 or args.timeout_seconds <= 0:
        parser.error("payload bytes and timeout must be positive")

    original = termios.tcgetattr(0)
    tty.setraw(0)
    try:
        write_all(1, f"\x1b]2;{args.title}\x07".encode())
        columns, rows = terminal_size(0)
        atomic_json(
            args.ready,
            {"pid": os.getpid(), "columns": columns, "rows": rows, "ready_ns": time.monotonic_ns()},
        )
        deadline = time.monotonic() + args.timeout_seconds
        while not args.gate.exists():
            if time.monotonic() >= deadline:
                raise TimeoutError("benchmark gate was not opened")
            time.sleep(0.005)

        columns, rows = terminal_size(0)
        started = time.monotonic_ns()
        written, lines = emit_workload(1, args.payload_bytes)
        write_all(1, b"\x1b[6n")
        cursor_column, cursor_row = read_cursor_report(0, args.timeout_seconds)
        elapsed_ms = (time.monotonic_ns() - started) / 1_000_000.0
        atomic_json(
            args.result,
            {
                "schema": 1,
                "pid": os.getpid(),
                "payload_bytes": written,
                "output_lines": lines,
                "columns": columns,
                "rows": rows,
                "cursor_column": cursor_column,
                "cursor_row": cursor_row,
                "pty_response_ms": elapsed_ms,
            },
        )
        signal.pause()
    finally:
        termios.tcsetattr(0, termios.TCSANOW, original)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
