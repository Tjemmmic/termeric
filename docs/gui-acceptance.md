# Native GUI and full-screen acceptance

Termeric has a deterministic Linux black-box harness for the boundary that
unit tests cannot cover: native window events, PTY input/output, desktop
clipboard ownership, window resizing, full-screen applications, and an actual
WGPU presentation surface. It always runs with disposable XDG configuration,
data, cache, home directories, shell fixtures, private tmux runtimes/sockets,
and documents. Its isolated font profile uses DejaVu Sans Mono as the primary
and the separately installed Noto Color Emoji as a distinct fallback, matching
the same duplicate-family validation applied to user configuration.
Vim disables viminfo and user configuration, less disables user options,
preprocessors, and history, and tmux uses an empty configuration explicitly.
Private Weston and Sway sessions use owned, mode-0700 runtime directories under
the system temporary directory so Wayland, Sway IPC, and Termeric control
sockets remain below Unix path-length limits even when the artifact directory
is deeply nested. The harness removes those runtime directories after stopping
the compositor.

## What runs

The X11 lane starts Xvfb and a minimal window manager, then drives the real
Termeric binary with `xdotool`. Its workspace scenario asserts:

- non-zero character and pixel PTY geometry at startup and after two resizes;
- typed input through the native keyboard event path;
- Unicode fixture output through an installed native color-emoji font (the
  deterministic dual-atlas/shader assertion runs in the headless GPU lane);
- F6/Tab/arrow/Enter/Escape keyboard reachability for tab-bar controls;
- search input plus a mouse hit on a search control;
- keyboard-only copy-mode search, selection, clipboard yank, and proof that
  modal navigation bytes never reach the PTY;
- a keyboard quick-select hint opening an OSC 8 link through the desktop spy;
- a mouse selection copied through the X11 clipboard;
- an OSC 52 clipboard write/read round trip with an exact reply in the
  originating PTY transcript;
- an OSC 8 link opened through an isolated `xdg-open` spy;
- BEL visual feedback plus an isolated `paplay` audio invocation;
- an OSC 777 desktop notification through an isolated `notify-send` spy;
- SGR mouse reports and one framed bracketed-paste write;
- tab creation and direct drag reorder, pane split/focus/close routing, nested
  layout interaction, and settings mouse hit-testing;
- complete workspace save, manager rename, tab/pane relaunch, and focused input;
- same-process second-window creation/focus/input/close with the first window
  remaining live;
- a Kitty image receiving a successful reply through the native PTY route,
  paired with the GPU image-geometry smoke;
- current `kitten icat` classic-stream and Unicode-placeholder-stream clients
  rendering a deterministic multi-row PNG at separate live cursor positions,
  with exact image IDs, client versions, accepted-protocol diagnostics, and a
  dedicated post-client native capture;
- current `kitten icat` stdin-backed temporary-file and POSIX shared-memory
  transfers at separate live positions, with fixed image IDs, exact accepted-
  protocol diagnostics, validated client target forms, and observed target
  removal after PTY delivery;
- independent `timg -pk` Kitty PNG output with its client-generated image ID
  extracted from the APC stream and bound to an exact acceptance diagnostic;
- real `img2sixel` and `timg -pi` clients emitting Sixel and iTerm2 PNG output
  through the PTY into the shared, pane-clipped GPU image path;
- a daemon-owned Kitty image created before closing the first live-domain
  window, followed by protocol-v9 decoded-state restoration and a review-only
  native capture in a newly attached window before later PTY output;
- injected `SurfaceError::Lost` and `SurfaceError::Outdated` states, each
  followed by real resize recovery;
- an injected unexpected device loss, whole-renderer recreation, and continued
  PTY/pane interaction without restarting a shell;
- an ICCCM window-manager close and clean process exit.

Separate scenarios start installed Vim, less, and tmux. Vim must persist typed
text, report a zero exit status, and leave Termeric responsive to a native close;
less must navigate and exit successfully, and tmux must expose typed text through
an independent `capture-pane` query before exiting. Each client runs through a
bounded util-linux `script` PTY recorder, and the harness requires an
alternate-screen entry followed by its matching exit in the recorded output.
This proves the alternate-screen, application-input, return-to-terminal, and
shutdown paths rather than inferring them from a screenshot or merely replaying
captured output. The raw per-client `*-screen.bin` files remain in the scenario
artifact directory for inspection.

