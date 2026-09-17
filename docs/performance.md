# Performance measurement

Termeric has a versioned release benchmark program, not a blanket optimization
claim. Compact byte-bounded scrollback, event-driven idle rendering, persistent
GPU buffers, and incremental terminal-row rendering now have measured
improvements. Git/transcript blocking work is isolated behind bounded workers;
a separate resource-soak lane now measures process growth without conflating a
short passing run with proof of multi-day stability.

Cargo artifact disk use is measured separately from runtime memory. See
[`build-footprint.md`](build-footprint.md) for the line-table development
profile, opt-in full-debug profile, deterministic target-directory reporter,
and explicit cleanup boundary.

## Run the suite

From a release-capable Linux checkout, one command runs the core, interaction,
renderer, PTY, tab/pane, memory, and displayed-idle workloads and writes a
machine-readable artifact:

```bash
WGPU_BACKEND=vulkan python3 scripts/run_benchmarks.py \
  --output target/benchmarks/latest.json
```

To measure llvmpipe through Mesa's surfaceless GL path on a host without a
software Vulkan ICD, use:

```bash
WGPU_BACKEND=gl EGL_PLATFORM=surfaceless LIBGL_ALWAYS_SOFTWARE=true \
python3 scripts/run_benchmarks.py --skip-displayed \
  --output target/benchmarks/llvmpipe-gl.json
```

Use `--skip-displayed` on a headless machine (`--skip-gui-idle` remains a
compatibility alias). Otherwise the runner builds and opens Termeric with an
isolated temporary configuration, samples `/proc` CPU ticks, RSS, proportional
set size (PSS), and private memory while the window is untouched, injects five
sequential X11 keys when `xdotool` is available, and then terminates it. The
offscreen renderer workload still runs without a display when a headless WGPU
adapter is available. It exercises Termeric's real glyph cache, text and
rectangle vertex construction, GPU upload, render pass, submission, and
completion; it does not include surface presentation or compositor latency.

Displayed CPU, RSS, PSS, private-memory, and key-to-present metric rows remain
in every artifact. When the lane is disabled or unavailable their sample lists
are empty and the workload field records the exact skip reason. A Linux host
that can launch the display probe but cannot read `smaps_rollup` keeps RSS and
marks only PSS/private memory explicitly unavailable.

Every timing workload produces five samples. The artifact records raw samples,
minimum, median, p95, maximum, mean, coefficient of variation, workload text,
adapter context, commit/dirty state, Rust/Cargo versions, OS/kernel, CPU, logical
CPU count, display session, and WGPU backend. Only release builds are accepted.

[`benchmarks/metrics.toml`](../benchmarks/metrics.toml) is the versioned metric
inventory. Each core, application/renderer, and displayed producer invocation
must emit exactly one row for every metric assigned to it, with the declared
unit and five finite, nonnegative samples. An unavailable optional renderer or
display probe must still emit an empty row whose workload starts with
`skipped:`; a missing row, unexpected row, duplicate, unit drift, wrong sample
count, missing required adapter/input context, or implicit empty result fails
the run. The JSON artifact retains the contract version, path, pass state, and
exact errors. Multi-process repeats must also retain exactly one record per
producer and identical workload and context values. An adapter, backend, input
route, or fixture change therefore fails before unlike samples can be merged
into one statistic. Renderer context includes the adapter name, backend, device
type, driver identity, vendor, and device identifiers rather than relying on a
potentially ambiguous display name alone.

To verify repeatability across five complete processes and compare with the
checked-in baseline:

```bash
WGPU_BACKEND=vulkan python3 scripts/run_benchmarks.py \
  --repeat 5 --skip-displayed \
  --baseline benchmarks/baselines/2026-08-25-linux.json \
  --output target/benchmarks/five-run.json
```

Do not compare debug builds, mix cold and warm results, or compare different
renderer adapters as if they were the same machine. Baseline comparison is
fail-closed at the evidence layer: architecture, CPU identity, logical CPU
count, metric units, nonempty exact workload descriptions, and per-metric
adapter/input context must agree. Missing, invalid, changed, or explicitly
skipped workload descriptions cannot produce a comparison percentage, even
when both records reuse the same metric name and unit. Matching rows remain
comparable when another row is rejected; if no rows match, the comparison is
explicitly unavailable. The artifact records every skipped row and its reason;
CI budgets still run when a historical baseline belongs to a different host,
but no cross-host percentage is emitted. Keep the complete JSON artifact in
CI/release evidence; a Markdown table alone discards useful context.

This comparison checks declared workload identity, not hidden fixture changes.
Producers must update the workload description or versioned context whenever
fixture inputs or measurement boundaries change; identical labels alone are
not a content hash or proof of equivalent runtime load.

## Workloads and boundaries

The current suite measures:

