"""Independently reconstruct and seal the private Wayland Settings evidence."""
from __future__ import annotations

import argparse
import base64
from pathlib import Path
import re
import sys

if __package__ in {None, ""}:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from scripts.gui_palette_evidence import decode_json, read_artifact, valid_pid

# Intentionally independent of the runner's case matrix and claimed outcomes.
CASE_NAMES = ("paste", "reject-multiline", "reject-oversize", "combining-backspace",
              "zwj-backspace", "pointer-wide", "pointer-combining", "home-delete",
              "end-word-backspace", "word-left-backspace", "word-right-delete")
FONT = re.compile(rb"Font size set to ([0-9]+(?:\.[0-9]+)?), grid resized to ([0-9]+)x([0-9]+)\r?\n")


def geometry(value):
    if not isinstance(value, list) or len(value) != 4 or not all(type(n) is int and 0 < n <= 65535 for n in value):
        raise ValueError("invalid Settings geometry")
    return value


def reconstruct(report, records, log):
    if not isinstance(report, dict) or type(report.get("schema")) is not int or report["schema"] != 2 or report.get("backend") != "wayland" or report.get("error") or "active_case" in report:
        raise ValueError("invalid or incomplete Settings sidecar")
    cases = report.get("cases")
    if not isinstance(cases, list) or [c.get("case") if isinstance(c, dict) else None for c in cases] != list(CASE_NAMES):
        raise ValueError("Settings evidence requires exactly eleven ordered cases")
    owner = cases[0].get("owner_pid")
    if not valid_pid(owner):
        raise ValueError("invalid Settings owner")
    inputs, streams, sizes, started = [], {}, [], set()
    for index, record in enumerate(records):
        if not isinstance(record, dict):
            raise ValueError("PTY transcript entry is not an object")
        event, pid = record.get("event"), record.get("pid")
        if event not in {"started", "winsize", "input"}:
            continue
        if not valid_pid(pid):
            raise ValueError("invalid fixture PID")
        if event == "started":
            if pid in started:
                raise ValueError("duplicate fixture start")
            started.add(pid)
        if pid not in started:
            raise ValueError("PTY event precedes its fixture start")
        if event in {"started", "winsize"} and pid == owner:
            sizes.append((index, geometry([record.get(key) for key in
                          ("rows", "cols", "pixel_width", "pixel_height")])))
        if event == "input":
            if not isinstance(record.get("base64"), str):
                raise ValueError("invalid PTY input encoding")
            data = base64.b64decode(record["base64"], validate=True)
            inputs.append((index, pid, data))
            streams.setdefault(pid, bytearray()).extend(data)

    def marker(token):
        token = token.encode()
        if sum(bytes(data).count(token) for data in streams.values()) != 1 or bytes(streams.get(owner, b"")).count(token) != 1:
            raise ValueError("missing, duplicated, or misrouted Settings marker")
        end = bytes(streams[owner]).index(token) + len(token)
        for index, pid, data in inputs:
            if pid == owner:
                if end <= len(data):
                    return index, end
                end -= len(data)
        raise ValueError("unreachable Settings marker")

    previous_end = marker("PALETTE_ORIGINAL_RESTORED")
    previous_log_end, baseline = 0, None
    for name, case in zip(CASE_NAMES, cases):
        if not valid_pid(case.get("owner_pid")) or case["owner_pid"] != owner:
            raise ValueError("Settings owner changed between cases")
        begin = marker(f"SETTINGS_BEGIN_{name}")
        fence = f"SETTINGS_END_{name}_{owner}".encode()
        end = marker(fence.decode())
        if not previous_end < begin < end:
            raise ValueError("Settings markers are out of order")
        delta = {}
        for index, pid, data in inputs:
            if begin[0] <= index <= end[0]:
                lo = begin[1] if index == begin[0] else 0
                hi = end[1] if index == end[0] else len(data)
                delta.setdefault(pid, bytearray()).extend(data[lo:hi])
        actual = {str(pid): bytes(data).hex() for pid, data in delta.items() if data}
        if actual != {str(owner): fence.hex()} or case.get("actual_hex") != actual:
            raise ValueError(f"Settings PTY delivery mismatch: {name}")
        before = next((size for index, size in reversed(sizes) if index <= begin[0]), None)
        if before is None:
            raise ValueError("missing initial Settings geometry")
        transitions = [before]
        for index, size in sizes:
            if begin[0] < index <= end[0] and size != transitions[-1]:
                transitions.append(size)
        if len(transitions) != 3 or transitions[-1] != before:
            raise ValueError("Settings geometry did not change and restore exactly")
        larger = transitions[1]
        if not (larger[0] < before[0] and larger[1] < before[1]):
            raise ValueError("Settings larger font did not reduce rows and columns")
        if baseline is not None and before != baseline:
            raise ValueError("Settings baseline changed between cases")
        baseline = before
        if [geometry(case.get(key)) for key in ("before", "larger", "restored")] != transitions:
            raise ValueError("Settings geometry sidecar disagrees with PTY")
        span = case.get("log_range")
        if not isinstance(span, list) or len(span) != 2 or not all(type(n) is int for n in span):
            raise ValueError("invalid Settings log range")
        start, stop = span
        if not previous_log_end <= start < stop <= len(log) or (start and log[start - 1] != 10) or log[stop - 1] != 10:
            raise ValueError("Settings log ranges overlap, split lines, or exceed the artifact")
        section = log[start:stop]
        changes = [[float(size), int(cols), int(rows)] for size, cols, rows in FONT.findall(section)]
        expected = [[15, larger[1], larger[0]], [14, before[1], before[0]]]
        if changes != expected or case.get("font_changes") != expected or section.count(b"Font size set to ") != 2:
            raise ValueError("Settings font diagnostics disagree with PTY transitions")
        if type(case.get("unrelated_focus_losses")) is not int or case["unrelated_focus_losses"] != 0 or b"Window focus changed: false" in section:
            raise ValueError("Settings evidence contains unrelated focus loss")
        previous_end, previous_log_end = end, stop
    # Reject omitted applications rather than letting ranges skip duplicates.
    if log.count(b"Font size set to ") != 2 * len(CASE_NAMES):
        raise ValueError("font applications occurred outside the Settings case ranges")


