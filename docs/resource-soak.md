# Resource soak testing

Termeric has a process-level Linux soak lane for detecting unbounded memory,
thread, file-descriptor, child-process, transcript-queue, and GPU-error growth.
It complements the short deterministic benchmarks; it does not turn a short CI
run into proof of multi-day stability.

A second runner measures the actual displayed Termeric process under a private
headless Sway compositor. The lanes are complementary: the core scenario drives
broader deterministic storage/session operations, while the native scenario
includes Winit, a compositor connection, a WGPU surface and renderer caches,
real PTYs, and repeated output-size and pane-lifecycle changes.

## Scenario

The ignored `scheduled_resource_soak_scenario` test repeatedly exercises one
long-lived terminal while creating and tearing down real PTY-backed panes and
tabs. Each iteration includes:

- high-volume ASCII, styled, CJK, combining-mark, and emoji-ZWJ output;
- alternating terminal resizes, scrollback eviction, and full-history search;
- bounded raw image storage and replacement;
- three-pane and four-tab PTY creation, output, exit, and cleanup;
- replacement of an on-disk session snapshot; and
- bounded asynchronous transcript input/output, durable shutdown, and file
  removal.

The scenario asserts its internal line/byte/image limits and clean PTY exits.
The runner independently samples the test process through Linux `/proc` and
checks peak RSS, first-to-last RSS/thread/fd growth, concurrent children, exit
status, final child cleanup, completion output, and WGPU validation/device-loss
messages. Child sampling aggregates and deduplicates every
`/proc/<pid>/task/<tid>/children` file so PTYs or helpers spawned by Rust worker
threads cannot evade the process ceiling or cleanup check.

Every iteration also reports the real byte backlog held by the bounded
background transcript channel before shutdown, the accepted and dropped entry
counts, and the remaining backlog after its durable flush. The runner requires
all 68 input/output entries to be accepted, no drops, no more than 2 MiB queued
by default, and exactly zero bytes after every iteration finishes. Missing or
malformed queue fields fail instead of being interpreted as zero pressure. Use
`--max-transcript-queue-mib` to tighten the peak backlog rail for a controlled
runner class; it cannot disable the no-drop or complete-drain requirements.

Both runners require at least two samples and at least half of the nominal
`duration / sample_interval` cadence. A stalled or misconfigured sampler can no
longer report zero growth from a single point and pass the memory-stability gate.

## Running it

Run the same 30-second lane used for pushes and pull requests:

```bash
WGPU_BACKEND=vulkan python3 scripts/run_soak.py \
  --duration-seconds 30 --gpu-smoke \
  --output target/soak/local.json \
  --log target/soak/local.log
```

The weekly and manually dispatched CI lane runs for ten minutes. For a release
candidate, run at least that lane on the exact candidate and retain both files:

```bash
WGPU_BACKEND=vulkan python3 scripts/run_soak.py \
  --duration-seconds 600 --gpu-smoke \
  --output target/soak/release.json \
  --log target/soak/release.log
```

## Displayed Wayland process soak

The native fixture continuously emits styled ASCII, CJK, combining marks,
emoji-ZWJ sequences, long lines, and bounded Kitty image replacement through a
real PTY. Each replacement requests an exact Kitty success response, and the
runner requires a duration-scaled minimum number of `i=91;OK` acknowledgements;
plain byte progress cannot stand in for graphics acceptance. The runner
alternates the Sway output between two sizes, repeatedly
creates and destroys a focused split pane, injects PID-unique keyboard tokens,
and requires exact single-pane routing. It samples the real application PID
through `/proc`, including direct children spawned by any thread, then closes
the native window and requires every observed fixture child to disappear.

Run the 30-second change lane after building the application:

```bash
cargo build -p termeric --locked
WGPU_BACKEND=gl python3 scripts/run_native_gui_soak.py \
  --duration-seconds 30 \
  --binary target/debug/termeric \
  --artifacts target/native-gui-soak
```

Weekly/manual CI runs both the core scenario above and this displayed scenario
for ten minutes. Native defaults allow 768 MiB peak RSS, 192 MiB
first-to-last-quartile RSS growth, eight threads and sixteen file descriptors of
growth, and eight concurrent direct children. The report also requires
continuous PTY progress, enough resize/pane actions for the configured duration,
zero runtime/GPU validation errors, a normal application exit, and complete
child cleanup. Resize and pane-churn counts have independent minimums; one
workload cannot compensate for the other in the aggregate action count. These
are broad regression rails, not expected steady-state consumption targets.

