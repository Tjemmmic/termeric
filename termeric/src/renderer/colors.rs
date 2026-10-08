//! Theme-derived chrome and terminal palettes, the high-contrast pass, and
//! the encoded-color surface formats frames render into.

use super::*;
use crate::ui::color::high_contrast_text_on;

#[derive(Clone)]
pub(crate) struct UiPalette {
    pub(crate) background: wgpu::Color,
    /// The terminal's foreground, for chrome drawn over terminal content:
    /// pane scrollbars and the visual bell.
    pub(crate) foreground: [f32; 4],
    pub(crate) selection: [f32; 4],
    pub(crate) selection_flash: [f32; 4],
    pub(crate) selection_text: Option<[f32; 4]>,
    pub(crate) cursor: [f32; 4],
    pub(crate) cursor_text: [f32; 4],
    /// Chrome strip behind the tabs, a step off the terminal background.
    pub(crate) tab_bar_bg: [f32; 4],
    /// Raised surface of the active tab.
    pub(crate) tab_active: [f32; 4],
    /// Hover surface of inactive tabs and toolbar buttons.
    pub(crate) tab_inactive: [f32; 4],
    /// Theme accent for the active-tab indicator.
    pub(crate) tab_accent: [f32; 4],
    // Popup menus
    pub(crate) menu_bg: [f32; 4],
    pub(crate) menu_border: [f32; 4],
    pub(crate) menu_highlight: [f32; 4],
    pub(crate) menu_text: [f32; 4],
    pub(crate) menu_text_dim: [f32; 4],
    /// Status hues from the terminal's own palette (yellow, red, green,
    /// cyan), for agent badges, tab activity, toasts, diffs, warnings, and
    /// pane outlines.
    pub(crate) warning: [f32; 4],
    pub(crate) error: [f32; 4],
    pub(crate) success: [f32; 4],
    pub(crate) info: [f32; 4],
    pub(crate) tab_border: [f32; 4],
    pub(crate) tab_text_active: [f32; 4],
    pub(crate) tab_text_inactive: [f32; 4],
    pub(crate) scrim: [f32; 4],
    // Search colors
    pub(crate) search_match: [f32; 4],
    pub(crate) search_match_current: [f32; 4],
    pub(crate) search_error: [f32; 4],
    // URL/hyperlink color
    pub(crate) url: [f32; 4],
    // Command palette colors
    pub(crate) palette_scrim: [f32; 4],
    pub(crate) palette_bg: [f32; 4],
    pub(crate) palette_input_bg: [f32; 4],
    pub(crate) palette_accent: [f32; 4],
    pub(crate) palette_text: [f32; 4],
    pub(crate) palette_text_dim: [f32; 4],
    pub(crate) palette_match_highlight: [f32; 4],
    pub(crate) palette_border: [f32; 4],
}

