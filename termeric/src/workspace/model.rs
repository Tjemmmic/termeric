use crate::panes::SplitDirection;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const WORKSPACE_SCHEMA_VERSION: u32 = 3;
pub const MAX_WORKSPACE_TABS: usize = 64;
pub const MAX_WORKSPACE_PANES_PER_TAB: usize = 64;
/// The longest tab or pane title a workspace keeps, in characters.
pub const MAX_WORKSPACE_TITLE_CHARS: usize = 256;
/// The longest workspace name, in characters.
pub const MAX_WORKSPACE_NAME_CHARS: usize = 128;

/// `title` as a workspace keeps it. A terminal or a profile can set a longer
/// title than a workspace holds, so it is cut to
/// [`MAX_WORKSPACE_TITLE_CHARS`], with `…` marking the cut.
pub fn bounded_workspace_title(title: &str) -> String {
    crate::safe_text::truncate_with_ellipsis(title, MAX_WORKSPACE_TITLE_CHARS)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceSnapshot {
    pub version: u32,
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub updated_at: String,
    pub active_tab: usize,
    #[serde(default)]
    pub config: WorkspaceConfigReference,
    /// Saved automatically when a window closed, not by the person. Only the
    /// archives the startup state still names are kept; saving or renaming
    /// one makes it the person's.
    #[serde(default)]
    pub automatic: bool,
    /// A tab kept when it closed, listed under tab snapshots rather than
    /// with workspaces, and reopened as one tab in the current window.
    #[serde(default)]
    pub closed_tab: bool,
    pub tabs: Vec<WorkspaceTabSnapshot>,
}

impl WorkspaceSnapshot {
    pub fn new(id: String, name: String, tabs: Vec<WorkspaceTabSnapshot>) -> Self {
        let now = super::storage::save_timestamp();
        Self {
            version: WORKSPACE_SCHEMA_VERSION,
            id,
            name,
            created_at: now.clone(),
            updated_at: now,
            active_tab: 0,
            config: WorkspaceConfigReference::default(),
            automatic: false,
            closed_tab: false,
            tabs,
        }
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if self.version != WORKSPACE_SCHEMA_VERSION {
            anyhow::bail!(
                "workspace schema {} cannot be saved as schema {}",
                self.version,
                WORKSPACE_SCHEMA_VERSION
            );
        }
        validate_workspace_id(&self.id)?;
        validate_workspace_name(&self.name)?;
        if self.tabs.is_empty() || self.tabs.len() > MAX_WORKSPACE_TABS {
            anyhow::bail!("workspace must contain between 1 and {MAX_WORKSPACE_TABS} tabs");
        }
        if self.active_tab >= self.tabs.len() {
            anyhow::bail!("active tab {} is outside the tab list", self.active_tab);
        }

        let mut tab_ids = HashSet::new();
        let mut saw_unpinned = false;
        for tab in &self.tabs {
            if !tab_ids.insert(tab.id) {
                anyhow::bail!("duplicate workspace tab id {}", tab.id);
            }
            if !tab.pinned {
                saw_unpinned = true;
            } else if saw_unpinned {
                anyhow::bail!("pinned workspace tabs must form a leading group");
            }
            tab.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceConfigReference {
    pub theme: Option<String>,
    pub shell_program: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceTabSnapshot {
    pub id: u32,
    pub title: String,
    #[serde(default)]
    pub user_title: Option<String>,
    #[serde(default)]
    pub pinned: bool,
    /// Pane that is relaunched first and adopts the tab's initial live PTY.
    #[serde(default)]
    pub primary_pane: u32,
    pub focused_pane: u32,
    pub layout: WorkspacePaneLayout,
    pub panes: Vec<WorkspacePaneSnapshot>,
}

impl WorkspaceTabSnapshot {
    /// What the tab relaunches as: its primary pane's launch.
    pub fn launch(&self) -> anyhow::Result<&WorkspaceLaunch> {
        self.panes
            .iter()
            .find(|pane| pane.id == self.primary_pane)
            .map(|pane| &pane.launch)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "workspace tab {} primary pane {} is missing",
                    self.id,
                    self.primary_pane
                )
            })
    }

    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        if self.title.chars().count() > MAX_WORKSPACE_TITLE_CHARS {
            anyhow::bail!("workspace tab {} title is too long", self.id);
        }
        if self.user_title.as_ref().is_some_and(|title| {
            title.trim().is_empty() || title.chars().count() > MAX_WORKSPACE_TITLE_CHARS
        }) {
            anyhow::bail!("workspace tab {} user title is invalid", self.id);
        }
        if self.panes.is_empty() || self.panes.len() > MAX_WORKSPACE_PANES_PER_TAB {
            anyhow::bail!(
                "workspace tab {} must contain between 1 and {} panes",
                self.id,
                MAX_WORKSPACE_PANES_PER_TAB
            );
        }
        let mut pane_ids = HashSet::new();
        for pane in &self.panes {
            if !pane_ids.insert(pane.id) {
                anyhow::bail!("workspace tab {} has duplicate pane {}", self.id, pane.id);
            }
            pane.validate()?;
        }
        if !pane_ids.contains(&self.focused_pane) {
            anyhow::bail!(
                "workspace tab {} focuses missing pane {}",
                self.id,
                self.focused_pane
            );
        }
        if !pane_ids.contains(&self.primary_pane) {
            anyhow::bail!(
                "workspace tab {} primary pane {} is missing",
                self.id,
                self.primary_pane
            );
        }
        let mut leaves = Vec::new();
        self.layout.validate_and_collect(&mut leaves, 0)?;
        let leaf_ids: HashSet<u32> = leaves.iter().copied().collect();
        if leaf_ids.len() != leaves.len() || leaf_ids != pane_ids {
            anyhow::bail!(
                "workspace tab {} layout leaves do not match its pane records",
                self.id
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkspaceLaunch {
    Shell,
    Profile {
        name: String,
    },
    Plugin {
        plugin_id: String,
        #[serde(default)]
        profile: Option<String>,
    },
}

/// What a tab or pane relaunches as, in words for warnings and logs.
impl std::fmt::Display for WorkspaceLaunch {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Shell => formatter.write_str("shell"),
            Self::Profile { name } => write!(formatter, "terminal profile {name:?}"),
            Self::Plugin {
                plugin_id,
                profile: None,
            } => write!(formatter, "plugin {plugin_id:?}"),
            Self::Plugin {
                plugin_id,
                profile: Some(profile),
            } => write!(formatter, "plugin {plugin_id:?} profile {profile:?}"),
        }
    }
}

impl WorkspaceLaunch {
    fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::Shell => {}
            Self::Profile { name } => {
                crate::config::validate_terminal_profile_name(name)
                    .context("invalid workspace terminal profile")?;
            }
            Self::Plugin { plugin_id, profile } => {
                if !is_safe_identifier(plugin_id) {
                    anyhow::bail!("invalid workspace plugin id {plugin_id:?}");
                }
                if profile
                    .as_ref()
                    .is_some_and(|profile| profile.is_empty() || profile.chars().count() > 128)
                {
                    anyhow::bail!("invalid workspace plugin profile");
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "node", rename_all = "snake_case")]
pub enum WorkspacePaneLayout {
    Pane {
        pane: u32,
    },
    Split {
        direction: SplitDirection,
        ratio: f32,
        first: Box<WorkspacePaneLayout>,
        second: Box<WorkspacePaneLayout>,
    },
}

impl WorkspacePaneLayout {
    fn validate_and_collect(&self, leaves: &mut Vec<u32>, depth: usize) -> anyhow::Result<()> {
        if depth >= MAX_WORKSPACE_PANES_PER_TAB {
            anyhow::bail!("workspace pane layout exceeds the supported nesting depth");
        }
        match self {
            Self::Pane { pane } => {
                if leaves.len() >= MAX_WORKSPACE_PANES_PER_TAB {
                    anyhow::bail!("workspace pane layout has too many leaves");
                }
                leaves.push(*pane);
            }
            Self::Split {
                ratio,
                first,
                second,
                ..
            } => {
                if !ratio.is_finite() || !(0.1..=0.9).contains(ratio) {
                    anyhow::bail!("workspace split ratio {ratio} is outside 0.1..=0.9");
                }
                first.validate_and_collect(leaves, depth + 1)?;
                second.validate_and_collect(leaves, depth + 1)?;
            }
        }
        Ok(())
    }
}

impl From<&crate::panes::PaneNode> for WorkspacePaneLayout {
    fn from(node: &crate::panes::PaneNode) -> Self {
        match node {
            crate::panes::PaneNode::Leaf(id) => Self::Pane { pane: id.0 },
            crate::panes::PaneNode::Split {
                direction,
                ratio,
                first,
                second,
                ..
            } => Self::Split {
                direction: *direction,
                ratio: *ratio,
                first: Box::new(Self::from(first.as_ref())),
                second: Box::new(Self::from(second.as_ref())),
            },
        }
    }
}

impl From<&WorkspacePaneLayout> for crate::panes::PaneNode {
    fn from(layout: &WorkspacePaneLayout) -> Self {
        match layout {
            WorkspacePaneLayout::Pane { pane } => Self::Leaf(crate::panes::PaneId(*pane)),
            WorkspacePaneLayout::Split {
                direction,
                ratio,
                first,
                second,
            } => Self::Split {
                local_split_id: 0,
                domain_split_id: None,
                direction: *direction,
                ratio: *ratio,
                first: Box::new(Self::from(first.as_ref())),
                second: Box::new(Self::from(second.as_ref())),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspacePaneSnapshot {
    pub id: u32,
    /// Exact relaunch identity for this pane; the primary pane's is the
    /// tab's.
    pub launch: WorkspaceLaunch,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub working_directory: Option<String>,
    #[serde(default)]
    pub last_command: Option<String>,
    #[serde(default)]
    pub scrollback_file: Option<String>,
    #[serde(default)]
    pub scrollback_truncated: bool,
    #[serde(skip)]
    pub scrollback_lines: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceRestoreOutcome {
    pub restored_tabs: usize,
    pub skipped_tabs: usize,
    pub warnings: Vec<String>,
}

impl WorkspacePaneSnapshot {
    fn validate(&self) -> anyhow::Result<()> {
        self.launch.validate()?;
        if self
            .title
            .as_ref()
            .is_some_and(|title| title.chars().count() > MAX_WORKSPACE_TITLE_CHARS)
        {
            anyhow::bail!("workspace pane {} title is too long", self.id);
        }
        if let Some(file) = &self.scrollback_file
            && !is_safe_scrollback_file(file)
        {
            anyhow::bail!("workspace pane {} has unsafe scrollback path", self.id);
        }
        Ok(())
    }
}

pub fn validate_workspace_id(id: &str) -> anyhow::Result<()> {
    if !is_safe_identifier(id) || id.len() > 160 {
        anyhow::bail!("invalid workspace id {id:?}");
    }
    Ok(())
}

pub fn validate_workspace_name(name: &str) -> anyhow::Result<()> {
    if name.trim().is_empty()
        || name.chars().count() > MAX_WORKSPACE_NAME_CHARS
        || name.contains(['\n', '\r', '\0'])
    {
        anyhow::bail!(
            "workspace name must contain 1 to {MAX_WORKSPACE_NAME_CHARS} single-line characters"
        );
    }
    Ok(())
}

fn is_safe_identifier(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('.')
        && !value.contains("..")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn is_safe_scrollback_file(value: &str) -> bool {
    let path = std::path::Path::new(value);
    path.components().count() == 2
        && path.starts_with("scrollback")
        && path.extension().and_then(|ext| ext.to_str()) == Some("txt")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn sample_workspace() -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            version: WORKSPACE_SCHEMA_VERSION,
            id: "workspace-1".into(),
            name: "Asymmetric roots".into(),
            created_at: "2026-08-26T00:00:00Z".into(),
            updated_at: "2026-08-26T00:00:00Z".into(),
            active_tab: 0,
            config: WorkspaceConfigReference {
                theme: Some("turmeric".into()),
                shell_program: Some("/bin/bash".into()),
            },
            automatic: false,
            closed_tab: false,
            tabs: vec![WorkspaceTabSnapshot {
                id: 7,
                title: "nested".into(),
                user_title: Some("My build".into()),
                pinned: true,
                primary_pane: 0,
                focused_pane: 2,
                layout: WorkspacePaneLayout::Split {
                    direction: SplitDirection::Vertical,
                    ratio: 0.37,
                    first: Box::new(WorkspacePaneLayout::Pane { pane: 0 }),
                    second: Box::new(WorkspacePaneLayout::Split {
                        direction: SplitDirection::Horizontal,
                        ratio: 0.62,
                        first: Box::new(WorkspacePaneLayout::Pane { pane: 1 }),
                        second: Box::new(WorkspacePaneLayout::Pane { pane: 2 }),
                    }),
                },
                panes: (0..3)
                    .map(|id| WorkspacePaneSnapshot {
                        id,
                        launch: WorkspaceLaunch::Shell,
                        title: Some(format!("pane-{id}")),
                        working_directory: Some(format!("/tmp/pane-{id}")),
                        last_command: Some(format!("command-{id}")),
                        scrollback_file: None,
                        scrollback_truncated: false,
                        scrollback_lines: vec![format!("history-{id}")],
                    })
                    .collect(),
            }],
        }
    }

    #[test]
    fn nested_asymmetric_workspace_is_valid_and_round_trips() {
        let workspace = sample_workspace();
        workspace.validate().unwrap();
        let encoded = toml::to_string_pretty(&workspace).unwrap();
        let decoded: WorkspaceSnapshot = toml::from_str(&encoded).unwrap();
        let mut expected = workspace;
        for tab in &mut expected.tabs {
            for pane in &mut tab.panes {
                pane.scrollback_lines.clear();
            }
        }
        assert_eq!(decoded, expected);
    }

    #[test]
    fn invalid_ids_ratios_focus_and_layout_are_rejected() {
        let mut workspace = sample_workspace();
        workspace.id = "../escape".into();
        assert!(workspace.validate().is_err());

        let mut workspace = sample_workspace();
        workspace.tabs[0].focused_pane = 99;
        assert!(workspace.validate().is_err());

        let mut workspace = sample_workspace();
        let WorkspacePaneLayout::Split { ratio, .. } = &mut workspace.tabs[0].layout else {
            unreachable!()
        };
        *ratio = f32::NAN;
        assert!(workspace.validate().is_err());

        let mut workspace = sample_workspace();
        workspace.tabs[0].panes.pop();
        assert!(workspace.validate().is_err());
    }

    #[test]
    fn launches_read_as_their_kind_and_name() {
        for (launch, words) in [
            (WorkspaceLaunch::Shell, "shell"),
            (
                WorkspaceLaunch::Profile { name: "dev".into() },
                "terminal profile \"dev\"",
            ),
            (
                WorkspaceLaunch::Plugin {
                    plugin_id: "codex".into(),
                    profile: None,
                },
                "plugin \"codex\"",
            ),
            (
                WorkspaceLaunch::Plugin {
                    plugin_id: "codex".into(),
                    profile: Some("review".into()),
                },
                "plugin \"codex\" profile \"review\"",
            ),
        ] {
            assert_eq!(launch.to_string(), words);
        }
    }

    #[test]
    fn scrollback_files_outside_the_scrollback_folder_are_rejected() {
        let mut workspace = sample_workspace();
        workspace.tabs[0].panes[0].scrollback_file = Some("scrollback/0123-4.txt".into());
        workspace.validate().unwrap();
        for hostile in [
            "../x.txt",
            "scrollback/../x.txt",
            "scrollback/../../x.txt",
            "/tmp/x.txt",
            "/scrollback/x.txt",
            "scrollback/nested/x.txt",
            "x.txt",
            "scrollback/x.toml",
        ] {
            workspace.tabs[0].panes[0].scrollback_file = Some(hostile.into());
            assert!(workspace.validate().is_err(), "accepted {hostile:?}");
        }
    }

    #[test]
    fn nonprimary_plugin_launch_identity_is_valid() {
        let mut workspace = sample_workspace();
        workspace.tabs[0].panes[1].launch = WorkspaceLaunch::Plugin {
            plugin_id: "fake".into(),
            profile: None,
        };
        assert!(workspace.validate().is_ok());
    }

    #[test]
    fn terminal_profile_workspace_names_match_config_identity_rules() {
        let mut workspace = sample_workspace();
        workspace.tabs[0].panes[0].launch = WorkspaceLaunch::Profile {
            name: "dev.rust".into(),
        };
        workspace.validate().unwrap();
        assert_eq!(
            workspace.tabs[0].launch().unwrap(),
            &WorkspaceLaunch::Profile {
                name: "dev.rust".into()
            }
        );

        for name in [".hidden", "two..dots", "bad name"] {
            workspace.tabs[0].panes[0].launch = WorkspaceLaunch::Profile { name: name.into() };
            assert!(workspace.validate().is_err(), "accepted {name:?}");
        }
    }
}
