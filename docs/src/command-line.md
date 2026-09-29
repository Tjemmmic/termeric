# Command line

The `termeric` command opens windows and tabs, runs programs in them, and restores workspaces. Its subcommands control a running Termeric, manage live domains and plugins, and connect coding agents. This page lists every option and subcommand; run `termeric --help`, `termeric control --help`, `termeric domain --help`, or `termeric plugin --help` for the same summary in your terminal.

Termeric checks all options, and that every working directory exists, before it opens anything. A mistake prints an error and exits with a nonzero status without flashing a window.

## Opening windows and tabs

```text
termeric [WINDOW/TAB OPTIONS] [-- COMMAND ARG...]
```

Options apply to the current tab or window. The first window and its first tab exist from the start; `--tab` starts another tab in the current window, and `--new-window` starts another window. Everything after `--` is the command for the current tab and must come last.

```bash
# Two tabs: one in ~/src, one in /tmp
termeric --working-directory ~/src --tab --working-directory /tmp

# A tab in ~/src split into two side-by-side shells
termeric --working-directory ~/src --split vertical

# Run a program with its arguments exactly as given
termeric --title logs -- tail -f /var/log/syslog

# Two windows: a shell in ~/src, and a saved workspace
termeric --working-directory ~/src --new-window --workspace WORKSPACE_ID

# A coding agent with a named profile
termeric --profile claude-code:review
```

| Option | What it does |
| --- | --- |
| `--tab` | Start another tab in the current window. |
| `--new-window` | Start another window. As the first option, it asks for a fresh window and skips restoring your last workspaces. |
| `--split horizontal\|vertical` | Split the current tab after it opens, adding a shell pane below (`horizontal`, or `h`) or to the right (`vertical`, or `v`). Repeat to split again. |
| `--working-directory PATH`, `--cwd PATH` | Start the current tab in `PATH`, which must exist. |
| `--title TITLE` | Set the current tab's title. |
| `--command PROGRAM` | Run `PROGRAM`, with no arguments, instead of your shell. |
| `-- PROGRAM ARG...` | Run `PROGRAM` with every remaining argument passed exactly, even ones starting with `-`. Nothing is interpreted by a shell. |
| `--terminal-profile NAME` | Start the current tab from the terminal profile `NAME` (see [Profiles](profiles.md)). |
| `--profile AGENT:PROFILE` | Start a [coding agent](agents.md) with one of its profiles, such as `codex:review`. |
| `--workspace ID` | Restore one saved [workspace](workspaces-and-sessions.md), by its ID (`workspace-…`), into this window. It can't be combined with tab, directory, title, command, profile, or split options in the same window. |
| `--geometry WIDTHxHEIGHT` | Set the window's initial size in physical pixels, from 160x120 to 16384x16384. |
| `--class NAME`, `--app-id NAME` | Set the window's X11 class and Wayland application ID, for window manager rules. |
| `--renderer auto\|gpu\|software` | Choose the renderer for every window: `auto` tries the GPU and falls back to software rendering; `gpu` and `software` insist on one. |
| `--quick-access` | Show or hide the quick-access drop-down terminal, starting it if needed. |
| `--separate-process` | Start a Termeric process of its own, even when one is already running. |
| `-h`, `--help` | Print a summary of options. |
| `-V`, `--version` | Print the version. |

Each tab takes one command: `--command`, `--`, `--terminal-profile`, and `--profile` can't be combined in the same tab. When a tab gives no command, it runs your configured shell.

When you give no window or tab options at all and [automatic restore](workspaces-and-sessions.md) is on, Termeric reopens the workspaces that were open when it last exited.

### Joining the running Termeric

When Termeric is already running on your display, a plain launch opens its window in that process instead of starting another, so every window shares one process and you can move tabs between them. The new window uses the running process's settings and environment. A tab without `--working-directory` opens in the directory you launched from.

Only a launch of one window of shell or command tabs, with at most a directory and a title each, joins. Anything else, such as a workspace, a profile, splits, `--geometry`, `--app-id`, `--renderer`, several windows, or `--quick-access`, starts a process of its own. So does a different or rebuilt `termeric` executable.

`--separate-process` always starts a new process, and this setting turns joining off:

```toml
[window]
share_process = false
```

Tabs can't move between separate processes.

### Quick access

`termeric --quick-access` is meant for a desktop keyboard shortcut. The first time, it opens a frameless, always-on-top window along the top of your screen; after that, each run hides or shows that same window, and its shells keep running while hidden. Launch options such as `--working-directory`, `--terminal-profile`, or `--geometry` apply only when the window is first created. On Wayland, the compositor decides placement and focus; match the application ID `termeric-quick-access` in a compositor rule if you need to.

