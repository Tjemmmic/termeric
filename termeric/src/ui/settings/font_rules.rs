//! The Font page's font ranges and OpenType features: a row for each, an
//! Add row, and the sub-page each one opens.

use super::{Choose, SettingItem, SettingKind, SettingsAction, SettingsSection, SettingsSubPage};
use crate::config::{Config, FontRangeOverride, MAX_FONT_FEATURES, MAX_FONT_RANGE_OVERRIDES};
use crate::ui::dialog::ButtonRole;

/// What a font range's rows ask the application to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FontRangeAction {
    /// Ask for a new range and its font.
    Add,
    /// Ask for a new first character.
    EditStart(FontRangeOverride),
    /// Ask for a new last character.
    EditEnd(FontRangeOverride),
    SetFamily {
        rule: FontRangeOverride,
        family: String,
    },
    /// Ask before deleting it.
    Delete(FontRangeOverride),
}

/// What an OpenType feature's rows ask the application to do, naming the
/// feature by its tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FontFeatureAction {
    /// Ask for a new tag and value.
    Add,
    /// Ask for a new tag that keeps the value.
    EditTag(String),
    /// Ask for a new value.
    EditValue(String),
    /// Ask before deleting it.
    Delete(String),
}

pub fn format_codepoint(value: u32) -> String {
    format!("U+{value:04X}")
}

/// A range's first and last characters, "U+0600–U+06FF".
pub fn range_text(rule: &FontRangeOverride) -> String {
    format!(
        "{}–{}",
        format_codepoint(rule.start),
        format_codepoint(rule.end)
    )
}

/// How a range's row and sub-page name it: its Unicode block when it is
/// exactly one, else its first and last characters. Ranges never overlap,
/// so no two share a name.
pub fn range_label(rule: &FontRangeOverride) -> String {
    crate::ui::unicode_blocks::exact_block_name(rule.start, rule.end)
        .map_or_else(|| range_text(rule), str::to_owned)
}

/// The Font page's sections after its own: the ranges and the features.
pub(super) fn page_sections(config: &Config) -> [(SettingsSection, Vec<SettingItem>); 2] {
    [
        (SettingsSection::FontRanges, range_rows(config)),
        (SettingsSection::FontFeatures, feature_rows(config)),
    ]
}

fn range_rows(config: &Config) -> Vec<SettingItem> {
    let mut rules = config.font.range_overrides.clone();
    rules.sort_unstable_by_key(|rule| (rule.start, rule.end));
    let mut rows: Vec<SettingItem> = rules
        .into_iter()
        .map(|rule| SettingItem {
            label: range_label(&rule).into(),
            value: rule.family.clone(),
            kind: SettingKind::Action {
                action: SettingsAction::OpenSubPage(SettingsSubPage::FontRange(rule)),
            },
        })
        .collect();
    rows.push(add_row(
        "Add a range…",
        SettingsAction::FontRange(FontRangeAction::Add),
        config.font.range_overrides.len() < MAX_FONT_RANGE_OVERRIDES,
        format!("At most {MAX_FONT_RANGE_OVERRIDES}; delete one first"),
    ));
    rows
}

fn feature_rows(config: &Config) -> Vec<SettingItem> {
    let mut rows: Vec<SettingItem> = config
        .font
        .features
        .iter()
        .map(|(tag, value)| SettingItem {
            label: tag.clone().into(),
            value: value.to_string(),
            kind: SettingKind::Action {
                action: SettingsAction::OpenSubPage(SettingsSubPage::FontFeature(tag.clone())),
            },
        })
        .collect();
    rows.push(add_row(
        "Add a feature…",
        SettingsAction::FontFeature(FontFeatureAction::Add),
        config.font.features.len() < MAX_FONT_FEATURES,
        format!("At most {MAX_FONT_FEATURES}; delete one first"),
    ));
    rows
}

/// The row that adds one more, or at the limit says why it can't.
fn add_row(label: &'static str, action: SettingsAction, room: bool, full: String) -> SettingItem {
    if room {
        SettingItem {
            label: label.into(),
            value: String::new(),
            kind: SettingKind::Action { action },
        }
    } else {
        SettingItem {
            label: label.into(),
            value: full,
            kind: SettingKind::ReadOnly,
        }
    }
}

/// A font range's own page: its start and end, its font from the installed
/// `families`, and Delete.
pub(super) fn range_page(
    rule: &FontRangeOverride,
    families: &[String],
) -> Vec<(SettingsSection, Vec<SettingItem>)> {
    let mut options = families.to_vec();
    if !options
        .iter()
        .any(|family| family.eq_ignore_ascii_case(&rule.family))
    {
        options.push(rule.family.clone());
    }
    let selected = options
        .iter()
        .position(|family| family.eq_ignore_ascii_case(&rule.family))
        .unwrap_or(0);
    let range = |action| SettingKind::Action {
        action: SettingsAction::FontRange(action),
    };
    vec![(
        SettingsSection::FontRange,
        vec![
            SettingItem {
                label: "Start".into(),
                value: format_codepoint(rule.start),
                kind: range(FontRangeAction::EditStart(rule.clone())),
            },
            SettingItem {
                label: "End".into(),
                value: format_codepoint(rule.end),
                kind: range(FontRangeAction::EditEnd(rule.clone())),
            },
            SettingItem {
                label: "Font".into(),
                value: rule.family.clone(),
                kind: SettingKind::Dropdown {
                    choose: Choose::Each(
                        options
                            .iter()
                            .map(|family| {
                                SettingsAction::FontRange(FontRangeAction::SetFamily {
                                    rule: rule.clone(),
                                    family: family.clone(),
                                })
                            })
                            .collect(),
                    ),
                    options,
                    selected,
                },
            },
            SettingItem {
                label: "Delete…".into(),
                value: String::new(),
                kind: SettingKind::Button {
                    action: SettingsAction::FontRange(FontRangeAction::Delete(rule.clone())),
                    role: ButtonRole::Destructive,
                },
            },
        ],
    )]
}

/// An OpenType feature's own page: its tag, its value, and Delete.
pub(super) fn feature_page(tag: &str, config: &Config) -> Vec<(SettingsSection, Vec<SettingItem>)> {
    let value = config
        .font
        .features
        .get(tag)
        .map_or_else(String::new, u32::to_string);
    let feature = |action| SettingKind::Action {
        action: SettingsAction::FontFeature(action),
    };
    vec![(
        SettingsSection::FontFeature,
        vec![
            SettingItem {
                label: "Tag".into(),
                value: tag.to_owned(),
                kind: feature(FontFeatureAction::EditTag(tag.to_owned())),
            },
            SettingItem {
                label: "Value".into(),
                value,
                kind: feature(FontFeatureAction::EditValue(tag.to_owned())),
            },
            SettingItem {
                label: "Delete…".into(),
                value: String::new(),
                kind: SettingKind::Button {
                    action: SettingsAction::FontFeature(FontFeatureAction::Delete(tag.to_owned())),
                    role: ButtonRole::Destructive,
                },
            },
        ],
    )]
}
