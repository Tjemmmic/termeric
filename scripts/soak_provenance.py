"""Linux soak launch identity, independent of resource-result evaluation.

The runner owns one read-only CLOEXEC descriptor. Children execute through its
parent /proc fd path without inheriting that descriptor. Replacing the original
pathname cannot switch the executable between measurement and exec, or between
GPU smoke and scenario. This detects observed drift, not hostile same-user
tampering or signed compiler provenance.
"""
from __future__ import annotations

import copy
import hashlib
import os
from pathlib import Path
import re
import stat
import subprocess
from typing import Any

POLICY = "linux-parent-executable-fd-v1"
MAX_EXECUTABLE_BYTES = 2 * 1024 * 1024 * 1024
IDENTITY_KEYS = {"device", "inode", "bytes", "mtime_ns", "ctime_ns", "sha256"}
SOURCE_KEYS = {"commit", "worktree_dirty"}
RECORD_KEYS = {
    "policy", "source_initial", "source_ready", "source_final",
    "binary_initial", "binary_final", "pinned_final", "launches",
}


def source_identity(root: Path) -> dict[str, Any]:
    commit = subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=root, text=True,
        stdout=subprocess.PIPE, check=True,
    ).stdout.strip()
    dirty = bool(subprocess.run(
        ["git", "status", "--porcelain=v1", "--untracked-files=all"], cwd=root,
        text=True, stdout=subprocess.PIPE, check=True,
    ).stdout.strip())
    if re.fullmatch(r"[0-9a-f]{40}", commit) is None:
        raise ValueError("invalid source commit")
    return {"commit": commit, "worktree_dirty": dirty}


def observe_source(root: Path) -> dict[str, Any] | None:
    try:
        return source_identity(root)
    except (OSError, subprocess.SubprocessError, ValueError):
        return None


def metadata_identity(metadata: os.stat_result) -> dict[str, int]:
    if not stat.S_ISREG(metadata.st_mode) or not metadata.st_mode & 0o111:
        raise ValueError("soak executable must be a regular executable file")
    if not 0 < metadata.st_size <= MAX_EXECUTABLE_BYTES:
        raise ValueError("soak executable exceeds its byte limit or is empty")
    return {
        "device": metadata.st_dev, "inode": metadata.st_ino,
        "bytes": metadata.st_size, "mtime_ns": metadata.st_mtime_ns,
        "ctime_ns": metadata.st_ctime_ns,
    }


def descriptor_identity(fd: int) -> dict[str, Any]:
    before = metadata_identity(os.fstat(fd))
    digest = hashlib.sha256()
    offset = 0
    while offset < before["bytes"]:
        chunk = os.pread(fd, min(1024 * 1024, before["bytes"] - offset), offset)
        if not chunk:
            raise ValueError("soak executable changed during measurement")
        digest.update(chunk)
        offset += len(chunk)
    if metadata_identity(os.fstat(fd)) != before or os.pread(fd, 1, offset):
        raise ValueError("soak executable changed during measurement")
    return {**before, "sha256": digest.hexdigest()}


def open_executable(path: Path) -> int:
    return os.open(path, os.O_RDONLY | os.O_CLOEXEC | os.O_NONBLOCK | os.O_NOFOLLOW)


def observe_binary(path: Path) -> dict[str, Any] | None:
    try:
        fd = open_executable(path)
        try:
            return descriptor_identity(fd)
        finally:
            os.close(fd)
    except (OSError, ValueError):
        return None


