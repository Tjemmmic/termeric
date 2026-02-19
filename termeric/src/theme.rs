//! Theme management for Termeric
//!
//! Handles loading, parsing, and applying color themes.

use anyhow::{Result, anyhow};
use serde::Deserialize;
use std::path::Path;

/// A complete color theme
#[derive(Debug, Clone, Deserialize)]
pub struct Theme {
    /// Theme metadata
    pub meta: ThemeMeta,
    /// Primary colors (ANSI 0-7)
    pub primary: PrimaryColors,
    /// Bright colors (ANSI 8-15)
    pub bright: BrightColors,
    /// UI colors
    pub ui: UiColors,
}

impl Default for Theme {
    fn default() -> Self {
        // Hardcoded turmeric default — avoids recursion via load_builtin
        Self {
            meta: ThemeMeta::default(),
            primary: PrimaryColors::default(),
            bright: BrightColors::default(),
            ui: UiColors::default(),
        }
    }
}

impl Theme {
    /// Load theme from a TOML file
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let theme: Theme = toml::from_str(&content)?;
        Ok(theme)
    }

    /// Load a built-in theme by name.
    /// Themes are embedded as TOML files at compile time.
    pub fn load_builtin(name: &str) -> Result<Self> {
        let toml_str = match name.to_lowercase().as_str() {
            // ── Root themes (Termeric house palettes) ──
            "turmeric" | "default" => include_str!("../themes/roots/turmeric.toml"),
            "ginger"              => include_str!("../themes/roots/ginger.toml"),
            "daikon"              => include_str!("../themes/roots/daikon.toml"),
            "malanga"             => include_str!("../themes/roots/malanga.toml"),
            "beet"                => include_str!("../themes/roots/beet.toml"),
            "turnip"              => include_str!("../themes/roots/turnip.toml"),
            "yuca"                => include_str!("../themes/roots/yuca.toml"),
            "taro"                => include_str!("../themes/roots/taro.toml"),
            "wasabi"              => include_str!("../themes/roots/wasabi.toml"),
            "galangal"            => include_str!("../themes/roots/galangal.toml"),
            "horseradish"         => include_str!("../themes/roots/horseradish.toml"),
            "parsnip"             => include_str!("../themes/roots/parsnip.toml"),
            // ── Community themes ──
            "solarized-dark"   => include_str!("../themes/community/solarized-dark.toml"),
            "solarized-light"  => include_str!("../themes/community/solarized-light.toml"),
            "dracula"          => include_str!("../themes/community/dracula.toml"),
            "nord"             => include_str!("../themes/community/nord.toml"),
            "gruvbox-dark"     => include_str!("../themes/community/gruvbox-dark.toml"),
            "gruvbox-light"    => include_str!("../themes/community/gruvbox-light.toml"),
            "catppuccin-mocha" => include_str!("../themes/community/catppuccin-mocha.toml"),
            "catppuccin-latte" => include_str!("../themes/community/catppuccin-latte.toml"),
            "tokyo-night"      => include_str!("../themes/community/tokyo-night.toml"),
            "one-dark"         => include_str!("../themes/community/one-dark.toml"),
            "one-light"        => include_str!("../themes/community/one-light.toml"),
            "rose-pine"        => include_str!("../themes/community/rose-pine.toml"),
            "kanagawa"         => include_str!("../themes/community/kanagawa.toml"),
            "everforest"       => include_str!("../themes/community/everforest.toml"),
            _ => return Err(anyhow!("Unknown built-in theme: {}", name)),
        };
        let theme: Theme = toml::from_str(toml_str)?;
        Ok(theme)
    }

    /// List all available built-in theme names.
    pub fn list_available() -> &'static [&'static str] {
        &[
            // Root themes
            "turmeric", "ginger", "daikon", "malanga", "beet", "turnip",
            "yuca", "taro", "wasabi", "galangal", "horseradish", "parsnip",
            // Community themes
            "solarized-dark", "solarized-light", "dracula", "nord",
            "gruvbox-dark", "gruvbox-light", "catppuccin-mocha", "catppuccin-latte",
            "tokyo-night", "one-dark", "one-light", "rose-pine", "kanagawa", "everforest",
        ]
    }

    /// Get ANSI color by index (0-15)
    pub fn ansi_color(&self, index: u8) -> Color {
        match index {
            0 => self.primary.black,
            1 => self.primary.red,
            2 => self.primary.green,
            3 => self.primary.yellow,
            4 => self.primary.blue,
            5 => self.primary.magenta,
            6 => self.primary.cyan,
            7 => self.primary.white,
            8 => self.bright.black,
            9 => self.bright.red,
            10 => self.bright.green,
            11 => self.bright.yellow,
            12 => self.bright.blue,
            13 => self.bright.magenta,
            14 => self.bright.cyan,
            15 => self.bright.white,
            _ => self.ui.foreground,
        }
    }
}

