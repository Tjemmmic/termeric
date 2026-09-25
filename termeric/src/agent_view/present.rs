//! How tool calls read in the panel: one-line summaries, grouping of quiet
//! exploration, diffs, to-do lists, questions, and plans.
//!
//! Everything here interprets tool input and results as Claude Code writes
//! them. Unknown tools and unexpected shapes fall back to the tool's name, so
//! a new tool shows plainly instead of disappearing.

use std::path::Path;

use serde_json::Value;

use super::transcript::{Item, ToolCall};

/// What kind of action a tool call is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolKind {
    Read,
    Search,
    Edit,
    Command,
    Web,
    SubAgent,
    Todo,
    Question,
    Plan,
    Other,
}

pub fn kind(name: &str) -> ToolKind {
    match name {
        "Read" | "NotebookRead" | "LS" => ToolKind::Read,
        "Grep" | "Glob" | "ToolSearch" => ToolKind::Search,
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => ToolKind::Edit,
        "Bash" | "BashOutput" | "KillShell" | "KillBash" => ToolKind::Command,
        "WebFetch" | "WebSearch" => ToolKind::Web,
        "Task" | "Agent" => ToolKind::SubAgent,
        "TodoWrite" => ToolKind::Todo,
        "AskUserQuestion" => ToolKind::Question,
        "ExitPlanMode" => ToolKind::Plan,
        _ => ToolKind::Other,
    }
}

/// A tool call in a few words: "Read" `app/mouse.rs` "lines 580–640".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    pub verb: String,
    /// The object, shown in the monospace face when it is code-like.
    pub subject: String,
    pub subject_is_code: bool,
    pub detail: Option<String>,
}

fn field<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    input.get(key).and_then(Value::as_str)
}

/// `path` relative to `cwd` when it is inside it.
pub fn relative(path: &str, cwd: &Path) -> String {
    Path::new(path)
        .strip_prefix(cwd)
        .ok()
        .filter(|relative| !relative.as_os_str().is_empty())
        .map_or_else(
            || path.to_owned(),
            |relative| relative.display().to_string(),
        )
}

pub fn summarize(tool: &ToolCall, cwd: &Path) -> Summary {
    let input = &tool.input;
    let path = |key: &str| field(input, key).map(|path| relative(path, cwd));
    let summary =
        |verb: &str, subject: Option<String>, code: bool, detail: Option<String>| Summary {
            verb: verb.to_owned(),
            subject: subject.unwrap_or_default(),
            subject_is_code: code,
            detail,
        };
    match tool.name.as_str() {
        "Read" => {
            let lines = match (
                input.get("offset").and_then(Value::as_u64),
                input.get("limit").and_then(Value::as_u64),
            ) {
                (Some(offset), Some(limit)) => Some(format!(
                    "lines {offset}–{}",
                    offset + limit.saturating_sub(1)
                )),
                (Some(offset), None) => Some(format!("from line {offset}")),
                _ => None,
            };
            summary("Read", path("file_path"), true, lines)
        }
        "LS" => summary("Listed", path("path"), true, None),
        "Grep" => summary(
            "Searched",
            field(input, "pattern").map(str::to_owned),
            true,
            path("path").map(|path| format!("in {path}")),
        ),
        "Glob" => summary(
            "Found files",
            field(input, "pattern").map(str::to_owned),
            true,
            None,
        ),
        "Edit" | "MultiEdit" => summary("Edited", path("file_path"), true, None),
        "Write" => summary("Wrote", path("file_path"), true, None),
        "NotebookEdit" => summary("Edited", path("notebook_path"), true, None),
        "Bash" => summary(
            "Ran",
            field(input, "command").map(first_line),
            true,
            field(input, "description").map(str::to_owned),
        ),
        "WebFetch" => summary(
            "Fetched",
            field(input, "url").map(str::to_owned),
            false,
            None,
        ),
        "WebSearch" => summary(
            "Searched the web",
            field(input, "query").map(str::to_owned),
            false,
            None,
        ),
        "Task" | "Agent" => summary(
            "Sub-agent",
            field(input, "description").map(str::to_owned),
            false,
            field(input, "subagent_type").map(str::to_owned),
        ),
        "TodoWrite" => summary("Updated to-dos", None, false, None),
        "AskUserQuestion" => summary("Asked", None, false, None),
        "ExitPlanMode" => summary("Plan", None, false, None),
        name => {
            // MCP tools are named mcp__server__tool.
            let mut parts = name.splitn(3, "__");
            match (parts.next(), parts.next(), parts.next()) {
                (Some("mcp"), Some(server), Some(tool)) => summary(
                    &tool.replace('_', " "),
                    None,
                    false,
                    Some(server.to_owned()),
                ),
                _ => summary(name, None, false, None),
            }
        }
    }
}

