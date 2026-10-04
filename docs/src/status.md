# Feature status

This page sums up what Termeric supports today. Use it to check whether something you need is ready before you depend on it. Each feature is in one of four groups:

- **Supported** features work fully on Linux.
- **Partial** features work for the cases described, with the stated gaps.
- **Experimental** features are previews: off by default or labelled Experimental, and they may change.
- **Not supported** features aren't implemented.

## Platforms

| Platform | Status |
|---|---|
| Linux, Wayland | Supported (developer preview) |
| Linux, X11 | Supported (developer preview) |
| macOS | Not supported yet |
| Windows | Not supported yet |

## Supported

**Rendering and text**

- GPU rendering, with automatic fallback to software rendering and recovery from GPU resets without losing your shells.
- 16-color, 256-color, and 24-bit color, with bold, dim, italic, blink, inverse, hidden, strikethrough, and five underline styles in any color.
- Wide characters, emoji, and combined characters in left-to-right text.
- Color emoji and programming ligatures.
- 26 built-in themes and your own theme files. See [Themes and backgrounds](themes.md).
- A Unicode character picker that searches every character by name, and emoji by English name and keyword.

**Terminal behavior**

- Shells and programs run in a real terminal (PTY) with resize and exit handling.
- The standard cursor, editing, scrolling-region, and alternate-screen sequences that full-screen programs such as Vim, less, and tmux rely on.
- Synchronized output, so programs can update the screen without flicker.
- Mouse reporting, focus reporting, and bracketed paste.
- Scrollback that rewraps when you resize the window.
- Window titles, OSC 52 clipboard writes, and color changes requested by programs.
- The `xterm-termeric` terminfo entry, with `xterm-256color` as a fallback. See [Terminal compatibility](compatibility.md).

**Working in Termeric**

- Tabs, split panes, and multiple windows, including layout presets, zoom, swap, and moving panes and tabs between windows. See [Tabs, panes, and windows](tabs-panes-windows.md).
- Broadcasting input to several panes of the active tab.
- Selection, copy, and paste, including the primary selection and middle-click paste; search with plain text, regular expressions, and a filter mode. See [Search, copy mode, and links](search-and-copy-mode.md).
- Keyboard copy mode and quick-select hints for links and other matches.
- Clickable OSC 8 hyperlinks and detected URLs.
- Shell integration for Bash, Zsh, Fish, Nushell, and Elvish: prompt jumping and failed-command navigation. See [Shell integration](shell-integration.md).
- Saving and restoring complete workspaces: windows, tabs, pane layouts, folders, and scrollback. Programs are started fresh on restore. See [Workspaces and sessions](workspaces-and-sessions.md).
- Launching windows, tabs, commands, profiles, and workspaces from the command line. See [Command line](command-line.md).
- Settings, the command palette, and rebindable shortcuts. See [Keyboard and mouse](keyboard-and-mouse.md).
- Third-party plugins, described by a declarative manifest and granted explicit permissions. See [Plugins](plugins.md).

## Partial

