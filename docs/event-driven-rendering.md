# Event-driven rendering

Termeric sleeps while the displayed state is unchanged. PTY reader threads
queue output and send a coalesced Winit user event; the application drains all
ready terminals, records a dirty reason, and asks for one native redraw. Input,
resize, focus, configuration, Git status, surface recovery, cooperative search,
accessibility actions/preferences, asynchronous domain/plugin/upload/background
work, and UI transitions join the same dirty-state scheduler. Background
completions are generation/window checked before they add a reason, so stale
work cannot manufacture a presentation loop.

Search query edits compile the matcher and replace a pending scan without
scanning retained rows in the keyboard event. Rendering consumes rows for at
most roughly 4 ms, records `search` as the next frame reason while work remains,
and exposes the completed result set atomically. Query changes cancel partial
results; a history/geometry shape change restarts the scan with valid row
offsets. The first scan still performs linear total work, but it cannot occupy
one unbounded input callback.

Occurrence indexing is performed inside the same row slices, so completion has
no hidden full-result indexing pass. Exact totals and matching-line membership
continue across the whole scan; recent highlighting/navigation storage is
bounded independently. Filter mode renders only one wheel-scrollable viewport
of the complete matching-row list instead of submitting every matching line.

`ControlFlow::WaitUntil` is used only for real deadlines:

- cursor blink phase boundaries;
- 16 ms animation frames while pixels are changing;
- exact per-frame Kitty graphics animation deadlines on the active tab only;
- toast fade boundaries and expiry (the opaque dwell period draws no frames);
- shortcut-recording timeout;
- the earliest active terminal's one-second synchronized-output safety deadline;
- 750 ms configuration/theme mtime checks; and
- the Git watcher's two-second refresh.

With no deadline, Winit uses `ControlFlow::Wait`. Multiple dirty reasons are
combined into one frame. Set `RUST_LOG=termeric::app=trace` to see each frame's
reason list, including `background-task` and `accessibility` instead of an
unexplained native redraw for those asynchronous paths; shutdown debug logging
reports requested, rendered, and coalesced frame counts.

Cursor movement uses a 150 ms `Instant`-based ease-out cubic transition, not
repeated frame-delta interpolation. Repeated samples of the same target do not
restart the clock; retargeting starts from the previous trajectory evaluated
at that instant. Sampling the final position clears the active flag, so the
scheduler cannot skip the endpoint frame merely because its deadline elapsed.
Hidden movement, visibility changes, new tabs, and reduced motion snap
immediately. Existing-tab switches remain animated. Only this presented
transition sustains cursor-animation deadlines: live-grid movement hidden
behind a synchronized-output snapshot does not manufacture redraws. Occluded
windows catch up from the absolute clock on their next actual render.

Synchronized-output mode is terminal-local. DECSET 2026 captures a bounded
snapshot of that terminal's visible grid, terminal-local colors, and placed
images at the exact parser boundary. Decoded image pixels use shared immutable
ownership, so the snapshot does not duplicate the bounded image payload. Live
PTY state continues advancing, while
frames reuse the snapshot and other panes plus native UI remain responsive.
DECRST 2026 releases the completed state on the PTY-triggered frame. If an
application never sends DECRST, the event loop wakes at that terminal's
one-second safety deadline, releases the snapshot, and explicitly requests the
final frame. Repeated DECSET does not extend this deadline.

An occluded window continues draining PTYs but retains dirty state until it is
exposed. Occlusion and pending lost/outdated-surface recovery suspend
cursor/text blink, UI, toast, and Kitty graphics presentation deadlines rather
than waking at 16 ms without a
surface that can present. Configuration/Git maintenance, PTY wakeups,
shortcut-recording expiry, and synchronized-output safety timeouts remain live.
Animations use their existing `Instant` clocks and catch up after exposure or
surface recovery instead of accumulating hidden frame steps. A lost surface
stops rendering until a real resize reconfigures it, preventing a retry loop.

Each frame is announced to the windowing system just before it is presented.
On Wayland this asks the compositor for a frame callback, and the next redraw
waits for it. A window the compositor is not showing, such as one on another
Sway workspace, which may not be reported as occluded, therefore receives no
redraws instead of presenting into a driver that waits for a callback that
never comes. Output, control requests, and agent status keep flowing while it
is hidden, and drawing resumes when it is shown.

## Verification

The scheduler has deterministic tests for dirty-source coalescing, deadline
selection, unique diagnostic-reason bits, and suppression of presentation-
cadence wakes under both occlusion and surface recovery while maintenance
deadlines remain available. Search
tests prove row-bounded progress, cancellation, hidden partial results, and
final correctness; release benchmarks separately measure
keyboard dispatch and complete 10k/100k scans. The PTY reader test proves that output and close events wake the
consumer only after they are queued. Synchronized-output tests cover the exact
same-parser-call snapshot boundary, text/color/image isolation, repeated begin,
deadline recovery, guaranteed final dirty state, and an unfocused pane. The
Kitty animation scheduler tests cover exact frame boundaries, loading/finite
loops, active-screen ownership, and one coalesced redraw request. Cursor
movement tests additionally cover sparse/dense sampling at equal
elapsed times, exact completion, retargeting without sampling-history drift,
large previous-frame gaps, hidden/reappearing cursors, new/existing tab
switches, reduced motion, and synchronized-output presentation ownership. The
workspace suite also covers terminal output, close cleanup, overlay
lifecycles, and surface-recovery guards.

On 2026-08-25, the release runner launched the real application under both
Wayland and X11/Vulkan and sampled five untouched 400 ms windows. Both paths
recorded `0.0%` CPU for all five samples (below one Linux scheduler tick per
window), versus the previous `2.50%` median / `7.50%` p95 of one core. Idle RSS
was 184,184 KiB on Wayland and 184,296 KiB on X11. Artifacts are written to:

- `target/benchmarks/issue-20-event-driven.json`
- `target/benchmarks/issue-20-event-driven-x11.json`

These local artifacts are reproducible evidence, not universal hardware
claims. WGPU does not expose portable GPU-idle utilization; release candidates
should additionally record `intel_gpu_top`, `radeontop`, or `nvidia-smi dmon`
on the target adapter as described in [performance.md](performance.md).

On 2026-09-02, the current tree was measured again under a private Xvfb and
private D-Bus session after extending reason coverage to accessibility and
asynchronous background completions. All five 400 ms idle windows recorded
exactly zero rendered frames and `0.0%` application CPU, while RSS stayed flat
at 246,812 KiB. The same artifact recorded a 1.13 ms median / 2.09 ms p95 native
X11 key-callback-to-present latency. The
artifact is `target/benchmarks/issue-20-final-xvfb.json`. Zero application frame
submissions is the portable idle GPU-work rail; vendor utilization and
compositor scanout remain separate hardware evidence.