## Subcommands

| Subcommand | Purpose |
| --- | --- |
| `termeric control …` | Inspect and control running Termeric windows from scripts. |
| `termeric domain …` | Manage live domains, shells that keep running after Termeric closes. Experimental. |
| `termeric plugin …` | Install and manage plugins. |
| `termeric mcp` | Serve Termeric's agent tools to a coding agent over MCP. |
| `termeric agent-event` | Report an agent's status. Run by agent hooks, not by hand. |

Apart from `termeric domain attach`, subcommands never open a window.

### `termeric control`

These commands act on Termeric processes that are already running. Most take IDs that `termeric control list` prints: `PROCESS` is the Termeric process ID, and `WINDOW`, `TAB`, `PANE`, and `DIVIDER` identify objects within it. Every command accepts `--json` for script-friendly output, and every command that changes something accepts `--request-id ID` so you can retry it safely. See [Controlling a running Termeric](control.md) for a walkthrough.

| Command | What it does |
| --- | --- |
| `list [--json]` | List every running Termeric with its windows, tabs, panes, and dividers. |
| `get-text PROCESS WINDOW TAB PANE [--history-lines N] [--bytes N] [--json]` | Print the text on a pane's screen, plus up to 256 lines of scrollback. |
| `send-text PROCESS WINDOW TAB PANE TEXT [--paste] [--enter] [--request-id ID] [--json]` | Type text into a pane. Requires `security.allow_gui_control_input = true`. |
| `new-window PROCESS SOURCE_WINDOW` | Open a window like `SOURCE_WINDOW`, in its focused tab's directory. |
| `new-tab PROCESS WINDOW` | Open a shell tab in a window. |
| `launch-tab PROCESS WINDOW [OPTIONS] -- PROGRAM [ARG...]` | Run a program in a new tab. |
| `launch-window PROCESS SOURCE_WINDOW [OPTIONS] -- PROGRAM [ARG...]` | Run a program in a new window. |
| `launch-status PROCESS REQUEST_ID [--json]` | Report whether a launched program is still running, or its exit code. |
| `wait-launch PROCESS REQUEST_ID [--timeout SECONDS] [--json]` | Wait for a launched program to exit and return its exit code. |
| `split-pane PROCESS WINDOW TAB PANE horizontal\|vertical` | Split a pane and print the new pane's ID. |
| `focus-window PROCESS WINDOW` | Ask the desktop to focus a window. Not possible on Wayland. |
| `select-tab PROCESS WINDOW TAB` | Select a tab without focusing its window. |
| `focus-pane PROCESS WINDOW TAB PANE` | Select a tab and focus one of its panes. |
| `close-window PROCESS WINDOW` | Close a window. |
| `close-tab PROCESS WINDOW TAB` | Close a tab. |
| `close-pane PROCESS WINDOW TAB PANE` | Close a pane. |
| `move-tab PROCESS SOURCE_WINDOW TAB DESTINATION_WINDOW DESTINATION_INDEX` | Move a tab to a position (counting from 0) in the same or another window. |
| `move-pane PROCESS SOURCE_WINDOW SOURCE_TAB PANE DESTINATION_WINDOW DESTINATION_TAB TARGET_PANE horizontal\|vertical` | Move a pane next to a pane in another tab. |
| `resize-window PROCESS WINDOW WIDTH HEIGHT` | Ask for a window size in physical pixels. |
| `resize-divider PROCESS WINDOW TAB DIVIDER RATIO` | Set a split's ratio in thousandths, from 100 to 900. |

Options for `launch-tab` and `launch-window`, all before the `--`:

| Option | What it does |
| --- | --- |
| `--cwd ABSOLUTE_DIRECTORY` | Start the program in this directory. |
| `--title TITLE` | Set the tab's title. |
| `--env NAME=VALUE` | Set an environment variable for the program. Repeatable. |
| `--env-stdin` | Read environment variables as a JSON object from standard input. |
| `--unset-env NAME` | Remove an inherited environment variable. Repeatable. |
| `--clear-env` | Start from a minimal environment instead of Termeric's. |
| `--wait [--timeout SECONDS]` | Wait for the program to exit, and exit with its exit code. |
| `--background` | `launch-tab` only: open the tab without selecting it. |
| `--request-id ID`, `--json` | As for every command. |

### `termeric domain`