The CLI scenario launches the real binary with a Unicode cwd, two tabs, a
split, arbitrary fixture argv after `--`, explicit 820x540 physical geometry,
and a custom X11 class/application ID. It verifies all three PTYs start in the
chosen directory and reads native window geometry and identity back from X11.

The command spies prevent the automated scenario from opening a real browser,
playing sound, or contacting a notification service. The captured
`desktop-events.tsv` file records exact requested arguments.

The Wayland lane starts a private headless Weston compositor with its kiosk
shell. It requires window creation, renderer initialization, a PTY fixture and
first terminal render, then verifies orderly application cleanup when the last
PTY exits. Desktop-policy behavior is intentionally not inferred from this
private compositor.

A separate headless Sway interaction scenario uses its virtual-keyboard
protocol and `wl-clipboard` clients to prove native keyboard delivery, a
compositor clipboard copied from keyboard-only copy mode, exact bracketed paste,
split-pane creation/focus/closure, PID-scoped input routing, surviving-pane
input, and native close. The scenario also injects one lost and one outdated
surface, changes and restores the private output size after each, injects one
unexpected device loss, and requires all three recoveries before continuing
those PTY checks. It records the
real PTY transcript and application/compositor logs. This proves the
private-compositor technical path, not the
permissions or presentation policy of a user's desktop.

The Wayland interaction lane now requires thirteen palette query-editing cases,
recorded in schema-2 `interaction/palette-input.json`. Native Ctrl+Delete repairs
a command palette filter to open the Unicode picker, then cases exercise
Home/End, Left/Right, middle insertion/Delete, and Backspace over combining,
ZWJ, and flag graphemes. Six additional cases exercise Ctrl+Left/Right,
Ctrl+Delete, and Ctrl+Backspace over combining, ZWJ, and flag sequences.
Every case must insert exactly one U+2603 snowman into its selected pane, with
no query text, editing escapes, or duplicate bytes in either pane. The cases
alternate between two real fixture PTYs, join fragmented reads, and fence each
owner after insertion before exact stream comparison. A persistent private
keyboard and zero unrelated focus losses prevent focus teardown from masking
input behavior. The temporary sibling must close and the original accept
input afterward. Checker tests reject wrong owners, wrong scalars, missing
fences, leaked keys, and duplicates. This is native keyboard/PTY evidence, not
IME composition, screen-reader, query-viewport pixels, or real-desktop evidence.

Clean local commit `2f7399d` passed this expanded private-Sway interaction lane
with GL software rendering and all three renderer-recovery injections enabled.
`/tmp/termeric-native-word-2f7399d/results.json` records candidate eligibility,
the schema-2 thirteen-case palette proof, and both artifact seals. Independent
offline reconstruction passed after the run. This supplies native Ctrl word-edit
delivery evidence for the command/Unicode palette path; it does not promote
domain/search editing, pointer placement, native IME, or vendor-GPU acceptance.

The same Wayland interaction scenario now records six held-button mouse cases
in `interaction/mouse-ownership.json`: ordinary local-tab/local-pane drags,
switch-before-release, and switch-before-motion. Ordinary drags require an exact
press/motion/release sequence at the original PTY; interrupted drags must not
deliver reports to the replacement owner. The oracle joins fragmented reads,
uses post-release per-owner input fences, and rejects malformed reports,
missing ordinary-drag events, or unrelated native focus loss. It keeps the
private keyboard attached during the matrix and verifies the original terminal
still accepts input after each temporary sibling closes. A pre-fix `5f64bdb`
negative control fails specifically on replacement-tab release leakage. This
does not establish X11, live-domain, or real-desktop held-button behavior.

