//! Request-scoped Settings editor for Unicode-range font overrides.

use super::*;
use crate::config::{FontRangeOverride, MAX_FONT_FAMILY_BYTES, MAX_FONT_RANGE_OVERRIDES};
use crate::ui::info_overlay::{InfoOverlayChoice, MAX_INFO_INPUT_BYTES};

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
    ConfirmDelete {
        page: usize,
        rule: FontRangeOverride,
    },
}

impl App {
    pub(super) fn open_font_range_manager(&mut self) {
        self.settings_panel.close();
        self.pending_profile_selection = None;
        self.pending_runtime_plugin_management = None;
        self.pending_file_upload = None;
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
        let page_count = rules.len().div_ceil(RANGE_PAGE_SIZE).max(1);
        let page = requested_page.min(page_count - 1);
        let start = page * RANGE_PAGE_SIZE;
        let end = (start + RANGE_PAGE_SIZE).min(rules.len());
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
        if rules.len() < MAX_FONT_RANGE_OVERRIDES {
            let source_line = lines.len();
            lines.push("[A] Add override".into());
            choices.push(InfoOverlayChoice::new(
                "add",
                "Add override",
                source_line,
                Some('a'),
            ));
        } else {
            lines.push(format!(
                "Add unavailable: the maximum of {MAX_FONT_RANGE_OVERRIDES} rules is configured."
            ));
        }
        lines.push(String::new());

        for (index, rule) in visible_rules.iter().enumerate() {
            let source_line = lines.len();
            let label = format_range_rule(rule);
            let shortcut = RANGE_SHORTCUTS[index];
            lines.push(format!("[{}] {label}", shortcut));
            choices.push(InfoOverlayChoice::new(
                format!("rule:{index}"),
                label,
                source_line,
                Some(shortcut),
            ));
        }
        if rules.is_empty() {
            lines.push("No Unicode-range font overrides are configured.".into());
        }
        if page > 0 {
            let source_line = lines.len();
            lines.push("[P] Previous page".into());
            choices.push(InfoOverlayChoice::new(
                "previous-page",
                "Previous page",
                source_line,
                Some('p'),
            ));
        }
        if page + 1 < page_count {
            let source_line = lines.len();
            lines.push("[N] Next page".into());
            choices.push(InfoOverlayChoice::new(
                "next-page",
                "Next page",
                source_line,
                Some('n'),
            ));
        }
        let source_line = lines.len();
        lines.push("[B] Back to Settings".into());
        choices.push(InfoOverlayChoice::new(
            "back",
            "Back to Settings",
            source_line,
            Some('b'),
        ));

        let request_id = self
            .info_overlay
            .show_choices("Font Range Overrides", lines, choices);
        self.pending_font_range_management = Some(PendingFontRangeManagement {
            request_id,
            view: FontRangeManagerView::List {
                page,
                visible_rules,
            },
        });
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
        let edit_line = lines.len();
        lines.push("[E] Edit override".into());
        let delete_line = lines.len();
        lines.push("[D] Delete override".into());
        let back_line = lines.len();
        lines.push("[B] Back to list".into());
        let request_id = self.info_overlay.show_choices(
            "Font Range Override",
            lines,
            vec![
                InfoOverlayChoice::new("edit", "Edit override", edit_line, Some('e')),
                InfoOverlayChoice::new("delete", "Delete override", delete_line, Some('d')),
                InfoOverlayChoice::new("back", "Back to list", back_line, Some('b')),
            ],
        );
        self.pending_font_range_management = Some(PendingFontRangeManagement {
            request_id,
            view: FontRangeManagerView::Detail { page, rule },
        });
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
        self.pending_font_range_management = Some(PendingFontRangeManagement {
            request_id,
            view: FontRangeManagerView::Edit { page, original },
        });
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
        let delete_line = lines.len();
        lines.push("[D] Delete".into());
        let cancel_line = lines.len();
        lines.push("[B] Keep override".into());
        let request_id = self.info_overlay.show_choices(
            "Delete Font Range Override",
            lines,
            vec![
                InfoOverlayChoice::new("confirm", "Delete override", delete_line, Some('d')),
                InfoOverlayChoice::new("cancel", "Keep override", cancel_line, Some('b')),
            ],
        );
        self.pending_font_range_management = Some(PendingFontRangeManagement {
            request_id,
            view: FontRangeManagerView::ConfirmDelete { page, rule },
        });
    }

    pub(super) fn handle_font_range_manager_choice(
        &mut self,
        request_id: u64,
        choice_id: &str,
    ) -> bool {
        let Some(pending) = self
            .pending_font_range_management
            .as_ref()
            .filter(|pending| pending.request_id == request_id)
            .cloned()
        else {
            return false;
        };
        self.info_overlay.dismiss(request_id);
        self.pending_font_range_management = None;
        match pending.view {
            FontRangeManagerView::List {
                page,
                visible_rules,
            } => match choice_id {
                "add" if self.config.font.range_overrides.len() < MAX_FONT_RANGE_OVERRIDES => {
                    self.show_font_range_edit(page, None, String::new(), None);
                }
                "previous-page" => self.show_font_range_list(page.saturating_sub(1)),
                "next-page" => self.show_font_range_list(page.saturating_add(1)),
                "back" => {
                    self.settings_panel
                        .open_to_section(crate::ui::settings::SettingsSection::Font);
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
            FontRangeManagerView::ConfirmDelete { page, rule } => match choice_id {
                "confirm" => self.delete_font_range_rule(page, rule),
                "cancel" | "back" => self.show_font_range_detail(page, rule),
                _ => self.show_font_range_delete_confirmation(page, rule),
            },
        }
        true
    }

    pub(super) fn submit_font_range_manager_input(&mut self, request_id: u64) -> bool {
        let Some(PendingFontRangeManagement {
            view: FontRangeManagerView::Edit { page, original },
            ..
        }) = self
            .pending_font_range_management
            .as_ref()
            .filter(|pending| pending.request_id == request_id)
            .cloned()
        else {
            return false;
        };
        let Some(value) = self
            .info_overlay
            .input_value(request_id)
            .map(ToOwned::to_owned)
        else {
            return false;
        };
        self.info_overlay.dismiss(request_id);
        self.pending_font_range_management = None;

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
                    Some("the maximum of 64 rules is already configured"),
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
        let Some(PendingFontRangeManagement {
            view: FontRangeManagerView::Edit { page, original },
            ..
        }) = self
            .pending_font_range_management
            .as_ref()
            .filter(|pending| pending.request_id == request_id)
            .cloned()
        else {
            return false;
        };
        self.info_overlay.dismiss(request_id);
        self.pending_font_range_management = None;
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
        assert!(app.pending_font_range_management.is_none());
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
