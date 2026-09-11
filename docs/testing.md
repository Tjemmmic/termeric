# Testing Termeric

The default verification lane is deterministic and does not require a window:

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

The test suite includes a headless `App` controller, direct terminal fixtures,
real PTY quick-exit capture, a real Bash/OSC 133/Harvest path, shared render and
hit-test geometry checks, and opt-in executable-contract probes. The installed
Codex parser probe is skipped before launching the CLI unless the operator sets
`TERMERIC_REAL_CODEX_CONTRACT=1`; the default lane uses only the fake
executable integration fixture.

Reusable terminal-profile coverage is hermetic: `/bin/sh` fixtures verify exact
environment/cwd/title delivery, CLI resolution before window creation, palette
hot replacement, frozen pane launch configuration, and workspace identity.
No provider-backed or authenticated AI command is executed by this lane.
The separate `scripts/run_fake_provider_acceptance.py` runner resolves and
executes only that exact fake-CLI test, strips the real-Codex opt-in variable,
and emits a sealed partial integration record. CI uploads it with the tested
binary digest; every non-fake integration row remains visibly unrecorded.

The feature-support contract rejects undefined status vocabulary, README
highlights without reviewed matrix mappings, missing/duplicate capability rows,
incomplete release-gate domains, missing contributor guidance, and malformed
GitHub issue links. Its negative tests prove those failures remain fail-closed;
the separate Markdown checker resolves every repository-local documentation
target.

For an exact clean candidate, the fixed-command
[hermetic release preflight](release-preflight.md) runs these provider-free
quality gates offline and binds their complete logs, tool context, Git SHA, and
release-binary digest into one validated artifact. It does not replace native,
package-manager, soak, remote-host, or authenticated-integration evidence.

Live-domain compatibility coverage includes an executable-level
`domain protocol-version --json` probe under an isolated runtime directory. It
must return the compiled wire version without creating a daemon socket or token.
Exact-version mismatch tests fail before requests or terminal input, while the
fake-SSH integration remains credential- and network-free.

Guardrail coverage is also provider-free. Pure state tests prove immediate
cancellation, one-shot request identity, stale-action rejection, exact
multi-destination ownership, and shared pointer/accessibility geometry.
Headless fake-plugin fixtures prove paste and pane-broadcast commits are
intercepted before delivery by using only an inert local `/bin/sh -c sleep`
process; the buffered payload is never confirmed or written, and no Codex,
Claude, network, provider SDK, or authenticated AI process is started.

Information-dialog coverage is entirely in-memory. It verifies title/line/
choice ceilings, omission markers, window-bounded shared geometry, vertical and
horizontal viewport clamping, immediate action revocation, modal isolation,
visible-only semantic content, scrolling actions, and rejection of replaced
profile choices. Fixture choices are inert identifiers: tests never launch a
plugin, apply a real AI profile, access the network, or consume provider tokens.

macOS and Windows run the same workspace build and unit-test commands in CI,
but that tier is source-portability evidence rather than a native runtime claim.
See [`platform-support.md`](platform-support.md) for promotion requirements.

## Linux software-renderer smoke lane

Set `TERMERIC_GPU_SMOKE=1` to require a headless WGPU adapter and submit a real
render pass:

```bash
TERMERIC_GPU_SMOKE=1 cargo test -p termeric linux_headless_gpu_render_smoke_when_requested
```

CI should install Mesa's Vulkan software renderer (`mesa-vulkan-drivers` on
Debian/Ubuntu), set `WGPU_BACKEND=vulkan`, and set
`TERMERIC_GPU_SMOKE=1`. The test strictly requests a fallback adapter
(llvmpipe); its absence is a hard failure rather than permission to hide the
missing software path behind hardware. Set `TERMERIC_GPU_SMOKE_HARDWARE=1` on
a real-GPU host to require a non-CPU adapter through the same visual subset.
Without `TERMERIC_GPU_SMOKE=1`, the test prints why it was skipped.
The software lane also asserts that adapter diagnostics report a CPU device, so
a changed WGPU selection policy cannot make the fallback lane pass on hardware.

Mesa hosts without a Vulkan software ICD can exercise the same strict CPU lane
through surfaceless OpenGL. `LIBGL_ALWAYS_SOFTWARE` must be present before WGPU
creates its instance so Mesa exposes llvmpipe instead of the installed GPU:

```bash
WGPU_BACKEND=gl EGL_PLATFORM=surfaceless LIBGL_ALWAYS_SOFTWARE=true \
TERMERIC_GPU_SMOKE=1 cargo test -p termeric \
  linux_headless_gpu_render_smoke_when_requested -- --nocapture
```

Native window and surface behavior runs in the dedicated Xvfb/Weston CI lane.
It drives the real application through X11 input, PTY transcripts, desktop
spies, Vim/less/tmux fixtures, one-shot lost/outdated-surface and device-loss
recovery, and a focused GPU visual signature. See
[`gui-acceptance.md`](gui-acceptance.md) for exact local commands, artifacts,
baseline updates, and the smaller desktop-policy matrix that remains manual.
The three full-screen clients run with isolated home/configuration state and a
bounded raw PTY-output recorder; each scenario must observe a matching
alternate-screen entry and exit before it can pass.

The private Wayland lane now separates Weston launch/lifecycle from Sway
interaction. The Sway fixture uses virtual keyboard and compositor clipboard
clients to prove typed input, keyboard copy mode, bracketed paste, split-pane
focus/closure, exact PID routing, fullscreen geometry isolation, and native
close without relying on a host input device. Its interaction run also injects
lost and outdated surfaces plus device loss, changes/restores the Sway output
after each surface error, and requires renderer recovery before the PTY
assertions. Real desktop permission and presentation policy remains manual.

The headless visual subset is deliberately requested from a fallback adapter
first. The native lane uses `--force-surface-loss`,
`--force-surface-outdated`, and `--force-device-loss`; each surface event enters
the same bounded resize-or-recreate path, while the device event replaces all
WGPU state and the fixture PTY remains owned and active. Pure tests simulate missing adapters,
device-request rejection, hardware-to-software selection, surface-error
classification, stale generations, and the three-in-30-second recovery cap.
See [renderer-recovery.md](renderer-recovery.md).

The default X11 set also exercises persistent local domains: an authenticated
daemon-owned shell is attached, the last GUI closes, CLI inspection proves the
process and output remain, and a fresh GUI reattaches and continues the same
PTY. `cargo test -p termeric domain --locked` covers protocol/path/resource,
descriptor-verified no-follow runtime paths, fixed-buffer token reads and
credential-publication naming, daemon-owned pane/tab closure, layout/focus
repair, attached-client notification,
stable-ID GUI completion/error handling, monotonic topology subscriptions,
slow-subscriber eviction, incremental tab/split/collapse reconciliation,
stale-revision rejection, relay-driven dissolution, and multi-client control
behavior without a display. A bounded input-admission fixture fills the exact
pane-local queue and proves full/disconnected writers fail immediately instead
of blocking shared daemon state. The socket fixture reports a skip only in
sandboxes that deny AF_UNIX.

## Terminal conformance and fuzzing

The machine-readable core sequence manifest lives at
[`termeric-core/tests/fixtures/vt_support.toml`](../termeric-core/tests/fixtures/vt_support.toml).
Statuses describe the named parser/core/app layer; they do not imply renderer
or desktop support. Every supported claim links to a replayable fixture or a
named unit test, while partial/parsed-only/unsupported claims link to an open
issue.

`cargo test --workspace --locked` replays the deterministic corpus, verifies
streaming at every byte split, checks manifest references, and feeds seeded
arbitrary bytes through parser/resize interleavings while enforcing grid and
cursor invariants. Manifest evidence must resolve to an actual test attribute,
and both visible and retained rows are checked for width/wide-cell consistency.
The same lane applies line and byte scrollback ceilings; a high-entropy
styled/Unicode reflow case deliberately exceeds a 4 KiB history budget and
proves eviction keeps accounting within it.
The application lane changes those limits at runtime, drives enough output to
evict history, and proves the exact line/byte ceilings apply to every existing
pane as well as panes and tabs created after the change.

The full corpus provenance, reference-capture rules, replay/minimization steps,
and resource limits are documented in
[`terminal-conformance.md`](terminal-conformance.md).

Unicode cases additionally assert grapheme widths and wide-pair invariants
through edits and reflow, map search/URL bytes back to terminal columns, exercise
a real PTY, and shape/submit complete clusters in the opt-in GPU lane. The pinned
policy and exclusions are in [`unicode-support.md`](unicode-support.md).

Search cases also require query edits to begin cancellable work without
publishing partial matches, advance by an explicit row bound, restart safely
when a pending query changes, and publish navigation/filter indexes only on
completion. The release benchmark records fresh and completed-broad-result
keyboard dispatch separately from the complete 10k/100k scan, so responsiveness,
candidate reuse, cleanup, and total work cannot be conflated.

