# Terminal launch profiles

Terminal profiles are reusable, non-plugin terminal launch definitions. A
stable name selects either a process PTY with exact executable/argv/cwd/env or
a native Linux serial transport. Neither form invokes a shell to interpret a
command string.

```toml
[profiles.development]
description = "Rust workspace"
program = "/bin/zsh"
args = ["-l"]
cwd = "/home/user/src/project"
title = "Project shell"
env = { RUST_BACKTRACE = "1" }

[profiles.board]
description = "Embedded development board"
title = "Board console"

[profiles.board.serial]
device = "/dev/serial/by-id/usb-example"
baud = 115200
data_bits = 8
parity = "none"
stop_bits = 1
flow_control = "hardware"
```

Every configured profile appears as **New Tab: NAME** in the command palette.
Configuration reload replaces that inventory atomically. The startup CLI uses
`--terminal-profile NAME`; it remains distinct from `--profile
PLUGIN:PROFILE`, which selects a built-in or third-party plugin profile.

## Launch and precedence

For a process profile, the executable is spawned directly in the PTY with its exact `args`.
There is no command-string concatenation, shell evaluation, or implicit login
flag. Supported interactive Bash, Zsh, and Fish programs still receive the
normal Termeric shell-integration preparation when their argument vector is
interactive. Unambiguous Nushell REPL profiles enable Nushell's native OSC
7/133 settings after user configuration. Nushell scripts, command strings,
language-server/IDE modes, and existing pre-REPL commands remain untouched.
Interactive Elvish profiles expose the generated `termeric-integration` module
without changing argv; the user's rc must import it once. Elvish scripts,
commands, daemon/language-server modes, `-norc`, and other script/`-c` profiles
remain untouched.

An explicit CLI `--working-directory` or `--title` overrides the corresponding
process-profile value for that tab. Other profile fields remain exact. A configured cwd
must be absolute. A missing cwd or unknown profile fails before startup windows
are queued; the palette reports the same failure without replacing the current
tab.

The selected executable, argv, and environment are frozen into the tab. New
panes in that tab inherit the same profile even after configuration reload;
open a new tab to pick up an edited profile. Cwd selection for each split still
follows `[panes].cwd`.

## Serial transport

Serial profiles are currently Linux-only. Termeric opens the configured path
read/write with `O_NOCTTY`, `O_CLOEXEC`, and a nonblocking open followed by
blocking I/O. It validates the already-open descriptor as a terminal character
device, requests kernel `TIOCEXCL` ownership, configures raw termios, and then
relays bounded 16 KiB chunks in both directions without UTF-8 conversion or
shell parsing. Device symlinks such as `/dev/serial/by-id/...` are supported;
the final opened target must still pass descriptor-level character-device and
termios validation.

Supported rates are 50 through 4,000,000 baud from the explicit Linux termios
set documented in the example/config validation; data bits are 5–8, parity is
`none`, `even`, or `odd`, stop bits are 1 or 2, and flow control is `none`,
`software`, or `hardware`. Termeric restores the attributes it observed after
an orderly bridge exit. A process crash or forced kill cannot guarantee driver
state restoration, so device firmware and host policy remain authoritative.

`program`, `args`, `cwd`, and `env` are rejected when `[profiles.NAME.serial]`
is present. `--working-directory` and `--split` are likewise rejected before a
window is created. Interactive/control-plane pane splitting and incoming pane
attachment are disabled for serial tabs so their single-pane workspace contract
cannot turn into an unrestorable mixed layout and Termeric never clones a
single hardware connection implicitly.
Closing or disconnecting ends the bridge; reconnection is an explicit new
profile launch rather than an automatic guess against a potentially replaced
device node. The device path and transport settings are ordinary local config,
not secrets, and are never passed to an AI provider by the serial feature.

## Bounds and environment policy

Configuration accepts at most 64 profiles. Names are 1–64 ASCII letters,
digits, `.`, `_`, or `-`, cannot begin with `.`, and cannot contain `..`.
Each process profile accepts at most 64 arguments and 64
environment variables, with per-value and aggregate byte limits. Titles and
descriptions must be bounded single-line text, programs cannot contain control
line breaks, and environment names use portable identifier syntax.

Termeric owns `PWD`, `TERM`, `COLORTERM`, `TERMINFO`, `TERMINFO_DIRS`,
`TERM_PROGRAM`, and `TERM_PROGRAM_VERSION`; profiles cannot override them.
Environment values are ordinary process configuration and may contain secrets,
so they are neither rendered in the palette nor copied into workspace files.
The same key, value, and identity-variable rules apply to optional `[shell].env`.

## Workspace behavior

Complete workspaces store only each pane's stable shell/profile identity plus
the tab's effective cwd/title/layout metadata. This remains truthful when a
profile pane moves into a shell tab or another profile tab. Restore resolves
each current profile definition and relaunches panes independently; if any
referenced profile was removed, became invalid, or names an unavailable serial
device, that tab is skipped with a
visible warning while healthy workspace tabs continue to restore. Legacy
single-terminal snapshots remain shell-only and do not preserve profile
identity.

## Evidence boundary

Unit and real-PTY tests cover process schema validation and round trips, CLI/plugin
profile disambiguation, pre-window resolution, palette hot replacement, exact
environment/cwd/title delivery, frozen pane launch configuration, workspace
capture/restore identity, and missing-profile failure. Native command-palette
click/keyboard presentation continues to use the shared palette acceptance
lane. Hermetic serial tests cover typed schema validation, exact internal argv,
binary-transparent bidirectional relay, pre-window rejection of process-only
overrides, and a real bridge subprocess connecting two kernel pseudoterminals
in raw mode. Physical UART/USB permissions, exclusive-open behavior,
disconnects, and line settings remain real-device acceptance work; arbitrary
executables and devices remain the user's responsibility.
