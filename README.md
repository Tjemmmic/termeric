# Termeric

A GPU-accelerated terminal emulator written in Rust, themed around roots and rhizomes.

## Highlights

- **GPU rendering** via `wgpu`, font shaping via `cosmic-text`, ANSI parsing via `vte`
- **26 themes and bounded backgrounds** — 12 "roots", 14 community ports, and optional static local PNG/JPEG terminal backgrounds with live fit/opacity controls
- **Tabs, panes, profiles, and workspaces** — top or vertical-left tabs, reusable exact PTY launches, complete versioned layout snapshots, bounded text history, and transcript export
- **Safe file drops** — local quoted-path insertion plus confirmed, bounded file/directory, progress-visible, cancellable, authenticated cross-process-resumable, atomic no-overwrite live-domain upload; neither presses Enter
- **Search** with regex and filter modes across scrollback
- **Keyboard copy mode and quick select** for mouse-free history navigation, selection, links, paths, and configured matches
- **Modern input and accessibility** — pane-positioned IME preedit, native color emoji, F6 keyboard chrome, and AccessKit screen-reader semantics
- **Visual accessibility preferences** — live reduced-motion and high-contrast application chrome without rewriting terminal colors
- **Modern terminal negotiation** — pane-local Kitty plus xterm keyboard/cursor/function/keypad/modifier/special/other-key resources with CSI-27/CSI-u formatting, DECRQSS/DECRQM status replies, UTF-8-safe C1 framing, and application-controlled cursor shape/blink
- **Command palette** (Ctrl+P) with fuzzy search, plus a customizable keyboard shortcuts panel
- **Developer integrations** — experimental built-in Claude Code, Codex, Git, diff review, and context workflows; authenticated provider checks require explicit operator consent
- **Hyperlinks and shell semantics** — OSC 8 links, URL detection, OSC 7 working directories, accepted Bash/Zsh/Fish OSC 133 semantics, acceptance-pending native Nushell and opt-in Elvish semantics, and bidirectional prompt navigation through retained scrollback on Linux
- **Harvest (experimental)** — opt-in semantic command history, per-terminal growth indicators, scoped output search, rerun/copy, and consent-gated AI explanation

Bounded Kitty, static Sixel, and static iTerm2 PNG profiles share one isolated
image cache and GPU renderer with truthful negotiation. Kitty Unicode
placeholders follow grid text through reflow and scrollback, bounded relative
placements follow ordinary or Unicode parents, and bounded Kitty frame
animations support loading, editing, composition, control, and deletion.
Live-domain reconnects stream already-decoded images, placements, aliases, and
animation state through a bounded one-shot replica snapshot; ordinary-file
media remains denied on both sides of the relay.
See the [Kitty](docs/kitty-graphics.md) and
[Sixel/iTerm2](docs/inline-images.md) capability contracts.
Left-to-right Unicode grapheme geometry, SGR text effects, and pane-local
legacy/xterm, xterm keyboard/cursor/function/keypad/modifier/special/other-key resources, and Kitty progressive keyboard input follow the documented [width policy](docs/unicode-support.md),
[rendering policy](docs/sgr-rendering.md), and
[keyboard policy](docs/keyboard-input.md). Mouse, focus, and bracketed paste
follow the [terminal-input policy](docs/terminal-input-protocols.md), and font
ligatures follow the [row-run shaping policy](docs/ligature-rendering.md).
Platform IME, color-font, keyboard-chrome, and screen-reader behavior follows
the [modern input and accessibility policy](docs/accessibility-and-input.md). See the
[OSC integration policy](docs/osc-integration.md) for terminal-controlled
titles, working directories, clipboard access, and dynamic colors. See the
[feature-support matrix](docs/feature-support.md) for exact boundaries rather
than inferring support from a parser, setting, or UI entry.

## Status

Active Linux developer-preview development — expect churn and correctness
gaps. Native X11 workflows and a private Wayland launch/render/close path run
in CI, but Termeric is not yet a general beta or daily-driver terminal. See the
[GUI acceptance harness](docs/gui-acceptance.md). The authoritative
[feature-support matrix](docs/feature-support.md) defines current behavior and
release gates. Historical release evidence is recorded separately in
[`docs/release-acceptance.md`](docs/release-acceptance.md). macOS and Windows
have build-and-unit CI only and remain unverified as runtime targets; see the
[platform support policy](docs/platform-support.md).

## Building

Requires Rust 2024 edition (1.85+).

```bash
cargo build --release
./target/release/termeric
```

For development:

```bash
cargo build              # optimized + debuginfo
cargo test --workspace --locked
```

For packaged installation, upgrades, uninstall steps, configuration paths,
and Linux troubleshooting, see [`docs/install.md`](docs/install.md).

## Configuration

Termeric reads `~/.config/termeric/config.toml`. A starter config lives in [`assets/config.example.toml`](assets/config.example.toml):

```toml
[font]
family = "monospace"
size   = 14.0
fallback = ["Noto Sans Mono", "Noto Color Emoji"] # ordered, up to 16
ligatures = true
bold_is_bright = false

[theme]
name = "turmeric"

[renderer]
backend = "auto" # auto, gpu, or software fallback

[accessibility]
reduce_motion = "system" # system, on, or off
high_contrast = "system" # application chrome only

[window]
opacity = 1.0
padding = 8
level = "normal" # normal, always_on_top, or always_on_bottom
tab_bar_position = "top" # top, or a vertical left rail

[window.background_image]
# path = "/absolute/path/to/background.png"
opacity = 0.20
fit = "cover" # cover, contain, stretch, or center

[scrollback]
lines = 10000
max_memory_mib = 128

[copy_mode]
bindings = "vi" # or "emacs"
```

Settings are also editable live from within the terminal — press the gear icon in the tab bar, or open the settings modal via the command palette.
Line and memory limits are both enforced per terminal and pane; see the
[scrollback memory model](docs/scrollback-memory.md).
Copy mode entry and quick select are configurable in Keyboard Shortcuts; modal
motions and semantic-pattern configuration are documented in the
[keyboard copy-mode contract](docs/copy-mode.md).
Adapter selection, software fallback, diagnostics, and loss recovery are
documented in the [renderer recovery contract](docs/renderer-recovery.md).
Local declarative extensions can be installed without rebuilding. API v2 can
pass a separately granted, bounded structured launch-context snapshot; API v3
adds bounded out-of-process app/tab lifecycle handlers with permissioned
message/notice responses; API v4 adds explicitly granted, bounded semantic
command-completion events without raw PTY streaming or terminal mutation; API
v5 adds opt-in correlated persistent handlers for bounded state retention and
launch reuse; API v6 adds an explicitly granted, bounded live inventory of the
current window without terminal-content disclosure; API v7 adds separately
granted stable-ID tab selection and pane focus with correlated results; API v8
adds independently granted local tab ordering and divider-ratio adjustment;
API v9 lets handlers contribute bounded, namespaced command-palette commands
that are dispatched only to their owning plugin; API v10 adds bounded typed
non-secret settings with explicit set/unset/reset and safe update behavior; API
v11 adds a separately granted, stable-ID read of bounded presented text from
one exact current-window pane, without raw PTY streaming or terminal input.
Settings includes an installed-plugin manager with source/permission inspection,
enable/disable, confirmed update, confirmed settings reset, and confirmed
managed-copy removal; installation and typed value entry remain explicit CLI
operations.
See the [third-party plugin API and security model](docs/plugins.md).

Bundled themes live under [`termeric/themes/`](termeric/themes/). Put personal
themes in `~/.config/termeric/themes/` (or point `[theme].path` at an explicit
file) and they appear in both theme pickers, survive restart, and reload when
the active file changes. A malformed edit keeps the last valid colors active
and reports the file error in-app.

The built-in Codex integration is tested against `codex-cli 0.137.0`. Its
friendly approval modes map to current Codex policies as follows: `suggest`
uses `untrusted`, `auto-edit` uses `on-request`, and `full-auto` uses `never`.
See the starter config for the corresponding sandbox options.

### Windows and command-line launch

