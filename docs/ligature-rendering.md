# Ligature rendering

Termeric shapes contiguous terminal row runs with `cosmic-text` and renders the
resulting positioned glyphs through the normal atlas and WGPU text pipeline.
The `font.ligatures` setting explicitly enables or disables `liga`, `clig`,
`calt`, and `dlig`; both states retain advanced shaping so combining marks and
script shaping do not depend on an optional programming-font preference.
Kerning is disabled because the terminal remains a fixed-column grid.

An optional `font.features` mapping supplies at most 32 explicit OpenType
values after that preset:

```toml
[font.features]
ss01 = 1
cv01 = 2
calt = 0
```

Tags are exactly four lowercase ASCII letters or digits, values span the
shaping API's `u32` range, and `kern` is rejected. Explicit values win when a
tag also belongs to the Ligatures preset. Unsupported tags are safely forwarded
and may have no visible effect in a particular font.

## Run boundaries and grid semantics

A run contains adjacent, visible, one-column character cells with identical
foreground color, bold/italic face request, pane clip, row origin, and cell
advance. Termeric flushes a run at whitespace, an empty or concealed cell, a
selection-color boundary, an attribute change, a geometry gap, a wide-cell
leader/spacer, or a multi-scalar grapheme. Graphemes keep their dedicated
cluster-shaping path, while wide cells remain anchored to their existing grid
geometry.

Ligatures change only glyph selection and placement. Cursor coordinates,
selection/search ranges, URL hit testing, copied text, pane clipping, and cell
background/decorations continue to use terminal cells. A block cursor redraws
the constituent cell on top of the already-shaped row and does not reshape or
move surrounding cells.

## Caching and invalidation

The shaping cache key includes run content, font size and line height, both
configured `wght` values, bold/italic request, the effective ordered feature
values, primary family, ordered fallback families, and fallback resolution. Unicode-range or OpenType
feature policy changes synchronously clear this cache and persistent row
geometry before another frame can reuse a run. Rasterized shaped glyphs are separately
reused by their resolved face/glyph/size/subpixel identity. The run cache is
bounded to 4,096 entries; reaching the bound clears the run map and records an
eviction count rather than allowing unbounded growth.

Font, weight, size, line-height, scale, atlas, ligature, and explicit feature
changes invalidate the relevant glyph/run state. Assigning an already-active
feature map is a no-op.

## Verification

The normal unit lane checks feature toggling against the deterministic Fira
Mono fixture shipped by the locked `cosmic-text` source. It proves explicit
zero/one values override both Ligatures preset states, participate in cache
identity, and reach advanced shaping without host fonts. It also verifies
row-run boundaries. The opt-in headless GPU lane renders enabled and disabled snapshots,
requires their pixels to differ, requires a repeated enabled snapshot to be
identical, and checks warm shaping-cache reuse. A second comparison covers
adjacent regular, bold/color, and italic/color runs under a shared pane clip so
attribute boundaries and clipping travel through the same real text pipeline:

```bash
TERMERIC_GPU_SMOKE=1 cargo test -p termeric \
  linux_headless_gpu_render_smoke_when_requested -- --nocapture
```

The release benchmark emits `renderer.ligature.code_run_build` and
`renderer.ligature.shaping_cache_hit_rate` for 1,000 warmed code-heavy runs.

For release acceptance, install a programming font with the relevant OpenType
features (for example Fira Code or JetBrains Mono), place `->`, `=>`, `!=`, and
`===` beside mixed bold/italic/color cells and pane edges, and toggle ligatures
in settings. Then configure a feature implemented by that font and verify both
the enabled and disabled value. The operators must visibly change without a restart; cell
alignment, selection, cursor placement, search highlights, copying, and pane
clipping must not.
