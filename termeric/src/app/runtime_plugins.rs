//! Request-scoped in-app management for installed declarative plugins.

use super::*;
use crate::plugin::permission::{PluginPermission, PluginPermissions};
use crate::plugin::runtime::{
    DeclarativeSetting, PluginAdminCommand, PluginConfigurationChange, PluginGrantsPreview,
    PluginInstallPreview, PluginSettingValue, PluginUpdatePreview, RuntimePluginStatus,
    preview_install,
};
use crate::ui::dialog::{
    ButtonRole, CANCEL, DialogBlock, DialogButton, DialogRow, DialogSpec, FieldSpec,
    MAX_FIELD_BYTES,
};

/// The answer that submits a prompt's field.
const SAVE: &str = "save";

#[derive(Debug, Clone)]
pub(super) struct PendingRuntimePluginManagement {
    request_id: u64,
    view: RuntimePluginManagerView,
}

#[derive(Debug, Clone)]
enum RuntimePluginManagerView {
    List,
    InstallPath,
    ReviewInstall {
        preview: crate::plugin::runtime::PluginInstallPreview,
        grants: crate::plugin::permission::PluginPermissions,
    },
    ReviewUpdate {
        preview: Box<crate::plugin::runtime::PluginUpdatePreview>,
        grants: crate::plugin::permission::PluginPermissions,
    },
    ReviewPermissions {
        preview: Box<crate::plugin::runtime::PluginGrantsPreview>,
        grants: crate::plugin::permission::PluginPermissions,
    },
    Detail {
        plugin_id: String,
    },
    DefaultProfile {
        plugin_id: String,
    },
    Settings {
        plugin_id: String,
    },
    Setting {
        plugin_id: String,
        setting: crate::plugin::runtime::DeclarativeSetting,
    },
    EditSetting {
        plugin_id: String,
        setting: crate::plugin::runtime::DeclarativeSetting,
    },
    Confirm {
        plugin_id: String,
        operation: RuntimePluginOperation,
    },
    /// Something failed; Back returns to the plugin, or the list.
    Failed {
        plugin_id: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimePluginOperation {
    ResetSettings,
    Remove,
}

impl App {
    pub(super) fn pending_runtime_plugin_management(
        &self,
    ) -> Option<&PendingRuntimePluginManagement> {
        match &self.dialog_flow {
            Some(DialogFlow::RuntimePlugins(pending)) => Some(pending),
            _ => None,
        }
    }

    pub(super) fn discover_runtime_plugins_for_app(
        &self,
    ) -> Result<(
        Vec<crate::plugin::runtime::DeclarativePlugin>,
        Vec<RuntimePluginStatus>,
    )> {
        // Without a configuration directory no plugin can be installed.
        let Some(paths) = &self.runtime_plugin_paths else {
            return Ok((Vec::new(), Vec::new()));
        };
        crate::plugin::runtime::discover_runtime_plugins_at(&paths.registry, &paths.root)
    }

    /// Where plugins live, or why there is nowhere for them.
    fn runtime_plugin_paths(&self) -> Result<&crate::plugin::runtime::PluginPaths> {
        self.runtime_plugin_paths
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!(crate::paths::NO_HOME_DIRECTORY))
    }

    fn execute_runtime_plugin_admin(&self, command: PluginAdminCommand) -> Result<String> {
        let paths = self.runtime_plugin_paths()?;
        crate::plugin::runtime::execute_admin_at(command, &paths.registry, &paths.root)
    }

    fn preview_runtime_plugin_update(&self, plugin_id: &str) -> Result<PluginUpdatePreview> {
        let paths = self.runtime_plugin_paths()?;
        crate::plugin::runtime::preview_update_at(plugin_id, &paths.registry, &paths.root)
    }

    #[cfg(test)]
    pub(crate) fn set_runtime_plugin_test_paths(&mut self, registry: PathBuf, root: PathBuf) {
        self.runtime_plugin_paths = Some(crate::plugin::runtime::PluginPaths { registry, root });
        self.sync_plugins_from_config();
    }

    pub(super) fn open_runtime_plugin_manager(&mut self) {
        self.settings_panel.close();
        self.dialog_flow = None;
        self.show_runtime_plugin_list();
    }

    fn show_runtime_plugin_flow(&mut self, spec: DialogSpec, view: RuntimePluginManagerView) {
        let request_id = self.dialog.show(spec);
        self.dialog_flow = Some(DialogFlow::RuntimePlugins(PendingRuntimePluginManagement {
            request_id,
            view,
        }));
    }

    fn show_runtime_plugin_list(&mut self) {
        let statuses = match self.discover_runtime_plugins_for_app() {
            Ok((_, statuses)) => statuses,
            Err(error) => {
                self.show_runtime_plugin_error(
                    None,
                    "Couldn't read the plugin registry",
                    &format!("{error:#}"),
                );
                return;
            }
        };
        let mut body = vec![DialogBlock::text(
            "Pick a plugin to see where it came from, what it may do, and its settings.",
        )];
        if statuses.is_empty() {
            body.push(DialogBlock::text("No third-party plugins are installed."));
        }
        let rows = statuses
            .iter()
            .map(|status| {
                let health = if status.diagnostic.is_some() {
                    "error"
                } else if status.enabled {
                    "on"
                } else {
                    "off"
                };
                DialogRow::new(format!("plugin:{}", status.id), status.name.as_str())
                    .with_detail(format!("{} · {health}", status.version))
            })
            .collect();
        let spec = DialogSpec::list("Third-party plugins", body, rows)
            .button(DialogButton::answer(CANCEL, "Close", ButtonRole::Cancel))
            .button(DialogButton::answer(
                "install",
                "Install…",
                ButtonRole::Suggested,
            ));
        self.show_runtime_plugin_flow(spec, RuntimePluginManagerView::List);
    }

    fn show_runtime_plugin_install_path(&mut self, initial: String, error: Option<&str>) {
        let spec = DialogSpec::prompt(
            "Install a plugin",
            vec![DialogBlock::text(format!(
                "Type the full path of the plugin's folder. It must hold a {} file.",
                crate::plugin::runtime::MANIFEST_NAME
            ))],
            FieldSpec::new("Folder", initial, MAX_FIELD_BYTES)
                .with_error(error.map(crate::safe_text::capitalized)),
            SAVE,
            "Review",
        );
        self.show_runtime_plugin_flow(spec, RuntimePluginManagerView::InstallPath);
    }

    fn show_runtime_plugin_install_review(
        &mut self,
        preview: PluginInstallPreview,
        grants: PluginPermissions,
        error: Option<&str>,
    ) {
        let mut body = Vec::new();
        if !preview.description.is_empty() {
            body.push(DialogBlock::text(preview.description.as_str()));
        }
        body.push(DialogBlock::value(
            [
                format!("{} {}", preview.id, preview.version),
                format!(
                    "{} profiles · {} commands",
                    preview.profile_count, preview.command_count
                ),
                format!("From {}", preview.source.display()),
                format!("Fingerprint {}", preview.source_fingerprint),
            ]
            .join("\n"),
        ));
        if let Some(error) = error {
            body.push(DialogBlock::error(format!("Couldn't install it: {error}")));
        }
        let enable = grants == preview.permissions;
        push_permission_prompt(&mut body, &preview.permissions, enable);
        let rows = permission_rows(&preview.permissions, &grants, |_| None);
        let mut spec = DialogSpec::list(format!("Install {}?", preview.name), body, rows)
            .button(DialogButton::back())
            .button(DialogButton::answer(
                "install-disabled",
                "Install turned off",
                ButtonRole::Neutral,
            ));
        if enable {
            spec = spec.button(DialogButton::answer(
                "install-enabled",
                "Install and turn on",
                ButtonRole::Suggested,
            ));
        }
        self.show_runtime_plugin_flow(
            spec,
            RuntimePluginManagerView::ReviewInstall { preview, grants },
        );
    }

    fn open_runtime_plugin_update_review(&mut self, plugin_id: String) {
        match self.preview_runtime_plugin_update(&plugin_id) {
            Ok(preview) => {
                let grants = preview
                    .current_grants
                    .intersection(&preview.candidate_permissions);
                self.show_runtime_plugin_update_review(Box::new(preview), grants, None);
            }
            Err(error) => {
                let error = format!("{error:#}");
                self.toasts
                    .error(format!("Plugin update preview failed: {error}"));
                self.show_runtime_plugin_error(
                    Some(plugin_id),
                    "Couldn't check for an update",
                    &error,
                );
            }
        }
    }

    fn show_runtime_plugin_update_review(
        &mut self,
        preview: Box<PluginUpdatePreview>,
        grants: PluginPermissions,
        error: Option<&str>,
    ) {
        let added = preview
            .candidate_permissions
            .uncovered_by(&preview.current_permissions);
        let removed = preview
            .current_permissions
            .uncovered_by(&preview.candidate_permissions);
        let mut body = vec![DialogBlock::value(
            [
                format!(
                    "{} {} → {}",
                    preview.id, preview.current_version, preview.candidate_version
                ),
                format!(
                    "{} → {} profiles · {} → {} commands",
                    preview.current_profile_count,
                    preview.candidate_profile_count,
                    preview.current_command_count,
                    preview.candidate_command_count
                ),
                format!("From {}", preview.source.display()),
                format!(
                    "Installed fingerprint {}",
                    preview.current_source_fingerprint
                ),
                format!("New fingerprint {}", preview.candidate_source_fingerprint),
                format!(
                    "Newly requested: {}",
                    display_permissions(added.iter().copied())
                ),
                format!(
                    "No longer requested: {}",
                    display_permissions(removed.iter().copied())
                ),
            ]
            .join("\n"),
        )];
        if preview.default_profile_will_be_cleared {
            body.push(DialogBlock::text(format!(
                "The default profile {:?} isn't in the new version, so profiles go back to manifest order.",
                preview.configured_default_profile.as_deref().unwrap_or_default()
            )));
        }
        if let Some(error) = error {
            body.push(DialogBlock::error(format!("Couldn't update it: {error}")));
        }
        let enable = grants == preview.candidate_permissions;
        push_permission_prompt(&mut body, &preview.candidate_permissions, enable);
        let rows = permission_rows(&preview.candidate_permissions, &grants, |permission| {
            added.contains(&permission).then_some("new")
        });
        let (disabled_label, enabled_label) = if preview.enabled {
            ("Update and turn off", "Update and keep on")
        } else {
            ("Update turned off", "Update and turn on")
        };
        let mut spec = DialogSpec::list(format!("Update {}?", preview.name), body, rows)
            .button(DialogButton::back())
            .button(DialogButton::answer(
                "update-disabled",
                disabled_label,
                ButtonRole::Neutral,
            ));
        if enable {
            spec = spec.button(DialogButton::answer(
                "update-enabled",
                enabled_label,
                ButtonRole::Suggested,
            ));
        }
        self.show_runtime_plugin_flow(
            spec,
            RuntimePluginManagerView::ReviewUpdate { preview, grants },
        );
    }

    fn open_runtime_plugin_permission_review(&mut self, plugin_id: String) {
        let preview = self.runtime_plugin_paths().and_then(|paths| {
            crate::plugin::runtime::preview_grants_at(&plugin_id, &paths.registry, &paths.root)
        });
        match preview {
            Ok(preview) => {
                // A grant narrower than its request is shown ungranted, and
                // selecting it grants the requested scope.
                let grants = preview.current_grants.intersection(&preview.requested);
                self.show_runtime_plugin_permission_review(Box::new(preview), grants, None);
            }
            Err(error) => self.show_runtime_plugin_error(
                Some(plugin_id),
                "Couldn't read its permissions",
                &format!("{error:#}"),
            ),
        }
    }

    fn show_runtime_plugin_permission_review(
        &mut self,
        preview: Box<PluginGrantsPreview>,
        grants: PluginPermissions,
        error: Option<&str>,
    ) {
        let mut body = vec![DialogBlock::value(format!(
            "{} {} · {}",
            preview.id,
            preview.version,
            if preview.enabled { "on" } else { "off" }
        ))];
        if let Some(error) = error {
            body.push(DialogBlock::error(format!(
                "Couldn't change its permissions: {error}"
            )));
        }
        let enable = preview.requested.uncovered_by(&grants).is_empty();
        push_permission_prompt(&mut body, &preview.requested, enable);
        let rows = permission_rows(&preview.requested, &grants, |_| None);
        let (disabled_label, enabled_label) = if preview.enabled {
            ("Save and turn off", "Save and keep on")
        } else {
            ("Save turned off", "Save and turn on")
        };
        let mut spec = DialogSpec::list(format!("{} permissions", preview.name), body, rows)
            .button(DialogButton::back())
            .button(DialogButton::answer(
                "save-disabled",
                disabled_label,
                ButtonRole::Neutral,
            ));
        if enable {
            spec = spec.button(DialogButton::answer(
                "save-enabled",
                enabled_label,
                ButtonRole::Suggested,
            ));
        }
        self.show_runtime_plugin_flow(
            spec,
            RuntimePluginManagerView::ReviewPermissions { preview, grants },
        );
    }

    fn show_runtime_plugin_detail(&mut self, plugin_id: String) {
        let status = match self.discover_runtime_plugins_for_app() {
            Ok((_, statuses)) => statuses.into_iter().find(|status| status.id == plugin_id),
            Err(error) => {
                self.show_runtime_plugin_error(
                    None,
                    "Couldn't read the plugin registry",
                    &format!("{error:#}"),
                );
                return;
            }
        };
        let Some(status) = status else {
            self.toasts.warning(format!(
                "Plugin {plugin_id} is no longer installed; refreshed the list"
            ));
            self.show_runtime_plugin_list();
            return;
        };

        let default_profile = if let Some(profile) = status
            .configured_default_profile
            .as_deref()
            .and_then(|id| status.profiles.iter().find(|profile| profile.id == id))
            .or_else(|| status.profiles.first())
        {
            let origin = if status.configured_default_profile.is_some() {
                "chosen"
            } else {
                "manifest order"
            };
            format!(
                "Default profile: {} ({}), {origin}",
                profile.name, profile.id
            )
        } else if status.diagnostic.is_some() {
            "Default profile: none while the plugin has an error".into()
        } else {
            "Default profile: none (no launch profiles)".into()
        };
        let mut details = vec![
            format!(
                "{} {} · {}",
                status.id,
                status.version,
                if status.enabled { "on" } else { "off" }
            ),
            format!("From {}", status.source.display()),
            default_profile,
            format!(
                "Requested: {}",
                display_permissions(status.requested.iter())
            ),
            format!("Granted: {}", display_permissions(status.granted.iter())),
            format!(
                "{} settings, {} changed",
                status.settings.len(),
                status.configured_setting_count
            ),
        ];
        details.extend(
            status
                .settings
                .iter()
                .map(|(key, value)| format!("  {key} = {}", display_setting_value(value))),
        );
        let mut body = vec![DialogBlock::value(details.join("\n"))];
        if let Some(diagnostic) = &status.diagnostic {
            body.push(DialogBlock::error(diagnostic.as_str()));
        }

        let mut rows = Vec::new();
        if status.diagnostic.is_none() && !status.profiles.is_empty() {
            rows.push(DialogRow::new("choose-default-profile", "Default profile…"));
        }
        if status.diagnostic.is_none() && !status.setting_definitions.is_empty() {
            rows.push(DialogRow::new("configure-settings", "Settings…"));
        }
        rows.push(DialogRow::new(
            "toggle",
            if status.enabled {
                "Turn off"
            } else {
                "Turn on"
            },
        ));
        if status.diagnostic.is_none() {
            rows.push(DialogRow::new("permissions", "Permissions…"));
        }
        rows.push(DialogRow::new("update", "Update from its folder…"));
        if status.configured_setting_count > 0 {
            rows.push(DialogRow::new("reset-settings", "Reset settings…"));
        }
        rows.push(DialogRow::new("remove", "Remove…"));

        let spec = DialogSpec::list(status.name.as_str(), body, rows).button(DialogButton::back());
        self.show_runtime_plugin_flow(spec, RuntimePluginManagerView::Detail { plugin_id });
    }

    fn show_runtime_plugin_default_profiles(&mut self, plugin_id: String) {
        let Some(status) = self.current_runtime_plugin_status(&plugin_id) else {
            return;
        };
        if status.diagnostic.is_some() || status.profiles.is_empty() {
            self.show_runtime_plugin_detail(plugin_id);
            return;
        }
        let effective_id = status
            .configured_default_profile
            .as_deref()
            .or_else(|| status.profiles.first().map(|profile| profile.id.as_str()));
        let rows = status
            .profiles
            .iter()
            .map(|profile| {
                let row = DialogRow::new(
                    format!("profile:{}", profile.id),
                    format!("{} ({})", profile.name, profile.id),
                );
                if effective_id == Some(profile.id.as_str()) {
                    row.with_detail("Default")
                } else if !profile.description.is_empty() {
                    row.with_detail(profile.description.as_str())
                } else {
                    row
                }
            })
            .collect();
        let origin = if status.configured_default_profile.is_some() {
            "You chose the default."
        } else {
            "The default is the first in the manifest."
        };
        let mut spec = DialogSpec::list(
            format!("Default profile for {}", status.name),
            vec![DialogBlock::text(format!(
                "It's used when a launch or restore doesn't name a profile. {origin}"
            ))],
            rows,
        )
        .button(DialogButton::back());
        if status.configured_default_profile.is_some() {
            spec = spec.button(DialogButton::answer(
                "manifest-order",
                "Use manifest order",
                ButtonRole::Neutral,
            ));
        }
        self.show_runtime_plugin_flow(spec, RuntimePluginManagerView::DefaultProfile { plugin_id });
    }

    fn show_runtime_plugin_settings(&mut self, plugin_id: String) {
        let Some(status) = self.current_runtime_plugin_status(&plugin_id) else {
            return;
        };
        if status.diagnostic.is_some() {
            self.show_runtime_plugin_detail(plugin_id);
            return;
        }
        let mut body = vec![DialogBlock::text("Pick a setting to change it.")];
        if status.setting_definitions.is_empty() {
            body.push(DialogBlock::text("This plugin has no settings."));
        }
        let rows = status
            .setting_definitions
            .iter()
            .map(|setting| {
                let id = setting.id();
                let value = status
                    .settings
                    .get(id)
                    .map(display_setting_value)
                    .unwrap_or_else(|| "unavailable".into());
                let changed = if status.configured_settings.contains_key(id) {
                    ", changed"
                } else {
                    ""
                };
                DialogRow::new(
                    format!("setting:{id}"),
                    format!("{} ({id})", setting.name()),
                )
                .with_detail(format!("{value}{changed}"))
            })
            .collect();
        let spec = DialogSpec::list(format!("{} settings", status.name), body, rows)
            .button(DialogButton::back());
        self.show_runtime_plugin_flow(spec, RuntimePluginManagerView::Settings { plugin_id });
    }

    fn show_runtime_plugin_setting(&mut self, plugin_id: String, setting: DeclarativeSetting) {
        let Some(status) = self.current_runtime_plugin_status(&plugin_id) else {
            return;
        };
        let Some(current) = status
            .setting_definitions
            .iter()
            .find(|candidate| candidate.id() == setting.id())
        else {
            self.toasts.warning(format!(
                "Setting {} no longer exists; refreshed plugin settings",
                setting.id()
            ));
            self.show_runtime_plugin_settings(plugin_id);
            return;
        };
        if current != &setting {
            self.toasts.warning(format!(
                "Setting {} changed after the editor opened; review it again",
                setting.id()
            ));
            self.show_runtime_plugin_settings(plugin_id);
            return;
        }
        let effective = status
            .settings
            .get(setting.id())
            .cloned()
            .unwrap_or_else(|| setting.default_value());
        let configured = status.configured_settings.contains_key(setting.id());
        let mut details = vec![
            format!("{} · {}", setting.id(), setting_kind(&setting)),
            format!(
                "Now {}{}",
                display_setting_value(&effective),
                if configured {
                    ", changed"
                } else {
                    ", the default"
                }
            ),
            format!(
                "Default {}",
                display_setting_value(&setting.default_value())
            ),
        ];
        match &setting {
            DeclarativeSetting::Integer { min, max, .. } => {
                details.push(format!("From {min} to {max}"));
            }
            DeclarativeSetting::Text { .. } => details.push(format!(
                "One line, at most {} UTF-8 bytes",
                crate::plugin::runtime::MAX_SETTING_TEXT_BYTES
            )),
            DeclarativeSetting::Boolean { .. } | DeclarativeSetting::Choice { .. } => {}
        }
        let mut body = Vec::new();
        if !setting.description().is_empty() {
            body.push(DialogBlock::text(setting.description()));
        }
        body.push(DialogBlock::value(details.join("\n")));
        let use_default = || DialogButton::answer("unset", "Use default", ButtonRole::Neutral);
        let title = format!("Change {}", setting.name());

        match &setting {
            DeclarativeSetting::Integer { .. } | DeclarativeSetting::Text { .. } => {
                let initial = match effective {
                    PluginSettingValue::Integer(value) => value.to_string(),
                    PluginSettingValue::Text(value) => value,
                    PluginSettingValue::Boolean(_) => String::new(),
                };
                let max_bytes = if matches!(setting, DeclarativeSetting::Integer { .. }) {
                    32
                } else {
                    crate::plugin::runtime::MAX_SETTING_TEXT_BYTES
                };
                let mut spec = DialogSpec::prompt(
                    title,
                    body,
                    FieldSpec::new("Value", initial, max_bytes),
                    SAVE,
                    "Save",
                );
                if configured {
                    spec.buttons.insert(1, use_default());
                }
                self.show_runtime_plugin_flow(
                    spec,
                    RuntimePluginManagerView::EditSetting { plugin_id, setting },
                );
            }
            DeclarativeSetting::Boolean { .. } | DeclarativeSetting::Choice { .. } => {
                let current = display_setting_value(&effective);
                let rows: Vec<DialogRow> = match &setting {
                    DeclarativeSetting::Choice { choices, .. } => choices
                        .iter()
                        .enumerate()
                        .map(|(index, value)| {
                            DialogRow::new(format!("value:{index}"), value.as_str())
                        })
                        .collect(),
                    _ => ["true", "false"]
                        .into_iter()
                        .map(|value| DialogRow::new(format!("value:{value}"), value))
                        .collect(),
                };
                let rows = rows
                    .into_iter()
                    .map(|row| {
                        if row.label == current || format!("{:?}", row.label) == current {
                            row.with_detail("Now")
                        } else {
                            row
                        }
                    })
                    .collect();
                let mut spec = DialogSpec::list(title, body, rows).button(DialogButton::back());
                if configured {
                    spec = spec.button(use_default());
                }
                self.show_runtime_plugin_flow(
                    spec,
                    RuntimePluginManagerView::Setting { plugin_id, setting },
                );
            }
        }
    }

    fn current_runtime_plugin_status(&mut self, plugin_id: &str) -> Option<RuntimePluginStatus> {
        match self.discover_runtime_plugins_for_app() {
            Ok((_, statuses)) => {
                let status = statuses.into_iter().find(|status| status.id == plugin_id);
                if status.is_none() {
                    self.toasts.warning(format!(
                        "Plugin {plugin_id} is no longer installed; refreshed the list"
                    ));
                    self.show_runtime_plugin_list();
                }
                status
            }
            Err(error) => {
                self.show_runtime_plugin_error(
                    None,
                    "Couldn't read the plugin registry",
                    &format!("{error:#}"),
                );
                None
            }
        }
    }

    fn show_runtime_plugin_confirmation(
        &mut self,
        plugin_id: String,
        operation: RuntimePluginOperation,
    ) {
        let spec = match operation {
            RuntimePluginOperation::ResetSettings => DialogSpec::confirm(
                format!("Reset the settings of {plugin_id}?"),
                vec![DialogBlock::text("Every setting goes back to its default.")],
                "confirm",
                "Reset",
                ButtonRole::Destructive,
            ),
            RuntimePluginOperation::Remove => DialogSpec::confirm(
                format!("Remove {plugin_id}?"),
                vec![DialogBlock::text(
                    "Its installed copy and registry entry are deleted. The folder it came from is left alone.",
                )],
                "confirm",
                "Remove",
                ButtonRole::Destructive,
            ),
        };
        self.show_runtime_plugin_flow(
            spec,
            RuntimePluginManagerView::Confirm {
                plugin_id,
                operation,
            },
        );
    }

    /// A failure with the plugin it happened to, if any, to go back to.
    fn show_runtime_plugin_error(&mut self, plugin_id: Option<String>, title: &str, error: &str) {
        let mut spec = DialogSpec::error(title, vec![DialogBlock::value(error)]);
        spec.buttons = vec![
            DialogButton::copy_details(),
            DialogButton::answer("back", "Back", ButtonRole::Suggested),
        ];
        self.show_runtime_plugin_flow(spec, RuntimePluginManagerView::Failed { plugin_id });
    }

    /// Answer the manager's dialog. Returns whether the answer was its.
    pub(super) fn handle_runtime_plugin_manager_choice(
        &mut self,
        request_id: u64,
        choice_id: &str,
    ) -> bool {
        let Some(pending) = self
            .pending_runtime_plugin_management()
            .filter(|pending| pending.request_id == request_id)
            .cloned()
        else {
            return false;
        };
        if choice_id == SAVE
            && matches!(
                pending.view,
                RuntimePluginManagerView::InstallPath
                    | RuntimePluginManagerView::EditSetting { .. }
            )
        {
            self.submit_runtime_plugin_field(request_id, pending.view);
            return true;
        }
        self.dialog.dismiss(request_id);
        self.dialog_flow = None;
        match pending.view {
            RuntimePluginManagerView::List => match choice_id {
                "install" => self.show_runtime_plugin_install_path(String::new(), None),
                choice if choice.starts_with("plugin:") => {
                    self.show_runtime_plugin_detail(choice["plugin:".len()..].to_string());
                }
                // Close leaves the manager.
                _ => {}
            },
            RuntimePluginManagerView::InstallPath => self.show_runtime_plugin_list(),
            RuntimePluginManagerView::ReviewInstall {
                preview,
                mut grants,
            } => match choice_id {
                "grant-all" => {
                    grants = preview.permissions.clone();
                    self.show_runtime_plugin_install_review(preview, grants, None);
                }
                "clear-grants" => self.show_runtime_plugin_install_review(
                    preview,
                    PluginPermissions::default(),
                    None,
                ),
                "install-disabled" => self.apply_runtime_plugin_install(preview, grants, false),
                "install-enabled" if grants == preview.permissions => {
                    self.apply_runtime_plugin_install(preview, grants, true)
                }
                CANCEL => self
                    .show_runtime_plugin_install_path(preview.source.display().to_string(), None),
                choice if choice.starts_with("permission:") => {
                    toggle_requested_grant(
                        &mut grants,
                        &preview.permissions,
                        &choice["permission:".len()..],
                    );
                    self.show_runtime_plugin_install_review(preview, grants, None);
                }
                _ => self.show_runtime_plugin_install_review(preview, grants, None),
            },
            RuntimePluginManagerView::ReviewUpdate {
                preview,
                mut grants,
            } => match choice_id {
                "grant-all" => {
                    grants = preview.candidate_permissions.clone();
                    self.show_runtime_plugin_update_review(preview, grants, None);
                }
                "clear-grants" => self.show_runtime_plugin_update_review(
                    preview,
                    PluginPermissions::default(),
                    None,
                ),
                "update-disabled" => self.apply_runtime_plugin_update(preview, grants, false),
                "update-enabled" if grants == preview.candidate_permissions => {
                    self.apply_runtime_plugin_update(preview, grants, true)
                }
                CANCEL => self.show_runtime_plugin_detail(preview.id.clone()),
                choice if choice.starts_with("permission:") => {
                    toggle_requested_grant(
                        &mut grants,
                        &preview.candidate_permissions,
                        &choice["permission:".len()..],
                    );
                    self.show_runtime_plugin_update_review(preview, grants, None);
                }
                _ => self.show_runtime_plugin_update_review(preview, grants, None),
            },
            RuntimePluginManagerView::ReviewPermissions {
                preview,
                mut grants,
            } => match choice_id {
                "grant-all" => {
                    grants = preview.requested.clone();
                    self.show_runtime_plugin_permission_review(preview, grants, None);
                }
                "clear-grants" => self.show_runtime_plugin_permission_review(
                    preview,
                    PluginPermissions::default(),
                    None,
                ),
                "save-disabled" => self.apply_runtime_plugin_grants(preview, grants, false),
                "save-enabled" if preview.requested.uncovered_by(&grants).is_empty() => {
                    self.apply_runtime_plugin_grants(preview, grants, true)
                }
                CANCEL => self.show_runtime_plugin_detail(preview.id.clone()),
                choice if choice.starts_with("permission:") => {
                    toggle_requested_grant(
                        &mut grants,
                        &preview.requested,
                        &choice["permission:".len()..],
                    );
                    self.show_runtime_plugin_permission_review(preview, grants, None);
                }
                _ => self.show_runtime_plugin_permission_review(preview, grants, None),
            },
            RuntimePluginManagerView::Detail { plugin_id } => match choice_id {
                "choose-default-profile" => self.show_runtime_plugin_default_profiles(plugin_id),
                "configure-settings" => self.show_runtime_plugin_settings(plugin_id),
                "toggle" => {
                    let Some(status) = self.current_runtime_plugin_status(&plugin_id) else {
                        return true;
                    };
                    if status.enabled {
                        let command = PluginAdminCommand::Disable {
                            id: plugin_id.clone(),
                        };
                        self.apply_runtime_plugin_operation(command, plugin_id, false);
                    } else if status.requested.uncovered_by(&status.granted).is_empty() {
                        let command = PluginAdminCommand::Enable {
                            id: plugin_id.clone(),
                            grants: PluginPermissions::default(),
                        };
                        self.apply_runtime_plugin_operation(command, plugin_id, false);
                    } else {
                        // Enabling needs every request granted: review them.
                        self.open_runtime_plugin_permission_review(plugin_id);
                    }
                }
                "permissions" => self.open_runtime_plugin_permission_review(plugin_id),
                "update" => self.open_runtime_plugin_update_review(plugin_id),
                "reset-settings" => self.show_runtime_plugin_confirmation(
                    plugin_id,
                    RuntimePluginOperation::ResetSettings,
                ),
                "remove" => {
                    self.show_runtime_plugin_confirmation(plugin_id, RuntimePluginOperation::Remove)
                }
                CANCEL => self.show_runtime_plugin_list(),
                _ => self.show_runtime_plugin_detail(plugin_id),
            },
            RuntimePluginManagerView::DefaultProfile { plugin_id } => match choice_id {
                "manifest-order" => self.apply_runtime_plugin_default_profile_change(
                    plugin_id,
                    PluginConfigurationChange::ClearDefaultProfile,
                ),
                CANCEL => self.show_runtime_plugin_detail(plugin_id),
                choice if choice.starts_with("profile:") => {
                    let profile_id = &choice["profile:".len()..];
                    let exists =
                        self.current_runtime_plugin_status(&plugin_id)
                            .is_some_and(|status| {
                                status
                                    .profiles
                                    .iter()
                                    .any(|profile| profile.id == profile_id)
                            });
                    if exists {
                        self.apply_runtime_plugin_default_profile_change(
                            plugin_id,
                            PluginConfigurationChange::DefaultProfile(profile_id.into()),
                        );
                    } else {
                        self.toasts.warning(format!(
                            "Profile {profile_id:?} no longer exists; refreshed plugin profiles"
                        ));
                        self.show_runtime_plugin_default_profiles(plugin_id);
                    }
                }
                _ => self.show_runtime_plugin_default_profiles(plugin_id),
            },
            RuntimePluginManagerView::Settings { plugin_id } => match choice_id {
                CANCEL => self.show_runtime_plugin_detail(plugin_id),
                choice if choice.starts_with("setting:") => {
                    let setting_id = &choice["setting:".len()..];
                    let setting =
                        self.current_runtime_plugin_status(&plugin_id)
                            .and_then(|status| {
                                status
                                    .setting_definitions
                                    .into_iter()
                                    .find(|setting| setting.id() == setting_id)
                            });
                    if let Some(setting) = setting {
                        self.show_runtime_plugin_setting(plugin_id, setting);
                    } else {
                        self.show_runtime_plugin_settings(plugin_id);
                    }
                }
                _ => self.show_runtime_plugin_settings(plugin_id),
            },
            RuntimePluginManagerView::Setting { plugin_id, setting } => {
                if choice_id == CANCEL {
                    self.show_runtime_plugin_settings(plugin_id);
                } else if choice_id == "unset" {
                    if self.runtime_plugin_setting_schema_is_current(&plugin_id, &setting) {
                        let key = setting.id().to_string();
                        self.apply_runtime_plugin_setting_change(
                            plugin_id,
                            PluginConfigurationChange::Unset { key },
                        );
                    }
                } else if let Some(encoded) = choice_id.strip_prefix("value:") {
                    let value = match &setting {
                        DeclarativeSetting::Boolean { .. }
                            if matches!(encoded, "true" | "false") =>
                        {
                            encoded.to_string()
                        }
                        DeclarativeSetting::Choice { choices, .. } => encoded
                            .parse::<usize>()
                            .ok()
                            .and_then(|index| choices.get(index).cloned())
                            .unwrap_or_default(),
                        _ => String::new(),
                    };
                    if !value.is_empty()
                        && self.runtime_plugin_setting_schema_is_current(&plugin_id, &setting)
                    {
                        self.apply_runtime_plugin_setting_change(
                            plugin_id,
                            PluginConfigurationChange::Set {
                                key: setting.id().to_string(),
                                value,
                            },
                        );
                    }
                }
            }
            RuntimePluginManagerView::EditSetting { plugin_id, setting } => {
                if choice_id == "unset"
                    && self.runtime_plugin_setting_schema_is_current(&plugin_id, &setting)
                {
                    self.apply_runtime_plugin_setting_change(
                        plugin_id,
                        PluginConfigurationChange::Unset {
                            key: setting.id().to_string(),
                        },
                    );
                } else if choice_id == CANCEL {
                    self.show_runtime_plugin_settings(plugin_id);
                }
            }
            RuntimePluginManagerView::Confirm {
                plugin_id,
                operation,
            } => match choice_id {
                "confirm" => {
                    let command = match operation {
                        RuntimePluginOperation::ResetSettings => PluginAdminCommand::Configure {
                            id: plugin_id.clone(),
                            change: PluginConfigurationChange::ResetSettings,
                        },
                        RuntimePluginOperation::Remove => PluginAdminCommand::Remove {
                            id: plugin_id.clone(),
                        },
                    };
                    self.apply_runtime_plugin_operation(
                        command,
                        plugin_id,
                        operation == RuntimePluginOperation::Remove,
                    );
                }
                _ => self.show_runtime_plugin_detail(plugin_id),
            },
            RuntimePluginManagerView::Failed { plugin_id } => match plugin_id {
                Some(plugin_id) => self.show_runtime_plugin_detail(plugin_id),
                None => self.show_runtime_plugin_list(),
            },
        }
        true
    }

    /// Submit the field of the install-path or setting prompt.
    fn submit_runtime_plugin_field(&mut self, request_id: u64, view: RuntimePluginManagerView) {
        let Some(value) = self.dialog.field_value(request_id).map(ToOwned::to_owned) else {
            return;
        };
        match view {
            RuntimePluginManagerView::InstallPath => {
                self.dialog.dismiss(request_id);
                self.dialog_flow = None;
                let directory = PathBuf::from(&value);
                let preview = if value.is_empty() {
                    Err(anyhow::anyhow!("type the plugin's folder"))
                } else if !directory.is_absolute() {
                    Err(anyhow::anyhow!("type the full path, starting with /"))
                } else {
                    preview_install(&directory)
                };
                match preview {
                    Ok(preview) => {
                        let duplicate =
                            self.discover_runtime_plugins_for_app()
                                .map(|(_, statuses)| {
                                    statuses.iter().any(|status| status.id == preview.id)
                                });
                        match duplicate {
                            Ok(true) => self.show_runtime_plugin_install_path(
                                value,
                                Some(&format!(
                                    "{:?} is already installed; update it from its page instead",
                                    preview.id
                                )),
                            ),
                            Ok(false) => self.show_runtime_plugin_install_review(
                                preview,
                                PluginPermissions::default(),
                                None,
                            ),
                            Err(error) => self.show_runtime_plugin_install_path(
                                value,
                                Some(&format!("couldn't read the plugin registry: {error:#}")),
                            ),
                        }
                    }
                    Err(error) => {
                        self.show_runtime_plugin_install_path(value, Some(&format!("{error:#}")))
                    }
                }
            }
            RuntimePluginManagerView::EditSetting { plugin_id, setting } => {
                if !self.runtime_plugin_setting_schema_is_current(&plugin_id, &setting) {
                    return;
                }
                self.dialog.dismiss(request_id);
                self.dialog_flow = None;
                self.apply_runtime_plugin_setting_change(
                    plugin_id,
                    PluginConfigurationChange::Set {
                        key: setting.id().to_string(),
                        value,
                    },
                );
            }
            _ => unreachable!("only the prompts have a field"),
        }
    }

    fn apply_runtime_plugin_install(
        &mut self,
        preview: PluginInstallPreview,
        grants: PluginPermissions,
        enable: bool,
    ) {
        let command = PluginAdminCommand::InstallReviewed {
            directory: preview.source.clone(),
            grants: grants.clone(),
            enable,
            expected_source_fingerprint: preview.source_fingerprint.clone(),
        };
        match self.execute_runtime_plugin_admin(command) {
            Ok(message) => {
                self.reload_plugins();
                self.toasts.success(message);
                self.show_runtime_plugin_detail(preview.id);
            }
            Err(error) => {
                let error = format!("{error:#}");
                self.toasts
                    .error(format!("Plugin installation failed: {error}"));
                self.show_runtime_plugin_install_review(preview, grants, Some(&error));
            }
        }
    }

    fn apply_runtime_plugin_update(
        &mut self,
        preview: Box<PluginUpdatePreview>,
        grants: PluginPermissions,
        enable: bool,
    ) {
        let command = preview.command(grants.clone(), enable);
        match self.execute_runtime_plugin_admin(command) {
            Ok(message) => {
                self.reload_plugins();
                self.toasts.success(message);
                self.show_runtime_plugin_detail(preview.id.clone());
            }
            Err(error) => {
                let error = format!("{error:#}");
                self.toasts.error(format!("Plugin update failed: {error}"));
                self.show_runtime_plugin_update_review(preview, grants, Some(&error));
            }
        }
    }

    fn apply_runtime_plugin_grants(
        &mut self,
        preview: Box<PluginGrantsPreview>,
        grants: PluginPermissions,
        enable: bool,
    ) {
        let command = preview.command(grants.clone(), enable);
        match self.execute_runtime_plugin_admin(command) {
            Ok(message) => {
                self.reload_plugins();
                self.toasts.success(message);
                self.show_runtime_plugin_detail(preview.id.clone());
            }
            Err(error) => {
                let error = format!("{error:#}");
                self.toasts
                    .error(format!("Plugin permission change failed: {error}"));
                self.show_runtime_plugin_permission_review(preview, grants, Some(&error));
            }
        }
    }

    fn apply_runtime_plugin_default_profile_change(
        &mut self,
        plugin_id: String,
        change: PluginConfigurationChange,
    ) {
        let command = PluginAdminCommand::Configure {
            id: plugin_id.clone(),
            change,
        };
        match self.execute_runtime_plugin_admin(command) {
            Ok(message) => {
                self.reload_plugins();
                self.toasts.success(message);
                self.show_runtime_plugin_detail(plugin_id);
            }
            Err(error) => {
                let error = format!("{error:#}");
                self.toasts
                    .error(format!("Default profile update failed: {error}"));
                self.show_runtime_plugin_error(
                    Some(plugin_id),
                    "Couldn't change the default profile",
                    &error,
                );
            }
        }
    }

    fn runtime_plugin_setting_schema_is_current(
        &mut self,
        plugin_id: &str,
        setting: &DeclarativeSetting,
    ) -> bool {
        let current = self
            .current_runtime_plugin_status(plugin_id)
            .and_then(|status| {
                status
                    .setting_definitions
                    .into_iter()
                    .find(|candidate| candidate.id() == setting.id())
            });
        if current.as_ref() == Some(setting) {
            true
        } else {
            self.toasts.warning(format!(
                "Setting {} changed after the editor opened; review it again",
                setting.id()
            ));
            self.show_runtime_plugin_settings(plugin_id.to_string());
            false
        }
    }

    fn apply_runtime_plugin_setting_change(
        &mut self,
        plugin_id: String,
        change: PluginConfigurationChange,
    ) {
        let command = PluginAdminCommand::Configure {
            id: plugin_id.clone(),
            change,
        };
        match self.execute_runtime_plugin_admin(command) {
            Ok(message) => {
                self.reload_plugins();
                self.toasts.success(message);
                self.show_runtime_plugin_settings(plugin_id);
            }
            Err(error) => {
                let error = format!("{error:#}");
                self.toasts
                    .error(format!("Plugin setting update failed: {error}"));
                self.show_runtime_plugin_error(
                    Some(plugin_id),
                    "Couldn't change the setting",
                    &error,
                );
            }
        }
    }

    fn apply_runtime_plugin_operation(
        &mut self,
        command: PluginAdminCommand,
        plugin_id: String,
        removed: bool,
    ) {
        match self.execute_runtime_plugin_admin(command) {
            Ok(message) => {
                self.reload_plugins();
                self.toasts.success(message);
                if removed {
                    self.show_runtime_plugin_list();
                } else {
                    self.show_runtime_plugin_detail(plugin_id);
                }
            }
            Err(error) => {
                let error = format!("{error:#}");
                self.toasts
                    .error(format!("Plugin operation failed: {error}"));
                self.show_runtime_plugin_error(
                    Some(plugin_id),
                    "Couldn't change the plugin",
                    &error,
                );
            }
        }
    }
}

fn setting_kind(setting: &DeclarativeSetting) -> &'static str {
    match setting {
        DeclarativeSetting::Boolean { .. } => "true or false",
        DeclarativeSetting::Integer { .. } => "a whole number",
        DeclarativeSetting::Text { .. } => "text",
        DeclarativeSetting::Choice { .. } => "one of a list",
    }
}

