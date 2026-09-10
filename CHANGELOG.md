# Changelog

All notable changes to Termeric are recorded here. The project follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and uses Semantic
Versioning once public beta tags begin.

## [Unreleased]

### Added

- The headless GPU acceptance lane now reads back and checks every pixel of a
  52-row Kitty classic placement against 52 independently queued Unicode-
  placeholder slices. Initial presentation, output-scrolled clipping, and
  scrollback restoration must all render completely in one frame, turning the
  observed row-at-a-time reveal failure into a deterministic llvmpipe gate.
  Native workspace runs also retain a dedicated post-client graphics capture.

- Native X11 workspace acceptance now launches current `kitten icat` in both
  classic-stream and Unicode-placeholder-stream modes against a deterministic
  multi-row PNG at separate cursor positions, plus the independent `timg -pk`
  Kitty backend against the same fixture. Exact fixed or client-generated IDs,
  versions, PTY geometry, client output, successful terminal diagnostics, and
  scenario-local prerequisites turn the previously ad hoc checks into
  reproducible artifacts while leaving visual/media/multi-pane/animation
  judgment explicit.

- `termeric domain protocol-version [--json]` now reports the local live-domain
  wire version without discovering or starting a daemon, giving upgrade and
  remote-mismatch diagnostics a credential-free source of truth. CLI and
  executable-level tests prove machine-readable output, reject impossible
  `--host` nesting, and verify that the probe creates no socket or token. The
  live-domain contract now accurately describes protocol v15 and its bounded
  upload revisions instead of incorrectly calling v10 current.

- A schema-versioned integration acceptance record now binds fake Claude/Codex,
  shell, Harvest, X11/Wayland desktop, and authenticated-provider results to one
  clean candidate and exact case/check matrix. Relative artifacts are SHA-256
  sealed; provider results additionally require explicit approval scope,
  spending/effect boundaries, credential cleanup, and a secret-review
  attestation. The generator and validator never launch a provider or read
  credentials, and their fail-closed contracts run hermetically in CI.

- Settings and `[accessibility]` now expose live, persisted `system`/`on`/`off`
  reduced-motion and high-contrast policies. The default follows XDG Desktop
  Portal appearance settings on a nonblocking background monitor, including
  live changes; unsupported portals and keys fall back independently, and old
  boolean values remain compatible as explicit overrides. Reduced motion snaps
  application-owned panels,
  dialogs, tabs, hover/cursor/focus feedback, toasts, and Harvest decoration
  while retaining terminal-content animation, configured cursor/text blink,
  and the visual bell. High contrast enforces a tested 4.5:1 text floor for
  built-in application chrome without changing terminal colors. Native
  macOS/Windows preference import and visual/screen-reader acceptance remain
  pending.

- Optional `[window.background_image]` static PNG/JPEG backgrounds now render
  beneath the shared terminal content viewport with typed cover, contain,
  stretch, and center geometry plus live opacity/fit controls. One
  generation-safe decoder worker per window enforces 16 MiB encoded, 64 MiB
  RGBA, and 8,192-pixel dimension limits, watches replacements, drops the CPU
  copy after GPU upload, and retains the last good image on failure. Native
  X11/Wayland visual and contrast acceptance remains pending.
- A typed, live `[window].tab_bar_position = "top" | "left"` preference now
  provides a DPI-aware bounded vertical tab rail with overflow navigation,
  active/focused-tab reveal, drag ordering, close/global controls, and shared
  render, PTY, pane, pointer, search, IME, image, Harvest, and AccessKit
  geometry. Top placement remains the default; native left-rail acceptance is
  still pending.
- A transient pane-local read-only command now blocks every user-originated
  keyboard, IME, paste/drop, mouse/focus-report, injection, GUI-control, and
  broadcast route while keeping PTY output and required terminal replies live.
  The lock follows live ownership through split/collapse/process-local moves,
  excludes broadcast targets, and is visible in the tab bar, split outline, and
  accessibility tree. Restored workspaces intentionally start unlocked.

- Ordinary windows now have a typed, persisted `normal`, `always_on_top`, or
  `always_on_bottom` desktop stacking request. It is applied at native creation
  and through live settings/config reload, while the dedicated quick-access
  window remains unconditionally topmost. Headless configuration and settings
  tests cover the wiring; native compositor acceptance remains an operator gate.

- Font configuration now enforces a bounded, unique 16-family chain and applies
  the primary plus fallbacks in exact order to complete Unicode graphemes before
  using platform fallback. Adjacent same-family graphemes remain one shaping
  span so ligatures are preserved. Settings can add, replace, and remove ordered
  fallbacks and now expose line height, programming ligatures, and the
  bold-as-bright compatibility policy. Deterministic bundled-font tests cover
  mixed Latin/Arabic family selection without relying on host fonts.

- Interactive Elvish launches now expose a generated
  `termeric-integration` module through the standard Elvish module search path.
  A one-time `use termeric-integration` entry appends OSC 7/133 hooks without
  replacing user hooks or evaluating user configuration in a restricted
  namespace, and restores the original `XDG_DATA_DIRS` before child commands
  inherit it. Deterministic routing and an opt-in real-PTY probe cover the
  boundary; real-PTY acceptance remains pending because `elvish` is not
  installed on the current runner.

- Unambiguous Nushell REPL launches now enable Nushell's native OSC 7 working
  directory and OSC 133 semantic-region producers after normal user
  configuration. Script/command, language-server/IDE, and existing pre-REPL
  command profiles remain byte-for-byte untouched. Deterministic launch tests
  and an opt-in isolated real-PTY acceptance probe cover the boundary; the
  latter remains pending because `nu` is not installed on the current runner.

- Native local file and directory drops now insert one absolute, bounded UTF-8
  path into an ordinary shell/profile tab as an exact POSIX-quoted word through
  the focused terminal's bracketed-paste route. Drops never read contents or
  send Enter; relative, control/bidirectional, broadcast, modal, plugin, and
  serial cases fail closed. An authenticated live-domain drop instead requires
  request-scoped confirmation and streams one revalidated regular file or a
  deterministic bounded directory archive through protocol v15 in contiguous
  64 KiB frames with a 64 MiB cap, four-transfer
  daemon ceiling, partial cleanup, private mode-`0600` staging, atomic
  no-overwrite publication, and exact-pane quoted-path insertion without Enter.
  GUI transfers now keep one in-place bounded progress notice and expose an
  exact-request command-palette cancel action; explicit pre-commit cancellation
  is acknowledged with the daemon-received byte count and never publishes the
  temporary file. The SSH stdio bridge now flushes every forwarded chunk, which
  fixes the prior readiness-frame deadlock in multi-stage remote uploads.
  Transient failures retry three times under one random 128-bit identity and
  resume at the daemon's contiguous offset. Optional CLI-retained partials now
  print a reusable ID, remain discoverable for up to 24 hours, verify the
  daemon-retained prefix with SHA-256, and can be resumed or cancelled by a
  later authenticated local/OpenSSH CLI process. Ordinary partials expire after
  60 seconds; all retention remains daemon-process-local. Four partials reserve
  at most 256 MiB and are removed immediately on cancellation or clean shutdown;
  recent completions replay idempotently without allowing an
  identity to change destination or transfer kind. Directory archives reject
  symlinks, special files, mount crossings, unsafe names, mutation, more than
  4,096 entries/64 levels, and unsupported metadata; they preserve empty
  directories and only the user-executable distinction, then publish through
  Linux `renameat2(RENAME_NOREPLACE)` without overwrite.
  `domain upload`, `domain uploads`, and `domain cancel-upload` expose the same
  local/OpenSSH transport and manual retention controls without GUI input.
  Native compositor and real-host acceptance remain operator gates.

