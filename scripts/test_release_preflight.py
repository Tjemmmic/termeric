import copy
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock

from scripts.release_preflight import (
    GATES,
    PROVIDER_ENVIRONMENT,
    RECORD_KIND,
    SCHEMA_VERSION,
    GateSpec,
    PreflightError,
    binary_evidence,
    ensure_safe_output,
    file_sha256,
    repository_version,
    run_gate,
    run_preflight,
    safe_relative_file,
    sanitized_environment,
    validate_record,
)


SHA = "a" * 40
ENVIRONMENT = {
    "os": "Linux",
    "os_release": "fixture",
    "machine": "x86_64",
    "python": "Python 3.14.0",
    "cargo": "cargo 1.90.0",
    "rustc": "rustc 1.90.0",
    "git": "git version 2.50.0",
    "terminfo": "ncurses 6.5.20250419",
}


def write_workspace(source: Path) -> Path:
    source.mkdir()
    (source / "Cargo.toml").write_text(
        '[workspace]\nmembers = []\n[workspace.package]\nversion = "1.2.3"\n'
    )
    binary = source / "target/release/termeric"
    binary.parent.mkdir(parents=True)
    binary.write_bytes(b"ELF release fixture")
    return binary


def passing_evidence(temporary: Path) -> tuple[Path, Path, dict[str, object]]:
    source = temporary / "source"
    binary = write_workspace(source)
    evidence = temporary / "evidence"
    logs = evidence / "logs"
    logs.mkdir(parents=True)
    gates: list[dict[str, object]] = []
    for index, spec in enumerate(GATES, start=1):
        log = logs / f"{index:02d}-{spec.name}.log"
        log.write_text(f"{spec.name} passed\n")
        gates.append(
            {
                "name": spec.name,
                "argv": list(spec.argv),
                "status": "pass",
                "returncode": 0,
                "duration_ms": 1,
                "timeout_seconds": spec.timeout_seconds,
                "log": f"logs/{log.name}",
                "log_bytes": log.stat().st_size,
                "log_sha256": file_sha256(log),
                "summary": log.read_text(),
                "failure": None,
            }
        )
    payload: dict[str, object] = {
        "schema": SCHEMA_VERSION,
        "kind": RECORD_KIND,
        "status": "pass",
        "candidate_eligible": True,
        "eligibility": "candidate",
        "started_at_utc": datetime.now(timezone.utc).isoformat(),
        "finished_at_utc": datetime.now(timezone.utc).isoformat(),
        "candidate": {"sha": SHA, "version": "1.2.3", "clean": True},
        "environment": dict(ENVIRONMENT),
        "policy": {
            "provider_backed_workflows": "prohibited",
            "cargo_network": "offline",
            "removed_environment": [],
        },
        "gates": gates,
        "binary": {
            "path": "target/release/termeric",
            "bytes": binary.stat().st_size,
            "sha256": file_sha256(binary),
            "version_output": "termeric 1.2.3",
        },
    }
    record = evidence / "record.json"
    record.write_text(json.dumps(payload))
    return source, record, payload


def write_payload(record: Path, payload: dict[str, object]) -> None:
    record.write_text(json.dumps(payload))


