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

## Appearance

Both placements share one "quiet native" tab design. The strip uses the
theme's `tab_inactive` color. The active tab is a slightly raised, rounded
surface marked by a thin accent line in the theme's `tab_active` color, along
the bottom edge on top placement and the left edge in the rail. Titles use the
desktop's sans-serif UI face at 10pt (resolved through fontconfig), are dimmed
on inactive tabs, and are shortened with an ellipsis to the measured space
instead of a fixed character count. Tab numbers are not drawn.

The close button appears on the active tab and on the tab under the pointer.
An inactive tab with new background output shows a small activity dot in the
close button's place. Plugin tabs lead with an accent dot, and the single
highest-priority state among broadcast active, broadcast armed, read-only, and
pinned is drawn as a small icon before the title. Unreviewed diffs show as a
count pill after it. Commands (a `›_` prompt), Workspaces (a chevron), and
Settings (a menu icon) sit at the right of the toolbar.

All tab and control rectangles come from one whole-pixel geometry shared by
rendering, hit testing, keyboard focus, and accessibility bounds, so a visible
control is exactly the area that responds to it.

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

## Dragging tabs between windows

Drag a tab out of its window and release it over another Termeric window to
move it there: over that window's tab bar it goes before the first tab whose
middle is past the pointer, elsewhere to the end (a pinned tab stays among the
pinned tabs, others after them). Release it where no Termeric window is, such
as the desktop, and it opens in a window of its own; a window's only tab stays
put, since it already has a window. The tab keeps its place in its old window
while the pointer is outside it.

Windows must share a process, which new windows do by default
([joining the running Termeric](command-line.md#joining-the-running-termeric)).
On Wayland no window knows where the others are, and the source window holds
the pointer until the release, so the drop target is the window the pointer
enters within 400 ms of the release; if none does, the tab tears off. The torn-
off window opens empty and adopts the tab, so a window that cannot open loses
nothing. Live-domain tabs keep their daemon-owned place.

## Overflow while dragging

While dragging a tab, hold over a visible Previous/Next overflow control to
move that same tab toward hidden tabs. The first move waits 400 ms; subsequent
moves wait 200 ms each, even without pointer motion. Top and left layouts use
their existing rendered control hit boxes, including the compact rail. Moving
away or reversing direction starts a fresh dwell. Traversal stops when the
control disappears or an owner, pinned, closing, or live-domain boundary refuses
the next move. It never selects a sibling, wraps at the end, or catches up with
a burst after a delayed event-loop wakeup. No repeat timer remains at a blocked
boundary. Arbitrary edge zones outside these controls are not implemented.

Direct pointer moves and timed overflow moves share the same admission policy:
the captured source must remain selected, open, and bound to the same local or
exact domain/remote-tab identity; the destination must be open and in the same
pinned group. A live drag additionally requires a same-domain destination and
no pending request for that domain, including requests started after button-down.
Requests for unrelated domains do not block it. Finalization rechecks the exact
captured remote owner before emitting a daemon move, so a replaced, detached,
closed, or closing source cannot address the previous remote tab. Existing
authoritative acknowledgment/rollback behavior remains unchanged.
Keyboard focus gates autonomous overflow repetition, not direct native pointer
input: a pointer-only Wayland seat may start a new drag. Actual focus loss still
finalizes an existing gesture and clears its repeat timer.

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
the button permits its drag to continue until release, window focus loss,
modal takeover, or terminal-geometry synchronization. These interruptions
finish tab/divider/scrollbar and native selection gestures at
their last preview; returning while still holding the button cannot resume them.
Pinned boundaries and the existing live-domain preview/acknowledgment rules
are retained. Native overflow tests check forward/reverse local reordering and
held-button Ctrl+Tab isolation using exact PTY identities. Private Sway also
checks release while hidden and return while still held, with observed focus
transitions. Live-domain finalization extracts each control once and restores
rejected previews, including when another control is pending. Lost release
without a focus transition, modal takeover, or geometry change, device unplug,
and native pinned/live-domain drag acceptance remain separate. The native
overflow lane checks stationary forward/reverse overflow-control holds through
the hidden range on the normal left rail, compact left rail, and top strip.
Normal/top controls stop at the first/last visible viewport; compact controls
continue to the first/last tab. Exact PTY markers check the selected owner before
keyboard reselection, the resulting tab indices, and restored sibling order.
The fixture rejects unrelated window focus loss during these checks. This is
private-compositor input evidence, not subjective visual or screen-reader
acceptance, and does not establish X11 or hosted CI results merely by selection.

Modal takeover is reconciled before native/user-event dispatch, before animation
cleanup, and after queued PTY work. A release consumed by the modal cannot leave
a background drag active, and dismissing the modal does not resume that gesture.
Panel hover handling and application-owned terminal mouse reports retain their
separate paths. Private Sway exercises palette takeover during a held tab drag
with no focus loss; other modal types have unit rather than native acceptance.

Settings, Commands, the workspace/session panel, and its dropdown own new
pointer presses ahead of background scrollbars and pane dividers. Outside
clicks dismiss the owning panel without starting a background gesture or
changing pane focus. Divider hover cannot suppress panel hover, and the global
Settings button still switches directly from another panel. The overflow lane
checks outside-divider dismissal for Settings, Commands, and the session dropdown
with unchanged pane geometry and exact focused-PTY input ownership.

Window resize, display-scale changes, and font/configuration paths share native
gesture finalization in terminal-geometry synchronization. Tab drags cannot
resume against changed hit targets, and live-domain divider previews are sent
through the normal acknowledgment/rollback path rather than silently discarded.
The native overflow lane holds a tab through a width change, rejects unrelated
focus loss, then verifies unchanged tab order and restored PTY width. Physical
monitor-scale transitions and native live-domain resizing remain separate checks.

Scrollbar drags capture the clicked terminal's stable tab/pane identity. A
focus change, closed/closing owner, or missing pane cancels the gesture instead
of scrolling whichever terminal became selected. Every scroll mutation
revalidates that identity, even when a caller omitted focus-surface refresh;
returning to the original pane does not revive a canceled drag. Tab/pane
isolation and stale-owner refusal have raw PTY/unit coverage; native pane-switch
and live-domain scrollbar checks remain separate acceptance work.

Divider drags finish when their tab loses selection, closes, or no longer has
the captured visible divider (including zoom). Focus changes between panes in
the same selected tab preserve the gesture. Refresh and the resize mutation
boundary both validate ownership, so a hidden tab cannot continue resizing and
returning to it cannot revive the drag. Live-domain previews still use normal
acknowledgment/rollback; a rejected finalization preserves any earlier pending
request. Local native coverage checks a real positive resize followed by a
held-button tab switch with unchanged source and destination PTY geometry.
Native live-domain acceptance remains separate.

A same-domain request becoming pending after divider button-down also finishes
the gesture before another preview can resize panes. Focus-surface refresh and
the resize boundary both enforce this. The existing control rejection path
restores the cached authoritative ratio and preserves the earlier request;
clearing that request does not revive the old drag. Requests for other domains
do not block local or unrelated live divider previews. Synthetic topology/local
PTY tests cover rollback, route preservation, and one-time finalization; this
does not establish real native live-domain dragging.

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
