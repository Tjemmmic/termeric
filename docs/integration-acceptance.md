# Integration acceptance

This document defines the repeatable acceptance contract for Termeric's AI,
Harvest, shell, clipboard, link, bell, and notification integrations. It keeps
hermetic evidence separate from actions that can contact an external account
or change external state. The authoritative product status remains
[feature-support.md](feature-support.md).

## Current result

| Integration | Evidence | Result on 2026-08-26 | Release interpretation |
|---|---|---|---|
| Claude Code built-in commands | Fake executable on a real PTY; argv, cwd, terminal-size environment, output, stdin, diff, error, transcript, export, context, profile, clear, resume, and restart paths | **Pass** | The Termeric integration contract is automated; the real Claude CLI and account are not implied. |
| Codex built-in commands | Same fake-PTY contract, including current approval/sandbox and `resume --last` argv | **Pass** | The Termeric integration contract is automated; the real Codex CLI and account are not implied. |
| Authenticated Claude Code | Manual procedure below | **Not run — operator consent required** | Named beta limitation; not a passing provider result. |
| Authenticated Codex | Manual procedure below | **Not run — operator consent required** | Named beta limitation; not a passing provider result. |
| Bash 5.3 | Isolated real PTY, custom user rc, multiline command, success/failure exits, OSC 7 cwd and OSC 133 regions | **Pass** | Linux shell semantics accepted. |
| Zsh 5.9.2 | Isolated real PTY, custom user rc, multiline command, success/failure exits, OSC 7 cwd and OSC 133 regions | **Pass** | Linux shell semantics accepted. |
| Fish 4.8.1 | Isolated real PTY, custom user rc, OSC 7 cwd, native OSC 133, startup capability-query replies, multiline command, success/failure exits | **Pass** | Linux shell semantics accepted. Fish 0-3 retain Termeric fallback hooks. |
| Nushell | Native OSC 7/133 settings enabled after user config; deterministic REPL/script/language-server/prelude routing; optional isolated real-PTY probe | **Implemented; real PTY not run — `nu` unavailable** | Current code is acceptance-pending and is not counted as a real shell pass. |
| Elvish | Standard generated module, exact module-search environment preservation, deterministic REPL/script/special-mode routing, user-rc import, and optional isolated real-PTY probe | **Implemented; real PTY not run — `elvish` unavailable** | Requires one `use termeric-integration` rc entry; current code is acceptance-pending and is not counted as a real shell pass. |
| Harvest ownership and interaction | Unit/integration fixtures with independent tab/pane observers, monotonic region/source/entry/request identities, signed scrollback and resize-reflow coordinates, bounded command/output snapshots, shared geometry, and modal input/AccessKit actions | **Pass** | History and actions remain associated with their producing terminal; capacity rollover never replays entries, evicted text cannot authorize stale row actions, and stale menu actions fail closed. Harvest stays experimental and disabled by default. Explain is not provider-tested. |
| Unsupported Harvest shell | Direct-command and unsupported-shell state carries a visible explanation | **Pass** | Empty Harvest state is not presented as a successful integration. |
| X11 selection copy/paste | Native Xvfb/twm window and `xclip` round trip | **Pass (automated X11)** | Real compositor permission policy remains manual. |
| Keyboard copy mode and quick select | Native Xvfb/twm keyboard-only search/select/yank plus isolated OSC 8 opener spy; PTY transcript proves modal keys are consumed | **Pass (automated X11)** | Screen-reader/IME acceptance is tracked by the platform checklist. |
| X11 OSC 52 write/read | Native Xvfb/twm fixture, exact `xclip` contents, requesting-PTY reply assertion | **Implemented; next CI/native result pending** | The extended scenario is a required CI gate; real compositor permission policy remains manual. |
| OSC 8 and detected links | Native Ctrl+click routed to an `xdg-open` spy | **Pass (automated X11)** | Real browser/portal choice remains manual. |
| Visual/audio bell | Native BEL, captured flash, and `paplay` spy | **Implemented; next CI/native result pending** | Real sound-device policy remains manual. |
| OSC 777 notification | Native PTY event routed to a `notify-send` spy | **Implemented; next CI/native result pending** | Real notification-daemon permission and presentation remain manual. |
| Kitty OSC 99 notifications | Fragmented, interleaved, bounded core protocol and exact owner-routed capability response; in-memory fixtures prove exact-origin `always`/`unfocused`/`invisible` filtering across active/inactive tabs, focused/visible/zoom-hidden panes, and native window focus/occlusion before delivery; Linux/backend fixtures prove stable origin-scoped replacement/close IDs, bounded expiry, requested close and default/button activation reports, standard sound/silence hints, alive queries, bounded PNG/JPEG/GIF decoding and process-wide icon-cache semantics, exact freedesktop image-data typing, timeout recovery, and exact delayed reply routing without contacting D-Bus | **Hermetic pass; native presentation/icon/action/signal/sound/occasion behavior deferred** | Application-requested native focus remains unadvertised; real notification-daemon icon/sound presentation, occasion observations, `ActionInvoked`, and `NotificationClosed` behavior remain manual and do not block continued implementation. |
| Wayland lifecycle | Private Weston launch, render, terminal identity, PTY exit, and normal close | **Pass (automated private compositor)** | Interactive clipboard/portal/notification policy on a real compositor remains manual. |