def collect_settings_evidence(root: Path):
    sidecar, sidecar_seal = read_artifact(root, "interaction/settings-input.json", 64 * 1024)
    transcript, transcript_seal = read_artifact(root, "interaction/pty-transcript.jsonl", 8 * 1024 * 1024)
    log, log_seal = read_artifact(root, "interaction/termeric.log", 16 * 1024 * 1024)
    reconstruct(decode_json(sidecar), [decode_json(line) for line in transcript.splitlines()], log)
    return {"schema": 1, "case_count": len(CASE_NAMES), "artifacts": [sidecar_seal, transcript_seal, log_seal]}


def validate_record(path: Path):
    data, _ = read_artifact(path.parent, path.name, 1024 * 1024)
    record = decode_json(data)
    candidate = record.get("candidate", {})
    if record.get("schema") != 6 or record.get("backend") != "wayland" or candidate.get("schema") != 3:
        raise ValueError("Settings validation requires a schema-6 Wayland candidate record")
    if candidate.get("candidate_eligible") is not True or "interaction" not in candidate.get("requested_scenarios", []):
        raise ValueError("record is not eligible Wayland interaction evidence")
    collected = collect_settings_evidence(path.parent)
    if candidate.get("interaction_settings") != collected:
        raise ValueError("Settings artifact seals or independent result changed")
    return collected


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Re-check sealed Settings routing/geometry/log evidence, not visual or IME acceptance")
    parser.add_argument("record", type=Path)
    args = parser.parse_args()
    try:
        result = validate_record(args.record)
    except (ValueError, OSError, TypeError, AttributeError, RecursionError) as error:
        parser.exit(1, f"GUI Settings evidence failed: {error}\n")
    print(f"Validated {result['case_count']} native Settings cases and all three artifact seals")
