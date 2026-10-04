//! Select Graphic Rendition: setting text attributes and reporting them.

use super::*;

impl Terminal {
    pub(super) fn sgr_status(&self) -> Vec<u8> {
        let mut params = Vec::new();
        let attrs = self.current_attrs;
        if attrs.contains(CellAttrs::BOLD) {
            params.push("1".to_string());
        }
        if attrs.contains(CellAttrs::DIM) {
            params.push("2".to_string());
        }
        if attrs.contains(CellAttrs::ITALIC) {
            params.push("3".to_string());
        }
        if attrs.contains(CellAttrs::CURLY_UNDERLINE) {
            params.push("4:3".to_string());
        } else if attrs.contains(CellAttrs::DOUBLE_UNDERLINE) {
            params.push("4:2".to_string());
        } else if attrs.contains(CellAttrs::DOTTED_UNDERLINE) {
            params.push("4:4".to_string());
        } else if attrs.contains(CellAttrs::DASHED_UNDERLINE) {
            params.push("4:5".to_string());
        } else if attrs.contains(CellAttrs::UNDERLINE) {
            params.push("4".to_string());
        }
        if attrs.contains(CellAttrs::BLINK) {
            params.push("5".to_string());
        }
        if attrs.contains(CellAttrs::RAPID_BLINK) {
            params.push("6".to_string());
        }
        if attrs.contains(CellAttrs::INVERSE) {
            params.push("7".to_string());
        }
        if attrs.contains(CellAttrs::HIDDEN) {
            params.push("8".to_string());
        }
        if attrs.contains(CellAttrs::STRIKE) {
            params.push("9".to_string());
        }
        Self::push_sgr_color(&mut params, self.current_fg_spec, true);
        Self::push_sgr_color(&mut params, self.current_bg_spec, false);
        if let Some(spec) = self.current_underline_color_spec {
            Self::push_sgr_extended_color(&mut params, spec, 58);
        }
        if params.is_empty() {
            params.push("0".to_string());
        }
        format!("{}m", params.join(";")).into_bytes()
    }

    fn push_sgr_color(params: &mut Vec<String>, spec: ColorSpec, foreground: bool) {
        match spec {
            ColorSpec::Default => {}
            ColorSpec::Indexed(index @ 0..=7) => {
                params.push((if foreground { 30 + index } else { 40 + index }).to_string());
            }
            ColorSpec::Indexed(index @ 8..=15) => {
                params.push(
                    (if foreground {
                        90 + index - 8
                    } else {
                        100 + index - 8
                    })
                    .to_string(),
                );
            }
            ColorSpec::Indexed(index) => {
                params.push(format!("{};5;{index}", if foreground { 38 } else { 48 }))
            }
            ColorSpec::TrueColor(color) => params.push(format!(
                "{};2;{};{};{}",
                if foreground { 38 } else { 48 },
                color.r,
                color.g,
                color.b
            )),
        }
    }

    fn push_sgr_extended_color(params: &mut Vec<String>, spec: ColorSpec, selector: u8) {
        match spec {
            ColorSpec::Default => {}
            ColorSpec::Indexed(index) => params.push(format!("{selector};5;{index}")),
            ColorSpec::TrueColor(color) => {
                params.push(format!("{selector};2;{};{};{}", color.r, color.g, color.b))
            }
        }
    }

    /// Set foreground color with its original specification
    pub(crate) fn set_fg(&mut self, color: Color, spec: ColorSpec) {
        self.current_fg = color;
        self.current_fg_spec = spec;
    }

    /// Set background color with its original specification
    pub(crate) fn set_bg(&mut self, color: Color, spec: ColorSpec) {
        self.current_bg = color;
        self.current_bg_spec = spec;
    }

    /// Reset all attributes to default
    pub(crate) fn reset_attrs(&mut self) {
        self.current_attrs = CellAttrs::empty();
        self.current_fg = Color::default_fg();
        self.current_bg = Color::default_bg();
        self.current_fg_spec = ColorSpec::Default;
        self.current_bg_spec = ColorSpec::Default;
        self.current_underline_color = None;
        self.current_underline_color_spec = None;
    }

    /// Add text attributes to the current set
    pub(crate) fn add_attrs(&mut self, attrs: CellAttrs) {
        self.current_attrs.insert(attrs);
    }

    /// Remove text attributes from the current set
    pub(crate) fn remove_attrs(&mut self, attrs: CellAttrs) {
        self.current_attrs.remove(attrs);
    }

    /// Set an underline color while retaining its original SGR encoding.
    pub(crate) fn set_underline_color_spec(&mut self, color: Color, spec: ColorSpec) {
        self.current_underline_color = Some(color);
        self.current_underline_color_spec = Some(spec);
    }

    /// Reset underline color (use fg color)
    pub(crate) fn reset_underline_color(&mut self) {
        self.current_underline_color = None;
        self.current_underline_color_spec = None;
    }

    /// Set underline style (clears all underline flags and sets the specified style)
    pub(crate) fn set_underline_style(&mut self, style: u16) {
        // Clear all underline-related flags first
        self.current_attrs.remove(CellAttrs::ALL_UNDERLINES);

        // Set the specified style
        match style {
            0 => {} // No underline
            1 => self.current_attrs.insert(CellAttrs::UNDERLINE),
            2 => self.current_attrs.insert(CellAttrs::DOUBLE_UNDERLINE),
            3 => self.current_attrs.insert(CellAttrs::CURLY_UNDERLINE),
            4 => self.current_attrs.insert(CellAttrs::DOTTED_UNDERLINE),
            5 => self.current_attrs.insert(CellAttrs::DASHED_UNDERLINE),
            _ => self.current_attrs.insert(CellAttrs::UNDERLINE), // Default to single
        }
    }
}