/// A tool's name as a person reads it; MCP tools show their tool and server.
pub fn summarize_name(name: &str) -> String {
    let mut parts = name.splitn(3, "__");
    match (parts.next(), parts.next(), parts.next()) {
        (Some("mcp"), Some(server), Some(tool)) => format!("{} ({server})", tool.replace('_', " ")),
        _ => name.to_owned(),
    }
}

fn first_line(text: &str) -> String {
    let mut lines = text.lines();
    let first = lines.next().unwrap_or_default().trim_end().to_owned();
    if lines.next().is_some() {
        format!("{first} …")
    } else {
        first
    }
}

/// Quiet, read-only calls that are grouped into one "Explored" row.
pub fn is_exploration(tool: &ToolCall) -> bool {
    matches!(kind(&tool.name), ToolKind::Read | ToolKind::Search)
}

/// One row of the conversation as the panel shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Item(usize),
    /// Consecutive exploration calls, shown as one collapsible row.
    Explored(Vec<usize>),
}

/// Groups `items` into rows. A tool call waiting for a decision is never
/// hidden inside a group.
pub fn entries(items: &[Item], awaiting: impl Fn(&str) -> bool) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut group: Vec<usize> = Vec::new();
    let flush = |group: &mut Vec<usize>, entries: &mut Vec<Entry>| match group.len() {
        0 => {}
        1 => entries.push(Entry::Item(group[0])),
        _ => entries.push(Entry::Explored(std::mem::take(group))),
    };
    for (index, item) in items.iter().enumerate() {
        match item {
            Item::Tool(tool) if is_exploration(tool) && !awaiting(&tool.id) => group.push(index),
            // Thinking between quiet steps does not break the group.
            Item::Thinking { .. } if !group.is_empty() => {}
            _ => {
                flush(&mut group, &mut entries);
                group.clear();
                entries.push(Entry::Item(index));
            }
        }
    }
    flush(&mut group, &mut entries);
    entries
}

/// "3 files, 1 search" for a group of exploration calls.
pub fn explored_summary<'a>(tools: impl Iterator<Item = &'a ToolCall>) -> String {
    let (mut files, mut searches) = (0, 0);
    for tool in tools {
        match kind(&tool.name) {
            ToolKind::Read => files += 1,
            _ => searches += 1,
        }
    }
    let count = |number: usize, one: &str, many: &str| {
        format!("{number} {}", if number == 1 { one } else { many })
    };
    match (files, searches) {
        (0, searches) => count(searches, "search", "searches"),
        (files, 0) => count(files, "file", "files"),
        (files, searches) => format!(
            "{}, {}",
            count(files, "file", "files"),
            count(searches, "search", "searches")
        ),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Added,
    Removed,
    /// A gap between hunks.
    Gap,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    /// The line number in the new file (or old, for removals), when known.
    pub number: Option<u32>,
    pub text: String,
}

/// The change a file tool made or proposes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    pub added: usize,
    pub removed: usize,
    pub lines: Vec<DiffLine>,
    /// A new file.
    pub created: bool,
}

/// Lines shown for a new file before the rest is summarized.
const MAX_CREATED_LINES: usize = 400;
/// Longest edit compared line by line; longer ones show whole blocks.
const MAX_COMPARED_LINES: usize = 600;