/// Theme metadata
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ThemeMeta {
    pub name: String,
    pub author: String,
    #[serde(default)]
    pub description: String,
}

/// Primary ANSI colors (0-7)
#[derive(Debug, Clone, Deserialize)]
pub struct PrimaryColors {
    pub black: Color,
    pub red: Color,
    pub green: Color,
    pub yellow: Color,
    pub blue: Color,
    pub magenta: Color,
    pub cyan: Color,
    pub white: Color,
}

impl Default for PrimaryColors {
    fn default() -> Self {
        Self {
            black:   Color { r: 0x1C, g: 0x14, b: 0x10, a: 1.0 },
            red:     Color { r: 0xC7, g: 0x56, b: 0x46, a: 1.0 },
            green:   Color { r: 0x5D, g: 0x8C, b: 0x4A, a: 1.0 },
            yellow:  Color { r: 0xC9, g: 0xA2, b: 0x27, a: 1.0 },
            blue:    Color { r: 0x6B, g: 0x8E, b: 0x9B, a: 1.0 },
            magenta: Color { r: 0xA6, g: 0x7B, b: 0x8A, a: 1.0 },
            cyan:    Color { r: 0x4A, g: 0x8C, b: 0x7B, a: 1.0 },
            white:   Color { r: 0xD4, g: 0xC4, b: 0xA8, a: 1.0 },
        }
    }
}

/// Bright ANSI colors (8-15)
#[derive(Debug, Clone, Deserialize)]
pub struct BrightColors {
    pub black: Color,
    pub red: Color,
    pub green: Color,
    pub yellow: Color,
    pub blue: Color,
    pub magenta: Color,
    pub cyan: Color,
    pub white: Color,
}

impl Default for BrightColors {
    fn default() -> Self {
        Self {
            black:   Color { r: 0x3D, g: 0x30, b: 0x28, a: 1.0 },
            red:     Color { r: 0xE0, g: 0x7B, b: 0x5F, a: 1.0 },
            green:   Color { r: 0x7C, g: 0xB8, b: 0x6A, a: 1.0 },
            yellow:  Color { r: 0xE8, g: 0xA8, b: 0x38, a: 1.0 },
            blue:    Color { r: 0x8B, g: 0xB0, b: 0xC0, a: 1.0 },
            magenta: Color { r: 0xC8, g: 0x99, b: 0xA8, a: 1.0 },
            cyan:    Color { r: 0x6B, g: 0xB8, b: 0x9E, a: 1.0 },
            white:   Color { r: 0xF5, g: 0xEB, b: 0xD9, a: 1.0 },
        }
    }
}

/// UI-specific colors
#[derive(Debug, Clone, Deserialize)]
pub struct UiColors {
    pub background: Color,
    pub foreground: Color,
    pub cursor: Color,
    pub cursor_text: Color,
    pub selection: Color,
    #[serde(default)]
    pub selection_text: Option<Color>,
    pub border: Color,
    pub tab_active: Color,
    pub tab_inactive: Color,
    pub search_match: Color,
    pub url: Color,
}

