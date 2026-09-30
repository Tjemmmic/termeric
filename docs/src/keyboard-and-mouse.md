# Keyboard and mouse

This page lists Termeric's default key bindings, shows how to change them, and explains how the mouse, clipboard, and input methods behave.

Termeric's shortcuts are checked before keys reach the program in the terminal, so a key bound here doesn't also reach your shell. The exceptions are Copy and Clear Selection: with nothing selected, their keys go to the program. Most defaults use `Ctrl+Shift` so that plain `Ctrl` combinations stay with your programs.

## Default key bindings

The **ID** column is the name to use in your configuration file; see [Change key bindings](#change-key-bindings).

### General

| Key | Action | ID |
| --- | --- | --- |
| `Ctrl+Shift+C` | Copy the selection. With nothing selected, the key goes to the program instead. | `copy` |
| `Ctrl+Shift+V` | Paste | `paste` |
| `Ctrl+Shift+F` | Find | `toggle_search` |
| `Ctrl+Shift+P` | Command palette | `toggle_command_palette` |
| `Ctrl+,` | Open Settings | `toggle_settings` |
| `F11` | Toggle fullscreen | `toggle_fullscreen` |
| `Escape` | Clear the selection. With nothing selected, the key goes to the program. | `clear_selection` |
| `Ctrl+Shift+I` | New agent (see [Agents in terminals](agents.md)) | `new_agent` |
| `Ctrl+Shift+A` | Open or close the [agent view](agent-view.md) (experimental) | `toggle_agent_view` |
| none | Insert a Unicode character by name | `open_unicode_picker` |

### Tabs

| Key | Action | ID |
| --- | --- | --- |
| `Ctrl+Shift+T` | New tab | `new_tab` |
| `Ctrl+Shift+W` | Close tab | `close_tab` |
| `Ctrl+Tab` | Next tab | `next_tab` |
| `Ctrl+Shift+Tab` | Previous tab | `prev_tab` |
| `Ctrl+1` … `Ctrl+9` | Go to tab 1–9 | `switch_to_tab_1` … `switch_to_tab_9` |
| none | Rename tab | `rename_tab` |
| none | Move tab left / right | `move_tab_left`, `move_tab_right` |
| none | Move tab to next window | `move_tab_to_next_window` |
| none | Pin or unpin tab | `toggle_pin_tab` |

### Panes

| Key | Action | ID |
| --- | --- | --- |
| `Alt+Shift+Right` | Split pane right | `split_vertical` |
| `Alt+Shift+Down` | Split pane down | `split_horizontal` |
| `Alt+Shift+W` | Close pane | `close_pane` |
| `Alt+Up` / `Alt+Down` / `Alt+Left` / `Alt+Right` | Focus the pane in that direction | `navigate_pane_up`, `navigate_pane_down`, `navigate_pane_left`, `navigate_pane_right` |
| none | Zoom pane | `toggle_pane_zoom` |
| none | Toggle pane read-only | `toggle_pane_read_only` |
| none | Swap pane with next | `swap_pane_next` |
| none | Balance panes | `balance_panes` |
| none | Move pane to next tab / next window | `move_pane_to_next_tab`, `move_pane_to_next_window` |
| none | Layout: columns, rows, grid, tall, fat | `layout_columns`, `layout_rows`, `layout_grid`, `layout_tall`, `layout_fat` |
| none | Cycle the directory new panes start in | `cycle_pane_cwd_mode` |
| none | Toggle broadcast input | `toggle_broadcast` |

What these do is described in [Tabs, panes, and windows](tabs-panes-windows.md).

### Navigation

| Key | Action | ID |
| --- | --- | --- |
| `Ctrl+Shift+X` | Keyboard copy mode | `toggle_copy_mode` |
| `Ctrl+Shift+U` | Quick select links and matches | `quick_select` |
| `Ctrl+Shift+Up` | Jump to the previous prompt | `previous_prompt` |
| `Ctrl+Shift+Down` | Jump to the next prompt | `next_prompt` |
| none | Jump to the previous / next failed command | `previous_failed_command`, `next_failed_command` |

Prompt and failed-command jumps need [shell integration](shell-integration.md). Copy mode and quick select are covered in [Search, copy mode, and links](search-and-copy-mode.md).

### Scrolling

| Key | Action | ID |
| --- | --- | --- |
| `Shift+PageUp` | Scroll up 10 lines | `scroll_page_up` |
| `Shift+PageDown` | Scroll down 10 lines | `scroll_page_down` |
| `Ctrl+Shift+Home` | Scroll to the top of the scrollback | `scroll_to_top` |
| `Ctrl+Shift+End` | Scroll to the bottom | `scroll_to_bottom` |

### Font

| Key | Action | ID |
| --- | --- | --- |
| `Ctrl+=` | Increase font size | `increase_font_size` |
| `Ctrl+-` | Decrease font size | `decrease_font_size` |
| `Ctrl+0` | Reset font size | `reset_font_size` |

### Terminal

| Key | Action | ID |
| --- | --- | --- |
| none | Clear scrollback | `clear_scrollback` |
| none | Reset terminal | `reset_terminal` |

### Fixed keys

A few keys aren't part of the shortcut list and can't be changed:

| Key | Action |
| --- | --- |
| `F6` / `Shift+F6` | Move keyboard focus to the tab bar and toolbar; see [Accessibility](accessibility.md#use-termeric-without-a-mouse) |
| `Escape` | Cancel broadcast input while it's armed or active |

Search, copy mode, the command palette, Settings, and other panels have their own keys while they're open; each is described with its feature.

## Change key bindings

### In the shortcuts editor

Open the editor with **Keyboard Shortcuts** in the command palette, or from **Settings → Keyboard → Keyboard Shortcuts**.

1. Type in the filter box to find an action, and move to it with `Up` and `Down`.
2. Press `Enter` (or click the row) to record, then press the new key combination.
3. To remove a binding, select the row and press `Delete`, or press `Backspace` or `Delete` while recording. Press `Escape` to cancel recording. Recording also stops after 15 seconds.

If the combination already belongs to another action, it moves to the one you're editing and the other action is left unbound; the editor tells you which one.

Changes take effect immediately. They're written to your configuration file when you save, with **Save** in Settings or **Save Configuration** in the command palette.

`Escape`, `Backspace`, and `Delete` can't be recorded as shortcuts, even with modifiers.

### In the configuration file

Each changed binding is a `[[shortcuts]]` entry in `~/.config/termeric/config.toml`. Only bindings that differ from the defaults need an entry.

```toml
# Zoom the focused pane with Ctrl+Shift+Z
[[shortcuts]]
action = "toggle_pane_zoom"
key = "z"
ctrl = true
shift = true

# Remove the default Alt+Left binding so the key reaches your shell
[[shortcuts]]
action = "navigate_pane_left"
```

| Field | Meaning |
| --- | --- |
| `action` | The action's ID from the tables above |
| `key` | The key name; leave it out to unbind the action |
| `ctrl`, `alt`, `shift`, `super` | Modifiers that must be held; each defaults to `false` |

Key names are case-insensitive. Use the character for letter, digit, and symbol keys (`"z"`, `"5"`, `"/"`), and these names for other keys: `enter`, `tab`, `space`, `backspace`, `escape`, `delete`, `insert`, `home`, `end`, `pageup`, `pagedown`, `up`, `down`, `left`, `right`, `f1` to `f12`.

Shortcuts match the character your keyboard layout produces, not the physical key. For a symbol typed with Shift, use the shifted character, for example `key = "?"` with `shift = true`.

Reload with **Reload Configuration** in the command palette after editing the file. See [Configuration](configuration.md) for where the file lives.

## Keys that type text or run commands

`[[keybindings]]` entries bind a key to text or a command instead of a built-in action:

```toml
# Ctrl+Alt+K types a command without running it
[[keybindings]]
key = "k"
modifiers = { ctrl = true, alt = true }
action = { type = "send_text", text = "kubectl get pods" }

# Ctrl+F5 clears the prompt line, types the command, and presses Enter
[[keybindings]]
key = "F5"
modifiers = { ctrl = true }
action = { type = "run_command", command = "make test" }
```

| Action type | What it does |
| --- | --- |
| `send_text` | Sends `text` to the focused terminal exactly as written. Add `\r` to press Enter. |
| `run_command` | Clears the current prompt line, types `command`, and presses Enter. |
| `plugin_command` | Runs a plugin's command by its `command_id`; see [Plugins](plugins.md). |

The modifiers must match exactly. If a combination is also a built-in shortcut, the built-in shortcut wins.

### Saved commands

`[[commands]]` entries save commands you run often. They appear in the Commands panel (the `›_` button in the toolbar), where you can also add, run, and delete them.

```toml
[[commands]]
name = "SSH production"
command = "ssh admin@prod.example.com"
alias = "prod"
```

Typing a saved command's name or alias as the first word at your prompt and pressing Enter replaces it with the full command; anything you typed after it is kept.

## Mouse

### Select and copy

| Action | Selects |
| --- | --- |
| Drag | Text from where you pressed to where you release |
| Double-click | A word |
| Triple-click | A line |
| `Alt`+drag | A rectangular block |

Selecting text doesn't copy it. Press `Ctrl+Shift+C` to copy the selection to the clipboard, and `Escape` to clear it. Clicking in the terminal also leaves keyboard copy mode.

`Ctrl+Shift+V` pastes the clipboard into the focused pane. If the program has turned on bracketed paste, the pasted text is marked as a paste, so shells and editors don't run it line by line as it arrives.

Termeric doesn't use the X11/Wayland primary selection for mouse selections: selecting doesn't fill it, and middle-click doesn't paste it.

### Links

Hold `Ctrl` and click a link to open it with your desktop's default handler (`xdg-open`). This works for links programs mark with OSC 8 hyperlinks and for plain `http`, `https`, `ftp`, `ftps`, `file`, `mailto`, `ssh`, and `git` URLs in the output. See [Search, copy mode, and links](search-and-copy-mode.md#links) for more.

### Programs that use the mouse

Programs such as `vim`, `htop`, and `tmux` can ask for mouse input. While they do, clicks, drags, and the wheel go to the program. Hold `Shift` to select text or scroll the scrollback yourself instead; `Alt+Shift`+drag selects a block. `Ctrl+Click` on a link still opens it.

Middle and right clicks are sent only to programs that ask for mouse input; otherwise Termeric ignores them.

### Scroll

The mouse wheel scrolls the scrollback of the focused pane; click a pane first to scroll it. `[scrollback].scroll_speed` sets how many lines each mouse-wheel step moves (default `6`).

In full-screen programs such as `less` that request it, the wheel sends arrow keys instead. Hold `Shift` to scroll the scrollback.

When a pane has scrollback, a scrollbar appears along its right edge; drag it to scroll.

### Panes and tabs

Click a pane to focus it, and drag the divider between panes to resize them. Drag tabs to reorder them or move them between windows; see [Tabs, panes, and windows](tabs-panes-windows.md).

To paste a file's path into the terminal, drag the file from your file manager onto the window; see [Images and file transfer](images-and-files.md).

## Clipboard

Copy and paste use the desktop clipboard on both Wayland and X11.

Programs in the terminal can also use the clipboard through the OSC 52 escape sequence, which is how `tmux`, `vim`, and similar tools copy over SSH. By default Termeric lets programs write to the clipboard but not read it, so a program can't quietly read what you copied elsewhere:

```toml
[security.osc52]
allow_write = true       # programs may copy to the clipboard
allow_read = false       # programs may not read the clipboard
max_bytes = 1048576      # largest clipboard text accepted either way (1 MiB)
```

A denied read gets an empty reply, so the program doesn't hang waiting.

## Typing text

### Input methods and compose

Termeric works with your desktop's input method (such as IBus or Fcitx 5) and dead keys. Text you're still composing is drawn at the cursor and isn't sent to the program; only the finished text is sent. Composition works the same way in the find box, the command palette, Settings, and rename fields. Switching panes or windows cancels an unfinished composition.

### Alt and AltGr

`Alt` combined with a key sends the key with an Escape prefix, which shells and editors read as Meta. Characters typed with `AltGr` are sent as the characters themselves, so layouts that use AltGr for `@`, `{`, or accented letters work normally, and `Ctrl+Alt` stays a separate combination.

Programs can ask for richer keyboard reporting, such as the Kitty keyboard protocol; see [Terminal compatibility](compatibility.md).

### Insert a Unicode character

Run **Insert Unicode Character…** from the command palette, search by name or code point, and press `Enter` to type the character into the focused terminal. It has no default key; bind `open_unicode_picker` if you use it often.