/// The diff for an edit, from the agent's own patch when it reported one,
/// otherwise from the proposed input (as when asking for approval).
pub fn file_diff(tool: &ToolCall, cwd: &Path) -> Option<FileDiff> {
    let path_key = if tool.name == "NotebookEdit" {
        "notebook_path"
    } else {
        "file_path"
    };
    let path = relative(field(&tool.input, path_key)?, cwd);
    let detail = tool
        .result
        .as_ref()
        .filter(|result| !result.is_error)
        .and_then(|result| result.detail.as_ref());
    if let Some(patch) = detail
        .and_then(|detail| detail.get("structuredPatch"))
        .and_then(Value::as_array)
        .filter(|patch| !patch.is_empty())
    {
        let lines = patch_lines(patch);
        return Some(counted(path, lines, false));
    }
    match tool.name.as_str() {
        "Edit" => {
            let old = field(&tool.input, "old_string")?;
            let new = field(&tool.input, "new_string")?;
            Some(counted(path, compare(old, new), false))
        }
        "MultiEdit" => {
            let mut lines = Vec::new();
            for edit in tool.input.get("edits")?.as_array()? {
                if !lines.is_empty() {
                    lines.push(DiffLine {
                        kind: DiffLineKind::Gap,
                        number: None,
                        text: String::new(),
                    });
                }
                lines.extend(compare(
                    field(edit, "old_string")?,
                    field(edit, "new_string")?,
                ));
            }
            Some(counted(path, lines, false))
        }
        "Write" => {
            let content = field(&tool.input, "content")?;
            let created = detail
                .and_then(|detail| field(detail, "type"))
                .is_none_or(|kind| kind == "create");
            let mut lines: Vec<DiffLine> = content
                .lines()
                .take(MAX_CREATED_LINES)
                .enumerate()
                .map(|(index, line)| DiffLine {
                    kind: DiffLineKind::Added,
                    number: Some(index as u32 + 1),
                    text: line.to_owned(),
                })
                .collect();
            let total = content.lines().count();
            let mut diff = counted(path, std::mem::take(&mut lines), created);
            diff.added = total;
            if total > MAX_CREATED_LINES {
                diff.lines.push(DiffLine {
                    kind: DiffLineKind::Gap,
                    number: None,
                    text: format!("{} more lines", total - MAX_CREATED_LINES),
                });
            }
            Some(diff)
        }
        _ => None,
    }
}

fn counted(path: String, lines: Vec<DiffLine>, created: bool) -> FileDiff {
    let added = lines
        .iter()
        .filter(|line| line.kind == DiffLineKind::Added)
        .count();
    let removed = lines
        .iter()
        .filter(|line| line.kind == DiffLineKind::Removed)
        .count();
    FileDiff {
        path,
        added,
        removed,
        lines,
        created,
    }
}

fn patch_lines(patch: &[Value]) -> Vec<DiffLine> {
    let mut lines = Vec::new();
    for (index, hunk) in patch.iter().enumerate() {
        let number = |key: &str| {
            hunk.get(key)
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .unwrap_or(1)
        };
        let (mut old, mut new) = (number("oldStart"), number("newStart"));
        if index > 0 {
            lines.push(DiffLine {
                kind: DiffLineKind::Gap,
                number: None,
                text: String::new(),
            });
        }
        for line in hunk
            .get("lines")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            let (kind, text) = match line.chars().next() {
                Some('+') => (DiffLineKind::Added, &line[1..]),
                Some('-') => (DiffLineKind::Removed, &line[1..]),
                Some(' ') => (DiffLineKind::Context, &line[1..]),
                // "\ No newline at end of file" and anything unexpected.
                _ => continue,
            };
            let shown = match kind {
                DiffLineKind::Removed => {
                    old += 1;
                    old - 1
                }
                DiffLineKind::Added => {
                    new += 1;
                    new - 1
                }
                _ => {
                    old += 1;
                    new += 1;
                    new - 1
                }
            };
            lines.push(DiffLine {
                kind,
                number: Some(shown),
                text: text.to_owned(),
            });
        }
    }
    lines
}