impl UiPalette {
    pub(super) fn from_theme_with_contrast(theme: &Theme, high_contrast: bool) -> Self {
        let bg = theme.ui.background;
        let fg = theme.ui.foreground;
        let selection = theme.ui.selection;
        let cursor = theme.ui.cursor;
        let tab_active = theme.ui.tab_active;
        let tab_inactive = theme.ui.tab_inactive;
        let tab_border = theme.ui.border;

        // Quiet chrome: the theme's inactive-tab color is the strip, raised and
        // hover surfaces step from it toward the foreground (so light and dark
        // themes both work), and the theme's active-tab color is reserved for
        // the thin active indicator.
        let strip = with_alpha(tab_inactive.to_f32_array(), 1.0);
        let chrome = |amount: f32| mix(strip, fg.to_f32_array(), amount);
        let tab_text_active = fg.to_f32_array();
        let tab_text_inactive = with_alpha(fg.to_f32_array(), 0.62);
        let mut palette = Self {
            background: bg.to_wgpu(),
            foreground: fg.to_f32_array(),
            selection: selection.to_f32_array(),
            selection_flash: with_alpha(fg.to_f32_array(), 0.85),
            selection_text: theme.ui.selection_text.map(|c| c.to_f32_array()),
            cursor: cursor.to_f32_array(),
            cursor_text: theme.ui.cursor_text.to_f32_array(),
            tab_bar_bg: strip,
            tab_active: chrome(0.08),
            tab_inactive: chrome(0.05),
            tab_accent: tab_active.to_f32_array(),
            menu_bg: chrome(0.03),
            menu_border: chrome(0.14),
            menu_highlight: chrome(0.11),
            menu_text: fg.to_f32_array(),
            menu_text_dim: with_alpha(fg.to_f32_array(), 0.55),
            warning: theme.primary.yellow.to_f32_array(),
            error: theme.primary.red.to_f32_array(),
            success: theme.primary.green.to_f32_array(),
            info: theme.primary.cyan.to_f32_array(),
            tab_border: tab_border.to_f32_array(),
            tab_text_active,
            tab_text_inactive,
            // Warm scrim derived from theme background
            scrim: with_alpha(scale_rgb(bg.to_f32_array(), 0.3), 0.30),
            // Search highlights and the find box's error color
            search_match: theme.ui.search_match.with_alpha(0.35).to_f32_array(),
            search_match_current: theme.ui.search_match.with_alpha(0.65).to_f32_array(),
            search_error: theme.ansi_color(1).to_f32_array(), // red
            // URL/hyperlink color from theme
            url: theme.ui.url.to_f32_array(),
            // Command palette. Its scrim is plain black at low alpha, like
            // every shadow, so it dims light and dark themes alike.
            palette_scrim: [0.0, 0.0, 0.0, 0.3],
            palette_bg: scale_rgb(bg.to_f32_array(), 0.65),
            palette_input_bg: scale_rgb(bg.to_f32_array(), 0.50),
            palette_accent: cursor.to_f32_array(),
            palette_text: fg.to_f32_array(),
            palette_text_dim: with_alpha(fg.to_f32_array(), 0.50),
            palette_match_highlight: cursor.to_f32_array(),
            palette_border: with_alpha(tab_border.to_f32_array(), 0.5),
        };
        if high_contrast {
            palette.enforce_high_contrast();
        }
        palette
    }

    fn enforce_high_contrast(&mut self) {
        let background = [
            self.background.r as f32,
            self.background.g as f32,
            self.background.b as f32,
            1.0,
        ];
        self.foreground = high_contrast_text(background);

        self.tab_bar_bg[3] = 1.0;
        self.tab_active[3] = 1.0;
        self.tab_inactive[3] = 1.0;
        self.tab_text_active = high_contrast_text(self.tab_active);
        self.tab_text_inactive = high_contrast_text(self.tab_bar_bg);
        self.tab_accent = self.tab_text_active;

        self.menu_bg[3] = 1.0;
        self.menu_highlight[3] = 1.0;
        self.menu_text = high_contrast_text(self.menu_bg);
        self.menu_text_dim = self.menu_text;
        self.menu_border = self.menu_text;
        self.tab_border = high_contrast_text(self.tab_bar_bg);

        // Errors draw on menu surfaces (find box, manager buttons).
        self.search_error = self.menu_text;

        self.palette_bg[3] = 1.0;
        self.palette_input_bg[3] = 1.0;
        self.palette_input_bg = self.palette_bg;
        // Palette text reads on cards and query fields (palette_bg) and, in
        // the shared modal frame, on the frame (menu_bg) and its selected row
        // (menu_highlight).
        let palette_text =
            high_contrast_text_on(&[self.palette_bg, self.menu_bg, self.menu_highlight]);
        self.palette_text = palette_text;
        self.palette_text_dim = palette_text;
        self.palette_border = palette_text;
        self.palette_accent = palette_text;
        // Matched characters keep their own color only while it stays
        // readable on the rows they are drawn in, selected or not, so a
        // search still shows what it matched.
        if [self.menu_bg, self.menu_highlight]
            .into_iter()
            .any(|row| contrast_ratio(self.palette_match_highlight, row) < 4.5)
        {
            self.palette_match_highlight = palette_text;
        }
        self.scrim[3] = self.scrim[3].max(0.65);
        self.palette_scrim[3] = self.palette_scrim[3].max(0.72);

        // Status colors mean something by their hue (a warning is yellow, a
        // failure red), so rather than turn them black or white they lighten
        // or darken just enough to read on every surface they are drawn on:
        // the terminal (pane outlines, growth indicators), the tab strip and
        // tabs (activity and agent dots), menus and side panels (agent rows),
        // and cards (toasts, diff review).
        let surfaces = [
            background,
            self.tab_bar_bg,
            self.tab_active,
            self.tab_inactive,
            self.menu_bg,
            self.menu_highlight,
            self.palette_bg,
        ];
        for status in [
            &mut self.warning,
            &mut self.error,
            &mut self.success,
            &mut self.info,
        ] {
            *status = readable_on(*status, &surfaces);
        }
    }
}

