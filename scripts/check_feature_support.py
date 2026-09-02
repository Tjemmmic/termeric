#!/usr/bin/env python3
"""Validate the feature matrix vocabulary and README coverage offline."""

from __future__ import annotations

import re
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
ALLOWED_STATUSES = {
    "Supported",
    "Partial",
    "Experimental",
    "Broken",
    "Unverified",
    "Unsupported",
}
STATUS = re.compile(
    r"\*\*(Supported|Partial|Experimental|Broken|Unverified|Unsupported)"
    r"(?: \([^*]+\))?\*\*"
)
ISSUE_LINK = re.compile(r"https://github\.com/Tjemmmic/termeric/issues/(\d+)")
HIGHLIGHT = re.compile(r"^- \*\*([^*]+)\*\*", re.MULTILINE)

# Every user-facing README highlight must resolve to at least one explicit
# capability row. A new highlight intentionally fails validation until its
# support boundary is added to the matrix and this mapping is reviewed.
HIGHLIGHT_CAPABILITIES = {
    "GPU rendering": {"GPU/software rendering"},
    "Safe file drops": {"File/directory drag and drop"},
    "26 themes and bounded backgrounds": {"Themes", "Terminal background images"},
    "Tabs, panes, profiles, and workspaces": {
        "Tabs",
        "Split panes",
        "Terminal launch profiles",
        "Complete workspace persistence",
    },
    "Search": {"Search"},
    "Keyboard copy mode and quick select": {
        "Keyboard copy mode and quick-select hints"
    },
    "Modern input and accessibility": {
        "IME/preedit",
        "Color emoji",
        "Accessibility and screen readers",
    },
    "Visual accessibility preferences": {"Accessibility and screen readers"},
    "Modern terminal negotiation": {"Application keyboard modes and modifiers"},
    "Command palette": {"Settings, command palette, and configurable shortcuts"},
    "Developer integrations": {
        "Git status/completion/diff workflow",
        "Claude Code and Codex integrations",
    },
    "Hyperlinks and shell semantics": {
        "OSC 8 hyperlinks and detected URLs",
        "OSC 133 shell semantics",
        "Shell-reported working directory",
    },
    "Harvest (experimental)": {"Harvest"},
}
RELEASE_GATE_TERMS = {
    "correctness": "Broken",
    "conformance": "conformance",
    "performance": "Performance",
    "soak": "soak",
    "packaging": "install/upgrade/uninstall",
    "manual evidence": "manual evidence",
}
RELEASE_DRY_RUN_ROW = re.compile(r"^\|\s*([1-9])\s*\|", re.MULTILINE)


def table_cells(line: str) -> list[str]:
    return [cell.strip() for cell in line.strip().strip("|").split("|")]


def capability_rows(matrix: str) -> list[tuple[int, list[str]]]:
    rows: list[tuple[int, list[str]]] = []
    in_capability_table = False
    for line_number, line in enumerate(matrix.splitlines(), 1):
        if line.startswith(("| Capability | Status |", "| Area | Status |")):
            in_capability_table = True
            continue
        if not in_capability_table:
            continue
        if not line.startswith("|"):
            in_capability_table = False
            continue
        cells = table_cells(line)
        if cells and set(cells[0]) <= {"-", ":"}:
            continue
        rows.append((line_number, cells))
    return rows


