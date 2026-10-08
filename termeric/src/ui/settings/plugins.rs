//! The Plugins page's installed third-party plugins: a row for each with
//! its On/Off switch, Install, and the sub-page each plugin opens.

use super::{Choose, SettingItem, SettingKind, SettingsAction, SettingsSection, SettingsSubPage};
use crate::plugin::permission::PluginPermission;
use crate::plugin::runtime::{DeclarativeSetting, PluginSettingValue, RuntimePluginStatus};
use crate::ui::dialog::ButtonRole;

/// The most characters of a plugin's own text (its name, version, source,
/// settings, and errors) a row shows.
const MAX_SHOWN_CHARS: usize = 512;

/// The labels of the Plugins page's own rows, which a plugin's name must
/// not take.
const PAGE_LABELS: [&str; 3] = ["Status", REGISTRY_LABEL, INSTALL_LABEL];
const REGISTRY_LABEL: &str = "Plugin Registry";
const INSTALL_LABEL: &str = "Install a plugin…";

/// What a plugin's rows ask the application to do, naming the plugin by
/// its ID.
#[derive(Debug, Clone, PartialEq)]
pub enum PluginAction {
    /// Ask for a plugin's folder, then review it.
    Install,
    /// Turn it on, which first reviews any permission it lacks, or off.
    SetEnabled { id: String, enabled: bool },
    /// Grant a requested permission, which asks first, or withhold it.
    SetPermission {
        id: String,
        permission: PluginPermission,
        granted: bool,
    },
    /// Choose the profile launches use, or `None` for the manifest's first.
    SetDefaultProfile { id: String, profile: Option<String> },
    /// Set a setting from its dropdown, or `None` to use its default. The
    /// setting is as the row showed it, so a changed schema is refused.
    SetSetting {
        id: String,
        setting: Box<DeclarativeSetting>,
        value: Option<String>,
    },
    /// Ask for a typed setting's new value.
    EditSetting {
        id: String,
        setting: Box<DeclarativeSetting>,
    },
    /// Review an update from its folder.
    Update(String),
    /// Ask before every setting goes back to its default.
    ResetSettings(String),
    /// Ask before the installed copy is removed.
    Remove(String),
}

/// The installed plugins as the application last read them.
#[derive(Debug, Clone, Default)]
pub struct InstalledPlugins {
    pub statuses: Vec<RuntimePluginStatus>,
    /// Why the registry couldn't be read, if it couldn't.
    pub error: Option<String>,
}

impl InstalledPlugins {
    pub(super) fn get(&self, id: &str) -> Option<&RuntimePluginStatus> {
        self.statuses.iter().find(|status| status.id == id)
    }

    /// Each plugin's name as its row shows it: the name, or with its ID
    /// when another plugin or a row of the page has the same name.
    pub(super) fn label(&self, status: &RuntimePluginStatus) -> String {
        distinct_label(
            &status.name,
            &status.id,
            self.statuses
                .iter()
                .map(|other| other.name.as_str())
                .chain(PAGE_LABELS),
        )
    }
}

fn shown(text: &str) -> String {
    crate::safe_text::display_line(text, MAX_SHOWN_CHARS, 4)
}

/// `name`, or "name (id)" when `names` holds it more than once, so rows,
/// options, and their accessibility names stay unique.
fn distinct_label<'a>(name: &str, id: &str, names: impl Iterator<Item = &'a str>) -> String {
    if names.filter(|other| *other == name).count() > 1 {
        shown(&format!("{name} ({id})"))
    } else {
        shown(name)
    }
}

fn on_off(on: bool) -> &'static str {
    if on { "On" } else { "Off" }
}

