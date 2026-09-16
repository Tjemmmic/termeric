"""Pure tab-placement harness guards; native results stay a separate lane."""
from pathlib import Path
import tempfile
import unittest
from unittest import mock

from scripts import gui_acceptance as gui
from scripts import gui_tab_placement as placement


class TabPlacementTests(unittest.TestCase):
    def test_fixture_configuration_changes_only_its_explicit_placement(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "config.toml"
            gui.write_config(path, "/fixture/program", ["literal argument"])
            config = '# example: tab_bar_position = "top"\n' + path.read_text()
            left = placement.configuration_for_position(config, "left")
            self.assertEqual(left.splitlines()[0], config.splitlines()[0])
            self.assertIn('\ntab_bar_position = "left"\n', left)
            self.assertEqual(placement.configuration_for_position(left, "top"), config)

    def test_missing_duplicate_and_invalid_configuration_is_rejected(self):
        for config, position in (("", "left"), ('tab_bar_position = "top"\n' * 2, "left"),
                                 ('tab_bar_position = "top"\ntab_bar_position = "left"', "top"),
                                 ('tab_bar_position = "top"', "bottom")):
            with self.subTest(config=config, position=position), self.assertRaises(ValueError):
                placement.configuration_for_position(config, position)

    def test_rail_requires_positive_character_and_pixel_width_changes_only(self):
        top = (30, 100, 1000, 600)
        self.assertTrue(placement.rail_reduced_width((30, 78, 780, 600), top))
        for current in (top, (30, 100, 780, 600), (30, 78, 1000, 600),
                        (29, 78, 780, 600), (30, 78, 780, 599),
                        (0, 78, 780, 600), (30, 0, 780, 600), (30, 78, 0, 600),
                        (30, 78, 780), (30, 110, 1100, 600)):
            with self.subTest(current=current):
                self.assertFalse(placement.rail_reduced_width(current, top))

    def test_backend_adapters_use_existing_app_keyboard_and_text_apis(self):
        app = mock.create_autospec(gui.AppProcess, instance=True)
        placement.native_chord(app, "x11", ("ctrl", "shift"), "t")
        placement.native_text(app, "x11", "TOKEN")
        app.key.assert_called_once_with("ctrl+shift+t")
        app.type.assert_called_once_with("TOKEN")
        placement.native_chord(app, "wayland", (), "F6")
        placement.native_text(app, "wayland", "TOKEN")
        app.wayland_chord.assert_called_once_with((), "F6")
        app.wayland_type.assert_called_once_with("TOKEN")
        with self.assertRaises(ValueError):
            placement.native_text(app, "invalid", "TOKEN")
        with self.assertRaises(ValueError):
            placement.native_chord(app, "invalid", (), "F6")

    def test_native_tools_are_scenario_local_and_ci_selects_the_new_lane(self):
        self.assertEqual(gui.required_tools("wayland", ["tab-placement"], False, True),
                         ["sway", "swaymsg", "tic", "wtype"])
        self.assertEqual(gui.required_tools("x11", ["tab-placement"], False, False, True),
                         ["openbox", "tic", "xdotool"])
        workflow = (gui.ROOT / ".github/workflows/ci.yml").read_text()
        block = workflow.split("- name: Run Wayland keyboard, pane, and clipboard scenario", 1)[1].split("- name:", 1)[0]
        self.assertIn("--scenario tab-placement", block)
        self.assertIn("--require-clean", block)
        self.assertEqual(workflow.count("python3 -m unittest scripts.test_gui_tab_placement"), 2)

    def test_overflow_traversal_crosses_last_close_and_previous_to_new_tab(self):
        for count in (2, 12, 32):
            keys = placement.overflow_navigation_keys(count)
            self.assertEqual(keys[:2], ("F6", "Home"))
            self.assertEqual(keys[2:-1], ("Down",) * (count * 2 + 1))
            self.assertEqual(keys[-1], "Return")
        for count in (0, 1, 33):
            with self.assertRaises(ValueError):
                placement.overflow_navigation_keys(count)

    def test_overflow_lane_is_selected_with_backend_local_tools(self):
        self.assertEqual(gui.required_tools("wayland", ["tab-overflow"], False, True),
                         ["sway", "swaymsg", "tic", "wtype"])
        self.assertEqual(gui.required_tools("x11", ["tab-overflow"], False, False, True),
                         ["openbox", "tic", "xdotool"])
        workflow = (gui.ROOT / ".github/workflows/ci.yml").read_text()
        block = workflow.split("- name: Run Wayland keyboard, pane, and clipboard scenario", 1)[1].split("- name:", 1)[0]
        self.assertIn("--scenario tab-overflow", block)
        self.assertIn("--require-clean", block)

    def test_compact_points_use_observed_metrics_and_reject_noncompact_fixtures(self):
        points = placement.compact_pointer_targets((5, 80, 720, 85))
        self.assertEqual(points, {"tab": (20, 43), "previous": (36, 113),
                                  "next": (110, 113), "new": (183, 113)})
        self.assertEqual(placement.compact_pointer_targets((5, 80, 720, 5))["tab"], (20, 42))
        for geometry in ((0, 80, 720, 85), (5, 80, 960, 85),
                         (1, 80, 720, 100)):
            with self.assertRaises(ValueError):
                placement.compact_pointer_targets(geometry)

    def test_compact_metrics_identify_one_or_two_visible_tabs_for_distant_click(self):
        for geometry, row_height, visible in (((5, 80, 720, 85), 29, 2),
                                               ((3, 80, 720, 75), 38, 1)):
            height = placement.compact_row_height(geometry)
            self.assertEqual(height, row_height)
            self.assertEqual((128 - 2 * height) // height, visible)
            self.assertLess(placement.compact_pointer_targets(geometry)["tab"][1], 2 * height)

    def test_pointer_adapter_is_backend_specific(self):
        app = mock.Mock(env={"SWAYSOCK": "/private/sway"})
        harness = mock.Mock(AcceptanceError=gui.AcceptanceError)
        placement.native_click(harness, app, "x11", (36, 113))
        app.click.assert_called_once_with(36, 113)
        harness.run.assert_not_called()
        with mock.patch.object(placement, "VirtualPointer") as pointer:
            placement.native_click(harness, app, "wayland", (36, 113))
            pointer.assert_called_once_with(app.env)
            pointer.return_value.__enter__.return_value.click.assert_called_once_with(36, 113, 960, 128)
        app.env = {}
        with self.assertRaises(ValueError):
            placement.native_click(harness, app, "wayland", (36, 113))

    def test_wayland_drag_holds_device_and_releases_on_callback_failure(self):
        app, harness = mock.Mock(env={}), mock.Mock()
        with mock.patch.object(placement, "VirtualPointer") as factory:
            pointer = factory.return_value.__enter__.return_value
            callback = mock.Mock()
            placement.native_drag(harness, app, "wayland", (20, 72), (20, 101), (960, 174), callback)
            self.assertEqual(pointer.method_calls, [mock.call.move(20, 72, 960, 174),
                mock.call.set_left_button(True), mock.call.move(20, 101, 960, 174),
                mock.call.set_left_button(False)])
            callback.assert_called_once_with()
            pointer.reset_mock()
            with self.assertRaisesRegex(RuntimeError, "fixture error"):
                placement.native_drag(harness, app, "wayland", (20, 72), (20, 101), (960, 174),
                                      mock.Mock(side_effect=RuntimeError("fixture error")))
            self.assertEqual(pointer.set_left_button.call_args_list, [mock.call(True), mock.call(False)])

    def test_x11_drag_releases_button_on_callback_failure(self):
        app, harness = mock.Mock(window_id="fixture"), mock.Mock()
        with self.assertRaisesRegex(RuntimeError, "fixture error"):
            placement.native_drag(harness, app, "x11", (20, 72), (20, 101), (960, 174),
                                  mock.Mock(side_effect=RuntimeError("fixture error")))
        self.assertEqual(app.xdo.call_args_list, [
            mock.call("mousemove", "--window", "fixture", "20", "72"),
            mock.call("mousedown", "1"), mock.call("mouseup", "1")])


if __name__ == "__main__":
    unittest.main()
