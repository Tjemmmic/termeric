//! Request-scoped Settings editor for Unicode-range font overrides.

use super::*;
use crate::config::{FontRangeOverride, MAX_FONT_FAMILY_BYTES, MAX_FONT_RANGE_OVERRIDES};
use crate::renderer::{
    FontCoverageReport, FontCoverageRoute, FontCoverageSource, MAX_FONT_COVERAGE_GRAPHEMES,
    MAX_FONT_COVERAGE_INPUT_BYTES,
};
use crate::ui::dialog::{
    ButtonRole, CANCEL, DialogBlock, DialogButton, DialogRow, DialogSpec, FieldSpec,
};

const RANGE_INPUT_BYTES: usize = MAX_FONT_FAMILY_BYTES + 32;
/// The answer that submits a prompt's field.
const SAVE: &str = "save";

#[derive(Debug, Clone)]
pub(super) struct PendingFontRangeManagement {
    pub(super) request_id: u64,
    view: FontRangeManagerView,
}

#[derive(Debug, Clone)]
enum FontRangeManagerView {
    /// The rules as listed, so a row answers for the one it showed.
    List {
        rules: Vec<FontRangeOverride>,
    },
    Detail {
        rule: FontRangeOverride,
    },
    Edit {
        original: Option<FontRangeOverride>,
    },
    InspectInput,
    CoverageReport {
        query: String,
    },
    ConfirmDelete {
        rule: FontRangeOverride,
    },
}

impl App {
    pub(super) fn open_font_range_manager(&mut self) {
        self.settings_panel.close();
        self.dialog_flow = None;
        self.show_font_range_list();
    }

    fn show_font_range_flow(&mut self, spec: DialogSpec, view: FontRangeManagerView) {
        let request_id = self.dialog.show(spec);
        self.dialog_flow = Some(DialogFlow::FontRanges(PendingFontRangeManagement {
            request_id,
            view,
        }));
    }

    fn show_font_range_list(&mut self) {
        let mut rules = self.config.font.range_overrides.clone();
        rules.sort_unstable_by(|left, right| {
            (left.start, left.end, left.family.to_ascii_lowercase()).cmp(&(
                right.start,
                right.end,
                right.family.to_ascii_lowercase(),
            ))
        });
        let mut body = vec![DialogBlock::text(
            "Characters in a range use its font when that font has the whole character. Changes apply at once; Save in Settings keeps them.",
        )];
        let full = rules.len() >= MAX_FONT_RANGE_OVERRIDES;
        if rules.is_empty() {
            body.push(DialogBlock::text("No font ranges are set."));
        } else if full {
            body.push(DialogBlock::text(format!(
                "That's the most there can be ({MAX_FONT_RANGE_OVERRIDES}). Delete one to add another."
            )));
        }
        let rows = rules
            .iter()
            .enumerate()
            .map(|(index, rule)| {
                DialogRow::new(format!("rule:{index}"), format_range(rule))
                    .with_detail(rule.family.as_str())
            })
            .collect();
        let mut spec = DialogSpec::list("Font ranges", body, rows)
            .button(DialogButton::back())
            .button(DialogButton::answer(
                "inspect",
                "Check text…",
                ButtonRole::Neutral,
            ));
        if !full {
            spec = spec.button(DialogButton::answer("add", "Add…", ButtonRole::Suggested));
        }
        self.show_font_range_flow(spec, FontRangeManagerView::List { rules });
    }

    fn show_font_range_detail(&mut self, rule: FontRangeOverride) {
        if !self.font_range_rule_is_current(&rule) {
            self.warn_stale_font_range();
            return;
        }
        let spec = DialogSpec::list(
            "Font range",
            vec![
                DialogBlock::value(format_range_rule(&rule)),
                DialogBlock::text(
                    "The main font still sets the cell size. If this font is missing or lacks a character, the next font is tried.",
                ),
            ],
            Vec::new(),
        )
        .button(DialogButton::back())
        .button(DialogButton::answer("delete", "Delete…", ButtonRole::Neutral))
        .button(DialogButton::answer("edit", "Edit…", ButtonRole::Suggested));
        self.show_font_range_flow(spec, FontRangeManagerView::Detail { rule });
    }

