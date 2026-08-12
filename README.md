# Termeric

A GPU-accelerated terminal emulator written in Rust, themed around roots and rhizomes.

## Highlights

- **GPU rendering** via `wgpu`, font shaping via `cosmic-text`, ANSI parsing via `vte`
- **26 themes** across two sets: 12 "roots" (beet, ginger, turmeric, wasabi, …) and 14 community ports (catppuccin, dracula, gruvbox, nord, rose-pine, tokyo-night, …)
- **Panes, tabs, and sessions** — split layouts, persisted sessions, per-tab transcript recording
- **Search** with regex and filter modes across scrollback
- **Command palette** (Ctrl+P) with fuzzy search, plus a customizable keyboard shortcuts panel
- **Plugins** — built-in Claude Code and Codex integrations with guardrails, diff review, and context picker
- **Graphics & hyperlinks** — Kitty image protocol, OSC 8 hyperlinks, shell integration, font ligatures
- **Harvest (experimental beta)** — opt-in semantic command history, per-terminal growth indicators, scoped output search, rerun/copy, and AI explanation

## Status

Active development — expect churn. Works day-to-day on Linux; macOS and Windows are best-effort until we get broader testing.

## Building

Requires Rust 2024 edition (1.85+).

```bash
cargo build --release
./target/release/termeric
```

For development:

```bash
cargo build              # optimized + debuginfo
cargo test               # ~320 tests, runs in <1s
```

## Configuration

Termeric reads `~/.config/termeric/config.toml`. A starter config lives in [`assets/config.example.toml`](assets/config.example.toml):

```toml
[font]
family = "monospace"
size   = 14.0

[theme]
name = "turmeric"

[window]
opacity = 1.0
padding = 8
```

Settings are also editable live from within the terminal — press the gear icon in the tab bar, or open the settings modal via the command palette.

Bundled themes live under [`termeric/themes/`](termeric/themes/). Put personal
themes in `~/.config/termeric/themes/` (or point `[theme].path` at an explicit
file) and they appear in both theme pickers, survive restart, and reload when
the active file changes. A malformed edit keeps the last valid colors active
and reports the file error in-app.

The built-in Codex integration is tested against `codex-cli 0.137.0`. Its
friendly approval modes map to current Codex policies as follows: `suggest`
uses `untrusted`, `auto-edit` uses `on-request`, and `full-auto` uses `never`.
See the starter config for the corresponding sandbox options.

### Harvest and shell integration

Harvest is an experimental beta and remains disabled by default. When enabled,
its commands appear in the command palette and its history is populated from
OSC 133 semantic prompt marks. Normal interactive Bash, Zsh, and Fish tabs get
those marks automatically; Termeric writes small generated integration files
under `~/.config/termeric/shell-integration/` for Bash and Zsh and uses Fish's
init-command hook. The generated files load the user's normal shell rc first.

Custom shells and shell configurations that execute a script or `-c` command
are left untouched. Harvest then explains that prompt marks are unavailable
instead of showing an empty menu. Existing tabs keep their launch environment
when shell configuration changes; open a new tab to pick up the new setup.

```toml
[harvest]
enabled = true
show_indicators = true
max_history = 100
```

## Project layout

```
termeric/           Binary crate — windowing, rendering, UI, plugins
  src/app/            App struct, winit event loop, action dispatch
  src/renderer/       wgpu rendering (tab bar, panels, overlays, palette, …)
  src/ui/             UI components (animation, palette, settings, shortcuts, …)
  src/plugin/         Plugin trait + Claude Code / Codex integrations
  src/panes/          Pane layout tree and split management
  src/session/        Session save/restore + transcript export
  themes/             Bundled theme TOML files (roots + community)

termeric-core/      Library crate — terminal emulation
  src/term/           Terminal state machine, cursor, modes, selection
  src/grid/           Cell grid, rows, scrollback storage
  src/ansi/           VT/ANSI escape sequence processor
  src/pty/            PTY creation and I/O
  src/graphics/       Kitty image protocol
  src/shell/          Shell integration, prompt detection
```

## Documentation

- [`CLAUDE.md`](CLAUDE.md) — architecture overview, critical patterns, module map
- [`STYLE_GUIDE.md`](STYLE_GUIDE.md) — UI / theming style guide (read before touching user-facing code)
- [`AGENTS.md`](AGENTS.md) — rules and conventions for AI agents working in this repo

## License

MIT
