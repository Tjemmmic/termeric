# Incremental terminal rendering

Termeric keeps terminal-row geometry and GPU buffers across rendered frames.
This work is tracked by
[#21](https://github.com/Tjemmmic/termeric/issues/21) and complements the
[event-driven redraw scheduler](event-driven-rendering.md): the scheduler avoids
unnecessary frames, while the row cache reduces the work in frames that are
needed.

## Architecture

Each visible terminal row is keyed by the grid identity and visual row. Its
signature covers the row's cells plus every renderer input that can affect its
appearance: viewport mapping, pane geometry and focus, cell size, brightness,
selection, search highlights, URL decoration, palette generation, and layout
position. All four pane clip boundaries participate explicitly, so sub-cell
divider movement cannot reuse text with stale clipping even when the terminal
column/row count is unchanged. An unchanged signature reuses that row's
rectangle and glyph geometry and its four capacity-managed GPU buffers. A
changed signature rebuilds and uploads only that row.

Cursor, scrollbar, pane-divider, tab, panel, toast, and other UI geometry remain
dynamic overlays. The draw order is terminal row backgrounds, dynamic base
rectangles, terminal images, terminal row text, and dynamic base text, followed
by higher UI layers. This retains the existing occlusion model while allowing
the terminal rows underneath it to be reused.

The general text, rectangle, and image pipelines also retain growable vertex
and index buffers. Equal-size or smaller uploads use `Queue::write_buffer`;
buffers grow to a power-of-two capacity only when required. Cached terminal
rows are retained for 120 rendered frames when the cache exceeds 4,096 entries,
which bounds stale tab/pane data without discarding normal visible working sets.

Cached text batches retain their glyph-atlas page references. Reusing a row
pins those pages for the current frame, while atlas eviction advances a
generation in every row signature. This prevents page recycling from changing
already-built geometry and causes stale prior-frame rows to rebuild safely. See
[glyph atlas lifetime](glyph-atlas.md).

Search highlight lookup is indexed by row and interval rather than scanning all
matches for every cell. URL detection joins visible soft-wrapped rows and
fingerprints each logical group's text, row boundaries, and byte-to-cell spans,
so unchanged geometry skips the detector without retaining stale columns after
wide-cell changes. Synthetic wide-wrap padding and wide spacers add no text;
real spaces and hard breaks still separate URLs. Detected intervals are ordered,
made non-overlapping, and binary-searched for cell lookup; each pane has its own
cache. When a URL contains another recognized scheme, the outer URL owns the
nested cells. A multirow URL has one shared payload referenced by each covered
row, not one copy of its string per row. Temporary mapping storage is bounded
by visible cells, and no offscreen history scan or persistent text model is
introduced. Cache metrics count logical-group scans/hits. Tests cover schemes
split across rows, suffix edits, hard-break-only changes, Unicode geometry,
resize, and compact history scrolled into view. Viewport edges still bound URL
recognition; this does not establish complete URLs continuing beyond the view
or change supported URL characters, schemes, or opener policy.

Ctrl+click resolves OSC 8 targets against the presented viewport, not live cells
hidden behind synchronized output. Live scrollback input coordinates are first
converted back to visual rows; frozen presentation grids have no history of
their own. Focus changes likewise rebuild detected-URL caches from presentation
state. Hyperlink collection retains IDs referenced by the frozen viewport in
the existing bounded registry, without cloning a second URL table; after release
they become eligible for ordinary collection. Regressions cover hidden target
replacement, collection, scrolled snapshots, alternate-screen switching,
timeout/release, and single-terminal/split-pane target resolution. These tests
resolve targets without opening them and do not establish native click/opener
acceptance. Keyboard selection and quick select's separate snapshot/action
contracts are documented in [copy mode and quick select](copy-mode.md). They
reuse the existing presentation grid rather than cloning a second scrollback
model. Standalone search follows the [search presentation contract](focused-pane-routing.md#search-presentation);
native selection follows the [mouse selection presentation contract](focused-pane-routing.md#mouse-selection-presentation).

URL scan scratch mapping coalesces contiguous one-byte/one-cell text into one
span per ASCII run. Row edges, skipped columns, wide glyphs, combining sequences,
and OSC 66 geometry keep distinct spans; endpoint lookup translates byte offsets
within a run back to exact columns. A dense-cell reference checks every byte
across Unicode, compact history, narrow grids, and OSC 66 cases. A 100×30 fully
wrapped ASCII grid retains 30 mapping records rather than 3,000.

The isolated `url_cache_memory` test counts allocations on its own test thread
after warming the production cache. On the local 64-bit Linux dev build, ten
unchanged scans requested 3,357,040 cumulative allocation bytes in 210 calls
before coalescing (production base `f408382`), versus 110,640 bytes in 140 calls
afterward—about 96.7% fewer requested bytes for this case. The regression gate
allows at most 32 KiB per hot scan, including text scratch. This excludes initial
grid/cache construction and is not peak memory, RSS, GPU usage, or native frame
latency evidence. Non-linear cells still need individual records; their linear
tag costs one extra machine word per record on this target, so this is not a
universal Unicode-memory reduction claim.

```sh
cargo test -p termeric --test url_cache_memory --locked --offline \
  hot_wrapped_ascii_url_mapping_has_a_small_allocation_budget -- --nocapture
```

## Invalidation

The renderer clears all cached rows when any global rasterization or geometry
input changes:

- surface dimensions or display scale;
- font size, family, fallback list, normal/bold weight, line height, or ligature setting;
- explicit glyph-cache reset;
- terminal padding; or
- theme and terminal palette.

Normal cell output, scrolling, selection, search/current-match state, detected
URLs, pane focus, and pane layout do not require a broad clear. Their row
signatures invalidate only the affected cached entries. Cursor blink and UI
animation never invalidate terminal rows because they are separate overlays.

## Correctness and diagnostics

Unit tests cover cache hit/miss accounting, one-row invalidation, search interval
boundaries, URL detector reuse/replacement, mapping-only invalidation, and
position-indexed URL lookup. A clip-signature regression test varies every pane
boundary independently. The release renderer benchmark builds and retains a
row, then asserts a forced full CPU rebuild produces the exact cached rectangle
and text batches. It also uploads and draws cached row buffers in a real
offscreen WGPU render pass.

At trace log level, each rendered frame reports terminal-row cache hits, misses,
entry count, and cumulative row-buffer allocations, followed by the aggregate
text/rectangle/image pipeline allocation and upload counters. These counters are
intended for diagnosing invalidation storms and unexpected capacity growth.

## Benchmark evidence

The 2026-08-25 release run in
`target/benchmarks/issue-21-final-wayland.json` used Vulkan on the acceptance
host's AMD GPU. Five samples produced:

| Workload | Median | p95 |
|---|---:|---:|
| 40 unchanged cached rows | 0.00056 ms | 0.00087 ms |
| One changed row among 40 | 0.0222 ms | 0.0278 ms |
| 40 remapped rows during scroll | 1.220 ms | 1.414 ms |
| Search highlights changed on half of 40 rows | 0.428 ms | 0.534 ms |
| 40 Unicode-heavy rows rebuilt | 1.229 ms | 1.295 ms |
| Eight panes × 40 unchanged rows | 0.0043 ms | 0.0193 ms |

All steady samples allocated zero new pipeline buffers and zero new cached-row
buffers. Every one-row mutation rebuilt exactly one row. The suite retains a
separate full 120×40 offscreen build/upload/submit safety rail; this run measured
1.050 ms median and 1.145 ms p95.

After adding pane-clip boundary invalidation and strengthening the cached versus
forced-full golden, the exact 2026-09-02 tree was captured again with the
software GL adapter in `target/benchmarks/issue-21-final-current.json`. Its
56-metric contract passed with no failed budgets. Static-row lookup measured
0.00089 ms p95, one-row rebuild/upload measured 0.0465 ms p95, and unchanged
eight-pane lookup measured 0.0086 ms p95. Scroll, search-heavy, and Unicode p95
were 1.348 ms, 0.849 ms, and 1.175 ms respectively. Both steady allocation
metrics remained exactly zero and every one-cell sample rebuilt one row.

These internal timings isolate row lookup/build/upload work. They do not include
OS input delivery, shell scheduling, surface presentation, compositor latency,
or vendor-specific GPU utilization. Use the displayed application and external
profilers from [performance.md](performance.md) before making end-to-end latency
or power claims.

## Running the evidence

Run the complete release suite so correctness assertions and budget evaluation
stay attached to the measurements:

```bash
WGPU_BACKEND=vulkan python3 scripts/run_benchmarks.py \
  --output target/benchmarks/latest.json
```

The relevant metrics are `renderer.steady_frame_buffer_allocations` and the
`renderer.row_cache.*` group. Release budgets require zero steady allocations,
at most one rebuilt row for a one-row change, a one-row p95 no greater than
1 ms, and an eight-pane unchanged lookup p95 no greater than 0.25 ms.