| Feature | What works | What doesn't |
|---|---|---|
| Keyboard protocols | Legacy, xterm modified-key modes, and the Kitty keyboard protocol. | A few rare keys and modifiers, such as Hyper, Meta, and the keypad Begin key. |
| Status queries | Common version, capability, mode, and setting queries programs send. | Window-manipulation requests only report sizes; programs can't move or resize the window. |
| Mouse pointer shapes | Programs can set the mouse pointer shape (OSC 22). | Old X11 cursor-font names aren't recognized. |
| Kitty multiple cursors | Extra cursors drawn by programs that use Kitty's protocol. | Their appearance over unusually narrow, wide, or scaled cells isn't verified yet. |
| Shell-reported folder | OSC 7 folder reports drive tab titles, new-pane folders, and workspaces. | Under Nushell, folder names containing an encoded slash (`%2F`) are misread. |
| Fonts | Font choice, an ordered fallback list, per-range font overrides, OpenType features, and variable font weight. | Variable font axes other than weight. |
| Text sizing | Kitty's text-sizing protocol (OSC 66) for larger and explicitly sized text. | How well it looks depends on the font. |
| Background images | A static local PNG or JPEG behind the terminal, with fit and opacity. | Tiling, animated images, remote URLs, and different images per tab or pane. |
| Kitty graphics | Most of the protocol, including animation, Unicode placeholders, and relative placements. | Programs can't load images from ordinary files on disk unless you set `allow_kitty_file_transfers = true` under `[security]`. |
| Sixel and iTerm2 images | Static Sixel images and iTerm2 inline PNG images. | iTerm2 file downloads, non-PNG iTerm2 images, and animation. |
| Kitty file transfer | Copying files, folders, and links from a program or remote shell to your machine, and sending up to 16 files (64 MiB in all) back, each after you approve it. | Resuming, overwriting, delta transfers, and sending folders back. |
| Input methods | Composing text with an input method such as Fcitx or IBus in the terminal and text fields. | Dead keys aren't verified yet. Kanji use your locale's CJK font variant. |
| Accessibility | Screen reader information for tabs, panes, text, and dialogs; keyboard access to the interface (F6); reduced motion and high contrast. See [Accessibility](accessibility.md). | Not yet confirmed with Orca. |
| Tabs | Creating, renaming, pinning, reordering, and dragging tabs between windows. | Tabs can't move between windows of separate Termeric processes. |
| Terminal profiles | Reusable launch profiles for programs and for serial devices. | Serial profiles aren't verified with physical serial hardware yet. |
| Read-only panes | Locking a pane against input while its output continues. | Restoring a workspace unlocks every pane. |
| Transcripts | Recording and exporting a terminal's output. | Removing secrets from exports is best effort. |
| Window stacking | Asking for always-on-top or always-on-bottom windows. | Your desktop decides the final order. |
| Quick-access terminal | A drop-down terminal toggled with `termeric --quick-access`. | On Wayland, exact placement and focus may need a compositor rule for its `termeric-quick-access` app ID. |
| Drag and drop | Dropping a file or folder inserts its quoted path. On an experimental live-domain pane, it uploads the file or folder after you confirm. | Live-domain uploads are limited to 64 MiB. |
| Fullscreen | F11 makes the current window fullscreen. | Fullscreen isn't remembered between launches. |
| Git | Branch-name completion for Git commands, and the branch recorded with an agent transcript. | Termeric isn't a Git client: staging and reverting are limited to reviewing an agent's changes. |
| Notifications and bells | Visual and audio bells, desktop notifications from programs (OSC 9, 777, and 99), link opening. | How notifications look and behave is up to your desktop. |

## Experimental

- **Coding agents in terminals.** Recognizing Claude Code, Codex, Gemini CLI, OpenCode, and your own agents, with status badges, change review, and a supervisor. Status reporting needs **Set Up Agent Status…**. See [Agents in terminals](agents.md).
- **Agent view.** A docked panel that runs Claude Code, Codex, or OpenCode as a conversation instead of a terminal. Open it with Ctrl+Shift+A. See [Agent view](agent-view.md).
- **Harvest.** A visual history of your commands and their output. Turn it on under **Experimental** in Settings or with `enabled = true` under `[harvest]`.
- **Controlling a running Termeric.** Listing windows and reading terminal text from scripts. Sending input is off until you set `security.allow_gui_control_input`. See [Controlling a running Termeric](control.md).
- **Live domains.** Terminals kept alive by a background daemon, locally or over SSH, that you can detach from and reattach to. See [Live domains](live-domains.md).

## Not supported

- macOS and Windows.
- Right-to-left and bidirectional text.
- Printing through the terminal.
- Programs moving or resizing the Termeric window through escape sequences.
- Named, saved, or cross-window broadcast groups.
