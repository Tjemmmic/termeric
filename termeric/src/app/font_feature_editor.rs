//! Request-scoped Settings editor for explicit OpenType feature values.

use super::*;
use crate::config::MAX_FONT_FEATURES;
use crate::ui::info_overlay::{InfoOverlayChoice, MAX_INFO_INPUT_BYTES};

const FEATURE_PAGE_SIZE: usize = 8;
const FEATURE_INPUT_BYTES: usize = 32;
const FEATURE_SHORTCUTS: [char; FEATURE_PAGE_SIZE] = ['1', '2', '3', '4', '5', '6', '7', '8'];

#[derive(Debug, Clone)]
pub(super) struct PendingFontFeatureManagement {
    pub(super) request_id: u64,
    view: FontFeatureManagerView,
}

#[derive(Debug, Clone)]
enum FontFeatureManagerView {
    List {
        page: usize,
        visible: Vec<(String, u32)>,
    },
    Detail {
        page: usize,
        tag: String,
        value: u32,
    },
    Edit {
        page: usize,
        original: Option<(String, u32)>,
    },
    ConfirmDelete {
        page: usize,
        tag: String,
        value: u32,
    },
}

impl App {
    pub(super) fn open_font_feature_manager(&mut self) {
        self.settings_panel.close();
        self.pending_profile_selection = None;
        self.pending_runtime_plugin_management = None;
        self.pending_font_range_management = None;
        self.pending_file_upload = None;
        self.show_font_feature_list(0);
    }

