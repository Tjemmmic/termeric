//! Serializable descriptions of live terminal domains.

use serde::{Deserialize, Serialize};
use termeric_core::split::SplitDirection;

pub const MAX_DOMAINS: usize = 16;
/// Panes across every domain of one daemon; it also bounds tabs per domain.
pub const MAX_DAEMON_PANES: usize = 16;
pub const MAX_ATTACHMENTS_PER_PANE: usize = 8;
pub const MAX_TOPOLOGY_SUBSCRIBERS_PER_DOMAIN: usize = 8;
pub const MAX_PANE_SCROLLBACK_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_TEXT_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainSummary {
    pub id: String,
    pub name: String,
    pub workspace_id: String,
    pub tabs: usize,
    pub panes: usize,
    pub attachments: usize,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainSnapshot {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub workspace: LiveWorkspace,
}

/// Minimal whole-domain state needed to reconcile attached GUIs. Commands,
/// working directories, geometry, and attachment counts remain available from
/// inspection requests but are deliberately excluded from frequent revisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainTopology {
    pub domain_id: String,
    pub active_tab: u32,
    pub tabs: Vec<LiveTopologyTab>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveTopologyTab {
    pub id: u32,
    pub title: String,
    pub user_title: Option<String>,
    pub pinned: bool,
    pub focused_pane: u32,
    pub layout: LivePaneLayout,
    pub panes: Vec<LiveTopologyPane>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveTopologyPane {
    pub id: u32,
    pub title: String,
    pub closed: bool,
}

impl From<&LivePane> for LiveTopologyPane {
    fn from(pane: &LivePane) -> Self {
        Self {
            id: pane.id,
            title: pane.title.clone(),
            closed: pane.closed,
        }
    }
}

impl From<&LiveTab> for LiveTopologyTab {
    fn from(tab: &LiveTab) -> Self {
        Self {
            id: tab.id,
            title: tab.title.clone(),
            user_title: tab.user_title.clone(),
            pinned: tab.pinned,
            focused_pane: tab.focused_pane,
            layout: tab.layout.clone(),
            panes: tab.panes.iter().map(LiveTopologyPane::from).collect(),
        }
    }
}

impl From<&DomainSnapshot> for DomainTopology {
    fn from(domain: &DomainSnapshot) -> Self {
        Self {
            domain_id: domain.id.clone(),
            active_tab: domain.workspace.active_tab,
            tabs: domain
                .workspace
                .tabs
                .iter()
                .map(LiveTopologyTab::from)
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveWorkspace {
    pub id: String,
    pub name: String,
    pub active_tab: u32,
    pub tabs: Vec<LiveTab>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveTab {
    pub id: u32,
    pub title: String,
    pub user_title: Option<String>,
    pub pinned: bool,
    pub focused_pane: u32,
    pub layout: LivePaneLayout,
    pub panes: Vec<LivePane>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LivePane {
    pub id: u32,
    pub title: String,
    pub working_directory: Option<String>,
    pub command: Vec<String>,
    pub cols: u16,
    pub rows: u16,
    pub closed: bool,
    pub attachments: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "node", rename_all = "snake_case")]
pub enum LivePaneLayout {
    Pane {
        pane: u32,
    },
    Split {
        /// Stable daemon identity for this divider for the lifetime of the
        /// split node. It is independent of pane IDs and tree position.
        split_id: u32,
        direction: SplitDirection,
        ratio_millis: u16,
        first: Box<LivePaneLayout>,
        second: Box<LivePaneLayout>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SpawnSpec {
    pub program: Option<String>,
    pub args: Vec<String>,
    pub working_directory: Option<String>,
    pub title: Option<String>,
    pub cols: u16,
    pub rows: u16,
}

impl Default for SpawnSpec {
    fn default() -> Self {
        Self {
            program: None,
            args: Vec::new(),
            working_directory: None,
            title: None,
            cols: 80,
            rows: 24,
        }
    }
}

pub fn validate_id(label: &str, value: &str) -> anyhow::Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        anyhow::bail!("invalid {label} {value:?}");
    }
    Ok(())
}

pub fn validate_name(label: &str, value: &str) -> anyhow::Result<()> {
    if value.is_empty() || value.chars().count() > 128 || value.chars().any(char::is_control) {
        anyhow::bail!("{label} must contain 1 to 128 single-line characters");
    }
    Ok(())
}

impl SpawnSpec {
    /// Checks the request's shape, wherever it is built. Whether the working
    /// directory exists is for the daemon that spawns it to say: with
    /// `--host`, it names a folder on the remote computer.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.cols == 0 || self.rows == 0 || self.cols > 1_000 || self.rows > 1_000 {
            anyhow::bail!("terminal geometry must be between 1x1 and 1000x1000 cells");
        }
        if self.args.len() > 256
            || self.args.iter().any(|argument| argument.contains('\0'))
            || self.args.iter().map(String::len).sum::<usize>() > 256 * 1024
        {
            anyhow::bail!("command arguments exceed the domain resource policy");
        }
        if self.program.as_ref().is_some_and(|program| {
            program.is_empty() || program.len() > 64 * 1024 || program.contains('\0')
        }) {
            anyhow::bail!("domain command program is empty or exceeds the resource policy");
        }
        if let Some(title) = &self.title {
            validate_name("pane title", title)?;
        }
        if let Some(cwd) = &self.working_directory {
            if cwd.len() > 4_096 || cwd.chars().any(char::is_control) {
                anyhow::bail!("working directory exceeds the domain resource policy");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_resource_policy_rejects_geometry_argv_paths_and_nuls() {
        for spawn in [
            SpawnSpec {
                cols: 0,
                ..SpawnSpec::default()
            },
            SpawnSpec {
                program: Some("bad\0program".into()),
                ..SpawnSpec::default()
            },
            SpawnSpec {
                args: vec!["x".repeat(256 * 1024 + 1)],
                ..SpawnSpec::default()
            },
            SpawnSpec {
                working_directory: Some("two\nlines".into()),
                ..SpawnSpec::default()
            },
        ] {
            assert!(spawn.validate().is_err());
        }
        assert!(SpawnSpec::default().validate().is_ok());
        let elsewhere = SpawnSpec {
            working_directory: Some("/path/that/does/not/exist/here".into()),
            ..SpawnSpec::default()
        };
        assert!(elsewhere.validate().is_ok());
    }

    #[test]
    fn identifier_and_name_boundaries_are_explicit() {
        assert!(validate_id("domain", "domain-1.ok").is_ok());
        assert!(validate_id("domain", "bad/id").is_err());
        assert!(validate_name("domain", "Daily λ").is_ok());
        assert!(validate_name("domain", "two\nlines").is_err());
    }

    #[test]
    fn topology_revisions_exclude_heavy_inspection_metadata() {
        let marker = "large-command-marker".repeat(10_000);
        let snapshot = DomainSnapshot {
            id: "domain-memory".into(),
            name: "memory".into(),
            created_at: "now".into(),
            workspace: LiveWorkspace {
                id: "live-domain-memory".into(),
                name: "memory".into(),
                active_tab: 0,
                tabs: vec![LiveTab {
                    id: 0,
                    title: "tab".into(),
                    user_title: None,
                    pinned: false,
                    focused_pane: 0,
                    layout: LivePaneLayout::Pane { pane: 0 },
                    panes: vec![LivePane {
                        id: 0,
                        title: "pane".into(),
                        working_directory: Some("/large/inspection/path".into()),
                        command: vec![marker.clone()],
                        cols: 80,
                        rows: 24,
                        closed: false,
                        attachments: 7,
                    }],
                }],
            },
        };
        let topology = DomainTopology::from(&snapshot);
        let encoded = serde_json::to_vec(&topology).unwrap();
        assert!(encoded.len() < 512);
        assert!(
            !encoded
                .windows(marker.len())
                .any(|window| window == marker.as_bytes())
        );
        assert_eq!(topology.tabs[0].panes[0].id, 0);
    }
}
