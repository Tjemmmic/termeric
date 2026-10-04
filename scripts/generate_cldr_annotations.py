#!/usr/bin/env python3
"""Generate Termeric's compact, pinned English CLDR annotation index.

Each input is pinned by its SHA-256; download them from:

--base     https://raw.githubusercontent.com/unicode-org/cldr-json/48.0.0/cldr-json/cldr-annotations-full/annotations/en/annotations.json
--derived  https://raw.githubusercontent.com/unicode-org/cldr-json/48.0.0/cldr-json/cldr-annotations-derived-full/annotationsDerived/en/annotations.json
--emoji    https://www.unicode.org/Public/17.0.0/emoji/emoji-test.txt
"""

from __future__ import annotations

import argparse
import hashlib
import json
import struct
import sys
import unicodedata
from dataclasses import dataclass
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_OUTPUT = (
    ROOT / "termeric" / "src" / "ui" / "unicode_picker" / "cldr_annotations.bin"
)
CLDR_VERSION = "48.0.0"
CLDR_LOCALE = "en"
BASE_SHA256 = "f22083cb86dffb63a643d5bacb5d9899f82d2fa5d388ad4f3aed72184acef505"
DERIVED_SHA256 = "15c9457afa703a32dcc78d54626418a91e820fe0ddd67b7d491c4c4d9f5846ab"
EMOJI_VERSION = "17.0"
EMOJI_SHA256 = "1d8a944f88d7952f7ef7c5167fef3c67995bcae24543949710231b03a201acda"
MAGIC = b"TRMCLD01"
HEADER = struct.Struct("<8sII")
RECORD = struct.Struct("<IIII")
MAX_RECORDS = 5_000
MAX_TEXT_BYTES = 64
MAX_NAME_BYTES = 128
MAX_SEARCH_BYTES = 512
MAX_POOL_BYTES = 700_000
MAX_OUTPUT_BYTES = 768 * 1024


class GenerationError(ValueError):
    """The pinned source or generated index violates its bounded contract."""


@dataclass(frozen=True)
class Annotation:
    text: str
    short_name: str
    searchable: str


def _unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise GenerationError(f"duplicate JSON key {key!r}")
        result[key] = value
    return result


def parse_document(text: str, *, root_key: str, source: str) -> dict[str, Any]:
    try:
        document = json.loads(text, object_pairs_hook=_unique_object)
    except (json.JSONDecodeError, UnicodeDecodeError) as error:
        raise GenerationError(f"{source}: invalid JSON: {error}") from error
    if not isinstance(document, dict) or set(document) != {root_key}:
        raise GenerationError(f"{source}: expected only top-level key {root_key!r}")
    root = document[root_key]
    if not isinstance(root, dict) or set(root) != {"identity", "annotations"}:
        raise GenerationError(
            f"{source}: {root_key!r} must contain identity and annotations"
        )
    identity = root["identity"]
    if not isinstance(identity, dict) or identity.get("language") != CLDR_LOCALE:
        raise GenerationError(f"{source}: expected English identity")
    annotations = root["annotations"]
    if not isinstance(annotations, dict):
        raise GenerationError(f"{source}: annotations must be an object")
    return annotations


def normalize_query(value: str) -> str:
    normalized: list[str] = []
    separator = False
    for character in value:
        if character.isalnum():
            if separator and normalized:
                normalized.append(" ")
            normalized.extend(character.upper())
            separator = False
        else:
            separator = True
    return "".join(normalized)


