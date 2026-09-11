#!/usr/bin/env python3
"""Inspect a Termeric Arch package and optionally exercise its lifecycle.

Archive mode is read-only and validates member safety, package metadata,
installed paths, modes, desktop integration, terminfo identity, dynamic
libraries, and the non-GUI version path before emitting optional JSON evidence.
Lifecycle mode mutates pacman's database, so it requires a disposable Arch host,
root, an explicit opt-in, and no pre-existing Termeric package.
"""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import gzip
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import subprocess
import sys
import tempfile

if __package__:
    from scripts.check_debian_package import CheckError, validate_desktop_entry
else:
    from check_debian_package import CheckError, validate_desktop_entry


PACKAGE_NAME = "termeric-git"
CONFLICTING_PACKAGE_NAME = "termeric"
TERMINFO_NAME = "xterm-termeric"
LIFECYCLE_OPT_IN = "TERMERIC_ARCH_PACKAGE_LIFECYCLE_ACCEPT"
SYNTHETIC_PREDECESSOR_VERSION = "0~termeric-lifecycle-1"
EXPECTED_FILES = {
    "usr/bin/termeric": 0o755,
    "usr/share/applications/termeric.desktop": 0o644,
    "usr/share/icons/hicolor/1024x1024/apps/termeric.png": 0o644,
    "usr/share/terminfo/x/xterm-termeric": 0o644,
    "usr/share/licenses/termeric-git/LICENSE": 0o644,
    "usr/share/doc/termeric/README.md": 0o644,
    "usr/share/doc/termeric/CHANGELOG.md": 0o644,
}
METADATA_FILES = {".BUILDINFO", ".MTREE", ".PKGINFO"}
REQUIRED_DEPENDENCIES = {
    "brotli",
    "fontconfig",
    "gcc-libs",
    "glibc",
    "libx11",
    "libxkbcommon",
    "openssl",
    "wayland",
    "zlib",
    "zstd",
}


def require_tool(name: str) -> str:
    path = shutil.which(name)
    if path is None:
        raise CheckError(f"required tool is unavailable: {name}")
    return path


def run(
    command: list[str],
    *,
    cwd: Path | None = None,
    env: dict[str, str] | None = None,
    timeout: int = 180,
) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        command,
        cwd=cwd,
        env=env,
        capture_output=True,
        text=True,
        timeout=timeout,
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip()
        raise CheckError(f"{' '.join(command)} failed ({result.returncode}): {detail}")
    return result


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
    if not re.fullmatch(r"[0-9][0-9A-Za-z.+_:~-]*", version):
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
    if "MIT" not in fields.get("license", []):
        raise CheckError("package does not declare the MIT license")
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
    icon = root / "usr/share/icons/hicolor/1024x1024/apps/termeric.png"
    if icon.read_bytes()[:8] != b"\x89PNG\r\n\x1a\n":
        raise CheckError("installed icon is not a PNG file")
    desktop = root / "usr/share/applications/termeric.desktop"
    validate_desktop_entry(desktop)
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


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


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


def parse_os_release(path: Path = Path("/etc/os-release")) -> set[str]:
    if not path.is_file():
        return set()
    identifiers: set[str] = set()
    for line in path.read_text(encoding="utf-8").splitlines():
        key, separator, raw_value = line.partition("=")
        if not separator or key not in {"ID", "ID_LIKE"}:
            continue
        value = raw_value.strip().strip('"').strip("'")
        identifiers.update(part.casefold() for part in value.split())
    return identifiers


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
    if os.environ.get(LIFECYCLE_OPT_IN) != "1":
        raise CheckError(f"system lifecycle mode requires {LIFECYCLE_OPT_IN}=1")
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


def replace_metadata_version(path: Path, version: str) -> None:
    text = path.read_text(encoding="utf-8")
    pattern = r"^pkgver = .*$"
    if len(re.findall(pattern, text, flags=re.MULTILINE)) != 1:
        raise CheckError(f"{path.name} has no unique pkgver field")
    changed = re.sub(pattern, f"pkgver = {version}", text, flags=re.MULTILINE)
    path.write_text(changed, encoding="utf-8")


def write_mtree(root: Path) -> None:
    members = sorted(
        f"./{path.relative_to(root).as_posix()}"
        for path in root.rglob("*")
        if path.name != ".MTREE"
    )
    member_input = ("\0".join(members) + "\0").encode()
    command = [
        require_tool("bsdtar"),
        "-cnf",
        "-",
        "--format=mtree",
        "--options=!all,use-set,type,uid,gid,mode,time,size,sha256,link",
        "--null",
        "--files-from=-",
    ]
    result = subprocess.run(command, cwd=root, input=member_input, capture_output=True)
    if result.returncode != 0:
        detail = result.stderr.decode(errors="replace").strip()
        raise CheckError(f"{' '.join(command)} failed ({result.returncode}): {detail}")
    (root / ".MTREE").write_bytes(gzip.compress(result.stdout, mtime=0))