The schema-3 `results.json` binds every run to the tested binary's SHA-256
digest, UTC time, compositor launcher, scenario outcomes, and exact lost-
surface, outdated-surface, and device-loss injection flags.
When more than one fault is requested, the child receives three initially
absent trigger paths instead of three immediately active faults. The harness
creates each inert marker only after the preceding recovery is observed and
nudges one compositor resize, so every log entry and recovery is independently
attributable. Renderer recreation cannot replay an already consumed trigger.

## Fullscreen promotion row

The native harness now creates two same-process windows and drives F11 into one
of them under an EWMH-capable Openbox X11 session and a headless Sway Wayland
session. It requires the target window/PTY to expand on at least one pixel axis
without shrinking the other, return to its exact character and pixel geometry
on exit, and leave its sibling window/PTY
unchanged. Request diagnostics and two-window close behavior are retained with
the transcript. Clean local commit `f9ccc2d` passed the private-Sway scenario;
the capability remains Partial until the X11 counterpart and accepted CI
history are green. Source presence alone is not acceptance evidence.

## Local software-renderer evidence at `7df2d38`

On 2026-09-16 UTC (2026-09-15 US/Central), clean local source
`7df2d38912fe29514e7e53358d783db17811913d` passed the private-Sway
interaction and fullscreen scenarios. Both used the debug executable with
SHA-256 `0f5ceaee3a3064eb283ab74ca6d473696c8a759d9cf62c737e437e490589a8b3`.
The host was Linux 7.2.4-arch1-2, Sway 1.12, and OpenGL llvmpipe
(LLVM 22.1.8, CPU adapter), with `WLR_RENDERER=pixman` and
`LIBGL_ALWAYS_SOFTWARE=true`. This is not vendor-GPU evidence.

- Interaction passed keyboard input, keyboard copy mode, compositor clipboard,
  exactly-once bracketed paste, exact split-pane PID routing/closure, and native
  close. Lost surface, outdated surface, and device loss were each injected
  once and recovered before the PTY assertions continued.
- Fullscreen passed reversible character/pixel geometry and sibling-window
  isolation. No faults were injected in this separate scenario.
- The opt-in surfaceless OpenGL GPU smoke passed its Unicode, emoji, atlas,
  SGR, ligature, and 52-row classic-image/Unicode-placeholder readback checks.
  The GUI signature comparison passed without changing the baseline. Its saved
  PNG is the small text-signature artifact, **not** a native image-client
  screenshot or a capture of every GPU subtest.

Local artifacts are under `/tmp/termeric-wayland-interaction-7df2d38`,
`/tmp/termeric-wayland-fullscreen-7df2d38`, and
`/tmp/termeric-renderer-7df2d38.{log,png}`. They are temporary local artifacts,
not uploaded CI history or a completed native-desktop acceptance record.
The two schema-3 result JSON hashes are respectively
`28237b5d552a139b9f359c95045973ac98da22697554b212a0de4b6a27cd67e0` and
`792c009171664809279e1ae9556059d7524c4deff458a75bd1239f351ef99200`.
The PNG hash is
`80b8c3b2e9a38c4e52f697e986702463f24dc3741f1b16dcd14fa2d71b8d2471`.
The GUI harness records the binary digest but does not itself pin source and
executable identity throughout execution; this is weaker provenance than the
soak/preflight contract.

X11 was not run: this host lacked Xvfb and Openbox. Weston was also unavailable.
Real desktop policy, vendor GPUs, IME/Orca, subjective rendering, actual graphics
clients, and hosted CI acceptance remain outstanding. No real AI-provider
workflow was invoked, and no capability status is promoted by these results.

## Visual evidence

Native-window captures are retained as diagnostic artifacts. GPU-presented
surfaces are not portably readable through X11 root-window capture, so CI does
not call a blank root capture a visual pass. Instead, the GPU lane renders a
focused snapshot through Termeric's real font shaping, paged atlas, upload,
text shader, blending, and readback paths using the bundled deterministic test
font. CI uploads the PNG and compares the checked-in, reviewable signature in
[`renderer.json`](../termeric/tests/gui/baselines/renderer.json). The signature
tracks visible-pixel area, bounds, mean color, and row/column ink profiles with
documented tolerances in `scripts/check_gui_visual.py`.