The UI lane additionally inserts and deletes multi-byte query text at a middle
cursor, rejects queries beyond 4 KiB without changing the prior value, and
checks one shared search layout against its semantic controls. AccessKit tests
require editable query state, live result status, typed toggles, explicit close,
and exact action bounds. Toast tests require a four-entry visible cap, stable
identities across incremental snapshots, 512-character messages, shared card
bounds, polite non-error announcements, assertive errors, and visibility above
an otherwise isolated modal tree.

A dense-result case generates 300,000 occurrences directly and requires an
exact total, complete 3,000-row filter membership, a circular latest-occurrence
window, correct navigation across its wrap, virtualized filter scrolling, and
an index footprint below 8 MiB. The release lane repeats the memory assertion
with 800,000 occurrences across 10,000 terminal lines.

SGR cases map every set/reset sequence through the grid, preserve rendition in
ANSI snapshot replay, and ingest a combined fixture through a real shell PTY.
The opt-in headless WGPU lane also verifies requested bold/italic faces,
hidden/blink suppression, clipped glyph geometry, each decoration geometry
golden, and a real text/rectangle submission. Exact behavior is documented in
[`sgr-rendering.md`](sgr-rendering.md).

The same WGPU lane loads Noto Color Emoji and requires RGBA preservation through
the dedicated color atlas, dual-texture bind group, vertex flag, and shader.
It also forces 2,048 Unicode scalars and regular/bold/italic/
bold-italic requests through a deliberately tiny paged atlas. It requires
complete geometry under exhaustion, exercises the visible replacement path,
changes font size, and submits the texture-array shader. Allocator unit tests
independently cover deterministic LRU and current-frame pinning. See
[`glyph-atlas.md`](glyph-atlas.md).

Font-range policy tests deserialize and round-trip hexadecimal intervals,
enforce the 64-rule/scalar/surrogate/overlap/family-name boundaries, and prove
adjacency plus non-BMP ranges. Deterministic Fira Mono and Noto Sans Arabic
fixtures prove mapped-family precedence, binary-search boundaries, first-
substantive-scalar ownership, complete-grapheme refusal, and missing-family
fallback without depending on host fonts. Headless application tests prove a
valid live reload retains the policy and an invalid overlapping replacement is
atomic. The Settings manager is additionally covered through its shared
keyboard, pointer, and accessibility action plus request-scoped add, edit,
confirmed delete, correction-preserving validation errors, cap/paging,
cancellation, live-reload identity drift, and overlay-replacement refusal. No
native compositor, downloaded font, or provider process is used.

Coverage-inspector tests use the same deterministic Fira Mono and Noto Sans
Arabic sources to prove mapped-family precedence, missing and incomplete
attempts, numbered configured-fallback routing, one-face multi-scalar grapheme
coverage, normal-style platform-face naming, no-glyph and default-ignorable
states, input/control bounds, and complete-grapheme truncation. Headless UI
tests cover literal versus exact-scalar parsing, correction-preserving errors,
read-only input/report/retry/back/cancel behavior, and the Settings → overlay →
text-input accessibility route. Platform fallback is a real cosmic-text shaping
probe over only the deterministic in-memory database; no host font, GPU,
compositor, external process, or provider is involved.

OpenType-feature tests deserialize and deterministically round-trip the bounded
typed map; reject malformed tags, `kern`, excessive entries, negative/overflow
values, and duplicate TOML keys; and prove explicit values override both states
of the Ligatures preset in deterministic Fira Mono shaping. Cache-key tests
include the effective feature sequence. Headless Settings and accessibility
tests cover list/add/edit/confirmed-delete, decimal/hex input, preserved
validation errors, stale tag/value refusal, atomic reload, and the shared
request-scoped input path without a host font, GPU, or compositor.

Keyboard cases cover every named legacy key, xterm modifier parameters 2-8,
DECCKM/DECKPAM transitions, physical keypad identity, Control and Alt, AltGr,
Unicode text, shortcut precedence, and focused-pane mode selection. Xterm
resource-zero cases cover complete modifyKeyboard/formatKeyboard 16-bit state,
disable/query/reset/S7/S8/DECRQSS lifecycle, normal-type neutrality,
DECSET/DECRST 1060/1061 legacy/VT220 translation and permission-mask behavior,
shared private-mode save/restore, configured startup/RIS behavior,
bounded DECUDK parsing, reset/preserve lifecycle, VT220 Shift dispatch, and domain replication,
pane isolation, and live-domain replication. modifyOtherKeys cases cover levels 0-3, CSI-27 and CSI-u output, modifier masks,
Super/Meta, XTMODKEYS/XTFMTKEYS direct and DECRQSS queries/resets, S7/S8
replies, pane isolation, and live-domain replication. Cursor-key resource cases
cover the disabled state and modes 0-4, DECCKM interaction, cursor/edit-key
codes, private and extended formats, Super/Meta, all-resource resets, pane
isolation, and live-domain replication. Function-key resource cases cover
F1-F35, disabled `ctrlFKeys=10` remapping,
modes 0-4, private/extended formats, Super/Meta, all-resource resets, pane
isolation, and live-domain replication. Keypad resource cases cover the
distinct mode-0 default, disable/query/reset lifecycle, conventional modified
DECKPAM sequences, all 18 portable keys and 11 Num Lock-off mode-4 identities,
both formats,
Super/Meta, pane isolation, and live-domain replication. The reserved
string-key resource case proves its encoder-neutral mode/format state remains
queryable, resettable, bounded, and replicated without assigning key behavior.
Kitty cases
cover bounded screen-local negotiation, all five flags, CSI-u alternates and
associated text, common functional/keypad identities, and event types. The exact
protocol and non-claims are in [`keyboard-input.md`](keyboard-input.md). A real
Bash/readline PTY test exercises both normal and application Home sequences;
the pinned Vim, less, and tmux traces assert that each requests DECCKM and
DECKPAM.

Fragmented DCS tests cover positive and negative DECRQSS framing for SGR,
DECSTBM, DECSCUSR, DECSCA, DECSACE, and legacy DECSLPP/DECSCPP/DECSNLS
geometry, plus CAN/SUB/ESC cancellation recovery. The geometry case verifies
both reply widths and keeps resize/status-line mutations disabled.
Cursor-style tests prove application shape/blink overrides reach presentation
state and remain isolated behind synchronized-output boundaries.

OSC 22 mouse-pointer tests cover all 30 canonical names, strict casing and
alphabet validation, plain and `=` replacement, empty reset, atomic multi-push,
pop/underflow, fixed 16-entry oldest-first eviction, primary/alternate-screen
isolation, DECSTR/RIS reset behavior, special and capability queries, 7/8-bit
reply framing, and rejection of oversized payloads and lists. The shared
terminal-event bridge proves shape changes remain app-facing while queries are
written only to their owner PTY, and an exhaustive app test maps every shape to
its corresponding Winit icon. Native appearance under actual X11/Wayland
cursor themes is intentionally left to manual release acceptance.

Fragmented CSI tests cover byte-exact DECRQM/DECRPM replies for every
authoritatively represented ANSI and DEC mode, status 0 for unknown modes, and
rejection of malformed plural/subparameter requests. The same cross-layer lane
proves DECTCEM visibility and ATT610 blinking agree with the renderer, RIS,
synchronized-output snapshots, and live-domain attachment snapshots.

Classic CSI fixtures cover ECH/REP, HPR/VPR/HPA aliases, CHT/CBT, TBC, and
DECST8C across arbitrary parser chunking. Core tests additionally require REP
to preserve complete Unicode graphemes while applying current rendition, ECH
to repair intersected wide pairs, and custom/default tab stops to remain
predictable across clearing, resize, and RIS.
Selective-erasure fixtures cover DECSCA and DECSED/DECSEL modes 0/1/2. Core
and domain tests require protected wide graphemes, retained per-cell visual and
protection attributes,
compact-history/reflow persistence, ANSI replay, RIS reset, malformed-request
rejection, and attachment-state restoration.
Saved-cursor fixtures cover fragmented ESC 7/8 plus CSI s/u and modes 1048/1049.
They require repeatable, screen-local restoration of cursor position,
rendition, G0-G3/GL/GR and pending SS2/SS3 state, DECOM, delayed wrap, and
DECSCA; DECAWM remains live, and an unsaved restore returns the defined
home/default state.
The fragmented DECSTR fixture and core state-matrix tests distinguish soft reset
from RIS: documented modes (including disabled DECLRMM), both margin pairs,
rendition, charsets, protection, cursor
presentation, delayed wrap, and the active saved cursor return to defaults while
the active cursor position, both screen buffers, scrollback, custom tabs,
unrelated modern modes, and inactive saved cursor remain intact. Nonzero,
plural, and subparameter lookalikes are rejected.

DECLRMM/DECSLRM fixtures separately require CSI s disambiguation, exact mode and
status queries, DECOM-relative addressing, cursor/tab clamping, Unicode-wide
wrapping, rectangular scrolling and edits with BCE, unchanged outside cells,
safe reflow flags, and margin-aware image placement. These tests are local and
do not launch an AI provider.

VT400 rectangular-area fixtures exercise DECCRA, DECFRA, DECERA, and DECSERA
through every parser split. Core tests cover overlap-safe snapshots, clipped and
DECOM-relative coordinates, the single-page model, SGR/DECSCA and BCE behavior,
selective attribute retention, cursor stability, wide-grapheme boundaries, and
normalized image-placement crop/copy/erase behavior. These tests are entirely
local and do not invoke an AI provider.

