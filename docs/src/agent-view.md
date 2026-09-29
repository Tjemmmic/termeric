# Agent view (experimental)

The agent view is a panel docked to the right of your terminals where a coding agent works as a conversation instead of a terminal program. Termeric starts the agent without its terminal interface and talks to it directly, so permission prompts, questions, plans, edits, and command output appear as buttons, choices, and diffs you can click and read.

> **Experimental.** There is nothing to turn on: press **Ctrl+Shift+A** to open it. It drives Claude Code, Codex, and OpenCode; other agents, including Gemini CLI, still run in [terminals](agents.md). It cannot attach images or mention files, sessions end when Termeric quits (the agents keep the conversations, so you can continue them), and a new agent release can break it until Termeric catches up.

## Opening it

| Command | Default key | What it does |
| --- | --- | --- |
| **Agent View** | Ctrl+Shift+A | Opens the panel and gives it the keyboard, or closes it when it already has the keyboard. |
| **New Agent View Session** | | Opens the panel on a new session. |

Each tab has its own agent view holding one conversation. Switching tabs switches the conversation, and a tab without one shows no panel, so Claude Code in one project and Codex in another are simply two tabs. A tab moved to another window takes its conversation along, and closing a tab ends its session.

The panel sits beside the tab's terminal, or fills the tab. The **Fill the tab** button in the panel's header, which becomes **Show the terminal beside**, switches between the two. Beside the terminal, drag the panel's left edge to resize it; the terminal keeps at least 360 pixels, and when the window is too narrow for both, the panel hides until there is room. When the panel fills the tab, the terminal keeps running underneath.

To have **New Agent…** and **Resume Agent Conversation…** open Claude Code, Codex, and OpenCode here instead of in a terminal, set **Open Agents In** on the Settings **Agents** page:

| Setting value | Config value | Effect |
| --- | --- | --- |
| Terminal | `terminal` | The agent opens in a terminal tab (default). |
| Agent View Beside Terminal | `split` | A new tab opens with the agent view beside its terminal. |
| Agent View Filling Tab | `full` | A new tab opens with the agent view filling it. |

In the agent view, both commands open the new-session page, where you pick an earlier conversation to continue. Choosing an agent profile always opens a terminal.

```toml
[agent_view]
open_agents_in = "split"
```

## Starting a session

The **+** (**New session**) button in the panel's header, or **New Agent View Session** in the command palette, opens the new-session page. When the tab already holds a conversation, the new session gets a tab of its own in the same folder. The page shows:

- the agent to start, and whether each is installed and turned on;
- the folder to work in: the focused terminal's directory, or another open terminal's from the folder menu;
- the agent's earlier conversations in that folder, which you can continue;
- **Open Claude in a terminal instead** (or Codex, or OpenCode), which starts the agent in a new terminal tab.

Type what the agent should work on and press Enter. The agent starts as soon as the page opens, so its commands, your account's models, and its earlier conversations are listed before you send anything.

Termeric looks for `claude`, `codex`, and `opencode` on its own `PATH` and then on the one your login shell sets up, so agents installed through a shell profile are found even when you start Termeric from your desktop. `program` and `env` under `[agents.claude-code]`, `[agents.codex]`, and `[agents.opencode]` apply here too; `args` and profiles apply only to terminal launches.

## The conversation

- **Messages** you send appear as cards. The agent's answers are formatted Markdown, including code blocks, tables, and links.
- **Reading and searching** fold into one *Explored* row that you can expand.
- **Edits** show a diff card with added and removed line counts.
- **Commands** show the command, its status, and the end of its output, which you can expand.
- **Sub-agents** show their task, how many steps they took, and their report.
- **To-do lists** and plan steps show as a checklist.
- **Thinking** shows as a collapsed *Thought* row.
- Each turn ends with its duration, the files it changed, **Review changes**, and a button that copies the answer.

The conversation follows new output until you scroll up; a button then jumps back to the latest message.

**Review changes** compares the folder's Git working tree with a snapshot taken when the session started and opens the same review as [Review Agent Changes…](agents.md#reviewing-an-agents-changes), where you can stage or revert each file.

## Approvals, questions, and plans

When the agent needs permission, a card under the tool call asks:

- **Allow** runs it once.
- **Always allow …** appears when the agent offers to remember the answer, such as allowing a command for the rest of the session or saving a rule to the agent's own settings.
- **Deny** refuses and ends the turn; the message box then asks what the agent should do instead.

A question from the agent shows its options as choices. **Other** lets you type your own answer, and **Skip** declines. A plan from Claude Code's *Plan only* mode offers to start, with or without asking before edits, or **Keep planning**.

A session that needs you while its tab is out of sight shows it on the tab and sends a desktop notification, following **Notify When Agents Need You**.

## The message box

| Key | Action |
| --- | --- |
| Enter | Send the message. |
| Shift+Enter | Start a new line. |
| Up, Down | On the first or last line, recall earlier messages. |
| Escape | Stop the running turn; with nothing to stop, give the keyboard back to the terminal. |
| Ctrl+A, Ctrl+C, Ctrl+X, Ctrl+V | Select all, copy, cut, paste. |
| Ctrl+Z, Ctrl+Shift+Z or Ctrl+Y | Undo, redo. |

