# Tabs, panes, and windows

Termeric gives you three ways to arrange terminals: tabs in a window, panes that split a tab, and several windows that share one process. Tabs and panes can move between windows without restarting the programs running in them.

Most of the commands on this page are in the command palette (`Ctrl+Shift+P`). Commands without a default key can be bound in the shortcuts editor; see [Keyboard and mouse](keyboard-and-mouse.md).

## Tabs

`Ctrl+Shift+T` opens a tab and `Ctrl+Shift+W` closes the current one. `Ctrl+Tab` and `Ctrl+Shift+Tab` go to the next and previous tab, and `Ctrl+1` to `Ctrl+9` go to a tab by its position.

You can also click **+** in the tab bar to open a tab, click a tab to select it, and click its **×** to close it. The close button shows on the active tab and on the tab under the pointer. An inactive tab that has printed new output shows a small dot in place of its close button.

Closing a tab closes every pane in it and ends its programs; there is no confirmation. Closing the last tab closes the window.

### Rename a tab

Double-click the tab, press `Ctrl+Alt+Shift+T`, or run **Rename Tab** from the command palette. Type the new title, then press `Enter` to save or `Escape` to keep the old one. Saving an empty title removes your name and lets the tab show its automatic title again.

A title you set always wins. Otherwise the tab shows the title the running program sets, or else the current directory or program.

### Reorder and pin tabs

Drag a tab along the tab bar to move it. **Move Tab Left** and **Move Tab Right** do the same from the keyboard.

**Pin or Unpin Tab** keeps a tab at the start of the bar and marks it with a pin icon. Pinned tabs stay together: dragging or moving a tab never crosses between the pinned and unpinned groups, so to change a tab's group, pin or unpin it.

### Tab bar position

The tab bar sits along the top by default. To use a vertical rail on the left instead, set:

```toml
[window]
tab_bar_position = "left" # "top" (default) or "left"
```

The same choice is in **Settings → Appearance → Window → Tab Bar Position**, and it applies to every open window immediately. With the left rail, the Commands, Workspaces, and Settings buttons stay in the top toolbar.

When there are more tabs than fit, previous and next arrows appear at the ends of the bar and the bar scrolls to keep the active tab in view. While you drag a tab, hold it over one of these arrows to keep moving it toward the hidden tabs.

### Tab markers

A small icon before a tab's title shows its most important state, in this order: broadcast input active, broadcast armed, read-only, pinned. See [Broadcast input](#broadcast-input) and [Read-only panes](#read-only-panes).

## Panes

Panes split one tab into several terminals, each running its own shell.

`Alt+Shift+Right` splits the focused pane with the new pane on the right, and `Alt+Shift+Down` puts it below. `Alt+Shift+W` closes the focused pane, and `Alt` with an arrow key focuses the pane in that direction.

Click a pane to focus it. When a tab has more than one pane, the focused pane has a thin outline. Keyboard input always goes to the focused pane.

> `Alt+Arrow` is also used by some shells and editors. If you need those keys in your programs, rebind the pane shortcuts in the shortcuts editor.

To resize panes, drag the divider between them.

### Where new panes start

A new pane starts in a working directory chosen by `[panes].cwd`:

| Value | New panes start in |
| --- | --- |
| `"focused"` (default) | The focused pane's current directory |
| `"tab"` | The directory of the tab's first pane |
| `"process"` | The directory Termeric itself was started in |

**Cycle New Pane Directory** switches between these and saves your choice. The current directory comes from your shell's directory reports when [shell integration](shell-integration.md) provides them, and from the running process otherwise.

### Zoom, swap, and layouts

| Command | What it does |
| --- | --- |
| **Zoom Pane** | Makes the focused pane fill the tab. Run it again to restore the layout. A zoomed pane has an accent outline. |
| **Swap Pane with Next** | Exchanges the focused pane with the next one. |
| **Balance Panes** | Makes every split in the tab even. |
| **Layout: Columns** / **Layout: Rows** | Arranges all panes side by side or stacked. |
| **Layout: Grid** | Arranges all panes in a grid. |
| **Layout: Tall** | Puts the focused pane on the left at full height and half width, and tiles the others on the right. |
| **Layout: Fat** | Puts the focused pane across the top at full width and half height, and tiles the others below. |

None of these restart the programs in the panes. Layout commands need at least two panes.

### Move panes

**Move Pane to Next Tab** takes the focused pane out of its tab and adds it to the next tab. **Move Pane to Next Window** adds it to the active tab of the next window. The program in the pane keeps running. A tab's only pane can't be moved; move the whole tab instead.

### Broadcast input

Broadcast sends what you type and paste to several panes of the current tab at once, for example to run the same command on several servers. Because it's easy to send something to the wrong place, turning it on takes two steps:

