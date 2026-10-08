//! Font ranges, edited on the Font page of Settings: the prompts that add a
//! range or move its start or end, its font, and the question before one is
//! deleted. Every change applies at once and Save keeps it, as any setting.

use super::*;
use crate::config::{FontRangeOverride, MAX_FONT_FAMILY_BYTES, MAX_FONT_RANGE_OVERRIDES};
use crate::ui::dialog::{ButtonRole, DialogBlock, DialogSpec, FieldSpec};
use crate::ui::settings::{FontRangeAction, SettingsSubPage, format_codepoint, range_text};

const RANGE_INPUT_BYTES: usize = MAX_FONT_FAMILY_BYTES + 32;
/// Room for one character in hexadecimal with its prefix and some spaces.
const CHARACTER_INPUT_BYTES: usize = 16;
/// The answer that submits a prompt's field.
const SAVE: &str = "save";
const DELETE: &str = "delete";

/// A font range question awaiting its answer.
#[derive(Debug, Clone)]
pub(super) struct PendingFontRangeQuestion {
    request_id: u64,
    question: FontRangeQuestion,
}

#[derive(Debug, Clone)]
enum FontRangeQuestion {
    /// The range and font of a new rule.
    Add,
    /// A new first character for the rule.
    Start(FontRangeOverride),
    /// A new last character for the rule.
    End(FontRangeOverride),
    Delete(FontRangeOverride),
}

impl App {
    pub(super) fn apply_font_range_action(&mut self, action: FontRangeAction) {
        match action {
            FontRangeAction::Add => {
                if self.config.font.range_overrides.len() < MAX_FONT_RANGE_OVERRIDES {
                    self.ask_font_range(FontRangeQuestion::Add, String::new(), None);
                } else {
                    self.toasts.warning(format!(
                        "There can be at most {MAX_FONT_RANGE_OVERRIDES} font ranges"
                    ));
                }
            }
            FontRangeAction::EditStart(rule) if self.font_range_is_current(&rule) => {
                let value = format_codepoint(rule.start);
                self.ask_font_range(FontRangeQuestion::Start(rule), value, None);
            }
            FontRangeAction::EditEnd(rule) if self.font_range_is_current(&rule) => {
                let value = format_codepoint(rule.end);
                self.ask_font_range(FontRangeQuestion::End(rule), value, None);
            }
            FontRangeAction::SetFamily { rule, family } if self.font_range_is_current(&rule) => {
                let changed = FontRangeOverride {
                    family,
                    ..rule.clone()
                };
                if let Err(error) = self.replace_font_range(&rule, changed) {
                    self.toasts
                        .error(format!("Font range could not be changed: {error}"));
                }
            }
            FontRangeAction::Delete(rule) if self.font_range_is_current(&rule) => {
                self.ask_font_range(FontRangeQuestion::Delete(rule), String::new(), None);
            }
            _ => self.warn_stale_font_range(),
        }
    }

    /// Ask `question`, with `value` in its field and the `error` that
    /// refused it the last time.
    fn ask_font_range(&mut self, question: FontRangeQuestion, value: String, error: Option<&str>) {
        let error = error.map(crate::safe_text::capitalized);
        let character_prompt = |title: &str, label: &str| {
            DialogSpec::prompt(
                title,
                vec![DialogBlock::text(
                    "One character in hexadecimal, such as U+0600. U+ and 0x are both accepted.",
                )],
                FieldSpec::new(label, value.as_str(), CHARACTER_INPUT_BYTES)
                    .with_error(error.clone()),
                SAVE,
                "Change",
            )
        };
        let spec = match &question {
            FontRangeQuestion::Add => DialogSpec::prompt(
                "Add a font range",
                vec![
                    DialogBlock::text(
                        "Characters in the range use its font when that font has the whole character; the main font still sets the cell size. Write the first and last character in hexadecimal, then the font. U+ and 0x are both accepted.",
                    ),
                    DialogBlock::value("U+E000-U+F8FF | Symbols Nerd Font Mono"),
                ],
                FieldSpec::new("Range", value.as_str(), RANGE_INPUT_BYTES)
                    .with_error(error.clone()),
                SAVE,
                "Add",
            ),
            FontRangeQuestion::Start(_) => {
                character_prompt("Change where the range starts", "Start")
            }
            FontRangeQuestion::End(_) => character_prompt("Change where the range ends", "End"),
            FontRangeQuestion::Delete(rule) => DialogSpec::confirm(
                "Delete this font range?",
                vec![
                    DialogBlock::value(format!("{} → {}", range_text(rule), rule.family)),
                    DialogBlock::text("It's gone at once; Save in Settings keeps the change."),
                ],
                DELETE,
                "Delete",
                ButtonRole::Destructive,
            ),
        };
        let request_id = self.dialog.show(spec);
        self.dialog_flow = Some(DialogFlow::FontRange(PendingFontRangeQuestion {
            request_id,
            question,
        }));
    }