Termeric can own multiple independent native windows in one process. Use **New
Window** in the command palette, or group startup windows/tabs with
`--new-window` and `--tab`. CLI controls cover cwd, direct commands and exact
argv after `--`, named plugin profiles, complete workspaces, initial pixel
geometry, X11 class/Wayland app ID, titles, and shell-pane splits. Run
`termeric --help` or see [`docs/command-line.md`](docs/command-line.md).
Bind `termeric --quick-access` to a desktop shortcut for a single-owner,
monitor-relative drop-down window whose hidden PTYs remain alive; see the
[quick-access contract](docs/quick-access.md) for X11 and Wayland boundaries.
Already-running Unix GUI processes expose authenticated inventory through
`termeric control list [--json]` plus replay-safe exact-source window creation,
exact-window local tab creation, and exact-pane splitting through `termeric
control new-window PROCESS SOURCE_WINDOW`, `termeric control new-tab PROCESS
WINDOW`, and `termeric control split-pane PROCESS WINDOW TAB PANE DIRECTION`.
Exact window-focus requests, tab selection, and pane focus are available through
`focus-window`, `select-tab`, and `focus-pane`; Wayland window activation remains
compositor-controlled and fails explicitly.
Typed `close-window`, `close-tab`, and `close-pane` actions preserve exact target
identity and replay safety; surviving pane IDs now remain stable through
single-pane collapse and later re-splitting.
Typed `move-tab` and `move-pane` actions validate complete source/destination
paths before transferring live owners across ordinary tabs or process-owned
windows, and return the fresh destination-local ID when its namespace changes.
Inventory now includes stable tab-local divider IDs; `resize-divider` applies an
exact 100-900 thousandths ratio synchronously, while `resize-window` honestly
acknowledges a bounded native physical-pixel size request that the window
manager may apply asynchronously.
`get-text` reads one exact pane's presented viewport plus at most 256 requested
primary-history rows, retains the newest content within a 64 KiB UTF-8 ceiling,
and never crosses from an alternate screen into primary history or reveals
hidden synchronized output. Opt-in `send-text` targets one stable pane with at
most 64 KiB of typed UTF-8, explicit paste/Enter semantics, replay protection,
and visible destructive-command confirmation for plugin tabs; arbitrary bytes
remain unavailable. Discovery and text queries do not expose argv,
environment, clipboard, or provider data. See the
[running-GUI control contract](docs/gui-control.md).
Press F11 or run **Toggle Fullscreen** from the command palette to change only
the current native window; the shortcut is configurable and the view state is
not restored from a workspace.
Reusable `[profiles.NAME]` entries bundle either exact executable/argv/cwd/env
or a typed Linux serial device/line configuration for CLI or command-palette launches; see the
[terminal-profile contract](docs/terminal-profiles.md).
Native local file and directory drops insert one validated, POSIX-quoted path
into an ordinary shell/profile tab without reading the object or pressing
Enter. A real file/directory drop on an authenticated local/OpenSSH live-domain pane
requires explicit confirmation, streams at most 64 MiB to that exact pane cwd,
uses a bounded metadata-normalizing archive for directories, never overwrites,
and can retain a SHA-256-verified partial for later authenticated CLI resume,
then inserts the quoted remote path without Enter. Plugin,
serial, broadcast, and modal routes fail closed. See the
[drag/drop and transfer contract](docs/file-drag-and-drop.md).

### Workspaces, sessions, and transcripts

The command palette can save and manage complete workspaces. A workspace
preserves ordered shell, terminal-profile, and plugin tabs, nested pane trees and ratios, focused
tab/pane, titles, working directories, last-command metadata, named plugin
profiles, configuration references, and bounded optional scrollback. Restore
relaunches fresh shells and plugin CLIs; it never claims to resurrect processes.
Corrupt tabs, unavailable directories, removed profiles, and missing CLIs are
reported and healthy tabs recover independently. See
[`docs/workspaces.md`](docs/workspaces.md).

Persistent local domains are also available as an experimental live-process
workflow. `termeric domain create NAME` starts a user-private daemon-owned
shell, `termeric domain attach DOMAIN` opens or reattaches a native window, and
the scriptable control API can list, spawn, split, focus, close panes/tabs, send
input, and read bounded text after every GUI closes. Attached GUI New Tab,
Split, Close, nested-divider drag, pane swap/balance/layout and cross-tab move,
Rename, Pin/Unpin, and tab-reorder actions use the daemon too, and monotonic
topology revisions converge every attached window without reattachment while
preserving existing relay PTYs.
Closure is daemon-owned and cannot implicitly destroy the final tab or domain.
Experimental CLI controls and GUI attachment can target an already trusted
OpenSSH destination without exposing the daemon token. GUI topology, controls,
focus, reconnects, and relays retain that authority. Bounded saved profiles can
name existing OpenSSH destinations for controls and GUI attachment. **Attach
Live Domain...** in the command palette asynchronously browses local/saved-host
domains and opens the selected domain in another native window. Native
real-host acceptance and a native TLS transport are not yet available. See
the [live-domain contract](docs/live-domains.md) and
[remote threat model](docs/remote-domains.md).

Legacy single-terminal sessions include metadata and up to 5 MB of terminal scrollback. Termeric
shows a warning when that scrollback limit is reached, and reports save,
restore, deletion, corruption, and partial-recovery failures in the UI.
Restoring a session launches a new shell; it does not restore a running process
or a complete pane/workspace layout. In a split tab, a legacy session captures only the
focused pane's cwd, last command, and bounded text history; the UI labels that
reduced scope when saving and browsing snapshots.

