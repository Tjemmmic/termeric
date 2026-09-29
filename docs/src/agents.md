# Agents in terminals

Termeric runs AI coding agents, the command-line tools that read and edit a repository from a terminal, in ordinary tabs and split panes. It recognizes an agent however you start it, shows what each agent is doing on its tab, notifies you when one needs you, and lets you review, stage, or revert the files an agent changed.

> **Experimental.** Agent support is on by default; install an agent and it is offered. Status badges and notifications work only for Claude Code and Codex, after you run **Set Up Agent Status…**. The supervisor and `termeric mcp` are the newest parts and have seen the least real use.

Four agents are built in, and you can add any other command-line agent in your configuration:

| Agent | Id | Program | Resume arguments | New conversation |
| --- | --- | --- | --- | --- |
| Claude Code | `claude-code` | `claude` | `--continue` | `/clear` |
| Codex | `codex` | `codex` | `resume --last` | `/new` |
| Gemini CLI | `gemini` | `gemini` | `--resume latest` | `/clear` |
| OpenCode | `opencode` | `opencode` | `--continue` | `/new` |

Only agents whose program is installed on your `PATH` are offered. Turn any of them off on the **Agents** page of Settings.

Claude Code, Codex, and OpenCode can also run in the [agent view](agent-view.md), a panel docked beside your terminals that shows the conversation with native buttons instead of the agent's own terminal interface.

## Starting an agent

Press **Ctrl+Shift+I** (**New Agent…**) to pick an installed agent, or one of its profiles, and start it in a new tab. The command palette also offers:

| Command | What it does |
| --- | --- |
| **New Agent…** | Start an agent in a new tab. |
| **Resume Agent Conversation…** | Start an agent with its resume arguments, continuing its latest conversation. Agents that cannot resume are left out. |
| **Split Right with Agent…** | Start an agent in a new pane to the right of the focused terminal. |
| **Split Down with Agent…** | Start an agent in a new pane below the focused terminal. |

An agent starts in the focused terminal's working directory unless its `inherit_cwd` setting is off. To start one from a script or a desktop launcher, use `termeric --profile AGENT:PROFILE` (see [Command line](command-line.md)).

If **Open Agents In** on the Settings **Agents** page is set to an agent view option, **New Agent…** and **Resume Agent Conversation…** open Claude Code, Codex, and OpenCode in the agent view instead. Choosing a profile always opens a terminal.

## Agents in any pane

You don't have to start an agent through Termeric. About once a second, Termeric looks at the program running in the foreground of each terminal, so typing `claude` or `codex` at a shell prompt is recognized too. A program counts as an agent when its name matches the agent's program or one of its `detect` names. For agents written in Node, Python, or another scripting language, Termeric looks at the installed script rather than the interpreter, so a file of your own named `claude.py` is not mistaken for Claude Code.

When the focused terminal runs an agent, the command palette adds commands for it. They are hidden everywhere else:

| Command | What it does |
| --- | --- |
| **Start New Agent Conversation** | Types the agent's new-conversation command, such as `/clear`, and submits it. |
| **Restart Agent** | Starts the same agent, profile, and directory again and closes the old process. Offered only for agents Termeric started. |
| **Send Context to Agent…** | Opens a picker of context to gather: unstaged or staged Git diff, file tree, the last 50 lines of terminal output, the last test output, or specific files. The result is pasted into the agent without pressing Enter, so you can add to it first. |
| **Review Agent Diff** | Shows the most recent unified diff the agent printed. |
| **Copy Agent Diff** | Copies that diff to the clipboard. |
| **Export Agent Session** | Exports the terminal's transcript, or its scrollback when it is not recording. |
| **Toggle Agent Transcript** | Starts or stops recording this terminal's raw output to a transcript. |
| **Copy Last Agent Response** | Copies the agent's most recent answer, from its status report or else from the end of its own conversation log. Needs agent status. |

**Review Agent Diff** and **Copy Agent Diff** appear only when the agent printed a diff Termeric recognized, and only for agents Termeric started.

Before Termeric types anything into an agent, it checks the terminal again. If the agent has exited, has handed the terminal to another program such as an editor, or the pane is read-only, you get a notice and nothing is typed. Text meant for an agent never lands in a shell.

## Agent status

With agent status set up, each agent's tab shows what it is doing:

| State | On the tab | Desktop notification |
| --- | --- | --- |
| Working | A ring before the title | None |
| Needs you (a permission prompt or a question) | A dot, until the agent moves on | "Codex needs you", with what it asks |
| Done | A dot, until you look at that terminal | "Codex is done", with the first line of the answer |
| Failed | A dot, until you look at that terminal | "Claude Code stopped with an error" |

