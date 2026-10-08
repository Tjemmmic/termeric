//! Request-scoped Settings editor for explicit OpenType feature values.

use super::*;
use crate::config::MAX_FONT_FEATURES;
use crate::ui::dialog::{
    ButtonRole, CANCEL, DialogBlock, DialogButton, DialogRow, DialogSpec, FieldSpec,
};

const FEATURE_INPUT_BYTES: usize = 32;
/// The answer that saves the edited feature.
const SAVE: &str = "save";

#[derive(Debug, Clone)]
pub(super) struct PendingFontFeatureManagement {
    pub(super) request_id: u64,
    view: FontFeatureManagerView,
}

#[derive(Debug, Clone)]
enum FontFeatureManagerView {
    /// The features as listed, so a row answers for the one it showed.
    List {
        entries: Vec<(String, u32)>,
    },
    Detail {
        tag: String,
        value: u32,
    },
    Edit {
        original: Option<(String, u32)>,
    },
    ConfirmDelete {
        tag: String,
        value: u32,
    },
}

impl App {
    pub(super) fn open_font_feature_manager(&mut self) {
        self.settings_panel.close();
        self.dialog_flow = None;
        self.show_font_feature_list();
    }

    fn show_font_feature_flow(&mut self, spec: DialogSpec, view: FontFeatureManagerView) {
        let request_id = self.dialog.show(spec);
        self.dialog_flow = Some(DialogFlow::FontFeatures(PendingFontFeatureManagement {
            request_id,
            view,
        }));
    }

    fn show_font_feature_list(&mut self) {
        let entries = self
            .config
            .font
            .features
            .iter()
            .map(|(tag, value)| (tag.clone(), *value))
            .collect::<Vec<_>>();
        let mut body = vec![DialogBlock::text(
            "A value set here overrides the Ligatures preset. kern stays off so the cell grid holds. Changes apply at once; Save in Settings keeps them.",
        )];
        let full = entries.len() >= MAX_FONT_FEATURES;
        if entries.is_empty() {
            body.push(DialogBlock::text("No feature values are set."));
        } else if full {
            body.push(DialogBlock::text(format!(
                "That's the most there can be ({MAX_FONT_FEATURES}). Delete one to add another."
            )));
        }
        let rows = entries
            .iter()
            .enumerate()
            .map(|(index, (tag, value))| {
                DialogRow::new(format!("feature:{index}"), tag.as_str())
                    .with_detail(value.to_string())
            })
            .collect();
        let mut spec =
            DialogSpec::list("OpenType features", body, rows).button(DialogButton::back());
        if !full {
            spec = spec.button(DialogButton::answer("add", "Add…", ButtonRole::Suggested));
        }
        self.show_font_feature_flow(spec, FontFeatureManagerView::List { entries });
    }

    fn show_font_feature_detail(&mut self, tag: String, value: u32) {
        if !self.font_feature_is_current(&tag, value) {
            self.warn_stale_font_feature();
            return;
        }
        let spec = DialogSpec::list(
            format!("Feature {tag}"),
            vec![
                DialogBlock::value(format_feature(&tag, value)),
                DialogBlock::text(
                    "The value is passed to shaping after the Ligatures preset. A font that doesn't have the feature ignores it.",
                ),
            ],
            Vec::new(),
        )
        .button(DialogButton::back())
        .button(DialogButton::answer("delete", "Delete…", ButtonRole::Neutral))
        .button(DialogButton::answer("edit", "Edit…", ButtonRole::Suggested));
        self.show_font_feature_flow(spec, FontFeatureManagerView::Detail { tag, value });
    }

