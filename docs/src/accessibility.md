# Accessibility

Termeric works with screen readers, can be used entirely from the keyboard, and can reduce motion and raise the contrast of its interface. By default it follows your desktop's accessibility settings.

## Screen readers

Termeric describes its interface to screen readers such as Orca through the standard Linux accessibility bus (AT-SPI). There is nothing to turn on: when a screen reader is running, it can read Termeric.

A screen reader can reach:

- the window, the tab list with the selected tab, each tab's close button, the New Tab button, the previous and next arrows when tabs overflow, and the Commands, Workspaces, and Settings buttons;
- each pane of the current tab, with the terminal text that's visible on screen, the cursor position, and any selection;
- new terminal output, which is announced politely as it appears;
- the command palette, Settings, the shortcuts editor, the Commands panel, the Workspaces manager, and other dialogs, including their fields, lists, buttons, and current values;
- the find bar, including its match count and toggle states, with an invalid regular expression announced right away;
- notifications, where errors are announced right away and other messages politely.

While a dialog such as the command palette is open, only the dialog is presented, so the screen reader doesn't wander into the terminal behind it.

Some things are deliberately left out:

- **Scrollback.** Only the rows visible on screen are exposed. To read earlier output, scroll it into view (`Shift+PageUp`, or the mouse wheel), or move through it with [keyboard copy mode](search-and-copy-mode.md#keyboard-copy-mode) or [find](search-and-copy-mode.md#find-text), both of which scroll the view as they go.
- **Inactive tabs.** Only the current tab's panes are exposed.
- **Hidden text.** Text a program marks as concealed, such as some password prompts, is presented as blank.

Screen-reader support has had only limited testing with Orca. If something is missing or confusing, please report it.

## Use Termeric without a mouse

Everything in Termeric can be reached from the keyboard.

- The **command palette** (`Ctrl+Shift+P`) lists every command, with its shortcut if it has one.
- **Settings** (`Ctrl+,`) is keyboard-driven: type to filter, `Tab` and `Shift+Tab` switch pages, `Up` and `Down` select a setting, `Left`, `Right`, or `Enter` change it, `Ctrl+S` saves, and `Escape` closes.
- **Keyboard copy mode** and **quick select** replace mouse selection and link clicking; see [Search, copy mode, and links](search-and-copy-mode.md).

### Tab bar and toolbar

Press `F6` to move keyboard focus from the terminal to the tab bar, or `Shift+F6` to start from the other end. A visible outline marks the focused control.

| Key | Action |
| --- | --- |
| `Tab` / `Shift+Tab`, or arrow keys | Move to the next / previous control |
| `Home` / `End` | Move to the first / last control |
| `Enter` or `Space` | Activate the focused control |
| `Escape` | Return focus to the terminal |

The controls are, in order: each tab followed by its close button, the previous and next arrows when tabs overflow, New Tab, Commands, Workspaces, and Settings. Clicking anywhere also returns focus to the terminal.

`F6` is reserved for this and can't be rebound. See [Keyboard and mouse](keyboard-and-mouse.md) for every other shortcut.

## Reduce motion and high contrast

Both settings live in **Settings → Accessibility** and in the `[accessibility]` table of your configuration file:

```toml
[accessibility]
reduce_motion = "system" # "system" (default), "on", or "off"
high_contrast = "system" # "system" (default), "on", or "off"
```

| Value | Meaning |
| --- | --- |
| `"system"` | Follow your desktop's setting. If the desktop doesn't provide one, the setting is off. |
| `"on"` | Always on, whatever the desktop says. |
| `"off"` | Always off, whatever the desktop says. |

Changes apply right away, including when your desktop setting changes while Termeric is running.

### Following your desktop

With `"system"`, Termeric reads the reduced-motion and contrast preferences your desktop publishes through the XDG desktop portal (`org.freedesktop.appearance`). If your desktop or portal doesn't publish one of them, that setting stays off unless you set it to `"on"` yourself.

### Reduce motion

Reduce motion makes Termeric's own interface change instantly instead of animating: panels, dialogs, menus, notifications, tab opening and closing, hover effects, the cursor gliding to a new position, and the flash when you copy.

It doesn't change what programs draw, and it doesn't stop signals you've asked for. To stop those too:

- turn off cursor blinking with `[cursor].blink = false`, or **Blink** in **Settings → Terminal → Cursor**;
- turn off the screen flash for the terminal bell with `[bell].visual = false`, or **Visual** in **Settings → Terminal → Bell**.

### High contrast

High contrast makes Termeric's interface opaque and raises its text contrast to at least 4.5:1 against its background: tabs, the toolbar, the find bar, Settings, the command palette, dialogs, menus, borders, and focus outlines.

It doesn't change the terminal's own colors or the colors programs use. For higher-contrast terminal text, choose a different theme; see [Themes and backgrounds](themes.md).

## Text size

| Key | Action |
| --- | --- |
| `Ctrl+=` | Make terminal text larger (2 points per step, up to 72) |
| `Ctrl+-` | Make terminal text smaller (down to 8 points) |
| `Ctrl+0` | Return to your configured size |

These keys change the window you're in. To change the size for every new window, set it in **Settings → Font → Size** or in your configuration file:

```toml
[font]
size = 14.0       # points; the default is 12
line_height = 1.4 # line spacing as a multiple of the font height, 0.8 to 3.0; the default is 1.2
```

A size you reach with the keys is written to the configuration file when you save your settings. Termeric's interface follows your desktop's display scaling. See [Fonts and text](fonts-and-text.md) for font choice and other text settings.

## Not yet supported

- Right-to-left scripts aren't reordered for display; Termeric lays out text left to right.
- Scrollback and inactive tabs aren't exposed to screen readers, as described above.
