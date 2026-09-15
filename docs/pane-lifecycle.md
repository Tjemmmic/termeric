# Pane lifecycle and divider ownership

Entering pane mode transfers the tab's complete live owner—terminal grid, PTY
handle, reader channel, Harvest state, and shell-integration state—into its
existing pane ID (initially zero). `PaneManager::from_existing()` does not spawn
a shell. The requested split then creates exactly one new pane and inherits
the focused shell's working directory. A safe local-authority OSC 7 report is preferred; platform
process inspection remains the fallback.

While pane mode is active, the tab has no single-terminal grid, PTY, or reader
allocation. `Tab::focused_terminal()` and `Tab::write()` resolve through the
pane manager. If the first new-pane spawn fails, the adopted pane is immediately
restored to the tab. When closing leaves one pane, that same ownership bundle
moves back to the tab without restarting the process or replaying content.

The collapse bundle also carries the next pane and local-divider IDs. Both
allocation high-water marks survive single-terminal mode, failed split rollback,
and subsequent re-entry; a retired ID cannot be assigned to a replacement in
the same live tab. Allocation history belongs to the tab, not an individually
moved pane. Exhausted counters reject new allocation rather than wrapping or
resetting, including the GUI-local IDs used by live-domain reconciliation.
Workspace relaunch starts a new runtime namespace; archives do not preserve
retired request identities across application restart.

Dropping a `PtyHandle` explicitly terminates a still-running child. Dropping a
closed pane also drops its bounded output receiver, releasing a reader blocked
on backpressure. Final PTY output is drained before process-state fallback is
allowed to mark a tab or pane closed, avoiding quick-exit output loss.

Both the unsplit `Pty` and reader-split `PtyHandle` declare their child owner
before their writer/master fields. This is a required destruction-order
invariant: portable-pty 0.8.1's Unix writer destructor emits LF followed by
VEOF. Terminating the child first prevents that cleanup from completing a
blocked read or submitting buffered, unentered input during terminal close.
The child observer still preserves a naturally completed process's real status;
the fix does not rewrite successful exits as failures. Detached-writer ownership
is a separate input-worker lifecycle and is not changed here.

## Divider geometry

The pane tree is the only divider-geometry source. Every frame passes its
current divider slice directly to `Renderer::render()` alongside pane grids;
there is no transient renderer cache to clear before drawing. Layout,
rendering, resize, and hit testing share the four-pixel divider thickness.
PTY rows, columns, and rounded physical pixels are derived independently from
each resulting pane rectangle; a split process starts with its final
post-divider geometry. See [PTY pixel geometry](pty-pixel-geometry.md).
Mouse coordinates and rendered coordinates both use content-local pane values,
then add the same window padding and dynamic content-top offset.
Mouse-down records the exact stable identity of a local divider and every
subsequent move resizes that same node, even after the pointer leaves its hit
rectangle. Live-domain trees use daemon-assigned split identities instead;
their final ratio is committed through the authenticated control protocol.

Divider colors derive from the active theme's border and cursor colors. The
four-pixel groove remains subtle while its eight-pixel hit margin keeps it
targetable.

## Regression evidence

The lifecycle test performs three adopt/split/close/dissolve cycles. It proves
that adoption spawns zero shells, each split spawns exactly one, the original
PID and output survive every transition, retired pane/divider IDs are never
reissued, new panes inherit cwd, and every closed pane plus the final tab child
disappears from `/proc`. A real output
chunk arrives while the original reader channel is pane-owned.

Additional raw-capture PTY tests close newer panes through the app handler,
collapse to the oldest survivor, and re-split. Retired pane input/focus/close and
divider-resize requests fail without changing focus, geometry, or owner input;
a stale Harvest rerun also sends no bytes. Fresh input still reaches only its
exact replacement pane. Near-exhaustion and failed-spawn regressions check both
allocation counters across repeated ownership transfers. These fixtures never
evaluate captured commands or invoke providers, and do not replace native
visual/compositor acceptance.

An asymmetric nested-tree test obtains divider render data and feeds positions
inside those exact four-pixel rectangles back through hit testing at 0.75x,
1x, and 2x coordinate scales. A separate regression drags a nested divider
across an ancestor and proves the mouse-down target cannot jump. The headless
GPU smoke exercises the resulting direct renderer argument.

Focused operations are documented in
[focused-pane-routing.md](focused-pane-routing.md), and event/reply ownership in
[pane-event-routing.md](pane-event-routing.md).
