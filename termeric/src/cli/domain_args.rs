//! `termeric domain`: persistent terminal domains (experimental).

use super::{Direction, LaunchPlan, ParseOutcome, WindowLaunch};
use crate::domain::model::{self, SpawnSpec};
use crate::domain::protocol::{self, Request};
use crate::domain::{DomainAuthority, DomainCommand, DomainTarget};
use crate::panes::PaneLayoutPreset;
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "termeric domain",
    about = "Persistent terminal domains (experimental)",
    after_help = "The daemon socket always remains local and user-private. `--host` starts a \
non-interactive OpenSSH stdio bridge using an already trusted host key and key/agent \
authentication. `--host-profile NAME` resolves a validated entry in `[domains.ssh_hosts]`; \
OpenSSH config remains authoritative for ports, keys, proxies, certificates, and host policy. \
Remote attachment uses the resolved authority for topology, controls, focus, and every pane \
relay.",
    arg_required_else_help = true,
    disable_version_flag = true
)]
pub(super) struct DomainCli {
    /// Control the domains on another computer, through SSH
    #[arg(long, value_name = "SSH_DESTINATION", value_parser = ssh_destination)]
    host: Option<String>,
    /// Like --host, with a destination from [domains.ssh_hosts]
    #[arg(long, value_name = "NAME", conflicts_with = "host")]
    host_profile: Option<String>,
    #[command(subcommand)]
    command: DomainArgs,
}

#[derive(Subcommand)]
enum DomainArgs {
    /// Print the domain protocol version this termeric speaks
    ProtocolVersion {
        #[command(flatten)]
        output: Output,
    },
    /// List configured SSH host profiles
    Hosts {
        #[command(flatten)]
        output: Output,
    },
    /// List live domains
    List {
        #[command(flatten)]
        output: Output,
    },
    /// Describe a domain's complete live state
    Show {
        /// Domain ID, as listed
        #[arg(value_parser = domain_id)]
        domain: String,
        #[command(flatten)]
        output: Output,
    },
    /// List a domain's panes
    Panes {
        /// Domain ID, as listed
        #[arg(value_parser = domain_id)]
        domain: String,
        #[command(flatten)]
        output: Output,
    },
    /// Create a persistent domain
    Create {
        /// A name for the domain
        #[arg(allow_hyphen_values = true)]
        name: String,
        #[command(flatten)]
        spawn: SpawnArgs,
    },
    /// Add a tab
    Spawn {
        /// Domain ID, as listed
        #[arg(value_parser = domain_id)]
        domain: String,
        #[command(flatten)]
        spawn: SpawnArgs,
    },
    /// Split a pane
    Split {
        #[command(flatten)]
        pane: PaneTarget,
        direction: Direction,
        #[command(flatten)]
        spawn: SpawnArgs,
    },
    /// Resize a stable divider (100-900 thousandths)
    ResizeSplit {
        /// Domain ID, as listed
        #[arg(value_parser = domain_id)]
        domain: String,
        split: u32,
        #[arg(value_parser = clap::value_parser!(u16).range(100..=900))]
        ratio_millis: u16,
    },
    /// Set a tab's title; an empty title clears it
    RenameTab {
        #[command(flatten)]
        tab: TabTarget,
        #[arg(allow_hyphen_values = true)]
        title: String,
    },
    /// Pin a tab
    PinTab {
        #[command(flatten)]
        tab: TabTarget,
    },
    /// Unpin a tab
    UnpinTab {
        #[command(flatten)]
        tab: TabTarget,
    },
    /// Move a tab within its pin group (counting from 0)
    MoveTab {
        #[command(flatten)]
        tab: TabTarget,
        index: u16,
    },
    /// Swap a pane with the next one in visual order
    SwapPane {
        #[command(flatten)]
        pane: PaneTarget,
    },
    /// Reset every divider in a tab to 50%
    BalanceTab {
        #[command(flatten)]
        tab: TabTarget,
    },
    /// Arrange a tab's panes in a preset layout
    LayoutTab {
        #[command(flatten)]
        tab: TabTarget,
        preset: Preset,
    },
    /// Move a pane to another tab
    MovePane {
        #[command(flatten)]
        pane: PaneTarget,
        tab: u32,
    },
    /// Set the persisted focus
    Focus {
        #[command(flatten)]
        pane: PaneTarget,
    },
    /// End a pane's program and remove the pane
    ClosePane {
        #[command(flatten)]
        pane: PaneTarget,
    },
    /// End every program in a tab and remove it
    CloseTab {
        #[command(flatten)]
        tab: TabTarget,
    },
    /// Type bounded UTF-8 text into a pane
    Send {
        #[command(flatten)]
        pane: PaneTarget,
        #[arg(allow_hyphen_values = true)]
        text: String,
        /// Press Enter after the text
        #[arg(long)]
        enter: bool,
    },
    /// Print a pane's newest text
    GetText {
        #[command(flatten)]
        pane: PaneTarget,
        /// Most bytes to print
        #[arg(
            long = "bytes",
            value_name = "N",
            default_value_t = 64 * 1024,
            value_parser = text_bytes
        )]
        max_bytes: usize,
        #[command(flatten)]
        output: Output,
    },
    /// Copy a file or directory into a pane's working directory, never overwriting
    Upload {
        #[command(flatten)]
        pane: PaneTarget,
        source: PathBuf,
        /// Name it this in the pane's directory
        #[arg(long, value_parser = upload_name, allow_hyphen_values = true)]
        name: Option<String>,
        #[command(flatten)]
        output: Output,
    },
    /// Open the domain in a GUI window
    Attach {
        /// Domain ID, as listed
        #[arg(value_parser = domain_id)]
        domain: String,
        /// Window size in physical pixels
        #[arg(long, value_name = "WIDTHxHEIGHT", value_parser = window_geometry)]
        geometry: Option<(u32, u32)>,
        /// Window application ID
        #[arg(long, alias = "class", value_name = "NAME", value_parser = app_id)]
        app_id: Option<String>,
    },
    /// End every program in a domain
    Kill {
        /// Domain ID, as listed
        #[arg(value_parser = domain_id)]
        domain: String,
    },
    /// Stop the daemon once no domain remains
    StopServer,
    /// Run the daemon in the foreground
    #[command(hide = true)]
    Server {
        #[arg(long, required = true)]
        foreground: bool,
    },
    /// The SSH stdio bridge `--host` runs on the other computer
    #[command(hide = true)]
    ProxyStdio {
        #[arg(long)]
        no_start: bool,
    },
    /// Relay one pane to this terminal (the GUI's pane process)
    #[command(hide = true)]
    Relay {
        #[command(flatten)]
        pane: PaneTarget,
    },
}

