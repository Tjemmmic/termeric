//! Installed third-party plugins, managed on the Plugins page of Settings:
//! the registry operations its rows ask for, and the questions some of them
//! ask first (the folder to install from, the install, update, and
//! permission reviews, a setting's new value, Reset and Remove). Every
//! question is its own dialog request, so an answer meant for an earlier
//! one does nothing, and every review is refused by the registry when what
//! it showed has changed since.

use super::*;
use crate::plugin::permission::{PluginPermission, PluginPermissions};
use crate::plugin::runtime::{
    DeclarativeSetting, PluginAdminCommand, PluginConfigurationChange, PluginGrantsPreview,
    PluginInstallPreview, PluginUpdatePreview, RuntimePluginStatus, preview_install,
};
use crate::ui::dialog::{
    ButtonRole, CANCEL, DialogBlock, DialogButton, DialogSpec, FieldSpec, MAX_FIELD_BYTES,
};
use crate::ui::settings::{PluginAction, SettingsSubPage, setting_value_text};

/// The answer that submits a prompt's field.
const SAVE: &str = "save";
const INSTALL_DISABLED: &str = "install-disabled";
const INSTALL_ENABLED: &str = "install-enabled";
const UPDATE_DISABLED: &str = "update-disabled";
const UPDATE_ENABLED: &str = "update-enabled";
/// Grant the one permission a review asks about.
const GRANT: &str = "grant";
/// Withhold a permission from a plugin that is on, which turns it off.
const WITHHOLD: &str = "withhold";
/// Grant every permission a plugin asks for and turn it on.
const TURN_ON: &str = "turn-on";
/// A setting prompt's "Use default".
const UNSET: &str = "unset";
/// Reset or Remove, confirmed.
const CONFIRM: &str = "confirm";

/// A plugin question awaiting its answer.
#[derive(Debug, Clone)]
pub(super) struct PendingPluginQuestion {
    request_id: u64,
    question: PluginQuestion,
}

