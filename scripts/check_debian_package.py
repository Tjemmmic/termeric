#!/usr/bin/env python3
"""Inspect Termeric's Debian package, and optionally install and remove it.

Inspection is read-only. --system-lifecycle changes the host's packages, so it
needs root on a disposable Debian-family machine without Termeric installed.
"""

from __future__ import annotations

import argparse
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tempfile

if __package__:
    from scripts.stage_install_tree import APP_ID, ICON_SIZES, QUICK_ACCESS_APP_ID, manifest
else:
    from stage_install_tree import APP_ID, ICON_SIZES, QUICK_ACCESS_APP_ID, manifest


ROOT = Path(__file__).resolve().parent.parent
CARGO_LOCK = ROOT / "Cargo.lock"
PACKAGE_NAME = "termeric"
TERMINFO_NAME = "xterm-termeric"
SOFTWARE_RENDERER_PACKAGE = "mesa-vulkan-drivers"
# The depends in termeric/Cargo.toml beside $auto. dpkg-shlibdeps sees only the
# linked libraries, so a missing one would only show up as a window that fails
# to open.
RUNTIME_DEPENDENCIES = {
    "fontconfig-config",
    "libegl1",
    "libvulkan1",
    "libwayland-client0",
    "libwayland-cursor0",
    "libwayland-egl1",
    "libx11-6",
    "libx11-xcb1",
    "libxcb1",
    "libxcursor1",
    "libxi6",
    "libxkbcommon0",
    "libxkbcommon-x11-0",
}
ICON_FILES = {
    f"usr/share/icons/hicolor/{size}x{size}/apps/{APP_ID}.png": size for size in ICON_SIZES
}
# The keys each installed desktop entry must carry, by path.
DESKTOP_ENTRIES = {
    f"usr/share/applications/{APP_ID}.desktop": {
        ("Desktop Entry", "Type"): "Application",
        ("Desktop Entry", "Exec"): "termeric",
        ("Desktop Entry", "Icon"): APP_ID,
        ("Desktop Entry", "Terminal"): "false",
        ("Desktop Entry", "StartupWMClass"): APP_ID,
        ("Desktop Action NewWindow", "Exec"): "termeric --new-window",
    },
    f"usr/share/applications/{QUICK_ACCESS_APP_ID}.desktop": {
        ("Desktop Entry", "Type"): "Application",
        ("Desktop Entry", "Exec"): "termeric --quick-access",
        ("Desktop Entry", "Icon"): APP_ID,
        ("Desktop Entry", "Terminal"): "false",
        ("Desktop Entry", "NoDisplay"): "true",
        ("Desktop Entry", "StartupWMClass"): QUICK_ACCESS_APP_ID,
    },
}


def packaged_path(path: str) -> str:
    """Where a package puts a staged file under /usr.

    Distribution packaging compresses manual pages: Debian policy has cargo-deb
    gzip them, and makepkg's default zipman option does the same on Arch.
    """
    return f"usr/{path}.gz" if path.startswith("share/man/") else f"usr/{path}"


# The staged install tree under /usr, and the copyright file cargo-deb writes.
EXPECTED_FILES = {
    **{packaged_path(path): mode for path, mode in manifest().items()},
    "usr/share/doc/termeric/copyright": 0o644,
}


class CheckError(RuntimeError):
    pass


def validate_icons(root: Path) -> None:
    """Every installed icon is a PNG whose pixel size matches its directory."""
    for relative, size in ICON_FILES.items():
        header = (root / relative).read_bytes()[:24]
        if header[:8] != b"\x89PNG\r\n\x1a\n":
            raise CheckError(f"/{relative} is not a PNG file")
        width = int.from_bytes(header[16:20], "big")
        height = int.from_bytes(header[20:24], "big")
        if (width, height) != (size, size):
            raise CheckError(f"/{relative} is {width}x{height}, expected {size}x{size}")


def locked_libgit2_version(lock_text: str) -> str:
    """The libgit2 release that the locked libgit2-sys bundles (0.17.0+1.8.1 is 1.8.1)."""
    versions = re.findall(
        r'^name = "libgit2-sys"\nversion = "[^"+]+\+([^"]+)"$', lock_text, re.MULTILINE
    )
    if len(versions) != 1:
        raise CheckError(
            "expected one libgit2-sys entry with a bundled libgit2 version in Cargo.lock, "
            f"found {len(versions)}"
        )
    return versions[0]