/// The Plugins page's rows after its status: each installed plugin, then
/// Install.
pub(super) fn page_rows(plugins: &InstalledPlugins) -> Vec<SettingItem> {
    let mut rows = Vec::new();
    if let Some(error) = &plugins.error {
        rows.push(SettingItem {
            label: REGISTRY_LABEL.into(),
            value: shown(&format!("Couldn't read it: {error}")),
            kind: SettingKind::ReadOnly,
        });
    }
    rows.extend(plugins.statuses.iter().map(|status| {
        let state = if status.diagnostic.is_some() {
            "Error"
        } else {
            on_off(status.enabled)
        };
        SettingItem {
            label: plugins.label(status).into(),
            value: format!("{} · {state}", shown(&status.version)),
            kind: SettingKind::SwitchedAction {
                action: SettingsAction::OpenSubPage(SettingsSubPage::Plugin(status.id.clone())),
                on: status.enabled,
                flip: SettingsAction::Plugin(PluginAction::SetEnabled {
                    id: status.id.clone(),
                    enabled: !status.enabled,
                }),
            },
        }
    }));
    rows.push(SettingItem {
        label: INSTALL_LABEL.into(),
        value: String::new(),
        kind: SettingKind::Action {
            action: SettingsAction::Plugin(PluginAction::Install),
        },
    });
    rows
}

/// A plugin's own page: where it came from, its default profile, its
/// permissions as toggles, its settings by type, and Update, Reset, and
/// Remove.
pub(super) fn plugin_page(
    status: &RuntimePluginStatus,
) -> Vec<(SettingsSection, Vec<SettingItem>)> {
    let id = &status.id;
    let healthy = status.diagnostic.is_none();
    let mut general = vec![
        SettingItem {
            label: "Version".into(),
            value: shown(&status.version),
            kind: SettingKind::ReadOnly,
        },
        SettingItem {
            label: "Source".into(),
            value: shown(&status.source.display().to_string()),
            kind: SettingKind::ReadOnly,
        },
    ];
    if let Some(diagnostic) = &status.diagnostic {
        general.push(SettingItem {
            label: "Problem".into(),
            value: shown(diagnostic),
            kind: SettingKind::ReadOnly,
        });
    }
    if healthy && !status.profiles.is_empty() {
        general.push(default_profile_row(status));
    }
    if healthy && status.requested.is_empty() {
        general.push(SettingItem {
            label: "Permissions".into(),
            value: "None requested".into(),
            kind: SettingKind::ReadOnly,
        });
    }
    let mut sections = vec![(SettingsSection::PluginGeneral, general)];

    if healthy && !status.requested.is_empty() {
        let permissions = status
            .requested
            .iter()
            .map(|permission| {
                let granted = status.granted.covers(permission);
                let set = |granted| {
                    SettingsAction::Plugin(PluginAction::SetPermission {
                        id: id.clone(),
                        permission,
                        granted,
                    })
                };
                SettingItem {
                    label: permission.label().into(),
                    value: on_off(granted).into(),
                    kind: SettingKind::Dropdown {
                        options: vec!["On".into(), "Off".into()],
                        selected: usize::from(!granted),
                        choose: Choose::Each(vec![set(true), set(false)]),
                    },
                }
            })
            .collect();
        sections.push((SettingsSection::PluginPermissions, permissions));
    }

    if healthy && !status.setting_definitions.is_empty() {
        let rows = status
            .setting_definitions
            .iter()
            .map(|setting| {
                let label = distinct_label(
                    setting.name(),
                    setting.id(),
                    status
                        .setting_definitions
                        .iter()
                        .map(DeclarativeSetting::name),
                );
                setting_row(
                    id,
                    setting,
                    label,
                    status.configured_settings.get(setting.id()),
                )
            })
            .collect();
        sections.push((SettingsSection::PluginSettings, rows));
    }

    let mut manage = vec![button(
        "Update…",
        PluginAction::Update(id.clone()),
        ButtonRole::Neutral,
    )];
    if status.configured_setting_count > 0 {
        manage.push(button(
            "Reset settings…",
            PluginAction::ResetSettings(id.clone()),
            ButtonRole::Neutral,
        ));
    }
    manage.push(button(
        "Remove…",
        PluginAction::Remove(id.clone()),
        ButtonRole::Destructive,
    ));
    sections.push((SettingsSection::PluginManage, manage));
    sections
}

