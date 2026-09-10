import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock

from scripts.integration_acceptance_record import RecordError, create_template
from scripts.run_fake_provider_acceptance import (
    FAKE_PROVIDER_CASES,
    FAKE_PROVIDER_TEST,
    fill_fake_provider_cases,
    run_fake_provider_test,
    safe_test_environment,
    test_binary_from_cargo_messages,
)


class FakeProviderAcceptanceTests(unittest.TestCase):
    def test_safe_environment_removes_real_provider_opt_in(self) -> None:
        environment = safe_test_environment(
            {"PATH": "/fixture", "TERMERIC_REAL_CODEX_CONTRACT": "1"}
        )
        self.assertEqual(environment, {"PATH": "/fixture"})

    def test_cargo_message_selects_only_the_termeric_test_binary(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / "termeric-test"
            binary.write_bytes(b"test")
            messages = "\n".join(
                [
                    json.dumps({"reason": "build-finished", "success": True}),
                    json.dumps(
                        {
                            "reason": "compiler-artifact",
                            "target": {"name": "termeric", "kind": ["bin"]},
                            "profile": {"test": True},
                            "executable": str(binary),
                        }
                    ),
                ]
            )
            self.assertEqual(test_binary_from_cargo_messages(messages), binary)

    def test_cargo_message_rejects_missing_or_ambiguous_test_binaries(self) -> None:
        with self.assertRaisesRegex(RecordError, "found 0"):
            test_binary_from_cargo_messages("not json")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binaries = [root / "first", root / "second"]
            for binary in binaries:
                binary.write_bytes(b"test")
            messages = "\n".join(
                json.dumps(
                    {
                        "reason": "compiler-artifact",
                        "target": {"name": "termeric", "kind": ["bin"]},
                        "profile": {"test": True},
                        "executable": str(binary),
                    }
                )
                for binary in binaries
            )
            with self.assertRaisesRegex(RecordError, "found 2"):
                test_binary_from_cargo_messages(messages)

    def test_exact_test_runner_records_binary_log_and_blocked_opt_in(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "termeric-test"
            binary.write_bytes(b"test-binary")
            build_message = json.dumps(
                {
                    "reason": "compiler-artifact",
                    "target": {"name": "termeric", "kind": ["bin"]},
                    "profile": {"test": True},
                    "executable": str(binary),
                }
            )
            build = mock.Mock(returncode=0, stdout=build_message, stderr="built")
            test = mock.Mock(
                returncode=0,
                stdout=(
                    f"running 1 test\ntest {FAKE_PROVIDER_TEST} ... ok\n"
                    "test result: ok. 1 passed; 0 failed\n"
                ),
                stderr="",
            )
            with mock.patch(
                "scripts.run_fake_provider_acceptance.subprocess.run",
                side_effect=[build, test],
            ) as run:
                result = run_fake_provider_test(
                    root / "artifacts",
                    candidate_sha="a" * 40,
                    environment={"TERMERIC_REAL_CODEX_CONTRACT": "1"},
                )
            self.assertEqual(result["status"], "pass")
            self.assertEqual(
                run.call_args_list[1].args[0],
                [str(binary), FAKE_PROVIDER_TEST, "--exact", "--nocapture"],
            )
            self.assertNotIn(
                "TERMERIC_REAL_CODEX_CONTRACT",
                run.call_args_list[1].kwargs["env"],
            )
            machine = json.loads(
                (root / "artifacts/fake-provider-result.json").read_text()
            )
            self.assertEqual(machine["candidate_sha"], "a" * 40)
            self.assertEqual(
                machine["removed_provider_opt_ins"],
                ["TERMERIC_REAL_CODEX_CONTRACT"],
            )

    def test_runner_refuses_to_overwrite_evidence_artifacts(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            artifacts = Path(directory)
            (artifacts / "fake-provider-test.log").write_text("existing")
            with self.assertRaisesRegex(RecordError, "refusing to overwrite"):
                run_fake_provider_test(artifacts, candidate_sha="a" * 40)

    def test_record_population_changes_only_two_fake_cases(self) -> None:
        record = create_template("a" * 40, "Operator")
        fill_fake_provider_cases(
            record,
            platform_name="Linux-test",
            rustc_version="rustc test",
            test_binary_sha256="b" * 64,
        )
        cases = record["cases"]
        assert isinstance(cases, list)
        for case in cases:
            assert isinstance(case, dict)
            if case["id"] in FAKE_PROVIDER_CASES:
                self.assertEqual(case["disposition"], "pass")
                self.assertEqual(set(case["checks"].values()), {"pass"})
                self.assertEqual(len(case["artifacts"]), 2)
            else:
                self.assertEqual(case["disposition"], "unrecorded")
                self.assertEqual(set(case["checks"].values()), {"unrecorded"})


if __name__ == "__main__":
    unittest.main()
