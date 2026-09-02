from pathlib import Path
import tempfile
import unittest

from PIL import Image, ImageDraw

from scripts import check_gui_visual


class GuiVisualTests(unittest.TestCase):
    def snapshot(self, path: Path, offset: int = 0) -> None:
        image = Image.new("RGB", (256, 64), "black")
        draw = ImageDraw.Draw(image)
        draw.rectangle((12 + offset, 12, 80 + offset, 32), fill=(220, 200, 170))
        image.save(path)

    def test_signature_accepts_identical_images(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "snapshot.png"
            self.snapshot(path)
            value = check_gui_visual.signature(path)
            self.assertEqual(check_gui_visual.compare(value, value), [])

    def test_signature_rejects_a_large_geometry_change(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            first = Path(temporary) / "first.png"
            second = Path(temporary) / "second.png"
            self.snapshot(first)
            self.snapshot(second, 100)
            failures = check_gui_visual.compare(
                check_gui_visual.signature(second), check_gui_visual.signature(first)
            )
            self.assertTrue(any("bbox" in failure or "column" in failure for failure in failures))


if __name__ == "__main__":
    unittest.main()
