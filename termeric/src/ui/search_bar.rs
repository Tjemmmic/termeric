//! The floating find box: what it shows and where its controls sit.

use crate::ui::geometry::Bounds;

/// Information needed to render the search bar
#[derive(Clone, Debug)]
pub struct SearchBarInfo {
    /// Current search query
    pub query: String,
    /// Whether case-sensitive mode is enabled
    pub case_sensitive: bool,
    /// Whether regex mode is enabled
    pub use_regex: bool,
    /// Whether filter mode is enabled (show only matching lines)
    pub filter_mode: bool,
    /// Current match index (1-based, None if no matches)
    pub current_match: Option<usize>,
    /// Total match count
    pub match_count: usize,
    /// Recent occurrences retained for navigation and highlighting
    pub retained_match_count: usize,
    /// Whether older occurrences fell outside the retained recent window
    pub results_truncated: bool,
    /// Whether the complete result set is still being assembled
    pub searching: bool,
    /// Number of lines containing matches (for filter mode display)
    pub matching_line_count: usize,
    /// Regex error message if any
    pub regex_error: Option<String>,
    /// Cursor position in query
    pub cursor_pos: usize,
    /// Whether the query currently owns keyboard input.
    pub is_active: bool,
}

impl SearchBarInfo {
    pub fn count_text(&self) -> String {
        if self.regex_error.is_some() {
            "Invalid regex".to_string()
        } else if self.searching {
            "Searching…".to_string()
        } else if self.match_count > 0 && self.results_truncated {
            format!(
                "{} of {} · recent",
                self.current_match.unwrap_or(0),
                self.match_count
            )
        } else if self.match_count > 0 {
            format!(
                "{} of {}",
                self.current_match.unwrap_or(0),
                self.match_count
            )
        } else if !self.query.is_empty() {
            "No matches".to_string()
        } else {
            String::new()
        }
    }

    pub fn accessible_status(&self) -> String {
        if let Some(error) = &self.regex_error {
            return format!("Regular expression error: {error}");
        }
        if self.searching {
            return "Searching terminal history".to_string();
        }
        if self.match_count == 0 {
            return if self.query.is_empty() {
                "Enter a terminal search query".to_string()
            } else {
                "No matches".to_string()
            };
        }
        let current = self.current_match.unwrap_or(0);
        let suffix = if self.results_truncated {
            ", navigation retains only the most recent matches"
        } else {
            ""
        };
        format!(
            "Match {current} of {}; {} matching lines{suffix}",
            self.match_count, self.matching_line_count
        )
    }
}

/// One shared geometry contract for painting, pointer hit-testing, and the
/// native accessibility tree.
///
/// The find box floats over the top-right of the terminal area; terminal
/// content never moves to make room for it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SearchBarLayout {
    /// The floating box, including the error line when one is shown.
    pub bar: Bounds,
    /// Magnifier slot at the start of the control row.
    pub icon: Bounds,
    pub input: Bounds,
    pub filter: Bounds,
    pub case_sensitive: Bounds,
    pub regex: Bounds,
    /// Right-aligned match status, sized for its text in terminal cells as an
    /// upper bound on the narrower UI face.
    pub status: Bounds,
    /// Always reserved so the box does not reflow as results arrive; they
    /// act only while `navigable`.
    pub previous: Bounds,
    pub next: Bounds,
    /// Whether there are matches to move between.
    pub navigable: bool,
    pub close: Bounds,
    /// A second line describing an invalid regular expression.
    pub error: Option<Bounds>,
    pub text_y: f32,
}

impl SearchBarLayout {
    /// The painted query and native IME share one grapheme-safe caret viewport.
    pub fn input_viewport(
        &self,
        info: &SearchBarInfo,
        cell_width: f32,
    ) -> Option<crate::ui::text_input_viewport::TextInputViewport> {
        crate::ui::text_input_viewport::TextInputViewport::calculate(
            &info.query,
            info.cursor_pos,
            self.input.x,
            self.text_y,
            self.input.x + self.input.width,
            cell_width,
        )
    }

    /// Place the find box in the terminal area that starts at
    /// (`content_x`, `content_y`) and is `content_width` wide.
    pub fn calculate(
        info: &SearchBarInfo,
        content_x: f32,
        content_width: f32,
        content_y: f32,
        cell_width: f32,
        cell_height: f32,
    ) -> Self {
        let margin = cell_width;
        // Clear the terminal scrollbar's hit strip, which takes presses first.
        let margin_right = (cell_width * 2.0).max(20.0);
        let inset = cell_width * 0.5;
        let control = (cell_height * 1.4).round();
        let row_height = control + inset * 2.0;
        let available = (content_width - margin - margin_right).max(0.0);
        let width = (cell_width * 56.0).min(available);
        let x = content_x + content_width - margin_right - width;
        let y = content_y + margin * 0.75;
        let row_y = y + inset;
        let square = |x: f32| Bounds {
            x,
            y: row_y,
            width: control,
            height: control,
        };
        let gap = cell_width * 0.25;
        let mut right = x + width - inset;
        let mut take = |width: f32, space_after: f32| {
            right -= space_after + width;
            right
        };
        let close = square(take(control, 0.0));
        let next = square(take(control, gap * 2.0));
        let previous = square(take(control, gap));
        let filter = square(take(control, gap * 2.0));
        let regex = square(take(control, gap));
        let case_sensitive = square(take(control, gap));
        let has_matches = info.retained_match_count > 0;

        let status_width = info.count_text().chars().count() as f32 * cell_width;
        let status_x = take(status_width, gap * 2.0);
        let icon = square(x + inset);
        let input_x = icon.x + icon.width + gap;
        let input_right = (status_x - gap * 2.0).max(input_x);
        let text_y = row_y + (control - cell_height) / 2.0;
        let error = info.regex_error.as_ref().map(|_| Bounds {
            x: input_x,
            y: row_y + control,
            width: (x + width - inset - input_x).max(0.0),
            height: cell_height * 1.3,
        });
        let height = row_height + error.map_or(0.0, |line| line.height);
        Self {
            bar: Bounds {
                x,
                y,
                width,
                height,
            },
            icon,
            input: Bounds {
                x: input_x,
                y: row_y,
                width: (input_right - input_x).max(0.0),
                height: control,
            },
            filter,
            case_sensitive,
            regex,
            status: Bounds {
                x: status_x,
                y: row_y,
                width: status_width,
                height: control,
            },
            previous,
            next,
            navigable: has_matches,
            close,
            error,
            text_y,
        }
    }
}
