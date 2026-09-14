//! One bounded background snapshot export; completion never targets a tab.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use super::transcript::{PendingTranscriptCheckpoint, TranscriptCheckpoint};

const MAX_ACTIVE_EXPORTS: usize = 4;
const MAX_SOURCE_BYTES: u64 = 50 * 1024 * 1024;
const MAX_MARKDOWN_BYTES: u64 = 128 * 1024 * 1024;
pub(crate) const MAX_SCROLLBACK_BYTES: usize = 32 * 1024 * 1024;
const CHECKPOINT_TIMEOUT: Duration = Duration::from_secs(5);
const EXPORT_TIMEOUT: Duration = Duration::from_secs(60);
static ACTIVE_EXPORTS: AtomicUsize = AtomicUsize::new(0);
#[cfg(test)]
pub(crate) static EXPORT_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) enum ExportSource {
    Transcript(PendingTranscriptCheckpoint),
    Scrollback(String),
}

#[derive(Debug)]
pub(crate) struct CompletedExport {
    pub markdown: PathBuf,
    pub includes_jsonl: bool,
}

struct ExportPermit;

impl ExportPermit {
    fn acquire() -> io::Result<Self> {
        reserve_slot(&ACTIVE_EXPORTS)?;
        Ok(Self)
    }
}

fn reserve_slot(counter: &AtomicUsize) -> io::Result<()> {
    counter
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
            (active < MAX_ACTIVE_EXPORTS).then_some(active + 1)
        })
        .map_err(|_| io::Error::other("Four exports are already active; retry later"))?;
    Ok(())
}

