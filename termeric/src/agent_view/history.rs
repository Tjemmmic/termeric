//! Earlier Claude Code conversations for a folder, so they can be continued.
//!
//! Claude Code keeps one log per conversation under
//! `~/.claude/projects/<folder>/<session id>.jsonl`, where `<folder>` is the
//! working directory with every character but letters and digits replaced by
//! `-`. The logs are internal to Claude Code, so reading is forgiving: only
//! the file name, modification time, and a title are taken, and a log that
//! cannot be read is skipped.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::Value;

use super::event::EarlierConversation;

/// How much of a log's start is searched for its first prompt.
const HEAD_BYTES: u64 = 256 * 1024;
/// How much of a log's end is searched for a title.
const TAIL_BYTES: u64 = 64 * 1024;
/// Longest title kept.
const TITLE_CHARS: usize = 80;

/// The log folder name Claude Code uses for a working directory.
pub fn project_folder_name(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect()
}

/// The most recent conversations started in `cwd`, newest first, from the
/// logs in Claude Code's configuration directory `claude_dir`.
pub fn past_conversations(claude_dir: &Path, cwd: &Path, limit: usize) -> Vec<EarlierConversation> {
    past_conversations_in(
        &claude_dir.join("projects").join(project_folder_name(cwd)),
        limit,
    )
}

fn past_conversations_in(folder: &Path, limit: usize) -> Vec<EarlierConversation> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut logs: Vec<(SystemTime, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            if path
                .extension()
                .is_none_or(|extension| extension != "jsonl")
            {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            metadata
                .is_file()
                .then(|| (metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH), path))
        })
        .collect();
    logs.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    logs.into_iter()
        .filter_map(|(modified, path)| {
            let id = path.file_stem()?.to_str()?.to_owned();
            if !is_session_id(&id) {
                return None;
            }
            let title = title_of(&path)?;
            Some(EarlierConversation {
                id,
                title,
                modified,
            })
        })
        .take(limit)
        .collect()
}

/// Whether a log's name can be Claude Code's session id. The id becomes a
/// command-line argument, so a file named like an option must never pass.
fn is_session_id(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id.starts_with(|character: char| character.is_ascii_alphanumeric())
        && id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
}

/// A conversation's title: one Claude Code or the person gave it, else its
/// first prompt. A log with neither holds no conversation.
fn title_of(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    if let Some(title) = read_tail(&mut file, length).and_then(|tail| named_title(&tail)) {
        return Some(shorten(&title));
    }
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut head = Vec::new();
    file.take(HEAD_BYTES).read_to_end(&mut head).ok()?;
    let head = String::from_utf8_lossy(&head);
    head.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find_map(|entry| first_prompt(&entry))
        .map(|prompt| shorten(&prompt))
}

fn read_tail(file: &mut std::fs::File, length: u64) -> Option<String> {
    let start = length.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut tail = Vec::new();
    file.take(TAIL_BYTES).read_to_end(&mut tail).ok()?;
    let text = String::from_utf8_lossy(&tail).into_owned();
    // The first line was cut by the seek.
    Some(if start > 0 {
        text.split_once('\n')
            .map_or(String::new(), |(_, rest)| rest.to_owned())
    } else {
        text
    })
}

/// The last title in `text`: a person's own name wins over Claude's.
fn named_title(text: &str) -> Option<String> {
    let mut custom = None;
    let mut generated = None;
    for entry in text
        .lines()
        .filter(|line| line.contains("title"))
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
    {
        match entry.get("type").and_then(Value::as_str) {
            Some("custom-title") => {
                custom = entry
                    .get("customTitle")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            }
            Some("ai-title") => {
                generated = entry
                    .get("aiTitle")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            }
            Some("summary") => {
                generated = generated.or_else(|| {
                    entry
                        .get("summary")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                });
            }
            _ => {}
        }
    }
    custom
        .or(generated)
        .filter(|title| !title.trim().is_empty())
}

fn first_prompt(entry: &Value) -> Option<String> {
    if entry.get("type").and_then(Value::as_str) != Some("user")
        || entry.get("isSidechain").and_then(Value::as_bool) == Some(true)
        || entry.get("isMeta").and_then(Value::as_bool) == Some(true)
    {
        return None;
    }
    let content = entry.get("message")?.get("content")?;
    let text = match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" "),
        _ => return None,
    };
    let text = text.trim();
    // Slash-command wrappers and tool results are not what the person said.
    (!text.is_empty() && !text.starts_with('<') && !text.starts_with("Caveat:"))
        .then(|| text.to_owned())
}

