"""Fail-closed clean-candidate publication for the Linux GUI harness."""
from __future__ import annotations

from pathlib import Path

from scripts.native_acceptance_record import atomic_write_json
from scripts.soak_provenance import PinnedExecutable, provenance_checks, source_identity


class GuiCandidate:
    def __init__(self, root: Path, binary: Path, artifacts: Path,
                 scenarios: list[str], skip_visual_checks: bool):
        self.root, self.binary, self.artifacts = root, binary, artifacts
        self.scenarios = list(scenarios)
        self.skip_visual_checks = skip_visual_checks
        self.document = None
        self.pinned = None

    def __enter__(self):
        if self.artifacts.exists() or self.artifacts.is_symlink():
            raise ValueError("clean GUI evidence requires a new artifact directory")
        initial = source_identity(self.root)
        if initial["worktree_dirty"]:
            raise ValueError("clean GUI evidence refuses dirty source")
        self.pinned = PinnedExecutable(self.root, self.binary, initial)
        try:
            self.artifacts.mkdir(parents=True, exist_ok=False)
        except BaseException:
            self.pinned.__exit__(None, None, None)
            raise
        return self

    @property
    def digest(self) -> str:
        return self.pinned.file_evidence["sha256"]

    def launch(self) -> Path:
        # One handoff to the scenario driver. All its direct binary invocations
        # use this parent-owned descriptor; it stays open until finalization.
        return self.pinned.launch("scenario")

    def publish(self, document: dict) -> None:
        if self.document is not None:
            raise ValueError("GUI result already supplied")
        self.document = document

    def __exit__(self, exc_type, exc, traceback):
        try:
            proof = self.pinned.finish()
            checks = provenance_checks(proof, ["scenario"])
            expected = [{"scenario": name, "status": "passed"} for name in self.scenarios]
            document = self.document
            completed = (
                exc_type is None and document is not None
                and document.get("scenarios") == expected
                and document.get("binary") == str(self.binary)
                and document.get("binary_sha256") == self.digest
            )
            checks.append({"name": "selected GUI scenarios and binary binding complete",
                           "actual": completed, "expected": True, "passed": completed})
            accepted = all(check["passed"] for check in checks)
            evidence = {
                "schema": 1, "candidate_sha": proof["source_initial"]["commit"],
                "candidate_eligible": accepted, "requested_scenarios": self.scenarios,
                "skip_visual_checks": self.skip_visual_checks, "provenance": proof, "checks": checks,
                "error": None if exc is None else str(exc),
            }
            atomic_write_json(self.artifacts / "provenance.json", evidence)
            if document is not None:
                document = {**document, "schema": 4, "candidate": evidence}
                atomic_write_json(self.artifacts / "results.json", document)
            if not accepted and exc_type is None:
                raise ValueError("GUI candidate evidence failed; see provenance.json")
        finally:
            self.pinned.__exit__(exc_type, exc, traceback)
        return False
