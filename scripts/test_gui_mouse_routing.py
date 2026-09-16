import base64
import unittest

from scripts.gui_acceptance import input_streams
from scripts.gui_mouse_routing import mouse_reports, verify_mouse_owners


class MouseOwnershipTests(unittest.TestCase):
    def test_reports_are_reassembled_across_pty_reads(self):
        chunks = [b"marker\x1b[<0;", b"2;3M\x1b[<32;4;", b"5M\x1b[<0;4;5m"]
        records = [{"event": "input", "pid": 7, "base64": base64.b64encode(chunk).decode()} for chunk in chunks]
        streams = input_streams(records)
        streams[8] = b"fence"
        reports = verify_mouse_owners(streams, 7, 8, switched=False)
        self.assertEqual(reports[7], [(0, 2, 3, "M"), (32, 4, 5, "M"), (0, 4, 5, "m")])

    def test_interrupted_report_requires_exact_press_and_no_replacement_input(self):
        press = b"\x1b[<0;2;3M"
        verify_mouse_owners({7: press, 8: b"fence"}, 7, 8, switched=True)
        for target in (b"\x1b[<0;2;3m", b"\x1b[<32;4;5M", press):
            with self.assertRaisesRegex(ValueError, "leaked"):
                verify_mouse_owners({7: press, 8: target}, 7, 8, switched=True)
        for source in (b"", press * 2, press + b"\x1b[<0;2;3m", b"\x1b[<4;2;3M"):
            with self.assertRaises(ValueError):
                verify_mouse_owners({7: source, 8: b""}, 7, 8, switched=True)

    def test_pre_switch_motion_is_explicit_and_never_allows_release(self):
        stream = b"\x1b[<0;2;3M\x1b[<32;4;5M"
        with self.assertRaises(ValueError):
            verify_mouse_owners({7: stream, 8: b""}, 7, 8, switched=True)
        verify_mouse_owners({7: stream, 8: b""}, 7, 8, switched=True, prior_motion=True)
        with self.assertRaises(ValueError):
            verify_mouse_owners({7: stream + b"\x1b[<0;4;5m", 8: b""}, 7, 8, switched=True, prior_motion=True)

    def test_oracle_refuses_missing_ordinary_events_bad_reports_and_owner_aliases(self):
        for stream in (b"", b"\x1b[<0;2;3M", b"\x1b[<0;2;3M\x1b[<0;2;3m"):
            with self.assertRaises(ValueError):
                verify_mouse_owners({7: stream, 8: b""}, 7, 8, switched=False)
        for stream in (b"\x1b[<0;2;", b"\x1b[<0;0;3M", b"\x1b[<bad"):
            with self.assertRaises(ValueError):
                mouse_reports(stream)
        for streams, source, target in (({7: b""}, 7, 7), ({7: b""}, 7, 8), ({7: b"", 8: b"", 9: b""}, 7, 8)):
            with self.assertRaises(ValueError):
                verify_mouse_owners(streams, source, target, switched=True)


if __name__ == "__main__":
    unittest.main()
