# Agent view

The agent view is a panel docked right of the terminals where a coding agent
works in a conversation instead of a terminal. Termeric starts the agent
without its terminal interface and talks to it over its structured protocol,
so approvals, questions, plans, edits, and command output appear as native
controls: buttons to allow or deny, choices to pick, diffs to read.

It is **experimental**. It sits beside the [terminal integration](agents.md),
which keeps working unchanged, and it can be removed without touching
terminals. It drives two agents, each through its own protocol:

- **Claude Code**, over its stream-json protocol;
- **Codex**, over its app server (`codex app-server`).

Gemini CLI and OpenCode still run in terminals.

## Opening it

**Agent View** (Ctrl+Shift+A by default) opens the panel with the keyboard, or
closes it when it already has the keyboard. The command palette also offers
**Agent View** and **New Agent View Session**.

Each tab has its own agent view and one conversation. Switching tabs switches
the conversation, and a tab without one shows no panel, so Claude Code in one
folder and Codex in another are simply two tabs. **New session** in a tab that
already holds a conversation opens a new tab for the new one, in the same
folder. A tab moved to another window takes its conversation along, and
closing a tab ends its conversation (the agent keeps it, so it can be
continued later).

The panel sits beside the tab's terminal, or fills the tab: the button left
of the clock in the header switches between the two. Filling the tab covers
the terminal, which keeps running at its size underneath, gives the panel the
keyboard, keeps the conversation to a readable column, and names the tab after
the conversation. A tab's agent view shows its state in the tab bar like an
agent in a terminal: working, needs you, or done and failed until you look.

**Open Agents In** on the Settings **Agents** page
(`[agent_view] open_agents_in`) makes **New Agent…** and **Resume Agent
Conversation…** open Claude Code and Codex in a new tab's agent view,
beside its terminal (`split`) or filling it (`full`), instead of in a
terminal (`terminal`, the default). Other agents still open in terminals,
and a launch that names an agent profile opens in a terminal, because
profiles apply only to terminal launches.

Beside the terminal, the panel reserves its width on the right; terminals reflow into what is left
and always keep at least 360 logical pixels. Drag the panel's left edge to
resize it. When the window is too narrow for both, the panel hides until
there is room, and while hidden it gives the keyboard back to the terminal.

The panel never takes over the window. A click in it, or its shortcut, gives
it the keyboard; a click in a terminal, or Escape with nothing left to cancel,
gives the keyboard back. While the panel has the keyboard, the terminal
cursor rests dim and steady, and terminal programs that track focus are told
they lost it, as they are when the window loses focus. Global shortcuts keep
working, except the chords the message box edits with.

## Starting a session

The **New session** page shows:

- the agent to start, Claude Code or Codex, and whether each was found. An
  agent that is not installed, or is turned off on the Settings **Agents**
  page, is listed with the reason and cannot be chosen. A new session from a
  tab's conversation starts with the same agent;
- the folder: the focused terminal's working directory, or another open
  terminal's from the folder menu;
- the chosen agent's earlier conversations in that folder, to continue:
  Claude Code's from its own logs, Codex's as its app server lists them
  once the session started ahead of the first message connects. The window
  keeps each folder's Codex list, so it stays while you navigate;
- **Open Claude in a terminal instead** (or Codex), which starts it in a new
  tab.

Type what the agent should work on and press Enter. The permission and model
chips under the message apply to the new session; choosing another agent
resets them to that agent's defaults. The agent starts as soon as the page
opens, so its commands, your account's models, and its earlier conversations
are listed before the first message. It follows the page's folder, including
a `cd` in the terminal beside it, and closing the panel before a message ends
it. Codex opens
its conversation (a thread) only with the first message, so a session that
never got one leaves nothing in Codex's history.

Termeric looks for `claude` and `codex` on its own `PATH` and then on the
`PATH` your login shell sets up, so a desktop-launched Termeric finds agents
installed through a shell profile. The login shell is asked once, in the
background; until it answers, the page shows *Looking for Claude Code…*.
`[agents.claude-code]` and `[agents.codex]` in the configuration set each
program and environment; their arguments apply only to terminal launches,
because the agent view needs its own.

## The conversation

- **Messages** you send appear as cards. The agent's answers are rendered
  Markdown: headings, lists, quotes, tables, code blocks, bold, italic,
  inline code, and links.
- **Quiet exploration** (reading files and searching) folds into one
  *Explored* row that expands.
- **Edits** show a diff card with added and removed line counts. Once an edit
  is made, the diff comes from Claude Code's own patch, with line numbers.
  Codex's patches show one card per file, numbered from its hunks, with new
  and deleted files marked.
