"""Line I/O the fake agents share: one JSON message per line on stdio."""

import json
import os
import select
import sys

_buffer = b""


def send(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def read_line(block):
    """One input line; "" when none is ready without blocking; None at EOF.

    Reads the descriptor directly: mixing a buffered reader with select()
    would miss lines already sitting in the buffer.
    """
    global _buffer
    while b"\n" not in _buffer:
        if not block and not select.select([0], [], [], 0)[0]:
            return ""
        chunk = os.read(0, 65536)
        if not chunk:
            line, _buffer = _buffer, b""
            return line.decode() if line else None
        _buffer += chunk
    line, _, _buffer = _buffer.partition(b"\n")
    return line.decode()
