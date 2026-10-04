//! Session export — convert JSONL transcripts to human-readable formats
//!
//! Supports Markdown and raw JSONL export.

#[cfg(test)]
use std::fs;
use std::io::{self, BufRead, Seek, SeekFrom, Write};
#[cfg(test)]
use std::path::Path;

use super::transcript::{EntryKind, MAX_TRANSCRIPT_RECORD_BYTES, TranscriptEntry};

/// Test helper: the Markdown export of a transcript file, as one String.
/// Production exports stream into a staged file instead.
#[cfg(test)]
pub(crate) fn markdown_of_file(jsonl_path: &Path) -> Result<String, String> {
    let mut reader = io::BufReader::new(fs::File::open(jsonl_path).map_err(|e| e.to_string())?);
    let mut output = Vec::new();
    render_markdown(&mut reader, &mut output).map_err(|e| e.to_string())?;
    String::from_utf8(output).map_err(|e| e.to_string())
}

/// At most one encoded record and one decoded entry are retained. Two passes
/// put the first metadata record's header first without collecting entries.
pub(super) fn render_markdown(
    reader: &mut (impl BufRead + Seek),
    writer: &mut impl Write,
) -> io::Result<usize> {
    let mut line = Vec::new();
    let mut peak_encoded_capacity = 0;
    let mut count = 0;
    let mut metadata = None;
    while read_record(reader, &mut line)? {
        peak_encoded_capacity = peak_encoded_capacity.max(line.capacity());
        count += 1;
        let entry = parse_entry(&line, count)?;
        if entry.kind == EntryKind::Meta {
            metadata = Some(entry);
            break;
        }
    }
    if count == 0 {
        return Err(io::Error::other("Empty transcript"));
    }
    write_header(writer, metadata.as_ref())?;
    drop(metadata);
    reader.seek(SeekFrom::Start(0))?;
    count = 0;
    while read_record(reader, &mut line)? {
        peak_encoded_capacity = peak_encoded_capacity.max(line.capacity());
        count += 1;
        write_entry(writer, &parse_entry(&line, count)?)?;
    }
    Ok(peak_encoded_capacity)
}

fn parse_entry(line: &[u8], number: usize) -> io::Result<TranscriptEntry> {
    serde_json::from_slice(line).map_err(|error| {
        io::Error::other(format!("Malformed transcript at line {number}: {error}"))
    })
}

/// Read the next record into `line`; false at the end of the transcript.
fn read_record(reader: &mut impl BufRead, line: &mut Vec<u8>) -> io::Result<bool> {
    match crate::line_reader::read_bounded_line(reader, MAX_TRANSCRIPT_RECORD_BYTES, line)? {
        None => Ok(false),
        Some(read) if read.oversized => Err(io::Error::other(
            "Transcript record exceeds the 16 MiB encoded limit",
        )),
        Some(_) => Ok(true),
    }
}

fn write_header(writer: &mut impl Write, metadata: Option<&TranscriptEntry>) -> io::Result<()> {
    writer.write_all(b"# AI Session Transcript\n\n")?;
    if let Some(meta) = metadata {
        if let Some(data) = &meta.metadata {
            writer.write_all(b"| Field | Value |\n|---|---|\n")?;
            for (key, label) in [
                ("repo", "Repository"),
                ("branch", "Branch"),
                ("host", "Host"),
                ("profile", "Profile"),
            ] {
                if let Some(value) = data.get(key).and_then(|value| value.as_str()) {
                    writeln!(writer, "| {label} | {value} |")?;
                }
            }
            writeln!(writer)?;
        }
        write!(
            writer,
            "**Started**: {}\n\n---\n\n",
            format_unix_ts(meta.timestamp / 1000)
        )?;
    }
    Ok(())
}

fn write_entry(writer: &mut impl Write, entry: &TranscriptEntry) -> io::Result<()> {
    match entry.kind {
        EntryKind::Meta => {}
        EntryKind::Input | EntryKind::Diff => {
            if entry.kind == EntryKind::Input {
                write!(
                    writer,
                    "### 📝 Input `{}`\n\n```\n",
                    format_unix_ts(entry.timestamp / 1000)
                )?;
            } else {
                let filename = entry
                    .metadata
                    .as_ref()
                    .and_then(|m| m.get("filename"))
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown");
                write!(
                    writer,
                    "### 📝 Diff: `{filename}` `{}`\n\n```diff\n",
                    format_unix_ts(entry.timestamp / 1000)
                )?;
            }
            writer.write_all(entry.content.as_bytes())?;
            if !entry.content.ends_with('\n') {
                writeln!(writer)?;
            }
            writer.write_all(b"```\n\n")?;
        }
        EntryKind::Output => {
            write!(
                writer,
                "### 💬 Output `{}`\n\n",
                format_unix_ts(entry.timestamp / 1000)
            )?;
            writer.write_all(entry.content.as_bytes())?;
            if !entry.content.ends_with('\n') {
                writeln!(writer)?;
            }
            writeln!(writer)?;
        }
    }
    Ok(())
}

