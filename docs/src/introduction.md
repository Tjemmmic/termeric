# Introduction

Termeric is a GPU-accelerated terminal emulator for Linux, written in Rust and themed around roots and rhizomes. It draws your terminal with the graphics card, keeps many shells organized in tabs, panes, and windows, and understands what your shell and your tools are doing, so you can jump between commands, open links, show images, and keep an eye on coding agents without leaving the terminal.

Termeric is for people who live in a terminal on a Linux desktop: developers, system administrators, and anyone who wants a fast, keyboard-friendly terminal that still has a point-and-click side when you want one.

## Highlights

- **GPU rendering.** Text, images, and the interface are drawn with your GPU. If no usable GPU is available, Termeric falls back to software rendering on its own.
- **Tabs, panes, and windows.** Split any tab into panes, rearrange them with layout presets, zoom a pane, broadcast input to several panes at once, and drag tabs between windows. See [Tabs, panes, and windows](tabs-panes-windows.md).
- **Workspaces.** Save the tabs, panes, folders, and scrollback of a window and bring them back later, or have Termeric restore them at startup. See [Workspaces and sessions](workspaces-and-sessions.md).
- **Themes named after roots.** Twelve built-in themes named after roots and rhizomes (Turmeric, Ginger, Daikon, Galangal, Wasabi, and more), fourteen popular community themes such as Dracula, Nord, and Catppuccin, and your own theme files. See [Themes and backgrounds](themes.md).
- **Good text.** Programming ligatures, color emoji, wide Unicode characters, a font fallback chain you control, and a searchable Unicode character picker. See [Fonts and text](fonts-and-text.md).
- **Images in the terminal.** Tools that use the Kitty graphics protocol, Sixel, or iTerm2 inline images can draw pictures directly in a pane. See [Images and file transfer](images-and-files.md).
- **Shell integration.** Bash, Zsh, Fish, and Nushell mark prompts and commands automatically (Elvish takes one line in its rc file), so you can jump between prompts, find failed commands, and open new panes in the current folder. See [Shell integration](shell-integration.md).
- **Search, copy mode, and links.** Search scrollback with plain text or regular expressions, select and copy with the keyboard only, and open links and file paths with a click or a hint key. See [Search, copy mode, and links](search-and-copy-mode.md).
- **Command palette and Settings.** Every action is one fuzzy search away, and most options have a place in the Settings dialog, with changes shown as you make them. Shortcuts can be rebound, apart from a few fixed keys such as F6.
- **Coding-agent integration (experimental).** Termeric recognizes Claude Code, Codex, Gemini CLI, OpenCode, and agents you add yourself, shows their status on tabs, and lets you review the changes they make. See [Agents in terminals](agents.md) and the experimental [Agent view](agent-view.md).
- **Automation and plugins.** Launch windows and tabs from the command line, add third-party plugins, and script a running Termeric through its control socket (experimental). See [Command line](command-line.md), [Controlling a running Termeric](control.md), and [Plugins](plugins.md).
- **Accessibility.** Screen reader support, keyboard access to the interface, reduced motion, and a high-contrast mode. See [Accessibility](accessibility.md).

## Status

Termeric is a **Linux developer preview**. It runs on both Wayland and X11 desktops. macOS and Windows are not supported yet.

Expect rough edges. Features that are still taking shape are labelled **Experimental** in this guide and in the interface, and some of them are turned off until you enable them. [Feature status](status.md) lists what works fully, what works in part, and what isn't there yet.

## Getting started

1. [Install Termeric](installation.md) from a package or build it from source.
2. Follow [First steps](first-steps.md) to open tabs and panes, find the command palette and Settings, and pick a theme and font size.
3. When something doesn't behave, see [Troubleshooting](troubleshooting.md).
