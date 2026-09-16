"""Bounded v1 screencopy for the harness's private, single-output Sway only.

Wire contract: https://raw.githubusercontent.com/swaywm/wlr-protocols/master/unstable/wlr-screencopy-unstable-v1.xml
No portals, desktop capture helpers, input devices, or user's compositor.
"""
import array
import mmap
import os
import socket
import struct
import sys
import time

from scripts.gui_wayland_pointer import private_display_path, wire_message, wire_string

MANAGER = b"zwlr_screencopy_manager_v1\0"
MAX_BYTES = 64 * 1024 * 1024


def buffer_layout(payload):
    if len(payload) != 16:
        raise RuntimeError("invalid screencopy buffer event")
    fmt, width, height, stride = struct.unpack("=IIII", payload)
    if (fmt not in (0, 1) or not 1 <= width <= 4096 or not 1 <= height <= 4096
            or stride < width * 4 or stride % 4 or stride * height > MAX_BYTES):
        raise RuntimeError("unsupported or oversized screencopy buffer")
    return fmt, width, height, stride


class PrivateCapture:
    def __init__(self, env):
        path = private_display_path(env)
        if sys.byteorder != "little":
            raise RuntimeError("private capture currently requires little-endian Linux")
        self.socket = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.deadline = time.monotonic() + 5
        try:
            self.socket.settimeout(5)
            self.socket.connect(str(path))
        except BaseException:
            self.socket.close()
            raise

    def send(self, obj, opcode, *values):
        self.socket.sendall(wire_message(obj, opcode, struct.pack("=" + "I" * len(values), *values)))

    def read_exact(self, size):
        data = bytearray()
        while len(data) < size:
            remaining = self.deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError("private screencopy deadline exceeded")
            self.socket.settimeout(remaining)
            chunk = self.socket.recv(size - len(data))
            if not chunk:
                raise RuntimeError("private screencopy compositor disconnected")
            data.extend(chunk)
        return bytes(data)

    def event(self):
        obj, word = struct.unpack("=II", self.read_exact(8))
        size, opcode = word >> 16, word & 65535
        if size < 8 or size % 4:
            raise RuntimeError("malformed screencopy wire event")
        payload = self.read_exact(size - 8)
        if obj == 1 and opcode == 0:
            raise RuntimeError(f"private screencopy protocol error: {payload!r}")
        return obj, opcode, payload

    def capture(self):
        from PIL import Image
        self.send(1, 1, 2)  # registry
        self.send(1, 0, 3)  # callback; subsequent object IDs stay contiguous
        globals_ = {}
        while True:
            obj, opcode, payload = self.event()
            if obj == 3 and opcode == 0:
                break
            if obj == 2 and opcode == 0:
                if len(payload) < 12:
                    raise RuntimeError("truncated screencopy registry event")
                name, length = struct.unpack_from("=II", payload)
                if not 1 <= length <= 256 or 8 + ((length + 3) & ~3) + 4 != len(payload):
                    raise RuntimeError("invalid screencopy registry string")
                interface = payload[8:8 + length]
                if interface in (b"wl_shm\0", b"wl_output\0", MANAGER):
                    if interface in globals_:
                        raise RuntimeError("private capture requires exactly one output/manager")
                    globals_[interface] = name
        for obj, interface in ((4, b"wl_shm\0"), (5, b"wl_output\0"), (6, MANAGER)):
            if interface not in globals_:
                raise RuntimeError(f"private compositor lacks {interface!r}")
            self.socket.sendall(wire_message(2, 0, struct.pack("=I", globals_[interface])
                + wire_string(interface) + struct.pack("=II", 1, obj)))
        self.send(6, 0, 7, 0, 5)  # capture next output frame, without cursor
        while True:
            obj, opcode, payload = self.event()
            if obj == 7:
                if opcode != 0:
                    raise RuntimeError("screencopy failed before buffer admission")
                fmt, width, height, stride = buffer_layout(payload)
                break
        fd = os.memfd_create("termeric-private-capture", os.MFD_CLOEXEC)
        try:
            os.ftruncate(fd, stride * height)
            with mmap.mmap(fd, stride * height) as memory:
                message = wire_message(4, 0, struct.pack("=II", 8, stride * height))
                sent = self.socket.sendmsg([message], [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array("i", [fd]))])
                if sent != len(message):
                    raise RuntimeError("partial screencopy pool creation")
                self.send(8, 0, 9, 0, width, height, stride, fmt)
                self.send(7, 0, 9)
                flags = None
                while True:
                    obj, opcode, payload = self.event()
                    if obj != 7:
                        continue
                    if opcode == 1 and len(payload) == 4 and flags is None:
                        flags, = struct.unpack("=I", payload)
                        if flags & ~1:
                            raise RuntimeError("unknown screencopy flags")
                    elif opcode == 2 and len(payload) == 12 and flags is not None:
                        if struct.unpack("=III", payload)[2] >= 1_000_000_000:
                            raise RuntimeError("invalid screencopy timestamp")
                        result = Image.frombytes("RGB", (width, height), memory[:], "raw", "BGRX", stride, -1 if flags & 1 else 1)
                        self.send(7, 1)  # frame.destroy
                        return result
                    else:
                        raise RuntimeError("failed or malformed screencopy frame")
        finally:
            os.close(fd)


def capture_private_output(env):
    capture = PrivateCapture(env)
    try:
        return capture.capture()
    finally:
        capture.socket.close()
