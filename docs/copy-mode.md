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
resize, a grid-bound transaction tracks the cursor and anchor through the actual
cell moves, so retained stream-selection endpoints follow their text. Mode
ownership includes the tab and pane ID; changing focus with the mouse exits the
mode rather than silently operating on another pane. Search uses the existing
grid and match index, including Unicode byte-to-cell mapping and retained
scrollback. Search filter mode is not duplicated or hidden.

Resize tracking shares the core selection mapper: wide-spacer anchors normalize
to their grapheme leader, and endpoints follow synthetic padding and one-column
collapse/re-expansion. It handles compact history, lines crossing into the
viewport, height-only archival, and eviction without reconstructing line numbers
from the oldest remaining row. Evicted, clipped, or discarded-padding endpoints
cannot alias other text. Losing either endpoint clears the selection; losing the
modal cursor also restarts navigation at the terminal cursor. Copy mode remains
active. Actual OSC 66 repair damage clears affected selections, not unrelated
ranges. Block and physical-line ranges survive height-only changes when their
cells survive, but retire on width changes while the cursor still follows text.

Each grid stores at most one fixed-size pair of modal coordinates. The existing
reflow pass handles four endpoints only when a transaction is present (the live
selection pair plus the modal pair); otherwise it retains its two-endpoint path.
There is no second text model, per-cell map, or extra retained-history scan.
Tokens are one-use, cannot consume another grid/request's result, and reject
intervening content mutations outside resize and its glyph repair. Multiple
resizes within one transaction compose their mappings. These are resource and
correctness bounds, not measured native resize-latency claims.

Hermetic coverage includes retained-character identities across widths, heights,
history limits and endpoint directions; Unicode round trips; fully archived and
crossing logical lines; lost ranges; glyph repairs; and single/split-pane
synchronization with the canonical terminal selection. Native shell/IME resize,
painting, and clipboard acceptance remain deferred.

During live output, an invisible core navigation anchor follows the copy cursor
even when no range is selected. It uses the same movement and actual-damage
rules as terminal selections: full/partial scrolling, line edits, horizontal
insertion/deletion, Unicode and multicell writes, erases, and history eviction.
Before render/input, a changed grid revision reconciles the modal cursor and
the core's canonical selection. Damaged ranges cannot be resurrected by the next
motion; an intact cursor remains usable when only the selection anchor is lost.
Losing the cursor restarts navigation at the terminal cursor with no selection.
Reset and primary/alternate grid changes retire old coordinates. Frozen views
retain their independent modal state and never register it on the hidden live
grid. Leaving copy mode releases the invisible anchor in its original owner.

This adds one optional fixed-size point represented by the existing selection
geometry, not a second grid or a history-sized scan. No-selection output still
returns before glyph lookups when there is no navigation anchor. Width changes
use the separate scoped resize transaction above and republish the live anchor
afterward. Hermetic regressions compare point movement/damage with canonical
selections and cover unselected navigation, damaged/evicted ranges, ownership,
screen transitions, and frozen/resize compatibility. This does not establish
all protocols' mutation coverage or native desktop acceptance.

## Quick select

Quick select labels visible OSC 8 links and configured regex matches with stable
base-26 keys. Labels are boxed and contain text, so they do not rely on color.
Type a complete hint to perform the displayed action. `Tab` toggles between
`OPEN` and `COPY`, `Backspace` edits a multi-key prefix, and `Escape` returns to
copy mode.

Quick-select hints scan the presented grid, including the frozen viewport during
synchronized output. They retain their matched values and the presented trusted
local OSC 7 directory for relative-path actions; hidden text and cwd changes do
not replace those targets. An absent or remote directory report does not borrow
a hidden local report or the application's directory: relative-path opening is
refused with feedback, while copying the target remains available.
One bounded OSC 7 metadata record is retained with the synchronized snapshot;
this is not a second scrollback model.

Hints are tied to the captured grid and synchronized snapshot lifetime.
Beginning, releasing, timing out, or replacing the snapshot invalidates existing
hints at render/input boundaries, with feedback. Live content revisions,
viewport scrolling, and resize revalidate targets before painting or accepting
hint input. Surviving targets keep their labels, action, and captured directory,
while their positions update. A label never transfers to a different payload or
source. Identical targets are paired in display order; this promises stable
actions, not persistent identity for indistinguishable text occurrences.

Targets that disappear are removed, and retired labels are not reused or
resurrected. Newly arriving targets require a fresh quick-select invocation.
If no captured targets remain, or a partial prefix no longer names any surviving
target, quick select closes with feedback; the input that detects this closure
is consumed without activating another target. Copy mode remains available.
Resize follows this same label policy instead of relabeling the new match list.
The quick-select cursor marks a presented surviving hint.

An unchanged grid revision/viewport check is constant-time and retains the
match allocation. Changed sources rescan only the presented viewport and use a
borrowed-key lookup over the existing bounded match list, not a history scan or
quadratic target search. The 4,096-match cap remains in force. Hidden output
behind a synchronized snapshot does not trigger a visible rescan. This is a
resource bound, not a native latency measurement.