- Terminal profiles may now select a native Linux serial device with validated
  baud, data bits, parity, stop bits, and software/hardware flow control. A
  descriptor-validated exclusive raw bridge relays bounded binary chunks,
  rejects process-only cwd/env/argv and pane cloning, preserves named workspace
  identity, and fails visibly on open/configuration/disconnect errors. Hermetic
  tests do not touch physical serial hardware; real-device acceptance remains
  an operator gate.

- `termeric --quick-access` now launches or toggles one dedicated drop-down
  terminal suitable for a desktop global shortcut. It uses primary-monitor
  80% × 45% top-centered geometry, an undecorated always-on-top window and
  distinct Linux app ID, authenticated protocol-v12 discovery, schema-v3
  visible/quick inventory, exact request-correlated toggling, and a private
  crash-released advisory ownership lock. Hidden PTYs stay alive while the
  transient window remains outside automatic workspace restore; Wayland's
  compositor-owned placement/activation boundary is documented explicitly.

- Declarative plugin API v3 adds optional `app-ready`, `tab-created`,
  `tab-closed`, and `tab-switched` handlers on a serialized, 32-entry bounded
  off-UI-thread process worker. Handlers receive only schema-versioned coarse
  lifecycle data, optionally the separately granted v2 workspace snapshot, and
  a cleared environment; they time out after two seconds and have strict
  stdin/stdout/stderr/action/text ceilings. Responses can request only bounded
  message/notice UI through an independent `ui-actions` grant; reload cancels
  prior-generation active/queued work, while stale, malformed, denied, failed,
  or overflowing work fails closed. An event-only
  sample and hermetic local-process tests exercise the contract without any AI
  provider process or tokens.

- Declarative plugin API v4 adds an explicitly `terminal-text`-granted
  `command-completed` event for authoritative OSC 133 regions. The schema-v1
  envelope identifies the exact tab, optional pane, terminal-local region, and
  exit status; bounded command/output objects report clipping and control
  sanitization, while evicted regions disclose no stale replacement text.
  Extraction and JSON encoding have independent UTF-8-safe limits inside the
  existing 16 KiB event ceiling, hook diagnostics never format terminal
  payloads, and the same timeout/queue/reload isolation remains in force. A
  checked-in inert command observer and hermetic cross-layer tests use no AI
  provider or tokens.

- Declarative plugin API v5 adds opt-in persistent event handlers using bounded
  newline-delimited schema-v2 requests and responses with exact request-ID
  correlation. One process per plugin/window/generation can retain state across
  serialized events; eight-process, 32-request, 16 KiB request, 64 KiB response,
  4 KiB retained-stderr, and two-second response ceilings remain fail-closed.
  Crash, timeout, malformed framing, correlation failure, registry reload, and
  shutdown kill the process group; a later event may restart it. Stateful local
  fixtures and a checked-in Python counter cover the contract without invoking
  any AI provider or spending provider tokens.

- Declarative plugin API v6 lets an exactly `workspace-query`-granted
  persistent handler request one fresh current-window inventory from an
  external subscribed event. The asynchronous schema-v2 result correlates both
  transport and plugin query IDs and exposes sanitized, explicitly truncated
  tab/pane/divider topology, titles, kinds, focus/activity state, geometry,
  workspace/domain identifiers, and active cwd. It is capped at 16 tabs, 32
  panes, 31 dividers, 2 KiB aggregate metadata, 1 KiB cwd, and the existing 16
  KiB request envelope; query results cannot recursively query. It exposes no
  terminal content, other windows, or mutations. Hermetic permission,
  correlation, same-process, sanitization, and sample-manifest tests use no AI
  provider or tokens.

- Declarative plugin API v7 adds an independent `workspace-actions` grant for
  persistent handlers. Responses can select an exact current-window tab or
  focus an exact pane using stable IDs from the v6 inventory, then receive a
  bounded correlated success/failure event. Complete-target validation, stale-
  generation rejection, one host operation per response, authority removal
  after mutation results, and suppression of plugin-origin `tab-switched`
  redispatch prevent partial retargeting and self-sustaining action loops. A
  checked-in non-invasive navigator and hermetic parser/runtime/app tests use no
  AI provider or tokens.

- Declarative plugin API v8 adds an independent `workspace-topology` grant for
  persistent handlers. Responses can reorder one exact local current-window
  tab or resize one exact local divider, with protocol-bounded indices/ratios,
  complete pinned/live/zoomed/closing validation, preserved active focus, and
  correlated success/failure events. Query/navigation/topology remain one host
  operation per response and every authority is removed from mutation results.
  Hermetic parser/runtime/app tests and a checked-in non-invasive organizer use
  no AI provider or tokens.

- Declarative plugin API v9 lets an event handler contribute up to 32 bounded,
  namespaced command-palette commands and receive exact `command-invoked`
  events. Command dispatch now targets only the namespace owner, and direct
  diff/context/export/profile actions remain restricted to the compiled-in AI
  integrations. Both process-per-event and persistent handlers are supported;
  the feature adds no new permission or host-mutation authority. Hermetic
  validation, conflict, routing, app, and sample-manifest tests use no AI
  provider or tokens.

- Declarative plugin API v10 adds up to 32 typed, non-secret settings per
  manifest: booleans, bounded integers, bounded single-line text, and bounded
  choices. Explicit CLI set/unset/reset operations persist only overrides;
  discovery merges defaults, live reloads them, shows effective values in
  diagnostics, and delivers them through a dedicated profile JSON environment
  variable and handler envelopes. Incompatible updates fail before replacing
  the installed manifest instead of losing configuration. Hermetic schema,
  CLI, persistence, reload, delivery, tamper, migration, and sample tests do
  not execute a plugin or AI provider.

- Declarative plugin API v11 adds an independent `terminal-query` grant for a
  persistent handler to read up to 256 history lines and 8 KiB of presented
  text from one exact stable-ID pane in the originating window. Primary and
  alternate-screen boundaries, hidden synchronized output, invalid targets,
  UTF-8 and 16 KiB JSON-envelope clipping, exact correlation, one host
  operation per response, and complete host-authority removal after the result
  fail closed. A checked-in command-driven terminal reader and hermetic parser,
  runtime, local-process, and in-memory terminal tests use no AI provider or
  tokens.

- Settings now exposes a bounded installed third-party plugin manager. It shows
  exact source, version, health, requested/granted permissions, effective
  settings, and explicit override counts; supports enable/disable; and requires
  request-scoped confirmation for source updates, settings resets, and managed
  copy removal. Keyboard, pointer, AccessKit button semantics, pagination,
  stale-request rejection, persistence, and source-preserving removal are
  covered by temporary-registry tests without launching a plugin or provider.

- Declarative plugin API v2 now preserves v1 process profiles while adding an
  explicit `workspace-query` grant. Granted profiles receive one bounded,
  schema-versioned `TERMERIC_PLUGIN_CONTEXT_JSON` snapshot containing only
  plugin/profile identity, active cwd, tab index/count, and terminal geometry.
  API v1 cannot acquire the grant; denied, unsupported, tampered, or oversized
  context paths fail closed, and workspace restore serializes its exact archived
  cwd. A checked-in context-probe manifest and hermetic lifecycle tests exercise
  installation, registry persistence, discovery, disclosure bounds, and visible
  failure without launching provider integrations.

- An opt-in cross-terminal X11 benchmark runner now launches only explicitly
  selected Termeric, Alacritty, kitty, WezTerm, Ghostty, or Konsole executables
  in fresh processes with isolated configurations. A shared raw-PTY fixture
  sends exact styled ASCII/Unicode bytes and records CSI cursor-response time;
  the runner separately records visible-window startup, terminal process-tree
  RSS, and post-workload idle CPU while excluding the common fixture process.
  Raw samples, versions, requested controls, actual grids/pixels, and logs are
  retained. Per-metric comparison contracts reject rankings on missing
  participants, grid or retention drift, and (for resources) pixel-geometry
  drift. Hermetic tests validate the adapters and evidence logic without
  launching any real terminal.