/// `color`, mixed toward black or white (whichever high-contrast text uses
/// on `surfaces`) by the least step that gives it 4.5:1 on all of them, so
/// it keeps as much of its hue as legibility allows. The mix is not
/// monotonic in contrast when `color` sits between surfaces, so it is
/// searched in order rather than bisected; the far end is the high-contrast
/// text color itself.
fn readable_on(color: [f32; 4], surfaces: &[[f32; 4]]) -> [f32; 4] {
    const STEPS: u16 = 64;
    let target = high_contrast_text_on(surfaces);
    let readable = |candidate: [f32; 4]| {
        surfaces
            .iter()
            .all(|surface| contrast_ratio(candidate, *surface) >= 4.5)
    };
    (0..=STEPS)
        .map(|step| with_alpha(mix(color, target, f32::from(step) / f32::from(STEPS)), 1.0))
        .find(|candidate| readable(*candidate))
        .unwrap_or(target)
}

/// Termeric composes in display-encoded sRGB space, as Qt, GTK and browsers
/// do: theme hex values reach the screen unchanged and alpha blends operate on
/// encoded values. Render targets therefore use non-sRGB views, so the GPU
/// never encodes those values a second time, and textures that store
/// sRGB-encoded pixels (images, color glyphs) use this format so sampling
/// returns the stored values instead of decoding them to linear light.
pub(crate) const ENCODED_RGBA_FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;

/// The presentation format a surface is configured with and the non-sRGB
/// view format that frames are rendered into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SurfaceFormats {
    pub(super) surface: TextureFormat,
    pub(super) render: TextureFormat,
}

/// Prefer 8-bit formats because wider or floating-point presentation formats
/// can carry different transfer semantics. An sRGB-only surface is still
/// usable: frames render through a non-sRGB view of the same texture.
pub(super) fn select_surface_formats(available: &[TextureFormat]) -> Option<SurfaceFormats> {
    const PREFERRED: [TextureFormat; 4] = [
        TextureFormat::Bgra8Unorm,
        TextureFormat::Rgba8Unorm,
        TextureFormat::Bgra8UnormSrgb,
        TextureFormat::Rgba8UnormSrgb,
    ];
    let surface = PREFERRED
        .into_iter()
        .find(|format| available.contains(format))
        .or_else(|| available.iter().copied().find(|format| !format.is_srgb()))
        .or_else(|| available.first().copied())?;
    Some(SurfaceFormats {
        surface,
        render: surface.remove_srgb_suffix(),
    })
}

#[cfg(test)]
mod surface_format_tests {
    use super::*;

    #[test]
    fn prefers_a_non_srgb_eight_bit_surface_even_when_srgb_is_listed_first() {
        let formats = select_surface_formats(&[
            TextureFormat::Bgra8UnormSrgb,
            TextureFormat::Rgba16Float,
            TextureFormat::Bgra8Unorm,
        ]);
        assert_eq!(
            formats,
            Some(SurfaceFormats {
                surface: TextureFormat::Bgra8Unorm,
                render: TextureFormat::Bgra8Unorm,
            })
        );
    }

    #[test]
    fn srgb_only_surfaces_render_through_a_non_srgb_view() {
        let formats = select_surface_formats(&[TextureFormat::Rgba8UnormSrgb]);
        assert_eq!(
            formats,
            Some(SurfaceFormats {
                surface: TextureFormat::Rgba8UnormSrgb,
                render: TextureFormat::Rgba8Unorm,
            })
        );
    }

    #[test]
    fn unlisted_formats_fall_back_without_ever_rendering_through_srgb() {
        let wide = select_surface_formats(&[TextureFormat::Rgb10a2Unorm]).unwrap();
        assert_eq!(wide.render, TextureFormat::Rgb10a2Unorm);
        for available in [
            vec![TextureFormat::Bgra8UnormSrgb],
            vec![TextureFormat::Rgba8UnormSrgb, TextureFormat::Bgra8UnormSrgb],
            vec![TextureFormat::Rgb10a2Unorm, TextureFormat::Rgba8UnormSrgb],
        ] {
            let formats = select_surface_formats(&available).unwrap();
            assert!(!formats.render.is_srgb(), "{available:?} -> {formats:?}");
        }
        assert_eq!(select_surface_formats(&[]), None);
    }
}

