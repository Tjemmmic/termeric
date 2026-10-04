# Command line

The `termeric` command opens windows and tabs, runs programs in them, and restores workspaces. Its subcommands control a running Termeric, manage live domains and plugins, and connect coding agents. This page lists every option for opening windows and tabs, and points to the page that documents each set of subcommands. Run `termeric --help` for a summary in your terminal.

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
| `--workspace ID` | Restore one saved [workspace](workspaces-and-sessions.md), by its ID (`workspace-…`), into this window; [`termeric workspace list`](#termeric-workspace-list) prints the IDs. It can't be combined with tab, directory, title, command, profile, or split options in the same window. |
| `--geometry WIDTHxHEIGHT` | Set the window's initial size in physical pixels, from 160x120 to 16384x16384. |
| `--class NAME`, `--app-id NAME` | Set the window's X11 class and Wayland application ID, for window manager rules. The default is `org.termeric.Termeric`. A window with another ID doesn't match Termeric's desktop entry, so on Wayland it may show a generic icon. |
| `--renderer auto\|gpu\|software` | Choose the renderer for the whole process, including windows opened later: `auto` tries the GPU and falls back to software rendering; `gpu` and `software` insist on one. |
| `--quick-access` | Show or hide the quick-access drop-down terminal, starting it if needed. |
| `--separate-process` | Start a Termeric process of its own, even when one is already running. |
| `-h`, `--help` | Print a summary of options. |
| `-V`, `--version` | Print the version. |

Each tab takes one command: `--command`, `--`, `--terminal-profile`, and `--profile` can't be combined in the same tab. When a tab gives no command, it runs your configured shell.

When you give no window or tab options at all and [automatic restore](workspaces-and-sessions.md) is on, Termeric reopens the workspaces that were open when it last exited. `--separate-process` and `--renderer` change only how the process runs, so they still restore.

### Joining the running Termeric

When Termeric is already running on your display, a plain launch opens its window in that process instead of starting another, so every window shares one process and you can move tabs between them. The new window uses the running process's settings, and its programs inherit the running process's environment rather than your launch's. A tab without `--working-directory` opens in the directory you launched from.

Only a launch of one window of shell or command tabs, with at most a directory and a title each, joins. Anything else, such as a workspace, a profile, splits, `--geometry`, `--app-id`, `--renderer`, several windows, or `--quick-access`, starts a process of its own. So does a different or rebuilt `termeric` executable, and a launch whose environment differs from the running process's in any variable that shapes Termeric itself: `RUST_LOG`, `WGPU_BACKEND`, `WGPU_ADAPTER_NAME`, `WGPU_POWER_PREF`, `LIBGL_ALWAYS_SOFTWARE`, `TERMERIC_TERM`, `TERMERIC_TERMINFO_DIR`, `TERMINFO`, `TERMINFO_DIRS`, `HOME`, `XDG_CONFIG_HOME`, or `XDG_DATA_HOME`. For example, `RUST_LOG=debug termeric` starts a process that logs at that level when the running one doesn't.

`--separate-process` always starts a new process, and this setting turns joining off:

```toml
[window]
share_process = false
```

Tabs can't move between separate processes.

### Quick access

`termeric --quick-access` is meant for a desktop keyboard shortcut. The first time, it opens a frameless, always-on-top window along the top of your screen; after that, each run hides or shows that same window, and its shells keep running while hidden. Launch options such as `--working-directory`, `--terminal-profile`, or `--geometry` apply only when the window is first created. On Wayland, the compositor decides placement and focus; match the application ID `org.termeric.Termeric.QuickAccess` in a compositor rule if you need to.

## Subcommands

| Subcommand | Purpose | Commands and options |
| --- | --- | --- |
| `termeric control …` | Inspect and control running Termeric windows from scripts. | [Controlling a running Termeric](control.md) |
| `termeric domain …` | Manage live domains, shells that keep running after Termeric closes. Experimental. | [Live domains](live-domains.md#command-reference) |
| `termeric plugin …` | Install and manage plugins. | [Plugins](plugins.md#from-the-command-line) |
| `termeric workspace list` | List saved workspaces and their IDs. | [Below](#termeric-workspace-list) |
| `termeric mcp` | Serve Termeric's agent tools to a coding agent over MCP. | [Below](#termeric-mcp) |
| `termeric agent-event` | Report an agent's status. Run by agent hooks, not by hand. | [Below](#termeric-agent-event) |

`termeric control --help`, `termeric domain --help`, and `termeric plugin --help` list each set's commands in your terminal. Apart from `termeric domain attach`, subcommands never start a Termeric window of their own; `termeric control` acts on a Termeric that is already running.

### `termeric workspace list`

```text
termeric workspace list [--json]
```

Lists your saved workspaces, most recently saved first: each one's ID, tab count, when it was last saved, and name. Open one with `termeric --workspace ID`. It reads the saved files directly, so it works whether or not Termeric is running. An archive that can't be read is listed last, with the reason, so you can find and delete it. `--json` prints an array of objects with `id`, `name`, `tabs`, `panes`, `updated_at`, and `warning` (`null` unless the archive can't be read).

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
