#!/usr/bin/env python3
"""Inspect a Termeric Arch package, and optionally install and remove it.

Both AUR packages are checked: termeric, built from a release's source, and
termeric-git, built from the latest commit. They install the same files.

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
        CheckError,
        packaged_path,
        parse_os_release,
        require_tool,
        run,
        validate_desktop_entries,
        validate_icons,
        validate_notice,
    )
    from scripts.stage_install_tree import manifest
else:
    from check_debian_package import (
        CARGO_LOCK,
        CheckError,
        packaged_path,
        parse_os_release,
        require_tool,
        run,
        validate_desktop_entries,
        validate_icons,
        validate_notice,
    )
    from stage_install_tree import manifest


RELEASE_PACKAGE_NAME = "termeric"
DEVELOPMENT_PACKAGE_NAME = "termeric-git"
PACKAGE_NAMES = (RELEASE_PACKAGE_NAME, DEVELOPMENT_PACKAGE_NAME)
TERMINFO_NAME = "xterm-termeric"
METADATA_FILES = {".BUILDINFO", ".MTREE", ".PKGINFO"}
# The depends array of both PKGBUILDs in packaging/arch/. Most are libraries
# the binary loads at runtime, which makepkg can't detect, so a missing one
# would only show up as a window that fails to open.
REQUIRED_DEPENDENCIES = {
    "fontconfig",
    "glibc",
    "hicolor-icon-theme",
    "libgcc",
    "libglvnd",
    "libx11",
    "libxcb",
    "libxcursor",
    "libxi",
    "libxkbcommon",
    "libxkbcommon-x11",
    "ttf-font",
    "vulkan-icd-loader",
    "wayland",
    "zlib",
}
# The license array of both PKGBUILDs.
REQUIRED_LICENSES = {
    "MIT",
    "0BSD",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "BSD-3-Clause WITH PCRE2-exception",
    "CC0-1.0",
    "GPL-2.0-only WITH GCC-exception-2.0",
    "ISC",
    "LGPL-2.1-or-later",
    "MPL-2.0",
    "Unicode-3.0",
    "Unicode-DFS-2016",
    "Zlib",
}


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


def expected_files(package_name: str) -> dict[str, int]:
    """The staged install tree under /usr, with the manual page compressed by
    makepkg's zipman, and the license where Arch keeps it."""
    return {
        **{packaged_path(path): mode for path, mode in manifest().items()},
        f"usr/share/licenses/{package_name}/LICENSE": 0o644,
    }


def expected_directories(package_name: str) -> set[str]:
    directories: set[str] = set()
    for name in expected_files(package_name):
        parent = PurePosixPath(name).parent
        while str(parent) != ".":
            directories.add(str(parent))
            parent = parent.parent
    return directories


