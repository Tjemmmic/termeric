import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

from scripts import run_shell_acceptance as runner
from scripts.cargo_artifacts import CargoArtifactError


def rows(extended=False):
    return {name: {"shell": name, "path": f"/usr/bin/{name}", "version": f"{name} 1.0",
                   "sha256": "a" * 64, "capability_replies": int(name == "fish")}
            for name in (runner.EXTENDED_SHELLS if extended else runner.SHELLS)}


def observed(extended=False):
    return {name: {"path": row["path"], "version": row["version"],
                   "capability_replies": row["capability_replies"]}
            for name, row in rows(extended).items()}


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
        self.assertEqual(runner.observations(test_output()), observed())

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
        self.assertEqual(runner.observations(output, True), observed(True))
        malformed = [test_output(), output + output,
                     output.replace("6 passed", "5 passed"),
                     output.replace("0 ignored", "1 ignored"),
                     test_output(rows(), True),
                     output + "test unexpected ... ok\n"]
        malformed.extend(output.replace(f"test {name} ... ok\n", "")
                         for name in runner.EXTENDED_TESTS)
        malformed.extend(test_output({name: row for name, row in rows(True).items()
                                      if name != missing}, True)
                         for missing in runner.EXTENDED_SHELLS)
        for text in malformed:
            with self.subTest(text=text), self.assertRaises(runner.ShellAcceptanceError):
                runner.observations(text, True)
        with self.assertRaises(runner.ShellAcceptanceError):
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
            with self.subTest(output=output), self.assertRaises(runner.ShellAcceptanceError):
                runner.observations(output)

    def test_malformed_observations_cannot_pass(self):
        for field, value in (("shell", []), ("shell", "nu"), ("path", "relative"),
                             ("version", ""), ("capability_replies", True),
                             ("capability_replies", -1)):
            data = rows()
            data["bash"][field] = value
            with self.subTest(field=field, value=value), self.assertRaises(runner.ShellAcceptanceError):
                runner.observations(test_output(data))

    @unittest.skipUnless(sys.platform == "linux", "Linux process-group runner")
    def test_bounded_command_success_failure_timeout_and_overflow_retain_logs(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            def run(code, name, timeout=5):
                return runner.run_logged([sys.executable, "-I", "-S", "-c", code],
                                         path / name, {}, timeout)
            self.assertEqual(run("print('ok')", "pass"), "ok\n")
            with self.assertRaisesRegex(runner.ShellAcceptanceError, "exited 7"):
                run("print('failed'); raise SystemExit(7)", "fail")
            self.assertEqual((path / "fail").read_text(), "failed\n")
            with self.assertRaisesRegex(runner.ShellAcceptanceError, "timed out"):
                run("import time; time.sleep(10)", "timeout", 0.05)
            with mock.patch.object(runner, "MAX_LOG_BYTES", 64):
                with self.assertRaisesRegex(runner.ShellAcceptanceError, "exceeds"):
                    run("print('x' * 1000)", "overflow")
            self.assertEqual((path / "overflow").stat().st_size, 64)

    def run_pipeline(self, output, *, test_result=None, extended=False):
        binary = Path("/fixture/test-binary")

        def logged(command, path, env, timeout):
            path.write_text("fixture log")
            self.assertEqual(env["TERMERIC_REQUIRE_SHELL_ACCEPTANCE"], "1")
            if command[0] == "cargo":
                return "build fixture"
            tests = runner.EXTENDED_TESTS if extended else (runner.TEST,)
            self.assertEqual(command, [str(binary), *tests, "--exact", "--show-output", "--test-threads=1"])
            if isinstance(test_result, Exception):
                raise test_result
            return test_output(extended=extended) if test_result is None else test_result

        with mock.patch.object(runner, "run_logged", side_effect=logged), \
             mock.patch.object(runner, "test_binary_from_cargo_messages", return_value=binary):
            try:
                outcome = runner.run(output, extended)
            except runner.ShellAcceptanceError as error:
                outcome = error
        return outcome, json.loads((output / "results.json").read_text())

    def test_passing_run_writes_shell_results(self):
        with tempfile.TemporaryDirectory() as directory:
            outcome, results = self.run_pipeline(Path(directory) / "out")
            self.assertEqual(results["status"], "pass")
            self.assertEqual(results["shells"], observed())
            self.assertEqual(outcome, results)

    def test_extended_run_requires_all_five_shells(self):
        with tempfile.TemporaryDirectory() as directory:
            _outcome, results = self.run_pipeline(Path(directory) / "out", extended=True)
            self.assertEqual(results["status"], "pass")
            self.assertEqual(set(results["shells"]), set(runner.EXTENDED_SHELLS))

    def test_failed_test_or_bad_output_records_a_failure(self):
        for result in (runner.ShellAcceptanceError("command exited 101"),
                       test_output().replace(" ... ok", " ... FAILED")):
            with self.subTest(result=result), tempfile.TemporaryDirectory() as directory:
                outcome, results = self.run_pipeline(Path(directory) / "out", test_result=result)
                self.assertIsInstance(outcome, runner.ShellAcceptanceError)
                self.assertEqual(results["status"], "fail")
                self.assertTrue(results["error"])


    def test_missing_test_binary_fails_cleanly(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "out"
            with mock.patch.object(runner, "run_logged", return_value="build fixture"), \
                 mock.patch.object(runner, "test_binary_from_cargo_messages",
                                   side_effect=CargoArtifactError("found 0")), \
                 mock.patch.object(sys, "argv", ["run_shell_acceptance.py", "--output", str(output)]), \
                 mock.patch("sys.stderr"):
                self.assertEqual(runner.main(), 1)
            self.assertEqual(json.loads((output / "results.json").read_text())["status"], "fail")

if __name__ == "__main__":
    unittest.main()