- A displayed-process resource soak now runs Termeric under private headless
  Sway with continuous styled/Unicode/Kitty-image PTY output, repeated compositor
  resizing, split-pane churn, PID-exclusive input, native close, and child
  cleanup. It samples the real application RSS/threads/fds/children, enforces
  explicit growth/progress/runtime-error ceilings, binds results to the binary
  digest and candidate, and has 30-second plus scheduled ten-minute CI lanes.

- The core resource soak now promotes its real background transcript backlog
  from unvalidated log data to a fail-closed gate. Every iteration records
  accepted/dropped entries plus pre-shutdown and post-flush queue bytes; missing
  fields, excess backlog, any dropped entry, or a nonempty durable shutdown now
  fails. Deliberate regression fixtures prove each failure independently.

- The private Sway native lane now drives real Wayland virtual-keyboard input,
  keyboard-only copy mode through the compositor clipboard, one exact bracketed
  paste from `wl-copy`, split-pane creation/navigation/closure with PID-scoped
  routing, surviving-pane input, and native close. The same run now injects
  lost and outdated surfaces, changes/restores the Sway output after each,
  injects a device loss, and requires all three renderer recoveries before
  continuing. It remains distinct from real-desktop permission, vendor-GPU,
  and presentation evidence. Native
  schema-3 results bind the tested binary digest, compositor, UTC time, scenario
  outcomes, and exact recovery-injection flags.

- Native Linux candidate testing now has a schema-versioned evidence record
  covering real X11/Wayland visual and desktop policy, IBus/Fcitx 5 IME, Orca,
  fractional scaling, and vendor-GPU recovery. Exact environment and checklist
  coverage, derived dispositions, clean-worktree candidate binding, tested-
  binary identity, path confinement, and SHA-256 artifact sealing prevent
  incomplete or changed manual evidence from becoming an accidental pass;
  provider-backed workflows are excluded.

- A guarded Debian package acceptance runner now validates populated dependency
  metadata, file types and modes, desktop integration, icon format, and the
  installed terminfo entry. On an explicitly opted-in disposable Debian-family
  CI host it installs a synthetic predecessor, upgrades to the candidate,
  executes only the non-GUI version probe, removes the package, rejects owned
  file residue, and verifies user-state retention; pre-existing installations
  are never replaced. An uploaded schema-1 JSON report binds lifecycle evidence
  to the package digest, metadata, candidate commit, and recording time.

- Running GUI processes now publish separate process-scoped, user-private Unix
  control endpoints. `termeric control list [--json]` securely discovers all
  live processes and returns bounded, sanitized window/tab/pane/divider metadata with
  stable runtime IDs. Protocol v11 adds exact-process/source-window native
  window creation, exact-window local tab creation, and exact-tab/pane local
  splitting plus stable-ID native-window focus requests, tab selection, and
  pane focus, exact window/tab/pane close actions, stable-ID tab reorder and
  cross-window transfer, exact cross-tab/window pane transfer, bounded native
  window resize requests, and stable-ID local divider resizing. Generated
  128-bit request IDs and a bounded replay cache return recent successful
  results exactly, safely reevaluate failures, and reject ID/parameter
  collisions.
  Peer credentials,
  independent 256-bit tokens, strict framing/versioning, timeouts, concurrency
  limits, aggregate snapshot budgets, stale-endpoint handling, and control-
  character removal are enforced. Exact-pane `get-text` queries expose only
  presented UTF-8 text, optionally include at most 256 primary-history rows,
  retain newest content within 64 KiB, and cannot cross alternate-screen or
  synchronized-output presentation boundaries; cwd, argv, environment,
  clipboard, broad mutations, and provider data remain unavailable. A tab
  request returns only after the new stable tab ID is available. Live-domain
  tab and pane creation now defer the response without blocking the GUI event
  loop, reconcile the daemon's authoritative snapshot, and return the resulting
  GUI-local stable ID. Exact retries join an in-flight mutation, completed
  outcomes replay without reissuing it, and request-ID collisions fail closed.
  Selection validates the complete path before mutation;
  Wayland window focus fails honestly because the compositor owns activation.
  Local pane closure completes before acknowledgement; tab/window closure is
  explicitly scheduled. Surviving pane identity now persists across collapse
  to a single terminal, workspace capture, and later splitting instead of
  silently resetting to pane zero. Tab and pane moves validate both complete
  paths before detachment, reject live-domain topology changes, pinned-boundary
  violations, same-tab pane rearrangement, and plugin-owned pane detachment,
  and return fresh destination-local IDs where ownership namespaces change.
  Local split dividers now carry monotonic tab-local IDs rather than mutable
  structural paths; JSON schema 2 exposes their ownership, direction, and ratio
  through a required per-tab divider array and publishes native window pixel
  dimensions for asynchronous resize confirmation.
  Disabled-by-default `send-text` adds exact-pane UTF-8 input with a 64 KiB
  ceiling, printable/tab/newline validation, target-mode bracketed paste,
  explicit Enter, terminal replay semantics after any PTY error, and visible
  exact-destination destructive-command confirmation for plugin tabs. No test
  launches an authenticated provider integration.
  Exact divider resizing completes synchronously, while native window resizing
  is explicitly a request because the window manager can apply it later or
  clamp it. Live-domain tab/pane closure now waits for authoritative removal,
  reconciles surviving local pane identity, and protects the last pane/tab.
  Exact live-domain divider resizing captures the previous authoritative ratio
  and succeeds only when the reconciled snapshot proves the requested ratio.
  Same-window live-domain tab reorder translates an exact same-domain visual
  slot to daemon order and succeeds only after the stable local tab reconciles
  at that visual index. Domain protocol v10 adds exact pane-move anchor and
  direction fields; running-GUI pane movement between same-domain tabs proves
  source removal, exact destination layout, stable anchor identity, and a
  destination-local moved-pane ID before responding. Cross-window live-tab or
  pane placement remains rejected because daemon topology does not own native
  windows.

- Repeated benchmark evidence now fails closed if a producer omits a repeat
  record or changes its workload, renderer adapter, backend, or other required
  context. The metric merger independently rejects context drift so unlike
  samples cannot be summarized as one regression result. Historical baseline
  comparisons now require matching architecture, CPU identity/count, units,
  and per-metric context; incompatible rows are explicitly recorded as skipped
  rather than producing cross-machine percentages. Release-only system
  allocator counters expose requested bytes and successful calls for 100k
  scrollback construction and repeated history reflow without affecting
  application builds. Compact hard-broken history rows that already fit a new
  width now retain their content/style buffers instead of expanding to rich
  cells and recompressing on every resize. The 10k-history/50-resize fixture
  fell from 6.90 GB and 23.94 million allocation calls to 132.1 MB and 67,493,
  with median time falling from 739.4 ms to 86.5 ms; deterministic allocation
  rails prevent the old path from returning. Rows leaving the viewport now
  recycle their full-width cell allocation, ordinary REP state stores one
  scalar inline, and CSI/SGR uses VTE's bounded parameter storage without
  temporary heap vectors. The 100k styled construction fixture fell from
  938.0 MB and 5.89 million allocation calls to 144.4 MB and 900,075; its broad
  256 MiB/two-million-call rails prevent the churn from silently returning.
  A symbol-preserving optimized Cargo profile makes profiler stacks actionable
  while production release binaries remain stripped.
- Advanced workspace rename now clamps pre-existing OSC titles and every input
  route—including the named Space key—to its character ceiling while removing
  control/newline content. Broadcast arming has an exact ten-second scheduler
  deadline, and raw, key, and paste delivery immediately prune failed PTY
  targets or cancel below the two-pane safety floor. Plugin-tab guardrails
  retain safe pasted prefixes so later typed broadcast input is checked as one
  combined command.