fn display_permissions(permissions: impl IntoIterator<Item = PluginPermission>) -> String {
    let names = permissions
        .into_iter()
        .map(|permission| permission.to_string())
        .collect::<Vec<_>>();
    if names.is_empty() {
        "none".into()
    } else {
        names.join(", ")
    }
}

/// What a review says above its permission rows.
fn push_permission_prompt(
    body: &mut Vec<DialogBlock>,
    requested: &PluginPermissions,
    enable: bool,
) {
    let count = requested.iter().count();
    body.push(DialogBlock::text(if count == 0 {
        "It asks for no permissions.".to_owned()
    } else {
        format!("It asks for {count} permissions. Pick one to grant or withhold it.")
    }));
    if !enable {
        body.push(DialogBlock::text(
            "It can be turned on only with every permission granted.",
        ));
    }
}

/// One row per requested permission, showing what it allows with its
/// window scope and whether the pending grants cover it, then rows to
/// grant every request or clear every grant.
fn permission_rows(
    requested: &PluginPermissions,
    grants: &PluginPermissions,
    note: impl Fn(PluginPermission) -> Option<&'static str>,
) -> Vec<DialogRow> {
    let mut rows: Vec<DialogRow> = requested
        .iter()
        .map(|permission| {
            let state = if grants.covers(permission) {
                "Granted"
            } else {
                "Not granted"
            };
            let detail = match note(permission) {
                Some(note) => format!("{state}, {note}"),
                None => state.to_owned(),
            };
            DialogRow::new(
                format!("permission:{permission}"),
                format!("{} · {permission}", permission.label()),
            )
            .with_detail(detail)
        })
        .collect();
    if grants != requested {
        rows.push(DialogRow::new("grant-all", "Grant all"));
    }
    if !grants.is_empty() {
        rows.push(DialogRow::new("clear-grants", "Withhold all"));
    }
    rows
}

