#!/usr/bin/env python3
"""Inspect Termeric's Arch package, and optionally install and remove it.

Inspection is read-only. --system-lifecycle changes the host's packages, so it
needs root on a disposable Arch machine without Termeric installed.
"""

from __future__ import annotations

import argparse
import os
from pathlib import Path, PurePosixPath
import re
import stat
import subprocess
import sys
import tempfile

if __package__:
    from scripts.check_debian_package import (
        CARGO_LOCK,
        ICON_FILES,
        CheckError,
        parse_os_release,
        require_tool,
        run,
        validate_desktop_entry,
        validate_icons,
        validate_notice,
    )
else:
    from check_debian_package import (
        CARGO_LOCK,
        ICON_FILES,
        CheckError,
        parse_os_release,
        require_tool,
        run,
        validate_desktop_entry,
        validate_icons,
        validate_notice,
    )


PACKAGE_NAME = "termeric-git"
CONFLICTING_PACKAGE_NAME = "termeric"
TERMINFO_NAME = "xterm-termeric"
EXPECTED_FILES = {
    "usr/bin/termeric": 0o755,
    "usr/share/applications/termeric.desktop": 0o644,
    **{path: 0o644 for path in ICON_FILES},
    "usr/share/terminfo/x/xterm-termeric": 0o644,
    "usr/share/licenses/termeric-git/LICENSE": 0o644,
    "usr/share/doc/termeric/README.md": 0o644,
    "usr/share/doc/termeric/CHANGELOG.md": 0o644,
    "usr/share/doc/termeric/THIRD_PARTY_NOTICES.md": 0o644,
}
METADATA_FILES = {".BUILDINFO", ".MTREE", ".PKGINFO"}
REQUIRED_DEPENDENCIES = {
    "fontconfig",
    "gcc-libs",
    "glibc",
    "libx11",
    "libxkbcommon",
    "wayland",
    "zlib",
}
REQUIRED_LICENSES = {"MIT", "Unicode-3.0"}


def normalize_member_name(raw_name: str) -> str:
    name = raw_name.removeprefix("./").rstrip("/")
    if not name or name.startswith("/") or "\\" in name:
        raise CheckError(f"unsafe package member path: {raw_name!r}")
    if any(ord(character) < 32 or ord(character) == 127 for character in name):
        raise CheckError(f"control character in package member path: {raw_name!r}")
    path = PurePosixPath(name)
    if any(part in {"", ".", ".."} for part in path.parts):
        raise CheckError(f"unsafe package member path: {raw_name!r}")
    return str(path)


def expected_directories() -> set[str]:
    directories: set[str] = set()
    for name in EXPECTED_FILES:
        parent = PurePosixPath(name).parent
        while str(parent) != ".":
            directories.add(str(parent))
            parent = parent.parent
    return directories


def validate_archive_inventory(names: list[str], member_types: list[str]) -> list[str]:
    if len(names) != len(member_types):
        raise CheckError("archive name and type listings have different lengths")
    normalized: list[str] = []
    seen: set[str] = set()
    directories = expected_directories()
    for raw_name, member_type in zip(names, member_types, strict=True):
        name = normalize_member_name(raw_name)
        if name in seen:
            raise CheckError(f"duplicate package member: {name}")
        seen.add(name)
        if member_type not in {"-", "d"}:
            raise CheckError(f"package member {name!r} has unsafe type {member_type!r}")
        if member_type == "d":
            if name not in directories:
                raise CheckError(f"unexpected package directory: {name}")
        elif name not in EXPECTED_FILES and name not in METADATA_FILES:
            raise CheckError(f"unexpected package file: {name}")
        normalized.append(name)
    missing = sorted(
        (set(EXPECTED_FILES) | METADATA_FILES | expected_directories()) - seen
    )
    if missing:
        raise CheckError("package is missing members: " + ", ".join(missing))
    return normalized


def parse_verbose_types(
    listing: str, *, require_root_ownership: bool = False
) -> list[str]:
    types: list[str] = []
    for line in listing.splitlines():
        if not line:
            continue
        fields = line.split()
        if len(fields) < 4:
            raise CheckError(f"could not parse archive member metadata: {line!r}")
        member_type = line[0]
        if member_type not in {"-", "d", "l", "h", "c", "b", "p", "s"}:
            raise CheckError(f"could not parse archive member type: {line!r}")
        if require_root_ownership and fields[2:4] != ["0", "0"]:
            raise CheckError(
                f"package member is not owned by root:root: {line!r}"
            )
        types.append(member_type)
    return types


