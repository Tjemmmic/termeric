"""The native oracle must reject absent, misplaced and incorrectly fitted images."""
import unittest
from PIL import Image
from scripts import gui_background as background
from scripts import gui_acceptance as gui


class BackgroundTests(unittest.TestCase):
    def test_probe_oracle_rejects_blank_and_wrong_colors(self):
        source = Image.new("RGB", (480, 240), (200, 20, 40))
        blank = Image.new("RGB", (960, 720), (0, 0, 0))
        viewport = (220, 29, 740, 691)
        probes = background.pixel_probes(source, blank, viewport, "stretch", 1)
        self.assertGreaterEqual(len(probes), 8)
        self.assertFalse(background.probes_match(blank, probes))
        image = blank.copy()
        for point, color in probes:
            image.putpixel(point, color)
        self.assertTrue(background.probes_match(image, probes))
        image.putpixel(probes[0][0], (0, 0, 0))
        self.assertFalse(background.probes_match(image, probes))
        self.assertTrue(background.probes_match(blank,
            background.pixel_probes(source, blank, viewport, "center", 0)))

    def test_fit_changes_admission_and_backend_tools_are_explicit(self):
        source = Image.new("RGB", (480, 240), (200, 20, 40))
        blank = Image.new("RGB", (960, 720))
        viewport = (220, 29, 740, 691)
        cover = background.pixel_probes(source, blank, viewport, "cover", 1)
        center = background.pixel_probes(source, blank, viewport, "center", 1)
        self.assertNotEqual(cover, center)
        with self.assertRaises(ValueError):
            background.pixel_probes(source, blank, viewport, "invalid", 1)
        self.assertEqual(gui.required_tools("wayland", ["background"], False, True),
                         ["sway", "swaymsg", "tic", "wtype"])


if __name__ == "__main__":
    unittest.main()
