# Unicode support policy

Termeric's terminal grid supports left-to-right Unicode extended grapheme
clusters. This is a cell-layout contract, not a claim of complete international
text support.

## Width and segmentation

- Grapheme boundaries use `unicode-segmentation` 1.12 data for Unicode 16.0.
- Cell widths use `unicode-width` 0.2 data for Unicode 17.0.
- East Asian ambiguous-width characters are narrow (one cell).
- CJK and emoji-presentation clusters use two cells when the width table says
  two; the second cell is an attributed empty spacer.
- Combining marks, variation selectors, regional-indicator flags, emoji skin
  tones, and ZWJ emoji extend the preceding cluster when Unicode grapheme rules
  keep the combined text in one cluster.
- An orphan zero-width scalar after an explicit cursor/control boundary is
  composed over a blank one-cell grapheme. It is not attached across that
  boundary or silently discarded.

If a one-column grid cannot represent a two-cell cluster, Termeric retains the
cluster in the only cell without creating an orphan spacer. This degenerate
case is intentionally not treated as a normal two-column layout.

## Grid invariants

A width-two leader and its spacer are an atomic pair. Write, insert, delete,
erase, resize, and reflow operations repair both halves together. Selection
coordinates remain cell-based, but selection, search, detected-URL mapping,
plain-text extraction, and session export emit the leader's complete grapheme
once and never serialize the spacer as an extra space.

The renderer sends complete multi-scalar clusters through advanced
`cosmic-text` shaping even when optional programming ligatures are disabled.
Native color-font glyphs retain RGBA pixels in a separate bounded atlas; mask
glyphs remain theme-tinted R8 data. Bitmap dimensions never replace the
terminal grapheme-width policy.

## Character discovery and insertion

The **Insert Unicode Character...** command opens a bounded in-app picker. Its
curated index covers common punctuation, currencies, arrows, mathematics,
technical symbols, box drawing, shapes, and emoji. Search includes displayed
characters, names, aliases, categories, and code points. Exact `U+XXXX`,
`0xXXXX`, and four-to-six-digit bare hexadecimal queries address any valid
non-control Unicode scalar without enumerating the Unicode code space.

The picker preserves complete curated grapheme sequences, including variation
selectors and joiners, and inserts their exact UTF-8 once through the normal
terminal-input path. It captures the opening tab/pane and refuses insertion if
focus, writability, or broadcast state changes. Controls, surrogates, malformed
hexadecimal, and values beyond `U+10FFFF` produce no executable result.

This is deliberately not a complete Unicode-name database. Characters outside
the curated index remain reachable by code point, but arbitrary full-name or
localized-name search is not claimed. The picker does not guarantee that the
configured fonts contain a glyph for the selected scalar.

Font choice can be refined with bounded inclusive `font.range_overrides`.
Lookup uses the grapheme's first non-default-ignorable scalar, but the selected
family must cover every substantive scalar in that grapheme or Termeric falls
through to the normal ordered family chain. Range routing never changes the
Unicode width result or the primary font's terminal cell metrics. See the
[SGR rendering policy](sgr-rendering.md#unicode-range-font-overrides).

## Evidence and exclusions

Table-driven core tests cover combining marks, CJK, ambiguous width, VS15/VS16,
flags, skin tones, ZWJ emoji, final-column wrapping, editing, selection, export,
search, URL hit targets, and repeated reflow. The conformance fixture is replayed
at every byte boundary, a real PTY emits representative clusters, and the
headless WGPU lane shapes and submits the full strings without scalar
truncation.

Bidirectional reordering, exhaustive font-coverage guarantees, and exhaustive
Unicode-name search remain outside this policy. IME/preedit, color emoji, and
accessibility semantics are covered
by [modern text input and accessibility](accessibility-and-input.md).