The passing shell versions above describe the local acceptance run. CI installs
the distribution Bash, Zsh, and Fish packages and sets
`TERMERIC_REQUIRE_SHELL_ACCEPTANCE=1`, so a missing required shell is a failure
rather than a skipped test. Nushell and Elvish probes are separate pending gates
until a runner installs the respective binary and sets
`TERMERIC_REQUIRE_NUSHELL_ACCEPTANCE=1` or
`TERMERIC_REQUIRE_ELVISH_ACCEPTANCE=1`.

## Hermetic AI command contract

The `fake_ai_clis_exercise_every_builtin_command_end_to_end` test creates an
executable fixture, launches it through the same PTY and tab path as a real AI
CLI, and exercises all twelve commands for both built-ins:

1. open;
2. resume;
3. copy last response;
4. clear session;
5. restart;
6. review diff;
7. copy diff;
8. insert context;
9. toggle transcript;
10. export session;
11. jump to the last error; and
12. select profile.

The fixture records argv, cwd, `COLUMNS`, `LINES`, and PTY input. Acceptance
asserts the exact initial, restart, and resume command vectors for both tools,
the inherited working directory, and terminal geometry. It emits a known
response, unified diff, and error after enough output to require scrollback.
The context workflow selects a synthetic terminal-output source, completes the
request against its stable destination, and proves the framed payload reaches
the fake PTY; profile selection activates the advertised profile through the
same choice action used by the UI. The test uses an isolated session directory
and never reads an API key or contacts a provider. Restart acceptance also
asserts that commands no longer resolve to a tab already entering its close
animation.

Run it with:

```bash
cargo test -p termeric fake_ai_clis_exercise_every_builtin_command_end_to_end -- --nocapture
```

Generate the candidate-bound partial acceptance record and sealed artifacts
with the narrower runner:

```bash
python3 scripts/run_fake_provider_acceptance.py \
  --output target/integration-acceptance/record.json \
  --operator "Local hermetic acceptance"
```

It resolves the exact compiled test executable from Cargo's JSON messages,
removes `TERMERIC_REAL_CODEX_CONTRACT`, runs only the fully qualified fake-CLI
test with libtest `--exact`, and records the test-binary and artifact digests.
Only `fake.claude_code` and `fake.codex` become passes. Shell, Harvest, desktop,
and authenticated-provider cases stay unrecorded until their own evidence is
collected. CI uploads this partial record rather than treating the full
workspace suite as evidence for every integration row.

The separate installed-Codex argument/parser probe is operator opt-in and is
skipped before launching the CLI unless
`TERMERIC_REAL_CODEX_CONTRACT=1` is set. It invokes only
`--version`/`--help` and is not authenticated-provider acceptance, but
keeping it outside the default lane guarantees ordinary roadmap verification
uses only the fake executable above.

## Shell and Harvest contract

Run the required three-shell lane with:

```bash
TERMERIC_REQUIRE_SHELL_ACCEPTANCE=1 \
  cargo test -p termeric \
  supported_shells_preserve_user_rc_and_emit_multiline_exit_regions \
  -- --nocapture
```

Run the acceptance-pending native Nushell lane on a host with `nu` installed:

```bash
TERMERIC_REQUIRE_NUSHELL_ACCEPTANCE=1 \
  cargo test -p termeric \
  nushell_native_integration_preserves_user_config_and_emits_regions_when_available \
  -- --nocapture
```

