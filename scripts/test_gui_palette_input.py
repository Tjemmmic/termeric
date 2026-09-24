import base64
import unittest
from unittest.mock import Mock, call

from scripts.gui_acceptance import input_streams
from scripts.gui_palette_input import CASES, apply_edit, palette_pointer_point, verify_palette_delivery


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
            "word-left", "word-right", "word-delete", "word-backspace-combining",
            "word-backspace-zwj", "word-backspace-flag",
            "pointer-middle", "pointer-wide", "pointer-combining", "pointer-scrolled",
        ])
        self.assertTrue(all("U+" in query and edits for _, query, edits in CASES))

    def test_pointer_targets_use_real_cell_metrics_and_scrolled_caret(self):
        geometry = (42, 126, 1008, 714)  # 8x17 physical-pixel cells
        # The query starts 2.6 cells into the field (FILTER_TEXT_INSET_CELLS).
        self.assertEqual(palette_pointer_point(geometry, 0), (319, 121))
        self.assertEqual(palette_pointer_point(geometry, 4), (351, 121))
        self.assertEqual(palette_pointer_point(geometry, 2.25), (337, 121))
        self.assertEqual(palette_pointer_point(geometry, 2.75), (341, 121))
        self.assertEqual(palette_pointer_point(geometry, 2, from_tail=True), (687, 121))
        for shape, column in [((0, 126, 1008, 714), 0), (geometry, -1),
                              (geometry, float("nan")), (geometry, 1000)]:
            with self.assertRaises(ValueError):
                palette_pointer_point(shape, column)

    def test_ctrl_edits_use_native_modifiers_not_typed_escape_sequences(self):
        app = Mock()
        for edit in (("ctrl", "Left"), ("key", "Delete"), ("text", "3")):
            apply_edit(app, *edit)
        self.assertEqual(app.mock_calls, [call.wayland_chord(("ctrl",), "Left"),
                                         call.wayland_chord((), "Delete"), call.wayland_type("3")])
        with self.assertRaises(ValueError):
            apply_edit(app, "unknown", "Left")


if __name__ == "__main__":
    unittest.main()
