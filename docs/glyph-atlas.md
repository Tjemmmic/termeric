# Glyph atlas lifetime and color fonts

Termeric renders monochrome masks and native color-font bitmaps from separate,
bounded GPU atlases. The mask texture is an eight-layer `R8Unorm` array and the
color texture is a four-layer `Rgba8UnormSrgb` array; every layer is 1024×1024.
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

Color glyphs preserve Swash's source RGBA bytes and sample the sRGB color
texture without applying terminal foreground color. Only final opacity is
applied. Mask and subpixel-mask glyphs remain compact R8 data and continue to
use the theme/SGR foreground. Synthetic bold is not applied to authored color
artwork.

Font family, ordered fallback list, bounded Unicode-range overrides, font
size/display scale, line height, and explicit cache resets clear raster and
shaped-run entries. For every extended grapheme, a matching range family is
tried first, followed by the first configured family whose character map covers
all substantive scalars; joiners, variation selectors, and tag scalars stay
with that grapheme. Range rules are compiled with installed-face metadata and
binary-searched rather than rescanning the system font database per cell.
Adjacent graphemes resolved to the same family remain one shaping span, so
fallback selection does not unnecessarily break ligatures. Cosmic-text's
platform fallback is retained only when no explicit family covers the
grapheme. Character entries include the
requested size and bold/italic style; shaped entries use cosmic-text's resolved
face, glyph, size, subpixel position, and synthetic-bold identity. Incompatible
rasters are therefore never reused across those changes.

The Font Range Overrides manager can inspect this selection policy on demand.
Its bounded report uses the same compiled range lookup and complete-grapheme
family predicate, then performs one diagnostic normal-style cosmic-text shaping
probe only when every explicit family fails. Inspection neither populates nor
invalidates the raster/shaping atlases and never runs during normal row
construction.

## Diagnostics and verification

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
dual-texture path, and verifies the 24 MiB cap. Run it with:

```bash
TERMERIC_GPU_SMOKE=1 WGPU_BACKEND=vulkan cargo test -p termeric \
  linux_headless_gpu_render_smoke_when_requested -- --nocapture
```
