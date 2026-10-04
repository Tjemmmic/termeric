# Termeric

Termeric is a GPU-accelerated terminal emulator for Linux, written in Rust and themed around roots and rhizomes. It uses wgpu for rendering, winit for windowing, cosmic-text for font shaping, and vte for escape-sequence parsing.

This file guides coding agents and contributors working in this repository. Read [`docs/dev/style-guide.md`](docs/dev/style-guide.md) before writing any UI, theme, or user-facing code.

## Build & Test

```bash
cargo build --workspace --locked
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo build --release --locked
python3 scripts/check_markdown_links.py
```

In Markdown, write each paragraph and list item on one line; don't hard-wrap prose.

[`docs/dev/testing.md`](docs/dev/testing.md) covers the slower suites (native GUI, soaks, benchmarks, fuzzing, packages) and CI. The user guide is an mdBook in `docs/` (`mdbook build docs`); update the affected page when user-visible behavior changes, and [`docs/src/status.md`](docs/src/status.md) when a feature's support level changes. Parser recognition or unit coverage alone does not make a feature supported.

## Workspace Structure

```
termeric/          Binary crate — windowing, rendering, UI, agents, plugins
termeric-core/     Library crate — terminal emulation, grid, ANSI parser, PTY
```

## Architecture

### Event Flow

```
winit event loop
  → app/application.rs   (Application — the process-wide ApplicationHandler; owns every window)
  → app/event_loop.rs    (per-window App event handlers: keyboard, mouse, resize, redraw)
  → app/mod.rs           (App — window state, input routing, action handling)
  → termeric-core        (ANSI parsing, grid updates, PTY I/O)
  → renderer/mod.rs      (builds render data, submits to wgpu)
```

PTY output and terminal-generated replies follow one stricter ownership path: `TabManager::poll_events()` polls every tab/pane, `PtyWrite` is consumed beside the originating `PtyHandle`, and only pane-tagged app events leave that owner. Never route a terminal-generated reply through focused input.

