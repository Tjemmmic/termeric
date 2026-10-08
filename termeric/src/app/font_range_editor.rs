//! Request-scoped Settings editor for Unicode-range font overrides.

use super::*;
use crate::config::{FontRangeOverride, MAX_FONT_FAMILY_BYTES, MAX_FONT_RANGE_OVERRIDES};
use crate::renderer::{
    FontCoverageReport, FontCoverageRoute, FontCoverageSource, MAX_FONT_COVERAGE_GRAPHEMES,
    MAX_FONT_COVERAGE_INPUT_BYTES,
};
use crate::ui::info_overlay::{
    InfoOverlayChoice, MAX_INFO_INPUT_BYTES, OverlayPage, push_key_choice,
};

const RANGE_PAGE_SIZE: usize = 8;
const RANGE_INPUT_BYTES: usize = MAX_FONT_FAMILY_BYTES + 32;
const RANGE_SHORTCUTS: [char; RANGE_PAGE_SIZE] = ['1', '2', '3', '4', '5', '6', '7', '8'];

#[derive(Debug, Clone)]
pub(super) struct PendingFontRangeManagement {
    pub(super) request_id: u64,
    view: FontRangeManagerView,
}

#[derive(Debug, Clone)]
enum FontRangeManagerView {
    List {
        page: usize,
        visible_rules: Vec<FontRangeOverride>,
    },
    Detail {
        page: usize,
        rule: FontRangeOverride,
    },
    Edit {
        page: usize,
        original: Option<FontRangeOverride>,
    },
    InspectInput {
        page: usize,
    },
    CoverageReport {
        page: usize,
        query: String,
    },
    ConfirmDelete {
        page: usize,
        rule: FontRangeOverride,
    },
}

impl App {
    pub(super) fn open_font_range_manager(&mut self) {
        self.settings_panel.close();
        self.overlay_flow = None;
        self.show_font_range_list(0);
    }

    fn show_font_range_list(&mut self, requested_page: usize) {
        let mut rules = self.config.font.range_overrides.clone();
        rules.sort_unstable_by(|left, right| {
            (left.start, left.end, left.family.to_ascii_lowercase()).cmp(&(
                right.start,
                right.end,
                right.family.to_ascii_lowercase(),
            ))
        });
        let OverlayPage {
            index: page,
            count: page_count,
            start,
            end,
        } = OverlayPage::of(rules.len(), RANGE_PAGE_SIZE, requested_page);
        let visible_rules = rules[start..end].to_vec();

        let mut lines = vec![
            format!(
                "Configured overrides: {}/{} · page {}/{}",
                rules.len(),
                MAX_FONT_RANGE_OVERRIDES,
                page + 1,
                page_count
            ),
            "A matching family is preferred only when it covers the complete grapheme.".into(),
            "Changes apply live; use Save in Settings to persist them.".into(),
            String::new(),
        ];
        let mut choices = Vec::new();
        push_key_choice(
            &mut lines,
            &mut choices,
            "inspect",
            'i',
            "Inspect glyph coverage",
        );
        if rules.len() < MAX_FONT_RANGE_OVERRIDES {
            push_key_choice(&mut lines, &mut choices, "add", 'a', "Add override");
        } else {
            lines.push(format!(
                "Add unavailable: the maximum of {MAX_FONT_RANGE_OVERRIDES} rules is configured."
            ));
        }
        lines.push(String::new());

        for (index, rule) in visible_rules.iter().enumerate() {
            let label = format_range_rule(rule);
            let shortcut = RANGE_SHORTCUTS[index];
            push_key_choice(
                &mut lines,
                &mut choices,
                format!("rule:{index}"),
                shortcut,
                label,
            );
        }
        if rules.is_empty() {
            lines.push("No Unicode-range font overrides are configured.".into());
        }
        if page > 0 {
            push_key_choice(
                &mut lines,
                &mut choices,
                "previous-page",
                'p',
                "Previous page",
            );
        }
        if page + 1 < page_count {
            push_key_choice(&mut lines, &mut choices, "next-page", 'n', "Next page");
        }
        push_key_choice(&mut lines, &mut choices, "back", 'b', "Back to Settings");

        let request_id = self
            .info_overlay
            .show_choices("Font Range Overrides", lines, choices);
        self.overlay_flow = Some(OverlayFlow::FontRanges(PendingFontRangeManagement {
            request_id,
            view: FontRangeManagerView::List {
                page,
                visible_rules,
            },
        }));
    }

