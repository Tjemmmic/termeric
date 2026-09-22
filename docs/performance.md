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

## Headless budget refresh at `2edd584`

The [preserved raw report](../benchmarks/results/2edd584-headless.json) records
clean `2edd584dff72eda42db576fdbe44e22e135466a2`, captured at
`2026-09-19T04:09:00Z`. Its SHA-256 is
`5c9586aae3c05c4b6ba1056b98b451965314b112b1f7d66b7935516ec60f4a7a`.
This is a retained observation, not a replacement for the checked-in baseline.

The existing runner completed one release-suite process per producer, with
five samples per measured metric, on an AMD Ryzen 9 7900X3D / Linux
`7.2.6-arch2-1`. Offscreen rendering used Mesa `26.2.3-arch1.1` llvmpipe through
Vulkan, explicitly selecting the software ICD. No displayed window or AI
provider was launched; PTY fixtures used `/bin/cat` and `/bin/true`.

```bash
CARGO_NET_OFFLINE=true WGPU_BACKEND=vulkan \
VK_DRIVER_FILES=/usr/share/vulkan/icd.d/lvp_icd.json \
python3 scripts/run_benchmarks.py --skip-displayed \
  --output target/benchmarks/2edd584-headless-refresh.json
```

The metric contract passed with 73 rows: 67 measured and six explicitly
skipped displayed-window rows. All 40 budgets with measurements passed.
Three displayed idle CPU/frame-count/RSS budgets were accepted under their
existing `allow_skipped` policy, **not measured successes**. Displayed
key-to-present latency, idle RSS/PSS/private memory, CPU, and frame counts
remain unmeasured by this artifact. The inherited display environment fields
describe the host, not evidence that those probes ran.

Selected timings (milliseconds):

| Workload | Median | p95 |
| --- | ---: | ---: |
| Parse 4 MiB plain text | 190.421 | 218.104 |
| Process 100,000 styled lines | 274.015 | 321.747 |
| 10,000-line history, 50 resizes | 74.773 | 75.522 |
| Cooperative search, 100,000 lines | 76.944 | 77.791 |
| Offscreen frame build/upload/submit | 2.581 | 2.667 |
| One-row renderer-cache update | 0.01236 | 0.01307 |

The 100,000-line fixture retained 117,287,496 estimated compact row-owned bytes
and requested 150,498,169 cumulative allocator bytes in 900,075 calls. The
50-resize fixture requested 145,192,607 bytes in 67,032 calls. Those counts
were identical across their five samples; cumulative requests are not peak
live heap or RSS. Warm steady rendering allocated no new pipeline buffers;
one-cell updates rebuilt one row, and shaping pressure caused no new physical
glyph rasterizations after warmup.

All raw-sample summaries, producer contracts, and 43 budget decisions were
independently recomputed from the saved JSON before recording this evidence.
The artifact does not establish a before/after improvement, vendor-GPU or
compositor latency, leak freedom, multi-process repeatability, or a comparison
with other terminals. In particular, its ordinary reflow fixture does not
isolate the preceding shell-boundary optimization; that has its own
[deterministic work-count evidence](#batched-shell-boundary-resize-mapping).
Native visual/provider acceptance and the broader Linux beta gates remain open.

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
Linux `/proc`, X11, and `xdotool`; ranking additionally requires working
Fontconfig `fc-list`/`fc-match` probes and a literal installed font family.
It uses a fresh process and isolated XDG
configuration per sample, requests the same font family, numeric size, grid,
opacity, padding, retention controls, warmups, and byte stream, and records
every executable and version plus its SHA-256 digest. The artifact records each
adapter's font-size unit. Termeric configures points (96/72 logical pixels per
point) like the other current adapters, but equal numeric inputs are still not
silently treated as proof of equal physical glyph size. Runs recorded before
this change used Termeric logical pixels. Actual initial/final
PTY grids and native pixel geometry remain the comparison authority. Sample
order rotates between terminals on each
round so one participant does not always inherit the coldest or hottest slot.
Adapters force fresh instances with the terminals' supported launch
controls: kitty sessions are disabled, WezTerm uses
`--always-new-process --no-auto-connect`, Ghostty disables GTK single-instance
mode, and Konsole uses `--separate --builtin-profile`. No personal terminal
configuration participates.

Schema 3 adds a per-sample `font_environment` observation, collected in that
participant's isolated XDG environment before starting the process/timer. It
records the requested family's presence in the installed-family inventory,
Fontconfig's resolved family aliases/file/face index, and the font file's
SHA-256. `fc-match` alone is insufficient because it silently substitutes
missing families. Missing tools, probe failures, unreadable files, malformed
records, substitutions, or inconsistent face fingerprints block **all four
rankings**, without discarding otherwise valid raw timing/resource samples.
A bad later sample cannot be hidden behind the first good sample. Generic
aliases or Fontconfig patterns are not proof of a literal installed family.
Schema-1/2 samples lack this evidence and fail the current font-control gate;
historical contract results below describe the runner version used then.

The gate establishes a shared installed/Fontconfig-resolved face, **not proof
that every terminal renderer chose that face**. Every sample explicitly marks
its renderer face `not_observed`; the comparison reports renderer-face
equivalence `not_verified`, even when other controls pass. Renderer-selected
faces and fallback glyphs still need independent evidence before claiming a
font-equivalent ranking. The pre-launch probes may warm font caches: new
startup timings must not be treated as directly interchangeable with older
runs lacking those probes. No package is installed or user font configuration
changed by the runner.