The rectangular-checksum fixture fragments DECRQCRA at every byte boundary and
asserts its exact DECCKSR response. Core tests cover default-deny and explicit
opt-in policy, defaulted/clipped/reversed/DECOM-relative rectangles, sole-page
normalization, cursor stability, DEC blank-cell behavior, legacy attributes,
and reversible DEC Special Graphics and UK G-set bytes. No desktop clipboard,
network, shell, or AI provider is involved.

The presentation-state fixture fragments DECRQPSR and both DECRSPS restore
forms at every byte boundary. It captures cursor information and custom tabs,
performs RIS, restores both reports, and requires byte-exact reports afterward.
Core tests additionally reject malformed coordinates, fields, designations,
and tab lists atomically while preserving unrelated modern rendition flags.
This is an in-memory protocol test and launches no external application.

The C1 fixture mixes raw single-byte controls with UTF-8 containing C1-range
continuation bytes, fragments every boundary, and verifies S7C1T/S8C1T reply
framing plus parse-time mode capture for delayed clipboard and dynamic-color
responses. Core and desktop tests separately cover CSI, DCS, APC, OSC, DECSTR,
RIS, and canonical 7-bit fallback. No native clipboard or external process is
needed for this lane.

The companion high-bit-body fixture uses GR equivalents for ESC
intermediates/finals, CSI parameters/finals, DCS/OSC/APC data, and the final
byte of `ESC \\`. It also carries multibyte UTF-8 through title and hyperlink
OSC paths so compatibility folding cannot regress modern user text.

The XTGETTCAP fixture exercises a fragmented multi-name request and one exact
S8C1T response. Focused processor coverage verifies malformed/unknown names,
termcap aliases, Fish extensions, and the 64-name/8-KiB response ceilings.

The XTWINOPS geometry fixture verifies exact fragmented `CSI 14 t`, `CSI 16 t`,
and `CSI 18 t` reports under S8C1T. A focused resize test uses fractional source
geometry to distinguish the exact represented text area from rounded-up cell
dimensions and confirms that unsupported window/screen operations stay silent.
The title-operations fixture fragments OSC 0/1/2 setters, S8C1T window/icon
queries, push/pop, and the title-stack position report. Focused tests cover all
target selectors, direct slots, the 10-slot wrap, partial-entry fallback,
malformed bounds, alternate-screen and reset persistence, response bytes, and
owner routing without a native window or external process.

The OSC 7 fixture fragments BEL/ST and raw-C1 working-directory reports,
reconstructs semicolons, decodes UTF-8 paths, rejects a non-file replacement,
and asserts the sequence never becomes visible text. Focused tests cover the
4-KiB/malformed limits, local-versus-remote host policy, RIS preservation,
Bash/Zsh/Fish integration, native Nushell launch routing, opt-in Elvish module
routing, focus refresh, and reported-cwd pane inheritance.

The DECSCL fixture fragments level/C1 selection and DECRQSS reports at every
boundary. It proves soft-reset preservation of display content, level-1 7-bit
behavior, omitted-parameter 8-bit behavior, atomic rejection of invalid
higher-level C1 selectors, and exact status framing. It does not launch an
external terminal or claim strict generation-based parser feature gating.

The attribute-area fixture adds DECSACE, DECCARA, and DECRARA under fragmented
input. Core tests distinguish drawn-cell-only wrapped streams from exact
rectangles that materialize untouched spaces, cover DECOM-relative endpoints,
ordered attribute changes and repeated-toggle cancellation, preserve unrelated
modern attributes and complete cell state, keep cursor/wide-cell structure
stable, report extent through DECRQSS, preserve it through DECSTR, and reset it
through RIS. This lane is also entirely local.

IME state tests prove preedit is not inserted early, UTF-8 cursor ranges are
validated, and a commit reaches an editable surface exactly once. Unicode
picker fixtures additionally cover its bounded curated categories, Unicode
17.0 canonical names, UAX #44 exact aliases, algorithmic Hangul/CJK names,
name/alias/code-point filtering, exact scalar syntaxes,
invalid/control/surrogate refusal, complete grapheme bytes, configurable
unbound shortcut, IME editing, reduced-motion state, exact PTY UTF-8,
changed/read-only/broadcast destination refusal, pointer geometry, and retired
semantic actions. The exhaustive-name resource rail fixes the retained compact
index ceiling at 8 MiB, asserts exactly 153,626 named non-control scalars, caps
construction at 4,000,000 name bytes and 160,000 fixed records, limits every
query to 256 result objects, and runs five warm broad/algorithmic queries under
250 ms aggregate (48.8 ms observed in the isolated rail on the September 2026
development host).
AccessKit
tests prove hidden-cell redaction, incremental tree updates, modal isolation,
UTF-8 editor state, shared render/hit-test bounds, stable palette/setting/custom
command/workspace/shortcut identities, exact activation, bounded list exposure,
and rejection of stale background or filtered actions. Live-domain semantic
tests use stable host/domain IDs and stop in the headless pre-client path, so
they never open a domain socket or SSH process. Shortcut tests use an in-memory
registry and never save a binding to user configuration.
They also cover settings-modal isolation, typed setting roles, UTF-8 filter
cursor and empty-result state, shared settings geometry, numeric adjustment,
exact dropdown-option activation, and the same validated configuration action
path used by keyboard and mouse input. Custom-command coverage adds modal
isolation, shared form/run/delete bounds, stable duplicate-aware command
identity, bounded UTF-8 value replacement, explicit Save/Cancel actions, and
stale-entry rejection; its fixtures never submit or execute a command. Native
workspace/session-manager coverage adds mode-specific action sets, stable saved
IDs, bounded visible rows, keyboard selection reveal, shared action geometry,
UTF-8 rename editing, explicit Save/Cancel controls, and stale-entry rejection.
These fixtures inject memory-only entries and invoke no persistence action.
Reduced-motion tests prove shared panels/overlays, toasts, tabs, cursor/chrome,
and Harvest decoration stop requesting transition frames while preserving
their logical state. Palette tests check a 4.5:1 application-text contrast
floor for every built-in theme and separately prove the terminal palette is
unchanged. These checks are headless and do not launch a provider or consume
AI-service tokens.
Native CJK/dead-key and screen-reader behavior remains a required platform acceptance row; see
[`accessibility-and-input.md`](accessibility-and-input.md).

Mouse/focus/paste cases assert byte-exact X10, VT200, button-event, any-event,
UTF-8, and SGR reports, modifier/release/wheel behavior, safe coordinate limits,
focus transition deduplication, alternate scroll, and bracket framing. A raw
shell PTY captures the combined stream from a focused split pane. See
[`terminal-input-protocols.md`](terminal-input-protocols.md). Native Wayland and
X11 window delivery, plus repeatable Vim/less/tmux interaction, now runs in the
[`gui-acceptance.md`](gui-acceptance.md) lane; human interaction quality remains
a visual judgment.

For coverage-guided fuzzing, install `cargo-fuzz`, materialize the reviewed
corpus, and run:

```bash
seed_dir="$(mktemp -d /tmp/termeric-fuzz-seeds.XXXXXX)"
python3 scripts/materialize_fuzz_corpus.py "$seed_dir"
cargo fuzz run terminal_bytes "$seed_dir" -- \
  -dict=fuzz/terminal.dict -max_len=65536 -timeout=5 -max_total_time=180
```

CI runs a 30-second coverage-guided smoke on pushes and pull requests and a
three-minute lane on the weekly schedule or manual workflow dispatch.
CI invokes libFuzzer's minimizer after a failure and uploads the raw and
minimized streams from `fuzz/artifacts/`. That directory is intentionally
ignored because corpus artifacts may contain arbitrary terminal data.

## Release performance lane

The versioned benchmark runner covers core, interaction, renderer, PTY,
tab/pane, memory, and (when a display is available) real idle process CPU,
rendered-frame deltas, RSS, plus native key-event-to-surface-present metrics:

```bash
WGPU_BACKEND=vulkan python3 scripts/run_benchmarks.py \
  --output target/benchmarks/latest.json
```

Before accepting budgets, the runner validates every producer against
[`benchmarks/metrics.toml`](../benchmarks/metrics.toml). A missing or silently
empty metric is a failure, not an optional-probe skip. Run
`python3 -m unittest scripts.test_run_benchmarks` to exercise the contract and
budget fail-closed cases without executing the release workloads.

See [`performance.md`](performance.md) for workload boundaries, repeatability,
baseline comparison, CI budgets, and heaptrack/DHAT/perf/flamegraph recipes.
The older opt-in three-second core acceptance tests remain coarse safety rails;
the JSON suite is the release measurement source of truth.

Cross-terminal evidence is a separate opt-in operator lane. It never runs in
ordinary CI and never auto-launches an installed terminal. Select every
participant explicitly under one controlled X11 session:

```bash
xvfb-run -a -s "-screen 0 1280x900x24" \
  python3 scripts/compare_terminals.py \
  --terminal termeric --terminal alacritty --terminal kitty \
  --output target/terminal-comparison/results.json
```

