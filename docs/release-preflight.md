# Hermetic release preflight

Termeric's release preflight turns the provider-free source-quality gates into
one candidate-bound evidence directory. It is the reproducible foundation for
a release review, not a claim that the candidate passed native desktops,
packages, long-duration stability, remote hosts, or authenticated integrations.

## Run a clean candidate

Install the normal Linux build dependencies plus `tic`, `infocmp`, Vim, less,
and tmux. Populate Cargo's locked cache before starting if dependencies are not
already present; every Cargo command inside the evidence run is forced offline.

```bash
cargo fetch --locked
python3 scripts/release_preflight.py run \
  --output target/release-preflight
python3 scripts/release_preflight.py validate \
  target/release-preflight/record.json
```

The runner refuses a dirty worktree and an existing output path. The output is
built in a temporary sibling directory and becomes visible at the requested
path only after every log and `record.json` has been written. The default path
is already `target/release-preflight` when `--output` is omitted.

For implementation work, `--allow-dirty` can produce a diagnostic dry run. Its
record permanently says `dirty-dry-run` and `candidate_eligible: false`; normal
validation rejects it. `validate --allow-ineligible` checks such a record's
shape and artifacts without promoting it to candidate evidence.

## Fixed gate set

The schema-1 runner executes these trusted, source-defined commands in order:

1. Rust formatting
2. local Markdown-link validation
3. authoritative feature-support validation
4. all Python unit tests under `scripts/`
5. compiled terminfo plus Vim/less/tmux PTY acceptance
6. strict workspace Clippy
7. the locked workspace Rust suite
8. the locked debug workspace build
9. the locked optimized release workspace build
10. locked workspace documentation without dependency docs
11. the release binary's non-GUI version probe

The command list cannot be supplied by a record or CLI argument. Each child
receives `CARGO_NET_OFFLINE=true`; known provider credentials and provider
opt-in variables are removed. No gate invokes installed Codex, Claude Code, an
AI SDK, authentication, or a provider service. Hermetic fake-executable tests
inside the ordinary Rust suite remain permitted.

## Evidence and validation

`record.json` binds:

- schema/kind and pass/eligibility state;
- the full Git SHA, workspace version, and clean-tree state before and after
  execution;
- OS, architecture, Python, Cargo, Rust, Git, and ncurses context;
- exact gate order, argv, timeout, exit status, duration, and bounded summary;
- relative complete logs with byte counts and SHA-256 digests; and
- the release binary's relative path, byte size, SHA-256 digest, and exact
  `termeric VERSION` output.

Validation uses the trusted current gate definitions and current checkout. It
rejects missing, extra, duplicate, reordered, failed, or changed gates;
absolute/traversing/symlink/duplicate log paths; log or summary tampering;
candidate/version/clean-state mismatch; invalid provider policy; and missing or
changed release binaries. It is deliberately read-only.

The output directory itself is not signed. Preserve it as one immutable CI
artifact, and use release signing/provenance separately when distribution work
reaches that stage.

## Boundaries and composition

A green preflight establishes the hermetic portions of release gates 1, 2, 5,
and 8 plus the ordinary test/build foundation of gate 3. It does not establish:

- X11/Wayland compositor, GPU, IME, screen-reader, clipboard, notification, or
  visual behavior;
- Debian/Arch install, upgrade, removal, or actual prior-release migration;
- short, scheduled, or release-duration resource stability;
- macOS/Windows runtime behavior;
- real SSH/domain interoperability or external-terminal comparisons; or
- Claude Code, Codex, Harvest Explain, account, spending, or provider behavior.

Those claims retain their own native, integration, package, benchmark, and soak
records. GitHub Actions exposes this preflight only through the explicit
`release_preflight` workflow-dispatch input and uploads the complete directory.