- Declarative plugin registry and manifest reads now stop at explicit byte
  ceilings, while registry entries validate canonical-source shape,
  fingerprints, profile IDs, reserved namespaces, and grant invariants before
  discovery. Registry corruption is visible in Settings/toasts and fails closed
  instead of looking like an empty healthy registry. Normal update cannot add a
  permission even to a disabled plugin; a reviewed replacement is required.
  Plugin roots must be real directories, and removal uses a rollback-capable
  same-root quarantine around the registry transaction. Plugin launch no longer
  runs an untrusted executable synchronously with `--version` on the UI
  thread; the program is invoked exactly once as the real PTY child, with normal
  spawn and quick-exit diagnostics. The installed-Codex parser probe is now
  explicitly environment-gated so the default suite never launches a real AI
  CLI.
- Renderer backend selection now keeps forced GPU mode strict even when WGPU's
  preferred-adapter request returns a CPU device, and the software CI lane
  proves it actually selected a CPU fallback. If every initial renderer/window
  fails, Termeric preserves the actionable error through Winit shutdown and
  exits nonzero instead of only logging an invisible-window failure. A lost
  surface or outdated swap chain now gets a one-shot whole-renderer recovery
  after a short compositor grace period when no resize arrives, avoiding the
  X11 close race and an indefinitely stale live window while retaining terminal
  and PTY ownership. Native X11/Sway evidence records each surface injection
  separately from complete device loss.
- CI now validates the authoritative feature-support contract: every README
  highlight maps to explicit matrix rows, capability statuses use the declared
  vocabulary, rows retain boundaries and tracking, release gates cover required
  evidence domains, contributor guidance remains present, and issue links are
  canonical. Negative fixtures prove undefined statuses and drifted highlights
  fail closed. Previously undefined `Working` rows are conservatively `Partial`
  with their missing release evidence named explicitly.
- Release benchmarking now has a versioned 56-metric inventory. Every producer
  invocation must emit the exact named rows, units, sample counts, and required
  context; missing, unexpected, duplicate, malformed, or silently empty rows
  fail closed. Unavailable optional renderer/display probes remain explicit
  `skipped:` rows, and budget `allow_skipped` no longer turns an absent metric
  into a pass. The artifact records the contract result and exact errors.
- Harvest's Root Cellar now exposes bounded real command history, newest-entry
  selection, health/status metadata, keyboard/pointer/wheel navigation, and an
  explicit close action. Root Cellar and sprout menus use shared clamped
  render/hit-test/AccessKit geometry, isolated modal semantics, stable entry and
  request identities, and fail-closed stale actions. Rerun, Search Output, and
  Scroll To switch to the exact producing tab and pane before acting. OSC 133
  regions now use monotonic identities and signed scrollback coordinates that
  follow resize reflow; bounded 64 KiB output snapshots preserve copy/rerun
  context after live grid eviction, while an 8 MiB aggregate owned-snapshot
  ceiling prevents high configured history counts from growing without bound.
  Count- and memory-driven eviction totals and clipped entries are visible and
  accessible. Live-only search/scroll actions are disabled with exact evicted
  or alternate-screen reasons instead of reading replacement text; disabled
  pointer, keyboard, and semantic actions fail closed. Hermetic tests describe but
  never invoke provider-backed Explain. Previous/Next Prompt now walk a
  terminal-local navigation cursor in both directions, preserve its viewport
  during new output, follow reflow, reset after direct scrolling, and reject
  evicted or alternate-screen targets. OSC 133 live-text authority now follows
  ED2, ED3, RIS, and the Clear Scrollback action: erased regions retain bounded
  completion identity but cannot resolve replacement text, valid historical
  marks survive viewport reset, and alternate-screen programs cannot publish
  primary-shell marks.
- OSC 133 command tracking now validates the `A → B → C → D` lifecycle.
  Stray and duplicate boundaries are ignored, prompt-only or empty cycles do
  not create ghost Harvest commands, and a new prompt recovers a missing `D`
  only after output began. Exit statuses are restricted to the shell-valid
  0–255 range; malformed values close a valid command with unknown status.
- Audio-bell and desktop-notification helpers now launch through a shared
  bounded worker that reaps child processes and caps concurrent helpers.
  Immediate spawn, worker, or capacity failures are logged and surfaced as
  bounded, deduplicated warning toasts; a later successful launch clears the
  failure latch. Existing bell/notification rate limits remain authoritative.
- Terminal search and transient notifications now have native accessibility
  semantics. Search exposes bounded UTF-8 query editing, live result/error
  status, typed toggles, navigation, and close controls through geometry shared
  with rendering and pointer input. Toasts use stable identities, bounded
  messages, shared card bounds, modal-safe tree attachment, polite ordinary
  announcements, and assertive errors.
- Machine-evidenced VT/xterm conformance now requires every non-unsupported
  protocol claim to reference an actual test or deterministic fixture, replays
  44 focused cases plus pinned Vim/less/tmux traces, and checks every byte split.
  Parser fuzzing covers C0/ESC/CSI/OSC/DCS/APC, Kitty/UTF-8/grid/resize paths,
  validates visible and retained row structure, and enforces both line and byte
  history caps. Push/PR and scheduled CI use reviewed corpora and retain raw and
  minimized replay artifacts on failure.
- Release benchmarking now complements synthetic offscreen latency with an
  opt-in displayed X11 key-event-to-present probe. Five sequential samples
  cross PTY echo, terminal parsing, GPU submission, and surface presentation;
  headless runs report the metric as explicitly skipped rather than implying
  end-to-end coverage.
- Protocol-v9 live-domain relay geometry. Daemon-owned PTYs now start with
  nonzero bootstrap pixels and receive exact rows, columns, pixel width, and
  pixel height from renderer-backed attachments. Zero-pixel external parents
  preserve and proportionally scale the last valid extent, and exact repeats
  remain no-ops instead of creating duplicate `SIGWINCH` traffic.
- Protocol-v8 live-domain graphics snapshots. Reattachment now streams bounded
  decoded root and animation pixels outside the daemon lock, then atomically
  restores storage/LRU order, image-number aliases, anonymous protocol images,
  normal/alternate placements, Unicode prototypes, relative parent identity,
  revisions, and animation clocks before live output resumes. The private
  import channel is enabled only for a new authenticated relay replica,
  consumes itself at commit, emits no application replies, and never enables
  ordinary-file reads on either host.
- Bounded Kitty frame animation with direct and supported local-media loading,
  chunk continuations, partial/root edits, prior-frame and color canvases,
  source-over or replacement composition, stopped/loading/looping control,
  exact active-tab deadlines, finite loops, frame deletion/root promotion,
  synchronized-output isolation, renderer revisions, and independent
  frame-count/320-MiB memory limits.
- Bounded Kitty temporary-file and POSIX shared-memory image transfer, with
  format-correct zlib and `S`/`O` semantics, marked known-temp-root policy,
  private same-user object enforcement, device/inode replacement checks,
  mandatory cleanup, and hermetic security coverage. Unix ordinary-file
  transfer is available through an explicit default-off security policy, with
  absolute regular-file targets, protected-system-root denial, identity checks,
  bounded range reads, safe symlink resolution, and no deletion. Domain-owned
  PTYs and GUI relay replicas keep the medium disabled to prevent remote paths
  from becoming reads on the GUI host.
- Kitty image numbers now allocate collision-free client IDs, return both
  identifiers, resolve the newest retained match for placement and deletion,
  fall back coherently after deletion, and discard aliases on ID replacement
  or cache eviction. Advisory `N` usage hints are accepted without weakening
  the existing hard cache limits.
- Kitty Unicode placeholders now use bounded virtual placements, the complete
  protocol diacritic table, exact indexed/true-color image and placement IDs,
  high image-ID bytes, coordinate inheritance, aspect-preserving sub-cell
  crops, and grid-derived edit, reflow, scrollback, synchronized-output, and
  alternate-screen behavior. Prototype replacement, deletion, storage
  eviction, renderer clipping, and underline-color replay remain coherent.
