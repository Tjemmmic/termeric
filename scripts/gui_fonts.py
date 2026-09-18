"""Installed-family prerequisites for isolated GUI fixtures, not glyph coverage."""
from __future__ import annotations

import json
from pathlib import Path
import subprocess

DEFAULT_PRIMARY = "DejaVu Sans Mono"
FALLBACK = "Noto Color Emoji"
PRIMARY_ENV = "TERMERIC_GUI_FONT_FAMILY"


def validate_primary(family: str) -> str:
    family = family.strip()
    if (not family or len(family.encode("utf-8")) > 128
            or any(ord(char) < 32 or ord(char) == 127 or char == "," for char in family)):
        raise ValueError("GUI font must be a nonempty literal family of at most 128 UTF-8 bytes")
    if family.casefold() == FALLBACK.casefold():
        raise ValueError("GUI primary font must differ from the color-emoji fallback")
    return family


def validate_profile_options(family: str, skip_visual: bool, update_baselines: bool) -> str:
    family = validate_primary(family)
    if family.casefold() != DEFAULT_PRIMARY.casefold() and (not skip_visual or update_baselines):
        raise ValueError("a custom GUI font requires --skip-visual-checks and cannot update baselines")
    return family


def check_font_profile(directory: Path, environment: dict[str, str], primary: str) -> None:
    """Probe with the exact child's isolated HOME/XDG environment before launch.

    fc-match is deliberately insufficient: it succeeds on substituted families.
    Fontconfig inventory is only an installation prerequisite, not proof of the
    renderer's selected face, shaping, glyph coverage, or unchanged font files.
    Keep failures beside successful observations for review.
    """
    primary = validate_primary(primary)
    command = ["fc-list", "--format=%{family}\\n"]
    report = {"schema": 1, "primary": primary, "fallback": [FALLBACK],
              "command": command, "status": "failed", "error": None,
              "renderer_face": "not_observed", "glyph_coverage": "not_observed"}
    try:
        result = subprocess.run(command, env=environment, capture_output=True,
                                text=True, timeout=10, check=False)
        report["exit_status"] = result.returncode
        if len(result.stdout.encode("utf-8")) > 1024 * 1024:
            raise ValueError("Fontconfig family inventory exceeds 1 MiB")
        report["inventory"] = result.stdout
        report["stderr"] = result.stderr[:4096]
        if result.returncode != 0:
            raise ValueError("Fontconfig family inventory failed")
        families = {alias.strip().casefold() for line in result.stdout.splitlines()
                    for alias in line.split(",") if alias.strip()}
        missing = [name for name in (primary, FALLBACK) if name.casefold() not in families]
        report["missing"] = missing
        if missing:
            raise ValueError("required GUI font families are not installed: " + ", ".join(missing))
        report["status"] = "passed"
    except (OSError, subprocess.TimeoutExpired, ValueError, UnicodeError) as error:
        report["error"] = str(error)
    (directory / "font-profile.json").write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    if report["status"] != "passed":
        raise ValueError(f"{report['error']}; see {directory / 'font-profile.json'}")