def validate_notice(notice_text: str, lock_text: str) -> None:
    """The third-party notice carries the Unicode license, names the libgit2 that is
    linked, and lists at least one crate under every crate license.

    The libgit2 section is pasted by hand into packaging/notices/notices.hbs, so a
    libgit2-sys upgrade leaves it stale until someone refreshes it. A license used
    only by a crate cargo-about should have skipped (Termeric's own, unless they
    are unpublished) would show up as a section that lists no crates.
    """
    if "Unicode License v3" not in notice_text:
        raise CheckError("third-party notice omits the Unicode License v3")
    if re.search(r"^Used by:\s*```", notice_text, re.MULTILINE):
        raise CheckError(
            "third-party notice has a license section that lists no crates; keep "
            "Termeric's own crates unpublished (publish = false) so cargo-about skips them"
        )
    expected = locked_libgit2_version(lock_text)
    named = re.findall(r"\blinks libgit2 (\d+\.\d+\.\d+)\b", notice_text)
    if named != [expected]:
        raise CheckError(
            f"third-party notice names libgit2 {', '.join(named) or 'nowhere'}, but Cargo.lock "
            f"bundles libgit2 {expected}; refresh the libgit2 section of "
            "packaging/notices/notices.hbs (see Dependencies in CONTRIBUTING.md)"
        )


def require_tool(name: str) -> str:
    path = shutil.which(name)
    if path is None:
        raise CheckError(f"required tool is unavailable: {name}")
    return path


def run(
    command: list[str],
    *,
    env: dict[str, str] | None = None,
    timeout: int = 180,
) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        command,
        env=env,
        capture_output=True,
        text=True,
        timeout=timeout,
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip()
        raise CheckError(
            f"{' '.join(command)} failed ({result.returncode}): {detail}"
        )
    return result


def package_field(package: Path, field: str) -> str:
    return run([require_tool("dpkg-deb"), "--field", str(package), field]).stdout.strip()


def parse_desktop_entry(text: str) -> dict[tuple[str, str], str]:
    values: dict[tuple[str, str], str] = {}
    section = ""
    for raw_line in text.splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("[") and line.endswith("]"):
            section = line[1:-1]
            continue
        if "=" not in line or not section:
            raise CheckError(f"malformed desktop entry line: {raw_line!r}")
        key, value = line.split("=", 1)
        values[(section, key)] = value
    return values


def validate_desktop_entry(path: Path, required: dict[tuple[str, str], str]) -> None:
    values = parse_desktop_entry(path.read_text(encoding="utf-8"))
    for key, expected in required.items():
        actual = values.get(key)
        if actual != expected:
            raise CheckError(
                f"desktop entry {key[0]}/{key[1]} is {actual!r}, expected {expected!r}"
            )
    actions = values.get(("Desktop Entry", "Actions"), "").split(";")
    for section, _key in required:
        action = section.removeprefix("Desktop Action ")
        if action != section and action not in actions:
            raise CheckError(f"desktop entry does not expose the {action} action")


def validate_desktop_entries(root: Path, *, exercise_native_tools: bool) -> None:
    for relative, required in DESKTOP_ENTRIES.items():
        validate_desktop_entry(root / relative, required)
        if exercise_native_tools:
            run([require_tool("desktop-file-validate"), str(root / relative)])


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

    if (root / "usr/bin/termeric").read_bytes()[:4] != b"\x7fELF":
        raise CheckError("/usr/bin/termeric is not an ELF executable")
    validate_icons(root)

    validate_desktop_entries(root, exercise_native_tools=exercise_native_tools)
    notice = root / "usr/share/doc/termeric/THIRD_PARTY_NOTICES.md"
    validate_notice(notice.read_text(encoding="utf-8"), CARGO_LOCK.read_text(encoding="utf-8"))
    if not exercise_native_tools:
        return

    terminfo_root = root / "usr/share/terminfo"
    listing = run(
        [require_tool("infocmp"), "-x", "-A", str(terminfo_root), TERMINFO_NAME]
    ).stdout
    if TERMINFO_NAME not in listing:
        raise CheckError("installed terminfo entry has the wrong identity")


def relation_names(field: str) -> set[str]:
    """The package names in a Depends-style field, alternatives included."""
    return {
        re.split(r"[\s(:]", alternative.strip(), maxsplit=1)[0]
        for relation in field.split(",")
        for alternative in relation.split("|")
        if alternative.strip()
    }


