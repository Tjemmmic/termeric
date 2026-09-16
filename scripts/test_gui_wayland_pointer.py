"""Wire/cleanup guards without connecting to any compositor or input device."""
import struct
import unittest
from unittest import mock

from scripts import gui_wayland_pointer as pointer

ENV = {"WLR_BACKENDS": "headless", "WLR_LIBINPUT_NO_DEVICES": "1",
       "XDG_RUNTIME_DIR": "/tmp/private-pointer-fixture", "WAYLAND_DISPLAY": "wayland-1",
       "SWAYSOCK": "/tmp/private-pointer-fixture/sway.sock"}


def done(callback):
    return pointer.wire_message(callback, 0, struct.pack("=I", 0))


class FragmentedSocket:
    def __init__(self, data):
        self.data = data
        self.sent = []
        self.closed = False

    def settimeout(self, _):
        pass

    def connect(self, path):
        self.path = path

    def recv(self, size):
        chunk, self.data = self.data[:min(size, 3)], self.data[min(size, 3):]
        return chunk

    def sendall(self, data):
        self.sent.append(data)

    def close(self):
        self.closed = True


class WaylandPointerTests(unittest.TestCase):
    def test_refuses_nonprivate_and_mismatched_display_environments(self):
        self.assertEqual(str(pointer.private_display_path(ENV)), "/tmp/private-pointer-fixture/wayland-1")
        for change in ({"WLR_BACKENDS": "drm"}, {"WLR_LIBINPUT_NO_DEVICES": "0"},
                       {"WAYLAND_DISPLAY": "/run/user/1000/wayland-0"},
                       {"SWAYSOCK": "/run/user/1000/sway.sock"}):
            with self.subTest(change=change), self.assertRaises(ValueError):
                pointer.VirtualPointer({**ENV, **change})

    def test_wire_padding_and_header_limits(self):
        self.assertEqual(pointer.wire_string(b"a\0"), struct.pack("=I", 2) + b"a\0\0\0")
        self.assertEqual(pointer.wire_message(5, 4), struct.pack("=II", 5, 8 << 16 | 4))
        for payload in (b"x", b"x" * 65536):
            with self.assertRaises(ValueError):
                pointer.wire_message(5, 4, payload)

    def test_fragmented_registry_roundtrips_motion_click_and_cleanup(self):
        registry = pointer.wire_message(2, 0, struct.pack("=I", 42)
            + pointer.wire_string(pointer.MANAGER) + struct.pack("=I", 2))
        sock = FragmentedSocket(registry + b"".join(done(i) for i in (3, 6, 7, 8)))
        with mock.patch.object(pointer.socket, "socket", return_value=sock), mock.patch.object(pointer.time, "sleep"):
            with pointer.VirtualPointer(ENV) as client:
                client.click(110, 113, 960, 128)
                with self.assertRaises(ValueError):
                    client.click(960, 128, 960, 128)
        self.assertTrue(sock.closed)
        self.assertEqual(sock.path, "/tmp/private-pointer-fixture/wayland-1")
        messages = [(struct.unpack_from("=I", data)[0], struct.unpack_from("=I", data, 4)[0] & 65535)
                    for data in sock.sent]
        self.assertEqual(messages, [(1, 1), (1, 0), (2, 0), (4, 0), (1, 0),
                                    (5, 1), (5, 4), (1, 0), (5, 2), (5, 4), (5, 2), (5, 4), (1, 0)])
        # New object IDs must start at 2 without skipping registry-map slots.
        self.assertEqual([struct.unpack_from("=I", sock.sent[index], -4)[0]
                          for index in (0, 1, 2, 3, 4, 7, 12)], list(range(2, 9)))
        self.assertEqual(struct.unpack_from("=IIII", sock.sent[5], 12), (110, 113, 960, 128))
        self.assertEqual(struct.unpack_from("=II", sock.sent[8], 12), (272, 1))
        self.assertEqual(struct.unpack_from("=II", sock.sent[10], 12), (272, 0))

    def test_missing_manager_malformed_events_and_disconnect_close_socket(self):
        for data in (done(3), b"", struct.pack("=II", 2, 4 << 16),
                     pointer.wire_message(1, 0, b"error!!!"), pointer.wire_message(2, 0)):
            sock = FragmentedSocket(data)
            with mock.patch.object(pointer.socket, "socket", return_value=sock), self.assertRaises(RuntimeError):
                pointer.VirtualPointer(ENV)
            self.assertTrue(sock.closed)

    def test_drag_keeps_one_device_through_motion_and_button_release(self):
        registry = pointer.wire_message(2, 0, struct.pack("=I", 42)
            + pointer.wire_string(pointer.MANAGER) + struct.pack("=I", 2))
        sock = FragmentedSocket(registry + b"".join(done(i) for i in (3, 6, 7, 8, 9, 10)))
        with mock.patch.object(pointer.socket, "socket", return_value=sock), mock.patch.object(pointer.time, "sleep"):
            with pointer.VirtualPointer(ENV) as client:
                client.move(20, 72, 960, 174)
                client.set_left_button(True)
                client.move(20, 101, 960, 174)
                client.set_left_button(False)
                with self.assertRaises(ValueError):
                    client.set_left_button(2)
        pointer_events = [(struct.unpack_from("=I", data, 4)[0] & 65535, data[8:])
                          for data in sock.sent if struct.unpack_from("=I", data)[0] == 5]
        self.assertEqual([opcode for opcode, _ in pointer_events], [1, 4, 2, 4, 1, 4, 2, 4])
        self.assertEqual([struct.unpack_from("=II", payload, 4) for opcode, payload in pointer_events if opcode == 2],
                         [(272, 1), (272, 0)])
        self.assertTrue(sock.closed)


if __name__ == "__main__":
    unittest.main()
