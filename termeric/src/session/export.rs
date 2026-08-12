//! Session export — convert JSONL transcripts to human-readable formats
//!
//! Supports Markdown and raw JSONL export.

use std::fs;
use std::path::Path;

use super::transcript::{EntryKind, TranscriptEntry};

/// Export a JSONL transcript file to Markdown format.
///
/// Reads the JSONL file, groups entries, and produces a formatted
/// Markdown document with timestamps, code blocks, and diff sections.
pub fn export_to_markdown(jsonl_path: &Path) -> Result<String, String> {
    let contents =
        fs::read_to_string(jsonl_path).map_err(|e| format!("Failed to read transcript: {}", e))?;

    let entries: Result<Vec<TranscriptEntry>, String> = contents
        .lines()
        .enumerate()
        .map(|(index, line)| {
            serde_json::from_str(line)
                .map_err(|error| format!("Malformed transcript at line {}: {error}", index + 1))
        })
        .collect();
    let entries = entries?;

    if entries.is_empty() {
        return Err("Empty transcript".to_string());
    }

    let mut md = String::new();

    // Header
    md.push_str("# AI Session Transcript\n\n");

    // Metadata from first entry
    if let Some(meta) = entries.iter().find(|e| e.kind == EntryKind::Meta) {
        if let Some(ref data) = meta.metadata {
            md.push_str("| Field | Value |\n|---|---|\n");
            if let Some(repo) = data.get("repo").and_then(|v| v.as_str()) {
                md.push_str(&format!("| Repository | {} |\n", repo));
            }
            if let Some(branch) = data.get("branch").and_then(|v| v.as_str()) {
                md.push_str(&format!("| Branch | {} |\n", branch));
            }
            if let Some(host) = data.get("host").and_then(|v| v.as_str()) {
                md.push_str(&format!("| Host | {} |\n", host));
            }
            if let Some(profile) = data.get("profile").and_then(|v| v.as_str()) {
                md.push_str(&format!("| Profile | {} |\n", profile));
            }
            md.push('\n');
        }

        // Timestamp
        let ts = meta.timestamp / 1000;
        md.push_str(&format!("**Started**: {}\n\n", format_unix_ts(ts)));
        md.push_str("---\n\n");
    }

    // Content entries
    for entry in &entries {
        match entry.kind {
            EntryKind::Meta => continue,
            EntryKind::Input => {
                md.push_str(&format!(
                    "### 📝 Input `{}`\n\n",
                    format_unix_ts(entry.timestamp / 1000)
                ));
                md.push_str("```\n");
                md.push_str(&entry.content);
                if !entry.content.ends_with('\n') {
                    md.push('\n');
                }
                md.push_str("```\n\n");
            }
            EntryKind::Output => {
                md.push_str(&format!(
                    "### 💬 Output `{}`\n\n",
                    format_unix_ts(entry.timestamp / 1000)
                ));
                md.push_str(&entry.content);
                if !entry.content.ends_with('\n') {
                    md.push('\n');
                }
                md.push('\n');
            }
            EntryKind::Diff => {
                let filename = entry
                    .metadata
                    .as_ref()
                    .and_then(|m| m.get("filename"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                md.push_str(&format!(
                    "### 📝 Diff: `{}` `{}`\n\n",
                    filename,
                    format_unix_ts(entry.timestamp / 1000)
                ));
                md.push_str("```diff\n");
                md.push_str(&entry.content);
                if !entry.content.ends_with('\n') {
                    md.push('\n');
                }
                md.push_str("```\n\n");
            }
            EntryKind::Command => {
                let exit_code = entry
                    .metadata
                    .as_ref()
                    .and_then(|m| m.get("exit_code"))
                    .and_then(|v| v.as_i64());
                let status = match exit_code {
                    Some(0) => "✅",
                    Some(_) => "❌",
                    None => "⏳",
                };
                md.push_str(&format!("### {} Command: `{}`\n\n", status, entry.content));
            }
        }
    }

    Ok(md)
}

/// Simple Unix timestamp formatter (UTC, no timezone dependency)
fn format_unix_ts(secs: u64) -> String {
    super::format_unix_timestamp(secs, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_test_transcript(dir: &Path) -> std::path::PathBuf {
        let path = dir.join("test_transcript.jsonl");
        let mut f = fs::File::create(&path).unwrap();

        let meta = TranscriptEntry {
            timestamp: 1700000000000,
            kind: EntryKind::Meta,
            content: String::new(),
            metadata: Some(serde_json::json!({
                "repo": "termeric",
                "branch": "main",
                "host": "Local",
                "profile": "default",
            })),
        };
        writeln!(f, "{}", serde_json::to_string(&meta).unwrap()).unwrap();

        let input = TranscriptEntry {
            timestamp: 1700000001000,
            kind: EntryKind::Input,
            content: "Hello Claude".to_string(),
            metadata: None,
        };
        writeln!(f, "{}", serde_json::to_string(&input).unwrap()).unwrap();

        let output = TranscriptEntry {
            timestamp: 1700000002000,
            kind: EntryKind::Output,
            content: "I'll help you with that.".to_string(),
            metadata: None,
        };
        writeln!(f, "{}", serde_json::to_string(&output).unwrap()).unwrap();

        let diff = TranscriptEntry {
            timestamp: 1700000003000,
            kind: EntryKind::Diff,
            content: "+new line\n-old line".to_string(),
            metadata: Some(serde_json::json!({"filename": "src/main.rs"})),
        };
        writeln!(f, "{}", serde_json::to_string(&diff).unwrap()).unwrap();

        let cmd = TranscriptEntry {
            timestamp: 1700000004000,
            kind: EntryKind::Command,
            content: "cargo test".to_string(),
            metadata: Some(serde_json::json!({"exit_code": 0})),
        };
        writeln!(f, "{}", serde_json::to_string(&cmd).unwrap()).unwrap();

        path
    }

    #[test]
    fn test_export_to_markdown() {
        let dir = std::env::temp_dir().join("termeric_test_export");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let path = write_test_transcript(&dir);
        let md = export_to_markdown(&path).unwrap();

        assert!(md.contains("# AI Session Transcript"));
        assert!(md.contains("| Repository | termeric |"));
        assert!(md.contains("| Branch | main |"));
        assert!(md.contains("📝 Input"));
        assert!(md.contains("Hello Claude"));
        assert!(md.contains("💬 Output"));
        assert!(md.contains("📝 Diff: `src/main.rs`"));
        assert!(md.contains("```diff"));
        assert!(md.contains("✅ Command: `cargo test`"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_export_empty_file() {
        let dir = std::env::temp_dir().join("termeric_test_export_empty");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let path = dir.join("empty.jsonl");
        fs::write(&path, "").unwrap();

        assert!(export_to_markdown(&path).is_err());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_format_unix_ts() {
        assert_eq!(format_unix_ts(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_unix_ts(3661), "1970-01-01T01:01:01Z");
    }

    #[test]
    fn markdown_export_is_complete_and_rejects_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_test_transcript(dir.path());
        let large = "x".repeat(6_000);
        let entry = TranscriptEntry {
            timestamp: 1_700_000_005_000,
            kind: EntryKind::Output,
            content: large.clone(),
            metadata: None,
        };
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(file, "{}", serde_json::to_string(&entry).unwrap()).unwrap();
        let md = export_to_markdown(&path).unwrap();
        assert!(md.contains(&large));
        assert!(!md.contains("truncated"));

        writeln!(file, "not-json").unwrap();
        let error = export_to_markdown(&path).unwrap_err();
        assert!(error.contains("Malformed transcript"));
    }
}
