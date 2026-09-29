#!/usr/bin/env python3
"""Fail when a local Markdown link points at a missing path or heading."""

from __future__ import annotations

import re
import sys
import unicodedata
from pathlib import Path
from urllib.parse import unquote


ROOT = Path(__file__).resolve().parent.parent
MARKDOWN_LINK = re.compile(r"(?<!!)\[[^\]]+\]\(([^)]+)\)")
EXTERNAL_SCHEMES = ("http://", "https://", "mailto:")
ATX_HEADING = re.compile(r"^ {0,3}#{1,6}\s+(.+?)\s*#*\s*$", re.MULTILINE)
FENCE = re.compile(r"^ {0,3}(`{3,}|~{3,})")


def markdown_files() -> list[Path]:
    # Root files include symlinks (AGENTS.md -> CLAUDE.md); check each target once.
    root_files = [path for path in sorted(ROOT.glob("*.md")) if not path.is_symlink()]
    return root_files + sorted((ROOT / "docs").glob("*.md"))


def link_destination(raw_target: str) -> str:
    """Return a Markdown inline-link destination without its optional title."""
    target = raw_target.strip()
    if target.startswith("<"):
        closing = target.find(">")
        return target[1:closing] if closing >= 0 else target[1:]
    return target.split(maxsplit=1)[0]


def local_reference(source: Path, raw_target: str) -> tuple[Path, str | None] | None:
    target = link_destination(raw_target)
    if not target or target.startswith(EXTERNAL_SCHEMES):
        return None

    raw_path, separator, raw_fragment = target.partition("#")
    path = unquote(raw_path)
    fragment = unquote(raw_fragment) if separator and raw_fragment else None

    if not path:
        return source, fragment
    if path.startswith("/"):
        return ROOT / path.lstrip("/"), fragment
    return source.parent / path, fragment


def outside_fenced_code(text: str) -> str:
    """Mask fenced code while preserving offsets and line numbers."""
    output: list[str] = []
    fence_marker: str | None = None
    fence_length = 0
    for line in text.splitlines(keepends=True):
        match = FENCE.match(line)
        if match is not None:
            marker = match.group(1)
            if fence_marker is None:
                fence_marker = marker[0]
                fence_length = len(marker)
            elif marker[0] == fence_marker and len(marker) >= fence_length:
                fence_marker = None
                fence_length = 0
            output.append(" " * (len(line.rstrip("\r\n"))))
            output.append(line[len(line.rstrip("\r\n")) :])
        elif fence_marker is None:
            output.append(line)
        else:
            output.append(" " * (len(line.rstrip("\r\n"))))
            output.append(line[len(line.rstrip("\r\n")) :])
    return "".join(output)


def github_slug(heading: str) -> str:
    """Approximate GitHub's documented heading-ID normalization."""
    heading = re.sub(r"<[^>]+>", "", heading)
    heading = re.sub(r"!?(?:\[([^]]+)\])\([^)]*\)", r"\1", heading)
    heading = heading.replace("`", "").replace("*", "")
    characters = [
        character
        for character in heading.casefold()
        if not unicodedata.category(character).startswith(("P", "S"))
        or character in {"-", "_"}
    ]
    return "".join(characters).strip().replace(" ", "-")


def markdown_anchors(text: str) -> set[str]:
    anchors: set[str] = set()
    occurrences: dict[str, int] = {}
    for match in ATX_HEADING.finditer(outside_fenced_code(text)):
        base = github_slug(match.group(1))
        suffix = occurrences.get(base, 0)
        anchor = base if suffix == 0 else f"{base}-{suffix}"
        while anchor in anchors:
            suffix += 1
            anchor = f"{base}-{suffix}"
        occurrences[base] = suffix + 1
        anchors.add(anchor)
    return anchors


def main() -> int:
    failures: list[str] = []
    anchor_cache: dict[Path, set[str]] = {}
    for source in markdown_files():
        text = source.read_text(encoding="utf-8")
        visible_text = outside_fenced_code(text)
        for match in MARKDOWN_LINK.finditer(visible_text):
            reference = local_reference(source, match.group(1))
            if reference is None:
                continue
            target, fragment = reference
            resolved = target.resolve()
            line = text.count("\n", 0, match.start()) + 1
            if not resolved.exists():
                failures.append(
                    f"{source.relative_to(ROOT)}:{line}: missing {match.group(1)}"
                )
                continue
            if fragment is None or resolved.suffix.casefold() != ".md":
                continue
            anchors = anchor_cache.get(resolved)
            if anchors is None:
                anchors = markdown_anchors(resolved.read_text(encoding="utf-8"))
                anchor_cache[resolved] = anchors
            if fragment not in anchors:
                failures.append(
                    f"{source.relative_to(ROOT)}:{line}: missing heading "
                    f"#{fragment} in {target.relative_to(ROOT)}"
                )

    if failures:
        print("Broken local Markdown links:", file=sys.stderr)
        print("\n".join(failures), file=sys.stderr)
        return 1

    print(f"Checked local Markdown links in {len(markdown_files())} files")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