    fn show_font_feature_edit(
        &mut self,
        original: Option<(String, u32)>,
        input: String,
        error: Option<&str>,
    ) {
        if let Some((tag, value)) = original.as_ref()
            && !self.font_feature_is_current(tag, *value)
        {
            self.warn_stale_font_feature();
            return;
        }
        let (title, action) = if original.is_some() {
            ("Edit feature", "Save")
        } else {
            ("Add a feature", "Add")
        };
        let spec = DialogSpec::prompt(
            title,
            vec![DialogBlock::text(
                "Write it as tag = value: a four-character tag and a number, in decimal or 0x hex. kern isn't available, so the cell grid holds.",
            )],
            FieldSpec::new("Feature", input, FEATURE_INPUT_BYTES)
                .with_error(error.map(crate::safe_text::capitalized)),
            SAVE,
            action,
        );
        self.show_font_feature_flow(spec, FontFeatureManagerView::Edit { original });
    }

    fn show_font_feature_delete_confirmation(&mut self, tag: String, value: u32) {
        if !self.font_feature_is_current(&tag, value) {
            self.warn_stale_font_feature();
            return;
        }
        let spec = DialogSpec::confirm(
            format!("Delete {tag}?"),
            vec![
                DialogBlock::value(format_feature(&tag, value)),
                DialogBlock::text("If the Ligatures preset sets this tag, it sets it again."),
            ],
            "delete",
            "Delete",
            ButtonRole::Destructive,
        );
        self.show_font_feature_flow(spec, FontFeatureManagerView::ConfirmDelete { tag, value });
    }

    /// Answer the manager's dialog. Returns whether the answer was its.
    pub(super) fn handle_font_feature_manager_choice(
        &mut self,
        request_id: u64,
        choice_id: &str,
    ) -> bool {
        let Some(pending) = self
            .pending_font_feature_management()
            .filter(|pending| pending.request_id == request_id)
            .cloned()
        else {
            return false;
        };
        if let FontFeatureManagerView::Edit { original } = &pending.view
            && choice_id == SAVE
        {
            self.submit_font_feature_edit(request_id, original.clone());
            return true;
        }
        self.dialog.dismiss(request_id);
        self.dialog_flow = None;
        match pending.view {
            FontFeatureManagerView::List { entries } => match choice_id {
                CANCEL => self.open_settings_section(crate::ui::settings::SettingsSection::Font),
                "add" if self.config.font.features.len() < MAX_FONT_FEATURES => {
                    self.show_font_feature_edit(None, String::new(), None);
                }
                choice if choice.starts_with("feature:") => {
                    let entry = choice["feature:".len()..]
                        .parse::<usize>()
                        .ok()
                        .and_then(|index| entries.get(index))
                        .cloned();
                    if let Some((tag, value)) =
                        entry.filter(|(tag, value)| self.font_feature_is_current(tag, *value))
                    {
                        self.show_font_feature_detail(tag, value);
                    } else {
                        self.warn_stale_font_feature();
                    }
                }
                _ => self.show_font_feature_list(),
            },
            FontFeatureManagerView::Detail { tag, value } => match choice_id {
                "edit" if self.font_feature_is_current(&tag, value) => {
                    let input = format_feature(&tag, value);
                    self.show_font_feature_edit(Some((tag, value)), input, None);
                }
                "delete" if self.font_feature_is_current(&tag, value) => {
                    self.show_font_feature_delete_confirmation(tag, value);
                }
                CANCEL => self.show_font_feature_list(),
                _ => self.warn_stale_font_feature(),
            },
            FontFeatureManagerView::Edit { original } => {
                self.return_from_font_feature_edit(original);
            }
            FontFeatureManagerView::ConfirmDelete { tag, value } => match choice_id {
                "delete" => self.delete_font_feature(tag, value),
                CANCEL => self.show_font_feature_detail(tag, value),
                _ => self.show_font_feature_delete_confirmation(tag, value),
            },
        }
        true
    }