    fn show_font_range_edit(
        &mut self,
        original: Option<FontRangeOverride>,
        value: String,
        error: Option<&str>,
    ) {
        if let Some(rule) = original.as_ref()
            && !self.font_range_rule_is_current(rule)
        {
            self.warn_stale_font_range();
            return;
        }
        let (title, action) = if original.is_some() {
            ("Edit font range", "Save")
        } else {
            ("Add a font range", "Add")
        };
        let spec = DialogSpec::prompt(
            title,
            vec![
                DialogBlock::text(
                    "Write the first and last character in hexadecimal, then the font. U+ and 0x are both accepted.",
                ),
                DialogBlock::value("U+E000-U+F8FF | Symbols Nerd Font Mono"),
            ],
            FieldSpec::new("Range", value, RANGE_INPUT_BYTES)
                .with_error(error.map(crate::safe_text::capitalized)),
            SAVE,
            action,
        );
        self.show_font_range_flow(spec, FontRangeManagerView::Edit { original });
    }

    fn show_font_coverage_input(&mut self, value: String, error: Option<&str>) {
        let spec = DialogSpec::prompt(
            "Check which font draws text",
            vec![DialogBlock::text(format!(
                "Type the text, or one character as U+ or 0x hex. Up to {MAX_FONT_COVERAGE_GRAPHEMES} characters are checked, and nothing is changed."
            ))],
            FieldSpec::new("Text", value, MAX_FONT_COVERAGE_INPUT_BYTES)
                .with_error(error.map(crate::safe_text::capitalized)),
            SAVE,
            "Check",
        )
        .cancel_label("Back");
        self.show_font_range_flow(spec, FontRangeManagerView::InspectInput);
    }

    fn inspect_font_coverage(&mut self, submitted: String) {
        let query = match parse_font_coverage_query(&submitted) {
            Ok(query) => query,
            Err(error) => {
                self.show_font_coverage_input(submitted, Some(&error));
                return;
            }
        };
        let configured_primary = self.config.font.family.clone();
        let report = self
            .renderer
            .as_mut()
            .ok_or_else(|| "the renderer is not initialized in this window".to_string())
            .and_then(|renderer| renderer.inspect_font_coverage(&configured_primary, &query));
        self.show_font_coverage_report(submitted, &query, report);
    }

    fn show_font_coverage_report(
        &mut self,
        submitted: String,
        query: &str,
        report: std::result::Result<FontCoverageReport, String>,
    ) {
        let mut body = vec![DialogBlock::value(format_grapheme(query))];
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
            .button(DialogButton::back())
            .button(DialogButton::answer(
                "retry",
                "Check other text…",
                ButtonRole::Suggested,
            ));
        self.show_font_range_flow(
            spec,
            FontRangeManagerView::CoverageReport { query: submitted },
        );
    }

    fn show_font_range_delete_confirmation(&mut self, rule: FontRangeOverride) {
        if !self.font_range_rule_is_current(&rule) {
            self.warn_stale_font_range();
            return;
        }
        let spec = DialogSpec::confirm(
            "Delete this font range?",
            vec![
                DialogBlock::value(format_range_rule(&rule)),
                DialogBlock::text("It's gone at once; Save in Settings keeps the change."),
            ],
            "delete",
            "Delete",
            ButtonRole::Destructive,
        );
        self.show_font_range_flow(spec, FontRangeManagerView::ConfirmDelete { rule });
    }

