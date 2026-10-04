#!/usr/bin/env python3
"""Check that a release's version, changelog, and AppStream metadata agree.

Prints the version. The release workflow runs it before building anything:
the tag must be v plus the workspace version, CHANGELOG.md needs that
version's section, and the AppStream metadata a matching release entry. A
release that will be published (--publish) also needs the section's date set,
and the same date in the metadata. --notes writes the section's text, for the
GitHub release.
"""

from __future__ import annotations

import argparse
from pathlib import Path
import re
import sys
import tomllib
import xml.etree.ElementTree as ElementTree


ROOT = Path(__file__).resolve().parent.parent
METAINFO = ROOT / "assets/org.termeric.Termeric.metainfo.xml"
CHANGELOG_HEADING = re.compile(
    r"^## \[(?P<version>[^\]]+)\] - (?P<date>\d{4}-\d{2}-\d{2}|Unreleased)$", re.MULTILINE
)


class ReleaseError(RuntimeError):
    pass


def workspace_package(cargo_toml: str, key: str) -> str:
    try:
        return tomllib.loads(cargo_toml)["workspace"]["package"][key]
    except (tomllib.TOMLDecodeError, KeyError) as error:
        raise ReleaseError(f"Cargo.toml has no workspace {key}: {error}") from error


def absolute_links(markdown: str, base: str) -> str:
    """Point the changelog's repository-relative links at `base`, since a
    GitHub release page doesn't resolve them."""
    return re.sub(r"\]\((?!https?://|#|mailto:)([^)]+)\)", lambda link: f"]({base}{link[1]})", markdown)


def changelog_section(changelog: str, version: str) -> tuple[str | None, str]:
    """The date of `version`'s section (None while unreleased) and its text."""
    headings = list(CHANGELOG_HEADING.finditer(changelog))
    for index, heading in enumerate(headings):
        if heading.group("version") != version:
            continue
        end = headings[index + 1].start() if index + 1 < len(headings) else len(changelog)
        # Stop at any later second-level heading, or the link definitions at the end.
        body = re.split(
            r"^(?:## |\[[^\]]+\]: )", changelog[heading.end() : end], maxsplit=1, flags=re.MULTILINE
        )[0]
        date = heading.group("date")
        return (None if date == "Unreleased" else date), body.strip()
    raise ReleaseError(f"CHANGELOG.md has no section for {version}")


def metainfo_release_date(metainfo: str, version: str) -> str:
    try:
        component = ElementTree.fromstring(metainfo)
    except ElementTree.ParseError as error:
        raise ReleaseError(f"AppStream metadata is malformed: {error}") from error
    for release in component.iterfind("releases/release"):
        if release.get("version") == version:
            date = release.get("date")
            if not date:
                raise ReleaseError(f"AppStream release {version} has no date")
            return date
    raise ReleaseError(f"AppStream metadata has no release entry for {version}")


def check_release(
    cargo_toml: str,
    changelog: str,
    metainfo: str,
    *,
    tag: str | None,
    publish: bool,
) -> tuple[str, str]:
    """The version and its release notes, once everything agrees."""
    version = workspace_package(cargo_toml, "version")
    if publish and tag is None:
        raise ReleaseError("publishing needs the release tag")
    if tag is not None and tag != f"v{version}":
        raise ReleaseError(f"tag {tag} does not match the workspace version {version}")
    changelog_date, notes = changelog_section(changelog, version)
    if not notes:
        raise ReleaseError(f"CHANGELOG.md's section for {version} is empty")
    metainfo_date = metainfo_release_date(metainfo, version)
    if publish:
        if changelog_date is None:
            raise ReleaseError(f"CHANGELOG.md's section for {version} is still Unreleased")
        if metainfo_date != changelog_date:
            raise ReleaseError(
                f"AppStream release {version} is dated {metainfo_date}, "
                f"but CHANGELOG.md says {changelog_date}"
            )
    repository = workspace_package(cargo_toml, "repository")
    return version, absolute_links(notes, f"{repository}/blob/{tag or 'main'}/")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--tag", help="the release tag, such as v0.1.0")
    parser.add_argument(
        "--publish", action="store_true", help="also require the release date to be set"
    )
    parser.add_argument("--notes", type=Path, help="write the changelog section here")
    arguments = parser.parse_args(argv)
    try:
        version, notes = check_release(
            (ROOT / "Cargo.toml").read_text(encoding="utf-8"),
            (ROOT / "CHANGELOG.md").read_text(encoding="utf-8"),
            METAINFO.read_text(encoding="utf-8"),
            tag=arguments.tag,
            publish=arguments.publish,
        )
        if arguments.notes is not None:
            arguments.notes.write_text(notes + "\n", encoding="utf-8")
    except (ReleaseError, OSError) as error:
        print(f"release check failed: {error}", file=sys.stderr)
        return 1
    print(version)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
