# Termeric Style Guide

> **For contributors and AI agents working in this repo.**
> Termeric is a play on *Turmeric* — the root. That identity shapes everything: the icon, the themes, the animations, and the overall feel. Read this guide before writing any UI code, theme, or user-facing feature.

---

## 1. Brand Identity

Termeric is a terminal emulator that draws its personality from **roots and rhizomes** — the humble, earthy, organic things that grow underground. The icon is a little turmeric root with warm golden-orange tones and rounded knobby limbs.

### Voice & Naming

| Do                                 | Don't                              |
|------------------------------------|------------------------------------|
| Name themes after real roots       | Use generic tech names             |
| Use warm, earthy language          | Use cold, clinical language        |
| Say "organic", "grounded", "warm"  | Say "sleek", "futuristic", "sharp" |

Built-in theme names **must** be real roots, rhizomes, or tubers:
`turmeric`, `ginger`, `daikon`, `turnip`, `malanga`, `beet`, `yuca`, `taro`, `wasabi`, `galangal`, `horseradish`, `parsnip`, etc.

Third-party / community themes (Solarized, Catppuccin, etc.) keep their original names but are presented as "guest themes" — the roots are the house palette.

---

## 2. Root-Themed Color Palettes

Every root theme **must** derive its colors from the **actual plant**. Study photos of the root's skin, flesh, leaves, and flowers, then use those hues to *tint* each terminal slot.

ANSI colors carry meaning that programs rely on: red for errors, green for success, yellow for warnings, blue for directories, dim bright-black for hints. A root theme therefore keeps every slot in its own hue family and only leans it toward the plant. Turnip's red is a rose-tinted red rather than the purple top itself, and a leaf becomes a leaf-tinted teal for `cyan` rather than another green.

### Color-mapping template

| Terminal Slot | Hue family | Tinted toward                              |
|---------------|------------|--------------------------------------------|
| `background`  | —          | The root's deepest skin/bark tone (dark)   |
| `foreground`  | —          | The root's lightest flesh (cream/ivory)    |
| `cursor`      | —          | The most vibrant part of the flesh         |
| `black`       | neutral    | Dried skin, a step above the background    |
| `white`       | neutral    | Lightest interior or pith                  |
| `red`         | red        | Skin blush or red/purple flesh streaks     |
| `green`       | green      | Leaf color of the plant                    |
| `yellow`      | yellow     | Flower color or golden inner flesh         |
| `blue`        | blue       | Stems or cool shadows                      |
| `magenta`     | magenta    | Flower or purple flesh highlights          |
| `cyan`        | cyan       | Young leaves or shoots                     |
| `selection`   | —          | Semi-transparent inner flesh tone          |
| `url`         | —          | Bright leaf or shoot color                 |

### Palette contract

`root_theme_palettes_keep_ansi_meaning_and_contrast` in `config/theme.rs` enforces these rules for every theme authored `"Termeric"`:

- Each chromatic slot stays in its hue family (HSL degrees): red below 20 or from 345, yellow 35–70, green 75–155, cyan 160–200, blue 200–255, magenta 275–345.
- Saturation is at least 0.25, so tinted colors still read as colors. Avoid the other extreme too: keep saturation near or below 0.6 so nothing turns neon.
- Contrast against the background is at least 3:1 for primary slots and 4.5:1 for bright slots, and every bright slot is lighter than its primary.
- Neighbouring families (red/yellow, yellow/green, green/cyan, cyan/blue, blue/magenta, magenta/red) stay at least 25° apart.
- `black` is visibly lighter than the background (at least 1.15:1), bright black is readable dim text (at least 3:1), and whites reach 4.5:1 and 7:1.
- Foreground text reaches at least 7:1.

### Reference palettes

#### Turmeric *(default theme)*
- **Skin**: Deep brown `#1C1410`, medium brown `#3D3028`
- **Flesh**: Bright orange-gold `#E07B20`, earthy yellow `#C9A227`
- **Leaves**: Forest green `#5D8C4A`, bright leaf `#7CB86A`
- **Dried**: Cream tan `#D4C4A8`, light cream `#F5EBD9`

