//! "Check which font draws text…" on the Font page of Settings: a prompt
//! for the text, then a report of which font draws each character. Nothing
//! is changed.

use super::*;
use crate::renderer::{
    FontCoverageReport, FontCoverageRoute, FontCoverageSource, MAX_FONT_COVERAGE_GRAPHEMES,
    MAX_FONT_COVERAGE_INPUT_BYTES,
};
use crate::ui::dialog::{ButtonRole, DialogBlock, DialogButton, DialogSpec, FieldSpec};
use crate::ui::settings::format_codepoint;

/// The prompt's answer, which checks the text.
const CHECK: &str = "check";
/// The report's answer, which asks for other text.
const RETRY: &str = "retry";

/// The coverage prompt or report awaiting an answer, with the text typed.
#[derive(Debug, Clone)]
pub(super) struct PendingFontCoverage {
    request_id: u64,
    /// What was typed, to show again on "Check other text…"; `None` while
    /// the prompt itself is open.
    reported: Option<String>,
}

impl App {
    pub(super) fn ask_font_coverage(&mut self, value: String, error: Option<&str>) {
        let spec = DialogSpec::prompt(
            "Check which font draws text",
            vec![DialogBlock::text(format!(
                "Type the text, or one character as U+ or 0x hex. Up to {MAX_FONT_COVERAGE_GRAPHEMES} characters are checked, and nothing is changed."
            ))],
            FieldSpec::new("Text", value, MAX_FONT_COVERAGE_INPUT_BYTES)
                .with_error(error.map(crate::safe_text::capitalized)),
            CHECK,
            "Check",
        );
        let request_id = self.dialog.show(spec);
        self.dialog_flow = Some(DialogFlow::FontCoverage(PendingFontCoverage {
            request_id,
            reported: None,
        }));
    }

    /// Answer the coverage prompt or report. Returns whether the answer was
    /// its; Cancel and Close aren't, and close the dialog.
    pub(super) fn handle_font_coverage_choice(&mut self, request_id: u64, choice_id: &str) -> bool {
        let reported = match &self.dialog_flow {
            Some(DialogFlow::FontCoverage(pending)) if pending.request_id == request_id => {
                pending.reported.clone()
            }
            _ => return false,
        };
        match (reported, choice_id) {
            (None, CHECK) => {
                let Some(value) = self.dialog.field_value(request_id).map(ToOwned::to_owned) else {
                    return false;
                };
                self.dialog.dismiss(request_id);
                self.dialog_flow = None;
                self.check_font_coverage(value);
            }
            (Some(reported), RETRY) => {
                self.dialog.dismiss(request_id);
                self.dialog_flow = None;
                self.ask_font_coverage(reported, None);
            }
            _ => return false,
        }
        true
    }

    fn check_font_coverage(&mut self, submitted: String) {
        let query = match parse_font_coverage_query(&submitted) {
            Ok(query) => query,
            Err(error) => return self.ask_font_coverage(submitted, Some(&error)),
        };
        let configured_primary = self.config.font.family.clone();
        let report = self
            .renderer
            .as_mut()
            .ok_or_else(|| "the renderer is not initialized in this window".to_string())
            .and_then(|renderer| renderer.inspect_font_coverage(&configured_primary, &query));
        let mut body = vec![DialogBlock::value(format_grapheme(&query))];
        match report {
            Ok(report) => {
                body.push(DialogBlock::text(
                    "From the installed fonts' character maps and a trial shaping in the regular style.",
                ));
                body.push(DialogBlock::value(
                    format_font_coverage_report(&report).join("\n"),
                ));
            }
            Err(error) => body.push(DialogBlock::error(format!(
                "Couldn't check: {error}. Nothing was changed."
            ))),
        }
        let spec = DialogSpec::list("Which font draws it", body, Vec::new())
            .button(DialogButton::answer(
                RETRY,
                "Check other text…",
                ButtonRole::Neutral,
            ))
            .button(DialogButton::close());
        let request_id = self.dialog.show(spec);
        self.dialog_flow = Some(DialogFlow::FontCoverage(PendingFontCoverage {
            request_id,
            reported: Some(submitted),
        }));
    }
}

fn parse_font_coverage_query(value: &str) -> std::result::Result<String, String> {
    if value.is_empty() {
        return Err("enter literal text or an exact Unicode scalar".into());
    }
    if value.len() > MAX_FONT_COVERAGE_INPUT_BYTES {
        return Err(format!(
            "input must be no more than {MAX_FONT_COVERAGE_INPUT_BYTES} UTF-8 bytes"
        ));
    }
    if value.chars().any(char::is_control) {
        return Err("input must not contain control characters".into());
    }
    let exact = value.trim();
    if exact.starts_with("U+")
        || exact.starts_with("u+")
        || exact.starts_with("0x")
        || exact.starts_with("0X")
    {
        let scalar = super::font_ranges::parse_hex_scalar(exact)?;
        let character = char::from_u32(scalar)
            .ok_or_else(|| format!("U+{scalar:04X} is not a Unicode scalar"))?;
        if character.is_control() {
            return Err(format!("U+{scalar:04X} is a control character"));
        }
        return Ok(character.to_string());
    }
    Ok(value.to_string())
}

