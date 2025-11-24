//! Theme management for Termeric
//!
//! Handles loading, parsing, and applying color themes.

use anyhow::{Result, anyhow};
use serde::Deserialize;
use std::path::Path;

/// A complete color theme
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
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
        Self::turmeric()
    }
}

impl Theme {
    /// Load theme from a TOML file
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let theme: Theme = toml::from_str(&content)?;
        Ok(theme)
    }

    /// Load a built-in theme by name
    pub fn load_builtin(name: &str) -> Result<Self> {
        match name.to_lowercase().as_str() {
            "turmeric" | "default" => Ok(Self::turmeric()),
            "solarized-dark" => Ok(Self::solarized_dark()),
            "solarized-light" => Ok(Self::solarized_light()),
            "dracula" => Ok(Self::dracula()),
            "nord" => Ok(Self::nord()),
            _ => Err(anyhow!("Unknown built-in theme: {}", name)),
        }
    }

    /// The default Turmeric theme - warm, golden tones
    pub fn turmeric() -> Self {
        Self {
            meta: ThemeMeta {
                name: "Turmeric".to_string(),
                author: "Termeric".to_string(),
                description: "Warm, golden theme inspired by turmeric spice".to_string(),
            },
            primary: PrimaryColors {
                black: Color::hex("#1A1612"),
                red: Color::hex("#C75646"),
                green: Color::hex("#8EB33B"),
                yellow: Color::hex("#D0B03C"),
                blue: Color::hex("#72A0C1"),
                magenta: Color::hex("#C8A0D1"),
                cyan: Color::hex("#218693"),
                white: Color::hex("#E8DCC4"),
            },
            bright: BrightColors {
                black: Color::hex("#3D352D"),
                red: Color::hex("#E09690"),
                green: Color::hex("#CDEE69"),
                yellow: Color::hex("#E8A838"),
                blue: Color::hex("#9CD9F0"),
                magenta: Color::hex("#FBB1F9"),
                cyan: Color::hex("#77DFD8"),
                white: Color::hex("#F7F3E8"),
            },
            ui: UiColors {
                background: Color::hex("#1A1612"),
                foreground: Color::hex("#E8DCC4"),
                cursor: Color::hex("#E8A838"),
                cursor_text: Color::hex("#1A1612"),
                selection: Color::hex("#E8A838").with_alpha(0.3),
                selection_text: None,
                border: Color::hex("#3D352D"),
                tab_active: Color::hex("#E8A838"),
                tab_inactive: Color::hex("#3D352D"),
                search_match: Color::hex("#E8A838").with_alpha(0.5),
                url: Color::hex("#72A0C1"),
            },
        }
    }

    /// Solarized Dark theme
    pub fn solarized_dark() -> Self {
        Self {
            meta: ThemeMeta {
                name: "Solarized Dark".to_string(),
                author: "Ethan Schoonover".to_string(),
                description: "Precision colors for machines and people".to_string(),
            },
            primary: PrimaryColors {
                black: Color::hex("#073642"),
                red: Color::hex("#DC322F"),
                green: Color::hex("#859900"),
                yellow: Color::hex("#B58900"),
                blue: Color::hex("#268BD2"),
                magenta: Color::hex("#D33682"),
                cyan: Color::hex("#2AA198"),
                white: Color::hex("#EEE8D5"),
            },
            bright: BrightColors {
                black: Color::hex("#002B36"),
                red: Color::hex("#CB4B16"),
                green: Color::hex("#586E75"),
                yellow: Color::hex("#657B83"),
                blue: Color::hex("#839496"),
                magenta: Color::hex("#6C71C4"),
                cyan: Color::hex("#93A1A1"),
                white: Color::hex("#FDF6E3"),
            },
            ui: UiColors {
                background: Color::hex("#002B36"),
                foreground: Color::hex("#839496"),
                cursor: Color::hex("#839496"),
                cursor_text: Color::hex("#002B36"),
                selection: Color::hex("#073642"),
                selection_text: None,
                border: Color::hex("#073642"),
                tab_active: Color::hex("#268BD2"),
                tab_inactive: Color::hex("#073642"),
                search_match: Color::hex("#B58900").with_alpha(0.5),
                url: Color::hex("#268BD2"),
            },
        }
    }

    /// Solarized Light theme
    pub fn solarized_light() -> Self {
        Self {
            meta: ThemeMeta {
                name: "Solarized Light".to_string(),
                author: "Ethan Schoonover".to_string(),
                description: "Precision colors for machines and people".to_string(),
            },
            primary: PrimaryColors {
                black: Color::hex("#073642"),
                red: Color::hex("#DC322F"),
                green: Color::hex("#859900"),
                yellow: Color::hex("#B58900"),
                blue: Color::hex("#268BD2"),
                magenta: Color::hex("#D33682"),
                cyan: Color::hex("#2AA198"),
                white: Color::hex("#EEE8D5"),
            },
            bright: BrightColors {
                black: Color::hex("#002B36"),
                red: Color::hex("#CB4B16"),
                green: Color::hex("#586E75"),
                yellow: Color::hex("#657B83"),
                blue: Color::hex("#839496"),
                magenta: Color::hex("#6C71C4"),
                cyan: Color::hex("#93A1A1"),
                white: Color::hex("#FDF6E3"),
            },
            ui: UiColors {
                background: Color::hex("#FDF6E3"),
                foreground: Color::hex("#657B83"),
                cursor: Color::hex("#657B83"),
                cursor_text: Color::hex("#FDF6E3"),
                selection: Color::hex("#EEE8D5"),
                selection_text: None,
                border: Color::hex("#EEE8D5"),
                tab_active: Color::hex("#268BD2"),
                tab_inactive: Color::hex("#EEE8D5"),
                search_match: Color::hex("#B58900").with_alpha(0.5),
                url: Color::hex("#268BD2"),
            },
        }
    }

    /// Dracula theme
    pub fn dracula() -> Self {
        Self {
            meta: ThemeMeta {
                name: "Dracula".to_string(),
                author: "Zeno Rocha".to_string(),
                description: "A dark theme for vampires".to_string(),
            },
            primary: PrimaryColors {
                black: Color::hex("#21222C"),
                red: Color::hex("#FF5555"),
                green: Color::hex("#50FA7B"),
                yellow: Color::hex("#F1FA8C"),
                blue: Color::hex("#BD93F9"),
                magenta: Color::hex("#FF79C6"),
                cyan: Color::hex("#8BE9FD"),
                white: Color::hex("#F8F8F2"),
            },
            bright: BrightColors {
                black: Color::hex("#6272A4"),
                red: Color::hex("#FF6E6E"),
                green: Color::hex("#69FF94"),
                yellow: Color::hex("#FFFFA5"),
                blue: Color::hex("#D6ACFF"),
                magenta: Color::hex("#FF92DF"),
                cyan: Color::hex("#A4FFFF"),
                white: Color::hex("#FFFFFF"),
            },
            ui: UiColors {
                background: Color::hex("#282A36"),
                foreground: Color::hex("#F8F8F2"),
                cursor: Color::hex("#F8F8F2"),
                cursor_text: Color::hex("#282A36"),
                selection: Color::hex("#44475A"),
                selection_text: None,
                border: Color::hex("#44475A"),
                tab_active: Color::hex("#BD93F9"),
                tab_inactive: Color::hex("#44475A"),
                search_match: Color::hex("#F1FA8C").with_alpha(0.5),
                url: Color::hex("#8BE9FD"),
            },
        }
    }

    /// Nord theme
    pub fn nord() -> Self {
        Self {
            meta: ThemeMeta {
                name: "Nord".to_string(),
                author: "Arctic Ice Studio".to_string(),
                description: "An arctic, north-bluish clean and elegant theme".to_string(),
            },
            primary: PrimaryColors {
                black: Color::hex("#3B4252"),
                red: Color::hex("#BF616A"),
                green: Color::hex("#A3BE8C"),
                yellow: Color::hex("#EBCB8B"),
                blue: Color::hex("#81A1C1"),
                magenta: Color::hex("#B48EAD"),
                cyan: Color::hex("#88C0D0"),
                white: Color::hex("#E5E9F0"),
            },
            bright: BrightColors {
                black: Color::hex("#4C566A"),
                red: Color::hex("#BF616A"),
                green: Color::hex("#A3BE8C"),
                yellow: Color::hex("#EBCB8B"),
                blue: Color::hex("#81A1C1"),
                magenta: Color::hex("#B48EAD"),
                cyan: Color::hex("#8FBCBB"),
                white: Color::hex("#ECEFF4"),
            },
            ui: UiColors {
                background: Color::hex("#2E3440"),
                foreground: Color::hex("#D8DEE9"),
                cursor: Color::hex("#D8DEE9"),
                cursor_text: Color::hex("#2E3440"),
                selection: Color::hex("#434C5E"),
                selection_text: None,
                border: Color::hex("#3B4252"),
                tab_active: Color::hex("#88C0D0"),
                tab_inactive: Color::hex("#3B4252"),
                search_match: Color::hex("#EBCB8B").with_alpha(0.5),
                url: Color::hex("#88C0D0"),
            },
        }
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
        Theme::turmeric().primary
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
        Theme::turmeric().bright
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
        Theme::turmeric().ui
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