The same opt-in GPU lane exercises the terminal image pipeline with a 52-row
color fixture. One classic placement and 52 independent Unicode-placeholder
slices render below text, matching the protocol's real placeholder z-plane.
Pixel readback requires both halves to match completely in the initial frame,
after three rows scroll out
through normal output, and after scrollback restores those rows. This is a
deterministic regression gate for the observed row-at-a-time reveal failure;
the separate native `graphics-clients.png` remains the compositor-level
artifact for later review.

The workspace client lane invokes seven external streams: four current
`kitten` modes (classic stream, Unicode-placeholder stream, stdin-backed
temporary file, and POSIX shared memory), independent `timg` Kitty and iTerm2
backends, and `img2sixel`. The local-media records are written only after the
temporary file or shared-memory name is no longer present. This makes cleanup
and successful parsing machine-checkable, while the retained native capture
remains review-only and can be judged later without blocking hermetic CI.

Copy-mode overlays additionally have a deterministic golden geometry test for
scrollback-to-visual-row placement. This keeps hint labels and the outlined
keyboard cursor aligned with the same coordinates used by selection and search;
the X11 lane uploads review-only `copy-mode-selection` and `quick-select-hints`
captures as end-to-end presentation evidence without treating compositor pixels
as a portable golden.

To intentionally update that signature after reviewing the before/after PNG:

```bash
TERMERIC_GPU_SMOKE=1 \
TERMERIC_GUI_VISUAL_OUTPUT="$PWD/target/gui-acceptance/renderer.png" \
WGPU_BACKEND=vulkan \
cargo test -p termeric linux_headless_gpu_render_smoke_when_requested -- --nocapture

python3 scripts/check_gui_visual.py \
  target/gui-acceptance/renderer.png --update
```

An update must accompany an explanation of the intended visual change. The
normal CI command omits `--update` and rejects changes beyond the perceptual
budgets.

## Local reproduction

### Clean-candidate mode

CI now requires `--require-clean` on each GUI harness invocation. Add it to any
of the local commands below after building from a clean checkout. This opt-in
mode refuses dirty source, an existing artifact directory, baseline updates,
and symlinked/out-of-checkout executable paths. Repeated scenarios are refused
in both modes because they would overwrite their own diagnostic artifacts.
Use a fresh ignored build subdirectory or an external temporary directory.

A parent-owned Linux executable descriptor stays open across the scenario run;
every direct application/CLI launch made by the harness receives its `/proc`
descriptor path. Final source/path/descriptor observations must match the
initial identities, and every selected scenario must have passed before the
CLI prints success. A detected change yields a nonzero exit and retained
failure evidence even if the functional scenarios themselves passed.

Strict runs emit schema-5 `results.json` with a schema-2 embedded `candidate` object
and a separate `provenance.json`: observed commit, exact requested scenarios,
the `skip_visual_checks` option, initial/final executable identities, provenance
checks, and `candidate_eligible`. The visual option records requested policy;
it does not claim that every scenario performs pixel comparisons. Default
debugging runs retain schema 3 and may use dirty source. A failure before any
scenario result is available retains provenance without fabricating results.

For Wayland `interaction`, candidate eligibility additionally requires the
thirteen palette cases to be independently reconstructed from
`interaction/pty-transcript.jsonl`. The verifier requires exactly the ordered
case set, two started alternating owners, unique ordered begin/end markers,
exact U+2603 insertion plus per-owner fences, zero reported unrelated focus
losses, and restoration of the original owner. It does not trust the sidecar's
expected bytes as the oracle. It then seals both that transcript (at most 8 MiB)
and `interaction/palette-input.json` (at most 64 KiB) with lengths and SHA-256
digests in `candidate.interaction_palette`. Missing, changed-during-read,
non-regular, symlinked, malformed, or inconsistent artifacts fail publication
and retain a diagnostic in `interaction_palette_error`.
The embedded palette proof and sidecar both use schema 2; schema-1 seven-case
records remain historical and cannot satisfy the expanded word-edit contract.

