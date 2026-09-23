# Focused terminal routing

A tab in pane mode transfers its single-terminal owner into `PaneManager`; it
does not retain a terminal, PTY, or reader allocation. `Tab::focused_terminal()`,
`Tab::focused_terminal_mut()`, and `Tab::write()` are the authoritative routes
for user-facing operations; they deliberately return no fallback if pane mode
has lost its focused pane.

## Routing contract

- Explicit plugin Context, Clear, Copy, and Jump to Error commands resolve the
  live plugin launch owner, not the focused pane of a plugin-tagged tab. They
  also work when a plugin pane lives in a shell-tagged mixed tab. Selection is
  deterministic: active tab first, then other matching tabs in tab order. Within
  each tab, its focused matching plugin pane wins, then its first matching pane
  in visual order. Context/Clear/Copy do not
  change focus; checked writes refuse a read-only selected owner without trying
  a different pane. Jump to Error focuses the selected owner, refreshes focused
  surfaces, and searches only its output. Profile/transcript operations retain
  their separate tab-wide lifecycle semantics.
- Plugin Restart resolves that same exact owner and prepares a single matching
  launch using its cwd and saved profile. It validates cwd and creates a fresh
  replacement tab before retiring the source pane (or its sole-owner tab), so
  unrelated panes survive. Missing/invalid launch responses, configuration or
  spawn failures, and unavailable cwd leave the old processes/layout intact.
  Read-only and live-domain targets refuse a local restart. When no owner exists,
  Restart retains the ordinary open-new-tab behavior. A successful PTY spawn is not proof
  of provider readiness; a later application exit still needs operator judgment.
- Harvest Re-run is an exact-source action, not keyboard input. It may focus
  its producing terminal for visibility, but writes only to that captured
  owner, bypassing broadcast/alias/completion routing. Destructive confirmation
  preserves the same origin; read-only and closed/closing targets refuse user
  writes even if confirmation arrives after a focus/lifecycle change.
- Input, paste, selection, search, scrolling, synchronized-output checks,
  session metadata, plugin context, Git working directory, and tab title updates
  use the focused terminal or its focused PTY. Search reads that terminal's
  presentation grid, not output hidden by synchronized-output mode. A local-authority OSC 7
  report takes precedence over platform process inspection for cwd consumers.
- A transient read-only flag is owned by that same live pane. All
  user-originated writers use the checked route, while a separately named exact
  origin route remains available only for app-mediated terminal replies. This
  prevents input bypasses without deadlocking OSC/color/capability queries.
- Mouse coordinates are converted relative to the focused pane rectangle and
  that pane's scroll offset. Native selection then maps the visual row into
  the presented grid; terminal-protocol reports retain live coordinates.
  Clicking content or a pane-local scrollbar focuses
  that pane before the operation is applied.
- Focus changes re-run the current search against the new grid, rebuild the
  visible URL cache, refresh the cwd-derived title, invalidate Git state, reset
  cursor blink state, clear only transient mouse-drag state, and request a
  redraw. Selection remains owned by each terminal.
- Command-output search scopes are an exception to query rebinding: a source,
  displayed-screen, snapshot-lifetime, or geometry change closes the scoped
  search rather than applying old row bounds to unrelated content.
- OSC 8 hyperlinks use scrollback-aware grid coordinates. Automatically
  detected URLs use the corresponding visible-row cache.
- Each pane contributes its own Kitty, Sixel, and iTerm2 image placements to rendering. GPU image
  cache keys combine terminal identity with the protocol-local image ID, so two
  panes may safely reuse the same image number.
- Search highlights and filter mode are applied only to the focused pane. Other
  panes continue rendering their complete, independent grids.
- Session saves take one stable snapshot of the focused pane's cwd, last
  command, and scrollback. The snapshot is labeled as focused-pane-only and
  restores into a new shell rather than recreating the split.

Ownership transitions use `Tab::take_terminal_owner()` and
`restore_terminal_owner()`; application modules never access terminal/PTY
fields directly. A source guard rejects bypasses of the focused-terminal API.

## Search presentation

