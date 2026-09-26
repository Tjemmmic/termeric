# Agent view

The agent view is a panel docked right of the terminals where Claude Code
works in a conversation instead of a terminal. Termeric starts Claude Code
without its terminal interface and talks to it over its structured protocol,
so approvals, questions, plans, edits, and command output appear as native
controls: buttons to allow or deny, choices to pick, diffs to read.

It is **experimental**. It sits beside the [terminal integration](agents.md),
which keeps working unchanged, and it can be removed without touching
terminals. Claude Code is the only agent it drives today.

## Opening it

**Agent View** (Ctrl+Shift+A by default) opens the panel with the keyboard, or
closes it when it already has the keyboard. The command palette also offers
**Agent View** and **New Agent View Session**.

The panel reserves its width on the right; terminals reflow into what is left
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

- the agent: Claude Code, and whether it was found;
- the folder: the focused terminal's working directory, or another open
  terminal's from the folder menu;
- earlier conversations in that folder, from Claude Code's own logs, to
  continue;
- **Open Claude in a terminal instead**, which starts it in a new tab.

Type what Claude should work on and press Enter. The permission and model
chips under the message apply to the new session.

Termeric looks for `claude` on its own `PATH` and then on the `PATH` your
login shell sets up, so a desktop-launched Termeric finds a `claude`
installed through a shell profile. The login shell is asked once, in the
background; until it answers, the page shows *Looking for Claude Code…*. `[agents.claude-code]` in the
configuration sets the program and environment; its arguments apply only to
terminal launches, because the agent view needs its own.

## The conversation

- **Messages** you send appear as cards. Claude's answers are rendered
  Markdown: headings, lists, quotes, tables, code blocks, bold, italic,
  inline code, and links.
- **Quiet exploration** (reading files and searching) folds into one
  *Explored* row that expands.
- **Edits** show a diff card with added and removed line counts. Once an edit
  is made, the diff comes from Claude Code's own patch, with line numbers.
- **Commands** show a card with the command, its status, and the end of its
  output, which expands.
- **Sub-agents** show their task, how many steps they took, and their latest
  step; their report expands.
- **To-dos** show as a checklist; only the latest list stays open.
- **Thinking** shows as a collapsed *Thought* row.
- Each turn ends with its duration and changed files, **Review changes**,
  and a button that copies the answer.

The conversation follows new output until you scroll up; a button then jumps
back to the latest message.

## Decisions

When Claude Code needs permission, a card asks for it under the tool call:

- **Allow** runs it once.
- **Always allow …** appears when Claude Code suggests a rule it can
  describe, such as *Always allow cargo test in this project*. The rule is
  Claude Code's own and is saved where it says.
- **Deny** refuses and ends the turn; the message box then asks what Claude
  should do instead.

A **question** Claude asks shows its options as choices; **Other** lets you
type an answer in the message box. **Skip** declines to answer.

A **plan** (from *Plan only* mode) shows as a card with **Start, ask before
edits**, **Start, edit without asking**, or **Keep planning**, which asks
what to change.

## The message box

Enter sends and Shift+Enter starts a new line. While Claude works, a message
you send waits as a *Queued* chip above the box and goes when the turn ends,
so every message starts a turn of its own; × on the chip cancels it and puts
the text back in the box. With the box empty, the send button becomes
**Stop**; Escape also stops the turn, and queued messages then go in order.

Typing `/` at the start of a message lists the commands Claude Code runs
without its terminal: its built-in ones, such as `/compact`, `/context`, and
`/init`, and your own skills and commands, filtered as you type. Up and Down
move through the list, Tab completes the highlighted command, Enter completes
it and sends it when it takes no argument, and Escape hides the list. Claude
Code refuses commands that open one of its own screens, such as
`/permissions`; sending one of those shows **Open in a terminal**, which
continues the conversation in Claude Code's terminal interface. Claude Code
sends its list when a session connects, so the list appears once a session
has started.

Up and Down on the first and last lines recall earlier messages. Ctrl+A,
Ctrl+C, Ctrl+X, Ctrl+V, Ctrl+Z, and Ctrl+Shift+Z (or Ctrl+Y) work as in any
text field, and double-clicking selects a word. Input methods compose at the
caret.

Chips above the box attach context from the focused terminal:

- **Terminal selection** attaches the selected text;
- the **last command**, when shell integration recorded one, attaches its
  output (up to 64 KiB), marked when it failed.

The permission chip switches between Claude Code's modes:

