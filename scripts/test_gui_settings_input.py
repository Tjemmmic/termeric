import unittest
from unittest import mock
import subprocess

from scripts import gui_settings_input as settings


class SettingsInputTests(unittest.TestCase):
    def test_matrix_covers_paste_rejection_graphemes_and_pointer_placement(self):
        self.assertEqual([case[0] for case in settings.CASES], ["paste", "reject-multiline",
            "reject-oversize", "combining-backspace", "zwj-backspace", "pointer-wide", "pointer-combining"])
        self.assertEqual(settings.CASES[2][2], (("paste", "x" * 257),))
        self.assertEqual(settings.CASES[-2][2], (("click", 3.75), ("key", "BackSpace")))

    def test_pointer_uses_actual_cells_and_shared_modal_input_metrics(self):
        geometry = (42, 126, 1008, 714)  # 8x17 physical cells
        self.assertEqual(settings.settings_pointer_point(geometry, 0), (291, 135))
        self.assertEqual(settings.settings_pointer_point(geometry, 3.75), (321, 135))
        self.assertEqual(settings.settings_pointer_point(geometry, 2.75), (313, 135))
        for shape, col in [((0, 1, 1, 1), 0), (geometry, -1), (geometry, float("nan")), (geometry, 1000)]:
            with self.assertRaises(ValueError):
                settings.settings_pointer_point(shape, col)

    def test_delivery_rejects_wrong_owner_query_keys_extra_pty_and_duplicate_fences(self):
        settings.verify_settings_delivery({7: b"fence", 8: b""}, 7, b"fence")
        for actual in ({}, {8: b"fence"}, {7: b"sizefence"}, {7: b"\x1b[Cfence"},
                       {7: b"fencefence"}, {7: b"fence", 8: b"x"}):
            with self.assertRaises(ValueError):
                settings.verify_settings_delivery(actual, 7, b"fence")
        with self.assertRaises(ValueError):
            settings.verify_settings_delivery({True: b"fence"}, True, b"fence")

    def test_font_state_oracle_preserves_duplicates_and_distinct_transitions(self):
        line = lambda size, cols, rows: f"INFO Font size set to {size}, grid resized to {cols}x{rows}\n"
        raised, restored = line(15, 112, 40), line(14, 126, 42)
        self.assertEqual(settings.font_changes(raised + restored),
                         [(15, 112, 40), (14, 126, 42)])
        self.assertEqual(settings.font_changes(raised + raised + restored + restored),
                         [(15, 112, 40), (15, 112, 40), (14, 126, 42), (14, 126, 42)])
        for extra in [raised, restored, line(16, 100, 38), line(15, 111, 40), restored + raised]:
            self.assertNotEqual(settings.font_changes(raised + extra + restored),
                                [(15, 112, 40), (14, 126, 42)])
        self.assertEqual(settings.font_changes(raised.rstrip("\n")), [])

    def test_clipboard_source_requires_transfer_and_is_closed_after_success(self):
        gui, app = mock.Mock(), mock.Mock(env={"WAYLAND_DISPLAY": "private"})
        gui.AcceptanceError = ValueError
        with mock.patch.object(settings.subprocess, "Popen") as popen:
            source = popen.return_value
            source.poll.side_effect = [None, 0]
            source.wait.return_value = 0
            settings.paste_private(gui, app, "size")
            self.assertEqual(popen.call_args.args[0], ["wl-copy", "--foreground", "--paste-once", "--", "size"])
            app.wayland_chord.assert_called_once_with(("ctrl", "shift"), "v")
            source.wait.assert_called_once_with(timeout=5)
            source.terminate.assert_not_called()
            source.stderr.close.assert_called_once_with()

    def test_clipboard_timeout_terminates_source_and_early_exit_never_sends_paste(self):
        for early in [True, False]:
            gui, app = mock.Mock(), mock.Mock(env={})
            gui.AcceptanceError = ValueError
            with mock.patch.object(settings.subprocess, "Popen") as popen:
                source = popen.return_value
                source.poll.return_value = 1 if early else None
                source.wait.side_effect = [subprocess.TimeoutExpired("wl-copy", 5), 0]
                with self.assertRaises(ValueError if early else subprocess.TimeoutExpired):
                    settings.paste_private(gui, app, "size")
                if early:
                    app.wayland_chord.assert_not_called()
                    source.terminate.assert_not_called()
                else:
                    source.terminate.assert_called_once_with()
                    self.assertEqual(source.wait.call_args_list, [mock.call(timeout=5), mock.call(timeout=3)])
                source.stderr.close.assert_called_once_with()


if __name__ == "__main__":
    unittest.main()