/// A line-level comparison of two snippets, without line numbers.
fn compare(old: &str, new: &str) -> Vec<DiffLine> {
    let old: Vec<&str> = old.lines().collect();
    let new: Vec<&str> = new.lines().collect();
    let line = |kind, text: &str| DiffLine {
        kind,
        number: None,
        text: text.to_owned(),
    };
    if old.len() > MAX_COMPARED_LINES || new.len() > MAX_COMPARED_LINES {
        return old
            .iter()
            .map(|text| line(DiffLineKind::Removed, text))
            .chain(new.iter().map(|text| line(DiffLineKind::Added, text)))
            .collect();
    }
    // Longest common subsequence table, from the end.
    let mut table = vec![vec![0u32; new.len() + 1]; old.len() + 1];
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            table[i][j] = if old[i] == new[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut lines = Vec::new();
    while i < old.len() || j < new.len() {
        if i < old.len() && j < new.len() && old[i] == new[j] {
            lines.push(line(DiffLineKind::Context, old[i]));
            i += 1;
            j += 1;
        } else if j < new.len() && (i == old.len() || table[i][j + 1] >= table[i + 1][j]) {
            lines.push(line(DiffLineKind::Added, new[j]));
            j += 1;
        } else {
            lines.push(line(DiffLineKind::Removed, old[i]));
            i += 1;
        }
    }
    // Removals read before additions within each changed run.
    let mut ordered = Vec::with_capacity(lines.len());
    let mut added = Vec::new();
    for diff_line in lines {
        match diff_line.kind {
            DiffLineKind::Added => added.push(diff_line),
            DiffLineKind::Removed => ordered.push(diff_line),
            _ => {
                ordered.append(&mut added);
                ordered.push(diff_line);
            }
        }
    }
    ordered.append(&mut added);
    ordered
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Todo {
    pub text: String,
    pub status: TodoStatus,
}

pub fn todos(tool: &ToolCall) -> Option<Vec<Todo>> {
    let list = tool.input.get("todos")?.as_array()?;
    Some(
        list.iter()
            .filter_map(|todo| {
                let status = match field(todo, "status")? {
                    "completed" => TodoStatus::Completed,
                    "in_progress" => TodoStatus::InProgress,
                    _ => TodoStatus::Pending,
                };
                let text = match status {
                    TodoStatus::InProgress => field(todo, "activeForm").or(field(todo, "content")),
                    _ => field(todo, "content"),
                }?;
                Some(Todo {
                    text: text.to_owned(),
                    status,
                })
            })
            .collect(),
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestionOption {
    pub label: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Question {
    pub header: String,
    pub question: String,
    pub options: Vec<QuestionOption>,
    pub multiple: bool,
}

pub fn questions(input: &Value) -> Vec<Question> {
    input
        .get("questions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|question| {
            Some(Question {
                header: field(question, "header").unwrap_or_default().to_owned(),
                question: field(question, "question")?.to_owned(),
                options: question
                    .get("options")?
                    .as_array()?
                    .iter()
                    .filter_map(|option| {
                        Some(QuestionOption {
                            label: field(option, "label")?.to_owned(),
                            description: field(option, "description")
                                .unwrap_or_default()
                                .to_owned(),
                        })
                    })
                    .collect(),
                multiple: question
                    .get("multiSelect")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
        })
        .collect()
}

/// The answers a finished question call recorded, question → answer.
pub fn recorded_answers(tool: &ToolCall) -> Vec<(String, String)> {
    tool.input
        .get("answers")
        .and_then(Value::as_object)
        .map(|answers| {
            answers
                .iter()
                .filter_map(|(question, answer)| {
                    Some((question.clone(), answer.as_str()?.to_owned()))
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn plan_text(input: &Value) -> Option<&str> {
    field(input, "plan").filter(|plan| !plan.trim().is_empty())
}

/// One approval request in words: the tool, what it acts on, and why.
pub fn approval_text(approval: &crate::agent_view::event::ApprovalRequest, cwd: &Path) -> String {
    let subject = approval
        .input
        .get("command")
        .or_else(|| approval.input.get("file_path"))
        .or_else(|| approval.input.get("url"))
        .or_else(|| approval.input.get("query"))
        .and_then(Value::as_str)
        .map(|subject| relative(subject, cwd))
        .unwrap_or_default();
    let mut text = summarize_name(&approval.tool_name);
    if !subject.is_empty() {
        text.push(' ');
        text.push_str(&subject);
    }
    if let Some(description) = &approval.description {
        text.push_str(&format!(" ({description})"));
    }
    text
}

/// The conversation as plain text for another agent to read, newest last,
/// within `max_bytes`, and whether older parts were left out. Pending
/// approvals come last with their ids, so a reader can answer them.
pub fn conversation_text(
    items: &[Item],
    approvals: &[crate::agent_view::event::ApprovalRequest],
    cwd: &Path,
    max_bytes: usize,
) -> (String, bool) {
    const MESSAGE_LIMIT: usize = 4000;
    let cut = |text: &str| -> String {
        let text = text.trim();
        match text.char_indices().nth(MESSAGE_LIMIT) {
            Some((end, _)) => format!("{}…", &text[..end]),
            None => text.to_owned(),
        }
    };
    let mut parts: Vec<String> = items
        .iter()
        .filter_map(|item| match item {
            Item::User { text, attachments } => {
                let mut part = format!("Person: {}", cut(text));
                for attachment in attachments {
                    part.push_str(&format!(" [attached: {}]", attachment.label));
                }
                Some(part)
            }
            Item::Text { text, .. } if !text.trim().is_empty() => {
                Some(format!("Claude: {}", cut(text)))
            }
            Item::Tool(tool) => {
                let summary = summarize(tool, cwd);
                let state = match tool.state(false) {
                    super::transcript::ToolState::Succeeded => "done",
                    super::transcript::ToolState::Failed => "failed",
                    super::transcript::ToolState::Denied => "denied",
                    super::transcript::ToolState::Stopped => "stopped",
                    _ => "running",
                };
                Some(format!(
                    "Tool: {} ({state})",
                    cut(&format!("{} {}", summary.verb, summary.subject))
                ))
            }
            Item::TurnEnd(end) => Some(match (&end.outcome.error, end.interrupted) {
                (_, true) => "Turn stopped by the person.".to_owned(),
                (Some(error), false) => format!("Turn failed: {error}"),
                (None, false) => "Turn finished.".to_owned(),
            }),
            Item::Notice(notice) => Some(format!("Notice: {}", cut(&notice.text))),
            _ => None,
        })
        .collect();
    for approval in approvals {
        parts.push(format!(
            "Waiting for approval {}: {}",
            approval.request_id,
            cut(&approval_text(approval, cwd))
        ));
    }
    // Keep the newest parts that fit. The newest is kept even when it alone
    // is too long, cut at its end, so a pending approval's id survives.
    let mut kept: Vec<String> = Vec::new();
    let mut used = 0;
    let mut truncated = false;
    for part in parts.into_iter().rev() {
        if used + part.len() + 1 > max_bytes {
            truncated = true;
            if kept.is_empty() {
                let mut end = max_bytes.saturating_sub('…'.len_utf8()).min(part.len());
                while !part.is_char_boundary(end) {
                    end -= 1;
                }
                kept.push(format!("{}…", &part[..end]));
            }
            break;
        }
        used += part.len() + 1;
        kept.push(part);
    }
    kept.reverse();
    (kept.join("\n"), truncated)
}

/// The last `limit` lines of a command's output, and how many were left out.
pub fn output_tail(text: &str, limit: usize) -> (Vec<&str>, usize) {
    let lines: Vec<&str> = text.trim_end().lines().collect();
    let skipped = lines.len().saturating_sub(limit);
    (lines[skipped..].to_vec(), skipped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_view::event::ToolResult;
    use serde_json::json;

    fn tool(name: &str, input: Value) -> ToolCall {
        ToolCall {
            id: format!("{name}-id"),
            name: name.into(),
            input,
            partial_input: String::new(),
            result: None,
            decision: None,
            withdrawn: false,
            sub_agent: Default::default(),
            abandoned: false,
        }
    }

    #[test]
    fn summaries_use_relative_paths_and_short_commands() {
        let cwd = Path::new("/work/termeric");
        let read = summarize(
            &tool(
                "Read",
                json!({"file_path": "/work/termeric/src/app/mouse.rs", "offset": 580, "limit": 61}),
            ),
            cwd,
        );
        assert_eq!(read.verb, "Read");
        assert_eq!(read.subject, "src/app/mouse.rs");
        assert_eq!(read.detail.as_deref(), Some("lines 580–640"));
        let outside = summarize(&tool("Read", json!({"file_path": "/etc/hosts"})), cwd);
        assert_eq!(outside.subject, "/etc/hosts");
        let bash = summarize(
            &tool(
                "Bash",
                json!({"command": "cargo test\n  --workspace", "description": "Run tests"}),
            ),
            cwd,
        );
        assert_eq!(bash.subject, "cargo test …");
        assert_eq!(bash.detail.as_deref(), Some("Run tests"));
        let mcp = summarize(&tool("mcp__termeric__read_terminal", json!({})), cwd);
        assert_eq!(mcp.verb, "read terminal");
        assert_eq!(mcp.detail.as_deref(), Some("termeric"));
        assert_eq!(summarize(&tool("Mystery", json!({})), cwd).verb, "Mystery");
        assert_eq!(
            summarize_name("mcp__termeric__read_terminal"),
            "read terminal (termeric)"
        );
        assert_eq!(summarize_name("Bash"), "Bash");
    }

    #[test]
    fn quiet_exploration_groups_until_something_else_happens() {
        let items = vec![
            Item::Tool(tool("Read", json!({}))),
            Item::Thinking {
                message_id: "m".into(),
                text: "…".into(),
                streaming: false,
                confirmed: true,
            },
            Item::Tool(tool("Grep", json!({}))),
            Item::Tool(tool("Edit", json!({}))),
            Item::Tool(tool("Read", json!({}))),
            Item::Tool(tool("Glob", json!({}))),
        ];
        assert_eq!(
            entries(&items, |_| false),
            [
                Entry::Explored(vec![0, 2]),
                Entry::Item(3),
                Entry::Explored(vec![4, 5])
            ]
        );
        // A call waiting for approval stays visible on its own.
        assert_eq!(
            entries(&items, |id| id == "Glob-id"),
            [
                Entry::Explored(vec![0, 2]),
                Entry::Item(3),
                Entry::Item(4),
                Entry::Item(5)
            ]
        );
        let tools = [
            tool("Read", json!({})),
            tool("Read", json!({})),
            tool("Grep", json!({})),
        ];
        assert_eq!(explored_summary(tools.iter()), "2 files, 1 search");
        assert_eq!(explored_summary(tools[2..].iter()), "1 search");
    }

    #[test]
    fn reported_patches_carry_line_numbers() {
        let mut edit = tool(
            "Edit",
            json!({"file_path": "/w/a.rs", "old_string": "x", "new_string": "y"}),
        );
        edit.result = Some(ToolResult {
            tool_use_id: "Edit-id".into(),
            content: "ok".into(),
            is_error: false,
            detail: Some(json!({"structuredPatch": [
                {"oldStart": 10, "newStart": 10, "lines": [" a", "-b", "+c", "+d", " e"]},
                {"oldStart": 40, "newStart": 41, "lines": [" z", "\\ No newline at end of file"]}
            ]})),
        });
        let diff = file_diff(&edit, Path::new("/w")).unwrap();
        assert_eq!(diff.path, "a.rs");
        assert_eq!((diff.added, diff.removed), (2, 1));
        let numbered: Vec<_> = diff
            .lines
            .iter()
            .map(|line| (line.kind, line.number, line.text.as_str()))
            .collect();
        assert_eq!(
            numbered,
            [
                (DiffLineKind::Context, Some(10), "a"),
                (DiffLineKind::Removed, Some(11), "b"),
                (DiffLineKind::Added, Some(11), "c"),
                (DiffLineKind::Added, Some(12), "d"),
                (DiffLineKind::Context, Some(13), "e"),
                (DiffLineKind::Gap, None, ""),
                (DiffLineKind::Context, Some(41), "z"),
            ]
        );
    }

    #[test]
    fn proposed_edits_compare_lines_and_keep_context() {
        let edit = tool(
            "Edit",
            json!({"file_path": "/w/a.rs", "old_string": "one\ntwo\nthree", "new_string": "one\n2\nthree\nfour"}),
        );
        let diff = file_diff(&edit, Path::new("/w")).unwrap();
        let kinds: Vec<_> = diff
            .lines
            .iter()
            .map(|line| (line.kind, line.text.as_str()))
            .collect();
        assert_eq!(
            kinds,
            [
                (DiffLineKind::Context, "one"),
                (DiffLineKind::Removed, "two"),
                (DiffLineKind::Added, "2"),
                (DiffLineKind::Context, "three"),
                (DiffLineKind::Added, "four"),
            ]
        );
        let write = tool(
            "Write",
            json!({"file_path": "/w/new.rs", "content": "a\nb\n"}),
        );
        let created = file_diff(&write, Path::new("/w")).unwrap();
        assert!(created.created);
        assert_eq!(created.added, 2);
        assert!(file_diff(&tool("Read", json!({"file_path": "/w/a"})), Path::new("/w")).is_none());
    }

    #[test]
    fn conversation_text_keeps_the_newest_parts_and_names_pending_approvals() {
        let mut bash = tool("Bash", json!({"command": "cargo test"}));
        bash.result = Some(ToolResult {
            tool_use_id: "Bash-id".into(),
            content: "ok".into(),
            is_error: false,
            detail: None,
        });
        let items = vec![
            Item::User {
                text: "old question".into(),
                attachments: Vec::new(),
            },
            Item::Tool(bash),
            Item::Text {
                message_id: "m".into(),
                text: "All green.".into(),
                streaming: false,
                confirmed: true,
            },
        ];
        let approvals = vec![crate::agent_view::event::ApprovalRequest {
            request_id: "req-1".into(),
            tool_use_id: None,
            tool_name: "Bash".into(),
            input: json!({"command": "rm -rf target"}),
            title: None,
            description: Some("Clean the build".into()),
            reason: None,
            remember: None,
            from_sub_agent: false,
        }];
        let (text, truncated) = conversation_text(&items, &approvals, Path::new("/w"), 10_000);
        assert!(!truncated);
        assert_eq!(
            text,
            "Person: old question\nTool: Ran cargo test (done)\nClaude: All green.\n\
             Waiting for approval req-1: Bash rm -rf target (Clean the build)"
        );
        let (short, truncated) = conversation_text(&items, &approvals, Path::new("/w"), 90);
        assert!(truncated);
        assert!(!short.contains("old question"));
        assert!(short.ends_with("(Clean the build)"));
        // A newest part longer than everything allowed keeps its start.
        let (tiny, truncated) = conversation_text(&items, &approvals, Path::new("/w"), 40);
        assert!(truncated);
        assert!(tiny.starts_with("Waiting for approval req-1: "));
        assert!(tiny.len() <= 40 && tiny.ends_with('…'));
        // A huge command is cut like any message.
        let mut huge = approvals.clone();
        huge[0].input = json!({"command": "é".repeat(100_000)});
        let (text, _) = conversation_text(&items, &huge, Path::new("/w"), 1_000_000);
        assert!(text.len() < 20_000);
    }

    #[test]
    fn todos_questions_and_plans_are_read_from_input() {
        let todo = tool(
            "TodoWrite",
            json!({"todos": [
                {"content": "Read", "status": "completed", "activeForm": "Reading"},
                {"content": "Fix", "status": "in_progress", "activeForm": "Fixing"},
                {"content": "Test", "status": "pending", "activeForm": "Testing"}
            ]}),
        );
        let list = todos(&todo).unwrap();
        assert_eq!(list[1].text, "Fixing");
        assert_eq!(list[1].status, TodoStatus::InProgress);
        assert_eq!(list[2].status, TodoStatus::Pending);

        let asked = questions(&json!({"questions": [{
            "question": "Which?", "header": "Pick", "multiSelect": true,
            "options": [{"label": "A", "description": "first"}, {"label": "B"}]
        }]}));
        assert_eq!(asked[0].options.len(), 2);
        assert!(asked[0].multiple);
        let mut answered = tool("AskUserQuestion", json!({"answers": {"Which?": "A"}}));
        assert_eq!(recorded_answers(&answered), [("Which?".into(), "A".into())]);
        answered.input = json!({});
        assert!(recorded_answers(&answered).is_empty());

        assert_eq!(plan_text(&json!({"plan": "## Steps"})), Some("## Steps"));
        assert_eq!(plan_text(&json!({"plan": "  "})), None);
        assert_eq!(output_tail("a\nb\nc\n", 2), (vec!["b", "c"], 1));
    }
}
