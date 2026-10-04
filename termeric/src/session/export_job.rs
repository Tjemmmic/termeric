//! One background session export: flush the transcript, copy what it holds,
//! and render that copy as Markdown, or write captured scrollback. The work
//! runs off the UI thread, and completion never targets a tab.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use super::transcript::PendingTranscriptFlush;

pub(crate) const MAX_SCROLLBACK_BYTES: usize = 32 * 1024 * 1024;
/// How long the transcript writer may take to flush what it holds.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) enum ExportSource {
    Transcript(PendingTranscriptFlush),
    Scrollback(String),
}

#[derive(Debug)]
pub(crate) struct CompletedExport {
    pub markdown: PathBuf,
    pub includes_jsonl: bool,
}

pub(crate) struct ExportJob {
    reply: Receiver<Result<CompletedExport, String>>,
}

impl ExportJob {
    pub fn start(
        source: ExportSource,
        base: PathBuf,
        wake: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> io::Result<Self> {
        let (send, reply) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("termeric-export".into())
            .spawn(move || {
                let result = export(source, &base).map_err(|error| error.to_string());
                let _ = send.send(result);
                if let Some(wake) = wake {
                    wake();
                }
            })?;
        Ok(Self { reply })
    }

    pub fn poll(&self) -> Option<Result<CompletedExport, String>> {
        match self.reply.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                Some(Err("Export worker stopped unexpectedly".into()))
            }
        }
    }
}

fn export(source: ExportSource, base: &Path) -> io::Result<CompletedExport> {
    fs::create_dir_all(base)?;
    // A hidden private directory until the export is complete; publishing
    // renames it, so a visible export folder is always whole.
    let mut builder = tempfile::Builder::new();
    builder.prefix(".transcript-export-");
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(0o700));
    }
    let staging = builder.tempdir_in(base)?;
    let markdown = staging.path().join("transcript.md");
    let mut writer = BufWriter::new(create_private(&markdown)?);
    let includes_jsonl = match source {
        ExportSource::Transcript(pending) => {
            let bytes = pending
                .reply
                .recv_timeout(FLUSH_TIMEOUT)
                .map_err(|_| io::Error::other("Transcript writer did not flush its recording"))??;
            let raw = staging.path().join("transcript.jsonl");
            copy_prefix(&pending.path, &raw, bytes)?;
            let mut reader = BufReader::new(File::open(&raw)?);
            super::export::render_markdown(&mut reader, &mut writer)?;
            true
        }
        ExportSource::Scrollback(text) => {
            if text.len() > MAX_SCROLLBACK_BYTES {
                return Err(io::Error::other("Scrollback export exceeds 32 MiB"));
            }
            writer.write_all(b"# Scrollback Session Export\n\n```\n")?;
            writer.write_all(text.as_bytes())?;
            writer.write_all(b"\n```\n")?;
            false
        }
    };
    writer.flush()?;
    writer.get_ref().sync_all()?;
    drop(writer);

    let name = staging
        .path()
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix('.'))
        .ok_or_else(|| io::Error::other("export staging directory has no name"))?
        .to_owned();
    let directory = base.join(name);
    // A rename onto an existing directory with files in it fails, so an
    // export never replaces another.
    fs::rename(staging.path(), &directory)?;
    let _ = staging.keep();
    Ok(CompletedExport {
        markdown: directory.join("transcript.md"),
        includes_jsonl,
    })
}