    fn submit_font_feature_edit(&mut self, request_id: u64, original: Option<(String, u32)>) {
        let Some(input) = self.dialog.field_value(request_id).map(ToOwned::to_owned) else {
            return;
        };
        self.dialog.dismiss(request_id);
        self.dialog_flow = None;
        if let Some((tag, value)) = original.as_ref()
            && !self.font_feature_is_current(tag, *value)
        {
            self.warn_stale_font_feature();
            return;
        }
        let (tag, value) = match parse_font_feature(&input) {
            Ok(feature) => feature,
            Err(error) => {
                self.show_font_feature_edit(original, input, Some(&error));
                return;
            }
        };
        let mut candidate = self.config.clone();
        if let Some((old_tag, old_value)) = original.as_ref() {
            if candidate.font.features.get(old_tag) != Some(old_value) {
                self.warn_stale_font_feature();
                return;
            }
            candidate.font.features.remove(old_tag);
        } else if candidate.font.features.len() >= MAX_FONT_FEATURES {
            self.show_font_feature_edit(
                None,
                input,
                Some(&format!(
                    "the most there can be ({MAX_FONT_FEATURES}) are already set"
                )),
            );
            return;
        }
        if candidate.font.features.contains_key(&tag) {
            self.show_font_feature_edit(original, input, Some("that tag already has a value"));
            return;
        }
        candidate.font.features.insert(tag, value);
        if let Err(error) = candidate.validate() {
            self.show_font_feature_edit(original, input, Some(&format!("{error:#}")));
            return;
        }
        if let Err(error) = self.apply_config(candidate) {
            self.show_font_feature_edit(original, input, Some(&error));
            return;
        }
        self.toasts.success(if original.is_some() {
            "OpenType feature updated"
        } else {
            "OpenType feature added"
        });
        self.show_font_feature_list();
    }

    fn return_from_font_feature_edit(&mut self, original: Option<(String, u32)>) {
        if let Some((tag, value)) =
            original.filter(|(tag, value)| self.font_feature_is_current(tag, *value))
        {
            self.show_font_feature_detail(tag, value);
        } else {
            self.show_font_feature_list();
        }
    }

    fn delete_font_feature(&mut self, tag: String, value: u32) {
        let mut candidate = self.config.clone();
        if candidate.font.features.get(&tag) != Some(&value) {
            self.warn_stale_font_feature();
            return;
        }
        candidate.font.features.remove(&tag);
        match self.apply_config(candidate) {
            Ok(()) => {
                self.toasts.success("OpenType feature deleted");
                self.show_font_feature_list();
            }
            Err(error) => {
                self.toasts
                    .error(format!("OpenType feature could not be deleted: {error}"));
                self.show_font_feature_detail(tag, value);
            }
        }
    }

    fn font_feature_is_current(&self, tag: &str, value: u32) -> bool {
        self.config.font.features.get(tag) == Some(&value)
    }

    fn warn_stale_font_feature(&mut self) {
        self.toasts
            .warning("OpenType features changed; refreshed the list");
        self.show_font_feature_list();
    }
}

fn parse_font_feature(input: &str) -> std::result::Result<(String, u32), String> {
    if input.len() > FEATURE_INPUT_BYTES {
        return Err(format!(
            "feature input must be no more than {FEATURE_INPUT_BYTES} UTF-8 bytes"
        ));
    }
    if input.chars().any(char::is_control) {
        return Err("feature input must not contain control characters".into());
    }
    let Some((tag, value)) = input.split_once('=') else {
        return Err("expected tag = value".into());
    };
    let tag = tag.trim().to_string();
    let value = value.trim();
    if tag.is_empty() || value.is_empty() || value.contains('=') {
        return Err("expected exactly one tag = value pair".into());
    }
    let parsed = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .map_or_else(
            || value.parse::<u32>(),
            |digits| u32::from_str_radix(digits, 16),
        )
        .map_err(|_| format!("feature value {value:?} must be a u32 decimal or 0x hex value"))?;
    Ok((tag, parsed))
}

