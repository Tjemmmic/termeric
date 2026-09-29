#!/usr/bin/env python3
"""Run the agent workflow test against fake Claude Code and Codex executables."""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))


FAKE_PROVIDER_TEST = (
    "app::tests::fake_agent_clis_exercise_every_agent_command_end_to_end"
)
PROVIDER_OPT_IN_ENVIRONMENT = ("TERMERIC_REAL_CODEX_CONTRACT",)


class AcceptanceError(RuntimeError):
    """The fake-agent test could not be built or did not pass."""


def safe_test_environment(source: dict[str, str]) -> dict[str, str]:
    environment = dict(source)
    for variable in PROVIDER_OPT_IN_ENVIRONMENT:
        environment.pop(variable, None)
    return environment


def test_binary_from_cargo_messages(output: str) -> Path:
    executables: set[Path] = set()
    for line in output.splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not isinstance(message, dict) or message.get("reason") != "compiler-artifact":
            continue
        target = message.get("target")
        profile = message.get("profile")
        executable = message.get("executable")
        kind = target.get("kind") if isinstance(target, dict) else None
        if (
            isinstance(target, dict)
            and target.get("name") == "termeric"
            and isinstance(kind, list)
            and "bin" in kind
            and isinstance(profile, dict)
            and profile.get("test") is True
            and isinstance(executable, str)
        ):
            reported = Path(executable)
            executables.add(
                (reported if reported.is_absolute() else ROOT / reported).resolve()
            )
    if len(executables) != 1:
        raise AcceptanceError(
            "expected one Termeric test executable from Cargo, found "
            f"{len(executables)}"
        )
    binary = executables.pop()
    if binary.is_symlink() or not binary.is_file():
        raise AcceptanceError(f"Cargo reported a missing test executable: {binary}")
    return binary


def run_fake_provider_test(
    output: Path,
    *,
    environment: dict[str, str] | None = None,
) -> dict[str, object]:
    """Build and run the test, writing results.json and a log to `output`."""
    output.mkdir(parents=True, exist_ok=True)
    log_path = output / "fake-provider-test.log"
    results: dict[str, object] = {
        "status": "fail",
        "recorded_at_utc": datetime.now(timezone.utc).isoformat(),
        "test": FAKE_PROVIDER_TEST,
        "removed_provider_opt_ins": list(PROVIDER_OPT_IN_ENVIRONMENT),
        "log": log_path.name,
    }
    try:
        test_environment = safe_test_environment(
            dict(os.environ) if environment is None else environment
        )
        build_command = [
            "cargo", "test", "-p", "termeric", "--locked", "--no-run", "--message-format=json",
        ]
        build = subprocess.run(
            build_command, cwd=ROOT, env=test_environment, text=True,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=900,
        )
        log_sections = [f"build_command={json.dumps(build_command)}", "build_stderr:", build.stderr]
        if build.returncode != 0:
            log_path.write_text("\n".join(log_sections), encoding="utf-8")
            raise AcceptanceError(
                f"failed to build the integration test ({build.returncode}); see {log_path}"
            )

        test_binary = test_binary_from_cargo_messages(build.stdout)
        test_command = [str(test_binary), FAKE_PROVIDER_TEST, "--exact", "--nocapture"]
        test = subprocess.run(
            test_command, cwd=ROOT, env=test_environment, text=True,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=300,
        )
        log_sections.extend([
            f"test_command={json.dumps(test_command)}",
            "test_stdout:", test.stdout, "test_stderr:", test.stderr,
        ])
        log_path.write_text("\n".join(log_sections), encoding="utf-8")
        if (
            test.returncode != 0
            or f"test {FAKE_PROVIDER_TEST} ... ok" not in test.stdout
            or "1 passed; 0 failed" not in test.stdout
        ):
            raise AcceptanceError(
                f"the fake-agent integration test did not pass; see {log_path}"
            )
        results["status"] = "pass"
        return results
    except Exception as error:
        results["error"] = str(error)
        raise
    finally:
        (output / "results.json").write_text(json.dumps(results, indent=2) + "\n", encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True,
                        help="directory for results.json and the test log")
    args = parser.parse_args()
    try:
        run_fake_provider_test(args.output)
    except (OSError, AcceptanceError, subprocess.SubprocessError) as error:
        print(f"Fake-agent acceptance failed: {error}", file=sys.stderr)
        return 1
    print(f"PASS: {FAKE_PROVIDER_TEST}")
    print(f"Results: {args.output / 'results.json'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