def build_synthetic_predecessor(
    package: Path, directory: Path, actual_version: str
) -> Path:
    comparison = run(
        [
            require_tool("vercmp"),
            SYNTHETIC_PREDECESSOR_VERSION,
            actual_version,
        ]
    ).stdout.strip()
    if comparison != "-1":
        raise CheckError(
            f"synthetic predecessor {SYNTHETIC_PREDECESSOR_VERSION!r} "
            f"is not older than {actual_version!r}"
        )
    root = directory / "predecessor-root"
    root.mkdir()
    run([require_tool("bsdtar"), "-xf", str(package.resolve()), "-C", str(root)])
    replace_metadata_version(root / ".PKGINFO", SYNTHETIC_PREDECESSOR_VERSION)
    replace_metadata_version(root / ".BUILDINFO", SYNTHETIC_PREDECESSOR_VERSION)
    write_mtree(root)
    predecessor = directory / (
        f"{PACKAGE_NAME}-{SYNTHETIC_PREDECESSOR_VERSION}-x86_64.pkg.tar.zst"
    )
    run(
        [
            require_tool("bsdtar"),
            "-caf",
            str(predecessor),
            ".PKGINFO",
            ".BUILDINFO",
            ".MTREE",
            "usr",
        ],
        cwd=root,
    )
    inspect_archive(predecessor)
    return predecessor


def verify_installed(expected_version: str) -> None:
    actual = installed_version()
    if actual != expected_version:
        raise CheckError(
            f"installed package version is {actual!r}, expected {expected_version!r}"
        )
    validate_layout(Path("/"))


def running_package_processes(proc_root: Path = Path("/proc")) -> list[int]:
    processes: list[int] = []
    for entry in proc_root.iterdir():
        if not entry.name.isdigit():
            continue
        try:
            executable = os.readlink(entry / "exe").removesuffix(" (deleted)")
        except OSError:
            continue
        if executable == "/usr/bin/termeric":
            processes.append(int(entry.name))
    return sorted(processes)


def verify_removed(
    root: Path = Path("/"), proc_root: Path = Path("/proc")
) -> None:
    if installed_version() is not None:
        raise CheckError(f"{PACKAGE_NAME} remains installed after removal")
    leftovers = [relative for relative in EXPECTED_FILES if (root / relative).exists()]
    if leftovers:
        raise CheckError(
            "package-owned files remain after removal: "
            + ", ".join(f"/{path}" for path in leftovers)
        )
    processes = running_package_processes(proc_root)
    if processes:
        raise CheckError(
            "package executable remains live after removal in PIDs: "
            + ", ".join(str(process) for process in processes)
        )


def verify_user_state(sentinel: Path) -> None:
    if not sentinel.is_file() or sentinel.read_text(encoding="utf-8") != "retain user state\n":
        raise CheckError("package removal changed synthetic user configuration")


def exercise_system_lifecycle(
    package: Path, actual_version: str
) -> dict[str, object]:
    require_lifecycle_host()
    with tempfile.TemporaryDirectory(prefix="termeric-arch-lifecycle-") as directory:
        temporary = Path(directory)
        synthetic_home = temporary / "home"
        sentinel = synthetic_home / ".config/termeric/lifecycle-sentinel"
        sentinel.parent.mkdir(parents=True)
        sentinel.write_text("retain user state\n", encoding="utf-8")
        environment = dict(os.environ)
        environment["HOME"] = str(synthetic_home)
        predecessor = build_synthetic_predecessor(
            package, temporary, actual_version
        )
        pacman = require_tool("pacman")
        installed_by_check = False
        try:
            run(
                [pacman, "-U", "--noconfirm", str(predecessor)],
                env=environment,
                timeout=600,
            )
            installed_by_check = True
            verify_installed(SYNTHETIC_PREDECESSOR_VERSION)
            print(f"ok install: synthetic predecessor {SYNTHETIC_PREDECESSOR_VERSION}")

            run(
                [pacman, "-U", "--noconfirm", str(package.resolve())],
                env=environment,
                timeout=600,
            )
            verify_installed(actual_version)
            print(f"ok upgrade: {SYNTHETIC_PREDECESSOR_VERSION} -> {actual_version}")

            run([pacman, "-R", "--noconfirm", PACKAGE_NAME], env=environment, timeout=600)
            installed_by_check = False
            verify_removed()
            verify_user_state(sentinel)
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
                    [pacman, "-R", "--noconfirm", PACKAGE_NAME],
                    env=environment,
                    capture_output=True,
                    text=True,
                    timeout=600,
                )


def write_report(
    path: Path,
    package: Path,
    *,
    status: str,
    metadata: dict[str, object] | None,
    lifecycle: dict[str, object] | None = None,
    error: str | None = None,
) -> None:
    package_details: dict[str, object] = {"path": package.name}
    if package.is_file():
        package_details.update(
            {"bytes": package.stat().st_size, "sha256": file_sha256(package)}
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
    temporary.write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    os.replace(temporary, path)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("package", type=Path)
    parser.add_argument(
        "--system-lifecycle",
        action="store_true",
        help="install, upgrade, and remove on this explicitly opted-in disposable host",
    )
    parser.add_argument("--report", type=Path)
    arguments = parser.parse_args(argv)
    metadata: dict[str, object] | None = None
    lifecycle: dict[str, object] | None = None
    try:
        metadata = inspect_archive(arguments.package)
        if arguments.system_lifecycle:
            lifecycle = exercise_system_lifecycle(
                arguments.package, str(metadata["version"])
            )
        if arguments.report is not None:
            write_report(
                arguments.report,
                arguments.package,
                status="pass",
                metadata=metadata,
                lifecycle=lifecycle,
            )
        return 0
    except (CheckError, OSError, subprocess.SubprocessError) as error:
        if arguments.report is not None:
            write_report(
                arguments.report,
                arguments.package,
                status="fail",
                metadata=metadata,
                lifecycle=lifecycle,
                error=str(error),
            )
        print(f"error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