fn button(label: &'static str, action: PluginAction, role: ButtonRole) -> SettingItem {
    SettingItem {
        label: label.into(),
        value: String::new(),
        kind: SettingKind::Button {
            action: SettingsAction::Plugin(action),
            role,
        },
    }
}

/// The profile a launch or restore uses when it names none: "Default", the
/// manifest's first, or one chosen.
fn default_profile_row(status: &RuntimePluginStatus) -> SettingItem {
    let names = || status.profiles.iter().map(|profile| profile.name.as_str());
    let first = status
        .profiles
        .first()
        .map_or_else(String::new, |profile| shown(&profile.name));
    let mut options = vec![format!("Default ({first})")];
    let mut actions = vec![SettingsAction::Plugin(PluginAction::SetDefaultProfile {
        id: status.id.clone(),
        profile: None,
    })];
    for profile in &status.profiles {
        options.push(distinct_label(&profile.name, &profile.id, names()));
        actions.push(SettingsAction::Plugin(PluginAction::SetDefaultProfile {
            id: status.id.clone(),
            profile: Some(profile.id.clone()),
        }));
    }
    let selected = status
        .configured_default_profile
        .as_deref()
        .and_then(|chosen| {
            status
                .profiles
                .iter()
                .position(|profile| profile.id == chosen)
        })
        .map_or(0, |index| index + 1);
    SettingItem {
        label: "Default Profile".into(),
        value: options[selected].clone(),
        kind: SettingKind::Dropdown {
            options,
            selected,
            choose: Choose::Each(actions),
        },
    }
}

/// How a setting's value reads in Settings.
pub fn setting_value_text(value: &PluginSettingValue) -> String {
    match value {
        PluginSettingValue::Boolean(value) => on_off(*value).into(),
        PluginSettingValue::Integer(value) => value.to_string(),
        PluginSettingValue::Text(value) => shown(value),
    }
}

/// A plugin setting as an ordinary row of its type: a dropdown for On/Off
/// and choices, whose first option is the default, and a prompt for numbers
/// and text.
fn setting_row(
    id: &str,
    setting: &DeclarativeSetting,
    label: String,
    configured: Option<&PluginSettingValue>,
) -> SettingItem {
    let default = format!("Default ({})", setting_value_text(&setting.default_value()));
    let set = |value: Option<String>| {
        SettingsAction::Plugin(PluginAction::SetSetting {
            id: id.to_owned(),
            setting: Box::new(setting.clone()),
            value,
        })
    };
    let dropdown = |values: Vec<(String, String)>| {
        let selected = configured
            .and_then(|configured| {
                values.iter().position(|(_, value)| match configured {
                    PluginSettingValue::Boolean(on) => *value == on.to_string(),
                    PluginSettingValue::Text(text) => value == text,
                    PluginSettingValue::Integer(_) => false,
                })
            })
            .map_or(0, |index| index + 1);
        let mut options = vec![default.clone()];
        let mut actions = vec![set(None)];
        for (option, value) in values {
            options.push(option);
            actions.push(set(Some(value)));
        }
        SettingKind::Dropdown {
            options,
            selected,
            choose: Choose::Each(actions),
        }
    };
    let kind = match setting {
        DeclarativeSetting::Boolean { .. } => dropdown(vec![
            ("On".into(), "true".into()),
            ("Off".into(), "false".into()),
        ]),
        DeclarativeSetting::Choice { choices, .. } => dropdown(
            choices
                .iter()
                .map(|choice| (shown(choice), choice.clone()))
                .collect(),
        ),
        DeclarativeSetting::Integer { .. } | DeclarativeSetting::Text { .. } => {
            SettingKind::Action {
                action: SettingsAction::Plugin(PluginAction::EditSetting {
                    id: id.to_owned(),
                    setting: Box::new(setting.clone()),
                }),
            }
        }
    };
    let value = match &kind {
        SettingKind::Dropdown {
            options, selected, ..
        } => options[*selected].clone(),
        _ => configured.map_or(default, setting_value_text),
    };
    SettingItem {
        label: label.into(),
        value,
        kind,
    }
}