- Kitty relative placements now support signed `H`/`V` offsets, stable `P`/`Q`
  parent identity, ordinary and Unicode-placeholder parents, 32-level bounded
  chains, exact missing-parent/cycle/depth errors, cursor invariance, negative
  edge clipping, scrolling, screen ownership, and cascading group lifetime.
- DEC Sixel macro and raster-attribute pixel aspects with bounded tall/wide
  display geometry, compact logical RGBA storage, nearest-pixel sizing, and
  fragmented end-to-end placement/cursor coverage.
- Window-local borderless fullscreen through F11, a configurable shortcut, and
  the command palette. Fullscreen is transient view state, remains independent
  across native windows, and is never written into workspace archives. The
  native acceptance lane now requires reversible Openbox/X11 and Sway/Wayland
  fullscreen geometry, exact PTY character/pixel resize, sibling isolation,
  and diagnostic artifacts before the capability can be promoted.
- Schema-2 pane-local workspace launch identity for mixed shell/profile tabs.
  Live pane transfers now retain their named relaunch profile and frozen split
  configuration across tabs, native windows, save/restore, and collapse back
  to a single owner; schema 0/1 archives migrate deterministically. Tab-owned
  plugin processes fail atomically when detached, while ordinary shell/profile
  splits beside a plugin remain movable.
- Bounded tab-bar overflow with active/focused-tab auto-reveal, pointer and
  keyboard navigation controls, shared render/hit-test geometry, and AccessKit
  semantics; crowded tabs no longer paint beneath or lose clicks to the global
  command, workspace, and settings controls.
- Process-local Move Tab to Next Window control preserving complete terminal,
  split-pane, plugin, pin, transcript, and Harvest ownership; destination-local
  identity/geometry are refreshed, emptied source windows close, and
  live-domain tabs fail atomically before detachment.
- Process-local Move Pane to Next Window control with deterministic native
  window ordering, destination-active-tab attachment, exact live PTY/state
  ownership transfer, geometry retargeting, visible safety failures, and atomic
  rejection for live-domain endpoints or a focused tab-owned plugin process.
- Per-activation broadcast target selection inside the active tab, with safe
  keyboard and mouse editing, a two-target confirmation floor, input blocking
  while armed, distinct amber/red pane and tab indicators, accessibility state,
  and exact selected-only real-PTY routing coverage.
- Bounded reusable terminal launch profiles with exact executable/argv,
  absolute cwd, initial title, and environment; profiles are available through
  `--terminal-profile`, hot-reloaded command-palette entries, profile-aware pane
  splits, and strict workspace restore references.
- Experimental OpenSSH stdio transport for remote domain CLI controls and
  authority-aware GUI attachment, with the selected host retained across
  topology, mutation, focus, reconnect, and every pane relay. It includes
  bounded saved host profiles, human/JSON inventory, one-time fail-before-launch
  resolution for controls and GUI attachment, and OpenSSH-owned per-host policy;
  plus a keyboard/mouse in-app picker with asynchronous discovery, stale-result
  suppression, authenticated snapshot preflight, and independent-window launch;
  strict known-host and non-interactive authentication policy, no forwarding or
  TTY, daemon-token non-disclosure, partial-frame-aware response deadlines, and
  hermetic fake-SSH lifecycle, relay, authority-isolation, and disconnect
  coverage.
- Protocol-v7 atomic live-domain pane movement between stable tabs across GUI
  and CLI. The daemon preserves the pane runtime, PTY, global ID, and every
  relay attachment while repairing source/destination trees and focus; attached
  GUIs transfer the existing relay owner instead of spawning a replacement.
- Protocol-v6 daemon-owned live-domain pane swap, recursive balance, and
  columns/rows/grid presets across GUI and CLI. Operations preserve pane IDs,
  focus, and relay PTYs; presets allocate fresh stable split identities and
  converge every attached window while zoom remains local view state.
- Protocol-v5 daemon-authoritative live-domain tab titles, pinned-prefix state,
  and stable-ID ordering across CLI, inline rename, shortcuts, and direct drag.
  Explicit titles outrank daemon-parsed OSC titles until cleared, rejected
  previews roll back atomically, and topology convergence preserves relay PTYs.
- Protocol-v4 stable live-domain split identities and validated 10%-90%
  `resize-split` control, with exact nested-divider drag targeting, optimistic
  GUI preview, rejection rollback, and monotonic multi-window ratio
  convergence. Ordinary nested pane drags now retain their mouse-down target
  instead of potentially jumping to an ancestor divider.
- Protocol-v3 local-domain topology subscriptions with atomic initial
  lightweight snapshots, monotonic whole-domain revisions, explicit
  domain-close frames, one-snapshot slow-reader bounds, retry/cancellation, and
  incremental multi-window relay reconciliation. Attached GUI New Tab and
  Split actions now mutate the daemon and appear in every attached window
  without reattachment.
- Protocol-v2 local-domain `close-pane` and `close-tab` controls with explicit
  last-pane/last-tab safety, deterministic tree/focus repair, child teardown,
  and attached-relay closure notification.
- Nonblocking close controls in attached live-domain GUIs, routed back to the
  originating native window with duplicate suppression, visible failures,
  relay-driven topology convergence, and stale pane-map repair.
- Opt-in VT420 DECRQCRA rectangular checksum reports with byte-exact DECCKSR
  responses, DECOM/clipping/single-page semantics, original DEC G-set byte and
  legacy rendition weights, deterministic conformance coverage, and a
  default-deny screen-scraping policy propagated across tabs, panes, restored
  sessions, live configuration, and domain-server terminals.
- VT400 DECSACE/DECCARA/DECRARA attribute-area operations with drawn-cell-only
  wrapped streams, exact rectangles, DECOM-relative endpoints, ordered
  set/reset and parity-cancelling reversal semantics, xterm hidden text,
  DECRQSS reporting, reset lifecycle, and deterministic conformance coverage.
- VT400 DECCRA/DECFRA/DECERA/DECSERA rectangular operations with overlap-safe
  snapshots, default/clipped and DECOM-relative coordinates, single-page
  normalization, SGR/DECSCA/BCE semantics, cursor stability, Unicode-wide
  repair, static-image coverage crop/copy/erase, deterministic conformance
  fixtures, and malformed-sequence rejection.
- DECLRMM/DECSLRM left/right margins with CSI s disambiguation, DECOM-aware
  cursor and tab behavior, bounded Unicode/BCE edits and rectangular scrolling,
  DECRQM/DECRQSS reporting, image placement movement, reset/resize handling,
  deterministic conformance evidence, and tmux-compatible terminfo extensions.
- DECSTR soft terminal reset with a documented VT/xterm reset-versus-preserve
  matrix, fragmented parser coverage, screen-local saved-state handling, and
  strict rejection of malformed parameterized forms.
- Complete supported DECSC/DECRC state across ESC 7/8, CSI s/u, and modes
  1048/1049: repeatable screen-local position, rendition, charset, DECOM,
  delayed-wrap, and DECSCA restoration with defined unsaved defaults.
- DECSCA protected-character state, DECSED/DECSEL selective erase, exact
  DECRQSS reporting, Unicode-wide-cell preservation, compact-history/reflow
  durability, and live-domain snapshot restoration.
- Bounded DECRQSS status replies for SGR, vertical/horizontal margins, and DECSCUSR, plus
  application-controlled blinking/steady block, underline, and beam cursors.
- Byte-exact DECRQM/DECRPM reports for the documented ANSI and DEC modes, with
  authoritative DECTCEM visibility and ATT610 cursor-blink rendering across
  reset, synchronized output, and live-domain attachment.
- Unicode-safe ECH/REP, standard relative cursor aliases, and complete
  CHT/CBT/TBC/DECST8C tab-stop behavior, now advertised through the conservative
  `xterm-termeric` terminfo entry.
