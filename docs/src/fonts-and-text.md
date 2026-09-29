# Fonts and text

Termeric draws text on a fixed grid of cells, one font size for the whole window. This page covers choosing the font, zooming, fallback fonts and emoji, ligatures and other OpenType features, how Unicode text fits the grid, and the Unicode character picker.

## Choosing a font

Open Settings (Ctrl+,) and go to the **Font** page. The **Family** dropdown lists the monospace fonts installed on your system; `monospace` means the system's default monospace font. Changes show immediately; press Ctrl+S or click **Save** to keep them.

The same settings in `config.toml`:

```toml
[font]
family = "JetBrains Mono"
size = 13.0          # points, 8 to 72
weight = 400         # normal text, 1 to 1000
bold_weight = 700    # bold text, at least weight
line_height = 1.2    # multiple of the font size, 0.8 to 3.0
```

The default is `monospace` at 12 points with a line height of 1.2. Use the family name exactly as your system reports it; `fc-list : family` lists them. If the family is not installed, Termeric draws with other installed fonts instead, so check the name if the text looks wrong. The primary font sets the size of every cell, so choose a true monospace font.

## Zoom

| Keys | Action |
| --- | --- |
| Ctrl+= | Increase the font size by 2 points |
| Ctrl+- | Decrease the font size by 2 points |
| Ctrl+0 | Return to the configured size |

Zoom affects only the current window and stays between 8 and 72 points. Saving Settings stores the window's current size as `font.size`. You can rebind these under Keyboard Shortcuts in Settings (see [Keyboard and mouse](keyboard-and-mouse.md)).

## Bold and italic

Bold and italic text use the matching faces of your font. If the font has no bold face, Termeric thickens the regular one slightly; if it has no italic face, the regular one is slanted. With a variable font, `weight` and `bold_weight` set its real weight axis, so you can use, say, 350 and 600 for a lighter look.

Some older color schemes expect bold text to switch to the bright colors. Turn on **Bold Is Bright** in Settings, or set `bold_is_bright = true`, to show bold text in ANSI colors 0–7 using colors 8–15 instead.

## Fallback fonts and emoji

When the primary font lacks a character, Termeric tries your fallback fonts in order, then the system's own font fallback, which picks a font suited to the script (for example a CJK font for Japanese). Each character is kept whole, including accents, emoji skin tones, and joined emoji sequences: a fallback font is used only if it can draw the complete character.

Set up to 16 fallbacks in Settings as **Fallback 1**, **Fallback 2**, and so on (a new row appears as you fill one), or in the file:

```toml
[font]
fallback = ["Noto Sans Mono", "Noto Sans Symbols 2", "Noto Color Emoji"]
```

Emoji appear in color when a color emoji font such as Noto Color Emoji is installed. Fallback fonts never change the cell size; their glyphs are placed in the grid set by the primary font.

### Preferring a font for a range of characters

A range override sends a block of Unicode code points to a particular font before the fallback list is tried. This is useful for icon fonts such as Nerd Fonts, or to pick which font draws a script.

In Settings, choose **Range Overrides** on the Font page. The manager lists your rules and lets you add, edit, and delete them. Enter a rule as `U+START-U+END | FAMILY`, for example:

```text
U+E000-U+F8FF | Symbols Nerd Font Mono
```

**Inspect glyph coverage** in the same manager takes some text or a code point and reports which font would draw each character, and whether a named family is missing or lacks some of the characters. Rule changes apply at once; save Settings to keep them.

In `config.toml`, add one entry per range (at most 64, and ranges may not overlap):

```toml
[[font.range_overrides]]
start = 0xE000
end = 0xF8FF
family = "Symbols Nerd Font Mono"
```

If the chosen font cannot draw a character in the range, Termeric falls through to the normal order.

## Ligatures

Fonts such as Fira Code and JetBrains Mono join sequences like `->`, `=>`, and `!=` into single symbols. Ligatures are on by default. Turn them off with **Ligatures** in Settings, or:

```toml
[font]
ligatures = false
```

This switches the standard, contextual, and discretionary ligature features (`liga`, `clig`, `calt`, `dlig`) together. Ligatures only change how text looks: the cursor, selection, search, and copying still work one cell at a time, and each ligature starts at its first cell.

## OpenType features

Many programming fonts offer stylistic sets and character variants, such as a slashed zero or a different `r`. You can turn these on by their four-letter OpenType tags.

In Settings, choose **OpenType Features** on the Font page, then add a feature as `tag = value`, for example `ss01 = 1` or `cv05 = 2`. In `config.toml`:

```toml
[font.features]
ss01 = 1
cv05 = 2
calt = 0
```

A value of `1` turns a feature on and `0` turns it off; some features take larger values to choose between alternatives. A feature you set here overrides the Ligatures switch for that tag, so `calt = 0` keeps contextual alternates off even with ligatures on. You can set up to 32 features. Kerning (`kern`) is not allowed because it would move characters off the grid. A tag the font does not support has no effect; check your font's documentation for the tags it offers.

## Unicode and wide characters

Termeric treats each user-perceived character (a grapheme cluster) as one unit:

- Accented letters, emoji with skin tones, flags, and joined emoji sequences occupy one grid position and are copied, searched, and selected as a whole.
- CJK characters and emoji shown as pictures take two cells.
- Characters of ambiguous width, such as some Greek, Cyrillic, and box-drawing symbols in East Asian fonts, take one cell.
- Widths follow Unicode 17.0.

Text is laid out left to right. Right-to-left scripts such as Arabic and Hebrew display their characters, but lines are not reordered for bidirectional text.

## Text sizing

Programs can draw text larger than the normal cell size with the Kitty text-sizing protocol (OSC 66), for example for headings in a terminal document viewer. A block can be scaled up to 7 times, and fractional sizes are aligned within their cells. The enlarged text moves, wraps, scrolls, and copies as one block.

```bash
printf '\e]66;s=2;Double size\a\n\n'
```

## Inserting a Unicode character

The Unicode character picker finds any character by name and types it into the terminal. Open it with **Insert Unicode Character…** in the command palette (Ctrl+Shift+P). It has no default key; you can assign one to **Insert Unicode Character** under Keyboard Shortcuts in Settings.

- With an empty search, it shows common punctuation, currency signs, arrows, math and technical symbols, box drawing, shapes, and emoji.
- Type words from a character's Unicode name, such as `right arrow` or `greek small alpha`.
- Emoji can also be found by everyday English keywords, such as `thumbs up` or `party`.
- Type a code point as `U+2192`, `0x2192`, or just `2192` to get that exact character.

Press Enter to insert the highlighted character into the terminal that was focused when you opened the picker, or Escape to close it. The picker is not available while broadcast input is on or in a read-only pane, and it inserts nothing if you move focus to another pane first. It does not check whether your fonts can draw the character; if one shows as a box, add a fallback font that covers it.
