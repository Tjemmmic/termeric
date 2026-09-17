import base64
import copy
import json
import os
from pathlib import Path
import tempfile
import subprocess
import sys
import unittest

from scripts import gui_settings_evidence as evidence

BEFORE = [42, 126, 1008, 714]
LARGER = [40, 112, 1008, 720]


def size_record(pid, shape, event="winsize"):
    return dict(event=event, pid=pid, **dict(zip(("rows", "cols", "pixel_width", "pixel_height"), shape)))


def input_record(pid, data):
    return {"event": "input", "pid": pid, "base64": base64.b64encode(data).decode()}


def fixture():
    records = [size_record(pid, BEFORE, "started") for pid in (7, 8)]
    records.append(input_record(7, b"PALETTE_ORIGINAL_RESTORED"))
    report = {"schema": 2, "backend": "wayland", "cases": []}
    log = b"startup\n"
    for name in evidence.CASE_NAMES:
        records.extend(input_record(7, bytes([byte])) for byte in f"SETTINGS_BEGIN_{name}".encode())
        records.extend([size_record(7, LARGER), size_record(7, BEFORE)])
        fence = f"SETTINGS_END_{name}_7".encode()
        records.extend(input_record(7, bytes([byte])) for byte in fence)
        start = len(log)
        log += (b"INFO Font size set to 15, grid resized to 112x40\n"
                b"INFO Font size set to 14, grid resized to 126x42\n")
        report["cases"].append({"case": name, "owner_pid": 7, "before": list(BEFORE),
            "larger": list(LARGER), "restored": list(BEFORE), "actual_hex": {"7": fence.hex()},
            "font_changes": [[15, 112, 40], [14, 126, 42]], "unrelated_focus_losses": 0,
            "log_range": [start, len(log)]})
    return report, records, log + b"shutdown\n"


def write_fixture(root, append=False):
    report, records, log = fixture()
    directory = root / "interaction"
    directory.mkdir(parents=True, exist_ok=True)
    transcript = directory / "pty-transcript.jsonl"
    if append and transcript.exists():
        previous = [json.loads(line) for line in transcript.read_text().splitlines()]
        for record in previous:
            if record["event"] == "started":
                record.update(size_record(record["pid"], BEFORE, "started"))
        records = previous + records[3:]  # keep the palette's original-owner fence
    transcript.write_text("".join(json.dumps(record) + "\n" for record in records))
    (directory / "settings-input.json").write_text(json.dumps(report))
    (directory / "termeric.log").write_bytes(log)
    return report, records, log