Re-check those two seals and the palette semantics offline:

```bash
python3 scripts/gui_palette_evidence.py /tmp/termeric-wayland/results.json
```

This command checks the palette evidence subset, not all GUI scenarios or the
source/compiler provenance. Older schema-4 records lack this sealed contract
and are rejected by this verifier; they remain historical evidence of their
original checks rather than equivalent current palette acceptance.

This detects observed drift, not hostile same-user tampering, signed compiler
provenance, or proof that an existing binary was freshly built from the named
source. Build first. It pins direct harness invocations, not every independently
spawned descendant, compositor, installed client, font, or driver. Scenario
logs other than the two palette artifacts above are retained but not individually
sealed by this mode. Historical schema-3
results, including the local `7df2d38` observations above, do not gain stronger
provenance retroactively. Real-desktop and provider acceptance remain separate.

Build first:

```bash
cargo build -p termeric --locked
```

On Debian/Ubuntu, install `xvfb`, `xauth`, `xdotool`, `xclip`, `imagemagick`,
`python3-pil`, `fonts-dejavu-core`, `fonts-noto-color-emoji`, `vim`, `less`,
`tmux`, `libsixel-bin`, `kitty` (for `kitten`), `timg`, `util-linux` (for
`script`), `openbox`, `weston`, `sway`, `wtype`, Mesa's Vulkan drivers, and the
normal Termeric build dependencies.

Run every X11 scenario:

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

Use repeated `--scenario workspace`, `--scenario fullscreen`, `--scenario cli`, `--scenario vim`,
`--scenario less`, or `--scenario tmux` to reproduce only a failed case. The output directory keeps
the exact config, PTY JSONL, application and window-manager logs, screenshots,
fixture files, desktop-spy calls, and a machine-readable result.

Prerequisite checks are scenario-local. Only `workspace` runs current
`kitten`, independent `img2sixel`/`timg` clients, and X11 clipboard round trips;
only scenarios that capture a native window require ImageMagick, Spectacle, or
FFmpeg. Fullscreen, CLI, Wayland, and full-screen application fixtures
explicitly skip unrelated graphics emission, so an isolated reproduction
cannot be blocked by tools that the selected scenario never executes.

Run Wayland launch/render/close:

```bash
WGPU_BACKEND=gl python3 scripts/gui_acceptance.py \
  --backend wayland \
  --start-weston \
  --binary target/debug/termeric \
  --artifacts target/gui-acceptance/wayland
```

Run Wayland fullscreen/resize/sibling isolation separately under Sway, whose
virtual-keyboard protocol support lets the harness inject F11 without relying
on host input devices:

```bash
WGPU_BACKEND=gl python3 scripts/gui_acceptance.py \
  --backend wayland \
  --start-sway \
  --scenario fullscreen \
  --binary target/debug/termeric \
  --artifacts target/gui-acceptance/wayland-fullscreen
```

Run Wayland keyboard, pane, and clipboard routing under the same private Sway
backend:

Each `wtype` invocation first cycles a non-text Shift key and waits briefly for
wlroots to register the new virtual keyboard; this avoids dropping the leading
tested key without adding bytes to the PTY. Input assertions reconstruct the
ordered byte stream independently for each fixture PID, since a PTY may split
one injected token across arbitrary reads. One-shot `wl-copy` sources stay
alive concurrently until Termeric requests the paste, then must exit cleanly.
The displayed soak retries an injected action up to three times only while its
specific pane-lifecycle or PID-routing effect remains absent; exhaustion still
fails the lane.

```bash
WGPU_BACKEND=gl python3 scripts/gui_acceptance.py \
  --backend wayland \
  --start-sway \
  --scenario interaction \
  --force-surface-loss \
  --force-surface-outdated \
  --force-device-loss \
  --binary target/debug/termeric \
  --artifacts target/gui-acceptance/wayland-interaction
```

Missing applications are failures, never silent skips. A developer running a
partial local command may record the exact tool as unavailable, but the CI
lane installs and requires the complete declared tool set.

## What remains manual

