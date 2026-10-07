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
/// Every export is staged in a hidden folder named this plus random
/// letters and digits, and published without the leading dot.
const STAGING_PREFIX: &str = ".transcript-export-";
/// A staging folder this old was left by an export that never finished
/// (Termeric stopped mid-export); a live one lasts seconds.
const STALE_STAGING_AGE: Duration = Duration::from_secs(24 * 60 * 60);

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
        Self::start_with_flush_timeout(source, base, wake, FLUSH_TIMEOUT)
    }

    fn start_with_flush_timeout(
        source: ExportSource,
        base: PathBuf,
        wake: Option<Arc<dyn Fn() + Send + Sync>>,
        flush_timeout: Duration,
    ) -> io::Result<Self> {
        let (send, reply) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("termeric-export".into())
            .spawn(move || {
                let result =
                    export(source, &base, flush_timeout).map_err(|error| error.to_string());
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

fn export(
    source: ExportSource,
    base: &Path,
    flush_timeout: Duration,
) -> io::Result<CompletedExport> {
    fs::create_dir_all(base)?;
    remove_stale_staging(base);
    // A hidden private directory until the export is complete; publishing
    // renames it, so a visible export folder is always whole.
    let mut builder = tempfile::Builder::new();
    builder.prefix(STAGING_PREFIX);
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
                .recv_timeout(flush_timeout)
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
    // Never replaces anything already there, so an export never replaces
    // another.
    crate::safe_fs::rename_no_replace(staging.path(), &directory)?;
    let _ = staging.keep();
    // The export is already published, so failing here would wrongly say it
    // was not. Directory durability is reported, not returned.
    if let Err(error) = crate::safe_fs::sync_directory(base) {
        log::warn!(
            "{} was exported, but its directory could not be synced: {error}",
            directory.display()
        );
    }
    Ok(CompletedExport {
        markdown: directory.join("transcript.md"),
        includes_jsonl,
    })
}

/// Remove the staging folders that exports cut short left in `base`: only
/// our own real directories named like a staging folder, and only once they
/// are too old to belong to an export still running in another window.
fn remove_stale_staging(base: &Path) {
    let Ok(entries) = fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        if !is_staging_name(&entry.file_name()) {
            continue;
        }
        let path = entry.path();
        // The entry itself, never what a symlink points to.
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        use std::os::unix::fs::MetadataExt;
        let stale = metadata.file_type().is_dir()
            && metadata.uid() == crate::local_socket::effective_uid()
            && metadata
                .modified()
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age >= STALE_STAGING_AGE);
        if stale && let Err(error) = fs::remove_dir_all(&path) {
            log::warn!(
                "could not remove an unfinished export at {}: {error}",
                path.display()
            );
        }
    }
}

fn is_staging_name(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .and_then(|name| name.strip_prefix(STAGING_PREFIX))
        .is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
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

    /// A transcript at `path` holding `bytes`, whose writer has not flushed
    /// yet: it flushes when the returned sender sends.
    fn unflushed(path: &Path, bytes: &[u8]) -> (ExportSource, mpsc::Sender<io::Result<u64>>) {
        fs::write(path, bytes).unwrap();
        let (reply, receiver) = mpsc::channel();
        let source = ExportSource::Transcript(PendingTranscriptFlush {
            path: path.to_owned(),
            reply: receiver,
        });
        (source, reply)
    }

    #[test]
    fn starting_an_export_never_waits_for_the_transcript_writer() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("exports");
        let content = row("FLUSHED LATER");
        let (source, writer) = unflushed(&dir.path().join("source"), &content);
        let job = ExportJob::start(source, base.clone(), None).unwrap();
        // The worker is still waiting for the flush, so nothing is done yet.
        std::thread::sleep(Duration::from_millis(50));
        assert!(job.poll().is_none());
        assert!(
            !base.exists()
                || fs::read_dir(&base).unwrap().all(|entry| {
                    entry
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .starts_with('.')
                })
        );
        writer.send(Ok(content.len() as u64)).unwrap();
        let exported = wait(&job).unwrap();
        assert!(
            fs::read_to_string(&exported.markdown)
                .unwrap()
                .contains("FLUSHED LATER")
        );
    }

    #[test]
    fn a_writer_that_never_flushes_times_out_and_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("exports");
        let (source, _writer) = unflushed(&dir.path().join("source"), &row("never"));
        let started = Instant::now();
        let job = ExportJob::start_with_flush_timeout(
            source,
            base.clone(),
            None,
            Duration::from_millis(100),
        )
        .unwrap();
        let error = wait(&job).unwrap_err();
        assert!(started.elapsed() >= Duration::from_millis(100));
        assert_eq!(error, "Transcript writer did not flush its recording");
        assert_eq!(fs::read_dir(&base).unwrap().count(), 0);
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
            assert!(export(pending, &base, FLUSH_TIMEOUT).is_err());
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
        assert!(export(silent, &base, FLUSH_TIMEOUT).is_err());
        assert_eq!(fs::read_dir(&base).unwrap().count(), 1);
    }

    #[test]
    fn repeated_exports_never_overwrite_and_scrollback_has_no_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let scrollback = |text: &str| {
            export(
                ExportSource::Scrollback(text.into()),
                dir.path(),
                FLUSH_TIMEOUT,
            )
        };
        let first = scrollback("first 界").unwrap();
        let second = scrollback("second").unwrap();
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
    fn exports_clean_up_only_old_staging_folders_of_their_own() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        let age = |path: &Path, age: Duration| {
            let modified = std::time::SystemTime::now() - age;
            File::open(path).unwrap().set_modified(modified).unwrap();
        };
        let stale = base.join(".transcript-export-Ab12cD");
        fs::create_dir(&stale).unwrap();
        fs::write(stale.join("transcript.md"), "cut short").unwrap();
        age(&stale, STALE_STAGING_AGE + Duration::from_secs(60));
        let running = base.join(".transcript-export-Zz9Yy8");
        fs::create_dir(&running).unwrap();
        let unrelated = base.join(".transcript-export-not.ours");
        fs::create_dir(&unrelated).unwrap();
        age(&unrelated, STALE_STAGING_AGE * 2);
        let published = base.join("transcript-export-Qq1Ww2");
        fs::create_dir(&published).unwrap();
        age(&published, STALE_STAGING_AGE * 2);
        let target = dir.path().join("elsewhere");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("kept"), "kept").unwrap();
        age(&target, STALE_STAGING_AGE * 2);
        let link = base.join(".transcript-export-L1nk00");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let file = base.join(".transcript-export-F1le00");
        fs::write(&file, "kept").unwrap();
        age(&file, STALE_STAGING_AGE * 2);

        let exported = export(ExportSource::Scrollback("new".into()), base, FLUSH_TIMEOUT).unwrap();

        assert!(!stale.exists());
        for kept in [&running, &unrelated, &published, &link, &file] {
            assert!(fs::symlink_metadata(kept).is_ok(), "{}", kept.display());
        }
        assert_eq!(fs::read_to_string(target.join("kept")).unwrap(), "kept");
        assert!(exported.markdown.exists());
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
