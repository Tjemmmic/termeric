//! Theme management for Termeric
//!
//! Handles loading, parsing, and applying color themes.

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use std::path::{Path, PathBuf};

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

/// Single source of truth for built-in theme names. Used by
/// `list_available()`; `load_builtin` must stay in sync (enforced by
/// `test_all_listed_themes_load`).
const BUILTIN_THEME_NAMES: [&str; 26] = [
    // Root themes (Termeric house palettes)
    "turmeric",
    "ginger",
    "daikon",
    "malanga",
    "beet",
    "turnip",
    "yuca",
    "taro",
    "wasabi",
    "galangal",
    "horseradish",
    "parsnip",
    // Community themes
    "solarized-dark",
    "solarized-light",
    "dracula",
    "nord",
    "gruvbox-dark",
    "gruvbox-light",
    "catppuccin-mocha",
    "catppuccin-latte",
    "tokyo-night",
    "one-dark",
    "one-light",
    "rose-pine",
    "kanagawa",
    "everforest",
];

impl Default for Theme {
    /// The house palette, parsed from the embedded `turmeric.toml` so the
    /// fallback can never drift from the built-in theme it stands in for.
    fn default() -> Self {
        Self::load_builtin("turmeric").expect("embedded turmeric theme is valid TOML")
    }
}

