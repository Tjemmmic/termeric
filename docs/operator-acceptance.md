# Operator acceptance runbook

This runbook covers the release evidence that cannot be completed by ordinary
unit tests alone. It is intentionally ordered from provider-free and private
environments to real desktops and, last, authenticated AI providers.

Authenticated Codex and Claude Code checks can consume paid tokens, transmit
repository context, and create provider-side conversations. Nothing in the
commands before the authenticated-provider section launches either provider.
Run that final section only after choosing the account, model, spending limit,
and allowed external effects yourself.

## What to install

### Arch Linux

Install the private X11/Wayland environments, desktop drivers, image clients,
full-screen applications, shells, accessibility tools, fonts, and Mesa
software-renderer support:

```bash
sudo pacman -S --needed \
  sway weston xorg-server-xvfb xorg-xauth openbox \
  wtype wl-clipboard xdotool xclip xdg-utils \
  imagemagick ffmpeg python-pillow \
  libsixel kitty vim less tmux \
  zsh fish nushell elvish \
  orca at-spi2-core ibus \
  fcitx5 fcitx5-configtool fcitx5-gtk fcitx5-qt fcitx5-chinese-addons \
  mesa mesa-utils vulkan-swrast vulkan-tools \
  noto-fonts noto-fonts-emoji \
  libnotify libpulse util-linux
```

`timg` is in the Arch User Repository rather than the official binary
repositories. Review its PKGBUILD, then install it with the AUR workflow you
already trust, for example:

```bash
paru -S timg
```

If you do not use an AUR helper, follow the upstream build or AppImage
instructions at <https://github.com/hzeller/timg#installation>. The required
result is a `timg` executable on `PATH` with iTerm2 output support.

Install the Rust fuzz runner separately if you intend to run the optional fuzz
campaigns:

```bash
cargo install cargo-fuzz
```

The package groups serve these purposes:

- Xvfb, Openbox, Sway, and Weston provide disposable native X11 and Wayland
  sessions without driving the operator's desktop.
- `xdotool`, `wtype`, `xclip`, and `wl-clipboard` inject native input and test
  compositor clipboard ownership.
- ImageMagick, FFmpeg, Pillow, fonts, and the desktop helpers capture evidence
  and exercise URL, notification, and bell paths.
- Kitty's `kitten icat`, `img2sixel`, and `timg` are independent graphics
  clients for Kitty, Sixel, and iTerm2 protocol acceptance.
- Vim, less, and tmux exercise alternate-screen and application-input behavior.
- Zsh, Fish, Nushell, and Elvish complete the shell-integration matrix.
- IBus, Fcitx 5, Orca, and AT-SPI provide the real IME and screen-reader gates.
- Mesa's Vulkan/GL software paths prove the fallback renderer does not silently
  use the installed hardware GPU.

### Debian or Ubuntu

Package availability varies by release, especially for Nushell, Elvish, and
`timg`. Install the available native dependencies with:

```bash
sudo apt-get update
sudo apt-get install \
  xvfb xauth openbox sway weston \
  xdotool xclip wtype wl-clipboard xdg-utils \
  imagemagick ffmpeg python3-pil \
  libsixel-bin timg kitty vim less tmux util-linux \
  zsh fish nushell elvish \
  orca at-spi2-core ibus fcitx5 fcitx5-chinese-addons \
  mesa-vulkan-drivers mesa-utils vulkan-tools \
  fonts-dejavu-core fonts-noto-core fonts-noto-color-emoji \
  libnotify-bin pulseaudio-utils
```

Use the shell or image client's upstream installation instructions if the
distribution release does not provide its package. A missing executable is an
`unavailable` acceptance result, not a pass.

Confirm the important executables before starting:

```bash
for tool in Xvfb openbox sway swaymsg weston wtype wl-copy wl-paste \
  xdotool xclip convert ffmpeg img2sixel timg kitten vim less tmux \
  zsh fish nu elvish orca; do
  command -v "$tool" || echo "MISSING: $tool"
done
```

## Test order

Use an exact, committed candidate. Acceptance-record generators reject a dirty
worktree because uncommitted evidence cannot identify what was tested.

```bash
git status --short
git rev-parse HEAD
cargo build -p termeric --locked
sha256sum target/debug/termeric
```

Keep each generated artifact directory intact. Review terminal transcripts and
screenshots for secrets before sharing or committing them.

### 1. Provider-free baseline