The schema-1 native report records UTC time, candidate commit, tested binary
SHA-256, backend/compositor/WGPU identity, every raw process sample, workload
counts including separate resize and pane-churn totals, the minimum required
sample count, accepted graphics acknowledgements, explicit thresholds, and
individual results. Application, Sway, and PTY
logs remain beside it. Run the pure regression detector with:

```bash
python3 -m unittest scripts.test_run_native_gui_soak
```

Defaults are intentionally broad host-independent safety rails: 512 MiB peak
RSS, 128 MiB first-to-last RSS growth, four threads of growth, eight file
descriptors of growth, no more than 16 concurrent direct children, at most
2 MiB of transcript backlog, no dropped transcript entries, complete queue
drainage, no surviving observed children, successful scenario/GPU exit, a
completion marker, and no recognized GPU validation/device-loss message. The
first and last quartiles are compared by median so process startup and one noisy
sample do not define the growth result. Tighten these budgets only with repeated
artifacts from the same class of runner; do not hide a regression by raising
them without an investigation.

The runner's JSON records UTC time, commit plus dirty-worktree disclosure, WGPU
backend, test-binary path and SHA-256, every raw sample, scenario progress,
explicit thresholds, and each pass/fail result. The companion log contains the
Rust test output and optional headless GPU smoke. CI uploads both even when a
ceiling fails. A dirty local report is implementation evidence, not an exact
candidate record even when its commit field matches the candidate branch.

## Regression-detector tests

The ceiling evaluator has deliberate flat, file-descriptor leak, RSS leak,
thread leak, excessive transcript backlog, incomplete drain, dropped-entry,
and missing-queue-metric fixtures. A synthetic multi-thread `/proc` tree
separately proves that children of non-leader threads are included exactly
once:

```bash
python3 -m unittest scripts.test_run_soak
```

`python3 -m unittest scripts.test_ci_resource_contract` separately fails if the
native macOS/Windows build-and-unit matrix, either 30-second Linux soak, either
scheduled 600-second soak, or the always-uploaded failure artifacts disappear
from CI. It verifies wiring, not a green remote run; actual job history remains
required evidence.

These tests prove the evaluator rejects the relevant monotonic-growth shapes;
they do not intentionally leak resources from the test runner process.

## Evidence and limitations

On 2026-08-25, the local 30-second change-gate scenario plus headless Vulkan
smoke completed 322 iterations with 61 samples and all ten checks in the
then-current runner passing. It peaked at 33.9 MiB RSS, measured 7.0 MiB
first-to-last-quartile RSS growth and zero thread/fd growth, emitted its
completion marker, cleaned up children, retained 16.0 MiB of scrollback, and
kept images at 512 KiB. The artifact is `target/soak/issue-23-final.json`; it is
historical local evidence, not a checked-in baseline or a scheduled/release-
duration result. It predates the fail-closed queue-drain fields and therefore
does not satisfy the current acceptance contract.

On 2026-09-02, the current fail-closed contract completed a fresh 30-second
core run plus headless GL submission in
`target/soak/issue-23-current-core.json`. It recorded 61 resource samples and
524 complete workload iterations, peaked at 42,900 KiB RSS, and measured 4,680
KiB first-to-last-quartile RSS growth with zero thread/fd growth. All children
were cleaned up; all 68 transcript entries per iteration were accepted with no
drops, the queue peaked at 10,526 bytes and drained to zero after every durable
finish, and no GPU validation/device error was present. All 18 current checks
passed. This replaces the obsolete local artifact for the short core lane, but
it still does not substitute for either ten-minute candidate soak.

The same working tree then completed the full 600-second core duration in
`target/soak/issue-23-current-core-10m.json`: 1,200 resource samples, 9,160
complete iterations, 42,996 KiB peak RSS, only 244 KiB quartile-to-quartile RSS
growth, zero thread/fd growth, four peak children, a 10,590-byte transcript
queue peak, zero post-finish queue bytes, zero drops, complete child cleanup,
and zero GPU errors. All 18 checks passed. Because the tree is intentionally
dirty roadmap work and this artifact predates the new embedded dirty-state and
binary-digest fields, it is strong local duration evidence but not an accepted
clean-candidate or scheduled-CI record.

The core soak still does not create a window; the new native lane does
so under a private software-rendered compositor. Neither measures vendor GPU
memory, substitutes for a long real-desktop observation, validates macOS/
Windows runtime behavior, or proves every terminal protocol. The native lane is
configured but has no accepted artifact yet. Those remaining boundaries stay
separate platform/manual rows in [platform-support.md](platform-support.md) and
[release-acceptance.md](release-acceptance.md).
