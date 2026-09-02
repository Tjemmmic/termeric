//! Request-scoped in-app management for installed declarative plugins.

use super::*;
use crate::plugin::runtime::{
    PluginAdminCommand, PluginConfigurationChange, PluginSettingValue, RuntimePluginStatus,
};
use crate::ui::info_overlay::InfoOverlayChoice;

const PLUGIN_MANAGER_PAGE_SIZE: usize = 24;
const PLUGIN_LIST_SHORTCUTS: [char; PLUGIN_MANAGER_PAGE_SIZE] = [
    '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l',
    'm', 'o', 'q', 'r', 's',
];

impl App {
    pub(super) fn discover_runtime_plugins_for_app(
        &self,
    ) -> Result<(
        Vec<crate::plugin::runtime::DeclarativePlugin>,
        Vec<RuntimePluginStatus>,
    )> {
        #[cfg(test)]
        if let Some((registry, root)) = &self.runtime_plugin_test_paths {
            return crate::plugin::runtime::discover_runtime_plugins_at(registry, root);
        }
        crate::plugin::runtime::discover_runtime_plugins()
    }

    fn execute_runtime_plugin_admin(&self, command: PluginAdminCommand) -> Result<String> {
        #[cfg(test)]
        if let Some((registry, root)) = &self.runtime_plugin_test_paths {
            return crate::plugin::runtime::execute_admin_at(command, registry, root);
        }
        crate::plugin::runtime::execute_admin(command)
    }

    fn runtime_plugin_registry_path_for_app(&self) -> PathBuf {
        #[cfg(test)]
        if let Some((registry, _)) = &self.runtime_plugin_test_paths {
            return registry.clone();
        }
        crate::plugin::runtime::registry_path()
    }

    #[cfg(test)]
    pub(crate) fn set_runtime_plugin_test_paths(&mut self, registry: PathBuf, root: PathBuf) {
        self.runtime_plugin_test_paths = Some((registry, root));
        self.sync_plugins_from_config();
    }

    pub(super) fn open_runtime_plugin_manager(&mut self) {
        self.settings_panel.close();
        self.pending_profile_selection = None;
        self.pending_file_upload = None;
        self.show_runtime_plugin_list(0);
    }

