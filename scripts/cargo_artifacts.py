"""Find the Termeric test executable in `cargo test --no-run --message-format=json` output."""

from __future__ import annotations

import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


class CargoArtifactError(ValueError):
    """Cargo did not report exactly one usable Termeric test executable."""


def test_binary_from_cargo_messages(output: str) -> Path:
    executables: set[Path] = set()
    for line in output.splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not isinstance(message, dict) or message.get("reason") != "compiler-artifact":
            continue
        target = message.get("target")
        profile = message.get("profile")
        executable = message.get("executable")
        kind = target.get("kind") if isinstance(target, dict) else None
        if (
            isinstance(target, dict)
            and target.get("name") == "termeric"
            and isinstance(kind, list)
            and "bin" in kind
            and isinstance(profile, dict)
            and profile.get("test") is True
            and isinstance(executable, str)
        ):
            reported = Path(executable)
            executables.add(
                (reported if reported.is_absolute() else ROOT / reported).resolve()
            )
    if len(executables) != 1:
        raise CargoArtifactError(
            "expected one Termeric test executable from Cargo, found "
            f"{len(executables)}"
        )
    binary = executables.pop()
    if binary.is_symlink() or not binary.is_file():
        raise CargoArtifactError(f"Cargo reported a missing test executable: {binary}")
    return binary
