# Competitive capability audit

Audited 2026-09-11 against the current working tree. This document is a
planning comparison, not a release claim. Termeric's authoritative status and
evidence boundaries remain in [feature-support.md](feature-support.md).

## Bottom line

Termeric's terminal engine is already credible enough to compete on breadth:
modern keyboard negotiation, Unicode graphemes, rich SGR, synchronized output,
three image protocols, bounded scrollback/reflow, tabs, nested panes, complete
workspace relaunch, copy mode, quick select, and persistent local/SSH domains
are implemented. Its uncommon advantage is the combination of those terminal
features with guarded broadcast, Git/diff/transcript workflows, bounded OSC 133
command history, declarative plugins, and explicit resource/evidence contracts.

It is not yet the best general recommendation over established terminals. The
current product is a Linux developer preview; accepted real-compositor,
hosted Debian/Arch package-lifecycle, accessibility/IME, vendor-GPU, and
release-duration evidence is incomplete. Its automation and extension surface is also substantially less
capable than kitty's remote control/kittens or WezTerm's Lua and multiplexer
APIs. Established projects additionally have distribution reach, user history,
documentation depth, and compatibility exposure that this repository cannot
replace with unit-test count.

The practical position is therefore:

- stronger than a minimal emulator such as Alacritty when a user wants
  integrated panes, workspaces, graphics, shell semantics, and developer tools;
- potentially differentiated for guarded AI/developer workflows and bounded
  persistent workspaces;
- not ready to displace kitty, WezTerm, Ghostty, or Konsole as a low-risk
  general daily driver until the release-evidence and automation gaps below are
  closed.

## Current comparison

| Area | Termeric now | Mainstream reference point | Standing |
|---|---|---|---|
| Terminal correctness | Broad tested VT/xterm subset, Kitty keyboard, xterm keyboard resources, synchronized output, Unicode graphemes, rich SGR, mouse/focus/paste, truthful terminfo | kitty and Ghostty explicitly target modern/xterm-compatible protocol breadth; WezTerm covers modern keyboard, mouse, images, and text attributes | Competitive in implemented breadth; native-client interoperability evidence is thinner |
| Text and graphics | Ligatures, color emoji, bounded glyph atlases, bounded static window backgrounds, partial Kitty graphics including animation/placeholders/relative placement, static Sixel and iTerm2 PNG | kitty is the protocol origin and supports images/animation/background layouts; WezTerm advertises Kitty, iTerm2, experimental Sixel, and image backgrounds; Ghostty advertises Kitty graphics and background images | Broad, but still partial until visible native-client/background acceptance and remaining protocol variants pass |
| Tabs, panes, windows | Multiple native windows, live top/vertical-left tabs, nested splits, zoom/swap/balance/presets, drag resizing, tab/pane transfer | kitty provides tabs and seven layouts; WezTerm and Ghostty provide windows/tabs/splits; Konsole provides split views, detach, maximize, and saved layouts | Competitive; kitty has more layout variants and mature interaction polish, while Termeric's new left rail still needs native acceptance |
| Persistence | Atomic schema-2 workspaces preserve mixed pane launch identity, topology, metadata, and bounded scrollback; legacy snapshots are separate | kitty session files describe startup layouts; WezTerm has workspaces/domains; Konsole can save tab layouts | Strong and unusually explicit, but relaunch is not process checkpointing |
| Search and keyboard navigation | Cancellable full-history literal/regex search, filter mode, copy mode, configurable quick-select hints, prompt navigation | Alacritty exposes search, Vi mode, and regex hints; WezTerm exposes search, copy mode, and QuickSelect; kitty exposes searchable history and hints | Competitive, with stronger deterministic memory/latency boundaries than most published feature lists |
| Multiplexing and remote work | Experimental daemon-owned PTYs, local/SSH domains, detach/reattach, topology mutation, GUI picker, bounded relays | WezTerm supports local Unix, SSH, and TLS domains plus an embedded SSH client; kitty provides SSH tooling and a powerful remote-control protocol | Promising but behind WezTerm in transport maturity and behind kitty in application automation |
| Extensibility and automation | Fuzzy palette, custom commands, bounded declarative process profiles, and APIs v2-v31 for permissioned context/events, persistent handlers, current-window inventory/navigation/topology, namespaced commands, typed settings, presented-text reads, ordinary pane/tab lifecycle, cross-tab pane movement, guarded exact-pane input, same-tab layouts, command-scoped exact-window closure, process-wide read-only inventory, exact non-activating cross-window tab/pane navigation, bounded cross-window presented-text reads, guarded cross-window UTF-8 terminal input, exact cross-window ordinary tab/pane creation, exact cross-window ordinary-pane and ordinary-tab movement, same-window tab reorder, exact cross-window ordinary pane/tab closure, exact cross-window ordinary pane swap/balance/layout presets, and result-gated closure of an exact non-final process-owned window. The in-app manager exposes grants and typed settings; the domain and authenticated GUI CLIs provide a broader typed control surface. | kitty offers Python kittens, event watchers, and remote control; WezTerm exposes a large Lua configuration/action/event API; Konsole exposes desktop-oriented profiles/plugins | Gap narrowed again; plugin installation remains CLI-only, while native activation, plugin-requested final process-window termination, live-domain pane layouts, raw output/raw-byte input, arbitrary layout trees, broader destructive actions, and render hooks remain absent |
| Shell/developer workflows | Accepted OSC 7/133 for Bash/Zsh/Fish, acceptance-pending native Nushell wiring, opt-in Elvish module, Git status/completion/diff, transcripts, guarded plugin input, Harvest command history | kitty and Ghostty use shell integration for prompt navigation, cwd inheritance, command-output selection, and SSH helpers | Differentiated combination, but Nushell/Elvish PTY evidence, automatic Elvish setup, and real-provider/manual acceptance are incomplete |
| Accessibility and input | AccessKit semantics for terminal/chrome/modals plus IME/preedit routing and tests | Ghostty emphasizes native platform UI; mainstream projects vary in screen-reader coverage | Architecturally serious, but remains Partial until Orca/real IME and desktop bridge acceptance exists |
| Performance/resources | Event-driven scheduler, incremental row cache, persistent GPU buffers, compact byte-bounded history, allocation/RSS/latency/soak contracts, and a fail-closed external comparison runner | kitty, Ghostty, and Alacritty explicitly market performance; kitty publishes its own throughput tool | Optimized in several measured subsystems; one local Termeric/Ghostty artifact permits startup and PTY-response comparison, while its pixel mismatch correctly rejects RSS/idle ranking. No clean release or vendor-GPU artifact supports a fastest/best claim |
| Platforms/distribution | Linux developer preview; Debian and maintained Arch `termeric-git` artifact construction with guarded disposable lifecycle lanes; one exact Arch candidate passes locally in the pinned official CI container; macOS/Windows build-and-unit CI only | kitty supports Linux/macOS/BSD; WezTerm and Alacritty support Linux/macOS/Windows/BSD; Ghostty supports Linux/macOS; Konsole is mature in KDE/Linux distribution channels | Arch coverage narrows the Linux installation gap, but absent published repositories/AUR release, hosted artifact history, broader runtime platforms, and adoption history remain major blockers |

