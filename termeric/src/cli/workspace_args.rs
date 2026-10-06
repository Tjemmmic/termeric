//! `termeric workspace`: saved workspaces, read without a running GUI.

use crate::workspace::WorkspaceCommand;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "termeric workspace",
    about = "List saved workspaces",
    after_help = "Open a listed workspace with termeric --workspace ID. The list comes from the \
saved files, so it works whether or not Termeric is running.",
    arg_required_else_help = true,
    disable_version_flag = true
)]
pub(super) struct WorkspaceCli {
    #[command(subcommand)]
    command: WorkspaceArgs,
}

#[derive(Subcommand)]
enum WorkspaceArgs {
    /// List saved workspaces, most recently saved first
    List {
        /// Print the result as JSON
        #[arg(long)]
        json: bool,
    },
}

impl WorkspaceCli {
    pub(super) fn into_command(self) -> WorkspaceCommand {
        match self.command {
            WorkspaceArgs::List { json } => WorkspaceCommand::List { json },
        }
    }
}