- 4 MiB printable parsing and a 2.4 MiB escape-heavy truecolor/OSC 8 trace;
- 10k and 100k styled scrollback processing, process RSS deltas, deterministic
  compact-memory categories, allocator bytes/calls for the 100k workload, and
  representation density for plain, styled, Unicode, hyperlink, and sparse
  rows;
- 10k-history reflow across 50 resizes, including allocator bytes/calls during
  the resize sequence;
- storage of 64 raw 256×256 RGBA images under the core image-memory cap;
- cooperative search across 10k and 100k lines, recording complete scan time,
  fresh-query keyboard dispatch, and a paused-typist extension after a broad
  completed result, plus an 800,000-occurrence dense workload that records
  completion time and deterministic result-index capacity;
- a real `/bin/cat` PTY write/read round trip;
- eight real PTY-backed tab creations;
- 8-pane and 64-pane layout/divider calculation;
- fresh structural/grapheme validation and warm natural-language lookup across
  the pinned CLDR annotation plus Unicode canonical-name indexes, including
  static annotation bytes and owned bounded result-state bytes;
- controller construction, glyph cache warmup, offscreen rendered frames, and
  synthetic terminal-mutation-to-GPU-completion latency, plus the warmed
  representative glyph-set atlas hit rate;
- static, one-row-update, scroll, search-heavy, Unicode-heavy, and eight-pane
  terminal-row cache workloads, plus steady GPU-buffer allocation counters;
- warmed code-heavy row shaping time and shaping-cache hit rate;
- 1,000 transcript input/output enqueues against an injected 2 ms persistence
  backend, including the explicitly shed-entry count;
- displayed idle CPU, exact rendered-frame deltas, RSS, PSS, and private memory
  when a Linux display session is available;
- native X11 key-event callback through PTY echo, terminal parsing, real GPU
  submission, and `SurfaceTexture::present` when X11 plus `xdotool` are
  available.

OSC 66 uses a four-byte packed descriptor in each occupied cell and keeps its
bounded text payload only in the leader. A regression test keeps the full
`Cell` at or below 64 bytes, compact-history tests require exact descriptor
reconstruction without covered-cell payload duplication, and scale variants
reuse the bounded shaping caches and glyph atlases. Ordinary cells retain the
existing allocation-free descriptor default and text fast path; native
scaled-font quality is intentionally not inferred from these resource checks.

Search and palette query viewports no longer allocate a full-query grapheme
index during painting, IME placement, or pointer hit testing. They locate the
caret with forward iteration, walk backward over the visible prefix, and scan
the visible suffix without retaining an index. Adjacent grapheme slices, the
remaining forward iterator, and the measured prefix width are reused so large
clusters are not segmented repeatedly. The isolated
[`query_viewport` regression](../termeric/tests/query_viewport.rs) compares every
geometry field against the frozen indexed implementation at every scalar caret
across ASCII, combining/ZWJ, regional-indicator, Indic, and zero-width fixtures.
Its thread-local allocator probe counts successful allocation/reallocation
requests only inside the measured region, excluding fixture and harness setup.
The local 4,096-byte ASCII fixture's former implementation requested 196,512
cumulative bytes in 11 calls per evaluation; the 3,328-byte mixed fixture
requested 49,056 bytes in 9 calls. The new implementation made zero allocation
requests across 100 viewport-plus-pointer evaluations for each fixture.
These are allocation-traffic results, not peak heap, RSS, or latency claims;
the query string and caller snapshots still have their existing storage costs.
This deterministic regression is separate from the 73-metric benchmark report:

```bash
cargo test -p termeric --test query_viewport --locked --offline -- --nocapture
```

An opt-in paired CPU diagnostic checks the memory/time tradeoff against that
same frozen reference. It verifies exact geometry before timing, warms both
paths, alternates order, and emits five raw samples per fixture. Each sample is
450 layout evaluations: 50 passes over start/scalar-midpoint/end carets and
1/80/512-column fields. Fixture setup and assertions are outside timing.
It deliberately includes a single 4,001-byte combining cluster: the initial
allocation-free traversal was roughly twice as slow there because it repeated
segmentation, which motivated the boundary/prefix reuse above.

Three local release-mode processes (15 samples per fixture) measured these
medians, in milliseconds per 450-evaluation batch:

| Query fixture | Indexed reference | Boundary-reusing traversal |
| --- | ---: | ---: |
| 4,096-byte ASCII | 23.798 | 12.443 |
| 3,328-byte mixed script | 8.823 | 5.494 |
| 3,584-byte regional indicators | 6.043 | 4.100 |
| 4,001-byte combining cluster | 11.499 | 10.972 |
| 2,688-byte Indic | 8.891 | 7.591 |

These are local microbenchmark diagnostics, not sealed release acceptance,
portable timing budgets, native input latency, or comparisons to other
terminals. The separate allocation regression still requires zero requests,
including the long combining and regional-indicator fixtures. Reproduce the
paired run without other build/benchmark work competing for CPU:

```bash
cargo test -p termeric --test query_viewport --release --locked --offline \
  paired_query_viewport_cpu_benchmark -- --ignored --exact --nocapture
```

The synthetic key-to-submit metric intentionally excludes OS keyboard delivery,
shell scheduling, surface presentation, and compositor latency. The separate
`latency.displayed_key_event_to_present` metric starts inside the native Winit
key callback immediately before the PTY write and stops only after echoed text
has been parsed and the rendered surface has called `present`. It includes
shell/PTY and real surface-submission cost but still excludes physical-device
delivery before the callback and compositor scanout after `present`; it is a
diagnostic until comparable native baselines justify a portable budget.
Portable GPU-idle utilization is not available from WGPU. The displayed lane
therefore requires zero application-rendered frames during unchanged idle
sampling as the portable submission-work rail; record vendor-tool results
separately with the exact adapter and driver.

Displayed RSS comes from `/proc/<pid>/status`. PSS and private memory come from
one strict `/proc/<pid>/smaps_rollup` parse per sample; private memory is the
sum of `Private_Clean`, `Private_Dirty`, and optional `Private_Hugetlb`. The
parser requires unique nonnegative integer `kB` fields and never substitutes
zero for missing or malformed values. All sampled memory rows name their
display backend and measurement source, so old context-free rows or X11 and
Wayland rows do not compare automatically. PSS apportions shared mappings
among their current users, while private memory includes more than Rust heap;
neither value estimates vendor VRAM.

The displayed runner treats `XDG_SESSION_TYPE` as the requested backend. It
clears inherited selectors for the other backend, selects Winit's backend, and
pins `EGL_PLATFORM` before launch. On X11, deliberately empty Wayland variables
also defeat WGPU's implicit `wayland-0` probe. Set `XDG_SESSION_TYPE=x11` for an
Xvfb run or `XDG_SESSION_TYPE=wayland` for a native Wayland run; this prevents a
benchmark started from a Wayland desktop from silently opening on that desktop
instead of the isolated X11 server.

## Budgets and CI

[`benchmarks/budgets.toml`](../benchmarks/budgets.toml) contains the initial
regression rails. Deterministic CI primarily compares related workloads from the
same run (100k/10k history, 100k/10k search, and 64/8 pane scaling), which is
more stable than transplanting absolute nanoseconds between machines. Broad
absolute safety rails remain for 10k/100k incremental search, 100k RSS,
displayed idle RSS, compact ownership, Unicode annotation validation/lookup,
repeated reflow, and offscreen frame completion. Search has
separate 10 ms p95 fresh-query keyboard-dispatch rails at 10k and 100k, 10/25
ms completed-result extension rails, and broad 50/500 ms completion rails. This
distinguishes total linear work from user-visible input blocking and catches
candidate-copy/cleanup regressions that a scale ratio alone would hide. See
[scrollback-memory.md](scrollback-memory.md) for the representation and its
measured memory/CPU tradeoff.

The Unicode picker has separate 25 ms p95 annotation-validation and 75 ms p95
warm keyword-query rails. Its required `latency.unicode_picker_exhaustive_batch`
row additionally enforces a 250 ms **maximum** for each of five complete batches
of `letter`, `black`, `arabic letter`, `mathematical symbol`, and `cjk unified`.
The canonical-name and annotation indexes are warmed before those batches;
annotation validation retains its separate rail. A single oversized batch,
missing metric, or attempted skip fails. Its immutable CLDR binary must remain at or below
768 KiB, and each final deduplicated result set must remain at or below 1 MiB
of owned candidate/action/command storage. These are broad regression limits;
the exact source, zero-copy format, normalization, and insertion semantics are
documented in [cldr-annotations.md](cldr-annotations.md).

Named picker searches now execute on a bounded background worker, with one
in-flight scan and one coalesced latest query. These existing timing rails still
measure synchronous search work (worker completion cost), not keyboard dispatch
or end-to-end presentation. Blocked-worker lifecycle tests prove that edits do
not wait for scan completion; they are not a native input-latency measurement.

Unicode name scoring has an opt-in paired microbenchmark and complete-catalog
ranking comparisons against its former three-pass implementation. The new
single-pass classification reduces repeated scans without another retained
cache; index/result ceilings and latency rails are unchanged. See the
[Unicode search measurements](unicode-support.md) for the command and locally
observed results. Run timing checks without simultaneous software-rendered GUI
workloads: concurrent runs exceeded the former correctness-suite 250 ms check,
while unchanged standalone runs passed. Wall-clock enforcement now runs in the
existing serial release performance lane, with the same five-name-query ceiling
checked against the worst of five samples. Ordinary tests retain exact catalog
size, index/result byte ceilings, nonempty results, and the 256-result bound;
they no longer interpret unrelated test-thread scheduling as search work.
This separation does not make performance runs immune to host contention.
Retain failed artifacts rather than loosening limits or treating a rerun as
proof of performance under contention.

