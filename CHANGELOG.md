# Changelog

Notable changes to Termeric, by release. The project follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and uses Semantic Versioning once public beta tags begin.

## [0.1.0] - Unreleased

The first release: a developer preview for Linux, on Wayland and X11. Features still taking shape are marked experimental; the README's [Status](README.md#status) section lists them.

### Added

- GPU rendering, with automatic fallback to software rendering and recovery from GPU resets without losing your shells.
- Tabs, split panes, and windows: layout presets, zoom, broadcast input to several panes, pinned tabs, and moving tabs and panes between windows.
- Workspaces that save windows, tabs, pane layouts, folders, and scrollback, and restore them at startup. Programs start fresh on restore.
- 26 built-in themes, twelve named after roots and fourteen from the community, plus your own theme files, which reload when you save them, and background images.
- Ligatures and OpenType features, color emoji, wide and combined characters, an ordered font fallback list, per-range font overrides, and a Unicode character picker.
- Inline images through the Kitty graphics protocol, Sixel, and iTerm2, and Kitty file transfer with approval for each file.
- Shell integration for Bash, Zsh, Fish, Nushell, and Elvish: prompt jumping and failed-command navigation.
- Search with plain text, regular expressions, and a filter mode; keyboard copy mode; quick-select hints; clickable hyperlinks and detected links.
- A command palette, a Settings dialog, rebindable shortcuts, terminal profiles, and a quick-access drop-down terminal.
- xterm modified keys and the Kitty keyboard protocol; mouse, focus, and bracketed-paste reporting; synchronized output; and the `xterm-termeric` terminfo entry.
- Screen reader information, keyboard access to the interface (F6), reduced motion, and a high-contrast mode.
- Launching windows, tabs, commands, profiles, and workspaces from the command line.
- A `.deb` package, an Arch PKGBUILD, and a tarball with an `install.sh` for other distributions, each with a manual page and Bash, Zsh, and Fish completions.
- Experimental: status, change review, and a supervisor for coding agents (Claude Code, Codex, Gemini CLI, OpenCode, and your own), the agent view, and `termeric mcp`, all off until you turn on agent features; controlling a running Termeric from scripts; third-party plugins that get only the permissions you grant; Harvest; and live domains.

### Known limitations

- macOS and Windows aren't supported yet.
- Right-to-left and bidirectional text aren't supported.
- Screen reader support hasn't been confirmed with Orca yet.
- Removing secrets from exported transcripts is best effort.