#### Ginger
- **Skin**: Pale sandy tan `#C8B491`, papery beige `#A89272`
- **Flesh**: Pale straw yellow `#F2E8C9`, fibrous cream `#EDE0C0`
- **Tips**: Pale pink-rose `#D4A0A0` from young ginger shoots
- **Leaves**: Warm olive green `#6B7D3C`, reed green `#8FA55A`

#### Daikon
- **Skin**: Clean white `#F0EDE8`, greenish top `#B8C9A0`
- **Flesh**: Crisp white `#FAFAF5`, translucent ivory `#F5F2E8`
- **Leaves**: Dark leathery green `#3D5E38`, bright leaf `#6BA34F`
- **Tips**: Pale green `#D4E0C4` fading to white

#### Malanga
- **Skin**: Grey-brown `#8C7B6B`, with purple-red blush `#7B4A5C`
- **Flesh**: Creamy white `#F0E8DC` with dark purple splotches `#4A2D5C`
- **Leaves**: Dark earthy green `#2D4A2D`, deep forest `#3D5C3D`
- **Flower**: Rich yellow `#D4A82A`, warm gold `#C99B1D`

#### Beet
- **Skin**: Dark burgundy `#4A1028`, maroon `#6B1832`
- **Flesh**: Vivid crimson `#C41E3A`, magenta streak `#D44D6E`
- **Rings**: Pale pink-white `#F0D0D8`, alternating beetroot rings
- **Leaves**: Dark red-green `#3D4A28`, veined with crimson

#### Turnip
- **Skin**: Creamy white `#F0EBE0` fading to purple-rose `#9B5BA0` at top
- **Flesh**: Pure white with faint yellow `#FAF8F0`
- **Leaves**: Fuzzy sage green `#7B9B5C`, bright spring `#A0C46A`
- **Roots**: Fine hairlike cream `#E0D8C8`

#### Yuca (Cassava)
- **Skin**: Bark-like dark brown `#5C4030`, rough woody `#7B6450`
- **Flesh**: Starchy bright white `#FAFAF8`, chalky `#F0ECE4`
- **Leaves**: Deep tropical green `#2D5C28`, palmate fan shape
- **Inner bark**: Pink-coral layer `#D4A090`

#### Taro
- **Skin**: Dark dusty brown `#5C4A40`, hairy fibrous
- **Flesh**: Pale lavender-white `#E8E0F0`, purple veins `#7B5C9B`
- **Leaves**: Giant elephant ear dark green `#2D4A30`, velvety
- **Flowers**: Creamy yellow spadix `#E8D88C`

#### Wasabi
- **Skin**: Bright green `#5C8C32`, knobby rhizome
- **Flesh**: Warm pale green `#C8D8A0`, fresh chartreuse `#A0C850`
- **Leaves**: Dark heart-shaped green `#2D5C2D`, glossy
- **Grated**: Vivid bright green `#7BC828`, the familiar wasabi paste

#### Galangal
- **Skin**: Pale pinkish-tan `#D4B8A0`, translucent rings
- **Flesh**: Pale ivory `#F5F0E4`, resinous golden `#D4B878`
- **Shoots**: Bright hot pink `#E05A8C`, magenta `#C84080`
- **Leaves**: Long blade green `#4A7D3C`, tropical

#### Horseradish
- **Skin**: Sandy tan `#C8B890`, rough and scarred
- **Flesh**: Sharp white `#FAFAF8`, cream when oxidized `#F0E8D0`
- **Leaves**: Large, crinkled dark green `#3D5C2D`, bold
- **Cross-section**: Pale yellow `#F0E8B8` with a fibrous core

#### Parsnip
- **Skin**: Off-white to pale cream `#F0E8D0`, subtle tan `#D8C8A4`
- **Flesh**: Sweet cream `#FAF5E4`, smooth with faint yellow core `#E8D898`
- **Leaves**: Celery green `#5C8C3C`, ferny and delicate
- **Tops**: Stem green-white `#B4C890`

---

## 3. Interaction Design Principles

### 3.1 Fluidity

Everything in Termeric **flows**. Nothing snaps.