fn format_feature(tag: &str, value: u32) -> String {
    format!("{tag} = {value}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::dialog::DialogAction;

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
    }

    #[test]
    fn feature_manager_adds_edits_and_confirm_deletes_atomically() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.open_font_feature_manager();
        assert_eq!(app.dialog.title(), "OpenType features");
        app.answer_dialog("add");
        app.submit_dialog_field("ss01 = 1");
        assert_eq!(app.config.font.features.get("ss01"), Some(&1));
        app.answer_dialog("feature:0");
        app.answer_dialog("edit");
        app.submit_dialog_field("cv01 = 2");
        assert_eq!(app.config.font.features.get("ss01"), None);
        assert_eq!(app.config.font.features.get("cv01"), Some(&2));
        app.answer_dialog("feature:0");
        app.answer_dialog("delete");
        app.answer_dialog(CANCEL);
        assert_eq!(app.config.font.features.get("cv01"), Some(&2));
        assert_eq!(app.dialog.title(), "Feature cv01");
        app.answer_dialog("delete");
        app.answer_dialog("delete");
        assert!(app.config.font.features.is_empty());
        // Back leaves the manager for Settings.
        app.answer_dialog(CANCEL);
        assert!(!app.dialog.is_awaiting_current());
        assert!(app.settings_panel.is_active());
    }

    #[test]
    fn feature_manager_preserves_invalid_input_and_rejects_stale_identity() {
        let mut config = Config::default();
        config.font.features.insert("ss01".into(), 1);
        let mut app = App::new_headless(config).unwrap();
        app.open_font_feature_manager();
        app.answer_dialog("feature:0");
        app.answer_dialog("edit");
        app.submit_dialog_field("kern = 1");
        assert_eq!(app.dialog.title(), "Edit feature");
        let field = app.dialog.field(app.dialog.request_id()).unwrap();
        assert_eq!(field.value, "kern = 1");
        assert!(field.error.is_some());
        assert_eq!(app.config.font.features.get("ss01"), Some(&1));

        app.config.font.features.insert("ss01".into(), 2);
        app.submit_dialog_field("cv01 = 3");
        assert_eq!(app.config.font.features.get("ss01"), Some(&2));
        assert_eq!(app.config.font.features.get("cv01"), None);
        assert_eq!(app.dialog.title(), "OpenType features");
    }

    #[test]
    fn feature_manager_lists_every_feature_caps_cancels_and_refuses_replaced_requests() {
        let mut config = Config::default();
        config.font.features = (0..MAX_FONT_FEATURES)
            .map(|index| (format!("f{index:03}"), index as u32))
            .collect();
        let mut app = App::new_headless(config).unwrap();
        app.open_font_feature_manager();
        // One scrolling list instead of pages, and no Add at the limit.
        assert_eq!(app.dialog.rows().len(), MAX_FONT_FEATURES);
        assert!(!app.dialog.answers(app.dialog.request_id(), "add"));
        app.answer_dialog(&format!("feature:{}", MAX_FONT_FEATURES - 1));
        assert_eq!(
            app.dialog.title(),
            format!("Feature f{:03}", MAX_FONT_FEATURES - 1)
        );

        let mut app = App::new_headless(Config::default()).unwrap();
        app.open_font_feature_manager();
        app.answer_dialog("add");
        app.answer_dialog(CANCEL);
        assert_eq!(app.dialog.title(), "OpenType features");
        assert!(app.config.font.features.is_empty());

        app.answer_dialog("add");
        let replaced = app.dialog.request_id();
        app.dialog.replace_field_text(replaced, "ss01 = 1");
        app.dialog.show(crate::ui::dialog::DialogSpec::message(
            "Replacement",
            vec![],
        ));
        app.apply_dialog_action(DialogAction::Answer {
            request_id: replaced,
            id: "save".into(),
        });
        assert!(app.config.font.features.is_empty());
        let current = app.dialog.request_id();
        app.apply_dialog_action(DialogAction::Close {
            request_id: current,
        });
        assert!(app.pending_font_feature_management().is_none());
    }
}
