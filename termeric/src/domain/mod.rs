//! Experimental persistent terminal domains.
//!
//! A domain daemon owns PTYs and bounded terminal state after every GUI has
//! detached. A local Unix socket is the daemon's only listener. Optional remote
//! control runs the same bounded protocol through an OpenSSH stdio bridge that
//! authenticates to the remote user's private socket without exporting its
//! token.

#[cfg(unix)]
mod archive;
#[cfg(unix)]
mod client;
pub mod model;
#[cfg(unix)]
pub(crate) mod paths;
pub mod protocol;
#[cfg(unix)]
mod server;

#[cfg(unix)]
pub use client::DomainClient;
#[cfg(unix)]
pub use client::request_focus_background;
#[cfg(unix)]
use client::run_ssh_stdio_bridge;
#[cfg(unix)]
pub use paths::RuntimePaths;

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

#[cfg(not(unix))]
pub struct DomainClient;

#[cfg(not(unix))]
impl DomainClient {
    pub fn new_upload_id() -> anyhow::Result<String> {
        anyhow::bail!("persistent local domains currently require Unix-domain sockets")
    }

    pub fn connect_or_start() -> anyhow::Result<Self> {
        anyhow::bail!("persistent local domains currently require Unix-domain sockets")
    }

    pub fn request(&self, _request: Request) -> anyhow::Result<Response> {
        anyhow::bail!("persistent local domains currently require Unix-domain sockets")
    }

    pub fn for_ssh(_destination: String) -> anyhow::Result<Self> {
        anyhow::bail!("SSH domain transport currently requires Unix-domain sockets")
    }

    pub fn relay(&self, _domain: String, _pane: u32) -> anyhow::Result<()> {
        anyhow::bail!("persistent local domains currently require Unix-domain sockets")
    }

    pub fn upload_file(
        &self,
        _domain: String,
        _pane: u32,
        _source: &std::path::Path,
        _name: Option<String>,
    ) -> anyhow::Result<Response> {
        anyhow::bail!("persistent local domains currently require Unix-domain sockets")
    }

    pub fn upload_path(
        &self,
        _domain: String,
        _pane: u32,
        _source: &std::path::Path,
        _name: Option<String>,
    ) -> anyhow::Result<Response> {
        anyhow::bail!("persistent local domains currently require Unix-domain sockets")
    }

    pub fn upload_path_with_manual_resume(
        &self,
        _domain: String,
        _pane: u32,
        _source: &std::path::Path,
        _name: Option<String>,
        _upload_id: String,
        _resume_only: bool,
    ) -> anyhow::Result<Response> {
        anyhow::bail!("persistent local domains currently require Unix-domain sockets")
    }

    pub fn upload_directory(
        &self,
        _domain: String,
        _pane: u32,
        _source: &std::path::Path,
        _name: Option<String>,
    ) -> anyhow::Result<Response> {
        anyhow::bail!("persistent local domains currently require Unix-domain sockets")
    }

    pub fn upload_directory_with_control(
        &self,
        _domain: String,
        _pane: u32,
        _source: &std::path::Path,
        _name: Option<String>,
        _expected_identity: Option<(u64, u64)>,
        _cancelled: &std::sync::atomic::AtomicBool,
        _progress: impl FnMut(u64, u64),
    ) -> anyhow::Result<Response> {
        anyhow::bail!("persistent local domains currently require Unix-domain sockets")
    }

    pub fn upload_open_file(
        &self,
        _domain: String,
        _pane: u32,
        _file: std::fs::File,
        _name: String,
    ) -> anyhow::Result<Response> {
        anyhow::bail!("persistent local domains currently require Unix-domain sockets")
    }

    pub fn upload_open_file_with_control(
        &self,
        _domain: String,
        _pane: u32,
        _file: std::fs::File,
        _name: String,
        _cancelled: &std::sync::atomic::AtomicBool,
        _progress: impl FnMut(u64, u64),
    ) -> anyhow::Result<Response> {
        anyhow::bail!("persistent local domains currently require Unix-domain sockets")
    }