#[derive(Clone)]
pub(super) struct TerminalPalette {
    pub(super) default_fg: TermColor,
    pub(super) default_bg: TermColor,
    pub(super) ansi: [TermColor; 16],
}

impl TerminalPalette {
    pub(super) fn from_theme(theme: &Theme) -> Self {
        let convert =
            |color: crate::config::theme::Color| TermColor::rgb(color.r, color.g, color.b);
        Self {
            default_fg: convert(theme.ui.foreground),
            default_bg: convert(theme.ui.background),
            ansi: std::array::from_fn(|index| convert(theme.ansi_color(index as u8))),
        }
    }

    pub(super) fn resolve(
        &self,
        dynamic: &termeric_core::DynamicColors,
        spec: ColorSpec,
        resolved: TermColor,
        default: TermColor,
    ) -> TermColor {
        match spec {
            ColorSpec::Default => default,
            ColorSpec::Indexed(index) => self.indexed(dynamic, index, resolved),
            ColorSpec::TrueColor(_) => resolved,
        }
    }

    /// Indexed color `index`: the terminal's own override when it set one,
    /// else the theme's ANSI color for the first sixteen, else `extended`,
    /// the 256-color value the caller already has for it.
    pub(super) fn indexed(
        &self,
        dynamic: &termeric_core::DynamicColors,
        index: u8,
        extended: TermColor,
    ) -> TermColor {
        dynamic.palette(index).unwrap_or_else(|| {
            if index < 16 {
                self.ansi[index as usize]
            } else {
                extended
            }
        })
    }

    pub(super) fn default_fg(&self, dynamic: &termeric_core::DynamicColors) -> TermColor {
        dynamic.foreground().unwrap_or(self.default_fg)
    }

    pub(super) fn default_bg(&self, dynamic: &termeric_core::DynamicColors) -> TermColor {
        dynamic.background().unwrap_or(self.default_bg)
    }
}

#[cfg(test)]
mod terminal_palette_tests {
    use super::*;

    fn term_color(color: crate::config::theme::Color) -> TermColor {
        TermColor::rgb(color.r, color.g, color.b)
    }

    /// A cell's foreground and background as the renderer paints them.
    fn resolve_cell(
        palette: &TerminalPalette,
        dynamic: &termeric_core::DynamicColors,
        cell: &termeric_core::Cell,
    ) -> (TermColor, TermColor) {
        let style = super::super::style::ResolvedCellStyle::resolve(
            palette,
            dynamic,
            cell,
            false,
            TextBlinkPhases::default(),
        );
        (style.foreground, style.background)
    }

    #[test]
    fn terminal_palette_resolves_theme_defaults_and_indexed_colors() {
        let theme = Theme::load_builtin("nord").unwrap();
        let palette = TerminalPalette::from_theme(&theme);
        let dynamic = termeric_core::DynamicColors::default();
        let mut cell = termeric_core::Cell::with_char('x');

        assert_eq!(
            resolve_cell(&palette, &dynamic, &cell),
            (
                term_color(theme.ui.foreground),
                term_color(theme.ui.background)
            )
        );

        cell.fg_spec = ColorSpec::Indexed(1);
        cell.fg = TermColor::rgb(205, 0, 0);
        assert_eq!(
            resolve_cell(&palette, &dynamic, &cell).0,
            term_color(theme.primary.red)
        );

        cell.attrs.insert(termeric_core::CellAttrs::INVERSE);
        assert_eq!(
            resolve_cell(&palette, &dynamic, &cell),
            (
                term_color(theme.ui.background),
                term_color(theme.primary.red)
            )
        );
    }

    #[test]
    fn terminal_local_dynamic_colors_override_and_reset_theme_resolution() {
        let theme = Theme::load_builtin("nord").unwrap();
        let palette = TerminalPalette::from_theme(&theme);
        let mut terminal = termeric_core::Terminal::new(4, 2);
        let override_color = TermColor::rgb(1, 2, 3);
        terminal.apply_dynamic_color(termeric_core::DynamicColorOperation::Set {
            target: termeric_core::DynamicColorTarget::Palette(1),
            color: override_color,
        });
        let mut cell = termeric_core::Cell::with_char('x');
        cell.fg_spec = ColorSpec::Indexed(1);
        assert_eq!(
            resolve_cell(&palette, terminal.dynamic_colors(), &cell).0,
            override_color
        );
        terminal.apply_dynamic_color(termeric_core::DynamicColorOperation::Reset(
            termeric_core::DynamicColorTarget::Palette(1),
        ));
        assert_eq!(
            resolve_cell(&palette, terminal.dynamic_colors(), &cell).0,
            term_color(theme.primary.red)
        );
    }
}