The runner replaces inherited Wayland/toolkit display selectors, reexecutes
once under a private `dbus-run-session`, pins EGL to X11, and applies each
terminal's explicit X11 control before any launch. Deliberately empty Wayland
display/socket variables make Winit select `DISPLAY` and defeat WGPU's implicit
`wayland-0` probe. This keeps an
`xvfb-run` comparison on its private display even when invoked from an active
Wayland desktop and keeps single-instance IPC out of the operator's live
session.
`python3 -m unittest scripts.test_compare_terminals` fails if an adapter loses
that isolation contract.

Run `python3 -m unittest scripts.test_compare_terminals` to validate adapters,
the deterministic byte fixture, process-tree accounting, and per-metric
fail-closed comparison logic without launching any terminal. Real runs remain
manual because installed versions, font resolution, compositor state, GPU
power state, and native geometry are evidence inputs rather than hermetic CI
facts. See the comparison section of [`performance.md`](performance.md).

For idle-render changes, run the displayed probe once with
`XDG_SESSION_TYPE=wayland` in the native Wayland session and once under Xvfb
with `XDG_SESSION_TYPE=x11`. The benchmark runner clears the competing display
selector, explicitly selects Winit's backend, and pins EGL to that backend
before it starts Termeric. On X11, the Wayland variables remain deliberately
present but empty to prevent WGPU from implicitly probing `wayland-0`. This
keeps inherited desktop variables from escaping the requested session.
Scheduler and PTY wake unit coverage plus the frame-reason diagnostic procedure
are in [`event-driven-rendering.md`](event-driven-rendering.md).

For incremental-rendering changes, keep the `renderer.row_cache.*` and
`renderer.steady_frame_buffer_allocations` workloads enabled. They assert
cached/full geometry equality, submit cached buffers through WGPU, cover static,
single-row, scroll, search, Unicode, and multi-pane cases, and enforce allocation
and invalidation budgets. Architecture and trace counters are documented in
[`incremental-rendering.md`](incremental-rendering.md).

For ligature changes, run the normal row-run/feature unit tests and the opt-in
headless GPU lane. The latter compares deterministic enabled/disabled rendered
pixels, checks repeat stability and cache reuse, and submits the result through
the real text pipeline. Keep `renderer.ligature.code_run_build` and
`renderer.ligature.shaping_cache_hit_rate` in release benchmark artifacts. See
[`ligature-rendering.md`](ligature-rendering.md) for boundaries and the manual
programming-font acceptance row.

For Git/transcript worker changes, run the deterministic injected-backend tests
and keep `latency.transcript_enqueue_under_slow_disk` in the release artifact.
The fixtures cover slow/failing repositories and writers, cwd churn, bounded
backpressure, cap reach, disk full, and shutdown during work. See
[`background-io.md`](background-io.md) for the concurrency and persistence
contracts.

## Resource soak lane

Linux pushes and pull requests run a 30-second process soak; the weekly schedule
and manual workflow dispatch run ten minutes. The scenario repeatedly drives
bounded scrollback, mixed Unicode/styled output, resize/search, images, real
PTY-backed panes and tabs, session replacement, transcript persistence, and
clean exit. The runner samples RSS, threads, file descriptors, and direct
children and scans the combined test/GPU log for validation or device-loss
errors. Every scenario iteration also records the real background transcript
queue backlog, accepted/dropped entry counts, and post-flush backlog. The gate
requires complete metric coverage, all 68 writes, no drops, a bounded peak, and
zero bytes after each durable shutdown:

Focused-pane session coverage separately assigns distinct cwd, command, and
history state to two live panes and proves focus selects all persisted fields as
one unit. Version 1, version 3, and current focused-pane fixtures protect format
compatibility; the scrollback boundary test writes exactly 5 MiB without
partial or duplicated lines. See
[`session-snapshots.md`](session-snapshots.md).

Complete workspace coverage separately round-trips shell, terminal-profile,
and plugin launches with a nested three-pane tree, pane and tab focus, titles,
cwd and command metadata, and explicit profile identities. Archive tests cover atomic
replacement and backup recovery, interrupted temporary files, schema migration
and future-version rejection, save/restore scrollback byte ceilings,
deduplication/truncation, symlink substitution without target disclosure or
mutation, corrupt-tab partial recovery, backup-preserving rename after primary
corruption, active-tab remapping across an earlier corrupt tab,
list/rename/delete operations, missing
scrollback, cwd, profile, and CLI failure modes. The native X11 scenario drives Save Workspace,
Workspace Manager rename, full relaunch, and focused-terminal input through the
real window. See [`workspaces.md`](workspaces.md).

Mixed-launch workspace tests move a live named-profile pane into a shell tab,
round-trip its schema-2 pane identity, and execute distinct hermetic shell and
profile markers after restore. They also cover profile adoption after pane-mode
collapse, inherited future splits, schema-1 launch migration, atomic rejection
of tab-owned plugin-pane detachment, and permitted movement of ordinary splits
beside a fake plugin process. No provider CLI or network service participates.

Tab-bar layout unit tests force more minimum-width tabs than the available
strip can hold. They prove the active tab remains inside the bounded viewport,
both end states expose only the applicable navigation control, ordinary tab
counts retain the full strip, and the New Tab/global-control zone is never
occupied by a rendered tab.

Fullscreen action coverage proves the F11 default and configurable binding,
command-palette discovery, both dispatch routes, and window-local transient
state without constructing a native window. The native harness now adds an
Openbox/X11 and Sway/Wayland two-window scenario: only the focused window and
its exact fixture PID may resize on entry, its character/pixel geometry must
return exactly on exit, and the sibling must remain unchanged. Pure record
analysis tests run before the native lane; promotion still requires a green
native artifact rather than source inspection.

CLI parser tests preserve arbitrary option-like argv after `--`, Unicode paths,
multi-window/tab/split grouping, profile/workspace selection, and specific
errors for conflicts and invalid geometry. Application startup tests prove
missing and non-UTF-8 shell, direct-command, and terminal-profile cwd values
fail before any window is queued; the built binary returns nonzero without a
display server. Headless PTY tests launch ordered shell/direct-command tabs and
exact named plugin profiles. The native X11 CLI
scenario and same-process second-window portion of the workspace scenario cover
identity, geometry, focus, independent close, surviving surface/PTY ownership,
and last-window exit. See [`command-line.md`](command-line.md).

```bash
WGPU_BACKEND=vulkan python3 scripts/run_soak.py \
  --duration-seconds 30 --gpu-smoke \
  --output target/soak/local.json \
  --log target/soak/local.log
```

Run `python3 -m unittest scripts.test_run_soak` to prove deliberate RSS,
thread, fd, transcript-backlog, incomplete-drain, dropped-entry, and missing-
metric fixtures fail the evaluator. Exact scenario boundaries, ceilings,
artifact fields, and known blind spots are documented in
[`resource-soak.md`](resource-soak.md).

The separate `scripts/run_native_gui_soak.py` runner samples the displayed
Wayland application while continuous PTY output, compositor resize, and exact
split-pane churn exercise Winit/WGPU/native lifecycle state. Its deliberate
growth/progress/cleanup/runtime-error fixtures run with:

```bash
python3 -m unittest scripts.test_run_native_gui_soak
```

## Serial profile lane

Ordinary workspace tests validate and round-trip the typed
`[profiles.NAME.serial]` schema, reject mixed process/serial fields, and reject
serial cwd/split overrides before native window creation. Linux-only hermetic
tests pass embedded NUL, non-UTF-8, and escape bytes through paired local pipes
to prove the relay is binary-transparent in both directions; another test proves
a regular file is rejected before any termios mutation. A Linux integration
test starts the real Termeric bridge executable between two kernel
pseudoterminals, waits for raw mode, proves exact bidirectional bytes, and
verifies terminal-close teardown plus orderly termios restoration. These tests do not open a physical serial
adapter. Before promotion, use the contract in
[`terminal-profiles.md`](terminal-profiles.md) with a disposable UART/USB loopback
to record permissions, requested line settings, exclusive ownership, bidirectional
bytes, disconnect, close, and reopen behavior.

## Pane event-routing lane

Advanced workspace controls add portable tree/property checks for swap,
balance, presets, zoom restoration, focus, tab identity, pin order, workspace
serialization, each split-cwd policy, and atomic rejection of an invalid
cross-window destination. The pure tree harness replays 96 mixed operations for
each of 128 fixed seeds; a separate 256-operation manager harness checks focus,
zoom, owner, layout, and resize invariants. Real-PTY tests move every detachable
owner between independent managers and verify the same child processes remain
without spawning replacements. Whole-tab transfer preserves nested
pane/plugin/pin metadata, assigns a fresh destination ID, closes an emptied
source window, and rejects live-domain ownership atomically. Selected broadcast
reaches only its chosen panes, rejects confirmation below two targets, retains
two live targets after an unselected-pane closure, and cancels after a selected
closure. The X11 workspace scenario drags the active second tab across the
first and proves the new numbered route by originating PID before building and
interacting with a nested split tree. See
[the control contract](advanced-workspaces.md).

