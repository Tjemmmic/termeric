#!/usr/bin/env python3
"""Materialize reviewable TOML terminal fixtures as binary libFuzzer seeds."""

from __future__ import annotations

import argparse
import base64
import tomllib
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "termeric-core" / "tests" / "fixtures"
DEFAULT_COLS = 80
DEFAULT_ROWS = 24
DEFAULT_CHUNK_SIZE = 17
MAX_COLS = 160
MAX_ROWS = 80
MAX_CHUNK_SIZE = 32


def load_toml(name: str) -> dict:
    with (FIXTURES / name).open("rb") as fixture:
        return tomllib.load(fixture)


def safe_name(group: str, fixture_id: str) -> str:
    allowed = set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_")
    if not fixture_id or any(character not in allowed for character in fixture_id):
        raise ValueError(f"unsafe fixture id: {fixture_id!r}")
    return f"{group}-{fixture_id}.seed"


def terminal_fuzz_input(
    payload: bytes,
    *,
    cols: int = DEFAULT_COLS,
    rows: int = DEFAULT_ROWS,
    chunk_size: int = DEFAULT_CHUNK_SIZE,
) -> bytes:
    """Wrap raw terminal bytes in the terminal_bytes target's three-byte header."""
    bounds = (
        ("cols", cols, MAX_COLS),
        ("rows", rows, MAX_ROWS),
        ("chunk_size", chunk_size, MAX_CHUNK_SIZE),
    )
    for name, value, maximum in bounds:
        if (
            not isinstance(value, int)
            or isinstance(value, bool)
            or not 1 <= value <= maximum
        ):
            raise ValueError(f"{name} must be an integer in 1..={maximum}, got {value!r}")
    return bytes((cols - 1, rows - 1, chunk_size - 1)) + payload


def collect_seeds() -> list[tuple[str, bytes]]:
    """Return named, target-ready seeds while preserving every raw fixture byte."""

    seeds: list[tuple[str, bytes]] = []
    for case in load_toml("conformance_cases.toml")["case"]:
        seeds.append(
            (
                safe_name("fixture", case["id"]),
                terminal_fuzz_input(
                    bytes.fromhex(case["input_hex"]),
                    cols=case["cols"],
                    rows=case["rows"],
                ),
            )
        )
    for seed in load_toml("parser_test_seeds.toml")["seed"]:
        seeds.append(
            (
                safe_name("parser", seed["id"]),
                terminal_fuzz_input(bytes.fromhex(seed["input_hex"])),
            )
        )
    for trace in load_toml("external_traces.toml")["trace"]:
        seeds.append(
            (
                safe_name("trace", trace["id"]),
                terminal_fuzz_input(
                    base64.b64decode(
                        "".join(trace["input_base64"].split()), validate=True
                    ),
                    cols=trace["cols"],
                    rows=trace["rows"],
                ),
            )
        )
    return seeds


def materialize(output: Path) -> int:
    output.mkdir(parents=True, exist_ok=True)
    seeds = collect_seeds()

    names = [name for name, _ in seeds]
    if len(names) != len(set(names)):
        raise ValueError("fixture names collide after materialization")
    for name, data in seeds:
        (output / name).write_bytes(data)
    return len(seeds)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path, help="directory to receive binary seed files")
    args = parser.parse_args()

    count = materialize(args.output)
    print(f"Materialized {count} terminal fuzz seeds in {args.output}")


if __name__ == "__main__":
    main()
