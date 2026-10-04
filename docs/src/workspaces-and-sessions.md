# Workspaces and sessions

A workspace is a saved copy of one window's layout: its tabs, their split panes, where each pane was working, and what was on screen. Save one before you close a project, reopen it later, and your tabs and panes come back arranged the way you left them. Termeric can also save each window automatically when you quit and bring it back the next time you start.

Restoring a workspace starts fresh processes. Termeric launches your shell (or the tab's profile or agent) again in each pane and in its saved directory; it doesn't resume programs that were running. An editor, an `ssh` session, a build, or a shell job that was running when you saved is not restarted.

## What a workspace saves

For each tab in the window:

- its position in the tab bar, whether it's pinned, and any title you gave it (a tab or pane title longer than 256 characters is shortened);
- whether it was an ordinary shell tab, a [terminal profile](profiles.md), or a coding agent opened with **New Agent…**, so the same kind of process is relaunched (an agent starts a new conversation);
- its split layout, including the exact divider positions and which pane had focus;
- each pane's working directory and title;
- each pane's scrollback, with colors, when **Save Scrollback** is on.

The workspace also remembers which tab was active. Pane zoom and broadcast input are not saved; the full split layout is always what comes back. Images are not saved, only text.

Scrollback is capped at 5 MiB per pane and 50 MiB per workspace. When a pane has more than that, the most recent lines are kept and Termeric tells you after the save that some scrollback was trimmed.

A window attached to a [live domain](live-domains.md) can't save or open workspaces: in its Workspaces menu, those entries are shown disabled and marked **Live domain**. Live domains keep their own processes and layout running, so there is nothing to relaunch.

## Save a workspace

Use either of these:

- Click the **Workspaces** button (the down arrow beside the Settings gear in the tab bar) and choose **Save Workspace** or **Save Workspace As…**.
- Open the command palette (Ctrl+Shift+P) and run either command.

**Save Workspace** saves the window back into the workspace it belongs to, the one it last opened or saved. A window that doesn't belong to one yet gets a new workspace named after the active tab: with a tab titled "termeric" active, it's called "termeric workspace". **Save Workspace As…** always creates a new workspace, named the same way, and the window belongs to that one from then on. Rename a workspace in the workspace manager. A toast confirms the save and shows the workspace's ID.

Saving a window restored from an automatic "Last workspace" makes it yours: it takes a name from the active tab, like a new workspace, and is kept for good.

## Open a workspace

- The **Workspaces** menu lists your six most recently saved workspaces with their tab counts. The one this window last opened or saved has a check mark. Click one to open it.
- **Manage Workspaces…** (in the same menu or the command palette) lists every workspace, newest first.
- From a shell, `termeric --workspace ID` opens a window with that workspace, and `termeric workspace list` prints every workspace's ID. `--workspace` can't be combined with tab options such as `--tab`, `--command`, or `--working-directory` for the same window. See [Command line](command-line.md#termeric-workspace-list).

Opening a workspace replaces the tabs in the current window, and the programs in those tabs end. What happens to the window first:

- If it belongs to a workspace and **Auto Save Workspace** is on, it's saved back into that workspace, as quitting would, and the other workspace opens.
- Otherwise, if any tab runs a program besides an idle shell (a command started at the prompt, a terminal profile, or an agent), Termeric asks before closing them: choose **Close N tabs and open…** or **Cancel**.
- A window of idle shells opens the workspace right away.

When you quit, the window is saved back into the workspace it last opened or saved. Opening "Project A", working for a while, and quitting updates "Project A". A window that never opened or saved a workspace is saved as "Last workspace" instead. Termeric keeps only the "Last workspace" entries from your most recent quit, the ones it can reopen; renaming one keeps it for good.

## Restore windows when Termeric starts

With **Auto Restore Workspace** on, starting Termeric with no window or tab options reopens every window that was open when you last quit, each with its own workspace. Options that only change how the process runs, `--separate-process` and `--renderer`, still restore. Starting with options that ask for a window or tabs of their own, such as `--new-window`, `--tab`, `--command`, or `--workspace`, skips the automatic restore.

If you close one window while others stay open, that window is not reopened next time, and unless it belongs to a workspace you opened or saved, nothing is saved for it. If a saved workspace can't be restored at startup, Termeric opens a normal shell and shows an error. With `--workspace`, a workspace that can't be restored stops Termeric from starting instead.

Automatic saving happens when a window closes with tabs still open. If you end the last shell in a window (for example by typing `exit`), the window closes with nothing left to save, and its workspace keeps whatever was saved before.

## Manage workspaces

**Manage Workspaces…** opens a panel below the tab bar. Each row shows the workspace name, its tab and pane counts, and when it was last saved. Hover or select a row to see its **Open**, **Rename**, and **Delete** buttons; double-click a row to open it.

| Key | Action |
| --- | --- |
| Up / Down | Select a workspace |
| Enter | Open the selected workspace |
| F2 | Rename the selected workspace |
| Delete | Delete the selected workspace |
| Escape | Close the panel, or cancel a rename |

While renaming, Enter saves the new name and Escape cancels. Names can be up to 128 characters. Deleting a workspace or tab snapshot asks for confirmation first, then removes it from disk; there's no undo.

A workspace that can't be read stays in the list, marked as unavailable, so you can delete it.

The **Workspaces** menu also works from the keyboard: press F6 to move focus to the window controls, select the Workspaces button, and press Enter. In the menu, Up, Down, Home, and End move between entries, Enter or Space chooses one, and Escape closes it.

## When a restore can't bring everything back

Termeric restores as much as it can. A toast says how many items need attention, and the details are written to Termeric's log (see [Troubleshooting](troubleshooting.md#collect-logs)):

- If a pane's saved directory no longer exists, that pane starts in your home directory instead.
- If a tab used a terminal profile that has since been removed from your configuration, or an agent or program that's no longer installed, the whole tab is skipped. The other tabs still open.
- If the workspace was saved with a different theme or shell than you use now, Termeric keeps your current settings and says so.
- If none of the tabs can be relaunched, the current window is left as it was.

Workspaces store profile and agent names rather than copies of their settings. If you edit a profile, the next restore uses the edited version.

## Where workspaces are stored

Each workspace is a folder under `~/.local/share/termeric/workspaces/`, named with its ID. The folder holds `workspace.toml` (the layout), a backup of the previous version that Termeric falls back to if the main file is damaged, and a `scrollback/` folder with the saved text. A folder whose name isn't the ID in its `workspace.toml`, such as a copy of one, is treated as an unreadable workspace and never mistaken for the original.

Saved scrollback is ordinary text on disk and can contain anything that was printed in your terminals. Turn **Save Scrollback** off if you don't want terminal output kept in these files.

## Settings

These are on the **Sessions** page in Settings, and in the `[session]` section of `config.toml`. See [Configuration](configuration.md).

| Setting | Config key | Default | Description |
| --- | --- | --- | --- |
| Auto Save Workspace | `session.auto_save` | on | Save each window's workspace when it closes, and save a tab snapshot when a tab closes |
| Auto Restore Workspace | `session.auto_restore` | off | Reopen the windows from your last session when Termeric starts with no arguments |
| Save Scrollback | `session.save_scrollback` | on | Include terminal text in workspaces and tab snapshots |
| — | `session.enabled` | `true` | Set to `false` to turn off all automatic saving and restoring; **Save Workspace** still works |

```toml
[session]
auto_save = true
auto_restore = true
save_scrollback = true
```

## Tab snapshots

When **Auto Save Workspace** is on and you close a tab where any pane has output or ran a command, Termeric saves a snapshot of that tab, the way it saves a workspace: every pane with its working directory, last command, and scrollback, and the split layout. It is named for the focused pane's directory (or the tab's title) and last command.

To reopen one, open **Manage Workspaces…** and click **Tab snapshots** at the bottom of the panel (it appears once there are snapshots). Opening a snapshot reopens the tab in the current window, as restoring a workspace does: fresh shells (or the tab's profile or agent) in the saved directories and splits, with the saved text above each prompt. A tab that was pinned reopens pinned, after your other pinned tabs. It doesn't rerun the last command, and closing the reopened tab updates the same snapshot. Press Escape or the back button to return to the workspace list, and Delete to remove a snapshot you don't need. Termeric keeps the 100 newest snapshots and deletes older ones.

Snapshots are stored with the workspaces, under `~/.local/share/termeric/workspaces/`.

## Agent transcripts and session export

For a terminal running a [coding agent](agents.md), Termeric can record a transcript and export the session. Both commands appear in the command palette only while the focused terminal is running an agent.

- **Toggle Agent Transcript** starts or stops recording that terminal's output and what you type into it. Transcripts are JSON Lines files in `~/.local/share/termeric/sessions/transcripts/`. Common token formats are redacted as they're recorded, but this isn't a complete secret scanner.
- **Export Agent Session** writes a Markdown copy of the session. If a transcript is recording, you get `transcript.md` and `transcript.jsonl` built from everything recorded so far, and recording continues. Otherwise you get `transcript.md` from the terminal's scrollback. Each export goes in a new `transcript-export-*` folder under `~/.local/share/termeric/sessions/`, and the path to the Markdown file is copied to your clipboard.

Review exports before sharing them; they contain whatever the terminal showed.