fn shorten(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    match line.char_indices().nth(TITLE_CHARS) {
        Some((end, _)) => format!("{}…", line[..end].trim_end()),
        None => line.to_owned(),
    }
}

/// How long ago, in words: "just now", "5m ago", "Yesterday", "3 days ago".
pub fn ago(modified: SystemTime, now: SystemTime) -> String {
    let elapsed = now
        .duration_since(modified)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    const HOUR: u64 = 3600;
    const DAY: u64 = 24 * HOUR;
    match elapsed {
        0..60 => "just now".to_owned(),
        60..HOUR => format!("{}m ago", elapsed / 60),
        HOUR..DAY => format!("{}h ago", elapsed / HOUR),
        DAY..172_800 => "Yesterday".to_owned(),
        172_800..1_209_600 => format!("{} days ago", elapsed / DAY),
        _ => format!("{} weeks ago", elapsed / (7 * DAY)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write_log(folder: &Path, id: &str, lines: &[Value]) {
        let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
        std::fs::write(folder.join(format!("{id}.jsonl")), text).unwrap();
    }

    #[test]
    fn folder_names_follow_claude_codes_encoding() {
        assert_eq!(
            project_folder_name(Path::new("/home/me/dev/termeric")),
            "-home-me-dev-termeric"
        );
        assert_eq!(
            project_folder_name(Path::new("/tmp/a.b_c d")),
            "-tmp-a-b-c-d"
        );
    }

    #[test]
    fn titles_prefer_names_then_the_first_real_prompt() {
        let folder = tempfile::tempdir().unwrap();
        write_log(
            folder.path(),
            "named",
            &[
                json!({"type": "user", "message": {"content": "first prompt"}}),
                json!({"type": "ai-title", "aiTitle": "Generated title"}),
                json!({"type": "custom-title", "customTitle": "My title"}),
                json!({"type": "ai-title", "aiTitle": "Later generated"}),
            ],
        );
        write_log(
            folder.path(),
            "prompted",
            &[
                json!({"type": "queue-operation"}),
                json!({"type": "user", "isMeta": true, "message": {"content": "meta"}}),
                json!({"type": "user", "message": {"content": [{"type": "text", "text": "<command-name>/clear</command-name>"}]}}),
                json!({"type": "user", "isSidechain": true, "message": {"content": "sub-agent"}}),
                json!({"type": "user", "message": {"content": [{"type": "text", "text": "  Fix the build\nplease"}]}}),
            ],
        );
        write_log(
            folder.path(),
            "empty",
            &[json!({"type": "queue-operation"})],
        );
        write_log(
            folder.path(),
            "--dangerously-skip-permissions",
            &[json!({"type": "user", "message": {"content": "looks real"}})],
        );
        std::fs::write(folder.path().join("notes.txt"), "ignored").unwrap();
        let mut found = past_conversations_in(folder.path(), 10);
        found.sort_by(|a, b| a.id.cmp(&b.id));
        let titles: Vec<_> = found
            .iter()
            .map(|past| (past.id.as_str(), past.title.as_str()))
            .collect();
        assert_eq!(
            titles,
            [("named", "My title"), ("prompted", "Fix the build")]
        );
        assert_eq!(past_conversations_in(folder.path(), 1).len(), 1);
        assert!(past_conversations_in(&folder.path().join("missing"), 5).is_empty());
    }

    #[test]
    fn long_titles_are_shortened_on_a_character_boundary() {
        let long = "é".repeat(100);
        let short = shorten(&long);
        assert_eq!(short.chars().count(), TITLE_CHARS + 1);
        assert!(short.ends_with('…'));
    }

    #[test]
    fn ages_read_naturally() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000_000);
        let before = |seconds: u64| now - Duration::from_secs(seconds);
        assert_eq!(ago(before(5), now), "just now");
        assert_eq!(ago(before(300), now), "5m ago");
        assert_eq!(ago(before(7200), now), "2h ago");
        assert_eq!(ago(before(100_000), now), "Yesterday");
        assert_eq!(ago(before(300_000), now), "3 days ago");
        assert_eq!(ago(before(2_000_000), now), "3 weeks ago");
        assert_eq!(ago(now + Duration::from_secs(5), now), "just now");
    }
}