Live domains are experimental; see [Live domains](live-domains.md) for how they work. `DOMAIN` is a domain ID such as `domain-…`, printed when you create one and by `termeric domain list`. Most commands accept `--json`. A command that needs the domain daemon starts it if it isn't running.

| Command | What it does |
| --- | --- |
| `create NAME [LAUNCH OPTIONS] [-- COMMAND...]` | Create a domain with one tab. |
| `list [--json]` | List live domains. |
| `show DOMAIN [--json]` | Describe a domain's tabs, panes, and splits. |
| `workspaces DOMAIN [--json]` | List a domain's workspaces. |
| `panes DOMAIN [--json]` | List a domain's panes. |
| `attach DOMAIN [--geometry WIDTHxHEIGHT] [--app-id NAME]` | Open a window showing the domain. |
| `spawn DOMAIN [LAUNCH OPTIONS] [-- COMMAND...]` | Add a tab. |
| `split DOMAIN PANE horizontal\|vertical [LAUNCH OPTIONS] [-- COMMAND...]` | Split a pane. `h` and `v` also work. |
| `resize-split DOMAIN SPLIT RATIO` | Set a split's ratio in thousandths, from 100 to 900. |
| `rename-tab DOMAIN TAB TITLE` | Set a tab's title; an empty title clears it. |
| `pin-tab DOMAIN TAB`, `unpin-tab DOMAIN TAB` | Pin or unpin a tab. |
| `move-tab DOMAIN TAB INDEX` | Move a tab within its pinned or unpinned group (counting from 0). |
| `swap-pane DOMAIN PANE` | Swap a pane with the next one. |
| `balance-tab DOMAIN TAB` | Reset every split in a tab to 50%. |
| `layout-tab DOMAIN TAB PRESET` | Arrange a tab's panes as `columns`, `rows`, `grid`, `tall`, or `fat`. |
| `move-pane DOMAIN PANE TAB` | Move a pane to another tab. |
| `focus DOMAIN PANE` | Focus a pane. |
| `close-pane DOMAIN PANE` | End a pane's process and remove the pane. |
| `close-tab DOMAIN TAB` | End a tab's processes and remove the tab. |
| `send DOMAIN PANE TEXT [--enter]` | Type up to 64 KiB of text into a pane. |
| `get-text DOMAIN PANE [--bytes N] [--json]` | Print a pane's recent text, up to 1 MiB (64 KiB by default). |
| `upload DOMAIN PANE SOURCE [--name NAME] [--retain\|--resume ID] [--json]` | Copy a file or directory into a pane's working directory, never overwriting. |
| `uploads [--json]` | List interrupted uploads kept for resuming. |
| `cancel-upload UPLOAD_ID [--json]` | Delete a kept upload. |
| `kill DOMAIN` | End all of a domain's processes and remove it. |
| `stop-server` | Stop the domain daemon once no domains remain. |
| `hosts [--json]` | List the SSH hosts saved in your configuration. |
| `protocol-version [--json]` | Print the domain protocol version this `termeric` speaks. |

Launch options for `create`, `spawn`, and `split` are `--working-directory PATH` (or `--cwd PATH`), `--title TITLE`, `--size COLSxROWS`, and `--json`. Put `--host SSH_DESTINATION` or `--host-profile NAME` right after `domain` to run a command on another computer over SSH.

### `termeric plugin`

These commands manage installed plugins; see [Plugins](plugins.md) for details.

| Command | Purpose |
| --- | --- |
| `list` | List installed plugins. |
| `show ID` | Show one plugin's details. |
| `validate DIRECTORY` | Check a plugin directory without installing it. |
| `install DIRECTORY` | Install a plugin from a directory. |
| `update ID` | Update an installed plugin from its source directory. |
| `enable ID`, `disable ID` | Turn a plugin on or off. |
| `configure ID …` | Change a plugin's default profile or settings. |
| `remove ID` | Uninstall a plugin. |

### `termeric mcp`

```text
termeric mcp [--process PID]
```

Serves Termeric's agent tools over the Model Context Protocol on standard input and output, for a coding agent to use. Inside a Termeric terminal it talks to that terminal's Termeric; elsewhere to the only running Termeric, or to the one named by `--process`. See [Agents in terminals](agents.md#letting-an-agent-use-termeric-termeric-mcp).

### `termeric agent-event`

```text
termeric agent-event AGENT EVENT
```

The command that **Set Up Agent Status…** installs as an agent hook. It reports the agent's state to the Termeric terminal it runs in, prints nothing, and always succeeds. You don't run it yourself.
