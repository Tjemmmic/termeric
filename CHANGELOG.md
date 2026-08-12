# Changelog

All notable changes to Termeric are recorded here. The project follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and uses Semantic
Versioning once public beta tags begin.

## [Unreleased]

### Added

- Headless App, PTY, terminal-protocol, plugin-exit, and optional WGPU smoke
  coverage.
- Persistent custom themes and automatic Bash, Zsh, and Fish semantic shell
  integration for experimental Harvest mode.
- Complete transcript Markdown/JSONL export and explicit persistence notices.
- Reproducible Linux packaging metadata, desktop integration, and CI gates.

### Changed

- Updated the Codex integration for current approval, sandbox, and resume CLI
  contracts.
- Unified live configuration application across startup, settings, reload,
  reset, plugins, shortcuts, and Harvest.
- Split App orchestration into focused configuration, desktop I/O, plugin,
  session, workspace, and mouse modules.

### Fixed

- Correct active-plugin routing, dynamic command execution, and visible Git,
  clipboard, plugin, session, and transcript failure feedback.
- Correct UTC session timestamps, collision-resistant identifiers, and visible
  5 MB scrollback / 50 MB transcript limits.

### Known limitations

- Linux is the current supported beta target; macOS and Windows are best-effort.
- Harvest is experimental and disabled by default.
- AI transcript redaction is best-effort, not a complete secret scanner.
- Interactive Claude/Codex sessions require their separately installed CLIs
  and user authentication.
