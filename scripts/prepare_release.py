#!/usr/bin/env python3
"""Set the version everywhere a release names it, and date its changelog.

Moves CHANGELOG.md's [Unreleased] notes under the new version, adds its
AppStream release, and sets the version in the Cargo manifest and lock files,
the manual page, and the three AUR recipes. `--pull-request` also commits the
result on a branch and opens a pull request for it.
"""

from __future__ import annotations

import argparse
import datetime
from pathlib import Path
import re
import subprocess
import sys

if __package__:
    from scripts.check_release import ROOT, ReleaseError, check_release, workspace_package
else:
    from check_release import ROOT, ReleaseError, check_release, workspace_package


VERSION = re.compile(r"\d+\.\d+\.\d+")
CARGO_TOML = "Cargo.toml"
CHANGELOG = "CHANGELOG.md"
METAINFO = "assets/org.termeric.Termeric.metainfo.xml"
MANUAL_PAGE = "assets/man/termeric.1"
LOCK_FILES = {"Cargo.lock": ("termeric", "termeric-core"), "fuzz/Cargo.lock": ("termeric-core",)}
RELEASE_RECIPE = "packaging/arch/termeric"
DEVELOPMENT_RECIPE = "packaging/arch/termeric-git"
BINARY_RECIPE = "packaging/arch/termeric-bin"
RECIPES = (RELEASE_RECIPE, DEVELOPMENT_RECIPE, BINARY_RECIPE)
FILES = [
    CARGO_TOML,
    CHANGELOG,
    METAINFO,
    MANUAL_PAGE,
    *LOCK_FILES,
    *(f"{recipe}/{name}" for recipe in RECIPES for name in ("PKGBUILD", ".SRCINFO")),
]


def replace_once(text: str, pattern: str, replacement: str, what: str) -> str:
    """`text` with the one match of `pattern` (by line) replaced, literally."""
    text, count = re.subn(pattern, lambda _: replacement, text, flags=re.MULTILINE)
    if count != 1:
        raise ReleaseError(f"expected one {what}, found {count}")
    return text


def version_key(version: str) -> tuple[int, ...]:
    if not VERSION.fullmatch(version):
        raise ReleaseError(f"{version!r} is not a version such as 0.2.0")
    return tuple(int(part) for part in version.split("."))


def date_changelog(changelog: str, version: str, date: str) -> str:
    section = re.search(r"^## \[Unreleased\]\n(.*?)(?=^## |\Z)", changelog, re.MULTILINE | re.DOTALL)
    if section is None:
        raise ReleaseError("CHANGELOG.md has no [Unreleased] section")
    if not section[1].strip():
        raise ReleaseError("CHANGELOG.md's [Unreleased] section is empty")
    return replace_once(
        changelog,
        r"^## \[Unreleased\]$",
        f"## [Unreleased]\n\n## [{version}] - {date}",
        "[Unreleased] heading in CHANGELOG.md",
    )


def add_metainfo_release(metainfo: str, version: str, date: str, repository: str) -> str:
    return replace_once(
        metainfo,
        r"^  <releases>$",
        f'  <releases>\n    <release version="{version}" date="{date}" type="development">\n'
        f'      <url type="details">{repository}/releases/tag/v{version}</url>\n'
        "    </release>",
        "<releases> element in the AppStream metadata",
    )


