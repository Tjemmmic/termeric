import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock

from scripts.run_fake_provider_acceptance import (
    FAKE_PROVIDER_TEST,
    AcceptanceError,
    run_fake_provider_test,
    safe_test_environment,
    test_binary_from_cargo_messages,
)


def build_message(binary: Path) -> str:
    return json.dumps({
        "reason": "compiler-artifact",
        "target": {"name": "termeric", "kind": ["bin"]},
        "profile": {"test": True},
        "executable": str(binary),
    })


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
            messages = "\n".join([
                json.dumps({"reason": "build-finished", "success": True}),
                build_message(binary),
            ])
            self.assertEqual(test_binary_from_cargo_messages(messages), binary)

    def test_cargo_message_rejects_missing_or_ambiguous_test_binaries(self) -> None:
        with self.assertRaisesRegex(AcceptanceError, "found 0"):
            test_binary_from_cargo_messages("not json")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binaries = [root / "first", root / "second"]
            for binary in binaries:
                binary.write_bytes(b"test")
            messages = "\n".join(build_message(binary) for binary in binaries)
            with self.assertRaisesRegex(AcceptanceError, "found 2"):
                test_binary_from_cargo_messages(messages)

    def run_with(self, root: Path, test_stdout: str):
        binary = root / "termeric-test"
        binary.write_bytes(b"test-binary")
        build = mock.Mock(returncode=0, stdout=build_message(binary), stderr="built")
        test = mock.Mock(returncode=0, stdout=test_stdout, stderr="")
        with mock.patch(
            "scripts.run_fake_provider_acceptance.subprocess.run",
            side_effect=[build, test],
        ) as run:
            try:
                outcome = run_fake_provider_test(
                    root / "out", environment={"TERMERIC_REAL_CODEX_CONTRACT": "1"}
                )
            except AcceptanceError as error:
                outcome = error
        results = json.loads((root / "out/results.json").read_text())
        return outcome, results, run, binary

    def test_passing_test_writes_results_without_the_provider_opt_in(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            outcome, results, run, binary = self.run_with(
                Path(directory),
                f"running 1 test\ntest {FAKE_PROVIDER_TEST} ... ok\n"
                "test result: ok. 1 passed; 0 failed\n",
            )
            self.assertEqual(outcome["status"], "pass")
            self.assertEqual(results["status"], "pass")
            self.assertEqual(results["removed_provider_opt_ins"], ["TERMERIC_REAL_CODEX_CONTRACT"])
            self.assertEqual(
                run.call_args_list[1].args[0],
                [str(binary), FAKE_PROVIDER_TEST, "--exact", "--nocapture"],
            )
            self.assertNotIn("TERMERIC_REAL_CODEX_CONTRACT", run.call_args_list[1].kwargs["env"])

    def test_missing_pass_line_fails_and_records_the_error(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            outcome, results, _run, _binary = self.run_with(
                Path(directory), "running 1 test\ntest result: ok. 0 passed; 0 failed\n"
            )
            self.assertIsInstance(outcome, AcceptanceError)
            self.assertEqual(results["status"], "fail")
            self.assertIn("did not pass", results["error"])
            self.assertTrue((Path(directory) / "out/fake-provider-test.log").is_file())


if __name__ == "__main__":
    unittest.main()
