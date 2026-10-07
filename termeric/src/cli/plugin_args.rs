//! `termeric plugin`: declarative plugin management.

use crate::plugin::permission::{PluginPermission, PluginPermissions};
use crate::plugin::runtime::{PluginAdminCommand, PluginConfigurationChange};
use clap::{ArgGroup, Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "termeric plugin",
    about = "Install and manage plugins (experimental)",
    after_help = "A plugin is a local directory with a termeric-plugin.toml manifest, which \
lists the permissions it needs. Each --allow grants one of them, and a plugin can be enabled \
only once all of them are granted. A permission that acts on windows covers only the window \
an event came from, unless the manifest requests it with an :all-windows suffix and you grant \
it that way, for example --allow pane-create:all-windows.\n\nPermissions: process-spawn, \
event-subscribe, ui-actions, command-output, window-focus, and, with an optional :all-windows \
suffix, workspace-query, navigate, terminal-read, terminal-input, tab-create, tab-close, \
tab-move, pane-create, pane-close, pane-move, pane-layout, window-close.\n\nInstalling copies \
the manifest and records its directory and fingerprint; it never downloads code. These commands \
edit the files directly, and a running Termeric reloads its plugins when they change. Settings, \
Plugins, offers the same management. examples/plugins in the source repository has example \
plugins and the manifest and protocol reference.",
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
    /// Show one installed plugin's source, permissions, settings, and any error
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
    /// Reinstall a plugin from its directory, keeping the grants it still requests
    Update { id: String },
    /// Enable an installed plugin
    Enable {
        id: String,
        /// Grant one requested permission first; repeat for more
        #[arg(long = "allow", value_name = "PERMISSION", value_parser = permission)]
        grants: Vec<PluginPermission>,
    },
    /// Disable an installed plugin without removing it
    Disable { id: String },
    /// Change one setting of an installed plugin
    Configure {
        id: String,
        #[command(flatten)]
        change: Change,
    },
    /// Grant permissions an installed plugin requests
    Grant {
        id: String,
        /// Grant one requested permission; repeat for more
        #[arg(
            long = "allow",
            value_name = "PERMISSION",
            value_parser = permission,
            required = true
        )]
        grants: Vec<PluginPermission>,
    },
    /// Withdraw grants from an installed plugin, disabling it if it was enabled
    Revoke {
        id: String,
        /// Withdraw the grant of one permission; repeat for more
        #[arg(
            long = "allow",
            value_name = "PERMISSION",
            value_parser = permission,
            required = true
        )]
        permissions: Vec<PluginPermission>,
    },
    /// Remove an installed plugin, leaving its source directory alone
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
            PluginArgs::Enable { id, grants } => PluginAdminCommand::Enable {
                id,
                grants: PluginPermissions::from_permissions(grants)?,
            },
            PluginArgs::Grant { id, grants } => PluginAdminCommand::Grant {
                id,
                grants: PluginPermissions::from_permissions(grants)?,
            },
            PluginArgs::Revoke { id, permissions } => PluginAdminCommand::Revoke {
                id,
                permissions: PluginPermissions::from_permissions(permissions)?,
            },
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