class SettingsEvidenceTests(unittest.TestCase):
    def test_standalone_cli_can_read_an_eligible_fixture(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            write_fixture(root)
            record = {"schema": 6, "backend": "wayland", "candidate": {
                "schema": 3, "candidate_eligible": True, "requested_scenarios": ["interaction"],
                "interaction_settings": evidence.collect_settings_evidence(root)}}
            path = root / "results.json"
            path.write_text(json.dumps(record))
            result = subprocess.run([sys.executable, str(Path(evidence.__file__).resolve()), str(path)],
                                    cwd=root, capture_output=True, text=True, timeout=5)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("11 native Settings cases", result.stdout)

    def test_reconstructs_fragmented_markers_and_coalesced_input(self):
        report, records, log = fixture()
        evidence.reconstruct(report, records, log)
        merged = []
        for record in records:
            if merged and record["event"] == merged[-1]["event"] == "input" and record["pid"] == merged[-1]["pid"]:
                merged[-1]["base64"] = base64.b64encode(base64.b64decode(merged[-1]["base64"]) + base64.b64decode(record["base64"])).decode()
            else:
                merged.append(dict(record))
        evidence.reconstruct(report, merged, log)

    def test_rejects_old_incomplete_reordered_or_self_reported_success(self):
        report, records, log = fixture()
        changes = [lambda r: r.update(schema=1), lambda r: r.update(schema=True),
            lambda r: r.update(error="failed"), lambda r: r.update(active_case="paste"),
            lambda r: r["cases"].pop(), lambda r: r["cases"].reverse(),
            lambda r: r["cases"].__setitem__(1, r["cases"][0]),
            lambda r: r["cases"][0].update(owner_pid=8),
            lambda r: r["cases"][0].update(actual_hex={}),
            lambda r: r["cases"][0].update(unrelated_focus_losses=True),
            lambda r: r["cases"][0].update(unrelated_focus_losses=1),
            lambda r: r["cases"][0].update(font_changes=[[14, 126, 42]])]
        for change in changes:
            altered = copy.deepcopy(report)
            change(altered)
            with self.assertRaises(ValueError):
                evidence.reconstruct(altered, records, log)

    def test_exact_owner_fences_reject_leaks_duplicates_and_missing_restore(self):
        report, records, log = fixture()
        first_resize = next(i for i, r in enumerate(records) if r["event"] == "winsize")
        for pid, data in [(7, b"query"), (8, b"query"), (7, b"SETTINGS_BEGIN_paste")]:
            altered = records[:first_resize] + [input_record(pid, data)] + records[first_resize:]
            with self.assertRaises(ValueError):
                evidence.reconstruct(report, altered, log)
        for altered in [records[2:], records[:2] + records[3:], records[:-1]]:
            with self.assertRaises(ValueError):
                evidence.reconstruct(report, altered, log)

    def test_geometry_must_be_reconstructed_not_taken_from_sidecar(self):
        report, records, log = fixture()
        for change in [lambda r: r.update(rows=0), lambda r: r.update(cols=True),
                       lambda r: r.update(cols=140), lambda r: r.update(pixel_height=700)]:
            altered = copy.deepcopy(records)
            change(next(r for r in altered if r["event"] == "winsize"))
            with self.assertRaises(ValueError):
                evidence.reconstruct(report, altered, log)
        altered = copy.deepcopy(report)
        altered["cases"][0]["larger"][1] = 111
        with self.assertRaises(ValueError):
            evidence.reconstruct(altered, records, log)
        missing = list(records)
        missing.pop(next(i for i, r in enumerate(missing) if r["event"] == "winsize"))
        with self.assertRaises(ValueError):
            evidence.reconstruct(report, missing, log)

    def test_forged_geometry_cannot_redefine_a_larger_font(self):
        report, records, log = fixture()
        for case in report["cases"]:
            case["larger"][1] = 140
            case["font_changes"][0][1] = 140
        for record in records:
            if record.get("cols") == 112:
                record["cols"] = 140
        with self.assertRaises(ValueError):
            evidence.reconstruct(report, records, log.replace(b"112x40", b"140x40"))

    def test_log_ranges_reject_overlap_partial_lines_and_unreported_diagnostics(self):
        report, records, log = fixture()
        for span in [[-1, 10], [True, 10], [8, len(log) + 1], [9, 100], [8, 8], [8]]:
            altered = copy.deepcopy(report)
            altered["cases"][0]["log_range"] = span
            with self.assertRaises(ValueError):
                evidence.reconstruct(altered, records, log)
        altered = copy.deepcopy(report)
        altered["cases"][1]["log_range"] = altered["cases"][0]["log_range"]
        with self.assertRaises(ValueError):
            evidence.reconstruct(altered, records, log)
        for extra in [b"Font size set to 15, grid resized to 112x40\n", b"Font size set to malformed\n"]:
            with self.assertRaises(ValueError):
                evidence.reconstruct(report, records, log + extra)
        # Keep all byte ranges valid while adding a focus loss within a case.
        altered = copy.deepcopy(report)
        offset = report["cases"][0]["log_range"][0]
        extra = b"Window focus changed: false\n"
        for index, case in enumerate(altered["cases"]):
            if index:
                case["log_range"][0] += len(extra)
            case["log_range"][1] += len(extra)
        with self.assertRaisesRegex(ValueError, "focus loss"):
            evidence.reconstruct(altered, records, log[:offset] + extra + log[offset:])

    def test_seals_all_artifacts_and_refuses_later_byte_changes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for name in ["settings-input.json", "pty-transcript.jsonl", "termeric.log"]:
                write_fixture(root)
                proof = evidence.collect_settings_evidence(root)
                record = {"schema": 6, "backend": "wayland", "candidate": {
                    "schema": 3, "candidate_eligible": True, "requested_scenarios": ["interaction"],
                    "interaction_settings": proof}}
                path = root / "results.json"
                path.write_text(json.dumps(record))
                self.assertEqual(evidence.validate_record(path), proof)
                changed = root / "interaction" / name
                changed.write_bytes(changed.read_bytes() + b"\n")
                with self.assertRaises(ValueError):
                    evidence.validate_record(path)
            record["schema"] = 5
            path.write_text(json.dumps(record))
            with self.assertRaisesRegex(ValueError, "schema-6"):
                evidence.validate_record(path)

    def test_refuses_unsafe_missing_and_oversize_log_artifacts(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for mode in ["missing", "symlink", "fifo", "oversize"]:
                write_fixture(root)
                path = root / "interaction/termeric.log"
                path.unlink()
                if mode == "symlink":
                    path.symlink_to("settings-input.json")
                elif mode == "fifo":
                    os.mkfifo(path)
                elif mode == "oversize":
                    path.write_bytes(b"x" * (16 * 1024 * 1024 + 1))
                with self.assertRaises((ValueError, OSError)):
                    evidence.collect_settings_evidence(root)
                if path.exists() or path.is_symlink():
                    path.unlink()


if __name__ == "__main__":
    unittest.main()