def validate_archive_inventory(
    names: list[str], member_types: list[str], package_name: str
) -> list[str]:
    if len(names) != len(member_types):
        raise CheckError("archive name and type listings have different lengths")
    files = expected_files(package_name)
    normalized: list[str] = []
    seen: set[str] = set()
    directories = expected_directories(package_name)
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
        elif name not in files and name not in METADATA_FILES:
            raise CheckError(f"unexpected package file: {name}")
        normalized.append(name)
    missing = sorted(
        (set(files) | METADATA_FILES | directories) - seen
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


# The architectures both PKGBUILDs' arch=() list.
SUPPORTED_ARCHITECTURES = ("x86_64", "aarch64")


def dependency_name(value: str) -> str:
    return re.split(r"[<>=]", value, maxsplit=1)[0]


def validate_package_metadata(fields: dict[str, list[str]]) -> dict[str, object]:
    name = one_field(fields, "pkgname")
    version = one_field(fields, "pkgver")
    architecture = one_field(fields, "arch")
    if name not in PACKAGE_NAMES:
        expected = " or ".join(repr(name) for name in PACKAGE_NAMES)
        raise CheckError(f"package name is {name!r}, expected {expected}")
    if not re.fullmatch(r"[0-9][0-9A-Za-z.+_:-]*", version):
        raise CheckError(f"package version is invalid: {version!r}")
    if architecture not in SUPPORTED_ARCHITECTURES:
        expected = " or ".join(repr(name) for name in SUPPORTED_ARCHITECTURES)
        raise CheckError(f"package architecture is {architecture!r}, expected {expected}")
    dependencies = {dependency_name(value) for value in fields.get("depend", [])}
    missing_dependencies = sorted(REQUIRED_DEPENDENCIES - dependencies)
    if missing_dependencies:
        raise CheckError(
            "package metadata is missing dependencies: " + ", ".join(missing_dependencies)
        )
    # The development package stands in for the release package; pacman
    # checks conflicts both ways, so only one side needs to declare it.
    if name == DEVELOPMENT_PACKAGE_NAME:
        provided = {dependency_name(value) for value in fields.get("provides", [])}
        if RELEASE_PACKAGE_NAME not in provided:
            raise CheckError(f"package does not provide {RELEASE_PACKAGE_NAME}")
        if RELEASE_PACKAGE_NAME not in fields.get("conflict", []):
            raise CheckError(f"package does not conflict with {RELEASE_PACKAGE_NAME}")
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


def validate_layout(
    root: Path, package_name: str, *, exercise_native_tools: bool = True
) -> None:
    for relative, expected_mode in expected_files(package_name).items():
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
    validate_desktop_entries(root, exercise_native_tools=exercise_native_tools)
    notice = root / "usr/share/doc/termeric/THIRD_PARTY_NOTICES.md"
    validate_notice(notice.read_text(encoding="utf-8"), CARGO_LOCK.read_text(encoding="utf-8"))
    if not exercise_native_tools:
        return
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


def validate_archive_modes(root: Path, package_name: str) -> None:
    for relative in expected_directories(package_name):
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
    # The package's name decides where its license goes, so read it first.
    metadata = validate_package_metadata(
        parse_pkginfo(run([bsdtar, "-xOf", str(package), ".PKGINFO"]).stdout)
    )
    name = str(metadata["name"])
    validate_archive_inventory(names, types, name)
    with tempfile.TemporaryDirectory(prefix="termeric-arch-inspect-") as directory:
        root = Path(directory) / "root"
        root.mkdir()
        run([bsdtar, "-xf", str(package), "-C", str(root)])
        validate_archive_modes(root, name)
        validate_layout(root, name)
    print(f"ok archive: {package.name} ({metadata['version']})")
    return metadata


def installed_version(package_name: str) -> str | None:
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
    for name in PACKAGE_NAMES:
        prior = installed_version(name)
        if prior is not None:
            raise CheckError(f"refusing to replace pre-existing {name} package {prior}")


def verify_installed(package_name: str, expected_version: str) -> None:
    actual = installed_version(package_name)
    if actual != expected_version:
        raise CheckError(
            f"installed package version is {actual!r}, expected {expected_version!r}"
        )
    validate_layout(Path("/"), package_name)


def verify_removed(package_name: str) -> None:
    if installed_version(package_name) is not None:
        raise CheckError(f"{package_name} remains installed after removal")
    leftovers = [
        relative for relative in expected_files(package_name) if (Path("/") / relative).exists()
    ]
    if leftovers:
        raise CheckError(
            "package-owned files remain after removal: "
            + ", ".join(f"/{path}" for path in leftovers)
        )


def exercise_system_lifecycle(package: Path, package_name: str, version: str) -> None:
    require_lifecycle_host()
    pacman = require_tool("pacman")
    try:
        run([pacman, "-U", "--noconfirm", str(package.resolve())], timeout=600)
        verify_installed(package_name, version)
        print(f"ok install: {package_name} {version}")
    finally:
        if installed_version(package_name) is not None:
            run([pacman, "-R", "--noconfirm", package_name], timeout=600)
    verify_removed(package_name)
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
            exercise_system_lifecycle(
                arguments.package, str(metadata["name"]), str(metadata["version"])
            )
    except (CheckError, OSError, subprocess.SubprocessError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
