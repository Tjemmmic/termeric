#!/usr/bin/env python3
"""Build the current Termeric checkout with one of the AUR recipes in packaging/arch/.

Both recipes download their source from GitHub. This helper stages the local
checkout instead, so an unpublished candidate can be packaged without changing
the recipe or contacting GitHub: termeric-git gets the working tree as it is,
and termeric gets an archive of the committed HEAD, laid out like GitHub's
archive of a tag. That archive's checksum differs from the one the recipe pins
for GitHub's, so its checksum is not verified.
"""

from __future__ import annotations

import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

if __package__:
    from scripts.check_release import ReleaseError, pkgbuild_version
else:
    from check_release import ReleaseError, pkgbuild_version


class BuildError(RuntimeError):
    pass


EXCLUDED_ROOT_NAMES = {"target"}
RELEASE_PACKAGE = "termeric"
DEVELOPMENT_PACKAGE = "termeric-git"
PACKAGES = (DEVELOPMENT_PACKAGE, RELEASE_PACKAGE)


def require_tool(name: str) -> str:
    path = shutil.which(name)
    if path is None:
        raise BuildError(f"required tool is unavailable: {name}")
    return path


def copy_checkout(source: Path, destination: Path) -> None:
    if not (source / "Cargo.toml").is_file() or not (source / ".git").exists():
        raise BuildError(f"source is not a Git-backed Termeric checkout: {source}")

    source = source.resolve()

    def ignore(directory: str, names: list[str]) -> set[str]:
        if Path(directory).resolve() == source:
            return set(names) & EXCLUDED_ROOT_NAMES
        return set()

    shutil.copytree(source, destination, symlinks=True, ignore=ignore)


def archive_head(source: Path, destination: Path, prefix: str) -> None:
    """Write a gzipped archive of `source`'s HEAD whose files sit under `prefix`/."""
    git = require_tool("git")
    command = [git, "-C", str(source), "archive", "--format=tar.gz"]
    command += [f"--prefix={prefix}/", "--output", str(destination), "HEAD"]
    result = subprocess.run(command, text=True)
    if result.returncode != 0:
        raise BuildError(f"git archive failed with exit status {result.returncode}")


def build_package(source: Path, output: Path, *, package: str, run_check: bool) -> Path:
    if hasattr(os, "geteuid") and os.geteuid() == 0:
        raise BuildError("makepkg refuses root; run this helper as an unprivileged user")
    if package not in PACKAGES:
        raise BuildError(f"unknown package {package!r}; expected one of {', '.join(PACKAGES)}")
    makepkg = require_tool("makepkg")
    recipe = source / "packaging/arch" / package / "PKGBUILD"
    if not recipe.is_file():
        raise BuildError(f"maintained PKGBUILD is missing: {recipe}")
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="termeric-arch-build-") as directory:
        staging = Path(directory)
        shutil.copy2(recipe, staging / "PKGBUILD")
        command = [makepkg, "--nodeps", "--force", "--nocolor"]
        if package == DEVELOPMENT_PACKAGE:
            # The recipe's VCS source would be cloned into src/termeric.
            (staging / "src").mkdir()
            copy_checkout(source, staging / "src/termeric")
            command.append("--noextract")
        else:
            # makepkg uses a source file already beside the recipe instead of
            # downloading it.
            try:
                version = pkgbuild_version(recipe.read_text(encoding="utf-8"))
            except ReleaseError as error:
                raise BuildError(str(error)) from error
            name = f"{RELEASE_PACKAGE}-{version}"
            archive_head(source, staging / f"{name}.tar.gz", name)
            command.append("--skipchecksums")
        if not run_check:
            command.append("--nocheck")
        result = subprocess.run(command, cwd=staging, text=True)
        if result.returncode != 0:
            raise BuildError(f"makepkg failed with exit status {result.returncode}")
        packages = sorted(staging.glob(f"{package}-[0-9]*.pkg.tar.zst"))
        if len(packages) != 1:
            raise BuildError(
                f"makepkg produced {len(packages)} candidate package archives, expected one"
            )
        destination = output / packages[0].name
        shutil.copy2(packages[0], destination)
        return destination


def main(argv: list[str] | None = None) -> int:
    repository = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package", choices=PACKAGES, default=DEVELOPMENT_PACKAGE)
    parser.add_argument("--source", type=Path, default=repository)
    parser.add_argument("--output", type=Path, default=repository / "target/arch")
    parser.add_argument(
        "--run-check",
        action="store_true",
        help="run the PKGBUILD check() function instead of reusing separate test gates",
    )
    arguments = parser.parse_args(argv)
    try:
        package = build_package(
            arguments.source.resolve(),
            arguments.output.resolve(),
            package=arguments.package,
            run_check=arguments.run_check,
        )
    except (BuildError, OSError, subprocess.SubprocessError) as error:
        print(f"Arch package build failed: {error}", file=sys.stderr)
        return 1
    print(package)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