fn format_font_coverage_report(report: &FontCoverageReport) -> Vec<String> {
    let mut lines = vec![
        format!(
            "Extended graphemes examined: {}{}",
            report.entries.len(),
            if report.graphemes_truncated {
                format!(" (truncated at {MAX_FONT_COVERAGE_GRAPHEMES})")
            } else {
                String::new()
            }
        ),
        "Based on the installed fonts' character maps and a trial shaping in the regular style."
            .into(),
    ];
    for (index, entry) in report.entries.iter().enumerate() {
        lines.push(String::new());
        let codepoints = entry
            .codepoints
            .iter()
            .map(|codepoint| format_codepoint(*codepoint))
            .collect::<Vec<_>>()
            .join(" ");
        lines.push(format!(
            "{}. {} · {}",
            index + 1,
            format_grapheme(&entry.grapheme),
            codepoints
        ));
        for attempt in &entry.attempts {
            let status = if !attempt.installed {
                "missing"
            } else if attempt.complete {
                "installed, complete"
            } else {
                "installed, incomplete"
            };
            lines.push(format!(
                "   {} · {} · {status}",
                format_coverage_source(&attempt.source),
                attempt.family
            ));
        }
        let route = match &entry.route {
            FontCoverageRoute::Explicit { source, family } => {
                format!("   route: {} → {family}", format_coverage_source(source))
            }
            FontCoverageRoute::Platform {
                families,
                names_truncated,
            } => format!(
                "   route: platform fallback → {}{}",
                families.join(" + "),
                if *names_truncated {
                    " + additional faces"
                } else {
                    ""
                }
            ),
            FontCoverageRoute::Missing => {
                "   route: missing glyph (no explicit or platform shaping face)".into()
            }
            FontCoverageRoute::DefaultIgnorable { family } => format!(
                "   route: default-ignorable-only content retained with {family}; no visible glyph required"
            ),
        };
        lines.push(route);
    }
    if report.graphemes_truncated {
        lines.push(String::new());
        lines.push(format!(
            "Additional graphemes were omitted after the {MAX_FONT_COVERAGE_GRAPHEMES}-grapheme report limit."
        ));
    }
    lines
}

fn format_coverage_source(source: &FontCoverageSource) -> String {
    match source {
        FontCoverageSource::Range { start, end } => format!(
            "range {}–{}",
            format_codepoint(*start),
            format_codepoint(*end)
        ),
        FontCoverageSource::Primary => "primary".into(),
        FontCoverageSource::Fallback { index } => format!("fallback {index}"),
    }
}

fn format_grapheme(grapheme: &str) -> String {
    format!("{grapheme:?}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_query_parser_distinguishes_literal_text_from_exact_scalars() {
        assert_eq!(parse_font_coverage_query("abc").unwrap(), "abc");
        assert_eq!(parse_font_coverage_query("  ").unwrap(), "  ");
        assert_eq!(parse_font_coverage_query(" U+1F680 ").unwrap(), "🚀");
        assert_eq!(parse_font_coverage_query("0x0634").unwrap(), "ش");
        for invalid in ["", "U+D800", "U+110000", "U+000A", "bad\ninput"] {
            assert!(
                parse_font_coverage_query(invalid).is_err(),
                "accepted {invalid:?}"
            );
        }
        assert!(parse_font_coverage_query(&"a".repeat(MAX_FONT_COVERAGE_INPUT_BYTES + 1)).is_err());
    }

    #[test]
    fn coverage_report_formatter_names_attempt_state_route_and_truncation() {
        let report = FontCoverageReport {
            entries: vec![crate::renderer::FontCoverageEntry {
                grapheme: "شّ".into(),
                codepoints: vec![0x0634, 0x0651],
                attempts: vec![crate::renderer::FontCoverageAttempt {
                    source: FontCoverageSource::Primary,
                    family: "Missing Primary".into(),
                    installed: false,
                    complete: false,
                }],
                route: FontCoverageRoute::Platform {
                    families: vec!["Noto Sans Arabic".into()],
                    names_truncated: false,
                },
            }],
            graphemes_truncated: true,
        };
        let lines = format_font_coverage_report(&report);
        assert!(lines.iter().any(|line| line.contains("U+0634 U+0651")));
        assert!(
            lines
                .iter()
                .any(|line| line.contains("primary · Missing Primary · missing"))
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("platform fallback → Noto Sans Arabic"))
        );
        assert!(lines.iter().any(|line| line.contains("truncated at 16")));
    }

    #[test]
    fn the_check_is_read_only_correctable_and_repeatable_headlessly() {
        let mut app = App::new_headless(Config::default()).unwrap();
        let original = app.config.clone();
        app.apply_settings_action(crate::ui::settings::SettingsAction::CheckFontCoverage);
        let prompt = app.dialog.request_id();
        assert!(app.dialog.field(prompt).is_some());

        app.submit_dialog_field("U+D800");
        let field = app.dialog.field(app.dialog.request_id()).unwrap();
        assert_eq!(field.value, "U+D800");
        assert!(field.error.is_some());

        // Without a renderer the report says it couldn't check.
        app.submit_dialog_field("U+0634");
        assert!(app.dialog.field(app.dialog.request_id()).is_none());
        assert!(
            app.dialog
                .body()
                .iter()
                .any(|block| matches!(block, DialogBlock::Error(_)))
        );
        app.answer_dialog(RETRY);
        assert_eq!(
            app.dialog.field_value(app.dialog.request_id()),
            Some("U+0634")
        );
        app.answer_dialog(crate::ui::dialog::CANCEL);
        assert!(!app.dialog.is_awaiting_current());
        assert_eq!(
            toml::to_string(&app.config).unwrap(),
            toml::to_string(&original).unwrap()
        );
    }
}