    fn show_font_range_detail(&mut self, page: usize, rule: FontRangeOverride) {
        if !self.font_range_rule_is_current(&rule) {
            self.warn_stale_font_range(page);
            return;
        }
        let mut lines = vec![
            format_range_rule(&rule),
            String::new(),
            "The primary font still owns cell metrics. A missing or incomplete mapped family falls through safely.".into(),
            String::new(),
        ];
        let mut choices = Vec::new();
        push_key_choice(&mut lines, &mut choices, "edit", 'e', "Edit override");
        push_key_choice(&mut lines, &mut choices, "delete", 'd', "Delete override");
        push_key_choice(&mut lines, &mut choices, "back", 'b', "Back to list");
        let request_id = self
            .info_overlay
            .show_choices("Font Range Override", lines, choices);
        self.overlay_flow = Some(OverlayFlow::FontRanges(PendingFontRangeManagement {
            request_id,
            view: FontRangeManagerView::Detail { page, rule },
        }));
    }

    fn show_font_range_edit(
        &mut self,
        page: usize,
        original: Option<FontRangeOverride>,
        value: String,
        error: Option<&str>,
    ) {
        if let Some(rule) = original.as_ref()
            && !self.font_range_rule_is_current(rule)
        {
            self.warn_stale_font_range(page);
            return;
        }
        let mut lines = vec![
            "Enter one inclusive range and family using:".into(),
            "U+START-U+END | FAMILY".into(),
            "Bounds are hexadecimal; U+ and 0x prefixes are accepted.".into(),
        ];
        if let Some(error) = error {
            lines.push(String::new());
            lines.push(format!("Validation error: {error}"));
        }
        lines.push(String::new());
        let input_line = lines.len();
        lines.push(String::new());
        lines.push("Enter applies · Escape cancels".into());
        let cancel_line = lines.len();
        lines.push("[Esc] Cancel".into());
        let title = if original.is_some() {
            "Edit Font Range Override"
        } else {
            "Add Font Range Override"
        };
        let request_id = self.info_overlay.show_input(
            title,
            lines,
            input_line,
            "Rule: ",
            value,
            RANGE_INPUT_BYTES.min(MAX_INFO_INPUT_BYTES),
            vec![InfoOverlayChoice::new(
                "cancel",
                "Cancel edit (Escape)",
                cancel_line,
                None,
            )],
        );
        self.overlay_flow = Some(OverlayFlow::FontRanges(PendingFontRangeManagement {
            request_id,
            view: FontRangeManagerView::Edit { page, original },
        }));
    }

    fn show_font_coverage_input(&mut self, page: usize, value: String, error: Option<&str>) {
        let mut lines = vec![
            "Enter literal text or one exact U+ / 0x Unicode scalar.".into(),
            format!(
                "The report is read-only and examines at most {MAX_FONT_COVERAGE_GRAPHEMES} complete graphemes."
            ),
            "The report shows which font draws each character, not how it looks on screen.".into(),
        ];
        if let Some(error) = error {
            lines.push(String::new());
            lines.push(format!("Validation error: {error}"));
        }
        lines.push(String::new());
        let input_line = lines.len();
        lines.push(String::new());
        lines.push("Enter inspects · Escape returns".into());
        let cancel_line = lines.len();
        lines.push("[Esc] Back to range manager".into());
        let request_id = self.info_overlay.show_input(
            "Inspect Glyph Coverage",
            lines,
            input_line,
            "Text: ",
            value,
            MAX_FONT_COVERAGE_INPUT_BYTES.min(MAX_INFO_INPUT_BYTES),
            vec![InfoOverlayChoice::new(
                "cancel",
                "Back to range manager (Escape)",
                cancel_line,
                None,
            )],
        );
        self.overlay_flow = Some(OverlayFlow::FontRanges(PendingFontRangeManagement {
            request_id,
            view: FontRangeManagerView::InspectInput { page },
        }));
    }

    fn inspect_font_coverage(&mut self, page: usize, submitted: String) {
        let query = match parse_font_coverage_query(&submitted) {
            Ok(query) => query,
            Err(error) => {
                self.show_font_coverage_input(page, submitted, Some(&error));
                return;
            }
        };
        let configured_primary = self.config.font.family.clone();
        let report = self
            .renderer
            .as_mut()
            .ok_or_else(|| "the renderer is not initialized in this window".to_string())
            .and_then(|renderer| renderer.inspect_font_coverage(&configured_primary, &query));
        self.show_font_coverage_report(page, submitted, &query, report);
    }