    /// Answer a font range question. Returns whether the answer was its;
    /// Cancel isn't, and closes the dialog.
    pub(super) fn handle_font_range_choice(&mut self, request_id: u64, choice_id: &str) -> bool {
        let question = match &self.dialog_flow {
            Some(DialogFlow::FontRange(pending)) if pending.request_id == request_id => {
                pending.question.clone()
            }
            _ => return false,
        };
        match (question, choice_id) {
            (FontRangeQuestion::Delete(rule), DELETE) => {
                self.dialog.dismiss(request_id);
                self.dialog_flow = None;
                self.delete_font_range(rule);
            }
            (FontRangeQuestion::Delete(_), _) => return false,
            (question, SAVE) => {
                let Some(value) = self.dialog.field_value(request_id).map(ToOwned::to_owned) else {
                    return false;
                };
                self.dialog.dismiss(request_id);
                self.dialog_flow = None;
                self.submit_font_range(question, value);
            }
            _ => return false,
        }
        true
    }

    /// Apply a prompt's answer, or ask again with why it was refused.
    fn submit_font_range(&mut self, question: FontRangeQuestion, value: String) {
        let parsed = match &question {
            FontRangeQuestion::Add => parse_font_range_expression(&value),
            FontRangeQuestion::Start(rule) => {
                parse_hex_scalar(&value).map(|start| FontRangeOverride {
                    start,
                    ..rule.clone()
                })
            }
            FontRangeQuestion::End(rule) => parse_hex_scalar(&value).map(|end| FontRangeOverride {
                end,
                ..rule.clone()
            }),
            FontRangeQuestion::Delete(_) => unreachable!("a confirmation has no field"),
        };
        let rule = match parsed {
            Ok(rule) => rule,
            Err(error) => return self.ask_font_range(question, value, Some(&error)),
        };
        let result = match &question {
            FontRangeQuestion::Start(original) | FontRangeQuestion::End(original) => {
                if !self.font_range_is_current(original) {
                    return self.warn_stale_font_range();
                }
                self.replace_font_range(original, rule)
            }
            _ => self.add_font_range(rule),
        };
        if let Err(error) = result {
            self.ask_font_range(question, value, Some(&error));
        }
    }

    fn add_font_range(&mut self, rule: FontRangeOverride) -> std::result::Result<(), String> {
        let mut candidate = self.config.clone();
        if candidate.font.range_overrides.len() >= MAX_FONT_RANGE_OVERRIDES {
            return Err(format!(
                "the most there can be ({MAX_FONT_RANGE_OVERRIDES}) are already set"
            ));
        }
        candidate.font.range_overrides.push(rule.clone());
        self.apply_font_config(candidate)?;
        self.toasts.success("Font range added");
        self.settings_panel
            .select_opener(SettingsSubPage::FontRange(rule));
        Ok(())
    }

    /// Replace `original` with `rule`, keeping its sub-page on it.
    fn replace_font_range(
        &mut self,
        original: &FontRangeOverride,
        rule: FontRangeOverride,
    ) -> std::result::Result<(), String> {
        let mut candidate = self.config.clone();
        let Some(index) = candidate
            .font
            .range_overrides
            .iter()
            .position(|current| current == original)
        else {
            return Err("that font range changed elsewhere".into());
        };
        candidate.font.range_overrides[index] = rule.clone();
        self.apply_font_config(candidate)?;
        self.toasts.success("Font range updated");
        self.settings_panel.retarget_sub_page(
            &SettingsSubPage::FontRange(original.clone()),
            SettingsSubPage::FontRange(rule),
        );
        Ok(())
    }

    fn delete_font_range(&mut self, rule: FontRangeOverride) {
        let mut candidate = self.config.clone();
        let Some(index) = candidate
            .font
            .range_overrides
            .iter()
            .position(|current| current == &rule)
        else {
            return self.warn_stale_font_range();
        };
        candidate.font.range_overrides.remove(index);
        match self.apply_font_config(candidate) {
            Ok(()) => {
                self.toasts.success("Font range deleted");
                self.settings_panel
                    .close_sub_page(&SettingsSubPage::FontRange(rule));
            }
            Err(error) => self
                .toasts
                .error(format!("Font range could not be deleted: {error}")),
        }
    }