The default workspace suite uses real PTYs to issue simultaneous DSR, DA1, and
DA2 requests from two panes, polls an inactive pane-owning tab, verifies
byte-exact origin-local replies plus pane-tagged bell and notification delivery,
keeps OSC 99 capability replies beside their originating PTY, addresses an
app-mediated clipboard reply to an unfocused pane, and closes a
pane during event-heavy output. Deterministic
fixtures also overflow the app-event budget and leave a PTY reader undrained to
prove both queue ceilings. The ownership and loss contracts are documented in
[`pane-event-routing.md`](pane-event-routing.md).

PTY geometry tests read the real kernel winsize at spawn and after resize,
exercise a same-cell pixel-only update, and compare fractional/asymmetric nested
pane layouts against every terminal grid and PTY. Live-domain tests additionally
cover nonzero daemon bootstrap geometry, exact protocol-v9 relay pixels,
duplicate updates, and zero-pixel parent fallback. The rounding and update policy
is documented in [`pty-pixel-geometry.md`](pty-pixel-geometry.md).

The VT identity and character-set lane covers byte-split DA1/DA2/DECID
requests, invalid DA parameters, every DEC Special Graphics code, G0-G3
designation, single and locking shifts, UK replacement, and
RIS/DECSC/DECRC/1049 restoration. The
truthful capability and terminfo boundary is documented in
[`device-attributes-and-charsets.md`](device-attributes-and-charsets.md).

Compile and exercise the dedicated terminfo entry with:

```bash
python3 scripts/check_terminfo.py --require-apps
```

The same local contract uses isolated homes and terminfo roots to cover the
SSH/sudo propagation boundary without contacting a host or escalating
privileges: a forwarded custom name is unusable without its database, a
per-user install restores it, and the compatibility identity works after
custom database variables are stripped.

The check runs `tic`, inspects the result with `infocmp`, expands representative
capabilities through `tput`, and launches Vim, less, and tmux on real PTYs. CI
also extracts the Debian artifact and reads its installed entry. Runtime
negotiation, compatibility rollback, and remote-host tests are documented in
[`terminfo.md`](terminfo.md).

## Debian package lifecycle lane

The package gate first performs a read-only archive inspection:

```bash
python3 scripts/check_debian_package.py target/debian/termeric_*.deb
```

It requires populated package name/version/architecture/dependencies, exact
regular-file modes, an ELF application binary, a valid PNG icon, the expected
desktop entry/action contract, and an independently readable compiled terminfo
entry. Pure unit fixtures prove that missing opt-in, layout/mode regressions,
and desktop-launch mismatches fail closed.

CI then runs the same checker as root on its disposable, initially uninstalled
Ubuntu host with `--system-lifecycle` and the exact
`TERMERIC_PACKAGE_LIFECYCLE_ACCEPT=1` opt-in. The checker creates a temporary
package with an older Debian version, installs it with APT, upgrades it to the
real artifact, executes only `termeric --version`, removes the package, checks
that every owned file is gone, and proves a synthetic user-configuration
sentinel survives. It refuses non-Debian hosts and any host where a Termeric
package was already installed. This is package-manager lifecycle evidence, not
a GUI launch or compatibility evidence against an actual prior Termeric
release. The uploaded schema-1 JSON report binds those results to the package's
SHA-256 digest, byte size, control metadata, candidate commit, and UTC time.

## Arch package lifecycle lane

The maintained [`packaging/arch/PKGBUILD`](../packaging/arch/PKGBUILD) is a
normal `termeric-git` VCS recipe. CI packages the exact checked-out candidate
with an unprivileged staging helper, so pull requests and unpublished commits
are tested without rewriting the upstream source declaration:

```bash
python3 scripts/build_arch_package.py --output target/arch
python3 scripts/check_arch_package.py target/arch/termeric-git-*.pkg.tar.zst \
  --report target/arch/package-acceptance.json
```

Read-only inspection rejects traversal, links, duplicate or unexpected archive
members, missing metadata or runtime dependencies, wrong modes, invalid
desktop/PNG/terminfo payloads, unresolved libraries, and a broken non-GUI
version path. Unit fixtures deliberately exercise those failure modes. The CI
job runs in an official timestamp-pinned `archlinux:base-devel` container and
verifies that `.SRCINFO` still matches the recipe.

The install/upgrade/remove phase is deliberately unavailable by default. It
requires root, an Arch-family host, `--system-lifecycle`, and the exact
`TERMERIC_ARCH_PACKAGE_LIFECYCLE_ACCEPT=1` opt-in. It refuses either
`termeric-git` or conflicting `termeric` if already installed. On the disposable
CI host it creates and validates a candidate-payload predecessor with an older
Arch version, installs it with Pacman, upgrades to the unchanged candidate,
runs only `termeric --version`, removes the package, rejects owned-file residue,
and confirms a synthetic user-state sentinel survives. The lifecycle uses a
temporary copy of the host Pacman configuration with active `NoExtract` rules
removed so slim container-image policy cannot silently omit package-owned files;
the host configuration itself is never changed. A cleanup attempt runs after
any failure. This proves package-manager mechanics, not migration from a real
previous release or GUI behavior.

The OSC integration lane adds BEL/ST and chunk-boundary parser coverage for
titles, clipboard operations, and dynamic colors; malformed and oversized input
recovery; security-policy defaults; xterm query formatting; and terminal-local
palette resolution. A two-pane inactive-tab fixture applies distinct operations
by stable origin. Exact behavior and the remaining native clipboard row are in
[`osc-integration.md`](osc-integration.md).

Kitty OSC 66 has separate BEL, S7/ST, S8/ST, and byte-fragmented conformance
coverage. Core tests validate bounded metadata and safe UTF-8, grapheme and
explicit-width placement, all overwrite classes, edit/erase/scroll/resize
lifecycle, ED 22 history archival, semantic extraction, ANSI/session replay,
and local-domain snapshot replay. Renderer tests validate fractional alignment,
block clipping, and expanded cursor geometry. Native font and compositor
quality remains the deferred checklist in [`text-sizing.md`](text-sizing.md).

The Kitty graphics lane covers APC framing across PTY chunks, strict controls,
padded and raw-unpadded standard base64, current-Kitty 128-KiB chunks,
raw/PNG/zlib decoding, bounded uploads, capability queries, quiet and
typed replies, placement replacement/deletion, scrolling and screen lifecycle,
renderer cache revisions, non-unique image-number allocation/responses,
newest/fallback lookup, numbered chunking/display/deletion, explicit-ID
replacement, eviction cleanup, advisory usage hints, bounded Unicode virtual
prototypes, Kitty's complete diacritic table, indexed/true-color image and
placement IDs, high ID bytes, coordinate
inheritance, aspect-preserving tile crops, ID-scoped deletion, replacement and
eviction cleanup, reflow, scrollback, synchronized output, alternate-screen
ownership, and renderer viewport clipping. Relative-placement fixtures cover
signed offsets, ordinary and Unicode parents, stable named-parent replacement,
scroll/screen ownership, negative clipping, missing parents, cycles, 32-level
depth enforcement, cascading deletion, and parent cap eviction. The same lane
covers Kitty animation parsing, direct and local frame loading,
chunk continuation, partial root/extra-frame edits, base-frame inheritance,
background canvases, source-over and replacement composition, strict bounds,
gapless/loading/finite-loop timing, exact active-screen deadlines, frame
deletion/root promotion, synchronized-output snapshots, renderer revisions,
and bounded frame count and memory accounting. Unix tests exercise
temporary-file `S`/`O` ranges and deletion; ordinary-file default denial and opted-in range reads
without deletion; safe symlinks, protected/relative/non-regular paths, identity
checks, and oversized sparse-file bounds without invoking an external client. POSIX
fixtures cover private shared-memory range reads, leading-slash and
current-Kitty bare-name addressing, early unlink, capability queries,
public-object and malformed-name denial, and oversized-object cleanup. Early
parse and APC-overflow fixtures prove `q=2` never emits a reply into the PTY.
Tab/domain fixtures prove an enabled local policy reaches existing and future
ordinary tabs and panes but is forced off for domain-owned PTYs and every GUI
relay replica, including after config reapplication.
Compressed-local fixtures distinguish raw input ceilings from PNG decompressed
sizes, cover short local objects and malformed streams, and prove cleanup before
decode. The opt-in llvmpipe lane reads back a 52-row color fixture rendered as
one negative-z classic image and 52 positive-z Unicode-placeholder slices.
Every pixel must agree in the initial frame, after output scroll clips three
rows, and after scrollback restores them, so draw submission alone cannot
masquerade as a graphics pass. The native X11 workspace scenario runs current
`kitten icat` classic and Unicode-placeholder streams, stdin-backed temporary-
file and POSIX shared-memory transfers, plus independent `timg -pk` direct-PNG
output against one deterministic 64-by-64 multi-row PNG. It binds every fixed
`kitten` ID and the client-generated `timg` ID to successful terminal
diagnostics. The two local-media cases additionally validate the client target
form and require the client-owned object to disappear after PTY delivery. A
current artifact and visual review, plus disabled/enabled ordinary-file, multi-
pane, animation, and direct relative-placement acceptance remain manual
Wayland/X11 rows; see
[`kitty-graphics.md`](kitty-graphics.md).

