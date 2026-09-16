"""Private screencopy framing, bounded allocation, pixels and cleanup."""
import os
import struct
import unittest
from unittest import mock

from scripts import gui_wayland_capture as capture
from scripts.gui_wayland_pointer import wire_message, wire_string
from scripts.test_gui_wayland_pointer import ENV, FragmentedSocket, done


def registry():
    return b"".join(wire_message(2, 0, struct.pack("=I", name) + wire_string(interface)
                    + struct.pack("=I", 3)) for name, interface in (
                        (10, b"wl_shm\0"), (11, b"wl_output\0"), (12, capture.MANAGER))) + done(3)


class CaptureSocket(FragmentedSocket):
    def sendmsg(self, messages, ancillary):
        self.fd = ancillary[0][2][0]
        os.pwrite(self.fd, b"\x00\x00\xff\xff\x00\xff\x00\xff", 0)
        self.sent.extend(messages)
        return len(messages[0])


class CaptureTests(unittest.TestCase):
    def test_fragmented_capture_converts_pixels_and_closes_fd_and_socket(self):
        for flags, expected in ((0, [(255, 0, 0), (0, 255, 0)]), (1, [(0, 255, 0), (255, 0, 0)])):
            data = (registry() + wire_message(7, 0, struct.pack("=IIII", 1, 1, 2, 4))
                    + wire_message(7, 1, struct.pack("=I", flags))
                    + wire_message(7, 2, struct.pack("=III", 0, 1, 2)))
            sock = CaptureSocket(data)
            with mock.patch.object(capture.socket, "socket", return_value=sock):
                image = capture.capture_private_output(ENV)
            self.assertEqual(image.size, (1, 2))
            self.assertEqual([image.getpixel((0, y)) for y in (0, 1)], expected)
            self.assertTrue(sock.closed)
            with self.assertRaises(OSError):
                os.fstat(sock.fd)
            self.assertEqual(struct.unpack_from("=III", sock.sent[5], 8), (7, 0, 5))
            self.assertEqual(struct.unpack_from("=IIIIII", sock.sent[7], 8), (9, 0, 1, 2, 4, 1))

    def test_allocation_is_rejected_before_any_shared_memory_creation(self):
        for values in ((2, 1, 1, 4), (1, 0, 1, 4), (1, 4097, 1, 16388),
                       (1, 1, 4097, 4), (1, 2, 1, 4), (1, 1, 1, 5),
                       (1, 1, 2, capture.MAX_BYTES)):
            sock = CaptureSocket(registry() + wire_message(7, 0, struct.pack("=IIII", *values)))
            with mock.patch.object(capture.socket, "socket", return_value=sock), \
                    mock.patch.object(capture.os, "memfd_create") as allocate, self.assertRaises(RuntimeError):
                capture.capture_private_output(ENV)
            allocate.assert_not_called()
            self.assertTrue(sock.closed)

    def test_protocol_failures_close_all_allocated_resources(self):
        for tail in (wire_message(7, 3), wire_message(7, 1, struct.pack("=I", 2)),
                     wire_message(7, 2, struct.pack("=III", 0, 0, 0)),
                     wire_message(7, 1, struct.pack("=I", 0)) + wire_message(7, 2, struct.pack("=III", 0, 0, 1_000_000_000))):
            sock = CaptureSocket(registry() + wire_message(7, 0, struct.pack("=IIII", 0, 1, 2, 4)) + tail)
            with mock.patch.object(capture.socket, "socket", return_value=sock), self.assertRaises(RuntimeError):
                capture.capture_private_output(ENV)
            self.assertTrue(sock.closed)
            with self.assertRaises(OSError):
                os.fstat(sock.fd)

    def test_refuses_desktop_missing_globals_malformed_registry_and_timeout(self):
        with mock.patch.object(capture.socket, "socket") as sock, self.assertRaises(ValueError):
            capture.capture_private_output({**ENV, "WLR_BACKENDS": "drm"})
        sock.assert_not_called()
        for data in (b"", done(3), wire_message(2, 0), struct.pack("=II", 2, 4 << 16),
                     wire_message(1, 0, b"failure!")):
            sock = CaptureSocket(data)
            with mock.patch.object(capture.socket, "socket", return_value=sock), self.assertRaises(RuntimeError):
                capture.capture_private_output(ENV)
            self.assertTrue(sock.closed)
        sock = CaptureSocket(registry())
        with mock.patch.object(capture.socket, "socket", return_value=sock), \
                mock.patch.object(capture.time, "monotonic", side_effect=(0, 6)), self.assertRaises(TimeoutError):
            capture.capture_private_output(ENV)
        self.assertTrue(sock.closed)


if __name__ == "__main__":
    unittest.main()
