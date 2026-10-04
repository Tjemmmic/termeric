//! The agent view's colors, derived from the theme.
//!
//! Surfaces step from the terminal background toward the foreground, so light
//! and dark themes both work, and status colors use the terminal's own
//! semantic hues: yellow asks for attention, green succeeded, red failed.

use crate::config::theme::Theme;
use crate::ui::color::{high_contrast_text, luminance, mix, with_alpha};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colors {
    pub panel: [f32; 4],
    /// Cards and the person's messages.
    pub surface: [f32; 4],
    /// Hover and pressed surfaces.
    pub surface_hover: [f32; 4],
    pub border: [f32; 4],
    /// Headings and the person's own words.
    pub text_strong: [f32; 4],
    pub text: [f32; 4],
    pub text_secondary: [f32; 4],
    pub text_muted: [f32; 4],
    pub accent: [f32; 4],
    /// Text on an accent fill.
    pub on_accent: [f32; 4],
    pub attention: [f32; 4],
    pub attention_surface: [f32; 4],
    pub attention_border: [f32; 4],
    pub success: [f32; 4],
    pub danger: [f32; 4],
    /// The supervisor's voice.
    pub supervisor: [f32; 4],
    pub supervisor_surface: [f32; 4],
    pub code_surface: [f32; 4],
    /// A recessed well for commands and output inside cards: the terminal's
    /// own background.
    pub well: [f32; 4],
    pub code_text: [f32; 4],
    pub link: [f32; 4],
    pub added_surface: [f32; 4],
    pub added_text: [f32; 4],
    pub removed_surface: [f32; 4],
    pub removed_text: [f32; 4],
    pub gutter_text: [f32; 4],
    pub focus_ring: [f32; 4],
    pub selection: [f32; 4],
    pub scrim: [f32; 4],
}

impl Colors {
    pub fn from_theme(theme: &Theme, high_contrast: bool) -> Self {
        let bg = with_alpha(theme.ui.background.to_f32_array(), 1.0);
        let fg = with_alpha(theme.ui.foreground.to_f32_array(), 1.0);
        let strong = with_alpha(theme.bright.white.to_f32_array(), 1.0);
        let accent = with_alpha(theme.ui.tab_active.to_f32_array(), 1.0);
        let yellow = with_alpha(theme.bright.yellow.to_f32_array(), 1.0);
        let green = with_alpha(theme.bright.green.to_f32_array(), 1.0);
        let red = with_alpha(theme.bright.red.to_f32_array(), 1.0);
        let cyan = with_alpha(theme.bright.cyan.to_f32_array(), 1.0);
        // Text on the accent must read: dark on a light accent, light on a
        // dark one.
        let on_accent = if luminance(accent) > 0.45 {
            if luminance(bg) < luminance(fg) {
                bg
            } else {
                fg
            }
        } else if luminance(bg) > luminance(fg) {
            bg
        } else {
            fg
        };
        let mut colors = Self {
            panel: mix(bg, fg, 0.035),
            surface: mix(bg, fg, 0.075),
            surface_hover: mix(bg, fg, 0.13),
            border: mix(bg, fg, 0.16),
            text_strong: strong,
            text: mix(fg, strong, 0.35),
            text_secondary: with_alpha(fg, 0.78),
            text_muted: with_alpha(fg, 0.6),
            accent,
            on_accent,
            attention: yellow,
            attention_surface: mix(mix(bg, fg, 0.035), yellow, 0.07),
            attention_border: mix(bg, yellow, 0.42),
            success: green,
            danger: red,
            supervisor: cyan,
            supervisor_surface: mix(mix(bg, fg, 0.035), cyan, 0.09),
            code_surface: mix(bg, fg, 0.09),
            well: bg,
            code_text: mix(fg, yellow, 0.25),
            link: with_alpha(theme.ui.url.to_f32_array(), 1.0),
            added_surface: with_alpha(green, 0.14),
            added_text: mix(fg, green, 0.45),
            removed_surface: with_alpha(red, 0.14),
            removed_text: mix(fg, red, 0.45),
            gutter_text: with_alpha(fg, 0.42),
            focus_ring: accent,
            selection: theme.ui.selection.to_f32_array(),
            scrim: with_alpha(bg, 0.55),
        };
        if high_contrast {
            colors.enforce_high_contrast(bg);
        }
        colors
    }

    /// Flatten every surface to the background and draw all text, borders,
    /// and focus in the chrome's high-contrast text color.
    fn enforce_high_contrast(&mut self, bg: [f32; 4]) {
        let text = high_contrast_text(bg);
        self.panel = bg;
        self.surface = bg;
        self.code_surface = bg;
        self.well = bg;
        self.attention_surface = bg;
        self.supervisor_surface = bg;
        self.text_strong = text;
        self.text = text;
        self.text_secondary = text;
        self.text_muted = text;
        self.gutter_text = text;
        self.code_text = text;
        self.border = text;
        self.attention_border = text;
        self.added_text = text;
        self.removed_text = text;
        self.focus_ring = text;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surfaces_step_toward_the_foreground_in_order() {
        let theme = Theme::default();
        let colors = Colors::from_theme(&theme, false);
        let bg = luminance(theme.ui.background.to_f32_array());
        let fg = luminance(theme.ui.foreground.to_f32_array());
        let distance = |color: [f32; 4]| (luminance(color) - bg).abs();
        assert!(distance(colors.panel) < distance(colors.surface));
        assert!(distance(colors.surface) < distance(colors.surface_hover));
        assert!(distance(colors.surface_hover) < distance(colors.border));
        assert!((fg - bg).abs() > distance(colors.border));
    }

    #[test]
    fn high_contrast_text_reaches_aa_on_every_theme() {
        for name in Theme::list_available() {
            let theme = Theme::load_builtin(name).unwrap();
            let colors = Colors::from_theme(&theme, true);
            for color in [
                colors.text,
                colors.text_secondary,
                colors.text_muted,
                colors.border,
                colors.focus_ring,
            ] {
                assert_eq!(color[3], 1.0);
                assert!(
                    crate::ui::color::contrast_ratio(color, colors.panel) >= 4.5,
                    "{name}"
                );
            }
        }
    }
}