The Sixel/iTerm2 lane adds independent encoder fixtures, DEC macro and raster
pixel-aspect geometry, fragmented input, malformed and resource-exhaustion
tests, cross-protocol cache/placement/scroll/resize/screen lifecycle checks,
and the dedicated `graphics_protocols` fuzz target. The native X11 acceptance
harness additionally requires libsixel's `img2sixel` and `timg -pi`, runs them
through real PTYs, and requires both adapters to accept their output. A pure
harness regression test places fake Kitty, Sixel, and iTerm2 clients on `PATH`
and proves the fixture actually executes and emits all seven configured streams,
including exact local-media target parsing and cleanup waits;
this prevents source presence or a dead fixture branch from being mistaken for
native-client coverage. A current accepted native artifact and subjective
visual review remain pending. Exact limits and unsupported extensions are in
[`inline-images.md`](inline-images.md).

The pane lifecycle lane adopts an existing real PTY, receives output while it
is pane-owned, and repeats split/dissolve transitions while checking spawn
counts, PID termination, cwd inheritance, and content preservation. Nested
asymmetric layouts feed the exact divider render data back through hit tests at
three coordinate scales. Stable mouse-down targets prevent nested local drags
from jumping to an ancestor. Local-domain tests additionally cover stable
daemon split IDs, validated resize requests, multi-window topology convergence,
and optimistic-preview rollback after a rejected stale target. Protocol-v5
coverage also proves explicit-title precedence and clearing, pinned-prefix
validation, stable-ID tab reorder without relay replacement, ignored
relay-local title authority, and rejected tab-state rollback. Protocol-v6 cases
add stable-pane swap, recursive balance, columns/rows/grid tree reconstruction,
fresh split IDs, retained focus/relay mappings, and single-pane rejection.
Protocol-v7 cases add atomic cross-tab runtime transfer, source and destination
focus/tree repair, fresh destination split identity, retained daemon
attachments, GUI relay-owner handoff without respawn, and invalid-target/last-
pane rejection. Protocol-v8 cases replay fragmented decoded graphics after the
text snapshot's RIS, verify quiet root/frame loads and an atomic metadata
commit, retain image-number identity for later live commands, and prove the
private import is ignored by ordinary terminals and consumed after one use. See
[`pane-lifecycle.md`](pane-lifecycle.md) and
[`live-domains.md`](live-domains.md).

The remote-domain lane replaces `ssh` with a local command spy and drives a
real daemon create/list/kill lifecycle entirely through stdio pipes. It checks
strict OpenSSH arguments and token non-disclosure. Separate fixtures reject a
token-bearing outer handshake, partial frames that exceed their deadline,
unsafe destinations, nested transports, and disconnects before negotiation.
Parser and authority tests prove remote GUI attachment retains the selected SSH
destination for topology, controls, focus, reconnect, and relay argv while
identically named local and remote domains remain disjoint. The fake SSH also
streams a relay marker without contacting a host. Headless window tests inject
topology events directly and require one authority-specific warning per outage,
an accepted equal-revision snapshot before recovery is announced, rejection of
mismatched domain routes without clearing the outage, and definitive-close
cleanup. Saved-profile fixtures cover
bounded config validation and round trips, human/JSON inventory, resolved
control and attachment authorities, pinned relay argv, and fail-before-launch
unknown profiles using an isolated config directory and command spy. In-app
picker cases add shared keyboard/mouse palette geometry, empty/error navigation,
generation-checked cancellation, mismatched snapshot rejection, modal cleanup,
and prepared-snapshot handoff into an explicit independent-window launch. See
[`remote-domains.md`](remote-domains.md).

## Shell, Harvest, AI, and desktop integration lane

CI requires real Bash, Zsh, and Fish PTYs to preserve an isolated custom user
rc, execute multiline success/failure commands, answer startup capability
queries, and produce complete OSC 133 regions. Separate Nushell and Elvish
fixtures cover native-producer launch routing and a standard opt-in module,
respectively. They can be promoted to required real-PTY gates with
`TERMERIC_REQUIRE_NUSHELL_ACCEPTANCE=1` and
`TERMERIC_REQUIRE_ELVISH_ACCEPTANCE=1`; both currently self-skip because their
executables are absent from the runner, so those claims remain pending. The
fake-AI lane launches both built-ins through real
PTYs and drives all twelve commands without credentials or network access. The
native X11 workspace fixture adds exact OSC 52,
selection/paste, link, visual/audio bell, and notification assertions through
isolated command spies. It also enters copy mode, searches, selects, yanks, and
activates an OSC 8 quick-select hint without a mouse, while proving those modal
keys never reach the fixture PTY. Portable state-machine tests cover motions,
Unicode wide-cell traversal, scrollback, selection types, cursor/selection and
quick-select reflow, soft-wrapped and capped regex matching, wrapped-link regex
precedence, hint labels, and a golden overlay coordinate snapshot. Commands,
evidence boundaries, and the consent-gated authenticated-provider procedure are in
[`integration-acceptance.md`](integration-acceptance.md).

The schema-1 integration acceptance record makes that release matrix
candidate-bound and fail-closed. Its pure tests validate exact case/check
coverage, derived dispositions, clean-candidate template policy, artifact path
confinement and SHA-256 sealing, and mandatory provider approval plus secret-
review attestations. They never launch a shell, desktop helper, or provider:

```bash
python3 -m unittest scripts.test_integration_acceptance_record
```

Desktop-helper unit fixtures substitute `/bin/true` and guaranteed-missing
executables without changing global `PATH`. They prove successful launch,
bounded and deduplicated audio/notification failures, retry recovery, and the
512-character error ceiling. An App event-routing fixture verifies one visible
warning per failing integration. The native X11 spies continue to prove exact
`paplay` and `notify-send` argv; no external desktop service is contacted by the
unit fixtures.

The OSC 99 conformance fixture fragments two interleaved notification IDs,
plain and base64 title/body payloads, completion, and a capability query under
S8C1T. Focused core tests cover both legal base64 chunking strategies,
UTF-8/control validation, strict identifiers, malformed-continuation cleanup,
screen/DECSTR/RIS lifecycle, owner routing, the 16-pending and 256-KiB aggregate
ceilings, and 4-KiB/16-KiB completed-field bounds. No native notification
service is contacted.

Diff-review tests are provider-free state and accessibility fixtures. They
cover bounded row/line construction, exact shared layout and pointer targets,
two-axis viewport clamping, complete-raw-diff Copy authority, replacement and
exit-animation stale-action rejection, isolated modal semantics, and stable
detector-local identities that review only one diff. These tests do not launch
Claude, Codex, or any authenticated provider.

Context-assembler tests are likewise provider-free. Pure fixtures cover
request replacement/cancellation, shared bounded geometry, source toggles,
workspace-contained Specific Files validation and symlink revalidation,
per-source and aggregate limits, relevant test-output extraction, editable
AccessKit fields, modal background rejection, and late-result retirement. A
local `/bin/sh` two-tab fixture proves a vanished captured destination cannot
fall back to another tab with the same plugin identity. No gathered fixture is
sent to an AI provider.

Harvest fixtures cover bounded normalized command storage, stable entry IDs,
newest-entry selection, non-replaying capacity eviction, signed negative
scrollback coordinates, resize reflow, bounded 64 KiB output snapshots,
an 8 MiB aggregate snapshot ceiling, counted count/memory evictions, visible
and semantic clipped-output state, live-text retirement, disabled unavailable
actions, viewport navigation, shared cellar and sprout geometry,
pointer hit targets, modal keyboard/wheel ownership, request replacement, stale
semantic-action rejection, and exact entry activation. A local two-tab PTY
fixture proves Search Output switches away from an unrelated active terminal
and binds to the producing terminal, Scroll To resolves the exact negative row,
and snapshot-backed Rerun still targets that terminal after live text eviction.
The real Bash/OSC 133 smoke fixture verifies that parsed command text reaches
the cellar. The Explain item is described and focus-tested only; tests never
activate it or launch Claude, Codex, or another AI provider.

Prompt-navigation fixtures construct multiple local OSC 133 regions and prove
that repeated Previous/Next Prompt actions walk rather than reselecting the live
cursor's nearest mark. They cover negative rows, exact viewport offsets,
bidirectional bounds, direct-scroll invalidation, continued-output anchoring,
resize reflow, capacity eviction, alternate-screen rejection, and focused app
routing. No shell or AI provider is contacted by these state fixtures.

OSC 133 lifecycle fixtures also cover erasure through ED2, ED3, RIS, and the
application Clear Scrollback action. They distinguish fully retained historical
regions from regions spanning erased rows, preserve one-shot identity for an
active command completed after erasure, return reset views to the live bottom,
and prove alternate-screen sequences cannot create primary-shell marks.

State-machine and byte-fragmentation fixtures validate complete OSC 133 command
lifecycles, duplicate/out-of-order rejection, prompt-only ghost suppression,
missing-D recovery, and recovery at the next valid command. Exit-status cases
cover 0, 255, negative, 256, malformed, and overflowing inputs. A real generated
Bash hook still crosses a local PTY and produces a valid Harvest region; no AI
provider participates in any of these tests.