#[derive(Args)]
struct Output {
    /// Print the result as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct TabTarget {
    /// Domain ID, as `list` shows it
    #[arg(value_parser = domain_id)]
    domain: String,
    /// Tab ID within the domain
    tab: u32,
}

#[derive(Args)]
struct PaneTarget {
    /// Domain ID, as `list` shows it
    #[arg(value_parser = domain_id)]
    domain: String,
    /// Pane ID within the domain
    pane: u32,
}

/// What `create`, `spawn`, and `split` start.
#[derive(Args)]
struct SpawnArgs {
    /// Start in this directory on the computer running the domain
    #[arg(
        long,
        visible_alias = "cwd",
        value_name = "PATH",
        allow_hyphen_values = true
    )]
    working_directory: Option<String>,
    /// Title for the pane
    #[arg(long, allow_hyphen_values = true)]
    title: Option<String>,
    /// Terminal size in cells
    #[arg(long, value_name = "COLSxROWS", value_parser = cell_geometry)]
    size: Option<(u16, u16)>,
    #[command(flatten)]
    output: Output,
    /// The program and its arguments, after `--`; the shell by default
    #[arg(last = true, value_name = "COMMAND")]
    command: Vec<String>,
}

#[derive(Clone, Copy, ValueEnum)]
enum Preset {
    Columns,
    Rows,
    Grid,
    Tall,
    Fat,
}

