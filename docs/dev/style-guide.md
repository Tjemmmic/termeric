# Termeric Style Guide

Termeric is named after turmeric, the root, and its house themes are named after roots too. Read this before adding a theme or changing anything a person sees: chrome, dialogs, panels, menus, or text.

## 1. Themes

### Naming

- **Root themes** are Termeric's own palettes. They must be named after a real root, rhizome, or tuber, and set `author = "Termeric"`. The current twelve: turmeric (the default), ginger, daikon, malanga, beet, turnip, yuca, taro, wasabi, galangal, horseradish, parsnip.
- **Guest themes** are community palettes (Solarized, Catppuccin, Dracula, Nord, and others). They keep their original names and authors. The theme picker lists them under "Guest", apart from the roots.

The file name and lookup key are the lowercase name (`turmeric.toml`); the display name in `meta.name` is capitalized (`"Turmeric"`).

### Deriving a root palette

A root theme takes its colors from the actual plant: its skin, flesh, leaves, and flowers. Programs rely on what ANSI colors mean, though — red for errors, green for success, yellow for warnings, blue for directories, dim bright-black for hints — so every slot stays in its own hue family and only leans toward the plant. Turnip's red is a rose-tinted red rather than its purple top, and a leaf becomes a leaf-tinted teal for `cyan` rather than another green.

| Slot | Hue family | Tinted toward |
|---|---|---|
| `background` | — | The root's deepest skin or bark tone (dark) |
| `foreground` | — | The root's lightest flesh |
| `cursor` | — | The most vibrant part of the flesh |
| `black` | neutral | Dried skin, a step above the background |
| `white` | neutral | Lightest interior or pith |
| `red` | red | Skin blush or red/purple flesh streaks |
| `green` | green | The plant's leaves |
| `yellow` | yellow | Flowers or golden flesh |
| `blue` | blue | Stems or cool shadows |
| `magenta` | magenta | Flowers or purple flesh |
| `cyan` | cyan | Young leaves or shoots |
| `selection` | — | Semi-transparent flesh tone |
| `url` | — | Bright leaf or shoot color |

Not every root is warm: daikon, wasabi, and horseradish use cool or neutral charcoal backgrounds because the plants do.

### Palette contract

The test `root_theme_palettes_keep_ansi_meaning_and_contrast` in `termeric/src/config/theme.rs` enforces these rules for every theme authored `"Termeric"`:

- Each chromatic slot stays in its hue family (HSL degrees): red below 20 or from 345, yellow 35–70, green 75–155, cyan 160–200, blue 200–255, magenta 275–345.
- Saturation is at least 0.25, so tinted colors still read as colors. Keep it near or below 0.6 so nothing turns neon (advice, not enforced).
- Contrast against the background is at least 3:1 for primary slots and 4.5:1 for bright slots, and every bright slot is lighter than its primary.
- Neighbouring families (red/yellow, yellow/green, green/cyan, cyan/blue, blue/magenta, magenta/red) stay at least 25° apart.
- `black` is visibly lighter than the background (at least 1.15:1), bright black is readable dim text (at least 3:1), and whites reach 4.5:1 and 7:1.
- Foreground text reaches at least 7:1.

### Theme files

Built-in themes are TOML files in `termeric/themes/roots/` and `termeric/themes/community/`, embedded into the binary. `themes/roots/turmeric.toml` is the reference. A file has four tables:

- `[meta]`: `name`, `author`, `description`
- `[primary]` and `[bright]`: the eight ANSI colors each
- `[ui]`: `background`, `foreground`, `cursor`, `cursor_text`, `selection`, `border`, `tab_active` (the active-tab accent), `tab_inactive` (the tab strip), `search_match`, `url`, and optionally `selection_text`

Colors are `#RRGGBB` or `#RRGGBBAA`. In a root theme, give every color a short comment naming the part of the plant it came from.

To add a root theme:

1. Add `termeric/themes/roots/<name>.toml`.
2. Add the name to `BUILTIN_THEME_NAMES` and an `include_str!` arm to `Theme::load_builtin`, both in `termeric/src/config/theme.rs`.
3. Bump the root count in `root_theme_palettes_keep_ansi_meaning_and_contrast`.
4. Run `cargo test -p termeric config::theme`.

