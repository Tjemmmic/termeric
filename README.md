# Termeric

A GPU-accelerated terminal emulator for Linux, written in Rust and themed around roots and rhizomes.

Termeric keeps your shells in tabs, panes, and windows, draws them with your GPU, and understands what your shell and tools are doing: jump between prompts, open links and paths, show images, and keep an eye on coding agents without leaving the terminal.

## Highlights

- **Tabs, panes, and windows**, with layout presets, zoom, broadcast input, and tabs you can drag between windows
- **Workspaces** that save your windows, tabs, panes, folders, and scrollback, and bring them back at startup
- **26 themes**: twelve named after roots (Turmeric, Ginger, Daikon, and more) and fourteen community favorites, plus your own
- **Good text**: ligatures, color emoji, wide characters, a fallback chain you control, and a Unicode character picker
- **Images** through the Kitty graphics protocol, Sixel, and iTerm2 inline images
- **Shell integration** for Bash, Zsh, Fish, Nushell, and Elvish: prompt jumping and failed-command navigation
- **Search, keyboard copy mode, and quick-select hints** for links, paths, and hashes
- **A command palette and Settings dialog**, with every shortcut rebindable
- **Coding-agent integration** for Claude Code, Codex, Gemini CLI, OpenCode, and your own agents
- **Automation**: launch from the command line, script a running Termeric, and add plugins

## Status

Termeric is a **developer preview for Linux** (Wayland and X11). Expect rough edges; features that are still taking shape are marked experimental. macOS and Windows aren't supported yet. See [Feature status](docs/src/status.md).

## Install

Packages for Debian/Ubuntu and Arch Linux will come with the first release. Until then, build from source with a stable Rust toolchain (1.89 or newer):

```bash
sudo apt-get install build-essential pkg-config libfontconfig1-dev libwayland-dev \
  libxkbcommon-dev libxcb-shape0-dev libxcb-xfixes0-dev libvulkan-dev mesa-vulkan-drivers
cargo build --release --locked
./target/release/termeric
```

[Installation](docs/src/installation.md) covers the desktop entry, icons, and terminfo.

## Documentation

The [user guide](docs/src/introduction.md) covers everything from [first steps](docs/src/first-steps.md) and [configuration](docs/src/configuration.md) to [agents](docs/src/agents.md) and [plugins](docs/src/plugins.md). Build it locally with `mdbook build docs`.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). [AGENTS.md](AGENTS.md) describes the architecture for contributors and coding agents.

## License

Termeric is released under the [MIT License](LICENSE). Third-party licenses are listed in [THIRD_PARTY_NOTICES.md](termeric/THIRD_PARTY_NOTICES.md).