    /// Answer the manager's dialog. Returns whether the answer was its.
    pub(super) fn handle_font_range_manager_choice(
        &mut self,
        request_id: u64,
        choice_id: &str,
    ) -> bool {
        let Some(pending) = self
            .pending_font_range_management()
            .filter(|pending| pending.request_id == request_id)
            .cloned()
        else {
            return false;
        };
        if choice_id == SAVE
            && matches!(
                pending.view,
                FontRangeManagerView::Edit { .. } | FontRangeManagerView::InspectInput
            )
        {
            self.submit_font_range_field(request_id, pending.view);
            return true;
        }
        self.dialog.dismiss(request_id);
        self.dialog_flow = None;
        match pending.view {
            FontRangeManagerView::List { rules } => match choice_id {
                CANCEL => self.open_settings_section(crate::ui::settings::SettingsSection::Font),
                "inspect" => self.show_font_coverage_input(String::new(), None),
                "add" if self.config.font.range_overrides.len() < MAX_FONT_RANGE_OVERRIDES => {
                    self.show_font_range_edit(None, String::new(), None);
                }
                choice if choice.starts_with("rule:") => {
                    let rule = choice["rule:".len()..]
                        .parse::<usize>()
                        .ok()
                        .and_then(|index| rules.get(index))
                        .cloned();
                    if let Some(rule) = rule.filter(|rule| self.font_range_rule_is_current(rule)) {
                        self.show_font_range_detail(rule);
                    } else {
                        self.warn_stale_font_range();
                    }
                }
                _ => self.show_font_range_list(),
            },
            FontRangeManagerView::Detail { rule } => match choice_id {
                "edit" if self.font_range_rule_is_current(&rule) => {
                    let value = format_range_expression(&rule);
                    self.show_font_range_edit(Some(rule), value, None);
                }
                "delete" if self.font_range_rule_is_current(&rule) => {
                    self.show_font_range_delete_confirmation(rule);
                }
                CANCEL => self.show_font_range_list(),
                _ => self.warn_stale_font_range(),
            },
            FontRangeManagerView::Edit { original } => self.return_from_font_range_edit(original),
            FontRangeManagerView::InspectInput => self.show_font_range_list(),
            FontRangeManagerView::CoverageReport { query } => match choice_id {
                CANCEL => self.show_font_range_list(),
                _ => self.show_font_coverage_input(query, None),
            },
            FontRangeManagerView::ConfirmDelete { rule } => match choice_id {
                "delete" => self.delete_font_range_rule(rule),
                CANCEL => self.show_font_range_detail(rule),
                _ => self.show_font_range_delete_confirmation(rule),
            },
        }
        true
    }

    /// Submit the field of the edit or inspect prompt.
    fn submit_font_range_field(&mut self, request_id: u64, view: FontRangeManagerView) {
        let Some(value) = self.dialog.field_value(request_id).map(ToOwned::to_owned) else {
            return;
        };
        self.dialog.dismiss(request_id);
        self.dialog_flow = None;
        let original = match view {
            FontRangeManagerView::Edit { original } => original,
            FontRangeManagerView::InspectInput => {
                self.inspect_font_coverage(value);
                return;
            }
            _ => unreachable!("only the prompts have a field"),
        };

        if let Some(rule) = original.as_ref()
            && !self.font_range_rule_is_current(rule)
        {
            self.warn_stale_font_range();
            return;
        }

        let parsed = match parse_font_range_expression(&value) {
            Ok(rule) => rule,
            Err(error) => {
                self.show_font_range_edit(original, value, Some(&error));
                return;
            }
        };
        let mut candidate = self.config.clone();
        match original.as_ref() {
            Some(original) => {
                let Some(index) = candidate
                    .font
                    .range_overrides
                    .iter()
                    .position(|rule| rule == original)
                else {
                    self.warn_stale_font_range();
                    return;
                };
                candidate.font.range_overrides[index] = parsed;
            }
            None if candidate.font.range_overrides.len() < MAX_FONT_RANGE_OVERRIDES => {
                candidate.font.range_overrides.push(parsed);
            }
            None => {
                self.show_font_range_edit(
                    None,
                    value,
                    Some(&format!(
                        "the most there can be ({MAX_FONT_RANGE_OVERRIDES}) are already set"
                    )),
                );
                return;
            }
        }

        if let Err(error) = candidate.validate() {
            self.show_font_range_edit(original, value, Some(&format!("{error:#}")));
            return;
        }
        if let Err(error) = self.apply_config(candidate) {
            self.show_font_range_edit(original, value, Some(&error));
            return;
        }
        self.toasts.success(if original.is_some() {
            "Font range updated"
        } else {
            "Font range added"
        });
        self.show_font_range_list();
    }