For pixel/grid calibration, `--termeric-font-size` overrides only Termeric's
point size (8–72); the other adapters keep `--font-size` in their recorded
units. `--termeric-line-height` sets Termeric's font-size multiplier
(0.8–3.0). Both the report settings and Termeric's per-terminal controls retain
the requested values. Without overrides, the historical common numeric size
and Termeric line-height multiplier 1.0 are unchanged. There is no implicit DPI
conversion or automatic claim that glyph metrics match.

On the chosen isolated X11 display, this is a starting point based on the
previously calibrated 100×30 / 901×601 geometry, now explicitly requesting
the installed Noto Sans Mono family:

```bash
python3 scripts/compare_terminals.py \
  --terminal termeric --terminal kitty \
  --font-family 'Noto Sans Mono' \
  --font-size 11 --termeric-font-size 14.67 --termeric-line-height 1.3 \
  --window-width 901 --window-height 601 \
  --samples 1 --warmups 0 \
  --output target/terminal-comparison/calibration.json
```

These values are not a portable preset. Inspect the font observations and the
actual initial/final grid and window pixels, retain unsuccessful calibration
records, then collect full warmup/repeated samples with the chosen settings.
Overrides never bypass the comparison contract. Even matching grid/window
dimensions do not prove identical glyph rasterization, chrome/content layout,
renderer backends, or allocator behavior; report those limits with any ranking.
See [the calibrated evidence](#calibrated-resource-comparison-at-5e7d4b9).

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

Equal *missing* pixel dimensions are not a match. Every sample must have
positive integer width/height and no geometry-query errors before either
resource metric can be ranked. Missing, null, boolean, fractional, nonpositive,
or malformed dimensions block resource ranking, even when every terminal has
the same bad values or other samples have matching valid dimensions. Only
valid geometry enters the observed-pixel inventory. The X11 query has a
five-second timeout, rejects missing/duplicate/invalid dimension fields, and
retains query failures explicitly in `window_geometry.errors`; it does not
discard otherwise valid timing or process-resource observations. Optional
coordinates do not establish comparability. Startup and PTY timing eligibility
remain governed by their separate contracts.

Parser/contract and mocked launch-path regressions cover these failures without
launching a real terminal. Re-evaluating the five retained schema-2 native
reports at `49b3fab`, `d6d5261`, and `4081caa` (50 measured samples) leaves all
their comparison contracts unchanged. Their actual pixel sizes still differ;
this fix closes a false-positive path without promoting mismatched runs. The
separate calibrated measurements below establish matching geometry locally.

### Current local comparison evidence

The initial clean baseline follows. The later
[calibrated resource comparison](#calibrated-resource-comparison-at-5e7d4b9)
is the newest local record and also permits RSS/CPU ranking.

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
resource comparisons are absent from this baseline and are addressed for one
local profile by the later calibrated run. Native-hardware repeats and broader
terminal coverage remain open.

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

### Core replay follow-up: multicell repair

Replaying the comparison fixture directly through the release core established
that parser/grid work alone takes substantial time: it does not require PTY
transport or a renderer to reproduce a roughly 63–66 ms workload. Inspection
found that every scrolling line called multicell repair, which scanned the
entire active grid twice even when no OSC 66 descriptors existed. Repair now
walks each row directly on the first pass and skips the second only when no
descriptor exists. An empty set of valid leaders is deliberately insufficient:
orphan and malformed descriptors must still receive the original BCE cleanup.

A local paired replay compared clean `4cacd7d` with this change using the exact
1 MiB Python comparison fixture, a 100×30 core, and 10,000-line/64 MiB retention.
Three process pairs alternated implementation order; each process discarded
one warmup round and retained five rounds with rotated chunk order. Medians
over 15 samples per implementation/chunk size, in milliseconds:

| Input chunk size | Before | After |
| --- | ---: | ---: |
| 4 KiB | 66.86 | 63.27 |
| 64 KiB | 67.78 | 62.85 |
| Entire 1 MiB payload | 68.79 | 62.03 |

Construction, file reads, and destruction are outside timing; parsing the final
CSI-6n query and draining replies are inside. Every replay produced the same
cursor, 8,313 retained rows, and reply. Separate regressions compare all cells
and dirty-row state against the frozen two-pass repair for 512 block-subset /
leader-content cases, and preserve clean ordinary Unicode/wide-cell rows.
This is a roughly 5–10% local core-workload improvement, not an end-to-end PTY,
memory, or cross-terminal gain. It does not explain the entire kitty gap.

The local archive
`target/terminal-comparison/2026-09-17-multicell-repair-replay.tar.gz` retains
the diagnostic source/executables, payload, raw logs, summary, exact source
patch, Cargo artifact paths, and method/hash record. Its SHA-256 is
`b78a3b9d42579fc735b728e185dd642188968a85d1c6736e3e417c549bd64365`.
It is an opt-in local diagnostic, not a new metric in the versioned release
benchmark contract or a published release artifact.

### Native follow-up at `d6d5261`

Two independent private-Sway/Xwayland runs on 2026-09-17 repeated the native
Termeric/kitty comparison after the multicell-repair optimization. Both used
the same launcher/settings as the `49b3fab` run: one warmup plus five measured
fresh processes per terminal, rotated launch order, exact 100×30 initial/final
grids, 1 MiB shared bytes, 10,000-line retention, and llvmpipe OpenGL. All twenty
measured samples completed with complete process-resource observations. Clean
source `d6d5261aac99cc2514d100e823bb38acfa9aafb1` and executable SHA-256
`feb3a86b55ad0fd87f22764e56c41199ac954ed732266f4936abeaa969805f84`
were unchanged before/after each run. kitty remained version 0.48.2 with the
same executable hash recorded above.

Both runs are retained, without selecting the faster result. Timings are in
milliseconds; each row summarizes five samples:

| Run | Terminal | Startup median | Startup p95 | PTY response median | PTY response p95 |
| --- | --- | ---: | ---: | ---: | ---: |
| First | Termeric | 201.43 | 209.65 | 74.66 | 84.21 |
| First | kitty | 365.92 | 410.49 | 27.68 | 30.12 |
| Repeat | Termeric | 204.18 | 223.45 | 71.49 | 73.18 |
| Repeat | kitty | 363.57 | 369.79 | 28.31 | 31.34 |

The new Termeric PTY medians are below the earlier 76.42 ms, but the original
70.19–96.34 ms sample range overlaps both new runs. The old/new native builds
were not interleaved, and kitty's timing also shifted from its earlier 22.85 ms
median. This is encouraging directional evidence, **not a proven end-to-end
percentage improvement** from the repair change. The paired core replay above
remains the narrower optimization evidence. The native results still identify
a substantial PTY-response gap to investigate; they do not justify claiming
that the parser, transport, or renderer alone explains that entire gap.

Startup and PTY rankings pass each run's comparison contract. RSS/idle rankings
remain rejected for the same 700×388 versus 1001×661 native-window mismatch;
complete resource reads do not make unlike pixel workloads comparable. Neither
run promotes native-hardware performance, presentation latency, or daily-driver
acceptance. No AI-provider workflow ran, and both private compositors/runtimes
were cleaned up without interacting with the operator's desktop.

The local archive `target/terminal-comparison/2026-09-17-d6d5261-kitty.tar.gz`
has SHA-256
`7097bb54ae1f8b69190573246cac9864f3053a38dc1dacdaac011e22328ef838`.
It contains the launcher plus `termeric-comparison-d6d5261-measured/` and
`termeric-comparison-d6d5261-repeat/`, each with its original report, source/
binary isolation sidecar, fixture/configuration records, and display/launcher
logs. Caches/data/state are excluded. Original `/tmp/` paths map to these
archive directories; this remains local, unpublished diagnostic evidence.

### Row-local multicell-presence cache

The next core replay isolated another avoidable cost: even after skipping the
second repair pass, every scroll still scanned every unchanged row for OSC 66
descriptors. Each row now caches descriptor presence independently of render
dirtiness. Cell edits and exposed mutable references invalidate it; clearing a
row establishes known absence. Rows retain their cache when moved by scrolling,
while reconstructed rows start unknown. Repair skips only rows known to have
no descriptors; rows with malformed/orphan descriptors still use the original
validation and BCE cleanup. No cached block coordinates or validation results
are reused after structural edits.

Tests cover all 15 row-edit routes with initially absent/present descriptors
and absent/present replacement cells, clone independence, metadata-only edits,
clean-row queries, and unchanged `Send`/`Sync` properties. The cache fits into
the previous row header's padding, with a layout regression check; this is not
a measured RSS improvement. The 512-case frozen repair comparison now also
primes cached absence before inserting descriptors.

The same local paired replay method (three alternating process pairs, one
discarded warmup and five measured rounds per process, 15 samples per chunk)
compared the `7b5d19e` core with this cache. Median milliseconds:

| Input chunk size | Before | Cached presence |
| --- | ---: | ---: |
| 4 KiB | 59.53 | 39.54 |
| 64 KiB | 60.69 | 39.98 |
| Entire 1 MiB payload | 60.95 | 40.63 |

This roughly 33–34% improvement applies to this core-only workload, not native
PTY response or another terminal. Cursor, history length, and reply agreed in
every replay. Two attempted gprofng captures reported unreliable interval-timer
sampling (the initial capture also lacked symbol resolution); their CPU
percentages are explicitly **not** used as evidence. The paired wall-clock
replays ran separately, without the profiler or concurrent builds/tests.

The local archive `target/terminal-comparison/2026-09-17-row-multicell-cache.tar.gz`
has SHA-256
`2dee766f4aba0f55d4f3e6f189d396228f9639c4adcb86104d6bf47bd5b4d2a9`.
It retains source/executables, exact patch/payload, build records, all paired
logs, summary/method hashes, and the rejected profiler captures for diagnosis.
The baseline replay executable is from the preceding `d6d5261` diagnostic;
the core source was verified unchanged between that commit and `7b5d19e`.
This remains local diagnostic evidence, not a release benchmark-contract row.

### Native follow-up at `4081caa`

Two more independent native runs on 2026-09-17 exercised the row-local cache
through the same private Sway/Xwayland, llvmpipe OpenGL, isolated configuration,
and Python PTY fixture. Each again used one warmup plus five measured fresh
processes per terminal, rotated launch order, a 1 MiB payload, and exact 100×30
initial/final grids. All twenty measured samples completed with complete
resource observations. Clean source `4081caa75798dac1d2633e82b9dbc0af919cf8e2`
and executable SHA-256
`a05af7c785118613273ea7ced12a254fc2fa7cf2b8e5255c4dcd5630198184a4`
were unchanged before/after both runs. kitty remained 0.48.2 with the same
executable hash. Both runs are retained; timings below are milliseconds:

| Run | Terminal | Startup median | Startup p95 | PTY response median | PTY response p95 |
| --- | --- | ---: | ---: | ---: | ---: |
| First | Termeric | 185.68 | 186.46 | 48.10 | 50.31 |
| First | kitty | 346.26 | 361.10 | 24.26 | 26.70 |
| Repeat | Termeric | 188.53 | 194.05 | 50.51 | 52.63 |
| Repeat | kitty | 346.90 | 366.33 | 23.60 | 29.88 |

All ten new Termeric PTY samples (46.65–52.63 ms) were below the previous
`d6d5261` samples (67.82–84.21 ms). This is stronger native evidence than the
overlapping ranges after the first repair change: the lower response times
recurred across both runs and agree in direction with the paired core result.
However, old/new native binaries were not interleaved, startup and kitty timing
also shifted, and software-rendering conditions limit generalization. Do not
derive a portable or precisely causal native percentage from these historical
comparisons. kitty still responds to this workload in roughly half the time;
Termeric's lower startup time does not make it the overall faster terminal.

Both per-run contracts allow startup/PTY ranking and reject RSS/idle ranking
for the unchanged 700×388 versus 1001×661 native-window mismatch. No memory,
hardware-rendering, scanout-latency, or daily-driver qualification is implied.
No provider workflow ran; both private compositors/runtimes were cleaned up.

The local archive `target/terminal-comparison/2026-09-17-4081caa-kitty.tar.gz`
has SHA-256
`37ba9fc3d4226ff39c866b482079c61a0bbb7e77c53776836318874bda9b27ff`.
It preserves the launcher plus `termeric-comparison-4081caa-measured/` and
`termeric-comparison-4081caa-repeat/`, including original reports, source/binary
sidecars, configurations, fixture records, and display/launcher logs. Original
`/tmp/` paths map to these archive directories. Caches/data/state are excluded;
this is local unpublished evidence, not a release artifact.

### Calibrated resource comparison at `5e7d4b9`

The explicit calibration controls now permit a locally matched resource
comparison, without weakening any metric guard. The first one-sample attempt
used Termeric size 16/line height 1.3 and kitty 12 points in 1001×661 windows:
Termeric had 29 rows while kitty had 30, so all rankings correctly failed.
The second attempt used the numeric geometry recipe above, still requesting
the historical default DejaVu Sans Mono, and matched both initial/final 100×30
grids and 901×601 native windows. Both attempts are retained as calibration,
not mixed into the measured sample distributions.

Two subsequent runs each used one warmup plus five fresh measured processes
per terminal, rotated order, the shared 1 MiB/8,342-line fixture, 10,000-line
retention, and two-second post-workload CPU observations. All twenty measured
samples had matched geometry and complete process-resource observations; every
startup, PTY, RSS, and CPU comparison contract passed. Source
`5e7d4b9e4da1914de5727d36e564dde8e0f7b3fc` and Termeric executable SHA-256
`a05af7c785118613273ea7ced12a254fc2fa7cf2b8e5255c4dcd5630198184a4`
were clean/unchanged across all four runs. This used the same private
Sway/Xwayland, llvmpipe software rendering, and kitty 0.48.2 executable as the
preceding comparisons. No AI-provider workflow ran; private runtimes were
cleaned up. No application memory optimization was made for this calibration.

| Run | Terminal | RSS median (MiB) | Startup median (ms) | PTY response median (ms) | Post-workload CPU median / max (% of one core) |
| --- | --- | ---: | ---: | ---: | ---: |
| First | Termeric | 185.42 | 194.46 | 49.34 | 0.00 / 11.00 |
| First | kitty | 256.31 | 359.09 | 25.62 | 0.00 / 0.00 |
| Repeat | Termeric | 185.60 | 194.32 | 52.72 | 0.00 / 12.00 |
| Repeat | kitty | 258.29 | 356.60 | 23.73 | 0.00 / 0.00 |

Termeric used less summed process-tree RSS and started sooner in this profile;
kitty answered the byte-stream workload sooner. RSS is resident mappings, not
PSS/private memory, peak allocation, or GPU memory. This limited software-rendered
workload does not establish a general memory-efficiency winner. The changed
font/window configuration also prevents treating these numbers as optimization
percentages against the earlier unmatched runs.

**Font-control limitation discovered in the follow-up:** the requested DejaVu
Sans Mono family is not installed on this host. Termeric's traced renderer
resolves to Noto Sans Mono; kitty's actual selected face was not captured.
The runner verifies requested settings and observed geometry, not matching
resolved fonts. These results therefore pass its existing geometry/resource
contract but do **not** prove a same-font comparison. The numbers above remain
observations of this local profile, not fully controlled font-equivalent
rankings. See the diagnostic follow-up below.

The CPU tail is important: Termeric's third sample in the first run consumed
22 CPU ticks during the observation, and its first sample in the repeat consumed
24 ticks, yielding approximately 11% and 12% of one core. All other measured
CPU samples were zero at this resolution. Neither burst was discarded. Sampling
starts after the fixture response, not after verified presentation/quiescence;
attribution of this post-workload activity remains open. A zero median must not
be presented as uniformly zero idle CPU or absence of background/GPU work.

The archive `target/terminal-comparison/2026-09-17-5e7d4b9-calibrated-kitty.tar.gz`
has SHA-256
`a4d0d9cb7ca35912f399272bd52674625e0ca6b21dc792f545376c1fe1538c6d`.
It includes the launcher, both `termeric-calibration-5e7d4b9-{first,second}/`
directories, and both `termeric-comparison-5e7d4b9-calibrated{,-repeat}/`
directories with original reports, source/binary sidecars, configuration,
fixture records, and display/launcher logs. Original `/tmp/` paths map to the
archive directories. Caches/data/state are excluded. This is local unpublished
diagnostic evidence, not hardware-backed or release-duration acceptance.

### Post-workload CPU diagnostic at `c9dadca`

The next investigation retained the calibrated recipe and unchanged release
executable, then observed only the explicitly launched terminal processes under
another private Sway/Xwayland session. A temporary wrapper sampled Linux
`/proc/PID/task/TID/stat` approximately every 25 ms between the runner's existing
resource snapshots. PID/TID start times, per-thread names/user/system ticks,
process ticks, monotonic/wall timestamps, errors, and both original boundary
snapshots are retained. The common fixture remains excluded. Sampling runs in
the runner, not the terminal; it still perturbs scheduling and is **diagnostic
evidence, not a replacement performance distribution**.

Three runs each excluded one warmup per terminal and rotated Termeric/kitty
order. The first two enabled `RUST_LOG=info,termeric::app=trace`; the third used
the original warning-level logging. All measured terminal process snapshots
were complete, and all thread reads succeeded.

| Run | Measured processes per terminal | Post-response observation | Termeric process CPU tick deltas | Largest observed Termeric CPU (% of one core) |
| --- | ---: | ---: | --- | ---: |
| Traced extended window | 5 | 6 seconds | 0, 0, 1, 1, 0 | 0.167 |
| Traced original window | 10 | 2 seconds | 0, 0, 0, 0, 1, 0, 0, 1, 0, 0 | 0.500 |
| Warning-level original window | 10 | 2 seconds | ten zeros | 0.000 |

The host exposes 100 CPU ticks per second. No run reproduced the previous
22/24-tick observations. All Termeric thread identities stayed stable and no
individual Termeric thread accrued a whole observed tick between its first
and last samples. This does not contradict the occasional one-tick process
delta: process/thread observations have different boundaries and independently
rounded counters. In particular, the Git worker did not show measurable CPU
growth. The frame traces show startup/animation/output frames, with no later
frame-reason lines during the remaining observation windows; their one-second
timestamp precision cannot establish whether the last output frame preceded
the exact resource boundary. This is evidence against a sustained redraw loop
in these samples, **not attribution or a fix for the original intermittent
bursts**. No settling delay was added and no original sample was removed.
Kitty's process CPU deltas were all zero at this resolution; one warning-level
sample changed its thread set, which remains visible in the diagnostic summary.

Tracing also exposed the font-control limitation above. Under a retained
sample's isolated XDG environment, `fc-list :family=DejaVu Sans Mono family`
returned no faces and `fc-match` resolved that request to Noto Sans Mono.
Termeric's renderer independently logged Noto Sans Mono. This does not prove
kitty used the same face. The next comparison-control work should reject
missing requested fonts or explicitly classify substitutions, retain resolved
face evidence where available, and rerun a genuinely installed common family.
CPU-burst attribution remains open; do not optimize a guessed subsystem or
advertise uniformly zero idle CPU from these non-reproductions.

Source `c9dadca8aa25984e1dcf82f1e9b14c06615fda4c` was clean/unchanged in all
three isolation records; executable SHA-256 remained
`a05af7c785118613273ea7ced12a254fc2fa7cf2b8e5255c4dcd5630198184a4`.
No provider workflow ran, and every private runtime was removed. Forced X11
window teardown produced errors in some launcher logs after sampling; these
runs do not qualify graceful-close behavior. The local unpublished archive
`target/terminal-comparison/2026-09-17-c9dadca-cpu-diagnostic.tar.gz` has SHA-256
`6f6e9bdb1c38af7baa0f811a1b1a740d3a14d27e5785712de4d3229a714f0ccb`.
It retains the temporary launcher/wrapper, all three runs including warmups,
thread samples, configurations, fixture/display/log records, font probes,
and a recountable `summary.json` plus `summarize.py`. All 419 archived files
were verified byte-for-byte against their originals. Caches/data/state are
excluded; original `/tmp/` paths map to the archive directory names.

### Native font-control verification at `cd801ad`

The schema-3 gate was exercised under the same private Sway/Xwayland and
llvmpipe setup in three clean-source runs. A one-sample/no-warmup negative run
requested absent DejaVu Sans Mono. Both terminals completed their workloads,
but their retained observations reported the missing family and Noto Sans Mono
substitution; **every ranking was blocked**, despite matching geometry and
matching substituted font fingerprints. This record is not pooled into the
installed-font distributions.

Two subsequent runs each used one excluded warmup and five measured fresh
processes per terminal, rotated order, explicitly requested Noto Sans Mono,
and used the numeric geometry recipe above. All twenty measured samples
matched initial/final 100×30 PTY grids and 901×601 native windows, with complete
resource observations and the common fixture excluded. Every Fontconfig probe
found the requested family and selected face index 0 of
`/usr/share/fonts/noto/NotoSansMono-Regular.ttf`, SHA-256
`65b5e2b2c4a1fba9ae8be1f026cb35b03dcb8886d9b2a4147054fde12f7e767d`.
All recorded comparison controls passed; all reports still explicitly mark
terminal-renderer face equivalence **not verified**.

| Run | Terminal | RSS median (MiB) | Startup median (ms) | PTY response median (ms) | Post-workload CPU median / max (% of one core) |
| --- | --- | ---: | ---: | ---: | ---: |
| First | Termeric | 183.46 | 269.43 | 64.93 | 0.00 / 10.00 |
| First | kitty | 253.20 | 377.43 | 32.37 | 0.00 / 0.50 |
| Repeat | Termeric | 183.57 | 212.42 | 53.43 | 0.00 / 10.00 |
| Repeat | kitty | 257.99 | 361.27 | 28.17 | 0.00 / 0.00 |

These are local observations, not an application optimization or a
renderer-font-equivalent/hardware-backed winner claim. The first run was
particularly variable: PTY response ranged 58.26–170.41 ms for Termeric and
27.10–176.94 ms for kitty. No outliers were discarded and their cause was not
established. In measured sample order, Termeric's process CPU tick deltas were
`[0, 17, 0, 0, 20]` and `[0, 0, 1, 20, 0]` over two-second post-response
windows; kitty's were `[0, 0, 0, 0, 1]` and five zeros. The negative run also
retained a 21-tick Termeric observation. Thus the larger CPU activity recurred
in the normal runner even though the earlier thread-sampling diagnostics did
not reproduce it; attribution remains open. Neither font probing nor the
zero medians demonstrate that the activity was fixed. RSS is summed resident
mappings, not PSS/private/peak/GPU memory. Pre-launch font probes and the new
explicit family prevent treating changes from older results as optimization
percentages.

Source `cd801ad07e3c6bf056f9298538ad4f8867bdf0fa` and the release executable
were clean/unchanged across all three runs. Termeric retained executable SHA-256
`a05af7c785118613273ea7ced12a254fc2fa7cf2b8e5255c4dcd5630198184a4`;
kitty 0.48.2 retained
`940cbe42c8cf1dcb342f20b32ee35f51818bbcd8deffc11029f5766d844783a4`.
The summaries/contracts were independently recomputed from retained samples,
and RSS totals were recounted from their process rows. No AI-provider workflow
ran; all private runtimes were cleaned up. These forced-window-teardown runs
do not qualify graceful-close behavior.

The local unpublished archive
`target/terminal-comparison/2026-09-17-cd801ad-font-controls.tar.gz` has SHA-256
`7183e266d91ee2a986327b01b5b5be57f9b9d9249c55195c0c2f95c197bae60d`.
It retains the launcher and all three `termeric-font-control-cd801ad-*`
directories, including original reports, source/binary sidecars, per-sample
font observations, configurations, fixture records, and display/launcher logs.
All 178 archived files were verified byte-for-byte against the originals.
Caches/data/state are excluded; original `/tmp/` paths map to archive names.

### CPU-tail attribution at `f9ad5d2`

Boundary-only diagnostics reproduced the intermittent post-response CPU burst
without continuous thread polling. Each original resource snapshot was followed
by one PID/TID/start-time-checked `/proc/PID/task/TID/stat` and `schedstat`
snapshot. The first `schedstat` field records scheduled CPU time in nanoseconds,
as specified by the [kernel scheduler documentation](https://docs.kernel.org/scheduler/sched-stats.html).
Raw counters, snapshot timestamps/durations, errors, and identity sets are
retained. This is diagnostic instrumentation, not a new comparison distribution;
the first/last thread observations are not simultaneous with each other or with
the original process counters.

In the first run's ninth measured Termeric process, the original process
counter increased by 21 ticks (about 210 ms at this host's 100 Hz accounting).
Independently summed thread runtime increased by 208.56 ms: 191.81 ms in
`llvmpipe-*` software-rendering workers, 15.94 ms on the main thread, 0.029 ms
in `termeric-git`, and 0.780 ms elsewhere. Thread identities were unchanged.
This attributes that reproduced burst primarily to software-rendering work,
not Git inspection or a provider workflow; it does not provide function stacks.

Further runs enabled the existing opt-in frame-count file, reading count and
mtime from the same open file at each boundary. Selected runs also enabled only
`termeric::app::event_loop=trace` frame reasons. Every run excluded one warmup
per terminal, rotated Termeric/kitty order, used the installed-Noto geometry
recipe, and retained two-second post-response observations.

| Diagnostic variant | Payload | Measured Termeric processes | Large-tail observations | Additional frames in affected windows |
| --- | ---: | ---: | --- | --- |
| Boundary snapshots only | 1 MiB | 10 | One 21-tick burst | Not observed |
| Frame counter | 1 MiB | 10 | None | Zero throughout |
| Frame-counter repeat | 1 MiB | 20 | Four bursts of 21–28 ticks | 17–22 |
| Counter plus narrow frame trace | 1 MiB | 10 | None | Zero throughout |
| Counter plus trace, short output | 4 KiB | 5 | All five, 29–34 ticks | 25–27 |
| Same short output, reduced motion on | 4 KiB | 5 | None; all process deltas zero | Zero throughout |

The four 1 MiB frame-counter bursts finished their last recorded frame
263–351 ms after the initial resource boundary. Their llvmpipe runtime deltas
were 190.74–243.94 ms, versus 22.74–26.52 ms on the main thread and only
0.023–0.039 ms in Git. In the short-output runs, every additional logged frame
had reason `animation`; the final frames occurred 395–429 ms after the first
boundary, with 250.18–301.00 ms of llvmpipe CPU per sample. All Termeric thread
identities stayed stable. Counter/trace non-reproductions are retained alongside
the reproductions, not substituted for them. The 4 KiB experiments are separate
workloads and must not be pooled with the 1 MiB results.

The short-output control changed only the generated Termeric configuration to
`[accessibility] reduce_motion = "on"` while retaining the instrumentation and
workload. It removed the extra frames and llvmpipe CPU growth in all five
samples; each still accumulated roughly 1 ms of other thread runtime, below a
whole process tick. This is an animation diagnostic, **not a recommendation
to disable animations as the fix or a cross-terminal ranking**.

Source inspection identifies a concrete cursor-timing defect:
`App::update_cursor_animation()` repeatedly interpolates by
`min(15 * frame_delta, 1)` until position error falls below 0.01 cells.
Consequently a frame gap of at least roughly 66.7 ms snaps immediately, while
shorter gaps can leave a distance-dependent tail spanning hundreds of
milliseconds. The scheduler requests animation frames while that cursor error
remains. This violates the project's fixed-duration, `Instant`-based animation
rule and explains a timing-sensitive post-output animation mechanism consistent
with the measurements. The next implementation should replace that recurrence
with a bounded ease-out transition, preserve retargeting/new-tab/visibility and
reduced-motion behavior, and test equal elapsed-time results across different
frame schedules. Do not claim that these diagnostics fixed the interpolation,
explain every historical burst, establish GPU completion, or measure steady
idle: the current resource window still starts at the parser response and
includes presentation/animation work. This is not evidence of a sustained
redraw loop beyond the observed short tails.

All six isolation records have clean/unchanged source
`f9ad5d2981f9a6188f4f865494a1a0908e70e076` and the unchanged Termeric executable
SHA-256 `a05af7c785118613273ea7ced12a254fc2fa7cf2b8e5255c4dcd5630198184a4`.
They used private Sway/Xwayland with llvmpipe, not the operator desktop or a
hardware-GPU performance lane. No provider workflow ran; private runtimes were
removed. The local unpublished archive
`target/terminal-comparison/2026-09-17-f9ad5d2-cpu-attribution.tar.gz` has SHA-256
`2f4cbd88df25cc019a901350272ec92de01ba6fc4a12cd04b0a42e8029354d2c`.
It includes the launcher, temporary diagnostic wrapper, recountable summary
and script, and all six run directories under
`termeric-cpu-boundary-f9ad5d2.kk7Maa/`. All 1,025 archived files were verified
byte-for-byte; caches/data/state are excluded. Forced teardown is not
graceful-close acceptance. Original `/tmp/` paths map to archive names.

### Bounded cursor-transition implementation

The cursor's legacy `min(15 * frame_delta, 1)` recurrence has been replaced
with a 150 ms absolute-clock ease-out transition. Repeated target samples do
not restart it; changed targets retarget from the previous curve evaluated at
that instant. The final sampled frame clears pending animation state. New
tabs, cursor visibility transitions, all hidden movement, and reduced motion
snap immediately; existing-tab switches continue to animate. Cursor scheduling
now follows the presented transition rather than comparing against live-grid
coordinates hidden by synchronized output.

Deterministic tests cover identical results at equal elapsed time across sparse
and dense frame schedules, fixed endpoints at different travel distances,
retargeting and cancellation, delayed-frame catch-up, new/existing tabs,
visibility, reduced motion, and synchronized-output ownership. This fixes a
timing/lifecycle defect; it does **not** imply every CPU sample improves. The
old implementation accidentally snapped after a sufficiently long frame gap,
whereas a visible new transition now animates consistently. Native before/after
evidence must retain that tradeoff, rather than comparing only the old slow
tail or hiding new animation work with reduced motion.

### Native cursor follow-up at `8f3154f`

After the cursor fix passed the clean release preflight, the same private
Sway/Xwayland llvmpipe diagnostic was repeated with animations enabled,
installed Noto Sans Mono, 100×30 cells, 901×601 windows, two-second resource
windows, boundary thread snapshots, the opt-in frame counter, and narrow
frame-reason logging. Separate 4 KiB and 1 MiB runs each excluded one warmup
and retained five measured fresh processes per terminal in rotated order.
There were no concurrent builds/tests or source changes during measurement.

| Termeric case | Measured samples | Additional post-boundary frames | Last frame after boundary | Process CPU ticks during 2-second window |
| --- | ---: | ---: | --- | --- |
| Old 4 KiB, counter + trace at `f9ad5d2` | 5 | 25–27 | 395–429 ms | 29–34 |
| Fixed 4 KiB at `8f3154f` | 5 | 9 in every sample | 135–142 ms | 9–11 |
| Old 1 MiB, counter + trace at `f9ad5d2` | 10 | 0 in every sample | Before the boundary | 0–1 |
| Fixed 1 MiB at `8f3154f` | 5 | 9–10 | 132–152 ms | 11–12 |

Every additional new frame had reason `animation`. The time column is relative
to the resource snapshot, **not the cursor transition's start**; rendering can
start before that snapshot and finish on the next available frame after the
150 ms endpoint. New 4 KiB process tick deltas were `[10, 9, 11, 11, 11]` and
1 MiB deltas were `[11, 12, 11, 11, 11]`. Corresponding summed thread runtimes
were 99.32–115.50 ms and 102.91–125.90 ms. Thread identities remained stable,
snapshots had no read errors, and all retained resource/font/geometry controls
passed. Kitty's process tick deltas were zero in both runs; no font-equivalent
cross-terminal ranking is inferred from that observation.

The short workload's formerly long tail is now bounded and consistently
shorter in this local follow-up. The larger workload exposes the cost of
correcting the old accidental snap: it now actually animates and consumes
more post-response CPU than the earlier traced no-animation samples. The old
untraced frame-counter repeat also had four intermittent longer tails, retained
in the previous section rather than pooled into this table. This is a
predictable-duration/lifecycle correction with a measured tradeoff, **not an
across-the-board CPU optimization or zero-idle-CPU claim**. These chronological
runs are not interleaved old/new trials and do not establish hardware-GPU
gains, scanout latency, or a general terminal ranking.

Source `8f3154ff1e258a61cf027308f2a63ce125c641ce` remained clean/unchanged;
both runs used Termeric executable SHA-256
`ac8c5c1bbe2ad3104f8ddda23d0bb97f41a49d3bd7efd00671c8ba679577d751`.
Summaries/contracts, process boundaries, frame counts/reasons, and thread
identity/runtime deltas were independently recounted. No provider workflow ran
and both private runtimes were removed. The unpublished local archive
`target/terminal-comparison/2026-09-17-8f3154f-cursor-motion.tar.gz` has SHA-256
`51872ce629f9c4400f72c22766e3b369aac2b4abab22b4d6f13d7a2ab8002237`.
It retains both run directories, launcher/diagnostic wrapper, and recount
script/summary. All 198 files were verified byte-for-byte. Caches/data/state
are excluded and original `/tmp/` paths map to archive names. Forced teardown
is not graceful-close acceptance; manual visual feel and hardware-GPU resource
testing remain separate checks.

### Reverse and region scrolling allocation rail

Reverse scrolling and full-width region scrolling now recycle the discarded
row's cell buffer instead of allocating another full-width row. Upward regions
starting at row zero still compact the outgoing content into history before
reusing its buffer; history limits and eviction counts are unchanged. Fresh
rows reset content, styles, links, protection, wrapping, and multicell cache
state, with row dirtiness identical to a newly allocated row.

The ordinary test lane includes a thread-local allocation counter:

```bash
cargo test -p termeric-core --locked --test scroll_allocations -- --nocapture
```

It excludes grid construction and destruction, and measures 1,000 iterations
each of reverse scrolling, downward/upward interior-region scrolling, and
row-zero region scrolling with history disabled, at 1, 80, and 240 columns.
Before recycling, each case requested 1,000 allocations: 64,000, 5,120,000, or
15,360,000 bytes respectively on this 64-bit build. After recycling, every case
requests zero allocations. This is cumulative allocation traffic, **not** RSS,
peak memory, or a throughput measurement. Archival history still allocates.
The test counts successful allocation, zeroed-allocation, and reallocation
requests only on its own thread, excluding test-runner activity.

A frozen pre-change reference separately checks exact visible cells, row
metadata, compact history, eviction counts, and history byte accounting across
empty/single/multi-row grids, invalid regions, repeated scrolling, and line or
byte eviction limits. This does not establish an end-to-end PTY speedup or
change the native comparison conclusions above.

### Batched shell-boundary resize mapping

Resize previously captured each OSC 133 boundary by scanning from the oldest
retained row, then repeated another oldest-to-target scan for every restored
boundary. Prompt navigation performed additional separate scans. With `R`
retained rows and `B` boundaries, the mapping alone could visit `O(R × B)` rows.
Capture and restoration now sort bounded target batches and each walk history
at most once: `O(R + B log B)` row/target operations, with `O(B)` temporary
metadata and no history copy. Compact history supplies cached wide-unit counts;
counting units in visible rows can additionally inspect their cells. Prompt
navigation uses the same resulting row map. Empty or
invalid-only capture batches and empty restoration batches perform no row
lookups. The exact logical-prefix eviction translation remains unchanged.

Run the deterministic diagnostic with:

```bash
cargo test -p termeric-core --locked shell_anchor_mapping_scans -- --nocapture
```

For the same 1,001-target, 10,001-row round trip, the pre-change mapper visited
10,012,002 rows and the batched mapper visits 20,002. The test enforces at most
one scan per direction, rather than a timing threshold. The local instrumented
test observations were 21.22 ms before and 99.71 µs after; these single samples
include test-only counting and are **not** production latency, whole-resize,
PTY-throughput, RSS, or hardware performance claims. Setup and grid destruction
are outside the measured mapping interval.

`batched_shell_anchors_match_scalar_mapping_through_reflow_and_eviction`
compares the preserved scalar mapper across column/height changes, zero/capped
history, wide and combined text, empty lines, duplicate/unsorted/invalid targets,
and anchors inside or beyond logical-line ends. Existing independently labeled
command-ID, navigation, partial-wide-line eviction, and active-completion tests
remain part of the ordinary lane; the optimization must not re-authorize
evicted command text.

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
