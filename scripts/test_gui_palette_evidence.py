import base64
import copy
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

from scripts.gui_palette_evidence import CASE_NAMES, collect_palette_evidence, read_artifact, reconstruct, validate_record


def fixture():
    records = [{"event": "started", "pid": pid} for pid in (7, 8)]
    report = {"schema": 1, "backend": "wayland", "cases": []}

    def append(pid, data):
        # Every byte is a separate input record, including UTF-8 and fences.
        records.extend({"event": "input", "pid": pid,
                        "base64": base64.b64encode(bytes([byte])).decode()} for byte in data)

    for index, name in enumerate(CASE_NAMES):
        owner, other = (7, 8) if index % 2 == 0 else (8, 7)
        append(owner, f"PALETTE_BEGIN_{name}".encode())
        expected = {str(owner): ("☃".encode() + f"PALETTE_END_{name}_{owner}".encode()).hex(),
                    str(other): f"PALETTE_END_{name}_{other}".encode().hex()}
        for pid in (owner, other):
            append(pid, bytes.fromhex(expected[str(pid)]))
        report["cases"].append({"case": name, "owner_pid": owner, "sibling_pid": other,
                                "expected_hex": expected, "actual_hex": dict(expected),
                                "unrelated_focus_losses": 0})
    append(7, b"PALETTE_ORIGINAL_RESTORED")
    return report, records


def write_fixture(root):
    report, records = fixture()
    directory = root / "interaction"
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "palette-input.json").write_text(json.dumps(report))
    (directory / "pty-transcript.jsonl").write_text("".join(json.dumps(r) + "\n" for r in records))
    return report, records


class PaletteEvidenceTests(unittest.TestCase):
    def test_reconstructs_fragmented_real_bytes_and_coalesced_case_boundaries(self):
        report, records = fixture()
        reconstruct(report, records)
        merged = []
        for record in records:
            if merged and record["event"] == "input" and merged[-1]["event"] == "input" and record["pid"] == merged[-1]["pid"]:
                merged[-1]["base64"] = base64.b64encode(base64.b64decode(merged[-1]["base64"]) + base64.b64decode(record["base64"])).decode()
            else:
                merged.append(dict(record))
        reconstruct(report, merged)

    def test_self_reported_success_cannot_replace_missing_or_corrupt_transcript(self):
        report, records = fixture()
        variants = [records[:-1], records[2:], []]
        corrupted = copy.deepcopy(records)
        first_utf8 = next(r for r in corrupted if r.get("base64") == base64.b64encode(b"\xe2").decode())
        first_utf8["base64"] = base64.b64encode(b"X").decode()
        variants.append(corrupted)
        variants.append(records + [{"event": "input", "pid": 8, "base64": base64.b64encode(b"PALETTE_BEGIN_middle-delete").decode()}])
        for data in variants:
            with self.subTest(data=data[:2]), self.assertRaises(ValueError):
                reconstruct(report, data)

    def test_rejects_missing_reordered_duplicate_or_forged_cases(self):
        report, records = fixture()
        changes = [lambda r: r["cases"].pop(), lambda r: r["cases"].reverse(),
                   lambda r: r["cases"].__setitem__(1, r["cases"][0]),
                   lambda r: r["cases"][0].__setitem__("owner_pid", 8),
                   lambda r: r["cases"][0].__setitem__("unrelated_focus_losses", 1),
                   lambda r: r["cases"][0].__setitem__("actual_hex", {}),
                   lambda r: r["cases"][0].__setitem__("expected_hex", {}),
                   lambda r: r.__setitem__("error", "failed")]
        for change in changes:
            altered = copy.deepcopy(report)
            change(altered)
            with self.assertRaises(ValueError):
                reconstruct(altered, records)

    def test_agreeing_sidecar_and_transcript_cannot_redefine_expected_insertion(self):
        report, records = fixture()
        for case in report["cases"]:
            for field in ("expected_hex", "actual_hex"):
                case[field] = {pid: value.replace("e29883", "e39883") for pid, value in case[field].items()}
        for record in records:
            if record.get("base64") == base64.b64encode(b"\xe2").decode():
                record["base64"] = base64.b64encode(b"\xe3").decode()
        with self.assertRaisesRegex(ValueError, "mismatch"):
            reconstruct(report, records)

    def test_seals_both_files_and_refuses_post_run_change_and_old_schema(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            write_fixture(root)
            proof = collect_palette_evidence(root)
            record = {"schema": 5, "backend": "wayland", "candidate": {
                "schema": 2, "candidate_eligible": True, "requested_scenarios": ["interaction"],
                "interaction_palette": proof,
            }}
            path = root / "results.json"
            path.write_text(json.dumps(record))
            self.assertEqual(validate_record(path), proof)
            sidecar = root / "interaction/palette-input.json"
            sidecar.write_text(sidecar.read_text() + "\n")
            with self.assertRaisesRegex(ValueError, "seals"):
                validate_record(path)
            write_fixture(root)
            record["schema"] = 4
            path.write_text(json.dumps(record))
            with self.assertRaisesRegex(ValueError, "schema-5"):
                validate_record(path)

    def test_artifact_reader_refuses_missing_symlink_fifo_oversize_and_duplicate_keys(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for mode in ("missing", "symlink", "fifo", "oversize", "duplicate"):
                write_fixture(root)
                path = root / "interaction/palette-input.json"
                path.unlink()
                if mode == "symlink":
                    path.symlink_to("pty-transcript.jsonl")
                elif mode == "fifo":
                    os.mkfifo(path)
                elif mode == "oversize":
                    path.write_bytes(b" " * (64 * 1024 + 1))
                elif mode == "duplicate":
                    path.write_text('{"schema":1,"schema":1}')
                with self.subTest(mode=mode), self.assertRaises((ValueError, OSError)):
                    collect_palette_evidence(root)
                if path.exists() or path.is_symlink():
                    path.unlink()

    def test_artifact_changed_during_read_is_not_sealed(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            write_fixture(root)
            before = (root / "interaction/palette-input.json").stat()
            changed = SimpleNamespace(**{key: getattr(before, key) for key in
                ("st_dev", "st_ino", "st_size", "st_mtime_ns", "st_ctime_ns")})
            changed.st_ctime_ns += 1
            with mock.patch("scripts.gui_palette_evidence.os.fstat", side_effect=[before, changed]):
                with self.assertRaisesRegex(ValueError, "changed during reading"):
                    read_artifact(root, "interaction/palette-input.json", 64 * 1024)


if __name__ == "__main__":
    unittest.main()
