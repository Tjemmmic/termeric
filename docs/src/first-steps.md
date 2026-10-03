# First steps

This page walks you through your first few minutes with Termeric: starting it, finding its two main tools (the command palette and Settings), opening tabs and panes, and making it look the way you like.

## Launch Termeric

Open **Termeric** from your desktop's application menu, or run `termeric` from any terminal.

If Termeric is already running on the same desktop, launching it again opens another window in the running Termeric rather than starting a second copy. Keeping every window in one process is what lets you drag tabs between windows. To open another window from the keyboard or mouse, use **New Window** in the command palette or in the launcher's right-click menu.

Each new tab starts your default shell: the one named in `$SHELL`, or your account's login shell if `$SHELL` isn't set. To run a different shell, set `program` under `[shell]` in your configuration file (see [Configuration](configuration.md)):

```toml
[shell]
program = "/usr/bin/fish"
```

Bash, Zsh, Fish, and Nushell get Termeric's shell integration automatically, which enables prompt jumping, failed-command navigation, and new panes that open in your current folder. See [Shell integration](shell-integration.md), including the one line Elvish needs.

## The command palette

Press **Ctrl+Shift+P** to open the command palette. Start typing to filter every command Termeric offers, use the arrow keys to pick one, and press **Enter** to run it. Commands that have a keyboard shortcut show it beside their name, so the palette is also a good way to learn shortcuts. Press **Escape** to close it.

If you remember only one shortcut, make it this one.

## Settings

Press **Ctrl+,** or click the gear icon in the top bar to open Settings. Pages in the sidebar group the options: Appearance, Font, Terminal, Keyboard, Agents, Sessions, Accessibility, Plugins, and Experimental.

Changes apply as soon as you make them, so you can try things out. To keep them, press **Ctrl+S** or click **Save**; until then the dialog shows "Unsaved changes". Settings are stored in `~/.config/termeric/config.toml`, and editing that file directly works too: Termeric reloads it when you save it.

## Tabs and panes

Click **+** in the tab bar or press **Ctrl+Shift+T** to open a tab, and press **Ctrl+Shift+W** to close it. **Ctrl+Tab** and **Ctrl+Shift+Tab** move between tabs.

Each tab can be split into panes, each running its own shell. **Alt+Shift+Right** and **Alt+Shift+Down** split the focused pane, **Alt** with an arrow key moves to the pane in that direction, and **Alt+Shift+W** closes a pane. [Keyboard and mouse](keyboard-and-mouse.md#default-key-bindings) lists every tab and pane shortcut.

You can also click a pane to focus it and drag the divider between panes to resize them. Typing `exit` in a shell closes its pane or tab as usual. Renaming, pinning, and reordering tabs, zooming and rearranging panes, and moving tabs between windows are covered in [Tabs, panes, and windows](tabs-panes-windows.md).

## Copy, paste, and find

Press **Ctrl+Shift+C** to copy the selection and **Ctrl+Shift+V** to paste. **Ctrl+Shift+F** finds text in the scrollback, and **Shift+Page Up** and **Shift+Page Down** scroll it.

Select text by dragging with the mouse; double-click selects a word and triple-click a line. Hold **Ctrl** and click a link to open it. When a program in the terminal uses the mouse itself, hold **Shift** to select text anyway. See [Search, copy mode, and links](search-and-copy-mode.md) and [Keyboard and mouse](keyboard-and-mouse.md).

## Change the theme

Run **Change Theme…** from the command palette. As you move through the list, each theme is previewed live on your terminal. Press **Enter** to keep the highlighted theme (it's saved right away) or **Escape** to go back to the one you had. You can also pick a theme under **Appearance** in Settings.

The first twelve themes are Termeric's own, named after roots: Turmeric (the default), Ginger, Daikon, Galangal, and others. The rest are well-known community themes. To make your own, see [Themes and backgrounds](themes.md).

## Change the font size

**Ctrl+=** makes the text larger, **Ctrl+-** smaller, and **Ctrl+0** returns it to your configured size. The keys change the size in the current window in steps of two points. To choose an exact size, or a different font family, open the **Font** page in Settings. The default is 12 points in your system's default monospace font. Either way, press **Ctrl+S** in Settings (or run **Save Configuration** from the palette) to keep the new size for future windows. See [Fonts and text](fonts-and-text.md) for fallback fonts, ligatures, and line height.

## Where to go next

- [Keyboard and mouse](keyboard-and-mouse.md) lists every shortcut and shows how to change them.
- [Tabs, panes, and windows](tabs-panes-windows.md) covers layouts, zoom, broadcast input, and moving tabs.
- [Workspaces and sessions](workspaces-and-sessions.md) shows how to save and restore a whole window.
- [Configuration](configuration.md) explains `config.toml`.
- [Agents in terminals](agents.md) covers Claude Code, Codex, and other coding agents.
- [Troubleshooting](troubleshooting.md) helps when something doesn't work.
