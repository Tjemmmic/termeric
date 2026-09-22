# Glyph atlas lifetime and color fonts

Termeric renders monochrome masks and native color-font bitmaps from separate,
bounded GPU atlases. The mask texture is an eight-layer `R8Unorm` array and the
color texture is a four-layer `Rgba8Unorm` array; every layer is 1024×1024.
Their fixed payload is 8 MiB plus 16 MiB, excluding small driver metadata. Both
views and the text bind group remain valid for the renderer lifetime.

## Allocation and reclamation

Pages use one-pixel-padded shelf packing. Mask page zero holds a permanent 7×9
replacement box and is never recycled. Color pages have no replacement glyph,
so every page is reclaimable. At either limit, the least-recently-used page not
referenced by the current frame is cleared and reused.

Glyph lookups pin the correct mask or color page for the frame. Persistent
terminal-row batches retain disjoint encoded page identities; a cache hit pins
those pages before later rasterization can reclaim them. Either atlas evicting
advances a combined generation
included in row signatures. This makes prior-frame geometry rebuild without
allowing an upload to overwrite masks used by the frame being submitted.

If every reclaimable page is already pinned, allocation does not silently omit
text. The renderer uses the permanent replacement box for that glyph, remembers
the temporary pressure miss for the rest of the frame to avoid repeated
rasterization, and advances the atlas generation at the next frame boundary.
That invalidates persistent row batches containing temporary replacements only
after the prior submission releases its pins, so the real glyph is retried
instead of leaving the fallback cached indefinitely. Shaped runs containing a
temporary replacement remain frame-local rather than poisoning the persistent
shaping cache. Permanently oversized masks use the same visible fallback and
are cached normally.

Color glyphs preserve Swash's source RGBA bytes and sample those sRGB-encoded
values unchanged, matching the renderer's encoded-space composition, without
applying terminal foreground color. Only final opacity is
applied. Mask and subpixel-mask glyphs remain compact R8 data and continue to
use the theme/SGR foreground. Synthetic bold is not applied to authored color
artwork.

Font family, ordered fallback list, bounded Unicode-range overrides, normal or
bold `wght` value, font size/display scale, line height, and explicit cache resets clear raster and
shaped-run entries. For every extended grapheme, a matching range family is
tried first, followed by the first configured family whose character map covers
all substantive scalars; joiners, variation selectors, and tag scalars stay
with that grapheme. Range rules are compiled with installed-face metadata and
binary-searched rather than rescanning the system font database per cell.
Adjacent graphemes resolved to the same family remain one shaping span, so
fallback selection does not unnecessarily break ligatures. Cosmic-text's
platform fallback is retained only when no explicit family covers the
grapheme. Character entries include the
requested size, effective weight, and bold/italic style; shaped entries use
cosmic-text's resolved face, glyph, size, weight, subpixel position, and
synthetic-bold identity. Shaped-run identity contains both configured weights.
Incompatible rasters are therefore never reused across those changes.

The shaped-run key also includes the effective ordered OpenType feature values.
Changing `font.features` clears normal and deferred shaped runs and invalidates
persistent row geometry; resolved glyph atlas entries remain safely reusable by
their independent face/glyph/size/subpixel identity.

The Font Range Overrides manager can inspect this selection policy on demand.
Its bounded report uses the same compiled range lookup and complete-grapheme
family predicate, then performs one diagnostic normal-style cosmic-text shaping
probe only when every explicit family fails. Inspection neither populates nor
invalidates the raster/shaping atlases and never runs during normal row
construction.

## Diagnostics and verification

Shaped-run retention has two independent owned-byte budgets: 8 MiB for
persistent runs and 1 MiB for frame-local retry runs, each also limited to
4,096 entries. Charges include key/run structures and the actual capacities
of query text, family/fallback strings and vectors, OpenType feature vectors,
and stored shaped-glyph vectors. Hash-table bucket/control storage is separate
and bounded by the entry ceiling. Font-system caches, caller-owned output runs,
and transient shaping buffers are not covered by these budgets.

