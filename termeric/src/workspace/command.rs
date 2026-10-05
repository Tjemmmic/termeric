//! `termeric workspace` commands, which read the saved archives directly.

use super::WorkspaceListEntry;

/// Longest load failure a text listing shows for an unreadable archive.
const MAX_WARNING_CHARS: usize = 512;

/// One `termeric workspace` command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceCommand {
    List { json: bool },
}

/// Run `command` and return what it prints.
pub fn execute(command: WorkspaceCommand) -> anyhow::Result<String> {
    match command {
        WorkspaceCommand::List { json } => render_list(&super::list_workspaces(), json),
    }
}

/// `workspaces`, newest first, as JSON or as one aligned row each. An
/// archive that can't be read is listed with why, so it can be found and
/// deleted. A name may hold any character but a line break, so the text
/// listing shows control characters as U+FFFD rather than sending them to
/// the terminal.
fn render_list(workspaces: &[WorkspaceListEntry], json: bool) -> anyhow::Result<String> {
    if json {
        let values: Vec<_> = workspaces
            .iter()
            .map(|workspace| {
                serde_json::json!({
                    "id": workspace.id,
                    "name": workspace.name,
                    "tabs": workspace.tab_count,
                    "panes": workspace.pane_count,
                    "updated_at": workspace.updated_at,
                    "warning": workspace.warning,
                })
            })
            .collect();
        return Ok(serde_json::to_string_pretty(&values)?);
    }
    if workspaces.is_empty() {
        return Ok("No saved workspaces".into());
    }
    let rows: Vec<[String; 4]> = workspaces
        .iter()
        .map(|workspace| match &workspace.warning {
            None => [
                workspace.id.clone(),
                workspace.tab_count.to_string(),
                whole_seconds(&workspace.updated_at),
                crate::safe_text::display_line(&workspace.name, super::MAX_WORKSPACE_NAME_CHARS, 1),
            ],
            Some(warning) => [
                workspace.id.clone(),
                "-".into(),
                "-".into(),
                format!(
                    "(unreadable: {})",
                    crate::safe_text::display_line(warning, MAX_WARNING_CHARS, 1)
                ),
            ],
        })
        .collect();
    let header = ["ID", "TABS", "UPDATED", "NAME"].map(str::to_owned);
    let widths: Vec<usize> = (0..3)
        .map(|column| {
            std::iter::once(&header)
                .chain(&rows)
                .map(|row| row[column].chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    Ok(std::iter::once(&header)
        .chain(&rows)
        .map(|row| {
            format!(
                "{:<id$}  {:>tabs$}  {:<updated$}  {}",
                row[0],
                row[1],
                row[2],
                row[3],
                id = widths[0],
                tabs = widths[1],
                updated = widths[2],
            )
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

/// `timestamp` without its fraction of a second, which only orders saves.
fn whole_seconds(timestamp: &str) -> String {
    match timestamp.split_once('.') {
        Some((whole, _)) => format!("{whole}Z"),
        None => timestamp.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, name: &str, tabs: usize, warning: Option<&str>) -> WorkspaceListEntry {
        WorkspaceListEntry {
            id: id.into(),
            name: name.into(),
            updated_at: "2026-10-04T09:30:00.250Z".into(),
            tab_count: tabs,
            pane_count: tabs + 1,
            warning: warning.map(str::to_owned),
        }
    }

    #[test]
    fn lists_align_their_columns_and_keep_unreadable_archives() {
        let workspaces = [
            entry("0123456789abcdef0123456789abcdef", "Project A", 12, None),
            entry("fedcba9876543210fedcba9876543210", "notes", 1, None),
            entry("00000000000000000000000000000000", "x", 0, Some("bad toml")),
        ];
        assert_eq!(
            render_list(&workspaces, false).unwrap(),
            [
                "ID                                TABS  UPDATED               NAME",
                "0123456789abcdef0123456789abcdef    12  2026-10-04T09:30:00Z  Project A",
                "fedcba9876543210fedcba9876543210     1  2026-10-04T09:30:00Z  notes",
                "00000000000000000000000000000000     -  -                     (unreadable: bad toml)",
            ]
            .join("\n")
        );
        assert_eq!(render_list(&[], false).unwrap(), "No saved workspaces");
        let hostile = render_list(&[entry("0123", "a\u{1b}]0;x\u{7}", 1, None)], false).unwrap();
        assert!(!hostile.contains(['\u{1b}', '\u{7}']), "{hostile:?}");
    }

    #[test]
    fn json_lists_every_field() {
        let json = render_list(&[entry("0123", "Project A", 2, None)], true).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            value,
            serde_json::json!([{
                "id": "0123",
                "name": "Project A",
                "tabs": 2,
                "panes": 3,
                "updated_at": "2026-10-04T09:30:00.250Z",
                "warning": null,
            }])
        );
        assert_eq!(render_list(&[], true).unwrap(), "[]");
    }
}