- **Ask before edits** approves each edit and command;
- **Edit without asking** applies edits at once, and commands still ask;
- **Plan only** reads and plans, then asks before changing anything;
- **Auto** lets Claude Code's safety classifier approve or block each action;
- **Deny unless allowed** runs only actions already allowed and refuses the rest;
- **Bypass permissions** runs everything without asking. It is offered only
  after you turn on **Allow Bypass Permissions** on the Settings **Agents**
  page (`[agent_view] allow_bypass_permissions = true`), which starts Claude
  Code with `--allow-dangerously-skip-permissions`. Use it only in a sandbox.
  Turning the setting off returns any session in that mode to asking.

Some accounts cannot use **Auto**; Claude Code's refusal then shows in the
conversation. The model chip lists the models Claude Code offers for this
account. The ring beside the send button shows how much of
the model's context the conversation fills.

## Sessions

The clock button, or the title in the header, opens **Agents**: this window's
sessions grouped into *Needs you*, *Working*, and *Recent*; agents running in
terminals; earlier conversations to continue; and the supervisor, when one
runs. **Show Agents** in the command palette lists agent view sessions too.

A session that needs you while you cannot see it sends a desktop
notification, following `agent_status.notify`.

The **⋯** menu offers **Open in a terminal**, which ends the session here and
continues the same conversation in a new tab; **Copy conversation**; **End
session**, which stops Claude Code and removes the session from the panel
(Claude Code keeps the conversation, so the new-session page offers it again
under earlier conversations); and **Hide panel**.

A conversation opened in a terminal stays readable in the panel but takes no
more messages there, because two Claude Code processes must never write one
conversation. Once the terminal is done with it, continue it from the
new-session page. A message sent while a session is still closing waits and
goes once Claude Code has restarted.

Sessions belong to their window and end with it. Claude Code keeps every
conversation in its logs, so any of them can be continued later from the
new-session page.

## Reviewing changes

Each session takes a snapshot of its folder's Git working tree when it
starts. **Review changes** lists what changed since then and opens the same
diff viewer as [agent changes in terminals](agents.md#reviewing-an-agents-changes),
with the same stage and revert actions and the same safeguards.

## The supervisor

A running [supervisor](agents.md#the-supervisor) oversees agent view sessions
as well as terminal agents. It is told, once each, when a session asks for a
decision and when a turn ends, and its tools reach sessions directly:

- `read_conversation` reads a session's recent conversation, ending with the
  approvals it waits for and their ids;
- `answer_approval` allows or denies one approval with a reason;
- `send_to_session` sends a session its next message.

Its mode decides what these do. **Auto** answers and sends, within the same
limits as its terminal sends, and the conversation shows *Allowed by the
supervisor* with its reason. **Assist** shows its answer on the approval
card as a suggestion for you to act on, tells the supervisor how you settled
it, and asks before each message, as for terminal agents. **Watch** refuses
both. Its tools are allowed up front, so it never stops at Claude Code's own
permission prompt to use them. A banner in each conversation says
which applies, with **Take over** and **Hand back**. A message is refused
while you are writing one to the same session.

## How it works

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

Claude Code runs in its own process group, which closing a session ends
completely; commands it leaves running when it exits end with it. It inherits Termeric's environment except `TERMERIC_TERMINAL_ID`,
so hooks it runs are not mistaken for a terminal's.

The code is split so each layer can be tested without the next:

| Module | Contents |
|---|---|
| `agent_view/event.rs` | Agent-neutral events and commands |
| `agent_view/claude.rs` | The stream-json protocol, both directions |
| `agent_view/process.rs` | The process, its pipes, and bounded line reading |
| `agent_view/session.rs` | One conversation: process, protocol, and transcript |
| `agent_view/transcript.rs` | The conversation model the panel shows |
| `agent_view/present.rs` | Tool summaries, grouping, diffs, and conversation text |
| `agent_view/markdown.rs` | Markdown as styled blocks |
| `agent_view/history.rs` | Earlier conversations from Claude Code's logs |
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

`termeric/tests/fixtures/agent_view/fake_claude.py` speaks the same protocol
without an account. Words in each prompt choose what it does: `edit`, `run`,
`ask`, `plan`, `todo`, `agent`, `slow`, `fail`, or `crash`. Session tests run
whole conversations through it, and it can stand in for Claude Code in the
GUI:

```toml
[agents.claude-code]
program = "/path/to/termeric/termeric/tests/fixtures/agent_view/fake_claude.py"
```

## Limits

- Claude Code only; Codex's app server is not driven yet.
- No image attachments or file mentions.
- Sessions do not survive a restart of Termeric, though their conversations
  can be continued.
- Accessibility exposes the visible part of the conversation.
- The protocol is not a documented contract of Claude Code; a release that
  changes it can break the panel until Termeric follows.
