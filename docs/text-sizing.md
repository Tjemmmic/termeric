# Kitty OSC 66 text sizing

Termeric implements the bounded grid and rendering subset described by
Kitty's [text sizing protocol](https://sw.kovidgoyal.net/kitty/text-sizing-protocol/).
The implementation is tracked by [issue #72](https://github.com/Tjemmmic/termeric/issues/72).

## Supported contract

`OSC 66 ; metadata ; text ST` and BEL-terminated forms accept the defined
`s`, `w`, `n`, `d`, `v`, and `h` keys. Metadata is strict, duplicate and
unknown keys fail closed, and text is valid safe UTF-8 limited to 4096 bytes.
Integer scale and fractional alignment are represented in a four-byte packed
descriptor repeated across an atomic `s*w` by `s` rectangle. With `w=0`, text
is segmented into Unicode grapheme clusters and each cluster receives its
normal one- or two-cell width before scaling.

The terminal enforces screen and scroll-region bounds, DECAWM placement,
cursor advance, combining-character extension, leader/top-row/lower-row
overwrite behavior, ICH/DCH/ECH, ED/EL, selective erase, line edits, scrolling,
rectangular operations, resize cleanup, synchronized-output snapshots, compact
history, ED 22 viewport archival, semantic text extraction, ANSI session replay,
and local-domain attachment snapshots. Structural edits either preserve a
complete block or erase it; malformed descriptors never gain authority over
neighboring cells.

The renderer shapes OSC 66 text at a distinct font size through the bounded
glyph atlas, aligns fractional render areas inside the full block, clips to the
block and pane, and expands block, beam, and underline cursor geometry over the
occupied rectangle. Ordinary one-cell text stays on the existing fast path.

## Resource and trust boundary

- Text is capped at 4096 bytes and metadata at 128 bytes per operation.
- Scale and logical width are each capped at seven, limiting a block to 49 by
  seven cells without auxiliary heap ownership per covered cell.
- Font-size-specific shapes share the existing fixed-capacity mask and color
  glyph atlases and their frame-safe eviction rules.
- OSC 66 performs no filesystem, network, clipboard, process, or provider
  operation.

## Deferred native acceptance

Automated tests establish protocol, grid, export, resize, semantic, alignment,
and cache integration behavior. They do not establish subjective font quality
or compositor-specific presentation. Run these checks later in a native
Termeric window; no AI provider or authenticated service is involved:

```sh
printf '\033]66;s=2;Double\007\n\n'
printf '\033]66;s=3;Triple\007\n\n\n'
printf '\033]66;n=1:d=2;Super\007 normal\n'
printf '\033]66;s=2:n=1:d=2:v=1:h=2:w=3;align\007\n\n'
```

Confirm that every string appears immediately as one stable rectangle, that
fractional examples align within their occupied cells, that resizing never
reveals rows progressively, and that placing each cursor style on any covered
cell highlights the complete protocol-defined shape. Record the terminal
dimensions, scale factor, font, compositor/session, GPU/backend, and screenshot
when accepting or reporting a failure.