Only policy- or perception-dependent checks remain manual: real-compositor
clipboard/primary-selection permissions, external URL and notification policy,
audible output on a real device, fractional scale behavior, font/fallback
quality across installed system fonts, vendor-specific presentation, and
authenticated external-provider workflows. Whole-window golden screenshots
are deliberately avoided because compositor decorations and rasterization are
not a stable product contract.

## Tab-placement geometry and keyboard lane

The `tab-placement` scenario now runs in the default X11 set and alongside the
private-Sway interaction scenario in CI. It starts two tabs, splits the second,
then changes only the disposable configuration's `tab_bar_position` through
`top → left → top`. All three original PTYs, including the inactive tab, must
lose columns and pixel width for the rail without changing rows/height, then
recover their exact original geometry. Their fixture PIDs must remain intact.

On the left rail, F6/Home/arrows/Enter/Escape select the two original tabs
without leaking navigation bytes to the PTY. Additional F6 navigation creates
and closes a third tab; unique typed tokens must reach only the intended
original or newly created fixture PID. The same original tabs remain usable
after top placement returns. Native close must terminate all fixture children.
`tab-placement.json` retains top/left/restored geometry and observed input
owners beside the PTY transcript and runtime logs.

For an isolated Wayland reproduction after a clean build:

```bash
WGPU_BACKEND=gl LIBGL_ALWAYS_SOFTWARE=true WLR_RENDERER=pixman \
python3 scripts/gui_acceptance.py --backend wayland --start-sway \
  --require-clean --scenario tab-placement \
  --binary target/debug/termeric \
  --artifacts target/gui-acceptance/tab-placement-wayland
```

X11 uses `--backend x11 --start-window-manager --scenario tab-placement` inside
the private Xvfb command above. The scenario needs no external graphics client,
clipboard helper, screenshot tool, or AI provider. Selecting it does not
establish their acceptance. A passing geometry/routing result also does not
prove visible overflow scrolling, pointer hit targets, or pixel alignment.

## Tab-overflow keyboard lane

The `tab-overflow` scenario creates twelve tabs in a compact left rail, selects
the first tab, then traverses F6 focus through the last tab's close target,
Previous, and New Tab. It requires a thirteenth PTY to start, no navigation bytes
to reach any original PTY, and subsequent typed input to reach only the new PTY.
Normal window closure must clean up every fixture child. `tab-overflow.json`
records the sequence and process identities beside the raw PTY/runtime logs.
The compact-rail phase, added in schema 2, uses a 960×128 surface: pointer clicks on
the first tab, Next, Previous, and New Tab must route subsequent typed tokens
to their exact expected PTYs. Existing-tab clicks must not create another tab.
The fixture derives its row height from observed PTY cell metrics and fails if
the geometry cannot exercise the compact layout.

Schema 3 adds F6-to-pointer focus transitions without an intervening Escape:
click terminal content while chrome is focused, select a distant tab revealed
by keyboard navigation, then click each half of a split tab. Exact typed-token
owners must match the active terminal, the displayed tab, and each clicked
pane respectively. The distant target is calculated from observed cell metrics
and the rail's one- or two-tab capacity; resetting the viewport before hit
testing must fail this check. `pointer_focus_input_owners` and `focus_split_pid`
retain the observations, and normal shutdown must reap the extra split child.

Schema 4 adds a six-row-high overflowing rail with two visible tab slots.
Forward and reverse pointer drags must swap and restore the first two PTY
identities, as observed by subsequent indexed keyboard selection and typed
tokens. A drag holding tab zero then switching to tab one with Ctrl+Tab must
leave their order unchanged: motion cannot transfer the gesture to the new
selection. `drag_input_owners`, `drag_points`, and `drag_surface` retain this
evidence. No drag may spawn another PTY. The Wayland helper holds one virtual
pointer connection across press/move/release; both backend adapters release
their button in a `finally` block if an intervening fixture action fails.

