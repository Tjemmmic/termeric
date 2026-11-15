# Feature status

What Termeric supports today, and what is planned. Termeric runs on Linux, on both Wayland and X11, and on macOS 11 or later; where they differ, the notes say so, and [macOS](#macos) gathers what is different there. Platforms and how to install on each are in the README's [Install](README.md#install) section.

| Status | Meaning |
| --- | --- |
| Supported | Works fully. |
| Partial | Works as the notes describe, with the gaps they name. |
| Experimental | Works, but is off by default or labelled Experimental, and may change between previews. |
| Planned | Not built yet, but on the roadmap. |
| Not supported | Not built, and not planned yet. |

## Rendering and text

| Feature | Status | Notes |
| --- | --- | --- |
| GPU rendering | Supported | Falls back to Mesa's software renderer when no GPU can be used, and recovers from GPU resets without losing your shells. |
| Colors and text styles | Supported | 16, 256, and 24-bit color; bold, dim, italic, blink, inverse, hidden, strikethrough, and five underline styles in any color. |
| Wide characters, emoji, and combining characters | Supported | Color emoji included. |
| Ligatures and OpenType features | Supported | |
| Fonts | Partial | Font choice, an ordered fallback list, per-range font overrides, and variable font weight. Variable font axes other than weight aren't used. |
| Text sizing (Kitty's OSC 66) | Partial | How well larger text looks depends on the font. |
| Right-to-left and bidirectional text | Planned | |
| Themes | Supported | 38 built in (twelve named after roots, each dark and light, and fourteen from the community) and your own theme files, which reload when you save them. |
| Window opacity | Supported | Needs a compositor with transparency. |
| Background images | Partial | A static local PNG or JPEG with fit and opacity, chosen in Settings with the desktop's file picker. Not tiling, animated images, remote URLs, or different images per tab or pane. |
| Unicode character picker | Supported | Searches every character by name, and emoji by English name and keyword. |

## Terminal compatibility

| Feature | Status | Notes |
| --- | --- | --- |
| Full-screen programs | Supported | The cursor, editing, scrolling-region, and alternate-screen sequences that Vim, less, and tmux rely on, and scrollback that rewraps on resize. |
| Synchronized output | Supported | |
| Mouse reporting, focus reporting, and bracketed paste | Supported | |
| Keyboard protocols | Partial | Legacy keys, xterm modified keys, and the Kitty keyboard protocol. The Hyper and Meta modifiers and the keypad Begin key aren't sent. |
| Status queries | Partial | The common version, capability, mode, and setting queries. Programs can't move or resize the window; window-manipulation requests only report sizes. |
| Window titles, colors, and OSC 52 clipboard | Supported | Programs can write the clipboard by default; reading it is off until you allow it under `[security]`. |
| Mouse pointer shapes (OSC 22) | Partial | CSS cursor names only, not old X11 cursor-font names. |
| Kitty multiple cursors | Partial | How they look over unusually narrow, wide, or scaled cells isn't verified yet. |
| Notifications and bells | Supported | Visual and audio bells, and desktop notifications from programs (OSC 9, 777, and 99). Clicking a notification raises its window on X11; on Wayland it only marks the window as needing attention. |
| Terminfo | Supported | The `xterm-termeric` entry, with `xterm-256color` when it isn't installed. |
| Printing | Not supported | |

## Images and files

| Feature | Status | Notes |
| --- | --- | --- |
| Kitty graphics | Partial | Most of the protocol, including animation, Unicode placeholders, and relative placements. Loading images from files on disk, ordinary or temporary, needs `allow_kitty_file_transfers = true` under `[security]`; without it, `kitten icat` sends images inline. |
| Sixel and iTerm2 images | Partial | Static Sixel images and iTerm2 inline PNG images. Not iTerm2 file downloads, other iTerm2 image formats, or animation. |
| Kitty file transfer | Partial | Files, folders, and links from a program or remote shell to your machine, and up to 16 files (64 MiB in all) back, each after you approve it. Not resuming, overwriting, delta transfers, or sending folders back. Refused in agent and plugin panes and in serial tabs. |
| Drag and drop | Partial | Dropping a file or folder types its quoted path into the pane, including agent and plugin panes. Not into serial tabs or read-only panes. On a live-domain pane it uploads the file after you confirm, one at a time, up to 64 MiB. Dropping several files types each path in turn. On X11 the pointer shows beforehand whether the pane takes the drop; on Wayland it doesn't, and a pane that refuses one says why. |

## Tabs, panes, and windows

| Feature | Status | Notes |
| --- | --- | --- |
| Tabs | Partial | Creating, renaming, pinning, reordering, and dragging tabs between windows, in a top bar or a vertical rail. Tabs can't move between separate Termeric processes. On Wayland, a dragged tab joins the window the pointer enters within 400 ms of leaving its own; otherwise it becomes a new window. |
| Split panes | Supported | Layout presets, zoom, swap, and moving panes between windows. |
| Broadcast input | Partial | To several panes of the active tab. Not named, saved, or cross-window groups. |
| Read-only panes | Partial | A locked pane keeps showing output. Restoring a workspace unlocks every pane. |
| Quick-access terminal | Partial | A drop-down terminal: `termeric --quick-access` starts it, then shows or hides it, and its shells keep running while it's hidden. Bound to a desktop shortcut, it takes focus when shown. Wayland compositors may ignore its placement; a compositor rule for its `org.termeric.Termeric.QuickAccess` app ID fixes that. |
| Always on top or bottom | Partial | Your desktop decides the final order, and many Wayland compositors ignore the request. |
| Fullscreen | Partial | F11 (⌃⌘F on macOS). Not remembered between launches. |

## Input, selection, and search

| Feature | Status | Notes |
| --- | --- | --- |
| Selection, copy, and paste | Supported | Including the primary selection, middle-click paste, and optional copy on select. On Wayland, the clipboard needs a compositor with the data-control protocol, or XWayland running. |
| Search | Supported | Plain text, regular expressions, and a filter mode. |
| Keyboard copy mode and quick select | Supported | Quick-select hints for links, paths, and hashes. |
| Hyperlinks | Supported | OSC 8 links and detected URLs. |
| Input methods | Partial | Composing text with an input method such as Fcitx or IBus. Dead keys aren't verified yet. |
| Shortcuts and keybindings | Supported | Every shortcut can be rebound; custom keybindings can type text, run commands, or run plugin commands. |

## Shells, workspaces, and launching

| Feature | Status | Notes |
| --- | --- | --- |
| Shell integration | Supported | Bash, Zsh, Fish, Nushell, and Elvish: prompt jumping and failed-command navigation, which scroll to the prompt and tint its row until you type or click. |
| Shell-reported folder (OSC 7) | Partial | Drives tab titles, new-pane folders, and workspaces. Under Nushell, folder names with an encoded slash (`%2F`) are misread. |
| Git branch completion | Supported | For Git commands typed at the prompt. |
| Workspaces | Supported | Saving and restoring windows, tabs, pane layouts, folders, and scrollback; programs start fresh. Save Workspace As…, and Save Workspace in a window not saved yet, ask for a name. In Manage Workspaces…, Enter opens the selected workspace, F2 renames it, and Delete deletes it. `termeric workspace list` lists them. |
| Terminal profiles | Partial | Reusable launch profiles for programs and serial devices. Serial profiles aren't verified with physical hardware yet. |
| Command line | Supported | Launching windows, tabs, commands, profiles, and workspaces, with a manual page and Bash, Zsh, and Fish completions. |
| Settings and the command palette | Supported | |

## Automation and extensions

| Feature | Status | Notes |
| --- | --- | --- |
| Scripting a running Termeric (`termeric control`) | Experimental | List, open, split, focus, move, resize, and close windows, tabs, and panes, wait for programs, and read pane text. Typing into panes needs `allow_gui_control_input = true` under `[security]`. On Wayland, focusing another window is refused. |
| Third-party plugins (`termeric plugin`) | Experimental | Each plugin gets only the permissions you grant. The manifest format, the permissions, and the handler protocol may change. |
| Live domains (`termeric domain`) | Experimental | Terminals kept running by a background daemon, here or over SSH, that you can detach from and attach to again. Nothing runs until you create one, with `termeric domain create` or **Attach Live Domain…** in the command palette. |
| Harvest | Experimental | A visual history of your commands. Off until you turn it on under **Experimental** in Settings. |

## Coding agents

Everything here is off until you turn on **Agent Features** on the **Agents** page of Settings.

| Feature | Status | Notes |
| --- | --- | --- |
| Recognizing agents in terminals | Experimental | Claude Code, Codex, Gemini CLI, OpenCode, and agents you add. Status reports work for Claude Code and Codex, after **Set Up Agent Status…**. |
| Change review | Experimental | Reviewing, staging, and reverting an agent's changes to a Git repository. |
| Agent view | Experimental | A docked panel that runs Claude Code, Codex, or OpenCode as a conversation. Tab moves between its controls, the arrow keys move the choice of agent, PageUp and PageDown scroll, and Ctrl+Home and Ctrl+End (⌥Home and ⌥End on macOS) go to the start and end (in the composer, while it is empty). |
| Supervisor | Experimental | |
| Transcripts | Experimental | Recording and exporting an agent's session. Removing secrets from exports is best effort. |
| `termeric mcp` | Experimental | Termeric's tools for a coding agent, over MCP. |
| A build without agent code | Planned | Agent features compiled out, not only switched off. |

## Accessibility

| Feature | Status | Notes |
| --- | --- | --- |
| Screen readers | Partial | Tabs, panes, terminal text, and dialogs are described to screen readers. Not yet confirmed with Orca. |
| Keyboard access to the interface | Supported | F6 moves between the terminal and the tab bar and toolbar. |
| Reduced motion and high contrast | Supported | |

## macOS

macOS is built and tested on Apple silicon in CI, including a run of the app itself; it hasn't been tested by hand yet. What works differently there:

| Feature | Status | Notes |
| --- | --- | --- |
| Keyboard shortcuts | Supported | The defaults use ⌘ as Mac apps do (⌘C, ⌘V, ⌘T, ⌘W, ⌘N, ⌘1–⌘9, ⇧⌘] and ⇧⌘[ for tabs, ⌘D and ⇧⌘D to split, ⌥⌘ arrows between panes); the Keyboard Shortcuts panel shows them with ⌃⌥⇧⌘. An unbound ⌘ chord sends nothing to the program, and ⌥← and ⌥→ move by word in the shell. |
| Option key | Supported | Option types characters by default; `macos_option_as_alt` under `[keyboard]` makes the left, right, or both Option keys act as Alt. |
| Menu bar | Supported | Menu commands run in the focused window as their shortcuts do; while a dialog, panel, the find box, tab rename, or the agent view has the keyboard, they do nothing, except Paste, which pastes there. Menus show only the system's own shortcuts (Hide, Quit, Minimize): macOS would otherwise take those keys before Termeric's own input handling. |
| Quitting | Supported | ⌘Q, the Dock's Quit, logging out, and shutting down keep every open window's workspace for the next start. |
| Shells | Supported | Your shell starts as a login shell, as in Terminal.app. macOS's own Bash 3.2 has no command-start marks (`PS0`); a newer Bash from Homebrew has them. Started from the Finder or the Dock, Termeric takes `PATH` from your login shell and sets `LANG` from your language and region when nothing else does. |
| Fonts | Supported | `monospace` is Menlo, and chrome uses the system font. A font size is in the Mac's points (one logical pixel), so `size = 12` looks as 12 pt does in other Mac apps. |
| Colors and transparency | Supported | Colors are drawn in sRGB, so themes look the same as elsewhere on wide-gamut displays; `opacity` under `[window]` works. Title bars follow the theme's light or dark appearance. |
| Clipboard | Supported | macOS has no primary selection, so Termeric keeps one of its own: selecting and middle-clicking work inside Termeric without replacing what you copied. |
| Links | Supported | ⌘-click opens a link with the Mac's default app. A local file or folder that would run (an app, a script, an installer) is shown in the Finder instead. |
| Notifications and the bell | Supported | Notifications need Termeric.app and your permission; buttons, replacing, closing, and reports work as on Linux. The bell plays your alert sound. |
| File chooser | Supported | Settings and the context picker open the Mac's own chooser as a sheet. |
| Reduced motion and contrast | Supported | Termeric follows **Reduce motion** and **Increase contrast** in **System Settings → Accessibility → Display**. |
| Quick-access terminal | Partial | `termeric --quick-access` starts it and toggles it, as on Linux. Set `macos_hotkey` under `[quick_access]` for a system-wide key that shows and hides it while it runs: macOS lets a key press bring it forward, but may refuse a request from another program (such as a Shortcuts automation). |
| Kitty file transfer | Partial | As on Linux, with two differences. Paths that go through a symbolic link are refused, as everywhere, and on macOS that includes `/tmp`, `/var`, and `/etc`, which are links into `/private`. Downloads to volumes that can't rename without replacing (exFAT, FAT, and some network volumes) are refused rather than risk replacing a file. |
| Serial terminals | Partial | Rates above 230400 baud depend on the adapter's driver; use the `/dev/cu.*` device. Not tested on hardware. |
| Software rendering | Not supported | Metal has no software renderer, so `--renderer software` can't start on macOS. |
| Dock menu, Linux-only identity | Not supported | The Linux desktop entry's New Window action has no Dock menu counterpart yet; `--app-id`, activation tokens, and the desktop entries are Linux only. |
| Signed and notarized downloads | Planned | Until then, build Termeric.app from source (see the README). |