These checks do not authenticate, contact an AI service, or consume AI tokens:

For a clean committed candidate, prefer the fixed-command evidence runner:

```bash
cargo fetch --locked
python3 scripts/release_preflight.py run \
  --output target/release-preflight
python3 scripts/release_preflight.py validate \
  target/release-preflight/record.json
```

It records the complete gate logs and release-binary digest while removing
provider opt-ins from child processes. See
[`release-preflight.md`](release-preflight.md). The individual commands below
remain useful for development diagnosis.

```bash
cargo fmt --all -- --check
cargo test --workspace --locked
cargo build --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
python3 scripts/check_markdown_links.py
python3 -m unittest scripts.test_check_markdown_links
python3 scripts/check_feature_support.py
python3 -m unittest scripts.test_check_feature_support
```

Run the fake AI executable workflow explicitly. It exercises all twelve
Claude/Codex integration commands through a real local PTY but cannot reach a
provider:

```bash
cargo test -p termeric \
  fake_ai_clis_exercise_every_builtin_command_end_to_end -- --nocapture
```

For a clean candidate, also produce the sealed partial record directly:

```bash
python3 scripts/run_fake_provider_acceptance.py \
  --output target/integration-acceptance/record.json \
  --operator "Release Operator"
```

This command executes only the exact fake-provider test and leaves every
shell, desktop, Harvest, and authenticated-provider row unrecorded.

Do not set `TERMERIC_REAL_CODEX_CONTRACT` in this lane. That separate opt-in
probe launches the installed CLI for `--version` and `--help`; it still is not
authenticated-provider acceptance.

### 2. Shell integration

```bash
TERMERIC_REQUIRE_SHELL_ACCEPTANCE=1 \
  cargo test -p termeric \
  supported_shells_preserve_user_rc_and_emit_multiline_exit_regions \
  -- --nocapture

TERMERIC_REQUIRE_NUSHELL_ACCEPTANCE=1 \
  cargo test -p termeric \
  nushell_native_integration_preserves_user_config_and_emits_regions_when_available \
  -- --nocapture

TERMERIC_REQUIRE_ELVISH_ACCEPTANCE=1 \
  cargo test -p termeric \
  elvish_module_preserves_user_config_and_emits_regions_when_available \
  -- --nocapture
```

The first command requires Bash, Zsh, and Fish. The next two fail rather than
skip when `nu` or `elvish` is unavailable. They use isolated homes and real
PTYs. They do not launch any AI provider. Elvish's acceptance fixture installs
the documented `use termeric-integration` import in its isolated rc file.

### 3. Renderer hardware and software paths

Run the strict software fallback first:

```bash
WGPU_BACKEND=vulkan TERMERIC_GPU_SMOKE=1 \
  cargo test -p termeric --locked \
  linux_headless_gpu_render_smoke_when_requested -- --nocapture
```

If the host has no usable software Vulkan ICD, use Mesa llvmpipe through
surfaceless OpenGL:

```bash
WGPU_BACKEND=gl EGL_PLATFORM=surfaceless LIBGL_ALWAYS_SOFTWARE=true \
TERMERIC_GPU_SMOKE=1 cargo test -p termeric --locked \
  linux_headless_gpu_render_smoke_when_requested -- --nocapture
```

The log must identify a CPU adapter. Then test the vendor GPU explicitly:

```bash
WGPU_BACKEND=vulkan TERMERIC_GPU_SMOKE=1 TERMERIC_GPU_SMOKE_HARDWARE=1 \
  cargo test -p termeric --locked \
  linux_headless_gpu_render_smoke_when_requested -- --nocapture
```

Retain the adapter name, backend, driver, test log, and any WGPU validation
messages. A software pass is not vendor-GPU recovery evidence, and a hardware
pass is not proof that fallback rendering works.

### 4. Private X11 and Wayland GUI acceptance

The X11 run covers normal workspace behavior, fullscreen, CLI startup, Vim,
less, tmux, clipboard/OSC integration, and the installed `img2sixel` and
`timg -pi` clients:

```bash
WGPU_BACKEND=gl EGL_PLATFORM=x11 \
xvfb-run -a -s "-screen 0 1280x800x24" \
python3 scripts/gui_acceptance.py \
  --backend x11 \
  --start-window-manager \
  --force-surface-loss \
  --force-surface-outdated \
  --force-device-loss \
  --skip-visual-checks \
  --binary target/debug/termeric \
  --artifacts target/gui-acceptance/x11
```