impl Default for UiColors {
    fn default() -> Self {
        Self {
            background:     Color { r: 0x1C, g: 0x14, b: 0x10, a: 1.0 },
            foreground:     Color { r: 0xD4, g: 0xC4, b: 0xA8, a: 1.0 },
            cursor:         Color { r: 0xE0, g: 0x7B, b: 0x20, a: 1.0 },
            cursor_text:    Color { r: 0x1C, g: 0x14, b: 0x10, a: 1.0 },
            selection:      Color { r: 0xC9, g: 0xA2, b: 0x27, a: 0.35 },
            selection_text:  None,
            border:         Color { r: 0x3D, g: 0x30, b: 0x28, a: 1.0 },
            tab_active:     Color { r: 0xE0, g: 0x7B, b: 0x20, a: 1.0 },
            tab_inactive:   Color { r: 0x2D, g: 0x24, b: 0x20, a: 1.0 },
            search_match:   Color { r: 0x7C, g: 0xB8, b: 0x6A, a: 0.4 },
            url:            Color { r: 0x6B, g: 0xB8, b: 0x9E, a: 1.0 },
        }
    }
}

/// A color with optional alpha channel
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(from = "ColorDef")]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: f32,
}

impl Color {
    /// Create a color from RGB values
    pub fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    /// Create a color from RGBA values
    pub fn rgba(r: u8, g: u8, b: u8, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// Create a color from a hex string
    pub fn hex(s: &str) -> Self {
        let s = s.trim_start_matches('#');
        let len = s.len();

        if len == 6 {
            let r = u8::from_str_radix(&s[0..2], 16).unwrap_or(0);
            let g = u8::from_str_radix(&s[2..4], 16).unwrap_or(0);
            let b = u8::from_str_radix(&s[4..6], 16).unwrap_or(0);
            Self::rgb(r, g, b)
        } else if len == 8 {
            let r = u8::from_str_radix(&s[0..2], 16).unwrap_or(0);
            let g = u8::from_str_radix(&s[2..4], 16).unwrap_or(0);
            let b = u8::from_str_radix(&s[4..6], 16).unwrap_or(0);
            let a = u8::from_str_radix(&s[6..8], 16).unwrap_or(255);
            Self::rgba(r, g, b, a as f32 / 255.0)
        } else {
            Self::rgb(0, 0, 0)
        }
    }

    /// Return this color with a different alpha value
    pub fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }

    /// Convert to float array for GPU
    pub fn to_f32_array(&self) -> [f32; 4] {
        [
            self.r as f32 / 255.0,
            self.g as f32 / 255.0,
            self.b as f32 / 255.0,
            self.a,
        ]
    }

    /// Convert to wgpu Color
    pub fn to_wgpu(&self) -> wgpu::Color {
        wgpu::Color {
            r: self.r as f64 / 255.0,
            g: self.g as f64 / 255.0,
            b: self.b as f64 / 255.0,
            a: self.a as f64,
        }
    }
}

impl Default for Color {
    fn default() -> Self {
        Self::rgb(255, 255, 255)
    }
}

/// Helper for deserializing colors from various formats
#[derive(Deserialize)]
#[serde(untagged)]
enum ColorDef {
    Hex(String),
    Rgb { r: u8, g: u8, b: u8 },
    Rgba { r: u8, g: u8, b: u8, a: f32 },
}

impl From<ColorDef> for Color {
    fn from(def: ColorDef) -> Self {
        match def {
            ColorDef::Hex(s) => Color::hex(&s),
            ColorDef::Rgb { r, g, b } => Color::rgb(r, g, b),
            ColorDef::Rgba { r, g, b, a } => Color::rgba(r, g, b, a),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hex_color() {
        let c = Color::hex("#FF5500");
        assert_eq!(c.r, 255);
        assert_eq!(c.g, 85);
        assert_eq!(c.b, 0);
    }

    #[test]
    fn test_hex_color_with_alpha() {
        let c = Color::hex("#FF550080");
        assert_eq!(c.r, 255);
        assert_eq!(c.g, 85);
        assert_eq!(c.b, 0);
        assert!((c.a - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_builtin_themes() {
        assert!(Theme::load_builtin("turmeric").is_ok());
        assert!(Theme::load_builtin("dracula").is_ok());
        assert!(Theme::load_builtin("nord").is_ok());
        assert!(Theme::load_builtin("solarized-dark").is_ok());
        assert!(Theme::load_builtin("unknown").is_err());
    }
}
