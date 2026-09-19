# Kitty graphics support

Termeric implements a bounded subset of the
[Kitty terminal graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/).
This document is the capability contract for that subset; unlisted protocol
extensions must not be inferred from the presence of an APC parser.

## Supported profile

The end-to-end PTY, terminal, storage, and GPU path supports:

- UTF-8-safe `ESC _ G ... ESC \` APC framing across arbitrary PTY read
  boundaries;
- direct (`t=d`) RGB (`f=24`), RGBA (`f=32`), and static PNG (`f=100`) data;
- RFC 1950 zlib compression (`o=z`) across direct and supported local media,
  with `S` required for compressed PNG;
- protocol chunking with continuation controls carried by the first chunk;
- on Unix, temporary-file (`t=t`) data from a tightly constrained local
  temporary directory, including bounded `S` read ceilings and `O` offsets;
- on Unix, ordinary-file (`t=f`) data in ordinary local tabs when the host
  explicitly opts in through `[security] allow_kitty_file_transfers = true`,
  with bounded `S` read ceilings and `O` offsets, final-target validation, and
  no file deletion;
- on POSIX systems, shared-memory (`t=s`) data from private same-user objects,
  also with bounded `S` read ceilings and `O` offsets;
- transmit, transmit-and-display, display, query, and delete actions;
- Kitty animation frame loading (`a=f`) through direct or supported local
  media, including direct chunk continuations, full or partial RGB/RGBA/PNG
  updates, transparent/background-color canvases, prior-frame bases, root and
  extra-frame edits, source-over blending, replacement mode, default/custom/
  gapless timing, and replies containing the resolved frame number;
- animation control (`a=a`) for stopped, loading, and looping playback,
  current-frame selection, per-frame gap changes, infinite or finite loop
  counts, and monotonic exact-deadline advancement. Only animations placed on
  the active tab's presented screen are scheduled; pane animations share the
  same deadline wake without forcing a permanent 60 Hz redraw loop;
- bounded frame composition (`a=c`) between full RGBA frames, with source-over
  or replacement semantics, strict source/destination bounds, same-frame
  overlap rejection, stable content revisions, and immediate renderer cache
  invalidation when the presented frame changes;
- frame deletion (`d=f/F`), including root promotion, current-frame repair,
  lowercase last-frame preservation, and uppercase whole-image cleanup;
- image and placement IDs; non-unique image numbers (`I`) that allocate a
  collision-free ID, reply with both identifiers, and resolve later commands
  to the newest retained image with that number; multiple anonymous images;
  replacement of a reused image ID; and replacement of a reused `(image ID,
  placement ID)` pair;
- bounded usage-hint bitmasks (`N`), including the transient hint, accepted as
  optional advisory metadata. The current in-memory cache deliberately ignores
  the eviction hint while still enforcing its hard limits;
- Unicode-placeholder virtual placements (`U=1`), including Kitty's exact
  297-entry row/column diacritic table, 8-bit indexed or 24-bit true-color image
  IDs, optional indexed or true-color underline placement IDs, high image-ID
  bytes, and the specified left-to-right coordinate inheritance rules. Visible
  tiles are derived from the presented text grid, preserve image aspect ratio,
  crop at sub-cell texture coordinates, and therefore follow ordinary text
  through edits, reflow, scrollback, synchronized output, and screen changes;
- relative placements (`P/Q/H/V`) against named or first-created ordinary and
  virtual parents. Signed cell offsets and chains up to 32 ancestors resolve at
  presentation time, so children follow parent scrolling and named-placement
  replacement without moving the cursor. Unicode parents use the independent
  minimum live placeholder x/y coordinates. Missing parents, cycles, and
  excessive chains report `ENOPARENT`, `ECYCLE`, and `ETOODEEP`; deleting or
  evicting a parent cascades through its descendants, and orphaned descendant
  image data is released;
- source rectangles (`x/y/w/h`), cell pixel offsets (`X/Y`), destination cell
  rectangles (`c/r`), aspect-preserving sizing when one destination dimension
  is omitted, natural pixel sizing when both are omitted, and z-index ordering
  above or below terminal text;
- `C=1` no-movement placement, or cursor movement using the explicit or
  pixel-derived cell footprint. Natural and single-axis placements retain
  exact pixel extents while cell geometry remains authoritative for scrolling;
- quiet replies (`q=1/2`) and typed `EINVAL`, `EPERM`, `ENOENT`, `ENOPARENT`,
  `ECYCLE`, `ETOODEEP`, `ENOTSUP`, `ENOSPC`, `E2BIG`, and `EIO` failures;
- deletion by visible screen, image/placement ID, cursor or specified cell,
  column, row, z-index, image number, and image-ID range. Uppercase selectors
  release image data once no placement still references it. As required for
  Unicode placeholders, coordinate/all-visible selectors leave virtual
  prototypes intact while ID, number, and ID-range selectors remove them;
- placement movement during full-page and fully enclosed DECLRMM rectangular
  terminal scrolling, including primary full-width top-origin archival above
  a fixed footer and DL from row zero, primary/alternate-screen
  isolation, alternate-screen cleanup, ED2 cleanup, RIS cleanup, storage LRU,
  and renderer cache invalidation when an image ID is reused.
- protocol-v8-and-later live-domain attachment/reconnect snapshots for complete decoded
  graphics state: root and coalesced animation pixels, storage order and
  revisions, image-number aliases, anonymous protocol ownership, both-screen
  ordinary placements, Unicode virtual prototypes, relative-parent UIDs, and
  animation playback/timing state. Pixel loads are quiet and fragmented; an
  exact metadata commit occurs before later live PTY output is released.

The direct transfer profile works with clients that stream data through the PTY.
On Unix, `kitten icat --transfer-mode=file --stdin=yes < image.png` can exercise
the temporary-file route because stdin data is materialized into a client-owned
temporary file. Passing an existing filename in file mode instead selects
ordinary-file (`t=f`) transfer and works only after the user enables
`allow_kitty_file_transfers`; stream mode remains the secure, portable default.
`kitten icat --transfer-mode=memory image.png` exercises the POSIX shared-memory
route. Current `kitten icat` stream commands commonly omit both `c` and `r`;
Termeric displays those at their natural pixel size and derives the scrolling
and cursor footprint from the live PTY cell-pixel geometry. If exactly one
destination axis is supplied, the other is derived from the source aspect
ratio.

## Deliberately unsupported

Termeric returns an error for chunked local media, APNG payloads, and other
unknown controls. APNG remains distinct from Kitty's protocol-level frame
animation and is not decoded as an animation source.
Ordinary-file and temporary-file transfer are not
enabled on non-Unix targets; POSIX shared memory is likewise Unix-only. Sixel
and iTerm2 inline images use separately documented adapters into the same
normalized renderer; see [their capability contract](inline-images.md).

## Resource and security boundaries

- The control section of one APC command is capped at 4 KiB. Direct encoded
  data is capped at 128 KiB per APC for interoperability with Kitty's current
  reference client, which emits 128-KiB chunks despite the published
  protocol's 4096-byte recommendation. The complete APC cap is the sum of
  those independent bounds plus the separator.
- One in-flight chunked upload is retained per terminal and is capped before
  vector growth at 64 MiB. A static image upload and an animation-frame upload
  cannot be interleaved.
- Ordinary-file transfer is denied by default with `EPERM`. Enabling
  `allow_kitty_file_transfers` authorizes every application in that terminal,
  including shells and remote programs, to ask Termeric to read an accessible
  persistent path for image decoding. Enable it only for trusted terminal
  applications; prefer stream, temporary-file, or shared-memory transfer when
  possible. Domain-owned PTYs and GUI relay replicas always deny this medium.
  Live-domain snapshots relay decoded pixels, never source paths, so a remote
  pathname cannot become a read from the GUI host.
- An enabled `t=f` payload must name an absolute Unix path. Symlinks are
  resolved as the Kitty protocol expects, but the final target must be a
  regular file outside `/dev`, `/proc`, and `/sys`. Termeric compares the open
  descriptor's device/inode identity with the resolved path before and after a
  bounded read, and never deletes or modifies the path. Reads are capped at 64
  MiB. This policy deliberately cannot prove that an opt-in path is
  non-sensitive; the opt-in is the trust decision.
- A `t=t` payload may name only an absolute path whose full bytes contain
  `tty-graphics-protocol` and whose canonical parent is exactly the process
  temporary directory, `/tmp`, or `/dev/shm`. Nested paths, symlinks, and every
  non-regular filesystem object are rejected. This is intentionally stricter
  than the protocol's minimum policy.
- For `t=t`, the terminal compares device/inode identity before reading and
  again before unlinking, reads at most 64 MiB from the validated `O`/`S` range,
  and removes the unchanged temporary file even when later image decoding
  fails. The bounded local-media reads are synchronous so a query reply remains
  ordered behind validation of the advertised medium.
- A `t=s` payload must be one POSIX name component, optionally prefixed by the
  conventional leading slash, with no later slash or NUL and at most 255 bytes
  after normalization. Kitty's current bare-name form is normalized before
  `shm_open`. The opened object must belong to the terminal's
  effective user with no group/other permission bits; non-Apple Unix targets
  additionally require a regular-file descriptor type. A second descriptor
  must identify the same device/inode before the name is unlinked; the original
  descriptor remains valid for the bounded read. Unsafe objects are rejected
  without unlinking.
- Apple targets use a page-aligned, size-checked `mmap` copy because their
  POSIX shared-memory descriptors are not ordinary readable files. Other Unix
  targets use bounded reads. Automated runtime evidence currently comes from
  Linux; native Apple acceptance remains a platform release gate.
- Unix ordinary/temporary-file descriptor opens use `O_NONBLOCK` and
  `O_NOFOLLOW` before type/identity validation. Linux shared-memory opens,
  including the cleanup identity recheck, use the same flags. A FIFO cannot
  wait for a writer before being rejected; rejected or substituted FIFOs are
  not read or unlinked. Temporary-file final-component symlink substitutions
  are refused as well. This is not a filesystem deadline: metadata operations
  and regular-file reads remain synchronous, including on slow or unusual
  filesystems. See [the FIFO-open fix](https://github.com/Tjemmmic/termeric/issues/103).
- For local media, `S` limits bytes read and a shorter object naturally ends at
  EOF. This applies to ordinary files as well as temporary files and shared
  memory. For compressed PNG it additionally declares the exact decompressed
  PNG size; compressed raw output size instead comes from checked `s`/`v` geometry.
  Both input reads and zlib expansion remain independently capped at 64 MiB.
- Raw dimension arithmetic is checked, dimensions are limited to 16,384 pixels,
  and decoded image data is capped at 64 MiB.
- Zlib output is read through a hard limit; declared decompressed sizes above
  the cap and decompression expansion beyond it fail before storage.
- PNG decoding uses allocation limits, checked output sizes, normalized RGBA
  output, and rejects APNG.
- Stored images are capped at 256 entries and 64 MiB per terminal. Before a
  new image is admitted, placed images are refreshed so an older unplaced
  transmit-only entry is evicted before pixels still referenced by terminal
  content. If all entries are placed, least-recently-used eviction still
  enforces the hard limits.
- Each animation is capped at 1,024 total frames and all animations in one
  terminal are capped at 4,096 additional frames. Coalesced extra-frame RGBA
  storage has an independent 320 MiB ceiling (five times the root-image
  budget); every new frame is admitted atomically or fails with `ENOSPC`.
  Full coalescing intentionally trades more bounded memory for constant-depth
  presentation and composition with no delta-chain traversal.
- Image-number aliases are indexed only for retained images, so the index is
  bounded by the same 256-image ceiling. Reusing a number creates a distinct
  image and selects the newest one; deleting or evicting it exposes the next
  newest retained match. Reusing its explicit ID removes the stale alias.
- Placements are independently capped at 1,024 per terminal. Repeated display
  of one reusable image retires the oldest placement first; retiring the last
  placement of an anonymous Sixel/iTerm2 image also releases its pixels.
  Relative-parent eviction cascades to every descendant instead of leaving a
  dangling reference. Relative chains are capped at 32 ancestors, above the
  protocol-required minimum of eight, so resolution cost is deterministic.
- Unicode virtual-placement prototypes have a separate 1,024-entry terminal
  cap. Adjacent compatible placeholder cells are coalesced into horizontal
  presentation spans before the independent 1,024-instance frame cap is
  applied. A current-client full-width image therefore costs approximately one
  renderer instance per visible row instead of one per cell. Prototypes keep
  their image data referenced but allocate no per-cell pixel copy; renderer
  presentations clone only the stored image's reference-counted handle.
- GPU textures are removed when their terminal placements disappear and are
  re-uploaded on content revision.
- Relay metadata is capped at 4 MiB independently of the existing decoded root
  and animation limits. Relay pixel loads continue using 3-KiB chunks below
  both the published recommendation and the 128-KiB compatibility ceiling;
  each domain snapshot message stays below the 64 KiB stream boundary,
  and daemon capture clones reference-counted pixel handles rather than the
  full RGBA allocation. The one-shot internal APC is disabled for ordinary
  terminals and consumes its permission on either commit or rejection.

Padded and raw-unpadded standard base64 are accepted. Malformed control data,
base64, dimensions, PNG, compression, and interrupted chunks must leave no
partial image behind. Error text is length-limited and stripped of control
characters before it is written to the PTY. An explicit `q=2` suppresses even
early parser and framing-limit errors, so replies cannot leak into the invoking
shell after a quiet client exits.

## Evidence

Top-origin archival moves ordinary placements wholly above the fixed footer,
including negative history coordinates, without cropping their source or exact
pixel geometry at row zero. Footer placements and placements crossing the fixed
boundary remain stationary, consistent with the existing fully-enclosed margin
policy (see Kitty's
[margin-scroll implementation](https://github.com/kovidgoyal/kitty/blob/master/kitty/graphics.c)).
This is Termeric's archival policy, not a claim that all terminals retain history
for the same margin operations. Full-page scrolling and full-height DL retain
their unbounded image movement, including images taller than the viewport.
On archival appends, placements wholly older than retained history are removed;
relative descendants follow their parent or are removed with it. Last-placement
removal releases anonymous Sixel/iTerm2 storage, while a synchronized snapshot
can retain its independent presentation until release.

CSI 3 J, the application Clear Scrollback API, and line/byte budget reductions
also immediately retire placements wholly older than the resulting retained
history, without waiting for subsequent output. Relative descendants are
removed with their parent even if an offset placed the child on-screen. Images
still intersecting retained rows keep their geometry; virtual prototypes and
reusable Kitty image data retain their normal independent cache lifetime.
Clearing alternate-screen history does not affect primary graphics, while a
global history-budget reduction prunes primary history even when the alternate
screen is active. These operations affect live state, not a frozen synchronized
presentation. `term::graphics_history` regressions cover these ownership rules,
line/byte reductions, both clear entry points, non-resurrection after regrowth,
explicit reuse, and actual anonymous-pixel release after snapshot teardown.

When synchronized output starts while viewing scrollback, captured image rows
are rebased into the frozen viewport's row-zero coordinates, matching its text.
Only placement metadata changes: source crops, sub-cell texture coordinates,
pixel offsets, and reference-counted pixel storage are preserved. Repeated
DECSET 2026 does not rebase again, and hidden output, live scrolling, or screen
switches cannot move the captured frame. The `renderer::image::tests` tests
prefixed `synchronized_images_` compare the actual renderer geometry and texture
coordinates before/after capture for classic, Unicode, relative, and Sixel
images, including clipping and fragmented mode sequences. This evidence covers
capture and release at unchanged dimensions. Height-only resize has separate
coverage below; ordinary-placement column-reflow tracking remains audit work.

When only the row count changes, ordinary images move by the exact number of
rows that their owning grid archives, not by the window-height difference or
net history growth. Primary and alternate grids use their own cursor-dependent
movement and history limits. Relative placements follow their parent once, and
evicting a parent cascades through its descendants. Blank-tail trimming and
viewport growth do not shift images. Source crops, pixel geometry, and raster
rectangles extending below the viewport are preserved; viewport clipping is
not permanent image erasure. Frozen renderer instances follow the frozen grid's
separate resize movement and retained captured rows, never hidden live output.
The `term::graphics_resize` regressions cover both screens, cursor positions,
line/byte eviction, grow/shrink cycles, relative lifetime, tall pixel-sized
images, and actual anonymous-pixel release after a frozen snapshot ends. The
renderer test `height_resize_moves_frozen_images_with_their_own_text_not_hidden_output`
also checks clipped geometry/UVs for ordinary, relative, and Unicode instances
and restores captured image rows by scrolling the resized snapshot. This is
core/geometry evidence, not native resize acceptance or column-reflow coverage.

The `term::scroll_graphics` regressions pair images with uniquely labeled text
rows across SU, DL, and LF; cover batched scrolling, zero/capped/growing history,
byte-budget eviction, preserved source geometry, fixed/cross-boundary images,
relative-parent motion and eviction, alternate/interior/horizontal-margin
isolation, tall full-page images, and pixel-sized Sixel snapshot ownership.
They do not establish native visual acceptance or resolve the separately
reported row-at-a-time reveal without a native retest.

Core unit tests cover strict parsing, raw/PNG/zlib decoding, queries, chunk
continuations, default-denied ordinary files, opted-in range reads without
deletion, safe symlink resolution, protected/relative/non-regular-path denial,
replacement checks, and oversized persistent files; temporary-file range reads
and deletion; shared-memory range reads and unlinking, query behavior,
public-object and invalid-name rejection, current-Kitty bare-name
normalization, oversized shared-memory cleanup; current-Kitty 128-KiB chunks
and raw-unpadded temporary-file payloads, early-error `q=2` suppression;
anonymous IDs, allocated image-number replies, repeated-number newest/fallback
lookup, numbered chunking, ID replacement, number deletion, eviction-index
cleanup, accepted advisory usage hints, format-specific compressed raw/PNG
local-media sizes, malformed zlib cleanup, replacement, quiet/error replies,
Unicode virtual-placement parsing and bounded replacement, exact indexed and
true-color IDs, diacritic inheritance, high ID bytes, aspect-preserving
per-cell crop geometry, prototype deletion/eviction, reflow, scrollback,
synchronized-output and alternate-screen ownership, horizontal presentation
coalescing, and a current-client-sized 103-by-52 placeholder grid; natural and
single-axis destination sizing from cell pixels, including no-`c`/`r` quiet
transmit-and-display cursor movement; animation frame loading,
partial edits, base inheritance, transparent blending/replacement, chunk
continuation, composition and overflow errors, root promotion/deletion,
finite/loading playback, exact deadlines, frame revisions, synchronized-output
isolation, and count/memory accounting; relative placement parsing,
signed offsets, normal and Unicode parent movement, named-parent replacement,
screen ownership, negative-edge clipping, depth/cycle/missing-parent errors,
group deletion, scroll behavior, and cap eviction; deletion, overflow checks,
reference-aware LRU behavior, the independent placement caps, arbitrary PTY
fragmentation, screen lifecycle, scrolling, and APC overflow. Application tests
also cover protocol-v8-and-later text-reset plus graphics replay, fragmented one-shot
import, transparent animation frames, exact revisions/placements, image-number
reuse by later live output, reply suppression, and ordinary-terminal rejection.
They also prove that an elapsed Kitty frame boundary requests one redraw through the
idle scheduler instead of enabling the UI's 16 ms animation polling path.
The opt-in headless Vulkan lane uploads a 52-row RGBA texture, compares one
classic placement against 52 independent Unicode-placeholder slices across
negative and positive z passes, and reads every output pixel back. It requires
the complete initial frame, exact clipping after three rows of output scroll,
and complete scrollback restoration; a row-at-a-time reveal cannot pass. The
native X11 workspace scenario launches the installed current `kitten` four
times against a deterministic 64-by-64 RGBA PNG: classic stream, Unicode
placeholders, stdin-backed temporary-file transfer, and POSIX shared memory.
It fixes terminal pixel geometry, uses distinct image IDs and cursor positions,
records exact client output sizes, versions, and geometry, requires all four
images to reach successful protocol diagnostics, validates current-client
local-media targets, and waits for both client-owned objects to be removed
after PTY delivery. It also retains a dedicated post-client capture. The same
scenario invokes independent `timg -pk --compress=0` direct-PNG output, extracts
its client-generated image ID from the APC control fields, and requires the
matching terminal diagnostic rather than treating nonempty client output as a
pass.

Manual release acceptance still requires visible Wayland and X11 runs with a
current `kitten icat --transfer-mode=stream`,
`kitten icat --unicode-placeholder --transfer-mode=stream`, the stdin-based temporary-file
command above, `--transfer-mode=memory`, and an existing filename in file mode
with the policy both disabled and enabled, plus two simultaneous pane-local
clients. Also run a direct `P/Q/H/V` protocol probe that moves and deletes both
an ordinary parent and a Unicode-placeholder parent, confirming child movement,
signed offsets, and cascading deletion. Record the exact Kitty version, commands, image fixtures, compositor,
GPU, config state, and candidate SHA in `release-acceptance.md`. The automated
workspace path covers all four safe `kitten` transfer cases plus static direct-
PNG output from independent `timg` in one X11 window; ordinary-file policy,
multi-pane, Wayland, and subjective checks remain operator-owned.

Also run a direct multi-frame `a=f`/`a=a`/`a=c` probe that exercises partial
updates, alpha blend and replacement, loading and finite-loop playback, root
promotion, and `d=F`. Capture both the visible frame sequence and PTY replies;
this native-client judgment remains the release gate for animation despite the
hermetic protocol coverage.