The find box floats over the top-right corner of the terminal area, clear of
the scrollbar strip; terminal content, pointer mapping, images, and IME
geometry never shift while it is open. It holds the query, a match status,
case/regex/filter toggles, previous/next, and close. Previous/next keep their
space without matches but act only while there are matches, and an invalid
pattern adds one line naming the problem.

Global literal/regex search, highlights, filter rows, and result navigation use
the displayed grid. During synchronized output that is the bounded viewport
captured at begin; hidden live changes and off-viewport history are not searched.
Navigation changes only the presentation grid's scroll offset, preserving the
hidden live viewport. There is no additional text or scrollback clone.

Search binds results to the focused tab/pane, presented primary/alternate
screen, synchronized snapshot lifetime, grid instance, dimensions, and content
revision. A change discards partial and completed results, then reschedules the
existing query within the
usual per-frame budget. Query text, case/regex/filter controls, and confirmed
versus editing state survive for global searches. Pending filter rows and
counts remain hidden until the rescan finishes. Ordinary output now invalidates
both completed and partially scanned results, including in-place edits and
scrollback rotation at a fixed history limit. Every bounded scan slice also
checks its grid revision so rows from different versions cannot be combined.

The grid keeps constant-size instance/content/row-mapping counters. Mutable
cell/row access conservatively counts as a content change, even for a no-op
write; cursor movement and user viewport scrolling do not. History eviction,
row movement, reflow, and replacement invalidate source-row coordinates.
Inactive owners and hidden writes behind a frozen presentation do not restart
the focused displayed search. The existing compiled query, bounded result
storage, and roughly 4 ms per-frame scan budget are reused; no text/history
snapshot is added. A stream changing faster than a complete scan can finish
may keep results pending until it quiets. Incremental changed-row indexing and
continuous-output latency qualification remain future work, not an implied
performance claim.

Harvest's command-output scope uses live command-region coordinates, which are
not interchangeable with a captured viewport. Starting it while synchronized
output is frozen is explicitly refused; retry after release. An existing scope
closes on source/screen/snapshot/geometry or row-mapping changes and consumes a
pending query edit instead of silently widening it into a global search.
In-place edits rescan the same row scope without including neighboring output.
Remapping moving command scopes or scopes into a frozen viewport is not
implemented.

Hermetic tests cover hidden text, partial and confirmed result invalidation,
filter rows, timeout/restart, single and split panes, scroll isolation, resize,
alternate screens, same-shape edits, capped-history rotation, replacement grids,
navigation/inactive-owner isolation, and strict command-scope invalidation.
Copy and accessibility publication reconcile filtered projections before using
their row coordinates, including when output arrives before the next render.
Native search-bar, screen-reader, and filter-painting acceptance of these paths remains
deferred. Native selection follows the contract below.

## Mouse selection presentation

Native drag, word, line, and rectangular selection read the presented grid,
including the matching-row projection when search is filtered. During synchronized output the terminal keeps
one small selection record alongside its existing frozen viewport; neither
text nor scrollback is cloned again. Rendering, accessible selection reporting,
word/line expansion, and clipboard text extraction read this same selection.

An existing selection is intersected with the captured viewport and translated
from scrollback into frozen coordinates at begin. Copying it while frozen
includes only that visible intersection. New selections made in the frozen
view clear any old live selection; their coordinates never transfer to hidden
content after release. A dimension-changing resize clears frozen native
selection rather than guessing new endpoints; an identical resize preserves it.

A native drag records its terminal owner, snapshot lifetime, presented screen,
and dimensions. Changed ownership or presentation cancels further drag updates
and resets multi-click tracking. Snapshot-captured selections can still be
cleared with Escape after that cancellation. Mouse-up finishes a selection on
its original presentation only; plain clicks without a drag clear, while
explicit one-cell word/line/block selections remain selectable.

Core and headless app regressions cover hidden replacement text, clipped and
reversed scrollback selections, word/line/block behavior, single/split panes,
timeout/restart/resize/focus invalidation, and no hidden-coordinate fallback.
Tests do not write the desktop clipboard. Native painting/clipboard acceptance
remains deferred.