impl Drop for ExportPermit {
    fn drop(&mut self) {
        ACTIVE_EXPORTS.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(crate) struct ExportJob {
    reply: Receiver<Result<CompletedExport, String>>,
    cancelled: Arc<AtomicBool>,
}

impl ExportJob {
    pub fn start(
        source: ExportSource,
        base: PathBuf,
        wake: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> io::Result<Self> {
        let permit = ExportPermit::acquire()?;
        let (send, reply) = mpsc::sync_channel(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel = cancelled.clone();
        std::thread::Builder::new()
            .name("termeric-export".into())
            .spawn(move || {
                let _permit = permit;
                let result = export(source, &base, &cancel).map_err(|error| error.to_string());
                let _ = send.send(result);
                if let Some(wake) = wake {
                    wake();
                }
            })?;
        Ok(Self { reply, cancelled })
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

impl Drop for ExportJob {
    fn drop(&mut self) {
        // Never join a thread that may be in a blocking filesystem operation.
        // Its process-wide permit remains held until the operation returns.
        self.cancelled.store(true, Ordering::Release);
    }
}

fn check(cancel: &AtomicBool, deadline: Instant) -> io::Result<()> {
    if cancel.load(Ordering::Acquire) {
        return Err(io::Error::other("Export cancelled before publication"));
    }
    if Instant::now() >= deadline {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Export exceeded its processing deadline",
        ));
    }
    Ok(())
}

fn export(source: ExportSource, base: &Path, cancel: &AtomicBool) -> io::Result<CompletedExport> {
    let deadline = Instant::now() + EXPORT_TIMEOUT;
    let check = || check(cancel, deadline);
    let source = match source {
        ExportSource::Transcript(pending) => {
            let until = Instant::now() + CHECKPOINT_TIMEOUT;
            let checkpoint = loop {
                check()?;
                match pending.reply.recv_timeout(Duration::from_millis(50)) {
                    Ok(result) => break result?,
                    Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() < until => continue,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "Transcript checkpoint timed out",
                        ));
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        return Err(io::Error::other(
                            "Transcript writer stopped before confirming export",
                        ));
                    }
                }
            };
            VerifiedSource::Transcript(pending.path, checkpoint)
        }
        ExportSource::Scrollback(text) => {
            if text.len() > MAX_SCROLLBACK_BYTES {
                return Err(io::Error::other("Scrollback export exceeds 32 MiB"));
            }
            VerifiedSource::Scrollback(text)
        }
    };
    export_verified(source, base, &check)
}

enum VerifiedSource {
    Transcript(PathBuf, TranscriptCheckpoint),
    Scrollback(String),
}

fn export_verified(
    source: VerifiedSource,
    base: &Path,
    check: &impl Fn() -> io::Result<()>,
) -> io::Result<CompletedExport> {
    check()?;
    fs::create_dir_all(base)?;
    // The exclusive private directory is a staging area until complete.json
    // is atomically installed. No existing destination is ever reused.
    let mut builder = tempfile::Builder::new();
    builder.prefix("transcript-export-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(0o700));
    }
    let staging = builder.tempdir_in(base)?;
    let markdown = staging.path().join("transcript.md");
    let mut writer = BufWriter::new(CheckedWriter {
        inner: create_private(&markdown)?,
        bytes: 0,
        check,
    });
    let mut recorded_prefix = None;
    let includes_jsonl = match source {
        VerifiedSource::Transcript(path, checkpoint) => {
            let raw = staging.path().join("transcript.jsonl");
            copy_verified_prefix(&path, &raw, &checkpoint, check)?;
            recorded_prefix = Some(serde_json::json!({
                "bytes": checkpoint.bytes,
                "sha256": checkpoint.sha256.iter().map(|byte| format!("{byte:02x}")).collect::<String>(),
                "recording_limit_reached": checkpoint.recording_limit_reached,
            }));
            let mut reader = BufReader::new(File::open(&raw)?);
            super::export::render_markdown(&mut reader, &mut writer, check)?;
            true
        }
        VerifiedSource::Scrollback(text) => {
            writer.write_all(b"# Scrollback Session Export\n\n```\n")?;
            writer.write_all(text.as_bytes())?;
            writer.write_all(b"\n```\n")?;
            false
        }
    };
    writer.flush()?;
    writer.get_ref().inner.sync_all()?;
    drop(writer);
    check()?;
    // This marker is the portable publication point for the complete pair.
    // Consumers must not treat a directory without it as a completed export.
    let mut marker = tempfile::NamedTempFile::new_in(staging.path())?;
    serde_json::to_writer(
        &mut marker,
        &serde_json::json!({
            "schema": 1, "markdown": "transcript.md",
            "jsonl": includes_jsonl.then_some("transcript.jsonl"),
            "recorded_prefix": recorded_prefix,
        }),
    )?;
    marker.flush()?;
    marker.as_file().sync_all()?;
    check()?;
    marker
        .persist_noclobber(staging.path().join("complete.json"))
        .map_err(|error| error.error)?;
    let directory = staging.keep();
    Ok(CompletedExport {
        markdown: directory.join("transcript.md"),
        includes_jsonl,
    })
}

fn create_private(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn copy_verified_prefix(
    source: &Path,
    destination: &Path,
    checkpoint: &TranscriptCheckpoint,
    check: &impl Fn() -> io::Result<()>,
) -> io::Result<()> {
    if checkpoint.bytes == 0 || checkpoint.bytes > MAX_SOURCE_BYTES {
        return Err(io::Error::other(
            "Transcript checkpoint must be within 1 byte..50 MiB",
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let file = options.open(source)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("Transcript source is not a regular file"));
    }
    let mut reader = file.take(checkpoint.bytes);
    let mut writer = create_private(destination)?;
    let mut digest = Sha256::new();
    let mut copied = 0;
    let mut buffer = [0; 64 * 1024];
    loop {
        check()?;
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        writer.write_all(&buffer[..read])?;
        digest.update(&buffer[..read]);
        copied += read as u64;
    }
    if copied != checkpoint.bytes || <[u8; 32]>::from(digest.finalize()) != checkpoint.sha256 {
        return Err(io::Error::other(
            "Transcript changed or was truncated after its checkpoint",
        ));
    }
    writer.sync_all()
}

struct CheckedWriter<'a, W, C> {
    inner: W,
    bytes: u64,
    check: &'a C,
}

impl<W: Write, C: Fn() -> io::Result<()>> Write for CheckedWriter<'_, W, C> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        (self.check)()?;
        let count = bytes.len().min(64 * 1024);
        if self.bytes + count as u64 > MAX_MARKDOWN_BYTES {
            return Err(io::Error::other("Markdown export exceeds 128 MiB"));
        }
        let written = self.inner.write(&bytes[..count])?;
        self.bytes += written as u64;
        Ok(written)
    }
    fn flush(&mut self) -> io::Result<()> {
        (self.check)()?;
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::super::transcript::TranscriptRecorder;
    use super::*;

    fn row(content: &str) -> Vec<u8> {
        let mut bytes = serde_json::to_vec(
            &serde_json::json!({"timestamp": 0, "kind": "output", "content": content}),
        )
        .unwrap();
        bytes.push(b'\n');
        bytes
    }

    fn fixture(path: &Path, bytes: &[u8]) -> VerifiedSource {
        fs::write(path, bytes).unwrap();
        VerifiedSource::Transcript(
            path.to_owned(),
            TranscriptCheckpoint {
                bytes: bytes.len() as u64,
                sha256: Sha256::digest(bytes).into(),
                recording_limit_reached: false,
            },
        )
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
    fn active_recorder_export_uses_one_verified_prefix_in_both_formats() {
        let _guard = EXPORT_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("exports");
        let mut recorder =
            TranscriptRecorder::new(dir.path(), None, None, "Local", "test").unwrap();
        recorder.record_output("INCLUDED 界").unwrap();
        let pending = recorder.checkpoint().unwrap();
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
        assert!(directory.join("complete.json").is_file());
        for file in [&result.markdown, &directory.join("transcript.jsonl")] {
            let text = fs::read_to_string(file).unwrap();
            assert!(text.contains("INCLUDED 界"));
            assert!(!text.contains("LATER_NOT_INCLUDED"));
        }
        let raw = fs::read(directory.join("transcript.jsonl")).unwrap();
        let marker: serde_json::Value =
            serde_json::from_slice(&fs::read(directory.join("complete.json")).unwrap()).unwrap();
        assert_eq!(marker["recorded_prefix"]["bytes"], raw.len() as u64);
        assert_eq!(
            marker["recorded_prefix"]["sha256"],
            format!("{:x}", Sha256::digest(&raw))
        );
        assert_eq!(marker["recorded_prefix"]["recording_limit_reached"], false);
        let mut expected = Vec::new();
        super::super::export::render_markdown(&mut io::Cursor::new(raw), &mut expected, &|| Ok(()))
            .unwrap();
        assert_eq!(fs::read(&result.markdown).unwrap(), expected);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(directory).unwrap().permissions().mode() & 0o077,
                0
            );
            assert_eq!(
                fs::metadata(result.markdown).unwrap().permissions().mode() & 0o077,
                0
            );
        }
    }

