#!/usr/bin/env python3
"""Report deterministic logical size accounting for a Cargo target directory."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import stat
import sys
from typing import Any


SCHEMA_VERSION = 1
REPORT_KIND = "termeric-cargo-build-footprint"
DEFAULT_LARGE_FILE_BYTES = 100 * 1024 * 1024
DEFAULT_LARGE_FILE_LIMIT = 20
KNOWN_PROFILE_DIRECTORIES = frozenset(
    {"debug", "release", "profiling", "debugging", "bench"}
)
CLASSES = (
    "build-scripts",
    "dependencies",
    "documentation",
    "examples-benches",
    "incremental",
    "metadata",
    "other",
    "top-level-products",
)


class FootprintError(RuntimeError):
    """Raised when a target directory cannot be accounted for exactly."""


def positive_integer(raw: str) -> int:
    try:
        value = int(raw)
    except ValueError as error:
        raise argparse.ArgumentTypeError("must be an integer") from error
    if value <= 0:
        raise argparse.ArgumentTypeError("must be greater than zero")
    return value


def nonnegative_integer(raw: str) -> int:
    try:
        value = int(raw)
    except ValueError as error:
        raise argparse.ArgumentTypeError("must be an integer") from error
    if value < 0:
        raise argparse.ArgumentTypeError("must be zero or greater")
    return value


def empty_totals() -> dict[str, int]:
    return {"bytes": 0, "files": 0}


def add_totals(destination: dict[str, int], size: int) -> None:
    destination["bytes"] += size
    destination["files"] += 1


def profile_and_class(parts: tuple[str, ...]) -> tuple[str, str]:
    """Classify a target-relative regular file without inspecting its name hash."""
    if not parts:
        raise FootprintError("cannot classify an empty relative path")

    profile = "<root>"
    profile_index: int | None = None
    if parts[0] in KNOWN_PROFILE_DIRECTORIES:
        profile = parts[0]
        profile_index = 0
    elif len(parts) > 1 and parts[1] in KNOWN_PROFILE_DIRECTORIES:
        profile = f"{parts[0]}/{parts[1]}"
        profile_index = 1

    if parts[0] == "doc":
        return "doc", "documentation"
    if profile_index is None:
        return profile, "other"

    within_profile = parts[profile_index + 1 :]
    if not within_profile:
        return profile, "other"
    artifact_root = within_profile[0]
    if artifact_root == "incremental":
        artifact_class = "incremental"
    elif artifact_root == "deps":
        artifact_class = "dependencies"
    elif artifact_root == "build":
        artifact_class = "build-scripts"
    elif artifact_root in ("examples", "benches"):
        artifact_class = "examples-benches"
    elif artifact_root == ".fingerprint":
        artifact_class = "metadata"
    elif len(within_profile) == 1:
        artifact_class = "top-level-products"
    else:
        artifact_class = "other"
    return profile, artifact_class


def scan_regular_files(root: Path) -> tuple[list[tuple[str, int]], int, int]:
    """Return target-relative files and explicit skipped-entry counts."""
    try:
        root_status = root.lstat()
    except OSError as error:
        raise FootprintError(f"cannot inspect target directory {root}: {error}") from error
    if stat.S_ISLNK(root_status.st_mode):
        raise FootprintError(f"target directory must not be a symlink: {root}")
    if not stat.S_ISDIR(root_status.st_mode):
        raise FootprintError(f"target path is not a directory: {root}")

    files: list[tuple[str, int]] = []
    symlinks = 0
    special = 0
    pending: list[tuple[Path, tuple[str, ...]]] = [(root, ())]
    while pending:
        directory, prefix = pending.pop()
        try:
            with os.scandir(directory) as entries:
                ordered = sorted(entries, key=lambda entry: entry.name)
        except OSError as error:
            relative = "/".join(prefix) or "."
            raise FootprintError(f"cannot read target directory {relative}: {error}") from error

        directories: list[tuple[Path, tuple[str, ...]]] = []
        for entry in ordered:
            parts = (*prefix, entry.name)
            try:
                if entry.is_symlink():
                    symlinks += 1
                elif entry.is_dir(follow_symlinks=False):
                    directories.append((Path(entry.path), parts))
                elif entry.is_file(follow_symlinks=False):
                    entry_status = entry.stat(follow_symlinks=False)
                    if not stat.S_ISREG(entry_status.st_mode):
                        special += 1
                        continue
                    files.append(("/".join(parts), entry_status.st_size))
                else:
                    special += 1
            except OSError as error:
                relative = "/".join(parts)
                raise FootprintError(f"cannot inspect target entry {relative}: {error}") from error
        pending.extend(reversed(directories))

    files.sort(key=lambda item: item[0])
    return files, symlinks, special


def build_report(
    target_directory: Path,
    *,
    large_file_bytes: int = DEFAULT_LARGE_FILE_BYTES,
    large_file_limit: int = DEFAULT_LARGE_FILE_LIMIT,
    warn_bytes: int | None = None,
    warn_files: int | None = None,
    max_bytes: int | None = None,
    max_files: int | None = None,
) -> dict[str, Any]:
    if large_file_bytes <= 0:
        raise FootprintError("large-file threshold must be greater than zero")
    if large_file_limit < 0:
        raise FootprintError("large-file limit must be zero or greater")
    for name, value in (
        ("warning bytes", warn_bytes),
        ("warning files", warn_files),
        ("maximum bytes", max_bytes),
        ("maximum files", max_files),
    ):
        if value is not None and value < 0:
            raise FootprintError(f"{name} must be zero or greater")
    if warn_bytes is not None and max_bytes is not None and warn_bytes > max_bytes:
        raise FootprintError("warning bytes must not exceed maximum bytes")
    if warn_files is not None and max_files is not None and warn_files > max_files:
        raise FootprintError("warning files must not exceed maximum files")

    root = target_directory.absolute()
    files, symlinks, special = scan_regular_files(root)
    totals = empty_totals()
    profiles: dict[str, dict[str, int]] = {}
    classes = {name: empty_totals() for name in CLASSES}
    large_files: list[dict[str, int | str]] = []

    for relative, size in files:
        add_totals(totals, size)
        profile, artifact_class = profile_and_class(tuple(relative.split("/")))
        add_totals(profiles.setdefault(profile, empty_totals()), size)
        add_totals(classes[artifact_class], size)
        if size >= large_file_bytes:
            large_files.append({"path": relative, "bytes": size})

    large_files.sort(key=lambda item: (-int(item["bytes"]), str(item["path"])))
    large_files = large_files[:large_file_limit]
    limits = {
        "warn_bytes": warn_bytes,
        "warn_files": warn_files,
        "max_bytes": max_bytes,
        "max_files": max_files,
    }
    warnings: list[str] = []
    violations: list[str] = []
    if max_bytes is not None and totals["bytes"] > max_bytes:
        violations.append(f"bytes {totals['bytes']} exceed maximum {max_bytes}")
    elif warn_bytes is not None and totals["bytes"] > warn_bytes:
        warnings.append(f"bytes {totals['bytes']} exceed warning {warn_bytes}")
    if max_files is not None and totals["files"] > max_files:
        violations.append(f"files {totals['files']} exceed maximum {max_files}")
    elif warn_files is not None and totals["files"] > warn_files:
        warnings.append(f"files {totals['files']} exceed warning {warn_files}")

    status = "fail" if violations else "warning" if warnings else "pass"

    return {
        "schema_version": SCHEMA_VERSION,
        "kind": REPORT_KIND,
        "target_directory": os.fspath(root),
        "size_semantics": "sum-of-regular-file-logical-bytes",
        "status": status,
        "totals": totals,
        "profiles": {name: profiles[name] for name in sorted(profiles)},
        "classes": classes,
        "skipped": {"symlinks": symlinks, "special_files": special},
        "large_file_threshold_bytes": large_file_bytes,
        "large_files": large_files,
        "limits": limits,
        "warnings": warnings,
        "violations": violations,
    }


def human_bytes(value: int) -> str:
    units = ("B", "KiB", "MiB", "GiB", "TiB")
    amount = float(value)
    unit = units[0]
    for unit in units:
        if amount < 1024.0 or unit == units[-1]:
            break
        amount /= 1024.0
    return f"{value} B ({amount:.2f} {unit})"


def render_human(report: dict[str, Any]) -> str:
    lines = [
        f"Cargo target: {report['target_directory']}",
        f"Status: {report['status']}",
        f"Logical size: {human_bytes(report['totals']['bytes'])}",
        f"Regular files: {report['totals']['files']}",
        (
            "Skipped entries: "
            f"{report['skipped']['symlinks']} symlink(s), "
            f"{report['skipped']['special_files']} special file(s)"
        ),
        "Profiles:",
    ]
    for name, totals in report["profiles"].items():
        lines.append(
            f"  {name}: {human_bytes(totals['bytes'])}, {totals['files']} file(s)"
        )
    lines.append("Artifact classes:")
    for name, totals in report["classes"].items():
        lines.append(
            f"  {name}: {human_bytes(totals['bytes'])}, {totals['files']} file(s)"
        )
    lines.append(f"Large files (>= {report['large_file_threshold_bytes']} B):")
    if report["large_files"]:
        for item in report["large_files"]:
            lines.append(f"  {human_bytes(item['bytes'])}: {item['path']}")
    else:
        lines.append("  none")
    if report["warnings"]:
        lines.append("Limit warnings:")
        lines.extend(f"  {warning}" for warning in report["warnings"])
    if report["violations"]:
        lines.append("Limit violations:")
        lines.extend(f"  {violation}" for violation in report["violations"])
    return "\n".join(lines)


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("target_directory", type=Path)
    parser.add_argument("--json", action="store_true", help="emit versioned JSON")
    parser.add_argument("--warn-bytes", type=nonnegative_integer)
    parser.add_argument("--warn-files", type=nonnegative_integer)
    parser.add_argument("--max-bytes", type=nonnegative_integer)
    parser.add_argument("--max-files", type=nonnegative_integer)
    parser.add_argument(
        "--large-file-bytes",
        type=positive_integer,
        default=DEFAULT_LARGE_FILE_BYTES,
    )
    parser.add_argument(
        "--large-file-limit",
        type=nonnegative_integer,
        default=DEFAULT_LARGE_FILE_LIMIT,
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    try:
        report = build_report(
            args.target_directory,
            large_file_bytes=args.large_file_bytes,
            large_file_limit=args.large_file_limit,
            warn_bytes=args.warn_bytes,
            warn_files=args.warn_files,
            max_bytes=args.max_bytes,
            max_files=args.max_files,
        )
    except FootprintError as error:
        print(f"build-footprint: {error}", file=sys.stderr)
        return 2
    if args.json:
        print(json.dumps(report, indent=2, sort_keys=True))
    else:
        print(render_human(report))
    return 1 if report["status"] == "fail" else 0


if __name__ == "__main__":
    raise SystemExit(main())