AI transcript recording writes JSONL up to 50 MB and reports the first cap or
write failure instead of silently discarding data. Export produces complete
Markdown and preserves the source JSONL beside it. Automatic redaction covers
common credential prefixes, but it is best-effort and is not a complete secret
scanner; review transcripts before sharing them.

### Harvest and shell integration

Harvest is an experimental beta and remains disabled by default. When enabled,
its commands appear in the command palette and its history is populated from
OSC 133 semantic prompt marks. The bounded Root Cellar displays real command
text and exit health, opens on the newest entry, and supports keyboard, mouse,
wheel, and screen-reader navigation. Stable command-region identities and
signed coordinates follow scrollback and resize reflow. Each history entry
retains a bounded output snapshot (64 KiB per entry and 8 MiB across the
Cellar), so copy and rerun remain useful after terminal scrollback eviction.
The Cellar reports how many older entries its count or memory limits evicted
and marks clipped snapshots. Search and scroll require retained live text;
their Sprout rows are visibly disabled and described as not retained when it
is gone (or as alternate-screen-blocked while a full-screen application owns
the source terminal), while truncated snapshot actions disclose that
limitation before execution. Previous/Next Prompt walk exact retained
marks in both directions from the viewed position, follow reflow and new output,
and reset their navigation origin after direct scrolling. Activating an exact
history entry opens a window-clamped action menu for copy, rerun, scoped search,
scroll, and optional AI explanation; menu requests are identity-scoped so
replaced actions cannot target stale commands. Normal interactive Bash, Zsh,
Fish, and Nushell tabs get those marks plus OSC 7 working-directory reports
automatically. Elvish gets the same semantics after the user adds
`use termeric-integration` once to `~/.config/elvish/rc.elv`. Termeric writes
small generated integration files under
`~/.config/termeric/shell-integration/` for Bash and Zsh, uses Fish's
init-command hook, and enables Nushell's native OSC 7/133 producers after its
normal configuration loads. It exposes the Elvish module through that shell's
standard module search path and restores the original environment when the
module loads. The generated Bash/Zsh files load the user's normal shell rc
first. Nushell and Elvish real-PTY acceptance remain pending on hosts with the
respective shells installed.

Prompt and command metadata is primary-screen state. Full viewport erase and
RIS retire only marks whose backing live rows were erased, clearing scrollback
retires historical row authority, and alternate-screen applications cannot add
shell marks. Stable completion identity remains bounded and observable once so
Harvest can fall back safely without treating replacement cells as old output.
The parser accepts complete `A → B → C → D` command lifecycles, ignores stray
or duplicate boundaries, and prevents prompt-only cycles from becoming command
history. Missing finishes recover at the next prompt after output begins, and
shell exit statuses are bounded to 0–255.

Custom shells and shell configurations that execute a script or `-c` command
are left untouched. Harvest then explains that prompt marks are unavailable
instead of showing an empty menu. Existing tabs keep their launch environment
when shell configuration changes; open a new tab to pick up the new setup.

```toml
[harvest]
enabled = true
show_indicators = true
# Retained command/output strings also share an 8 MiB hard cap.
max_history = 100
```

## Project layout

```
termeric/           Binary crate — windowing, rendering, UI, plugins
  src/app/            App struct, winit event loop, action dispatch
  src/control/        Private running-GUI discovery and inventory protocol
  src/renderer/       wgpu rendering (tab bar, panels, overlays, palette, …)
  src/ui/             UI components (animation, palette, settings, shortcuts, …)
  src/plugin/         Plugin trait + Claude Code / Codex integrations
  src/panes/          Pane layout tree and split management
  src/session/        Legacy session snapshots + transcript export
  src/workspace/      Complete workspace model, archive, and recovery
  themes/             Bundled theme TOML files (roots + community)

termeric-core/      Library crate — terminal emulation
  src/term/           Terminal state machine, cursor, modes, selection
  src/grid/           Cell grid, rows, scrollback storage
  src/ansi/           VT/ANSI escape sequence processor
  src/pty/            PTY creation and I/O
  src/graphics/       Kitty, Sixel, and iTerm2 image protocols
  src/shell/          Shell integration, prompt detection
```

## Documentation