Displayed idle RSS has a deliberately broad 512 MiB p95 rail. The row may be
explicitly skipped with the whole display probe, but a present oversized sample
fails. This catches catastrophic process regressions while leaving PSS/private
memory diagnostic until repeated clean artifacts from comparable runner classes
justify tighter limits. The longer displayed soak retains its separate 768 MiB
peak and growth rails.

Dense search additionally has a one-second p95 completion rail and an 8 MiB
hard result-index rail. Exact total occurrences and matching rows continue
through the entire scan, but highlighting/navigation retain the latest 131,072
occurrences in a circular window. The UI labels that state as `recent`, and
filter rendering builds only the focused terminal's visible matching-row slice.
The deterministic byte metric covers retained occurrences and the complete
matching-row vector; allocator/runtime and transient per-row matcher storage
remain visible in process RSS rather than this owned-capacity counter.

The offscreen suite requests WGPU's fallback adapter first, so the broad 100 ms
p95 frame-build/upload/submit rail is also the minimum software-renderer
usability budget. It excludes native surface presentation and does not imply
GPU-equivalent performance. See [renderer-recovery.md](renderer-recovery.md).

The renderer budgets also require zero new text/rectangle buffers after an
equal-size warmup, zero cached-row buffer allocations on unchanged frames, and
exactly one rebuilt row after a one-row mutation. Latency rails cover one-row
updates and unchanged eight-pane lookup. See
[incremental-rendering.md](incremental-rendering.md) for cache keys,
invalidation, draw ordering, diagnostics, and measured results.

The ligature workload builds 1,000 warmed code-token runs per sample and records
both elapsed time and cache-hit percentage. It is diagnostic until repeated CI
artifacts justify a stable machine-independent latency budget; the test itself
requires at least a 99% hit rate. See
[`ligature-rendering.md`](ligature-rendering.md).

The transcript producer budget requires the full 1,000-entry slow-writer
workload to remain below 25 ms p95. Its shed-entry companion metric is evidence
that the queue degraded explicitly rather than hiding work or growing without
bound; it is not a target for normal persistence loss. See
[background-io.md](background-io.md) for queue limits, Git cancellation
generations, flush policy, and failure semantics.

### Shaped-run byte-pressure cost

Six `shaping_pressure` metrics compare identical sets of 128 distinct,
2,048-glyph ASCII runs in Fira Mono. The production 8 MiB run cache must evict
under this working set; a **test-only** 64 MiB reference retains all runs.
The reference is not an application setting or a recommendation to raise the
production limit. Both use the real shaping/cache path and an already-warmed
physical glyph atlas, isolating run-cache churn from glyph rasterization.

One unrecorded pair precedes five measured pairs, with alternating order.
Each case clears the run cache and prefills the identical input outside timing.
Timing covers one further pass including shaping/cache lookup, run cloning,
and returned-vector allocation; fixture/font setup, diagnostics, exact
per-glyph geometry comparisons, and returned-output destruction are excluded.
Every pass must return all glyphs with identical atlas entries and offsets.
Assertions require all reference hits, all pressure misses, real byte-driven
evictions below the entry-count limit, no bypass/deferred runs, and no new
physical rasterizations or atlas evictions.

The six rows report pressure/reference milliseconds and final owned cache
bytes, plus pressure-evicted entries and physical rasterizations. Deterministic
rails enforce the production 8 MiB ceiling and zero rerasterizations; timing
is diagnostic until repeated comparable-host data justifies a useful rail.
Owned bytes exclude map bucket overhead and transient/caller/font-system
allocations, so neither row measures peak heap or RSS. GPU adapter identity,
fixture revision, and font SHA-256 are retained as comparison context; absence
of an adapter produces explicit skipped rows. This workload does not
measure atlas eviction, native frame latency, mixed fonts, or typical terminal
row lengths; its deliberately long rows force byte pressure with few entries.
Run it through the release suite above with `--skip-displayed` to keep manual
desktop checks deferred.

Font-family span construction now resolves identical adjacent **complete
graphemes** once per call and allocates family-name ownership only at an
actual span boundary. Previously it repeated font coverage lookup and created
a temporary family-name string for every grapheme, even when all names were
immediately merged into one span. The shortcut retains only the last borrowed
grapheme/name during the call; it adds no persistent cache or resource budget.
Range overrides and ordered fallback still require complete grapheme coverage.
Tests compare the former uncached routing algorithm across combining marks,
Arabic, emoji sequences, default ignorables, missing faces, and range policies;
separate resolver-call tests cover repeated and alternating input. The pressure
workload above measures the resulting rebuild cost without changing its input,
font fingerprint, cache limits, or correctness assertions.

