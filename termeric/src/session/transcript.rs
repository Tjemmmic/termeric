//! Transcript recording for Claude Code sessions
//!
//! Records input/output events to a JSONL file with timestamps,
//! secret redaction, and metadata. Each line is a self-contained
//! JSON object for easy streaming and recovery.

use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Entry type in the transcript
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    /// User input sent to the PTY
    Input,
    /// Terminal output from the process
    Output,
    /// Detected diff in output
    Diff,
    /// Detected command execution
    Command,
    /// Session metadata (first entry)
    Meta,
}

/// A single transcript entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptEntry {
    /// Unix timestamp (milliseconds)
    pub timestamp: u64,
    /// Entry type
    pub kind: EntryKind,
    /// Content (text data)
    pub content: String,
    /// Optional metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Secret redaction patterns
const REDACTION_PATTERNS: &[&str] = &[
    "sk-",         // OpenAI / Anthropic API keys
    "ghp_",        // GitHub personal access tokens
    "gho_",        // GitHub OAuth tokens
    "github_pat_", // GitHub fine-grained personal access tokens
    "xoxb-",       // Slack bot tokens
    "xoxp-",       // Slack user tokens
    "AKIA",        // AWS access key IDs
    "eyJ",         // JWT tokens (base64 encoded JSON)
];

/// Redact potential secrets from a string
pub fn redact_secrets(input: &str) -> String {
    let mut result = input.to_string();
    for pattern in REDACTION_PATTERNS {
        // Find all occurrences and redact the subsequent characters
        let mut search_from = 0;
        while let Some(pos) = result[search_from..].find(pattern) {
            let abs_pos = search_from + pos;
            let prefix_end = abs_pos + pattern.len();
            // Redact up to 40 characters after the prefix (typical key length)
            let end = (prefix_end + 40).min(result.len());
            // Find the end of the token (space, newline, quote, or end of string)
            let token_end = result[prefix_end..end]
                .find(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == ',' || c == ';')
                .map(|i| prefix_end + i)
                .unwrap_or(end);
            let redacted = format!("{}[REDACTED]", pattern);
            result.replace_range(abs_pos..token_end, &redacted);
            search_from = abs_pos + redacted.len();
        }
    }
    result
}

/// Active transcript recorder
pub struct TranscriptRecorder {
    /// Path to the JSONL file
    path: PathBuf,
    /// Buffered writer
    writer: BufWriter<File>,
    /// Number of entries written
    pub entry_count: usize,
    /// Maximum file size in bytes (default: 50MB)
    max_size: usize,
    /// Whether to redact secrets
    redact: bool,
    /// Flips true the first time `write_entry` drops a line because the
    /// on-disk file has reached `max_size`. Callers can poll this to emit
    /// a one-shot user-facing notification.
    cap_reached: bool,
}

impl std::fmt::Debug for TranscriptRecorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TranscriptRecorder")
            .field("path", &self.path)
            .field("entry_count", &self.entry_count)
            .finish()
    }
}

impl TranscriptRecorder {
    /// Create a new transcript recorder at the given path.
    ///
    /// Writes a metadata entry as the first line.
    pub fn new(
        dir: &Path,
        repo: Option<&str>,
        branch: Option<&str>,
        host: &str,
        profile: &str,
    ) -> std::io::Result<Self> {
        fs::create_dir_all(dir)?;

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let filename = format!("transcript_{}.jsonl", timestamp);
        let path = dir.join(&filename);

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        let mut writer = BufWriter::new(file);

        // Write metadata entry
        let meta = TranscriptEntry {
            timestamp,
            kind: EntryKind::Meta,
            content: String::new(),
            metadata: Some(serde_json::json!({
                "repo": repo,
                "branch": branch,
                "host": host,
                "profile": profile,
                "version": "1.0",
            })),
        };
        let line = serde_json::to_string(&meta).unwrap_or_default();
        writeln!(writer, "{}", line)?;
        writer.flush()?;

        Ok(Self {
            path,
            writer,
            entry_count: 1,
            max_size: 50 * 1024 * 1024, // 50MB
            redact: true,
            cap_reached: false,
        })
    }

    /// Whether the transcript has hit its size cap and is dropping writes.
    /// Once true it stays true for the lifetime of this recorder.
    pub fn cap_reached(&self) -> bool {
        self.cap_reached
    }

    /// Record an input event (user typed something)
    pub fn record_input(&mut self, data: &str) {
        self.write_entry(EntryKind::Input, data, None);
    }

    /// Record an output event (terminal output)
    pub fn record_output(&mut self, data: &str) {
        self.write_entry(EntryKind::Output, data, None);
    }