    pub fn subscribe_topology(
        &self,
        _domain: String,
        _alive: &std::sync::atomic::AtomicBool,
        _receive: impl FnMut(DomainTopologyEvent) -> bool,
    ) -> anyhow::Result<()> {
        anyhow::bail!("persistent local domains currently require Unix-domain sockets")
    }
}

#[cfg(not(unix))]
pub fn request_focus_background(_authority: DomainAuthority, _domain: String, _pane: u32) {}

use crate::panes::{PaneLayoutPreset, SplitDirection};
use model::SpawnSpec;
use protocol::{Request, Response};

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
            Self::Ssh { destination } => DomainClient::for_ssh(destination.clone()),
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
    Remote {
        destination: String,
        command: Box<DomainCommand>,
    },
    List {
        json: bool,
    },
    Workspaces {
        domain: String,
        json: bool,
    },
    Panes {
        domain: String,
        json: bool,
    },
    Show {
        domain: String,
        json: bool,
    },
    Create {
        name: String,
        spawn: SpawnSpec,
        json: bool,
    },
    Spawn {
        domain: String,
        spawn: SpawnSpec,
        json: bool,
    },
    Split {
        domain: String,
        pane: u32,
        direction: SplitDirection,
        spawn: SpawnSpec,
        json: bool,
    },
    ResizeSplit {
        domain: String,
        split: u32,
        ratio_millis: u16,
    },
    RenameTab {
        domain: String,
        tab: u32,
        title: Option<String>,
    },
    SetTabPinned {
        domain: String,
        tab: u32,
        pinned: bool,
    },
    MoveTab {
        domain: String,
        tab: u32,
        index: u16,
    },
    SwapPaneNext {
        domain: String,
        pane: u32,
    },
    BalanceTab {
        domain: String,
        tab: u32,
    },
    ApplyPaneLayout {
        domain: String,
        tab: u32,
        preset: PaneLayoutPreset,
    },
    MovePane {
        domain: String,
        pane: u32,
        tab: u32,
    },
    Focus {
        domain: String,
        pane: u32,
    },
    ClosePane {
        domain: String,
        pane: u32,
    },
    CloseTab {
        domain: String,
        tab: u32,
    },
    Send {
        domain: String,
        pane: u32,
        data: Vec<u8>,
    },
    GetText {
        domain: String,
        pane: u32,
        max_bytes: usize,
        json: bool,
    },
    Uploads {
        json: bool,
    },
    CancelUpload {
        upload_id: String,
        json: bool,
    },
    Upload {
        domain: String,
        pane: u32,
        source: std::path::PathBuf,
        name: Option<String>,
        retain: bool,
        resume: Option<String>,
        json: bool,
    },
    Relay {
        domain: String,
        pane: u32,
    },
    Kill {
        domain: String,
    },
    StopServer,
}

pub fn execute(command: DomainCommand) -> anyhow::Result<Option<String>> {
    if let DomainCommand::ProtocolVersion { json } = &command {
        return if *json {
            Ok(Some(serde_json::to_string_pretty(&serde_json::json!({
                "protocol_version": protocol::PROTOCOL_VERSION,
            }))?))
        } else {
            Ok(Some(format!(
                "Domain protocol version {}",
                protocol::PROTOCOL_VERSION
            )))
        };
    }
    if command == DomainCommand::ServerForeground {
        run_server_foreground()?;
        return Ok(None);
    }
    if let DomainCommand::ProxyStdio { start_server } = command {
        run_proxy_stdio(start_server)?;
        return Ok(None);
    }
    if let DomainCommand::Hosts { json } = &command {
        let config = crate::config::Config::load_or_default()?;
        return if *json {
            Ok(Some(serde_json::to_string_pretty(
                &config.domains.ssh_hosts,
            )?))
        } else {
            Ok(Some(format_ssh_hosts(&config.domains.ssh_hosts)))
        };
    }
    if let DomainCommand::Remote {
        destination,
        command,
    } = command
    {
        // Stopping must not start a daemon just to stop it.
        let client = if *command == DomainCommand::StopServer {
            DomainClient::for_running_ssh(destination)?
        } else {
            DomainClient::for_ssh(destination)?
        };
        return execute_with_client(*command, client);
    }
    let client = if command == DomainCommand::StopServer {
        DomainClient::connect_running()?
            .ok_or_else(|| anyhow::anyhow!(client::NO_SERVER_RUNNING))?
    } else {
        DomainClient::connect_or_start()?
    };
    execute_with_client(command, client)
}