def validate_package_metadata(package: Path) -> dict[str, str]:
    name = package_field(package, "Package")
    version = package_field(package, "Version")
    architecture = package_field(package, "Architecture")
    dependencies = package_field(package, "Depends")
    recommends = package_field(package, "Recommends")
    if name != PACKAGE_NAME:
        raise CheckError(f"package name is {name!r}, expected {PACKAGE_NAME!r}")
    if not version:
        raise CheckError("package Version field is empty")
    if not architecture:
        raise CheckError("package Architecture field is empty")
    depended_packages = relation_names(dependencies)
    if "$auto" in depended_packages or "libc6" not in depended_packages:
        raise CheckError("package Depends field was not populated by dpkg-shlibdeps")
    missing = sorted(RUNTIME_DEPENDENCIES - depended_packages)
    if missing:
        raise CheckError(
            "package does not depend on the libraries Termeric loads at runtime: "
            + ", ".join(missing)
        )
    if SOFTWARE_RENDERER_PACKAGE not in relation_names(recommends):
        raise CheckError(
            "package does not recommend the documented software renderer "
            f"{SOFTWARE_RENDERER_PACKAGE}"
        )
    return {
        "name": name,
        "version": version,
        "architecture": architecture,
        "depends": dependencies,
        "recommends": recommends,
    }


def inspect_archive(package: Path) -> dict[str, str]:
    if not package.is_file():
        raise CheckError(f"Debian package does not exist: {package}")
    metadata = validate_package_metadata(package)
    with tempfile.TemporaryDirectory(prefix="termeric-deb-inspect-") as directory:
        root = Path(directory) / "root"
        root.mkdir()
        run([require_tool("dpkg-deb"), "--extract", str(package), str(root)])
        validate_layout(root)
    print(f"ok archive: {package.name} ({metadata['version']})")
    return metadata


def parse_os_release(path: Path = Path("/etc/os-release")) -> set[str]:
    if not path.is_file():
        return set()
    identifiers: set[str] = set()
    for line in path.read_text(encoding="utf-8").splitlines():
        if "=" not in line:
            continue
        key, raw_value = line.split("=", 1)
        if key not in {"ID", "ID_LIKE"}:
            continue
        value = raw_value.strip().strip('"').strip("'")
        identifiers.update(part.casefold() for part in value.split())
    return identifiers


def installed_version() -> str | None:
    result = subprocess.run(
        [
            require_tool("dpkg-query"),
            "--show",
            "--showformat=${db:Status-Status} ${Version}",
            PACKAGE_NAME,
        ],
        capture_output=True,
        text=True,
        timeout=30,
    )
    if result.returncode != 0:
        return None
    status, separator, version = result.stdout.strip().partition(" ")
    return version if status == "installed" and separator else None


def require_lifecycle_host() -> None:
    if os.geteuid() != 0:
        raise CheckError("system lifecycle mode must run as root")
    if "debian" not in parse_os_release():
        raise CheckError("system lifecycle mode requires a Debian-family host")
    for tool in ["apt-get", "dpkg", "dpkg-deb", "dpkg-query"]:
        require_tool(tool)
    prior = installed_version()
    if prior is not None:
        raise CheckError(
            f"refusing to replace pre-existing {PACKAGE_NAME} package {prior}"
        )


def verify_installed(expected_package_version: str) -> None:
    actual = installed_version()
    if actual != expected_package_version:
        raise CheckError(
            f"installed package version is {actual!r}, expected {expected_package_version!r}"
        )
    validate_layout(Path("/"))
    output = run(["/usr/bin/termeric", "--version"], timeout=30).stdout.strip()
    if not re.fullmatch(r"termeric \d+\.\d+\.\d+(?:[-+][^\s]+)?", output):
        raise CheckError(f"installed non-GUI version probe returned {output!r}")


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
    apt = require_tool("apt-get")
    environment = {**os.environ, "DEBIAN_FRONTEND": "noninteractive"}
    try:
        run([apt, "install", "-y", str(package.resolve())], env=environment, timeout=600)
        verify_installed(version)
        print(f"ok install: {version}")
    finally:
        if installed_version() is not None:
            run([apt, "remove", "-y", PACKAGE_NAME], env=environment, timeout=600)
    verify_removed()
    print("ok remove: package files removed")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("package", type=Path, help="path to the generated .deb")
    parser.add_argument(
        "--system-lifecycle",
        action="store_true",
        help="install and remove the package on this disposable host",
    )
    args = parser.parse_args()
    try:
        metadata = inspect_archive(args.package)
        if args.system_lifecycle:
            exercise_system_lifecycle(args.package, metadata["version"])
    except (CheckError, OSError, subprocess.TimeoutExpired) as error:
        print(f"Debian package check failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
