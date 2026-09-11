# SGR text rendering

Termeric renders every text attribute represented by `CellAttrs`. Attribute
state is resolved once per cell and the result is shared by color selection,
font shaping, glyph rasterization, decorations, blink visibility, selection,
and block-cursor repaint.

## Supported attributes

- Bold and italic request real matching faces from `cosmic-text` for primary
  and fallback glyphs.
- If no bold face is available, Termeric applies a bounded one-pixel embolden.
  If no italic face is available, `cosmic-text` applies a 14-degree synthetic
  slant. Each fallback is reported once in the diagnostic log.
- Dim scales foreground glyphs and decorations to 66% intensity. It does not
  alter the cell background.
- Hidden suppresses glyphs, underline, strikethrough, and link decoration. The
  underlying content remains searchable, selectable, copyable, and exportable.
- Slow and rapid blink gate foreground paint at 500 ms and 250 ms phase
  boundaries respectively. Terminal content never changes.
- Single, double, curly, dotted, and dashed underline are distinct. SGR 58
  underline color is honored, and strikethrough uses the resolved foreground.
- Inverse swaps resolved foreground and background before dim and decoration
  colors are applied. An explicit underline color is not swapped.

All foreground glyph and decoration geometry is clipped to its pane viewport,
including synthetic bold and italic overhang.

Applications can query the complete represented rendition with DECRQSS `m`.
The response preserves underline variants, indexed/true colors, and underline
color in a replayable SGR body. DECRQSS also reports vertical and horizontal
margins, legacy page/screen geometry, the current DECSCUSR cursor setting, the
independent DECSCA character-protection setting, and the DECSACE
attribute-change extent; unsupported setting families return a negative
response rather than fabricated state.

DECFRA applies the current SGR and DECSCA state to every filled character.
DECERA creates ordinary BCE blanks, while DECSERA and DECSED/DECSEL clear only
erasable content and retain each affected cell's existing visual and protection
attributes.

DECSACE selects how DECCARA and DECRARA interpret their endpoints. Stream mode
wraps across the full page width and changes only cells containing drawn
characters. Exact-rectangle mode keeps the requested columns and materializes
never-drawn cells as spaces before changing their rendition. DECCARA applies
ordered set/reset operations and DECRARA applies parity-preserving reversals for
bold, single underline, blink, inverse, and xterm hidden text. Both preserve
unrelated modern rendition flags, colors, protection, hyperlinks, content,
wide-cell structure, cursor position, and delayed-wrap state.

DECSCUSR parameters 0-6 select cursor shape and blink policy. DECSET/DECRST 12
can then enable or disable blink without changing that shape; a later DECSCUSR
becomes the new authoritative shape/blink choice. Until either control is used,
the configured cursor remains authoritative, and RIS restores it. DECRQM
reports the live mode-12 state, while synchronized output freezes the cursor
presentation seen by the renderer until the batch is released.

## Bold and bright colors

Font weight and ANSI color intensity are independent by default. Set the
following compatibility option to promote indexed foreground colors 0-7 to
their bright 8-15 counterparts when bold is active:

```toml
[font]
bold_is_bright = true
```

The promotion happens before inverse. Default, 256-color, and truecolor
foregrounds are never promoted.

## Unicode-range font overrides

`font.range_overrides` can prefer an installed family for an inclusive Unicode
scalar interval before the primary and ordered fallback chain:

```toml
[font]
family = "Fira Mono"
fallback = ["Noto Sans Mono", "Noto Color Emoji"]

[[font.range_overrides]]
start = 0xE000
end = 0xF8FF
family = "Symbols Nerd Font Mono"
```

At most 64 rules are accepted. Shared configuration validation rejects unsafe
family names, reversed ranges, values beyond `U+10FFFF`, intervals that touch
the surrogate block, and inclusive overlap. Adjacent rules are valid. Rules
are compiled once in start-code-point order and selected with bounded binary
search.

The first non-default-ignorable scalar in a grapheme selects a rule. Joiners,
variation selectors, tags, and all other substantive scalars remain in the
same complete-grapheme coverage check. The mapped installed family is used
only if one of its faces covers that entire content; otherwise normal ordered
fallback continues. This applies identically to scalar rasterization and
advanced run/grapheme shaping. The primary family alone still defines cell
metrics, and terminal width remains governed by the Unicode width policy.

Startup and live reload apply the rules. A changed rule list invalidates
persistent terminal rows plus scalar, shaped-run, mask-atlas, and color-atlas
state. Settings shows how many rules are active; structured rule editing remains
configuration-file-only.

## Blink scheduling

Blink uses `Instant`-based phase boundaries. After each frame, the renderer
reports whether slow or rapid blinking foreground occurs in the visible rows.
The event loop schedules only the required next boundary. Hidden cells, empty
cells without decorations, offscreen history, and tabs or panes outside the
current render do not create periodic redraws. An unfocused or occluded window
waits until the next real event and paints the current phase when shown again.

## Evidence and remaining manual checks

Parser tests cover every set/reset sequence, alternate-screen rendition state,
underline styles, underline colors, and rapid blink. A real shell PTY emits a
combined attribute fixture. The headless WGPU lane checks requested font faces,
hidden/blink suppression, pane clipping, deterministic decoration geometry,
and a real glyph/rectangle submission. Existing Vim, less, and tmux raw traces
remain in the core conformance suite, and a real Git color-diff fixture verifies
bold rendition. Native Vim/less/tmux interaction now runs in the GUI acceptance
lane; the manual release matrix covers subjective font and decoration quality.

Color emoji uses a separate RGBA glyph atlas. Font-family-specific
appearance and vendor GPU presentation remain visual acceptance concerns, not
claims made by this document.