A message you send while the agent is working waits as a *Queued* chip above the box and is sent when the turn ends; × on the chip cancels it and puts the text back. With the box empty, the send button becomes **Stop**.

Typing `/` at the start of a message lists the agent's commands, filtered as you type. Up and Down move through the list, Tab completes the highlighted command, Enter completes it (and sends it when it takes no argument), and Escape hides the list. Claude Code and OpenCode send their own lists, including your skills and custom commands. Codex has no list of its own, so Termeric offers `/compact`, `/review`, and your Codex skills. A command the agent only runs in its own interface offers **Open in a terminal**.

Chips above the box attach context from the terminal beside the panel: the **Terminal selection**, or the output of the **last command** when [shell integration](shell-integration.md) recorded one.

## Modes, models, and effort

Chips under the message box choose how the agent works. A change applies to the current session; for a new session, choosing another agent resets them to its defaults.

The permission chip picks the agent's mode:

| Agent | Modes |
| --- | --- |
| Claude Code | **Ask before edits** (default), **Edit without asking**, **Plan only**, **Auto** (Claude Code's safety classifier decides), **Deny unless allowed**, **Bypass permissions** |
| Codex | **Ask before changes** (default), **Auto** (edits and runs commands inside the folder), **Full access** |
| OpenCode | **Build** (default), **Plan**, and any agents you configured in OpenCode, each with an optional **Auto-approve** |

**Bypass permissions**, **Full access**, and OpenCode's **Auto-approve** run everything without asking. They are offered only after you turn on **Allow Bypass Permissions** on the Settings **Agents** page, and turning it off returns every session to asking. Use them only where nothing can be harmed, such as a container or a throwaway checkout.

```toml
[agent_view]
allow_bypass_permissions = true
```

OpenCode asks only where its own `permission` settings say to, and by default it edits and runs commands without asking. To approve them in the panel, set them to ask in `opencode.json`:

```json
{ "permission": { "edit": "ask", "bash": "ask", "webfetch": "ask" } }
```

The model chip lists the models your account can use with that agent; *Default* keeps the one the agent's own settings choose. OpenCode lists models from every provider you are signed in to, grouped by provider. When a model offers levels of effort, an effort chip appears beside it. Long menus have a search field: type to narrow the list, and Escape clears the search before closing the menu. The ring beside the send button shows how much of the model's context the conversation fills.

Codex applies a mode or model change with your next message; Claude Code and OpenCode apply it at once.

## Sessions and history

The **All sessions** button (a clock) in the panel's header opens **Agents**: this window's sessions in every tab, grouped into *Needs you*, *Working*, and *Recent*; agents running in terminals; the earlier conversations you can continue; and the supervisor, when one runs. Choosing a session goes to its tab. **Show Agents** in the command palette lists agent view sessions too.

The **⋯** menu offers:

- **Open in a terminal**, which ends the session here and continues the same conversation in a new terminal tab;
- **Copy conversation**;
- **End session**, which stops the agent and removes the session from the panel; the conversation stays in the agent's history and appears again under earlier conversations;
- **Hide panel**.

A conversation opened in a terminal stays readable in the panel but takes no more messages there, so two agent processes never write the same conversation.

## Keyboard use

The panel never takes over the window. A click in the panel, or Ctrl+Shift+A, gives it the keyboard; a click in a terminal, or Escape with nothing left to cancel, gives it back. While the panel has the keyboard, the terminal cursor turns dim and steady. Global shortcuts keep working, except the chords the message box uses for editing.

| Key | Action |
| --- | --- |
| Tab, Shift+Tab | Move between controls. |
| Enter, Space | Activate the focused control. |
| Page Up, Page Down | Scroll the conversation. |
| Typing | Returns focus to the message box. |

The panel appears to screen readers as an *Agent view* pane with its messages, buttons, and message box; only the visible part of the conversation is exposed.

## The supervisor

The [supervisor](agents.md#the-supervisor) is itself an agent view conversation in a tab of its own. It oversees other agent view sessions as well as agents in terminals: it can read a session's conversation, answer its approvals, and send it messages. In Auto mode it answers and sends on its own, and the conversation shows *Allowed by the supervisor* with its reason. In Assist mode its answer appears on the approval card as a suggestion for you, and its messages wait for your **Send** or **Don't send**. In Watch mode it does neither. A banner in each supervised conversation offers **Take over** and **Hand back**.

## Limits

- Claude Code, Codex, and OpenCode only. The supervisor runs Claude Code or Codex.
- No image attachments or file mentions.
- Sessions don't survive a restart of Termeric, though every conversation can be continued from the new-session page.
- Codex shows a command's output when the command ends, not while it runs, and doesn't show its plan-mode proposals.
- Claude Code's protocol is undocumented and Codex marks its server experimental, so an agent update can break the panel until Termeric follows.
