#!/usr/bin/env python3
"""Inspect Termeric's Debian package and optionally exercise its lifecycle.

The default mode is read-only. The lifecycle mode deliberately mutates the
host package database and therefore requires root, a Debian-family host, an
explicit environment opt-in, and a machine on which Termeric is not already
installed.
"""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tempfile


PACKAGE_NAME = "termeric"
TERMINFO_NAME = "xterm-termeric"
SOFTWARE_RENDERER_PACKAGE = "mesa-vulkan-drivers"
LIFECYCLE_OPT_IN = "TERMERIC_PACKAGE_LIFECYCLE_ACCEPT"
SYNTHETIC_PREDECESSOR_VERSION = "0~termeric-lifecycle"
EXPECTED_FILES = {
    "usr/bin/termeric": 0o755,
    "usr/share/doc/termeric/copyright": 0o644,
    "usr/share/applications/termeric.desktop": 0o644,
    "usr/share/icons/hicolor/1024x1024/apps/termeric.png": 0o644,
    "usr/share/terminfo/x/xterm-termeric": 0o644,
}


class CheckError(RuntimeError):
    pass


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


def validate_desktop_entry(path: Path) -> None:
    values = parse_desktop_entry(path.read_text(encoding="utf-8"))
    required = {
        ("Desktop Entry", "Type"): "Application",
        ("Desktop Entry", "Exec"): "termeric",
        ("Desktop Entry", "Icon"): "termeric",
        ("Desktop Entry", "Terminal"): "false",
        ("Desktop Entry", "StartupWMClass"): "termeric",
        ("Desktop Action NewWindow", "Exec"): "termeric --new-window",
    }
    for key, expected in required.items():
        actual = values.get(key)
        if actual != expected:
            raise CheckError(
                f"desktop entry {key[0]}/{key[1]} is {actual!r}, expected {expected!r}"
            )
    actions = values.get(("Desktop Entry", "Actions"), "").split(";")
    if "NewWindow" not in actions:
        raise CheckError("desktop entry does not expose the NewWindow action")


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
    if (
        root / "usr/share/icons/hicolor/1024x1024/apps/termeric.png"
    ).read_bytes()[:8] != b"\x89PNG\r\n\x1a\n":
        raise CheckError("installed icon is not a PNG file")

    desktop = root / "usr/share/applications/termeric.desktop"
    validate_desktop_entry(desktop)
    if not exercise_native_tools:
        return

    run([require_tool("desktop-file-validate"), str(desktop)])
    terminfo_root = root / "usr/share/terminfo"
    listing = run(
        [require_tool("infocmp"), "-x", "-A", str(terminfo_root), TERMINFO_NAME]
    ).stdout
    if TERMINFO_NAME not in listing:
        raise CheckError("installed terminfo entry has the wrong identity")


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
    if not dependencies or dependencies == "$auto":
        raise CheckError("package Depends field was not populated by dpkg-shlibdeps")
    recommended_packages = {
        dependency.split(" ", 1)[0].strip()
        for dependency in recommends.split(",")
        if dependency.strip()
    }
    if SOFTWARE_RENDERER_PACKAGE not in recommended_packages:
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
    if os.environ.get(LIFECYCLE_OPT_IN) != "1":
        raise CheckError(
            f"system lifecycle mode requires {LIFECYCLE_OPT_IN}=1"
        )
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


def replace_control_version(control: Path, version: str) -> None:
    text = control.read_text(encoding="utf-8")
    changed, count = re.subn(
        r"^Version:.*$", f"Version: {version}", text, count=1, flags=re.MULTILINE
    )
    if count != 1:
        raise CheckError("extracted Debian control file has no unique Version field")
    control.write_text(changed, encoding="utf-8")


def build_synthetic_predecessor(package: Path, directory: Path, actual: str) -> Path:
    comparison = subprocess.run(
        [
            require_tool("dpkg"),
            "--compare-versions",
            SYNTHETIC_PREDECESSOR_VERSION,
            "lt",
            actual,
        ],
        timeout=30,
    )
    if comparison.returncode != 0:
        raise CheckError(
            f"synthetic predecessor {SYNTHETIC_PREDECESSOR_VERSION!r} is not older than {actual!r}"
        )
    root = directory / "predecessor-root"
    run([require_tool("dpkg-deb"), "--raw-extract", str(package), str(root)])
    replace_control_version(root / "DEBIAN/control", SYNTHETIC_PREDECESSOR_VERSION)
    predecessor = directory / f"{PACKAGE_NAME}_synthetic-predecessor.deb"
    run([require_tool("dpkg-deb"), "--build", str(root), str(predecessor)])
    return predecessor


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