Run the acceptance-pending Elvish module lane on a host with `elvish` installed:

```bash
TERMERIC_REQUIRE_ELVISH_ACCEPTANCE=1 \
  cargo test -p termeric \
  elvish_module_preserves_user_config_and_emits_regions_when_available \
  -- --nocapture
```

Each shell receives a private `HOME` and configuration directory. The fixture
proves that the user's rc file still runs, submits a multiline command and a
failing command through a real PTY, returns terminal capability replies to the
originating PTY, and checks the resulting semantic command exit regions.
Nushell uses its own OSC producers rather than generated prompt hooks. Elvish's
isolated user rc imports the generated module exactly as documented; the module
appends rather than replaces hooks and restores the original `XDG_DATA_DIRS`.
Nushell scripts, command strings, language-server/IDE modes, and profiles that
already provide a pre-REPL command are deliberately not modified. Elvish
scripts, command strings, daemon/language-server modes, and `-norc` launches
are also preserved; missing Elvish rc setup and other unsupported shells receive
a truthful Harvest explanation.

The resulting Root Cellar stores bounded normalized command text, at most
64 KiB of output per entry, and at most 8 MiB of aggregate owned snapshots,
then opens on the newest entry. Count- and memory-driven retention evictions
are counted visibly, and clipped entries are labeled in visual and semantic
surfaces. Monotonic OSC 133
region identities remain observable exactly once across tracker-capacity
rollover, negative scrollback rows, resize reflow, and live-text eviction.
Evicted coordinates are explicitly unavailable: snapshot-backed copy/rerun can
continue, while search/scroll are disabled and described as not retained
rather than targeting replacement grid text. Execution revalidates the live
region in case scrollback changes while the menu is open. An active alternate
screen gets its own disabled reason and cannot reinterpret primary-screen rows.
Model, renderer, pointer, keyboard, wheel, and AccessKit fixtures exercise one
shared viewport and exact stable entry IDs. Activating an entry opens a clamped
sprout menu scoped to a monotonic request identity; replaced requests cannot
execute. These tests inspect the Explain contract but never select it, launch
an AI CLI, contact a provider, or consume AI tokens.

Previous/Next Prompt navigation is terminal-local and starts from the current
cursor at the live bottom or the relevant edge of a manually scrolled viewport.
Repeated actions walk exact retained marks in both directions. The selected
prompt follows ongoing PTY scrolling and resize reflow; a direct scroll resets
the navigation cursor, scrollback eviction retires it, and the alternate screen
never navigates primary-shell marks. Core state tests and a local two-way app
action fixture cover this path without external services.

Semantic marks are authoritative only while their primary-grid rows remain
backed. ED2 and RIS retire regions intersecting erased viewport rows while
preserving fully historical regions; ED3 and the user Clear Scrollback action
retire removed negative rows through the same terminal API. Active commands can
still complete once after an erase, but the resulting region is explicitly
unavailable for live-text extraction. OSC 133 emitted on the alternate screen
is ignored so full-screen applications cannot pollute primary-shell history.

The parser validates the command lifecycle rather than treating every marker as
an independent event. Stray or duplicate B/C/D sequences are ignored;
prompt-only and A/B/D cycles remain navigation marks but never become Harvest
commands. A new A recovers a C-stage command whose D was lost, using the new
prompt row as its finish with unknown status. D statuses parse only as 0–255;
missing, malformed, negative, overflowing, and out-of-range values remain
unknown while still closing an otherwise valid command.

## Native desktop contract

The X11 workspace scenario in `scripts/gui_acceptance.py` uses an isolated
configuration and executable spies. F4 tells the PTY fixture to emit BEL, an
OSC 777 notification, an OSC 52 clipboard write, and an OSC 52 read. The
harness then asserts:

- visual-bell capture and a `paplay` invocation;
- exact notification title/body arguments to `notify-send`;
- exact clipboard contents through `xclip`; and
- an exact base64 OSC 52 reply in the originating PTY transcript.

Audio and notification helpers are submitted to one 16-request background
queue with an eight-child ceiling and explicit child reaping. Launch, worker,
and capacity failures are capped to 512 characters, deduplicated until a
successful retry, logged, and exposed as warning toasts. Helper nonzero exits
are logged; actual sound-device and notification-daemon presentation remains a
manual platform boundary.