fn create_private(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

/// Copy the first `bytes` of the transcript at `source`: what the writer had
/// flushed when asked, whatever it has written since.
fn copy_prefix(source: &Path, destination: &Path, bytes: u64) -> io::Result<()> {
    let file = crate::safe_fs::open_regular(source, usize::MAX)?;
    let mut writer = create_private(destination)?;
    let copied = io::copy(&mut file.take(bytes), &mut writer)?;
    if copied != bytes {
        return Err(io::Error::other(
            "Transcript was truncated after it was flushed",
        ));
    }
    writer.sync_all()
}

#[cfg(test)]
mod tests {
    use super::super::transcript::TranscriptRecorder;
    use super::*;
    use std::time::Instant;

    fn row(content: &str) -> Vec<u8> {
        let mut bytes = serde_json::to_vec(
            &serde_json::json!({"timestamp": 0, "kind": "output", "content": content}),
        )
        .unwrap();
        bytes.push(b'\n');
        bytes
    }

    /// A transcript at `path` holding `bytes`, already flushed.
    fn flushed(path: &Path, bytes: &[u8]) -> ExportSource {
        fs::write(path, bytes).unwrap();
        let (reply, receiver) = mpsc::channel();
        reply.send(Ok(bytes.len() as u64)).unwrap();
        ExportSource::Transcript(PendingTranscriptFlush {
            path: path.to_owned(),
            reply: receiver,
        })
    }

    fn wait(job: &ExportJob) -> Result<CompletedExport, String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(result) = job.poll() {
                return result;
            }
            assert!(Instant::now() < deadline, "export fixture did not complete");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_recording_export_holds_what_was_flushed_in_both_formats() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("exports");
        let mut recorder =
            TranscriptRecorder::new(dir.path(), None, None, "Local", "test").unwrap();
        recorder.record_output("INCLUDED 界").unwrap();
        let pending = recorder.flush().unwrap();
        recorder.record_output("LATER_NOT_INCLUDED").unwrap();
        recorder.finish(Duration::from_secs(2)).unwrap();
        let (woke, wake) = mpsc::channel();
        let job = ExportJob::start(
            ExportSource::Transcript(pending),
            base.clone(),
            Some(Arc::new(move || {
                let _ = woke.send(());
            })),
        )
        .unwrap();
        wake.recv_timeout(Duration::from_secs(5)).unwrap();
        let result = wait(&job).unwrap();
        let directory = result.markdown.parent().unwrap();
        assert!(result.includes_jsonl);
        assert!(
            !directory
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with('.')
        );
        for file in [&result.markdown, &directory.join("transcript.jsonl")] {
            let text = fs::read_to_string(file).unwrap();
            assert!(text.contains("INCLUDED 界"));
            assert!(!text.contains("LATER_NOT_INCLUDED"));
        }
        let raw = fs::read(directory.join("transcript.jsonl")).unwrap();
        let mut expected = Vec::new();
        super::super::export::render_markdown(&mut io::Cursor::new(raw), &mut expected).unwrap();
        assert_eq!(fs::read(&result.markdown).unwrap(), expected);
        use std::os::unix::fs::PermissionsExt;
        for path in [directory, result.markdown.as_path()] {
            assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o077, 0);
        }
    }

    #[test]
    fn a_failed_export_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("exports");
        fs::create_dir(&base).unwrap();
        let sentinel = base.join("keep");
        fs::write(&sentinel, "unchanged").unwrap();
        let source = dir.path().join("source");
        for failure in 0..3 {
            let original = row("original");
            let pending = flushed(
                &source,
                if failure == 0 {
                    b"not-json\n"
                } else {
                    &original
                },
            );
            match failure {
                1 => fs::write(&source, b"").unwrap(),
                2 => fs::remove_file(&source).unwrap(),
                _ => {}
            }
            assert!(export(pending, &base).is_err());
            assert_eq!(fs::read_dir(&base).unwrap().count(), 1);
            assert_eq!(fs::read_to_string(&sentinel).unwrap(), "unchanged");
        }
        // A writer that stops without flushing fails the export.
        let (writer, reply) = mpsc::channel::<io::Result<u64>>();
        drop(writer);
        let silent = ExportSource::Transcript(PendingTranscriptFlush {
            path: source,
            reply,
        });
        assert!(export(silent, &base).is_err());
        assert_eq!(fs::read_dir(&base).unwrap().count(), 1);
    }

    #[test]
    fn repeated_exports_never_overwrite_and_scrollback_has_no_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let first = export(ExportSource::Scrollback("first 界".into()), dir.path()).unwrap();
        let second = export(ExportSource::Scrollback("second".into()), dir.path()).unwrap();
        assert_ne!(first.markdown, second.markdown);
        assert!(!first.includes_jsonl);
        assert!(
            !first
                .markdown
                .parent()
                .unwrap()
                .join("transcript.jsonl")
                .exists()
        );
        assert!(
            fs::read_to_string(first.markdown)
                .unwrap()
                .contains("first 界")
        );
    }

    #[test]
    fn a_fifo_source_is_refused_without_waiting_for_a_writer() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("source.fifo");
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: name is a live C string inside this private fixture directory.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let started = Instant::now();
        let result = copy_prefix(&fifo, &dir.path().join("unused-copy"), 1);
        assert!(started.elapsed() < Duration::from_millis(500));
        assert!(result.unwrap_err().to_string().contains("regular file"));
        assert!(!dir.path().join("unused-copy").exists());
    }
}