## Quick-access terminal acceptance

Quick-access tests are display-free. They prove single-window CLI parsing,
primary-monitor percentage geometry and clamping (including negative monitor
coordinates), focused/visible toggle decisions, owner-private nonblocking lock
exclusion, crash-safe descriptor lifetime, symlink refusal, protocol-v12
request/response framing, and an authenticated temporary Unix-socket inventory
plus toggle round trip. They do not claim compositor behavior. Real X11 and
Wayland acceptance must bind `termeric --quick-access` to a desktop shortcut and
record first placement/size/app ID/always-on-top state, hide/show without PTY
replacement, focus/attention results, multi-monitor behavior, explicit geometry,
close/relaunch, and the documented Wayland rule requirement. See
[`quick-access.md`](quick-access.md).

## Local file drag-and-drop lane

Display-free tests cover exact POSIX quoting of spaces and single quotes, the
4,096-byte limit, and rejection of relative, non-UTF-8, control-character, and
bidirectional-formatting paths. App-routing coverage uses only a short-lived
local `/bin/sh` fixture and proves that ordinary shell tabs accept the payload
while plugin tabs, serial tabs, and armed broadcast refuse it. A headless
live-domain fixture proves file/directory confirmation, cancel, and stale-action
behavior without starting a transfer. Protocol tests cover upload framing,
typed file/directory descriptors, retention/list/cancel messages, canonical
prefix digests, and safe names. Archive tests cover exact
tree reconstruction, empty directories, normalized executable policy, and
symlink/special-file/path-traversal rejection; daemon tests cover private
atomic no-overwrite publication, explicit cancellation,
monotonic progress, forced-disconnect offset resume, byte-exact prefix
preservation, idempotent completion replay, identity-rebinding refusal, bounded
automatic/manual expiration, authenticated retained-record inventory, inactive
cancellation, wrong-prefix refusal without data loss, and cleanup. App tests reject stale/decreasing/wrong-total
progress, keep one persistent status entry, and scope cancellation to the exact
active request. The fake-SSH
real-binary fixture streams a multi-frame binary file and bounded directory
tree into a temporary daemon cwd, exercises `--retain`, inventory, and completed
record cancellation, and rejects overwrite without contacting a network. No provider executable
is launched. That fixture also prevents the SSH stdio bridge from regressing to
buffering the server's upload-readiness frame until connection close.

Native acceptance remains manual. On both X11 and Wayland, drag a file and a
directory whose names contain spaces and a single quote from the desktop file
manager into an idle local shell. Record the copy cursor, exact visible quoted
text, and absence of automatic execution. For shell-semantics evidence, first
type `printf '<%s>\n' `, drop the object, then manually press Enter and compare
the printed path byte-for-byte. Record refusal while a modal is open and in
plugin, serial, and broadcast contexts, plus cancellation and multi-file
behavior. For a live-domain pane, record the exact confirmation target, Cancel
with no remote object, confirmed multi-frame file upload into the pane cwd,
mode `0600`, then a directory containing a nested executable and empty
directory. Confirm destination directory mode `0700`, ordinary file mode
`0600`, executable mode `0700`, collision refusal, symlink refusal, quoted
remote-path insertion with no Enter, a
mid-transfer command-palette cancellation that leaves no destination or staging
file, and a disconnect/reconnect that resumes without publishing a partial.
For the CLI, interrupt one `--retain` upload, verify a separate `domain uploads`
process reports its exact target/progress/expiry, resume it from the unchanged
source, and compare the published bytes. Repeat with a modified same-size local
prefix and confirm resume refuses while leaving the partial available, then
cancel it from a separate process. Verify ordinary abandoned staging disappears
within 65 seconds and retained staging within 24 hours (or cancel it rather than
waiting). Stop the daemon and confirm retained records are removed rather than
claimed as restart-persistent. Repeat through one real OpenSSH
host and inspect ambiguous post-commit disconnect behavior manually. See the
[drag/drop and transfer contract](file-drag-and-drop.md).

## Declarative plugin acceptance

