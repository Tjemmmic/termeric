#!/usr/bin/env python3
"""Build Termeric's signed apt repository from one release's .deb packages.

The repository is written from nothing each time and holds only the packages
given, so publishing a release replaces the one before it. `--out` gets the
repository and `termeric.sources`, the one file a person installs to use it:
it names the repository and carries the public half of the key that signed it.
The key is the only secret key in gpg's keyring (set GNUPGHOME to choose it).

It needs dpkg-deb and gpg.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import datetime
from email.utils import format_datetime
import gzip
import hashlib
from pathlib import Path
import shutil
import subprocess
import sys


PACKAGE_NAME = "termeric"
SUITE = "stable"
COMPONENT = "main"
SOURCES_NAME = "termeric.sources"


class RepositoryError(RuntimeError):
    pass


@dataclass(frozen=True)
class Package:
    path: Path
    version: str
    architecture: str
    control: str


def run(command: list[str]) -> str:
    tool = shutil.which(command[0])
    if tool is None:
        raise RepositoryError(f"required tool is unavailable: {command[0]}")
    result = subprocess.run([tool, *command[1:]], capture_output=True, text=True)
    if result.returncode != 0:
        raise RepositoryError(
            f"{' '.join(command)} failed ({result.returncode}): {result.stderr.strip()}"
        )
    return result.stdout


def control_field(control: str, name: str) -> str:
    for line in control.splitlines():
        key, separator, value = line.partition(":")
        if separator and key == name:
            return value.strip()
    raise RepositoryError(f"package has no {name} field")


def read_package(path: Path) -> Package:
    control = run(["dpkg-deb", "--field", str(path)]).strip()
    name = control_field(control, "Package")
    if name != PACKAGE_NAME:
        raise RepositoryError(f"{path.name} is the package {name!r}, expected {PACKAGE_NAME!r}")
    return Package(
        path, control_field(control, "Version"), control_field(control, "Architecture"), control
    )


def require_one_release(packages: list[Package]) -> None:
    """The packages are one version, with one package for each architecture."""
    if not packages:
        raise RepositoryError("no packages given")
    versions = sorted({package.version for package in packages})
    if len(versions) != 1:
        raise RepositoryError(f"packages have different versions: {', '.join(versions)}")
    architectures = [package.architecture for package in packages]
    if len(set(architectures)) != len(architectures):
        raise RepositoryError(f"more than one package for an architecture: {', '.join(architectures)}")


def pool_path(package: Package) -> str:
    return f"pool/{COMPONENT}/t/{PACKAGE_NAME}/{package.path.name}"


def packages_index(package: Package) -> str:
    """The Packages file for `package`'s architecture."""
    contents = package.path.read_bytes()
    return (
        f"{package.control}\n"
        f"Filename: {pool_path(package)}\n"
        f"Size: {len(contents)}\n"
        f"SHA256: {hashlib.sha256(contents).hexdigest()}\n"
    )


def release_text(architectures: list[str], indexes: dict[str, bytes], date: datetime.datetime) -> str:
    """The Release file listing `indexes` by their paths under dists/SUITE."""
    lines = [
        "Origin: Termeric",
        "Label: Termeric",
        f"Suite: {SUITE}",
        f"Codename: {SUITE}",
        f"Date: {format_datetime(date, usegmt=True)}",
        f"Architectures: {' '.join(sorted(architectures))}",
        f"Components: {COMPONENT}",
        "Description: Termeric's latest release",
        # A cached Release names its indexes by hash, so an index cached from
        # another release can't be taken for one of its own.
        "Acquire-By-Hash: yes",
        "SHA256:",
    ]
    lines += [
        f" {hashlib.sha256(contents).hexdigest()} {len(contents)} {path}"
        for path, contents in sorted(indexes.items())
    ]
    return "\n".join(lines) + "\n"


def sources_text(url: str, public_key: str) -> str:
    """The deb822 source, with the armored key as its Signed-By field.

    A field's continuation lines start with a space, and a dot stands for an
    empty one.
    """
    key = "\n".join(f" {line or '.'}" for line in public_key.strip().splitlines())
    return (
        "Types: deb\n"
        f"URIs: {url}\n"
        f"Suites: {SUITE}\n"
        f"Components: {COMPONENT}\n"
        f"Signed-By:\n{key}\n"
    )


def secret_key_fingerprint(colons: str) -> str:
    """The primary fingerprint in `gpg --with-colons --list-secret-keys` output."""
    fingerprints = []
    primary = False
    for record in (line.split(":") for line in colons.splitlines()):
        if record[0] == "sec":
            primary = True
        elif record[0] == "fpr" and primary:
            fingerprints.append(record[9])
            primary = False
    if len(fingerprints) != 1:
        raise RepositoryError(f"expected one secret key in gpg's keyring, found {len(fingerprints)}")
    return fingerprints[0]


def write_file(path: Path, contents: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(contents)


def build_repository(
    packages: list[Package], url: str, out: Path, fingerprint: str, date: datetime.datetime
) -> None:
    if out.exists():
        raise RepositoryError(f"output already exists: {out}")
    require_one_release(packages)
    distribution = out / "dists" / SUITE
    indexes: dict[str, bytes] = {}
    for package in packages:
        destination = out / pool_path(package)
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(package.path, destination)
        directory = f"{COMPONENT}/binary-{package.architecture}"
        index = packages_index(package).encode()
        for name, contents in {"Packages": index, "Packages.gz": gzip.compress(index, mtime=0)}.items():
            indexes[f"{directory}/{name}"] = contents
            write_file(distribution / directory / name, contents)
            digest = hashlib.sha256(contents).hexdigest()
            write_file(distribution / directory / "by-hash/SHA256" / digest, contents)

    release = distribution / "Release"
    architectures = [package.architecture for package in packages]
    write_file(release, release_text(architectures, indexes, date).encode())
    sign = ["gpg", "--batch", "--yes", "--local-user", fingerprint, "--digest-algo", "SHA512"]
    run([*sign, "--clearsign", "--output", str(distribution / "InRelease"), str(release)])
    run([
        *sign, "--armor", "--detach-sign", "--output", str(distribution / "Release.gpg"), str(release)
    ])
    public_key = run(["gpg", "--batch", "--armor", "--export-options", "export-minimal", "--export", fingerprint])
    write_file(out / SOURCES_NAME, sources_text(url, public_key).encode())


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("packages", nargs="+", type=Path, help="the release's .deb files")
    parser.add_argument("--url", required=True, help="where the repository will be served")
    parser.add_argument("--out", required=True, type=Path, help="the directory to create")
    arguments = parser.parse_args(argv)
    try:
        fingerprint = secret_key_fingerprint(
            run(["gpg", "--batch", "--with-colons", "--list-secret-keys"])
        )
        build_repository(
            [read_package(path) for path in arguments.packages],
            arguments.url,
            arguments.out,
            fingerprint,
            datetime.datetime.now(datetime.timezone.utc),
        )
    except (RepositoryError, OSError) as error:
        print(f"apt repository build failed: {error}", file=sys.stderr)
        return 1
    print(f"{arguments.out}: signed by {fingerprint}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
