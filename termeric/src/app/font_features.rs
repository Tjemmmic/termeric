//! OpenType feature values, edited on the Font page of Settings: the
//! prompts that add a feature or change its tag or value, and the question
//! before one is deleted. Every change applies at once and Save keeps it.

use super::*;
use crate::config::MAX_FONT_FEATURES;
use crate::ui::dialog::{ButtonRole, DialogBlock, DialogSpec, FieldSpec};
use crate::ui::settings::{FontFeatureAction, SettingsSubPage};

const FEATURE_INPUT_BYTES: usize = 32;
/// The answer that submits a prompt's field.
const SAVE: &str = "save";
const DELETE: &str = "delete";

/// An OpenType feature question awaiting its answer.
#[derive(Debug, Clone)]
pub(super) struct PendingFontFeatureQuestion {
    request_id: u64,
    question: FontFeatureQuestion,
}

/// What is asked, with the feature's tag and value as they were when it
/// was asked, so an answer never lands on a feature that changed since.
#[derive(Debug, Clone)]
enum FontFeatureQuestion {
    /// A new feature as "tag = value".
    Add,
    Tag(String, u32),
    Value(String, u32),
    Delete(String, u32),
}

impl App {
    pub(super) fn apply_font_feature_action(&mut self, action: FontFeatureAction) {
        let current = |app: &Self, tag: String| {
            let value = app.config.font.features.get(&tag).copied();
            value.map(|value| (tag, value))
        };
        match action {
            FontFeatureAction::Add => {
                if self.config.font.features.len() < MAX_FONT_FEATURES {
                    self.ask_font_feature(FontFeatureQuestion::Add, String::new(), None);
                } else {
                    self.toasts.warning(format!(
                        "There can be at most {MAX_FONT_FEATURES} OpenType features"
                    ));
                }
            }
            FontFeatureAction::EditTag(tag) => match current(self, tag) {
                Some((tag, value)) => {
                    let input = tag.clone();
                    self.ask_font_feature(FontFeatureQuestion::Tag(tag, value), input, None);
                }
                None => self.warn_stale_font_feature(),
            },
            FontFeatureAction::EditValue(tag) => match current(self, tag) {
                Some((tag, value)) => self.ask_font_feature(
                    FontFeatureQuestion::Value(tag, value),
                    value.to_string(),
                    None,
                ),
                None => self.warn_stale_font_feature(),
            },
            FontFeatureAction::Delete(tag) => match current(self, tag) {
                Some((tag, value)) => self.ask_font_feature(
                    FontFeatureQuestion::Delete(tag, value),
                    String::new(),
                    None,
                ),
                None => self.warn_stale_font_feature(),
            },
        }
    }

    /// Ask `question`, with `input` in its field and the `error` that
    /// refused it the last time.
    fn ask_font_feature(
        &mut self,
        question: FontFeatureQuestion,
        input: String,
        error: Option<&str>,
    ) {
        let field = |label: &str| {
            FieldSpec::new(label, input.as_str(), FEATURE_INPUT_BYTES)
                .with_error(error.map(crate::safe_text::capitalized))
        };
        let spec = match &question {
            FontFeatureQuestion::Add => DialogSpec::prompt(
                "Add a feature",
                vec![DialogBlock::text(
                    "Write it as tag = value: a four-character tag and a number, in decimal or 0x hex. A value set here overrides the Ligatures preset, and kern isn't available, so the cell grid holds.",
                )],
                field("Feature"),
                SAVE,
                "Add",
            ),
            FontFeatureQuestion::Tag(..) => DialogSpec::prompt(
                "Change the tag",
                vec![DialogBlock::text(
                    "Four lowercase letters or digits, such as ss01. The value stays.",
                )],
                field("Tag"),
                SAVE,
                "Change",
            ),
            FontFeatureQuestion::Value(..) => DialogSpec::prompt(
                "Change the value",
                vec![DialogBlock::text(
                    "A number, in decimal or 0x hex: usually 1 turns the feature on and 0 off. A font that doesn't have the feature ignores it.",
                )],
                field("Value"),
                SAVE,
                "Change",
            ),
            FontFeatureQuestion::Delete(tag, value) => DialogSpec::confirm(
                format!("Delete {tag}?"),
                vec![
                    DialogBlock::value(format!("{tag} = {value}")),
                    DialogBlock::text("If the Ligatures preset sets this tag, it sets it again."),
                ],
                DELETE,
                "Delete",
                ButtonRole::Destructive,
            ),
        };
        let request_id = self.dialog.show(spec);
        self.dialog_flow = Some(DialogFlow::FontFeature(PendingFontFeatureQuestion {
            request_id,
            question,
        }));
    }