Notifications appear only for terminals you are not looking at, and clicking one shows that terminal. Turn them off with **Notify When Agents Need You** on the Settings **Agents** page, or with `notify = false` under `[agent_status]`. Screen readers hear the same state after the tab title.

Pressing Escape or Ctrl+C in a working or waiting agent ends the ring, since Claude Code does not report an interruption itself.

### Setting it up

Agents report their state through hooks, commands they run at points in a conversation. **Set Up Agent Status…**, in the command palette or on the Settings **Agents** page, adds Termeric's hooks to:

| Agent | File |
| --- | --- |
| Claude Code | `~/.claude/settings.json`, or `settings.json` in `$CLAUDE_CONFIG_DIR` |
| Codex | `~/.codex/hooks.json`, or `hooks.json` in `$CODEX_HOME` |

Setup lists the files it will change and waits for you to confirm. Your other settings and hooks keep their exact text and order, and each file's previous version is saved beside it with `.termeric-backup` appended. Codex asks you to trust the new hooks the next time it starts; Termeric does not approve them for you.

If you set `CLAUDE_CONFIG_DIR` or `CODEX_HOME` only in your shell's startup file, a Termeric started from your desktop won't see it. Set it in the agent's `env` in your configuration too, and setup will find the right file.

Each hook runs `termeric agent-event AGENT EVENT`. It does nothing outside a Termeric terminal, never prints, and always succeeds, so it cannot slow down or break an agent. **Remove Agent Status Hooks…** takes the hooks out again. The Settings page shows **Needs updating** when you should run setup again, for example after moving the Termeric program.

### Finding agents

| Command | What it does |
| --- | --- |
| **Show Agents** | Opens a panel listing every agent in the window, those that need you first, with what each is doing and for how long. Choose one to show its terminal. |
| **Go to Agent…** | Lists the same agents in the command palette, where you can type to find one. |

## Reviewing an agent's changes

When Termeric first sees an agent in a terminal inside a Git working tree, it takes a snapshot of that working tree, including your own uncommitted work. Taking it never changes your index or files. **Review Agent Changes…** then lists every file that differs from the snapshot, with lines added and removed, and marks files already staged. Choose a file to see its diff:

| Key | Action |
| --- | --- |
| S | **Stage** the file as it is now, including a deletion. |
| R | **Revert** the file to how it was when the agent started, after you confirm. A file the agent created is removed. Only the working tree changes; the index and commits stay as they are. |
| C | **Copy** the file's diff. |
| Up, Down, J, K, Page Up, Page Down | Scroll. |
| Enter, Escape | Close. |

**Stage All Agent Changes** stages every changed file at once. Both commands stay available after the agent exits, until another agent starts in that terminal; a restart takes a new snapshot.

Revert is careful: it never writes outside the repository or through a linked directory, never removes a file Git ignored when the agent started, and refuses a path that turned from a file into a directory or back. Untracked files over 16 MiB and nested repositories are left out of the snapshot, and working trees with more than 50,000 files cannot be snapshotted. Changes you make yourself while the agent runs are listed too.

## The supervisor

The supervisor is an agent that watches the other agents in its window and answers them for you, as far as you allow. **Start Supervisor** opens a tab named **Supervisor** holding a Claude Code or Codex conversation in the [agent view](agent-view.md). It first summarizes what each agent is doing. Afterwards Termeric sends it a message, beginning `Termeric:`, whenever an agent needs input, finishes, or fails. You can talk to it there like any agent.

It learns about agents in terminals only through agent status hooks, so starting it without them offers to set them up. Agent view sessions need no hooks.

The mode decides what happens when the supervisor tries to send a message to an agent:

| Mode | What happens |
| --- | --- |
| Watch | Refused. The supervisor reports and suggests instead. |
| Assist (default) | The message waits in the supervisor's conversation with **Send** and **Don't send**, and its tab shows that it needs you. |
| Auto | Sent at once, within the limits below. |

For an approval in an agent view session, Assist shows the supervisor's answer on the approval card as a suggestion, and Auto answers it.

The supervisor reads agents' screens and conversations, and it can send messages only to agents, never to a shell. Whatever its instructions say, it:

- sends only to agents in its own window, never to itself;
- sends at most `max_sends_per_minute` messages (6 by default), counting ones you decline;
- cannot send the same message to the same agent twice within 30 seconds;
- sends only plain text of at most 4000 bytes and 80 lines, never starting with `!` or `/`, which the agent would run as a command;
- waits while you are typing to that agent.

