import copy
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

from scripts import run_shell_acceptance as runner
from scripts.integration_acceptance_record import create_template, file_sha256, validate_record


def rows(extended=False):
    return {name: {"shell": name, "path": f"/usr/bin/{name}", "version": f"{name} 1.0",
                   "sha256": "a" * 64, "capability_replies": int(name == "fish")}
            for name in (runner.EXTENDED_SHELLS if extended else runner.SHELLS)}


def test_output(entries=None, extended=False):
    tests = runner.EXTENDED_TESTS if extended else (runner.TEST,)
    entries = rows(extended) if entries is None else entries
    return ("".join(f"test {test} ... ok\n" for test in tests)
            + "\n".join(runner.PREFIX + json.dumps(row) for row in entries.values())
            + f"\ntest result: ok. {len(tests)} passed; 0 failed; 0 ignored; 10 filtered out\n")


class ShellAcceptanceTests(unittest.TestCase):
    def test_environment_requires_shells_and_removes_provider_opt_ins_and_startup_overrides(self):
        env = {name: "private" for name in runner.REMOVED_ENVIRONMENT}
        env["PATH"] = "/usr/bin:/bin"
        self.assertEqual(runner.safe_environment(env), {
            "PATH": "/usr/bin:/bin", "TERMERIC_REQUIRE_SHELL_ACCEPTANCE": "1",
        })

    def test_exact_observations_preserve_zero_reply_count(self):
        self.assertEqual(runner.observations(test_output()), rows())

    def test_extended_environment_requires_both_optional_shells_without_provider_opt_ins(self):
        source = {name: "private" for name in runner.REMOVED_ENVIRONMENT}
        source["TERMERIC_REQUIRE_NUSHELL_ACCEPTANCE"] = "0"
        source["TERMERIC_REQUIRE_ELVISH_ACCEPTANCE"] = "0"
        self.assertEqual(runner.safe_environment(source, True), {
            "TERMERIC_REQUIRE_SHELL_ACCEPTANCE": "1",
            "TERMERIC_REQUIRE_NUSHELL_ACCEPTANCE": "1",
            "TERMERIC_REQUIRE_ELVISH_ACCEPTANCE": "1",
        })

    def test_extended_observations_require_all_exact_tests_and_all_five_shells(self):
        output = test_output(extended=True)
        self.assertEqual(runner.observations(output, True), rows(True))
        malformed = [test_output(), output + output,
                     output.replace("6 passed", "5 passed"),
                     output.replace("0 ignored", "1 ignored"),
                     test_output(rows(), True),
                     output + f"test unexpected ... ok\n"]
        malformed.extend(output.replace(f"test {name} ... ok\n", "")
                         for name in runner.EXTENDED_TESTS)
        malformed.extend(test_output({name: row for name, row in rows(True).items()
                                      if name != missing}, True)
                         for missing in runner.EXTENDED_SHELLS)
        for text in malformed:
            with self.subTest(text=text), self.assertRaises(runner.RecordError):
                runner.observations(text, True)
        with self.assertRaises(runner.RecordError):
            runner.observations(output)  # extended coverage must be explicit

    def test_skipped_zero_failed_or_wrong_test_cannot_pass(self):
        for output in (
            test_output().replace("1 passed", "0 passed"),
            test_output().replace("0 ignored", "1 ignored"),
            test_output().replace(runner.TEST, "different_test"),
            test_output().replace(" ... ok", " ... FAILED"),
            test_output({"bash": rows()["bash"]}),
            test_output() + runner.PREFIX + json.dumps(rows()["bash"]),
            test_output() + runner.PREFIX + "{",
        ):
            with self.subTest(output=output), self.assertRaises(runner.RecordError):
                runner.observations(output)

    def test_malformed_observations_cannot_pass(self):
        for field, value in (("shell", []), ("shell", "nu"), ("path", "relative"),
                             ("version", ""), ("sha256", "bad"),
                             ("capability_replies", True), ("capability_replies", -1)):
            data = rows()
            data["bash"][field] = value
            with self.subTest(field=field, value=value), self.assertRaises(runner.RecordError):
                runner.observations(test_output(data))

    def test_fill_does_not_promote_unexercised_replies_or_other_cases(self):
        record = create_template("a" * 40, "operator")
        before = copy.deepcopy(record)
        runner.fill_shell_cases(record, rows(), "b" * 64)
        for case, previous in zip(record["cases"], before["cases"]):
            if case["id"] == "shell.fish":
                self.assertEqual(case["disposition"], "pass")
            elif case["id"] in {"shell.bash", "shell.zsh"}:
                self.assertEqual(case["disposition"], "accepted_limitation")
                self.assertEqual(list(case["checks"].values()).count("pass"), 4)
            else:
                self.assertEqual(case, previous)
        self.assertEqual(validate_record(Path("/tmp/record.json"), record,
                                        allow_incomplete=True, require_artifact_files=False), [])

    def test_extended_fill_retains_nushell_bug_and_elvish_opt_in_without_other_promotions(self):
        record = create_template("a" * 40, "operator")
        before = copy.deepcopy(record)
        runner.fill_shell_cases(record, rows(True), "b" * 64, True)
        for case, previous in zip(record["cases"], before["cases"]):
            if case["id"] == "shell.nushell":
                self.assertEqual(case["disposition"], "accepted_limitation")
                self.assertEqual(case["checks"]["native_osc7_working_directory"], "accepted_limitation")
                self.assertEqual(list(case["checks"].values()).count("pass"), 3)
                self.assertIn("literal-percent", case["notes"])
            elif case["id"] == "shell.elvish":
                self.assertEqual(case["disposition"], "pass")
                self.assertEqual(len(case["checks"]), 6)
                self.assertIn("explicitly imports", case["notes"])
            elif not case["id"].startswith("shell."):
                self.assertEqual(case, previous)
        self.assertEqual(validate_record(Path("/tmp/record.json"), record,
                                        allow_incomplete=True, require_artifact_files=False), [])

    @unittest.skipUnless(sys.platform == "linux", "Linux process-group runner")
    def test_bounded_command_success_failure_timeout_and_overflow_retain_logs(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            def run(code, name, timeout=5):
                return runner.run_logged([sys.executable, "-I", "-S", "-c", code],
                                         path / name, {}, timeout)
            self.assertEqual(run("print('ok')", "pass"), "ok\n")
            with self.assertRaisesRegex(runner.RecordError, "exited 7"):
                run("print('failed'); raise SystemExit(7)", "fail")
            self.assertEqual((path / "fail").read_text(), "failed\n")
            with self.assertRaisesRegex(runner.RecordError, "timed out"):
                run("import time; time.sleep(10)", "timeout", 0.05)
            with mock.patch.object(runner, "MAX_LOG_BYTES", 64):
                with self.assertRaisesRegex(runner.RecordError, "exceeds"):
                    run("print('x' * 1000)", "overflow")
            self.assertEqual((path / "overflow").stat().st_size, 64)
            with self.assertRaises(FileExistsError):
                run("print('changed')", "pass")
            self.assertEqual((path / "pass").read_text(), "ok\n")

    def pipeline(self, root, *, output=None, proof_ok=True, final_dirty=False, test_failure=False,
                 extended=False, observation_output=None):
        root = Path(root)
        output = output or root / "evidence"
        binary = root / "test-binary"
        binary.write_bytes(b"binary")
        initial = {"commit": "a" * 40, "worktree_dirty": False}
        pinned = mock.MagicMock()
        pinned.__enter__.return_value = pinned
        pinned.launch.return_value = Path("/proc/123/fd/4")
        pinned.file_evidence = {"sha256": "b" * 64}
        pinned.finish.return_value = {"fixture": "proof"}

        def logged(command, path, env, timeout):
            path.write_text("fixture log")
            self.assertEqual(env["TERMERIC_REQUIRE_SHELL_ACCEPTANCE"], "1")
            if command[0] == "cargo":
                return "build fixture"
            tests = runner.EXTENDED_TESTS if extended else (runner.TEST,)
            self.assertEqual(command, ["/proc/123/fd/4", *tests, "--exact", "--show-output", "--test-threads=1"])
            if extended:
                self.assertEqual(env["TERMERIC_REQUIRE_NUSHELL_ACCEPTANCE"], "1")
                self.assertEqual(env["TERMERIC_REQUIRE_ELVISH_ACCEPTANCE"], "1")
            if test_failure:
                raise runner.RecordError("fixture failed")
            return test_output(extended=extended) if observation_output is None else observation_output

        with mock.patch.object(runner, "source_identity", side_effect=[initial, {**initial, "worktree_dirty": final_dirty}]), \
             mock.patch.object(runner, "run_logged", side_effect=logged), \
             mock.patch.object(runner, "test_binary_from_cargo_messages", return_value=binary), \
             mock.patch.object(runner, "PinnedExecutable", return_value=pinned), \
             mock.patch.object(runner, "provenance_checks", return_value=[{"passed": proof_ok}]), \
             mock.patch.object(runner, "file_sha256", side_effect=lambda path: "a" * 64 if str(path).startswith("/usr/bin/") else file_sha256(path)):
            runner.run(output, "operator", extended)
        return output

    @unittest.skipUnless(sys.platform == "linux", "Linux provenance")
    def test_pipeline_publishes_only_partial_record_with_exact_candidate_and_pinned_test(self):
        with tempfile.TemporaryDirectory() as directory:
            output = self.pipeline(directory)
            record = json.loads((output / "record.json").read_text())
            self.assertEqual(record["candidate"]["sha"], "a" * 40)
            self.assertEqual(sum(case["disposition"] == "unrecorded" for case in record["cases"]), 9)
            self.assertTrue((output / "artifacts/shell-result.json").is_file())

    @unittest.skipUnless(sys.platform == "linux", "Linux provenance")
    def test_extended_pipeline_seals_exact_selection_and_keeps_other_cases_unrecorded(self):
        with tempfile.TemporaryDirectory() as directory:
            output = self.pipeline(directory, extended=True)
            record = json.loads((output / "record.json").read_text())
            self.assertEqual(sum(case["disposition"] == "unrecorded" for case in record["cases"]), 7)
            evidence = json.loads((output / "artifacts/shell-result.json").read_text())
            self.assertEqual(evidence["schema"], 2)
            self.assertEqual(evidence["tests"], list(runner.EXTENDED_TESTS))
            self.assertEqual(set(evidence["shells"]), set(runner.EXTENDED_SHELLS))
            self.assertEqual(evidence["candidate_sha"], record["candidate"]["sha"])
            self.assertEqual(validate_record(output / "record.json", record, allow_incomplete=True), [])

    @unittest.skipUnless(sys.platform == "linux", "Linux provenance")
    def test_extended_incomplete_or_malformed_observations_never_publish(self):
        for observation_output in (test_output(), test_output(extended=True).replace("6 passed", "0 passed")):
            with tempfile.TemporaryDirectory() as directory:
                with self.assertRaises(runner.RecordError):
                    self.pipeline(directory, extended=True, observation_output=observation_output)
                self.assertFalse((Path(directory) / "evidence/record.json").exists())
                self.assertTrue((Path(directory) / "evidence/artifacts/shell-provenance.json").is_file())

    @unittest.skipUnless(sys.platform == "linux", "Linux provenance")
    def test_drift_or_test_failure_retains_diagnostics_without_publishing_record(self):
        for options in ({"proof_ok": False}, {"final_dirty": True}, {"test_failure": True}):
            with tempfile.TemporaryDirectory() as directory:
                with self.assertRaises(runner.RecordError):
                    self.pipeline(directory, **options)
                self.assertFalse((Path(directory) / "evidence/record.json").exists())
                self.assertTrue((Path(directory) / "evidence/artifacts/shell-test.log").is_file())

    @unittest.skipUnless(sys.platform == "linux", "Linux provenance")
    def test_dirty_source_and_existing_output_are_refused_before_build(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "evidence"
            with mock.patch.object(runner, "source_identity", return_value={"commit": "a" * 40, "worktree_dirty": True}), \
                 mock.patch.object(runner, "run_logged") as run:
                with self.assertRaisesRegex(runner.RecordError, "dirty"):
                    runner.run(output, "operator")
                run.assert_not_called()
                self.assertFalse(output.exists())
            output.mkdir()
            with self.assertRaisesRegex(runner.RecordError, "overwrite"):
                runner.run(output, "operator")

    def test_ci_uses_required_runner_and_separate_always_uploaded_artifact(self):
        workflow = (runner.ROOT / ".github/workflows/ci.yml").read_text()
        self.assertIn("python3 -m unittest scripts.test_run_shell_acceptance", workflow)
        block = workflow.split("- name: Record Bash, Zsh, and Fish shell integration", 1)[1].split("- run: cargo clippy", 1)[0]
        self.assertIn("python3 scripts/run_shell_acceptance.py", block)
        self.assertIn("--output target/shell-acceptance", block)
        self.assertIn("if: always()", block)
        self.assertIn("name: required-shell-acceptance", block)
        self.assertIn("path: target/shell-acceptance/", block)


if __name__ == "__main__":
    unittest.main()
