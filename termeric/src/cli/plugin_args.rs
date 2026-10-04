//! `termeric plugin`: declarative plugin management.

use crate::plugin::permission::{PluginPermission, PluginPermissions};
use crate::plugin::runtime::{PluginAdminCommand, PluginConfigurationChange};
use clap::{ArgGroup, Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "termeric plugin",
    about = "Termeric declarative plugin management",
    after_help = "Manifests declare api_version = 1 and request permissions. Each --allow grants \
one requested permission; window-acting permissions cover the plugin's own window unless \
requested and granted with an :all-windows suffix, for example --allow pane-create:all-windows. \
Enabling requires every requested permission.\n\nPermissions: process-spawn, event-subscribe, \
ui-actions, command-output, window-focus, and (scoped) workspace-query, navigate, terminal-read, \
terminal-input, tab-create, tab-close, tab-move, pane-create, pane-close, pane-move, \
pane-layout, window-close. Installation records the canonical local source and a manifest \
fingerprint; it never downloads code.",
    arg_required_else_help = true,
    disable_version_flag = true
)]
pub(super) struct PluginCli {
    #[command(subcommand)]
    command: PluginArgs,
}

#[derive(Subcommand)]
enum PluginArgs {
    /// List installed plugins
    List {
        /// Print the result as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show one installed plugin
    Show {
        id: String,
        /// Print the result as JSON
        #[arg(long)]
        json: bool,
    },
    /// Check a plugin directory's manifest without installing it
    Validate { directory: PathBuf },
    /// Install a plugin from a local directory
    Install {
        directory: PathBuf,
        /// Grant one requested permission; repeat for more
        #[arg(long = "allow", value_name = "PERMISSION", value_parser = permission)]
        grants: Vec<PluginPermission>,
        /// Enable it once installed
        #[arg(long)]
        enable: bool,
        /// Replace an installed plugin of the same ID
        #[arg(long)]
        replace: bool,
    },
    /// Reinstall a plugin from its recorded source
    Update { id: String },
    /// Enable an installed plugin
    Enable { id: String },
    /// Disable an installed plugin
    Disable { id: String },
    /// Change one setting of an installed plugin
    Configure {
        id: String,
        #[command(flatten)]
        change: Change,
    },
    /// Remove an installed plugin
    Remove { id: String },
}

#[derive(Args)]
#[command(group(ArgGroup::new("change").required(true).multiple(false)))]
struct Change {
    /// Use this profile when the plugin opens a tab
    #[arg(
        long,
        value_name = "PROFILE",
        group = "change",
        allow_hyphen_values = true
    )]
    default_profile: Option<String>,
    /// Forget the default profile
    #[arg(long, group = "change")]
    clear_default_profile: bool,
    /// Set one of the plugin's settings
    #[arg(
        long,
        num_args = 2,
        value_names = ["KEY", "VALUE"],
        group = "change",
        allow_hyphen_values = true
    )]
    set: Option<Vec<String>>,
    /// Return one setting to its default
    #[arg(long, value_name = "KEY", group = "change")]
    unset: Option<String>,
    /// Return every setting to its default
    #[arg(long, group = "change")]
    reset_settings: bool,
}

impl PluginCli {
    pub(super) fn into_command(self) -> anyhow::Result<PluginAdminCommand> {
        Ok(match self.command {
            PluginArgs::List { json } => PluginAdminCommand::List { json },
            PluginArgs::Show { id, json } => PluginAdminCommand::Show { id, json },
            PluginArgs::Validate { directory } => PluginAdminCommand::Validate { directory },
            PluginArgs::Install {
                directory,
                grants,
                enable,
                replace,
            } => PluginAdminCommand::Install {
                directory,
                grants: PluginPermissions::from_permissions(grants)?,
                enable,
                replace,
            },
            PluginArgs::Update { id } => PluginAdminCommand::Update { id },
            PluginArgs::Enable { id } => PluginAdminCommand::Enable { id },
            PluginArgs::Disable { id } => PluginAdminCommand::Disable { id },
            PluginArgs::Remove { id } => PluginAdminCommand::Remove { id },
            PluginArgs::Configure { id, change } => PluginAdminCommand::Configure {
                id,
                change: change.into_change(),
            },
        })
    }
}

impl Change {
    fn into_change(self) -> PluginConfigurationChange {
        if let Some(profile) = self.default_profile {
            PluginConfigurationChange::DefaultProfile(profile)
        } else if self.clear_default_profile {
            PluginConfigurationChange::ClearDefaultProfile
        } else if let Some(mut pair) = self.set {
            let value = pair.pop().expect("clap takes KEY VALUE");
            let key = pair.pop().expect("clap takes KEY VALUE");
            PluginConfigurationChange::Set { key, value }
        } else if let Some(key) = self.unset {
            PluginConfigurationChange::Unset { key }
        } else {
            // The required group leaves only this one.
            PluginConfigurationChange::ResetSettings
        }
    }
}

fn permission(value: &str) -> anyhow::Result<PluginPermission> {
    value.parse()
}
