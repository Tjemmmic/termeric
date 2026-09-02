#!/usr/bin/env python3
"""Compare a deterministic renderer PNG with Termeric's visual signature."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys

from PIL import Image


SIZE = (128, 32)


def signature(path: Path) -> dict[str, object]:
    with Image.open(path) as source:
        image = source.convert("RGB").resize(SIZE, Image.Resampling.BILINEAR)
    flatten = getattr(image, "get_flattened_data", image.getdata)
    pixels = list(flatten())
    ink = [max(pixel) > 8 for pixel in pixels]
    points = [(index % SIZE[0], index // SIZE[0]) for index, value in enumerate(ink) if value]
    if not points:
        raise ValueError("renderer snapshot contains no visible pixels")
    colors = [pixel for pixel, visible in zip(pixels, ink) if visible]
    rows = [sum(ink[y * SIZE[0] : (y + 1) * SIZE[0]]) for y in range(SIZE[1])]
    columns = [sum(ink[y * SIZE[0] + x] for y in range(SIZE[1])) for x in range(SIZE[0])]
    return {
        "normalized_size": list(SIZE),
        "ink_pixels": len(points),
        "ink_bbox": [
            min(point[0] for point in points),
            min(point[1] for point in points),
            max(point[0] for point in points),
            max(point[1] for point in points),
        ],
        "mean_ink_rgb": [round(sum(pixel[channel] for pixel in colors) / len(colors), 2) for channel in range(3)],
        "row_ink": rows,
        "column_ink": columns,
    }


def normalized_l1(actual: list[int], expected: list[int]) -> float:
    scale = max(1, sum(expected))
    return sum(abs(left - right) for left, right in zip(actual, expected)) / scale


def compare(actual: dict[str, object], expected: dict[str, object]) -> list[str]:
    failures: list[str] = []
    if actual["normalized_size"] != expected["normalized_size"]:
        failures.append("normalized dimensions changed")
    actual_ink = int(actual["ink_pixels"])
    expected_ink = int(expected["ink_pixels"])
    if abs(actual_ink - expected_ink) / max(1, expected_ink) > 0.12:
        failures.append(f"visible pixel count changed: {actual_ink} vs {expected_ink}")
    for name in ("ink_bbox", "mean_ink_rgb"):
        left = list(actual[name])
        right = list(expected[name])
        tolerance = 4 if name == "ink_bbox" else 20
        if any(abs(float(a) - float(b)) > tolerance for a, b in zip(left, right)):
            failures.append(f"{name} changed: {left} vs {right}")
    for name in ("row_ink", "column_ink"):
        distance = normalized_l1(list(actual[name]), list(expected[name]))
        if distance > 0.18:
            failures.append(f"{name} profile changed: normalized L1 {distance:.4f} > 0.1800")
    return failures


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("snapshot", type=Path)
    parser.add_argument(
        "--baseline",
        type=Path,
        default=Path(__file__).resolve().parents[1]
        / "termeric"
        / "tests"
        / "gui"
        / "baselines"
        / "renderer.json",
    )
    parser.add_argument("--update", action="store_true")
    args = parser.parse_args()

    actual = signature(args.snapshot)
    if args.update:
        args.baseline.parent.mkdir(parents=True, exist_ok=True)
        args.baseline.write_text(json.dumps(actual, indent=2) + "\n", encoding="utf-8")
        print(f"Updated GUI visual signature: {args.baseline}")
        return 0
    if not args.baseline.is_file():
        print(f"missing GUI visual signature: {args.baseline}", file=sys.stderr)
        return 1
    expected = json.loads(args.baseline.read_text(encoding="utf-8"))
    failures = compare(actual, expected)
    if failures:
        for failure in failures:
            print(f"GUI visual regression: {failure}", file=sys.stderr)
        return 1
    print(f"GUI visual signature passed: {args.snapshot}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