The same scenario independently proves selection copy, bracketed paste, and
OSC 8 link opening. Run the native suite as described in
[gui-acceptance.md](gui-acceptance.md).

## Versioned candidate record

Candidate integration results belong in the schema-1 record generated and
validated by
[`scripts/integration_acceptance_record.py`](../scripts/integration_acceptance_record.py).
It names every fake-provider command, supported and acceptance-pending shell,
Harvest workflow, X11/Wayland desktop integration, and authenticated-provider
command explicitly. A missing, renamed, or invented case/check fails instead of
silently changing the release matrix.

Create an unrecorded template only from the exact clean candidate:

```bash
python3 scripts/integration_acceptance_record.py template \
  --output target/integration-acceptance/record.json \
  --operator "Release Operator"
```

Fill each check with `pass`, `fail`, `unavailable`, or
`accepted_limitation`; non-pass cases require explanatory notes. Record the
exact shell/helper/provider versions and candidate binary or test-binary hash.
Artifacts must be relative to the record directory. Seal and validate them
with:

```bash
python3 scripts/integration_acceptance_record.py seal \
  target/integration-acceptance/record.json
python3 scripts/integration_acceptance_record.py validate \
  target/integration-acceptance/record.json \
  --expected-sha "$(git rev-parse HEAD)"
```

Use `--allow-incomplete` only while filling the template. Use
`--require-pass` only when policy requires every row to pass; ordinary beta
validation permits explicit unavailable or accepted-limitation rows but never
an unrecorded row. The generator refuses a dirty worktree so results cannot be
bound to a commit that omits the tested source.

Authenticated Claude Code and Codex cases have an additional approval object.
Any `pass` or `fail` provider check requires prior `granted` status, approver,
UTC approval time, redacted account scope, spending limit, allowed external
effects, and credential-cleanup disposition. Every referenced provider artifact
must be marked `reviewed_for_secrets: true` and pass the same path-confinement
and SHA-256 verification as other artifacts. These fields record permission;
the tool never launches a CLI, authenticates, contacts a service, or reads a
credential.

Run the pure validator regressions with:

```bash
python3 -m unittest scripts.test_integration_acceptance_record
python3 -m unittest scripts.test_run_fake_provider_acceptance
```

Clean commit `ccb234a` produced the first sealed partial schema-1 record in
`target/integration-acceptance/ccb234a/record.json`. Its two fake-provider cases
pass all 30 named checks against test-binary SHA-256
`de44a9ca9cd2cb68396e0f69054a7b46faa5aa84179ea0d414325d1af1c3a297`;
the record SHA-256 is
`c7659749b0c581fa62eac52411953840255799392de5d39a47b353cc7d6ffb55`.
Every shell, Harvest, desktop, and authenticated-provider row remains
unrecorded, so this is not a complete candidate record. The dated table above
remains implementation/history evidence and cannot fill those rows.

## Authenticated-provider procedure

Authenticated checks are manual because they can consume paid service,
transmit repository context, create provider-side conversations, or obey tool
requests. Do not run them from CI and do not infer permission from a request to
test Termeric generally.

An operator who explicitly approves the run should provide a disposable test
repository and state which provider(s), account, spending boundary, model, and
allowed external effects are in scope. For each provider:

1. record the release candidate SHA, OS/compositor, Termeric version, CLI
   version, selected profile, and redacted configuration;
2. authenticate using the provider's normal CLI flow outside Termeric;
3. open and resume a disposable conversation;
4. exercise all twelve commands, declining any unapproved filesystem,
   network, or account mutation;
5. verify transcript/export output after redacting secrets and personal data;
6. record pass, fail, unavailable, or accepted limitation for each command in
   the versioned candidate record with a redacted artifact path; and
7. revoke disposable credentials or sessions when the agreed procedure calls
   for it.

Never commit credentials, raw authentication output, private prompts, provider
conversation identifiers, or unreviewed transcripts. A `Not run` row must stay
`Not run`; CLI `--version` output and fake-executable evidence are not
substitutes for authentication.

## Promotion rule

Hermetic failures block integration changes. Authenticated and real-desktop
rows may remain named beta limitations only while the feature stays
experimental or the manual boundary is stated in user-facing documentation.
No row can be promoted to full support from parser, unit, fake-provider, or
private-compositor evidence alone.
