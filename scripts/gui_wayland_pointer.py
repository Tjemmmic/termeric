"""Minimal v1 wlr virtual-pointer client for the private headless GUI fixture.

Only registry discovery, sync, absolute motion, and left clicks are implemented.
No generated bindings, input-device access, or external helper is required.
Wire requests follow wayland.xml and wlr-virtual-pointer-unstable-v1.xml.
"""
from pathlib import Path
import socket
import struct
import time

MANAGER = b"zwlr_virtual_pointer_manager_v1\0"


def wire_string(value: bytes) -> bytes:
    return struct.pack("=I", len(value)) + value + b"\0" * (-len(value) % 4)


def wire_message(object_id: int, opcode: int, payload: bytes = b"") -> bytes:
    size = 8 + len(payload)
    if size > 65535 or size % 4 or not 0 <= opcode <= 65535:
        raise ValueError("invalid Wayland message size/opcode")
    return struct.pack("=II", object_id, size << 16 | opcode) + payload


def private_display_path(env: dict) -> Path:
    if env.get("WLR_BACKENDS") != "headless" or env.get("WLR_LIBINPUT_NO_DEVICES") != "1":
        raise ValueError("pointer injection requires the private headless compositor")
    runtime = Path(env["XDG_RUNTIME_DIR"]).resolve()
    display = (runtime / env["WAYLAND_DISPLAY"]).resolve()
    if display.parent != runtime or Path(env["SWAYSOCK"]).resolve().parent != runtime:
        raise ValueError("Wayland and Sway sockets must belong to the same private runtime")
    return display


class VirtualPointer:
    def __init__(self, env: dict):
        path = private_display_path(env)
        self.socket = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.next_callback = 3
        self.manager_name = None
        try:
            self.socket.settimeout(5)
            self.socket.connect(str(path))
            self.send(1, 1, 2)  # wl_display.get_registry -> object 2
            self.roundtrip()
            if self.manager_name is None:
                raise RuntimeError("private compositor has no wlr virtual-pointer manager")
            self.socket.sendall(wire_message(2, 0, struct.pack("=I", self.manager_name)
                + wire_string(MANAGER) + struct.pack("=II", 1, 4)))
            self.send(4, 0, 0, 5)  # manager.create_virtual_pointer(null seat, object 5)
            self.next_callback = 6
            self.roundtrip()
        except BaseException:
            self.socket.close()
            raise

    def send(self, object_id, opcode, *values):
        self.socket.sendall(wire_message(object_id, opcode, struct.pack("=" + "I" * len(values), *values)))

    def read_exact(self, size, deadline):
        result = bytearray()
        while len(result) < size:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError("private Wayland pointer roundtrip timed out")
            self.socket.settimeout(remaining)
            chunk = self.socket.recv(size - len(result))
            if not chunk:
                raise RuntimeError("private Wayland compositor disconnected")
            result.extend(chunk)
        return bytes(result)

    def roundtrip(self):
        callback = self.next_callback
        self.next_callback += 1
        self.send(1, 0, callback)
        deadline = time.monotonic() + 5
        while True:
            object_id, word = struct.unpack("=II", self.read_exact(8, deadline))
            size, opcode = word >> 16, word & 65535
            if size < 8 or size % 4:
                raise RuntimeError("malformed private Wayland event")
            payload = self.read_exact(size - 8, deadline)
            if object_id == 1 and opcode == 0:
                raise RuntimeError(f"private Wayland protocol error: {payload!r}")
            if object_id == callback and opcode == 0:
                return
            if object_id == 2 and opcode == 0:  # wl_registry.global
                if len(payload) < 12:
                    raise RuntimeError("truncated Wayland registry global")
                name, length = struct.unpack_from("=II", payload)
                if length == 0 or 8 + ((length + 3) & ~3) + 4 != len(payload):
                    raise RuntimeError("invalid Wayland registry string")
                if payload[8:8 + length] == MANAGER:
                    self.manager_name = name

    def click(self, x, y, width, height):
        if not (0 <= x < width and 0 <= y < height):
            raise ValueError("pointer point is outside the fixture surface")
        timestamp = int(time.monotonic() * 1000) & 0xffffffff
        self.send(5, 1, timestamp, x, y, width, height)
        self.send(5, 4)  # pointer.frame
        self.roundtrip()
        # Let the app bind wl_pointer after the seat gains its first device.
        time.sleep(0.08)
        for state in (1, 0):
            self.send(5, 2, timestamp, 272, state)  # BTN_LEFT, pressed/released
            self.send(5, 4)
        self.roundtrip()

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.socket.close()  # destroys every server-side object, also on failure