During ordinary full-screen output scrolling, the terminal owns the shifted
selection coordinates. Before drag updates and gesture finalization, the app
copies that small canonical selection record into pointer state instead of
reapplying its old anchor. This also covers viewport archival with ED 22 and
user scrolling into history; no text or history is copied. Mouse-up, focus loss,
and modal takeover share the same finalizer. Completed selection caches follow
the same coordinates, and shifted endpoints reset positional multi-click state.
A completed plain click still preserves the click history needed for word and
line selection.

Line/byte-limit eviction, explicit history clearing, and terminal reset cannot
resurrect removed selections on the next move or release. Clearing history
discards a selection only when an endpoint no longer has backing rows; visible
and frozen selections survive when their own rows remain present. Hermetic
tests cover forward/reversed/block/word/line selection, single/split panes,
release without another move, archived viewports, and completed selection cache
cleanup. Native acceptance remains deferred.

Vertical movement inside partial-height or left/right-margin rectangles now
uses one selection mapping policy for SU/SD, IL/DL, line feed, and reverse index.
Selections outside the affected cells stay unchanged. A range wholly within
the surviving source cells follows the same row displacement, preserving its
direction, columns, type, and active/finished state. A range that loses cells
or splits between moving and stationary cells is cleared: the current selection
model cannot represent that discontiguous set without selecting unrelated text.
Full-screen upward archival retains its distinct history-preserving path;
full-screen downward scrolling uses the erasing rectangle policy.

The range calculation samples only first/last/interior row shapes, so it is
constant-time even for a selection extending far into history and adds no
per-cell storage. A cellwise reference checks 48,000 small-grid combinations
across all four selection types, reversed endpoints, stationary history,
rectangles, and shifts. Protocol and headless single/split-pane tests cover
survival, erased edges, mixed movement, frozen selection isolation, and native
drag/release. Native acceptance remains deferred.

Ordinary printable output now clears a selection when it rewrites selected
cells, including an identical character rewrite. Unrelated writes, cursor/SGR
changes, and selected history outside the live edit stay selected. Combining
characters invalidate the changed grapheme; width changes include old wide
halves and newly overwritten neighbors, including the destination of a wrapping
width expansion. Normal text overwriting a top-row OSC 66 cell or extending its
grapheme invalidates the entire affected block's selection; writing past a
skipped lower row does not. Insert-mode printing clears selections in its
affected shift span rather than attempting horizontal remapping. Frozen visible
selections remain isolated from hidden live writes.

This uses the shared constant-time rectangle intersection calculation. With no
selection, writes return before additional grid lookups. When a selection
survives outside a write's direct span, only old multicell descriptors within
that edited row span are inspected; no selected-text/history copy is added.
Hermetic tests cover all selection types and reversed endpoints, Unicode,
OSC 66 collision/skip/extension, history/frozen isolation, and single/split-pane
drag and release after replacement. They do not invoke the desktop clipboard.

Explicit ED/EL/ECH, DECSED/DECSEL, and DECERA/DECSERA now share a text-erasure
path that invalidates selections from actual damage. Row erasure reports its
expanded wide-grapheme span; selective erasure reports only unprotected spans,
and multicell erasure reports the whole block rather than only the requested
intersection. These callbacks come from the mutation routines themselves, so
selection does not maintain a second protection policy or rescan/copy selected
history. There is no persistent damage list or extra block-validation pass.

Protected selections survive selective erasure when all of their selected cells
survive. Ordinary erasure still ignores protection. ED 22 moves retained ranges
into history before erasing the viewport; viewport erasure does not remove
backed history selections. Hidden erases cannot alter a frozen visible selection.
Empty core erase ranges and unsupported erase modes do not invalidate it.
Native drag/release consumes this canonical state and cannot resurrect erased
ranges. Hermetic coverage includes 360 command/cell comparisons, protected
wide pairs and OSC 66 blocks extending beyond the requested rectangle, history,
alternate/frozen screens, and single/split-pane native selection. Desktop
painting and clipboard acceptance remain deferred.

DECFRA fill and DECCRA copy also report destination replacement and actual
wide-pair/multicell repair damage. Reading a copy source does not invalidate its
selection; overlap invalidates it only where destination writes or glyph repairs
affect selected cells. Replacing selected cells retires the selection even when
the new text is identical, while self-copy and rejected operations remain no-ops.
Copy still snapshots its source before mutation; selection adds no further grid
clone, damage list, or repair pass. Untouched history and frozen presentation
selections remain isolated. Hermetic coverage includes 1,728 cellwise-reference
cases across all selection shapes and reversed endpoints, overlap directions,
clipping, origin/margin coordinates, protected wide/multicell boundary damage,
and single/split-pane drag and release without invoking the desktop clipboard.

