# Keyboard copy mode and quick select

Termeric provides a pane-local keyboard cursor for navigating the visible grid
and complete retained scrollback without sending navigation keys to the child
process. The default entry shortcuts are `Ctrl+Shift+X` for copy mode and
`Ctrl+Shift+U` for quick select. Both appear in Settings > Keyboard Shortcuts,
use the ordinary shortcut conflict resolver, and can be rebound or disabled.

## Copy mode

Set `[copy_mode].bindings` to `"vi"` (the default) or `"emacs"`. Arrow, Home,
End, Page Up, and Page Down work in either preset.

| Action | vi | Emacs |
|---|---|---|
| Move by cell/row | `H J K L` | `Ctrl+B/F/P/N` |
| Line start/end | `0` / `$` | `Ctrl+A/E` |
| Word backward/forward | `B` / `W` | `Alt+B/F` |
| Page up/down | `PageUp/PageDown` | `Alt+V` / `Ctrl+V` |
| Oldest/newest history | `gg` / `G` | `Ctrl+Home/End` or page keys |
| Character selection | `V` | `Space` |
| Line/block selection | `Shift+V` / `Ctrl+V` | use the vi preset when these forms are required |
| Search | `/` | `Ctrl+S` |
| Next/previous result | `N` / `Shift+N` | `N` / `Shift+N` |
| Copy and exit | `Y` or `Enter` | `Alt+W` or `Enter` |
| Exit and clear | `Escape` | `Ctrl+G` or `Escape` |

The cursor and selection use the terminal's scrollback-aware cell coordinates.
Wide-character spacer cells are never selected as a cursor position. Before a
column resize, logical-line bookmarks are captured for both endpoints and
resolved after reflow, so wrapped selection endpoints follow their text. Mode
ownership includes the tab and pane ID; changing focus with the mouse exits the
mode rather than silently operating on another pane. Search uses the existing
grid and match index, including Unicode byte-to-cell mapping and retained
scrollback. Search filter mode is not duplicated or hidden.

## Quick select

Quick select labels visible OSC 8 links and configured regex matches with stable
base-26 keys. Labels are boxed and contain text, so they do not rely on color.
Type a complete hint to perform the displayed action. `Tab` toggles between
`OPEN` and `COPY`, `Backspace` edits a multi-key prefix, and `Escape` returns to
copy mode.

The explicit hint keystroke is the user's open action. Opening delegates to the
same argument-safe desktop opener used by Ctrl+click; no match text is evaluated
by a shell. Relative paths use the focused terminal's trusted local OSC 7
working directory; remote-host reports are never converted into local paths.
OSC 8 links take precedence over overlapping regex matches across every row of
a wrapped link. Resize reflow rebuilds visible matches and their cell
coordinates while preserving the chosen open/copy action. Configured
matches follow soft-wrapped logical lines instead of truncating a path or URL
at a physical row edge. Scans are limited to the focused pane's visible rows,
at most 32 configured patterns, regexes of at most 4096 bytes, and 4096 visible
hints. Invalid regex configuration is rejected before it can replace the last
working runtime configuration.

Defaults detect URLs, local/relative paths, and 7-40 digit hexadecimal hashes.
Replace the list with TOML array entries when different project semantics are
needed:

```toml
[copy_mode]
bindings = "vi"

[[copy_mode.patterns]]
name = "ticket"
regex = "PROJ-[0-9]+"
```

## Evidence and boundaries

State-machine tests cover scrollback motions, word/line/page limits, character
selection, directional movement across Unicode wide cells, stable labels,
prefix activation, wrapped OSC priority, regex byte-to-cell mapping, and
soft-wrapped regex matching with a hard result cap, plus logical endpoint and
quick-select restoration across reflow. A
renderer golden test fixes the overlay's scrollback-to-visual-row mapping. The
native X11 workspace scenario performs search-driven selection and clipboard
yank entirely by keyboard, verifies no modal bytes reached the PTY, and opens an
OSC 8 link through the isolated desktop-command spy using a quick-select hint.

Opening arbitrary configured path patterns is intentionally as powerful as
clicking that path. Users should only configure patterns they are comfortable
passing to their desktop opener. Platform text input, screen-reader semantics,
and the application-chrome focus cycle are documented in
[modern text input and accessibility](accessibility-and-input.md).