The scale ratio uses deterministic owned-byte estimates rather than RSS deltas.
Allocators commonly retain freed pages between the five samples in one process,
so later RSS deltas can be zero; p95 RSS remains the external peak safety rail.
The ordinary core producer also installs a release-test-only counting wrapper
around the system allocator. Its cumulative requested-byte and successful-call
deltas expose allocation traffic for 100k scrollback construction and repeated
reflow without changing application builds. They are diagnostic until several
same-host artifacts justify tighter regression budgets; they measure requested
traffic, not peak live heap or allocator-retained pages. Broad deterministic
rails cap 100k construction at 256 MiB and 2,000,000 calls and repeated reflow
at 256 MiB and 200,000 calls. DHAT remains the site-level tool and intentionally
replaces this wrapper when its feature is enabled.

The initial counting run exposed 6.90 GB and 23.94 million allocation calls for
the 10k-history/50-resize fixture. Hard-broken compact rows that already fit the
new width now retain their content/style buffers and only update width metadata.
On the same release build this reduced the fixture to 132.1 MB, 67,493 calls,
and an 86.5 ms median, down from 6.90 GB, 23.94 million calls, and 739.4 ms.
Wrapped lines and rows that cross the new boundary continue through the full
Unicode-wide-cell repairing reflow path.

The initial 100k styled construction run requested 937,991,979 bytes through
5,888,926 allocator calls per sample. Recycling the full-width viewport row
buffer instead of dropping and recreating it removed the dominant byte-volume
site. Symbol-preserving DHAT then identified per-scalar REP `String` ownership
and heap-built CSI/SGR parameter views as the dominant remaining call sites;
single-scalar REP state is now inline and VTE's bounded 32-value parameters are
viewed through stack storage. The same release fixture now requests 144,402,609
bytes through 900,075 calls, reductions of 84.6% and 84.7%, with 192.1 ms median
ingestion on the measurement host. Compact owned history remains unchanged at
115,688,120 bytes, so this reduces transient traffic rather than weakening the
history-retention contract.

`python3 -m unittest scripts.test_run_benchmarks` includes a deliberate 21%
slowdown against a 20% threshold and proves that the budget evaluator rejects
it. The same tests prove that absent metrics cannot exploit `allow_skipped`,
while present, explicitly skipped rows can. They also reject missing,
unexpected, duplicate, unit-drifted, incorrectly sampled, and context-free
metrics. Push/pull-request CI runs those tests, the release workloads, and
uploads the JSON artifact even when a budget fails. The committed baseline is
a comparison reference, not a universal pass/fail threshold.

### Passive streamed-diff observer

Four required release metrics isolate the passive diff observer used by
plugin-owned terminals. They never launch a provider, PTY, or window:

- `latency.diff_observer.plain_4mib`: 4 MiB of plain 128-byte LF-terminated
  lines, delivered in 4096-byte reads, with no captures or rejection notices.
- `latency.diff_observer.patches_bulk` and `patches_fragmented`: the identical
  stream of 64 small plus sixteen 200-line patches, each large data line
  containing 340 copies of `界`. SGR-decorated headers/data, OSC-decorated
  prompt boundaries, and CRLF exercise framing with 4096- versus 257-byte
  reads. All 80 completions, stable-ID totals, total normalized raw lengths,
  final raw/parsed content, and byte-pressure eviction are checked.
- `memory.diff_observer.retained_history`: independently recounted final
  history storage after that stream. Both read sizes must retain the same
  storage, at least 3 MiB and no more than the hard 4 MiB history ceiling.
  This includes string/vector capacities, preallocated slots, and conservative
  inline charges; it is **not** peak live allocation, allocation traffic, or RSS.

Fixture generation, detector construction/destruction, and correctness/storage
diagnostics are outside timing. Timed work includes framing, capture, parsing,
constant-cost completion counters, retention, eviction, and EOF. One unrecorded
warmup precedes five recorded samples with fresh detectors; paired read order
alternates. Initial broad rails cap each whole-stream p95 at 100 ms and the
fragmented/bulk median ratio at 4. These are regression alarms, not claims of
sub-frame dispatch latency or an optimization win. Tighten them only with
repeatable host/CI evidence. The ordinary Rust fixture test and Python tests
prove correctness and reject over-budget, missing, or skipped evidence.

The separate [resource soak](resource-soak.md) checks persistent history peaks
and repeated eviction; this benchmark supplies the missing processing-cost
baseline. Neither measures the transcript callback, full app polling, native
presentation, or an authenticated provider. Use `--skip-displayed` to keep
native display checks deferred while collecting these metrics.

## Comparing other terminals

[`scripts/compare_terminals.py`](../scripts/compare_terminals.py) records an
opt-in, end-to-end X11 comparison against explicitly selected Termeric,
Alacritty, kitty, WezTerm, Ghostty, and Konsole executables. It never installs a
terminal, silently adds an available terminal, or treats an unavailable
selection as a successful comparison. Run it on one otherwise-idle host and
display session; an isolated Xvfb session is preferred:

```bash
cargo build --release --locked
xvfb-run -a -s "-screen 0 1280x900x24" \
  python3 scripts/compare_terminals.py \
  --terminal termeric \
  --terminal alacritty \
  --terminal kitty \
  --terminal wezterm \
  --terminal ghostty \
  --terminal konsole \
  --output target/terminal-comparison/results.json
```

Only name terminals that are intentionally participating. The runner requires
Linux `/proc`, X11, and `xdotool`. It uses a fresh process and isolated XDG
configuration per sample, requests the same font family, numeric size, grid,
opacity, padding, retention controls, warmups, and byte stream, and records
every executable and version plus its SHA-256 digest. The artifact records each
adapter's font-size unit because Termeric configures logical pixels while the
other current adapters configure point sizes; the numeric inputs are not
silently treated as proof of equal physical glyph size. Actual initial/final
PTY grids and native pixel geometry remain the comparison authority. Sample
order rotates between terminals on each
round so one participant does not always inherit the coldest or hottest slot.
Adapters force fresh instances with the terminals' supported launch
controls: kitty sessions are disabled, WezTerm uses
`--always-new-process --no-auto-connect`, Ghostty disables GTK single-instance
mode, and Konsole uses `--separate --builtin-profile`. No personal terminal
configuration participates.

The runner also replaces inherited Wayland/toolkit display selectors and
reexecutes itself once under a private `dbus-run-session` before every version
probe and launch. The Wayland display and socket variables are deliberately
present but empty: Winit 0.30 then selects `DISPLAY`, while libwayland cannot
fall back to the default `wayland-0` socket. EGL is pinned to X11; kitty
receives `linux_display_server x11`, WezTerm
receives `enable_wayland = false`, Ghostty receives `GDK_BACKEND=x11`, and
Konsole receives `QT_QPA_PLATFORM=xcb`. This remains necessary under
`xvfb-run`: a Wayland desktop commonly exports both `DISPLAY` and
`WAYLAND_DISPLAY`, and without explicit isolation a participant could open on
the operator's real desktop, select the wrong WGPU platform, or hand work to an
already-running desktop process and invalidate both isolation and process-tree
accounting. The one private D-Bus daemon is outside each terminal's measured
process tree.

