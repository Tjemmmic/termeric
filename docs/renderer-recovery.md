# Renderer selection, diagnostics, and recovery

Termeric uses WGPU for both hardware and software rendering. It does not ship a
second CPU framebuffer implementation: the supported fallback is WGPU's
fallback adapter, normally Mesa llvmpipe on Linux. If neither path exists,
startup fails visibly with the attempted paths and concrete recovery commands.

## Selection policy

The default `auto` policy requests a surface-compatible hardware adapter and
device first, then WGPU's fallback adapter. If a native backend enumerates a
surface-compatible CPU adapter but does not return it for the fallback request,
Termeric selects that CPU adapter explicitly. Enumeration never substitutes a
hardware adapter for strict software mode. A device-creation failure also
advances to the fallback; finding an adapter alone is not treated as success.

```toml
[renderer]
backend = "auto" # auto, gpu, or software
```

The process-wide CLI override wins over configuration for every initial window:

```bash
termeric --renderer software
termeric --renderer gpu
```

`gpu` and `software` are strict. They do not silently cross over to the other
adapter class. Renderer selection is a startup policy; changing it through
config hot reload takes effect on the next renderer recreation or process
start. `WGPU_BACKEND` can still restrict WGPU's API backend (for example,
`vulkan` or `gl`) independently of Termeric's adapter policy.

WGPU treats `force_fallback_adapter = false` as a preference and may still
return a CPU adapter. Termeric checks the returned device type: `gpu` rejects a
CPU adapter, while `auto` advances to its separately diagnosed software attempt.
If every initial window fails before becoming visible, the complete actionable
renderer error is returned from the process with a nonzero exit status rather
than being present only in the log.

Software selection always emits a warning because latency and power use may be
materially worse. Every successful selection logs the requested policy,
adapter name, WGPU backend, device type, and whether the software path is in
use. Run with `RUST_LOG=termeric=debug` when collecting diagnostics.

The Debian package recommends `mesa-vulkan-drivers`, which supplies llvmpipe on
supported Debian-family systems. Other packages and source installations must
provide a working hardware driver or WGPU-compatible software implementation.

## Loss and recovery

A WGPU device-loss callback is routed to the owning native window and tagged
with a renderer generation. Stale callbacks from a renderer already replaced
or intentionally destroyed are ignored. A current unexpected loss:

1. drops only adapter-owned surfaces, pipelines, buffers, textures, and caches;
2. leaves the window controller, tabs, panes, terminal grids, and PTYs alive;
3. repeats the configured adapter/device selection policy;
4. recomputes cell and kernel pixel geometry; and
5. requests one recovery frame.

There is no timer retry loop. At most three real loss events are admitted in a
rolling 30-second window; another loss exits that window with an actionable
error. A lost surface or outdated swap chain pauses animation and presentation
for a 100 ms compositor grace period. A real resize reconfigures it immediately;
otherwise one deadline recreates the complete renderer. The delay allows
Winit's close event to win over the known X11 destroyed-drawable race without
leaving a live window blank forever. Presentation-only blink, UI, toast, and
graphics deadlines are removed while surface recovery is pending; PTY and
maintenance state continue to advance without a 16 ms wake loop. A transient
acquisition timeout waits for the next real dirty event instead of spinning,
while GPU out-of-memory remains fatal rather than risking further allocation.

No glyph, image, row, bind-group, or pipeline cache survives recreation. The
terminal-owned source state is rendered again into new adapter resources.

## Evidence and performance boundary

The Linux headless lane requests a fallback adapter first and submits the core
Unicode, color emoji, SGR, ligature, image, and bounded-atlas visual subset.
The native X11 lane injects one surface loss, one outdated swap chain, and one
unexpected device loss; it keeps the PTY fixture active, recovers each surface
state, recreates the renderer, and then completes the normal multi-pane
interaction suite. The private-Sway Wayland interaction lane requires the same
injections, changes and restores the compositor's headless output size after
each surface error, waits for bounded surface and complete device recovery, and
then proves keyboard, clipboard, pane routing, child cleanup, and native close
against the surviving PTYs. Both native lanes await fresh green artifacts for
the current working tree; real vendor-GPU acceptance remains separate.

On a real-GPU release host, set both `TERMERIC_GPU_SMOKE=1` and
`TERMERIC_GPU_SMOKE_HARDWARE=1` to reject CPU adapters and run the identical
visual subset. CI's llvmpipe lane is strict and never silently substitutes a
hardware adapter. A Mesa source installation without lavapipe can instead set
`WGPU_BACKEND=gl EGL_PLATFORM=surfaceless LIBGL_ALWAYS_SOFTWARE=true` before
running the smoke or benchmark; the strict test still requires WGPU to report a
CPU adapter.

The release benchmark also requests the fallback adapter first. Its p95
`renderer.frame_build_upload_submit` usability rail is 100 ms, with zero new
steady-frame pipeline-buffer allocations and the normal incremental-row
budgets. This is a deliberately broad minimum-functional budget, not a claim
that software rendering matches a GPU or includes compositor presentation.

There is not yet evidence that a separate CPU window/framebuffer renderer is
needed. Add one only if supported hosts cannot expose a WGPU fallback despite
correct packaging; duplicating text, image, clipping, accessibility, and visual
semantics preemptively would create a second correctness surface.
