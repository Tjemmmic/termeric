//! What the workspace panel lists, read from disk: saved workspaces or
//! closed-tab snapshots.

use crate::ui::session_panel::SessionEntry;

/// One reading of the panel's list.
pub struct SessionListing {
    /// Saved workspaces rather than closed-tab snapshots.
    pub workspaces: bool,
    pub entries: Vec<SessionEntry>,
    /// Closed-tab snapshots, counted for the workspace list's link to them.
    pub snapshot_count: usize,
}

impl SessionListing {
    pub fn workspaces() -> Self {
        Self::load(true)
    }

    pub fn closed_tabs() -> Self {
        Self::load(false)
    }

    pub fn load(workspaces: bool) -> Self {
        let now = super::unix_now();
        let (archives, snapshot_count) = if workspaces {
            (
                crate::workspace::list_workspaces(),
                crate::workspace::list_closed_tabs().len(),
            )
        } else {
            (crate::workspace::list_closed_tabs(), 0)
        };
        Self {
            workspaces,
            entries: archives
                .into_iter()
                .map(|archive| entry(archive, workspaces, now))
                .collect(),
            snapshot_count,
        }
    }
}

/// One listed archive. A workspace counts its tabs and panes; a closed tab,
/// already named for its directory and command, counts only extra panes.
fn entry(archive: crate::workspace::WorkspaceListEntry, workspace: bool, now: u64) -> SessionEntry {
    use crate::ui::text::plural;
    let subtitle = archive.warning.unwrap_or_else(|| {
        if workspace {
            format!(
                "{} · {}",
                plural(archive.tab_count, "tab", "tabs"),
                plural(archive.pane_count, "pane", "panes"),
            )
        } else if archive.pane_count > 1 {
            format!("{} panes", archive.pane_count)
        } else {
            String::new()
        }
    });
    SessionEntry {
        id: archive.id,
        display_name: archive.name,
        subtitle,
        last_modified: super::describe_age(&archive.updated_at, now).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_tabs_list_apart_from_workspaces_newest_first() {
        let root = tempfile::tempdir().unwrap();
        crate::workspace::set_test_workspaces_base_dir(Some(root.path().to_owned()));
        let tab = |id: &str, closed_tab: bool, panes: u32| {
            let mut workspace = crate::workspace::WorkspaceSnapshot::new(
                id.into(),
                format!("~/src | {id}"),
                vec![crate::workspace::WorkspaceTabSnapshot {
                    id: 0,
                    title: "tab".into(),
                    user_title: None,
                    pinned: false,
                    primary_pane: 0,
                    focused_pane: 0,
                    layout: (1..panes).fold(
                        crate::workspace::WorkspacePaneLayout::Pane { pane: 0 },
                        |first, pane| crate::workspace::WorkspacePaneLayout::Split {
                            direction: crate::panes::SplitDirection::Vertical,
                            ratio: 0.5,
                            first: Box::new(first),
                            second: Box::new(crate::workspace::WorkspacePaneLayout::Pane { pane }),
                        },
                    ),
                    panes: (0..panes)
                        .map(|pane| crate::workspace::WorkspacePaneSnapshot {
                            id: pane,
                            launch: crate::workspace::WorkspaceLaunch::Shell,
                            title: None,
                            working_directory: None,
                            last_command: None,
                            scrollback_file: None,
                            scrollback_truncated: false,
                            scrollback_lines: Vec::new(),
                        })
                        .collect(),
                }],
            );
            workspace.closed_tab = closed_tab;
            workspace.updated_at = format!("2026-01-0{}T00:00:00Z", panes + 1);
            crate::workspace::save_workspace(&mut workspace).unwrap();
        };
        tab("workspace-a", true, 1);
        tab("workspace-b", true, 2);
        tab("workspace-c", false, 1);
        let workspaces = SessionListing::workspaces();
        let closed_tabs = SessionListing::closed_tabs();
        crate::workspace::set_test_workspaces_base_dir(None);
        assert_eq!(workspaces.snapshot_count, 2);
        assert_eq!(workspaces.entries.len(), 1);
        assert_eq!(workspaces.entries[0].id, "workspace-c");
        let ids: Vec<_> = closed_tabs
            .entries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect();
        assert_eq!(ids, ["workspace-b", "workspace-a"]);
        assert_eq!(closed_tabs.entries[0].subtitle, "2 panes");
        assert_eq!(closed_tabs.entries[1].subtitle, "");
        assert_eq!(closed_tabs.entries[1].display_name, "~/src | workspace-a");
    }
}
