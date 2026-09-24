//! Reading an agent's last answer from its own conversation log.
//!
//! These logs are internal to each agent and change between releases (Claude
//! Code documents its format as unstable), so the reader is deliberately
//! forgiving. It reads only the end of the file, skips any line it does not
//! understand, and gives up rather than guessing.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde_json::Value;

/// How much of the end of a log is examined.
const TAIL_BYTES: u64 = 4 * 1024 * 1024;

/// The most recent complete answer in the log at `path`.
pub fn last_response(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let start = length.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.take(TAIL_BYTES).read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = text.lines();
    if start > 0 {
        // The first line was cut by the seek.
        lines.next();
    }
    lines
        .rev()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find_map(|entry| response_in(&entry))
}

/// The answer an entry records, in either agent's log shape.
fn response_in(entry: &Value) -> Option<String> {
    // Codex: `{"type":"event_msg","payload":{"type":"task_complete",
    // "last_agent_message":"..."}}` closes every turn.
    if entry.get("type").and_then(Value::as_str) == Some("event_msg")
        && entry.pointer("/payload/type").and_then(Value::as_str) == Some("task_complete")
    {
        return non_empty(entry.pointer("/payload/last_agent_message")?.as_str()?);
    }
    // Claude Code: `{"type":"assistant","message":{"content":[{"type":"text",
    // "text":"..."}, ...]}}`; entries holding only tool calls are skipped.
    if entry.get("type").and_then(Value::as_str) == Some("assistant") {
        let text: Vec<&str> = entry
            .pointer("/message/content")?
            .as_array()?
            .iter()
            .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect();
        return non_empty(&text.join("\n\n"));
    }
    None
}

fn non_empty(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn log(lines: &[&str]) -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
        file
    }

    #[test]
    fn claude_logs_yield_the_last_text_answer() {
        let file = log(&[
            r#"{"type":"user","message":{"role":"user","content":"hi"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"First answer"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Part one"},{"type":"tool_use","name":"Bash"},{"type":"text","text":"Part two"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read"}]}}"#,
            r#"not json at all"#,
            r#"{"type":"system","subtype":"turn_duration"}"#,
        ]);
        assert_eq!(
            last_response(file.path()).as_deref(),
            Some("Part one\n\nPart two")
        );
    }

    #[test]
    fn codex_logs_yield_the_completed_turns_message() {
        let file = log(&[
            r#"{"timestamp":"t","type":"event_msg","payload":{"type":"task_complete","turn_id":"1","last_agent_message":"Old"}}"#,
            r#"{"timestamp":"t","type":"response_item","payload":{"type":"message","role":"assistant"}}"#,
            r#"{"timestamp":"t","type":"event_msg","payload":{"type":"task_complete","turn_id":"2","last_agent_message":"  Newest  "}}"#,
            r#"{"timestamp":"t","type":"event_msg","payload":{"type":"task_started"}}"#,
        ]);
        assert_eq!(last_response(file.path()).as_deref(), Some("Newest"));
    }

    #[test]
    fn unknown_or_missing_logs_give_nothing() {
        let file = log(&[r#"{"type":"assistant","message":{"content":"plain string"}}"#]);
        assert_eq!(last_response(file.path()), None);
        assert_eq!(last_response(Path::new("/definitely/missing.jsonl")), None);
    }
}
