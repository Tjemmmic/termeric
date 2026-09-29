#!/usr/bin/env python3
"""Run the real-shell integration test for Bash, Zsh, and Fish (optionally Nushell and Elvish)."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import selectors
import signal
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from scripts.run_fake_provider_acceptance import test_binary_from_cargo_messages

TEST = "terminal::shell_integration::tests::supported_shells_preserve_user_rc_and_emit_multiline_exit_regions"
PREFIX = "TERMERIC_SHELL_ACCEPTANCE="
SHELLS = {"bash": "Bash", "zsh": "Zsh", "fish": "Fish"}
EXTENDED_SHELLS = {**SHELLS, "nushell": "Nushell", "elvish": "Elvish"}
EXTENDED_TESTS = (
    TEST,
    "terminal::shell_integration::tests::nushell_native_integration_preserves_user_config_and_emits_regions_when_available",
    "terminal::shell_integration::tests::elvish_module_preserves_user_config_and_emits_regions_when_available",
    "terminal::shell_integration::nushell_launch_tests::nushell_non_repl_and_unknown_launches_are_preserved_without_side_effects",
    "terminal::shell_integration::elvish_launch_tests::elvish_non_repl_and_invalid_launches_have_no_integration_side_effects",
    "terminal::shell_integration::elvish_environment_tests::elvish_module_restores_only_unchanged_transport_and_cleans_child_environment",
)
MAX_LOG_BYTES = 8 * 1024 * 1024
REMOVED_ENVIRONMENT = (
    "TERMERIC_REAL_CODEX_CONTRACT", "OPENAI_API_KEY", "CODEX_API_KEY",
    "ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN",
    "BASH_ENV", "ENV", "ZDOTDIR", "XDG_CONFIG_HOME", "XDG_DATA_HOME",
)


class ShellAcceptanceError(RuntimeError):
    """The shell integration test failed or reported unexpected output."""


def safe_environment(source: dict[str, str], extended: bool = False) -> dict[str, str]:
    result = {key: value for key, value in source.items() if key not in REMOVED_ENVIRONMENT}
    result["TERMERIC_REQUIRE_SHELL_ACCEPTANCE"] = "1"
    if extended:
        result["TERMERIC_REQUIRE_NUSHELL_ACCEPTANCE"] = "1"
        result["TERMERIC_REQUIRE_ELVISH_ACCEPTANCE"] = "1"
    return result


def run_logged(command: list[str], path: Path, environment: dict[str, str], timeout: float) -> str:
    """Bound output and wall time, retaining diagnostics and reaping the private group."""
    with path.open("wb") as log:
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
                        raise ShellAcceptanceError(f"command timed out; see {path}")
                    for key, _ in selector.select(min(0.1, max(0, deadline - time.monotonic()))):
                        chunk = os.read(key.fd, min(65536, MAX_LOG_BYTES - len(output) + 1))
                        if not chunk:
                            selector.unregister(key.fileobj)
                            continue
                        remaining = MAX_LOG_BYTES - len(output)
                        log.write(chunk[:remaining])
                        output.extend(chunk[:remaining])
                        if len(chunk) > remaining:
                            raise ShellAcceptanceError(f"command output exceeds {MAX_LOG_BYTES} bytes; see {path}")
            try:
                code = process.wait(timeout=max(0.001, deadline - time.monotonic()))
            except subprocess.TimeoutExpired as error:
                raise ShellAcceptanceError(f"command timed out; see {path}") from error
            if code != 0:
                raise ShellAcceptanceError(f"command exited {code}; see {path}")
            return output.decode("utf-8", errors="replace")
        finally:
            # Only the private session we just created, never the caller's group.
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait()
            process.stdout.close()


def observations(output: str, extended: bool = False) -> dict[str, dict]:
    """Require exactly the selected tests to pass and every shell to report in."""
    tests = EXTENDED_TESTS if extended else (TEST,)
    shells = EXTENDED_SHELLS if extended else SHELLS
    passed = re.findall(r"^test (.+) \.\.\. ok$", output, re.MULTILINE)
    if sorted(passed) != sorted(tests):
        raise ShellAcceptanceError("exact shell tests did not pass, or unexpected tests ran")
    summaries = re.findall(r"^test result: (.+)$", output, re.MULTILINE)
    if len(summaries) != 1 or not summaries[0].startswith(
        f"ok. {len(tests)} passed; 0 failed; 0 ignored;"
    ):
        raise ShellAcceptanceError("expected exactly the selected passing, non-ignored shell tests")
    result = {}
    for line in output.splitlines():
        if not line.startswith(PREFIX):
            continue
        try:
            row = json.loads(line[len(PREFIX):])
        except json.JSONDecodeError as error:
            raise ShellAcceptanceError("invalid shell observation JSON") from error
        if not isinstance(row, dict) or set(row) != {"shell", "path", "version", "sha256", "capability_replies"}:
            raise ShellAcceptanceError("invalid shell observation fields")
        name = row["shell"]
        if not isinstance(name, str) or name not in shells or name in result:
            raise ShellAcceptanceError("unexpected or duplicate shell observation")
        if (not isinstance(row["path"], str) or not Path(row["path"]).is_absolute()
            or not isinstance(row["version"], str) or not row["version"].strip()
            or len(row["version"]) > 1024
            or type(row["capability_replies"]) is not int or not 0 <= row["capability_replies"] <= 10000):
            raise ShellAcceptanceError("invalid shell observation values")
        result[name] = {key: row[key] for key in ("path", "version", "capability_replies")}
    if set(result) != set(shells):
        raise ShellAcceptanceError("missing required shell observations")
    return result


def run(output: Path, extended: bool = False) -> dict:
    """Build and run the shell test, writing results.json and logs to `output`."""
    output.mkdir(parents=True, exist_ok=True)
    tests = list(EXTENDED_TESTS if extended else (TEST,))
    results: dict = {"status": "fail", "tests": tests, "shells": {},
                     "removed_environment": list(REMOVED_ENVIRONMENT),
                     "logs": ["shell-build.log", "shell-test.log"]}
    try:
        environment = safe_environment(dict(os.environ), extended)
        build = run_logged(
            ["cargo", "test", "-p", "termeric", "--locked", "--no-run", "--message-format=json"],
            output / "shell-build.log", environment, 900,
        )
        binary = test_binary_from_cargo_messages(build)
        command = [str(binary), *tests, "--exact", "--show-output", "--test-threads=1"]
        test_output = run_logged(command, output / "shell-test.log", environment, 60)
        results["shells"] = observations(test_output, extended)
        results["status"] = "pass"
        return results
    except Exception as error:
        results["error"] = str(error)
        raise
    finally:
        (output / "results.json").write_text(json.dumps(results, indent=2) + "\n", encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True, help="directory for results.json and logs")
    parser.add_argument("--include-extended-shells", action="store_true",
                        help="also require Nushell and Elvish")
    args = parser.parse_args()
    try:
        results = run(args.output, args.include_extended_shells)
    except (OSError, ValueError, ShellAcceptanceError, subprocess.SubprocessError) as error:
        print(f"Shell acceptance failed: {error}", file=sys.stderr)
        return 1
    for shell, row in results["shells"].items():
        print(f"PASS: {shell} {row['version']} ({row['capability_replies']} capability replies)")
    print(f"Results: {args.output / 'results.json'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