class ReleasePreflightTests(unittest.TestCase):
    def validate_fixture(
        self,
        source: Path,
        record: Path,
        *,
        clean: bool = True,
        allow_ineligible: bool = False,
    ) -> dict[str, object]:
        with mock.patch(
            "scripts.release_preflight.git_identity", return_value=(SHA, clean)
        ):
            return validate_record(
                record, source, allow_ineligible=allow_ineligible
            )

    def test_complete_candidate_record_validates(self):
        with tempfile.TemporaryDirectory() as directory:
            source, record, _payload = passing_evidence(Path(directory))
            validated = self.validate_fixture(source, record)
            self.assertTrue(validated["candidate_eligible"])
            self.assertEqual(
                [gate["name"] for gate in validated["gates"]],
                [spec.name for spec in GATES],
            )

    def test_evidence_inventory_rejects_extra_missing_and_renamed_artifacts(self):
        mutations = {
            "extra": lambda record: (record.parent / "stale.log").write_text("stale"),
            "missing": lambda record: (
                record.parent / "logs/01-rust-format.log"
            ).unlink(),
            "renamed": lambda record: record.rename(record.with_name("other.json")),
        }
        for name, mutate in mutations.items():
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                source, record, _payload = passing_evidence(Path(directory))
                result = mutate(record)
                checked_record = result if isinstance(result, Path) else record
                with self.assertRaisesRegex(PreflightError, "inventory|named record"):
                    self.validate_fixture(source, checked_record)

    def test_gate_set_rejects_missing_extra_reordered_and_failed_entries(self):
        mutations = {
            "count": lambda gates: gates.pop(),
            "extra": lambda gates: gates.append(copy.deepcopy(gates[-1])),
            "definition": lambda gates: gates[0].update(name="renamed"),
            "order": lambda gates: gates.reverse(),
            "failure": lambda gates: gates[0].update(
                status="fail", returncode=1, failure="failed"
            ),
        }
        for name, mutate in mutations.items():
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                source, record, payload = passing_evidence(Path(directory))
                mutate(payload["gates"])
                write_payload(record, payload)
                with self.assertRaises(PreflightError):
                    self.validate_fixture(source, record)

    def test_log_paths_reject_escape_symlink_duplicate_and_tampering(self):
        for raw_path in ["/tmp/absolute.log", "logs/../escape.log"]:
            with self.subTest(path=raw_path), tempfile.TemporaryDirectory() as directory:
                source, record, payload = passing_evidence(Path(directory))
                payload["gates"][0]["log"] = raw_path
                write_payload(record, payload)
                with self.assertRaisesRegex(PreflightError, "artifact path|gate log"):
                    self.validate_fixture(source, record)

        with tempfile.TemporaryDirectory() as directory:
            temporary = Path(directory)
            source, record, payload = passing_evidence(temporary)
            first = record.parent / payload["gates"][0]["log"]
            first.write_text("tampered\n")
            with self.assertRaisesRegex(PreflightError, "size changed|digest changed"):
                self.validate_fixture(source, record)

        with tempfile.TemporaryDirectory() as directory:
            temporary = Path(directory)
            source, record, payload = passing_evidence(temporary)
            payload["gates"][1]["log"] = payload["gates"][0]["log"]
            write_payload(record, payload)
            with self.assertRaisesRegex(PreflightError, "duplicated"):
                self.validate_fixture(source, record)

        with tempfile.TemporaryDirectory() as directory:
            temporary = Path(directory)
            source, record, payload = passing_evidence(temporary)
            first = record.parent / payload["gates"][0]["log"]
            target = temporary / "outside.log"
            target.write_bytes(first.read_bytes())
            first.unlink()
            first.symlink_to(target)
            with self.assertRaisesRegex(PreflightError, "escapes|symlink"):
                self.validate_fixture(source, record)

    def test_candidate_and_binary_identity_fail_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            temporary = Path(directory)
            source, record, payload = passing_evidence(temporary)
            payload["candidate"]["sha"] = "b" * 40
            write_payload(record, payload)
            with self.assertRaisesRegex(PreflightError, "SHA"):
                self.validate_fixture(source, record)

        with tempfile.TemporaryDirectory() as directory:
            temporary = Path(directory)
            source, record, _payload = passing_evidence(temporary)
            (source / "target/release/termeric").write_bytes(b"changed")
            with self.assertRaisesRegex(PreflightError, "size changed|digest changed"):
                self.validate_fixture(source, record)

    def test_dirty_dry_run_is_permanently_ineligible_by_default(self):
        with tempfile.TemporaryDirectory() as directory:
            source, record, payload = passing_evidence(Path(directory))
            payload["candidate_eligible"] = False
            payload["eligibility"] = "dirty-dry-run"
            payload["candidate"]["clean"] = False
            write_payload(record, payload)
            with self.assertRaisesRegex(PreflightError, "not eligible"):
                self.validate_fixture(source, record, clean=False)
            validated = self.validate_fixture(
                source, record, clean=False, allow_ineligible=True
            )
            self.assertFalse(validated["candidate_eligible"])

    def test_provider_policy_and_environment_are_exact(self):
        direct_argv = "\n".join(" ".join(spec.argv).casefold() for spec in GATES)
        self.assertNotIn("codex", direct_argv)
        self.assertNotIn("claude", direct_argv)
        self.assertNotIn("provider", direct_argv)
        with mock.patch.dict(
            os.environ,
            {
                "OPENAI_API_KEY": "secret",
                "ANTHROPIC_AUTH_TOKEN": "secret",
                "TERMERIC_REAL_CODEX_CONTRACT": "1",
                "SAFE_FIXTURE": "kept",
            },
            clear=True,
        ):
            environment, removed = sanitized_environment()
        self.assertEqual(
            removed,
            ["ANTHROPIC_AUTH_TOKEN", "OPENAI_API_KEY", "TERMERIC_REAL_CODEX_CONTRACT"],
        )
        self.assertTrue(PROVIDER_ENVIRONMENT.isdisjoint(environment))
        self.assertEqual(environment["SAFE_FIXTURE"], "kept")
        self.assertEqual(environment["CARGO_NET_OFFLINE"], "true")

        with tempfile.TemporaryDirectory() as directory:
            source, record, payload = passing_evidence(Path(directory))
            payload["policy"]["provider_backed_workflows"] = "allowed"
            write_payload(record, payload)
            with self.assertRaisesRegex(PreflightError, "does not prohibit"):
                self.validate_fixture(source, record)

    def test_run_gate_records_success_nonzero_timeout_and_spawn_failure(self):
        spec = GateSpec("fixture", ("fixture-command",), 7)
        outcomes = [
            (subprocess.CompletedProcess(spec.argv, 0), "pass", 0, None),
            (subprocess.CompletedProcess(spec.argv, 3), "fail", 3, "status 3"),
            (subprocess.CompletedProcess(spec.argv, -9), "fail", -9, "status -9"),
            (subprocess.TimeoutExpired(spec.argv, 7), "fail", None, "timed out"),
            (FileNotFoundError("missing"), "fail", None, "could not start"),
        ]
        for outcome, status, returncode, failure in outcomes:
            with self.subTest(
                status=status, failure=failure
            ), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                log = root / "gate.log"
                patch_arguments = (
                    {"side_effect": outcome}
                    if isinstance(outcome, BaseException)
                    else {"return_value": outcome}
                )
                with mock.patch(
                    "scripts.release_preflight.subprocess.run", **patch_arguments
                ):
                    result = run_gate(
                        spec, source=root, log=log, environment={}
                    )
                self.assertEqual(result["status"], status)
                self.assertEqual(result["returncode"], returncode)
                self.assertEqual(result["failure"] is None, failure is None)
                if failure is not None:
                    self.assertIn(failure, result["failure"])
                    self.assertIn("[release-preflight]", log.read_text())

    def test_output_refusal_and_atomic_dirty_policy(self):
        with tempfile.TemporaryDirectory() as directory:
            temporary = Path(directory)
            source = temporary / "source"
            write_workspace(source)
            output = temporary / "evidence"
            output.mkdir()
            with self.assertRaisesRegex(PreflightError, "overwrite"):
                ensure_safe_output(source, output)

            output.rmdir()
            with mock.patch(
                "scripts.release_preflight.git_identity", return_value=(SHA, False)
            ):
                with self.assertRaisesRegex(PreflightError, "dirty"):
                    run_preflight(source, output, allow_dirty=False)
            self.assertFalse(output.exists())

    def test_run_preflight_publishes_complete_directory_only_at_end(self):
        with tempfile.TemporaryDirectory() as directory:
            temporary = Path(directory)
            source = temporary / "source"
            binary = write_workspace(source)
            output = temporary / "evidence"
            spec = GateSpec("fixture", ("fixture",), 1)

            def fake_gate(
                _spec: GateSpec,
                *,
                source: Path,
                log: Path,
                environment: dict[str, str],
            ) -> dict[str, object]:
                self.assertFalse(output.exists())
                self.assertEqual(source, source.resolve())
                self.assertEqual(environment["CARGO_NET_OFFLINE"], "true")
                log.write_text("pass\n")
                return {
                    "name": spec.name,
                    "argv": list(spec.argv),
                    "status": "pass",
                    "returncode": 0,
                    "duration_ms": 1,
                    "timeout_seconds": spec.timeout_seconds,
                    "log": f"logs/{log.name}",
                    "log_bytes": log.stat().st_size,
                    "log_sha256": file_sha256(log),
                    "summary": "pass\n",
                    "failure": None,
                }

            with mock.patch("scripts.release_preflight.GATES", (spec,)), mock.patch(
                "scripts.release_preflight.git_identity", return_value=(SHA, True)
            ), mock.patch(
                "scripts.release_preflight.run_gate", side_effect=fake_gate
            ), mock.patch(
                "scripts.release_preflight.binary_evidence",
                return_value={
                    "path": "target/release/termeric",
                    "bytes": binary.stat().st_size,
                    "sha256": file_sha256(binary),
                    "version_output": "termeric 1.2.3",
                },
            ), mock.patch(
                "scripts.release_preflight.environment_context",
                return_value=dict(ENVIRONMENT),
            ), mock.patch("builtins.print"):
                self.assertEqual(run_preflight(source, output, allow_dirty=False), 0)
            self.assertTrue((output / "record.json").is_file())
            self.assertTrue((output / "logs/01-fixture.log").is_file())

    def test_source_drift_during_run_publishes_an_ineligible_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            temporary = Path(directory)
            source = temporary / "source"
            binary = write_workspace(source)
            output = temporary / "evidence"
            spec = GateSpec("fixture", ("fixture",), 1)
            gate = {
                "name": spec.name,
                "argv": list(spec.argv),
                "status": "pass",
                "returncode": 0,
                "duration_ms": 1,
                "timeout_seconds": 1,
                "log": "logs/01-fixture.log",
                "log_bytes": 0,
                "log_sha256": hashlib.sha256(b"").hexdigest(),
                "summary": "",
                "failure": None,
            }

            def fake_gate(*_args, log: Path, **_kwargs):
                log.write_bytes(b"")
                return gate

            with mock.patch("scripts.release_preflight.GATES", (spec,)), mock.patch(
                "scripts.release_preflight.git_identity",
                side_effect=[(SHA, True), (SHA, False)],
            ), mock.patch(
                "scripts.release_preflight.run_gate", side_effect=fake_gate
            ), mock.patch(
                "scripts.release_preflight.binary_evidence",
                return_value={
                    "path": "target/release/termeric",
                    "bytes": binary.stat().st_size,
                    "sha256": file_sha256(binary),
                    "version_output": "termeric 1.2.3",
                },
            ), mock.patch(
                "scripts.release_preflight.environment_context",
                return_value=dict(ENVIRONMENT),
            ), mock.patch("builtins.print"):
                self.assertEqual(run_preflight(source, output, allow_dirty=False), 1)
            payload = json.loads((output / "record.json").read_text())
            self.assertEqual(payload["status"], "fail")
            self.assertFalse(payload["candidate_eligible"])
            self.assertIn("changed during preflight", payload["failure"])

    def test_repository_version_and_safe_path_validation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source"
            write_workspace(source)
            self.assertEqual(repository_version(source), "1.2.3")
            safe = root / "evidence/logs/gate.log"
            safe.parent.mkdir(parents=True)
            safe.write_text("log")
            self.assertEqual(
                safe_relative_file(root / "evidence", "logs/gate.log"), safe
            )
            with self.assertRaisesRegex(PreflightError, "unsafe"):
                safe_relative_file(root / "evidence", "../gate.log")

            (source / "Cargo.toml").write_text(
                '[workspace]\n[workspace.package]\nversion = "not-semver"\n'
            )
            with self.assertRaisesRegex(PreflightError, "version is invalid"):
                repository_version(source)

    def test_binary_probe_rejects_missing_and_mismatched_versions(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "source"
            binary = write_workspace(source)
            binary.unlink()
            with self.assertRaisesRegex(PreflightError, "release binary is missing"):
                binary_evidence(source)

        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "source"
            write_workspace(source)
            with mock.patch(
                "scripts.release_preflight.run_text", return_value="termeric 9.9.9"
            ):
                with self.assertRaisesRegex(PreflightError, "expected 'termeric 1.2.3'"):
                    binary_evidence(source)

    def test_record_rejects_invalid_timestamp_environment_and_summary(self):
        mutations = {
            "timestamp": lambda payload: payload.update(started_at_utc="not-a-time"),
            "environment": lambda payload: payload["environment"].pop("rustc"),
            "summary": lambda payload: payload["gates"][0].update(summary="changed"),
        }
        for name, mutate in mutations.items():
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                source, record, payload = passing_evidence(Path(directory))
                mutate(payload)
                write_payload(record, payload)
                with self.assertRaises(PreflightError):
                    self.validate_fixture(source, record)


if __name__ == "__main__":
    unittest.main()
