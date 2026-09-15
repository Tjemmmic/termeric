# Advanced tab and pane controls

Termeric keeps tab IDs stable for a window lifetime and pane IDs stable within
their owning tab. Workspace archives preserve visual tab order, the pinned-tab
prefix, explicit user titles, pane focus, and the unzoomed split tree. These
operations act on existing PTYs; reorder, swap, balance, zoom, and move do not
restart a child process.

## Commands and shortcuts

Every operation below appears in the command palette and in Settings > Keyboard
Shortcuts. They intentionally have no default shortcut, so an existing shell or
application binding cannot be captured unexpectedly.

| Command | Behavior |
|---|---|
| Rename Tab | Captures bounded, single-line text inline; Enter commits and Escape restores the prior user title. An empty title clears the override. |
| Move Tab Left / Right | Reorders within the pinned or unpinned group. Number shortcuts follow visual order. |
| Move Tab to Next Window | Transfers the complete active tab to the next native window and activates it there. |
| Pin or Unpin Tab | Moves the tab to the end of the pinned prefix or the start of the unpinned group. |
| Zoom Pane | Makes the focused pane fill the content view without modifying its split tree. |
| Toggle Pane Read-only | Locks or unlocks user-originated input for the focused pane while output and required terminal replies remain live. |
| Swap Pane with Next | Exchanges terminal positions in leaf order while retaining focus and PTY ownership. |
| Balance Panes | Resets every divider ratio in the tab to an even split. |
| Layout: Columns / Rows / Grid | Rebuilds balanced geometry from the same pane IDs and PTYs. |
| Layout: Tall / Fat | Makes the focused pane a full-height left or full-width top master at 50% and evenly tiles the remaining panes in visual order. |
| Move Pane to Next Tab | Detaches the focused pane and attaches it to the next tab, focusing the moved pane. |
| Move Pane to Next Window | Transfers the focused pane into the active tab of the next native window without restarting its process. |
| Cycle New Pane Directory | Selects focused pane, tab primary pane, or Termeric process cwd and saves the choice. |
| Toggle Broadcast Input | Arms, confirms, or cancels keyboard broadcast for the active tab. |

Tabs can also be reordered by dragging their visible tab-bar body. Pinning is a
group boundary, so dragging never silently changes pin state.

The tab strip can be placed horizontally at the top or vertically on the left
through `[window].tab_bar_position` and the live Window settings. Both modes
share overflow navigation, drag ordering, keyboard focus, close controls, and
semantic bounds. See [tab-bar placement](tab-bar.md) for the rail sizing and
terminal-geometry contract.

Move Tab to Next Window follows native-window creation order with wraparound.
The complete tab owner moves: a single terminal or nested pane tree, every live
PTY and reader, titles, pin state, launch/plugin identity, transcript and
Harvest state. The destination assigns a fresh window-local tab ID, inserts a
pinned tab at the end of its pinned prefix (or an unpinned tab at the end),
activates it, and resizes its terminals to the destination geometry. If the
source loses its last tab, that now-empty native window closes normally. A
missing second window, closing source tab, or live-domain tab fails before
detachment; live-domain tab placement remains authoritative topology shared by
its attached clients.

For attached live-domain tabs, rename, pin/unpin, shortcut movement, and drag
movement are daemon-owned controls rather than window-local presentation
changes. Every attached window converges by stable remote tab ID while keeping
its existing relay PTYs. Explicit remote titles retain the same precedence over
OSC titles, and clearing one reveals the daemon's newest focused-pane title.

Swap, balance, and columns/rows/grid/tall/fat are daemon-owned as well: stable
pane IDs, focus, and relay PTYs survive, while a preset receives fresh stable
split IDs. Tall and Fat always use the focused pane as the master; the other
panes retain their relative visual order.
Cross-tab movement is an atomic daemon transaction that preserves the global
pane ID, process, and relay streams; repairs the source; creates a fresh split
at the destination; and converges every attached window. Zoom remains
window-local view state.

The cwd policy is stored as `[panes].cwd = "focused"`, `"tab"`, or
`"process"`. The default is `focused` for compatibility with existing split
behavior. Focused/tab inheritance prefers that terminal's safe local OSC 7
report and falls back to platform process inspection.

## Title precedence

An explicit user rename has highest priority. OSC 0/2 comes next, followed by
the focused terminal's OSC 7 path, foreground-process cwd discovery, and the
generated fallback. OSC changes keep updating in the background while a user
title is present, so clearing the user title immediately reveals the newest
terminal-controlled title.

## Zoom and persistence

Zoom is view state, not layout state. Hidden panes keep their existing PTY
dimensions while zoomed. Resizing the window resizes only the visible pane;
unzoom restores the original tree and then synchronizes every pane to its real
rectangle. Workspace save records that underlying tree, not a transient zoom.

## Read-only panes

Read-only is a transient safety lock for the focused pane. The active tab label
begins with `RO`; split panes also receive a thick teal outline, and AccessKit
announces the locked state. The command is available through the palette and
the configurable shortcut registry but has no default binding.