Standalone ICH/DCH move an intact single-row selection with its surviving cells,
preserving endpoint direction, type, and active state. Stationary selections stay
put; clipped/deleted selections and ranges mixing moved and stationary cells
are cleared. The constant-time mapping does not scan selected history. Whole
multicell erasures invalidate old coordinates before movement, while wide-pair
repair reports damage in the new coordinates. Single-row multicell blocks that
cross the right edit margin are erased atomically on deletion as on insertion.
Zero-count core calls are no-ops (CSI zero still defaults to one); margins,
history, and synchronized snapshots retain their existing isolation rules.
Coverage includes 107,520 small-grid cellwise mapping cases, 1,344 protocol
count/margin cases, wide/multicell boundary repair, and single/split-pane mouse
release and drag reconciliation. Insert-mode printing retains its separately
documented invalidation policy; native desktop acceptance remains deferred.

New OSC 66 text placement invalidates selections over its destination and any
whole old multicell block removed by a collision. Lower-row skips preserve the
untouched old block's selection. Insert-mode placement retires selections in its
shifted row spans, like ordinary insert-mode printing, and reports block erasure
outside those spans. Placement now repairs orphan ordinary wide halves on each
written row and reports that damage, including halves outside its rectangle.
Since new multicell cells contain no ordinary wide halves, this repair checks
only the two outside boundary cells rather than scanning a whole row per
naturally segmented grapheme.
This uses the shared live-grid damage callback without copying selected text or
retaining a damage list. Placement-induced full-screen scrolling retains the
existing history mapping, and hidden placement cannot change frozen selection.
Coverage includes 700 cellwise placement/selection cases, natural-width Unicode
segmentation, fractional sizing, rejected input, whole-block and wide-pair
collisions, lower-row skips, insert mode, wrapping into history, frozen alternate
screens, and single/split-pane native drag/release. Native painting and clipboard
acceptance remain deferred.

Vertical scrolling and insert/delete lines now include glyph damage beyond
their movement rectangle. Whole-block boundary/intersection erasures report old
coordinates before selection mapping; partial-width row copies, vacated-row
erasure, and final descriptor repair report destination coordinates afterward.
Thus an outside stationary selection is cleared when its glyph is actually
erased, while intact moved glyphs retain their mapped selection. The callbacks
reuse existing mutation/repair passes without a damage list or added history
scan. Zero-count core scroll/line-edit calls do not erase blocks or move anchors.
Hermetic tests cover SU/SD/IL/DL and scrolling LF/RI, both wide-character margins,
vacated rows, early and final multicell repair, surviving wide/multicell glyphs,
history, frozen alternate screens, and single/split-pane drag reconciliation.