/// Simple Unix timestamp formatter (UTC, no timezone dependency)
fn format_unix_ts(secs: u64) -> String {
    super::format_unix_timestamp(secs, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::io::Write;

    #[test]
    fn streaming_conversion_keeps_one_record_for_a_large_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.jsonl");
        let mut file = fs::File::create(&path).unwrap();
        let entry = TranscriptEntry {
            timestamp: 0,
            kind: EntryKind::Output,
            content: "x".repeat(512 * 1024),
            metadata: None,
        };
        let line = serde_json::to_vec(&entry).unwrap();
        for _ in 0..90 {
            file.write_all(&line).unwrap();
            file.write_all(b"\n").unwrap();
        }
        drop(file);
        struct Count(u64, u64);
        impl Write for Count {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0 += bytes.len() as u64;
                self.1 += bytes.iter().filter(|byte| **byte == b'x').count() as u64;
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut output = Count(0, 0);
        let mut input = io::BufReader::new(fs::File::open(path).unwrap());
        let peak = render_markdown(&mut input, &mut output).unwrap();
        assert!(peak <= 1024 * 1024, "encoded buffer grew to {peak}");
        assert!(output.0 >= 45 * 1024 * 1024, "content was truncated");
        assert_eq!(
            output.1,
            45 * 1024 * 1024,
            "payload bytes were lost or duplicated"
        );
    }

    #[test]
    fn encoded_record_limit_accepts_maximum_escaped_recorder_content() {
        // Queue accounting reserves 128 bytes; NUL expands sixfold in JSON.
        let entry = TranscriptEntry {
            timestamp: 0,
            kind: EntryKind::Output,
            content: "\0".repeat(2 * 1024 * 1024 - 128),
            metadata: None,
        };
        let line = serde_json::to_vec(&entry).unwrap();
        assert!(line.len() < MAX_TRANSCRIPT_RECORD_BYTES);
        let peak = render_markdown(&mut io::Cursor::new(line), &mut io::sink()).unwrap();
        assert!(peak <= MAX_TRANSCRIPT_RECORD_BYTES);
        let mut oversized =
            io::BufReader::new(io::repeat(b'x').take(MAX_TRANSCRIPT_RECORD_BYTES as u64 + 1));
        assert!(
            read_record(&mut oversized, &mut Vec::new())
                .unwrap_err()
                .to_string()
                .contains("16 MiB")
        );
    }

    #[test]
    fn streaming_metadata_order_and_io_errors_remain_explicit() {
        let input = b"{\"timestamp\":0,\"kind\":\"output\",\"content\":\"early\"}\n{\"timestamp\":0,\"kind\":\"meta\",\"content\":\"\",\"metadata\":{\"repo\":\"later metadata\"}}\n";
        let mut output = Vec::new();
        render_markdown(&mut io::Cursor::new(input), &mut output).unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.find("later metadata").unwrap() < text.find("early").unwrap());
        struct Fail;
        impl Write for Fail {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("injected full disk"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        assert!(
            render_markdown(&mut io::Cursor::new(input), &mut Fail)
                .unwrap_err()
                .to_string()
                .contains("full disk")
        );
    }

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

        path
    }

    #[test]
    fn test_markdown_of_file() {
        let dir = std::env::temp_dir().join("termeric_test_export");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let path = write_test_transcript(&dir);
        let md = markdown_of_file(&path).unwrap();

        assert!(md.contains("# AI Session Transcript"));
        assert!(md.contains("| Repository | termeric |"));
        assert!(md.contains("| Branch | main |"));
        assert!(md.contains("📝 Input"));
        assert!(md.contains("Hello Claude"));
        assert!(md.contains("💬 Output"));
        assert!(md.contains("📝 Diff: `src/main.rs`"));
        assert!(md.contains("```diff"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_export_empty_file() {
        let dir = std::env::temp_dir().join("termeric_test_export_empty");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let path = dir.join("empty.jsonl");
        fs::write(&path, "").unwrap();

        assert!(markdown_of_file(&path).is_err());

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
        let md = markdown_of_file(&path).unwrap();
        assert!(md.contains(&large));
        assert!(!md.contains("truncated"));

        writeln!(file, "not-json").unwrap();
        let error = markdown_of_file(&path).unwrap_err();
        assert!(error.contains("Malformed transcript"));
    }
}