def parse_emoji_test(text: str) -> dict[str, str]:
    if f"# Version: {EMOJI_VERSION}" not in text.splitlines()[:20]:
        raise GenerationError(f"emoji source is not version {EMOJI_VERSION}")
    qualified: dict[str, str] = {}
    for line_number, raw_line in enumerate(text.splitlines(), 1):
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        fields = line.split("#", 1)[0].split(";", 1)
        if len(fields) != 2:
            raise GenerationError(f"emoji source line {line_number} is malformed")
        codepoints, status = (field.strip() for field in fields)
        if status not in {
            "fully-qualified",
            "minimally-qualified",
            "unqualified",
            "component",
        }:
            raise GenerationError(
                f"emoji source line {line_number} has unknown status {status!r}"
            )
        try:
            scalars = [int(codepoint, 16) for codepoint in codepoints.split()]
            if any(0xD800 <= scalar <= 0xDFFF for scalar in scalars):
                raise ValueError("surrogate is not a Unicode scalar")
            sequence = "".join(chr(scalar) for scalar in scalars)
        except (ValueError, OverflowError) as error:
            raise GenerationError(
                f"emoji source line {line_number} has invalid code points"
            ) from error
        if not sequence:
            raise GenerationError(f"emoji source line {line_number} is empty")
        if status not in {"fully-qualified", "component"}:
            continue
        annotation_key = sequence.replace("\ufe0e", "").replace("\ufe0f", "")
        previous = qualified.get(annotation_key)
        if previous is not None and previous != sequence:
            raise GenerationError(
                f"emoji source has conflicting qualified forms for {annotation_key!r}"
            )
        qualified[annotation_key] = sequence
    if not qualified:
        raise GenerationError("emoji source contains no qualified sequences")
    return qualified


def _bounded_text(value: Any, *, field: str, maximum: int) -> str:
    if not isinstance(value, str) or not value:
        raise GenerationError(f"{field} must be a nonempty string")
    if any(0xD800 <= ord(character) <= 0xDFFF for character in value):
        raise GenerationError(f"{field} contains a surrogate")
    if any(unicodedata.category(character) == "Cc" for character in value):
        raise GenerationError(f"{field} contains a control character")
    if len(value.encode("utf-8")) > maximum:
        raise GenerationError(f"{field} exceeds {maximum} UTF-8 bytes")
    return value


def build_annotations(
    base: dict[str, Any], derived: dict[str, Any], presentations: dict[str, str]
) -> list[Annotation]:
    overlap = sorted(set(base) & set(derived))
    if overlap:
        raise GenerationError(f"duplicate annotation sequence {overlap[0]!r}")
    if len(base) + len(derived) > MAX_RECORDS:
        raise GenerationError(f"annotation count exceeds {MAX_RECORDS}")

    generated: list[Annotation] = []
    for text, value in [*base.items(), *derived.items()]:
        presentation_key = text.replace("\ufe0e", "").replace("\ufe0f", "")
        text = presentations.get(presentation_key, text)
        text = _bounded_text(text, field="annotation text", maximum=MAX_TEXT_BYTES)
        if not isinstance(value, dict) or not set(value) <= {"default", "tts"}:
            raise GenerationError(f"{text!r}: annotation must contain only default/tts")
        tts = value.get("tts")
        if not isinstance(tts, list) or len(tts) != 1:
            raise GenerationError(f"{text!r}: tts must contain exactly one short name")
        short_name = _bounded_text(
            tts[0], field=f"{text!r} short name", maximum=MAX_NAME_BYTES
        )
        keywords = value.get("default", [])
        if not isinstance(keywords, list):
            raise GenerationError(f"{text!r}: default keywords must be an array")
        if len(keywords) != len(set(keywords)):
            raise GenerationError(f"{text!r}: duplicate keyword")

        normalized_terms: list[str] = []
        for position, phrase in enumerate([short_name, *keywords]):
            phrase = _bounded_text(
                phrase,
                field=f"{text!r} search phrase {position}",
                maximum=MAX_SEARCH_BYTES,
            )
            normalized = normalize_query(phrase)
            for term in normalized.split():
                if term not in normalized_terms:
                    normalized_terms.append(term)
        searchable = " ".join(normalized_terms)
        if not searchable:
            raise GenerationError(f"{text!r}: all search phrases normalize to empty")
        if len(searchable.encode("utf-8")) > MAX_SEARCH_BYTES:
            raise GenerationError(f"{text!r}: combined search data exceeds {MAX_SEARCH_BYTES} bytes")
        generated.append(Annotation(text, short_name, searchable))
    generated.sort(key=lambda annotation: tuple(map(ord, annotation.text)))
    for previous, current in zip(generated, generated[1:]):
        if previous.text == current.text:
            raise GenerationError(
                f"presentation mapping duplicates annotation {current.text!r}"
            )
    return generated


