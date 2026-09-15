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
  deterministic: first matching tab in tab order, then its focused matching
  plugin pane or first matching pane in visual order. Context/Clear/Copy do not
  change focus; checked writes refuse a read-only selected owner without trying
  a different pane. Jump to Error focuses the selected owner, refreshes focused
  surfaces, and searches only its output. Tab-wide restart/profile/transcript
  operations retain their separate lifecycle semantics.
- Harvest Re-run is an exact-source action, not keyboard input. It may focus
  its producing terminal for visibility, but writes only to that captured
  owner, bypassing broadcast/alias/completion routing. Destructive confirmation
  preserves the same origin; read-only and closed/closing targets refuse user
  writes even if confirmation arrives after a focus/lifecycle change.
- Input, paste, selection, search, scrolling, synchronized-output checks,
  session metadata, plugin context, Git working directory, and tab title updates
  use the focused live terminal or its focused PTY. A local-authority OSC 7
  report takes precedence over platform process inspection for cwd consumers.
- A transient read-only flag is owned by that same live pane. All
  user-originated writers use the checked route, while a separately named exact
  origin route remains available only for app-mediated terminal replies. This
  prevents input bypasses without deadlocking OSC/color/capability queries.
- Mouse coordinates are converted relative to the focused pane rectangle and
  that pane's scroll offset. Clicking content or a pane-local scrollbar focuses
  that pane before the operation is applied.
- Focus changes re-run the current search against the new grid, rebuild the
  visible URL cache, refresh the cwd-derived title, invalidate Git state, reset
  cursor blink state, clear only transient mouse-drag state, and request a
  redraw. Selection remains owned by each terminal.
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

Pane lifecycle/dividers and event/reply ownership are documented in
[pane-lifecycle.md](pane-lifecycle.md) and
[pane-event-routing.md](pane-event-routing.md). Session persistence semantics
are documented in [session-snapshots.md](session-snapshots.md).