Official comparison sources: [kitty overview](https://sw.kovidgoyal.net/kitty/overview/),
[kitty extension catalog](https://sw.kovidgoyal.net/kitty/kittens_intro/),
[kitty remote-control protocol](https://sw.kovidgoyal.net/kitty/rc_protocol/),
[WezTerm features](https://wezterm.org/features.html),
[WezTerm multiplexing](https://wezterm.org/multiplexing.html),
[WezTerm action API](https://wezterm.org/config/lua/keyassignment/index.html),
[Ghostty features](https://ghostty.org/docs/features),
[Ghostty shell integration](https://ghostty.org/docs/features/shell-integration),
[Alacritty features](https://alacritty.org/index.html), and
[Konsole command reference](https://docs.kde.org/trunk_kf6/en/konsole/konsole/commandreference.html).

## What Termeric offers over common alternatives

These are defensible advantages of the current design, not claims of universal
superiority:

1. **One bounded developer workspace model.** Mixed shell/profile/plugin panes,
   topology, titles, cwd, command metadata, and scrollback share one versioned,
   partially recoverable archive rather than relying only on startup scripts.
2. **Safety-aware broadcast and plugin input.** Per-activation target selection,
   a two-target floor, distinct armed/live states, failed-target pruning, and
   exact-destination destructive-command confirmation are unusually deliberate.
3. **Semantic command history as terminal state.** OSC 133 regions survive
   scroll/reflow with stable identities, bounded retained output, explicit
   eviction, and exact rerun/search/copy routing. Harvest builds on this without
   making an AI provider part of the terminal's correctness path.
4. **Resource limits are product behavior.** Scrollback, images, search results,
   transcripts, helpers, plugins, and relays have explicit ceilings and visible
   degradation instead of relying only on eventual allocator pressure.
5. **Evidence is fail closed.** A fixed provider-free preflight binds source
   quality gates, complete logs, the clean candidate, and release binary into
   one tamper-checked artifact. The benchmark contract rejects missing,
   duplicate, context-drifted, or incompatible metrics; the opt-in external
   runner separately rejects rankings when selected terminals, grids,
   retention, or native pixel geometry differ; the feature matrix
   distinguishes implementation from native acceptance.
6. **Protocol breadth and workspace features coexist.** Alacritty intentionally
   delegates tabs/multiplexing; desktop terminals often lack modern graphics or
   keyboard protocols; workflow terminals may not expose Termeric's bounded
   resource and conformance contracts.

## Gaps that block sufficiency

These are release requirements, not optional competitive flourishes:

1. **Finish Linux release evidence.** Obtain green revised X11 and Wayland
   native artifacts, then record real-compositor input/scale/visual checks,
   native IME and Orca, clipboard/portal/notification behavior, vendor-GPU
   recovery/idle behavior, and accepted hosted Debian and Arch
   install/synthetic-upgrade/uninstall artifacts. A real prior-release upgrade
   remains later distribution evidence.
   This is the unfinished acceptance work in issues #17, #23, #26, #32,
   #33, and #35.
2. **Accumulate stability and comparative evidence.** Retain the clean local
   ten-minute displayed-Sway result and obtain green scheduled core/displayed
   resource-soak history, then run release-duration real-
   desktop/vendor-GPU observation, retain benchmark history by compatible
   machine/adapter context, record a same-host external-terminal comparison
   whose per-metric contract is green, and
   add crash/recovery dogfooding. Thirty-second fixtures cannot establish a
   daily-driver lifetime.
3. **Productionize domains.** Complete native remote-host GUI acceptance,
   reconnect/error UX, and an explicitly supported transport story. A native
   TLS transport is not required for a first Linux beta if OpenSSH remains an
   accepted limitation, but silent authority or reconnect ambiguity is not.
4. **Make issue/release state truthful.** Several roadmap issues remain open
   even where their implementation rows are Supported. Each needs an acceptance
   checklist comment and then closure or a narrowed residual scope; otherwise
   the tracker no longer communicates actual risk.

## Missing competitive capabilities

The following are genuinely absent or intentionally narrow, rather than merely
awaiting evidence:

### Highest leverage

- **Complete control of an already-running GUI.** Ordinary GUI processes now
  have private authenticated multi-process discovery, bounded read-only
  window/tab/pane inventory with stable runtime IDs, plus replay-safe exact
  creation, focus/selection, closure, tab reorder/window transfer, and pane
  transfer between ordinary tabs/windows, bounded native-window resize
  requests, stable-ID local divider resizing, bounded exact-pane presented-text
  queries, and disabled-by-default exact typed input with paste/Enter semantics.
  Live-domain tab/pane creation, closure, exact divider resize, representable
  same-window tab reorder, and exact same-domain pane movement now wait for an
  authoritative snapshot and return reconciled GUI-local state. Cross-window
  live placement remains an explicit topology-model boundary rather than an
  optimistically acknowledged action.
  That narrower typed surface is still
  the largest gap against kitty remote control and WezTerm's CLI/Lua automation
  and belongs in the remaining scope of issue #29.
- **Richer extension hooks.** API v2 adds an explicitly granted, 16 KiB,
  schema-versioned read-only launch-context snapshot while preserving API v1.
  API v3 now provides coarse app/tab lifecycle subscriptions on a bounded
  process worker and accepts only permissioned message/notice responses. API
  v4 adds permission-gated, bounded semantic command-completion regions with
  exact source identity and stale-content refusal. API v5 adds bounded,
  request-correlated persistent handler processes with deterministic teardown.
  API v6 adds an explicitly granted, bounded, non-chainable live inventory of
  the originating window. API v7 adds separately granted current-window stable-
  ID tab selection and pane focus with correlated results and explicit loop
  prevention. API v8 adds independently granted local tab reorder and divider-
  ratio actions. API v9 adds bounded palette commands dispatched only to their
  owning handler without expanding its grants. API v10 adds typed non-secret
  settings with explicit persistence and fail-closed schema evolution. API v11
  adds a separately granted, bounded exact-pane query of presented terminal
  text with stable targeting and host-operation loop prevention. API v12 adds
  separately granted exact ordinary-pane creation with inherited launch/cwd
  policy, correlated stable identity, and the same loop prevention. API v13
  adds separately granted ordinary local-tab creation in the originating
  window, with correlated stable identity and lifecycle-loop suppression.
  API v14 adds separately granted exact ordinary-pane closure, with stale and
  unsupported-target refusal, last-pane protection, split collapse, correlated
  state, and complete result-authority removal.
  API v15 adds separately granted ordinary local-tab closure scheduling, with
  exact targets, final-non-closing-tab protection, deterministic selection,
  truthful scheduled results, and lifecycle-loop suppression. API v16 adds
  separately granted exact ordinary-pane movement between current-window tabs,
  preserving the live PTY while returning its fresh destination-local identity
  and resulting focus state. API v17 adds separately granted exact-pane bounded
  terminal input behind the existing disabled-by-default global policy, with
  live bracketed-paste encoding and destructive-command confirmation.
  API v18 adds separately granted exact same-tab pane swap, balance, and
  columns/rows/grid presets while preserving terminal owners, pane identity,
  focus, and background-tab selection.
  API v19 adds separately granted closure of the exact invoking window, only
  from a declared plugin command and only after its correlated final callback.
  API v20 adds separately granted, bounded process-wide read-only inventory
  with creation-ordered stable window IDs and explicit origin identity.
  API v21 adds separately granted exact tab selection and pane focus in any
  live process-owned window without activating its native window.
  API v22 adds separately granted exact bounded presentation-safe text reads
  from any live process-owned pane without activation or focus changes.
  API v23 adds separately granted exact guarded UTF-8 input to any live
  process-owned pane behind destination policy and destructive confirmation.
  API v24 adds separately granted ordinary local-tab creation in one exact
  live process-owned window without native activation or hook redispatch.
  API v25 adds separately granted exact ordinary-pane splitting in any live
  process-owned window without native activation or focus fallback.
  API v26 adds separately granted exact ordinary-pane movement between live
  process-owned tabs and windows, atomically preserving the live PTY and
  returning a fresh destination-local identity.
  API v27 adds separately granted exact ordinary-tab movement between live
  process-owned windows and exact same-window reorder while refusing implicit
  source-window closure.
  API v28 adds separately granted closure of one exact ordinary pane in any
  live process-owned window while preserving its containing tab/window and
  refusing final-pane or unsupported targets.
  API v29 adds separately granted scheduled closure of one exact ordinary tab
  in any live process-owned window while preserving the window and refusing
  final-tab or unsupported targets.
  API v30 adds separately granted swap, balance, and columns/rows/grid presets
  for one exact ordinary split tab in any live process-owned window while
  preserving terminal identity, focus, and active-tab selection.
  API v31 adds separately granted result-gated closure of one exact non-final
  live process-owned window without fallback, native activation, or lifecycle
  feedback. There are still no native activation, plugin-requested final
  process-window termination, live-domain cross-window layouts, raw terminal-
  output events, kernel-sandboxed runtimes, raw-byte input, arbitrary layout
  trees, broader destructive actions, or render decorations.
  This remains a
  substantial gap against kittens and Lua and belongs in future separately
  permissioned slices.
- **Quick-access/drop-down terminal.** Termeric now exposes a one-command,
  single-owner, monitor-relative hide/show terminal with authenticated
  discovery, profile-aware first launch, and preserved hidden PTYs. This closes
  the X11 workflow gap with kitty, Ghostty, and Konsole. Exact Wayland placement
  and reactivation remain compositor-policy dependent and need native evidence.

### Useful breadth

- **Serial transport evidence.** Typed Linux serial profiles, exclusive raw
  descriptor handling, binary relay, palette/CLI launch, and workspace identity
  are now implemented. Physical USB/UART acceptance remains before this can be
  claimed as equal to WezTerm's established serial workflow.
- **Desktop drag/drop and remote file workflows.** Termeric now accepts native
  local file/directory drops as bounded, non-executing POSIX-quoted path
  insertion in ordinary shell/profile tabs. Authenticated live domains add an
  exact-pane, explicitly confirmed, 64 MiB file or bounded deterministic
  directory streaming upload with source
  revalidation, atomic no-overwrite publication, partial cleanup, and
  non-executing remote-path insertion. Transfers expose bounded in-place GUI
  progress and exact pre-commit cancellation with acknowledged daemon cleanup.
  Same-call transient failures resume automatically from a bounded daemon
  offset and recent completed responses replay idempotently. CLI users can opt
  into 24-hour retained partials, discover them from another authenticated
  process, verify a SHA-256 source prefix, and explicitly resume or cancel them.
  Plugin, serial, broadcast, and modal routes fail closed. Native
  compositor/real-host evidence, daemon-restart persistence, rich metadata
  preservation, remote clipboard,
  notification forwarding, and URL opening remain gaps. kitty still ships a
  substantially broader remote-file workflow.
- **Broader shell integration.** Modern Nushell's native OSC 7/133 path is now
  wired for unambiguous REPL launches, but its real-PTY acceptance is still
  pending. Elvish now has a configuration-preserving module and deterministic
  launch routing, but requires one explicit rc import and still lacks a
  real-PTY acceptance result. Automatic Elvish setup without altering arbitrary
  user configuration remains unsolved.
- **Unicode character picker and deeper font controls.** Termeric now provides
  a validated 16-family ordered fallback chain, complete-grapheme coverage
  selection, and settings controls for that chain, line height, ligatures, and
  bold-as-bright. Per-range family selection, variable-font axes/arbitrary
  OpenType features, and an in-app Unicode selector still trail kitty/WezTerm's
  power-user font and character tooling.
- **More window policies.** Ordinary windows now expose live and persisted
  normal/always-on-top/always-on-bottom requests, and quick access has a
  dedicated topmost placement policy. A transient pane-local read-only lock now
  covers all user-input routes while preserving terminal replies. Top and
  vertical-left tab placement and bounded static background images are
  implemented. Richer quick-terminal placement and platform-native
  restoration remain absent or narrower than one or more competitors. Native
  stacking acceptance is still required because the compositor/window manager
  has final authority.

### Emerging/niche protocols

- kitty's text-sizing, drag/drop, multiple-cursor, file-transfer, desktop
  notification, and mouse-pointer-shape extensions are not implemented. The
  [kitty protocol index](https://sw.kovidgoyal.net/kitty/protocol-extensions/)
  is the moving reference. These should be demand-driven after the beta gates;
  implementing every new private protocol is not a prerequisite for terminal
  sufficiency.
- Bidirectional/RTL layout is unsupported. This is an accessibility and language
  coverage gap, but even Ghostty documents left-to-right-only layout today, so
  it should be treated as a deliberate long-term capability rather than a
  release-blocking parity checkbox.

## Recommended execution order

1. Complete and record the Linux beta evidence already tracked by #17/#23/#26/
   #31/#32/#33/#35; do not add more default-on surface area before this gate.
2. Preserve #29's implemented request-correlated live-domain mutation and
   resumable-transfer authority while gathering native remote-host evidence.
3. Continue #34/#36/#37/#39/#40/#41/#42/#43/#44/#47/#48/#49/#50/#51/#52/#53/#54/#55/#56/#57/#58 from the bounded API v31
   current-window query/navigation/topology/handler-command/configuration/
   presented-text, process-wide inventory, and non-activating cross-window
   navigation, terminal-text, guarded terminal-input, ordinary tab/pane creation,
   exact ordinary-pane/tab movement, exact ordinary pane/tab closure,
   exact ordinary pane layouts, and exact non-final process-window closure
   toward only
   demonstrably useful, separately permissioned actions.
   Keep external processes isolated;
   evaluate WASM only after permissions and resource accounting are proven.
4. Record native compositor evidence for the implemented bounded background
   image, quick-access, and top/vertical-left-tab policies before expanding
   window appearance or placement further.
5. Record physical serial, native local-drop, and real-host upload evidence;
   then decide whether daemon-restart upload persistence and any richer metadata
   policy justify their additional filesystem and trust-boundary complexity.
6. Expand shell/font/window polish beyond the implemented top/vertical-left tab
   placement, then evaluate emerging private protocols by
   demonstrated client demand.
7. Promote macOS or Windows only with the native acceptance policy in
   [platform-support.md](platform-support.md); a compiling port is not a product
   claim.

## When it becomes worth recommending broadly

For a Linux beta, the sufficiency threshold is green release evidence, a tested
package lifecycle, no Broken default path, truthful limitations, and repeatable
long-duration stability. For a credible daily-driver claim, add several weeks
of dogfooding artifacts, real GPU/compositor coverage, reliable running-GUI
automation, and a supportable update/distribution path. For a claim that it is
better than existing options, publish like-for-like external-terminal latency,
throughput, memory, and idle-resource tests and demonstrate that the guarded
workspace/Harvest/plugin workflows save real work. Feature count alone cannot
establish that claim.
