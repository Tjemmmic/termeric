"""Bounded, independently reconstructed native palette evidence and file seals."""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import stat


# Deliberately independent of the runner's CASES and its reported expected bytes.
CASE_NAMES = ("middle-delete", "left-insert", "home-right", "end-insert",
              "combining-backspace", "zwj-backspace", "flag-backspace")


def read_artifact(root: Path, name: str, limit: int):
    path = root / name
    for parent in [path, *path.parents]:
        if parent == root:
            break
        if parent.is_symlink():
            raise ValueError(f"symlinked GUI artifact: {name}")
    fd = os.open(path, os.O_RDONLY | os.O_CLOEXEC | os.O_NONBLOCK | os.O_NOFOLLOW)
    try:
        before = os.fstat(fd)
        if not stat.S_ISREG(before.st_mode) or not 0 < before.st_size <= limit:
            raise ValueError(f"GUI artifact is not a bounded regular file: {name}")
        with os.fdopen(fd, "rb", closefd=False) as source:
            data = source.read(limit + 1)
        after = os.fstat(fd)
        current = path.stat(follow_symlinks=False)
        identity = lambda s: (s.st_dev, s.st_ino, s.st_size, s.st_mtime_ns, s.st_ctime_ns)
        if len(data) != before.st_size or identity(before) != identity(after) or identity(after) != identity(current):
            raise ValueError(f"GUI artifact changed during reading: {name}")
        return data, {"path": name, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}
    finally:
        os.close(fd)


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def decode_json(data):
    return json.loads(data, object_pairs_hook=unique_object)


def valid_pid(value):
    return type(value) is int and value > 0


def reconstruct(report, records):
    if not isinstance(report, dict) or type(report.get("schema")) is not int or report["schema"] != 1 or report.get("backend") != "wayland" or report.get("error"):
        raise ValueError("invalid palette sidecar header or recorded failure")
    cases = report.get("cases")
    if not isinstance(cases, list) or [c.get("case") if isinstance(c, dict) else None for c in cases] != list(CASE_NAMES):
        raise ValueError("palette evidence must contain exactly the seven ordered cases")
    inputs, streams, starts = [], {}, set()
    for record in records:
        if not isinstance(record, dict):
            raise ValueError("PTY transcript entry is not an object")
        if record.get("event") == "started":
            if not valid_pid(record.get("pid")):
                raise ValueError("invalid fixture PID")
            starts.add(record["pid"])
        if record.get("event") != "input":
            continue
        pid = record.get("pid")
        if not valid_pid(pid) or not isinstance(record.get("base64"), str):
            raise ValueError("invalid PTY input record")
        data = base64.b64decode(record["base64"], validate=True)
        inputs.append((pid, data))
        streams.setdefault(pid, bytearray()).extend(data)

    def marker(owner, token):
        token = token.encode()
        if sum(bytes(data).count(token) for data in streams.values()) != 1 or bytes(streams.get(owner, b"")).count(token) != 1:
            raise ValueError("missing, duplicated, or misrouted palette marker")
        end = bytes(streams[owner]).index(token) + len(token)
        for index, (pid, data) in enumerate(inputs):
            if pid == owner:
                if end <= len(data):
                    return index, end
                end -= len(data)
        raise ValueError("unreachable palette marker")

    owners = (cases[0].get("owner_pid"), cases[0].get("sibling_pid"))
    if not all(valid_pid(pid) for pid in owners) or owners[0] == owners[1] or not set(owners) <= starts:
        raise ValueError("palette cases require two distinct started PTYs")
    previous_end = (-1, 0)
    for index, case in enumerate(cases):
        owner, other = owners[index % 2], owners[1 - index % 2]
        if not valid_pid(case.get("owner_pid")) or not valid_pid(case.get("sibling_pid")) or case["owner_pid"] != owner or case["sibling_pid"] != other:
            raise ValueError("palette owners did not alternate exactly")
        if type(case.get("unrelated_focus_losses")) is not int or case["unrelated_focus_losses"] != 0:
            raise ValueError("palette evidence contains unrelated focus loss")
        name = CASE_NAMES[index]
        begin = marker(owner, f"PALETTE_BEGIN_{name}")
        fences = {pid: f"PALETTE_END_{name}_{pid}".encode() for pid in (owner, other)}
        owner_end = marker(owner, fences[owner].decode())
        end = marker(other, fences[other].decode())
        if not previous_end < begin < owner_end < end:
            raise ValueError("palette markers are out of order")
        delta = {}
        for number in range(begin[0], end[0] + 1):
            pid, data = inputs[number]
            lo = begin[1] if number == begin[0] else 0
            hi = end[1] if number == end[0] else len(data)
            delta.setdefault(pid, bytearray()).extend(data[lo:hi])
        actual = {str(pid): bytes(data).hex() for pid, data in delta.items() if data}
        expected = {str(owner): (b"\xe2\x98\x83" + fences[owner]).hex(), str(other): fences[other].hex()}
        if actual != expected or case.get("actual_hex") != actual or case.get("expected_hex") != expected:
            raise ValueError(f"palette transcript/sidecar mismatch: {name}")
        previous_end = end
    if marker(owners[0], "PALETTE_ORIGINAL_RESTORED") <= previous_end:
        raise ValueError("original palette owner was not restored afterward")


def collect_palette_evidence(root: Path):
    sidecar, sidecar_seal = read_artifact(root, "interaction/palette-input.json", 64 * 1024)
    transcript, transcript_seal = read_artifact(root, "interaction/pty-transcript.jsonl", 8 * 1024 * 1024)
    reconstruct(decode_json(sidecar), [decode_json(line) for line in transcript.splitlines()])
    return {"schema": 1, "case_count": len(CASE_NAMES), "artifacts": [sidecar_seal, transcript_seal]}


def validate_record(path: Path):
    data, _ = read_artifact(path.parent, path.name, 1024 * 1024)
    record = decode_json(data)
    candidate = record.get("candidate", {})
    if record.get("schema") != 5 or record.get("backend") != "wayland" or candidate.get("schema") != 2:
        raise ValueError("palette validation requires a schema-5 Wayland candidate record")
    if candidate.get("candidate_eligible") is not True or "interaction" not in candidate.get("requested_scenarios", []):
        raise ValueError("record is not eligible Wayland interaction evidence")
    collected = collect_palette_evidence(path.parent)
    if candidate.get("interaction_palette") != collected:
        raise ValueError("palette artifact seals or independent result changed")
    return collected


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Re-check sealed GUI palette evidence, not other GUI scenarios or compiler provenance")
    parser.add_argument("record", type=Path)
    args = parser.parse_args()
    try:
        result = validate_record(args.record)
    except (ValueError, OSError, TypeError, AttributeError, RecursionError) as error:
        parser.exit(1, f"GUI palette evidence failed: {error}\n")
    print(f"Validated {result['case_count']} native palette cases and both artifact seals")