#[cfg(test)]
mod ui_palette_accessibility_tests {
    use super::*;

    #[test]
    fn high_contrast_chrome_meets_aa_text_contrast() {
        for theme_name in Theme::list_available() {
            let theme = Theme::load_builtin(theme_name).unwrap();
            let palette = UiPalette::from_theme_with_contrast(&theme, true);
            let terminal_background = [
                palette.background.r as f32,
                palette.background.g as f32,
                palette.background.b as f32,
                1.0,
            ];
            for (foreground, background) in [
                (palette.foreground, terminal_background),
                (palette.tab_text_active, palette.tab_active),
                (palette.tab_text_inactive, palette.tab_bar_bg),
                (palette.tab_text_inactive, palette.tab_inactive),
                (palette.menu_text, palette.menu_bg),
                (palette.menu_text, palette.menu_highlight),
                (palette.menu_text_dim, palette.menu_bg),
                (palette.search_error, palette.menu_bg),
                (palette.palette_text, palette.menu_bg),
                (palette.palette_text, palette.menu_highlight),
                (palette.palette_text, palette.palette_bg),
                (palette.palette_text, palette.palette_input_bg),
                (palette.palette_text_dim, palette.palette_bg),
                (palette.palette_text_dim, palette.palette_input_bg),
                // Command palette matches, on plain and selected rows.
                (palette.palette_match_highlight, palette.menu_bg),
                (palette.palette_match_highlight, palette.menu_highlight),
            ]
            .into_iter()
            .chain(status_on_surfaces(&palette, terminal_background))
            {
                assert!(
                    contrast_ratio(foreground, background) >= 4.5,
                    "{theme_name} contrast was {}",
                    contrast_ratio(foreground, background)
                );
            }
        }
    }

    /// Every status color against every surface it is drawn on.
    fn status_on_surfaces(
        palette: &UiPalette,
        terminal_background: [f32; 4],
    ) -> Vec<([f32; 4], [f32; 4])> {
        let surfaces = [
            terminal_background,
            palette.tab_bar_bg,
            palette.tab_active,
            palette.tab_inactive,
            palette.menu_bg,
            palette.menu_highlight,
            palette.palette_bg,
        ];
        [
            palette.warning,
            palette.error,
            palette.success,
            palette.info,
        ]
        .into_iter()
        .flat_map(|status| surfaces.map(|surface| (status, surface)))
        .collect()
    }

    #[test]
    fn high_contrast_status_colors_keep_their_hue() {
        let hue = |color: [f32; 4]| {
            let [red, green, blue, _] = color;
            let max = red.max(green).max(blue);
            let min = red.min(green).min(blue);
            let chroma = max - min;
            let degrees = if max == red {
                60.0 * ((green - blue) / chroma).rem_euclid(6.0)
            } else if max == green {
                60.0 * ((blue - red) / chroma + 2.0)
            } else {
                60.0 * ((red - green) / chroma + 4.0)
            };
            (degrees, chroma)
        };
        for theme_name in Theme::list_available() {
            let theme = Theme::load_builtin(theme_name).unwrap();
            let plain = UiPalette::from_theme_with_contrast(&theme, false);
            let contrast = UiPalette::from_theme_with_contrast(&theme, true);
            for (before, after) in [
                (plain.warning, contrast.warning),
                (plain.error, contrast.error),
                (plain.success, contrast.success),
                (plain.info, contrast.info),
            ] {
                let (before_hue, _) = hue(before);
                let (after_hue, after_chroma) = hue(after);
                let distance = (before_hue - after_hue).abs();
                assert!(
                    after_chroma > 0.05 && distance.min(360.0 - distance) < 1.0,
                    "{theme_name}: {before:?} became {after:?}"
                );
            }
        }
    }

    #[test]
    fn high_contrast_policy_does_not_change_terminal_palette() {
        let theme = Theme::load_builtin("turmeric").unwrap();
        let before = TerminalPalette::from_theme(&theme);
        let _chrome = UiPalette::from_theme_with_contrast(&theme, true);
        let after = TerminalPalette::from_theme(&theme);
        assert_eq!(before.default_fg, after.default_fg);
        assert_eq!(before.default_bg, after.default_bg);
        assert_eq!(before.ansi, after.ansi);
    }
}
