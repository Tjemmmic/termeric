//! Color conversion utilities for ANSI escape sequences

use crate::grid::Color;

/// ANSI color index (0-15)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnsiColor {
    Black = 0,
    Red = 1,
    Green = 2,
    Yellow = 3,
    Blue = 4,
    Magenta = 5,
    Cyan = 6,
    White = 7,
    BrightBlack = 8,
    BrightRed = 9,
    BrightGreen = 10,
    BrightYellow = 11,
    BrightBlue = 12,
    BrightMagenta = 13,
    BrightCyan = 14,
    BrightWhite = 15,
}

impl AnsiColor {
    /// Convert an index to an AnsiColor
    pub fn from_index(idx: u8) -> Option<Self> {
        match idx {
            0 => Some(Self::Black),
            1 => Some(Self::Red),
            2 => Some(Self::Green),
            3 => Some(Self::Yellow),
            4 => Some(Self::Blue),
            5 => Some(Self::Magenta),
            6 => Some(Self::Cyan),
            7 => Some(Self::White),
            8 => Some(Self::BrightBlack),
            9 => Some(Self::BrightRed),
            10 => Some(Self::BrightGreen),
            11 => Some(Self::BrightYellow),
            12 => Some(Self::BrightBlue),
            13 => Some(Self::BrightMagenta),
            14 => Some(Self::BrightCyan),
            15 => Some(Self::BrightWhite),
            _ => None,
        }
    }
}

/// Get the default color for a normal ANSI color (0-7)
pub fn ansi_color(idx: u16) -> Color {
    match idx {
        0 => Color::rgb(0, 0, 0),         // Black
        1 => Color::rgb(205, 49, 49),     // Red
        2 => Color::rgb(13, 188, 121),    // Green
        3 => Color::rgb(229, 229, 16),    // Yellow
        4 => Color::rgb(36, 114, 200),    // Blue
        5 => Color::rgb(188, 63, 188),    // Magenta
        6 => Color::rgb(17, 168, 205),    // Cyan
        7 => Color::rgb(229, 229, 229),   // White
        _ => Color::rgb(255, 255, 255),
    }
}

/// Get the default color for a bright ANSI color (8-15)
pub fn ansi_bright_color(idx: u16) -> Color {
    match idx {
        0 => Color::rgb(102, 102, 102),   // Bright Black (Gray)
        1 => Color::rgb(241, 76, 76),     // Bright Red
        2 => Color::rgb(35, 209, 139),    // Bright Green
        3 => Color::rgb(245, 245, 67),    // Bright Yellow
        4 => Color::rgb(59, 142, 234),    // Bright Blue
        5 => Color::rgb(214, 112, 214),   // Bright Magenta
        6 => Color::rgb(41, 184, 219),    // Bright Cyan
        7 => Color::rgb(255, 255, 255),   // Bright White
        _ => Color::rgb(255, 255, 255),
    }
}

/// Convert a 256-color palette index to RGB
pub fn color_256(idx: u8) -> Color {
    match idx {
        // Standard ANSI colors (0-7)
        0..=7 => ansi_color(idx as u16),

        // Bright ANSI colors (8-15)
        8..=15 => ansi_bright_color((idx - 8) as u16),

        // 6x6x6 color cube (16-231)
        16..=231 => {
            let idx = idx - 16;
            let r = (idx / 36) % 6;
            let g = (idx / 6) % 6;
            let b = idx % 6;

            // Convert to 0-255 range
            // 0 -> 0, 1 -> 95, 2 -> 135, 3 -> 175, 4 -> 215, 5 -> 255
            let to_255 = |v: u8| -> u8 {
                if v == 0 { 0 } else { v * 40 + 55 }
            };

            Color::rgb(to_255(r), to_255(g), to_255(b))
        }

        // Grayscale ramp (232-255)
        232..=255 => {
            // 24 shades of gray from 8 to 238
            let gray = (idx - 232) * 10 + 8;
            Color::rgb(gray, gray, gray)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ansi_colors() {
        let black = ansi_color(0);
        assert_eq!(black, Color::rgb(0, 0, 0));

        let red = ansi_color(1);
        assert_eq!(red, Color::rgb(205, 49, 49));
    }

    #[test]
    fn test_bright_colors() {
        let bright_black = ansi_bright_color(0);
        assert_eq!(bright_black, Color::rgb(102, 102, 102));

        let bright_white = ansi_bright_color(7);
        assert_eq!(bright_white, Color::rgb(255, 255, 255));
    }

    #[test]
    fn test_color_256_ansi() {
        // First 16 should match ANSI colors
        assert_eq!(color_256(0), ansi_color(0));
        assert_eq!(color_256(7), ansi_color(7));
        assert_eq!(color_256(8), ansi_bright_color(0));
    }

    #[test]
    fn test_color_256_cube() {
        // Test color cube corners
        let black = color_256(16); // 0,0,0
        assert_eq!(black, Color::rgb(0, 0, 0));

        let red = color_256(196); // 5,0,0
        assert_eq!(red, Color::rgb(255, 0, 0));

        let white = color_256(231); // 5,5,5
        assert_eq!(white, Color::rgb(255, 255, 255));
    }

    #[test]
    fn test_color_256_grayscale() {
        let dark = color_256(232);
        assert_eq!(dark, Color::rgb(8, 8, 8));

        let light = color_256(255);
        assert_eq!(light, Color::rgb(238, 238, 238));
    }

}