Capacity pressure preserves the existing whole-cache eviction policy. An
individual oversized run is rendered normally but is not cached, and does not
discard smaller useful entries. Atlas-page eviction subtracts the released run
charges; font/feature resets and new-frame retry cleanup clear the appropriate
ledger. Equal-key replacement removes the old key allocation too, so differing
reserved capacities cannot corrupt accounting. Trace diagnostics expose retained
bytes, entry counts, bypasses, hits/misses, and capacity evictions. This closes a
retention gap in the previous count-only persistent cache and unbounded retry
cache; it does not establish a total renderer RSS cap or a speed improvement.
The release suite now includes a paired real-shaping byte-pressure workload
against a test-only fully retained reference; see
[pressure measurement boundaries](performance.md#shaped-run-byte-pressure-cost).
Native eviction-heavy frame performance remains a separate measurement concern.

On a shaping miss, adjacent identical graphemes share their transient font
coverage resolution and family-name strings are owned only per coalesced span.
This avoids per-grapheme temporary name allocation without retaining another
lookup cache or changing fallback/range policy. Whole-grapheme routing is
compared against the prior per-grapheme reference in deterministic tests.

Deterministic tests independently recount every stored capacity, exercise both
limits, verify replacement/retain/clear accounting, and check that oversized
bypass preserves the caller's complete run. The GPU stress lane forces tiny
byte limits for real persistent and deferred runs: output remains complete,
deferred glyphs do not rerasterize within the frame, and next-frame retries
still recover after atlas pins are released.

The production rasterizer now uses Swash's uncached-image path while retaining
its scaler context. Previously, every uploaded image also stayed in Swash's
unbounded CPU image map, including rasters whose atlas pages were evicted or
whose font sizes had been replaced. Scalar and shaped atlas hits still bypass
rasterization; scalar aliases check the shaped atlas before rasterizing, and
same-frame deferred pressure hits also remain suppressed. A
glyph revisited after eviction may now need rerasterizing instead of retrieving
a duplicate CPU bitmap. This trades that cold-path work for eliminating
persistent duplicate pixel payloads, without changing atlas limits or pixels.

Trace logging separately reports rasterization calls, retained CPU image count,
and owned image-buffer capacity. Count and bytes remain zero after scalar,
shaped, color, pressure, and resize workloads. They do **not** count transient
images, font data, shaping caches, scaler scratch space, driver allocations, or
total process RSS. The deterministic Fira Mono fixture compares 282 glyph/size
keys at 24/48/96 pixels: old cached and new uncached paths produce identical
content, placement, and pixels; the old map retains 343,899 image-payload bytes
on the pinned dependency fixture, versus zero in the new rasterizer. This is
an owned-buffer observation, not an application RSS reduction or throughput
claim. The earlier ten-minute displayed baseline predates this change; broader
real-desktop impact and eviction-heavy performance still need measurement.

Trace logging reports mask and color page counts, maximum pages, combined byte
capacity, occupied pixels, hits, misses, page evictions, and allocation failures.
Allocation failures are observable even though the replacement glyph prevents
invisible output.

Normal tests use tiny deterministic allocators to cover page growth, LRU
selection, current-frame pinning, next-frame row-cache invalidation after a
pressure miss, the fixed byte cap, and generation changes.
The opt-in headless WGPU test forces 2,048 Unicode scalars with four style
combinations through a three-page 64×64 logical atlas, requires one quad per
scalar under exhaustion, verifies same-frame pressure misses do not rerasterize,
requires both scalar and shaped lookups to retry after frame pins are released,
changes font size, and submits through the real texture-array shader. It also
loads Noto Color Emoji, requires a real color entry, compiles/uploads the
dual-texture path, and verifies the 24 MiB cap. It additionally checks that CPU
image retention stays zero and atlas/deferred cache hits do not increase the
rasterization counter. Run it with:

```bash
TERMERIC_GPU_SMOKE=1 WGPU_BACKEND=vulkan cargo test -p termeric \
  linux_headless_gpu_render_smoke_when_requested -- --nocapture
```
