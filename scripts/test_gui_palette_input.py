import base64
import unittest

from scripts.gui_acceptance import input_streams
from scripts.gui_palette_input import CASES, verify_palette_delivery


class PaletteInputTests(unittest.TestCase):
    def test_fragmented_utf8_and_fences_are_joined_before_exact_comparison(self):
        expected = {7: "☃".encode() + b"owner-fence", 8: b"other-fence"}
        records = [
            {"event": "input", "pid": pid, "base64": base64.b64encode(bytes([byte])).decode()}
            for pid, data in expected.items() for byte in data
        ]
        streams = input_streams(records)
        streams[9] = b""  # historical, closed fixture with no new bytes
        verify_palette_delivery(streams, expected)

    def test_oracle_rejects_wrong_owner_query_keys_duplicates_and_missing_fences(self):
        expected = {7: "☃".encode() + b"owner", 8: b"other"}
        for actual in (
            {7: b"owner", 8: "☃".encode() + b"other"},
            {7: b"U+2603" + expected[7], 8: b"other"},
            {7: b"\x1b[D" + expected[7], 8: b"other"},
            {7: "☃".encode() + expected[7], 8: b"other"},
            {7: "☰".encode() + b"owner", 8: b"other"},
            {7: expected[7]},
            {7: expected[7], 8: b"other", 9: b"leak"},
        ):
            with self.subTest(actual=actual), self.assertRaises(ValueError):
                verify_palette_delivery(actual, expected)
        for invalid in ({7: b"x"}, {7: b"x", 8: b""}):
            with self.assertRaises(ValueError):
                verify_palette_delivery(invalid, invalid)

    def test_native_matrix_names_every_editor_and_grapheme_case(self):
        self.assertEqual([case[0] for case in CASES], [
            "middle-delete", "left-insert", "home-right", "end-insert",
            "combining-backspace", "zwj-backspace", "flag-backspace",
        ])
        self.assertTrue(all(query.startswith("U+") and edits for _, query, edits in CASES))


if __name__ == "__main__":
    unittest.main()