| Element              | Behavior                                                                    |
|----------------------|-----------------------------------------------------------------------------|
| **Cursor movement**  | Smoothly animates position, even across tab switches                        |
| **Panel open/close** | Slide-in with ease-out easing (`1 - (1-t)²`), ~200ms                       |
| **Settings values**  | Transition smoothly between states (don't just swap the text)               |
| **Tab switching**    | Content cross-fades or slides, active tab indicator animates position       |
| **Scroll**           | Always smooth-scrolled, never jump unless terminal output demands it        |
| **Selection**        | Highlight grows/shrinks fluidly as mouse moves                              |

**Implementation rule**: If a value changes on screen, it should animate to the new state using `Instant`-based, frame-rate-independent interpolation. Never use raw `dt` from frame deltas — use absolute elapsed time against a fixed duration.

```rust
// ✅ Good: absolute time animation
let elapsed = self.trigger_time.elapsed().as_secs_f32();
let t = (elapsed / ANIMATION_DURATION).min(1.0);
let eased = 1.0 - (1.0 - t) * (1.0 - t); // ease-out

// ❌ Bad: delta-time accumulation (drifts, stutters)
self.progress += dt / duration;
```

### 3.2 Roundedness

Termeric avoids sharp corners. Things should look **knobby and organic**, like an actual root.

| Element              | Radius / Treatment                                                         |
|----------------------|-----------------------------------------------------------------------------|
| **Window corners**   | Use OS-native rounded corners (platform default or custom if available)     |
| **Tab bar**          | Active tab is a rounded raised surface with a thin accent underline         |
| **Settings panel**   | Rounded selection highlights, soft accent bars                              |
| **Cursor**           | Block cursor has 2px corner radius; beam cursor has rounded caps            |
| **Scrollbar**        | Thin, rounded, semi-transparent — appears on hover, fades away             |
| **Dialogs/overlays** | Rounded corners with subtle border, never sharp 90° boxes                  |
| **Buttons**          | Full pill-shape (`border-radius: 50%` of height) or generous rounding      |

**General rule**: Default corner radius = `4px` for small elements, `8px` for panels, `12px` for overlays. When in doubt, round it more.

### 3.3 Earthiness

The UI should feel **warm and grounded**, not sterile or clinical.

- **Backgrounds**: Prefer warm-tinted darks (brown-black, not blue-black or grey-black)
- **Text**: Cream and tan tones for foreground, not pure `#FFFFFF`
- **Accents**: Gold, amber, orange, leaf-green — never neon or electric
- **Shadows**: Warm-toned, not pure black. Use `rgba(28, 20, 16, 0.4)` — the turmeric black
- **Separators**: Thin, low-contrast, almost invisible. They suggest boundaries, not enforce them

### 3.4 Subtlety

Less is more. The terminal content is king; the UI is the soil.

- **Animations**: Short (150–250ms). Never bouncy or springy — use ease-out only
- **Hover effects**: Gentle background tint change, not color explosion
- **Notifications**: Minimal, non-intrusive. Prefer status text over modals
- **Settings**: One centered dialog with a sidebar of pages and a single search across every page. Quiet surfaces and the UI face; values sit in one column with steppers, dropdown chevrons, or a trailing chevron for actions. No key-hint strips: the footer only states whether changes are saved
- **Tab bar**: Compact, understated, "quiet native". The strip uses the theme's `tab_inactive` color; the active tab is a slightly raised surface with a 2px `tab_active` accent line (a left bar in the vertical rail); inactive titles are dimmed; × appears on the active or hovered tab, and background activity shows as a small dot in its place. Never fill a whole tab with a saturated color

---

## 4. Typography

- **Terminal font**: Respect the user's configured monospace font. Default to system `monospace`. `font.size` is in points (96/72 logical pixels per point)
- **UI text** (tab titles, toolbar labels): The desktop's sans-serif UI face at 10pt, resolved through fontconfig, so chrome reads like the rest of the desktop. Draw it with `Renderer::push_ui_label`, which measures proportionally, centres on the face's own metrics, and shortens with an ellipsis. The tab bar uses it today; settings, palette, and panel text still use the terminal font and should move to the UI face when they are redesigned
- **Font scaling**: Minimum 8pt, maximum 72pt. Step by 1pt
- **Character spacing**: Terminal text keeps the standard monospace grid. No custom letter-spacing or squeezed advances anywhere
- **Emoji**: Rendered inline if the font supports it. Fall back gracefully

---

## 5. Settings Panel Design

The settings panel is a **slide-in side panel** from the right edge. It has:

1. **Sidebar** — Section tabs with icons and labels, vertical accent bar for active section
2. **Content area** — Section header, separator, items with `◀ value ▶` controls
3. **Footer** — Save/Reset keyboard hints

### Layout constants

| Property          | Value                                |
|-------------------|--------------------------------------|
| Panel width       | 45% of window, clamped 360–520px     |
| Sidebar width     | 14× cell width, clamped 120–160px    |
| Row height        | 2.2× cell height, min 32px           |
| Padding X         | 1.5× cell width                      |
| Animation         | 200ms ease-out slide-in from right   |
| Scrim             | `rgba(0,0,0,0.25)` over terminal     |

### Interaction modes

| Input   | Action                               |
|---------|--------------------------------------|
| ↑↓      | Navigate between items (wraps across sections) |
| ←→      | Adjust selected value                |
| Tab     | Cycle to next section                |
| Escape  | Close panel                          |
| Ctrl+S  | Save config to disk                  |
| Ctrl+R  | Reset to defaults                    |
| Mouse   | Click section tabs, item arrows, or outside to close |

---

## 6. Code Conventions

### Theme implementation

Every root theme is a `pub fn theme_name() -> Self` method on `Theme`. Follow this pattern:

```rust
/// Malanga theme
pub fn malanga() -> Self {
    Self {
        meta: ThemeMeta {
            name: "Malanga".into(),
            author: "Termeric".into(),
            description: "Grey-brown skin with purple splotches — tropical root vibes".into(),
        },
        primary: PrimaryColors {
            black: Color::hex("#2D2420"),   // Dark root bark
            // ... derive every color from the actual plant
        },
        // ...
    }
}
```

**Rules**:
- Every color **must** have a comment explaining what part of the root/plant it came from
- `author` for root themes is always `"Termeric"`
- `description` should mention the root's visual characteristics
- Add a match arm in `load_builtin()` for the new theme name
- Add the name to `BUILTIN_THEMES` in `settings.rs`

### Animation implementation

- Use `std::time::Instant` for all animation timing
- Store the trigger `Instant`, compute progress as `elapsed / DURATION`
- Apply easing: `ease_out(t) = 1.0 - (1.0 - t) * (1.0 - t)`
- Clamp to `[0.0, 1.0]` before applying
- Duration constant: `const ANIMATION_DURATION: f32 = 0.2;` (200ms default)

### Rectangle drawing

`RectPipeline::push_rounded_rect` draws anti-aliased rounded rectangles, and a radius of half the height makes a pill or circle. Use these radii, scaled by the display factor and rounded to whole pixels:

```rust
const CORNER_RADIUS_SM: f32 = 4.0;   // buttons, selection highlights
const CORNER_RADIUS_MD: f32 = 8.0;   // panels, cards
const CORNER_RADIUS_LG: f32 = 12.0;  // overlays, dialogs
```

Chrome geometry must land on whole physical pixels, and rendering, hit testing, keyboard focus, and accessibility bounds must come from one shared function (see `TabChrome` in `renderer/tab_bar.rs`).

---

## 7. Quick Reference: Root vs. Guest Themes

| Category     | Examples                                       | Author field  | Naming rule        |
|--------------|------------------------------------------------|---------------|--------------------|
| **Root**     | turmeric, ginger, daikon, malanga, beet, taro  | `"Termeric"`  | Lowercase root name|
| **Guest**    | solarized-dark, catppuccin-mocha, tokyo-night  | Original author| Original name      |

Root themes are the **soul** of Termeric. Guest themes are welcome visitors. When in doubt about a design decision, ask: *"Would this feel like holding a warm root in your hands?"*
