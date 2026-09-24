# Coding agents

Termeric runs AI coding agents, the command-line tools that edit a
repository from a terminal, in ordinary tabs and panes. It knows four of them
out of the box, and any other can be added with a few lines of configuration:

| Agent | Program | Resume | New conversation |
|---|---|---|---|
| Claude Code (`claude-code`) | `claude` | `--continue` | `/clear` |
| Codex (`codex`) | `codex` | `resume --last` | `/new` |
| Gemini CLI (`gemini`) | `gemini` | `--resume latest` | `/clear` |
| OpenCode (`opencode`) | `opencode` | `--continue` | `/new` |

Only agents whose program is installed on `PATH` are offered. Each one can be
turned off on the **Agents** page of Settings.

## Starting an agent

**New Agent…** (Ctrl+Shift+I by default) opens a picker of installed agents
and their profiles, and starts the choice in a new tab. The command palette
also offers:

- **Resume Agent Conversation…**, which starts the agent with its resume
  arguments. Agents without resume support are left out.
- **Split Right with Agent…** and **Split Down with Agent…**, which start the
  agent in a new pane beside the focused terminal. Pane placement follows the
  [explicit plugin-pane rules](plugin-panes.md#placement-and-safety).

A new agent starts in the focused terminal's working directory unless its
`inherit_cwd` is off. It receives the terminal size as `COLUMNS` and `LINES`.
`termeric --profile AGENT:PROFILE` starts one from the
[command line](command-line.md).

## Agents in any pane

Termeric recognizes an agent however it was started, including when you type
`claude` or `codex` at a shell prompt. About once a second it reads the
program in each terminal's foreground process group. A program counts as an
agent when its name matches one of the agent's `detect` names. For Node, Bun,
Deno, Python, and shell scripts, the script's name is used instead of the
interpreter's. Only an installed program counts: a script without an
extension, or a package's entry point under `node_modules`. Your own
`claude.py` is not taken for Claude Code. An agent Termeric started is known
by what it started, whatever its program is called. On platforms without
`/proc`, only agents Termeric started itself are recognized.

While an agent terminal is focused, the palette adds commands for it. They are
hidden everywhere else:

| Command | What it does |
|---|---|
| **Start New Agent Conversation** | Types the agent's own new-conversation command, such as `/clear`, then presses Enter a moment later so the agent submits it. |
| **Restart Agent** | Starts the same agent, profile, and directory again, then closes the old process. Only offered for agents Termeric started. |
| **Send Context to Agent…** | Gathers the chosen context and pastes it into the agent without pressing Enter, so you can add to it first. Bracketed paste is used when the agent enabled it. |
| **Review Agent Diff** / **Copy Agent Diff** | Reviews or copies the latest unified diff the agent printed. Offered only when the agent printed one. See [diff capture](diff-capture.md). |
| **Export Agent Session** | Exports the terminal's transcript, or its scrollback when it is not recording. See [transcript export](transcript-export.md). |
| **Toggle Agent Transcript** | Starts or stops recording this terminal's raw transcript. |

Harvest **Explain** and **Send Context to Agent…** go to the focused agent. If
no agent is focused, they go to the only running agent. If several agents are
running, focus the one you mean. Delivery checks that the same agent still
owns the terminal. If it has exited or the terminal is read-only, you get a
notice and nothing is typed.

Terminal output context comes from a shell pane beside the agent in its tab,
or else from the first shell tab. It never comes from an agent's screen.

What you type or paste into an agent reaches it directly. Input that software
sends on your behalf, through the [GUI control protocol](gui-control.md) or a
[runtime plugin](plugins.md), still asks for confirmation when it would run a
guarded command.

## Reviewing an agent's changes

When Termeric notices an agent in a terminal inside a Git working tree, it
records a snapshot of that working tree. The snapshot includes your own
uncommitted work and new files that are not ignored. **Review Agent Changes…**
then lists every file that differs from the snapshot, with lines added and
removed, and marks files whose current content is already staged. Choosing a
file shows its diff, with these controls:

- **Stage** (S) adds the file to the index as it is now, including a deletion.
- **Revert** (R) puts the file back as it was when the agent started, after
  you confirm. A file the agent created is removed. Only the working tree
  changes: staged content and commits stay as they are. Reverting writes only
  inside the repository, never through a linked directory, and applies the
  repository's line-ending settings. A path that became a directory, or the
  other way round, is refused rather than half changed.
- **Copy** (C) copies the file's diff.

**Stage All Agent Changes** stages every changed file at once. Both commands
stay available after the agent exits, until another agent starts in that
terminal. A snapshot belongs to one run of one agent: when the agent restarts,
or another starts, a new snapshot is taken, and a diff still open from the
earlier run can no longer be staged or reverted.

The snapshot is taken from an in-memory copy of the index. Taking it never
changes your index or files; it adds objects to the repository, which
`git gc` removes once nothing uses them. Untracked files over 16 MiB are left
out, and are neither listed nor reverted. Nested repositories are skipped.
Working trees with more than 50,000 files cannot be snapshotted. Changes
you make yourself while the agent runs are listed too.

## Agent status

With agent status set up, Termeric knows what each agent is doing:

| State | Shown as | Notification |
|---|---|---|
| Working | A hollow ring before the tab title | — |
| Needs you (a permission prompt or question) | A yellow dot, until the agent moves on | "Codex needs you", with what it asks |
| Done | A green dot, until you look at the terminal | "Codex is done", with the answer's first line |
| Failed | A red dot, until you look at the terminal | "Claude Code stopped with an error" |

When you answer a waiting agent, by typing or pasting into its terminal, the
yellow dot gives way to the working ring. When you interrupt a turn, the ring
goes away: Codex reports the interruption, and Claude Code, which does not,
reports once it has sat at its prompt for a while. Compacting a long
conversation is not reported as a new one.

Notifications appear only for terminals you are not looking at, and clicking
one shows that terminal. Turn them off with **Notify When Agents Need You** on
the Settings **Agents** page, or `[agent_status] notify = false`. The tab
announces the same state to screen readers.

**Go to Agent…** lists every agent in the window, the ones that need you
first, and shows the chosen terminal. **Copy Last Agent Response** copies the
focused agent's most recent answer. The answer comes from the agent's report,
or else from the end of the agent's own conversation log. Both commands appear
only when they have something to show.

### Setting it up

Agents report their state through hooks, commands they run at points in a
conversation. **Set Up Agent Status…**, in the palette or on the Settings
**Agents** page, adds Termeric's hooks for Claude Code and Codex:

| Agent | File | Hooks |
|---|---|---|
| Claude Code | `~/.claude/settings.json` (or `$CLAUDE_CONFIG_DIR`) | `SessionStart`, `UserPromptSubmit`, `PostToolUse`, `PostToolUseFailure`, `Notification` (permission prompts, questions, and idle at the prompt), `Stop`, `StopFailure`, `SessionEnd` |
| Codex | `~/.codex/hooks.json` (or `$CODEX_HOME`) | `SessionStart`, `UserPromptSubmit`, `PostToolUse`, `PermissionRequest`, `Stop`, `Interrupt`, `SessionEnd` |

Setup lists the files it will change and waits for you to confirm. Every other
setting and hook in those files keeps its exact text and order, and each file's
previous version is kept beside it with `.termeric-backup` appended. A file
linked from elsewhere, for example by a dotfile manager, is updated where it
really lives. If a file changes while the dialog is open, it is left alone.
Codex asks you to trust new hooks the next time it starts. Termeric does not
approve them on your behalf.

Setup looks for each file where the agent will: through `CLAUDE_CONFIG_DIR` or
`CODEX_HOME` as set in the agent's [`env`](#configuration), else as set in
Termeric's own environment, else in your home directory. A variable exported
only in your shell's startup file is not seen by a Termeric started from your
desktop, so set it in `[agents.<id>].env` too.

Each hook runs `termeric agent-event AGENT EVENT`. That command does nothing
unless it runs inside a Termeric terminal. Inside one, it reports to the
Termeric window that owns the terminal, which it finds through the
`TERMERIC_TERMINAL_ID` variable every Termeric terminal sets for its
processes. It never prints, always succeeds, and gives up after two seconds,
so a hook cannot slow or fail an agent's turn. Reports travel over the same
private, user-only socket as the [GUI control protocol](gui-control.md).

**Remove Agent Status Hooks…** takes Termeric's hooks out again. The setup
records the path of the Termeric program that ran it, so run setup again after
moving Termeric, and after an update that listens to more events. The
Settings page shows **Needs updating** when that is due, and Termeric says so
once when it starts.

## The supervisor

A supervisor is an agent that watches the other agents in its window and
answers them for you, as far as you allow. **Start Supervisor** opens the
configured agent, Claude Code or Codex, in a tab of its own. It starts with the
[MCP tools](#letting-an-agent-direct-the-others) already connected, its
instructions, and your rules. It then summarizes what each agent is doing.

Whenever another agent in the window needs input, finishes, or fails, Termeric
tells the supervisor with a message that begins `Termeric:`. While the
supervisor is busy, messages wait and arrive together once it finishes, if its
[status hooks](#agent-status) are set up. The supervisor reads terminals and
sends prompts through the same tools as any MCP client. Termeric then decides
each message it tries to send, according to the mode:

| Mode | What happens to a message |
|---|---|
| Watch | Refused. The supervisor reports and suggests instead. |
| Assist (default) | Shown to you with **Send** and **Don't send**. The supervisor is told the outcome. |
| Auto | Sent at once, within the limits below. |

In every mode Termeric enforces these limits, whatever the supervisor's
instructions say:

- It sends only to agents in its own window, never to itself or to a terminal
  without that agent.
- It sends at most `max_sends_per_minute` messages (6 by default).
- It cannot send the same message to the same agent twice within 30 seconds.

**Take Over from Supervisor** puts it in watch mode until **Hand Back to
Supervisor**, and tells it so. **Supervisor Mode…** changes the mode,
including for a running supervisor. **Supervisor Activity** lists what it was
told, what it asked, and what was sent or refused. **Stop Supervisor** closes
its terminal.

The rules are plain language in `supervisor.md`, beside `config.toml`.
**Edit Supervisor Rules** opens it in `$VISUAL` or `$EDITOR`, creating it with
examples the first time. A supervisor reads the rules when it starts, so
restart it after editing. The supervisor's sends do not need
`security.allow_gui_control_input`, because starting it is the permission. They
are not checked for destructive commands, because each agent still applies its
own approvals.

```toml
[supervisor]
agent = "claude-code"      # or "codex"
mode = "assist"            # "watch", "assist", or "auto"
max_sends_per_minute = 6   # 1 to 60
```

The supervisor's MCP server carries a random token that identifies it to
Termeric. Other MCP clients use the ordinary rules above and never gain its
authority.

## Letting an agent direct the others

`termeric mcp` is a [Model Context Protocol](https://modelcontextprotocol.io)
server on standard input and output. It gives an agent three tools:

| Tool | What it does |
|---|---|
| `list_agents` | Every agent in Termeric, with its window, tab, and pane, state, what it is waiting for, and last answer |
| `read_terminal` | A pane's screen and up to 256 lines of scrollback |
| `send_to_agent` | Pastes a prompt into one agent and submits it, if that pane still runs the named agent |

Register it once with each agent that should use it:

```sh
claude mcp add termeric -- termeric mcp
codex mcp add termeric -- termeric mcp
```

Started inside a Termeric terminal, the server talks to that terminal's
Termeric process. Elsewhere it uses the only running Termeric, or the one named
with `--process PID`. Reading needs nothing more than
[`termeric control get-text`](gui-control.md) already allows. Sending is
refused unless `security.allow_gui_control_input = true`, because it lets one
program type into another agent. Prompts are pasted, then submitted a moment
later, the same way as **Start New Agent Conversation**. They are not checked
for destructive commands, because they are requests to an agent, which still
applies its own approvals.

## Configuration

`[agents.<id>]` entries in `config.toml` change a built-in agent. An entry with
any other id defines a new agent. Every key is optional:

| Key | Meaning |
|---|---|
| `enabled` | `false` hides the agent from pickers and detection. |
| `name` | Display name. A custom agent defaults to its id. |
| `program` | Executable to launch. Its file name is also recognized, unless it is a launcher such as `npx`, `docker`, `node`, or `sh`; then list the agent's own name in `detect`. A custom agent defaults to its id. |
| `detect` | Extra program names to recognize, for example a wrapper script. |
| `args` | Arguments passed on every launch. |
| `resume_args` | Arguments that continue the latest conversation. Without them, the agent cannot resume. |
| `new_conversation` | Text typed to start over. An empty string removes the command. |
| `inherit_cwd` | Start in the focused terminal's directory. Defaults to `true`. |
| `env` | Environment added on every launch. |
| `profiles.<name>` | A named launch variant with `description`, `args`, and `env`. |
| `default_profile` | The profile used when a launch names none. It must exist. |

```toml
# Always let Claude Code act without asking, and offer a read-only variant.
[agents.claude-code]
args = ["--dangerously-skip-permissions"]

[agents.claude-code.profiles.review]
description = "Read-only review"
args = ["--permission-mode", "plan"]

# An agent Termeric does not ship.
[agents.aider]
name = "Aider"
resume_args = ["--restore-chat-history"]
new_conversation = "/clear"
```

Profile arguments follow the agent's `args`, and resume arguments come last.
Agent ids use letters, digits, `-`, and `_`. Environment keys that Termeric
owns, such as `PWD`, are rejected.

### Earlier `[plugins.claude_code]` and `[plugins.codex]` sections

Earlier releases configured Claude Code and Codex under `[plugins.*]`, with a
fixed model list and friendly approval names. Those sections are still read.
Each is converted to the matching `[agents.*]` entry, keeping only what
differed from the defaults, and is dropped the next time Settings saves.

| Earlier setting | Becomes |
|---|---|
| `binary` | `program` |
| `model = "X"` | `args = ["--model", "X"]` |
| `args`, `inherit_cwd`, `default_profile` | Unchanged |
| Profile `extra_args` | Profile `args` |
| Claude `approval_mode = "auto-accept"` | `--dangerously-skip-permissions` |
| Codex `approval_mode = "auto-edit"` / `"full-auto"` | `--ask-for-approval on-request` / `never` |
| Codex `sandbox_mode = "read-only"` / `"full-access"` | `--sandbox read-only` / `danger-full-access` |

A profile's approval and sandbox settings replaced the section's for that
profile, and still do. With profiles, each converted profile carries its own
approval and sandbox arguments, taking the section's for what it did not set.
The section's own become a profile named `default`, used when no other
default was chosen. A `default_profile` naming a missing profile is dropped.

The earlier defaults, `suggest` and `workspace-write`, are dropped, so a
converted Codex launch follows your own Codex configuration. An existing
`[agents.<id>]` entry always wins over a converted one.

## Workspaces and restarts

A saved [workspace](workspaces.md) records each agent terminal's agent,
profile, and directory. Restoring it starts a fresh agent process. The
conversation is not resumed, so use **Resume Agent Conversation…** for that.
An agent pane keeps its identity, transcript, and diff history when it is
split around, collapsed, or moved to another tab or window.

## Limits

- The supervisor has been exercised with fake agents only. Real Claude Code
  and Codex supervisors, and their MCP clients, have not been run.
- Status comes only from agents with hooks set up: Claude Code and Codex.
  Other agents are recognized, but their state is not known.
- Agents in [live-domain](live-domains.md) terminals cannot report status.
  Their processes belong to the domain daemon rather than to a window.
- A tmux or screen server keeps the environment of the terminal it was started
  in. Agents inside it report to that Termeric terminal, whichever terminal
  shows them. Start tmux outside Termeric, or run
  `tmux set-environment -gu TERMERIC_TERMINAL_ID` before starting agents in it.
- Diffs are read from the terminal output, so an agent that never prints a
  unified diff has nothing to review.
- Automated tests use fake agent executables in real PTYs. They never start a
  real agent or contact a provider. See
  [integration acceptance](integration-acceptance.md).
