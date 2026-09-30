# Search, copy mode, and links

Termeric can search everything a terminal has printed, let you select and copy text without touching the mouse, and open links and file paths from the output. All of these work on the focused pane.

## Find text

Press `Ctrl+Shift+F` to open the find box, then type. Matches are highlighted as you type, across the visible screen and the whole scrollback of the focused pane, and the box shows the current match number and the total. On very long scrollback the count fills in over a moment.

| Key (in the find box) | Action |
| --- | --- |
| `Down` / `Up` | Go to the next / previous match |
| `Enter` | Close the box and keep the matches highlighted |
| `Escape` | Close the box and clear the highlights |
| `Alt+C` | Toggle case-sensitive matching |
| `Alt+R` | Toggle regular expressions |
| `Alt+F` | Toggle filter mode |
| `Left`, `Right`, `Home`, `End`, `Backspace`, `Delete` | Edit the query; add `Ctrl` to move or delete by word |
| `Ctrl+Shift+V` | Paste into the query |

The same toggles, previous and next buttons, and a close button are on the find bar, so you can also use them with the mouse. After you press `Enter`, the bar stays visible with its buttons while the highlights remain; click its close button to clear them, or click the query to edit it again.

Searches ignore case and match literal text by default. With regular expressions on, the query uses [Rust regex syntax](https://docs.rs/regex/latest/regex/#syntax), and an invalid pattern shows its error under the query.

### Filter mode

Filter mode hides every line that doesn't match, so you see only the matching lines, in order. Scroll with the mouse wheel to move through them. Selecting and copying in the filtered view copies only the matching lines you selected. Turn filter mode off, or close the find box, to see everything again.

## Keyboard copy mode

Copy mode puts a cursor in the terminal that you move with the keyboard, so you can select and copy anything on screen or in the scrollback without the mouse. Keys you press in copy mode never reach the program.

Press `Ctrl+Shift+X` to start. The cursor starts at the terminal's cursor, or at the bottom-left of the view if you've scrolled back, and a status line lists the main keys. Press `Escape` to leave copy mode; clicking anywhere in the window also leaves it.

### Choose vi or Emacs keys

Copy mode uses vi-style keys by default. To use Emacs-style keys instead:

```toml
[copy_mode]
bindings = "emacs" # "vi" (default) or "emacs"
```

### Copy-mode keys

The arrow keys, `Home`, `End`, `Page Up`, and `Page Down` work with either style.

| Action | vi | Emacs |
| --- | --- | --- |
| Move left / down / up / right | `h` `j` `k` `l` | `Ctrl+B` `Ctrl+N` `Ctrl+P` `Ctrl+F` |
| Start / end of line | `0` / `$` | `Ctrl+A` / `Ctrl+E` |
| Previous / next word | `b` / `w` | `Alt+B` / `Alt+F` |
| Page up / down | `Page Up` / `Page Down` | `Alt+V` / `Ctrl+V` |
| Top of scrollback / bottom | `gg` / `G` | `Ctrl+Home` / `Ctrl+End` |
| Start or stop selecting characters | `v` | `Space` |
| Start or stop selecting whole lines | `Shift+V` | none |
| Start or stop selecting a block | `Ctrl+V` | none |
| Search | `/` | `Ctrl+S` |
| Next / previous search match | `n` / `Shift+N` | `n` / `Shift+N` |
| Copy the selection and leave (with nothing selected, a warning appears and copy mode stays open) | `y` or `Enter` | `Alt+W` or `Enter` |
| Leave without copying | `Escape` | `Ctrl+G` or `Escape` |

To copy, move to one end of the text, start a selection, move to the other end, and copy. The selection grows from where you started it to the cursor.

Searching from copy mode opens the find box. Press `Enter` to jump the copy-mode cursor to the match, then use `n` and `Shift+N` to move between matches. Filter mode isn't available inside copy mode.

Copy mode belongs to the pane you started it in. Switching to another tab or pane leaves it.

## Quick select

Quick select labels every link, file path, and commit hash on screen with a short hint, so you can open or copy one by typing its hint.

1. Press `Ctrl+Shift+U`. Each match gets a boxed hint of one or more letters.
2. Type a hint to open that match. Press `Tab` first to switch between **OPEN** and **COPY**; the status line shows which one is active.
3. Press `Backspace` to correct a partly typed hint, or `Escape` to cancel.

Quick select runs inside copy mode. After you open or copy a match, or press `Escape`, you're back in copy mode; press `Escape` again to return to the terminal.

Opening uses your desktop's default handler (`xdg-open`). A relative path such as `./src/main.rs` opens relative to the directory the shell last reported, which requires [shell integration](shell-integration.md); without it, relative paths can be copied but not opened. Absolute paths and URLs always open. A target that begins with `-` is refused; write it as `./-name` instead.

Quick select only looks at what's visible in the focused pane. Scroll to bring other matches into view first.

### What quick select finds

By default quick select finds:

- links that programs mark with OSC 8 hyperlinks;
- URLs starting with `http://`, `https://`, `ftp://`, `ftps://`, `file://`, `ssh://`, `git://`, or `mailto:`;
- file paths starting with `/`, `./`, or `../`;
- hexadecimal hashes of 7 to 40 digits, such as Git commit IDs.

To match something else, list your own patterns. A `[[copy_mode.patterns]]` list replaces the built-in URL, path, and hash patterns, so copy any of them you want to keep:

```toml
[[copy_mode.patterns]]
name = "ticket"
regex = "PROJ-[0-9]+"

[[copy_mode.patterns]]
name = "URL"
regex = "(?:https?|ftp|ftps|file|ssh|git)://[^\\s<>]+|mailto:[^\\s<>]+"
```

Patterns use [Rust regex syntax](https://docs.rs/regex/latest/regex/#syntax). You can list up to 32 patterns, each up to 4096 bytes. If a pattern is invalid, Termeric reports it and doesn't apply the configuration. OSC 8 links are always included and take priority over patterns that overlap them.

## Links

Hold `Ctrl` and click a link to open it with `xdg-open`. This works for:

- **OSC 8 hyperlinks**, which programs such as `ls --hyperlink`, `gcc`, and `systemctl` can print. The link text can differ from its target, as on a web page.
- **Plain URLs** in the output that start with `http://`, `https://`, `ftp://`, `ftps://`, `file://`, `ssh://`, `git://`, or `mailto:`.

`Ctrl+Click` also works when the program in the terminal is using the mouse, and in the find box's filter view. Relative targets follow the same rules as quick select: they open relative to the directory your shell reported, and are refused if there isn't one.

To open or copy a link without the mouse, use [quick select](#quick-select).