Print-time grapheme extension now uses the ordinary text placement/collision
path after removing its old cluster. Width growth can erase a whole top-row
multicell collision, replace a covered top-row collision with spaces, or skip
lower rows without damaging that block or its selection. This fixes orphaned
descriptors previously left by a variation-selector expansion. Original
rendition/protection is retained through relocation and wrapping. Atomic wide
pair clearing replaces the previous whole-row repair scans in this extension
path; insert-mode printing also forwards actual erase/repair damage. Hermetic
tests cover growth, wrapping, margins, no-autowrap skips, frozen alternate
screens, descriptor/wide invariants, and single/split-pane drag reconciliation.
The collision policy follows [Kitty's text-sizing rules](https://sw.kovidgoyal.net/kitty/text-sizing-protocol/#wrapping-and-overwriting-behavior).

Height-only resize now translates live selection rows by the storage layer's
exact archived-row count, not by the change in history length (which can stay
capped). It preserves endpoint direction, columns, type, and active state when
all selected rows survive. Evicted history and discarded bottom rows retire the
selection instead of clamping onto unrelated content. Repair of a clipped
multicell reports further damage after mapping. This adds no row/text copy,
history scan, or retained mapping table. Frozen selections keep their existing
policy: identical dimensions preserve them; changed dimensions retire the frozen
projection while independently updating live state. Native resize cancels the
old drag geometry without restoring stale anchors over the canonical selection.
Hermetic tests use retained-row identities across cursor positions, heights,
selection shapes/directions, and history limits, plus clipped multicells,
primary/alternate frozen screens, and single/split-pane pointer reconciliation.

Width-changing reflow now carries Simple/Semantic stream-selection endpoints
alongside the existing cell moves. Two fixed-size tracking slots follow compact
history, the history/viewport continuation, wide-character padding, one-column
collapse/re-expansion, and final archival/eviction. No per-cell map or second
copy of retained history is allocated. Valid wide-spacer anchors normalize to
their grapheme leader; discarded padding, orphan spacers, evicted endpoints,
and clipped bottom rows retire the selection rather than aliasing other text.
Surviving stream selections retain their direction, type, and active state.
Block and physical-line selections are deliberately retired on width changes:
their old rectangular/row geometry cannot represent the same text after reflow.
Actual multicell repair damage is checked against the new column geometry.
Frozen projections and native drag geometry keep the resize policy above.

Hermetic tests cover Unicode round trips, both endpoint directions, compact
history fast paths, crossing history/viewport lines, retained-character identity
across widths/heights/history limits, clipped and evicted endpoints, discarded
padding/orphan spacers, frozen primary/alternate screens, and single/split-pane
pointer reconciliation. Keyboard copy mode now shares this mapping through its
own grid-bound [resize transaction](copy-mode.md#copy-mode), including frozen
views. Its live navigation point now shares core movement/damage handling and
reconciles with the canonical selection before input/render, rather than
restoring stale modal coordinates after output. This is not a claim that every
grid mutation is covered. Native desktop acceptance remains deferred.

## Filtered selection and accessible text

When a completed search filter displays matches, mouse rows map through the
same virtualized row window as rendering. Blank space below the matches does
not select hidden source rows; dragging below them clamps to the last displayed
match. Word/line expansion uses that source row, and rectangular selections
keep their columns across noncontiguous matching rows.

Clipboard extraction borrows the existing full matching-row index and visits
only matching rows inside the selected range. There is no cloned history or
second result index. Scrolling the filtered window preserves the result-set
identity, so a drag can span multiple pages. New queries, rescans, filter
toggles, or source changes invalidate that identity and clear the original
terminal's projected selection; stale copy attempts fail before reconciliation.
Skipped source rows always create a newline. Adjacent soft-wrapped rows still
join for stream/semantic selection; line/block selection retains physical rows.

The accessibility subtree now reads the presentation grid too, not hidden live
text. Its focused pane uses exactly the displayed filter rows and maps selection
endpoints into those visual line nodes; other panes retain their independent
unfiltered presentation. Wide and combining Unicode text keeps cell-to-character
mapping, including reversed selections. Pending/empty filters retain the
renderer's existing ordinary-viewport fallback rather than inventing a second
display policy for selection or accessibility.

Hermetic tests cover row mapping, word/line/reversed-block selection, multi-page
drag/copy, index-storage reuse, query/toggle/focus invalidation, Unicode wrapping,
and frozen filtered accessibility nodes. Native painting, clipboard, and Orca
acceptance remain deferred. A changed search projection invalidates its filtered
selection rather than applying ordinary contiguous-range scroll tracking to it.

## Detected Unicode links

Automatic URL detection preserves Unicode hosts, paths, query text, and
fragments for the existing scheme list (`http`, `https`, `ftp`,
`ftps`, `file`, `mailto`, `ssh`, and `git`). Scheme matching is ASCII
case-insensitive as required by [RFC 3986 §3.1](https://www.rfc-editor.org/rfc/rfc3986.html#section-3.1);
the complete target retains its original case, including its scheme, path,
query, and percent escapes. Detection scans original bytes without allocating
a lowercase copy of each logical line. Exhaustive scheme-case permutations,
negative Unicode lookalikes, nested/wrapped Unicode cache mapping and reflow,
and an isolated allocation probe cover this boundary without launching a handler.
It retains the original UTF-8,
including combining sequences and ZWNJ/ZWJ, without normalization, IDNA
conversion, percent encoding, or decoding. The desktop handler remains
responsible for interpreting the target; detection is not validation or a
guarantee that a handler supports an internationalized resource identifier.

The non-ASCII repertoire follows RFC 3987's
[`ucschar` ranges](https://www.rfc-editor.org/rfc/rfc3987.html#section-2.2),
with conservative boundaries for prose: Unicode whitespace, soft hyphen,
zero-width space, word joiner, BOM, and explicit bidi formatting controls end
an inferred target. Private-use characters and noncharacters are not inferred
as URL text. This is deliberately narrower than a complete IRI parser (including
private-use query text), and not a Unicode-spoofing defense. Explicit OSC 8
targets retain their separate semantics and priority.

Matched ASCII parentheses and square brackets remain part of a target,
including IPv6 hosts; an unmatched closing delimiter is excluded rather than
passed to the opener. Existing trailing ASCII sentence-punctuation trimming
is unchanged. Ambiguous prose, Unicode punctuation adjacent to a URL, unmatched
opening delimiters remain heuristic boundaries; use explicit
OSC 8 metadata when the target cannot be inferred unambiguously.

Detection tracks UTF-8 byte boundaries in one forward scan, then the existing
logical-line cache maps those spans to cells. Wide halves and combined/emoji
graphemes resolve to the complete original target, including across soft wraps,
resize reflow, and retained history. The scan adds no per-character allocation
or normalization buffer. Tests check exact bytes, delimiter and scalar-range
boundaries, all occupied cells of representative graphemes, unchanged-cache
reuse, and single/split-pane filtered and synchronized-output activation data.
They open no links; native Unicode underline/click/handler acceptance remains
deferred. This is not a measured whole-application performance claim.

## Filtered link presentation

Detected-URL underlines and Ctrl+click targets use the same displayed matching-row
slice as the focused pane. Visual rows resolve to their original signed source
rows, including compact history; blank space below the results has no target.
OSC 8 targets retain priority over detected URL text. Unfocused panes keep their
ordinary presentation caches, and pending/empty filters use the renderer's
ordinary-viewport fallback.

Detection can join a soft wrap only when the next displayed row is also the next
source row. Omitting an intervening row must not stitch unrelated text into a
new target. Caches keep visual-row coordinates, share wrapped URL payloads, and
reuse the borrowed filter window without copying the full matching-row index.
Activation reconciles the search source and refreshes the focused cache, so a
filter scroll, toggle, or query change does not require an intervening render to
discard old targets. Synchronized output still resolves only presented cells.

Hermetic regressions cover semantic-link priority, hidden intervening targets,
history/Unicode mapping, contiguous and omitted wraps, cache reuse and shrink,
filter scrolling/query/toggle changes, and frozen single/split-pane targets with
a different live scroll offset. They inspect targets without launching a desktop
handler. Native filtered underline/click/opener acceptance remains deferred.

## Regression evidence

Automated coverage uses distinct pane contents and state to prove that focus
changes rebind search results, URL activation data, selection, scrollback, and
synchronized-output state. Coordinate tests cover asymmetric pane origins and
scrollback offsets. The normal single-terminal route still uses the same
focused APIs, preserving its prior behavior.

Plugin-command regressions use synthetic plugin launch metadata on raw local
capture shells, never real provider executables. They cover focused shell
siblings, mixed tab presentation, split/collapse/nonzero survivors, checked
write refusal, exact copied output, matching-focus preference, and error
navigation without changing sibling scroll state. See
[#122](https://github.com/Tjemmmic/termeric/issues/122).

Restart fixtures add configuration/spawn/cwd/response validation failures,
readonly/domain refusal, same-provider sibling isolation, single and nonzero
owners, exact source cwd/profile, and original sibling PID preservation. Only
synthetic plugin hooks and raw local shells are used; real provider/native
acceptance remains deferred. See
[#123](https://github.com/Tjemmmic/termeric/issues/123).

Pane lifecycle/dividers and event/reply ownership are documented in
[pane-lifecycle.md](pane-lifecycle.md) and
[pane-event-routing.md](pane-event-routing.md). Session persistence semantics
are documented in [session-snapshots.md](session-snapshots.md).