fn execute_with_client(
    command: DomainCommand,
    client: DomainClient,
) -> anyhow::Result<Option<String>> {
    if let DomainCommand::Relay { domain, pane } = command {
        client.relay(domain, pane)?;
        return Ok(None);
    }
    if let DomainCommand::Upload {
        domain,
        pane,
        source,
        name,
        retain,
        resume,
        json,
    } = command
    {
        let response = if let Some(upload_id) = resume {
            client.upload_path_with_manual_resume(domain, pane, &source, name, upload_id, true)?
        } else if retain {
            let upload_id = DomainClient::new_upload_id()?;
            eprintln!(
                "Resumable upload ID: {upload_id} (retained after interruption for up to 24 hours)"
            );
            client.upload_path_with_manual_resume(domain, pane, &source, name, upload_id, false)?
        } else {
            client.upload_path(domain, pane, &source, name)?
        };
        return if json {
            Ok(Some(serde_json::to_string_pretty(&response)?))
        } else {
            Ok(Some(format_human(response)))
        };
    }
    let (request, json) = match command {
        DomainCommand::List { json } => (Request::ListDomains, json),
        DomainCommand::Workspaces { domain, json } => (Request::ListWorkspaces { domain }, json),
        DomainCommand::Panes { domain, json } => (Request::ListPanes { domain }, json),
        DomainCommand::Show { domain, json } => (Request::Describe { domain }, json),
        DomainCommand::Create { name, spawn, json } => {
            (Request::CreateDomain { name, spawn }, json)
        }
        DomainCommand::Spawn {
            domain,
            spawn,
            json,
        } => (Request::SpawnTab { domain, spawn }, json),
        DomainCommand::Split {
            domain,
            pane,
            direction,
            spawn,
            json,
        } => (
            Request::SplitPane {
                domain,
                pane,
                direction,
                spawn,
            },
            json,
        ),
        DomainCommand::ResizeSplit {
            domain,
            split,
            ratio_millis,
        } => (
            Request::ResizeSplit {
                domain,
                split,
                ratio_millis,
            },
            false,
        ),
        DomainCommand::RenameTab { domain, tab, title } => {
            (Request::RenameTab { domain, tab, title }, false)
        }
        DomainCommand::SetTabPinned {
            domain,
            tab,
            pinned,
        } => (
            Request::SetTabPinned {
                domain,
                tab,
                pinned,
            },
            false,
        ),
        DomainCommand::MoveTab { domain, tab, index } => {
            (Request::MoveTab { domain, tab, index }, false)
        }
        DomainCommand::SwapPaneNext { domain, pane } => {
            (Request::SwapPaneNext { domain, pane }, false)
        }
        DomainCommand::BalanceTab { domain, tab } => (Request::BalanceTab { domain, tab }, false),
        DomainCommand::ApplyPaneLayout {
            domain,
            tab,
            preset,
        } => (
            Request::ApplyPaneLayout {
                domain,
                tab,
                preset,
            },
            false,
        ),
        DomainCommand::MovePane { domain, pane, tab } => (
            Request::MovePane {
                domain,
                pane,
                tab,
                target: None,
                direction: crate::panes::SplitDirection::Vertical,
            },
            false,
        ),
        DomainCommand::Focus { domain, pane } => (Request::FocusPane { domain, pane }, false),
        DomainCommand::ClosePane { domain, pane } => (Request::ClosePane { domain, pane }, false),
        DomainCommand::CloseTab { domain, tab } => (Request::CloseTab { domain, tab }, false),
        DomainCommand::Send { domain, pane, data } => {
            (Request::SendText { domain, pane, data }, false)
        }
        DomainCommand::GetText {
            domain,
            pane,
            max_bytes,
            json,
        } => (
            Request::GetText {
                domain,
                pane,
                max_bytes,
            },
            json,
        ),
        DomainCommand::Uploads { json } => (Request::ListUploads, json),
        DomainCommand::CancelUpload { upload_id, json } => {
            (Request::CancelUpload { upload_id }, json)
        }
        DomainCommand::Kill { domain } => (Request::KillDomain { domain }, false),
        DomainCommand::StopServer => (Request::Shutdown, false),
        DomainCommand::ServerForeground
        | DomainCommand::ProxyStdio { .. }
        | DomainCommand::ProtocolVersion { .. }
        | DomainCommand::Hosts { .. }
        | DomainCommand::Remote { .. }
        | DomainCommand::Relay { .. }
        | DomainCommand::Upload { .. } => unreachable!(),
    };
    let response = client.request(request)?;
    if json {
        return Ok(Some(serde_json::to_string_pretty(&response)?));
    }
    Ok(Some(format_human(response)))
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

#[cfg(unix)]
fn run_proxy_stdio(start_server: bool) -> anyhow::Result<()> {
    run_ssh_stdio_bridge(start_server)
}

#[cfg(not(unix))]
fn run_proxy_stdio(start_server: bool) -> anyhow::Result<()> {
    anyhow::bail!("the SSH domain bridge currently requires Unix-domain sockets")
}

#[cfg(unix)]
fn run_server_foreground() -> anyhow::Result<()> {
    let (config, warning) = crate::config::Config::load_for_startup();
    if let Some(warning) = warning {
        log::warn!("Domain server config fallback: {warning}");
    }
    server::run_foreground(
        RuntimePaths::discover()?,
        config.security.allow_screen_checksum_reports,
        config.keyboard.xterm_type.terminal_type(),
    )
}

#[cfg(not(unix))]
fn run_server_foreground() -> anyhow::Result<()> {
    anyhow::bail!("persistent local domains currently require Unix-domain sockets")
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
        Response::Workspaces { workspaces } => workspaces
            .into_iter()
            .map(|workspace| {
                format!(
                    "{}\t{}\t{} tabs\tactive {}",
                    workspace.id,
                    workspace.name,
                    workspace.tabs.len(),
                    workspace.active_tab
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
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
            "{}\t{}\tworkspace {}\t{} tabs\t{} panes",
            domain.id,
            domain.name,
            domain.workspace.id,
            domain.workspace.tabs.len(),
            domain
                .workspace
                .tabs
                .iter()
                .map(|tab| tab.panes.len())
                .sum::<usize>()
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
        Response::Uploads { uploads } => {
            if uploads.is_empty() {
                return "No retained uploads".into();
            }
            uploads
                .into_iter()
                .map(|upload| {
                    let kind = match upload.kind {
                        protocol::UploadKind::File => "file".to_string(),
                        protocol::UploadKind::Directory { entries } => {
                            format!("directory/{entries}")
                        }
                    };
                    format!(
                        "{}\t{}\t{}/{} bytes\t{}\t{}/{}\t{}\texpires {}s",
                        upload.upload_id,
                        kind,
                        upload.bytes_received,
                        upload.total_bytes,
                        if upload.active { "active" } else { "retained" },
                        upload.domain,
                        upload.pane,
                        upload.name,
                        upload.expires_in_seconds
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
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