impl DomainCli {
    pub(super) fn into_outcome(
        self,
        load_domains: super::LoadDomains,
    ) -> anyhow::Result<ParseOutcome> {
        let destination = match (self.host, self.host_profile) {
            (Some(destination), _) => Some(destination),
            (None, Some(name)) => Some(load_domains()?.ssh_host(&name)?.destination.clone()),
            (None, None) => None,
        };
        let request = DomainCommand::request;
        let command = match self.command {
            DomainArgs::Attach {
                domain,
                geometry,
                app_id,
            } => {
                let authority = match destination {
                    Some(destination) => DomainAuthority::Ssh { destination },
                    None => DomainAuthority::Local,
                };
                return Ok(ParseOutcome::Run(LaunchPlan {
                    windows: vec![WindowLaunch {
                        domain: Some(DomainTarget {
                            authority,
                            domain_id: domain,
                        }),
                        pixel_size: geometry,
                        app_id,
                        ..WindowLaunch::default()
                    }],
                    ..LaunchPlan::default()
                }));
            }
            DomainArgs::ProtocolVersion { output } => {
                local_only(&destination, "protocol-version")?;
                DomainCommand::ProtocolVersion { json: output.json }
            }
            DomainArgs::Hosts { output } => {
                local_only(&destination, "hosts")?;
                DomainCommand::Hosts { json: output.json }
            }
            DomainArgs::Server { .. } => {
                local_only(&destination, "server")?;
                DomainCommand::ServerForeground
            }
            DomainArgs::ProxyStdio { no_start } => {
                local_only(&destination, "proxy-stdio")?;
                DomainCommand::ProxyStdio {
                    start_server: !no_start,
                }
            }
            DomainArgs::List { output } => DomainCommand::Request {
                request: Request::ListDomains,
                json: output.json,
            },
            DomainArgs::Show { domain, output } => DomainCommand::Request {
                request: Request::Describe { domain },
                json: output.json,
            },
            DomainArgs::Panes { domain, output } => DomainCommand::Request {
                request: Request::ListPanes { domain },
                json: output.json,
            },
            DomainArgs::Create { name, spawn } => {
                let (spawn, json) = spawn.into_spec()?;
                DomainCommand::Request {
                    request: Request::CreateDomain { name, spawn },
                    json,
                }
            }
            DomainArgs::Spawn { domain, spawn } => {
                let (spawn, json) = spawn.into_spec()?;
                DomainCommand::Request {
                    request: Request::SpawnTab { domain, spawn },
                    json,
                }
            }
            DomainArgs::Split {
                pane,
                direction,
                spawn,
            } => {
                let (spawn, json) = spawn.into_spec()?;
                DomainCommand::Request {
                    request: Request::SplitPane {
                        domain: pane.domain,
                        pane: pane.pane,
                        direction: direction.into(),
                        spawn,
                    },
                    json,
                }
            }
            DomainArgs::ResizeSplit {
                domain,
                split,
                ratio_millis,
            } => request(Request::ResizeSplit {
                domain,
                split,
                ratio_millis,
            }),
            DomainArgs::RenameTab { tab, title } => {
                let title = (!title.trim().is_empty()).then_some(title);
                if let Some(title) = &title {
                    model::validate_name("tab title", title)?;
                }
                request(Request::RenameTab {
                    domain: tab.domain,
                    tab: tab.tab,
                    title,
                })
            }
            DomainArgs::PinTab { tab } => request(Request::SetTabPinned {
                domain: tab.domain,
                tab: tab.tab,
                pinned: true,
            }),
            DomainArgs::UnpinTab { tab } => request(Request::SetTabPinned {
                domain: tab.domain,
                tab: tab.tab,
                pinned: false,
            }),
            DomainArgs::MoveTab { tab, index } => request(Request::MoveTab {
                domain: tab.domain,
                tab: tab.tab,
                index,
            }),
            DomainArgs::SwapPane { pane } => request(Request::SwapPaneNext {
                domain: pane.domain,
                pane: pane.pane,
            }),
            DomainArgs::BalanceTab { tab } => request(Request::BalanceTab {
                domain: tab.domain,
                tab: tab.tab,
            }),
            DomainArgs::LayoutTab { tab, preset } => request(Request::ApplyPaneLayout {
                domain: tab.domain,
                tab: tab.tab,
                preset: match preset {
                    Preset::Columns => PaneLayoutPreset::Columns,
                    Preset::Rows => PaneLayoutPreset::Rows,
                    Preset::Grid => PaneLayoutPreset::Grid,
                    Preset::Tall => PaneLayoutPreset::Tall,
                    Preset::Fat => PaneLayoutPreset::Fat,
                },
            }),
            // The daemon picks the anchor: the destination's focused pane.
            DomainArgs::MovePane { pane, tab } => request(Request::MovePane {
                domain: pane.domain,
                pane: pane.pane,
                tab,
                target: None,
                direction: crate::panes::SplitDirection::Vertical,
            }),
            DomainArgs::Focus { pane } => request(Request::FocusPane {
                domain: pane.domain,
                pane: pane.pane,
            }),
            DomainArgs::ClosePane { pane } => request(Request::ClosePane {
                domain: pane.domain,
                pane: pane.pane,
            }),
            DomainArgs::CloseTab { tab } => request(Request::CloseTab {
                domain: tab.domain,
                tab: tab.tab,
            }),
            DomainArgs::Send { pane, text, enter } => {
                let mut data = text.into_bytes();
                if enter {
                    data.push(b'\r');
                }
                if data.len() > protocol::MAX_STREAM_CHUNK_BYTES {
                    anyhow::bail!("domain send input exceeds 64 KiB");
                }
                request(Request::SendText {
                    domain: pane.domain,
                    pane: pane.pane,
                    data,
                })
            }
            DomainArgs::GetText {
                pane,
                max_bytes,
                output,
            } => DomainCommand::Request {
                request: Request::GetText {
                    domain: pane.domain,
                    pane: pane.pane,
                    max_bytes,
                },
                json: output.json,
            },
            DomainArgs::Upload {
                pane,
                source,
                name,
                output,
            } => DomainCommand::Upload {
                domain: pane.domain,
                pane: pane.pane,
                source,
                name,
                json: output.json,
            },
            DomainArgs::Relay { pane } => DomainCommand::Relay {
                domain: pane.domain,
                pane: pane.pane,
            },
            DomainArgs::Kill { domain } => request(Request::KillDomain { domain }),
            DomainArgs::StopServer => request(Request::Shutdown),
        };
        Ok(ParseOutcome::Domain(match destination {
            Some(destination) => DomainCommand::Remote {
                destination,
                command: Box::new(command),
            },
            None => command,
        }))
    }
}

