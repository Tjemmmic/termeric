//! The toolbar Workspaces menu: recent workspaces, then save and manage.

use crate::ui::popup_menu::{MenuItem, MenuRow};
use crate::ui::text::plural;
use crate::workspace::WorkspaceListEntry;

/// Recent workspaces listed before the actions; the manager shows the rest.
pub const MAX_RECENT_WORKSPACES: usize = 6;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceMenuAction {
    Open(String),
    Save,
    SaveAs,
    Manage,
}

/// What a live-domain window shows beside the rows it can't use: its tabs
/// are the domain's, which keeps them itself, so it neither saves nor opens
/// workspaces.
const LIVE_DOMAIN_DETAIL: &str = "Live domain";

/// Rows for `workspaces`, which arrive most recently updated first. The
/// current workspace is checked; an unreadable archive stays visible but
/// disabled so the manager remains the place to inspect or delete it. In a
/// `live_domain` window, opening and saving are disabled.
pub fn workspace_menu_rows(
    workspaces: &[WorkspaceListEntry],
    current: Option<&str>,
    live_domain: bool,
) -> Vec<MenuRow<WorkspaceMenuAction>> {
    let mut rows: Vec<_> = workspaces
        .iter()
        .take(MAX_RECENT_WORKSPACES)
        .map(|workspace| {
            let readable = workspace.warning.is_none();
            MenuRow::Item(MenuItem {
                label: workspace.name.clone(),
                detail: Some(if !readable {
                    "Unavailable".into()
                } else if live_domain {
                    LIVE_DOMAIN_DETAIL.into()
                } else {
                    plural(workspace.tab_count, "tab", "tabs")
                }),
                checked: current == Some(workspace.id.as_str()),
                enabled: readable && !live_domain,
                action: WorkspaceMenuAction::Open(workspace.id.clone()),
            })
        })
        .collect();
    if !rows.is_empty() {
        rows.push(MenuRow::Separator);
    }
    for (label, action, savable) in [
        ("Save Workspace", WorkspaceMenuAction::Save, true),
        ("Save Workspace As…", WorkspaceMenuAction::SaveAs, true),
        ("Manage Workspaces…", WorkspaceMenuAction::Manage, false),
    ] {
        let disabled = savable && live_domain;
        rows.push(MenuRow::Item(MenuItem {
            label: label.into(),
            detail: disabled.then(|| LIVE_DOMAIN_DETAIL.into()),
            checked: false,
            enabled: !disabled,
            action,
        }));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(id: &str, tabs: usize, warning: bool) -> WorkspaceListEntry {
        WorkspaceListEntry {
            id: id.into(),
            name: format!("Workspace {id}"),
            updated_at: "2026-09-23T00:00:00Z".into(),
            tab_count: tabs,
            pane_count: tabs,
            warning: warning.then(|| "archive unavailable".into()),
        }
    }

    fn labels(rows: &[MenuRow<WorkspaceMenuAction>]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                MenuRow::Item(item) => item.label.clone(),
                MenuRow::Separator => "—".into(),
            })
            .collect()
    }

    #[test]
    fn without_workspaces_only_the_actions_remain() {
        let rows = workspace_menu_rows(&[], None, false);
        assert_eq!(
            labels(&rows),
            ["Save Workspace", "Save Workspace As…", "Manage Workspaces…"]
        );
    }

    #[test]
    fn recent_workspaces_precede_a_separator_and_mark_the_current_one() {
        let workspaces = [workspace("a", 3, false), workspace("b", 1, false)];
        let rows = workspace_menu_rows(&workspaces, Some("b"), false);
        assert_eq!(
            labels(&rows),
            [
                "Workspace a",
                "Workspace b",
                "—",
                "Save Workspace",
                "Save Workspace As…",
                "Manage Workspaces…"
            ]
        );
        let MenuRow::Item(first) = &rows[0] else {
            panic!("expected an item")
        };
        assert_eq!(first.detail.as_deref(), Some("3 tabs"));
        assert!(!first.checked);
        let MenuRow::Item(second) = &rows[1] else {
            panic!("expected an item")
        };
        assert_eq!(second.detail.as_deref(), Some("1 tab"));
        assert!(second.checked);
        assert_eq!(second.action, WorkspaceMenuAction::Open("b".into()));
    }

    #[test]
    fn unreadable_archives_are_listed_but_cannot_be_opened() {
        let rows = workspace_menu_rows(&[workspace("broken", 0, true)], None, false);
        let MenuRow::Item(item) = &rows[0] else {
            panic!("expected an item")
        };
        assert!(!item.enabled);
        assert_eq!(item.detail.as_deref(), Some("Unavailable"));
    }

    #[test]
    fn at_most_six_recent_workspaces_are_listed() {
        let workspaces: Vec<_> = (0..9)
            .map(|index| workspace(&index.to_string(), 1, false))
            .collect();
        let rows = workspace_menu_rows(&workspaces, None, false);
        assert_eq!(rows.len(), MAX_RECENT_WORKSPACES + 4);
    }

    #[test]
    fn a_live_domain_window_shows_saving_and_opening_disabled() {
        let rows = workspace_menu_rows(&[workspace("a", 2, false)], Some("a"), true);
        let items: Vec<_> = rows
            .iter()
            .filter_map(|row| match row {
                MenuRow::Item(item) => {
                    Some((item.label.as_str(), item.enabled, item.detail.as_deref()))
                }
                MenuRow::Separator => None,
            })
            .collect();
        assert_eq!(
            items,
            [
                ("Workspace a", false, Some("Live domain")),
                ("Save Workspace", false, Some("Live domain")),
                ("Save Workspace As…", false, Some("Live domain")),
                ("Manage Workspaces…", true, None),
            ]
        );
    }
}