#[derive(Debug, Clone)]
enum PluginQuestion {
    /// The folder of a plugin to install.
    InstallPath,
    ReviewInstall(PluginInstallPreview),
    ReviewUpdate(Box<PluginUpdatePreview>),
    /// Grant `permission`, which the plugin requests.
    Grant {
        preview: Box<PluginGrantsPreview>,
        permission: PluginPermission,
    },
    /// Withhold `permission` from a plugin that is on.
    Withhold {
        preview: Box<PluginGrantsPreview>,
        permission: PluginPermission,
    },
    /// Grant every permission a plugin lacks and turn it on.
    TurnOn(Box<PluginGrantsPreview>),
    /// A number or text setting's new value; `changed` while it isn't
    /// the default.
    EditSetting {
        plugin_id: String,
        setting: DeclarativeSetting,
        changed: bool,
    },
    Confirm {
        plugin_id: String,
        operation: RuntimePluginOperation,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimePluginOperation {
    ResetSettings,
    Remove,
}

impl App {
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

    fn preview_runtime_plugin_grants(&self, plugin_id: &str) -> Result<PluginGrantsPreview> {
        let paths = self.runtime_plugin_paths()?;
        crate::plugin::runtime::preview_grants_at(plugin_id, &paths.registry, &paths.root)
    }

    #[cfg(test)]
    pub(crate) fn set_runtime_plugin_test_paths(&mut self, registry: PathBuf, root: PathBuf) {
        self.runtime_plugin_paths = Some(crate::plugin::runtime::PluginPaths { registry, root });
        self.sync_plugins_from_config();
    }

    /// Do what a row of the Plugins page asks.
    pub(super) fn apply_plugin_action(&mut self, action: PluginAction) {
        match action {
            PluginAction::Install => self.choose_plugin_folder(None),
            PluginAction::SetEnabled { id, enabled } => self.set_plugin_enabled(id, enabled),
            PluginAction::SetPermission {
                id,
                permission,
                granted,
            } => self.set_plugin_permission(id, permission, granted),
            PluginAction::SetDefaultProfile { id, profile } => {
                self.set_plugin_default_profile(id, profile);
            }
            PluginAction::SetSetting { id, setting, value } => {
                if self.runtime_plugin_setting_schema_is_current(&id, &setting) {
                    let key = setting.id().to_string();
                    let change = match value {
                        Some(value) => PluginConfigurationChange::Set { key, value },
                        None => PluginConfigurationChange::Unset { key },
                    };
                    if let Err(error) = self.configure_plugin(id, change) {
                        self.show_plugin_error("Couldn't change the setting", &error);
                    }
                }
            }
            PluginAction::EditSetting { id, setting } => {
                if !self.runtime_plugin_setting_schema_is_current(&id, &setting) {
                    return;
                }
                let Some(status) = self.current_runtime_plugin_status(&id) else {
                    return;
                };
                let changed = status.configured_settings.contains_key(setting.id());
                let value = status
                    .settings
                    .get(setting.id())
                    .map_or_else(String::new, raw_setting_value);
                self.ask_plugin(
                    PluginQuestion::EditSetting {
                        plugin_id: id,
                        setting: *setting,
                        changed,
                    },
                    value,
                    None,
                );
            }
            PluginAction::Update(id) => match self.preview_runtime_plugin_update(&id) {
                Ok(preview) if preview.is_unchanged() => {
                    self.dialog.show(DialogSpec::message(
                        format!("{} is up to date", preview.name),
                        vec![
                            DialogBlock::text(
                                "Its folder holds the same manifest that's installed, so there's nothing to update.",
                            ),
                            DialogBlock::value(preview.source.display().to_string()),
                        ],
                    ));
                }
                Ok(preview) => self.ask_plugin(
                    PluginQuestion::ReviewUpdate(Box::new(preview)),
                    String::new(),
                    None,
                ),
                Err(error) => {
                    self.show_plugin_error("Couldn't check for an update", &format!("{error:#}"));
                }
            },
            PluginAction::ResetSettings(id) => self.ask_plugin(
                PluginQuestion::Confirm {
                    plugin_id: id,
                    operation: RuntimePluginOperation::ResetSettings,
                },
                String::new(),
                None,
            ),
            PluginAction::Remove(id) => self.ask_plugin(
                PluginQuestion::Confirm {
                    plugin_id: id,
                    operation: RuntimePluginOperation::Remove,
                },
                String::new(),
                None,
            ),
        }
    }

    /// Turn a plugin off, or on: directly when it has every permission it
    /// asks for, and otherwise after a review that grants them.
    fn set_plugin_enabled(&mut self, plugin_id: String, enabled: bool) {
        let Some(status) = self.current_runtime_plugin_status(&plugin_id) else {
            return;
        };
        if status.enabled == enabled {
            return;
        }
        let command = if !enabled {
            PluginAdminCommand::Disable { id: plugin_id }
        } else if status.requested.uncovered_by(&status.granted).is_empty() {
            PluginAdminCommand::Enable {
                id: plugin_id,
                grants: PluginPermissions::default(),
            }
        } else {
            match self.preview_runtime_plugin_grants(&plugin_id) {
                Ok(preview) => self.ask_plugin(
                    PluginQuestion::TurnOn(Box::new(preview)),
                    String::new(),
                    None,
                ),
                Err(error) => {
                    self.show_plugin_error("Couldn't read its permissions", &format!("{error:#}"));
                }
            }
            return;
        };
        self.run_plugin_command(command, "Couldn't change the plugin");
    }

    /// Grant a requested permission after its review, or withhold it: at
    /// once from a plugin that is off, and after asking from one that is
    /// on, since it can run only with every permission it asks for.
    fn set_plugin_permission(
        &mut self,
        plugin_id: String,
        permission: PluginPermission,
        granted: bool,
    ) {
        let preview = match self.preview_runtime_plugin_grants(&plugin_id) {
            Ok(preview) => Box::new(preview),
            Err(error) => {
                return self
                    .show_plugin_error("Couldn't read its permissions", &format!("{error:#}"));
            }
        };
        if preview.requested.scope(permission.capability) != Some(permission.scope) {
            return self.toasts.warning(format!(
                "{} no longer asks for that permission; nothing was changed",
                preview.name
            ));
        }
        let held = preview.current_grants.covers(permission);
        if granted == held {
            return;
        }
        let question = if granted {
            PluginQuestion::Grant {
                preview,
                permission,
            }
        } else if preview.enabled {
            PluginQuestion::Withhold {
                preview,
                permission,
            }
        } else {
            let grants = without(&preview.current_grants, permission);
            let command = preview.command(grants, false);
            self.run_plugin_command(command, "Couldn't change its permissions");
            return;
        };
        self.ask_plugin(question, String::new(), None);
    }

    fn set_plugin_default_profile(&mut self, plugin_id: String, profile: Option<String>) {
        let Some(status) = self.current_runtime_plugin_status(&plugin_id) else {
            return;
        };
        let change = match profile {
            Some(profile) if status.profiles.iter().any(|known| known.id == profile) => {
                PluginConfigurationChange::DefaultProfile(profile)
            }
            Some(profile) => {
                return self.toasts.warning(format!(
                    "Profile {profile:?} no longer exists; nothing was changed"
                ));
            }
            None => PluginConfigurationChange::ClearDefaultProfile,
        };
        if let Err(error) = self.configure_plugin(plugin_id, change) {
            self.show_plugin_error("Couldn't change the default profile", &error);
        }
    }

    /// Ask `question`, with `value` in its field (or the reviewed folder)
    /// and the `error` that refused the last answer.
    fn ask_plugin(&mut self, question: PluginQuestion, value: String, error: Option<&str>) {
        let spec = match &question {
            PluginQuestion::InstallPath => DialogSpec::prompt(
                "Install a plugin",
                vec![DialogBlock::text(format!(
                    "Type the full path of the plugin's folder. It must hold a {} file.",
                    crate::plugin::runtime::MANIFEST_NAME
                ))],
                FieldSpec::new("Folder", value, MAX_FIELD_BYTES)
                    .with_error(error.map(crate::safe_text::capitalized)),
                SAVE,
                "Review",
            ),
            PluginQuestion::ReviewInstall(preview) => install_review(preview, error),
            PluginQuestion::ReviewUpdate(preview) => update_review(preview, error),
            PluginQuestion::Grant {
                preview,
                permission,
            } => {
                let mut body = vec![DialogBlock::value(permission.to_string())];
                if !preview.enabled {
                    body.push(DialogBlock::text(
                        "It stays off until it's turned on; it can run only with every permission it asks for.",
                    ));
                }
                push_error(&mut body, "Couldn't grant it", error);
                DialogSpec::confirm(
                    format!(
                        "Allow {} to {}?",
                        preview.name,
                        lowercase_first(&permission.label())
                    ),
                    body,
                    GRANT,
                    "Allow",
                    ButtonRole::Suggested,
                )
            }
            PluginQuestion::Withhold {
                preview,
                permission,
            } => {
                let mut body = vec![DialogBlock::text(format!(
                    "It can run only with every permission it asks for, so withholding \"{}\" turns it off.",
                    permission.label()
                ))];
                push_error(&mut body, "Couldn't withhold it", error);
                DialogSpec::confirm(
                    format!("Turn off {}?", preview.name),
                    body,
                    WITHHOLD,
                    "Withhold and turn off",
                    ButtonRole::Suggested,
                )
            }
            PluginQuestion::TurnOn(preview) => {
                let missing = preview.requested.uncovered_by(&preview.current_grants);
                let mut body = vec![
                    DialogBlock::text(
                        "It can run only with every permission it asks for. Turning it on grants these:",
                    ),
                    DialogBlock::value(permission_lines(missing)),
                ];
                push_error(&mut body, "Couldn't turn it on", error);
                DialogSpec::confirm(
                    format!("Turn on {}?", preview.name),
                    body,
                    TURN_ON,
                    "Allow and turn on",
                    ButtonRole::Suggested,
                )
            }
            PluginQuestion::EditSetting {
                setting, changed, ..
            } => setting_prompt(setting, *changed, value, error),
            PluginQuestion::Confirm {
                plugin_id,
                operation,
            } => {
                let (title, text, label) = match operation {
                    RuntimePluginOperation::ResetSettings => (
                        format!("Reset the settings of {plugin_id}?"),
                        "Every setting goes back to its default.",
                        "Reset",
                    ),
                    RuntimePluginOperation::Remove => (
                        format!("Remove {plugin_id}?"),
                        "Its installed copy and registry entry are deleted. The folder it came from is left alone.",
                        "Remove",
                    ),
                };
                DialogSpec::confirm(
                    title,
                    vec![DialogBlock::text(text)],
                    CONFIRM,
                    label,
                    ButtonRole::Destructive,
                )
            }
        };
        let request_id = self.dialog.show(spec);
        self.dialog_flow = Some(DialogFlow::Plugin(PendingPluginQuestion {
            request_id,
            question,
        }));
    }

    /// Answer a plugin question. Returns whether the answer was its; a
    /// plain Cancel isn't, and closes the dialog.
    pub(super) fn handle_plugin_choice(&mut self, request_id: u64, choice_id: &str) -> bool {
        let question = match &self.dialog_flow {
            Some(DialogFlow::Plugin(pending)) if pending.request_id == request_id => {
                pending.question.clone()
            }
            _ => return false,
        };
        let field = self.dialog.field_value(request_id).map(ToOwned::to_owned);
        let finish = |app: &mut Self| {
            app.dialog.dismiss(request_id);
            app.dialog_flow = None;
        };
        match (question, choice_id) {
            (PluginQuestion::InstallPath, SAVE) => {
                finish(self);
                self.review_plugin_install(field.unwrap_or_default());
            }
            // Back from the review chooses again, beside the folder it read.
            (PluginQuestion::ReviewInstall(preview), CANCEL) => {
                finish(self);
                self.choose_plugin_folder(Some(preview.source));
            }
            (PluginQuestion::ReviewInstall(preview), INSTALL_DISABLED | INSTALL_ENABLED) => {
                finish(self);
                let enable = choice_id == INSTALL_ENABLED;
                let grants = if enable {
                    preview.permissions.clone()
                } else {
                    PluginPermissions::default()
                };
                self.install_plugin(preview, grants, enable);
            }
            (PluginQuestion::ReviewUpdate(preview), UPDATE_DISABLED | UPDATE_ENABLED) => {
                finish(self);
                let enable = choice_id == UPDATE_ENABLED;
                // Permissions that stay keep their grants; turning it on
                // grants what the update newly asks for, as reviewed.
                let grants = if enable {
                    preview.candidate_permissions.clone()
                } else {
                    preview
                        .current_grants
                        .intersection(&preview.candidate_permissions)
                };
                let command = preview.command(grants, enable);
                self.run_plugin_review(command, PluginQuestion::ReviewUpdate(preview));
            }
            (
                PluginQuestion::Grant {
                    preview,
                    permission,
                },
                GRANT,
            ) => {
                finish(self);
                let mut grants = preview.current_grants.clone();
                grants.set(permission);
                let enable = preview.enabled && preview.requested.uncovered_by(&grants).is_empty();
                let command = preview.command(grants, enable);
                self.run_plugin_review(
                    command,
                    PluginQuestion::Grant {
                        preview,
                        permission,
                    },
                );
            }
            (
                PluginQuestion::Withhold {
                    preview,
                    permission,
                },
                WITHHOLD,
            ) => {
                finish(self);
                let command = preview.command(without(&preview.current_grants, permission), false);
                self.run_plugin_review(
                    command,
                    PluginQuestion::Withhold {
                        preview,
                        permission,
                    },
                );
            }
            (PluginQuestion::TurnOn(preview), TURN_ON) => {
                finish(self);
                let command = preview.command(preview.requested.clone(), true);
                self.run_plugin_review(command, PluginQuestion::TurnOn(preview));
            }
            (
                PluginQuestion::EditSetting {
                    plugin_id,
                    setting,
                    changed,
                },
                SAVE | UNSET,
            ) => {
                finish(self);
                if !self.runtime_plugin_setting_schema_is_current(&plugin_id, &setting) {
                    return true;
                }
                let value = field.unwrap_or_default();
                let key = setting.id().to_string();
                let change = if choice_id == UNSET {
                    PluginConfigurationChange::Unset { key }
                } else {
                    PluginConfigurationChange::Set {
                        key,
                        value: value.clone(),
                    }
                };
                if let Err(error) = self.configure_plugin(plugin_id.clone(), change) {
                    self.ask_plugin(
                        PluginQuestion::EditSetting {
                            plugin_id,
                            setting,
                            changed,
                        },
                        value,
                        Some(&error),
                    );
                }
            }
            (
                PluginQuestion::Confirm {
                    plugin_id,
                    operation,
                },
                CONFIRM,
            ) => {
                finish(self);
                let command = match operation {
                    RuntimePluginOperation::ResetSettings => PluginAdminCommand::Configure {
                        id: plugin_id.clone(),
                        change: PluginConfigurationChange::ResetSettings,
                    },
                    RuntimePluginOperation::Remove => PluginAdminCommand::Remove {
                        id: plugin_id.clone(),
                    },
                };
                if self.run_plugin_command(command, "Couldn't change the plugin")
                    && operation == RuntimePluginOperation::Remove
                {
                    self.settings_panel
                        .close_sub_page(&SettingsSubPage::Plugin(plugin_id));
                }
            }
            _ => return false,
        }
        true
    }

    /// Open the desktop's folder chooser for a plugin to install, starting
    /// beside `previous` (the folder last reviewed), else ask for its path.
    fn choose_plugin_folder(&mut self, previous: Option<PathBuf>) {
        let start = previous
            .as_deref()
            .and_then(std::path::Path::parent)
            .map(std::path::Path::to_path_buf);
        let opened = self.open_file_chooser(
            super::file_chooser::FileChooserPurpose::PluginFolder,
            move || crate::portal::choose_folder("Choose a plugin folder", start.as_deref()),
        );
        if !opened {
            let folder = previous
                .map(|folder| folder.display().to_string())
                .unwrap_or_default();
            self.ask_plugin(PluginQuestion::InstallPath, folder, None);
        }
    }

    /// Review the plugin in the folder the chooser returned, or ask for its
    /// path when no chooser could open.
    pub(super) fn handle_plugin_folder_chosen(
        &mut self,
        result: std::result::Result<crate::portal::FileChoice, String>,
    ) {
        match result {
            Ok(crate::portal::FileChoice::Chosen(folder)) => match folder.to_str() {
                Some(folder) => self.review_plugin_install(folder.to_owned()),
                None => self.ask_plugin(
                    PluginQuestion::InstallPath,
                    folder.display().to_string(),
                    Some("the folder's path isn't valid UTF-8; move or rename it"),
                ),
            },
            Ok(crate::portal::FileChoice::Cancelled) => {}
            Err(error) => {
                log::info!("No folder chooser ({error}); asking for a path instead");
                self.ask_plugin(PluginQuestion::InstallPath, String::new(), None);
            }
        }
    }

    /// Read the plugin in `folder` and review installing it, or ask for
    /// the folder again with why it can't be.
    fn review_plugin_install(&mut self, folder: String) {
        let directory = PathBuf::from(&folder);
        let preview = if folder.is_empty() {
            Err(anyhow::anyhow!("type the plugin's folder"))
        } else if !directory.is_absolute() {
            Err(anyhow::anyhow!("type the full path, starting with /"))
        } else {
            preview_install(&directory)
        };
        let refusal = match preview {
            Ok(preview) => match self.discover_runtime_plugins_for_app() {
                Ok((_, statuses)) if statuses.iter().any(|status| status.id == preview.id) => {
                    format!(
                        "{:?} is already installed; update it from its page instead",
                        preview.id
                    )
                }
                Ok(_) => {
                    return self.ask_plugin(
                        PluginQuestion::ReviewInstall(preview),
                        String::new(),
                        None,
                    );
                }
                Err(error) => format!("couldn't read the plugin registry: {error:#}"),
            },
            Err(error) => format!("{error:#}"),
        };
        self.ask_plugin(PluginQuestion::InstallPath, folder, Some(&refusal));
    }

    fn install_plugin(
        &mut self,
        preview: PluginInstallPreview,
        grants: PluginPermissions,
        enable: bool,
    ) {
        let command = PluginAdminCommand::InstallReviewed {
            directory: preview.source.clone(),
            grants,
            enable,
            expected_source_fingerprint: preview.source_fingerprint.clone(),
        };
        match self.execute_runtime_plugin_admin(command) {
            Ok(message) => {
                self.reload_plugins();
                self.toasts.success(message);
                self.settings_panel
                    .open_sub_page(SettingsSubPage::Plugin(preview.id));
            }
            Err(error) => self.ask_plugin(
                PluginQuestion::ReviewInstall(preview),
                String::new(),
                Some(&format!("{error:#}")),
            ),
        }
    }

    /// Run a reviewed command, or show its review again with why it was
    /// refused, such as the registry having changed since.
    fn run_plugin_review(&mut self, command: PluginAdminCommand, review: PluginQuestion) {
        match self.execute_runtime_plugin_admin(command) {
            Ok(message) => {
                self.reload_plugins();
                self.toasts.success(message);
            }
            Err(error) => self.ask_plugin(review, String::new(), Some(&format!("{error:#}"))),
        }
    }

    /// Run `command`, showing what went wrong under `failure`. Returns
    /// whether it ran.
    fn run_plugin_command(&mut self, command: PluginAdminCommand, failure: &str) -> bool {
        match self.execute_runtime_plugin_admin(command) {
            Ok(message) => {
                self.reload_plugins();
                self.toasts.success(message);
                true
            }
            Err(error) => {
                self.show_plugin_error(failure, &format!("{error:#}"));
                false
            }
        }
    }

    fn configure_plugin(
        &mut self,
        plugin_id: String,
        change: PluginConfigurationChange,
    ) -> std::result::Result<(), String> {
        let command = PluginAdminCommand::Configure {
            id: plugin_id,
            change,
        };
        let message = self
            .execute_runtime_plugin_admin(command)
            .map_err(|error| format!("{error:#}"))?;
        self.reload_plugins();
        self.toasts.success(message);
        Ok(())
    }

    fn show_plugin_error(&mut self, title: &str, error: &str) {
        self.dialog
            .show(DialogSpec::error(title, vec![DialogBlock::value(error)]));
    }

    /// The plugin's status as the registry has it now, or why not, told
    /// as a toast or an error.
    fn current_runtime_plugin_status(&mut self, plugin_id: &str) -> Option<RuntimePluginStatus> {
        match self.discover_runtime_plugins_for_app() {
            Ok((_, statuses)) => {
                let status = statuses.into_iter().find(|status| status.id == plugin_id);
                if status.is_none() {
                    self.toasts
                        .warning(format!("Plugin {plugin_id} is no longer installed"));
                }
                status
            }
            Err(error) => {
                self.show_plugin_error("Couldn't read the plugin registry", &format!("{error:#}"));
                None
            }
        }
    }

    /// Whether `setting` is still what the plugin's manifest declares, as
    /// the row or prompt showed it; a toast says when it isn't.
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
        let same = current.as_ref() == Some(setting);
        if !same {
            self.toasts.warning(format!(
                "Setting {} changed after it was shown; review it again",
                setting.id()
            ));
        }
        same
    }
}

/// `grants` without `permission`'s capability.
fn without(grants: &PluginPermissions, permission: PluginPermission) -> PluginPermissions {
    let mut grants = grants.clone();
    grants.remove(permission.capability);
    grants
}

fn lowercase_first(text: &str) -> String {
    let mut characters = text.chars();
    characters
        .next()
        .map(|first| first.to_lowercase().chain(characters).collect())
        .unwrap_or_default()
}

fn push_error(body: &mut Vec<DialogBlock>, prefix: &str, error: Option<&str>) {
    if let Some(error) = error {
        body.push(DialogBlock::error(format!("{prefix}: {error}")));
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

/// One line per permission: what it allows, with its window scope, and its
/// name.
fn permission_lines(permissions: impl IntoIterator<Item = PluginPermission>) -> String {
    permissions
        .into_iter()
        .map(|permission| format!("{} · {permission}", permission.label()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// What a review says about the permissions it grants.
fn push_requests(body: &mut Vec<DialogBlock>, requested: &PluginPermissions) {
    if requested.is_empty() {
        body.push(DialogBlock::text("It asks for no permissions."));
    } else {
        body.push(DialogBlock::text(
            "It asks for these permissions. Turning it on grants them all; turned off, it gets none until you grant them on its page.",
        ));
        body.push(DialogBlock::value(permission_lines(requested.iter())));
    }
}

fn install_review(preview: &PluginInstallPreview, error: Option<&str>) -> DialogSpec {
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
    push_requests(&mut body, &preview.permissions);
    push_error(&mut body, "Couldn't install it", error);
    let spec = DialogSpec::confirm(
        format!("Install {}?", preview.name),
        body,
        INSTALL_ENABLED,
        "Install and turn on",
        ButtonRole::Suggested,
    )
    .cancel_label("Back");
    between(
        spec,
        DialogButton::answer(INSTALL_DISABLED, "Install turned off", ButtonRole::Neutral),
    )
}

/// `spec` with `button` between its Cancel and its action.
fn between(mut spec: DialogSpec, button: DialogButton) -> DialogSpec {
    spec.buttons.insert(1, button);
    spec
}

fn update_review(preview: &PluginUpdatePreview, error: Option<&str>) -> DialogSpec {
    let added = preview
        .candidate_permissions
        .uncovered_by(&preview.current_permissions);
    let removed = preview
        .current_permissions
        .uncovered_by(&preview.candidate_permissions);
    let mut body = vec![DialogBlock::value(
        [
            if preview.current_version == preview.candidate_version {
                format!(
                    "{} {} (the same version, with a changed manifest)",
                    preview.id, preview.candidate_version
                )
            } else {
                format!(
                    "{} {} → {}",
                    preview.id, preview.current_version, preview.candidate_version
                )
            },
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
    body.push(DialogBlock::text(if added.is_empty() {
        "Permissions that stay keep their grants. It can run only with every permission it asks for."
    } else {
        "Permissions that stay keep their grants. Turning it on also grants the new ones; it can run only with every permission it asks for."
    }));
    if !preview.candidate_permissions.is_empty() {
        body.push(DialogBlock::value(permission_lines(
            preview.candidate_permissions.iter(),
        )));
    }
    push_error(&mut body, "Couldn't update it", error);
    let (disabled_label, enabled_label) = if preview.enabled {
        ("Update and turn off", "Update and keep on")
    } else {
        ("Update turned off", "Update and turn on")
    };
    let spec = DialogSpec::confirm(
        format!("Update {}?", preview.name),
        body,
        UPDATE_ENABLED,
        enabled_label,
        ButtonRole::Suggested,
    );
    between(
        spec,
        DialogButton::answer(UPDATE_DISABLED, disabled_label, ButtonRole::Neutral),
    )
}

/// The prompt for a number or text setting's new value, with "Use
/// default" while it is `changed`.
fn setting_prompt(
    setting: &DeclarativeSetting,
    changed: bool,
    value: String,
    error: Option<&str>,
) -> DialogSpec {
    let mut details = vec![format!(
        "Default {}",
        setting_value_text(&setting.default_value())
    )];
    let max_bytes = match setting {
        DeclarativeSetting::Integer { min, max, .. } => {
            details.push(format!("From {min} to {max}"));
            32
        }
        _ => {
            details.push(format!(
                "One line, at most {} UTF-8 bytes",
                crate::plugin::runtime::MAX_SETTING_TEXT_BYTES
            ));
            crate::plugin::runtime::MAX_SETTING_TEXT_BYTES
        }
    };
    let mut body = Vec::new();
    if !setting.description().is_empty() {
        body.push(DialogBlock::text(setting.description()));
    }
    body.push(DialogBlock::value(details.join("\n")));
    let spec = DialogSpec::prompt(
        format!("Change {}", setting.name()),
        body,
        FieldSpec::new("Value", value, max_bytes)
            .with_error(error.map(crate::safe_text::capitalized)),
        SAVE,
        "Save",
    );
    if changed {
        between(
            spec,
            DialogButton::answer(UNSET, "Use default", ButtonRole::Neutral),
        )
    } else {
        spec
    }
}

/// A setting's value as typed into its prompt.
fn raw_setting_value(value: &crate::plugin::runtime::PluginSettingValue) -> String {
    match value {
        crate::plugin::runtime::PluginSettingValue::Boolean(value) => value.to_string(),
        crate::plugin::runtime::PluginSettingValue::Integer(value) => value.to_string(),
        crate::plugin::runtime::PluginSettingValue::Text(value) => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::permission::{Capability, PermissionScope};
    use crate::plugin::runtime::{InstalledPlugin, PluginSettingValue};
    use crate::ui::dialog::DialogAction;
    use crate::ui::settings::{SettingKind, SettingsAction, SettingsPage, SettingsSection};

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

    /// A test registry and installed-plugin root under a temporary
    /// directory.
    struct Fixture {
        temp: tempfile::TempDir,
        registry: PathBuf,
        root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let registry = temp.path().join("registry.toml");
            let root = temp.path().join("installed");
            std::fs::create_dir_all(&root).unwrap();
            Self {
                temp,
                registry,
                root,
            }
        }

        fn base(&self) -> &std::path::Path {
            self.temp.path()
        }

        fn statuses(&self) -> Vec<RuntimePluginStatus> {
            crate::plugin::runtime::discover_runtime_plugins_at(&self.registry, &self.root)
                .unwrap()
                .1
        }

        /// Settings open on the Plugins page over these plugins.
        fn app(&self) -> App {
            let mut app = App::new_headless(Config::default()).unwrap();
            app.set_runtime_plugin_test_paths(self.registry.clone(), self.root.clone());
            app.settings_panel.set_reduced_motion(true);
            app.dialog.set_reduced_motion(true);
            app.settings_panel.toggle();
            app.settings_panel.select_page(SettingsPage::Plugins);
            app
        }
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

    /// Pick `option` from the dropdown of the row `label`.
    fn choose(app: &mut App, section: SettingsSection, label: &str, option: &str) {
        assert!(app.settings_panel.accessibility_set_dropdown_expanded(
            section,
            label,
            true,
            &app.config,
            &app.shortcut_registry,
        ));
        let action = app.settings_panel.accessibility_choose_dropdown_option(
            section,
            label,
            option,
            &app.config,
            &app.shortcut_registry,
        );
        assert!(
            !matches!(action, SettingsAction::None),
            "{label:?} has no {option:?}"
        );
        app.apply_settings_action(action);
    }

    /// Flip a plugin row's switch.
    fn flip(app: &mut App, label: &str) {
        let action = app.settings_panel.accessibility_flip_switch(
            SettingsSection::Plugins,
            label,
            &app.config,
            &app.shortcut_registry,
        );
        app.apply_settings_action(action);
    }

    fn open_plugin(app: &mut App, id: &str) {
        app.apply_settings_action(SettingsAction::OpenSubPage(SettingsSubPage::Plugin(
            id.into(),
        )));
        assert_eq!(
            app.settings_panel.sub_page(),
            Some(&SettingsSubPage::Plugin(id.into()))
        );
    }

    fn rows(app: &App) -> Vec<crate::ui::settings::SettingEntry> {
        app.settings_panel
            .build_entries(&app.config, &app.shortcut_registry)
    }

    fn row(app: &App, label: &str) -> Option<crate::ui::settings::SettingEntry> {
        rows(app).into_iter().find(|entry| entry.label == label)
    }

    fn shows_error(app: &App) -> bool {
        app.dialog
            .body()
            .iter()
            .any(|block| matches!(block, DialogBlock::Error(_)))
    }

    #[test]
    fn a_custom_agent_taking_an_installed_plugin_id_is_reported_by_name() {
        let fixture = Fixture::new();
        install_fixture(
            fixture.base(),
            &fixture.registry,
            &fixture.root,
            "local-tool",
            false,
        );
        crate::plugin::runtime::execute_admin_at(
            PluginAdminCommand::Enable {
                id: "local-tool".into(),
                grants: PluginPermissions::default(),
            },
            &fixture.registry,
            &fixture.root,
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
        app.set_runtime_plugin_test_paths(fixture.registry.clone(), fixture.root.clone());
        app.sync_plugins_from_config();
        assert!(app.toasts.iter().any(|toast| toast.message
            == "Third-party plugin local-tool was not loaded: the agent Local Agent uses the same ID"));
    }

    #[test]
    fn install_reviews_the_plugin_and_opens_its_page() {
        let fixture = Fixture::new();
        let source = fixture.base().join("source-local-tool");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(
            source.join(crate::plugin::runtime::MANIFEST_NAME),
            fixture_manifest("local-tool", "1.0.0", false),
        )
        .unwrap();
        let mut app = fixture.app();

        activate(&mut app, SettingsSection::Plugins, "Install a plugin…");
        app.answer_dialog(CANCEL);
        assert!(!app.dialog.is_awaiting_current());
        activate(&mut app, SettingsSection::Plugins, "Install a plugin…");
        app.submit_dialog_field("relative/plugin");
        let field = app.dialog.field(app.dialog.request_id()).unwrap();
        assert_eq!(field.value, "relative/plugin");
        assert!(field.error.is_some());

        app.submit_dialog_field(&source.display().to_string());
        let text = app.dialog_text();
        assert!(text.contains(&source.display().to_string()));
        assert!(text.contains("fnv1a64:"));
        assert!(text.contains("Start processes · process-spawn"));
        // Back returns to the folder it reviewed.
        app.answer_dialog(CANCEL);
        assert_eq!(
            app.dialog.field_value(app.dialog.request_id()),
            Some(source.display().to_string().as_str())
        );
        app.submit_dialog_field(&source.display().to_string());
        app.answer_dialog(INSTALL_ENABLED);

        let statuses = fixture.statuses();
        assert_eq!(statuses.len(), 1);
        assert!(statuses[0].enabled);
        assert_eq!(statuses[0].granted, permissions(&["process-spawn"]));
        open_plugin(&mut app, "local-tool");
        assert!(row(&app, "Remove…").is_some());

        // The same plugin again is refused at its folder.
        app.settings_panel.leave_sub_page();
        activate(&mut app, SettingsSection::Plugins, "Install a plugin…");
        app.submit_dialog_field(&source.display().to_string());
        assert!(
            app.dialog
                .field(app.dialog.request_id())
                .unwrap()
                .error
                .is_some()
        );
        assert_eq!(fixture.statuses().len(), 1);
    }

    #[test]
    fn a_folder_chosen_in_the_desktops_chooser_is_reviewed() {
        use crate::portal::FileChoice;
        let fixture = Fixture::new();
        let source = fixture.base().join("source-chosen-tool");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(
            source.join(crate::plugin::runtime::MANIFEST_NAME),
            fixture_manifest("chosen-tool", "1.0.0", false),
        )
        .unwrap();
        let empty = fixture.base().join("not-a-plugin");
        std::fs::create_dir_all(&empty).unwrap();
        let mut app = fixture.app();

        // Closing the chooser changes nothing.
        app.handle_plugin_folder_chosen(Ok(FileChoice::Cancelled));
        assert!(!app.dialog.is_awaiting_current());

        // A plugin folder goes straight to its review.
        app.handle_plugin_folder_chosen(Ok(FileChoice::Chosen(source.clone())));
        assert!(
            app.dialog_text().contains("fnv1a64:"),
            "{}",
            app.dialog_text()
        );
        app.answer_dialog(CANCEL);

        // A folder with no plugin in it is refused where it can be retyped.
        app.handle_plugin_folder_chosen(Ok(FileChoice::Chosen(empty.clone())));
        let field = app.dialog.field(app.dialog.request_id()).unwrap();
        assert_eq!(field.value, empty.display().to_string());
        assert!(field.error.is_some());
        app.answer_dialog(CANCEL);

        // Without a chooser, the path is asked for.
        app.handle_plugin_folder_chosen(Err("no portal".into()));
        app.submit_dialog_field(&source.display().to_string());
        app.answer_dialog(INSTALL_DISABLED);
        assert_eq!(fixture.statuses().len(), 1);
    }

    #[test]
    fn turning_on_reviews_missing_grants_and_withholding_one_turns_it_off() {
        let fixture = Fixture::new();
        let source = fixture.base().join("source-local-tool");
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
            &fixture.registry,
            &fixture.root,
        )
        .unwrap();
        // The review reads the installed manifest, not the source.
        std::fs::remove_dir_all(&source).unwrap();
        let status = || {
            let statuses = fixture.statuses();
            (statuses[0].granted.clone(), statuses[0].enabled)
        };
        let mut app = fixture.app();
        let label = "Fixture local-tool";
        assert_eq!(row(&app, label).unwrap().value, "1.0.0 · Off");

        // Its switch asks before granting what it lacks.
        flip(&mut app, label);
        assert!(app.dialog.answers(app.dialog.request_id(), TURN_ON));
        assert!(app.dialog_text().contains("process-spawn"));
        assert_eq!(status(), (PluginPermissions::default(), false));
        app.answer_dialog(TURN_ON);
        assert_eq!(status(), (permissions(&["process-spawn"]), true));
        assert_eq!(row(&app, label).unwrap().value, "1.0.0 · On");

        // Withholding from a plugin that is on asks, then turns it off.
        open_plugin(&mut app, "local-tool");
        choose(
            &mut app,
            SettingsSection::PluginPermissions,
            "Start processes",
            "Off",
        );
        assert_eq!(status(), (permissions(&["process-spawn"]), true));
        app.answer_dialog(WITHHOLD);
        assert_eq!(status(), (PluginPermissions::default(), false));

        // Granting asks too, and leaves it off.
        choose(
            &mut app,
            SettingsSection::PluginPermissions,
            "Start processes",
            "On",
        );
        let review = app.dialog.request_id();
        // A review the registry has moved past is refused, not applied.
        crate::plugin::runtime::execute_admin_at(
            PluginAdminCommand::Grant {
                id: "local-tool".into(),
                grants: permissions(&["process-spawn"]),
            },
            &fixture.registry,
            &fixture.root,
        )
        .unwrap();
        app.answer_dialog(GRANT);
        assert_ne!(app.dialog.request_id(), review);
        assert!(shows_error(&app));
        assert_eq!(status(), (permissions(&["process-spawn"]), false));

        // The switch turns it off directly.
        app.answer_dialog(CANCEL);
        flip(&mut app, label);
        app.settings_panel.leave_sub_page();
        flip(&mut app, label);
        assert_eq!(status(), (permissions(&["process-spawn"]), true));
        flip(&mut app, label);
        assert_eq!(status(), (permissions(&["process-spawn"]), false));
    }

    #[test]
    fn an_install_refuses_a_source_changed_after_its_review() {
        let fixture = Fixture::new();
        let source = fixture.base().join("source-stale-tool");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(
            source.join(crate::plugin::runtime::MANIFEST_NAME),
            fixture_manifest("stale-tool", "1.0.0", false),
        )
        .unwrap();
        let mut app = fixture.app();
        activate(&mut app, SettingsSection::Plugins, "Install a plugin…");
        app.submit_dialog_field(&source.display().to_string());
        let review = app.dialog.request_id();
        std::fs::write(
            source.join(crate::plugin::runtime::MANIFEST_NAME),
            fixture_manifest("stale-tool", "2.0.0", false),
        )
        .unwrap();
        // An answer for another request does nothing.
        app.apply_dialog_action(DialogAction::Answer {
            request_id: review + 1,
            id: INSTALL_DISABLED.into(),
        });
        assert_eq!(app.dialog.request_id(), review);
        app.answer_dialog(INSTALL_DISABLED);
        assert!(
            app.dialog
                .answers(app.dialog.request_id(), INSTALL_DISABLED)
        );
        assert!(shows_error(&app));
        assert!(fixture.statuses().is_empty());
        assert!(!fixture.root.join("stale-tool").exists());
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
        app.ask_plugin(PluginQuestion::ReviewInstall(preview), String::new(), None);
        let text = app.dialog_text();
        for permission in every_permission().iter() {
            assert!(
                text.contains(&format!("{} · {permission}", permission.label())),
                "{permission}"
            );
        }
        assert!(text.contains("Create panes (all windows) · pane-create:all-windows"));
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
        app.ask_plugin(
            PluginQuestion::ReviewUpdate(Box::new(preview)),
            String::new(),
            None,
        );
        let text = app.dialog_text();
        for permission in every_permission().iter() {
            assert!(
                text.contains(&format!("{} · {permission}", permission.label())),
                "{permission}"
            );
        }
    }

    #[test]
    fn the_default_profile_is_chosen_and_cleared_without_other_changes() {
        let fixture = Fixture::new();
        install_profile_fixture(
            fixture.base(),
            &fixture.registry,
            &fixture.root,
            "profile-tool",
            2,
        );
        crate::plugin::runtime::execute_admin_at(
            PluginAdminCommand::Configure {
                id: "profile-tool".into(),
                change: PluginConfigurationChange::Set {
                    key: "verbose".into(),
                    value: "false".into(),
                },
            },
            &fixture.registry,
            &fixture.root,
        )
        .unwrap();
        let installed_manifest = fixture
            .root
            .join("profile-tool")
            .join(crate::plugin::runtime::MANIFEST_NAME);
        let installed_bytes = std::fs::read(&installed_manifest).unwrap();
        let before = fixture.statuses().remove(0);

        let mut app = fixture.app();
        open_plugin(&mut app, "profile-tool");
        let general = SettingsSection::PluginGeneral;
        assert_eq!(
            row(&app, "Default Profile").unwrap().value,
            "Default (Profile 0)"
        );
        choose(&mut app, general, "Default Profile", "Profile 1");
        let selected = fixture.statuses().remove(0);
        assert_eq!(
            selected.configured_default_profile.as_deref(),
            Some("profile-1")
        );
        assert_eq!(row(&app, "Default Profile").unwrap().value, "Profile 1");
        assert_eq!(selected.enabled, before.enabled);
        assert_eq!(selected.source, before.source);
        assert_eq!(selected.requested, before.requested);
        assert_eq!(selected.granted, before.granted);
        assert_eq!(selected.settings, before.settings);

        choose(&mut app, general, "Default Profile", "Default (Profile 0)");
        assert!(fixture.statuses()[0].configured_default_profile.is_none());
        assert_eq!(std::fs::read(installed_manifest).unwrap(), installed_bytes);

        // A profile the plugin no longer has is refused.
        app.apply_settings_action(SettingsAction::Plugin(PluginAction::SetDefaultProfile {
            id: "profile-tool".into(),
            profile: Some("not-declared".into()),
        }));
        assert!(fixture.statuses()[0].configured_default_profile.is_none());
    }

    #[test]
    fn every_profile_is_offered_and_an_event_only_plugin_has_no_default_profile() {
        let fixture = Fixture::new();
        install_profile_fixture(
            fixture.base(),
            &fixture.registry,
            &fixture.root,
            "many-profiles",
            32,
        );
        let event_source = fixture.base().join("source-event-only");
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
            &fixture.registry,
            &fixture.root,
        )
        .unwrap();

        let mut app = fixture.app();
        open_plugin(&mut app, "many-profiles");
        let profile = row(&app, "Default Profile").unwrap();
        let SettingKind::Dropdown { options, .. } = profile.kind else {
            panic!("the default profile is a dropdown");
        };
        assert_eq!(options.len(), 33, "the default and every profile");
        app.settings_panel.leave_sub_page();
        open_plugin(&mut app, "event-only");
        assert!(row(&app, "Default Profile").is_none());
        // Its permissions are toggles, one per request.
        assert_eq!(
            rows(&app)
                .iter()
                .filter(
                    |entry| entry.section == SettingsSection::PluginPermissions && !entry.is_header
                )
                .count(),
            2
        );
    }

    #[test]
    fn update_reset_and_remove_ask_first_and_keep_the_source() {
        let fixture = Fixture::new();
        let source = install_fixture(
            fixture.base(),
            &fixture.registry,
            &fixture.root,
            "fixture-tool",
            true,
        );
        crate::plugin::runtime::execute_admin_at(
            PluginAdminCommand::Configure {
                id: "fixture-tool".into(),
                change: PluginConfigurationChange::Set {
                    key: "verbose".into(),
                    value: "false".into(),
                },
            },
            &fixture.registry,
            &fixture.root,
        )
        .unwrap();
        let mut app = fixture.app();
        open_plugin(&mut app, "fixture-tool");
        assert_eq!(
            row(&app, "Source").unwrap().value,
            source.display().to_string()
        );
        assert_eq!(row(&app, "Verbose").unwrap().value, "Off");
        let manage = SettingsSection::PluginManage;

        std::fs::write(
            source.join(crate::plugin::runtime::MANIFEST_NAME),
            fixture_manifest("fixture-tool", "2.0.0", true),
        )
        .unwrap();
        activate(&mut app, manage, "Update…");
        let review = app.dialog.request_id();
        app.apply_dialog_action(DialogAction::Answer {
            request_id: review + 1,
            id: UPDATE_DISABLED.into(),
        });
        assert_eq!(fixture.statuses()[0].version, "1.0.0");
        app.answer_dialog(UPDATE_DISABLED);
        let statuses = fixture.statuses();
        assert_eq!(statuses[0].version, "2.0.0");
        assert_eq!(statuses[0].configured_setting_count, 1);
        assert_eq!(row(&app, "Version").unwrap().value, "2.0.0");

        // With the same manifest in its folder, there is nothing to update.
        let before = app.dialog.request_id();
        activate(&mut app, manage, "Update…");
        assert_ne!(app.dialog.request_id(), before);
        assert!(
            app.dialog.title().ends_with("is up to date"),
            "{}",
            app.dialog.title()
        );
        app.apply_dialog_action(DialogAction::Close {
            request_id: app.dialog.request_id(),
        });
        let reported = crate::plugin::runtime::execute_admin_at(
            PluginAdminCommand::Update {
                id: "fixture-tool".into(),
            },
            &fixture.registry,
            &fixture.root,
        )
        .unwrap();
        assert!(reported.contains("is up to date"), "{reported}");

        activate(&mut app, manage, "Reset settings…");
        assert_eq!(
            app.dialog.focus(),
            Some(crate::ui::dialog::DialogFocus::Button(0)),
            "Cancel is focused"
        );
        app.answer_dialog(CONFIRM);
        assert_eq!(fixture.statuses()[0].configured_setting_count, 0);
        assert!(row(&app, "Reset settings…").is_none());

        activate(&mut app, manage, "Remove…");
        app.answer_dialog(CANCEL);
        assert_eq!(fixture.statuses().len(), 1);
        activate(&mut app, manage, "Remove…");
        app.answer_dialog(CONFIRM);
        assert!(fixture.statuses().is_empty());
        assert!(source.is_dir(), "the original source must never be removed");
        // Back on the Plugins page.
        assert_eq!(app.settings_panel.sub_page(), None);
        assert!(row(&app, "Install a plugin…").is_some());
    }

    #[test]
    fn an_update_reviews_permission_changes_before_turning_it_on() {
        let fixture = Fixture::new();
        let source = install_fixture(
            fixture.base(),
            &fixture.registry,
            &fixture.root,
            "permission-tool",
            false,
        );
        let candidate = fixture_manifest("permission-tool", "2.0.0", false).replace(
            "permissions = [\"process-spawn\"]",
            "permissions = [\"process-spawn\", \"workspace-query\"]",
        );
        std::fs::write(
            source.join(crate::plugin::runtime::MANIFEST_NAME),
            candidate,
        )
        .unwrap();
        let mut app = fixture.app();
        open_plugin(&mut app, "permission-tool");
        activate(&mut app, SettingsSection::PluginManage, "Update…");
        let text = app.dialog_text();
        assert!(text.contains("1.0.0 → 2.0.0"));
        assert!(text.contains("Newly requested: workspace-query"));
        app.answer_dialog(UPDATE_ENABLED);
        let statuses = fixture.statuses();
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
        activate(&mut app, SettingsSection::PluginManage, "Update…");
        assert!(
            app.dialog_text()
                .contains("Newly requested: workspace-query:all-windows")
        );
        app.answer_dialog(UPDATE_ENABLED);
        assert_eq!(
            fixture.statuses()[0].granted,
            permissions(&["process-spawn", "workspace-query:all-windows"])
        );

        // An update that drops a permission drops its grant and stays on.
        std::fs::write(
            source.join(crate::plugin::runtime::MANIFEST_NAME),
            fixture_manifest("permission-tool", "3.0.0", false),
        )
        .unwrap();
        activate(&mut app, SettingsSection::PluginManage, "Update…");
        assert!(
            app.dialog_text()
                .contains("No longer requested: workspace-query:all-windows")
        );
        app.answer_dialog(UPDATE_ENABLED);
        let statuses = fixture.statuses();
        assert_eq!(statuses[0].version, "3.0.0");
        assert_eq!(statuses[0].granted, permissions(&["process-spawn"]));
        assert!(statuses[0].enabled);

        // Kept turned off, it keeps only the grants it had.
        let again = fixture_manifest("permission-tool", "4.0.0", false).replace(
            "permissions = [\"process-spawn\"]",
            "permissions = [\"process-spawn\", \"workspace-query\"]",
        );
        std::fs::write(source.join(crate::plugin::runtime::MANIFEST_NAME), again).unwrap();
        activate(&mut app, SettingsSection::PluginManage, "Update…");
        app.answer_dialog(UPDATE_DISABLED);
        let statuses = fixture.statuses();
        assert_eq!(statuses[0].version, "4.0.0");
        assert_eq!(statuses[0].granted, permissions(&["process-spawn"]));
        assert!(!statuses[0].enabled);
    }

    #[test]
    fn an_update_review_stays_open_when_its_source_drifts() {
        let fixture = Fixture::new();
        let source = install_fixture(
            fixture.base(),
            &fixture.registry,
            &fixture.root,
            "drift-tool",
            false,
        );
        std::fs::write(
            source.join(crate::plugin::runtime::MANIFEST_NAME),
            fixture_manifest("drift-tool", "2.0.0", false),
        )
        .unwrap();
        let installed_manifest = fixture
            .root
            .join("drift-tool")
            .join(crate::plugin::runtime::MANIFEST_NAME);
        let installed_before = std::fs::read(&installed_manifest).unwrap();
        let registry_before = std::fs::read(&fixture.registry).unwrap();
        let mut app = fixture.app();
        open_plugin(&mut app, "drift-tool");
        activate(&mut app, SettingsSection::PluginManage, "Update…");
        std::fs::write(
            source.join(crate::plugin::runtime::MANIFEST_NAME),
            fixture_manifest("drift-tool", "2.0.1", false),
        )
        .unwrap();
        app.answer_dialog(UPDATE_DISABLED);
        assert!(app.dialog.answers(app.dialog.request_id(), UPDATE_DISABLED));
        assert!(shows_error(&app));
        assert_eq!(
            std::fs::read(&installed_manifest).unwrap(),
            installed_before
        );
        assert_eq!(std::fs::read(&fixture.registry).unwrap(), registry_before);
    }

    #[test]
    fn every_plugin_is_a_row_and_like_names_show_their_ids() {
        let fixture = Fixture::new();
        for index in 0..25 {
            install_fixture(
                fixture.base(),
                &fixture.registry,
                &fixture.root,
                &format!("fixture-{index:02}"),
                false,
            );
        }
        // Two plugins by one name, and one named like a row of the page.
        for (id, name) in [("twin-a", "Twin"), ("twin-b", "Twin"), ("status", "Status")] {
            let source = fixture.base().join(format!("source-{id}"));
            std::fs::create_dir_all(&source).unwrap();
            std::fs::write(
                source.join(crate::plugin::runtime::MANIFEST_NAME),
                fixture_manifest(id, "1.0.0", false).replace(
                    &format!("name = \"Fixture {id}\""),
                    &format!("name = \"{name}\""),
                ),
            )
            .unwrap();
            crate::plugin::runtime::execute_admin_at(
                PluginAdminCommand::Install {
                    directory: source,
                    grants: PluginPermissions::default(),
                    enable: false,
                    replace: false,
                },
                &fixture.registry,
                &fixture.root,
            )
            .unwrap();
        }
        let mut app = fixture.app();
        let labels: Vec<String> = rows(&app)
            .into_iter()
            .filter(|entry| matches!(entry.kind, SettingKind::SwitchedAction { .. }))
            .map(|entry| entry.label.into_owned())
            .collect();
        assert_eq!(labels.len(), 28);
        for label in [
            "Fixture fixture-00",
            "Twin (twin-a)",
            "Twin (twin-b)",
            "Status (status)",
        ] {
            assert!(labels.iter().any(|shown| shown == label), "{label}");
        }
        activate(&mut app, SettingsSection::Plugins, "Fixture fixture-24");
        assert_eq!(
            app.settings_panel.sub_page(),
            Some(&SettingsSubPage::Plugin("fixture-24".into()))
        );
        assert_eq!(
            app.settings_panel
                .build_info(&app.config, &app.shortcut_registry)
                .sub_page_title
                .as_deref(),
            Some("Fixture fixture-24")
        );
    }

    #[test]
    fn every_setting_type_is_an_ordinary_row_and_its_default_can_return() {
        let fixture = Fixture::new();
        install_typed_fixture(fixture.base(), &fixture.registry, &fixture.root);
        let mut app = fixture.app();
        open_plugin(&mut app, "typed-tool");
        let section = SettingsSection::PluginSettings;
        assert_eq!(row(&app, "Verbose").unwrap().value, "Default (On)");
        assert_eq!(row(&app, "Limit").unwrap().value, "Default (5)");

        choose(&mut app, section, "Verbose", "Off");
        activate(&mut app, section, "Limit");
        // Unchanged, there is no default to return to.
        assert!(!app.dialog.answers(app.dialog.request_id(), UNSET));
        assert_eq!(app.dialog.field_value(app.dialog.request_id()), Some("5"));
        app.submit_dialog_field("7");
        activate(&mut app, section, "Label");
        app.submit_dialog_field("warm 🫚");
        choose(&mut app, section, "Mode", "fast");

        let status = fixture.statuses().remove(0);
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
        assert_eq!(row(&app, "Limit").unwrap().value, "7");

        choose(&mut app, section, "Mode", "Default (safe)");
        activate(&mut app, section, "Limit");
        app.answer_dialog(UNSET);
        let status = fixture.statuses().remove(0);
        assert_eq!(status.configured_setting_count, 2);
        assert!(!status.configured_settings.contains_key("mode"));
        assert!(!status.configured_settings.contains_key("limit"));
    }

    #[test]
    fn a_refused_or_stale_setting_leaves_the_registry_unchanged() {
        let fixture = Fixture::new();
        let source = install_typed_fixture(fixture.base(), &fixture.registry, &fixture.root);
        let mut app = fixture.app();
        open_plugin(&mut app, "typed-tool");
        let section = SettingsSection::PluginSettings;
        activate(&mut app, section, "Limit");
        let cancelled = app.dialog.request_id();
        assert!(app.dialog.replace_field_text(cancelled, "9"));
        app.answer_dialog(CANCEL);
        assert_eq!(fixture.statuses()[0].configured_setting_count, 0);

        // Past its maximum: the prompt keeps the text and says why.
        activate(&mut app, section, "Limit");
        app.submit_dialog_field("999");
        let field = app.dialog.field(app.dialog.request_id()).unwrap();
        assert_eq!(field.value, "999");
        assert!(field.error.is_some());
        assert_eq!(fixture.statuses()[0].configured_setting_count, 0);

        // The schema changes while the prompt is open.
        let stale = app.dialog.request_id();
        assert!(app.dialog.replace_field_text(stale, "8"));
        std::fs::write(
            source.join(crate::plugin::runtime::MANIFEST_NAME),
            typed_fixture_manifest(20),
        )
        .unwrap();
        crate::plugin::runtime::execute_admin_at(
            PluginAdminCommand::Update {
                id: "typed-tool".into(),
            },
            &fixture.registry,
            &fixture.root,
        )
        .unwrap();
        app.apply_dialog_action(DialogAction::Answer {
            request_id: stale,
            id: SAVE.into(),
        });
        assert_eq!(fixture.statuses()[0].configured_setting_count, 0);
        assert!(!app.dialog.is_awaiting_current());
        assert!(
            app.toasts
                .iter()
                .any(|toast| toast.message.contains("Setting limit"))
        );
    }

    #[test]
    fn every_setting_and_choice_value_is_offered() {
        let fixture = Fixture::new();
        let settings = (0..32)
            .map(|index| {
                format!(
                    "[[settings]]\ntype = \"boolean\"\nid = \"flag-{index}\"\nname = \"Flag {index}\"\ndefault = false\n"
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        install_generated_fixture(
            fixture.base(),
            &fixture.registry,
            &fixture.root,
            "many-settings",
            &settings,
        );
        let choice_values = (0..32)
            .map(|index| format!("\"value-{index}\""))
            .collect::<Vec<_>>()
            .join(", ");
        install_generated_fixture(
            fixture.base(),
            &fixture.registry,
            &fixture.root,
            "many-choices",
            &format!(
                "[[settings]]\ntype = \"choice\"\nid = \"mode\"\nname = \"Mode\"\ndefault = \"value-0\"\nchoices = [{choice_values}]\n"
            ),
        );
        let mut app = fixture.app();
        open_plugin(&mut app, "many-settings");
        assert_eq!(
            rows(&app)
                .iter()
                .filter(|entry| entry.section == SettingsSection::PluginSettings
                    && !entry.is_header)
                .count(),
            32
        );
        app.settings_panel.leave_sub_page();
        open_plugin(&mut app, "many-choices");
        let SettingKind::Dropdown { options, .. } = row(&app, "Mode").unwrap().kind else {
            panic!("a choice is a dropdown");
        };
        assert_eq!(options.len(), 33, "the default and every choice");
        choose(
            &mut app,
            SettingsSection::PluginSettings,
            "Mode",
            "value-31",
        );
        let status = fixture
            .statuses()
            .into_iter()
            .find(|status| status.id == "many-choices")
            .unwrap();
        assert_eq!(
            status.settings["mode"],
            PluginSettingValue::Text("value-31".into())
        );
    }
}