def parse_pkginfo(text: str) -> dict[str, list[str]]:
    fields: dict[str, list[str]] = {}
    for raw_line in text.splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        key, separator, value = line.partition(" = ")
        if not separator or not key or not value:
            raise CheckError(f"malformed .PKGINFO line: {raw_line!r}")
        fields.setdefault(key, []).append(value)
    return fields


def one_field(fields: dict[str, list[str]], name: str) -> str:
    values = fields.get(name, [])
    if len(values) != 1:
        raise CheckError(f".PKGINFO requires exactly one {name} field")
    return values[0]


def dependency_name(value: str) -> str:
    return re.split(r"[<>=]", value, maxsplit=1)[0]


def validate_package_metadata(fields: dict[str, list[str]]) -> dict[str, object]:
    name = one_field(fields, "pkgname")
    version = one_field(fields, "pkgver")
    architecture = one_field(fields, "arch")
    if name != PACKAGE_NAME:
        raise CheckError(f"package name is {name!r}, expected {PACKAGE_NAME!r}")
    if not re.fullmatch(r"[0-9][0-9A-Za-z.+_:-]*", version):
        raise CheckError(f"package version is invalid: {version!r}")
    if architecture != "x86_64":
        raise CheckError(f"package architecture is {architecture!r}, expected 'x86_64'")
    dependencies = {dependency_name(value) for value in fields.get("depend", [])}
    missing_dependencies = sorted(REQUIRED_DEPENDENCIES - dependencies)
    if missing_dependencies:
        raise CheckError(
            "package metadata is missing dependencies: " + ", ".join(missing_dependencies)
        )
    if "termeric" not in fields.get("provides", []):
        raise CheckError("package does not provide termeric")
    if "termeric" not in fields.get("conflict", []):
        raise CheckError("package does not conflict with termeric")
    missing_licenses = sorted(REQUIRED_LICENSES - set(fields.get("license", [])))
    if missing_licenses:
        raise CheckError(
            "package metadata is missing licenses: " + ", ".join(missing_licenses)
        )
    return {
        "name": name,
        "version": version,
        "architecture": architecture,
        "dependencies": sorted(dependencies),
    }


def validate_layout(root: Path, *, exercise_native_tools: bool = True) -> None:
    for relative, expected_mode in EXPECTED_FILES.items():
        path = root / relative
        if path.is_symlink() or not path.is_file():
            raise CheckError(f"package is missing regular file /{relative}")
        actual_mode = stat.S_IMODE(path.stat().st_mode)
        if actual_mode != expected_mode:
            raise CheckError(
                f"/{relative} has mode {actual_mode:o}, expected {expected_mode:o}"
            )
    binary = root / "usr/bin/termeric"
    if binary.read_bytes()[:4] != b"\x7fELF":
        raise CheckError("/usr/bin/termeric is not an ELF executable")
    validate_icons(root)
    desktop = root / "usr/share/applications/termeric.desktop"
    validate_desktop_entry(desktop)
    notice = root / "usr/share/doc/termeric/THIRD_PARTY_NOTICES.md"
    validate_notice(notice.read_text(encoding="utf-8"), CARGO_LOCK.read_text(encoding="utf-8"))
    if not exercise_native_tools:
        return
    run([require_tool("desktop-file-validate"), str(desktop)])
    listing = run(
        [
            require_tool("infocmp"),
            "-x",
            "-A",
            str(root / "usr/share/terminfo"),
            TERMINFO_NAME,
        ]
    ).stdout
    if TERMINFO_NAME not in listing:
        raise CheckError("installed terminfo entry has the wrong identity")
    linked = run([require_tool("ldd"), str(binary)]).stdout
    if "not found" in linked:
        raise CheckError("packaged binary has unresolved dynamic libraries")
    version = run([str(binary), "--version"], timeout=30).stdout.strip()
    if not re.fullmatch(r"termeric \d+\.\d+\.\d+(?:[-+][^\s]+)?", version):
        raise CheckError(f"packaged non-GUI version probe returned {version!r}")


