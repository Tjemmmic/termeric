#!/usr/bin/env python3
"""Assemble Termeric.app from a built macOS binary.

The bundle holds the binary, its Info.plist, an .icns icon written here from
the committed PNGs (no Mac tool needed, so the result is the same on every
machine and the script is tested on Linux), and the terminfo entry in both
database layouts. On macOS, --sign then signs it ad hoc, which Apple silicon
requires before it runs anything.
"""

from __future__ import annotations

import argparse
from pathlib import Path
import plistlib
import shutil
import struct
import subprocess
import sys
import tomllib


ROOT = Path(__file__).resolve().parent.parent
BUNDLE_ID = "org.termeric.Termeric"
MINIMUM_MACOS = "11.0"
ICON_NAME = "Termeric.icns"
TERMINFO = "xterm-termeric"
EXECUTABLE_MODE = 0o755
DATA_MODE = 0o644

# ICNS entries Termeric ships, as (type, pixel size): every slot of a Mac
# app icon, 16 to 512 points at 1x and 2x.
ICNS_ENTRIES = (
    (b"icp4", 16),
    (b"icp5", 32),
    (b"ic11", 32),
    (b"ic12", 64),
    (b"ic07", 128),
    (b"ic13", 256),
    (b"ic08", 256),
    (b"ic14", 512),
    (b"ic09", 512),
    (b"ic10", 1024),
)

# A terminal is the process macOS holds responsible for what its shells run,
# so it carries a reason for every protected resource a program in it may
# reach; without one, macOS refuses (or ends) the program asking.
USAGE_DESCRIPTIONS = {
    "NSAppleEventsUsageDescription": "a program run in Termeric wants to control another app",
    "NSBluetoothAlwaysUsageDescription": "a program run in Termeric wants to use Bluetooth",
    "NSCalendarsUsageDescription": "a program run in Termeric wants to read your calendars",
    "NSCameraUsageDescription": "a program run in Termeric wants to use the camera",
    "NSContactsUsageDescription": "a program run in Termeric wants to read your contacts",
    "NSDesktopFolderUsageDescription": "a program run in Termeric wants to use files on your Desktop",
    "NSDocumentsFolderUsageDescription": "a program run in Termeric wants to use files in Documents",
    "NSDownloadsFolderUsageDescription": "a program run in Termeric wants to use files in Downloads",
    "NSLocalNetworkUsageDescription": "a program run in Termeric wants to reach devices on your network",
    "NSLocationUsageDescription": "a program run in Termeric wants to know your location",
    "NSMicrophoneUsageDescription": "a program run in Termeric wants to use the microphone",
    "NSNetworkVolumesUsageDescription": "a program run in Termeric wants to use files on network volumes",
    "NSRemovableVolumesUsageDescription": "a program run in Termeric wants to use files on removable volumes",
}


class BundleError(RuntimeError):
    pass


def workspace_version(repository: Path = ROOT) -> str:
    with (repository / "Cargo.toml").open("rb") as manifest:
        return tomllib.load(manifest)["workspace"]["package"]["version"]


def icon_png(repository: Path, size: int) -> Path:
    if size == 1024:
        return repository / "assets/icons/termeric.png"
    return repository / f"assets/icons/hicolor/{size}x{size}/apps/{BUNDLE_ID}.png"


def png_size(data: bytes) -> tuple[int, int]:
    if data[:8] != b"\x89PNG\r\n\x1a\n" or data[12:16] != b"IHDR":
        raise BundleError("an icon is not a PNG")
    return struct.unpack(">II", data[16:24])


def icns(repository: Path = ROOT) -> bytes:
    """The .icns file: its header, then one PNG chunk per entry."""
    chunks = []
    for kind, size in ICNS_ENTRIES:
        path = icon_png(repository, size)
        data = path.read_bytes()
        if png_size(data) != (size, size):
            raise BundleError(f"{path} is not {size}×{size}")
        chunks.append(kind + struct.pack(">I", len(data) + 8) + data)
    body = b"".join(chunks)
    return b"icns" + struct.pack(">I", len(body) + 8) + body


def info_plist(version: str) -> dict[str, object]:
    return {
        "CFBundleDevelopmentRegion": "en",
        "CFBundleDisplayName": "Termeric",
        "CFBundleExecutable": "termeric",
        "CFBundleIconFile": ICON_NAME,
        "CFBundleIdentifier": BUNDLE_ID,
        "CFBundleInfoDictionaryVersion": "6.0",
        "CFBundleName": "Termeric",
        "CFBundlePackageType": "APPL",
        "CFBundleShortVersionString": version,
        "CFBundleVersion": version,
        "LSApplicationCategoryType": "public.app-category.developer-tools",
        "LSMinimumSystemVersion": MINIMUM_MACOS,
        "NSHighResolutionCapable": True,
        "NSHumanReadableCopyright": "Copyright © 2026 Termeric Contributors. MIT License.",
        "NSSupportsAutomaticGraphicsSwitching": True,
        **{key: f"Termeric asks because {reason}." for key, reason in USAGE_DESCRIPTIONS.items()},
    }


def copy_file(source: Path, destination: Path, mode: int) -> None:
    if source.is_symlink() or not source.is_file():
        raise BundleError(f"missing regular file: {source}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, destination)
    destination.chmod(mode)


def is_mach_o(binary: Path) -> bool:
    magic = binary.read_bytes()[:4]
    # 64-bit Mach-O in either byte order, or a universal binary.
    return magic in {b"\xcf\xfa\xed\xfe", b"\xfe\xed\xfa\xcf", b"\xca\xfe\xba\xbe"}


def bundle(binary: Path, output: Path, repository: Path = ROOT) -> Path:
    """Write Termeric.app at `output`, which must not exist yet."""
    if not binary.is_file() or not is_mach_o(binary):
        raise BundleError(f"not a macOS executable: {binary}")
    if output.suffix != ".app":
        raise BundleError(f"the bundle's name must end in .app: {output}")
    if output.exists():
        raise BundleError(f"output already exists: {output}")
    contents = output / "Contents"
    copy_file(binary, contents / "MacOS/termeric", EXECUTABLE_MODE)
    resources = contents / "Resources"
    resources.mkdir(parents=True)
    (resources / ICON_NAME).write_bytes(icns(repository))
    entry = repository / "assets/terminfo/x" / TERMINFO
    for layout in ("x", f"{ord('x'):x}"):
        copy_file(entry, resources / "terminfo" / layout / TERMINFO, DATA_MODE)
    with (contents / "Info.plist").open("wb") as plist:
        plistlib.dump(info_plist(workspace_version(repository)), plist)
    (contents / "PkgInfo").write_bytes(b"APPL????")
    return output


def sign(app: Path) -> None:
    """Sign ad hoc: Apple silicon runs nothing unsigned."""
    subprocess.run(["codesign", "--force", "--deep", "--sign", "-", str(app)], check=True)
    subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)], check=True)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--binary", type=Path, required=True, help="the built termeric binary")
    parser.add_argument("--out", type=Path, required=True, help="where Termeric.app goes")
    parser.add_argument("--sign", action="store_true", help="sign ad hoc (macOS only)")
    arguments = parser.parse_args(argv)
    try:
        app = bundle(arguments.binary, arguments.out)
        if arguments.sign:
            sign(app)
    except (BundleError, OSError, subprocess.CalledProcessError) as error:
        print(f"assembling Termeric.app failed: {error}", file=sys.stderr)
        return 1
    print(f"assembled {app}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
