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
        let body = match report {
            Ok(report) => coverage_report(&report),
            Err(error) => vec![DialogBlock::error(format!(
                "Couldn't check: {error}. Nothing was changed."
            ))],
        };
        let spec = DialogSpec::report("Which font draws it", body)
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

/// The report as the dialog reads it: for each character, its code points
/// as a value, then in plain words which font draws it and which fonts were
/// tried.
fn coverage_report(report: &FontCoverageReport) -> Vec<DialogBlock> {
    let mut body = vec![DialogBlock::text(
        "From the installed fonts' character maps and a trial shaping in the regular style.",
    )];
    for entry in &report.entries {
        let codepoints = entry
            .codepoints
            .iter()
            .map(|codepoint| format_codepoint(*codepoint))
            .collect::<Vec<_>>()
            .join(" ");
        body.push(DialogBlock::value(format!(
            "{}  {codepoints}",
            format_grapheme(&entry.grapheme)
        )));
        let mut lines = vec![match &entry.route {
            FontCoverageRoute::Explicit { source, family } => {
                format!("Drawn with {family}, {}.", source_name(source))
            }
            FontCoverageRoute::Platform {
                families,
                names_truncated,
            } => format!(
                "Drawn with {}{}, from the system's fallback fonts.",
                families.join(" and "),
                if *names_truncated { " and others" } else { "" }
            ),
            FontCoverageRoute::Missing => "No installed font draws it.".into(),
            FontCoverageRoute::DefaultIgnorable { family } => {
                format!("Nothing to draw: it's invisible formatting, kept with {family}.")
            }
        }];
        lines.extend(entry.attempts.iter().map(|attempt| {
            let state = if !attempt.installed {
                "isn't installed"
            } else if attempt.complete {
                "has it"
            } else {
                "lacks part of it"
            };
            format!(
                "Tried {family}, {source}: {state}.",
                family = attempt.family,
                source = source_name(&attempt.source)
            )
        }));
        body.push(DialogBlock::text(lines.join("\n")));
    }
    if report.graphemes_truncated {
        body.push(DialogBlock::text(format!(
            "Only the first {MAX_FONT_COVERAGE_GRAPHEMES} characters were checked."
        )));
    }
    body
}

/// Where a font sits in the configuration, as the report names it.
fn source_name(source: &FontCoverageSource) -> String {
    match source {
        FontCoverageSource::Range { start, end } => format!(
            "the font for {}–{}",
            format_codepoint(*start),
            format_codepoint(*end)
        ),
        FontCoverageSource::Primary => "the main font".into(),
        FontCoverageSource::Fallback { index } => format!("fallback {index}"),
    }
}

/// A character as the report shows it: quoted, with anything invisible
/// escaped.
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
    fn the_report_gives_each_character_its_code_points_and_every_font_tried() {
        let attempt =
            |source, family: &str, installed, complete| crate::renderer::FontCoverageAttempt {
                source,
                family: family.into(),
                installed,
                complete,
            };
        let report = FontCoverageReport {
            entries: vec![
                crate::renderer::FontCoverageEntry {
                    grapheme: "شّ".into(),
                    codepoints: vec![0x0634, 0x0651],
                    attempts: vec![attempt(
                        FontCoverageSource::Primary,
                        "Missing Primary",
                        false,
                        false,
                    )],
                    route: FontCoverageRoute::Platform {
                        families: vec!["Noto Sans Arabic".into()],
                        names_truncated: false,
                    },
                },
                crate::renderer::FontCoverageEntry {
                    grapheme: "\u{E0A0}".into(),
                    codepoints: vec![0xE0A0],
                    attempts: vec![attempt(
                        FontCoverageSource::Range {
                            start: 0xE000,
                            end: 0xF8FF,
                        },
                        "Symbols",
                        true,
                        true,
                    )],
                    route: FontCoverageRoute::Explicit {
                        source: FontCoverageSource::Range {
                            start: 0xE000,
                            end: 0xF8FF,
                        },
                        family: "Symbols".into(),
                    },
                },
            ],
            graphemes_truncated: true,
        };
        let body = coverage_report(&report);
        let values: Vec<_> = body
            .iter()
            .filter_map(|block| match block {
                DialogBlock::Value(value) => Some(value.as_str()),
                _ => None,
            })
            .collect();
        // One value per character, holding its code points; the words
        // around them are in the UI face.
        assert_eq!(values.len(), 2);
        assert!(values[0].ends_with("U+0634 U+0651"));
        assert!(values[1].ends_with("U+E0A0"));
        let text = |index: usize| match &body[index] {
            DialogBlock::Text(text) => text.clone(),
            other => panic!("block {index} is {other:?}"),
        };
        assert!(text(2).contains("Noto Sans Arabic") && text(2).contains("Missing Primary"));
        assert!(text(4).contains("Symbols") && text(4).contains("U+E000–U+F8FF"));
        assert!(matches!(body.last(), Some(DialogBlock::Text(_))));
        assert_eq!(
            body.len(),
            6,
            "an opening note, two per character, and the cut"
        );
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
