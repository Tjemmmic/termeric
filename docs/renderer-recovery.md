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
against the surviving PTYs. Acceptance-only fault paths are initially absent
files; the harness creates them sequentially and the renderer consumes each at
most once per process. This prevents a recovery resize from accidentally
injecting the next requested failure. Clean local commit `f9ccc2d` passed both
the private-Sway interaction and two-window fullscreen scenarios on an AMD
Radeon RX 7900 XTX with all three injections enabled. The schema-3 records bind
both runs to debug binary SHA-256
`9cfc391dd5fd4b321c002c3afeb38568f677012fd25081e20e9ecaaae86541b0`.
Equivalent current X11 evidence remains open. A separate validated local
release-binary vendor-memory observation is now recorded in
[resource soak evidence](resource-soak.md#2026-09-18-release-binary-refresh);
portable signed vendor evidence remains absent.

### Release-build Wayland refresh (2026-09-18)

Clean `6ab38bf` passed the private-Sway `interaction` and `fullscreen` scenarios
with all three injections enabled, using `target/release/termeric` SHA-256
`da9140702571c8df8445407f01d1bf1f277884c17cd0f64a799dca33e9646817`.
The adapter reported AMD Ryzen 9 7900X3D / `raphael_mendocino`, radeonsi OpenGL,
`software=false`; this is not the earlier RX 7900 XTX configuration.
The schema-6 result and schema-3 candidate record are retained locally at
`/tmp/termeric-wayland-recovery-6ab38bf-release/results.json` with sibling logs
and provenance. All six candidate checks passed before this documentation-only
follow-up.

Logs show lost/outdated-surface recovery and device recreation before continued
PTY interaction, followed by keyboard-only copy, private-compositor bracketed
paste, exact pane routing/closure, modal input checks, and native close. The
two-window scenario observed fullscreen entry/exit resizing only the focused
PTY, with the sibling remaining live until its own native close. Offline
verifiers separately reconstructed all seventeen palette cases and eleven
Settings cases and checked their two/three artifact seals. The six-case mouse
ownership sidecar also completed without an error; it remains driver evidence,
not an independently sealed proof. The palette/Settings verifiers do not
independently re-prove recovery or fullscreen semantics; those remain harness
assertions and retained logs.

The command used an explicit installed Noto Sans Mono primary with Noto Color
Emoji fallback and **skipped visual comparisons**:

```bash
env -u WLR_RENDERER -u LIBGL_ALWAYS_SOFTWARE WGPU_BACKEND=gl \
  python3 scripts/gui_acceptance.py --backend wayland \
  --scenario interaction --scenario fullscreen --start-sway \
  --skip-visual-checks --font-family 'Noto Sans Mono' \
  --force-surface-loss --force-surface-outdated --force-device-loss \
  --require-clean --binary target/release/termeric \
  --artifacts /tmp/termeric-wayland-recovery-6ab38bf-release
python3 scripts/gui_palette_evidence.py \
  /tmp/termeric-wayland-recovery-6ab38bf-release/results.json
python3 scripts/gui_settings_evidence.py \
  /tmp/termeric-wayland-recovery-6ab38bf-release/results.json
```

No user desktop or real provider was exercised. This does not establish X11,
pixel-perfect visuals, native IME/Orca, physical device removal, or real-desktop
policy. Xvfb and Openbox were unavailable on this host, so the X11 lane was not
run or replaced by this Wayland result. The
[operator prerequisites](operator-acceptance.md#arch-linux) now also include the
default DejaVu primary-font package that was missing from the Arch command.
Local `/tmp` bundles are not published or signed artifacts, and the GUI
[provenance boundary](gui-acceptance.md) still applies.

### GPU smoke and benchmark evidence

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
On clean commit `f9ccc2d`, Mesa 26.2.2 llvmpipe/Vulkan passed the deterministic
visual signature and every release budget; the renderer frame p95 was 2.363 ms
and steady-frame pipeline-buffer allocations were zero. The visual artifact's
SHA-256 is
`80b8c3b2e9a38c4e52f697e986702463f24dc3741f1b16dcd14fa2d71b8d2471`.

There is not yet evidence that a separate CPU window/framebuffer renderer is
needed. Add one only if supported hosts cannot expose a WGPU fallback despite
correct packaging; duplicating text, image, clipping, accessibility, and visual
semantics preemptively would create a second correctness surface.