Outside pane mode a tab owns its one terminal as a `Pane`; entering pane mode detaches that pane (with the tab's pane allocation history, as a `PaneOwner`) and adopts it as the first pane, and no replacement PTY may be spawned. User-facing reads and writes must use `Tab::focused_pane()`, `Tab::focused_terminal()`, `focused_terminal_mut()`, or `Tab::write()`; focus changes must refresh search/URL/Git/cursor surfaces. Lifecycle transitions use the take/restore owner APIs.

Session persistence takes one stable focused-terminal snapshot. Pane-local last-command state must move with terminal ownership, and close-time tab snapshot saves must continue through `Tab::save_as_session()`. Complete startup/exit persistence uses the separate versioned workspace archive and reconstructs tabs/panes by relaunching fresh processes. See [`docs/src/workspaces-and-sessions.md`](docs/src/workspaces-and-sessions.md).

Agent hooks report through `termeric agent-event` over the control socket. Each report names its terminal by the `TERMERIC_TERMINAL_ID` its processes inherited from `PtyHandle`, never by focus. Status lives on the terminal's `TerminalSession` and moves with it. See [`docs/src/agents.md`](docs/src/agents.md).

The experimental agent view is a docked, non-modal panel right of the terminals that drives Claude Code (stream-json), Codex (app-server), and OpenCode (Agent Client Protocol) headlessly, each through an `agent_view::protocol::Protocol` that translates to agent-neutral events (`agent_view/`). It owns its agent processes, not PTYs, and never routes through focused terminal input. The terminal area ends at `Renderer::content_right()`. A dock width change resizes only the active tab (`sync_agent_dock` → `sync_active_terminal_geometry`); every other geometry change resizes all tabs through `sync_terminal_geometry`. Each tab owns one agent view: the panel state is bound to the active tab in `refresh_focused_terminal_surfaces`, moves with a tab between windows, and ends with it (`app/agent_view_tabs.rs`). Its scene (`ui/agent_view/scene.rs`) is the single source for drawing, hit targets, focus order, and accessibility. See [`docs/src/agent-view.md`](docs/src/agent-view.md).

Experimental live domains are a third, intentionally separate lifecycle. The `domain/` daemon owns real PTYs and parser state; GUI tabs own only relay PTYs. Never save a relay as a workspace shell or mutate its layout locally. Protocol I/O is versioned, authenticated, length-bounded, and kept off PTY reader locks. See [`docs/src/live-domains.md`](docs/src/live-domains.md).

### Key Modules (termeric/)

| Module | Purpose |
|--------|---------|
| `app/` | `Application` (multi-window handler), per-window `App`, event handlers, input routing, and action handlers split by feature |
| `renderer/` | wgpu rendering (see below) |
| `ui/` | UI component state: animation, command palette, settings, panels, pickers, popup menus, toasts, text input and layout |
| `ui/animation.rs` | `OverlayAnimation` and `PanelAnimation` state machines, `ease_out_cubic` |
| `ui/text_input.rs` | Shared single-line text input buffer with cursor management |
| `ui/text_layout.rs`, `ui/text_area.rs` | Measured, wrapped UI text (`TextMeasure`) and the multi-line editor buffer |
| `ui/constants.rs` | Shared UI constants (corner radii, border width, filter-field insets) |
| `ui/geometry.rs`, `ui/color.rs`, `ui/icon.rs` | `Bounds`, the one rectangle type (half-open `contains`) for layout, hit tests, drawing, and accessibility; color alpha/mix/brightness helpers; the icon set drawn from `renderer/icons.rs` |
| `config/` | TOML config loading, theme definitions |
| `input/` | Shortcut registry, focused mode-aware keyboard encoder, mouse state |
| `tabs/` | Tab management |
| `panes/` | Split-pane tree and layout |
| `session/` | Session save/restore, the session panel, transcripts, and export |
| `workspace/` | Complete workspace schema, atomic storage, migration, and recovery |
| `domain/` | Same-user local daemon, control protocol/client, PTY ownership, and GUI relay |
| `control/` | Local control socket for running GUI processes: inventory, text queries, launches, opt-in input, and `agent-event` reports |
| `agent_view/` | Agent view core: agent-neutral events, the agents it drives and their protocols (Claude Code stream-json, Codex app-server, ACP for OpenCode), agent process and pipes, sessions, transcript, Markdown, tool presentation, earlier-conversation history |
| `ui/agent_view/` | Agent view panel state, colors, scene (primitives, hits, accessibility nodes), and layout |
| `agents/` | Coding-agent catalog (built-in + `[agents.<id>]`), foreground-process detection, config migration, hook status (`status`, `hook`, `setup`, `transcript`), Git change review (`changes`), context gathering for the context picker (`context`), supervisor policy (`supervisor`) |
| `plugin/` | Plugin trait, hooks, and manager; `AgentPlugin` launch integration for catalog agents; third-party plugins: `permission.rs` (capabilities and `:all-windows` scopes), `runtime.rs` (manifests, registry, install/update/enable), `structured.rs` (handler processes, protocol, follow-up authority) |
| `terminal/` | Search, copy mode, URL detection, diff detection, shell integration, shared terminal-event draining (`events.rs`), and guardrails for automated (control-protocol and plugin) input |
| `git/` | Branch detection, Git completion, and repository watching |
| `harvest/` | Experimental Harvest mode: command history cellar, growth indicators, sprout actions |
| `notifications/` | Bell and desktop notifications |
| `cli.rs` | Command-line parsing into window-local startup plans |
| `mcp.rs` | `termeric mcp`, an MCP server over stdio backed by the control protocol |
| `serial.rs` | Serial transport for serial terminal profiles |
| `paths.rs` | Configuration and data directories; test builds resolve them under a per-process temporary directory |
| `safe_fs.rs` | Hardened reads (no symlinks, FIFOs, or unbounded reads) and atomic private-staging writes for Termeric's own files |
| `line_reader.rs` | Reading newline-delimited input from other processes with bounded memory; callers decide what an oversized or unterminated line means |
| `safe_text.rs` | Bounding untrusted text shown in chrome or sent over protocols (character classes come from termeric-core) |

### Key Modules (termeric-core/)

| Module | Purpose |
|--------|---------|
| `term/` | Terminal state machine, cursor, modes, selection |
| `grid/` | Cell grid, rows, scrollback storage |
| `ansi/` | VT/ANSI escape sequence processor (vte-based) |
| `pty/` | PTY creation and I/O, terminal identity and terminfo discovery |
| `graphics/` | Inline images: Kitty graphics, Sixel, and iTerm2 (OSC 1337) |
| `shell/` | Shell integration, prompt detection |
| `event.rs` | `TerminalEvent`, the events a terminal raises for the app |
| `file_transfer.rs` | Kitty file-transfer (OSC 5113) wire types |
| `unicode.rs` | Grapheme segmentation and cell-width rules |
| `checksum.rs` | FNV-1a for relay checksums and persisted names and fingerprints |
| `hex.rs` | Random lowercase-hex identifiers and the checks for their one spelling |
| `safe_text.rs` | Character classes for untrusted text in chrome and protocol fields, and character-boundary cuts (also used by the fuzzed domain protocol) |
| `split.rs` | `SplitDirection`, `PaneLayoutPreset`, and preset shapes shared by the pane tree, workspaces, the control protocol, plugins, and the domain daemon |

### Renderer Sub-modules

| File | Contains |
|------|----------|
| `mod.rs` | `Renderer` struct, `new()`, `render()`, `build_render_data()` dispatch, pane grid and image data, `UiPalette`, color helpers |
| `types.rs` | `*Info` structs and click/action enums (re-exported via `pub use types::*`) |
| `draw.rs` | Shared card, shadow, scrollbar, button, and focus-ring drawing for dialogs and menus |
| `rect.rs`, `buffer.rs` | Rounded-rectangle pipeline and capacity-managed GPU upload buffers |
| `text.rs`, `ui_text.rs` | Terminal glyph queue and text pipeline; chrome text in the desktop UI face |
| `style.rs` | Resolved SGR cell style, decoration geometry, visibility, and color/face ordering |
| `glyph_cache.rs` | Styled face matching, fallback diagnostics, shaping, rasterization, and glyph atlas |
| `row_cache.rs` | Persistent row geometry/GPU buffers and blink-phase invalidation |
| `cursor.rs`, `ime.rs`, `quick_select.rs` | Cursor painting, IME preedit, copy-mode and quick-select overlays |
| `image.rs`, `background_image.rs` | Inline-graphics and background-image pipelines |
| `tab_bar.rs` | Top strip and vertical rail, `tab_bar_hit_test()` |
| `drag_picture.rs` | The picture of a dragged tab |
| `search_bar.rs` | Floating find box, `search_bar_hit_test()` |
| `icons.rs` | The one stroke table for `ui::icon::Icon`, used by chrome and the agent view (`draw_icon`, `push_icon`, `draw_arc`, `push_magnifier`) |
| `palette.rs` | Command palette and the shared modal frame (`build_modal_frame`) |
| `settings_render.rs` | Settings dialog and its dropdown overlay |
| `shortcuts_render.rs` | Shortcuts panel |
| `panels.rs` | Commands, Agents, and session panels, their shared frame, and the Commands/session panel hit tests |
| `popup_menu.rs` | Anchored popup menu (used by the Workspaces menu) |
| `overlays.rs` | Info overlay, diff review, context picker, guardrail confirmation |
| `toasts.rs` | Toast cards |
| `git.rs` | Git completion dropdown |
| `harvest.rs` | Harvest indicators, cellar, sprout menu, `sprout_menu_hit_test()` |
| `agent_view.rs` | Draws the agent view scene; dock width and `content_right()` |

`build_*` entry points are `pub(super)`; helpers used only inside their own file stay private. Hit tests called from `app/` (`mouse.rs`, `event_loop.rs`, `tab_drag.rs`) are `pub`.

## Critical Patterns

### PTY and terminal-event ownership

- Poll inactive tabs and unfocused panes; focus controls input, not progress.
- Consume `TerminalEvent::PtyWrite` beside its originating PTY.
- Preserve `TerminalEventOrigin` until app-mediated replies complete, then clean up closed panes.
- Keep PTY and app-facing event queues bounded. If a loss policy changes, document it and test protocol replies under overflow.
- Use the single `poll_events` path; do not add a second pane event pump.

### Animation State Machines

Two animation types in `ui/animation.rs`:

**`OverlayAnimation`** — modal overlays (fade in/out)
- `close()` keeps `active = true` during the close animation. `update()` clears it when done.
- `is_active()` = blocks input. `is_visible()` = should render.
- Every component using OverlayAnimation **must** call `update()` each frame (the calls live in `App::render()`).
- CommandPalette uses `close_immediately()`, which clears `active` right away. With reduced motion on, `close()` does the same.

**`PanelAnimation`** — slide-in panels (supports mid-animation reversal)
- `close()` sets `open = false` **immediately**. The close animation still plays via `raw_progress()`.
- `is_open()` = false immediately after close. `is_visible()` = true during the close animation. `is_closed()` = fully closed. There is no `is_active()`.
- `toggle()` calls `open()`/`close()` based on current state. Seamless reversal is handled by `animation_base`.

When adding a component that blocks input, trace its whole lifecycle — open, input dispatch, close, state reset — before wiring it into a dispatch chain.

### Modal Layers

`ModalLayer::INPUT_PRIORITY` (`ui/modal.rs`) is the one stacking order: guardrail → info overlay → diff review → context picker → Harvest sprout menu → Harvest cellar → domain picker → Unicode picker → command palette → shortcuts panel → Settings → Workspaces menu → session panel → Commands panel → Agents panel. Only `App::top_modal_layer()` owns input: keys, clicks, wheel, hover, IME, paste, and accessibility actions and snapshots all gate on it, and the renderer draws the layers in the reverse order, so the surface in front is the one that answers. A consumer with nothing to do for the top layer swallows the input rather than passing it beneath.

### Mouse Click Dispatch Priority

In `app/event_loop.rs` (MouseInput handler), before anything else:
1. A press exits copy mode; a release that completes terminal mouse capture goes back to its terminal
2. The top modal layer above the panels, which swallows clicks while on top
3. Everything else goes to `handle_mouse_input`

In `app/mouse.rs` (`handle_mouse_input`): 0. The docked agent view claims presses on its area and divider when `pointer_panel_owns_input()` is false; a press elsewhere returns the keyboard to the terminals
1. Native scrollbar/divider presses only when `pointer_panel_owns_input()` is false
2. Global Settings button (preserves direct panel-to-Settings switching)
3. The top panel layer (Settings, Workspaces menu, session, Commands, or Agents panel) intercepts its clicks, including outside-click dismissal, before background controls
4. Tab bar and search bar, then toggling a pane in or out of an armed broadcast target set
5. Pane focus, Ctrl+Click hyperlink, and terminal mouse reporting when requested; Shift bypasses to native selection
6. Terminal selection (default)

The same panel-ownership predicate blocks background divider hover from returning before panel hover handling. Non-primary panel presses are consumed; terminal-protocol releases keep their separate completion path. Native gestures are finalized on window focus loss or modal takeover, before dismissal/animation cleanup can discard that ownership. Terminal-geometry synchronization also finalizes gestures for resize, scale, font/configuration, and topology paths; never clear divider state directly and orphan a live-domain preview. Scrollbar capture is an optional stable terminal identity, not a boolean. Revalidate it at the scroll mutation boundary and on focus-surface refresh so a held gesture cannot scroll a newly selected tab or pane. Divider drags likewise require their original open tab to remain selected and their exact divider to remain visible. Finish invalid drags through the shared control path; never silently discard an unacknowledged live-domain preview.

**Any `is_open()`/`is_active()` that stays true after closing will block ALL clicks below it in the chain.**

### Keyboard Input Priority

In `app/event_loop.rs` (KeyboardInput handler), after AltGraph tracking and query-paste handling, checked in order:

1. The top modal layer
2. Tab rename
3. Agent view, only while it holds the keyboard (global shortcuts still run, except its composer's editing chords)
4. Search, then copy mode/quick select
5. Normal input: broadcast cancel on Escape, F6 chrome focus, Git completion keys, broadcast arming, application shortcuts, plugin/custom keybindings, broadcast writes, then `input/keyboard.rs` encoding

Snapshot DECCKM/DECKPAM from `active_terminal()` immediately before encoding, then write through `Tab::write()` so the mode and PTY always belong to the same focused pane. AltGraph state is explicit and is cleared on focus loss.

Copy mode is bound to the focused tab/pane identity at entry. Mouse focus changes cancel it, resize tracks its cursor/anchor through a grid-bound resize transaction, and its selection is synchronized only through `focused_terminal_mut()`. Quick select scans the visible focused grid and must not create a second text model.

Mouse, focus, alternate-scroll, and paste reports snapshot mode from the same focused terminal that receives the bytes. Protocol writes bypass editable-line alias/completion handling. Shift is the explicit native mouse bypass.

### Rendering vs Input Visibility

| Method | Meaning | Use for |
|--------|---------|---------|
| `is_active()` | `OverlayAnimation`: accepting input (true through the close animation) | Input dispatch guards |
| `is_open()` | `PanelAnimation`: logically open (false as soon as close starts) | Panel click interception |
| `is_visible()` | Should be rendered (includes the close animation) | Render guards |
| `is_closed()` | `PanelAnimation`: fully closed, animation complete | Cleanup |

### Renderer Module Conventions

- Child modules access the parent's private fields via `&mut self` on `impl Renderer` blocks
- Types in `types.rs` are re-exported via `pub use types::*` in mod.rs; a type defined in another sub-module needs its own `pub use` there (e.g. `PopupMenuInfo`)
- A hit test must use the same geometry as the `build_*` function that drew the element: the tab bar shares one geometry source (`TabChrome`), the side panels and the find box each compute one `*Layout` that their builder and hit test both use (`commands_panel_layout`, `session_panel_layout`, `SearchBarLayout::calculate`)

### Animation Timing

All animations use `Instant`-based, frame-rate-independent interpolation. Never use delta-time accumulation.

```rust
// Correct
let elapsed = self.animation_start.elapsed().as_secs_f32();
let t = (elapsed / DURATION).min(1.0);

// Wrong
self.progress += dt * speed;
```

Easing: ease-out cubic `1.0 - (1.0 - t).powi(3)` (`ease_out_cubic`). Default durations: 150ms overlays (`DEFAULT_OVERLAY_DURATION`), 200ms panels (`DEFAULT_PANEL_DURATION`); the Commands and Agents panels and the guardrail overlay use 250ms; the tab-bar hover fade-in takes 100ms.