impl Theme {
    /// Load theme from a TOML file
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read theme {}", path.display()))?;
        let theme: Theme = toml::from_str(&content)
            .with_context(|| format!("failed to parse theme {}", path.display()))?;
        Ok(theme)
    }

    /// Load a built-in theme by name.
    /// Themes are embedded as TOML files at compile time.
    pub fn load_builtin(name: &str) -> Result<Self> {
        let toml_str = match name.to_lowercase().as_str() {
            // ── Root themes (Termeric house palettes) ──
            "turmeric" | "default" => include_str!("../../themes/roots/turmeric.toml"),
            "ginger" => include_str!("../../themes/roots/ginger.toml"),
            "daikon" => include_str!("../../themes/roots/daikon.toml"),
            "malanga" => include_str!("../../themes/roots/malanga.toml"),
            "beet" => include_str!("../../themes/roots/beet.toml"),
            "turnip" => include_str!("../../themes/roots/turnip.toml"),
            "yuca" => include_str!("../../themes/roots/yuca.toml"),
            "taro" => include_str!("../../themes/roots/taro.toml"),
            "wasabi" => include_str!("../../themes/roots/wasabi.toml"),
            "galangal" => include_str!("../../themes/roots/galangal.toml"),
            "horseradish" => include_str!("../../themes/roots/horseradish.toml"),
            "parsnip" => include_str!("../../themes/roots/parsnip.toml"),
            // ── Community themes ──
            "solarized-dark" => include_str!("../../themes/community/solarized-dark.toml"),
            "solarized-light" => include_str!("../../themes/community/solarized-light.toml"),
            "dracula" => include_str!("../../themes/community/dracula.toml"),
            "nord" => include_str!("../../themes/community/nord.toml"),
            "gruvbox-dark" => include_str!("../../themes/community/gruvbox-dark.toml"),
            "gruvbox-light" => include_str!("../../themes/community/gruvbox-light.toml"),
            "catppuccin-mocha" => include_str!("../../themes/community/catppuccin-mocha.toml"),
            "catppuccin-latte" => include_str!("../../themes/community/catppuccin-latte.toml"),
            "tokyo-night" => include_str!("../../themes/community/tokyo-night.toml"),
            "one-dark" => include_str!("../../themes/community/one-dark.toml"),
            "one-light" => include_str!("../../themes/community/one-light.toml"),
            "rose-pine" => include_str!("../../themes/community/rose-pine.toml"),
            "kanagawa" => include_str!("../../themes/community/kanagawa.toml"),
            "everforest" => include_str!("../../themes/community/everforest.toml"),
            _ => return Err(anyhow!("Unknown built-in theme: {}", name)),
        };
        let theme: Theme = toml::from_str(toml_str)?;
        Ok(theme)
    }

    /// List all available built-in theme names. Kept in sync with
    /// `load_builtin` via the `test_all_listed_themes_load` test below —
    /// if you add a theme to `load_builtin`, add it here too (the test
    /// will catch any drift in CI).
    pub fn list_available() -> &'static [&'static str] {
        &BUILTIN_THEME_NAMES
    }

    /// List custom theme names found in the user's themes directory.
    ///
    /// Scans `Config::themes_dir()` for `.toml` files and returns their
    /// stem names (e.g., `my-theme.toml` -> `"my-theme"`).
    pub fn list_custom() -> Vec<String> {
        Self::list_custom_in_dir(&super::Config::themes_dir())
    }

    fn list_custom_in_dir(dir: &Path) -> Vec<String> {
        let mut names = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("toml") {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        names.push(stem.to_string());
                    }
                }
            }
        }
        names.sort();
        names
    }

    /// List every selectable theme name, with custom themes appended and
    /// duplicate names removed. Custom files take precedence during loading.
    pub fn list_all() -> Vec<String> {
        let mut names: Vec<String> = Self::list_available()
            .iter()
            .map(|name| (*name).to_string())
            .collect();
        for custom in Self::list_custom() {
            if !names.contains(&custom) {
                names.push(custom);
            }
        }
        names
    }

    /// Resolve a configured theme and return the file that should be watched,
    /// if the selected theme is file-backed.
    pub fn resolve(name: &str, explicit_path: Option<&Path>) -> Result<(Self, Option<PathBuf>)> {
        if let Some(path) = explicit_path {
            return Self::load(path).map(|theme| (theme, Some(path.to_path_buf())));
        }
        Self::resolve_named_in_dir(name, &super::Config::themes_dir())
    }

    fn resolve_named_in_dir(name: &str, custom_dir: &Path) -> Result<(Self, Option<PathBuf>)> {
        if Path::new(name).components().count() == 1 {
            let custom_path = custom_dir.join(format!("{name}.toml"));
            if custom_path.is_file() {
                return Self::load(&custom_path).map(|theme| (theme, Some(custom_path)));
            }
        }
        Self::load_builtin(name).map(|theme| (theme, None))
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
    pub fn to_f32_array(self) -> [f32; 4] {
        [
            self.r as f32 / 255.0,
            self.g as f32 / 255.0,
            self.b as f32 / 255.0,
            self.a,
        ]
    }

    /// Convert to wgpu Color
    pub fn to_wgpu(self) -> wgpu::Color {
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
    fn shipped_custom_theme_example_loads() {
        let example =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../assets/themes/custom-example.toml");
        let theme = Theme::load(&example).unwrap();
        assert_eq!(theme.meta.name, "Custom Example");
    }

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

    fn relative_luminance(color: &Color) -> f64 {
        let linear = |channel: u8| {
            let value = f64::from(channel) / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
    }

    fn contrast(first: &Color, second: &Color) -> f64 {
        let (first, second) = (relative_luminance(first), relative_luminance(second));
        (first.max(second) + 0.05) / (first.min(second) + 0.05)
    }

    /// HSL hue in degrees and saturation in `0..=1`.
    fn hue_and_saturation(color: &Color) -> (f64, f64) {
        let [r, g, b] = [color.r, color.g, color.b].map(|channel| f64::from(channel) / 255.0);
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        let delta = max - min;
        if delta == 0.0 {
            return (0.0, 0.0);
        }
        let lightness = (max + min) / 2.0;
        let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs());
        let hue = if max == r {
            60.0 * ((g - b) / delta).rem_euclid(6.0)
        } else if max == g {
            60.0 * ((b - r) / delta + 2.0)
        } else {
            60.0 * ((r - g) / delta + 4.0)
        };
        (hue, saturation)
    }

    /// Hue families (degrees) in which each ANSI slot stays recognisable.
    fn in_hue_family(slot: &str, hue: f64) -> bool {
        match slot {
            "red" => !(20.0..345.0).contains(&hue),
            "yellow" => (35.0..70.0).contains(&hue),
            "green" => (75.0..155.0).contains(&hue),
            "cyan" => (160.0..200.0).contains(&hue),
            "blue" => (200.0..255.0).contains(&hue),
            "magenta" => (275.0..345.0).contains(&hue),
            _ => unreachable!("unknown chromatic slot {slot}"),
        }
    }

    fn root_palette_violations(name: &str, theme: &Theme) -> Vec<String> {
        let background = &theme.ui.background;
        let mut violations = Vec::new();
        let mut require = |ok: bool, message: String| {
            if !ok {
                violations.push(format!("{name}: {message}"));
            }
        };
        let fg_contrast = contrast(&theme.ui.foreground, background);
        require(
            fg_contrast >= 7.0,
            format!("foreground contrast {fg_contrast:.1} < 7"),
        );
        let (p, b) = (&theme.primary, &theme.bright);
        let chromatic = [
            ("red", &p.red, &b.red),
            ("green", &p.green, &b.green),
            ("yellow", &p.yellow, &b.yellow),
            ("blue", &p.blue, &b.blue),
            ("magenta", &p.magenta, &b.magenta),
            ("cyan", &p.cyan, &b.cyan),
        ];
        for (slot, primary, bright) in chromatic {
            for (variant, color, minimum) in [("", primary, 3.0), ("bright ", bright, 4.5)] {
                let (hue, saturation) = hue_and_saturation(color);
                require(
                    in_hue_family(slot, hue),
                    format!("{variant}{slot} hue {hue:.0} is outside the {slot} family"),
                );
                require(
                    saturation >= 0.25,
                    format!("{variant}{slot} saturation {saturation:.2} < 0.25"),
                );
                let ratio = contrast(color, background);
                require(
                    ratio >= minimum,
                    format!("{variant}{slot} contrast {ratio:.1} < {minimum}"),
                );
            }
            require(
                relative_luminance(bright) > relative_luminance(primary),
                format!("bright {slot} is not lighter than {slot}"),
            );
        }
        for (variant, colors) in [
            (
                "",
                [&p.red, &p.yellow, &p.green, &p.cyan, &p.blue, &p.magenta],
            ),
            (
                "bright ",
                [&b.red, &b.yellow, &b.green, &b.cyan, &b.blue, &b.magenta],
            ),
        ] {
            let names = ["red", "yellow", "green", "cyan", "blue", "magenta"];
            for index in 0..names.len() {
                let next = (index + 1) % names.len();
                let (first, _) = hue_and_saturation(colors[index]);
                let (second, _) = hue_and_saturation(colors[next]);
                let distance = (first - second).abs() % 360.0;
                let separation = distance.min(360.0 - distance);
                require(
                    separation >= 25.0,
                    format!(
                        "{variant}{} and {variant}{} hues are only {separation:.0} degrees apart",
                        names[index], names[next]
                    ),
                );
            }
        }
        let black = contrast(&p.black, background);
        require(
            black >= 1.15,
            format!("black contrast {black:.2} < 1.15 against background"),
        );
        let bright_black = contrast(&b.black, background);
        require(
            bright_black >= 3.0,
            format!("bright black contrast {bright_black:.1} < 3"),
        );
        let white = contrast(&p.white, background);
        require(white >= 4.5, format!("white contrast {white:.1} < 4.5"));
        let bright_white = contrast(&b.white, background);
        require(
            bright_white >= 7.0,
            format!("bright white contrast {bright_white:.1} < 7"),
        );
        violations
    }

    /// Root palettes are tinted toward their plant, but applications rely on
    /// ANSI meaning (red for errors, green for success, dim bright-black
    /// hints), so every slot must stay in its hue family and readable.
    #[test]
    fn root_theme_palettes_keep_ansi_meaning_and_contrast() {
        let mut violations = Vec::new();
        let mut roots = 0;
        for name in Theme::list_available() {
            let theme = Theme::load_builtin(name).unwrap();
            if theme.meta.author == "Termeric" {
                roots += 1;
                violations.extend(root_palette_violations(name, &theme));
            }
        }
        assert_eq!(roots, 12, "every root theme must be checked");
        assert!(
            violations.is_empty(),
            "root palette contract violations:\n{}",
            violations.join("\n")
        );
    }

    #[test]
    fn default_theme_is_the_embedded_turmeric_palette() {
        let fallback = Theme::default();
        let turmeric = Theme::load_builtin("turmeric").unwrap();
        assert_eq!(fallback.meta.name, "Turmeric");
        assert_eq!(
            fallback.ui.background.to_f32_array(),
            turmeric.ui.background.to_f32_array()
        );
        assert_eq!(
            fallback.primary.red.to_f32_array(),
            turmeric.primary.red.to_f32_array()
        );
    }

    #[test]
    fn test_all_listed_themes_load() {
        // Every name exposed by `list_available` must be loadable by
        // `load_builtin`. Catches drift between the two lists.
        for name in Theme::list_available() {
            assert!(
                Theme::load_builtin(name).is_ok(),
                "theme '{}' is listed but can't be loaded",
                name
            );
        }
    }

    #[test]
    fn resolve_explicit_path_returns_watch_source() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("warm-root.toml");
        std::fs::write(&path, include_str!("../../themes/roots/turmeric.toml")).unwrap();

        let (theme, source) = Theme::resolve("ignored", Some(&path)).unwrap();
        assert_eq!(theme.meta.name, "Turmeric");
        assert_eq!(source, Some(path));
    }

    #[test]
    fn resolve_custom_name_returns_watch_source() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("warm-root.toml");
        std::fs::write(&path, include_str!("../../themes/roots/turmeric.toml")).unwrap();

        let (_, source) = Theme::resolve_named_in_dir("warm-root", dir.path()).unwrap();
        assert_eq!(source, Some(path));
    }

    #[test]
    fn custom_theme_discovery_is_sorted_and_ignores_other_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("zinnia.toml"), "").unwrap();
        std::fs::write(dir.path().join("arrowroot.toml"), "").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "").unwrap();

        assert_eq!(
            Theme::list_custom_in_dir(dir.path()),
            vec!["arrowroot".to_string(), "zinnia".to_string()]
        );
    }

    #[test]
    fn malformed_theme_reports_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.toml");
        std::fs::write(&path, "not = [valid").unwrap();

        let error = Theme::resolve("ignored", Some(&path)).unwrap_err();
        assert!(error.to_string().contains("failed to parse theme"));
        assert!(format!("{error:#}").contains("broken.toml"));
    }
}