    fn return_from_font_range_edit(&mut self, original: Option<FontRangeOverride>) {
        if let Some(rule) = original.filter(|rule| self.font_range_rule_is_current(rule)) {
            self.show_font_range_detail(rule);
        } else {
            self.show_font_range_list();
        }
    }

    fn delete_font_range_rule(&mut self, rule: FontRangeOverride) {
        let mut candidate = self.config.clone();
        let Some(index) = candidate
            .font
            .range_overrides
            .iter()
            .position(|current| current == &rule)
        else {
            self.warn_stale_font_range();
            return;
        };
        candidate.font.range_overrides.remove(index);
        match self.apply_config(candidate) {
            Ok(()) => {
                self.toasts.success("Font range deleted");
                self.show_font_range_list();
            }
            Err(error) => {
                self.toasts
                    .error(format!("Font range could not be deleted: {error}"));
                self.show_font_range_detail(rule);
            }
        }
    }

    fn font_range_rule_is_current(&self, rule: &FontRangeOverride) -> bool {
        self.config
            .font
            .range_overrides
            .iter()
            .any(|current| current == rule)
    }

    fn warn_stale_font_range(&mut self) {
        self.toasts
            .warning("Font ranges changed; refreshed the list");
        self.show_font_range_list();
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
        let scalar = parse_hex_scalar(exact)?;
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

fn parse_font_range_expression(value: &str) -> std::result::Result<FontRangeOverride, String> {
    if value.len() > RANGE_INPUT_BYTES {
        return Err(format!(
            "rule must be no more than {RANGE_INPUT_BYTES} UTF-8 bytes"
        ));
    }
    if value.chars().any(char::is_control) {
        return Err("rule must not contain control characters".into());
    }
    let Some((range, family)) = value.split_once('|') else {
        return Err("expected U+START-U+END | FAMILY".into());
    };
    let Some((start, end)) = range.trim().split_once('-') else {
        return Err("expected an inclusive START-END range".into());
    };
    let start = parse_hex_scalar(start)?;
    let end = parse_hex_scalar(end)?;
    let family = family.trim().to_string();
    if family.is_empty() {
        return Err("font family must not be empty".into());
    }
    Ok(FontRangeOverride { start, end, family })
}

fn parse_hex_scalar(value: &str) -> std::result::Result<u32, String> {
    let value = value.trim();
    let digits = value
        .strip_prefix("U+")
        .or_else(|| value.strip_prefix("u+"))
        .or_else(|| value.strip_prefix("0x"))
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("invalid hexadecimal scalar {value:?}"));
    }
    u32::from_str_radix(digits, 16).map_err(|_| format!("scalar {value:?} is too large"))
}

fn format_codepoint(value: u32) -> String {
    format!("U+{value:04X}")
}

/// A rule's range: its first and last code points.
fn format_range(rule: &FontRangeOverride) -> String {
    format!(
        "{}–{}",
        format_codepoint(rule.start),
        format_codepoint(rule.end)
    )
}

fn format_range_rule(rule: &FontRangeOverride) -> String {
    format!(
        "{}–{} → {}",
        format_codepoint(rule.start),
        format_codepoint(rule.end),
        rule.family
    )
}

