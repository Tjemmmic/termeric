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

The **Insert Unicode Character...** command opens a bounded in-app picker. Empty
input shows a curated index of common punctuation, currencies, arrows,
mathematics, technical symbols, box drawing, shapes, and emoji. Non-empty name
queries search all 153,626 non-control scalars with canonical names in the
Unicode 17.0 database supplied by `unicode_names2` 4.0.0. Whole-name, token
prefix, and token-substring matches have deterministic ranking; exact canonical
names and Unicode name aliases follow UAX #44 loose matching. Curated names,
displayed graphemes, friendly aliases, categories, and code points participate
in that same search. Exact `U+XXXX`, `0xXXXX`, and four-to-six-digit bare
hexadecimal queries address any valid non-control Unicode scalar.

The canonical-name index is built once after the picker opens and retains
5,226,122 bytes in its compact name/offset representation on Unicode 17.0.
Construction preallocates and hard-caps the backing name bytes at 4,000,000 and
the fixed-size record table at 160,000 entries, avoiding growth overshoot.
Queries scan that immutable index without reconstructing Unicode names and keep
only the best 256 scalar candidates; after curated results are merged and
deduplicated, no more than 256 executable rows enter the shared palette or its
accessibility tree. The deterministic warm-query rail covers five broad and
algorithmic queries under a 250 ms aggregate ceiling (48.8 ms observed on the
September 2026 development host). This is a regression guard, not a universal
hardware latency guarantee.

The picker preserves complete curated grapheme sequences, including variation
selectors and joiners, and inserts their exact UTF-8 once through the normal
terminal-input path. It captures the opening tab/pane and refuses insertion if
focus, writability, or broadcast state changes. Controls, surrogates, malformed
hexadecimal, and values beyond `U+10FFFF` produce no executable result.

Canonical names are English Unicode identifiers, not localized character
descriptions or CLDR emoji annotations. Unassigned/private-use scalars and other
values without canonical names remain reachable only by exact code point when
they pass the non-control safety policy. Alias resolution never makes a control
character executable. The picker does not guarantee that the configured fonts
contain a glyph for the selected scalar.

Font choice can be refined with bounded inclusive `font.range_overrides`.
Lookup uses the grapheme's first non-default-ignorable scalar, but the selected
family must cover every substantive scalar in that grapheme or Termeric falls
through to the normal ordered family chain. Range routing never changes the
Unicode width result or the primary font's terminal cell metrics. The bounded
Settings manager lists canonical intervals and atomically adds, edits, or
confirmed-deletes rules through the same validation contract; edits apply live
and persist only when Settings is saved. Its read-only coverage inspector
segments literal input into at most 16 complete graphemes (or accepts an exact
`U+` / `0x` scalar), identifies the explicit family attempts and final platform
shaping face, and visibly distinguishes missing from installed-incomplete
families. This is font-database/shaping evidence, not a native screenshot or
appearance guarantee. See the
[SGR rendering policy](sgr-rendering.md#unicode-range-font-overrides).

## Evidence and exclusions

Table-driven core tests cover combining marks, CJK, ambiguous width, VS15/VS16,
flags, skin tones, ZWJ emoji, final-column wrapping, editing, selection, export,
search, URL hit targets, and repeated reflow. The conformance fixture is replayed
at every byte boundary, a real PTY emits representative clusters, and the
headless WGPU lane shapes and submits the full strings without scalar
truncation.

Bidirectional reordering, exhaustive system-font recommendations, localized
character descriptions, and CLDR emoji keyword search remain outside this
policy. IME/preedit, color emoji, and accessibility semantics are covered
by [modern text input and accessibility](accessibility-and-input.md).