X11 uses its existing window-relative pointer adapter. Private Sway uses a
small, bounded Wayland virtual-pointer client: its device-free seat cannot
deliver usable clicks through the deprecated Sway cursor commands alone.
The client requires headless/no-libinput fixture settings and Wayland/Sway
sockets in the same private runtime; it never opens a host input device.
Pure tests cover wire framing, fragmented replies, object IDs, failure cleanup,
and rejection of non-fixture environments. Seat/tree snapshots retain the
private compositor geometry before the pointer phase.

This catches a reproduced focus regression: switching from a distant tab to an
arrow reset the viewport to the active tab, removed that arrow, and abandoned
chrome navigation. The shared layout now retains its last tab anchor through
chrome controls and resets it when keyboard focus returns to the terminal.
Pure tests cover both horizontal and vertical layouts and anchor reset/clamping.

Use the command above with `--scenario tab-overflow` and a fresh artifact
directory. Default X11 and the private Sway interaction CI command select it;
that wiring is not a hosted or X11 execution result. Like `tab-placement`, this
uses only synthetic local PTYs, without graphics clients or AI providers.
It does not verify normal-height arrow activation, lost release without a focus
transition or modal takeover, device unplug, edge auto-scrolling,
pinned/live-domain native drags, screenshots,
IME, screen-reader announcements, or real-desktop appearance. Pure geometry
tests cover disjoint control/tab bounds and center hit targets from sub-row
windows through normal-height rails at multiple row sizes.

## Remaining automated promotion coverage

Beyond the geometry/keyboard lane, vertical-tab promotion requires a native scenario with
`tab_bar_position = "left"`: create enough tabs to overflow the rail, verify
that the active tab remains visible, exercise previous/next/new/close and drag
reordering with mouse and F6 keyboard navigation, then switch back to `top`.
During that pass, verify that terminal columns shrink and recover, split-pane
dividers and scrollbars remain aligned, and search, IME preedit, inline images,
mouse reporting, and the accessibility tab-list bounds stay inside the shifted
content viewport. These functional checks must cover private X11 and Wayland
compositors before promotion; subjective alignment and real-compositor quality
remain manual. They do not exercise any authenticated provider workflow.

The overflow lane now switches to an empty private workspace while holding a
tab, then returns and moves the pointer both after releasing elsewhere and
while still held. It retains a private virtual keyboard during the gesture so
short-lived key injection cannot introduce unrelated device-removal focus
events. `tab-overflow.json` records observed loss/regain counts for
both cases and exact PTY routing checks. The held-return test fails on the
previous binary and passes after native gesture cleanup; Sway still delivers
the hidden release, so that case alone does not prove missing-release handling.
The X11 adapter is unit tested but native X11 execution remains pending.
Live-domain preview reconciliation is unit tested separately, not promoted to
native drag acceptance. Real-desktop and the excluded lifecycle cases above
remain deferred checks.

Schema 6 adds a held-tab drag interrupted by opening the command palette. The
query must be swallowed by the palette, movement/release must not reorder the
background tabs, and exact PTY order must survive dismissal. A persistent
private keyboard and a zero-focus-loss assertion prevent focus-loss cleanup
from masking modal-lifecycle bugs. The previous binary fails this regression;
the fixed private Sway run passes. Other modal types, native live-domain gestures,
and X11 execution are not promoted by this check.

Schema 7 additionally resizes the existing split to 960×720 and opens Settings,
Commands, and the session dropdown through F6 navigation. Each click at the
background divider's lower end must dismiss the panel and return typing to the
same focused PTY without changing either pane's geometry. The click helper uses
the explicit resized surface rather than the compact fixture's dimensions.
This checks divider-click priority, not every panel control, scrollbar path,
subjective hover appearance, or native workspace-panel interaction.

Schema 8 adds width changes from 960 to 1040 and back while holding a tab drag.
The fixture waits for PTY geometry before moving the pointer using the new
surface dimensions, rejects any unrelated focus loss, and checks original tab
order after release. This reproduces the old retained-tab-drag bug. The shared
geometry-sync finalizer additionally has unit coverage for scale changes and
live-domain preview rollback; the native lane does not establish physical
monitor-scale or live-domain divider acceptance.

