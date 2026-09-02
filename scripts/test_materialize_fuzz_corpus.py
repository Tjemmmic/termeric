import base64
import tempfile
import unittest
from pathlib import Path

from scripts.materialize_fuzz_corpus import (
    DEFAULT_CHUNK_SIZE,
    DEFAULT_COLS,
    DEFAULT_ROWS,
    load_toml,
    materialize,
    terminal_fuzz_input,
)


class MaterializeFuzzCorpusTests(unittest.TestCase):
    def test_terminal_fuzz_input_preserves_every_payload_byte(self):
        payload = b"\x1b[31mhello\x1b[0m"

        encoded = terminal_fuzz_input(payload, cols=160, rows=80, chunk_size=32)

        self.assertEqual(encoded[:3], bytes((159, 79, 31)))
        self.assertEqual(encoded[3:], payload)

    def test_terminal_fuzz_input_rejects_unrepresentable_metadata(self):
        for keyword, value in (
            ("cols", 0),
            ("cols", 161),
            ("rows", 81),
            ("chunk_size", 33),
            ("chunk_size", True),
        ):
            with self.subTest(keyword=keyword, value=value):
                with self.assertRaises(ValueError):
                    terminal_fuzz_input(b"payload", **{keyword: value})

    def test_materialized_seeds_wrap_but_do_not_consume_reviewed_payloads(self):
        expected: dict[str, tuple[bytes, bytes]] = {}
        for case in load_toml("conformance_cases.toml")["case"]:
            expected[f"fixture-{case['id']}.seed"] = (
                bytes(
                    (case["cols"] - 1, case["rows"] - 1, DEFAULT_CHUNK_SIZE - 1)
                ),
                bytes.fromhex(case["input_hex"]),
            )
        for seed in load_toml("parser_test_seeds.toml")["seed"]:
            expected[f"parser-{seed['id']}.seed"] = (
                bytes((DEFAULT_COLS - 1, DEFAULT_ROWS - 1, DEFAULT_CHUNK_SIZE - 1)),
                bytes.fromhex(seed["input_hex"]),
            )
        for trace in load_toml("external_traces.toml")["trace"]:
            expected[f"trace-{trace['id']}.seed"] = (
                bytes((trace["cols"] - 1, trace["rows"] - 1, DEFAULT_CHUNK_SIZE - 1)),
                base64.b64decode(
                    "".join(trace["input_base64"].split()), validate=True
                ),
            )

        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            count = materialize(output)
            actual_names = {path.name for path in output.iterdir()}

            self.assertEqual(count, len(expected))
            self.assertEqual(actual_names, set(expected))
            for name, (header, payload) in expected.items():
                with self.subTest(seed=name):
                    encoded = (output / name).read_bytes()
                    self.assertEqual(encoded[:3], header)
                    self.assertEqual(encoded[3:], payload)


if __name__ == "__main__":
    unittest.main()
