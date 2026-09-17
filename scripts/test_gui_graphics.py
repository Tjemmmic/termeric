import copy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock

from scripts import gui_acceptance, gui_graphics
from scripts.gui_acceptance import required_tools
from scripts.gui_graphics import FIXED_IDS, PROTOCOLS, graphics_clients_accepted, graphics_clients_ready


class GraphicsAcceptanceTests(unittest.TestCase):
    def setUp(self):
        self.records = [{"event": "graphics_client", "tool": tool, "protocol": protocol,
                         "version": "fixture 1", "output_bytes": 100,
                         "image_id": FIXED_IDS.get(tool, 12345), "cleanup_observed": True}
                        for tool, protocol in PROTOCOLS.items()]
        self.records.append({"event": "graphics_clients_complete", "drained": True, "attempted": 8, "failed": 0})
        self.log = "\n".join([
            *(f"Accepted Kitty image: id={number}, 64x64 pixels" for number in (1701, 1703, 1704, 12345)),
            "Accepted Kitty Unicode placeholder image: id=1702, 64x64 pixels",
            "Accepted Sixel image: cursor advance 8x4 cells",
            "Accepted iTerm2 inline image: cursor advance 2x1 cells",
            "Accepted Sixel image: cursor advance 2x1 cells",
        ])

    def accepted(self, records=None, log=None):
        return graphics_clients_accepted(self.records if records is None else records,
                                         (self.log if log is None else log).__contains__,
                                         (self.log if log is None else log).count)

    def test_requires_exact_client_set_ids_versions_protocols_and_cleanup(self):
        self.assertTrue(self.accepted())
        self.assertFalse(self.accepted(self.records[:-1]))
        self.assertFalse(self.accepted(self.records + [self.records[0]]))
        for index, key, value in [(0, "image_id", 17010), (0, "image_id", "1701"),
                                  (0, "version", ""), (0, "output_bytes", 0),
                                  (0, "output_bytes", True), (0, "protocol", "sixel"),
                                  (2, "cleanup_observed", False), (3, "cleanup_observed", 1),
                                  (4, "image_id", 17), (4, "image_id", 1701),
                                  (4, "image_id", -1), (4, "image_id", 2**32)]:
            records = copy.deepcopy(self.records)
            records[index][key] = value
            with self.subTest(index=index, key=key, value=value):
                self.assertFalse(self.accepted(records))

    def test_log_ids_require_delimiters_and_each_protocol_acceptance(self):
        for message in self.log.splitlines():
            self.assertFalse(self.accepted(log=self.log.replace(message, "")))
        self.assertFalse(self.accepted(log=self.log.replace("id=1701,", "id=17010,")))
        self.assertFalse(self.accepted(log=self.log.replace("id=12345,", "id=123456,")))

    def test_completion_rejects_mismatched_counts_order_and_duplicate_markers(self):
        for field, value in [("attempted", 7), ("attempted", True), ("failed", 1), ("failed", False),
                             ("drained", False), ("drained", None), ("drained", 1)]:
            records = copy.deepcopy(self.records)
            records[-1][field] = value
            with self.assertRaisesRegex(ValueError, "completion"):
                graphics_clients_ready(records, self.log.__contains__, self.log.count)
        for records in [self.records + [self.records[-1]],
                        [self.records[-1]] + self.records[:-1],
                        [self.records[1], self.records[0]] + self.records[2:]]:
            with self.assertRaisesRegex(ValueError, "completion"):
                graphics_clients_ready(records, self.log.__contains__, self.log.count)

    def test_pre_emission_error_waits_for_remaining_clients_then_fails(self):
        error = {"event": "graphics_client_error", "tool": "img2sixel", "protocol": "sixel",
                 "stage": "validate-output", "error": "no output", "continuing": True}
        partial = self.records[:5] + [error]
        self.assertFalse(graphics_clients_ready(partial, self.log.__contains__, self.log.count))
        completed = partial + self.records[6:8] + [{"event": "graphics_clients_complete", "drained": True, "attempted": 8, "failed": 1}]
        self.assertFalse(self.accepted(completed))
        with self.assertRaisesRegex(ValueError, "img2sixel.*validate-output.*no output"):
            graphics_clients_ready(completed, self.log.__contains__, self.log.count)
        fatal = {**error, "stage": "emit", "continuing": False}
        with self.assertRaisesRegex(ValueError, "emit"):
            graphics_clients_ready(self.records[:5] + [fatal], self.log.__contains__, self.log.count)
        with self.assertRaisesRegex(ValueError, "emit"):
            graphics_clients_ready(self.records[:5] + [{**fatal, "continuing": True}], self.log.__contains__, self.log.count)

    def test_wayland_graphics_declares_all_clients_and_private_input_tools(self):
        tools = required_tools("wayland", ["graphics"], False, True)
        self.assertEqual(set(tools), {"tic", "kitten", "img2sixel", "timg", "wtype", "sway", "swaymsg"})

    def test_both_sixel_clients_require_two_acceptances_and_old_seven_client_shape_fails(self):
        for count in [0, 1, 3, True, "2"]:
            self.assertFalse(graphics_clients_accepted(self.records, self.log.__contains__, lambda _: count))
        self.assertTrue(graphics_clients_accepted(self.records, self.log.__contains__, lambda _: 2))
        old = self.records[:7] + [{"event": "graphics_clients_complete", "drained": True, "attempted": 7, "failed": 0}]
        self.assertFalse(self.accepted(old))

    def test_explicit_failure_is_never_accepted_and_stops_polling(self):
        error = {"event": "graphics_client_error", "tool": "img2sixel",
                 "stage": "validate-output", "error": "produced no terminal output"}
        self.assertFalse(self.accepted(self.records + [error]))
        self.assertFalse(graphics_clients_ready(self.records[:-1], self.log.__contains__, self.log.count))
        self.assertTrue(graphics_clients_ready(self.records, self.log.__contains__, self.log.count))
        with mock.patch.object(gui_acceptance.time, "sleep") as sleep:
            with self.assertRaisesRegex(ValueError, "img2sixel.*validate-output.*no terminal output"):
                gui_acceptance.wait_until("graphics", lambda:
                    graphics_clients_ready(self.records[:5] + [error], self.log.__contains__, self.log.count))
            sleep.assert_not_called()

    def test_failed_wayland_run_retains_partial_results_and_cleans_up(self):
        error = {"event": "graphics_client_error", "tool": "img2sixel",
                 "stage": "validate-output", "error": "produced no terminal output"}
        with tempfile.TemporaryDirectory() as directory:
            run_dir = Path(directory)
            records = self.records[:5] + [error]
            (run_dir / "pty-transcript.jsonl").write_text(
                "".join(json.dumps(row) + "\n" for row in records))
            with mock.patch.object(gui_acceptance, "AppProcess") as factory, \
                    mock.patch.object(gui_graphics, "capture_private_output") as capture:
                app = factory.return_value
                with self.assertRaisesRegex(ValueError, "img2sixel"):
                    gui_graphics.run_wayland_graphics(gui_acceptance, run_dir / "termeric", run_dir, {})
                app.stop_after_failure.assert_called_once_with()
                app.finish.assert_called_once_with()
                app.wayland_type.assert_not_called()
                capture.assert_not_called()
            report = json.loads((run_dir / "graphics-input.json").read_text())
            self.assertEqual(report["clients"], self.records[:5])
            self.assertEqual(report["client_errors"], [error])
            self.assertIn("img2sixel", report["error"])
            self.assertNotIn("capture", report)

    def test_x11_checks_client_failure_before_reply_or_render_waits(self):
        error = {"event": "graphics_client_error", "tool": "kitten-stream",
                 "stage": "version", "error": "no version"}
        with tempfile.TemporaryDirectory() as directory, \
                mock.patch.object(gui_acceptance, "AppProcess") as factory, \
                mock.patch.object(gui_acceptance, "read_records", return_value=[error]), \
                mock.patch.object(gui_acceptance.time, "sleep") as sleep:
            with self.assertRaisesRegex(ValueError, "kitten-stream.*version"):
                gui_acceptance.run_workspace_scenario(Path("termeric"), Path(directory), {}, False)
            sleep.assert_not_called()
            factory.return_value.snapshot.assert_not_called()
            factory.return_value.stop_after_failure.assert_called_once_with()
            factory.return_value.finish.assert_called_once_with()

    def test_wayland_cleanup_runs_even_if_failure_reporting_cannot_read_transcript(self):
        with tempfile.TemporaryDirectory() as directory, \
                mock.patch.object(gui_acceptance, "AppProcess") as factory, \
                mock.patch.object(gui_acceptance, "read_records", side_effect=OSError("read failed")):
            with self.assertRaisesRegex(OSError, "read failed"):
                gui_graphics.run_wayland_graphics(gui_acceptance, Path("termeric"), Path(directory), {})
            factory.return_value.stop_after_failure.assert_called_once_with()
            factory.return_value.finish.assert_called_once_with()

    def failed_completed_records(self):
        records = copy.deepcopy(self.records)
        records[5] = {"event": "graphics_client_error", "tool": "img2sixel", "protocol": "sixel",
                      "stage": "validate-output", "error": "produced no terminal output", "continuing": True}
        records[-1]["failed"] = 1
        return [{"event": "started", "pid": 123}] + records

    def test_failure_capture_requires_complete_drained_safe_single_owner(self):
        records = self.failed_completed_records()
        self.assertTrue(gui_graphics.failed_capture_ready(records))
        for changed in [records[:-1], records[1:], records + [records[0]], self.records,
                        records[:-1] + [{**records[-1], "drained": False}],
                        records[:-1] + [{**records[-1], "failed": 0}]]:
            self.assertFalse(gui_graphics.failed_capture_ready(changed))
        for key, value in [("stage", "emit"), ("stage", "cleanup"), ("continuing", False), ("continuing", 1)]:
            changed = copy.deepcopy(records)
            changed[6][key] = value
            self.assertFalse(gui_graphics.failed_capture_ready(changed))
        for pid in [0, -1, True, "123"]:
            self.assertFalse(gui_graphics.failed_capture_ready([{**records[0], "pid": pid}] + records[1:]))

    def test_completed_failed_run_captures_diagnostics_but_never_becomes_a_pass(self):
        records = self.failed_completed_records()
        with tempfile.TemporaryDirectory() as directory, \
                mock.patch.object(gui_acceptance, "AppProcess") as factory, \
                mock.patch.object(gui_acceptance, "read_records", return_value=records), \
                mock.patch.object(gui_acceptance.time, "sleep"), \
                mock.patch.object(gui_graphics, "capture_private_output") as capture:
            app = factory.return_value
            app.process.poll.return_value = None
            snapshot = capture.return_value
            snapshot.size = (4, 3)
            snapshot.save.side_effect = lambda path: path.write_bytes(b"fixture capture")
            with self.assertRaisesRegex(ValueError, "img2sixel.*no terminal output"):
                gui_graphics.run_wayland_graphics(gui_acceptance, Path("termeric"), Path(directory), {})
            capture.assert_called_once_with(app.env)
            app.wayland_type.assert_not_called()
            app.wayland_chord.assert_not_called()
            app.stop_after_failure.assert_called_once_with()
            app.finish.assert_called_once_with()
            report = json.loads((Path(directory) / "graphics-input.json").read_text())
            self.assertEqual(report["schema"], 4)
            self.assertIn("img2sixel", report["error"])
            self.assertNotIn("capture", report)
            self.assertEqual(report["diagnostic_capture"], {
                "path": "graphics-clients-failed.png", "size": [4, 3],
                "sha256": hashlib.sha256(b"fixture capture").hexdigest(),
                "review_only": True, "failed_run": True, "acceptance": False,
            })
            self.assertFalse(self.accepted(records))

    def test_secondary_capture_failure_preserves_original_error_and_cleanup(self):
        with tempfile.TemporaryDirectory() as directory, \
                mock.patch.object(gui_acceptance, "AppProcess") as factory, \
                mock.patch.object(gui_acceptance, "read_records", return_value=self.failed_completed_records()), \
                mock.patch.object(gui_acceptance.time, "sleep"), \
                mock.patch.object(gui_graphics, "capture_private_output", side_effect=RuntimeError("capture unavailable")):
            factory.return_value.process.poll.return_value = None
            with self.assertRaisesRegex(ValueError, "img2sixel.*no terminal output"):
                gui_graphics.run_wayland_graphics(gui_acceptance, Path("termeric"), Path(directory), {})
            report = json.loads((Path(directory) / "graphics-input.json").read_text())
            self.assertEqual(report["diagnostic_capture_error"], "capture unavailable")
            self.assertIn("img2sixel", report["error"])
            self.assertNotIn("diagnostic_capture", report)
            factory.return_value.stop_after_failure.assert_called_once_with()
            factory.return_value.finish.assert_called_once_with()

    def test_exited_app_skips_failed_run_capture(self):
        with tempfile.TemporaryDirectory() as directory, \
                mock.patch.object(gui_acceptance, "AppProcess") as factory, \
                mock.patch.object(gui_acceptance, "read_records", return_value=self.failed_completed_records()), \
                mock.patch.object(gui_graphics, "capture_private_output") as capture:
            factory.return_value.process.poll.return_value = 1
            with self.assertRaisesRegex(ValueError, "img2sixel"):
                gui_graphics.run_wayland_graphics(gui_acceptance, Path("termeric"), Path(directory), {})
            capture.assert_not_called()
            factory.return_value.stop_after_failure.assert_called_once_with()
            factory.return_value.finish.assert_called_once_with()


if __name__ == "__main__":
    unittest.main()
