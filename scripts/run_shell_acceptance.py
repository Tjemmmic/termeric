#!/usr/bin/env python3
"""Record the required Linux Bash/Zsh/Fish real-PTY acceptance subset."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import platform
import re
import selectors
import signal
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from scripts.integration_acceptance_record import (
    CASE_DEFINITIONS, RecordError, atomic_write_json, create_template,
    disposition_for_checks, file_sha256, validate_record,
)
from scripts.run_fake_provider_acceptance import test_binary_from_cargo_messages
from scripts.soak_provenance import PinnedExecutable, provenance_checks, source_identity

TEST = "terminal::shell_integration::tests::supported_shells_preserve_user_rc_and_emit_multiline_exit_regions"
PREFIX = "TERMERIC_SHELL_ACCEPTANCE="
SHELLS = {"bash": "Bash", "zsh": "Zsh", "fish": "Fish"}
MAX_LOG_BYTES = 8 * 1024 * 1024
REMOVED_ENVIRONMENT = (
    "TERMERIC_REAL_CODEX_CONTRACT", "OPENAI_API_KEY", "CODEX_API_KEY",
    "ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN",
    "BASH_ENV", "ENV", "ZDOTDIR", "XDG_CONFIG_HOME", "XDG_DATA_HOME",
)


def safe_environment(source: dict[str, str]) -> dict[str, str]:
    result = {key: value for key, value in source.items() if key not in REMOVED_ENVIRONMENT}
    result["TERMERIC_REQUIRE_SHELL_ACCEPTANCE"] = "1"
    return result


def run_logged(command: list[str], path: Path, environment: dict[str, str], timeout: float) -> str:
    """Bound output and wall time, retaining diagnostics and reaping the private group."""
    with path.open("xb") as log:
        process = subprocess.Popen(
            command, cwd=ROOT, env=environment, stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, start_new_session=True,
        )
        output = bytearray()
        deadline = time.monotonic() + timeout
        try:
            with selectors.DefaultSelector() as selector:
                selector.register(process.stdout, selectors.EVENT_READ)
                while selector.get_map():
                    if time.monotonic() >= deadline:
                        raise RecordError(f"command timed out; see {path}")
                    for key, _ in selector.select(min(0.1, max(0, deadline - time.monotonic()))):
                        chunk = os.read(key.fd, min(65536, MAX_LOG_BYTES - len(output) + 1))
                        if not chunk:
                            selector.unregister(key.fileobj)
                            continue
                        remaining = MAX_LOG_BYTES - len(output)
                        log.write(chunk[:remaining])
                        output.extend(chunk[:remaining])
                        if len(chunk) > remaining:
                            raise RecordError(f"command output exceeds {MAX_LOG_BYTES} bytes; see {path}")
            try:
                code = process.wait(timeout=max(0.001, deadline - time.monotonic()))
            except subprocess.TimeoutExpired as error:
                raise RecordError(f"command timed out; see {path}") from error
            if code != 0:
                raise RecordError(f"command exited {code}; see {path}")
            return output.decode("utf-8", errors="replace")
        finally:
            # Only the private session we just created, never the caller's group.
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait()
            process.stdout.close()


def observations(output: str) -> dict[str, dict]:
    if not re.search(rf"^test {re.escape(TEST)} \.\.\. ok$", output, re.MULTILINE):
        raise RecordError("exact shell test did not pass")
    if not re.search(r"^test result: ok\. 1 passed; 0 failed; 0 ignored;", output, re.MULTILINE):
        raise RecordError("expected exactly one passing, non-ignored shell test")
    result = {}
    for line in output.splitlines():
        if not line.startswith(PREFIX):
            continue
        try:
            row = json.loads(line[len(PREFIX):])
        except json.JSONDecodeError as error:
            raise RecordError("invalid shell observation JSON") from error
        if not isinstance(row, dict) or set(row) != {"shell", "path", "version", "sha256", "capability_replies"}:
            raise RecordError("invalid shell observation fields")
        name = row["shell"]
        if not isinstance(name, str) or name not in SHELLS or name in result:
            raise RecordError("unexpected or duplicate shell observation")
        if (not isinstance(row["path"], str) or not Path(row["path"]).is_absolute()
            or not isinstance(row["version"], str) or not row["version"].strip()
            or len(row["version"]) > 1024
            or not isinstance(row["sha256"], str) or re.fullmatch(r"[0-9a-f]{64}", row["sha256"]) is None
            or type(row["capability_replies"]) is not int or not 0 <= row["capability_replies"] <= 10000):
            raise RecordError("invalid shell observation values")
        result[name] = row
    if set(result) != set(SHELLS):
        raise RecordError("missing required shell observations")
    return result


def fill_shell_cases(record: dict, rows: dict[str, dict], binary_digest: str) -> None:
    for case in record["cases"]:
        name = case["id"].removeprefix("shell.")
        if case["id"] != f"shell.{name}" or name not in SHELLS:
            continue
        row = rows[name]
        case["environment"] = {
            "platform": platform.platform(), "shell": SHELLS[name],
            "shell_path": row["path"], "shell_version": row["version"],
            "test_binary_sha256": binary_digest,
        }
        case["checks"] = {check: "pass" for check in CASE_DEFINITIONS[case["id"]].checks}
        if not row["capability_replies"]:
            case["checks"]["capability_replies_return_to_origin"] = "accepted_limitation"
        case["disposition"] = disposition_for_checks(case["checks"])
        case["notes"] = (
            "Isolated real PTY, synthetic rc and commands; no provider or native GUI. "
            f"Observed {row['capability_replies']} replies written to the originating PTY. "
            "A zero count is explicitly an unexercised reply-path limitation, not a pass."
        )
        case["artifacts"] = [
            {"path": f"artifacts/{filename}", "description": description, "sha256": "0" * 64}
            for filename, description in (
                ("shell-test.log", "Exact required real-shell fixture output"),
                ("shell-result.json", "Shell identities, observed replies and pinned test provenance"),
                ("shell-build.log", "Cargo build output identifying the test executable"),
            )
        ]


def run(output: Path, operator: str) -> None:
    if sys.platform != "linux":
        raise RecordError("shell evidence runner requires Linux executable-fd provenance")
    if not operator.strip():
        raise RecordError("operator must not be empty")
    # A fresh directory prevents accidental mixing with another candidate's artifacts.
    output = output.absolute()
    if output.exists() or output.is_symlink():
        raise RecordError(f"refusing to overwrite existing output: {output}")
    initial = source_identity(ROOT)
    if initial["worktree_dirty"]:
        raise RecordError("refusing dirty candidate source")
    output.mkdir(parents=True)
    artifacts = output / "artifacts"
    artifacts.mkdir()
    environment = safe_environment(dict(os.environ))
    build_command = ["cargo", "test", "-p", "termeric", "--locked", "--no-run", "--message-format=json"]
    build = run_logged(build_command, artifacts / "shell-build.log", environment, 900)
    binary = test_binary_from_cargo_messages(build)
    with PinnedExecutable(ROOT, binary, initial) as pinned:
        command = [str(pinned.launch("scenario")), TEST, "--exact", "--show-output", "--test-threads=1"]
        try:
            result = run_logged(command, artifacts / "shell-test.log", environment, 60)
        finally:
            proof = pinned.finish()
            atomic_write_json(artifacts / "shell-provenance.json", proof)
        if not all(check["passed"] for check in provenance_checks(proof, ["scenario"])):
            raise RecordError("source or test executable changed; see shell-provenance.json")
        rows = observations(result)
        for row in rows.values():
            if file_sha256(Path(row["path"])) != row["sha256"]:
                raise RecordError("shell executable changed after its probe")
        digest = pinned.file_evidence["sha256"]
    evidence = {
        "schema": 1, "test": TEST, "candidate_sha": initial["commit"],
        "test_binary_sha256": digest, "shells": rows, "provenance": proof,
        "removed_environment": list(REMOVED_ENVIRONMENT),
        "build_command": build_command, "test_command": command,
    }
    atomic_write_json(artifacts / "shell-result.json", evidence)
    record = create_template(initial["commit"], operator)
    fill_shell_cases(record, rows, digest)
    path = output / "record.json"
    # Seal in memory first: no record is published before checks and hashes agree.
    for case in record["cases"]:
        for artifact in case["artifacts"]:
            artifact["sha256"] = file_sha256(output / artifact["path"])
    failures = validate_record(path, record, allow_incomplete=True, expected_sha=initial["commit"])
    if failures:
        raise RecordError("; ".join(failures))
    if source_identity(ROOT) != initial:
        raise RecordError("source changed before record publication")
    atomic_write_json(path, record)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True, help="new evidence directory")
    parser.add_argument("--operator", required=True)
    args = parser.parse_args()
    try:
        run(args.output, args.operator)
    except (OSError, ValueError, RecordError, subprocess.SubprocessError) as error:
        print(f"Shell acceptance failed: {error}", file=sys.stderr)
        return 1
    print(f"Recorded partial shell acceptance: {args.output / 'record.json'}")
    print("Only required Bash/Zsh/Fish checks recorded; all other integration cases remain unrecorded.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