Plugin tests use isolated temporary registries and installed-copy directories.
They cover the complete local lifecycle, version/schema mismatch, permission
denial, forbidden capabilities, source fingerprint tamper, fingerprint-bound
in-app installation review, explicit grant/enable gating, permission pagination,
duplicate-ID refusal, source drift without mutation, permission-aware in-app
update review, added/removed permission migration, retained-grant intersection,
enabled-update gating, update permission pagination, exact candidate/registry
drift rejection without partial commit, in-app default-profile origin/selection/
reset, maximum-profile pagination, event-only refusal, stale-ID revalidation,
CLI reset parsing, profile and command
conflicts, installed-directory and administrative-file symlink substitution,
bounded hook dispatch, crash/timeout child profiles, resource ceilings,
explicit update behavior, and default-profile configuration. API v2 cases also
cover exact `workspace-query` grant persistence, schema-v1 JSON content,
workspace-restore cwd substitution, the 16 KiB ceiling, API v1 denial, and a
checked-in context-probe manifest. They inspect only generated actions and
environment data. API v3 cases cover exact coarse event envelopes, optional
context omission, manifest/grant failures, bounded response validation, and a
checked-in event-only sample. A hermetic `/bin/sh` fixture proves the local
stdin/stdout and nonzero-exit path. API v4 cases prove an OSC 133 completion
crosses the app/plugin boundary with exact source IDs and bounded command/output,
while terminal-text denial, evicted-region non-disclosure, UTF-8/JSON ceilings,
control sanitization, and a checked-in command observer fail closed. API v5
cases use only local fixtures to prove one-process state reuse, exact request
correlation, malformed-response restart, bounded line framing, generation
cancellation, and the per-window process ceiling; a checked-in persistent
counter validates the manifest contract. API v6 cases prove independently
granted and denied workspace-query actions, no query chaining, exact logical
and transport correlation, same-process round trips, current-window scoping,
control-safe metadata, total request bounds, and a checked-in workspace
inspector. API v7 cases prove exact `workspace-actions` grant/version/mode
gating, one host operation per response, query-to-action-to-result authority
reduction, stable-ID success and failure results, non-retargeting on invalid
panes, lifecycle-loop
suppression, same-process reuse, and a checked-in non-invasive workspace
navigator. API v8 cases add exact `workspace-topology` version/mode/grant
denial, index and ratio bounds, cross-kind one-operation rejection, query-to-
topology-to-result authority reduction, atomic stable-ID reorder/resize success
and failure, active-focus preservation, same-process reuse, and a checked-in
non-invasive workspace organizer. API v9 cases add manifest version,
subscription, count, duplicate, and generated-command conflict rejection;
exact-owner manager routing; both handler lifetime modes; third-party isolation
from built-in AI command suffixes; and a checked-in command tool. API v10 adds
typed schema/default/bound validation, atomic set/unset/reset persistence,
effective-value reload and diagnostics, profile/handler delivery, registry
tamper refusal, compatible update preservation, incompatible update rollback,
and a checked-in configurable command. API v11 adds exact terminal-query
grant/version/mode gating, 256-line and 8 KiB action bounds, stable-ID target
success/failure, presented-state exclusion of hidden synchronized output,
UTF-8/JSON-envelope clipping, same-process request correlation, one-host-
operation enforcement, result-authority removal, and a checked-in terminal
reader. API v12 adds exact pane-create version/mode/grant denial, typed
direction parsing, stable-ID success/failure routing, ordinary/live/serial tab
distinction, returned pane identity, one-operation enforcement, result-
authority removal, and a checked-in pane builder. Only inert local shell/Python
fixtures and in-memory terminals are used. API v13 adds exact tab-create
version/mode/grant denial, strict action parsing, stable returned identity,
unavailable/live/capacity/launch failure routing, lifecycle-hook suppression,
one-operation enforcement, result-authority removal, and a checked-in tab
builder. No AI provider process or tokens are involved. API v14 adds exact
pane-close version/mode/grant denial, strict stable-ID action
parsing, missing/live/serial/closing/last-pane refusal, non-active target focus
preservation, split collapse, correlated remaining/focused state, same-process
query-to-close reuse, one-operation enforcement, authority removal, and a
checked-in pane pruner. Only inert local shell/Python fixtures are used; no AI
provider process or tokens are involved. API v15 adds exact tab-close
version/mode/grant denial, strict stable-ID action parsing, local serial
acceptance, missing/live/closing/final-tab refusal, background focus
preservation, active handoff, copy-mode cleanup, scheduled correlated results,
lifecycle-hook suppression, same-process query-to-close reuse, one-operation
enforcement, authority removal, and a checked-in tab pruner. Only inert local
shell/Python fixtures are used; no AI provider process or tokens are involved.
API v16 adds exact pane-move version/mode/grant denial, strict typed action
parsing, complete stable-ID source/destination validation, same-tab,
missing/closed/closing/live/serial/last-pane/plugin-owned/invalid-anchor
refusal before detachment, live PTY identity preservation, fresh destination-
local IDs, correlated source/focus state, copy-mode and broadcast cleanup,
same-process query-to-move reuse, one-operation enforcement, authority removal,
and a checked-in pane mover. Only inert local shell/Python fixtures are used;
no AI provider process or tokens are involved.
API v17 adds exact terminal-input version/mode/grant and global-policy denial,
strict action/control/8-KiB bounds, stable-ID success/failure routing, live
bracketed-paste encoding, read-only refusal, focus preservation, destructive
input confirmation for ordinary shell destinations, exact confirmation
ownership/cancellation, correlated byte/disposition results, same-process
query-to-input reuse, one-operation enforcement, authority removal, and a
checked-in non-executing terminal writer. Only inert local shell/Python
fixtures are used; no AI provider process or tokens are involved.
API v18 adds exact pane-layout version/mode/grant denial, strict swap/balance/
preset parsing, atomic stale/duplicate/unsplit/closed/closing/live/serial
refusal, background-tab routing, live PTY and stable-ID preservation, focus and
zoom contracts, balance idempotence, columns/rows/grid geometry, correlated
state, same-process query-to-layout reuse, one-operation enforcement,
authority removal, and a checked-in pane arranger. Only inert local shell/
Python fixtures are used; no AI provider process or tokens are involved.
API v19 adds exact window-close version/mode/grant/declared-command denial,
command-only authority, strict targetless parsing, unavailable and duplicate
refusal, submission-before-teardown behavior, exact plugin/generation/request
correlation, stale/unloaded/error final callback handling, authority removal,
and a checked-in window closer. Only inert local shell/Python fixtures are
used; no AI provider process or tokens are involved.
API v20 adds exact process-query version/mode/grant denial, strict bounded
query parsing, deterministic process-window ordering and stable origin identity,
closing-window exclusion, aggregate topology/metadata and encoded-frame
ceilings, control-safe UTF-8 clipping, retained-grant install/update behavior,
one-operation enforcement, complete result-authority removal, and a checked-in
process inspector. All fixtures are local and hermetic; no AI provider process
or tokens are involved.
API v21 adds exact process-action version/mode/grant denial, strict typed action
parsing, same- and sibling-window stable-ID routing, atomic tab/pane validation,
stale/closed/closing/uninitialized refusal, non-activation, lifecycle-hook
suppression, bounded correlated results, process-query authority reduction,
complete process-action result authority removal, retained-grant updates, and a
checked-in process navigator. All fixtures are local and hermetic; no AI
provider process or tokens are involved.
API v22 adds exact process-terminal-text version/mode/grant denial, strict
typed query parsing, history/byte and encoded-frame ceilings, same- and
sibling-window stable-ID reads, stale/closed/closing/uninitialized refusal,
presentation-safe text extraction, non-activation, correlated bounded errors,
process-query authority reduction, complete terminal-result authority removal,
retained-grant updates, and a checked-in process terminal reader. All fixtures
are local and hermetic; no AI provider process or tokens are involved.
API v23 adds exact process-terminal-input version/mode/grant and destination-
policy denial, strict input/control/8-KiB bounds, same- and sibling-window
stable-ID routing, stale/read-only target refusal, destination-mode paste/Enter
encoding, non-activation and focus preservation, exact-target destructive
confirmation/cancellation, bounded correlated results, process-query authority
reduction, complete input-result authority removal, retained-grant updates,
and a checked-in process terminal writer. All fixtures are local and inert; no
AI provider process, destructive command, or tokens are involved.
API v24 adds exact process-tab-create version/mode/grant denial, strict action
and identifier parsing, same- and sibling-window stable-ID routing,
stale/closing/live-domain target refusal, ordinary local-shell creation and
in-window selection without native activation, lifecycle-hook suppression,
bounded correlated results, process-query authority reduction, complete
process-action result authority removal, retained-grant updates, and a
checked-in process tab builder. All fixtures are local and inert; no AI
provider process or tokens are involved.
API v25 adds exact process-pane-create version/mode/grant denial, strict action,
direction, and stable-path parsing, same- and sibling-window routing,
stale/closing/live-domain/serial refusal, atomic ordinary split creation,
launch/cwd inheritance, fresh destination-local pane identity, non-activation,
bounded correlated results, process-query authority reduction, complete
process-action result authority removal, retained-grant updates, and a
checked-in process pane builder. All fixtures are local and inert; no AI
provider process or tokens are involved.
API v26 adds exact process-pane-move version/mode/grant denial, strict
source/destination path and direction parsing, one-operation enforcement,
same- and sibling-window exact routing, stale-target refusal before detach,
live PTY and metadata preservation, fresh destination-local identity,
non-activation, bounded correlated results, process-query authority reduction,
complete result-authority removal, retained-grant updates, and a checked-in
process pane mover. All fixtures are local and inert; no AI provider process or
tokens are involved.
API v27 adds exact process-tab-move version/mode/grant denial, strict stable-ID
and insertion-index parsing, one-operation enforcement, same- and sibling-
window routing, final-source-tab and stale-target atomic refusal, live
tab/PTY/pane state preservation, fresh destination-local identity,
non-activation, bounded correlated results, process-query authority reduction,
complete result-authority removal, retained-grant updates, and a checked-in
process tab mover. All fixtures are local and inert; no AI provider process or
tokens are involved.
API v28 adds exact process-pane-close version/mode/grant denial, strict stable-
ID parsing, one-operation enforcement, same- and sibling-window routing,
final-pane and stale-target refusal, safe two-pane collapse, containing-tab/
window preservation, non-selection/non-activation, bounded correlated results,
process-query authority reduction, complete result-authority removal, retained-
grant updates, and a checked-in process pane pruner. All fixtures are local and
inert; no AI provider process or tokens are involved.
API v29 adds exact process-tab-close version/mode/grant denial, strict stable-ID
parsing, one-operation enforcement, same- and sibling-window routing, final-tab
and stale-target refusal, scheduled close behavior, containing-window
preservation, lifecycle-hook suppression, non-activation, bounded correlated
results, process-query authority reduction, complete result-authority removal,
retained-grant updates, and a checked-in process tab pruner. All fixtures are
local and inert; no AI provider process or tokens are involved.
API v30 adds exact process-pane-layout version/mode/grant denial, strict stable-
ID/action/preset parsing, distinct swap targets, one-operation enforcement,
same- and sibling-window routing, stale and unsupported-target refusal,
terminal-owner/stable-ID/focus/active-tab preservation, non-activation, bounded
correlated results, process-query authority reduction, complete result-
authority removal, retained-grant updates, and a checked-in process pane
arranger. All fixtures are local and inert; no AI provider process or tokens
are involved.
API v31 adds exact process-window-close version/mode/grant denial, strict stable-
window/action parsing, one-operation enforcement, same- and sibling-window
routing, stale/closing/pending/uninitialized and final-live-window refusal,
result-before-teardown ordering, exact origin callback correlation, sibling
isolation, non-activation, bounded correlated results, process-query authority
reduction, complete result-authority removal, retained-grant updates, and a
checked-in process window closer. All fixtures are local and inert; no AI
provider process or tokens are involved.
API v32 adds exact process-window-focus version/mode/grant denial, strict stable-
window/action parsing, one-operation enforcement, exact same-/sibling-window
routing, stale/closing/native-less refusal, shared already-focused/Wayland/
hidden/minimized backend policy, truthful requested-versus-confirmed results,
bounded correlated results, process-query authority reduction, complete result-
authority removal, retained-grant updates, and a checked-in process window
focuser. All fixtures are local and inert; no AI provider process or tokens are
involved. Real compositor focus transfer remains operator acceptance because a
successful native request cannot prove the window manager honored it.
In-app management cases use temporary registries and non-executed
fake profile names to cover the Settings action row, keyboard and pointer
routing, AccessKit button semantics, bounded pagination, stale overlay
requests, exact detail disclosure, enable/disable persistence, confirmed reset,
cancelled/confirmed removal, and preservation of the original source directory.
The samples
and author-facing reproduction matrix are in [`plugins.md`](plugins.md).

## Feature claims and evidence

[`feature-support.md`](feature-support.md) is the authoritative capability
matrix. A parser branch, unit test, configuration field, or successful launch
does not by itself justify marking a feature supported. Tests and manual
evidence must exercise the complete user path named by the matrix row.

Changes to user-visible behavior must update the relevant matrix row and link
the evidence or tracking issue. Candidate-specific manual results belong in
[`release-acceptance.md`](release-acceptance.md) with the exact commit, date,
platform, and tool versions.

Real Linux GUI, IME, Orca, desktop-policy, scale/font, and vendor-GPU evidence
uses the schema-versioned [native acceptance record](native-linux-acceptance.md).
Its pure regression suite is safe to run without a GUI:

```bash
python3 -m unittest scripts.test_native_acceptance_record
```

## Clippy policy

Strict Clippy is part of the default quality gate. The application crate has
two narrow, documented exceptions: nested conditions are retained where UI
dispatch priority or short Rust borrow scopes are clearer that way, and
renderer builders may take explicit geometry/state parameters rather than
hiding unrelated inputs inside generic parameter bags. New exceptions require
a local rationale; broad lint-family suppression is not accepted.