- **Commands** show a card with the command, its status, and the end of its
  output, which expands.
- **Sub-agents** show their task, how many steps they took, and their latest
  step; their report expands.
- **To-dos**, and Codex's plan steps, show as a checklist; only the latest
  list stays open.
- **Thinking**, and Codex's reasoning summaries, show as a collapsed
  *Thought* row.
- Each turn ends with its duration and changed files, **Review changes**,
  and a button that copies the answer.

The conversation follows new output until you scroll up; a button then jumps
back to the latest message.

## Decisions

When the agent needs permission, a card asks for it under the tool call:

- **Allow** runs it once.
- **Always allow …** appears when the agent offers to remember the answer.
  Claude Code suggests rules it can describe, such as *Always allow cargo
  test in this project*, saved where it says. Codex offers *Always allow
  cargo test commands* when it proposes a command prefix to allow (saved to
  Codex's own rules), otherwise *Allow this command for the rest of the
  session*, and for patches *Allow edits to these files for the rest of the
  session*.
- **Deny** refuses and ends the turn; the message box then asks what the
  agent should do instead.

A denial with a reason, from the supervisor or `answer_approval`, lets the
agent continue its turn. Claude Code reads the reason with the refusal;
Codex takes none, so the reason follows as a note to the running turn.

A **question** the agent asks shows its options as choices; **Other** lets
you type an answer in the message box. **Skip** declines to answer.

A **plan** (from Claude Code's *Plan only* mode) shows as a card with
**Start, ask before edits**, **Start, edit without asking**, or **Keep
planning**, which asks what to change.

## The message box

Enter sends and Shift+Enter starts a new line. While the agent works, a message
you send waits as a *Queued* chip above the box and goes when the turn ends,
so every message starts a turn of its own; × on the chip cancels it and puts
the text back in the box. With the box empty, the send button becomes
**Stop**; Escape also stops the turn, and queued messages then go in order.

Typing `/` at the start of a message lists the commands the agent runs
without its terminal, filtered as you type. Up and Down move through the
list, Tab completes the highlighted command, Enter completes it and sends it
when it takes no argument, and Escape hides the list. The list appears once
a session has started:

- Claude Code sends its own list: built-in commands such as `/compact`,
  `/context`, and `/init`, and your own skills and commands. It refuses
  commands that open one of its own screens, such as `/permissions`.
- Codex has no list of its own, so Termeric offers `/compact` (compacts the
  conversation), `/review` (reviews uncommitted changes, or what you write
  after it), and your Codex skills, which go to Codex with the message. Its
  other commands, such as `/status`, work only in its terminal.

Sending a command the agent runs only in its terminal shows **Open in a
terminal**, which continues the conversation in the agent's terminal
interface.

Up and Down on the first and last lines recall earlier messages. Ctrl+A,
Ctrl+C, Ctrl+X, Ctrl+V, Ctrl+Z, and Ctrl+Shift+Z (or Ctrl+Y) work as in any
text field, and double-clicking selects a word. Input methods compose at the
caret.

Chips above the box attach context from the focused terminal:

- **Terminal selection** attaches the selected text;
- the **last command**, when shell integration recorded one, attaches its
  output (up to 64 KiB), marked when it failed.

The permission chip switches between the agent's modes. Claude Code's:

- **Ask before edits** approves each edit and command;
- **Edit without asking** applies edits at once, and commands still ask;
- **Plan only** reads and plans, then asks before changing anything;
- **Auto** lets Claude Code's safety classifier approve or block each action;
- **Deny unless allowed** runs only actions already allowed and refuses the rest;
- **Bypass permissions** runs everything without asking. It is offered only
  after you turn on **Allow Bypass Permissions** on the Settings **Agents**
  page (`[agent_view] allow_bypass_permissions = true`), which starts Claude
  Code with `--allow-dangerously-skip-permissions`. A session started before
  the setting was on restarts with that allowance when you choose the mode,
  continuing its conversation; during a turn, it restarts once the turn
  ends. Turning the setting off returns any session in that mode to asking.

Codex's, which pair an approval policy with a sandbox:

- **Ask before changes** (`on-request`, read-only sandbox) reads and runs
  read-only commands, and asks before editing or anything else;
- **Auto** (`on-request`, workspace-write sandbox) edits and runs commands in
  the folder, and asks before leaving it or using the network;
- **Full access** (`never`, no sandbox) runs everything without asking. Like
  **Bypass permissions**, it needs **Allow Bypass Permissions**; turning the
  setting off returns the session to asking.

Nothing requires a sandbox for these two modes, but they run every command
and edit without asking, so they suit a container, a virtual machine, or a
throwaway checkout. Like every setting, **Allow Bypass Permissions** applies
at once to the window it is changed in; **Save** keeps it for new windows
and later starts.

A session never starts in a mode that skips every check without that
setting, whoever asked for it. Codex takes a mode or model change with the
next message, since it applies them per turn.

Some accounts cannot use Claude Code's **Auto**; its refusal then shows in the
conversation. The model chip lists the models the agent offers for this
account; *Default* keeps the model the agent's own settings choose. The ring
beside the send button shows how much of the model's context the
conversation fills.

## Sessions

The clock button, or the title in the header, opens **Agents**: this window's
sessions, in every tab, grouped into *Needs you*, *Working*, and *Recent*
(choosing one goes to its tab); agents running in
terminals; the tab's agent's earlier conversations to continue; and the
supervisor, when one runs. **Show Agents** in the command palette lists agent view sessions too.

A session that needs you while you cannot see it sends a desktop
notification, following `agent_status.notify`.

The **⋯** menu offers **Open in a terminal**, which ends the session here and
continues the same conversation in a new tab; **Copy conversation**; **End
session**, which stops the agent and removes the session from the panel
(the agent keeps the conversation, and the new-session page offers it again
under earlier conversations); and **Hide panel**. **Open in a terminal** runs `claude --resume <id>` or
`codex resume <id>`.

A conversation opened in a terminal stays readable in the panel but takes no
more messages there, because two agent processes must never write one
conversation. Once the terminal is done with a Claude Code conversation,
continue it from the new-session page. A message sent while a session is
still closing waits and goes once the agent has restarted; Codex continues
the same thread.

Sessions belong to their tab and end with it, or with its window. The agents
keep every conversation in their logs, so any of them can be continued later.

## Reviewing changes

Each session takes a snapshot of its folder's Git working tree when it
starts. **Review changes** lists what changed since then and opens the same
diff viewer as [agent changes in terminals](agents.md#reviewing-an-agents-changes),
with the same stage and revert actions and the same safeguards.

## The supervisor

A running [supervisor](agents.md#the-supervisor) is itself an agent view
conversation, in a tab of its own, and oversees the other agent view
sessions as well as terminal agents. It is told, once each, when a session asks for a
decision and when a turn ends, and its tools reach sessions directly:

- `read_conversation` reads a session's recent conversation, ending with the
  approvals it waits for and their ids;
- `answer_approval` allows or denies one approval with a reason;
- `send_to_session` sends a session its next message.

Its mode decides what these do. **Auto** answers and sends, within the same
limits as its terminal sends, and the conversation shows *Allowed by the
supervisor* with its reason. **Assist** shows its answer on the approval
card as a suggestion for you to act on, tells the supervisor how you settled
it, and holds each message as a **Send** / **Don't send** card in the
supervisor's own conversation. **Watch** refuses both. Its tools are allowed up front, so it never stops at Claude Code's own
permission prompt to use them. A banner in each conversation says
which applies, with **Take over** and **Hand back**. A message is refused
while you are writing one to the same session.

## How it works

Each agent's protocol translates to and from the same agent-neutral events
and commands (`agent_view/protocol.rs`), so the session, conversation, and
panel do not know which agent they show. Tool calls are presented in Claude
Code's vocabulary (`Bash`, `Edit`, `TodoWrite`, `AskUserQuestion`, …), which
other protocols translate into, plus `Patch` for a change to several files.

### Claude Code

Termeric runs:

```text
claude -p --input-format stream-json --output-format stream-json --verbose \
  --include-partial-messages --permission-prompt-tool stdio \
  --permission-mode <mode> [--model <model>] [--resume <session>]
```

Claude Code then writes one JSON message per line: streamed text, complete
messages, tool results, turn results, and permission requests
(`can_use_tool` control requests), which Termeric answers on the same pipe.
The official Claude Agent SDKs speak this protocol, but Claude Code does not
document it as a stable interface, so decoding is forgiving: it reads only
the fields it needs, ignores message kinds it does not know, and refuses
control requests it cannot serve so the agent never waits on them. If Claude
Code does not answer the first request within 30 seconds, the session
explains that and offers the terminal instead. The agent view was built
against Claude Code 2.1.278 to 2.1.282 and warns about releases before 2.1.

### Codex

Termeric runs `codex app-server`, which speaks JSON-RPC over its pipes, one
message per line. After `initialize`, Termeric reads the account (to say
when Codex is not signed in), `model/list`, `skills/list`, and `thread/list`
for the folder's earlier threads started from Codex's terminal interface,
its editor extension, or an app server (scripted `codex exec` runs are left
out); each is titled by its name, else its first message. The list is asked
of Codex's index first (`useStateDbOnly`, which answers at once) and then in
full, reconciled with its logs, which takes seconds and replaces it. The first
message starts a thread (`thread/start`, or `thread/resume` to continue one)
with the mode's approval policy and sandbox; each message is a `turn/start`
carrying the mode's sandbox policy and the chosen model. Streamed items
(agent text, reasoning summaries, commands, file changes, MCP tool calls,
web searches, plan updates) become the same events as Claude Code's, and
`turn/completed` ends the turn.

Codex asks through requests the panel must answer: command and patch
approvals, permission requests, and questions become approval cards; an MCP
server's request for input is declined with a notice, and any other request
is refused with an error, because a request left unanswered stalls the turn.
Only the conversation's own thread shapes it; sub-agents' threads are
ignored except for their requests. Turns are stopped with `turn/interrupt`,
and a note written after declining reaches the running turn through
`turn/steer`. The agent view was built against Codex 0.157.

### Both

Each agent runs in its own process group, which closing a session ends
completely; commands it leaves running when it exits end with it. It
inherits Termeric's environment except `TERMERIC_TERMINAL_ID`, so hooks it
runs are not mistaken for a terminal's.

The code is split so each layer can be tested without the next:

| Module | Contents |
|---|---|
| `agent_view/event.rs` | Agent-neutral events and commands |
| `agent_view/agent.rs` | The agents it drives: names, modes, programs, and protocols |
| `agent_view/protocol.rs` | What every protocol provides to a session |
| `agent_view/claude.rs` | Claude Code's stream-json protocol, both directions |
| `agent_view/codex.rs` | Codex's app-server protocol, both directions |
| `agent_view/process.rs` | The process, its pipes, and bounded line reading |
| `agent_view/session.rs` | One conversation: process, protocol, and transcript |
| `agent_view/transcript.rs` | The conversation model the panel shows |
| `agent_view/present.rs` | Tool summaries, grouping, diffs, and conversation text |
| `agent_view/markdown.rs` | Markdown as styled blocks |
| `agent_view/history.rs` | Earlier conversations from Claude Code's logs (Codex lists its own) |
| `ui/agent_view/` | Panel state, colors, and the scene: primitives, hit targets, and accessibility nodes built once per change |
| `renderer/agent_view.rs` | Drawing a scene |
| `app/agent_view*.rs` | Sessions in a window, input, actions, and control requests |

The scene is the single source for drawing, pointer targets, keyboard focus
order, and the accessibility tree, so what is drawn is what is clicked and
announced. Text is laid out through a measuring trait: real fonts in the app,
fixed advances in tests.

## Accessibility

The panel joins the accessibility tree beside the terminals as an *Agent
view* pane. Messages and answers are text, controls are buttons, toggles
(with their expanded state), options (with their selection), menu items, and
a multi-line text input holding the message. Focus follows the panel's
keyboard focus. Tab and Shift+Tab move through every control, with a focus
ring, and Enter or Space activates one. As with terminals, the conversation's
text is exposed for what is on screen.

## Testing

`termeric/tests/fixtures/agent_view/fake_claude.py` and `fake_codex.py`
speak each agent's protocol without an account or any usage. Words in each
prompt choose what they do: `edit`, `run`, `ask`, `todo`, `slow`, `fail`, or
`crash` for both; `plan` and `agent` for Claude Code; `settings` for Codex,
which answers with the mode and model its turn ran with. Session tests run
whole conversations through them, and they can stand in for the real agents
in the GUI:

```toml
[agents.claude-code]
program = "/path/to/termeric/termeric/tests/fixtures/agent_view/fake_claude.py"

[agents.codex]
program = "/path/to/termeric/termeric/tests/fixtures/agent_view/fake_codex.py"
```

## Limits

- Claude Code and Codex only; Gemini CLI and OpenCode run in terminals.
- Codex shows a command's output when the command ends, not while it runs,
  and does not show Codex's plan-mode proposals.
- No image attachments or file mentions.
- Sessions do not survive a restart of Termeric, though their conversations
  can be continued.
- Accessibility exposes the visible part of the conversation.
- Neither protocol is a stable contract (Claude Code's is undocumented, and
  Codex marks its app server experimental); a release that changes one can
  break the panel until Termeric follows.