    fn show_runtime_plugin_list(&mut self, requested_page: usize) {
        let statuses = match self.discover_runtime_plugins_for_app() {
            Ok((_, statuses)) => statuses,
            Err(error) => {
                self.show_runtime_plugin_error(
                    RuntimePluginManagerView::List { page: 0 },
                    "Plugin registry rejected",
                    &format!("{error:#}"),
                );
                return;
            }
        };
        let page_count = statuses.len().div_ceil(PLUGIN_MANAGER_PAGE_SIZE).max(1);
        let page = requested_page.min(page_count - 1);
        let start = page * PLUGIN_MANAGER_PAGE_SIZE;
        let end = (start + PLUGIN_MANAGER_PAGE_SIZE).min(statuses.len());
        let mut lines = vec![
            format!(
                "Installed third-party plugins: {} · page {}/{}",
                statuses.len(),
                page + 1,
                page_count
            ),
            "Select a plugin to inspect its source, permissions, status, and actions.".into(),
            "Installation and typed value editing remain available through `termeric plugin`."
                .into(),
            String::new(),
        ];
        let mut choices = Vec::new();
        for (index, status) in statuses[start..end].iter().enumerate() {
            let source_line = lines.len();
            let health = if status.diagnostic.is_some() {
                "error"
            } else if status.enabled {
                "enabled"
            } else {
                "disabled"
            };
            let label = format!(
                "{} ({}) v{} · {health}",
                status.name, status.id, status.version
            );
            let shortcut = PLUGIN_LIST_SHORTCUTS[index];
            lines.push(format!("[{}] {label}", shortcut.to_ascii_uppercase()));
            choices.push(InfoOverlayChoice::new(
                format!("plugin:{}", status.id),
                label,
                source_line,
                Some(shortcut),
            ));
        }
        if statuses.is_empty() {
            lines.push("No third-party plugins are installed.".into());
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
        let request_id =
            self.info_overlay
                .show_choices("Third-party Plugin Manager", lines, choices);
        self.pending_runtime_plugin_management = Some(PendingRuntimePluginManagement {
            request_id,
            view: RuntimePluginManagerView::List { page },
        });
    }

    fn show_runtime_plugin_detail(&mut self, plugin_id: String, page: usize) {
        let status = match self.discover_runtime_plugins_for_app() {
            Ok((_, statuses)) => statuses.into_iter().find(|status| status.id == plugin_id),
            Err(error) => {
                self.show_runtime_plugin_error(
                    RuntimePluginManagerView::List { page },
                    "Plugin registry rejected",
                    &format!("{error:#}"),
                );
                return;
            }
        };
        let Some(status) = status else {
            self.toasts.warning(format!(
                "Plugin {plugin_id} is no longer installed; refreshed the list"
            ));
            self.show_runtime_plugin_list(page);
            return;
        };

        let requested = display_permissions(&status.requested);
        let granted = display_permissions(&status.granted);
        let mut lines = vec![
            format!("{} ({})", status.name, status.id),
            format!("Version: {}", status.version),
            format!(
                "Status: {}",
                if status.enabled {
                    "enabled"
                } else {
                    "disabled"
                }
            ),
        ];
        push_wrapped_field(&mut lines, "Source", &status.source.display().to_string());
        lines.extend([
            format!("Requested permissions: {requested}"),
            format!("Granted permissions: {granted}"),
            format!(
                "Settings: {} effective · {} explicitly configured",
                status.settings.len(),
                status.configured_setting_count
            ),
        ]);
        for (key, value) in &status.settings {
            lines.push(format!("  {key} = {}", display_setting_value(value)));
        }
        if let Some(diagnostic) = &status.diagnostic {
            lines.push(format!("Diagnostic: {diagnostic}"));
        }
        lines.push(String::new());

        let mut choices = Vec::new();
        let toggle_label = if status.enabled { "Disable" } else { "Enable" };
        let source_line = lines.len();
        lines.push(format!("[E] {toggle_label}"));
        choices.push(InfoOverlayChoice::new(
            "toggle",
            toggle_label,
            source_line,
            Some('e'),
        ));
        let source_line = lines.len();
        lines.push("[U] Update from source".into());
        choices.push(InfoOverlayChoice::new(
            "update",
            "Update from source",
            source_line,
            Some('u'),
        ));
        if status.configured_setting_count > 0 {
            let source_line = lines.len();
            lines.push("[R] Reset settings to defaults".into());
            choices.push(InfoOverlayChoice::new(
                "reset-settings",
                "Reset settings to defaults",
                source_line,
                Some('r'),
            ));
        }
        let source_line = lines.len();
        lines.push("[X] Remove installed copy".into());
        choices.push(InfoOverlayChoice::new(
            "remove",
            "Remove installed copy",
            source_line,
            Some('x'),
        ));
        let source_line = lines.len();
        lines.push("[B] Back to plugin list".into());
        choices.push(InfoOverlayChoice::new(
            "back",
            "Back to plugin list",
            source_line,
            Some('b'),
        ));

        let request_id =
            self.info_overlay
                .show_choices(format!("Plugin: {}", status.name), lines, choices);
        self.pending_runtime_plugin_management = Some(PendingRuntimePluginManagement {
            request_id,
            view: RuntimePluginManagerView::Detail { plugin_id, page },
        });
    }

    fn show_runtime_plugin_confirmation(
        &mut self,
        plugin_id: String,
        page: usize,
        operation: RuntimePluginOperation,
    ) {
        let (title, explanation) = match operation {
            RuntimePluginOperation::Update => (
                "Confirm Plugin Update",
                "Re-read the recorded source and replace the managed copy. Updates that request new permissions or invalidate stored settings are rejected.",
            ),
            RuntimePluginOperation::ResetSettings => (
                "Confirm Settings Reset",
                "Clear every explicit override for this plugin and restore its manifest defaults.",
            ),
            RuntimePluginOperation::Remove => (
                "Confirm Plugin Removal",
                "Remove the managed installed copy and registry entry. The original source directory is not deleted.",
            ),
        };
        let lines = vec![
            format!("Plugin: {plugin_id}"),
            explanation.into(),
            String::new(),
            "[Y] Confirm".into(),
            "[N] Cancel".into(),
        ];
        let choices = vec![
            InfoOverlayChoice::new("confirm", "Confirm", 3, Some('y')),
            InfoOverlayChoice::new("cancel", "Cancel", 4, Some('n')),
        ];
        let request_id = self.info_overlay.show_choices(title, lines, choices);
        self.pending_runtime_plugin_management = Some(PendingRuntimePluginManagement {
            request_id,
            view: RuntimePluginManagerView::Confirm {
                plugin_id,
                page,
                operation,
            },
        });
    }

    fn show_runtime_plugin_error(
        &mut self,
        return_view: RuntimePluginManagerView,
        title: &str,
        error: &str,
    ) {
        let (plugin_id, page) = match &return_view {
            RuntimePluginManagerView::List { page } => (None, *page),
            RuntimePluginManagerView::Detail { plugin_id, page }
            | RuntimePluginManagerView::Confirm {
                plugin_id, page, ..
            } => (Some(plugin_id.clone()), *page),
        };
        let lines = vec![error.into(), String::new(), "[B] Back".into()];
        let request_id = self.info_overlay.show_choices(
            title,
            lines,
            vec![InfoOverlayChoice::new("back", "Back", 2, Some('b'))],
        );
        self.pending_runtime_plugin_management = Some(PendingRuntimePluginManagement {
            request_id,
            view: plugin_id.map_or(RuntimePluginManagerView::List { page }, |plugin_id| {
                RuntimePluginManagerView::Detail { plugin_id, page }
            }),
        });
    }

    pub(super) fn handle_runtime_plugin_manager_choice(
        &mut self,
        request_id: u64,
        choice_id: &str,
    ) -> bool {
        let Some(pending) = self
            .pending_runtime_plugin_management
            .as_ref()
            .filter(|pending| pending.request_id == request_id)
            .cloned()
        else {
            return false;
        };
        self.info_overlay.dismiss(request_id);
        self.pending_runtime_plugin_management = None;
        match pending.view {
            RuntimePluginManagerView::List { page } => match choice_id {
                "previous-page" => self.show_runtime_plugin_list(page.saturating_sub(1)),
                "next-page" => self.show_runtime_plugin_list(page.saturating_add(1)),
                "back" => self.show_runtime_plugin_list(page),
                choice if choice.starts_with("plugin:") => {
                    self.show_runtime_plugin_detail(choice[7..].to_string(), page);
                }
                _ => {}
            },
            RuntimePluginManagerView::Detail { plugin_id, page } => match choice_id {
                "toggle" => {
                    let enabled = self
                        .discover_runtime_plugins_for_app()
                        .ok()
                        .and_then(|(_, statuses)| {
                            statuses.into_iter().find(|status| status.id == plugin_id)
                        })
                        .is_some_and(|status| status.enabled);
                    let command = if enabled {
                        PluginAdminCommand::Disable {
                            id: plugin_id.clone(),
                        }
                    } else {
                        PluginAdminCommand::Enable {
                            id: plugin_id.clone(),
                        }
                    };
                    self.apply_runtime_plugin_operation(command, plugin_id, page, false);
                }
                "update" => self.show_runtime_plugin_confirmation(
                    plugin_id,
                    page,
                    RuntimePluginOperation::Update,
                ),
                "reset-settings" => self.show_runtime_plugin_confirmation(
                    plugin_id,
                    page,
                    RuntimePluginOperation::ResetSettings,
                ),
                "remove" => self.show_runtime_plugin_confirmation(
                    plugin_id,
                    page,
                    RuntimePluginOperation::Remove,
                ),
                "back" => self.show_runtime_plugin_list(page),
                _ => {}
            },
            RuntimePluginManagerView::Confirm {
                plugin_id,
                page,
                operation,
            } => match choice_id {
                "confirm" => {
                    let command = match operation {
                        RuntimePluginOperation::Update => PluginAdminCommand::Update {
                            id: plugin_id.clone(),
                        },
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
                        page,
                        operation == RuntimePluginOperation::Remove,
                    );
                }
                "cancel" | "back" => self.show_runtime_plugin_detail(plugin_id, page),
                _ => {}
            },
        }
        true
    }

    fn apply_runtime_plugin_operation(
        &mut self,
        command: PluginAdminCommand,
        plugin_id: String,
        page: usize,
        removed: bool,
    ) {
        match self.execute_runtime_plugin_admin(command) {
            Ok(message) => {
                self.last_plugin_registry_mtime =
                    std::fs::metadata(self.runtime_plugin_registry_path_for_app())
                        .and_then(|metadata| metadata.modified())
                        .ok();
                self.sync_plugins_from_config();
                self.reload_dynamic_keybindings();
                self.toasts.success(message);
                if removed {
                    self.show_runtime_plugin_list(page);
                } else {
                    self.show_runtime_plugin_detail(plugin_id, page);
                }
            }
            Err(error) => {
                let error = format!("{error:#}");
                self.toasts
                    .error(format!("Plugin operation failed: {error}"));
                self.show_runtime_plugin_error(
                    RuntimePluginManagerView::Detail { plugin_id, page },
                    "Plugin Operation Failed",
                    &error,
                );
            }
        }
    }
}

fn display_permissions(
    permissions: &std::collections::BTreeSet<crate::plugin::runtime::PluginPermission>,
) -> String {
    if permissions.is_empty() {
        "none".into()
    } else {
        permissions
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn display_setting_value(value: &PluginSettingValue) -> String {
    match value {
        PluginSettingValue::Boolean(value) => value.to_string(),
        PluginSettingValue::Integer(value) => value.to_string(),
        PluginSettingValue::Text(value) => format!("{value:?}"),
    }
}

fn push_wrapped_field(lines: &mut Vec<String>, label: &str, value: &str) {
    const CHUNK_CHARS: usize = crate::ui::info_overlay::MAX_INFO_LINE_CHARS - 32;
    let mut characters = value.chars();
    let mut first = true;
    loop {
        let chunk = characters.by_ref().take(CHUNK_CHARS).collect::<String>();
        if chunk.is_empty() {
            if first {
                lines.push(format!("{label}:"));
            }
            break;
        }
        if first {
            lines.push(format!("{label}: {chunk}"));
            first = false;
        } else {
            lines.push(format!("  {chunk}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::runtime::{PluginAdminCommand, PluginConfigurationChange};
    use crate::ui::info_overlay::InfoOverlayAction;
    use std::collections::BTreeSet;

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
            r#"api_version = 10
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
                grants: BTreeSet::from([crate::plugin::runtime::PluginPermission::ProcessSpawn]),
                enable: false,
                replace: false,
            },
            registry,
            root,
        )
        .unwrap();
        source
    }

    fn choose(app: &mut App, choice_id: &str) {
        let request_id = app.info_overlay.request_id();
        assert!(app.info_overlay.choice(request_id, choice_id).is_some());
        app.apply_info_overlay_action(InfoOverlayAction::Choose {
            request_id,
            choice_id: choice_id.into(),
        });
    }

    #[test]
    fn wrapped_detail_fields_preserve_bounded_unicode_text_exactly() {
        let value = "🫚root/".repeat(1_500);
        let mut lines = Vec::new();
        push_wrapped_field(&mut lines, "Source", &value);
        assert!(lines.len() > 1);
        assert!(
            lines.iter().all(|line| {
                line.chars().count() <= crate::ui::info_overlay::MAX_INFO_LINE_CHARS
            })
        );
        let reconstructed = lines
            .iter()
            .enumerate()
            .map(|(index, line)| {
                if index == 0 {
                    line.strip_prefix("Source: ").unwrap()
                } else {
                    line.strip_prefix("  ").unwrap()
                }
            })
            .collect::<String>();
        assert_eq!(reconstructed, value);
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
        let dismissed_request = app.info_overlay.request_id();
        app.apply_info_overlay_action(InfoOverlayAction::Dismiss {
            request_id: dismissed_request,
        });
        assert!(app.pending_runtime_plugin_management.is_none());

        app.open_runtime_plugin_manager();
        let list_request = app.info_overlay.request_id();
        app.apply_info_overlay_action(InfoOverlayAction::Choose {
            request_id: list_request + 1,
            choice_id: "plugin:fixture-tool".into(),
        });
        assert_eq!(app.info_overlay.request_id(), list_request);
        assert!(matches!(
            app.pending_runtime_plugin_management
                .as_ref()
                .map(|pending| &pending.view),
            Some(RuntimePluginManagerView::List { page: 0 })
        ));

        choose(&mut app, "plugin:fixture-tool");
        assert!(
            app.info_overlay
                .lines()
                .iter()
                .any(|line| line.contains(&format!("Source: {}", source.display())))
        );
        assert!(
            app.info_overlay
                .lines()
                .iter()
                .any(|line| line.contains("verbose = false"))
        );
        assert!(
            app.info_overlay
                .choice(app.info_overlay.request_id(), "reset-settings")
                .is_some()
        );

        std::fs::write(
            source.join(crate::plugin::runtime::MANIFEST_NAME),
            fixture_manifest("fixture-tool", "2.0.0", true),
        )
        .unwrap();
        choose(&mut app, "update");
        assert_eq!(app.info_overlay.title(), "Confirm Plugin Update");
        choose(&mut app, "confirm");
        let (_, statuses) =
            crate::plugin::runtime::discover_runtime_plugins_at(&registry, &root).unwrap();
        assert_eq!(statuses[0].version, "2.0.0");
        assert_eq!(statuses[0].configured_setting_count, 1);

        choose(&mut app, "reset-settings");
        assert_eq!(app.info_overlay.title(), "Confirm Settings Reset");
        choose(&mut app, "confirm");
        let (_, statuses) =
            crate::plugin::runtime::discover_runtime_plugins_at(&registry, &root).unwrap();
        assert_eq!(statuses[0].configured_setting_count, 0);
        assert!(
            app.info_overlay
                .choice(app.info_overlay.request_id(), "reset-settings")
                .is_none()
        );

        choose(&mut app, "toggle");
        let (_, statuses) =
            crate::plugin::runtime::discover_runtime_plugins_at(&registry, &root).unwrap();
        assert!(statuses[0].enabled);

        choose(&mut app, "remove");
        assert_eq!(app.info_overlay.title(), "Confirm Plugin Removal");
        choose(&mut app, "cancel");
        assert!(app.info_overlay.title().starts_with("Plugin:"));
        choose(&mut app, "remove");
        choose(&mut app, "confirm");
        let (_, statuses) =
            crate::plugin::runtime::discover_runtime_plugins_at(&registry, &root).unwrap();
        assert!(statuses.is_empty());
        assert!(source.is_dir(), "the original source must never be removed");
    }

    #[test]
    fn manager_paginates_within_the_bounded_overlay_choice_limit() {
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
        assert_eq!(app.info_overlay.choices().len(), 25);
        let shortcuts = app
            .info_overlay
            .choices()
            .iter()
            .filter_map(|choice| choice.shortcut)
            .collect::<BTreeSet<_>>();
        assert_eq!(shortcuts.len(), 25);
        assert!(!shortcuts.contains(&'c'), "C remains reserved for copy");
        assert_eq!(
            app.info_overlay
                .choice_for_shortcut(app.info_overlay.request_id(), '1')
                .map(|choice| choice.id.as_str()),
            Some("plugin:fixture-00")
        );
        assert!(
            app.info_overlay
                .choices()
                .iter()
                .any(|choice| choice.id == "next-page")
        );
        choose(&mut app, "next-page");
        assert_eq!(app.info_overlay.choices().len(), 2);
        assert!(
            app.info_overlay
                .choices()
                .iter()
                .any(|choice| choice.id == "previous-page")
        );
    }
}