    #[test]
    fn corruption_truncation_and_replacement_leave_no_published_or_staged_pair() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("exports");
        fs::create_dir(&base).unwrap();
        let sentinel = base.join("keep");
        fs::write(&sentinel, "unchanged").unwrap();
        let source = dir.path().join("source");
        for failure in 0..4 {
            let original = row("original");
            let snapshot = fixture(
                &source,
                if failure == 0 {
                    b"not-json\n"
                } else {
                    &original
                },
            );
            match failure {
                1 => fs::write(&source, b"").unwrap(),
                2 => fs::write(&source, row("modified")).unwrap(),
                3 => fs::remove_file(&source).unwrap(),
                _ => {}
            }
            assert!(export_verified(snapshot, &base, &|| Ok(())).is_err());
            assert_eq!(fs::read_dir(&base).unwrap().count(), 1);
            assert_eq!(fs::read_to_string(&sentinel).unwrap(), "unchanged");
        }
    }

    #[test]
    fn publication_collision_and_mid_copy_cancellation_clean_only_owned_staging() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("exports");
        let source = dir.path().join("source");
        let snapshot = fixture(&source, &row("complete data"));
        let check = || {
            if let Ok(entries) = fs::read_dir(&base) {
                for directory in entries.flatten() {
                    if directory.path().is_dir() {
                        let marker = directory.path().join("complete.json");
                        if !marker.exists() {
                            fs::write(marker, "injected collision")?;
                        }
                    }
                }
            }
            Ok(())
        };
        assert!(export_verified(snapshot, &base, &check).is_err());
        assert_eq!(fs::read_dir(&base).unwrap().count(), 0);
        let snapshot = fixture(&source, &row(&"x".repeat(256 * 1024)));
        let calls = AtomicUsize::new(0);
        assert!(
            export_verified(snapshot, &base, &|| {
                if calls.fetch_add(1, Ordering::Relaxed) == 3 {
                    Err(io::Error::other("cancel fixture"))
                } else {
                    Ok(())
                }
            })
            .is_err()
        );
        assert_eq!(fs::read_dir(&base).unwrap().count(), 0);
    }

    #[test]
    fn pending_checkpoint_start_and_drop_never_wait_for_the_writer() {
        let _guard = EXPORT_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (_sender, reply) = mpsc::channel();
        let (completed, done) = mpsc::channel();
        let started = Instant::now();
        let job = ExportJob::start(
            ExportSource::Transcript(PendingTranscriptCheckpoint {
                path: dir.path().join("never-opened"),
                reply,
            }),
            dir.path().join("exports"),
            Some(Arc::new(move || {
                let _ = completed.send(());
            })),
        )
        .unwrap();
        assert!(job.poll().is_none());
        drop(job);
        assert!(started.elapsed() < Duration::from_millis(100));
        done.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(!dir.path().join("exports").exists());
    }

    #[test]
    fn checked_writer_enforces_caps_short_writes_errors_and_deadlines() {
        struct Short {
            fail: bool,
        }
        impl Write for Short {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if self.fail {
                    return Err(io::Error::other("disk full"));
                }
                self.fail = true;
                Ok(bytes.len().min(2))
            }
            fn flush(&mut self) -> io::Result<()> {
                Err(io::Error::other("flush failed"))
            }
        }
        let check_ok = || Ok(());
        let mut writer = CheckedWriter {
            inner: Short { fail: false },
            bytes: 0,
            check: &check_ok,
        };
        assert!(
            writer
                .write_all(b"abcdef")
                .unwrap_err()
                .to_string()
                .contains("disk full")
        );
        assert_eq!(writer.bytes, 2);
        assert!(writer.flush().is_err());
        writer.bytes = MAX_MARKDOWN_BYTES;
        assert!(
            writer
                .write(b"x")
                .unwrap_err()
                .to_string()
                .contains("128 MiB")
        );
        assert!(check(&AtomicBool::new(false), Instant::now()).is_err());
        assert!(
            check(
                &AtomicBool::new(true),
                Instant::now() + Duration::from_secs(1)
            )
            .is_err()
        );
    }

    #[test]
    fn repeated_exports_never_overwrite_and_scrollback_has_no_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let first = export_verified(
            VerifiedSource::Scrollback("first 界".into()),
            dir.path(),
            &|| Ok(()),
        )
        .unwrap();
        let second = export_verified(
            VerifiedSource::Scrollback("second".into()),
            dir.path(),
            &|| Ok(()),
        )
        .unwrap();
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
    fn process_export_admission_is_bounded_under_concurrent_requests() {
        let counter = Arc::new(AtomicUsize::new(0));
        let ready = Arc::new(std::sync::Barrier::new(17));
        let release = Arc::new(std::sync::Barrier::new(17));
        let workers: Vec<_> = (0..16)
            .map(|_| {
                let counter = counter.clone();
                let ready = ready.clone();
                let release = release.clone();
                std::thread::spawn(move || {
                    let admitted = reserve_slot(&counter).is_ok();
                    ready.wait();
                    release.wait();
                    if admitted {
                        counter.fetch_sub(1, Ordering::AcqRel);
                    }
                })
            })
            .collect();
        ready.wait();
        assert_eq!(counter.load(Ordering::Acquire), MAX_ACTIVE_EXPORTS);
        release.wait();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(counter.load(Ordering::Acquire), 0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn fifo_source_is_rejected_without_waiting_for_a_writer() {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("source.fifo");
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: name is a live C string inside this private fixture directory.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let source = fifo.clone();
        let destination = dir.path().join("unused-copy");
        let (send, receive) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = copy_verified_prefix(
                &source,
                &destination,
                &TranscriptCheckpoint {
                    bytes: 1,
                    sha256: [0; 32],
                    recording_limit_reached: false,
                },
                &|| Ok(()),
            );
            send.send(result).unwrap();
        });
        let mut needed_rescue = false;
        let result = match receive.recv_timeout(Duration::from_millis(300)) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                needed_rescue = true;
                let _peer = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(&fifo)
                    .unwrap();
                receive.recv_timeout(Duration::from_secs(2)).unwrap()
            }
            Err(error) => panic!("FIFO fixture worker failed: {error}"),
        };
        worker.join().unwrap();
        assert!(!needed_rescue, "export waited for a FIFO writer");
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("not a regular file")
        );
        assert!(!dir.path().join("unused-copy").exists());
        assert!(fifo.exists());
    }
}
