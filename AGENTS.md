# Termeric

Termeric is a GPU-accelerated terminal emulator for Linux, written in Rust and themed around roots and rhizomes. It uses wgpu for rendering, winit for windowing, cosmic-text for font shaping, and vte for escape-sequence parsing.

This file guides coding agents and contributors working in this repository. Read the [Style Guide](#style-guide) at the end before writing any UI, theme, or user-facing code.

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

[CONTRIBUTING.md](CONTRIBUTING.md#testing) covers the slower suites (the GPU smoke, the soak, benchmarks, fuzzing, packages) and CI.

There is no separate user guide. What people read lives in four places: the [README](README.md) (installing, getting started, troubleshooting, and the experimental features under Status), [`assets/config.example.toml`](assets/config.example.toml) (every setting with its default and range, every shortcut action ID, and the built-in themes), the command-line help in `termeric/src/cli/` (which also generates the manual page and completions), and [`examples/plugins/README.md`](examples/plugins/README.md) (the plugin API). When user-visible behavior changes, update whichever of these describes it; a new setting, shortcut action, or built-in theme always goes into the example configuration, and a change in a feature's support level goes into the README's Status section. Parser recognition or unit coverage alone does not make a feature supported.

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
  → renderer/frame.rs    (builds render data, submits to wgpu)
```

PTY output and terminal-generated replies stay with the terminal that produced them and never route through focused input; the rules are under "PTY and terminal-event ownership" below.

A background task that returns a result to one window (an `AppEvent` with a `window` field, or a plugin handler result) names it by the window's control ID (`App::window_key()`), never its winit `WindowId`: on Wayland that comes from the surface's address, which a later window can reuse, and per-window request counters start alike. `Application` resolves the key through its control-ID map and drops results for windows that have closed. AccessKit events alone route by native ID, since each window's adapter ends with it.

Every tab owns exactly one `PaneManager` (`Tab::panes`); a tab with one terminal is a one-leaf tree. Splitting and closing panes only change the tree: no terminal is ever handed between the tab and its panes, no replacement PTY is spawned, and pane IDs are never reused within a tab (a fresh tab's terminal is pane 0). Every terminal event names its pane (`TerminalEventOrigin { tab_id, pane_id }`). A tab's only pane is focused, unzoomed, and primary, and shows the tab's title rather than its own. Features that exist only between panes (broadcast, dividers, zoom, pane focus, outlines) check `Tab::split_panes()`. User-facing reads and writes must use `Tab::focused_pane()`, `Tab::focused_terminal()`, `focused_terminal_mut()`, or `Tab::write()`; focus changes must refresh search/URL/Git/cursor surfaces.

Closed tabs are kept as one-tab workspaces (`Tab::save_closed_tab()`, marked `closed_tab`, newest 100 kept). Pane-local last-command state must move with terminal ownership. Complete startup/exit persistence uses the same versioned workspace archive and reconstructs tabs/panes by relaunching fresh processes.

Everything agent-related is behind one switch, `[agent_features] enabled`, off by default. Every agent entry point (recognition, palette commands, shortcuts, panels, the agent view, the supervisor, Harvest's Explain, agent launch integration, and agent control requests) asks `App::agent_features_enabled()` (`app/agent_features.rs`), never the config field. Turning it off closes every agent surface; while an agent view conversation or the supervisor runs, the window keeps agent features on (`agent_features_held`, with a notice) and `release_agent_features` turns them off once the last one ends. Every configuration change (Settings, reload, reset) goes through this one path in `apply_config`.

Agent hooks report through `termeric agent-event` over the control socket. Each report names its terminal by the `TERMERIC_TERMINAL_ID` its processes inherited from `PtyHandle`, never by focus. Status lives on the terminal's `TerminalIntegration` and moves with it.

The experimental agent view is a docked, non-modal panel right of the terminals that drives Claude Code (stream-json), Codex (app-server), and OpenCode (Agent Client Protocol) headlessly, each through an `agent_view::protocol::Protocol` that translates to agent-neutral events (`agent_view/`). It owns its agent processes, not PTYs, and never routes through focused terminal input. The terminal area ends at `Renderer::content_right()`. A dock width change resizes only the active tab (`sync_agent_dock` → `sync_active_terminal_geometry`); every other geometry change resizes all tabs through `sync_terminal_geometry`. Each tab owns one agent view: the panel state is bound to the active tab in `refresh_focused_terminal_surfaces`, moves with a tab between windows, and ends with it (`app/agent_view_tabs.rs`). Its scene (`ui/agent_view/scene.rs`) is the single source for drawing, hit targets, focus order, and accessibility.

Experimental live domains are a third, intentionally separate lifecycle. The `domain/` daemon owns real PTYs and parser state; GUI tabs own only relay PTYs. Never save a relay as a workspace shell or mutate its layout locally. Protocol I/O is versioned, authenticated, length-bounded, and kept off PTY reader locks.

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
| `panes/` | Each tab's pane tree (one leaf until it is split), its layout over the tab's `TerminalArea`, and pane/divider ID history |
| `session/` | Per-terminal agent state, transcripts and their export, and what the workspace panel lists |
| `workspace/` | Complete workspace schema, atomic storage, and recovery |
| `domain/` | Same-user local daemon, control protocol/client, PTY ownership, and GUI relay |
| `control/` | Local control socket for running GUI processes: inventory, text queries, launches, opt-in input, and `agent-event` reports |
| `agent_view/` | Agent view core: agent-neutral events, the agents it drives and their protocols (Claude Code stream-json, Codex app-server, ACP for OpenCode), agent process and pipes, sessions, transcript, Markdown, tool presentation, earlier-conversation history |
| `ui/agent_view/` | Agent view panel state, colors, scene (primitives, hits, accessibility nodes), and layout |
| `agents/` | Coding-agent catalog (built-in + `[agents.<id>]`), foreground-process detection, hook status (`status`, `hook`, `setup`, `transcript`), Git change review (`changes`), context gathering for the context picker (`context`), supervisor policy (`supervisor`) |
| `plugin/` | Plugin trait, hooks, and manager; `AgentPlugin` launch integration for catalog agents; third-party plugins: `permission.rs` (capabilities and `:all-windows` scopes), `runtime.rs` (manifests, registry, install/update/enable), `structured.rs` (handler processes, protocol, follow-up authority) |
| `terminal/` | Search, copy mode, URL detection, diff detection, shell integration, shared terminal-event draining (`events.rs`), and guardrails for automated (control-protocol and plugin) input |
| `git/` | Branch detection, Git completion, and repository watching |
| `harvest/` | Experimental Harvest mode: command history cellar, growth indicators, sprout actions |
| `notifications/` | Bell and desktop notifications |
| `cli.rs` | Command-line parsing into window-local startup plans |
| `mcp.rs` | `termeric mcp`, an MCP server over stdio backed by the control protocol |
| `serial.rs` | Serial transport for serial terminal profiles |
| `test_support/` | Test builds only: shared fixtures (`sleeping_shell`, `poll_until`, …), the release benchmark suite (`benchmarks.rs`), and soaks (`soak.rs`) |
| `paths.rs` | Configuration and data directories; test builds resolve them under a per-process temporary directory |
| `safe_fs.rs` | Hardened reads (no symlinks, FIFOs, or unbounded reads) and atomic private-staging writes for Termeric's own files |
| `line_reader.rs` | Reading newline-delimited input from other processes with bounded memory; callers decide what an oversized or unterminated line means |
| `deadline_io.rs` | Frame reads and writes bounded by one deadline that trickled bytes cannot renew: the time left (`remaining`), a readiness poll (`readable`), `PollReader` for sockets and pipes (optionally idle without limit until input starts, and cancellable), and `SocketWriter` |
| `local_socket.rs` | What the domain daemon and the control socket share: the private runtime directory, socket and token paths, same-user peer checks, version-and-token authentication, the bounded accept loop, and stale-socket removal |
| `safe_text.rs` | Bounding untrusted text shown in chrome or sent over protocols (character classes come from termeric-core) |
| `process_group.rs` | Starting, reaping, and signalling a child that leads its own process group with piped stdio (agent view agents, plugin handlers): its group is signalled only while it is unreaped |
| `stderr_capture.rs` | Keeping the start or the end of a child's stderr on a thread of its own, waiting a bounded time for it to close (agent view agents, plugin handlers) |

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
| `mod.rs` | `Renderer` struct, `new()`, setters, font-size conversion |
| `device.rs` | Adapter and device selection, renderer diagnostics |
| `colors.rs` | `UiPalette` and its high-contrast pass, the terminal palette, surface formats |
| `frame.rs` | `Frame` (every visible pane of the active tab, plus chrome), `render()`, `build_render_data()` layer dispatch, image data, pane dividers |
| `grid.rs` | `build_pane_grid()`: a grid's cached rows, cursor, and scrollbar |
| `headless_tests.rs`, `image_crop_smoke.rs`, `benchmark.rs`, `visual_baseline.json` | GPU smokes run by `gpu_render_smoke`, the visual baseline they compare against, and the headless renderer benchmark (test builds only) |
| `shaders/` | WGSL for the rectangle, text, and image pipelines (the background image draws through the image pipeline) |
| `types.rs` | `*Info` structs, click/action enums, `PaneRenderData`, and `PanePlacement`, which places panes and their scrollbars (`PaneScrollbar`) in the window (re-exported via `pub use types::*`) |
| `draw.rs` | Shared card, shadow, scrollbar, button, and focus-ring drawing for dialogs and menus |
| `rect.rs`, `buffer.rs` | Rounded-rectangle pipeline and capacity-managed GPU upload buffers |
| `text.rs`, `ui_text.rs` | Terminal glyph queue and text pipeline; chrome text in the desktop UI face |
| `style.rs` | Resolved SGR cell style, decoration geometry, visibility, and color/face ordering |
| `glyph_cache.rs` | Glyph and shaped-run lookup, shaping, rasterization, and upload; its `glyph_cache/` children hold the paged atlas allocator (`atlas.rs`), run and UI-layout caches (`caches.rs`), family coverage and the coverage report (`coverage.rs`), and font setup and cell metrics (`fonts.rs`) |
| `row_cache.rs` | Persistent row geometry/GPU buffers and blink-phase invalidation |
| `cursor.rs`, `ime.rs`, `quick_select.rs` | The main and extra cursors (`draw_cursor_at`, `build_extra_cursors`), IME preedit, copy-mode and quick-select overlays |
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
| `harvest.rs` | Harvest indicators, cellar, sprout menu |
| `agent_view.rs` | Draws the agent view scene; dock width and `content_right()` |

`build_*` entry points are `pub(super)`; helpers used only inside their own file stay private. Hit tests called from `app/` (`mouse.rs`, `event_loop.rs`, `tab_drag.rs`) are `pub`.

## Critical Patterns

### PTY and terminal-event ownership

- `TabManager::poll_events()` polls inactive tabs and unfocused panes; focus controls input, not progress.
- Consume `TerminalEvent::PtyWrite` beside its originating `PtyHandle`; only pane-tagged app events leave that owner.
- Preserve `TerminalEventOrigin` until app-mediated replies complete, then clean up closed panes.
- Keep PTY and app-facing event queues bounded. If a loss policy changes, document it and test protocol replies under overflow.
- Use the single `poll_events` path; do not add a second pane event pump.

### Animation State Machines

Two animation types in `ui/animation.rs`:

**`OverlayAnimation`** — modal overlays (fade in/out)
- `close()` keeps `active = true` during the close animation. `update()` clears it when done.
- `is_active()` = blocks input. `is_visible()` = should render.
- Every component using OverlayAnimation **must** call `update()` each frame (the calls live in `App::build_chrome`, `app/frame.rs`).
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
1. A press on a toast dismisses it and nothing else: toasts draw above every modal layer (`Renderer::toast_hit_test`, the same placement `build_toasts` draws)
2. A press exits copy mode; a release that completes terminal mouse capture goes back to its terminal
3. The top modal layer above the panels, which swallows clicks while on top
4. Everything else goes to `handle_mouse_input`

In `app/mouse.rs` (`handle_mouse_input`): 0. The docked agent view claims presses on its area and divider when `pointer_panel_owns_input()` is false; a press elsewhere returns the keyboard to the terminals
1. A primary press on a pane's scrollbar (when no find box or toast is drawn over it) or a divider, only when `pointer_panel_owns_input()` is false
2. Global Settings button (preserves direct panel-to-Settings switching)
3. The top panel layer (Settings, Workspaces menu, session, Commands, or Agents panel) intercepts its clicks, including outside-click dismissal, before background controls
4. Tab bar, then any press on chrome floating over the panes (`press_floating_chrome`: the find box works it, a scrollbar swallows other buttons), then toggling a pane in or out of an armed broadcast target set
5. Pane focus, Ctrl+Click hyperlink, and terminal mouse reporting when requested; Shift bypasses to native selection
6. Terminal selection (default)

The same panel-ownership predicate blocks background divider hover from returning before panel hover handling. Non-primary panel presses are consumed; terminal-protocol releases keep their separate completion path. Native gestures are finalized on window focus loss or modal takeover, before dismissal/animation cleanup can discard that ownership. Terminal-geometry synchronization also finalizes gestures for resize, scale, font/configuration, and topology paths; never clear divider state directly and orphan a live-domain preview. Scrollbar capture is an optional stable terminal identity, not a boolean. Revalidate it at the scroll mutation boundary and on focus-surface refresh so a held gesture cannot scroll a newly selected tab or pane. Divider drags likewise require their original open tab to remain selected and their exact divider to remain visible. Finish invalid drags through the shared control path; never silently discard an unacknowledged live-domain preview.

Chrome floating over the panes (toasts, the find box, and pane scrollbars) owns the pointer wherever it is drawn. `App::floating_chrome_at` finds it frontmost first, through `toast_hit_test`, `search_bar_hit_test`, and `PanePlacement::pane_scrollbar` (the geometry `build_pane_grid` draws scrollbars with), and it is the one check behind presses, hover reports, the wheel, and the pointer's shape. Hover over chrome is never reported to the program beneath, though a drag the program holds keeps reporting; the wheel over a scrollbar scrolls that pane's scrollback without reporting, and over other chrome it scrolls nothing.

**Any `is_open()`/`is_active()` that stays true after closing will block ALL clicks below it in the chain.**

### Keyboard Input Priority

In `app/event_loop.rs` (KeyboardInput handler), after AltGraph tracking and query-paste handling, checked in order:

1. The top modal layer
2. Tab rename
3. Agent view, only while it holds the keyboard (global shortcuts still run, except its composer's editing chords)
4. Search, then copy mode/quick select
5. Normal input: broadcast cancel on Escape, F6 chrome focus, Git completion keys, broadcast arming, application shortcuts, plugin/custom keybindings, broadcast writes, then `input/keyboard.rs` encoding

Snapshot DECCKM/DECKPAM from `active_terminal()` immediately before encoding, then write through `Tab::write()` so the mode and PTY always belong to the same focused pane. AltGraph state is explicit and is cleared on focus loss.

Text that isn't a key press (clipboard and middle-click pastes, IME preedit and commits, and the IME caret) goes where `App::text_target()` (`app/text_target.rs`) says, decided in this same order. A surface with no field to take it is `Blocked`; text never falls through to a field or terminal beneath. Whether a key types text rather than making a chord is `types_text` (`app/text_field_keys.rs`), shared by every field.

Copy mode is bound to the focused tab/pane identity at entry. Mouse focus changes cancel it, resize tracks its cursor/anchor through a grid-bound resize transaction, and its selection is synchronized only through `focused_terminal_mut()`. Quick select scans the visible focused grid and must not create a second text model.

Mouse, focus, alternate-scroll, and paste reports snapshot mode from the terminal that receives the bytes (the focused one, or the hovered pane for the wheel). Protocol writes bypass editable-line alias/completion handling. Shift is the explicit native mouse bypass.

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
- Terminal panes are placed by `PanePlacement::pane_bounds` for drawing, the Git completion anchor, pointer-to-cell mapping, the IME caret, and accessibility. It is the one place a tab's only pane differs from split panes: it fills the terminal area inside the padding on every side (its grid starts one padding below the tab bar) and shows its grid's own cells, while split panes sit at their layout rectangles from the area's top edge. `PanePlacement::pane_cells` is how many cells each shows, and `PanePlacement::pane_scrollbar` where its scrollbar is drawn and hit

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

Never bounce or overshoot, and keep every transition at 250ms or less. The cursor glides to its new position over 150ms, including across tab switches. Honor reduced motion: `OverlayAnimation` and `PanelAnimation` snap when it is on, and any new animation must do the same.

## Style Guide

Termeric is named after turmeric, the root, and its house themes are named after roots too. Read this before adding a theme or changing anything a person sees: chrome, dialogs, panels, menus, or text.

### Themes

#### Naming

- **Root themes** are Termeric's own palettes. They must be named after a real root, rhizome, or tuber, and set `author = "Termeric"`. The current twelve: turmeric (the default), ginger, daikon, malanga, beet, turnip, yuca, taro, wasabi, galangal, horseradish, parsnip.
- **Guest themes** are community palettes (Solarized, Catppuccin, Dracula, Nord, and others). They keep their original names and authors. The theme picker lists them under "Guest", apart from the roots.

The file name and lookup key are the lowercase name (`turmeric.toml`); the display name in `meta.name` is capitalized (`"Turmeric"`).

#### Deriving a root palette

A root theme takes its colors from the actual plant: its skin, flesh, leaves, and flowers. Programs rely on what ANSI colors mean, though — red for errors, green for success, yellow for warnings, blue for directories, dim bright-black for hints — so every slot stays in its own hue family and only leans toward the plant. Turnip's red is a rose-tinted red rather than its purple top, and a leaf becomes a leaf-tinted teal for `cyan` rather than another green.

| Slot | Hue family | Tinted toward |
|---|---|---|
| `background` | — | The root's deepest skin or bark tone (dark) |
| `foreground` | — | The root's lightest flesh |
| `cursor` | — | The most vibrant part of the flesh |
| `black` | neutral | Dried skin, a step above the background |
| `white` | neutral | Lightest interior or pith |
| `red` | red | Skin blush or red/purple flesh streaks |
| `green` | green | The plant's leaves |
| `yellow` | yellow | Flowers or golden flesh |
| `blue` | blue | Stems or cool shadows |
| `magenta` | magenta | Flowers or purple flesh |
| `cyan` | cyan | Young leaves or shoots |
| `selection` | — | Semi-transparent flesh tone |
| `url` | — | Bright leaf or shoot color |

Not every root is warm: daikon, wasabi, and horseradish use cool or neutral charcoal backgrounds because the plants do.

#### Palette contract

The test `root_theme_palettes_keep_ansi_meaning_and_contrast` in `termeric/src/config/theme.rs` enforces these rules for every theme authored `"Termeric"`:

- Each chromatic slot stays in its hue family (HSL degrees): red below 20 or from 345, yellow 35–70, green 75–155, cyan 160–200, blue 200–255, magenta 275–345.
- Saturation is at least 0.25, so tinted colors still read as colors. Keep it near or below 0.6 so nothing turns neon (advice, not enforced).
- Contrast against the background is at least 3:1 for primary slots and 4.5:1 for bright slots, and every bright slot is lighter than its primary.
- Neighbouring families (red/yellow, yellow/green, green/cyan, cyan/blue, blue/magenta, magenta/red) stay at least 25° apart.
- `black` is visibly lighter than the background (at least 1.15:1), bright black is readable dim text (at least 3:1), and whites reach 4.5:1 and 7:1.
- Foreground text reaches at least 7:1.

#### Theme files

Built-in themes are TOML files in `termeric/themes/roots/` and `termeric/themes/community/`, embedded into the binary. `themes/roots/turmeric.toml` is the reference. A file has four tables:

- `[meta]`: `name`, `author`, `description`
- `[primary]` and `[bright]`: the eight ANSI colors each
- `[ui]`: `background`, `foreground`, `cursor`, `cursor_text`, `selection`, `border`, `tab_active` (the active-tab accent), `tab_inactive` (the tab strip), `search_match`, `url`, and optionally `selection_text`

Colors are `#RRGGBB` or `#RRGGBBAA`. In a root theme, give every color a short comment naming the part of the plant it came from.

To add a root theme:

1. Add `termeric/themes/roots/<name>.toml`.
2. Add the name to `BUILTIN_THEME_NAMES` and an `include_str!` arm to `Theme::load_builtin`, both in `termeric/src/config/theme.rs`.
3. Bump the root count in `root_theme_palettes_keep_ansi_meaning_and_contrast`.
4. Add it to the theme list under `[theme]` in `assets/config.example.toml`.
5. Run `cargo test -p termeric config::theme`.

People can add their own themes as `.toml` files in `~/.config/termeric/themes/`; the **Change Theme…** picker previews each theme as it is highlighted.

### Chrome

The terminal content comes first; the chrome around it stays quiet.

- **Color:** every chrome color comes from the active theme through `UiPalette` (`termeric/src/renderer/colors.rs`), so light themes and custom themes work. Don't hardcode colors. Shadows are black at low alpha.
- **Tab bar:** compact and "quiet native". The strip uses the theme's `tab_inactive` color; the active tab is a slightly raised surface with a 2px `tab_active` accent line (a left bar in the vertical rail); inactive titles are dimmed; × appears on the active or hovered tab, and background activity shows as a small dot in its place. Never fill a whole tab with a saturated color.
- **Settings:** one centered dialog with a sidebar of pages and one search across every page. Values sit in one column with steppers, dropdown chevrons, or a trailing chevron for actions. The footer only says whether changes are saved (with a Save button when they aren't); no key-hint strips.
- **Dialogs and menus:** start from the shared pieces — `build_modal_frame` (`renderer/palette.rs`) for centered dialogs, `build_side_panel_frame` (`renderer/panels.rs`) for side panels, and `build_popup_menu` (`renderer/popup_menu.rs`) for anchored menus; a menu with its own rows, such as the Settings dropdown, still sits on the shared `push_menu_surface`.
- **Icons:** use the vector icons (`ui::icon::Icon`, drawn with `push_icon`/`draw_icon` from `renderer/icons.rs`) rather than text glyphs such as ◀ ▶ ×.
- **Hover and feedback:** a gentle background tint, not a color change. Prefer status text and toasts over modal dialogs.

#### Corners

Draw rounded rectangles with `RectPipeline::push_rounded_rect`; a radius of half the height makes a pill or circle. The radii in use:

| Radius | Used for |
|---|---|
| 12px (`CARD_CORNER_RADIUS`) | Cards: toasts, the info overlay, the guardrail confirmation, Harvest panels |
| 10px | The shared modal frame (Settings, command palette) |
| 8px | Popup menus, dropdowns, the find box |
| 6px (`BUTTON_CORNER_RADIUS`) | Buttons, tabs, Settings sidebar rows |
| 2px | Block cursor |

Shared constants live in `termeric/src/ui/constants.rs`; add a constant there rather than a new literal when a radius is reused.

#### Geometry

Chrome geometry lands on whole physical pixels. Rendering, hit testing, keyboard focus, and accessibility bounds come from one shared function per component (see `TabChrome` in `renderer/tab_bar.rs`), so what a person clicks is always what they see.

### Typography

- **Terminal text** uses the configured monospace font (default `monospace`, 12pt). `font.size` is in points at 96/72 logical pixels per point, from 8pt to 72pt. The Settings stepper moves 1pt; the zoom shortcuts move 2pt. Terminal text keeps the monospace grid: no custom letter-spacing or squeezed advances.
- **UI text** (tabs, Settings, the palette, panels, menus, dialogs, toasts, the find box) uses the desktop's sans-serif face at 10pt, resolved through fontconfig, so chrome reads like the rest of the desktop. Draw it with `Renderer::push_ui_label`, which measures proportionally, centres on the face's own metrics, and shortens with an ellipsis.
- **Emoji** render inline when a font supports them and fall back gracefully.

### Accessibility

- High-contrast mode raises built-in chrome text to at least 4.5:1 (`enforce_high_contrast` in `renderer/colors.rs`); new chrome colors must go through `UiPalette` so they get the same treatment.
- Every interactive control needs keyboard access and an AccessKit node whose bounds come from the same geometry used to draw it.
