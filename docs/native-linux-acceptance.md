# Native Linux candidate evidence

Unit and private-compositor tests cannot establish real desktop policy, input
method behavior, screen-reader usability, font quality, fractional scaling, or
vendor-GPU behavior. Candidate evidence for those boundaries uses the
schema-versioned native acceptance record instead of free-form release notes.

## Create a candidate record

Generate one complete template from the exact candidate commit:

```bash
python3 scripts/native_acceptance_record.py template \
  --output target/native-acceptance/record.json \
  --operator "YOUR NAME"
```

The tool records the current Git SHA, Termeric version, and UTC time and refuses
to overwrite an existing record. It also refuses a dirty worktree or a supplied
SHA that does not equal checked-out `HEAD`; candidate evidence cannot represent
uncommitted source. The template contains exact cases for:

- real X11 and Wayland visual, fractional-scale, font, emoji, resize, and
  fullscreen behavior;
- Kitty OSC 66 integer/fractional text sizing, explicit-width placement,
  clipping, overwrite behavior, and expanded block/beam/underline cursors;
- clipboard/primary selection, OSC 52, URL, notification, and audible-bell
  desktop policy on both display stacks;
- lost/outdated-surface and device recovery, PTY survival, idle behavior, and
  observed GPU memory on a named vendor driver;
- IBus and Fcitx 5 CJK/dead-key/preedit/focus-cancellation behavior under X11
  and Wayland; and
- Orca/AT-SPI focus, text, updates, actions, modal isolation, and hidden-content
  behavior under X11 and Wayland.

Each case records its own distro, kernel, desktop session, backend, compositor,
GPU/driver, scale, locale, and applicable input-method or screen-reader version.
It also records `termeric --version` output and the tested executable's SHA-256
digest. This avoids treating evidence from one build or environment as proof for
another.

## Record results

Every check uses one of five exact values: `pass`, `fail`, `unavailable`,
`accepted_limitation`, or `unrecorded`. The case disposition must be the value
derived from its checks: any failure is `fail`; all-pass is `pass`; an accepted
limitation takes precedence over unavailable; and any unrecorded check keeps the
case unrecorded. Non-pass cases require explanatory notes. `unavailable` is
evidence of a missing prerequisite, never a pass.

Pass or fail checks require at least one evidence file. Keep evidence under the
record's directory and use relative paths. Useful artifacts include compositor
and Termeric logs, PTY transcripts, screen-reader speech/log output, screenshots,
short recordings, desktop-policy observations, and GPU-tool captures. Terminal
artifacts may contain secrets or private output; review and redact them before
sharing.

After adding artifact paths and descriptions, bind the files to the record:

```bash
python3 scripts/native_acceptance_record.py seal \
  target/native-acceptance/record.json
```

Sealing refuses missing files, symlinks, absolute paths, and paths outside the
record directory, then records each SHA-256 digest.

## Validate evidence

During testing, validate template structure while allowing unfinished rows:

```bash
python3 scripts/native_acceptance_record.py validate \
  target/native-acceptance/record.json --allow-incomplete
```

Validate a completed record and every local artifact/digest:

```bash
python3 scripts/native_acceptance_record.py validate \
  target/native-acceptance/record.json \
  --expected-sha "$(git rev-parse HEAD)"
```

For a release policy that requires every native row to pass rather than allowing
an explicitly reviewed limitation or unavailable prerequisite, add
`--require-pass`. The validator rejects missing/extra cases or checks,
environment placeholders, inconsistent dispositions, evidence-free pass/fail
claims, path traversal, missing files, and digest changes.

Authenticated Claude, Codex, or other provider workflows are outside this
record and must not be invoked as part of native Linux acceptance.
