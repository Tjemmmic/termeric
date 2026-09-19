# PTY pixel geometry

Termeric reports both cell counts and physical pixel dimensions in the PTY
window size at process spawn and on every effective resize. On Unix these are
the `ws_col`, `ws_row`, `ws_xpixel`, and `ws_ypixel` values returned by
`TIOCGWINSZ`; `portable-pty` carries the same four fields to other platforms.

## Geometry policy

Winit window sizes, scaled padding, renderer cells, and pane rectangles are all
physical-pixel values. The shared `TerminalGeometry` calculation:

1. subtracts the top toolbar, any vertical-left tab rail, and both outer
   padding edges from the window;
2. floors the available extent by the physical cell width and height;
3. limits rows and columns to the PTY's 16-bit ABI; and
4. reports each pixel extent as `round(cell_count * physical_cell_size)`.

The reported pixels therefore describe the drawable full-cell grid, not the
raw window or a partial-cell remainder. A 59-column grid with 8.4-pixel cells,
for example, reports 496 pixels. Rows, columns, and pixels always derive from
the same calculation.

The terminal core retains that exact represented width and height separately
from the integer cell size used by pixel-addressed image protocols. The cell
size is rounded up independently in each dimension so that an uneven extent
still covers every represented pixel. For example, a 496-by-293 grid with 59
columns and 17 rows has an exact text area of 496 by 293 pixels and a reported
cell size of 9 by 18 pixels.

Pane layout applies the same policy independently after subtracting each
four-pixel divider. A new split is spawned with its final post-divider size;
nested and asymmetrically dragged panes then update from their exact current
rectangle.

## Update lifecycle

Window resize, physical scale-factor changes, font size/family/weight/line-height,
padding, pane split/close, and divider movement all synchronize the complete
four-field geometry. Synchronization is not conditional on rows or columns
changing, so a pixel-only scale or font change still reaches the PTY.

Single-tab, split-pane, and daemon-owned terminals apply those dimensions through
the core's atomic `resize_with_pixel_geometry` path. Live and frozen image cell
footprints are updated for the new metrics before grid resize can retire lost
rows. This preserves still-visible pixel images during simultaneous font/scale
and viewport changes. Pixel-only changes update derived image geometry without
resetting margins, cursor, or the captured content of synchronized output.

Authenticated live-domain protocol version 9 forwards the same four fields
from each relay PTY to its daemon-owned application PTY. A daemon pane starts
with a nonzero 8×16-pixel-per-cell bootstrap matching the terminal core's
initial metrics; the first renderer-backed relay resize replaces it with exact
pane geometry. If a non-Termeric parent reports zero pixel fields, the daemon
scales its last known physical extent with the new row/column count instead of
regressing the kernel winsize to zero.

Tabs and panes compare all four fields before resizing, and the core PTY layer
also compares against the kernel's current size. Repeated equivalent layout
events therefore do not create duplicate `SIGWINCH` storms.

## Read-only terminal geometry reports

Termeric implements the terminal-local, read-only geometry subset of xterm's
window operations (`XTWINOPS`):

- `CSI 14 t` returns `CSI 4 ; height ; width t` using the exact represented
  text-area pixels;
- `CSI 16 t` returns `CSI 6 ; height ; width t` using the rounded-up cell
  pixels used by graphics placement; and
- `CSI 18 t` returns `CSI 8 ; rows ; columns t` using the current grid.

Replies use the terminal's selected S7C1T or S8C1T framing. Requests with
additional parameters and unsupported `CSI ... t` operations are ignored. In
particular, Termeric does not expose outer-window or screen geometry, window
position or state, or any window mutation. Application-owned title reads and
the fixed title stack are the separate bounded
[OSC integration contract](osc-integration.md). The legacy
DECSLPP, DECSCPP, and DECSNLS settings have read-only DECRQSS replies, but their
mutation controls remain ignored. Those operations need explicit
application/window policy and are not implied by the terminal-local PTY pixel
contract.

## Evidence and platform boundary

The core PTY test creates a real Unix PTY with nonzero pixels, reads the kernel
size at startup, resizes it, reads it again, and exercises an exact duplicate
no-op. Application tests cover a pixel-only single-tab resize and compare the
kernel value. A three-pane test uses fractional cells, an asymmetric divider,
and a nested split, then requires renderer-derived cell dimensions, terminal
grids, and every kernel PTY size to agree. A daemon-pane test verifies nonzero
bootstrap pixels, an exact pixel-only renderer update, duplicate suppression,
and proportional preservation for a zero-pixel external relay. Deterministic
parser and conformance tests verify the three read-only geometry replies,
both reply framings, exact versus rounded geometry, resize updates, and
rejection of unsupported forms.

The three real-PTY owner tests also shrink an 8×5 grid with 2×3-pixel cells to
8×2 with 1×1-pixel cells and no history. A 4×6-pixel Sixel must remain partially
visible in both live and frozen state, with its original pixel storage, while
the kernel receives the new four-field geometry. This is automated ownership
and geometry evidence, not native font/DPI visual acceptance.

Linux provides current runtime evidence. macOS and Windows retain the platform
verification status documented in [platform support](platform-support.md).
