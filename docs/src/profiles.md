# Profiles

A terminal profile is a named way to open a tab: a program with its arguments, a starting directory, a tab title, and extra environment variables. Define one for each thing you open often, such as a login shell in a project directory, a Python REPL, or a console on a serial device, and open it from the command palette or the command line instead of typing the same setup each time.

Terminal profiles are separate from agent profiles, which pick a model and options for a coding agent. See [Agents in terminals](agents.md) for those.

## Define a profile

Profiles live in `~/.config/termeric/config.toml`, one `[profiles.NAME]` table each. See [Configuration](configuration.md) for where the file is and how to edit it.

```toml
[profiles.project]
description = "Termeric workspace"
program = "/bin/zsh"
args = ["-l"]
cwd = "/home/me/src/termeric"
title = "termeric"
env = { RUST_BACKTRACE = "1" }

[profiles.python]
description = "Python REPL"
program = "python3"
title = "Python"
```

| Key | Required | Description |
| --- | --- | --- |
| `program` | yes (except for serial profiles) | The program to run: a full path, or a name found on your `PATH` |
| `args` | no | Arguments, one list item each |
| `cwd` | no | Absolute path of the starting directory; without it the tab starts in your home directory |
| `title` | no | Tab title |
| `description` | no | Short text shown after the name in the command palette |
| `env` | no | Extra environment variables for the program |

The name after `profiles.` can use letters, digits, `.`, `_`, and `-`, up to 64 characters. A `title` or `description` is one line of text; control characters, such as tabs, and bidirectional formatting characters are refused, because both are shown in the tab bar, window title, or command palette.

Termeric runs `program` directly with exactly the `args` you list. There's no shell in between, so a string like `"ls -la | less"` is not split or interpreted. To run a shell pipeline, make the program a shell and pass the command, for example `program = "/bin/sh"` and `args = ["-c", "ls -la | less"]`.

Termeric sets `TERM`, `COLORTERM`, `TERM_PROGRAM`, `TERM_PROGRAM_VERSION`, `TERMINFO`, `TERMINFO_DIRS`, `PWD`, and `TERMERIC_TERMINAL_ID` itself, so a profile can't override them. Environment values are not shown in the command palette or copied into saved workspaces, but they are stored in plain text in your configuration file.

Termeric reloads the file when you save it, and the profile list updates at once.

## Open a profile

From the command palette (Ctrl+Shift+P), each profile appears as **New Tab: NAME**, followed by its description if it has one. Typing the profile name finds it. The new tab opens in the current window.

From the command line:

```sh
termeric --terminal-profile project
termeric --terminal-profile project --title "Release work"
termeric --terminal-profile project --tab --terminal-profile python
```

`--working-directory` and `--title` given on the command line replace the profile's `cwd` and `title` for that tab. `--terminal-profile` is for the profiles on this page; `--profile AGENT:PROFILE` starts a coding agent with one of its profiles. See [Command line](command-line.md) for the other options.

If the profile's `cwd` doesn't exist, or the name doesn't match a profile, the tab isn't opened and Termeric shows an error.

## Splits and shell integration

When you split a pane in a profile tab, the new pane runs the same profile. The tab keeps the profile as it was when the tab opened, so after you edit a profile, open a new tab to use the new version. The directory for new panes follows your `[panes].cwd` setting, the same as in shell tabs.

If a profile starts an interactive Bash, Zsh, Fish, Nushell, or Elvish shell, it gets the same [shell integration](shell-integration.md) as your default shell. A profile that runs a script or a command with `-c` is left untouched.

## Profiles in workspaces

A saved [workspace](workspaces-and-sessions.md) records the profile name for each tab and pane, not a copy of its settings. When you restore the workspace, each pane starts from the profile as it is defined now. If you've removed the profile, that tab is skipped and the rest of the workspace still opens.

## Serial profiles

A serial profile opens a tab connected to a serial device, such as a USB-to-serial adapter on a development board, instead of running a program. Serial profiles work on Linux.

```toml
[profiles.board]
description = "Dev board console"
title = "Board"

[profiles.board.serial]
device = "/dev/serial/by-id/usb-FTDI_FT232R_USB_UART_A10K1234-if00-port0"
baud = 115200
data_bits = 8
parity = "none"
stop_bits = 1
flow_control = "none"
```

| Key | Default | Values |
| --- | --- | --- |
| `device` | none (required) | Absolute path to the device, such as `/dev/ttyUSB0` or a stable `/dev/serial/by-id/...` link |
| `baud` | `115200` | A standard rate from 50 to 4000000, such as 9600, 57600, 115200, 230400, 460800, or 921600 |
| `data_bits` | `8` | 5 to 8 |
| `parity` | `"none"` | `"none"`, `"even"`, or `"odd"` |
| `stop_bits` | `1` | 1 or 2 |
| `flow_control` | `"none"` | `"none"`, `"software"` (XON/XOFF), or `"hardware"` (RTS/CTS) |

A serial profile can have a `title` and `description`, but not `program`, `args`, `cwd`, or `env`. Open it the same way as any other profile: **New Tab: board** in the command palette, or `termeric --terminal-profile board`.

Things to know about serial tabs:

- You need permission to open the device. On most distributions that means being in the group that owns it (often `dialout` or `uucp`); check with `ls -l /dev/ttyUSB0`.
- Termeric claims the device exclusively while the tab is connected, so other programs can't open it at the same time.
- Bytes pass through unchanged in both directions. Termeric sets the line to raw mode with your settings and restores the device's previous settings when the tab closes normally.
- Serial tabs can't be split into panes, and `--working-directory` and `--split` are rejected for them.
- If the device is unplugged, the connection ends. Open the profile again to reconnect.
- Shell integration, prompt jumping, and file drops don't apply to serial tabs.
