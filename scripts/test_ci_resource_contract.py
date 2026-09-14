"""Offline contract tests for platform-tier and resource-soak CI wiring."""

from __future__ import annotations

from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = ROOT / ".github" / "workflows" / "ci.yml"


def job_block(document: str, name: str) -> str:
    match = re.search(
        rf"(?ms)^  {re.escape(name)}:\n(?P<body>.*?)(?=^  [a-z0-9-]+:\n|\Z)",
        document,
    )
    if match is None:
        raise AssertionError(f"CI workflow is missing job {name!r}")
    return match.group("body")


class CiResourceContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.workflow = WORKFLOW.read_text(encoding="utf-8")

    def test_portable_tiers_build_and_test_on_native_runners(self) -> None:
        block = job_block(self.workflow, "portable-build-and-unit-tests")
        self.assertIn("os: [macos-latest, windows-latest]", block)
        self.assertIn('TERMERIC_REQUIRE_NATIVE_PTY_SMOKE: "1"', block)
        self.assertIn("native_pty_round_trip_when_requested", block)
        self.assertIn("cargo test --workspace --locked", block)
        self.assertIn("cargo build --workspace --locked", block)

    def test_change_gates_run_both_core_and_displayed_soaks(self) -> None:
        core = job_block(self.workflow, "linux-resource-soak-smoke")
        self.assertIn("github.event_name == 'push'", core)
        self.assertIn("github.event_name == 'pull_request'", core)
        self.assertIn("scripts/run_soak.py --duration-seconds 30 --gpu-smoke", core)
        self.assertIn("scripts/validate_soak.py target/soak/ci-smoke.json", core)
        self.assertRegex(
            core,
            r"Validate 30-second resource soak evidence\n\s+if: always\(\)",
        )
        self.assertIn("if: always()", core)
        self.assertIn("if-no-files-found: error", core)

        native = job_block(self.workflow, "linux-native-gui-acceptance")
        self.assertIn("scripts/run_native_gui_soak.py", native)
        self.assertIn("--duration-seconds 30", native)
        self.assertIn("--require-clean", native)
        self.assertIn("scripts/validate_native_gui_soak.py", native)
        self.assertIn("target/gui-acceptance/native-soak/results.json", native)
        self.assertRegex(
            native,
            r"Validate 30-second displayed resource soak evidence\n\s+if: always\(\)",
        )
        self.assertIn("if: always()", native)
        self.assertIn("if-no-files-found: error", native)
        self.assertNotIn("--require-gpu-memory", native)

    def test_native_lane_installs_every_real_graphics_client(self) -> None:
        block = job_block(self.workflow, "linux-native-gui-acceptance")
        for package in ("kitty", "libsixel-bin", "timg"):
            self.assertRegex(block, rf"\b{re.escape(package)}\b")
        self.assertIn("--backend x11", block)
        self.assertNotIn("--scenario", block.split("Run X11 interaction", 1)[1].split("Run Wayland", 1)[0])

    def test_scheduled_lane_runs_and_retains_both_ten_minute_soaks(self) -> None:
        block = job_block(self.workflow, "scheduled-linux-resource-soak")
        self.assertIn("github.event_name == 'schedule'", block)
        self.assertIn("github.event_name == 'workflow_dispatch'", block)
        self.assertIn("scripts/run_soak.py --duration-seconds 600 --gpu-smoke", block)
        self.assertIn("scripts/validate_soak.py target/soak/scheduled.json", block)
        self.assertRegex(
            block,
            r"Validate 10-minute resource soak evidence\n\s+if: always\(\)",
        )
        self.assertIn("scripts/run_native_gui_soak.py", block)
        self.assertIn("--duration-seconds 600", block)
        self.assertIn("--require-clean", block)
        self.assertIn("scripts/validate_native_gui_soak.py", block)
        self.assertIn("target/soak/native-gui/results.json", block)
        self.assertRegex(
            block,
            r"Validate 10-minute displayed resource soak evidence\n\s+if: always\(\)",
        )
        self.assertIn("if: always()", block)
        self.assertIn("if-no-files-found: error", block)
        self.assertNotIn("--require-gpu-memory", block)

    def test_quality_lane_records_and_uploads_hermetic_provider_evidence(self) -> None:
        block = job_block(self.workflow, "quality-and-package")
        self.assertIn("scripts.test_run_fake_provider_acceptance", block)
        self.assertIn("scripts/run_fake_provider_acceptance.py", block)
        self.assertIn("target/integration-acceptance/record.json", block)
        self.assertIn("name: hermetic-integration-acceptance", block)

    def test_quality_lane_exercises_both_soak_validators_and_launch_provenance(self):
        block = job_block(self.workflow, "quality-and-package")
        for module in (
            "scripts.test_soak_provenance", "scripts.test_validate_soak",
            "scripts.test_run_native_gui_soak", "scripts.test_validate_native_gui_soak",
        ):
            self.assertIn(module, block)


if __name__ == "__main__":
    unittest.main()