def validate_archive_modes(root: Path) -> None:
    for relative in expected_directories():
        path = root / relative
        if path.is_symlink() or not path.is_dir():
            raise CheckError(f"package is missing directory /{relative}")
        actual_mode = stat.S_IMODE(path.stat().st_mode)
        if actual_mode != 0o755:
            raise CheckError(f"/{relative} has mode {actual_mode:o}, expected 755")
    for relative in METADATA_FILES:
        path = root / relative
        actual_mode = stat.S_IMODE(path.stat().st_mode)
        if actual_mode != 0o644:
            raise CheckError(f"/{relative} has mode {actual_mode:o}, expected 644")


def inspect_archive(package: Path) -> dict[str, object]:
    if not package.is_file():
        raise CheckError(f"Arch package does not exist: {package}")
    bsdtar = require_tool("bsdtar")
    names = run([bsdtar, "-tf", str(package)]).stdout.splitlines()
    types = parse_verbose_types(
        run([bsdtar, "--numeric-owner", "-tvf", str(package)]).stdout,
        require_root_ownership=True,
    )
    validate_archive_inventory(names, types)
    with tempfile.TemporaryDirectory(prefix="termeric-arch-inspect-") as directory:
        root = Path(directory) / "root"
        root.mkdir()
        run([bsdtar, "-xf", str(package), "-C", str(root)])
        metadata = validate_package_metadata(
            parse_pkginfo((root / ".PKGINFO").read_text(encoding="utf-8"))
        )
        validate_archive_modes(root)
        validate_layout(root)
    print(f"ok archive: {package.name} ({metadata['version']})")
    return metadata


def installed_version(package_name: str = PACKAGE_NAME) -> str | None:
    result = subprocess.run(
        [require_tool("pacman"), "-Q", package_name],
        capture_output=True,
        text=True,
        timeout=30,
    )
    if result.returncode != 0:
        return None
    name, separator, version = result.stdout.strip().partition(" ")
    return version if name == package_name and separator else None


def require_lifecycle_host() -> None:
    if os.geteuid() != 0:
        raise CheckError("system lifecycle mode must run as root")
    if "arch" not in parse_os_release():
        raise CheckError("system lifecycle mode requires an Arch-family host")
    for tool in ["bsdtar", "pacman", "vercmp"]:
        require_tool(tool)
    for name in [PACKAGE_NAME, CONFLICTING_PACKAGE_NAME]:
        prior = installed_version(name)
        if prior is not None:
            raise CheckError(f"refusing to replace pre-existing {name} package {prior}")


def verify_installed(expected_version: str) -> None:
    actual = installed_version()
    if actual != expected_version:
        raise CheckError(
            f"installed package version is {actual!r}, expected {expected_version!r}"
        )
    validate_layout(Path("/"))


def verify_removed() -> None:
    if installed_version() is not None:
        raise CheckError(f"{PACKAGE_NAME} remains installed after removal")
    leftovers = [relative for relative in EXPECTED_FILES if (Path("/") / relative).exists()]
    if leftovers:
        raise CheckError(
            "package-owned files remain after removal: "
            + ", ".join(f"/{path}" for path in leftovers)
        )


def exercise_system_lifecycle(package: Path, version: str) -> None:
    require_lifecycle_host()
    pacman = require_tool("pacman")
    try:
        run([pacman, "-U", "--noconfirm", str(package.resolve())], timeout=600)
        verify_installed(version)
        print(f"ok install: {version}")
    finally:
        if installed_version() is not None:
            run([pacman, "-R", "--noconfirm", PACKAGE_NAME], timeout=600)
    verify_removed()
    print("ok remove: package files removed")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("package", type=Path)
    parser.add_argument(
        "--system-lifecycle",
        action="store_true",
        help="install and remove the package on this disposable host",
    )
    arguments = parser.parse_args(argv)
    try:
        metadata = inspect_archive(arguments.package)
        if arguments.system_lifecycle:
            exercise_system_lifecycle(arguments.package, str(metadata["version"]))
    except (CheckError, OSError, subprocess.SubprocessError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