Those controls follow the projects' own command/configuration references:
[Alacritty CLI](https://github.com/alacritty/alacritty/blob/master/extra/man/alacritty.1.scd),
[kitty invocation](https://sw.kovidgoyal.net/kitty/invocation/),
[WezTerm `start`](https://wezterm.org/cli/start.html),
[Ghostty configuration](https://ghostty.org/docs/config), and
[Konsole command-line options](https://docs.kde.org/trunk_kf6/en/konsole/konsole/command-line-options.html).
Treat a changed or rejected flag in an installed version as an adapter failure,
not permission to retry with the user's ordinary configuration.

The shared child writes the same bounded styled ASCII/Unicode stream, requests
the cursor position with CSI 6 n, records its actual PTY grid, and then remains
idle. The artifact keeps four dimensions separate:

- process launch to discovery of a visible window containing a unique OSC 2
  title marker;
- child byte-stream start through receipt of the cursor-position response;
- terminal-owned process-tree RSS after the workload; and
- terminal-owned process-tree CPU while idle.

The common Python fixture is excluded from resource totals. PTY response time
proves parser/PTY response, not compositor presentation. Startup includes child
startup and OSC-title parsing. CPU is percent of one logical core, and RSS is a
sum across the observed terminal process tree rather than the launcher PID
alone.

Schema **2** makes resource observations explicitly complete or unavailable.
Every discovered terminal PID must yield unique, valid RSS/thread fields,
readable descriptor counts, and matching Linux start-time identities before and
after its observation. Missing RSS is not zero, and an unreadable process does
not disappear from the aggregate. Failed discovery or any incomplete process
observation leaves the tree totals `null`, with reasons and successfully read
per-process diagnostics retained. The common fixture is still excluded from
resource totals and retained separately for cleanup. Known PID/start-time
identities also remain available to cleanup when later RSS or descriptor reads
fail; metric ineligibility does not discard process-lifecycle bookkeeping.

Idle CPU requires two complete observations of the same PID/start-time set and
nondecreasing counters for every process. Membership changes, PID reuse,
counter decreases, and incomplete observations produce an unavailable CPU row,
not a zero-clamped delta or a subtraction across different process trees. A
complete final RSS snapshot can remain usable even when the idle interval is
not. Resource summaries retain missing samples as `null` and do not compute
statistics from a cherry-picked subset. Each metric's comparison contract also
requires explicit complete measurement status and finite, nonnegative values;
usable startup/PTY timing rows are independent of resource-read failures.

These checks cover the discovered trees at the two observation points, not an
atomic `/proc` snapshot or CPU used by helpers born and exited entirely between
them. RSS remains summed resident mappings, not proportional or private memory,
and these observations do not measure peak allocation or GPU memory. Synthetic
read/discovery denial, exit/reuse/churn/counter, partial-summary, and launch-path
tests plus a live self-process read exercise this contract without launching an
external terminal or provider. Older schema-1 resource totals lack this evidence
and are not upgraded into schema-2 measurements.

Comparison is fail closed per metric. Initial-grid drift blocks startup
ranking; final-grid or retention drift blocks PTY ranking; native-window pixel
drift additionally blocks RSS/idle-CPU ranking. Raw samples remain useful when
a row is not comparable, but the JSON's `comparison_contract.metrics` says
which rankings are permitted and why. Tune `--window-width` and
`--window-height` if Termeric or Konsole do not reach the requested grid. Keep
the complete JSON and per-run logs; do not combine these external end-to-end
rows with Termeric's internal Rust or WGPU metrics.

### Current local comparison evidence

On 2026-09-17, clean source `49b3fabf62909e4a58183f61495d19131c68b10d`
completed a schema-2 comparison against installed kitty 0.48.2 on the Ryzen 9
7900X3D host. This used a private headless Sway/Xwayland display, not the
operator's desktop or Xvfb. Sway used pixman; GL diagnostics and Termeric's
adapter logs identified Mesa 26.2.3 llvmpipe/LLVM 22.1.8 software rendering.
This is separate from the earlier AMD/radeonsi hardware-soak evidence.

Each terminal had one warmup and five fresh measured processes, with rotated
launch order, isolated configuration/private D-Bus, a shared 1 MiB workload,
10,000-line retention, and two-second idle observations. All ten measured
initial and final grids were exactly 100×30. Only these timing comparisons
are permitted by the recorded contract (milliseconds):

| Metric | Termeric median | Termeric p95 | kitty median | kitty p95 |
| --- | ---: | ---: | ---: | ---: |
| Launch to visible | 194.79 | 201.18 | 361.83 | 379.44 |
| Child stream to CSI-6n response | 76.42 | 96.34 | 22.85 | 25.26 |

Termeric started sooner in this run; kitty processed/responded to this workload
sooner. Startup includes child startup/title parsing, and PTY response does
not measure compositor presentation. Five software-rendered samples do not
establish native-hardware performance, interactive latency, or a general
“faster terminal” winner.

All process-resource observations were complete, with unchanged observed
identities and the shared fixture excluded. Nevertheless, **RSS and idle CPU
are not rankable**: native windows were 700×388 for Termeric and 1001×661 for
kitty. The common numeric font size 12 means logical pixels in Termeric and
points in kitty, not equal physical glyph size. Raw resource observations are
retained without a memory-efficiency claim; the overall `comparable` flag is
false even though the two individual timing contracts pass. Pixel-matched
resource comparisons, native-hardware repeats, and broader terminal coverage
remain open.

The full local evidence is preserved in the ignored archive
`target/terminal-comparison/2026-09-17-49b3fab-kitty.tar.gz`, SHA-256
`69c25b9eeb9414b1adad22829b736db4b1ce74720aefc67f39366bd3b2bb7852`.
It includes calibration and measured JSON, per-run fixture/launcher logs and
configurations, display diagnostics, the private-display launcher, and an
isolation sidecar. Caches/data/state directories are excluded. The authoritative
report is `measured/results.json`; its original absolute temporary paths map
to the retained relative `measured/runs/` tree inside the archive. This archive
is local evidence, not a committed or published release artifact.

The sidecar records identical clean source and executable hashes before and
after the run. Termeric's release executable SHA-256 was
`ee379a4ec1cba1002a7e6c6e411f3fc5dd6416bb22a99866f1038de16badcbbe`;
kitty's was
`940cbe42c8cf1dcb342f20b32ee35f51818bbcd8deffc11029f5766d844783a4`.
This detects observed run-time drift, not signed build provenance. To repeat
the local diagnostic, inspect the archived `run_private_comparison.py` and
its checkout path, then use `--output` with a fresh directory, `--samples 5`,
and `--warmups 1` on a clean release build. It wraps the existing comparison
runner with private Sway/Xwayland isolation; it is not a new portable harness
backend. No installed packages were changed or AI-provider commands invoked.

### Historical dirty-tree comparison

On 2026-09-02, a private-Xvfb run on an AMD Ryzen 9 7900X3D host compared the
current dirty Termeric working tree with Arch Ghostty 1.3.1. The run used one
warmup and five fresh measured processes per terminal, a 1 MiB shared workload,
and an exact 100×30 initial and final PTY grid. Both terminals completed every
sample. Median launch-to-visible time was 171.23 ms for Termeric and 633.65 ms
for Ghostty; median child-stream-to-CSI-6n response was 43.02 ms for Termeric
and 40.67 ms for Ghostty. Those are the only permitted comparisons from this
artifact.

The resource rows are deliberately not ranked. Termeric's observed native
window was 700×388 pixels while Ghostty's was 1000×660 pixels, so the contract
rejected both RSS and idle-CPU comparison even though it retained their raw
samples. This is development evidence from a dirty tree and software/Xvfb
environment, not a release claim or a general “faster terminal” claim. The
full local artifact is
`target/terminal-comparison/2026-09-02-termeric-ghostty.json`;
release evidence should preserve an equivalent artifact from a clean candidate
and intentionally selected terminal versions.

## Profiling recipes

The core resource soak now includes a persistent diff history with 80 synthetic
captures per iteration, count/owned-capacity pressure, eviction reconciliation,
and independently recounted allocations. Schema-5 evidence is required for
this workload; older observations do not cover it. This checks retention limits
and process growth, not provider behavior or renderer/compositor performance.
See [the sustained workload](resource-soak.md).

Build the optimized, symbol-preserving profiling target first so compilation is
never included in a profile and allocation/call stacks are not erased by the
stripped release profile:

```bash
cargo build --workspace --profile profiling --locked
```

### perf and flamegraphs

Record the core benchmark executable:

```bash
cargo test -p termeric-core --profile profiling --locked \
  --test performance_benchmarks --no-run
benchmark_bin="$(find target/profiling/deps -maxdepth 1 \
  -type f -executable -name 'performance_benchmarks-*' -print -quit)"
perf record --call-graph dwarf -- "$benchmark_bin" \
  release_benchmark_suite --ignored --nocapture --test-threads=1
perf report
```

With `cargo-flamegraph` installed, use the same executable arguments:

```bash
cargo flamegraph -p termeric-core --test performance_benchmarks \
  --profile profiling -- \
  release_benchmark_suite --ignored --nocapture --test-threads=1
```

### heaptrack

```bash
heaptrack "$benchmark_bin" release_benchmark_suite \
  --ignored --nocapture --test-threads=1
heaptrack_gui heaptrack.performance_benchmarks.*.gz
```

Record RSS, PSS, and private memory from the benchmark artifact alongside
heaptrack's allocation sites; allocator retention and mapped GPU/runtime memory
make all four views different.

### DHAT

The core benchmark has an opt-in DHAT allocator so ordinary tests are
unaffected:

```bash
cargo test -p termeric-core --profile profiling --locked --features dhat-heap \
  --test performance_benchmarks release_benchmark_suite -- \
  --ignored --nocapture --test-threads=1
```

Open the emitted `dhat-heap.json` with Valgrind's DHAT viewer. Run a normal
artifact immediately before or after it; allocator instrumentation changes
timing, so DHAT results are allocation evidence, not latency baselines.

### Idle CPU/GPU and process growth

Use the runner for `/proc` CPU/RSS. For longer displayed captures, combine
`pidstat -p PID 1`, `ps -o rss,vsz,nlwp -p PID`, and `lsof -p PID` with the
appropriate GPU tool (`intel_gpu_top`, `radeontop`, or `nvidia-smi dmon`). Note
sampling interval, duration, foreground/background state, cursor blinking,
animations, and whether shell output was active. For repeatable
fd/thread/child/RSS growth checks, use `scripts/run_soak.py`; for the actual
displayed process, WGPU surface, resize, and pane-churn path, use
`scripts/run_native_gui_soak.py`. Both retain reconciled privacy-safe
descriptor/thread/child category trends so aggregate failures identify the
resource class without exposing fd targets or process arguments. Their
scenarios and ceilings are documented
in [resource-soak.md](resource-soak.md). Vendor GPU memory utilization and
long real-desktop surface/compositor soaks remain manual platform
evidence.

Core-soak evidence is only accepted after
`scripts/validate_soak.py <report> --source .` re-hashes its log and tested
binary, verifies the exact clean checkout, and reconstructs all summaries and
checks. Displayed/native evidence uses
`scripts/validate_native_gui_soak.py <bundle>/results.json --source .` to verify
its exact bounded artifact manifest and independently reconstruct transcript,
log, resource, and outcome checks. The two record kinds are intentionally not
interchangeable.

See [event-driven-rendering.md](event-driven-rendering.md) for scheduler design,
frame-reason diagnostics, and the measured Wayland/X11 before-and-after result.
See [incremental-rendering.md](incremental-rendering.md) for row-cache and GPU
buffer diagnostics and benchmark interpretation.

## Adding a workload

Keep correctness assertions beside each measured operation. Emit raw samples,
not a precomputed average; add environment/context fields; add a same-run scale
or control ratio when possible; prove any new threshold rejects a deliberate
regression; and update the feature-support boundary without claiming an
optimization before measurements demonstrate it.