- Pane- and screen-local Kitty progressive keyboard negotiation with bounded
  stacks, disambiguated/all-key input, alternate keys, event types, associated
  keyboard/IME text, and exact legacy fallback when no flags are active.
- Bounded xterm modifyOtherKeys and formatOtherKeys resource-4 negotiation with
  levels 0-3, queries/resets, modifier masks, CSI-27/CSI-u encoding,
  focused/broadcast pane ownership, Kitty precedence, AltGr preservation,
  S7/S8 replies, DECRQSS status, and live-domain replication.
- Bounded xterm modifyCursorKeys and formatCursorKeys resource-1 negotiation,
  including the disabled state and modes 0-4, exact legacy/private forms,
  xterm private-use cursor/edit-key codes in CSI-27 or CSI-u form, queries and
  all-resource resets, focused/broadcast pane ownership, S7/S8 and DECRQSS
  replies, reset lifecycle, and live-domain replication.
- Bounded xterm modifyFunctionKeys and formatFunctionKeys resource-2
  negotiation across F1-F35, including disabled `ctrlFKeys=10` remapping,
  modes 0-4, exact legacy/private forms, U+E2BE-U+E2E0 extended codes,
  CSI-27/CSI-u formats, query/reset/S7/S8/DECRQSS lifecycle, focused and
  broadcast routing, and live-domain replication.
- Bounded xterm modifyKeypadKeys and formatKeypadKeys resource-3 negotiation
  across 18 portable numeric-keypad keys and 11 Num Lock-off identities, with
  the distinct mode-0 default, conventional application-keypad modifier
  parameters, exact xterm
  private-use keysym codes in mode 4, CSI-27/CSI-u formats, query/reset/S7/S8/
  DECRQSS lifecycle, focused and broadcast routing, and domain replication.
- Bounded xterm modifyModifierKeys and formatModifierKeys resource-6
  negotiation with xterm's silent modes -1/0-3 and exact mode-4 private-use
  codes for 13 portable Winit modifier/lock identities, CSI-27/CSI-u formats,
  query/reset/S7/S8/DECRQSS lifecycle, focused and broadcast routing, Kitty
  precedence, and domain replication.
- Bounded xterm modifySpecialKeys and formatSpecialKeys resource-7 negotiation
  across 72 canonical Winit control, Compose/IME, and unambiguous XF86 keys,
  with exact fallback for otherwise-unmapped convertible Linux XKB keysyms,
  category-safe mode-4 CSI-27/CSI-u output, query/reset/S7/S8/DECRQSS
  lifecycle, pane routing, Kitty precedence, and domain replication.
- Exact xterm resource-0 `modifyKeyboard`/`formatKeyboard` negotiation state,
  including complete 16-bit round trips, unsigned disable reporting, direct and
  DECRQSS queries, reset lifecycle, focused-pane ownership, and live-domain
  replication. Runtime DECSET/DECRST 1060/1061 legacy and VT220 keyboard types
  now apply xterm-compatible F-key/editing-key/keypad translations and the four
  resource-zero modifier permission bits while normal remains encoder-neutral.
- Advanced window-local workspace controls: user-title precedence, pinned and
  draggable tab ordering, view-only pane zoom, swap/balance and row/column/grid
  presets, live-PTY pane transfer, configurable split cwd, and deliberately
  confirmed mode-aware broadcast input with persistent warnings.
- Added a conservative `xterm-termeric` terminfo entry, safe runtime discovery,
  Debian packaging, compatibility fallback, and ncurses/Vim/less/tmux checks.
- Event-driven PTY wakeups, centralized redraw reasons/deadlines, and idle-frame
  diagnostics.
- Incremental terminal-row rendering, persistent capacity-managed GPU buffers,
  per-row search/URL lookup caches, and allocation/invalidation diagnostics.
- Generation-checked background Git snapshots and bounded, batched transcript
  persistence with explicit backpressure and durable-shutdown reporting.
- Compact style-run scrollback with per-terminal line/byte limits, categorized
  memory diagnostics, and representation benchmarks.
- Headless App, PTY, terminal-protocol, plugin-exit, and optional WGPU smoke
  coverage.
- Persistent custom themes and automatic Bash, Zsh, and Fish semantic shell
  integration for experimental Harvest mode.
- Complete transcript Markdown/JSONL export and explicit persistence notices.
- Reproducible Linux packaging metadata, desktop integration, and CI gates.
- Linux process-resource soak testing with explicit RSS/thread/fd/child/GPU
  ceilings and uploaded diagnostics, plus macOS and Windows build/unit CI tiers.
- Bounded all-tab/all-pane PTY polling with origin-tagged app events and local
  terminal protocol replies.
- A bounded eight-page glyph texture array with frame-safe LRU reclamation,
  visible exhaustion fallback, page-aware row caching, and atlas diagnostics.
- Native X11 and Wayland acceptance with isolated PTY transcripts, desktop
  spies, Vim/less/tmux scenarios, surface-loss recovery, reviewable GPU visual
  signatures, and uploaded reproduction artifacts.
- Required real-PTY Bash/Zsh/Fish semantic-region acceptance and hermetic
  end-to-end coverage for every built-in Claude Code and Codex command.
- Experimental persistent local terminal domains with same-UID/token
  authentication, daemon-owned PTYs, bounded multi-client streaming, complete
  CLI control, nested layout/focus state, and native GUI detach/reattach.
- Pane-local keyboard copy mode with vi/Emacs motions, search-driven Unicode
  selection, logical reflow bookmarks, and boxed quick-select hints for OSC 8
  links and validated configurable patterns.
- Bounded static Sixel and iTerm2 PNG support with real-client native
  acceptance, exact pixel placement, shared cache/render normalization,
  scroll/resize/alternate-screen lifecycle, and dedicated fuzz coverage.
- Platform IME composition with pane-positioned preedit and focused commit
  routing, a bounded native-color glyph atlas, F6 keyboard-only tab-bar focus,
  and incremental AccessKit screen-reader semantics for tabs, panes, visible
  text, cursor, selection, and named controls.
- Automatic hardware-to-WGPU-software renderer fallback, strict CLI/config
  selection, adapter diagnostics, and bounded whole-device recovery that keeps
  terminal processes alive and discards all adapter-owned caches.
- Declarative third-party plugin API v1 with sourced local installation,
  explicit process-launch grants, bounded manifests, live enable/disable and
  updates, palette/profile/workspace integration, diagnostics, and a complete
  sample/authoring contract.

### Changed

- Terminal search now scans retained history in cancellable render-frame
  slices, publishes results atomically, retains exact totals and filter rows,
  bounds recent navigation/highlight occurrences below an 8 MiB index budget,
  and virtualizes wheel-scrollable filter rendering to one viewport.
- Unchanged windows now sleep; cursor blink, UI animation, config, Git, and
  toast lifecycle work runs only at explicit deadlines.
- Unchanged terminal rows now reuse their glyph/rectangle geometry and GPU
  buffers; cell, search, URL, and viewport changes rebuild affected rows while
  global rasterization/layout changes invalidate the cache explicitly.
- Git completion now filters worker-published branch snapshots, while transcript
  redaction, serialization, cap accounting, writes, and flushes run off the
  terminal input/output path.
- Pane output now reaches transcript/diff/activity hooks even when its tab or
  pane is inactive, while Harvest continues observing each terminal separately.
- Scrollback reflow now consumes one logical line at a time instead of cloning
  the complete rich-cell archive.
- Updated the Codex integration for current approval, sandbox, and resume CLI
  contracts.
- Unified live configuration application across startup, settings, reload,
  reset, plugins, shortcuts, and Harvest.
- Split App orchestration into focused configuration, desktop I/O, plugin,
  session, workspace, and mouse modules.
- WGPU instance creation now honors backend/debug environment selection, which
  permits deterministic llvmpipe GUI lanes without binding the wrong display.
