# Configuration

Termeric reads its settings from one TOML file. You can edit that file by hand, change most settings in the Settings dialog, or mix the two. Every setting has a default, so the file only needs the keys you want to change, and Termeric runs fine with no file at all.

## The configuration file

Termeric looks for `config.toml` in its configuration directory:

```text
~/.config/termeric/config.toml
```

If you set `XDG_CONFIG_HOME`, the file is `$XDG_CONFIG_HOME/termeric/config.toml` instead. The same directory holds your [custom themes](themes.md) (`themes/`) and the third-party plugin registry (`plugins.toml`, see [Plugins](plugins.md)).

To start from a commented file, copy the [example configuration](https://github.com/Tjemmmic/termeric/blob/main/assets/config.example.toml) to that path and edit it. From a source checkout:

```bash
mkdir -p ~/.config/termeric
cp assets/config.example.toml ~/.config/termeric/config.toml
```

Keys are grouped into tables such as `[font]` and `[window]`. A key you leave out keeps its default. Termeric ignores keys it does not recognize in most tables, so check the spelling if a change seems to have no effect. The `[profiles.*]`, `[agents.*]`, `[agent_status]`, `[agent_view]`, and `[supervisor]` tables are strict: an unknown key there is an error.

## Live reload

Termeric checks the file for changes about once a second and applies a saved edit without a restart. A "Config reloaded" notice confirms it. Most settings take effect immediately in every open window, with a few exceptions:

- `[shell]` changes apply to new tabs and panes. Running shells keep going.
- `window.width`, `window.height`, and `window.share_process` apply to windows opened later.
- `renderer.backend` applies the next time Termeric starts.
- Session and splash settings apply the next time Termeric saves, restores, or shows the splash screen.

You can also reload on demand with **Reload Configuration** in the command palette (Ctrl+Shift+P). An active custom theme file and a background image are watched the same way.

## When the file has an error

Termeric never lets a mistake in `config.toml` stop it from starting, and never overwrites a file it could not read.

- **At startup**, an unreadable or invalid file makes Termeric start with the default settings and show an error notice explaining the problem.
- **On reload**, an invalid edit is rejected with a warning notice, and the settings you were already using stay in effect. Fix the file and save it again.
- **Saving** from Settings (or anything else that saves) refuses to replace a file that has an error, so your settings are never lost to a typo. The notice tells you to fix the file, or to use **Reset Configuration to Defaults**.
- **Reset Configuration to Defaults** in the command palette replaces the file with one that sets nothing, so every setting returns to its default. Termeric first keeps the old file, whether or not it had an error, beside it as `config.toml.<seconds>.bak`, where `<seconds>` is the current Unix time, and the notice names the copy.

Values are also checked against the ranges in the reference below; a value outside its range is an error, not a silent clamp.

## The Settings dialog

Open Settings with Ctrl+, (Ctrl+comma), the Settings button at the right end of the top bar, or **Open Settings** in the command palette. The sidebar has pages for Appearance, Font, Terminal, Keyboard, Agents, Sessions, Accessibility, Plugins, and Experimental. Type to filter every page at once.

- Changes apply immediately to the window you are in, so you can try a setting before keeping it.
- The footer shows "Unsaved changes" when the live settings differ from the file. Press Ctrl+S or click **Save** to write them. Saving updates every other open window within a second.
- If you close Settings without saving, the changes stay in effect in that window until the file is next reloaded, then they are gone.
- Ctrl+R inside Settings resets every setting to its default and saves immediately.

Saving changes only the settings that differ from the file. Everything else in `config.toml` stays as you wrote it: comments, blank lines, the order and form of keys, and settings you never touched. A changed value keeps the comment after it, a new setting goes into its table (or a new table at the end of the file), and a setting you cleared is removed. An edit you made to the file in another program since Termeric last read it is kept too. The file is replaced in one step, so it is never left half-written; if `config.toml` is a symbolic link, the file it points to is replaced and the link stays.

Choosing a theme with **Change Theme…**, adding or deleting a saved command in the Commands panel, and **Cycle New Pane Directory** also save the file, together with any other unsaved changes from Settings.

The font size you save is the current size of that window, so a zoom you made with Ctrl+= or Ctrl+- becomes the saved size.

Some settings are not in the Settings dialog, so you change them in the file:

- `window.width`, `window.height`, `window.share_process`, and `window.background_image.path` (Settings shows the image path but can't change it);
- `theme.path`;
- everything under `[shell]` (Settings shows the program and its arguments but can't change them);
- `keyboard.xterm_type`, everything under `[copy_mode]`, and `renderer.backend`;
- `session.enabled`;
- everything under `[security]`;
- launch profiles, `[[keybindings]]`, and SSH host names;
- custom agents, every built-in agent key except turning the agent on or off, and `supervisor.max_sends_per_minute`.

`panes.cwd` is also set by **Cycle New Pane Directory** in the command palette. A few Settings controls offer a narrower range, or a list of common values, than the file accepts; the reference below gives the full ranges.

## Reference

Each table lists a key, its default, and what it does. Ranges are inclusive.

### [font]

See [Fonts and text](fonts-and-text.md) for how these settings work together.

| Setting | Default | Description |
| --- | --- | --- |
| `family` | `"monospace"` | Primary font family. `monospace` is the system's default monospace font. |
| `size` | `12.0` | Font size in points, 8 to 72. |
| `weight` | `400` | OpenType weight for normal text, 1 to 1000. |
| `bold_weight` | `700` | OpenType weight for bold text, 1 to 1000, at least `weight`. |
| `line_height` | `1.2` | Line height as a multiple of the font size, 0.8 to 3.0. |
| `fallback` | `[]` | Up to 16 extra families tried in order for characters the primary font lacks. No duplicates. |
| `ligatures` | `true` | Turn programming ligatures on or off (`liga`, `clig`, `calt`, `dlig`). |
| `bold_is_bright` | `false` | Show bold text in ANSI colors 0–7 using the bright colors 8–15. |
| `features` | `{}` | Up to 32 OpenType feature values, such as `ss01 = 1`. Tags are four lowercase letters or digits; `kern` is not allowed. |
| `range_overrides` | `[]` | Up to 64 Unicode ranges that prefer a specific family. Each entry has `start`, `end`, and `family`. Ranges may not overlap or touch U+D800–U+DFFF. |

```toml
[font]
family = "JetBrains Mono"
size = 13.0
fallback = ["Noto Sans Mono", "Noto Color Emoji"]

[font.features]
ss01 = 1

[[font.range_overrides]]
start = 0xE000
end = 0xF8FF
family = "Symbols Nerd Font Mono"
```

### [window]

| Setting | Default | Description |
| --- | --- | --- |
| `width` | `800` | Initial window width in pixels, 320 to 16384. |
| `height` | `600` | Initial window height in pixels, 200 to 16384. |
| `opacity` | `1.0` | Window opacity, 0.1 to 1.0. Needs a compositor that supports transparency. |
| `padding` | `8` | Space around the terminal content in pixels, 0 to 256. |
| `decorations` | `true` | Use the desktop's title bar and borders. |
| `level` | `"normal"` | Stacking request: `normal`, `always_on_top`, or `always_on_bottom`. The window manager decides whether to honor it. |
| `tab_bar_position` | `"top"` | Put tabs in the top bar (`top`) or in a vertical rail on the left (`left`). |
| `share_process` | `true` | Open new windows inside an already running Termeric on the same display, so tabs can move between windows. |

### [window.background_image]

An optional picture behind the terminal content. See [Themes and backgrounds](themes.md#background-images).

| Setting | Default | Description |
| --- | --- | --- |
| `path` | unset | PNG or JPEG file. A relative path is resolved beside `config.toml`. Leave unset for no image. |
| `opacity` | `0.2` | Image opacity over the theme background, 0.0 to 1.0. |
| `fit` | `"cover"` | How the image fills the area: `cover`, `contain`, `stretch`, or `center`. |

### [theme]

| Setting | Default | Description |
| --- | --- | --- |
| `name` | `"turmeric"` | A built-in theme name, or the name of a file in `~/.config/termeric/themes/` without `.toml`. |
| `path` | unset | Path to a theme file to use instead of `name`. Use an absolute path. |

### [cursor]

| Setting | Default | Description |
| --- | --- | --- |
| `style` | `"block"` | Cursor shape: `block`, `underline`, or `beam`. Programs can change the shape while they run. |
| `blink` | `true` | Blink the cursor. |
| `blink_rate` | `530` | How long the cursor stays on, and then off, in milliseconds, 100 to 5000. |

### [scrollback]

| Setting | Default | Description |
| --- | --- | --- |
| `lines` | `10000` | Lines of history kept per terminal, 100 to 1,000,000. |
| `max_memory_mib` | `128` | Memory limit for history per terminal or pane, in MiB, 8 to 4096. The older lines are dropped first when either limit is reached. |
| `scroll_speed` | `6.0` | Lines moved per mouse-wheel step, 0.1 to 100. |

### [bell]

| Setting | Default | Description |
| --- | --- | --- |
| `visual` | `true` | Flash the terminal when a program rings the bell. |
| `audio` | `false` | Play a sound when a program rings the bell, through `paplay`. One sound plays at a time. If the player fails, or is still running after 5 seconds and has to be stopped, Termeric shows a warning. |
| `visual_duration_ms` | `100` | Length of the flash in milliseconds, 10 to 5000. |

### [shell]

| Setting | Default | Description |
| --- | --- | --- |
| `program` | your login shell | Program to run in new tabs and panes. |
| `args` | `[]` | Arguments passed to `program`. |
| `env` | `{}` | Extra environment variables for new shells. `TERM`, `COLORTERM`, `TERMINFO`, `TERMINFO_DIRS`, `TERM_PROGRAM`, `TERM_PROGRAM_VERSION`, `TERMERIC_TERMINAL_ID`, and `PWD` are reserved. |

```toml
[shell]
program = "/usr/bin/zsh"
args = ["-l"]
env = { EDITOR = "nvim" }
```

### [panes]

| Setting | Default | Description |
| --- | --- | --- |
| `cwd` | `"focused"` | Working directory for a new split: the focused pane's (`focused`), the tab's first pane's (`tab`), or the directory Termeric was started in (`process`). |

### [keyboard]

| Setting | Default | Description |
| --- | --- | --- |
| `xterm_type` | `"normal"` | xterm keyboard type: `normal`, `legacy`, or `vt220`. Programs can switch it while they run; changing this setting applies it to every open terminal. |

### [copy_mode]

See [Search, copy mode, and links](search-and-copy-mode.md).

| Setting | Default | Description |
| --- | --- | --- |
| `bindings` | `"vi"` | Keys used inside copy mode: `vi` or `emacs`. |
| `patterns` | URL, path, and hash patterns | Up to 32 `{ name, regex }` entries that quick select highlights. Setting this list replaces the built-in patterns. |

```toml
[[copy_mode.patterns]]
name = "ticket"
regex = "PROJ-[0-9]+"
```

### [accessibility]

See [Accessibility](accessibility.md).

| Setting | Default | Description |
| --- | --- | --- |
| `reduce_motion` | `"system"` | Remove non-essential animations: `system` follows the desktop setting, or use `on` or `off`. |
| `high_contrast` | `"system"` | Raise the contrast of Termeric's own interface (not terminal colors): `system`, `on`, or `off`. |

### [renderer]

| Setting | Default | Description |
| --- | --- | --- |
| `backend` | `"auto"` | `auto` prefers the GPU and falls back to software rendering; `gpu` requires a GPU; `software` always renders on the CPU. Applies at the next start. |

### [session]

See [Workspaces and sessions](workspaces-and-sessions.md).

| Setting | Default | Description |
| --- | --- | --- |
| `enabled` | `true` | `false` turns off all automatic saving and restoring, as if `auto_save` and `auto_restore` were both off. **Save Workspace** and opening a workspace by hand still work. |
| `auto_restore` | `false` | Reopen the windows from your last session when Termeric starts with no arguments. |
| `auto_save` | `true` | Save a window's workspace when it closes, if it is the last window open or belongs to a saved workspace, and save a [tab snapshot](workspaces-and-sessions.md#tab-snapshots) when a tab with output closes. |
| `save_scrollback` | `true` | Include terminal history in workspaces and tab snapshots. |

### [splash]

| Setting | Default | Description |
| --- | --- | --- |
| `mode` | `"firstboot"` | When to show the splash screen: `never`, `firstboot` (only the first launch ever), `newwindow`, or `newtab`. |

### [security]

These settings control what programs running in the terminal are allowed to do.

| Setting | Default | Description |
| --- | --- | --- |
| `allow_screen_checksum_reports` | `false` | Answer DECRQCRA checksum requests. Off because a program could use them to read the screen one cell at a time. |
| `allow_kitty_file_transfers` | `false` | Let programs display images by naming a local file (the Kitty graphics `t=f` medium). Other ways of sending images do not need this. |
| `allow_gui_control_input` | `false` | Let `termeric` control commands and granted plugins type into terminals. See [Controlling a running Termeric](control.md). |

### [security.osc52]

OSC 52 lets programs, including ones on remote hosts, use your clipboard.

| Setting | Default | Description |
| --- | --- | --- |
| `allow_read` | `false` | Let programs read your clipboard. A denied read gets an empty answer. |
| `allow_write` | `true` | Let programs copy text to your clipboard. |
| `max_bytes` | `1048576` | Largest accepted copy, in bytes, 1 to 1048576. |

### [profiles.NAME]

Reusable launches that appear in the command palette and work with `termeric --terminal-profile NAME`. Names use letters, digits, `-`, `_`, and `.`, up to 64 characters, and can't start with `.` or contain `..`. See [Profiles](profiles.md).

| Setting | Default | Description |
| --- | --- | --- |
| `description` | unset | One-line description shown in the palette. |
| `program` | required | Program to run (not run through a shell). |
| `args` | `[]` | Arguments. |
| `cwd` | unset | Absolute starting directory. |
| `title` | unset | Tab title. |
| `env` | `{}` | Extra environment variables, with the same reserved names as `[shell]`. |

A serial profile replaces `program`, `args`, `cwd`, and `env` with a `[profiles.NAME.serial]` table:

| Setting | Default | Description |
| --- | --- | --- |
| `device` | required | Absolute path to the serial device, such as `/dev/ttyUSB0`. |
| `baud` | `115200` | Baud rate, from the standard rates between 50 and 4000000. |
| `data_bits` | `8` | 5 to 8. |
| `parity` | `"none"` | `none`, `even`, or `odd`. |
| `stop_bits` | `1` | 1 or 2. |
| `flow_control` | `"none"` | `none`, `software`, or `hardware`. |

### [[commands]]

Saved commands for the Commands panel and command palette. You normally manage these in the Commands panel.

| Setting | Default | Description |
| --- | --- | --- |
| `name` | required | Name shown in the panel and palette. |
| `command` | required | Command run in the focused terminal: Termeric clears the current input line, types this text, and presses Enter. |
| `alias` | unset | Short name to find it in the palette. |

```toml
[[commands]]
name = "SSH Production"
command = "ssh admin@prod.example.com"
alias = "prod"
```

### [[shortcuts]]

Changes to built-in keyboard shortcuts. The shortcut editor in Settings (Keyboard page) writes these for you. See [Keyboard and mouse](keyboard-and-mouse.md) for the list of actions.

| Setting | Default | Description |
| --- | --- | --- |
| `action` | required | Action ID, such as `new_tab` or `toggle_settings`. |
| `key` | unset | Key name. Leave it out to unbind the action. |
| `ctrl`, `alt`, `shift`, `super` | `false` | Modifiers that must be held. |

```toml
[[shortcuts]]
action = "new_tab"
key = "t"
ctrl = true
```

### [[keybindings]]

Extra keys that type text, run a command, or call a plugin command. See [Keyboard and mouse](keyboard-and-mouse.md#keys-that-type-text-or-run-commands).

| Setting | Default | Description |
| --- | --- | --- |
| `key` | required | A character such as `k`, or a named key: `Enter`, `Tab`, `Space`, `Backspace`, `Escape`, `Delete`, `Insert`, `Home`, `End`, `PageUp`, `PageDown`, `Up`, `Down`, `Left`, `Right`, `F1`–`F12`. |
| `modifiers` | none | `{ ctrl, alt, shift, super }`; the held modifiers must match exactly. |
| `action` | required | `{ type = "send_text", text = "…" }` sends the text exactly as written (add `\r` to press Enter); `{ type = "run_command", command = "…" }` clears the current input line, types the command, and presses Enter; `{ type = "plugin_command", command_id = "…" }` runs a plugin's command. |

```toml
[[keybindings]]
key = "F5"
modifiers = { ctrl = true }
action = { type = "send_text", text = "make\r" }
```

### [domains.ssh_hosts.NAME]

> **Experimental.** Live domains may change between releases. See [Live domains](live-domains.md).

Friendly names for remote hosts, at most 64. OpenSSH settings stay in `~/.ssh/config`.

| Setting | Default | Description |
| --- | --- | --- |
| `destination` | required | An OpenSSH host alias or `user@host`. |
| `description` | unset | One-line description, up to 256 characters. |

### [agents.ID]

> **Experimental.** Coding-agent support may change between releases. See [Agents in terminals](agents.md).

Settings for the built-in agents (`claude-code`, `codex`, `gemini`, `opencode`), or a custom agent under any other ID. For a built-in agent every key is an override; [Agents in terminals](agents.md) lists each built-in agent's program and arguments. IDs are 1 to 64 letters, digits, `-`, and `_`.

| Setting | Default | Description |
| --- | --- | --- |
| `enabled` | `true` | `false` hides the agent from pickers and detection. |
| `name` | built-in name, or the ID | Display name. |
| `program` | built-in program, or the ID | Executable to launch. Its file name also identifies the agent, unless it is a launcher such as `npx`, `docker`, `node`, or `sh`; then add the agent's own name to `detect`. |
| `detect` | `[]` | Extra program names that identify the agent, such as a wrapper script. |
| `args` | `[]` | Arguments passed on every launch in a terminal. |
| `resume_args` | built-in agent's own; none for a custom agent | Arguments that continue the most recent conversation. Without them the agent can't resume. |
| `new_conversation` | built-in agent's own; none for a custom agent | Text typed into the agent to start a new conversation. An empty string removes the command. |
| `inherit_cwd` | `true` | Start in the focused pane's directory. |
| `env` | `{}` | Extra environment variables on every launch. |
| `profiles.NAME` | none | Named launch variants, each with `description`, `args`, and `env`. A profile's arguments follow `args`, and resume arguments come last. |
| `default_profile` | unset | Profile used when a launch names none. Must exist in `profiles`. |

```toml
# Offer Claude Code a read-only planning variant.
[agents.claude-code.profiles.review]
description = "Read-only review"
args = ["--permission-mode", "plan"]

# An agent Termeric doesn't ship.
[agents.aider]
name = "Aider"
resume_args = ["--restore-chat-history"]
new_conversation = "/clear"
```

### [agent_status]

See [Agent status](agents.md#agent-status).

| Setting | Default | Description |
| --- | --- | --- |
| `notify` | `true` | Send a desktop notification when an agent you are not looking at needs you, finishes, or fails. |

### [agent_view]

> **Experimental.** See [Agent view](agent-view.md).

| Setting | Default | Description |
| --- | --- | --- |
| `open_agents_in` | `"terminal"` | Where **New Agent…** opens an agent: its own interface in a terminal tab (`terminal`), a new tab with the agent view beside the terminal (`split`), or a new tab filled by the agent view (`full`). |
| `allow_bypass_permissions` | `false` | Offer each agent's mode that runs every action without asking. |

### [supervisor]

> **Experimental.** See [The supervisor](agents.md#the-supervisor).

| Setting | Default | Description |
| --- | --- | --- |
| `agent` | `"claude-code"` | The agent that supervises: `claude-code` or `codex`. |
| `mode` | `"assist"` | `watch` (reports only), `assist` (asks you before each message), or `auto` (sends on its own within the limit). |
| `max_sends_per_minute` | `6` | Messages the supervisor may send per minute across all agents, 1 to 60. |

### [harvest]

> **Experimental.** Harvest shows your command history with shell integration and may change between releases.

| Setting | Default | Description |
| --- | --- | --- |
| `enabled` | `false` | Turn on Harvest. |
| `show_indicators` | `true` | Show growth indicators while commands run. |
| `max_history` | `100` | Commands kept, 1 to 10000. |