def prepare(files: dict[str, str], version: str, date: str) -> dict[str, str]:
    """`files` (by their paths in the repository) as `version` releases them."""
    current = workspace_package(files[CARGO_TOML], "version")
    if version_key(version) <= version_key(current):
        raise ReleaseError(f"{version} is not newer than {current}")
    repository = workspace_package(files[CARGO_TOML], "repository")
    old = re.escape(current)
    prepared = dict(files)

    def edit(path: str, pattern: str, replacement: str) -> None:
        prepared[path] = replace_once(prepared[path], pattern, replacement, f"{pattern!r} in {path}")

    edit(CARGO_TOML, rf'^version = "{old}"$', f'version = "{version}"')
    for path, crates in LOCK_FILES.items():
        for crate in crates:
            edit(
                path,
                rf'^name = "{crate}"\nversion = "{old}"$',
                f'name = "{crate}"\nversion = "{version}"',
            )
    edit(MANUAL_PAGE, rf'^\.TH termeric 1  "termeric {old}" $', f'.TH termeric 1  "termeric {version}" ')
    edit(MANUAL_PAGE, rf"^v{old}$", f"v{version}")
    # makepkg replaces termeric-git's version with the commit it builds.
    for recipe, package_version in {
        RELEASE_RECIPE: version,
        DEVELOPMENT_RECIPE: f"{version}.r0.g0000000",
        BINARY_RECIPE: version,
    }.items():
        edit(f"{recipe}/PKGBUILD", r"^pkgver=.*$", f"pkgver={package_version}")
        edit(f"{recipe}/PKGBUILD", r"^pkgrel=.*$", "pkgrel=1")
        edit(f"{recipe}/.SRCINFO", r"^\tpkgver = .*$", f"\tpkgver = {package_version}")
        edit(f"{recipe}/.SRCINFO", r"^\tpkgrel = .*$", "\tpkgrel = 1")
    edit(
        f"{RELEASE_RECIPE}/.SRCINFO",
        r"^\tsource = .*$",
        f"\tsource = termeric-{version}.tar.gz::{repository}/archive/refs/tags/v{version}.tar.gz",
    )
    for architecture in ("x86_64", "aarch64"):
        tarball = f"termeric-{version}-{architecture}-linux.tar.gz"
        edit(
            f"{BINARY_RECIPE}/.SRCINFO",
            rf"^\tsource_{architecture} = .*$",
            f"\tsource_{architecture} = {tarball}::{repository}/releases/download/v{version}/{tarball}",
        )
    prepared[CHANGELOG] = date_changelog(prepared[CHANGELOG], version, date)
    prepared[METAINFO] = add_metainfo_release(prepared[METAINFO], version, date, repository)
    # What the release workflow will check on the tag.
    check_release(
        prepared[CARGO_TOML],
        prepared[CHANGELOG],
        prepared[METAINFO],
        prepared[f"{RELEASE_RECIPE}/PKGBUILD"],
        tag=f"v{version}",
        publish=True,
    )
    return prepared


def run(*command: str) -> str:
    result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
    if result.returncode != 0:
        raise ReleaseError(f"{' '.join(command)} failed: {result.stderr.strip()}")
    return result.stdout.strip()


def require_current_main() -> None:
    if run("git", "status", "--porcelain"):
        raise ReleaseError("the working tree has changes; commit or stash them first")
    run("git", "fetch", "origin", "main")
    if run("git", "rev-parse", "HEAD") != run("git", "rev-parse", "FETCH_HEAD"):
        raise ReleaseError("check out the latest origin/main first")


def open_pull_request(version: str) -> str:
    branch = f"release/v{version}"
    title = f"chore(release): {version}"
    run("git", "switch", "--create", branch)
    run("git", "add", "--", *FILES)
    run("git", "commit", "--message", title)
    run("git", "push", "--set-upstream", "origin", branch)
    body = f"Sets the version to {version} and dates its changelog section. Once this is merged and CI passes on `main`, tag that commit `v{version}`."
    return run("gh", "pr", "create", "--base", "main", "--title", title, "--body", body)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("version", help="the version to release, such as 0.2.0")
    parser.add_argument(
        "--date", default=datetime.date.today().isoformat(), help="the release date (default: today)"
    )
    parser.add_argument(
        "--pull-request",
        action="store_true",
        help="commit the release on a branch from the latest main, push it, and open a pull request",
    )
    arguments = parser.parse_args(argv)
    try:
        datetime.date.fromisoformat(arguments.date)
    except ValueError:
        parser.error(f"{arguments.date!r} is not a YYYY-MM-DD date")
    try:
        if arguments.pull_request:
            require_current_main()
        files = {path: (ROOT / path).read_text(encoding="utf-8") for path in FILES}
        for path, text in prepare(files, arguments.version, arguments.date).items():
            (ROOT / path).write_text(text, encoding="utf-8")
        if arguments.pull_request:
            print(open_pull_request(arguments.version))
        else:
            print(f"prepared {arguments.version}; review the changes with git diff")
    except (ReleaseError, OSError) as error:
        print(f"release preparation failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