    /// Record a detected diff
    pub fn record_diff(&mut self, filename: &str, diff_content: &str) {
        self.write_entry(EntryKind::Diff, diff_content, Some(serde_json::json!({
            "filename": filename,
        })));
    }

    /// Get the path to the transcript file
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn write_entry(&mut self, kind: EntryKind, content: &str, metadata: Option<serde_json::Value>) {
        // Skip if over size limit
        if let Ok(md) = fs::metadata(&self.path) {
            if md.len() as usize >= self.max_size {
                if !self.cap_reached {
                    log::warn!(
                        "Transcript reached size cap ({} bytes); further writes will be dropped",
                        self.max_size
                    );
                }
                self.cap_reached = true;
                return;
            }
        }

        let content = if self.redact {
            redact_secrets(content)
        } else {
            content.to_string()
        };

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let entry = TranscriptEntry {
            timestamp,
            kind,
            content,
            metadata,
        };

        if let Ok(line) = serde_json::to_string(&entry) {
            let _ = writeln!(self.writer, "{}", line);
            let _ = self.writer.flush();
            self.entry_count += 1;
        }
    }
}

#[cfg(test)]
impl TranscriptRecorder {
    fn record_command(&mut self, command: &str, exit_code: Option<i32>) {
        self.write_entry(EntryKind::Command, command, Some(serde_json::json!({
            "exit_code": exit_code,
        })));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redact_secrets_api_keys() {
        let input = "My key is sk-abc123def456 and token is ghp_xyzzy789";
        let redacted = redact_secrets(input);
        assert!(redacted.contains("sk-[REDACTED]"));
        assert!(redacted.contains("ghp_[REDACTED]"));
        assert!(!redacted.contains("abc123"));
        assert!(!redacted.contains("xyzzy789"));
    }

    #[test]
    fn test_redact_preserves_safe_text() {
        let input = "This is a normal command: ls -la";
        assert_eq!(redact_secrets(input), input);
    }

    #[test]
    fn test_redact_jwt() {
        let input = r#"Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.payload"#;
        let redacted = redact_secrets(input);
        assert!(redacted.contains("eyJ[REDACTED]"));
    }

    #[test]
    fn test_recorder_creates_file() {
        let dir = std::env::temp_dir().join("termeric_test_transcript");
        let _ = fs::remove_dir_all(&dir);

        let recorder = TranscriptRecorder::new(
            &dir, Some("termeric"), Some("main"), "Local", "default"
        ).unwrap();

        assert!(recorder.path().exists());
        assert_eq!(recorder.entry_count, 1); // metadata entry

        // Read and verify metadata
        let contents = fs::read_to_string(recorder.path()).unwrap();
        let first_line = contents.lines().next().unwrap();
        let entry: TranscriptEntry = serde_json::from_str(first_line).unwrap();
        assert_eq!(entry.kind, EntryKind::Meta);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_recorder_writes_entries() {
        let dir = std::env::temp_dir().join("termeric_test_transcript_entries");
        let _ = fs::remove_dir_all(&dir);

        let mut recorder = TranscriptRecorder::new(
            &dir, None, None, "Local", "default"
        ).unwrap();

        recorder.record_input("Hello Claude");
        recorder.record_output("I'll help you with that.");
        recorder.record_diff("src/main.rs", "+new line\n-old line");
        recorder.record_command("cargo test", Some(0));

        assert_eq!(recorder.entry_count, 5); // 1 meta + 4 entries

        let contents = fs::read_to_string(recorder.path()).unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 5);

        // Verify each entry kind
        let e1: TranscriptEntry = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(e1.kind, EntryKind::Input);
        assert_eq!(e1.content, "Hello Claude");

        let e2: TranscriptEntry = serde_json::from_str(lines[2]).unwrap();
        assert_eq!(e2.kind, EntryKind::Output);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_recorder_redacts_secrets() {
        let dir = std::env::temp_dir().join("termeric_test_transcript_redact");
        let _ = fs::remove_dir_all(&dir);

        let mut recorder = TranscriptRecorder::new(
            &dir, None, None, "Local", "default"
        ).unwrap();

        recorder.record_input("export API_KEY=sk-abc123xyz456");

        let contents = fs::read_to_string(recorder.path()).unwrap();
        let input_line = contents.lines().nth(1).unwrap();
        let entry: TranscriptEntry = serde_json::from_str(input_line).unwrap();
        assert!(entry.content.contains("sk-[REDACTED]"));
        assert!(!entry.content.contains("abc123"));

        let _ = fs::remove_dir_all(&dir);
    }
}
