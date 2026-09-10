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

WGPU_BACKEND=gl python3 scripts/run_native_gui_soak.py \
  --duration-seconds 30 \
  --require-clean \
  --binary target/debug/termeric \
  --artifacts target/native-gui-soak
```

For a candidate, repeat both soaks for 600 seconds. Keep the reports and logs:

```bash
WGPU_BACKEND=vulkan python3 scripts/run_soak.py \
  --duration-seconds 600 --gpu-smoke \
  --output target/soak/release.json \
  --log target/soak/release.log

WGPU_BACKEND=gl python3 scripts/run_native_gui_soak.py \
  --duration-seconds 600 \
  --require-clean \
  --binary target/debug/termeric \
  --artifacts target/native-gui-soak-release
```

The pass criteria include peak and growth RSS, thread/file-descriptor/child
growth, complete child cleanup, bounded and fully drained transcript queues,
continuous PTY and graphics progress, pane routing, resize churn, normal exit,
and absence of GPU validation/device-loss errors. These artifacts address the
local portion of issue #23; green pushed CI history and long real-desktop
vendor-GPU observation remain separate evidence.

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