Run the three private Wayland paths:

```bash
WGPU_BACKEND=gl python3 scripts/gui_acceptance.py \
  --backend wayland --start-weston \
  --binary target/debug/termeric \
  --artifacts target/gui-acceptance/wayland

WGPU_BACKEND=gl python3 scripts/gui_acceptance.py \
  --backend wayland --start-sway --scenario fullscreen \
  --binary target/debug/termeric \
  --artifacts target/gui-acceptance/wayland-fullscreen

WGPU_BACKEND=gl python3 scripts/gui_acceptance.py \
  --backend wayland --start-sway --scenario interaction \
  --force-surface-loss \
  --force-surface-outdated \
  --force-device-loss \
  --binary target/debug/termeric \
  --artifacts target/gui-acceptance/wayland-interaction
```

Every selected scenario must report a pass in its `results.json`; a missing
tool or scenario is not a silent skip. These private-compositor runs prove the
technical input, PTY, surface, and renderer paths, not the policies of a real
desktop session.

#### Deferred real-window command launch and wait

For the background mode added in protocol v18, rebuild both the GUI and CLI.
Keep a tab with split panes selected and start a local `launch-tab --background`
from another terminal. Confirm the original tab/pane remains selected, including
an in-progress mouse selection, search, copy mode and IME composition (test these
separately), while a new tab appears immediately even for a silent child. Select
the new tab explicitly to inspect its output. Repeat with `--wait`, confirming
the child's exit status returns without selecting the background tab. No hidden
window or detached process is expected; closing the owning GUI still closes its
PTYs. Use synthetic local commands only, not a provider program.

This manual row is separate from automated fake-endpoint coverage. Run it later
in a disposable ordinary Termeric window, with another window kept alive.
`target/debug/termeric control list --json` supplies the actual `PROCESS` and
`WINDOW` runtime IDs; substitute them below. Use only the listed local commands,
not a provider program.

```bash
target/debug/termeric control launch-tab PROCESS WINDOW --wait --timeout 10 --json -- /bin/sh -c 'exit 7'
target/debug/termeric control launch-window PROCESS WINDOW --wait --timeout 10 --json -- /bin/true
target/debug/termeric control launch-tab PROCESS WINDOW --wait --timeout 1 --request-id 10900000000000000000000000000001 --json -- /bin/sleep 5
```

Expect one final status JSON and CLI exit code 7 for the first, and code 0 for
the second. Confirm the requested tab/window actually appears. The third should
time out with no stdout JSON, retain its ID in the diagnostic, and leave the
sleeping child alone. Then use
`control wait-launch PROCESS 10900000000000000000000000000001 --timeout 10 --json`
to observe that same child's eventual successful exit. Use a fresh 32-digit
lowercase-hex ID for each new logical test, not a retained ID from a prior run.

Repeat with an explicit request ID and Ctrl+C in the standalone waiting CLI;
query that ID afterward and confirm no cancellation/relaunch occurred. Check
the two-command `launch-status`/`wait-launch` workflow and no-wait launch output
still work. Record source commit, compositor, target IDs, command/result and
visible behavior. Do not promote the running-GUI control row based only on the
hermetic tests. See [the control contract](gui-control.md).