- Fish 4 startup capability probes now receive bounded, truthful Kitty keyboard,
  XTVERSION, OSC background-color, and required XTGETTCAP replies; Fish 4 uses
  its native OSC 133 marks while older Fish releases retain fallback hooks.
- Added bounded, terminal-local OSC 7 working-directory reports with strict
  percent-decoded UTF-8 paths, safe local/remote authority separation,
  Bash/Zsh/Fish prompt emission, and focused title, Git/plugin, pane-spawn,
  persistence, and local-domain cwd routing.
- Added exact, terminal-local read-only XTWINOPS reports for text-area pixels,
  rounded cell pixels, and rows/columns in S7C1T or S8C1T framing, while leaving
  window mutation, title, outer-window, screen, and DECSLPP operations disabled.
- XTGETTCAP now returns one bounded xterm-compatible multi-name response for
  Termeric's native identity, color depth, Fish extensions, and every special
  key advertised by its terminfo entry, including termcap aliases and S8C1T
  framing; malformed or unknown names terminate the list without overclaiming.
- Added versioned complete-workspace archives with atomic replacement and
  backup recovery, nested pane topology/focus, ordered shell and plugin tabs,
  exact named-profile relaunch, bounded deduplicated scrollback, partial
  recovery, last-workspace startup restore, and save/manage/rename/delete UI.
- Added one-process multi-window ownership and deterministic CLI startup plans
  for grouped windows/tabs, shell splits, Unicode cwd, direct commands and
  exact argv, plugin profiles, workspaces, titles, pixel geometry, and Linux
  application identity; closing or losing one window no longer exits siblings.

### Fixed

- Native GUI and displayed-soak fixtures no longer generate an invalid font
  configuration by repeating the primary family as its own fallback. Their
  isolated shell fixture now launches reliably instead of silently giving way
  to the default shell after configuration validation fails.
- Private-Wayland acceptance now reconstructs fragmented PTY input per process,
  activates each short-lived virtual keyboard before injecting tested input,
  serves one-shot clipboard data without deadlocking the driver, and uses
  owned short runtime paths that cannot exceed Unix socket limits. The complete
  interaction scenario and displayed-process soak also retry a bounded input
  request only when its exact observable effect is still absent.
- Native renderer-failure acceptance now arms lost-surface, outdated-surface,
  and device-loss hooks one at a time through inert one-shot trigger files.
  Combined recovery runs can no longer collapse multiple faults into one
  recovery before the harness observes them. Wayland fullscreen evidence also
  accepts the compositor-valid case where only one PTY pixel axis expands,
  while still rejecting shrinkage, sibling changes, or inexact restoration.
- Displayed Wayland soak reports now use schema 2, disclose worktree dirtiness,
  and offer a fail-closed `--require-clean` candidate mode. Change and
  scheduled CI lanes require that mode, so a commit plus binary digest can no
  longer be mistaken for clean-candidate evidence when tracked files differ.
- Hermetic AI-integration acceptance now has a candidate-bound runner that
  resolves the exact compiled libtest binary, strips the installed-Codex opt-in,
  executes only the fake Claude/Codex real-PTY workflow, and seals its logs and
  machine result into the versioned integration record. CI uploads that partial
  record while leaving shell, desktop, Harvest, and provider rows unrecorded.
- The integration acceptance record CLI now bootstraps the repository import
  root, so its documented direct `python3 scripts/...` template, seal, and
  validate commands work independently of the caller's working directory or
  ambient `PYTHONPATH`.
- Kitty graphics now interoperates with the current `kitten icat` wire format:
  bounded 128-KiB direct chunks, raw-unpadded standard base64, and bare POSIX
  shared-memory names are accepted while existing image and local-media limits
  remain enforced. Malformed or oversized commands honor `q=2` before emitting
  any reply, preventing protocol error APCs from leaking into the invoking
  shell after a quiet client exits.
- Kitty placements without explicit rows or columns now render at natural
  pixel size and derive exact cell footprints from live PTY geometry; supplying
  one axis derives the other from the source aspect ratio. Unicode-placeholder
  cells are coalesced into horizontal renderer spans, preventing current
  `kitten icat` images from being truncated by the bounded instance ceiling.

- The displayed-process soak now requires exact Kitty success replies for its
  repeated image replacements. Those commands previously used reply-suppressed
  mode, so ordinary PTY progress could hide a completely broken graphics path.

- Displayed-process soak reports now count and require output resizing and pane
  churn independently. Their former aggregate action check could stay green if
  one entire workload class disappeared while the other kept incrementing.

- Core and displayed-process soak reports now require at least two `/proc`
  samples and half of the requested sampling cadence. A stalled or
  misconfigured one-sample run previously reported zero resource growth and
  could pass the memory-stability checks without a meaningful time series.

- Native GUI acceptance prerequisites now match the selected scenario. The
  shared PTY fixture previously ran `img2sixel` and `timg` for unrelated
  fullscreen, CLI, Wayland, and post-application checks, while X11 clipboard
  and capture tools were required even by scenarios that never used them.

- Native Vim acceptance now requires the editor to exit successfully and the
  surviving Termeric window to honor a normal desktop close. Previously a file
  write alone could pass before cleanup forcibly terminated a hung terminal.

- The native graphics acceptance fixture now actually executes and emits both
  `img2sixel` and `timg -pi` output. Their loop had been unreachable after the
  soak-frame return, making the advertised real-client acceptance path time
  out before either client record could exist. Fake-executable regression
  coverage now proves both independent streams reach the fixture output path.

- AccessKit now exposes selections that intersect the visible scrollback
  viewport instead of dropping all selection semantics away from the live
  bottom. Long IME preedit runs are clipped at grapheme boundaries to the
  focused pane's remaining width and no longer paint across pane dividers.
- Sixel now rejects undefined background selectors instead of silently treating
  them as opaque, and its supported scrolling profile retains the text cursor's
  starting column on the next complete line as VT340/xterm specify. The
  unsupported DECSDM and xterm private mode variants are now explicit in the
  capability contract.
- Keyboard copy mode now steps over wide-grapheme spacer cells in both
  directions, honors the documented `Ctrl+Home`/`Ctrl+End` history jumps, and
  rebuilds quick-select coordinates after resize reflow. Wrapped OSC 8 links
  suppress overlapping regex hints on every covered continuation row, while
  soft-wrapped configured matches retain their complete value. Relative path
  activation now inherits only the focused terminal's trusted local OSC 7
  directory, and Windows opening no longer passes targets through `cmd.exe`.
- Live-domain input, attached GUI input, and terminal-generated replies no
  longer perform blocking PTY writes while holding the daemon's global state
  lock. Each pane now has a four-frame/256 KiB bounded writer queue with
  immediate backpressure failure, isolating a child that stops reading from
  unrelated PTY drains and controls.
- Shell, direct-command, and terminal-profile CLI working directories are now
  validated before winit connects to a display or queues a native window.
  Missing and non-UTF-8 PTY paths produce an ordinary nonzero startup error
  instead of a hidden post-window failure followed by successful process exit.
- Partial workspace recovery now remaps `active_tab` to the exact surviving
  source tab after filtering earlier corrupt entries. It falls back to the
  first recovered tab only when the saved focused tab itself is unavailable.
- Renaming a workspace recovered from `workspace.toml.bak` no longer replaces
  that last known-good copy with the corrupt primary before installing repaired
  metadata. A later interrupted write or second primary corruption can still
  fall back safely.
- The hermetic Claude Code/Codex acceptance fixture now completes context
  insertion and profile selection instead of stopping at their dialogs, and
  checks exact initial/restart/resume argv, inherited cwd, terminal geometry,
  and the payload received by the fake PTY. It still starts no provider-backed
  process and consumes no AI-service tokens.
- Native `xterm-termeric` PTYs now pin `TERMINFO` to the exact database root
  whose compiled entry justified selecting the custom identity. An inherited
  conflicting `TERMINFO` can no longer outrank `TERMERIC_TERMINFO_DIR` and leave
  the child with a stale or undiscoverable entry; ordered `TERMINFO_DIRS`
  fallbacks remain available.
