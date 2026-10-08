# Test fonts

The renderer's tests shape and rasterize with these fonts, built into the test binary, so the same text shapes the same way on every machine. Each is used under the SIL Open Font License beside it; none declares a Reserved Font Name.

| File | Source | Kept |
|---|---|---|
| `FiraMono-Medium.ttf` | Fira Mono Medium 3.206 | Whole font: the GPU smokes and the visual baseline draw with it |
| `InterVariable.ttf` | Inter Variable 4.001 | Space and A–Z, with every variation axis: the variable-weight test shapes "M" |
| `NotoSansArabic.ttf` | Noto Sans Arabic 2.004, variable | Basic Latin, no-break space, ش (U+0634), fatha (U+064E), shadda (U+0651), the Arabic letter mark, the joiners and direction marks (U+200C–200F), and the dotted circle (U+25CC): the fallback and range-override tests shape digits and "شَّ" |

The two subsets were made with fontTools' `pyftsubset`, keeping every layout feature and name:

```bash
pyftsubset InterVariable.ttf --unicodes="U+0020,U+0041-005A" \
  --layout-features='*' --name-IDs='*' --name-languages='*' --notdef-outline --glyph-names
pyftsubset NotoSansArabic.ttf --unicodes="U+0020-007E,U+00A0,U+0634,U+064E,U+0651,U+061C,U+200C-200F,U+25CC" \
  --layout-features='*' --name-IDs='*' --name-languages='*' --notdef-outline --glyph-names
```

A test that needs another character from these two must add it to the subset.