For `--env-stdin`, create a private test file containing only the synthetic
document `{"TERMERIC_TEST_INPUT":"synthetic-only","EMPTY":""}`. Redirect
that file into each launch target with `--env-stdin`, once with and once without
`--wait`, launching a local command that checks the synthetic variable and
returns success without printing its value. Check the visible target and final
exit status, then try duplicate keys, an unset/set collision and stdin from the
interactive terminal; these must fail before creating a target. Confirm the
source GUI environment and a later ordinary tab are unchanged. Do not use a
real provider credential or program. The five-second stdin collection deadline
precedes, and is separate from, any launch/wait `--timeout`. See the
[input and exposure contract](gui-control.md#environment-input-without-literal-values-in-argv).

### 5. Independent graphics clients on real desktops

On both a real X11 session and a real Wayland session, open Termeric in this
repository and use `assets/icons/termeric.png` as the fixture. Run and visually
inspect each command:

```bash
kitten icat --transfer-mode=stream assets/icons/termeric.png
kitten icat --unicode-placeholder --transfer-mode=stream \
  assets/icons/termeric.png
kitten icat --transfer-mode=file --stdin=yes < assets/icons/termeric.png
kitten icat --transfer-mode=memory assets/icons/termeric.png
img2sixel assets/icons/termeric.png
timg -pi assets/icons/termeric.png
```

For every `kitten icat` case, success means the image is visible, the prompt
returns normally, and neither raw `ESC_G` text nor `E2BIG`/`EINVAL` graphics
errors appear in the terminal or become shell input. The stream case exercises
the reference client's 128-KiB APC chunks; stdin-backed file mode exercises
raw-unpadded base64; memory mode exercises its bare POSIX shared-memory name.

For Kitty ordinary-file transfer, first verify that the same existing-filename
request is rejected with the default security policy. Then temporarily set
`allow_kitty_file_transfers = true` under `[security]`, restart the candidate,
verify it succeeds, and restore the default-deny setting. Also repeat the
stream and Unicode-placeholder commands concurrently in two panes and verify
that images and replies stay pane-local.

For full Kitty promotion, additionally follow the relative-placement and
multi-frame animation probes in [kitty-graphics.md](kitty-graphics.md). Record
the exact Kitty version, command, image, candidate SHA, compositor, GPU, config
state, visible result, and PTY reply. For Sixel/iTerm2, verify color, aspect,
cursor placement, resize, scrollback, pane clipping, and cleanup. These are the
manual gates for issues #12 and #31.

### 5a. Kitty terminal file-transfer client

This row is deliberately separate from Kitty graphics: the similarly named
`allow_kitty_file_transfers` setting controls graphics `t=f` reads and has no
effect on OSC 5113. No configuration change is needed for the transfer test;
each session must display its own approval dialog.

From a shell inside the candidate Termeric window, use expendable paths and run
the current Kitty client in normal/simple mode:

```bash
probe_root="$(mktemp -d)"
printf 'termeric kitty transfer\n' > "$probe_root/source.txt"
mkdir "$probe_root/source-tree"
mkdir "$probe_root/received-root"
printf 'nested\n' > "$probe_root/source-tree/nested.txt"
printf 'host upload\n' > "$probe_root/host-source.txt"

kitten transfer --compress=never \
  "$probe_root/source.txt" "$probe_root/received.txt"
kitten transfer --compress=always \
  "$probe_root/source-tree" "$probe_root/received-root/"
kitten transfer --direction=upload --compress=never \
  "$probe_root/host-source.txt" "$probe_root/child-received.txt"
```

Approve each request. Verify the prompt identifies the originating tab/pane,
progress remains responsive, the commands return normally, and the copied
bytes/tree match. Repeat one transfer from a non-focused pane and, if used,
from an attached live-domain pane; replies and the dialog must remain bound to
the origin. Deny another request and confirm the client reports `EPERM` and no
destination appears. Pre-create a destination with sentinel content, retry,
and confirm Termeric reports a collision without changing it. Start a larger
transfer, cancel it, and confirm no destination is published.

When you resume native testing, also exercise owner lifetime in both directions:
keep the requesting pane while splitting/collapsing its layout and confirm that
the same transfer continues there, even with a sibling focused. Remove the
requester while approval is pending or data is in flight; no reply or file data
may appear in the surviving/replacement pane, and the old approval must not
authorize it. Repeat with a nonzero surviving pane. Hermetic owner-routing
regressions cover these transitions, but native acceptance is still deferred.

For the upload command, the approval must list the exact resolved
`host-source.txt` path before any read occurs. Verify `child-received.txt`
matches byte-for-byte. Repeat with two regular files, then separately request a
directory, a symbolic link, a FIFO, a duplicate path, and a path containing
`..`; each must fail without returning contents. Request more than 64 MiB and
more than 16 paths and confirm bounded refusal. Cancel during a multi-chunk
regular file and confirm the originating client receives no later payload.

Record candidate SHA, `kitten --version`, compositor, commands, source and
destination hashes, dialog result, and any protocol error. Quiet levels 1/2,
`--permissions-bypass`, `--transmit-deltas`, receive-side directory/link
export, overwrite, and resume are outside the supported profile and should fail
explicitly. The feature matrix stays Partial until both direction rows have
accepted real-client evidence.

### 6. Resource and optimization evidence

Run the release benchmark suite and retain its JSON:

```bash
WGPU_BACKEND=vulkan python3 scripts/run_benchmarks.py \
  --output target/benchmarks/latest.json
```

Run the short core and displayed change gates:

```bash
WGPU_BACKEND=vulkan python3 scripts/run_soak.py \
  --duration-seconds 30 --gpu-smoke \
  --output target/soak/local.json \
  --log target/soak/local.log
python3 scripts/validate_soak.py target/soak/local.json --source .

WGPU_BACKEND=gl python3 scripts/run_native_gui_soak.py \
  --duration-seconds 30 \
  --require-clean \
  --binary target/debug/termeric \
  --artifacts target/native-gui-soak
python3 scripts/validate_native_gui_soak.py \
  target/native-gui-soak/results.json --source .
```

For a candidate, repeat both soaks for 600 seconds. Keep the reports and logs:

```bash
WGPU_BACKEND=vulkan python3 scripts/run_soak.py \
  --duration-seconds 600 --gpu-smoke \
  --output target/soak/release.json \
  --log target/soak/release.log
python3 scripts/validate_soak.py target/soak/release.json --source .

WGPU_BACKEND=gl python3 scripts/run_native_gui_soak.py \
  --duration-seconds 600 \
  --require-clean \
  --binary target/debug/termeric \
  --artifacts target/native-gui-soak-release
python3 scripts/validate_native_gui_soak.py \
  target/native-gui-soak-release/results.json --source .
```

The pass criteria include peak and growth RSS, thread/file-descriptor/child
growth, complete child cleanup, bounded and fully drained transcript queues,
continuous PTY and graphics progress, pane routing, resize churn, normal exit,
and absence of GPU validation/device-loss errors. Schema-3 core and schema-5
displayed reports seal their artifacts, independently reconstruct their checks,
and reconcile every aggregate thread/descriptor/child count
with privacy-safe categories and summarize category quartile growth and peaks,
so a failed ceiling remains diagnosable without publishing raw paths or process
arguments. The displayed report also records standardized process-attributed
DRM total/resident memory or an explicit bounded unavailable reason for every
sample. These artifacts address the
local portion of issue #23; green pushed CI history and long real-desktop
vendor-GPU observation remain separate evidence.

On a vendor-GPU host, repeat the displayed 600-second command with a new
artifact directory and `--require-gpu-memory`. This makes missing or intermittent
DRM fdinfo coverage fail rather than pass as optional software-runner evidence.
Use `--max-gpu-resident-mib` and `--max-gpu-resident-growth-mib` only to apply a
documented runner-class budget; retain the defaults until repeated artifacts
justify a tighter ceiling.

### 7. Real-desktop, IME, screen-reader, and vendor-GPU record

Create a template from the clean candidate:

```bash
python3 scripts/native_acceptance_record.py template \
  --output target/native-acceptance/record.json \
  --operator "YOUR NAME"
```

The record itself names every required X11 and Wayland check for:

- visual quality, fonts, emoji, ligatures, fractional scale, resize, and
  fullscreen;
- clipboard, primary selection, OSC 52, URL, notification, and audible bell;
- vendor adapter selection, surface/device recovery, PTY survival, idle work,
  and observed GPU memory;
- IBus and Fcitx 5 CJK/dead-key preedit, one-time commit, split-pane routing,
  and focus cancellation; and
- Orca window/tab/pane identity, visible terminal semantics, announcements,
  F6 chrome navigation, modal isolation, actions, and exclusion of hidden
  content.

For deferred OSC 99 notification checks, show an identified notification before
splitting/collapsing its owner. Replacement, close, alive queries, button reports,
and focus actions must still address that same surviving pane, including a
nonzero survivor. Remove the notifying pane and verify that delayed activation
or closure cannot focus or write to its replacement. Report-only actions must
leave focus alone. These checks have hermetic ownership coverage but still need
real notification-daemon and compositor acceptance when native testing resumes.

Run the candidate under real X11 and Wayland sessions, fill every check with
`pass`, `fail`, `unavailable`, or `accepted_limitation`, and attach evidence for
pass/fail results. Validate while working:

```bash
python3 scripts/native_acceptance_record.py validate \
  target/native-acceptance/record.json --allow-incomplete
```

After reviewing artifacts for secrets, seal and validate the finished record:

```bash
python3 scripts/native_acceptance_record.py seal \
  target/native-acceptance/record.json
python3 scripts/native_acceptance_record.py validate \
  target/native-acceptance/record.json \
  --expected-sha "$(git rev-parse HEAD)"
```

Use `--require-pass` only when release policy requires every native row to
pass. This record supplies the remaining evidence for issues #17, #32, and #33.

## Deferred Harvest rerun isolation (no provider)

When convenient, enable Harvest in a disposable workspace with three ordinary
shell panes and run a harmless marked command such as
`printf 'harvest-rerun-probe\n'` in one pane. Select another pane, activate
broadcast, then use the original command's Re-run action. Only its source pane
should receive the rerun; the other panes must retain their input and broadcast
membership. Repeat with the source excluded from the two selected broadcast
targets. An armed-but-unconfirmed broadcast or read-only source must refuse the
action without a sent/success notice. Confirm that aliases and completion do not
replace the historical command. Record native menu/focus/toast behavior; the
retained-command path should also preserve quoted double spaces after live text
is cleared. A command whose retained text exceeds 4 KiB must show a disabled
Re-run action with a truncation reason, not execute a shortened caption. The
raw-PTY regression evidence is not a native UI acceptance substitute. Do not
run destructive commands to test confirmation; automated fixtures capture those
bytes without evaluating them. This lane does not use Harvest Explain.

Also retain a harmless command from a single terminal, split it, select its
sibling, and use the old entry's Search, Scroll-to and Re-run actions. They
should still select the original terminal. Close the sibling and repeat after
another split; newly recorded commands should remain under the same source
without duplicates. Repeat while keeping a newer, nonzero pane as the sole
survivor. Closing the actual source must disable its live actions instead of
redirecting them to a replacement. These visual checks can be done later;
ownership and deferred-confirmation byte routing have local regression coverage.

## Deferred picker destination checks (no provider)

In disposable ordinary shell panes, open **Insert Unicode Character...** and
select a harmless character such as `λ`. If the original owner is still focused
after a split/collapse, it should receive the character once. If its sibling is
focused or the original pane was closed, completion must insert nothing into the
sibling. Read-only, broadcast and cancelled states must also refuse insertion.
Check keyboard/mouse selection, the refusal notice and unchanged sibling input
when convenient; automated byte-capture checks are not native visual acceptance.

Context routing has separate local-fixture coverage for plugin-owner selection,
working directory, asynchronous completion and split/collapse refusal. Testing
context insertion with a real AI CLI remains part of the explicitly authorized
provider section below, not this provider-free manual check.

Direct plugin Context, Clear, Copy, and Jump to Error also have provider-free
raw-shell coverage for mixed-pane ownership. When you choose to test real
plugins later, keep a shell sibling focused and verify that Context/Clear/Copy
still address only the plugin pane; Jump to Error should deliberately focus that
pane and leave the sibling's scroll position alone. A read-only plugin pane must
refuse Context/Clear without redirecting to a writable sibling. These are later
operator checks, not permission for an automated provider run.

## Authenticated Codex, Claude Code, and Harvest Explain

This section is deliberately last. It is a manual test plan, not authorization
for an automated agent or CI job to run a provider.

### Preparation and consent

1. Use a disposable repository containing no secrets or private history.
2. Decide which provider account, model, and maximum spend are allowed.
3. Decide whether edits inside the disposable repository are allowed; deny all
   other filesystem, network, account, and process effects.
4. Authenticate each CLI outside Termeric using its normal login flow.
5. Record only the CLI version and a redacted configuration summary. Never
   record credentials, raw login output, conversation identifiers, private
   prompts, or unreviewed transcripts.
6. Start with conservative Termeric settings. Codex is disabled by default;
   enable it explicitly. Claude Code defaults to enabled, but declaring both is
   clearer for an acceptance profile:

```toml
[plugins.claude_code]
enabled = true
binary = "claude"
inherit_cwd = true
copy_lines = 50

[plugins.codex]
enabled = true
binary = "codex"
inherit_cwd = true
approval_mode = "suggest"
sandbox_mode = "read-only"
copy_lines = 50

[harvest]
enabled = true
show_indicators = true
max_history = 100
```

Use `workspace-write` only for the planned disposable-repository diff test.
Do not use Claude's `auto-accept`, Codex `full-auto`, or `full-access` for this
acceptance run.

### Per-provider checklist

Open the command palette with Ctrl+Shift+P. Repeat the following for Claude
Code and Codex, recording expected and actual behavior separately:

1. **Open**: launch from a nested directory. Confirm the plugin tab has the
   exact cwd, terminal size, selected profile/model, and visible provider
   identity. Send a harmless unique phrase and receive one response.
2. **Resume**: close the plugin tab, run Resume, and confirm the prior unique
   phrase remains known. A new unrelated conversation is a failure.
3. **Copy Last Response**: confirm the clipboard receives only the configured
   recent response lines from the correct provider tab.
4. **Clear Session**: start a long response, clear it, and verify cancellation
   is immediate, output is cleared, and input is not delivered to another pane.
5. **Restart**: restart from a split-pane layout. Confirm the old PTY exits and
   the replacement keeps the same provider, profile, and cwd without targeting
   a tab that is still closing.
6. **Review Diff**: with `workspace-write` explicitly approved, ask the provider
   to make one harmless, predetermined edit in the disposable repository.
   Confirm the review overlay shows that exact diff, supports keyboard/pointer
   scrolling, and does not mutate or stage anything merely by reviewing it.
7. **Copy Diff**: confirm the clipboard contains that exact diff and no
   unrelated terminal output or secrets.
8. **Insert Context**: select one small disposable file and one bounded terminal
   output source. Verify preview, size budget, cancellation, and insertion into
   the exact provider pane captured when the picker opened, even if focus moves
   before completion.
9. **Toggle Transcript**: start recording, exchange one unique prompt/response,
   then stop. Verify visible recording state, bounded JSONL output, correct
   provider ownership, and error reporting for an unwritable destination.
10. **Export Session**: export Markdown and preserve JSONL. Review both for
    completeness, ordering, automatic redaction behavior, and accidental
    secrets before retaining either artifact.
11. **Jump to Last Error**: produce or locate a known provider error after
    enough output to scroll. Confirm the command selects the correct plugin tab
    and lands on that error without disturbing another pane.
12. **Select Profile**: configure two harmless profiles, switch between them,
    and confirm the selected profile is used for open, restart, and resume. A
    missing or removed profile must fail visibly rather than silently using a
    default.

Also test two providers in different panes and windows. While a picker, diff,
or transcript action is pending, change focus and close/reopen unrelated panes;
completion must remain bound to the original stable destination or fail closed.
Decline one provider-requested action and verify the denial is represented
without bypassing the CLI's own approval controls.

For **Harvest Explain**, run a harmless marked shell command, open its exact
Root Cellar entry, choose Explain, and confirm the UI identifies what command
and bounded output will be sent before provider execution. Cancel once and
verify that no request is made. Then explicitly approve one request within the
chosen spending boundary and verify its response remains attached to the
originating command, tab, and pane. Repeat after changing focus; stale or
evicted entries must fail closed.

### Record and validate provider evidence

Generate the integration record from the same clean candidate:

```bash
python3 scripts/integration_acceptance_record.py template \
  --output target/integration-acceptance/record.json \
  --operator "YOUR NAME"
```

Before any authenticated check can be `pass` or `fail`, set that provider's
approval object to `granted` and record the approver, UTC time, redacted account
scope, spending limit, allowed external effects, and credential-cleanup plan.
For each provider artifact, use a relative path and set
`reviewed_for_secrets` to `true` only after manual review.

```bash
python3 scripts/integration_acceptance_record.py validate \
  target/integration-acceptance/record.json --allow-incomplete
python3 scripts/integration_acceptance_record.py seal \
  target/integration-acceptance/record.json
python3 scripts/integration_acceptance_record.py validate \
  target/integration-acceptance/record.json \
  --expected-sha "$(git rev-parse HEAD)"
```

Revoke disposable credentials or sessions if that was the approved cleanup
plan. Authenticated provider evidence completes issue #26 only when both
provider cases, their twelve commands, redaction review, and unapproved-effect
denial are recorded. A CLI version, fake executable pass, unrecorded row, or
token-free parser probe is not a substitute.

## Completion summary

The remaining acceptance gates should be closed in this order:

1. private software/hardware renderer and GUI lanes;
2. shell matrix and independent Kitty/Sixel/iTerm2 clients;
3. short and ten-minute core/displayed soaks;
4. real X11/Wayland visual, policy, vendor-GPU, IME, and Orca record;
5. pushed CI artifacts on the committed workflow; and
6. authenticated provider and Harvest Explain record, only with explicit
   operator consent and budget.

Do not collapse `unavailable`, `accepted_limitation`, private-compositor,
fake-provider, or local dirty-tree evidence into a full release pass.