Tests cover frozen OSC/regex targets, local/remote/absent directory metadata,
single and split panes, live replacement, viewport movement, stable partial
labels, duplicate targets at the cap, resize and lifetime invalidation without
opening links or writing to the desktop clipboard. Native acceptance remains
deferred.

Ordinary copy mode also navigates, selects, searches, and extracts text from the
presented grid. During synchronized output this is only the captured viewport:
hidden new output and off-viewport history are not accessible until release.
Navigation reuses that grid without cloning scrollback; its selection remains
owned by copy mode and is not installed on the hidden live grid. Single and
split panes render that same selection. Reflow within a snapshot tracks its
retained endpoints through that snapshot's own grid, including wrapped text;
the hidden live grid never receives those coordinates.

Beginning, releasing, timing out, or replacing the synchronized snapshot resets
copy mode onto the new displayed grid and clears selection, hints, and search.
Copy extraction rejects stale snapshot coordinates even before the next render.
Exiting during a frozen snapshot does not retain its selection on the live grid.
Search outside copy mode follows its separate [presentation contract](focused-pane-routing.md#search-presentation).
Native mouse selection follows its [presentation contract](focused-pane-routing.md#mouse-selection-presentation).
Automated tests cover cursor entry, text extraction, Unicode/physical selection
semantics, copy-mode search, reflow, and lifecycle transitions. Native keyboard,
selection painting, and clipboard acceptance of this synchronized-output path
remain deferred.

The explicit hint keystroke is the user's open action. Opening delegates to the
same argument-safe desktop opener used by Ctrl+click; no match text is evaluated
by a shell. Filesystem-relative targets, including bare filenames, `src/file`,
`.`/`..`, and explicit `./`/`../` forms, require a trusted **absolute** local OSC 7
working directory. Quick select uses its captured presented directory;
Ctrl+click uses the clicked pane's presented directory, and routed activation
events use their originating terminal's presented directory. Missing, remote,
or invalid relative directory reports never fall back to the application cwd or
another pane. The error suggests enabling local OSC 7 reporting or using an
absolute path/file URI. Copying is unaffected. Paths remain literal: no shell,
environment-variable, or tilde expansion is performed.

Absolute filesystem targets and targets with a URI scheme do not need a terminal
directory. URI schemes remain delegated to the desktop handler; use an explicit
`./` prefix when a colon-bearing filename should be treated as a relative path.
On Windows, drive-relative paths are rejected rather than using an implicit
per-drive directory; native Windows opener acceptance remains separate.
File URI targets are passed unchanged to the desktop opener: their authority
and percent escapes are not stripped or decoded by Termeric. Termeric no longer
silently turns a remote file URI into the same pathname on the local machine. The
desktop handler decides whether that URI is supported; this does not promise
remote-file mounting or access. This preserves the host distinction in
[RFC 8089](https://www.rfc-editor.org/rfc/rfc8089.html#section-3), and Linux's
[xdg-open](https://portland.freedesktop.org/doc/xdg-open.html) accepts file URIs.
Side-effect-free tests inspect the actual launch command's single target
argument and cwd, exercise pre-launch rejection, and verify local/remote/absent
and frozen owner routing in single and split panes. No successful test launches
a desktop handler; native file-handler acceptance remains deferred.
Bare targets beginning with `-` are refused before launching an opener so a
link or custom hint cannot become a helper option such as `--help`. Use an
explicit relative path (`./-report.txt`) or an absolute path/file URI for such
filenames. Quick select and hyperlink activation report launch/refusal errors
as non-blocking error toasts. Tests exercise refusal through the public opener
without spawning a child and verify application error feedback; this does not
claim a general desktop-handler sandbox or native Windows option-policy audit.
OSC 8 links take precedence over overlapping regex matches across every row of
a wrapped link. Resize reflow rebuilds visible matches and their cell
coordinates while preserving the chosen open/copy action. Configured
matches follow soft-wrapped logical lines instead of truncating a path or URL
at a physical row edge. Wide-character spacers and synthetic wrap padding do
not insert spaces into regex text. OSC 8 link hints likewise join across a
synthetic padding column, but not across an ordinary unlinked blank or a hard
line break. Byte-to-cell spans retain the physical positions for highlighting;
this also applies to compact history when scrolled into view. Scans are limited
to the focused pane's visible rows,
at most 32 configured patterns, regexes of at most 4096 bytes, and 4096 visible
hints. Invalid regex configuration is rejected before it can replace the last
working runtime configuration.

When widening after a one-column collapse, reflow reconstructs a wide glyph's
spacer with the surviving leader's hyperlink and rendition. Regression tests
cover regex and OSC 8 hints through narrow/wide round trips, wide CJK and emoji
text, compact history, explicit spaces, and hard line breaks. These are local
grid/state-machine checks, not additional native desktop-opener acceptance.

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