- [`CLAUDE.md`](CLAUDE.md) — architecture overview, critical patterns, module map
- [`STYLE_GUIDE.md`](STYLE_GUIDE.md) — UI / theming style guide (read before touching user-facing code)
- [`AGENTS.md`](AGENTS.md) — rules and conventions for AI agents working in this repo
- [`CHANGELOG.md`](CHANGELOG.md) — release-facing changes and known limitations
- [`docs/feature-support.md`](docs/feature-support.md) — authoritative capability status and release gates
- [`docs/terminfo.md`](docs/terminfo.md) — terminal identity, installation, compatibility fallback, and remote use
- [`docs/integration-acceptance.md`](docs/integration-acceptance.md) — AI, shell, Harvest, and native desktop acceptance boundaries
- [`docs/operator-acceptance.md`](docs/operator-acceptance.md) — native tool installation, ordered release commands, and manual provider checklist
- [`docs/install.md`](docs/install.md) — install, upgrade, uninstall, and troubleshooting
- [`docs/release-acceptance.md`](docs/release-acceptance.md) — candidate-specific Linux evidence and open checks
- [`docs/terminal-conformance.md`](docs/terminal-conformance.md) — VT support manifest, reference traces, and fuzzing
- [`docs/device-attributes-and-charsets.md`](docs/device-attributes-and-charsets.md) — conservative DA identity, G0-G3 invocation, DEC graphics mapping, and presentation-state reports
- [`docs/kitty-graphics.md`](docs/kitty-graphics.md) — bounded static and frame-animation profile, Unix-local media, limits, and unsupported extensions
- [`docs/inline-images.md`](docs/inline-images.md) — bounded Sixel and iTerm2 PNG profiles, placement, and resource limits
- [`docs/performance.md`](docs/performance.md) — release benchmarks, baselines, budgets, and profiling
- [`docs/glyph-atlas.md`](docs/glyph-atlas.md) — bounded paged glyph storage, reclamation, diagnostics, and stress coverage
- [`docs/window-policies.md`](docs/window-policies.md) — ordinary-window stacking requests and quick-access override
- [`docs/background-images.md`](docs/background-images.md) — local static-image formats, fit, layering, reload, and strict resource limits
- [`docs/background-io.md`](docs/background-io.md) — Git snapshots and bounded transcript persistence
- [`docs/pane-event-routing.md`](docs/pane-event-routing.md) — pane PTY ownership, replies, and queue bounds
- [`docs/focused-pane-routing.md`](docs/focused-pane-routing.md) — focused pane input, search, URL, session, and render routing
- [`docs/pane-lifecycle.md`](docs/pane-lifecycle.md) — pane process ownership, teardown, cwd inheritance, and divider geometry
- [`docs/pty-pixel-geometry.md`](docs/pty-pixel-geometry.md) — physical grid rounding, PTY winsize propagation, and pane behavior
- [`docs/session-snapshots.md`](docs/session-snapshots.md) — focused-pane save policy, restore semantics, limits, and compatibility
- [`docs/workspaces.md`](docs/workspaces.md) — complete workspace schema, relaunch semantics, limits, and recovery
- [`docs/terminal-profiles.md`](docs/terminal-profiles.md) — reusable executable/argv/cwd/environment launches and workspace behavior
- [`docs/file-drag-and-drop.md`](docs/file-drag-and-drop.md) — safe local path insertion and confirmed bounded live-domain transfer
- [`docs/advanced-workspaces.md`](docs/advanced-workspaces.md) — tab pin/reorder/rename, pane zoom/read-only/move/layouts, cwd policy, and safe broadcast
- [`docs/tab-bar.md`](docs/tab-bar.md) — live top/vertical-left tab placement, overflow, geometry, and acceptance contract
- [`docs/command-line.md`](docs/command-line.md) — multi-window/tab grouping, startup options, and precedence
- [`docs/gui-control.md`](docs/gui-control.md) — authenticated inventory and typed control of running GUI processes
- [`docs/live-domains.md`](docs/live-domains.md) — daemon ownership, control API, security, limits, and reconnect contract
- [`docs/unicode-support.md`](docs/unicode-support.md) — grapheme segmentation, width policy, invariants, and exclusions
- [`docs/keyboard-input.md`](docs/keyboard-input.md) — focused legacy/xterm and Kitty progressive key encoding and boundaries
- [`docs/terminal-input-protocols.md`](docs/terminal-input-protocols.md) — mouse, focus, alternate-scroll, and bracketed-paste routing
- [`docs/resource-soak.md`](docs/resource-soak.md) — Linux process-growth soak scenario, ceilings, and artifacts
- [`docs/platform-support.md`](docs/platform-support.md) — platform evidence tiers and promotion rules

## License

MIT
