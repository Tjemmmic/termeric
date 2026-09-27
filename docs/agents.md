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

Claude Code and Codex can also work in the experimental
[agent view](agent-view.md), a panel docked beside the terminals where
Termeric drives them without their terminal interfaces and shows approvals,
questions, plans, and diffs as native controls.

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
extension, or a package's entry point under `node_modules`, which is also
recognized by its package, as pnpm and Volta install them. Your own
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
running, focus the one you mean. Right before typing, Termeric checks the
terminal again. If the agent has exited, has handed the terminal to another
program such as an editor, or the terminal is read-only, you get a notice and
nothing is typed, so text meant for an agent never runs in a shell or lands
in an editor. If you type into the agent's prompt in the moment before
Termeric presses Enter, it does not press it.

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
  A submodule is staged at the commit it is on now. A file reached through a
  linked directory is staged as removed, as Git would.
- **Revert** (R) puts the file back as it was when the agent started, after
  you confirm. A file the agent created is removed. Only the working tree
  changes: staged content and commits stay as they are. Reverting writes only
  inside the repository, never through a linked directory, and applies the
  repository's line-ending settings. A file that Git ignored when the agent
  started, or anything in a directory it ignored, is never removed, since the
  snapshot does not know whether it was already there. A path that
  became a directory, or the other way round, is refused rather than half
  changed; revert the files inside first, and the emptied directories the
  agent made go with them, where the file system records when they were
  made.
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
yellow dot gives way to the working ring. Arrow and page keys, which only move
through its menu, do not count as an answer. Escape or Ctrl+C in a working or
waiting agent's terminal ends the turn, and the ring goes away; Claude Code
reports nothing when interrupted, or when you decline its request, so
Termeric goes by the key. Codex also reports interruptions itself. A finished
answer keeps showing when Claude Code later notes that it has sat at its
prompt. Compacting a long conversation is not reported as a new one.

Notifications appear only for terminals you are not looking at, and clicking
one shows that terminal. Turn them off with **Notify When Agents Need You** on
the Settings **Agents** page, or `[agent_status] notify = false`. The tab
announces the same state to screen readers.

**Show Agents** opens a panel beside the terminals that lists every agent in
the window, the ones that need you first. Each row says what the agent is
doing, for how long, and what it waits for, its error, or the start of its
answer. Choosing a row, with the pointer or with the arrow keys and Enter,
shows that terminal. While a supervisor runs, the foot of the panel shows its
mode, a **Take Over** or **Hand Back** button, and what it did most recently.
**Go to Agent…** is the quick way to jump: it lists the same agents in the
palette, where you can type to find one. **Copy Last Agent Response** copies the
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
setting and hook in those files keeps its exact text and order, and Termeric's
own hooks are updated where they already are, since Codex remembers which
hooks you trusted by their position. A file already up to date is not
touched. When removal leaves an event, or the hooks, empty, it is removed too.
Each file's previous version is kept beside it with `.termeric-backup`
appended. A file
linked from elsewhere, for example by a dotfile manager, is updated where it
really lives. If a file changes while the dialog is open, it is left alone.
Codex asks you to trust new hooks the next time it starts. Termeric does not
approve them on your behalf.