class PinnedExecutable:
    def __init__(self, root: Path, path: Path, initial_source: dict[str, Any]):
        self.root = root.resolve()
        self.path = path.absolute()
        relative = self.path.relative_to(self.root)
        if any(component in {".", ".."} for component in relative.parts):
            raise ValueError("soak executable path is not canonical")
        current = self.root
        for component in relative.parts:
            current /= component
            if current.is_symlink():
                raise ValueError("soak executable path contains a symlink")
        self.fd = open_executable(self.path)
        try:
            initial_binary = descriptor_identity(self.fd)
            ready_source = observe_source(self.root)
        except BaseException:
            os.close(self.fd)
            raise
        self.file_evidence = {
            "path": relative.as_posix(), "bytes": initial_binary["bytes"],
            "sha256": initial_binary["sha256"],
        }
        self.record = {
            "policy": POLICY, "source_initial": copy.deepcopy(initial_source),
            "source_ready": ready_source, "source_final": None,
            "binary_initial": initial_binary, "binary_final": None,
            "pinned_final": None, "launches": [],
        }

    def __enter__(self):
        return self

    def __exit__(self, *_):
        os.close(self.fd)

    def launch(self, role: str) -> Path:
        if role not in {"gpu-smoke", "scenario", "displayed"} or len(self.record["launches"]) >= 2:
            raise ValueError("invalid soak launch role or count")
        source = observe_source(self.root)
        binary = descriptor_identity(self.fd)
        self.record["launches"].append({"role": role, "source": source, "binary": binary})
        if (source != self.record["source_initial"]
            or self.record["source_ready"] != self.record["source_initial"]
            or binary != self.record["binary_initial"]):
            raise ValueError("soak source or executable changed before launch")
        # /proc/self/fd would require inheriting the fd into every child. The
        # parent-owned path keeps it private and pinned through exec instead.
        return Path(f"/proc/{os.getpid()}/fd/{self.fd}")

    def finish(self) -> dict[str, Any]:
        self.record["source_final"] = observe_source(self.root)
        self.record["binary_final"] = observe_binary(self.path)
        try:
            self.record["pinned_final"] = descriptor_identity(self.fd)
        except (OSError, ValueError):
            self.record["pinned_final"] = None
        return copy.deepcopy(self.record)


def validate_source(value: object, *, optional: bool = False) -> None:
    if value is None and optional:
        return
    if (not isinstance(value, dict) or set(value) != SOURCE_KEYS
        or type(value["worktree_dirty"]) is not bool
        or not isinstance(value["commit"], str)
        or re.fullmatch(r"[0-9a-f]{40}", value["commit"]) is None):
        raise ValueError("invalid soak source provenance")


def validate_binary(value: object, *, optional: bool = False) -> None:
    if value is None and optional:
        return
    if not isinstance(value, dict) or set(value) != IDENTITY_KEYS:
        raise ValueError("invalid soak binary provenance")
    for field in IDENTITY_KEYS - {"sha256"}:
        if type(value[field]) is not int or not 0 <= value[field] < 2**64:
            raise ValueError("invalid soak executable identity number")
    if (not 0 < value["bytes"] <= MAX_EXECUTABLE_BYTES
        or not isinstance(value["sha256"], str)
        or re.fullmatch(r"[0-9a-f]{64}", value["sha256"]) is None):
        raise ValueError("invalid soak executable length/digest")


def provenance_checks(record: object, roles: list[str]) -> list[dict[str, Any]]:
    if not isinstance(record, dict) or set(record) != RECORD_KEYS or record["policy"] != POLICY:
        raise ValueError("unsupported soak provenance contract")
    validate_source(record["source_initial"])
    for field in ("source_ready", "source_final"):
        validate_source(record[field], optional=True)
    validate_binary(record["binary_initial"])
    for field in ("binary_final", "pinned_final"):
        validate_binary(record[field], optional=True)
    launches = record["launches"]
    if not isinstance(launches, list) or len(launches) > 2:
        raise ValueError("invalid soak launch inventory")
    for launch in launches:
        if (not isinstance(launch, dict) or set(launch) != {"role", "source", "binary"}
            or not isinstance(launch["role"], str)
            or launch["role"] not in {"gpu-smoke", "scenario", "displayed"}):
            raise ValueError("invalid soak launch identity")
        validate_source(launch["source"], optional=True)
        validate_binary(launch["binary"])
    initial_source = record["source_initial"]
    initial_binary = record["binary_initial"]
    checks = [
        ("source provenance unchanged through build and run",
         record["source_ready"] == initial_source == record["source_final"]
         and all(item["source"] == initial_source for item in launches)),
        ("measured executable identity unchanged through run",
         record["binary_final"] == initial_binary == record["pinned_final"]
         and all(item["binary"] == initial_binary for item in launches)),
        ("expected executable launch attempts recorded", [item["role"] for item in launches] == roles),
    ]
    return [{"name": name, "actual": passed, "expected": True, "passed": passed}
            for name, passed in checks]


def validate_record_binding(record: dict, environment: dict, file_evidence: dict) -> None:
    if record["source_initial"] != {key: environment[key] for key in SOURCE_KEYS}:
        raise ValueError("environment differs from initial source provenance")
    if any(record["binary_initial"][key] != file_evidence[key] for key in ("bytes", "sha256")):
        raise ValueError("binary evidence differs from measured launch object")
