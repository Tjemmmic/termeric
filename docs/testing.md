# Testing Termeric

The default verification lane is deterministic and does not require a window:

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
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

## Release performance lane

The opt-in release checks enforce initial beta budgets of three seconds for
processing 100,000 styled output lines and three seconds for 200 large-history
reflows:

```bash
TERMERIC_PERF_ACCEPTANCE=1 cargo test -p termeric-core --release --locked \
  --test performance_acceptance -- --nocapture
```

These generous regression ceilings are machine-independent safety rails, not
claims about interactive latency. Record actual release-machine measurements
in [`release-acceptance.md`](release-acceptance.md).

## Clippy policy

Strict Clippy is part of the default quality gate. The application crate has
two narrow, documented exceptions: nested conditions are retained where UI
dispatch priority or short Rust borrow scopes are clearer that way, and
renderer builders may take explicit geometry/state parameters rather than
hiding unrelated inputs inside generic parameter bags. New exceptions require
a local rationale; broad lint-family suppression is not accepted.