1. Run **Toggle Broadcast Input**. Every writable pane in the tab is selected and outlined in amber, and nothing you type is sent anywhere while broadcast is armed.
2. Change the selection if you want: click a pane, or focus it with `Alt+Arrow` and press `Space`, to add or remove it.
3. Press `Enter` or run **Toggle Broadcast Input** again to start. The target panes are outlined in red.

You have 10 seconds to confirm before arming expires; each change to the selection restarts the timer. Broadcast needs at least two target panes.

Press `Escape` or run **Toggle Broadcast Input** to stop. Switching to another tab also stops it. Each pane receives keys encoded for its own terminal modes, so programs in different states still get the right input. Mouse input is never broadcast.

### Read-only panes

**Toggle Pane Read-only** locks the focused pane against your input: typing, pasting, dropped files, and mouse reports are all blocked. Output keeps arriving, and you can still select, copy, and search. A read-only pane has a teal outline and its tab shows a padlock. Read-only panes are skipped by broadcast input.

The lock lasts until you toggle it off or close the pane. It isn't saved with workspaces, so a restored workspace always starts unlocked.

## Windows

To open another window, run **New Window** from the command palette, or start `termeric` again from a launcher or shell.

Each window has its own tabs. Use **Toggle Fullscreen** (`F11`) to make the current window fullscreen.

### Move tabs between windows

- Drag a tab out of its window and release it over another Termeric window. It's inserted where you drop it on the tab bar, or at the end if you drop it elsewhere in the window.
- Release a dragged tab away from every Termeric window, for example over the desktop, to tear it off into a window of its own. A window's only tab can't be torn off, since it already has a window.
- Run **Move Tab to Next Window** to send the active tab to the next window, in the order the windows were opened.

The tab keeps its panes, programs, title, and pin state. A window that loses its last tab closes.

On Wayland, Termeric learns where a tab was dropped from the window the pointer enters after you release. If no Termeric window sees the pointer within about half a second, the tab tears off into a new window.

Tabs attached to a [live domain](live-domains.md) keep their place in the domain and can't be dragged out of their window. A window attached to a live domain holds only that domain's tabs, so tabs from other windows can't be moved into it either; Termeric says so and leaves the tab where it was.

### Windows share one process

Tabs and panes can move only between windows of the same Termeric process. By default, starting `termeric` while it's already running on the same display opens the new window in the running process, so all your windows share it. The new window uses the running process's settings and environment, and a tab with no `--working-directory` opens in the directory you launched from.

A launch starts a process of its own instead when:

- you pass `--separate-process`;
- you set `[window].share_process = false`;
- it asks for something a running process can't provide for one window, such as `--workspace`, a terminal or agent profile, `--split`, `--class`/`--app-id`, `--geometry`, `--renderer`, several windows, or `--quick-access`;
- the running Termeric is a different build, for example an installed release beside a development build;
- the launch sets a variable that shapes Termeric itself to a different value than the running process has, such as `RUST_LOG`, `WGPU_BACKEND`, `TERMERIC_TERM`, or `XDG_CONFIG_HOME` (see [Command line](command-line.md#joining-the-running-termeric) for the full list).

Windows in separate processes can't exchange tabs. See [Command line](command-line.md) for every launch option.

### Keep a window above or below others

To ask the desktop to keep windows above or below other windows, set:

```toml
[window]
level = "always_on_top" # "normal" (default), "always_on_top", or "always_on_bottom"
```

This is **Window Level** in **Settings → Appearance → Window**, and it applies right away. Your compositor or window manager has the final say and may ignore the request; many Wayland compositors do.

## Quick-access window

The quick-access window is a drop-down terminal you show and hide with a global shortcut. Bind this command to a key in your desktop's keyboard settings:

```bash
termeric --quick-access
```

The first time it runs, it opens a borderless, always-on-top window centered along the top edge of your primary monitor, 80% of its width and 45% of its height. Each later run hides the window if it's visible and focused, and shows and focuses it otherwise. Its tabs and programs keep running while it's hidden, and it never hides by itself when it loses focus.

You can choose what the window starts with; these options only apply when the window is first created:

```bash
termeric --quick-access --working-directory ~/src
termeric --quick-access --terminal-profile dev
termeric --quick-access --geometry 1400x620
```

There is only one quick-access window. `--quick-access` can't be combined with `--new-window`. The quick-access window isn't included in automatic workspace saves; use **Save Workspace** if you want to keep its layout.

The window's application ID (X11 `WM_CLASS`) is `termeric-quick-access`. Wayland compositors may ignore its requested position or refuse to focus it; if so, add a window rule for that application ID in your compositor's configuration.

## Related pages

- [Workspaces and sessions](workspaces-and-sessions.md): save and restore windows, tabs, and panes.
- [Keyboard and mouse](keyboard-and-mouse.md): every shortcut and how to change it.
- [Command line](command-line.md): open tabs, splits, and windows from the shell.
