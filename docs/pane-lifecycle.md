# Pane lifecycle and divider ownership

Entering pane mode transfers the tab's complete live owner—terminal grid, PTY
handle, reader channel, Harvest state, and shell-integration state—into pane
zero. `PaneManager::from_existing()` does not spawn a shell. The requested
split then creates exactly one new pane and inherits the focused shell's
working directory. A safe local-authority OSC 7 report is preferred; platform
process inspection remains the fallback.

While pane mode is active, the tab has no single-terminal grid, PTY, or reader
allocation. `Tab::focused_terminal()` and `Tab::write()` resolve through the
pane manager. If the first new-pane spawn fails, pane zero is immediately
restored to the tab. When closing leaves one pane, that same ownership bundle
moves back to the tab without restarting the process or replaying content.

Dropping a `PtyHandle` explicitly terminates a still-running child. Dropping a
closed pane also drops its bounded output receiver, releasing a reader blocked
on backpressure. Final PTY output is drained before process-state fallback is
allowed to mark a tab or pane closed, avoiding quick-exit output loss.

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
Mouse-down records the exact tree-path identity of a local divider and every
subsequent move resizes that same node, even after the pointer leaves its hit
rectangle. Live-domain trees use daemon-assigned split identities instead;
their final ratio is committed through the authenticated control protocol.

Divider colors derive from the active theme's border and cursor colors. The
four-pixel groove remains subtle while its eight-pixel hit margin keeps it
targetable.

## Regression evidence

The lifecycle test performs three adopt/split/close/dissolve cycles. It proves
that adoption spawns zero shells, each split spawns exactly one, the original
PID and output survive every transition, new panes inherit cwd, and every
closed pane plus the final tab child disappears from `/proc`. A real output
chunk arrives while the original reader channel is pane-owned.

An asymmetric nested-tree test obtains divider render data and feeds positions
inside those exact four-pixel rectangles back through hit testing at 0.75x,
1x, and 2x coordinate scales. A separate regression drags a nested divider
across an ancestor and proves the mouse-down target cannot jump. The headless
GPU smoke exercises the resulting direct renderer argument.

Focused operations are documented in
[focused-pane-routing.md](focused-pane-routing.md), and event/reply ownership in
[pane-event-routing.md](pane-event-routing.md).