    /// Answer an OpenType feature question. Returns whether the answer was
    /// its; Cancel isn't, and closes the dialog.
    pub(super) fn handle_font_feature_choice(&mut self, request_id: u64, choice_id: &str) -> bool {
        let question = match &self.dialog_flow {
            Some(DialogFlow::FontFeature(pending)) if pending.request_id == request_id => {
                pending.question.clone()
            }
            _ => return false,
        };
        match (question, choice_id) {
            (FontFeatureQuestion::Delete(tag, value), DELETE) => {
                self.dialog.dismiss(request_id);
                self.dialog_flow = None;
                self.delete_font_feature(tag, value);
            }
            (FontFeatureQuestion::Delete(..), _) => return false,
            (question, SAVE) => {
                let Some(input) = self.dialog.field_value(request_id).map(ToOwned::to_owned) else {
                    return false;
                };
                self.dialog.dismiss(request_id);
                self.dialog_flow = None;
                self.submit_font_feature(question, input);
            }
            _ => return false,
        }
        true
    }

    /// Apply a prompt's answer, or ask again with why it was refused.
    fn submit_font_feature(&mut self, question: FontFeatureQuestion, input: String) {
        let original = match &question {
            FontFeatureQuestion::Tag(tag, value) | FontFeatureQuestion::Value(tag, value) => {
                if !self.font_feature_is_current(tag, *value) {
                    return self.warn_stale_font_feature();
                }
                Some((tag.clone(), *value))
            }
            _ => None,
        };
        let parsed = match &question {
            FontFeatureQuestion::Add => parse_font_feature(&input),
            FontFeatureQuestion::Tag(_, value) => {
                parse_feature_tag(&input).map(|tag| (tag, *value))
            }
            FontFeatureQuestion::Value(tag, _) => {
                parse_feature_value(&input).map(|value| (tag.clone(), value))
            }
            FontFeatureQuestion::Delete(..) => unreachable!("a confirmation has no field"),
        };
        let result = parsed.and_then(|(tag, value)| self.set_font_feature(original, tag, value));
        if let Err(error) = result {
            self.ask_font_feature(question, input, Some(&error));
        }
    }

    /// Set `tag` to `value`, in place of `original` when it replaces one.
    fn set_font_feature(
        &mut self,
        original: Option<(String, u32)>,
        tag: String,
        value: u32,
    ) -> std::result::Result<(), String> {
        let mut candidate = self.config.clone();
        if let Some((old_tag, _)) = &original {
            candidate.font.features.remove(old_tag);
        } else if candidate.font.features.len() >= MAX_FONT_FEATURES {
            return Err(format!(
                "the most there can be ({MAX_FONT_FEATURES}) are already set"
            ));
        }
        if candidate.font.features.contains_key(&tag) {
            return Err("that tag already has a value".into());
        }
        candidate.font.features.insert(tag.clone(), value);
        self.apply_font_config(candidate)?;
        match original {
            Some((old_tag, _)) => {
                self.toasts.success("OpenType feature updated");
                self.settings_panel.retarget_sub_page(
                    &SettingsSubPage::FontFeature(old_tag),
                    SettingsSubPage::FontFeature(tag),
                );
            }
            None => {
                self.toasts.success("OpenType feature added");
                self.settings_panel
                    .select_opener(SettingsSubPage::FontFeature(tag));
            }
        }
        Ok(())
    }