    fn show_font_coverage_report(
        &mut self,
        page: usize,
        submitted: String,
        query: &str,
        report: std::result::Result<FontCoverageReport, String>,
    ) {
        let mut lines = match report {
            Ok(report) => format_font_coverage_report(&report),
            Err(error) => vec![
                format!("Coverage inspection unavailable: {error}"),
                String::new(),
                "No configuration, terminal text, cache, or renderer policy was changed.".into(),
            ],
        };
        lines.insert(0, format!("Query: {}", format_grapheme(query)));
        lines.insert(1, String::new());
        lines.push(String::new());
        let mut choices = Vec::new();
        push_key_choice(
            &mut lines,
            &mut choices,
            "retry",
            'r',
            "Retry or inspect different text",
        );
        push_key_choice(
            &mut lines,
            &mut choices,
            "back",
            'b',
            "Back to range manager",
        );
        let request_id = self
            .info_overlay
            .show_choices("Glyph Coverage Report", lines, choices);
        self.overlay_flow = Some(OverlayFlow::FontRanges(PendingFontRangeManagement {
            request_id,
            view: FontRangeManagerView::CoverageReport {
                page,
                query: submitted,
            },
        }));
    }

    fn show_font_range_delete_confirmation(&mut self, page: usize, rule: FontRangeOverride) {
        if !self.font_range_rule_is_current(&rule) {
            self.warn_stale_font_range(page);
            return;
        }
        let mut lines = vec![
            "Delete this font preference?".into(),
            format_range_rule(&rule),
            String::new(),
            "The change applies live and is persisted only when Settings is saved.".into(),
            String::new(),
        ];
        let mut choices = Vec::new();
        push_key_choice(&mut lines, &mut choices, "confirm", 'd', "Delete override");
        push_key_choice(&mut lines, &mut choices, "cancel", 'b', "Keep override");
        let request_id =
            self.info_overlay
                .show_choices("Delete Font Range Override", lines, choices);
        self.overlay_flow = Some(OverlayFlow::FontRanges(PendingFontRangeManagement {
            request_id,
            view: FontRangeManagerView::ConfirmDelete { page, rule },
        }));
    }

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
        self.info_overlay.dismiss(request_id);
        self.overlay_flow = None;
        match pending.view {
            FontRangeManagerView::List {
                page,
                visible_rules,
            } => match choice_id {
                "inspect" => self.show_font_coverage_input(page, String::new(), None),
                "add" if self.config.font.range_overrides.len() < MAX_FONT_RANGE_OVERRIDES => {
                    self.show_font_range_edit(page, None, String::new(), None);
                }
                "previous-page" => self.show_font_range_list(page.saturating_sub(1)),
                "next-page" => self.show_font_range_list(page.saturating_add(1)),
                "back" => {
                    self.open_settings_section(crate::ui::settings::SettingsSection::Font);
                }
                choice if choice.starts_with("rule:") => {
                    let rule = choice[5..]
                        .parse::<usize>()
                        .ok()
                        .and_then(|index| visible_rules.get(index))
                        .cloned();
                    if let Some(rule) = rule.filter(|rule| self.font_range_rule_is_current(rule)) {
                        self.show_font_range_detail(page, rule);
                    } else {
                        self.warn_stale_font_range(page);
                    }
                }
                _ => self.show_font_range_list(page),
            },
            FontRangeManagerView::Detail { page, rule } => match choice_id {
                "edit" if self.font_range_rule_is_current(&rule) => {
                    let value = format_range_expression(&rule);
                    self.show_font_range_edit(page, Some(rule), value, None);
                }
                "delete" if self.font_range_rule_is_current(&rule) => {
                    self.show_font_range_delete_confirmation(page, rule);
                }
                "back" => self.show_font_range_list(page),
                _ => self.warn_stale_font_range(page),
            },
            FontRangeManagerView::Edit { page, original } => {
                if choice_id == "cancel" {
                    self.return_from_font_range_edit(page, original);
                } else {
                    self.show_font_range_edit(page, original, String::new(), None);
                }
            }
            FontRangeManagerView::InspectInput { page } => {
                if choice_id == "cancel" {
                    self.show_font_range_list(page);
                } else {
                    self.show_font_coverage_input(page, String::new(), None);
                }
            }
            FontRangeManagerView::CoverageReport { page, query } => match choice_id {
                "retry" => self.show_font_coverage_input(page, query, None),
                "back" => self.show_font_range_list(page),
                _ => self.show_font_coverage_input(page, query, None),
            },
            FontRangeManagerView::ConfirmDelete { page, rule } => match choice_id {
                "confirm" => self.delete_font_range_rule(page, rule),
                "cancel" | "back" => self.show_font_range_detail(page, rule),
                _ => self.show_font_range_delete_confirmation(page, rule),
            },
        }
        true
    }

    pub(super) fn submit_font_range_manager_input(&mut self, request_id: u64) -> bool {
        let Some(pending) = self
            .pending_font_range_management()
            .filter(|pending| pending.request_id == request_id)
            .cloned()
        else {
            return false;
        };
        if !matches!(
            pending.view,
            FontRangeManagerView::Edit { .. } | FontRangeManagerView::InspectInput { .. }
        ) {
            return false;
        }
        let Some(value) = self
            .info_overlay
            .input_value(request_id)
            .map(ToOwned::to_owned)
        else {
            return false;
        };
        self.info_overlay.dismiss(request_id);
        self.overlay_flow = None;

        let (page, original) = match pending.view {
            FontRangeManagerView::Edit { page, original } => (page, original),
            FontRangeManagerView::InspectInput { page } => {
                self.inspect_font_coverage(page, value);
                return true;
            }
            _ => unreachable!("input view checked above"),
        };

        if let Some(rule) = original.as_ref()
            && !self.font_range_rule_is_current(rule)
        {
            self.warn_stale_font_range(page);
            return true;
        }

        let parsed = match parse_font_range_expression(&value) {
            Ok(rule) => rule,
            Err(error) => {
                self.show_font_range_edit(page, original, value, Some(&error));
                return true;
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
                    self.warn_stale_font_range(page);
                    return true;
                };
                candidate.font.range_overrides[index] = parsed;
            }
            None if candidate.font.range_overrides.len() < MAX_FONT_RANGE_OVERRIDES => {
                candidate.font.range_overrides.push(parsed);
            }
            None => {
                self.show_font_range_edit(
                    page,
                    None,
                    value,
                    Some(&format!(
                        "the maximum of {MAX_FONT_RANGE_OVERRIDES} rules is already configured"
                    )),
                );
                return true;
            }
        }

        if let Err(error) = candidate.validate() {
            self.show_font_range_edit(page, original, value, Some(&format!("{error:#}")));
            return true;
        }
        if let Err(error) = self.apply_config(candidate) {
            self.show_font_range_edit(page, original, value, Some(&error));
            return true;
        }
        self.toasts.success(if original.is_some() {
            "Font range override updated"
        } else {
            "Font range override added"
        });
        self.show_font_range_list(page);
        true
    }

    pub(super) fn cancel_font_range_manager_input(&mut self, request_id: u64) -> bool {
        let Some(pending) = self
            .pending_font_range_management()
            .filter(|pending| pending.request_id == request_id)
            .cloned()
        else {
            return false;
        };
        let destination = match pending.view {
            FontRangeManagerView::Edit { page, original } => Some((page, original)),
            FontRangeManagerView::InspectInput { page } => {
                self.info_overlay.dismiss(request_id);
                self.overlay_flow = None;
                self.show_font_range_list(page);
                return true;
            }
            _ => None,
        };
        let Some((page, original)) = destination else {
            return false;
        };
        self.info_overlay.dismiss(request_id);
        self.overlay_flow = None;
        self.return_from_font_range_edit(page, original);
        true
    }

    fn return_from_font_range_edit(&mut self, page: usize, original: Option<FontRangeOverride>) {
        if let Some(rule) = original.filter(|rule| self.font_range_rule_is_current(rule)) {
            self.show_font_range_detail(page, rule);
        } else {
            self.show_font_range_list(page);
        }
    }

    fn delete_font_range_rule(&mut self, page: usize, rule: FontRangeOverride) {
        let mut candidate = self.config.clone();
        let Some(index) = candidate
            .font
            .range_overrides
            .iter()
            .position(|current| current == &rule)
        else {
            self.warn_stale_font_range(page);
            return;
        };
        candidate.font.range_overrides.remove(index);
        match self.apply_config(candidate) {
            Ok(()) => {
                self.toasts.success("Font range override deleted");
                self.show_font_range_list(page);
            }
            Err(error) => {
                self.toasts
                    .error(format!("Font range override could not be deleted: {error}"));
                self.show_font_range_detail(page, rule);
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

    fn warn_stale_font_range(&mut self, page: usize) {
        self.toasts
            .warning("Font range overrides changed; refreshed the current list");
        self.show_font_range_list(page);
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
    use crate::ui::info_overlay::InfoOverlayAction;

    fn rule(start: u32, end: u32, family: &str) -> FontRangeOverride {
        FontRangeOverride {
            start,
            end,
            family: family.into(),
        }
    }

    fn choose(app: &mut App, choice_id: &str) {
        let request_id = app.info_overlay.request_id();
        app.apply_info_overlay_action(InfoOverlayAction::Choose {
            request_id,
            choice_id: choice_id.into(),
        });
    }

    fn submit(app: &mut App, value: &str) {
        let request_id = app.info_overlay.request_id();
        assert!(app.info_overlay.replace_input_text(request_id, value));
        app.apply_info_overlay_action(InfoOverlayAction::SubmitInput { request_id });
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
        assert!(
            app.info_overlay
                .choice_for_shortcut(app.info_overlay.request_id(), 'i')
                .is_some()
        );
        choose(&mut app, "inspect");
        assert_eq!(app.info_overlay.title(), "Inspect Glyph Coverage");

        submit(&mut app, "U+D800");
        assert_eq!(app.info_overlay.title(), "Inspect Glyph Coverage");
        assert_eq!(
            app.info_overlay.input_value(app.info_overlay.request_id()),
            Some("U+D800")
        );
        assert!(
            app.info_overlay
                .lines()
                .iter()
                .any(|line| line.contains("not a Unicode scalar"))
        );

        submit(&mut app, "U+0634");
        assert_eq!(app.info_overlay.title(), "Glyph Coverage Report");
        assert!(
            app.info_overlay
                .lines()
                .iter()
                .any(|line| line.contains("renderer is not initialized"))
        );
        assert_eq!(app.config.font.range_overrides, original);

        choose(&mut app, "retry");
        assert_eq!(app.info_overlay.title(), "Inspect Glyph Coverage");
        assert_eq!(
            app.info_overlay.input_value(app.info_overlay.request_id()),
            Some("U+0634")
        );
        let request_id = app.info_overlay.request_id();
        app.apply_info_overlay_action(InfoOverlayAction::CancelInput { request_id });
        assert_eq!(app.info_overlay.title(), "Font Range Overrides");
        assert_eq!(app.config.font.range_overrides, original);
    }

    #[test]
    fn manager_adds_edits_and_confirm_deletes_atomically() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.apply_settings_action(SettingsAction::ManageFontRangeOverrides);
        assert_eq!(app.info_overlay.title(), "Font Range Overrides");
        assert!(
            app.info_overlay
                .choice_for_shortcut(app.info_overlay.request_id(), 'a')
                .is_some()
        );

        choose(&mut app, "add");
        assert_eq!(app.info_overlay.title(), "Add Font Range Override");
        submit(&mut app, "0xe000-U+f8ff | Symbols Nerd Font Mono");
        assert_eq!(
            app.config.font.range_overrides,
            vec![rule(0xE000, 0xF8FF, "Symbols Nerd Font Mono")]
        );
        assert_eq!(app.info_overlay.title(), "Font Range Overrides");

        choose(&mut app, "rule:0");
        assert_eq!(app.info_overlay.title(), "Font Range Override");
        choose(&mut app, "edit");
        assert_eq!(
            app.info_overlay.input_value(app.info_overlay.request_id()),
            Some("U+E000-U+F8FF | Symbols Nerd Font Mono")
        );
        submit(&mut app, "U+E100-U+E1FF | Portable Symbols");
        assert_eq!(
            app.config.font.range_overrides,
            vec![rule(0xE100, 0xE1FF, "Portable Symbols")]
        );

        choose(&mut app, "rule:0");
        choose(&mut app, "delete");
        assert_eq!(app.info_overlay.title(), "Delete Font Range Override");
        assert_eq!(app.config.font.range_overrides.len(), 1);
        choose(&mut app, "confirm");
        assert!(app.config.font.range_overrides.is_empty());
        assert_eq!(app.info_overlay.title(), "Font Range Overrides");
    }

    #[test]
    fn invalid_edit_retains_input_and_live_config_for_correction() {
        let mut config = Config::default();
        config.font.range_overrides = vec![rule(0xE000, 0xE0FF, "Existing")];
        let mut app = App::new_headless(config).unwrap();
        app.open_font_range_manager();
        choose(&mut app, "add");
        let submitted = "U+E080-U+E1FF | Overlap";
        submit(&mut app, submitted);

        assert_eq!(
            app.config.font.range_overrides,
            vec![rule(0xE000, 0xE0FF, "Existing")]
        );
        assert_eq!(app.info_overlay.title(), "Add Font Range Override");
        assert_eq!(
            app.info_overlay.input_value(app.info_overlay.request_id()),
            Some(submitted)
        );
        assert!(
            app.info_overlay
                .lines()
                .iter()
                .any(|line| line.contains("overlaps"))
        );

        submit(&mut app, "U+E100-U+E1FF | Adjacent");
        assert_eq!(app.config.font.range_overrides.len(), 2);
    }

    #[test]
    fn stale_identity_and_request_cannot_retarget_a_reloaded_rule() {
        let mut config = Config::default();
        config.font.range_overrides = vec![rule(0xE000, 0xE0FF, "Original")];
        let mut app = App::new_headless(config).unwrap();
        app.open_font_range_manager();
        let stale_request = app.info_overlay.request_id();

        let mut reloaded = app.config.clone();
        reloaded.font.range_overrides = vec![rule(0xF000, 0xF0FF, "Reloaded")];
        app.apply_config(reloaded.clone()).unwrap();
        app.apply_info_overlay_action(InfoOverlayAction::Choose {
            request_id: stale_request,
            choice_id: "rule:0".into(),
        });
        assert_eq!(
            app.config.font.range_overrides,
            reloaded.font.range_overrides
        );
        assert_eq!(app.info_overlay.title(), "Font Range Overrides");
        assert!(
            app.info_overlay
                .lines()
                .iter()
                .any(|line| line.contains("U+F000–U+F0FF"))
        );

        choose(&mut app, "rule:0");
        choose(&mut app, "delete");
        let stale_delete = app.info_overlay.request_id();
        let mut replaced_again = app.config.clone();
        replaced_again.font.range_overrides = vec![rule(0x1000, 0x10FF, "Newest")];
        app.apply_config(replaced_again.clone()).unwrap();
        app.apply_info_overlay_action(InfoOverlayAction::Choose {
            request_id: stale_delete,
            choice_id: "confirm".into(),
        });
        assert_eq!(
            app.config.font.range_overrides,
            replaced_again.font.range_overrides
        );

        let retired_request = app.info_overlay.request_id();
        let replacement = app
            .info_overlay
            .show("Replacement", vec!["No editor authority".into()]);
        assert_ne!(retired_request, replacement);
        app.apply_info_overlay_action(InfoOverlayAction::Choose {
            request_id: retired_request,
            choice_id: "rule:0".into(),
        });
        assert_eq!(
            app.config.font.range_overrides,
            replaced_again.font.range_overrides
        );
        assert_eq!(app.info_overlay.title(), "Replacement");
        app.apply_info_overlay_action(InfoOverlayAction::Dismiss {
            request_id: replacement,
        });
        assert!(app.pending_font_range_management().is_none());
    }

    #[test]
    fn manager_pages_at_the_rule_limit_and_cancel_returns_without_mutation() {
        let mut config = Config::default();
        config.font.range_overrides = (0..MAX_FONT_RANGE_OVERRIDES)
            .map(|value| rule(value as u32, value as u32, &format!("Family {value}")))
            .collect();
        config.validate().unwrap();
        let original = config.font.range_overrides.clone();
        let mut app = App::new_headless(config).unwrap();
        app.open_font_range_manager();
        assert!(
            app.info_overlay
                .choices()
                .iter()
                .all(|choice| choice.id != "add")
        );
        assert!(
            app.info_overlay
                .lines()
                .iter()
                .any(|line| line.contains("page 1/8"))
        );
        choose(&mut app, "next-page");
        assert!(
            app.info_overlay
                .lines()
                .iter()
                .any(|line| line.contains("page 2/8"))
        );

        choose(&mut app, "rule:0");
        choose(&mut app, "edit");
        let request_id = app.info_overlay.request_id();
        app.apply_info_overlay_action(InfoOverlayAction::CancelInput { request_id });
        assert_eq!(app.config.font.range_overrides, original);
        assert_eq!(app.info_overlay.title(), "Font Range Override");
    }
}
