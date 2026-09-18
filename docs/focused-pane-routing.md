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

Global literal/regex search, highlights, filter rows, and result navigation use
the displayed grid. During synchronized output that is the bounded viewport
captured at begin; hidden live changes and off-viewport history are not searched.
Navigation changes only the presentation grid's scroll offset, preserving the
hidden live viewport. There is no additional text or scrollback clone.

Search binds results to the focused tab/pane, presented primary/alternate
screen, synchronized snapshot lifetime, and grid dimensions. A change discards
partial and completed results, then reschedules the existing query within the
usual per-frame budget. Query text, case/regex/filter controls, and confirmed
versus editing state survive for global searches. Pending filter rows and
counts remain hidden until the rescan finishes. This does not introduce general
continuous rescanning for every ordinary output mutation.

Harvest's command-output scope uses live command-region coordinates, which are
not interchangeable with a captured viewport. Starting it while synchronized
output is frozen is explicitly refused; retry after release. An existing scope
closes on source/screen/snapshot/geometry changes and consumes a pending query
edit instead of silently widening it into a global search. Remapping command
scopes into a frozen viewport is not implemented.

Hermetic tests cover hidden text, partial and confirmed result invalidation,
filter rows, timeout/restart, single and split panes, scroll isolation, resize,
alternate screens, and strict command-scope invalidation. Native search-bar,
screen-reader, and filter-painting acceptance of these newer paths remains
deferred. Native selection follows the contract below.

## Mouse selection presentation

Native drag, word, line, and rectangular selection in the unfiltered terminal
viewport read the presented grid. During synchronized output the terminal keeps
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
remains deferred. Noncontiguous filtered-search-row selection and selection
tracking during ordinary live-output drags retain their separate existing
policies and are not established by these tests.

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