Keyboard encoding, IME commits, paste, dropped-path insertion, mouse/focus
reports, custom/plugin text injection, confirmed guardrail writes, running-GUI
`send-text`, and broadcast all use the same user-input gate. Exact control
requests fail with a read-only error rather than silently retargeting another
pane. Locked panes cannot be selected as broadcast targets, and locking an
active target immediately reconciles or cancels that broadcast.

PTY output, local selection/copy/search, terminal parsing, and app-mediated
protocol replies remain active. Replies bypass only the user-input lock so
queries such as OSC 52 and dynamic-color requests cannot deadlock an
application. The lock travels with a live pane through splitting, collapse, and
process-local pane/tab movement. It is deliberately not written to workspace or
legacy-session archives: restoring a workspace starts unlocked rather than
surprising the user with a terminal that rejects input.

## Moving panes

Window-local moves transfer the live `Terminal`, PTY handle, reader, shell
metadata, and Harvest state. The source focuses the detached pane's sibling;
the destination focuses the moved pane under a new destination-local pane ID.

Any ordinary shell, terminal-profile, or plugin pane in a split tab can move.
Its live owner carries both the persisted launch identity and frozen process
configuration. When the former
launch-primary pane moves, the source keeps every surviving pane ID and records
a new explicit primary ID for deterministic relaunch; the destination assigns
the moved PTY a new destination-local ID. If a moved pane later becomes the sole
owner, its launch type becomes the tab presentation. Shell/profile owners also
become the ordinary split default; plugin owners retain the tab's ordinary
split policy rather than implicitly cloning a plugin. The plugin terminal's
session, recorder, and diff state stay with its existing PTY.
For live-domain tabs, the daemon keeps the global pane ID and moves the complete
runtime between stable tabs; each attached GUI transfers its existing relay
owner to a new tab-local ID. Moving the last pane or moving into a
missing/closing destination fails with a visible explanation.

Move Pane to Next Window follows native-window creation order with wraparound.
The destination is that window's active ordinary tab; a single-terminal
destination is converted to pane mode without spawning another process. The
terminal, PTY handle, reader, shell metadata, title, and Harvest state move as
one owner and are resized to the destination geometry. Source and destination
broadcast state is cancelled before further input. Shell and named-profile
panes may cross unlike tab types because workspace persistence records each
launch independently. A source with one pane, a missing second window, a
closing destination tab, a live-domain endpoint, an unavailable anchor, or
exhausted destination pane/divider IDs fails without detaching anything. Focused
shortcuts and exact-ID moves share those checks, including allocation history
after collapse. Live-domain topology remains daemon-owned. Plugin sessions and
mixed-plugin archives follow the [terminal-owner contract](plugin-panes.md).

## Broadcast safety and routing

Broadcast is intentionally a two-step operation. The first invocation selects
every writable pane and arms the operation for ten seconds. While armed, selected panes
have thick amber outlines and the tab label begins with `??`; clicking a pane
toggles it, as does Space for the focused pane after navigating with the normal
pane shortcuts. Enter or a second Toggle Broadcast Input invocation confirms
the selected identities. Confirmation requires at least two selected panes. The
arming expiration is an exact event-loop deadline, so an idle window clears the
amber state without waiting for unrelated cursor animation or input.

Active target panes receive thick red outlines and the tab label begins with
`!!`. Escape or the command cancels immediately. No ordinary keyboard input is
sent to a PTY while the operation is armed. Changing tabs, losing the pane
manager, or falling below two live panes also cancels. If a selected pane exits,
its identity is pruned; active broadcast continues only while at least two
confirmed targets remain.

Each key and paste is encoded independently using the target terminal's own
DECCKM, keypad, and bracketed-paste mode. Alias expansion and speculative Git
completion are bypassed while active so targets receive identical committed
input. Mouse reports, focus reports, terminal protocol replies, and application
control traffic are never broadcast.

Every write path prunes panes whose PTY writer has failed. The selected set is
updated immediately when at least two targets survive; otherwise broadcast is
cancelled before later input. Key, raw-byte, and paste paths use the same
two-target floor rather than waiting for a render-time cleanup.
Destructive-command guardrails retain a safe pasted line prefix, so a later
typed suffix cannot bypass the combined-command check.

Target selection is per activation and limited to the active tab. Named or
persistent target groups and cross-tab/window broadcast are not provided.

## Verification boundary

Unit and real-PTY tests cover 128 reproducible 96-step tree-operation sequences,
a 256-step focus/zoom/layout/owner sequence, tree swap/balance/presets,
zoom/resize restoration, and repeated live PTY detach/attach across independent
managers, tabs, and window controllers without replacing child processes,
whole-tab transfer with pinned/plugin metadata and empty-source closure, stable
tab identity, workspace serialization,
authoritative live-domain title/pin/order, pane-layout and cross-tab relay
convergence, input to multiple pane PTYs, and cancellation after partial
closure. The Linux native GUI lane covers
tab creation and drag reorder plus nested pane interaction. As
with other native UI behavior, non-Linux promotion follows the platform evidence
policy.