def validate(readme: str, matrix: str) -> list[str]:
    failures: list[str] = []
    rows = capability_rows(matrix)
    capabilities: set[str] = set()
    for line_number, cells in rows:
        if len(cells) != 4:
            failures.append(
                f"feature-support.md:{line_number}: capability row has {len(cells)} columns, expected 4"
            )
            continue
        capability, status, boundary, tracking = cells
        if capability in capabilities:
            failures.append(
                f"feature-support.md:{line_number}: duplicate capability {capability!r}"
            )
        capabilities.add(capability)
        match = STATUS.fullmatch(status)
        if match is None or match.group(1) not in ALLOWED_STATUSES:
            failures.append(
                f"feature-support.md:{line_number}: unknown status {status!r} for {capability!r}"
            )
        if not boundary:
            failures.append(
                f"feature-support.md:{line_number}: missing boundary for {capability!r}"
            )
        if tracking != "—" and "](" not in tracking:
            failures.append(
                f"feature-support.md:{line_number}: tracking must contain a link or an em dash"
            )

    if not rows:
        failures.append("feature-support.md: no capability rows found")

    highlights_match = re.search(r"## Highlights\n(?P<body>.*?)\n## ", readme, re.DOTALL)
    highlights = (
        set(HIGHLIGHT.findall(highlights_match.group("body")))
        if highlights_match is not None
        else set()
    )
    if highlights_match is None:
        failures.append("README.md: missing Highlights section")
    missing_mappings = sorted(highlights - HIGHLIGHT_CAPABILITIES.keys())
    stale_mappings = sorted(HIGHLIGHT_CAPABILITIES.keys() - highlights)
    for highlight in missing_mappings:
        failures.append(f"README.md: highlight {highlight!r} has no reviewed matrix mapping")
    for highlight in stale_mappings:
        failures.append(f"README.md: mapped highlight {highlight!r} no longer exists")
    for highlight, expected in HIGHLIGHT_CAPABILITIES.items():
        for capability in sorted(expected - capabilities):
            failures.append(
                f"README.md: highlight {highlight!r} references missing capability {capability!r}"
            )

    release_section = matrix.partition("## Release gates")[2].partition(
        "## Maintaining this matrix"
    )[0]
    if not release_section:
        failures.append("feature-support.md: missing Release gates section")
    for name, term in RELEASE_GATE_TERMS.items():
        if term.casefold() not in release_section.casefold():
            failures.append(f"feature-support.md: release gates omit {name}")

    maintenance = matrix.partition("## Maintaining this matrix")[2]
    for term in ["user-visible capability", "repeatable evidence", "README", "Supported"]:
        if term.casefold() not in maintenance.casefold():
            failures.append(
                f"feature-support.md: contributor guidance omits {term!r}"
            )

    for match in re.finditer(r"https://github\.com/[^)\s]+/issues/[^)\s]+", matrix):
        if ISSUE_LINK.fullmatch(match.group(0)) is None:
            line = matrix.count("\n", 0, match.start()) + 1
            failures.append(
                f"feature-support.md:{line}: non-canonical issue link {match.group(0)!r}"
            )

    return failures


def validate_release_dry_run(release_acceptance: str) -> list[str]:
    failures: list[str] = []
    section = release_acceptance.partition(
        "## Current working-tree release-gate dry run"
    )[2].partition("## Current automated core-conformance baseline")[0]
    if not section:
        return [
            "release-acceptance.md: missing current working-tree release-gate dry run"
        ]

    gates = [int(match.group(1)) for match in RELEASE_DRY_RUN_ROW.finditer(section)]
    if gates != list(range(1, 10)):
        failures.append(
            "release-acceptance.md: current dry run must contain gates 1 through 9 in order"
        )
    for term in ["provisional pass", "blocked", "exact candidate"]:
        if term not in section.casefold():
            failures.append(
                f"release-acceptance.md: current dry run omits {term!r}"
            )
    return failures


def main() -> int:
    failures = validate(
        (ROOT / "README.md").read_text(encoding="utf-8"),
        (ROOT / "docs/feature-support.md").read_text(encoding="utf-8"),
    )
    failures.extend(
        validate_release_dry_run(
            (ROOT / "docs/release-acceptance.md").read_text(encoding="utf-8")
        )
    )
    if failures:
        print("Feature-support contract failures:", file=sys.stderr)
        print("\n".join(failures), file=sys.stderr)
        return 1
    print(
        "Validated feature-support statuses, README highlights, release gates, dry run, and issue links"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
