"""Private executable and mocked-compositor tests; no desktop or providers."""
import contextlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

from scripts import gui_acceptance as gui, gui_candidate, soak_provenance


@unittest.skipUnless(sys.platform == "linux", "Linux executable descriptors")
class GuiCandidateTests(unittest.TestCase):
    def invoke(self, mode="pass", extra=()):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = root / "termeric"
            binary.write_bytes(b"original fixture binary")
            binary.chmod(0o700)
            artifacts = root / "evidence"
            state = {"commit": "a" * 40, "worktree_dirty": mode == "dirty"}
            observed = {}
            if mode == "exists":
                artifacts.mkdir()
                (artifacts / "sentinel").write_text("preserve")

            def scenario(path, run_dir, env):
                observed["fd"] = int(path.name)
                self.assertTrue(str(path).startswith(f"/proc/{os.getpid()}/fd/"))
                self.assertEqual(path.read_bytes(), b"original fixture binary")
                self.assertFalse((artifacts / "results.json").exists())
                if mode == "source":
                    state["commit"] = "b" * 40
                if mode == "dirty-after":
                    state["worktree_dirty"] = True
                if mode == "replaced":
                    replacement = root / "replacement"
                    replacement.write_bytes(b"replacement fixture")
                    replacement.chmod(0o700)
                    replacement.replace(binary)
                    self.assertEqual(path.read_bytes(), b"original fixture binary")
                if mode == "deleted":
                    binary.unlink()
                if mode == "scenario-failed":
                    raise gui.AcceptanceError("injected scenario failure")

            argv = ["gui_acceptance.py", "--backend", "wayland", "--start-sway",
                    "--scenario", "interaction", "--binary", str(binary),
                    "--artifacts", str(artifacts), "--require-clean", *extra]
            stdout = io.StringIO()
            with (
                mock.patch.object(gui, "ROOT", root),
                mock.patch.object(gui, "Image", object()),
                mock.patch.object(gui, "ImageChops", object()),
                mock.patch.object(gui.shutil, "which", return_value="/fixture/tool"),
                mock.patch.object(gui, "run"),
                mock.patch.object(gui, "sway_environment", return_value=({}, None)),
                mock.patch.object(gui, "run_wayland_interaction_scenario", side_effect=scenario) as run,
                mock.patch.object(gui_candidate, "source_identity", side_effect=lambda _: dict(state)),
                mock.patch.object(soak_provenance, "observe_source", side_effect=lambda _: dict(state)),
                mock.patch.object(sys, "argv", argv),
                contextlib.redirect_stdout(stdout),
            ):
                invalid_extra = "--update-baselines" in extra or "--scenario" in extra
                if mode == "pass" and not invalid_extra:
                    self.assertEqual(gui.main(), 0)
                else:
                    with self.assertRaises((ValueError, gui.AcceptanceError)):
                        gui.main()
                if mode in {"dirty", "exists"} or invalid_extra:
                    run.assert_not_called()
            if "fd" in observed:
                with self.assertRaises(OSError):
                    os.fstat(observed["fd"])
            if mode == "exists":
                self.assertEqual((artifacts / "sentinel").read_text(), "preserve")
            record = artifacts / "results.json"
            proof = artifacts / "provenance.json"
            return (json.loads(record.read_text()) if record.exists() else None,
                    json.loads(proof.read_text()) if proof.exists() else None,
                    stdout.getvalue())

    def test_clean_run_seals_selected_scenarios_and_pinned_binary(self):
        record, proof, stdout = self.invoke()
        self.assertEqual(record["schema"], 4)
        self.assertEqual(record["candidate"], proof)
        self.assertTrue(proof["candidate_eligible"])
        self.assertEqual(proof["requested_scenarios"], ["interaction"])
        self.assertFalse(proof["skip_visual_checks"])
        self.assertEqual(proof["candidate_sha"], "a" * 40)
        self.assertEqual(record["binary_sha256"], proof["provenance"]["binary_initial"]["sha256"])
        self.assertIn("GUI acceptance passed", stdout)

    def test_skipped_visual_policy_is_explicit_without_changing_scenario_results(self):
        record, proof, _ = self.invoke(extra=("--skip-visual-checks",))
        self.assertTrue(proof["skip_visual_checks"])
        self.assertTrue(proof["candidate_eligible"])
        self.assertEqual(record["scenarios"], [{"scenario": "interaction", "status": "passed"}])

    def test_source_and_binary_drift_cannot_publish_success(self):
        for mode in ("source", "dirty-after", "replaced", "deleted", "scenario-failed"):
            with self.subTest(mode=mode):
                record, proof, stdout = self.invoke(mode)
                self.assertIsNotNone(record)
                self.assertFalse(proof["candidate_eligible"])
                self.assertEqual(record["candidate"], proof)
                self.assertNotIn("GUI acceptance passed", stdout)
                self.assertTrue(any(not check["passed"] for check in proof["checks"]))

    def test_dirty_existing_duplicate_and_baseline_update_refused_before_scenarios(self):
        for mode, extra in (("dirty", ()), ("exists", ()),
                            ("pass", ("--update-baselines",)),
                            ("pass", ("--scenario", "interaction"))):
            with self.subTest(mode=mode, extra=extra):
                record, proof, stdout = self.invoke(mode, extra)
                self.assertIsNone(record)
                self.assertIsNone(proof)
                self.assertNotIn("GUI acceptance passed", stdout)

    def test_real_pinned_execution_survives_replacement_but_evidence_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "true"
            shutil.copyfile("/bin/true", binary)
            binary.chmod(0o700)
            source = {"commit": "a" * 40, "worktree_dirty": False}
            with (
                mock.patch.object(gui_candidate, "source_identity", return_value=source),
                mock.patch.object(soak_provenance, "observe_source", return_value=source),
                self.assertRaisesRegex(ValueError, "candidate evidence failed"),
                gui_candidate.GuiCandidate(root, binary, root / "evidence", ["launch"], False) as candidate,
            ):
                path = candidate.launch()
                replacement = root / "replacement"
                shutil.copyfile("/bin/false", replacement)
                replacement.chmod(0o700)
                replacement.replace(binary)
                self.assertEqual(subprocess.run([str(path)], timeout=5).returncode, 0)
                candidate.publish({"binary": str(binary), "binary_sha256": candidate.digest,
                                   "scenarios": [{"scenario": "launch", "status": "passed"}]})
            proof = json.loads((root / "evidence/provenance.json").read_text())
            self.assertFalse(proof["candidate_eligible"])
            self.assertFalse(proof["skip_visual_checks"])

    def test_missing_result_never_passes_and_closes_descriptor(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "fixture"
            binary.write_bytes(b"fixture")
            binary.chmod(0o700)
            source = {"commit": "a" * 40, "worktree_dirty": False}
            with (
                mock.patch.object(gui_candidate, "source_identity", return_value=source),
                mock.patch.object(soak_provenance, "observe_source", return_value=source),
                self.assertRaises(ValueError),
                gui_candidate.GuiCandidate(root, binary, root / "evidence", ["launch"], True) as candidate,
            ):
                fd = int(candidate.launch().name)
            self.assertFalse((root / "evidence/results.json").exists())
            self.assertFalse(json.loads((root / "evidence/provenance.json").read_text())["candidate_eligible"])
            with self.assertRaises(OSError):
                os.fstat(fd)

    def test_ci_requires_clean_mode_for_each_native_gui_command(self):
        workflow = (gui.ROOT / ".github/workflows/ci.yml").read_text()
        self.assertEqual(workflow.count("python3 -m unittest scripts.test_gui_candidate"), 2)
        commands = workflow.split("python3 scripts/gui_acceptance.py")[1:]
        self.assertEqual(len(commands), 4)
        for command in commands:
            self.assertIn("--require-clean", command.split("- name:", 1)[0])


if __name__ == "__main__":
    unittest.main()
