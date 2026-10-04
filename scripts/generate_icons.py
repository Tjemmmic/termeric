#!/usr/bin/env python3
"""Render Termeric's freedesktop hicolor icon set from the master artwork.

Icon themes only search the size directories their `index.theme` declares, and
hicolor declares none larger than 512x512, so a lone 1024x1024 icon is never
found by desktops. Re-run this after changing `assets/icons/termeric.png`.
"""

from __future__ import annotations

import argparse
from pathlib import Path

from PIL import Image


ROOT = Path(__file__).resolve().parents[1]
MASTER = ROOT / "assets" / "icons" / "termeric.png"
HICOLOR = ROOT / "assets" / "icons" / "hicolor"
# Keep in sync with ICON_SIZES in check_debian_package.py.
SIZES = (16, 24, 32, 48, 64, 128, 256, 512)


def main() -> int:
    argparse.ArgumentParser(description=__doc__.splitlines()[0]).parse_args()
    with Image.open(MASTER) as source:
        master = source.convert("RGBA")
    for size in SIZES:
        target = HICOLOR / f"{size}x{size}" / "apps" / "termeric.png"
        target.parent.mkdir(parents=True, exist_ok=True)
        master.resize((size, size), Image.Resampling.LANCZOS).save(target, optimize=True)
        print(f"wrote {target.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
