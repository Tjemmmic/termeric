# Tab-bar placement

Termeric supports a horizontal top tab strip and a vertical left tab rail.
Placement is a window preference rather than workspace state, so every open
window follows the live configuration while saved workspace topology remains
portable.

```toml
[window]
tab_bar_position = "top" # top or left
```

The same setting is available under **Settings → Window → Tab Bar Position**.
Changing it, reloading the configuration, or resetting settings immediately
recalculates every local PTY and split-pane pixel geometry; running processes
are resized rather than restarted.

## Left-rail contract

Left placement keeps Commands, Workspaces, and Settings in the existing top
toolbar. The tab strip begins below that toolbar and provides the same tab body,
close button, activity/diff/plugin/read-only/broadcast state, drag reorder,
keyboard focus, and new-tab action as top placement.

The rail prefers 220 logical pixels and scales with the native display scale.
It uses a 160-logical-pixel minimum when the window can preserve it, retains at
least 160 logical pixels for terminal content on ordinary window sizes, and
splits exceptionally narrow viewports evenly instead of allowing the rail to
consume the entire window.

When all tabs do not fit, bounded previous/next controls reserve fixed rows and
the viewport automatically follows the active tab or F6 keyboard focus. Hidden
tabs remain reachable without shrinking labels into unusable slivers.

Short overflowing rails (less than five tab-row heights) share one bottom row
between Previous, Next, and New Tab. This prevents the old separate Next row
from covering a visible tab and intercepting its clicks. At least three row
heights are needed for the toolbar, one full tab, and the compact controls.
Below that size, tab labels are omitted rather than overlapping controls;
keyboard tab shortcuts still work. Controls use the available rail height and
disappear when no space remains below the toolbar. Normal-height rails retain
their separate control rows.

Tab dragging remains bound to the tab selected at button-down. Motion cannot
reorder a different tab after a keyboard selection change or source closure;
the original open tab must still be selected. Returning to it while holding
the button permits its drag to continue until release or window focus loss.
Focus loss finishes tab/divider/scrollbar and native selection gestures at
their last preview; returning while still holding the button cannot resume them.
Pinned boundaries and the existing live-domain preview/acknowledgment rules
are retained. Native overflow tests check forward/reverse local reordering and
held-button Ctrl+Tab isolation using exact PTY identities. Private Sway also
checks release while hidden and return while still held, with observed focus
transitions. Live-domain finalization extracts each control once and restores
rejected previews, including when another control is pending. Lost release
without a focus transition, device unplug, modal interruptions, edge
auto-scrolling, and native pinned/live-domain drag acceptance remain separate.

## Shared geometry

Rail width is part of the authoritative terminal geometry. The same physical
offset is applied to:

- PTY columns and pixel width;
- single terminals, nested panes, dividers, scrollbars, images, and Harvest
  indicators/menus;
- selection, mouse protocols, pane focus, divider dragging, and tab dragging;
- search, Git completion, plugin chrome, IME candidate/preedit placement, and
  AccessKit bounds.

Render, pointer hit-test, keyboard-focus, and accessibility tab bounds all use
the same visible-slot layout. Native X11/Wayland visual, drag, IME, and screen-
reader acceptance for the left rail remains an operator release check.

The automated `tab-placement` GUI scenario now checks live top/left/top reload,
character/pixel geometry for an inactive tab and two split panes, original PTY
identity, and F6 selection/new/close routing. It is selected by X11 and private
Sway CI, but selection alone is not a hosted pass. Overflow visibility, dragging,
IME, screen readers, and subjective alignment remain separate acceptance work.
See [the native scenario](gui-acceptance.md#tab-placement-geometry-and-keyboard-lane).

Keyboard focus now retains the last tab viewport when moving onto overflow or
toolbar controls. Previously, returning to the active tab's viewport could
remove the focused arrow and send the next navigation key back to the terminal.
Leaving chrome clears the retained anchor and restores active-tab tracking.
Normal left clicks perform this handoff after dispatch, preserving the visible
keyboard-scrolled target during hit testing and returning typing to the clicked
tab or pane. The native regression checks both input ownership and a distant
tab that would be misidentified if the viewport were reset before the click.
Pure regressions cover top and left layouts; the separate
[`tab-overflow` scenario](gui-acceptance.md#tab-overflow-keyboard-lane) checks
native keyboard reachability and PTY isolation without claiming visual acceptance.