def exercise_system_lifecycle(package: Path, actual_version: str) -> dict[str, object]:
    require_lifecycle_host()
    with tempfile.TemporaryDirectory(prefix="termeric-deb-lifecycle-") as directory:
        temporary = Path(directory)
        synthetic_home = temporary / "home"
        sentinel = synthetic_home / ".config/termeric/lifecycle-sentinel"
        sentinel.parent.mkdir(parents=True)
        sentinel.write_text("retain user state\n", encoding="utf-8")
        environment = dict(os.environ)
        environment.update(
            {
                "DEBIAN_FRONTEND": "noninteractive",
                "HOME": str(synthetic_home),
            }
        )
        predecessor = build_synthetic_predecessor(package, temporary, actual_version)
        apt = require_tool("apt-get")
        installed_by_check = False
        try:
            run([apt, "install", "-y", str(predecessor)], env=environment, timeout=600)
            installed_by_check = True
            verify_installed(SYNTHETIC_PREDECESSOR_VERSION)
            print(f"ok install: synthetic predecessor {SYNTHETIC_PREDECESSOR_VERSION}")

            run([apt, "install", "-y", str(package.resolve())], env=environment, timeout=600)
            verify_installed(actual_version)
            print(f"ok upgrade: {SYNTHETIC_PREDECESSOR_VERSION} -> {actual_version}")

            run([apt, "remove", "-y", PACKAGE_NAME], env=environment, timeout=600)
            installed_by_check = False
            verify_removed()
            if sentinel.read_text(encoding="utf-8") != "retain user state\n":
                raise CheckError("package removal changed synthetic user configuration")
            print("ok remove: package files removed and user state retained")
            return {
                "performed": True,
                "host_family": sorted(parse_os_release()),
                "predecessor_version": SYNTHETIC_PREDECESSOR_VERSION,
                "candidate_version": actual_version,
                "install": "pass",
                "upgrade": "pass",
                "non_gui_version_probe": "pass",
                "remove": "pass",
                "owned_file_cleanup": "pass",
                "user_state_retention": "pass",
            }
        finally:
            if installed_by_check and installed_version() is not None:
                subprocess.run(
                    [apt, "remove", "-y", PACKAGE_NAME],
                    env=environment,
                    capture_output=True,
                    text=True,
                    timeout=600,
                )


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def write_report(
    path: Path,
    package: Path,
    *,
    status: str,
    metadata: dict[str, str] | None,
    lifecycle: dict[str, object] | None,
    error: str | None = None,
) -> None:
    package_details: dict[str, object] = {"path": package.name}
    if package.is_file():
        package_details.update(
            {
                "bytes": package.stat().st_size,
                "sha256": file_sha256(package),
            }
        )
    if metadata is not None:
        package_details.update(metadata)
    payload: dict[str, object] = {
        "schema": 1,
        "status": status,
        "recorded_at_utc": datetime.now(timezone.utc).isoformat(),
        "candidate_sha": os.environ.get("GITHUB_SHA"),
        "package": package_details,
        "archive_contract": "pass" if metadata is not None else "not-established",
        "lifecycle": lifecycle or {"performed": False},
    }
    if error is not None:
        payload["error"] = error
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.tmp")
    temporary.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
    os.replace(temporary, path)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("package", type=Path, help="path to the generated .deb")
    parser.add_argument(
        "--system-lifecycle",
        action="store_true",
        help="install, upgrade, and remove on this explicitly opted-in disposable host",
    )
    parser.add_argument(
        "--report",
        type=Path,
        help="write a machine-readable JSON evidence report",
    )
    args = parser.parse_args()

    metadata: dict[str, str] | None = None
    lifecycle: dict[str, object] | None = None
    try:
        metadata = inspect_archive(args.package)
        if args.system_lifecycle:
            lifecycle = exercise_system_lifecycle(args.package, metadata["version"])
    except (CheckError, OSError, subprocess.TimeoutExpired) as error:
        if args.report is not None:
            write_report(
                args.report,
                args.package,
                status="fail",
                metadata=metadata,
                lifecycle=lifecycle,
                error=str(error),
            )
        print(f"Debian package check failed: {error}", file=sys.stderr)
        return 1
    if args.report is not None:
        write_report(
            args.report,
            args.package,
            status="pass",
            metadata=metadata,
            lifecycle=lifecycle,
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