impl SpawnArgs {
    fn into_spec(self) -> anyhow::Result<(SpawnSpec, bool)> {
        let mut command = self.command.into_iter();
        let mut spawn = SpawnSpec {
            program: command.next(),
            args: command.collect(),
            working_directory: self.working_directory,
            title: self.title,
            ..SpawnSpec::default()
        };
        if let Some((cols, rows)) = self.size {
            spawn.cols = cols;
            spawn.rows = rows;
        }
        spawn.validate()?;
        Ok((spawn, self.output.json))
    }
}

/// Commands about this computer's own daemon, never a remote one.
fn local_only(destination: &Option<String>, command: &str) -> anyhow::Result<()> {
    if destination.is_some() {
        anyhow::bail!("`domain {command}` can't run on another computer through --host");
    }
    Ok(())
}

fn domain_id(value: &str) -> anyhow::Result<String> {
    model::validate_id("domain id", value)?;
    Ok(value.to_owned())
}

fn ssh_destination(value: &str) -> anyhow::Result<String> {
    crate::domain::validate_ssh_destination(value)?;
    Ok(value.to_owned())
}

fn upload_name(value: &str) -> anyhow::Result<String> {
    protocol::validate_upload_name(value)?;
    Ok(value.to_owned())
}

fn text_bytes(value: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .ok()
        .filter(|bytes| (1..=model::MAX_TEXT_RESPONSE_BYTES).contains(bytes))
        .ok_or_else(|| {
            format!(
                "--bytes must be between 1 and {}",
                model::MAX_TEXT_RESPONSE_BYTES
            )
        })
}

fn cell_geometry(value: &str) -> anyhow::Result<(u16, u16)> {
    super::parse_cell_geometry(value)
}

fn window_geometry(value: &str) -> anyhow::Result<(u32, u32)> {
    super::parse_geometry(value)
}

fn app_id(value: &str) -> anyhow::Result<String> {
    super::validate_identifier("application identity", value)?;
    Ok(value.to_owned())
}
