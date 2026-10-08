//! Experimental persistent terminal domains.
//!
//! A domain daemon owns PTYs and bounded terminal state after every GUI has
//! detached. A local Unix socket is the daemon's only listener. Optional remote
//! control runs the same bounded protocol through an OpenSSH stdio bridge that
//! authenticates to the remote user's private socket without exporting its
//! token.

mod archive;
mod client;
mod io_util;
mod layout;
pub mod model;
pub(crate) mod paths;
pub mod protocol;
mod server;

pub use client::request_focus_background;
use client::run_ssh_stdio_bridge;
pub use client::{DomainClient, UploadOptions};
use protocol::{Request, Response};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainTopologyEvent {
    Snapshot {
        revision: u64,
        domain: model::DomainTopology,
    },
    Closed {
        domain: String,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DomainAuthority {
    Local,
    Ssh { destination: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainTarget {
    pub authority: DomainAuthority,
    pub domain_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainRelayCommand {
    program: String,
    arguments: Vec<String>,
}

pub fn validate_ssh_destination(destination: &str) -> anyhow::Result<()> {
    if destination.is_empty()
        || destination.len() > 512
        || destination.starts_with('-')
        || destination.contains("://")
        || destination
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        anyhow::bail!("invalid SSH destination {destination:?}");
    }
    Ok(())
}

impl DomainAuthority {
    pub fn client(&self) -> anyhow::Result<DomainClient> {
        match self {
            Self::Local => DomainClient::connect_or_start(),
            Self::Ssh { destination } => DomainClient::for_ssh(destination.clone(), true),
        }
    }

    pub fn relay_command(&self, executable: String) -> DomainRelayCommand {
        let mut arguments = vec!["domain".into()];
        if let Self::Ssh { destination } = self {
            arguments.extend(["--host".into(), destination.clone()]);
        }
        arguments.push("relay".into());
        DomainRelayCommand {
            program: executable,
            arguments,
        }
    }
}

impl DomainRelayCommand {
    #[cfg(test)]
    pub fn local(program: impl Into<String>) -> Self {
        DomainAuthority::Local.relay_command(program.into())
    }

    pub fn program(&self) -> &str {
        &self.program
    }

    pub fn pane_arguments(&self, domain: &str, pane: u32) -> Vec<String> {
        let mut arguments = self.arguments.clone();
        arguments.extend([domain.to_string(), pane.to_string()]);
        arguments
    }
}

/// What `termeric domain` runs. Commands about this computer alone are
/// variants of their own, so that only a [`DaemonCommand`] can go to another
/// computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainCommand {
    ServerForeground,
    /// The SSH bridge; without `start_server` it refuses when no daemon runs.
    ProxyStdio {
        start_server: bool,
    },
    ProtocolVersion {
        json: bool,
    },
    Hosts {
        json: bool,
    },
    /// A command for this computer's domain daemon.
    Local(DaemonCommand),
    /// A command for the domain daemon on `destination`, through SSH.
    Remote {
        destination: String,
        command: DaemonCommand,
    },
}

/// A command a domain daemon answers, on this computer or another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonCommand {
    Upload {
        domain: String,
        pane: u32,
        source: std::path::PathBuf,
        name: Option<String>,
        json: bool,
    },
    Relay {
        domain: String,
        pane: u32,
    },
    /// Every other command is one daemon request, printed as JSON or text.
    Request {
        request: Request,
        json: bool,
    },
}

impl DaemonCommand {
    /// A request whose reply prints as text.
    pub fn request(request: Request) -> Self {
        Self::Request {
            request,
            json: false,
        }
    }

    fn stops_the_server(&self) -> bool {
        matches!(
            self,
            Self::Request {
                request: Request::Shutdown,
                ..
            }
        )
    }
}

pub fn execute(command: DomainCommand) -> anyhow::Result<Option<String>> {
    let (client, command) = match command {
        DomainCommand::ProtocolVersion { json } => {
            return Ok(Some(if json {
                serde_json::to_string_pretty(&serde_json::json!({
                    "protocol_version": protocol::PROTOCOL_VERSION,
                }))?
            } else {
                format!("Domain protocol version {}", protocol::PROTOCOL_VERSION)
            }));
        }
        DomainCommand::ServerForeground => {
            run_server_foreground()?;
            return Ok(None);
        }
        DomainCommand::ProxyStdio { start_server } => {
            run_ssh_stdio_bridge(start_server)?;
            return Ok(None);
        }
        DomainCommand::Hosts { json } => {
            let hosts = crate::config::Config::load_or_default()?.domains.ssh_hosts;
            return Ok(Some(if json {
                serde_json::to_string_pretty(&hosts)?
            } else {
                format_ssh_hosts(&hosts)
            }));
        }
        // Stopping must not start a daemon just to stop it.
        DomainCommand::Remote {
            destination,
            command,
        } => {
            let start_server = !command.stops_the_server();
            (DomainClient::for_ssh(destination, start_server)?, command)
        }
        DomainCommand::Local(command) if command.stops_the_server() => (
            DomainClient::connect_running()?
                .ok_or_else(|| anyhow::anyhow!(client::NO_SERVER_RUNNING))?,
            command,
        ),
        DomainCommand::Local(command) => (DomainClient::connect_or_start()?, command),
    };
    let (response, json) = match command {
        DaemonCommand::Relay { domain, pane } => {
            client.relay(domain, pane)?;
            return Ok(None);
        }
        DaemonCommand::Upload {
            domain,
            pane,
            source,
            name,
            json,
        } => (
            client.upload(domain, pane, &source, UploadOptions::named(name))?,
            json,
        ),
        DaemonCommand::Request { request, json } => (client.request(request)?, json),
    };
    Ok(Some(if json {
        serde_json::to_string_pretty(&response)?
    } else {
        format_human(response)
    }))
}

fn format_ssh_hosts(
    hosts: &std::collections::BTreeMap<String, crate::config::SshHostProfile>,
) -> String {
    if hosts.is_empty() {
        return "No SSH host profiles".into();
    }
    hosts
        .iter()
        .map(|(name, profile)| {
            profile.description.as_ref().map_or_else(
                || format!("{name}\t{}", profile.destination),
                |description| format!("{name}\t{}\t{description}", profile.destination),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn run_server_foreground() -> anyhow::Result<()> {
    let (config, warning) = crate::config::Config::load_for_startup();
    if let Some(warning) = warning {
        log::warn!("Domain server config fallback: {warning}");
    }
    server::run_foreground(
        paths::discover()?,
        config.security.allow_screen_checksum_reports,
        config.keyboard.xterm_type.terminal_type(),
    )
}

fn format_human(response: Response) -> String {
    match response {
        Response::Domains { domains } => {
            if domains.is_empty() {
                return "No live terminal domains".into();
            }
            domains
                .into_iter()
                .map(|domain| {
                    format!(
                        "{}\t{}\t{} tabs\t{} panes\t{} attached",
                        domain.id, domain.name, domain.tabs, domain.panes, domain.attachments
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        Response::Panes { panes } => panes
            .into_iter()
            .map(|pane| {
                format!(
                    "{}\t{}\t{}x{}\t{}\t{} attached",
                    pane.id,
                    pane.title,
                    pane.cols,
                    pane.rows,
                    if pane.closed { "closed" } else { "live" },
                    pane.attachments
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Response::Domain { domain } => format!(
            "{}\t{}\t{} tabs\t{} panes",
            domain.id,
            domain.name,
            domain.tabs.len(),
            domain.tabs.iter().map(|tab| tab.panes.len()).sum::<usize>()
        ),
        Response::Text { text, truncated } => {
            if truncated {
                format!("{text}\n[Termeric: older domain text was truncated]")
            } else {
                text
            }
        }
        Response::FileUploaded { path, bytes } => {
            format!("uploaded {bytes} bytes to {path}")
        }
        Response::DirectoryUploaded {
            path,
            bytes,
            entries,
        } => format!("uploaded directory with {entries} entries ({bytes} archive bytes) to {path}"),
        Response::UploadCancelled { bytes_received } => {
            format!("upload cancelled after {bytes_received} bytes")
        }
        Response::UploadBusy { retry_after_ms } => {
            format!("upload busy; retry after {retry_after_ms} ms")
        }
        Response::UploadReady { .. } => "upload ready".into(),
        Response::Ok { message } => message,
        Response::Attached { .. } => "attached".into(),
        Response::TopologySubscribed { revision, domain } => {
            format!(
                "subscribed to {} topology revision {revision}",
                domain.domain_id
            )
        }
    }
}

#[cfg(test)]
mod authority_tests {
    use super::{
        DomainAuthority, DomainCommand, DomainRelayCommand, DomainTarget, execute, format_ssh_hosts,
    };

    #[test]
    fn protocol_version_is_available_without_a_daemon_connection() {
        assert_eq!(
            execute(DomainCommand::ProtocolVersion { json: false }).unwrap(),
            Some(format!(
                "Domain protocol version {}",
                super::protocol::PROTOCOL_VERSION
            ))
        );
        let json = execute(DomainCommand::ProtocolVersion { json: true })
            .unwrap()
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["protocol_version"], super::protocol::PROTOCOL_VERSION);
    }

    #[test]
    fn relay_commands_and_targets_keep_local_and_ssh_authorities_disjoint() {
        let local = DomainAuthority::Local.relay_command("/opt/termeric".into());
        assert_eq!(local.program(), "/opt/termeric");
        assert_eq!(
            local.pane_arguments("same-id", 7),
            ["domain", "relay", "same-id", "7"]
        );

        let remote_authority = DomainAuthority::Ssh {
            destination: "developer@build.example".into(),
        };
        let remote = remote_authority.relay_command("/opt/termeric".into());
        assert_eq!(
            remote.pane_arguments("same-id", 7),
            [
                "domain",
                "--host",
                "developer@build.example",
                "relay",
                "same-id",
                "7",
            ]
        );
        assert_ne!(
            DomainTarget {
                authority: DomainAuthority::Local,
                domain_id: "same-id".into(),
            },
            DomainTarget {
                authority: remote_authority,
                domain_id: "same-id".into(),
            }
        );
        assert_eq!(DomainRelayCommand::local("/opt/termeric"), local);
    }

    #[test]
    fn configured_ssh_host_inventory_is_stable_and_human_readable() {
        let hosts = std::collections::BTreeMap::from([
            (
                "build".into(),
                crate::config::SshHostProfile {
                    destination: "developer@build.example".into(),
                    description: Some("Build farm".into()),
                },
            ),
            (
                "production".into(),
                crate::config::SshHostProfile {
                    destination: "prod-alias".into(),
                    description: None,
                },
            ),
        ]);
        assert_eq!(
            format_ssh_hosts(&hosts),
            "build\tdeveloper@build.example\tBuild farm\nproduction\tprod-alias"
        );
        let json = serde_json::to_string(&hosts).unwrap();
        assert!(json.contains("\"build\""));
        assert!(json.contains("\"destination\":\"developer@build.example\""));
        assert_eq!(
            format_ssh_hosts(&Default::default()),
            "No SSH host profiles"
        );
    }
}