def encode_annotations(annotations: list[Annotation]) -> bytes:
    pool = bytearray()
    records = bytearray()
    previous_text: str | None = None
    for annotation in annotations:
        if previous_text is not None and tuple(map(ord, previous_text)) >= tuple(
            map(ord, annotation.text)
        ):
            raise GenerationError("annotations must be unique and code-point sorted")
        previous_text = annotation.text
        text_start = len(pool)
        pool.extend(annotation.text.encode("utf-8"))
        name_start = len(pool)
        pool.extend(annotation.short_name.encode("utf-8"))
        search_start = len(pool)
        pool.extend(annotation.searchable.encode("utf-8"))
        end = len(pool)
        records.extend(RECORD.pack(text_start, name_start, search_start, end))

    if len(annotations) > MAX_RECORDS:
        raise GenerationError(f"annotation count exceeds {MAX_RECORDS}")
    if len(pool) > MAX_POOL_BYTES:
        raise GenerationError(f"annotation pool exceeds {MAX_POOL_BYTES} bytes")
    output = HEADER.pack(MAGIC, len(annotations), len(pool)) + records + pool
    if len(output) > MAX_OUTPUT_BYTES:
        raise GenerationError(f"generated index exceeds {MAX_OUTPUT_BYTES} bytes")
    return output


def generate(base_text: str, derived_text: str, emoji_text: str) -> bytes:
    base = parse_document(base_text, root_key="annotations", source="base")
    derived = parse_document(
        derived_text, root_key="annotationsDerived", source="derived"
    )
    return encode_annotations(
        build_annotations(base, derived, parse_emoji_test(emoji_text))
    )


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--base", type=Path, required=True)
    parser.add_argument("--derived", type=Path, required=True)
    parser.add_argument("--emoji", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument(
        "--check",
        action="store_true",
        help="compare generated bytes with --output instead of writing",
    )
    args = parser.parse_args(argv)

    try:
        base_bytes = args.base.read_bytes()
        derived_bytes = args.derived.read_bytes()
        emoji_bytes = args.emoji.read_bytes()
        if sha256(base_bytes) != BASE_SHA256:
            raise GenerationError("base source does not match pinned CLDR 48.0.0 SHA-256")
        if sha256(derived_bytes) != DERIVED_SHA256:
            raise GenerationError(
                "derived source does not match pinned CLDR 48.0.0 SHA-256"
            )
        if sha256(emoji_bytes) != EMOJI_SHA256:
            raise GenerationError(
                "emoji source does not match pinned Unicode Emoji 17.0 SHA-256"
            )
        generated = generate(
            base_bytes.decode("utf-8"),
            derived_bytes.decode("utf-8"),
            emoji_bytes.decode("utf-8"),
        )
        if args.check:
            if not args.output.exists() or args.output.read_bytes() != generated:
                raise GenerationError(f"generated index differs from {args.output}")
            print(
                f"Verified {args.output}: {len(generated)} bytes, CLDR {CLDR_VERSION} {CLDR_LOCALE}"
            )
            return 0
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_bytes(generated)
        print(
            f"Wrote {args.output}: {len(generated)} bytes, CLDR {CLDR_VERSION} {CLDR_LOCALE}"
        )
        return 0
    except (GenerationError, OSError, UnicodeDecodeError) as error:
        print(f"CLDR annotation generation failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