    fn show_font_feature_list(&mut self, requested_page: usize) {
        let entries = self
            .config
            .font
            .features
            .iter()
            .map(|(tag, value)| (tag.clone(), *value))
            .collect::<Vec<_>>();
        let page_count = entries.len().div_ceil(FEATURE_PAGE_SIZE).max(1);
        let page = requested_page.min(page_count - 1);
        let start = page * FEATURE_PAGE_SIZE;
        let end = (start + FEATURE_PAGE_SIZE).min(entries.len());
        let visible = entries[start..end].to_vec();
        let mut lines = vec![
            format!(
                "Explicit features: {}/{} · page {}/{}",
                entries.len(),
                MAX_FONT_FEATURES,
                page + 1,
                page_count
            ),
            "Explicit values override the Ligatures preset; kern remains disabled for the cell grid."
                .into(),
            "Changes apply live; use Save in Settings to persist them.".into(),
            String::new(),
        ];
        let mut choices = Vec::new();
        if entries.len() < MAX_FONT_FEATURES {
            let source_line = lines.len();
            lines.push("[A] Add feature override".into());
            choices.push(InfoOverlayChoice::new(
                "add",
                "Add feature override",
                source_line,
                Some('a'),
            ));
        } else {
            lines.push(format!(
                "Add unavailable: the maximum of {MAX_FONT_FEATURES} overrides is configured."
            ));
        }
        lines.push(String::new());
        for (index, (tag, value)) in visible.iter().enumerate() {
            let source_line = lines.len();
            let label = format_feature(tag, *value);
            let shortcut = FEATURE_SHORTCUTS[index];
            lines.push(format!("[{shortcut}] {label}"));
            choices.push(InfoOverlayChoice::new(
                format!("feature:{index}"),
                label,
                source_line,
                Some(shortcut),
            ));
        }
        if entries.is_empty() {
            lines.push("No explicit OpenType feature overrides are configured.".into());
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
        let request_id =
            self.info_overlay
                .show_choices("OpenType Feature Overrides", lines, choices);
        self.pending_font_feature_management = Some(PendingFontFeatureManagement {
            request_id,
            view: FontFeatureManagerView::List { page, visible },
        });
    }

    fn show_font_feature_detail(&mut self, page: usize, tag: String, value: u32) {
        if !self.font_feature_is_current(&tag, value) {
            self.warn_stale_font_feature(page);
            return;
        }
        let mut lines = vec![
            format_feature(&tag, value),
            String::new(),
            "The value is forwarded to advanced shaping after the Ligatures preset.".into(),
            "A font may ignore a feature it does not implement.".into(),
            String::new(),
        ];
        let edit_line = lines.len();
        lines.push("[E] Edit override".into());
        let delete_line = lines.len();
        lines.push("[D] Delete override".into());
        let back_line = lines.len();
        lines.push("[B] Back to list".into());
        let request_id = self.info_overlay.show_choices(
            "OpenType Feature Override",
            lines,
            vec![
                InfoOverlayChoice::new("edit", "Edit override", edit_line, Some('e')),
                InfoOverlayChoice::new("delete", "Delete override", delete_line, Some('d')),
                InfoOverlayChoice::new("back", "Back to list", back_line, Some('b')),
            ],
        );
        self.pending_font_feature_management = Some(PendingFontFeatureManagement {
            request_id,
            view: FontFeatureManagerView::Detail { page, tag, value },
        });
    }

    fn show_font_feature_edit(
        &mut self,
        page: usize,
        original: Option<(String, u32)>,
        input: String,
        error: Option<&str>,
    ) {
        if let Some((tag, value)) = original.as_ref()
            && !self.font_feature_is_current(tag, *value)
        {
            self.warn_stale_font_feature(page);
            return;
        }
        let mut lines = vec![
            "Enter one feature using: tag = value".into(),
            "Tags are four lowercase ASCII letters/digits; values are u32 decimal or 0x hex."
                .into(),
            "kern is unavailable because terminal cell origins remain fixed.".into(),
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
            "Edit OpenType Feature"
        } else {
            "Add OpenType Feature"
        };
        let request_id = self.info_overlay.show_input(
            title,
            lines,
            input_line,
            "Feature: ",
            input,
            FEATURE_INPUT_BYTES.min(MAX_INFO_INPUT_BYTES),
            vec![InfoOverlayChoice::new(
                "cancel",
                "Cancel edit (Escape)",
                cancel_line,
                None,
            )],
        );
        self.pending_font_feature_management = Some(PendingFontFeatureManagement {
            request_id,
            view: FontFeatureManagerView::Edit { page, original },
        });
    }

    fn show_font_feature_delete_confirmation(&mut self, page: usize, tag: String, value: u32) {
        if !self.font_feature_is_current(&tag, value) {
            self.warn_stale_font_feature(page);
            return;
        }
        let mut lines = vec![
            "Delete this explicit feature value?".into(),
            format_feature(&tag, value),
            String::new(),
            "The Ligatures preset will supply its normal value if this tag belongs to that preset."
                .into(),
            String::new(),
        ];
        let delete_line = lines.len();
        lines.push("[D] Delete".into());
        let cancel_line = lines.len();
        lines.push("[B] Keep override".into());
        let request_id = self.info_overlay.show_choices(
            "Delete OpenType Feature",
            lines,
            vec![
                InfoOverlayChoice::new("confirm", "Delete override", delete_line, Some('d')),
                InfoOverlayChoice::new("cancel", "Keep override", cancel_line, Some('b')),
            ],
        );
        self.pending_font_feature_management = Some(PendingFontFeatureManagement {
            request_id,
            view: FontFeatureManagerView::ConfirmDelete { page, tag, value },
        });
    }

    pub(super) fn handle_font_feature_manager_choice(
        &mut self,
        request_id: u64,
        choice_id: &str,
    ) -> bool {
        let Some(pending) = self
            .pending_font_feature_management
            .as_ref()
            .filter(|pending| pending.request_id == request_id)
            .cloned()
        else {
            return false;
        };
        self.info_overlay.dismiss(request_id);
        self.pending_font_feature_management = None;
        match pending.view {
            FontFeatureManagerView::List { page, visible } => match choice_id {
                "add" if self.config.font.features.len() < MAX_FONT_FEATURES => {
                    self.show_font_feature_edit(page, None, String::new(), None);
                }
                "previous-page" => self.show_font_feature_list(page.saturating_sub(1)),
                "next-page" => self.show_font_feature_list(page.saturating_add(1)),
                "back" => self
                    .settings_panel
                    .open_to_section(crate::ui::settings::SettingsSection::Font),
                choice if choice.starts_with("feature:") => {
                    let entry = choice[8..]
                        .parse::<usize>()
                        .ok()
                        .and_then(|index| visible.get(index))
                        .cloned();
                    if let Some((tag, value)) =
                        entry.filter(|(tag, value)| self.font_feature_is_current(tag, *value))
                    {
                        self.show_font_feature_detail(page, tag, value);
                    } else {
                        self.warn_stale_font_feature(page);
                    }
                }
                _ => self.show_font_feature_list(page),
            },
            FontFeatureManagerView::Detail { page, tag, value } => match choice_id {
                "edit" if self.font_feature_is_current(&tag, value) => {
                    let input = format_feature(&tag, value);
                    self.show_font_feature_edit(page, Some((tag, value)), input, None);
                }
                "delete" if self.font_feature_is_current(&tag, value) => {
                    self.show_font_feature_delete_confirmation(page, tag, value);
                }
                "back" => self.show_font_feature_list(page),
                _ => self.warn_stale_font_feature(page),
            },
            FontFeatureManagerView::Edit { page, original } => {
                if choice_id == "cancel" {
                    self.return_from_font_feature_edit(page, original);
                } else {
                    self.show_font_feature_edit(page, original, String::new(), None);
                }
            }
            FontFeatureManagerView::ConfirmDelete { page, tag, value } => match choice_id {
                "confirm" => self.delete_font_feature(page, tag, value),
                "cancel" | "back" => self.show_font_feature_detail(page, tag, value),
                _ => self.show_font_feature_delete_confirmation(page, tag, value),
            },
        }
        true
    }

    pub(super) fn submit_font_feature_manager_input(&mut self, request_id: u64) -> bool {
        let Some(pending) = self
            .pending_font_feature_management
            .as_ref()
            .filter(|pending| pending.request_id == request_id)
            .cloned()
        else {
            return false;
        };
        let FontFeatureManagerView::Edit { page, original } = pending.view else {
            return false;
        };
        let Some(input) = self
            .info_overlay
            .input_value(request_id)
            .map(ToOwned::to_owned)
        else {
            return false;
        };
        self.info_overlay.dismiss(request_id);
        self.pending_font_feature_management = None;
        if let Some((tag, value)) = original.as_ref()
            && !self.font_feature_is_current(tag, *value)
        {
            self.warn_stale_font_feature(page);
            return true;
        }
        let (tag, value) = match parse_font_feature(&input) {
            Ok(feature) => feature,
            Err(error) => {
                self.show_font_feature_edit(page, original, input, Some(&error));
                return true;
            }
        };
        let mut candidate = self.config.clone();
        if let Some((old_tag, old_value)) = original.as_ref() {
            if candidate.font.features.get(old_tag) != Some(old_value) {
                self.warn_stale_font_feature(page);
                return true;
            }
            candidate.font.features.remove(old_tag);
        } else if candidate.font.features.len() >= MAX_FONT_FEATURES {
            self.show_font_feature_edit(
                page,
                None,
                input,
                Some("the maximum of 32 overrides is already configured"),
            );
            return true;
        }
        if candidate.font.features.contains_key(&tag) {
            self.show_font_feature_edit(
                page,
                original,
                input,
                Some("that feature tag already has an explicit value"),
            );
            return true;
        }
        candidate.font.features.insert(tag, value);
        if let Err(error) = candidate.validate() {
            self.show_font_feature_edit(page, original, input, Some(&format!("{error:#}")));
            return true;
        }
        if let Err(error) = self.apply_config(candidate) {
            self.show_font_feature_edit(page, original, input, Some(&error));
            return true;
        }
        self.toasts.success(if original.is_some() {
            "OpenType feature override updated"
        } else {
            "OpenType feature override added"
        });
        self.show_font_feature_list(page);
        true
    }

    pub(super) fn cancel_font_feature_manager_input(&mut self, request_id: u64) -> bool {
        let Some(pending) = self
            .pending_font_feature_management
            .as_ref()
            .filter(|pending| pending.request_id == request_id)
            .cloned()
        else {
            return false;
        };
        let FontFeatureManagerView::Edit { page, original } = pending.view else {
            return false;
        };
        self.info_overlay.dismiss(request_id);
        self.pending_font_feature_management = None;
        self.return_from_font_feature_edit(page, original);
        true
    }

    fn return_from_font_feature_edit(&mut self, page: usize, original: Option<(String, u32)>) {
        if let Some((tag, value)) =
            original.filter(|(tag, value)| self.font_feature_is_current(tag, *value))
        {
            self.show_font_feature_detail(page, tag, value);
        } else {
            self.show_font_feature_list(page);
        }
    }

    fn delete_font_feature(&mut self, page: usize, tag: String, value: u32) {
        let mut candidate = self.config.clone();
        if candidate.font.features.get(&tag) != Some(&value) {
            self.warn_stale_font_feature(page);
            return;
        }
        candidate.font.features.remove(&tag);
        match self.apply_config(candidate) {
            Ok(()) => {
                self.toasts.success("OpenType feature override deleted");
                self.show_font_feature_list(page);
            }
            Err(error) => {
                self.toasts.error(format!(
                    "OpenType feature override could not be deleted: {error}"
                ));
                self.show_font_feature_detail(page, tag, value);
            }
        }
    }

    fn font_feature_is_current(&self, tag: &str, value: u32) -> bool {
        self.config.font.features.get(tag) == Some(&value)
    }

    fn warn_stale_font_feature(&mut self, page: usize) {
        self.toasts
            .warning("OpenType features changed; refreshed the current list");
        self.show_font_feature_list(page);
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
    use crate::ui::info_overlay::InfoOverlayAction;

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
        assert_eq!(app.info_overlay.title(), "OpenType Feature Overrides");
        choose(&mut app, "add");
        submit(&mut app, "ss01 = 1");
        assert_eq!(app.config.font.features.get("ss01"), Some(&1));
        choose(&mut app, "feature:0");
        choose(&mut app, "edit");
        submit(&mut app, "cv01 = 2");
        assert_eq!(app.config.font.features.get("ss01"), None);
        assert_eq!(app.config.font.features.get("cv01"), Some(&2));
        choose(&mut app, "feature:0");
        choose(&mut app, "delete");
        choose(&mut app, "cancel");
        assert_eq!(app.config.font.features.get("cv01"), Some(&2));
        choose(&mut app, "delete");
        choose(&mut app, "confirm");
        assert!(app.config.font.features.is_empty());
    }

    #[test]
    fn feature_manager_preserves_invalid_input_and_rejects_stale_identity() {
        let mut config = Config::default();
        config.font.features.insert("ss01".into(), 1);
        let mut app = App::new_headless(config).unwrap();
        app.open_font_feature_manager();
        choose(&mut app, "feature:0");
        choose(&mut app, "edit");
        submit(&mut app, "kern = 1");
        assert_eq!(app.info_overlay.title(), "Edit OpenType Feature");
        assert_eq!(
            app.info_overlay.input_value(app.info_overlay.request_id()),
            Some("kern = 1")
        );
        assert_eq!(app.config.font.features.get("ss01"), Some(&1));

        app.config.font.features.insert("ss01".into(), 2);
        submit(&mut app, "cv01 = 3");
        assert_eq!(app.config.font.features.get("ss01"), Some(&2));
        assert_eq!(app.config.font.features.get("cv01"), None);
        assert_eq!(app.info_overlay.title(), "OpenType Feature Overrides");
    }

    #[test]
    fn feature_manager_pages_caps_cancels_and_refuses_replaced_requests() {
        let mut config = Config::default();
        config.font.features = (0..MAX_FONT_FEATURES)
            .map(|index| (format!("f{index:03}"), index as u32))
            .collect();
        let mut app = App::new_headless(config).unwrap();
        app.open_font_feature_manager();
        assert!(
            app.info_overlay
                .lines()
                .iter()
                .any(|line| line.contains("maximum of 32"))
        );
        assert!(
            app.info_overlay
                .choice(app.info_overlay.request_id(), "add")
                .is_none()
        );
        choose(&mut app, "next-page");
        assert!(app.info_overlay.lines()[0].contains("page 2/4"));

        let mut app = App::new_headless(Config::default()).unwrap();
        app.open_font_feature_manager();
        choose(&mut app, "add");
        let cancelled = app.info_overlay.request_id();
        app.apply_info_overlay_action(InfoOverlayAction::CancelInput {
            request_id: cancelled,
        });
        assert_eq!(app.info_overlay.title(), "OpenType Feature Overrides");
        assert!(app.config.font.features.is_empty());

        choose(&mut app, "add");
        let replaced = app.info_overlay.request_id();
        app.info_overlay
            .show("Replacement", vec!["new request".into()]);
        assert!(!app.submit_font_feature_manager_input(replaced));
        assert!(app.config.font.features.is_empty());
        let current = app.info_overlay.request_id();
        app.apply_info_overlay_action(InfoOverlayAction::Dismiss {
            request_id: current,
        });
        assert!(app.pending_font_feature_management.is_none());
    }
}