fn format_range_expression(rule: &FontRangeOverride) -> String {
    format!(
        "{}-{} | {}",
        format_codepoint(rule.start),
        format_codepoint(rule.end),
        rule.family
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(start: u32, end: u32, family: &str) -> FontRangeOverride {
        FontRangeOverride {
            start,
            end,
            family: family.into(),
        }
    }

    #[test]
    fn range_expression_parser_accepts_prefixes_and_formats_canonically() {
        let rule = parse_font_range_expression(" 0xe000 - U+1faff | Noto Color Emoji ").unwrap();
        assert_eq!(
            rule,
            FontRangeOverride {
                start: 0xE000,
                end: 0x1FAFF,
                family: "Noto Color Emoji".into(),
            }
        );
        assert_eq!(
            format_range_expression(&rule),
            "U+E000-U+1FAFF | Noto Color Emoji"
        );
        assert_eq!(format_codepoint(0x41), "U+0041");
    }

    #[test]
    fn range_expression_parser_rejects_ambiguous_or_unsafe_syntax() {
        for invalid in [
            "E000-F8FF Symbols",
            "E000 | Symbols",
            "E000-F8FF | ",
            "E000-G000 | Symbols",
            "E000-F8FF | Bad\nFamily",
        ] {
            assert!(
                parse_font_range_expression(invalid).is_err(),
                "accepted {invalid:?}"
            );
        }
        assert_eq!(
            parse_font_range_expression("E000-F8FF | Symbols | Alternate")
                .unwrap()
                .family,
            "Symbols | Alternate"
        );
    }

    #[test]
    fn parsed_semantic_scalar_errors_are_rejected_by_shared_config_validation() {
        for expression in [
            "U+D800-U+DFFF | Surrogate",
            "U+110000-U+110000 | Too High",
            "U+F900-U+E000 | Reversed",
        ] {
            let mut config = Config::default();
            config
                .font
                .range_overrides
                .push(parse_font_range_expression(expression).unwrap());
            assert!(config.validate().is_err(), "accepted {expression:?}");
        }
    }

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
        assert!(
            lines
                .last()
                .is_some_and(|line| line.starts_with("Additional graphemes were omitted"))
        );
    }

    #[test]
    fn coverage_inspector_flow_is_read_only_correctable_and_reversible_headlessly() {
        let mut app = App::new_headless(Config::default()).unwrap();
        let original = app.config.font.range_overrides.clone();
        app.open_font_range_manager();
        app.answer_dialog("inspect");
        assert_eq!(app.dialog.title(), "Check which font draws text");

        app.submit_dialog_field("U+D800");
        assert_eq!(app.dialog.title(), "Check which font draws text");
        let field = app.dialog.field(app.dialog.request_id()).unwrap();
        assert_eq!(field.value, "U+D800");
        assert!(field.error.is_some());

        app.submit_dialog_field("U+0634");
        assert_eq!(app.dialog.title(), "Which font draws it");
        assert!(
            app.dialog
                .body()
                .iter()
                .any(|block| matches!(block, crate::ui::dialog::DialogBlock::Error(_)))
        );
        assert_eq!(app.config.font.range_overrides, original);

        app.answer_dialog("retry");
        assert_eq!(app.dialog.title(), "Check which font draws text");
        assert_eq!(
            app.dialog.field_value(app.dialog.request_id()),
            Some("U+0634")
        );
        app.answer_dialog(CANCEL);
        assert_eq!(app.dialog.title(), "Font ranges");
        assert_eq!(app.config.font.range_overrides, original);
    }

    #[test]
    fn manager_adds_edits_and_confirm_deletes_atomically() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.apply_settings_action(SettingsAction::ManageFontRangeOverrides);
        assert_eq!(app.dialog.title(), "Font ranges");
        assert!(app.dialog.answers(app.dialog.request_id(), "add"));

        app.answer_dialog("add");
        assert_eq!(app.dialog.title(), "Add a font range");
        app.submit_dialog_field("0xe000-U+f8ff | Symbols Nerd Font Mono");
        assert_eq!(
            app.config.font.range_overrides,
            vec![rule(0xE000, 0xF8FF, "Symbols Nerd Font Mono")]
        );
        assert_eq!(app.dialog.title(), "Font ranges");

        app.answer_dialog("rule:0");
        assert_eq!(app.dialog.title(), "Font range");
        app.answer_dialog("edit");
        assert_eq!(
            app.dialog.field_value(app.dialog.request_id()),
            Some("U+E000-U+F8FF | Symbols Nerd Font Mono")
        );
        app.submit_dialog_field("U+E100-U+E1FF | Portable Symbols");
        assert_eq!(
            app.config.font.range_overrides,
            vec![rule(0xE100, 0xE1FF, "Portable Symbols")]
        );

        app.answer_dialog("rule:0");
        app.answer_dialog("delete");
        assert_eq!(app.dialog.title(), "Delete this font range?");
        assert_eq!(app.config.font.range_overrides.len(), 1);
        app.answer_dialog("delete");
        assert!(app.config.font.range_overrides.is_empty());
        assert_eq!(app.dialog.title(), "Font ranges");
    }

    #[test]
    fn invalid_edit_retains_input_and_live_config_for_correction() {
        let mut config = Config::default();
        config.font.range_overrides = vec![rule(0xE000, 0xE0FF, "Existing")];
        let mut app = App::new_headless(config).unwrap();
        app.open_font_range_manager();
        app.answer_dialog("add");
        let submitted = "U+E080-U+E1FF | Overlap";
        app.submit_dialog_field(submitted);

        assert_eq!(
            app.config.font.range_overrides,
            vec![rule(0xE000, 0xE0FF, "Existing")]
        );
        assert_eq!(app.dialog.title(), "Add a font range");
        let field = app.dialog.field(app.dialog.request_id()).unwrap();
        assert_eq!(field.value, submitted);
        assert!(field.error.is_some());

        app.submit_dialog_field("U+E100-U+E1FF | Adjacent");
        assert_eq!(app.config.font.range_overrides.len(), 2);
    }

    #[test]
    fn stale_identity_and_request_cannot_retarget_a_reloaded_rule() {
        use crate::ui::dialog::DialogAction;
        let mut config = Config::default();
        config.font.range_overrides = vec![rule(0xE000, 0xE0FF, "Original")];
        let mut app = App::new_headless(config).unwrap();
        app.open_font_range_manager();
        let stale_request = app.dialog.request_id();

        let mut reloaded = app.config.clone();
        reloaded.font.range_overrides = vec![rule(0xF000, 0xF0FF, "Reloaded")];
        app.apply_config(reloaded.clone()).unwrap();
        app.apply_dialog_action(DialogAction::Answer {
            request_id: stale_request,
            id: "rule:0".into(),
        });
        assert_eq!(
            app.config.font.range_overrides,
            reloaded.font.range_overrides
        );
        assert_eq!(app.dialog.title(), "Font ranges");
        assert_eq!(app.dialog.rows()[0].label, "U+F000–U+F0FF");

        app.answer_dialog("rule:0");
        app.answer_dialog("delete");
        let stale_delete = app.dialog.request_id();
        let mut replaced_again = app.config.clone();
        replaced_again.font.range_overrides = vec![rule(0x1000, 0x10FF, "Newest")];
        app.apply_config(replaced_again.clone()).unwrap();
        app.apply_dialog_action(DialogAction::Answer {
            request_id: stale_delete,
            id: "delete".into(),
        });
        assert_eq!(
            app.config.font.range_overrides,
            replaced_again.font.range_overrides
        );

        let retired_request = app.dialog.request_id();
        let replacement = app.dialog.show(crate::ui::dialog::DialogSpec::message(
            "Replacement",
            vec![],
        ));
        assert_ne!(retired_request, replacement);
        app.apply_dialog_action(DialogAction::Answer {
            request_id: retired_request,
            id: "rule:0".into(),
        });
        assert_eq!(
            app.config.font.range_overrides,
            replaced_again.font.range_overrides
        );
        assert_eq!(app.dialog.title(), "Replacement");
        app.apply_dialog_action(DialogAction::Close {
            request_id: replacement,
        });
        assert!(app.pending_font_range_management().is_none());
    }

    #[test]
    fn manager_lists_every_rule_at_the_limit_and_cancel_returns_without_mutation() {
        let mut config = Config::default();
        config.font.range_overrides = (0..MAX_FONT_RANGE_OVERRIDES)
            .map(|value| rule(value as u32, value as u32, &format!("Family {value}")))
            .collect();
        config.validate().unwrap();
        let original = config.font.range_overrides.clone();
        let mut app = App::new_headless(config).unwrap();
        app.open_font_range_manager();
        assert!(!app.dialog.answers(app.dialog.request_id(), "add"));
        assert_eq!(app.dialog.rows().len(), MAX_FONT_RANGE_OVERRIDES);

        app.answer_dialog(&format!("rule:{}", MAX_FONT_RANGE_OVERRIDES - 1));
        app.answer_dialog("edit");
        app.answer_dialog(CANCEL);
        assert_eq!(app.config.font.range_overrides, original);
        assert_eq!(app.dialog.title(), "Font range");
    }
}