Setup looks for each file where the agent will: through `CLAUDE_CONFIG_DIR` or
`CODEX_HOME` as set in the agent's [`env`](#configuration) or its default
profile's, else as set in Termeric's own environment, else in your home
directory. A variable exported
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
answers them for you, as far as you allow. It is a conversation in the
[agent view](agent-view.md): **Start Supervisor** opens a tab named
**Supervisor**, filled by a Claude Code conversation that has the
[MCP tools](#letting-an-agent-direct-the-others) already connected, its
instructions, and your rules. It first summarizes what each agent is doing.
**Start Supervisor** while one runs goes to its tab.

Termeric tells the supervisor things as messages in that conversation, each
beginning `Termeric:`: an agent that needs input, finished, or failed, and
what became of the messages it tried to send. While it is busy, or while you
are writing to it, they wait and arrive together once it is free. You can
talk to it there as to any agent. It hears about agents in terminals only
through [status hooks](#agent-status), so starting one without them offers to
set them up; agent view sessions need no hooks.

Termeric decides each message the supervisor tries to send, according to the
mode. A banner at the top of its conversation says which applies:

| Mode | What happens to a message |
|---|---|
| Watch | Refused. The supervisor reports and suggests instead. |
| Assist (default) | Waits as a card in the supervisor's conversation, in full, with **Send** and **Don't send**; the tab shows that it needs you. The supervisor is told the outcome. |
| Auto | Sent at once, within the limits below. |

For an agent view session's approval, Assist shows the supervisor's answer on
the approval card as a suggestion, and Auto answers it.

In every mode Termeric enforces these limits, whatever the supervisor's
instructions say:

- It sends only to agents in its own window, never to itself or to a terminal
  without that agent.
- It sends at most `max_sends_per_minute` messages (6 by default). Messages
  you decline count too, so it cannot ask faster than it could send, and so
  do its answers to agent view approvals.
- It cannot send the same message to the same agent twice within 30 seconds,
  however the spacing or capitals differ.
- A message is plain text of at most 4000 bytes and 80 lines. It may not start
  with `!` or `/`, which the agent would run as a shell or agent command
  without its own approval. Control and text-direction characters are refused,
  so what you approve is what is sent.
- It waits while you type to that agent. Nothing it sends is added to a
  prompt you are writing. If a message cannot be delivered after all, or its
  recipient ends while the message waits for you, the supervisor is told.

**Take Over from Supervisor**, or **Take over** on its banner, puts it in
watch mode until you hand back, and tells it so. **Supervisor Mode…** changes
the mode, including for a running supervisor. The foot of the **Show Agents**
panel lists what it was told, what it asked, and what was sent or refused,
newest first. **Stop Supervisor** ends it and closes its tab; closing the tab
stops it too.

The rules are plain language in `supervisor.md`, beside `config.toml`.
**Edit Supervisor Rules** opens it in `$VISUAL` or `$EDITOR`, creating it with
examples the first time. A supervisor reads the rules when it starts, so
restart it after editing. Rules are limited to 16 KiB. The supervisor's sends do not need
`security.allow_gui_control_input`, because starting it is the permission. They
are not checked for destructive commands, because each agent still applies its
own approvals.

```toml
[supervisor]
agent = "claude-code"      # the agent view drives Claude Code for now
mode = "assist"            # "watch", "assist", or "auto"
max_sends_per_minute = 6   # 1 to 60
```

The supervisor runs Claude Code; Codex, which the agent view now drives, can
supervise once its launch carries the supervisor's server and instructions.

The supervisor's MCP server, named `termeric_supervisor` so it never replaces
a `termeric` server you registered, carries a random token that identifies it
to Termeric. Its tools are allowed up front, so it never stops at Claude
Code's own permission prompt to use them. The token is on the supervisor's
command line, where your other programs can read it, so Termeric also
requires each send to come from a process started under the supervisor's own
Claude Code process. An agent the supervisor oversees cannot borrow its
authority. That check needs Linux; elsewhere the supervisor can read and
report but not send. Other MCP clients use the ordinary rules below.

## Letting an agent direct the others

`termeric mcp` is a [Model Context Protocol](https://modelcontextprotocol.io)
server on standard input and output. It gives an agent these tools:

| Tool | What it does |
|---|---|
| `list_agents` | Every agent in Termeric, with its window, tab, and pane (or agent view session), state, what it is waiting for, and last answer |
| `read_terminal` | A pane's screen and up to 256 lines of scrollback |
| `send_to_agent` | Pastes a prompt into one agent and submits it, if that pane still runs the named agent |
| `read_conversation` | An [agent view](agent-view.md) session's recent conversation and the approvals it waits for |
| `answer_approval` | Allows or denies one approval in an agent view session, with a reason |
| `send_to_session` | Sends an agent view session its next message |

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
later, the same way as **Start New Agent Conversation**. Several prompts to one
agent are submitted one after another, never merged. The supervisor's message
rules apply: plain text within the size limit, nothing starting with `!` or
`/`, and nothing while you are typing to that agent. Prompts are not checked
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

The earlier defaults, `suggest` and `workspace-write`, are dropped from the
section, so a converted Codex launch follows your own Codex configuration. A
profile's own setting is always kept, `suggest` as `--ask-for-approval
untrusted`, because it overrode the section. A name the earlier plugins did
not know, which they refused to launch with, turns the agent off instead of
guessing its policy; the log names the setting. An existing
`[agents.<id>]` entry always wins over a converted one.

## Workspaces and restarts

A saved [workspace](workspaces.md) records each agent terminal's agent,
profile, and directory. Restoring it starts a fresh agent process. The
conversation is not resumed, so use **Resume Agent Conversation…** for that.
An agent pane keeps its identity, transcript, and diff history when it is
split around, collapsed, or moved to another tab or window.

## Limits

- Real Claude Code has been run once, in a contained test: detection, status
  hooks, answering and declining a request, interrupting, `termeric mcp`, and
  a supervisor reporting on a worker. Codex, and Codex as a supervisor, have
  been exercised with fake programs, and real Codex only up to its handshake,
  without a turn.
- Status comes only from agents with hooks set up: Claude Code and Codex.
  Other agents are recognized, but their state is not known.
- Agents in [live-domain](live-domains.md) terminals cannot report status.
  Their processes belong to the domain daemon rather than to a window.
- Agents inside tmux or screen cannot report status. The server keeps the
  environment of the terminal it was started in, but it detaches from that
  terminal, and a report from a process that does not descend from the
  terminal's own is set aside. This check needs a platform that reports the
  sender's process, as Linux does.
- Diffs are read from the terminal output, so an agent that never prints a
  unified diff has nothing to review.
- Automated tests use fake agent executables in real PTYs. They never start a
  real agent or contact a provider. See
  [integration acceptance](integration-acceptance.md).