    /// Validate and apply a configuration with a changed font rule.
    pub(super) fn apply_font_config(
        &mut self,
        candidate: Config,
    ) -> std::result::Result<(), String> {
        candidate.validate().map_err(|error| format!("{error:#}"))?;
        self.apply_config(candidate)
    }

    fn font_range_is_current(&self, rule: &FontRangeOverride) -> bool {
        self.config.font.range_overrides.contains(rule)
    }

    /// A row named a range a reload has since changed: nothing is done, and
    /// Settings already shows the ranges as they are now.
    fn warn_stale_font_range(&mut self) {
        self.toasts
            .warning("Font ranges changed elsewhere; nothing was changed");
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

/// One code point written in hexadecimal, with or without a U+ or 0x prefix.
pub(super) fn parse_hex_scalar(value: &str) -> std::result::Result<u32, String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::dialog::{CANCEL, DialogAction};
    use crate::ui::settings::{SettingKind, SettingsAction, SettingsPage, SettingsSection};

    fn rule(start: u32, end: u32, family: &str) -> FontRangeOverride {
        FontRangeOverride {
            start,
            end,
            family: family.into(),
        }
    }

    /// Settings open on the Font page.
    fn app_on_font_page(config: Config) -> App {
        let mut app = App::new_headless(config).unwrap();
        app.settings_panel.set_reduced_motion(true);
        app.settings_panel.toggle();
        app.settings_panel.select_page(SettingsPage::Font);
        app
    }

    /// Activate the row `label` of `section` as a press or Enter would.
    fn activate(app: &mut App, section: SettingsSection, label: &str) {
        let action = app.settings_panel.accessibility_activate_setting(
            section,
            label,
            &app.config,
            &app.shortcut_registry,
        );
        assert!(
            !matches!(action, SettingsAction::None),
            "{label:?} does nothing"
        );
        app.apply_settings_action(action);
    }

    fn row_value(app: &App, label: &str) -> Option<String> {
        app.settings_panel
            .build_entries(&app.config, &app.shortcut_registry)
            .into_iter()
            .find(|entry| entry.label == label)
            .map(|entry| entry.value)
    }

    #[test]
    fn range_expression_parser_accepts_prefixes() {
        let rule = parse_font_range_expression(" 0xe000 - U+1faff | Noto Color Emoji ").unwrap();
        assert_eq!(
            rule,
            FontRangeOverride {
                start: 0xE000,
                end: 0x1FAFF,
                family: "Noto Color Emoji".into(),
            }
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
    fn a_range_is_added_edited_and_deleted_from_its_settings_page() {
        let mut app = app_on_font_page(Config::default());
        activate(&mut app, SettingsSection::FontRanges, "Add a range…");
        app.submit_dialog_field("0xe000-U+f8ff | Symbols Nerd Font Mono");
        assert_eq!(
            app.config.font.range_overrides,
            vec![rule(0xE000, 0xF8FF, "Symbols Nerd Font Mono")]
        );
        assert!(!app.dialog.is_awaiting_current());
        // A range that is exactly one Unicode block reads by its name.
        assert_eq!(
            row_value(&app, "Private Use Area").as_deref(),
            Some("Symbols Nerd Font Mono")
        );

        activate(&mut app, SettingsSection::FontRanges, "Private Use Area");
        assert_eq!(
            app.settings_panel.sub_page(),
            Some(&SettingsSubPage::FontRange(rule(
                0xE000,
                0xF8FF,
                "Symbols Nerd Font Mono"
            )))
        );
        activate(&mut app, SettingsSection::FontRange, "Start");
        assert_eq!(
            app.dialog.field_value(app.dialog.request_id()),
            Some("U+E000")
        );
        app.submit_dialog_field("U+E100");
        // The page follows its range as it changes.
        assert_eq!(row_value(&app, "Start").as_deref(), Some("U+E100"));
        activate(&mut app, SettingsSection::FontRange, "End");
        app.submit_dialog_field("0xe1ff");
        assert_eq!(
            app.config.font.range_overrides,
            vec![rule(0xE100, 0xE1FF, "Symbols Nerd Font Mono")]
        );

        app.apply_settings_action(SettingsAction::FontRange(FontRangeAction::SetFamily {
            rule: rule(0xE100, 0xE1FF, "Symbols Nerd Font Mono"),
            family: "Portable Symbols".into(),
        }));
        assert_eq!(row_value(&app, "Font").as_deref(), Some("Portable Symbols"));

        // Delete asks first, with Cancel focused.
        activate(&mut app, SettingsSection::FontRange, "Delete…");
        let request = app.dialog.request_id();
        assert_eq!(
            app.dialog.focus(),
            Some(crate::ui::dialog::DialogFocus::Button(0))
        );
        app.answer_dialog(CANCEL);
        assert!(!app.dialog.is_awaiting(request));
        assert_eq!(app.config.font.range_overrides.len(), 1);
        activate(&mut app, SettingsSection::FontRange, "Delete…");
        app.answer_dialog(DELETE);
        assert!(app.config.font.range_overrides.is_empty());
        // Back on the Font page.
        assert_eq!(app.settings_panel.sub_page(), None);
        assert!(row_value(&app, "Add a range…").is_some());
    }

    #[test]
    fn a_refused_range_keeps_the_typed_text_and_the_configuration() {
        let mut config = Config::default();
        config.font.range_overrides = vec![rule(0xE000, 0xE0FF, "Existing")];
        let mut app = app_on_font_page(config);
        activate(&mut app, SettingsSection::FontRanges, "Add a range…");
        let submitted = "U+E080-U+E1FF | Overlap";
        app.submit_dialog_field(submitted);
        assert_eq!(
            app.config.font.range_overrides,
            vec![rule(0xE000, 0xE0FF, "Existing")]
        );
        let field = app.dialog.field(app.dialog.request_id()).unwrap();
        assert_eq!(field.value, submitted);
        assert!(field.error.is_some());
        app.submit_dialog_field("U+E100-U+E1FF | Adjacent");
        assert_eq!(app.config.font.range_overrides.len(), 2);

        // An end before the start is refused the same way.
        app.settings_panel
            .open_sub_page(SettingsSubPage::FontRange(rule(0xE000, 0xE0FF, "Existing")));
        activate(&mut app, SettingsSection::FontRange, "End");
        app.submit_dialog_field("U+0041");
        let field = app.dialog.field(app.dialog.request_id()).unwrap();
        assert_eq!(field.value, "U+0041");
        assert!(field.error.is_some());
        assert!(app.font_range_is_current(&rule(0xE000, 0xE0FF, "Existing")));
    }

    #[test]
    fn a_question_about_a_range_a_reload_replaced_changes_nothing() {
        let mut config = Config::default();
        config.font.range_overrides = vec![rule(0xE000, 0xE0FF, "Original")];
        let mut app = app_on_font_page(config);
        app.settings_panel
            .open_sub_page(SettingsSubPage::FontRange(rule(0xE000, 0xE0FF, "Original")));
        activate(&mut app, SettingsSection::FontRange, "Delete…");
        let stale_delete = app.dialog.request_id();
        let mut reloaded = app.config.clone();
        reloaded.font.range_overrides = vec![rule(0xF000, 0xF0FF, "Reloaded")];
        app.apply_config(reloaded.clone()).unwrap();
        app.apply_dialog_action(DialogAction::Answer {
            request_id: stale_delete,
            id: DELETE.into(),
        });
        assert_eq!(
            app.config.font.range_overrides,
            reloaded.font.range_overrides
        );
        // The page its range left falls back to the Font page.
        let _ = app
            .settings_panel
            .build_info(&app.config, &app.shortcut_registry);
        assert_eq!(app.settings_panel.sub_page(), None);

        // A row built before the reload names the old range and does nothing.
        app.apply_settings_action(SettingsAction::FontRange(FontRangeAction::EditStart(rule(
            0xE000, 0xE0FF, "Original",
        ))));
        assert!(!app.dialog.is_awaiting_current());
        assert_eq!(
            app.config.font.range_overrides,
            reloaded.font.range_overrides
        );
    }

    #[test]
    fn at_the_limit_the_add_row_says_so_and_adds_nothing() {
        let mut config = Config::default();
        config.font.range_overrides = (0..MAX_FONT_RANGE_OVERRIDES)
            .map(|value| rule(value as u32, value as u32, &format!("Family {value}")))
            .collect();
        config.validate().unwrap();
        let mut app = app_on_font_page(config);
        let add = app
            .settings_panel
            .build_entries(&app.config, &app.shortcut_registry)
            .into_iter()
            .find(|entry| entry.label == "Add a range…")
            .unwrap();
        assert!(matches!(add.kind, SettingKind::ReadOnly));
        app.apply_settings_action(SettingsAction::FontRange(FontRangeAction::Add));
        assert!(!app.dialog.is_awaiting_current());
        assert_eq!(
            app.config.font.range_overrides.len(),
            MAX_FONT_RANGE_OVERRIDES
        );
    }
}