| Command | What it does |
| --- | --- |
| **Start Supervisor** | Starts it, or shows its tab when it is already running. |
| **Supervisor Mode…** | Changes the mode, also for a running supervisor. |
| **Take Over from Supervisor** | Puts it in Watch mode until you hand back. |
| **Hand Back to Supervisor** | Returns it to its mode. |
| **Edit Supervisor Rules** | Opens your rules in `$VISUAL` or `$EDITOR` in a new tab. |
| **Stop Supervisor** | Ends it and closes its tab. Closing the tab stops it too. |

Choose the supervisor's agent and its starting mode with **Supervisor** and **Supervisor Mode** on the Settings **Agents** page, or in your configuration:

```toml
[supervisor]
agent = "claude-code"      # or "codex"
mode = "assist"            # "watch", "assist", or "auto"
max_sends_per_minute = 6   # 1 to 60
```

Your rules are plain language in `supervisor.md`, next to `config.toml`. The file is created with examples the first time and may be at most 16 KiB. The supervisor reads it when it starts, so restart it after editing.

The supervisor's messages don't need `security.allow_gui_control_input`: starting it is the permission. Only the supervisor's own agent process and what it runs can use that authority, so the agents it oversees cannot borrow it.

## Letting an agent use Termeric: `termeric mcp`

`termeric mcp` is a [Model Context Protocol](https://modelcontextprotocol.io) server on standard input and output. It gives an agent these tools:

| Tool | What it does |
| --- | --- |
| `list_agents` | Lists every agent in Termeric with its window, tab, and pane (or agent view session), its state, what it waits for, and its last answer. |
| `read_terminal` | Reads a pane's screen and up to 256 lines of scrollback. |
| `send_to_agent` | Pastes a prompt into one agent and submits it, if that pane still runs the named agent. |
| `read_conversation` | Reads an agent view session's recent conversation and the approvals it waits for. |
| `answer_approval` | Allows or denies one approval in an agent view session, with a reason. |
| `send_to_session` | Sends an agent view session its next message. |

Register it once with each agent that should use it:

```sh
claude mcp add termeric -- termeric mcp
codex mcp add termeric -- termeric mcp
```

Run inside a Termeric terminal, the server talks to that terminal's Termeric. Elsewhere it uses the only running Termeric, or the one you name with `--process PID`.

Reading works as soon as the server runs. Sending, answering, and submitting are refused until you add this to your configuration, because they let one program type into another:

```toml
[security]
allow_gui_control_input = true
```

Messages follow the same rules as the supervisor's: plain text within the size limit, nothing starting with `!` or `/`, and nothing while you are typing to that agent. See [Controlling a running Termeric](control.md) for what else this setting allows.

## Configuration

An `[agents.<id>]` entry in `config.toml` changes a built-in agent; an entry with any other id adds a new one. Every key is optional:

| Key | Meaning |
| --- | --- |
| `enabled` | `false` hides the agent from pickers and detection. |
| `name` | Display name. A new agent defaults to its id. |
| `program` | Executable to launch. Its file name is also recognized, unless it is a launcher such as `npx`, `docker`, `node`, or `sh`; then add the agent's own name to `detect`. A new agent defaults to its id. |
| `detect` | Extra program names to recognize, such as a wrapper script. |
| `args` | Arguments passed on every launch. |
| `resume_args` | Arguments that continue the latest conversation. Without them the agent cannot resume. |
| `new_conversation` | Text typed to start a new conversation. An empty string removes the command. |
| `inherit_cwd` | Start in the focused terminal's directory. Defaults to `true`. |
| `env` | Environment variables added on every launch. |
| `profiles.<name>` | A named variant with `description`, `args`, and `env`. |
| `default_profile` | The profile used when a launch names none. It must exist. |

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

Profile arguments follow the agent's `args`, and resume arguments come last. Agent ids use letters, digits, `-`, and `_`.

Older `[plugins.claude_code]` and `[plugins.codex]` sections are still read and converted to `[agents.*]` entries; Settings drops the old sections the next time it saves.

## Workspaces and moving panes

A saved [workspace](workspaces-and-sessions.md) remembers each agent terminal's agent, profile, and directory. Restoring it starts a fresh agent; use **Resume Agent Conversation…** to continue the conversation. An agent pane keeps its transcript, diffs, and status when you split around it or move it to another tab or window.

## Limits

- Status comes only from Claude Code and Codex. Gemini CLI, OpenCode in a terminal, and custom agents are recognized, but their state is not known.
- Agents in [live domain](live-domains.md) terminals, or inside tmux or screen, cannot report status.
- Diff review reads diffs from the terminal output, so an agent that never prints a unified diff has nothing to show there. **Review Agent Changes…** works regardless.