/// Toggle the grant for one requested permission at exactly its requested
/// scope. A narrower existing grant is widened rather than removed.
fn toggle_requested_grant(
    grants: &mut PluginPermissions,
    requested: &PluginPermissions,
    choice: &str,
) {
    let Ok(permission) = choice.parse::<PluginPermission>() else {
        return;
    };
    if requested.scope(permission.capability) != Some(permission.scope) {
        return;
    }
    if grants.covers(permission) {
        grants.remove(permission.capability);
    } else {
        grants.set(permission);
    }
}

fn display_setting_value(value: &PluginSettingValue) -> String {
    match value {
        PluginSettingValue::Boolean(value) => value.to_string(),
        PluginSettingValue::Integer(value) => value.to_string(),
        PluginSettingValue::Text(value) => format!("{value:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::permission::{Capability, PermissionScope};
    use crate::plugin::runtime::{InstalledPlugin, PluginAdminCommand, PluginConfigurationChange};
    use crate::ui::dialog::{CANCEL, DialogAction};

    fn permissions(values: &[&str]) -> PluginPermissions {
        PluginPermissions::from_permissions(values.iter().map(|value| value.parse().unwrap()))
            .unwrap()
    }

    fn fixture_manifest(id: &str, version: &str, with_setting: bool) -> String {
        let setting = if with_setting {
            r#"
[[settings]]
type = "boolean"
id = "verbose"
name = "Verbose"
default = true
"#
        } else {
            ""
        };
        format!(
            r#"api_version = 1
id = "{id}"
name = "Fixture {id}"
version = "{version}"
permissions = ["process-spawn"]

[[profiles]]
id = "open"
name = "Open Fixture"
program = "termeric-test-fixture-must-not-run"
{setting}"#
        )
    }

    fn profile_fixture_manifest(id: &str, version: &str, profile_count: usize) -> String {
        let profiles = (0..profile_count)
            .map(|index| {
                format!(
                    r#"[[profiles]]
id = "profile-{index}"
name = "Profile {index}"
description = "Local inert profile {index}"
program = "termeric-test-fixture-must-not-run-{index}"
"#
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            r#"api_version = 1
id = "{id}"
name = "Fixture {id}"
version = "{version}"
permissions = ["process-spawn"]

{profiles}
[[settings]]
type = "boolean"
id = "verbose"
name = "Verbose"
default = true
"#
        )
    }

    fn event_only_fixture_manifest(id: &str) -> String {
        format!(
            r#"api_version = 1
id = "{id}"
name = "Fixture {id}"
version = "1.0.0"
permissions = ["process-spawn", "event-subscribe"]

[event_handler]
program = "termeric-test-fixture-must-not-run"
events = ["app-ready"]
"#
        )
    }

    fn typed_fixture_manifest(integer_max: i64) -> String {
        format!(
            r#"api_version = 1
id = "typed-tool"
name = "Typed Tool"
version = "1.0.0"
permissions = ["process-spawn"]

[[profiles]]
id = "open"
name = "Open Fixture"
program = "termeric-test-fixture-must-not-run"

[[settings]]
type = "boolean"
id = "verbose"
name = "Verbose"
description = "Emit additional diagnostics"
default = true

[[settings]]
type = "integer"
id = "limit"
name = "Limit"
description = "Maximum result count"
default = 5
min = 1
max = {integer_max}

[[settings]]
type = "text"
id = "label"
name = "Label"
description = "Displayed label"
default = "root"

[[settings]]
type = "choice"
id = "mode"
name = "Mode"
description = "Operating mode"
default = "safe"
choices = ["safe", "fast", "careful"]
"#
        )
    }

    fn install_typed_fixture(
        base: &std::path::Path,
        registry: &std::path::Path,
        root: &std::path::Path,
    ) -> PathBuf {
        let source = base.join("source-typed-tool");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(
            source.join(crate::plugin::runtime::MANIFEST_NAME),
            typed_fixture_manifest(10),
        )
        .unwrap();
        crate::plugin::runtime::execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: permissions(&["process-spawn"]),
                enable: false,
                replace: false,
            },
            registry,
            root,
        )
        .unwrap();
        source
    }

    fn install_generated_fixture(
        base: &std::path::Path,
        registry: &std::path::Path,
        root: &std::path::Path,
        id: &str,
        settings: &str,
    ) {
        let source = base.join(format!("source-{id}"));
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(
            source.join(crate::plugin::runtime::MANIFEST_NAME),
            format!(
                r#"api_version = 1
id = "{id}"
name = "Generated {id}"
version = "1.0.0"
permissions = ["process-spawn"]

[[profiles]]
id = "open"
name = "Open Fixture"
program = "termeric-test-fixture-must-not-run"

{settings}"#
            ),
        )
        .unwrap();
        crate::plugin::runtime::execute_admin_at(
            PluginAdminCommand::Install {
                directory: source,
                grants: permissions(&["process-spawn"]),
                enable: false,
                replace: false,
            },
            registry,
            root,
        )
        .unwrap();
    }

    fn install_fixture(
        base: &std::path::Path,
        registry: &std::path::Path,
        root: &std::path::Path,
        id: &str,
        with_setting: bool,
    ) -> PathBuf {
        let source = base.join(format!("source-{id}"));
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(
            source.join(crate::plugin::runtime::MANIFEST_NAME),
            fixture_manifest(id, "1.0.0", with_setting),
        )
        .unwrap();
        crate::plugin::runtime::execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: permissions(&["process-spawn"]),
                enable: false,
                replace: false,
            },
            registry,
            root,
        )
        .unwrap();
        source
    }

    fn install_profile_fixture(
        base: &std::path::Path,
        registry: &std::path::Path,
        root: &std::path::Path,
        id: &str,
        profile_count: usize,
    ) -> PathBuf {
        let source = base.join(format!("source-{id}"));
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(
            source.join(crate::plugin::runtime::MANIFEST_NAME),
            profile_fixture_manifest(id, "1.0.0", profile_count),
        )
        .unwrap();
        crate::plugin::runtime::execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: permissions(&["process-spawn"]),
                enable: false,
                replace: false,
            },
            registry,
            root,
        )
        .unwrap();
        source
    }

    #[test]
    fn a_custom_agent_taking_an_installed_plugin_id_is_reported_by_name() {
        let temp = tempfile::tempdir().unwrap();
        let registry = temp.path().join("registry.toml");
        let root = temp.path().join("installed");
        std::fs::create_dir_all(&root).unwrap();
        install_fixture(temp.path(), &registry, &root, "local-tool", false);
        crate::plugin::runtime::execute_admin_at(
            PluginAdminCommand::Enable {
                id: "local-tool".into(),
                grants: PluginPermissions::default(),
            },
            &registry,
            &root,
        )
        .unwrap();
        let mut config = Config::default();
        config.agents.insert(
            "local-tool".into(),
            crate::agents::AgentConfig {
                name: Some("Local Agent".into()),
                program: Some("/bin/true".into()),
                ..Default::default()
            },
        );
        let mut app = App::new_headless(config).unwrap();
        app.set_runtime_plugin_test_paths(registry, root);
        app.sync_plugins_from_config();
        assert!(app.toasts.iter().any(|toast| toast.message
            == "Third-party plugin local-tool was not loaded: the agent Local Agent uses the same ID"));
    }

    #[test]
    fn manager_reviews_permissions_and_installs_local_plugin() {
        let temp = tempfile::tempdir().unwrap();
        let registry = temp.path().join("registry.toml");
        let root = temp.path().join("installed");
        let source = temp.path().join("source-local-tool");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(
            source.join(crate::plugin::runtime::MANIFEST_NAME),
            fixture_manifest("local-tool", "1.0.0", false),
        )
        .unwrap();

        let mut app = App::new_headless(Config::default()).unwrap();
        app.set_runtime_plugin_test_paths(registry.clone(), root.clone());
        app.open_runtime_plugin_manager();
        assert!(app.dialog.answers(app.dialog.request_id(), "install"));
        app.answer_dialog("install");
        let cancelled_request = app.dialog.request_id();
        app.apply_dialog_action(DialogAction::Answer {
            request_id: cancelled_request,
            id: CANCEL.into(),
        });
        assert!(app.dialog.answers(app.dialog.request_id(), "install"));
        app.answer_dialog("install");
        app.submit_dialog_field("relative/plugin");
        assert_eq!(app.dialog.title(), "Install a plugin");
        assert!(
            app.dialog
                .field(app.dialog.request_id())
                .unwrap()
                .error
                .is_some()
        );

        app.submit_dialog_field(&source.display().to_string());
        assert_eq!(app.dialog.title(), "Install Fixture local-tool?");
        assert!(app.dialog_text().contains(&source.display().to_string()));
        assert!(app.dialog_text().contains("fnv1a64:"));
        assert!(
            app.dialog
                .answers(app.dialog.request_id(), "permission:process-spawn")
        );
        assert!(
            !app.dialog
                .answers(app.dialog.request_id(), "install-enabled")
        );
        app.answer_dialog("permission:process-spawn");
        assert!(
            app.dialog
                .answers(app.dialog.request_id(), "install-enabled")
        );
        app.answer_dialog("install-enabled");

        let (_, statuses) =
            crate::plugin::runtime::discover_runtime_plugins_at(&registry, &root).unwrap();
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].id, "local-tool");
        assert!(statuses[0].enabled);
        assert_eq!(statuses[0].granted, permissions(&["process-spawn"]));
        assert!(!app.dialog.answers(app.dialog.request_id(), "install"));
        assert!(app.dialog.answers(app.dialog.request_id(), "remove"));

        app.answer_dialog(CANCEL);
        app.answer_dialog("install");
        app.submit_dialog_field(&source.display().to_string());
        assert_eq!(app.dialog.title(), "Install a plugin");
        assert!(
            app.dialog
                .field(app.dialog.request_id())
                .unwrap()
                .error
                .is_some()
        );
        let (_, statuses) =
            crate::plugin::runtime::discover_runtime_plugins_at(&registry, &root).unwrap();
        assert_eq!(statuses.len(), 1);
    }

    #[test]
    fn manager_reviews_an_installed_plugins_permissions_before_enabling_it() {
        let temp = tempfile::tempdir().unwrap();
        let registry = temp.path().join("registry.toml");
        let root = temp.path().join("installed");
        let source = temp.path().join("source-local-tool");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(
            source.join(crate::plugin::runtime::MANIFEST_NAME),
            fixture_manifest("local-tool", "1.0.0", false),
        )
        .unwrap();
        crate::plugin::runtime::execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: PluginPermissions::default(),
                enable: false,
                replace: false,
            },
            &registry,
            &root,
        )
        .unwrap();
        // The review reads the installed manifest, not the source.
        std::fs::remove_dir_all(&source).unwrap();
        let status = || {
            let (_, statuses) =
                crate::plugin::runtime::discover_runtime_plugins_at(&registry, &root).unwrap();
            (statuses[0].granted.clone(), statuses[0].enabled)
        };

        let mut app = App::new_headless(Config::default()).unwrap();
        app.set_runtime_plugin_test_paths(registry.clone(), root.clone());
        app.open_runtime_plugin_manager();
        app.answer_dialog("plugin:local-tool");
        assert_eq!(app.dialog.title(), "Fixture local-tool");

        // Enabling a plugin that lacks a grant opens its permission review.
        app.answer_dialog("toggle");
        assert_eq!(app.dialog.title(), "Fixture local-tool permissions");
        let request_id = app.dialog.request_id();
        assert!(!app.dialog.answers(request_id, "save-enabled"));
        assert!(app.dialog.answers(request_id, "save-disabled"));
        app.answer_dialog("permission:process-spawn");
        app.answer_dialog("save-enabled");
        assert_eq!(status(), (permissions(&["process-spawn"]), true));
        assert_eq!(app.dialog.title(), "Fixture local-tool");

        // The same review withdraws grants, which disables the plugin.
        app.answer_dialog("permissions");
        app.answer_dialog("clear-grants");
        assert!(!app.dialog.answers(app.dialog.request_id(), "save-enabled"));
        app.answer_dialog("save-disabled");
        assert_eq!(status(), (PluginPermissions::default(), false));

        // A review the registry has moved past is refused, not applied.
        app.answer_dialog("permissions");
        crate::plugin::runtime::execute_admin_at(
            PluginAdminCommand::Grant {
                id: "local-tool".into(),
                grants: permissions(&["process-spawn"]),
            },
            &registry,
            &root,
        )
        .unwrap();
        app.answer_dialog("save-disabled");
        assert_eq!(app.dialog.title(), "Fixture local-tool permissions");
        assert!(app.dialog_text().contains("changed after review"));
        assert_eq!(status(), (permissions(&["process-spawn"]), false));
    }

    #[test]
    fn manager_rejects_source_changed_after_install_review() {
        let temp = tempfile::tempdir().unwrap();
        let registry = temp.path().join("registry.toml");
        let root = temp.path().join("installed");
        let source = temp.path().join("source-stale-tool");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(
            source.join(crate::plugin::runtime::MANIFEST_NAME),
            fixture_manifest("stale-tool", "1.0.0", false),
        )
        .unwrap();

        let mut app = App::new_headless(Config::default()).unwrap();
        app.set_runtime_plugin_test_paths(registry.clone(), root.clone());
        app.open_runtime_plugin_manager();
        app.answer_dialog("install");
        app.submit_dialog_field(&source.display().to_string());
        let stale_request = app.dialog.request_id();
        std::fs::write(
            source.join(crate::plugin::runtime::MANIFEST_NAME),
            fixture_manifest("stale-tool", "2.0.0", false),
        )
        .unwrap();
        app.apply_dialog_action(DialogAction::Answer {
            request_id: stale_request + 1,
            id: "install-disabled".into(),
        });
        assert_eq!(app.dialog.request_id(), stale_request);
        app.apply_dialog_action(DialogAction::Answer {
            request_id: stale_request,
            id: "permission:not-a-permission".into(),
        });
        assert_eq!(app.dialog.request_id(), stale_request);
        app.answer_dialog("install-disabled");
        assert_eq!(app.dialog.title(), "Install Fixture stale-tool?");
        assert!(app.dialog_text().contains("source changed after review"));
        let (_, statuses) =
            crate::plugin::runtime::discover_runtime_plugins_at(&registry, &root).unwrap();
        assert!(statuses.is_empty());
        assert!(!root.join("stale-tool").exists());
    }

    fn every_permission() -> PluginPermissions {
        PluginPermissions::from_permissions(Capability::ALL.into_iter().map(|capability| {
            PluginPermission {
                capability,
                scope: if capability.is_scoped() {
                    PermissionScope::AllWindows
                } else {
                    PermissionScope::Window
                },
            }
        }))
        .unwrap()
    }

    /// The label and detail of the open review's row answering `id`.
    fn row(app: &App, id: &str) -> (String, Option<String>) {
        let row = app
            .dialog
            .rows()
            .iter()
            .find(|row| row.id == id)
            .unwrap_or_else(|| panic!("no row {id}"));
        (row.label.clone(), row.detail.clone())
    }

    fn assert_every_permission_is_listed_with_its_scope(app: &mut App) {
        let request_id = app.dialog.request_id();
        // One scrolling list holds every permission.
        for permission in every_permission().iter() {
            assert!(
                app.dialog
                    .answers(request_id, &format!("permission:{permission}")),
                "{permission}"
            );
        }
        let granted = |app: &App, id: &str| {
            row(app, id)
                .1
                .is_some_and(|detail| detail.starts_with("Granted"))
        };
        let (label, _) = row(app, "permission:pane-create:all-windows");
        assert_eq!(
            label,
            "Create panes (all windows) · pane-create:all-windows"
        );
        assert_eq!(
            row(app, "permission:process-spawn").0,
            "Start processes · process-spawn"
        );
        assert!(!granted(app, "permission:pane-create:all-windows"));

        // A row must name the exact requested scope to toggle a grant.
        app.answer_dialog("permission:pane-create:all-windows");
        assert!(granted(app, "permission:pane-create:all-windows"));
        let request_id = app.dialog.request_id();
        app.apply_dialog_action(DialogAction::Answer {
            request_id,
            id: "permission:pane-create".into(),
        });
        assert!(granted(app, "permission:pane-create:all-windows"));
    }

    #[test]
    fn install_review_lists_every_scoped_permission() {
        let preview = PluginInstallPreview {
            id: "many-permissions".into(),
            name: "Many Permissions".into(),
            version: "1.0.0".into(),
            description: String::new(),
            source: PathBuf::from("/tmp/many-permissions"),
            source_fingerprint: "fnv1a64:0000000000000000".into(),
            permissions: every_permission(),
            profile_count: 1,
            command_count: 0,
        };
        let mut app = App::new_headless(Config::default()).unwrap();
        app.show_runtime_plugin_install_review(preview, PluginPermissions::default(), None);
        assert_every_permission_is_listed_with_its_scope(&mut app);
    }

    #[test]
    fn update_review_lists_every_scoped_permission() {
        let expected_installed = InstalledPlugin {
            id: "many-permissions".into(),
            enabled: false,
            source: PathBuf::from("/tmp/many-permissions"),
            source_fingerprint: "fnv1a64:0000000000000000".into(),
            grants: PluginPermissions::default(),
            default_profile: None,
            settings: Default::default(),
        };
        let preview = PluginUpdatePreview {
            id: "many-permissions".into(),
            name: "Many Permissions".into(),
            source: expected_installed.source.clone(),
            current_version: "1.0.0".into(),
            candidate_version: "2.0.0".into(),
            current_source_fingerprint: expected_installed.source_fingerprint.clone(),
            candidate_source_fingerprint: "fnv1a64:1111111111111111".into(),
            current_permissions: PluginPermissions::default(),
            candidate_permissions: every_permission(),
            current_grants: PluginPermissions::default(),
            enabled: false,
            current_profile_count: 0,
            candidate_profile_count: 1,
            current_command_count: 0,
            candidate_command_count: 0,
            configured_default_profile: None,
            default_profile_will_be_cleared: false,
            expected_installed,
        };
        let mut app = App::new_headless(Config::default()).unwrap();
        app.show_runtime_plugin_update_review(
            Box::new(preview),
            PluginPermissions::default(),
            None,
        );
        assert_every_permission_is_listed_with_its_scope(&mut app);
    }

    #[test]
    fn manager_selects_and_clears_default_profile_without_other_mutation() {
        let temp = tempfile::tempdir().unwrap();
        let registry = temp.path().join("registry.toml");
        let root = temp.path().join("installed");
        std::fs::create_dir_all(&root).unwrap();
        install_profile_fixture(temp.path(), &registry, &root, "profile-tool", 2);
        crate::plugin::runtime::execute_admin_at(
            PluginAdminCommand::Configure {
                id: "profile-tool".into(),
                change: PluginConfigurationChange::Set {
                    key: "verbose".into(),
                    value: "false".into(),
                },
            },
            &registry,
            &root,
        )
        .unwrap();
        let installed_manifest = root
            .join("profile-tool")
            .join(crate::plugin::runtime::MANIFEST_NAME);
        let installed_bytes = std::fs::read(&installed_manifest).unwrap();
        let (_, before) =
            crate::plugin::runtime::discover_runtime_plugins_at(&registry, &root).unwrap();
        let before = before.into_iter().next().unwrap();

        let mut app = App::new_headless(Config::default()).unwrap();
        app.set_runtime_plugin_test_paths(registry.clone(), root.clone());
        app.open_runtime_plugin_manager();
        app.answer_dialog("plugin:profile-tool");
        assert!(
            app.dialog_text()
                .contains("Default profile: Profile 0 (profile-0), manifest order")
        );
        app.answer_dialog("choose-default-profile");
        let picker_request = app.dialog.request_id();
        app.apply_dialog_action(DialogAction::Answer {
            request_id: picker_request + 1,
            id: "profile:profile-1".into(),
        });
        assert_eq!(app.dialog.request_id(), picker_request);
        app.apply_dialog_action(DialogAction::Answer {
            request_id: picker_request,
            id: "profile:not-declared".into(),
        });
        assert_eq!(app.dialog.request_id(), picker_request);
        app.answer_dialog("profile:profile-1");

        let (_, selected) =
            crate::plugin::runtime::discover_runtime_plugins_at(&registry, &root).unwrap();
        let selected = selected.into_iter().next().unwrap();
        assert_eq!(
            selected.configured_default_profile.as_deref(),
            Some("profile-1")
        );
        assert!(
            app.dialog_text()
                .contains("Default profile: Profile 1 (profile-1), chosen")
        );
        assert_eq!(selected.enabled, before.enabled);
        assert_eq!(selected.source, before.source);
        assert_eq!(selected.requested, before.requested);
        assert_eq!(selected.granted, before.granted);
        assert_eq!(selected.settings, before.settings);
        assert_eq!(std::fs::read(&installed_manifest).unwrap(), installed_bytes);

        app.answer_dialog("choose-default-profile");
        assert!(
            app.dialog
                .answers(app.dialog.request_id(), "manifest-order")
        );
        app.answer_dialog("manifest-order");
        let (_, reset) =
            crate::plugin::runtime::discover_runtime_plugins_at(&registry, &root).unwrap();
        assert!(reset[0].configured_default_profile.is_none());
        assert!(
            app.dialog_text()
                .contains("Default profile: Profile 0 (profile-0), manifest order")
        );
        assert_eq!(std::fs::read(installed_manifest).unwrap(), installed_bytes);
    }

    #[test]
    fn default_profile_picker_lists_every_profile_and_is_hidden_for_event_only_plugins() {
        let temp = tempfile::tempdir().unwrap();
        let registry = temp.path().join("registry.toml");
        let root = temp.path().join("installed");
        std::fs::create_dir_all(&root).unwrap();
        install_profile_fixture(temp.path(), &registry, &root, "many-profiles", 32);
        let event_source = temp.path().join("source-event-only");
        std::fs::create_dir_all(&event_source).unwrap();
        std::fs::write(
            event_source.join(crate::plugin::runtime::MANIFEST_NAME),
            event_only_fixture_manifest("event-only"),
        )
        .unwrap();
        crate::plugin::runtime::execute_admin_at(
            PluginAdminCommand::Install {
                directory: event_source,
                grants: PluginPermissions::default(),
                enable: false,
                replace: false,
            },
            &registry,
            &root,
        )
        .unwrap();

        let mut app = App::new_headless(Config::default()).unwrap();
        app.set_runtime_plugin_test_paths(registry, root);
        app.open_runtime_plugin_manager();
        app.answer_dialog("plugin:many-profiles");
        app.answer_dialog("choose-default-profile");
        // Every profile in one list, and Back to the plugin.
        assert_eq!(app.dialog.rows().len(), 32);
        assert!(app.dialog.answers(app.dialog.request_id(), CANCEL));

        app.open_runtime_plugin_manager();
        app.answer_dialog("plugin:event-only");
        assert!(
            !app.dialog
                .answers(app.dialog.request_id(), "choose-default-profile")
        );
        assert!(app.dialog_text().contains("Default profile: none"));
    }

    #[test]
    fn removed_profile_cannot_be_selected_from_a_stale_picker() {
        let temp = tempfile::tempdir().unwrap();
        let registry = temp.path().join("registry.toml");
        let root = temp.path().join("installed");
        std::fs::create_dir_all(&root).unwrap();
        let source = install_profile_fixture(temp.path(), &registry, &root, "changing-tool", 2);

        let mut app = App::new_headless(Config::default()).unwrap();
        app.set_runtime_plugin_test_paths(registry.clone(), root.clone());
        app.open_runtime_plugin_manager();
        app.answer_dialog("plugin:changing-tool");
        app.answer_dialog("choose-default-profile");
        let stale_request = app.dialog.request_id();
        assert!(app.dialog.answers(stale_request, "profile:profile-1"));
        std::fs::write(
            source.join(crate::plugin::runtime::MANIFEST_NAME),
            profile_fixture_manifest("changing-tool", "2.0.0", 1),
        )
        .unwrap();
        crate::plugin::runtime::execute_admin_at(
            PluginAdminCommand::Update {
                id: "changing-tool".into(),
            },
            &registry,
            &root,
        )
        .unwrap();
        app.apply_dialog_action(DialogAction::Answer {
            request_id: stale_request,
            id: "profile:profile-1".into(),
        });

        let (_, statuses) =
            crate::plugin::runtime::discover_runtime_plugins_at(&registry, &root).unwrap();
        assert!(statuses[0].configured_default_profile.is_none());
        assert_eq!(
            app.dialog.title(),
            "Default profile for Fixture changing-tool"
        );
        assert!(
            !app.dialog
                .answers(app.dialog.request_id(), "profile:profile-1")
        );
        assert!(
            app.toasts
                .iter()
                .any(|toast| toast.message.contains("no longer exists"))
        );
    }

    #[test]
    fn manager_uses_request_scoped_validated_actions_and_preserves_original_source() {
        let temp = tempfile::tempdir().unwrap();
        let registry = temp.path().join("registry.toml");
        let root = temp.path().join("installed");
        std::fs::create_dir_all(&root).unwrap();
        let source = install_fixture(temp.path(), &registry, &root, "fixture-tool", true);
        crate::plugin::runtime::execute_admin_at(
            PluginAdminCommand::Configure {
                id: "fixture-tool".into(),
                change: PluginConfigurationChange::Set {
                    key: "verbose".into(),
                    value: "false".into(),
                },
            },
            &registry,
            &root,
        )
        .unwrap();

        let mut app = App::new_headless(Config::default()).unwrap();
        app.set_runtime_plugin_test_paths(registry.clone(), root.clone());
        app.open_runtime_plugin_manager();
        let dismissed_request = app.dialog.request_id();
        app.apply_dialog_action(DialogAction::Close {
            request_id: dismissed_request,
        });
        assert!(app.pending_runtime_plugin_management().is_none());

        app.open_runtime_plugin_manager();
        let list_request = app.dialog.request_id();
        app.apply_dialog_action(DialogAction::Answer {
            request_id: list_request + 1,
            id: "plugin:fixture-tool".into(),
        });
        assert_eq!(app.dialog.request_id(), list_request);
        assert!(matches!(
            app.pending_runtime_plugin_management()
                .map(|pending| &pending.view),
            Some(RuntimePluginManagerView::List)
        ));

        app.answer_dialog("plugin:fixture-tool");
        assert!(app.dialog_text().contains(&source.display().to_string()));
        assert!(app.dialog_text().contains("verbose = false"));
        assert!(
            app.dialog
                .answers(app.dialog.request_id(), "reset-settings")
        );

        std::fs::write(
            source.join(crate::plugin::runtime::MANIFEST_NAME),
            fixture_manifest("fixture-tool", "2.0.0", true),
        )
        .unwrap();
        app.answer_dialog("update");
        assert_eq!(app.dialog.title(), "Update Fixture fixture-tool?");
        app.answer_dialog("update-disabled");
        let (_, statuses) =
            crate::plugin::runtime::discover_runtime_plugins_at(&registry, &root).unwrap();
        assert_eq!(statuses[0].version, "2.0.0");
        assert_eq!(statuses[0].configured_setting_count, 1);

        app.answer_dialog("reset-settings");
        assert_eq!(app.dialog.title(), "Reset the settings of fixture-tool?");
        app.answer_dialog("confirm");
        let (_, statuses) =
            crate::plugin::runtime::discover_runtime_plugins_at(&registry, &root).unwrap();
        assert_eq!(statuses[0].configured_setting_count, 0);
        assert!(
            !app.dialog
                .answers(app.dialog.request_id(), "reset-settings")
        );

        app.answer_dialog("toggle");
        let (_, statuses) =
            crate::plugin::runtime::discover_runtime_plugins_at(&registry, &root).unwrap();
        assert!(statuses[0].enabled);

        app.answer_dialog("remove");
        assert_eq!(app.dialog.title(), "Remove fixture-tool?");
        app.answer_dialog(CANCEL);
        assert!(!app.dialog.answers(app.dialog.request_id(), "install"));
        assert!(app.dialog.answers(app.dialog.request_id(), "remove"));
        app.answer_dialog("remove");
        app.answer_dialog("confirm");
        let (_, statuses) =
            crate::plugin::runtime::discover_runtime_plugins_at(&registry, &root).unwrap();
        assert!(statuses.is_empty());
        assert!(source.is_dir(), "the original source must never be removed");
    }

    #[test]
    fn manager_reviews_permission_deltas_before_enabling_an_update() {
        let temp = tempfile::tempdir().unwrap();
        let registry = temp.path().join("registry.toml");
        let root = temp.path().join("installed");
        std::fs::create_dir_all(&root).unwrap();
        let source = install_fixture(temp.path(), &registry, &root, "permission-tool", false);
        let candidate = fixture_manifest("permission-tool", "2.0.0", false).replace(
            "permissions = [\"process-spawn\"]",
            "permissions = [\"process-spawn\", \"workspace-query\"]",
        );
        std::fs::write(
            source.join(crate::plugin::runtime::MANIFEST_NAME),
            candidate,
        )
        .unwrap();

        let mut app = App::new_headless(Config::default()).unwrap();
        app.set_runtime_plugin_test_paths(registry.clone(), root.clone());
        app.open_runtime_plugin_manager();
        app.answer_dialog("plugin:permission-tool");
        app.answer_dialog("update");
        assert_eq!(app.dialog.title(), "Update Fixture permission-tool?");
        assert!(app.dialog_text().contains("1.0.0 → 2.0.0"));
        assert!(
            app.dialog_text()
                .contains("Newly requested: workspace-query")
        );
        assert!(
            app.dialog
                .answers(app.dialog.request_id(), "permission:workspace-query")
        );
        assert!(
            !app.dialog
                .answers(app.dialog.request_id(), "update-enabled")
        );

        app.answer_dialog("permission:workspace-query");
        assert!(
            app.dialog
                .answers(app.dialog.request_id(), "update-enabled")
        );
        app.answer_dialog("update-enabled");
        let (_, statuses) =
            crate::plugin::runtime::discover_runtime_plugins_at(&registry, &root).unwrap();
        assert_eq!(statuses[0].version, "2.0.0");
        assert!(statuses[0].enabled);
        assert_eq!(
            statuses[0].granted,
            permissions(&["process-spawn", "workspace-query"])
        );

        // Widening a granted capability's scope is reviewed like a new one.
        let widened = fixture_manifest("permission-tool", "2.1.0", false).replace(
            "permissions = [\"process-spawn\"]",
            "permissions = [\"process-spawn\", \"workspace-query:all-windows\"]",
        );
        std::fs::write(source.join(crate::plugin::runtime::MANIFEST_NAME), widened).unwrap();
        app.answer_dialog("update");
        assert!(
            app.dialog_text()
                .contains("Newly requested: workspace-query:all-windows")
        );
        assert_eq!(
            row(&app, "permission:workspace-query:all-windows"),
            (
                "Inspect windows, tabs, and panes (all windows) · workspace-query:all-windows"
                    .into(),
                Some("Not granted, new".into())
            )
        );
        assert!(
            !app.dialog
                .answers(app.dialog.request_id(), "update-enabled")
        );
        app.answer_dialog("permission:workspace-query:all-windows");
        app.answer_dialog("update-enabled");
        let (_, statuses) =
            crate::plugin::runtime::discover_runtime_plugins_at(&registry, &root).unwrap();
        assert_eq!(statuses[0].version, "2.1.0");
        assert_eq!(
            statuses[0].granted,
            permissions(&["process-spawn", "workspace-query:all-windows"])
        );

        std::fs::write(
            source.join(crate::plugin::runtime::MANIFEST_NAME),
            fixture_manifest("permission-tool", "3.0.0", false),
        )
        .unwrap();
        app.answer_dialog("update");
        assert!(
            app.dialog_text()
                .contains("No longer requested: workspace-query:all-windows")
        );
        assert!(
            app.dialog
                .answers(app.dialog.request_id(), "update-enabled"),
            "removed permissions must not leave stale grants that block an enabled update"
        );
        app.answer_dialog("update-enabled");
        let (_, statuses) =
            crate::plugin::runtime::discover_runtime_plugins_at(&registry, &root).unwrap();
        assert_eq!(statuses[0].version, "3.0.0");
        assert_eq!(statuses[0].granted, permissions(&["process-spawn"]));
        assert!(statuses[0].enabled);
    }

    #[test]
    fn manager_keeps_review_open_when_update_source_drifts() {
        let temp = tempfile::tempdir().unwrap();
        let registry = temp.path().join("registry.toml");
        let root = temp.path().join("installed");
        std::fs::create_dir_all(&root).unwrap();
        let source = install_fixture(temp.path(), &registry, &root, "drift-tool", false);
        std::fs::write(
            source.join(crate::plugin::runtime::MANIFEST_NAME),
            fixture_manifest("drift-tool", "2.0.0", false),
        )
        .unwrap();
        let installed_manifest = root
            .join("drift-tool")
            .join(crate::plugin::runtime::MANIFEST_NAME);
        let installed_before = std::fs::read(&installed_manifest).unwrap();
        let registry_before = std::fs::read(&registry).unwrap();

        let mut app = App::new_headless(Config::default()).unwrap();
        app.set_runtime_plugin_test_paths(registry.clone(), root.clone());
        app.open_runtime_plugin_manager();
        app.answer_dialog("plugin:drift-tool");
        app.answer_dialog("update");
        let stale_request = app.dialog.request_id();
        app.apply_dialog_action(DialogAction::Answer {
            request_id: stale_request + 1,
            id: "update-disabled".into(),
        });
        assert_eq!(app.dialog.request_id(), stale_request);
        assert_eq!(std::fs::read(&registry).unwrap(), registry_before);
        app.apply_dialog_action(DialogAction::Answer {
            request_id: stale_request,
            id: "permission:workspace-query".into(),
        });
        assert_eq!(std::fs::read(&registry).unwrap(), registry_before);
        std::fs::write(
            source.join(crate::plugin::runtime::MANIFEST_NAME),
            fixture_manifest("drift-tool", "2.0.1", false),
        )
        .unwrap();
        app.answer_dialog("update-disabled");

        assert_eq!(app.dialog.title(), "Update Fixture drift-tool?");
        assert!(app.dialog_text().contains("source changed after review"));
        assert_eq!(
            std::fs::read(&installed_manifest).unwrap(),
            installed_before
        );
        assert_eq!(std::fs::read(&registry).unwrap(), registry_before);
    }

    #[test]
    fn manager_lists_every_plugin_in_one_scrolling_list() {
        let temp = tempfile::tempdir().unwrap();
        let registry = temp.path().join("registry.toml");
        let root = temp.path().join("installed");
        std::fs::create_dir_all(&root).unwrap();
        for index in 0..25 {
            install_fixture(
                temp.path(),
                &registry,
                &root,
                &format!("fixture-{index:02}"),
                false,
            );
        }

        let mut app = App::new_headless(Config::default()).unwrap();
        app.set_runtime_plugin_test_paths(registry, root);
        app.open_runtime_plugin_manager();
        // One scrolling list instead of pages, and no letter keys.
        assert_eq!(app.dialog.rows().len(), 25);
        assert_eq!(app.dialog.rows()[0].id, "plugin:fixture-00");
        app.answer_dialog("plugin:fixture-24");
        assert_eq!(app.dialog.title(), "Fixture fixture-24");
    }

    #[test]
    fn manager_edits_every_typed_setting_and_unsets_one_override() {
        let temp = tempfile::tempdir().unwrap();
        let registry = temp.path().join("registry.toml");
        let root = temp.path().join("installed");
        std::fs::create_dir_all(&root).unwrap();
        install_typed_fixture(temp.path(), &registry, &root);

        let mut app = App::new_headless(Config::default()).unwrap();
        app.set_runtime_plugin_test_paths(registry.clone(), root.clone());
        app.open_runtime_plugin_manager();
        app.answer_dialog("plugin:typed-tool");
        app.answer_dialog("configure-settings");
        assert_eq!(app.dialog.title(), "Typed Tool settings");

        app.answer_dialog("setting:verbose");
        app.answer_dialog("value:false");

        app.answer_dialog("setting:limit");
        let request = app.dialog.request_id();
        assert!(app.dialog.replace_field_text(request, "7"));
        app.apply_dialog_action(DialogAction::Answer {
            request_id: request,
            id: "save".into(),
        });

        app.answer_dialog("setting:label");
        let request = app.dialog.request_id();
        assert!(app.dialog.replace_field_text(request, "warm 🫚"));
        app.apply_dialog_action(DialogAction::Answer {
            request_id: request,
            id: "save".into(),
        });

        app.answer_dialog("setting:mode");
        app.answer_dialog("value:1");

        let (_, statuses) =
            crate::plugin::runtime::discover_runtime_plugins_at(&registry, &root).unwrap();
        let status = &statuses[0];
        assert_eq!(status.configured_setting_count, 4);
        assert_eq!(
            status.settings["verbose"],
            PluginSettingValue::Boolean(false)
        );
        assert_eq!(status.settings["limit"], PluginSettingValue::Integer(7));
        assert_eq!(
            status.settings["label"],
            PluginSettingValue::Text("warm 🫚".into())
        );
        assert_eq!(
            status.settings["mode"],
            PluginSettingValue::Text("fast".into())
        );

        app.answer_dialog("setting:mode");
        app.answer_dialog("unset");
        let (_, statuses) =
            crate::plugin::runtime::discover_runtime_plugins_at(&registry, &root).unwrap();
        let status = &statuses[0];
        assert_eq!(status.configured_setting_count, 3);
        assert!(!status.configured_settings.contains_key("mode"));
        assert_eq!(
            status.settings["mode"],
            PluginSettingValue::Text("safe".into())
        );
    }

    #[test]
    fn invalid_and_stale_editor_submissions_leave_registry_unchanged() {
        let temp = tempfile::tempdir().unwrap();
        let registry = temp.path().join("registry.toml");
        let root = temp.path().join("installed");
        std::fs::create_dir_all(&root).unwrap();
        let source = install_typed_fixture(temp.path(), &registry, &root);

        let mut app = App::new_headless(Config::default()).unwrap();
        app.set_runtime_plugin_test_paths(registry.clone(), root.clone());
        app.open_runtime_plugin_manager();
        app.answer_dialog("plugin:typed-tool");
        app.answer_dialog("configure-settings");
        app.answer_dialog("setting:limit");
        let cancelled_request = app.dialog.request_id();
        assert!(app.dialog.replace_field_text(cancelled_request, "9"));
        app.apply_dialog_action(DialogAction::Answer {
            request_id: cancelled_request,
            id: CANCEL.into(),
        });
        let (_, statuses) =
            crate::plugin::runtime::discover_runtime_plugins_at(&registry, &root).unwrap();
        assert_eq!(statuses[0].configured_setting_count, 0);
        assert_eq!(app.dialog.title(), "Typed Tool settings");

        app.answer_dialog("setting:limit");
        let request = app.dialog.request_id();
        assert!(app.dialog.replace_field_text(request, "999"));
        app.apply_dialog_action(DialogAction::Answer {
            request_id: request,
            id: "save".into(),
        });
        assert_eq!(app.dialog.kind(), crate::ui::dialog::DialogKind::Error);
        let (_, statuses) =
            crate::plugin::runtime::discover_runtime_plugins_at(&registry, &root).unwrap();
        assert_eq!(statuses[0].configured_setting_count, 0);

        app.open_runtime_plugin_manager();
        app.answer_dialog("plugin:typed-tool");
        app.answer_dialog("configure-settings");
        app.answer_dialog("setting:limit");
        let stale_request = app.dialog.request_id();
        assert!(app.dialog.replace_field_text(stale_request, "8"));
        std::fs::write(
            source.join(crate::plugin::runtime::MANIFEST_NAME),
            typed_fixture_manifest(20),
        )
        .unwrap();
        crate::plugin::runtime::execute_admin_at(
            PluginAdminCommand::Update {
                id: "typed-tool".into(),
            },
            &registry,
            &root,
        )
        .unwrap();
        app.apply_dialog_action(DialogAction::Answer {
            request_id: stale_request,
            id: "save".into(),
        });
        let (_, statuses) =
            crate::plugin::runtime::discover_runtime_plugins_at(&registry, &root).unwrap();
        assert_eq!(statuses[0].configured_setting_count, 0);
        assert_eq!(app.dialog.title(), "Typed Tool settings");
        assert!(
            app.toasts
                .iter()
                .any(|toast| toast.message.contains("changed after the editor opened"))
        );
    }

    #[test]
    fn every_setting_and_choice_value_is_listed() {
        let temp = tempfile::tempdir().unwrap();
        let registry = temp.path().join("registry.toml");
        let root = temp.path().join("installed");
        std::fs::create_dir_all(&root).unwrap();
        let settings = (0..32)
            .map(|index| {
                format!(
                    "[[settings]]\ntype = \"boolean\"\nid = \"flag-{index}\"\nname = \"Flag {index}\"\ndefault = false\n"
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        install_generated_fixture(temp.path(), &registry, &root, "many-settings", &settings);
        let choice_values = (0..32)
            .map(|index| format!("\"value-{index}\""))
            .collect::<Vec<_>>()
            .join(", ");
        install_generated_fixture(
            temp.path(),
            &registry,
            &root,
            "many-choices",
            &format!(
                "[[settings]]\ntype = \"choice\"\nid = \"mode\"\nname = \"Mode\"\ndefault = \"value-0\"\nchoices = [{choice_values}]\n"
            ),
        );

        let mut app = App::new_headless(Config::default()).unwrap();
        app.set_runtime_plugin_test_paths(registry, root);
        app.open_runtime_plugin_manager();
        app.answer_dialog("plugin:many-settings");
        app.answer_dialog("configure-settings");
        assert_eq!(app.dialog.rows().len(), 32);

        app.open_runtime_plugin_manager();
        app.answer_dialog("plugin:many-choices");
        app.answer_dialog("configure-settings");
        app.answer_dialog("setting:mode");
        assert_eq!(app.dialog.rows().len(), 32);
        app.answer_dialog("value:31");
        let request = app.dialog.request_id();
        assert_eq!(app.dialog.title(), "Generated many-choices settings");
        assert!(app.dialog.is_awaiting(request));
    }
}
