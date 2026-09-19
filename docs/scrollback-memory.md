# Scrollback memory model

Termeric keeps the visible grid as mutable, full-width `Cell` rows and archives
rows that scroll off-screen in a separate compact representation. Historical
rows trim default trailing cells, store content independently, and encode cell
styles as runs. Wide leaders and spacers live in the compact content encoding,
so they do not split an otherwise uniform paint-style run. Rendering
reconstructs only requested historical cells; resize reflow expands and
consumes one logical line at a time rather than cloning the entire rich history.
OSC 66 leaders likewise own their text once while a packed four-byte descriptor
is reconstructed for each covered cell. Complete one-row blocks survive normal
scrolling into history, and ED 22 archives the full viewport before erasure;
multirow blocks split by a live viewport/history boundary are cleared atomically
instead of leaving an invalid partial block.

## Resource policy

Every terminal and pane enforces both `[scrollback].lines` and
`[scrollback].max_memory_mib`. The default is 10,000 lines and 128 MiB. The
oldest rows are evicted as soon as either limit is exceeded, including after a
live configuration reduction or resize. The line setting accepts up to one
million rows, but the byte limit remains authoritative, so that setting is not
a promise that every workload can retain one million rich rows.

The budget is the terminal's total archival-history allowance, not an allowance
per screen buffer. The primary screen owns compact scrollback; alternate-screen
applications are viewport-only and retain zero archived rows. Entering an
alternate screen clears any legacy alternate history, and scrolling there
avoids allocating a compact row only to discard it. Primary history remains
available on exit and runtime limit reductions still evict it while the
alternate screen is active.

The byte counter includes compact-row objects, retained content capacity,
style-run capacity, and heap-backed grapheme bytes. Hyperlink URLs are
deduplicated separately, reject individual URL/ID fields above 8 KiB, receive a
fractional memory budget, and are reference-pruned as rows are evicted.
Selections whose oldest backing row is evicted are cleared rather than left
pointing at different content. Search results are recomputed from row views and
session export uses the same compact cells, so neither owns stale row pointers.
OSC 133 command regions likewise lose live-content authority when their oldest
boundary is evicted, while retaining a bounded monotonic identity so consumers
cannot replay or confuse completions. When experimental Harvest is enabled, its
separate 100-entry default history may retain up to 64 KiB of output per entry
(6.25 MiB at the default count) and independently caps aggregate owned command
and output snapshot capacity at 8 MiB. Count- or byte-limit eviction removes the
oldest entries and is surfaced in Root Cellar rather than occurring silently;
this snapshot budget is not charged to terminal scrollback.

The same retirement path is used by CSI 3 J and the application Clear
Scrollback action. ED2 and RIS retire semantic authority for erased live rows
without discarding fully retained historical marks. Completed identities stay
inside the existing bounded tracker so consumers observe each completion at
most once, but no erased coordinate can authorize a later grid read.

Graphics placements wholly outside retained history are retired immediately on
clear or budget reduction, along with relative descendants. Last-placement
removal frees anonymous Sixel/iTerm2 pixels; reusable Kitty data remains in its
independently bounded image cache. Images intersecting retained rows and frozen
synchronized-output presentations are preserved. See the
[graphics lifetime evidence](kitty-graphics.md#evidence); this does not fold
image storage into the compact text-history budget.

## Viewing retained history during output

A scrolled-back viewport follows its retained top row when new history is
appended, even when the line cap keeps history length constant or a larger row
causes several byte-budget evictions. Full-screen scrolling, full-width regions
starting at row zero (such as output above a fixed footer), and whole-viewport
archival share this rule. If the viewed row is actually evicted, the viewport
clamps to the oldest remaining row; if no history remains, the offset becomes
zero. Offset zero continues following live output. Interior or invalid scroll
regions do not change the history view.

This uses one constant-time offset adjustment, not a retained-row search or
extra history copy. Tests use unique row contents as an independent identity
oracle, cover line and multi-row byte eviction, and replay SU, DL, and LF above
an unchanged footer. Selection and copy-cursor archival have their own
[identity and mixed-motion checks](copy-mode.md). OSC 133 marks, command
boundaries, and prompt-navigation anchors also follow top-origin, full-width
archival (including DL). Historical and moving rows shift together; fixed-footer
marks stay put. A command spanning both parts loses live-text authority rather
than resolving a range containing unrelated cells. Active command extents
include the current cursor; valid commands can continue and complete with the
same stable identity. Line/byte eviction prunes marks and retires command text
without discarding completion identity or exit status. This bounded tracker
update does not scan or copy history.

The `term::scroll_prompts` tests cover retained-text identity across SU/DL/LF,
mixed-motion retirement, active completion, fixed footers, line/byte eviction,
prompt navigation, and isolation from non-archiving operations. These are core
regressions, not native shell or graphics acceptance. The separate
[graphics archival checks](kitty-graphics.md#evidence) preserve source geometry
and image/text row identity, leave fixed-footer placements stationary, and
retire fully evicted placements with their relative descendants.

## Diagnostics

`Terminal::memory_usage()` reports:

- full-width visible-grid bytes;
- compact scrollback bytes and retained line count;
- grapheme heap bytes;
- style-run bytes;
- hyperlink reference and registry bytes;
- configured line and byte limits.

The release benchmark records the same categories for 100k styled rows, total
requested allocation bytes/calls during construction, and bytes per line for
plain, styled, Unicode, OSC 8 hyperlink, and sparse history. DHAT and heaptrack
remain the allocation-site/peak-memory sources of truth; these counters are
deterministic ownership and traffic evidence suitable for enforcing limits.
An application-level invariant test also changes the limits after a multi-pane
tab exists, creates another split and tab afterward, drives eviction in every
terminal owner, and requires all of them to retain the same configured line and
byte ceilings.

## Measured result and tradeoff

On the original issue #18 Linux/llvmpipe host, the 100k styled workload fell
from a 645 MiB p95 RSS delta to approximately 109 MiB in the issue #19 five-run
validation. The 2026-08-27 working-tree measurement retains all 99k+ archived
rows at 110.3 MiB estimated ownership and roughly 112 MiB p95 RSS. Structural
wide-cell markers no longer multiply paint-style runs: style storage returned
from 15.3 MiB in the intermediate candidate to 3.8 MiB. The 2026-08-31 release
fixture measures 192.1 ms median ingestion. Recycling the row buffer that leaves
the viewport, storing ordinary REP scalars inline, and using VTE's fixed
parameter ceiling for stack-based CSI/SGR views reduced construction traffic
from 938.0 MB and 5.89 million allocation calls to 144.4 MB and 900,075 calls
without changing the 115.7 MB owned-history result.

Fifty alternating resizes of 10k rows originally took 739.4 ms and requested
6.90 GB through 23.94 million allocation calls. Hard-broken compact rows that
already fit now retain their buffers and only retarget width metadata; the
controlled after-run measured 86.5 ms, 132.1 MB, and 67,493 calls. Wrapped lines
and boundary-crossing rows still take the full repairing reflow path. Both
construction and reflow retain broad deterministic regression rails while
additional release-duration and cross-machine evidence accumulates.