    fn delete_font_feature(&mut self, tag: String, value: u32) {
        if !self.font_feature_is_current(&tag, value) {
            return self.warn_stale_font_feature();
        }
        let mut candidate = self.config.clone();
        candidate.font.features.remove(&tag);
        match self.apply_font_config(candidate) {
            Ok(()) => {
                self.toasts.success("OpenType feature deleted");
                self.settings_panel
                    .close_sub_page(&SettingsSubPage::FontFeature(tag));
            }
            Err(error) => self
                .toasts
                .error(format!("OpenType feature could not be deleted: {error}")),
        }
    }

    fn font_feature_is_current(&self, tag: &str, value: u32) -> bool {
        self.config.font.features.get(tag) == Some(&value)
    }

    /// A row or question named a feature a reload has since changed:
    /// nothing is done, and Settings already shows the features as they are.
    fn warn_stale_font_feature(&mut self) {
        self.toasts
            .warning("OpenType features changed elsewhere; nothing was changed");
    }
}

/// "tag = value".
fn parse_font_feature(input: &str) -> std::result::Result<(String, u32), String> {
    check_feature_input(input)?;
    let Some((tag, value)) = input.split_once('=') else {
        return Err("expected tag = value".into());
    };
    if tag.trim().is_empty() || value.trim().is_empty() || value.contains('=') {
        return Err("expected exactly one tag = value pair".into());
    }
    Ok((parse_feature_tag(tag)?, parse_feature_value(value)?))
}

/// A tag as typed; whether it is one is up to the configuration's checks.
fn parse_feature_tag(input: &str) -> std::result::Result<String, String> {
    check_feature_input(input)?;
    let tag = input.trim();
    if tag.is_empty() {
        return Err("type a tag".into());
    }
    Ok(tag.to_owned())
}

/// A u32 in decimal or 0x hex.
fn parse_feature_value(input: &str) -> std::result::Result<u32, String> {
    check_feature_input(input)?;
    let value = input.trim();
    value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .map_or_else(
            || value.parse::<u32>(),
            |digits| u32::from_str_radix(digits, 16),
        )
        .map_err(|_| format!("feature value {value:?} must be a u32 decimal or 0x hex value"))
}

