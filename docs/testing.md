# Testing Termeric

The default verification lane is deterministic and does not require a window:

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo build --workspace
```

The test suite includes a headless `App` controller, direct terminal fixtures,
real PTY quick-exit capture, a real Bash/OSC 133/Harvest path, shared render and
hit-test geometry checks, and optional executable-contract probes. Tests that
depend on an optional executable print an explicit skip reason when it is not
installed.

## Linux software-renderer smoke lane

Set `TERMERIC_GPU_SMOKE=1` to require a headless WGPU adapter and submit a real
render pass:

```bash
TERMERIC_GPU_SMOKE=1 cargo test -p termeric linux_headless_gpu_render_smoke_when_requested
```

CI should install Mesa's Vulkan software renderer (`mesa-vulkan-drivers` on
Debian/Ubuntu), set `WGPU_BACKEND=vulkan`, and set
`TERMERIC_GPU_SMOKE=1`. The test first requests a fallback adapter (llvmpipe)
and then a normal headless adapter. With the environment flag set, absence of
an adapter is a hard failure; without it, the test prints why it was skipped.

A full window/surface acceptance check remains a release-lane test under a
virtual display because creating a Winit window is platform-event-loop work,
not a unit-test-safe operation.
