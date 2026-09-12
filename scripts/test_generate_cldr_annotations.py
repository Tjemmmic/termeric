import contextlib
import io
import json
import struct
import tempfile
import unittest
from pathlib import Path

from scripts.generate_cldr_annotations import (
    HEADER,
    MAGIC,
    RECORD,
    GenerationError,
    build_annotations,
    encode_annotations,
    generate,
    main,
    normalize_query,
    parse_document,
    parse_emoji_test,
)


def document(root: str, annotations: dict) -> str:
    return json.dumps(
        {root: {"identity": {"language": "en"}, "annotations": annotations}},
        ensure_ascii=False,
        separators=(",", ":"),
    )


EMOJI_FIXTURE = """\
# emoji-test.txt
# Version: 17.0
2764 FE0F ; fully-qualified # ❤️ E0.6 red heart
2764 ; unqualified # ❤ E0.6 red heart
1F44D ; fully-qualified # 👍 E0.6 thumbs up
1F44D 1F3FD ; fully-qualified # 👍🏽 E1.0 thumbs up: medium skin tone
1FA85 ; component # 🪅 E13.0 piñata
"""


class CldrAnnotationGeneratorTests(unittest.TestCase):
    def test_generation_is_sorted_compact_and_deterministic(self):
        base = document(
            "annotations",
            {
                "‼": {
                    "default": ["!", "bangbang", "exclamation"],
                    "tts": ["double exclamation mark"],
                },
                "❤": {"default": ["heart", "love"], "tts": ["red heart"]},
                "👍": {"default": ["good", "thumb", "up"], "tts": ["thumbs up"]},
                "🪅": {"default": ["celebration", "party"], "tts": ["piñata"]},
            },
        )
        derived = document(
            "annotationsDerived",
            {"👍🏽": {"tts": ["thumbs up: medium skin tone"]}},
        )
        first = generate(base, derived, EMOJI_FIXTURE)
        second = generate(base, derived, EMOJI_FIXTURE)
        self.assertEqual(first, second)
        magic, count, pool_bytes = HEADER.unpack_from(first)
        self.assertEqual(magic, MAGIC)
        self.assertEqual(count, 5)
        records_end = HEADER.size + count * RECORD.size
        self.assertEqual(len(first), records_end + pool_bytes)
        pool = first[records_end:].decode("utf-8")
        decoded = []
        for position in range(count):
            offsets = RECORD.unpack_from(first, HEADER.size + position * RECORD.size)
            decoded.append(
                (
                    pool.encode()[offsets[0] : offsets[1]].decode(),
                    pool.encode()[offsets[1] : offsets[2]].decode(),
                    pool.encode()[offsets[2] : offsets[3]].decode(),
                )
            )
        self.assertEqual([item[0] for item in decoded], ["‼", "❤️", "👍", "👍🏽", "🪅"])
        self.assertEqual(
            decoded[0][1:],
            ("double exclamation mark", "DOUBLE EXCLAMATION MARK BANGBANG"),
        )
        self.assertEqual(decoded[1][1:], ("red heart", "RED HEART LOVE"))
        self.assertEqual(decoded[2][1:], ("thumbs up", "THUMBS UP GOOD THUMB"))
        self.assertEqual(decoded[4][1:], ("piñata", "PIÑATA CELEBRATION PARTY"))

    def test_emoji_source_selects_only_exact_qualified_presentations(self):
        presentations = parse_emoji_test(EMOJI_FIXTURE)
        self.assertEqual(presentations["❤"], "❤️")
        self.assertEqual(presentations["👍🏽"], "👍🏽")
        with self.assertRaises(GenerationError):
            parse_emoji_test(EMOJI_FIXTURE + "D800 ; component # invalid\n")

    def test_normalization_is_ordered_casefolded_and_separator_bounded(self):
        self.assertEqual(normalize_query("  thumbs-up: Medium tone "), "THUMBS UP MEDIUM TONE")
        self.assertEqual(normalize_query("piñata / fête"), "PIÑATA FÊTE")
        self.assertEqual(normalize_query("---"), "")

    def test_duplicate_json_keys_and_sequences_fail(self):
        duplicate_key = (
            '{"annotations":{"identity":{"language":"en"},'
            '"annotations":{"👍":{"tts":["one"]},"👍":{"tts":["two"]}}}}'
        )
        with self.assertRaisesRegex(GenerationError, "duplicate JSON key"):
            parse_document(duplicate_key, root_key="annotations", source="fixture")

        record = {"👍": {"default": ["good"], "tts": ["thumbs up"]}}
        with self.assertRaisesRegex(GenerationError, "duplicate annotation sequence"):
            build_annotations(record, record, {})

    def test_malformed_records_and_bounds_fail_closed(self):
        valid = {"👍": {"default": ["good"], "tts": ["thumbs up"]}}
        invalid_values = (
            {"👍": {"default": ["good"]}},
            {"👍": {"tts": []}},
            {"👍": {"tts": ["one", "two"]}},
            {"👍": {"default": "good", "tts": ["thumbs up"]}},
            {"👍": {"default": ["good", "good"], "tts": ["thumbs up"]}},
            {"!": {"default": ["!!"], "tts": ["!"]}},
            {"\n": {"tts": ["unsafe"]}},
            {"👍": {"tts": ["x\n"]}},
            {"👍": {"tts": ["x"], "unknown": []}},
        )
        for invalid in invalid_values:
            with self.subTest(invalid=invalid):
                with self.assertRaises(GenerationError):
                    build_annotations(invalid, {}, {})
        annotations = build_annotations(valid, {}, {})
        self.assertEqual(len(annotations), 1)
        with self.assertRaisesRegex(GenerationError, "code-point sorted"):
            encode_annotations([annotations[0], annotations[0]])

    def test_cli_check_rejects_unpinned_fixtures_without_writing(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            base = root / "base.json"
            derived = root / "derived.json"
            output = root / "annotations.bin"
            emoji = root / "emoji-test.txt"
            base.write_text(document("annotations", {}), encoding="utf-8")
            derived.write_text(document("annotationsDerived", {}), encoding="utf-8")
            emoji.write_text(EMOJI_FIXTURE, encoding="utf-8")
            with contextlib.redirect_stderr(io.StringIO()):
                result = main(
                    [
                        "--base",
                        str(base),
                        "--derived",
                        str(derived),
                        "--emoji",
                        str(emoji),
                        "--output",
                        str(output),
                        "--check",
                    ]
                )
            self.assertEqual(result, 1)
            self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main()