- Linux soak child-process accounting now aggregates every task thread's
  `/proc` children list instead of sampling only the process leader. PTYs and
  helpers spawned from Rust worker threads therefore participate in concurrent
  child ceilings and final leak checks, with duplicate PIDs counted once.
- Slow Git repositories no longer starve status and branch-completion updates
  when a scan exceeds the two-second polling interval. Periodic same-directory
  refreshes preserve the publishable in-flight generation and coalesce exactly
  one follow-up; cwd changes and explicit invalidations still reject stale
  results.
- URL render/hit-test caching now fingerprints terminal-column mappings as well
  as text, so wide-cell geometry changes cannot retain stale link columns.
  Per-row matches are position-indexed and binary-searched instead of linearly
  rescanned for every rendered cell; nested schemes resolve to the containing
  outer URL.
- Occluded and surface-lost windows no longer retain 16 ms cursor/text blink,
  UI, toast, or Kitty-animation wake deadlines when they cannot present.
  Pending dirty state, PTY draining, configuration/Git maintenance,
  shortcut expiry, and synchronized-output safety deadlines remain active and
  animations catch up from monotonic time after exposure or recovery.
- Alternate-screen applications no longer retain a second compact scrollback
  archive. Their viewport always has zero history limits, zero-limit scrolling
  avoids transient compaction, legacy alternate history is cleared on entry,
  and the configured per-terminal byte limit now bounds total primary history
  rather than permitting nearly one full budget per screen buffer.
- Make terminal-search editing follow its visible UTF-8 cursor instead of
  appending and deleting only at the end; cap replacement/insertion at 4 KiB,
  restore a visible empty-query affordance, and add explicit dismissal for
  retained results.
- Make context assembly a request-scoped accessible modal with shared geometry,
  a real workspace-bounded Specific Files editor, bounded timeout-controlled
  background gathering, visible failure/status handling, and exact stable
  tab/pane injection that cannot retarget a replacement plugin tab.
- Make diff review a bounded, two-axis-scrollable, request-scoped modal with
  shared render/pointer/AccessKit geometry, visible-row-only frame handoff,
  explicit Copy/Done controls, and exact stable tab/diff review targeting.
- Make reusable information/error notices window-bounded and accessible, with
  two-axis scrolling, retained Copy/Continue controls, visible-only document
  semantics, explicit pointer/screen-reader profile choices, shared geometry,
  and request identities that prevent replaced notices from accepting stale
  actions.
- Make destructive-command guardrail confirmation request-scoped and
  destination-stable: cancellation revokes buffered input before the closing
  animation ends, stale actions cannot retarget later prompts, paste and pane
  broadcast are intercepted before delivery, and keyboard, pointer, and
  AccessKit use explicit safe-default Cancel and Run-anyway actions.
- Treat glyph-atlas exhaustion caused by current-frame page pins as temporary:
  scalar and shaped lookups now deduplicate replacement-glyph work within the
  frame and retry after pins are released, without persisting a pressure
  fallback in the shaping cache. Permanently oversized glyphs remain bounded
  and cached as visible replacements.
- Report nonzero physical grid dimensions through PTY spawn and resize for
  single tabs and nested panes, including pixel-only scale/font changes.
- Exit the native event loop when the final PTY closes during idle-source
  processing instead of leaving an empty window asleep indefinitely.
- Correct active-plugin routing, dynamic command execution, and visible Git,
  clipboard, plugin, session, and transcript failure feedback.
- Correct UTC session timestamps, collision-resistant identifiers, and visible
  5 MB scrollback / 50 MB transcript limits.
- Avoid X11 close/surface-loss races that could panic or continuously redraw an
  already-destroyed native window.
- Return DSR and other terminal-generated replies to the exact pane PTY and
  address app-mediated replies by stable tab/pane identity instead of focus.
- Complete bounded OSC 0/2 titles, policy-controlled OSC 52 clipboard access,
  and terminal-local OSC 4/10/11/12 palette/default color queries and resets.
- Return conservative byte-exact DA1/DA2/DECID identities; render complete
  resident ASCII, UK, and DEC Special Graphics G0-G3 state with single and
  locking shifts; and preserve that state through save/restore and reset.
- Add byte-exact, bounded VT420 DECRQPSR cursor/tab reports and atomic DECRSPS
  restore for the supported rendition, protection, charset, shift, origin,
  delayed-wrap, cursor, and tab-stop state.
- Accept supported raw C1 controls without confusing valid UTF-8 continuation
  bytes, and add terminal-local S7C1T/S8C1T reply framing across core CSI/DCS/APC
  and origin-addressed OSC clipboard/color responses.
- Normalize ECMA-48 GR equivalents throughout supported ESC/CSI/DCS/OSC/APC
  protocol bodies and two-byte ST while preserving UTF-8 titles, hyperlinks,
  and notifications.
- Add bounded DECSCL levels 1-5 with DECSTR-style soft reset, level-aware
  7/8-bit reply selection, RIS defaults, DECRQSS reporting, and fragmented
  conformance coverage while explicitly excluding strict feature gating.
- Replace the nonfunctional Kitty APC path with a bounded streaming static-image
  profile: direct RGB/RGBA/PNG, zlib, chunking, queries, placement/crop/z-order,
  lifecycle/deletion, cache invalidation, and capability-accurate replies.
- Route input, selection, search, scrolling, links, working-directory context,
  sessions, synchronized output, and pane-local images through the focused live
  terminal, with focus-change cache rebinding and detached-owner guards.
- Adopt the existing PTY on first split without a replacement shell, terminate
  dropped pane children, preserve live output through dissolve, inherit cwd for
  new panes, and render dividers directly from current tree geometry.
- Save cwd, last-command metadata, and bounded scrollback from one stable
  focused-pane source; label the reduced split scope, retain legacy session
  compatibility, describe restores honestly as new shells, and capture
  simultaneous all-pane exits before topology cleanup discards that source.
- Preserve complete Unicode grapheme clusters with terminal widths, atomic wide
  pairs, edit/reflow/search/copy/export invariants, and full-cluster GPU shaping.
- Render every stored SGR text attribute through one resolved style path,
  including real/fallback bold and italic faces, dim, hidden, slow/rapid blink,
  strikethrough, five underline styles/colors, pane clipping, and cursor or
  highlight composition; `font.bold_is_bright` is an explicit opt-in policy.
- Encode arrows, Home/End, the physical numeric keypad, function/editing keys,
  modifiers, Control, Alt, AltGr, and Unicode text from the focused terminal's
  DECCKM/DECKPAM state with shortcut-first dispatch.
- Route plugin commands only to live tabs, so a restart cannot send immediate
  follow-up input back to the tab already closing.
- Emit focused-pane X10/VT200/button/any-event mouse reports with UTF-8 or SGR
  coordinates, exact focus transitions, DECSET 1007 alternate scroll, and
  one-write bracketed paste framing; Shift remains the native-selection bypass.

### Known limitations

- Termeric is a Linux developer preview, not yet a general beta. The
  [feature-support matrix](docs/feature-support.md) is the authoritative list
  of working, partial, broken, experimental, unverified, and unsupported paths.
- Linux Wayland/X11 have launch and short resource evidence but open correctness
  and interaction gates; macOS and Windows build/unit tiers do not yet establish
  runtime support.
- Kitty graphics uses the documented bounded static and frame-animation
  profile, including decoded live-domain reconnect snapshots. Bidirectional
  text reordering remains unsupported; native IME,
  color-font, and screen-reader acceptance is still required on each release
  platform.
- Harvest is experimental and disabled by default.
- AI transcript redaction is best-effort, not a complete secret scanner.
- Interactive Claude/Codex sessions require their separately installed CLIs
  and user authentication.