People can add their own themes as `.toml` files in `~/.config/termeric/themes/`; the **Change Theme…** picker previews each theme as it is highlighted.

## 2. Chrome

The terminal content comes first; the chrome around it stays quiet.

- **Color:** every chrome color comes from the active theme through `UiPalette` (`termeric/src/renderer/colors.rs`), so light themes and custom themes work. Don't hardcode colors. Shadows are black at low alpha.
- **Tab bar:** compact and "quiet native". The strip uses the theme's `tab_inactive` color; the active tab is a slightly raised surface with a 2px `tab_active` accent line (a left bar in the vertical rail); inactive titles are dimmed; × appears on the active or hovered tab, and background activity shows as a small dot in its place. Never fill a whole tab with a saturated color.
- **Settings:** one centered dialog with a sidebar of pages and one search across every page. Values sit in one column with steppers, dropdown chevrons, or a trailing chevron for actions. The footer only says whether changes are saved (with a Save button when they aren't); no key-hint strips.
- **Dialogs and menus:** start from the shared pieces — `build_modal_frame` (`renderer/palette.rs`) for centered dialogs, `build_side_panel_frame` (`renderer/panels.rs`) for side panels, and `build_popup_menu` (`renderer/popup_menu.rs`) for anchored menus; a menu with its own rows, such as the Settings dropdown, still sits on the shared `push_menu_surface`.
- **Icons:** use the vector icons in `renderer/icons.rs` (`Icon`, `push_icon`) rather than text glyphs such as ◀ ▶ ×.
- **Hover and feedback:** a gentle background tint, not a color change. Prefer status text and toasts over modal dialogs.

### Corners

Draw rounded rectangles with `RectPipeline::push_rounded_rect`; a radius of half the height makes a pill or circle. The radii in use:

| Radius | Used for |
|---|---|
| 12px (`CARD_CORNER_RADIUS`) | Cards: toasts, the info overlay, the guardrail confirmation, Harvest panels |
| 10px | The shared modal frame (Settings, command palette) |
| 8px | Popup menus, dropdowns, the find box |
| 6px (`BUTTON_CORNER_RADIUS`) | Buttons, tabs, Settings sidebar rows |
| 2px | Block cursor |

Shared constants live in `termeric/src/ui/constants.rs`; add a constant there rather than a new literal when a radius is reused.

### Geometry

Chrome geometry lands on whole physical pixels. Rendering, hit testing, keyboard focus, and accessibility bounds come from one shared function per component (see `TabChrome` in `renderer/tab_bar.rs`), so what a person clicks is always what they see.

## 3. Motion

- Animate with `Instant`-based timing and the shared ease-out cubic (`ease_out_cubic` in `termeric/src/ui/animation.rs`). Never accumulate frame deltas, and never bounce or overshoot.
- Keep transitions short: 150ms for overlays and 200ms for panels by default (`DEFAULT_OVERLAY_DURATION`, `DEFAULT_PANEL_DURATION`); nothing over 250ms.
- The cursor glides to its new position over 150ms, including across tab switches.
- Honor reduced motion: `OverlayAnimation` and `PanelAnimation` snap when it is on, and any new animation must do the same.

## 4. Typography

- **Terminal text** uses the configured monospace font (default `monospace`, 12pt). `font.size` is in points at 96/72 logical pixels per point, from 8pt to 72pt. The Settings stepper moves 1pt; the zoom shortcuts move 2pt. Terminal text keeps the monospace grid: no custom letter-spacing or squeezed advances.
- **UI text** (tabs, Settings, the palette, panels, menus, dialogs, toasts, the find box) uses the desktop's sans-serif face at 10pt, resolved through fontconfig, so chrome reads like the rest of the desktop. Draw it with `Renderer::push_ui_label`, which measures proportionally, centres on the face's own metrics, and shortens with an ellipsis.
- **Emoji** render inline when a font supports them and fall back gracefully.

## 5. Accessibility

- High-contrast mode raises built-in chrome text to at least 4.5:1 (`enforce_high_contrast` in `renderer/colors.rs`); new chrome colors must go through `UiPalette` so they get the same treatment.
- Every interactive control needs keyboard access and an AccessKit node whose bounds come from the same geometry used to draw it. See [Accessibility](../src/accessibility.md).