Schema 9 adds 100 bounded owner-tagged history lines to the overflow fixture.
It holds a scrollbar while switching tabs with Ctrl+Tab, moves the pointer to
the top, and requires the newly selected tab to remain at its original bottom
marker. Keyboard copy mode reads the visible line through the private native
clipboard (`wl-paste`, or `xclip` for the unrun X11 adapter), without searching
or resetting scroll. Focus-loss counts must remain unchanged during the drag.
Tab setup retains a private virtual keyboard; its focus predicate handles
Termeric's initially focused state, where no transition log exists yet.
This does not establish native pane-switch or live-domain scrollbar acceptance.

Schema 10 adds a positive local divider drag that must resize both child PTYs,
followed by a second drag interrupted by Ctrl+Tab. Subsequent pointer motion
must change neither the hidden source panes' geometry nor the selected tab's
geometry. An exact-owner input marker confirms the tab switch, and focus-loss
counts must remain unchanged so unrelated window blur cannot mask the bug.
Live-domain acknowledgment/rollback has separate unit coverage, not native
acceptance in this scenario.

Schema 12 checks stationary four-second Next/Previous holds in three layouts:
the normal left rail, compact left rail, and top strip. Fixture geometry derives
from the observed cell metrics and rejects out-of-range rows/tab counts rather
than silently skipping a case. Each transition waits for configuration reload
and matching PTY geometry. Normal/top arrows disappear when the hidden range is
exhausted; compact arrows remain until the first/last active index. Exact PTY
markers assert these stopping indices, the selected owner immediately after
each hold (before reselection), and restored sibling order. Any final visible
slot move uses a normal drag. Unrelated focus loss fails the case. The artifact
retains each layout's surface, points, and expected indices.

These checks establish timer-driven hidden-range traversal without extra pointer
motion in the private compositor used for the run. They do not establish
pinned/live-domain native traversal, arbitrary edge zones, visual quality, or
an unrun X11/hosted CI environment.

## Native background-image pixels

Run the bounded private-Sway screenshot scenario independently:

```bash
env WGPU_BACKEND=gl LIBGL_ALWAYS_SOFTWARE=true WLR_RENDERER=pixman \
  python3 scripts/gui_acceptance.py --backend wayland --start-sway \
  --require-clean --scenario background \
  --force-surface-loss --force-surface-outdated --force-device-loss \
  --binary target/debug/termeric --artifacts target/gui-acceptance/background
```

It checks 32 static PNG/JPEG cases: four fits, opacity 0/1, and top/left tabs.
An independent inverse-fit reference samples clear content pixels; an explicit
terminal-cell background must remain above the image. The same native window
then checks atomic replacement, malformed-source last-good retention, restored
pattern, search geometry, a shared background across split panes, output
resize/restore, requested simulated renderer recovery, and removal on disable.
All child PTYs must exit. `background/background.json` records each frame's
viewport and sample count, with adjacent PNG screenshots including the latest
frame on pixel failure. This is sampled native composition evidence, not an
all-pixel comparison (the separate offscreen GPU lane provides that).

The capture helper implements v1 `wlr-screencopy` with bounded shared memory;
no external screenshot tool is needed. It refuses non-private compositor
settings, missing/duplicate outputs, unsupported formats, dimensions above
4096, buffers above 64 MiB, malformed events, and a five-second deadline.
It only connects to the harness runtime and never captures the user's desktop.
The scenario requires private Sway and `wtype`, is not available on X11, and
cannot use `--skip-visual-checks`. CI selects it alongside native interaction
and tab scenarios; this does not itself establish an accepted hosted result.

Plugin chrome, driver-memory bounds, physical GPU recovery, and equivalent X11
presentation remain separate gates. Visual/contrast judgment and real desktop
policy remain a manual pass on one X11 and one Wayland compositor without
invoking any provider integration.

Real X11/Wayland candidate results belong in the schema-versioned,
artifact-hashed [native Linux acceptance record](native-linux-acceptance.md).
That record fails closed on missing environments, incomplete checks, inconsistent
dispositions, and changed evidence; an unavailable prerequisite remains visibly
different from a pass.
