#!/usr/bin/env python3
"""Run and validate Termeric's candidate-bound hermetic release preflight.

This Linux preflight has a fixed provider-free command set. It establishes
source/build/test/documentation evidence only; it does not run native GUI,
package-manager, long-duration soak, remote-host, or authenticated-provider
acceptance.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import shutil
import subprocess
import sys
import tempfile
import time
import tomllib


SCHEMA_VERSION = 1
RECORD_KIND = "termeric-hermetic-release-preflight"
SUMMARY_LIMIT = 4096
PROVIDER_ENVIRONMENT = {
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CODEX_API_KEY",
    "OPENAI_API_KEY",
    "OPENAI_ORG_ID",
    "TERMERIC_FAKE_AI_LOG",
    "TERMERIC_REAL_CODEX_CONTRACT",
}


class PreflightError(RuntimeError):
    pass


@dataclass(frozen=True)
class GateSpec:
    name: str
    argv: tuple[str, ...]
    timeout_seconds: int


GATES = (
    GateSpec("rust-format", ("cargo", "fmt", "--all", "--", "--check"), 180),
    GateSpec(
        "markdown-links",
        ("python3", "scripts/check_markdown_links.py"),
        180,
    ),
    GateSpec(
        "feature-support",
        ("python3", "scripts/check_feature_support.py"),
        180,
    ),
    GateSpec(
        "python-tests",
        (
            "python3",
            "-m",
            "unittest",
            "discover",
            "-s",
            "scripts",
            "-p",
            "test_*.py",
        ),
        300,
    ),
    GateSpec(
        "terminfo-apps",
        ("python3", "scripts/check_terminfo.py", "--require-apps"),
        300,
    ),
    GateSpec(
        "rust-clippy",
        (
            "cargo",
            "clippy",
            "--workspace",
            "--all-targets",
            "--locked",
            "--",
            "-D",
            "warnings",
        ),
        1200,
    ),
    GateSpec(
        "rust-tests",
        ("cargo", "test", "--workspace", "--locked"),
        1800,
    ),
    GateSpec(
        "debug-build",
        ("cargo", "build", "--workspace", "--locked"),
        1200,
    ),
    GateSpec(
        "release-build",
        ("cargo", "build", "--workspace", "--release", "--locked"),
        1800,
    ),
    GateSpec(
        "workspace-docs",
        ("cargo", "doc", "--workspace", "--no-deps", "--locked"),
        1200,
    ),
    GateSpec(
        "release-version",
        ("target/release/termeric", "--version"),
        30,
    ),
)


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat()


def run_text(
    argv: tuple[str, ...] | list[str], *, cwd: Path, timeout: int = 30
) -> str:
    result = subprocess.run(
        argv,
        cwd=cwd,
        capture_output=True,
        text=True,
        timeout=timeout,
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip()
        raise PreflightError(
            f"{' '.join(argv)} failed ({result.returncode}): {detail}"
        )
    return result.stdout.strip()


def repository_version(source: Path) -> str:
    manifest = source / "Cargo.toml"
    try:
        data = tomllib.loads(manifest.read_text(encoding="utf-8"))
        version = data["workspace"]["package"]["version"]
    except (OSError, KeyError, tomllib.TOMLDecodeError) as error:
        raise PreflightError(f"cannot read workspace version from {manifest}: {error}")
    if not isinstance(version, str) or not re.fullmatch(
        r"\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?", version
    ):
        raise PreflightError(f"workspace version is invalid: {version!r}")
    return version


def git_identity(source: Path) -> tuple[str, bool]:
    sha = run_text(("git", "rev-parse", "HEAD"), cwd=source)
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise PreflightError(f"Git returned an invalid candidate SHA: {sha!r}")
    status = run_text(
        ("git", "status", "--porcelain=v1", "--untracked-files=all"), cwd=source
    )
    return sha, not bool(status)


def sanitized_environment() -> tuple[dict[str, str], list[str]]:
    environment = dict(os.environ)
    removed: list[str] = []
    for name in sorted(PROVIDER_ENVIRONMENT):
        if name in environment:
            removed.append(name)
            environment.pop(name)
    environment["CARGO_NET_OFFLINE"] = "true"
    return environment, removed


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def bounded_summary(path: Path) -> str:
    with path.open("rb") as source:
        source.seek(0, os.SEEK_END)
        source.seek(max(0, source.tell() - SUMMARY_LIMIT), os.SEEK_SET)
        text = source.read(SUMMARY_LIMIT).decode("utf-8", errors="replace")
    return "".join(
        character
        if character in "\n\t" or 32 <= ord(character) < 127 or ord(character) > 127
        else "�"
        for character in text
    )


def run_gate(
    spec: GateSpec,
    *,
    source: Path,
    log: Path,
    environment: dict[str, str],
) -> dict[str, object]:
    started = time.monotonic_ns()
    returncode: int | None = None
    failure: str | None = None
    with log.open("wb") as output:
        try:
            result = subprocess.run(
                spec.argv,
                cwd=source,
                env=environment,
                stdout=output,
                stderr=subprocess.STDOUT,
                timeout=spec.timeout_seconds,
            )
            returncode = result.returncode
            if returncode != 0:
                failure = f"command exited with status {returncode}"
        except subprocess.TimeoutExpired:
            failure = f"command timed out after {spec.timeout_seconds} seconds"
        except OSError as error:
            failure = f"command could not start: {error}"
        if failure is not None:
            output.write(f"\n[release-preflight] {failure}\n".encode())
    duration_ms = (time.monotonic_ns() - started) // 1_000_000
    return {
        "name": spec.name,
        "argv": list(spec.argv),
        "status": "pass" if failure is None else "fail",
        "returncode": returncode,
        "duration_ms": duration_ms,
        "timeout_seconds": spec.timeout_seconds,
        "log": f"logs/{log.name}",
        "log_bytes": log.stat().st_size,
        "log_sha256": file_sha256(log),
        "summary": bounded_summary(log),
        "failure": failure,
    }


def tool_version(argv: tuple[str, ...], source: Path) -> str:
    output = run_text(argv, cwd=source)
    first_line = output.splitlines()[0] if output else ""
    if not first_line or len(first_line) > 512:
        raise PreflightError(f"invalid tool version output from {' '.join(argv)}")
    return first_line


def environment_context(source: Path) -> dict[str, str]:
    def checked_version(argv: tuple[str, ...]) -> str:
        try:
            return tool_version(argv, source)
        except (PreflightError, OSError, subprocess.SubprocessError) as error:
            return f"unavailable: {str(error)[:384]}"

    return {
        "os": platform.system(),
        "os_release": platform.release(),
        "machine": platform.machine(),
        "python": checked_version(("python3", "--version")),
        "cargo": checked_version(("cargo", "--version")),
        "rustc": checked_version(("rustc", "--version")),
        "git": checked_version(("git", "--version")),
        "terminfo": checked_version(("tic", "-V")),
    }


def relative_source_path(source: Path, path: Path) -> str:
    try:
        relative = path.resolve().relative_to(source.resolve())
    except ValueError as error:
        raise PreflightError(f"artifact is outside the repository: {path}") from error
    return relative.as_posix()


def binary_evidence(source: Path) -> dict[str, object]:
    binary = source / "target/release/termeric"
    if binary.is_symlink() or not binary.is_file():
        raise PreflightError(f"release binary is missing or not a regular file: {binary}")
    version = run_text((str(binary), "--version"), cwd=source)
    expected = f"termeric {repository_version(source)}"
    if version != expected:
        raise PreflightError(
            f"release binary version is {version!r}, expected {expected!r}"
        )
    return {
        "path": relative_source_path(source, binary),
        "bytes": binary.stat().st_size,
        "sha256": file_sha256(binary),
        "version_output": version,
    }


def ensure_safe_output(source: Path, output: Path) -> None:
    if output.exists() or output.is_symlink():
        raise PreflightError(f"refusing to overwrite existing evidence path: {output}")
    source = source.resolve()
    output = output.resolve()
    try:
        relative = output.relative_to(source)
    except ValueError:
        return
    result = subprocess.run(
        ["git", "check-ignore", "--quiet", "--no-index", relative.as_posix()],
        cwd=source,
        capture_output=True,
        timeout=30,
    )
    if result.returncode != 0:
        raise PreflightError(
            "evidence inside the repository must use a Git-ignored path"
        )


def write_json_atomic(path: Path, payload: dict[str, object]) -> None:
    temporary = path.with_name(f".{path.name}.tmp")
    temporary.write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    os.replace(temporary, path)


def run_preflight(source: Path, output: Path, *, allow_dirty: bool) -> int:
    source = source.resolve()
    output = output.resolve()
    ensure_safe_output(source, output)
    sha, clean = git_identity(source)
    if not clean and not allow_dirty:
        raise PreflightError(
            "candidate worktree is dirty; use --allow-dirty only for an ineligible dry run"
        )
    output.parent.mkdir(parents=True, exist_ok=True)
    temporary = Path(
        tempfile.mkdtemp(prefix=f".{output.name}.tmp-", dir=output.parent)
    )
    try:
        logs = temporary / "logs"
        logs.mkdir()
        environment, removed_environment = sanitized_environment()
        started_at = utc_now()
        gates = [
            run_gate(
                spec,
                source=source,
                log=logs / f"{index:02d}-{spec.name}.log",
                environment=environment,
            )
            for index, spec in enumerate(GATES, start=1)
        ]
        binary: dict[str, object] | None = None
        internal_failures: list[str] = []
        try:
            binary = binary_evidence(source)
        except PreflightError as error:
            internal_failures.append(str(error))
        try:
            final_sha, final_clean = git_identity(source)
            if final_sha != sha or final_clean != clean:
                internal_failures.append(
                    "candidate Git identity or clean-tree state changed during preflight"
                )
        except PreflightError as error:
            internal_failures.append(f"final Git identity unavailable: {error}")
        gates_pass = all(gate["status"] == "pass" for gate in gates)
        eligible = clean and gates_pass and binary is not None and not internal_failures
        payload: dict[str, object] = {
            "schema": SCHEMA_VERSION,
            "kind": RECORD_KIND,
            "status": (
                "pass"
                if gates_pass and binary is not None and not internal_failures
                else "fail"
            ),
            "candidate_eligible": eligible,
            "eligibility": "candidate" if clean else "dirty-dry-run",
            "started_at_utc": started_at,
            "finished_at_utc": utc_now(),
            "candidate": {
                "sha": sha,
                "version": repository_version(source),
                "clean": clean,
            },
            "environment": environment_context(source),
            "policy": {
                "provider_backed_workflows": "prohibited",
                "cargo_network": "offline",
                "removed_environment": removed_environment,
            },
            "gates": gates,
            "binary": binary,
        }
        if internal_failures:
            payload["failure"] = "; ".join(internal_failures)
        write_json_atomic(temporary / "record.json", payload)
        os.rename(temporary, output)
        print(f"Wrote release preflight: {output / 'record.json'}")
        return 0 if gates_pass and binary is not None and not internal_failures else 1
    except BaseException:
        shutil.rmtree(temporary, ignore_errors=True)
        raise


def safe_relative_file(root: Path, raw_path: object) -> Path:
    if not isinstance(raw_path, str):
        raise PreflightError("artifact path must be a string")
    pure = PurePosixPath(raw_path)
    if pure.is_absolute() or any(part in {"", ".", ".."} for part in pure.parts):
        raise PreflightError(f"unsafe artifact path: {raw_path!r}")
    path = root.joinpath(*pure.parts)
    root_resolved = root.resolve()
    try:
        resolved = path.resolve(strict=True)
        resolved.relative_to(root_resolved)
    except (OSError, ValueError) as error:
        raise PreflightError(f"artifact path escapes or is missing: {raw_path!r}") from error
    current = root
    for part in pure.parts:
        current = current / part
        if current.is_symlink():
            raise PreflightError(f"artifact path contains a symlink: {raw_path!r}")
    if not resolved.is_file():
        raise PreflightError(f"artifact is not a regular file: {raw_path!r}")
    return resolved


def require_exact_keys(
    value: object, expected: set[str], context: str
) -> dict[str, object]:
    if not isinstance(value, dict):
        raise PreflightError(f"{context} must be an object")
    actual = set(value)
    if actual != expected:
        raise PreflightError(
            f"{context} fields differ: missing={sorted(expected - actual)}, "
            f"extra={sorted(actual - expected)}"
        )
    return value


def validate_evidence_inventory(record: Path) -> None:
    if record.name != "record.json":
        raise PreflightError("release preflight record must be named record.json")
    root_entries = {entry.name: entry for entry in record.parent.iterdir()}
    if set(root_entries) != {"record.json", "logs"}:
        raise PreflightError("release preflight evidence inventory is not exact")
    log_directory = root_entries["logs"]
    if log_directory.is_symlink() or not log_directory.is_dir():
        raise PreflightError("release preflight logs path is not a regular directory")
    expected_logs = {
        f"{index:02d}-{spec.name}.log"
        for index, spec in enumerate(GATES, start=1)
    }
    actual_logs = {entry.name for entry in log_directory.iterdir()}
    if actual_logs != expected_logs:
        raise PreflightError("release preflight log inventory is not exact")


def validate_record(
    record: Path, source: Path, *, allow_ineligible: bool = False
) -> dict[str, object]:
    if record.is_symlink() or not record.is_file():
        raise PreflightError(f"record is missing or is not a regular file: {record}")
    validate_evidence_inventory(record)
    try:
        payload = json.loads(record.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise PreflightError(f"cannot parse release preflight record: {error}")
    top_keys = {
        "schema",
        "kind",
        "status",
        "candidate_eligible",
        "eligibility",
        "started_at_utc",
        "finished_at_utc",
        "candidate",
        "environment",
        "policy",
        "gates",
        "binary",
    }
    if isinstance(payload, dict) and "failure" in payload:
        top_keys.add("failure")
    root = require_exact_keys(payload, top_keys, "record")
    if root["schema"] != SCHEMA_VERSION or root["kind"] != RECORD_KIND:
        raise PreflightError("record schema or kind is unsupported")
    candidate = require_exact_keys(
        root["candidate"], {"sha", "version", "clean"}, "candidate"
    )
    policy = require_exact_keys(
        root["policy"],
        {"provider_backed_workflows", "cargo_network", "removed_environment"},
        "policy",
    )
    if policy["provider_backed_workflows"] != "prohibited":
        raise PreflightError("record does not prohibit provider-backed workflows")
    if policy["cargo_network"] != "offline":
        raise PreflightError("record does not establish offline Cargo execution")
    removed = policy["removed_environment"]
    if not isinstance(removed, list) or removed != sorted(set(removed)) or any(
        not isinstance(name, str) or name not in PROVIDER_ENVIRONMENT for name in removed
    ):
        raise PreflightError("record has invalid removed provider environment names")
    environment = require_exact_keys(
        root["environment"],
        {"os", "os_release", "machine", "python", "cargo", "rustc", "git", "terminfo"},
        "environment",
    )
    if any(
        not isinstance(value, str) or not value or len(value) > 512
        for value in environment.values()
    ):
        raise PreflightError("record has invalid environment context")
    parsed_timestamps: list[datetime] = []
    for timestamp_name in ["started_at_utc", "finished_at_utc"]:
        timestamp = root[timestamp_name]
        if not isinstance(timestamp, str):
            raise PreflightError(f"{timestamp_name} must be a string")
        try:
            parsed = datetime.fromisoformat(timestamp)
        except ValueError as error:
            raise PreflightError(f"{timestamp_name} is invalid") from error
        if parsed.tzinfo is None:
            raise PreflightError(f"{timestamp_name} must include a timezone")
        parsed_timestamps.append(parsed)
    if parsed_timestamps[1] < parsed_timestamps[0]:
        raise PreflightError("record finishes before it starts")
    gates = root["gates"]
    if not isinstance(gates, list) or len(gates) != len(GATES):
        raise PreflightError("record gate count differs from the required fixed set")
    gate_keys = {
        "name",
        "argv",
        "status",
        "returncode",
        "duration_ms",
        "timeout_seconds",
        "log",
        "log_bytes",
        "log_sha256",
        "summary",
        "failure",
    }
    seen_logs: set[str] = set()
    for index, (gate, spec) in enumerate(zip(gates, GATES, strict=True), start=1):
        checked = require_exact_keys(gate, gate_keys, f"gate {spec.name}")
        if checked["name"] != spec.name or checked["argv"] != list(spec.argv):
            raise PreflightError(f"gate definition drifted at {spec.name}")
        if checked["timeout_seconds"] != spec.timeout_seconds:
            raise PreflightError(f"gate timeout drifted at {spec.name}")
        if (
            checked["status"] != "pass"
            or type(checked["returncode"]) is not int
            or checked["returncode"] != 0
        ):
            raise PreflightError(f"gate did not pass: {spec.name}")
        if checked["failure"] is not None:
            raise PreflightError(f"passing gate retains a failure: {spec.name}")
        if type(checked["duration_ms"]) is not int or checked["duration_ms"] < 0:
            raise PreflightError(f"gate duration is invalid: {spec.name}")
        if (
            not isinstance(checked["summary"], str)
            or len(checked["summary"]) > SUMMARY_LIMIT
        ):
            raise PreflightError(f"gate summary is invalid: {spec.name}")
        if not isinstance(checked["log"], str) or checked["log"] in seen_logs:
            raise PreflightError(f"gate log path is invalid or duplicated: {spec.name}")
        expected_log = f"logs/{index:02d}-{spec.name}.log"
        if checked["log"] != expected_log:
            raise PreflightError(f"gate log path drifted at {spec.name}")
        seen_logs.add(checked["log"])
        log = safe_relative_file(record.parent, checked["log"])
        if (
            type(checked["log_bytes"]) is not int
            or checked["log_bytes"] != log.stat().st_size
        ):
            raise PreflightError(f"gate log size changed: {spec.name}")
        if (
            not isinstance(checked["log_sha256"], str)
            or not re.fullmatch(r"[0-9a-f]{64}", checked["log_sha256"])
            or checked["log_sha256"] != file_sha256(log)
        ):
            raise PreflightError(f"gate log digest changed: {spec.name}")
        if checked["summary"] != bounded_summary(log):
            raise PreflightError(f"gate log summary changed: {spec.name}")
    source = source.resolve()
    current_sha, current_clean = git_identity(source)
    if (
        not isinstance(candidate["sha"], str)
        or not re.fullmatch(r"[0-9a-f]{40}", candidate["sha"])
        or candidate["sha"] != current_sha
    ):
        raise PreflightError("record candidate SHA does not match the checkout")
    if candidate["version"] != repository_version(source):
        raise PreflightError("record candidate version does not match the checkout")
    if (
        type(candidate["clean"]) is not bool
        or candidate["clean"] is not current_clean
    ):
        raise PreflightError("record clean-tree state does not match the checkout")
    if root["status"] != "pass":
        raise PreflightError("record status is not pass")
    if "failure" in root:
        raise PreflightError("passing record retains an internal failure")
    expected_eligibility = "candidate" if candidate["clean"] else "dirty-dry-run"
    expected_candidate_eligible = candidate["clean"]
    if (
        root["eligibility"] != expected_eligibility
        or root["candidate_eligible"] is not expected_candidate_eligible
    ):
        raise PreflightError("record eligibility is inconsistent with candidate state")
    eligible = (
        root["status"] == "pass"
        and root["candidate_eligible"] is True
        and root["eligibility"] == "candidate"
        and candidate["clean"] is True
    )
    if not eligible and not allow_ineligible:
        raise PreflightError("record is not eligible candidate evidence")
    binary = require_exact_keys(
        root["binary"], {"path", "bytes", "sha256", "version_output"}, "binary"
    )
    binary_path = safe_relative_file(source, binary["path"])
    if (
        type(binary["bytes"]) is not int
        or binary["bytes"] != binary_path.stat().st_size
    ):
        raise PreflightError("release binary size changed")
    if (
        not isinstance(binary["sha256"], str)
        or not re.fullmatch(r"[0-9a-f]{64}", binary["sha256"])
        or binary["sha256"] != file_sha256(binary_path)
    ):
        raise PreflightError("release binary digest changed")
    expected_version = f"termeric {repository_version(source)}"
    if binary["version_output"] != expected_version:
        raise PreflightError("release binary version evidence is invalid")
    return root


def default_output(source: Path) -> Path:
    return source / "target/release-preflight"


def main(argv: list[str] | None = None) -> int:
    repository = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    run_parser = subparsers.add_parser("run", help="run the fixed hermetic gate set")
    run_parser.add_argument("--source", type=Path, default=repository)
    run_parser.add_argument("--output", type=Path)
    run_parser.add_argument(
        "--allow-dirty",
        action="store_true",
        help="produce a permanently ineligible development dry run",
    )
    validate_parser = subparsers.add_parser(
        "validate", help="validate an existing record and its artifacts"
    )
    validate_parser.add_argument("record", type=Path)
    validate_parser.add_argument("--source", type=Path, default=repository)
    validate_parser.add_argument("--allow-ineligible", action="store_true")
    arguments = parser.parse_args(argv)
    try:
        source = arguments.source.resolve()
        if arguments.command == "run":
            output = (
                arguments.output.resolve()
                if arguments.output is not None
                else default_output(source)
            )
            return run_preflight(source, output, allow_dirty=arguments.allow_dirty)
        validate_record(
            arguments.record.resolve(),
            source,
            allow_ineligible=arguments.allow_ineligible,
        )
        print(f"Validated release preflight: {arguments.record}")
        return 0
    except (PreflightError, OSError, subprocess.SubprocessError) as error:
        print(f"Release preflight failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
