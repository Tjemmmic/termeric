# Sixel and iTerm2 inline images

Termeric implements bounded static-image subsets of DEC Sixel and iTerm2's
original OSC 1337 `File` protocol. Both adapters normalize decoded pixels and
grid placement into the same per-terminal cache and renderer path as Kitty
graphics. The protocol presence is not a claim that every extension is
supported.

The primary references are DEC's
[VT330/VT340 Programmer Reference Manual](https://vt100.net/docs/vt3xx-gp/),
[xterm's control-sequence reference](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html),
and the [iTerm2 inline-images specification](https://iterm2.com/3.5/documentation-images.html).

## Supported Sixel profile

The 7-bit `DCS ... q ... ST` and 8-bit C1 DCS/ST paths support:

- the three Sixel introducer parameters, including transparent background
  selection with `P2=1`;
- Sixel data bytes, repeat introducers, graphics carriage return, and graphics
  newline;
- every defined `P1` macro pixel aspect plus raster-attribute `Pan:Pad`
  overrides and an optional declared extent;
- palette selection and definition in DEC HLS or RGB-percentage coordinates;
- arbitrary PTY fragmentation, placement at the text cursor, and movement to
  the starting column on the next complete line below the resulting cell
  footprint, matching the VT340/xterm scrolling profile.

Pixel aspect changes only the bounded display geometry: decoded RGBA remains in
the compact logical Sixel raster, while placement and cursor movement use the
nearest whole-pixel display extent. Raster attributes override `P1`, as DEC
specifies, and both tall and wide ratios are supported.

Termeric advertises Sixel with Primary DA parameter `4`. It deliberately does
not claim ReGIS, stored Sixel macro definition/recall, printer behavior, or
DECSDM display mode. The xterm private shared-color-register and alternate
cursor-placement modes (`1070` and `8452`) are also unsupported; each image has
private color registers and uses the fixed cursor behavior above.
Unsupported or malformed controls reject the complete image without leaving a
partial placement.

## Supported iTerm2 profile

The original `OSC 1337 ; File=... : base64 BEL/ST` path supports inline static
PNG images and these metadata fields:

- `inline=1`;
- an optional exact decoded `size`;
- a validated base64 `name` of at most 4 KiB;
- `width` and `height` as `auto`, cells, pixels (`px`), or percentages;
- `preserveAspectRatio=0/1`.

Unknown `File` metadata is ignored for client compatibility, but cannot enable
an unsupported operation. File downloads (`inline=0`), multipart transfers,
non-PNG formats, animated PNG, GIF/video animation, and newer proprietary image
commands are rejected. Terminal output never selects or opens a filesystem
path.

## Placement, scrolling, and cleanup

Each terminal and pane owns an isolated graphics manager. Protocol adapters
produce a normalized RGBA image plus source pixels, exact display pixels, grid
footprint, cursor anchor, screen-buffer identity, and z-order. The renderer
therefore uses one clipping, texture, cache-revision, and pane-composition path
for Kitty, Sixel, and iTerm2 images.

Exact pixel dimensions remain stable while grid footprints are recomputed when
cell pixel geometry changes, including in a frozen synchronized presentation.
Host geometry updates refresh live and frozen footprints before resizing the
grid, so retention uses the new cell size. Placements follow primary scrollback;
scroll regions clip their source and destination extents together, and primary and
alternate screens remain isolated. ED2, RIS, alternate-screen cleanup, or
scrolling the final placement out of retained history immediately releases
unaddressable Sixel/iTerm2 decoded pixels. Stale GPU textures are collected when
their normalized placements disappear.

When DECLRMM selects a partial-width scrolling rectangle, a placement moves only
if its complete cell footprint is inside both the horizontal and vertical
margins. Placements outside or crossing a horizontal boundary remain fixed;
ordinary vertical clipping still applies to fully enclosed placements that
scroll through the top or bottom edge.

VT400 rectangular text operations share this normalized placement model.
DECCRA copies source image coverage and clears absent coverage at the
destination; DECFRA and DECERA crop or remove only the placement cells they
overwrite. DECSERA remains character-selective and does not reinterpret image
pixels as DECSCA-protected text. Cropping adjusts source and display extents
together and retains the independent 1,024-placement bound.

## Resource and security boundaries

- DCS and OSC capture is capped at 16 MiB and releases its peak buffer after a
  complete command.
- iTerm2 base64 input is capped at 16 MiB. Sixel input is capped at 16 MiB,
  repeats at 1,000,000 columns, palette registers at 1,024, and total raster
  work at 64 million pixel operations.
- Sixel source and aspect-adjusted display dimensions are independently limited
  to 16,384 pixels per axis, while all checked RGBA output is capped at 64 MiB.
  Aspect conversion does not expand source storage. iTerm2 display dimensions
  are limited to 65,535 pixels per axis.
- PNG decoding has an allocation ceiling, checks dimensions before RGBA
  normalization, and rejects animation.
- Every terminal shares one 256-image/64-MiB decoded cache and a separate
  1,024-placement cap across all graphics protocols. Referenced images are
  refreshed before admission so unplaced reusable entries evict first; FIFO
  placement retirement releases anonymous pixels after their final reference.
  The renderer keys textures by terminal identity plus image ID.

## Evidence and client compatibility

Unit tests use independent encoder-shaped Sixel and imgcat fixtures, exact
PNG/RGBA assertions, every DEC macro aspect family, tall/wide/fractional raster
aspects, fragmented aspect input, malformed/oversized inputs, cursor movement,
resize, scroll clipping, alternate screens, cleanup, cross-protocol cache
pressure, and two-terminal isolation. The dedicated `graphics_protocols` fuzz
target wraps arbitrary fragmented Sixel DCS and iTerm2 OSC payloads and
continuously asserts the cache, placement, and ownership bounds.

The native Linux GUI harness additionally requires `img2sixel` from libsixel,
independent `timg -I -ps --frames=1` Sixel output, and `timg -pi`, runs all three
against a real PNG through the PTY, requires each
commands to reach the protocol adapters, and presents their normalized geometry
through the WGPU image path. The same `timg` executable is independently invoked
with `-pk` for the Kitty adapter; its generated image ID is not reused as iTerm2
evidence. Both Sixel producers must emit complete frames, and the controlled
fixture must produce exactly two Sixel adapter acceptance log entries. One
working client cannot waive a broken producer. The hermetic fixture test proves that all distinct client commands
execute and emit their protocol streams, but a current accepted native artifact
and subjective scaling/color review on vendor GPUs remain pending.