fn check_feature_input(input: &str) -> std::result::Result<(), String> {
    if input.len() > FEATURE_INPUT_BYTES {
        return Err(format!(
            "feature input must be no more than {FEATURE_INPUT_BYTES} UTF-8 bytes"
        ));
    }
    if input.chars().any(char::is_control) {
        return Err("feature input must not contain control characters".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::dialog::{CANCEL, DialogAction};
    use crate::ui::settings::{SettingKind, SettingsAction, SettingsPage, SettingsSection};

    fn app_on_font_page(config: Config) -> App {
        let mut app = App::new_headless(config).unwrap();
        app.settings_panel.set_reduced_motion(true);
        app.settings_panel.toggle();
        app.settings_panel.select_page(SettingsPage::Font);
        app
    }

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

    #[test]
    fn feature_parser_accepts_bounded_decimal_and_hex_values() {
        assert_eq!(
            parse_font_feature(" ss01 = 1 ").unwrap(),
            ("ss01".into(), 1)
        );
        assert_eq!(
            parse_font_feature("cv01=0xFFFFFFFF").unwrap(),
            ("cv01".into(), u32::MAX)
        );
        for invalid in [
            "ss01",
            "ss01 =",
            "= 1",
            "ss01 = -1",
            "ss01 = 1 = 2",
            "bad\n=1",
        ] {
            assert!(parse_font_feature(invalid).is_err(), "accepted {invalid:?}");
        }
        assert_eq!(parse_feature_value(" 0x10 ").unwrap(), 16);
        assert!(parse_feature_value("one").is_err());
        assert!(parse_feature_tag("  ").is_err());
    }

    #[test]
    fn a_feature_is_added_edited_and_deleted_from_its_settings_page() {
        let mut app = app_on_font_page(Config::default());
        activate(&mut app, SettingsSection::FontFeatures, "Add a feature…");
        app.submit_dialog_field("ss01 = 1");
        assert_eq!(app.config.font.features.get("ss01"), Some(&1));

        activate(&mut app, SettingsSection::FontFeatures, "ss01");
        activate(&mut app, SettingsSection::FontFeature, "Value");
        assert_eq!(app.dialog.field_value(app.dialog.request_id()), Some("1"));
        app.submit_dialog_field("0x2");
        activate(&mut app, SettingsSection::FontFeature, "Tag");
        app.submit_dialog_field("cv01");
        assert_eq!(app.config.font.features.get("ss01"), None);
        assert_eq!(app.config.font.features.get("cv01"), Some(&2));
        // The page follows its feature to the new tag.
        assert_eq!(
            app.settings_panel.sub_page(),
            Some(&SettingsSubPage::FontFeature("cv01".into()))
        );

        activate(&mut app, SettingsSection::FontFeature, "Delete…");
        app.answer_dialog(CANCEL);
        assert_eq!(app.config.font.features.get("cv01"), Some(&2));
        activate(&mut app, SettingsSection::FontFeature, "Delete…");
        app.answer_dialog(DELETE);
        assert!(app.config.font.features.is_empty());
        assert_eq!(app.settings_panel.sub_page(), None);
    }

    #[test]
    fn a_refused_feature_keeps_the_typed_text_and_a_stale_one_changes_nothing() {
        let mut config = Config::default();
        config.font.features.insert("ss01".into(), 1);
        config.font.features.insert("ss02".into(), 1);
        let mut app = app_on_font_page(config);
        app.settings_panel
            .open_sub_page(SettingsSubPage::FontFeature("ss01".into()));
        activate(&mut app, SettingsSection::FontFeature, "Tag");
        for refused in ["kern", "ss02", "TOOLONG"] {
            app.submit_dialog_field(refused);
            let field = app.dialog.field(app.dialog.request_id()).unwrap();
            assert_eq!(field.value, refused);
            assert!(field.error.is_some(), "{refused}");
            assert_eq!(app.config.font.features.get("ss01"), Some(&1));
        }

        // The value changes elsewhere while the prompt is open.
        let request = app.dialog.request_id();
        app.config.font.features.insert("ss01".into(), 5);
        app.dialog.replace_field_text(request, "cv01");
        app.apply_dialog_action(DialogAction::Answer {
            request_id: request,
            id: SAVE.into(),
        });
        assert_eq!(app.config.font.features.get("ss01"), Some(&5));
        assert_eq!(app.config.font.features.get("cv01"), None);
    }

    #[test]
    fn at_the_limit_the_add_row_says_so_and_adds_nothing() {
        let mut config = Config::default();
        config.font.features = (0..MAX_FONT_FEATURES)
            .map(|index| (format!("f{index:03}"), index as u32))
            .collect();
        let mut app = app_on_font_page(config);
        let entries = app
            .settings_panel
            .build_entries(&app.config, &app.shortcut_registry);
        // Every feature is a row, and the last one still opens.
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.section == SettingsSection::FontFeatures && !entry.is_header)
                .count(),
            MAX_FONT_FEATURES + 1
        );
        let add = entries
            .iter()
            .find(|entry| entry.label == "Add a feature…")
            .unwrap();
        assert!(matches!(add.kind, SettingKind::ReadOnly));
        app.apply_settings_action(SettingsAction::FontFeature(FontFeatureAction::Add));
        assert!(!app.dialog.is_awaiting_current());
        activate(
            &mut app,
            SettingsSection::FontFeatures,
            &format!("f{:03}", MAX_FONT_FEATURES - 1),
        );
        assert!(app.settings_panel.sub_page().is_some());
    }
}
