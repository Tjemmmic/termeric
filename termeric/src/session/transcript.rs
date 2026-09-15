//! Bounded background transcript recording for AI sessions.
//!
//! Input and PTY paths only enqueue owned entries. A dedicated worker performs
//! redaction, serialization, file creation, writes, periodic flushes, cap
//! accounting, and the durable shutdown flush.

#[cfg(test)]
mod shutdown_tests;
#[cfg(test)]
mod streaming_tests;
mod utf8;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_MAX_FILE_BYTES: usize = 50 * 1024 * 1024;
// A queued entry admits at most 2 MiB of content plus serialized metadata;
// JSON escaping expands bytes by at most six. Leave room for record framing.
pub(crate) const MAX_TRANSCRIPT_RECORD_BYTES: usize = 16 * 1024 * 1024;
const DEFAULT_MAX_QUEUE_BYTES: usize = 2 * 1024 * 1024;
const DEFAULT_QUEUE_ENTRIES: usize = 512;
const FLUSH_INTERVAL: Duration = Duration::from_millis(250);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(500);

type WakeCallback = Arc<dyn Fn() + Send + Sync>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Input,
    Output,
    Diff,
    Command,
    Meta,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptEntry {
    pub timestamp: u64,
    pub kind: EntryKind,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

const REDACTION_PATTERNS: &[&str] = &[
    "sk-",
    "ghp_",
    "gho_",
    "github_pat_",
    "xoxb-",
    "xoxp-",
    "AKIA",
    "eyJ",
];

pub fn redact_secrets(input: &str) -> String {
    // A forward-only scan avoids repeated suffix moves and arbitrary byte
    // cutoffs. This is a known-prefix heuristic, not universal secret detection.
    let mut result = String::with_capacity(input.len());
    let mut scanned = 0;
    let mut untouched = 0;
    while scanned < input.len() {
        if let Some(prefix) = REDACTION_PATTERNS
            .iter()
            .find(|prefix| input[scanned..].starts_with(**prefix))
        {
            let prefix_end = scanned + prefix.len();
            let token_end = input[prefix_end..]
                .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ',' | ';'))
                .map_or(input.len(), |offset| prefix_end + offset);
            result.push_str(&input[untouched..prefix_end]);
            result.push_str("[REDACTED]");
            scanned = token_end;
            untouched = token_end;
        } else {
            // All slice boundaries come from ASCII prefixes or UTF-8 scalars.
            scanned += input[scanned..].chars().next().unwrap().len_utf8();
        }
    }
    result.push_str(&input[untouched..]);
    result
}

fn redact_entry(entry: &mut TranscriptEntry) {
    entry.content = redact_secrets(&entry.content);
    if let Some(metadata) = &mut entry.metadata {
        redact_metadata_values(metadata);
    }
}

fn redact_metadata_values(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(text) => *text = redact_secrets(text),
        serde_json::Value::Array(values) => values.iter_mut().for_each(redact_metadata_values),
        serde_json::Value::Object(values) => values.values_mut().for_each(redact_metadata_values),
        _ => {}
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptWrite {
    Written,
    DroppedAtCap,
    Backpressure,
    DroppedAtBackpressure,
    DroppedAfterFailure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptNotice {
    CapReached { max_bytes: usize },
    Failed(String),
    TextReplaced { input: bool },
}

enum WriterCommand {
    Entry {
        entry: TranscriptEntry,
        accounted_bytes: usize,
    },
    Checkpoint(mpsc::Sender<std::io::Result<TranscriptCheckpoint>>),
}

/// Exact flushed prefix, confirmed in order by the recording worker.
#[derive(Debug, Clone)]
pub(crate) struct TranscriptCheckpoint {
    pub bytes: u64,
    pub sha256: [u8; 32],
    pub recording_limit_reached: bool,
}

pub(crate) struct PendingTranscriptCheckpoint {
    pub path: PathBuf,
    pub reply: Receiver<std::io::Result<TranscriptCheckpoint>>,
}

enum WriterEvent {
    CapReached,
    Failed(String),
}

trait TranscriptSink: Write + Send {
    fn sync_data(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct FileSink(File);

impl Write for FileSink {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0.write(buffer)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

impl TranscriptSink for FileSink {
    fn sync_data(&mut self) -> std::io::Result<()> {
        self.0.sync_data()
    }
}

type SinkFactory = Box<dyn FnOnce() -> std::io::Result<Box<dyn TranscriptSink>> + Send>;

#[derive(Clone, Copy)]
struct RecorderLimits {
    max_file_bytes: usize,
    max_queue_bytes: usize,
    queue_entries: usize,
}

impl Default for RecorderLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            max_queue_bytes: DEFAULT_MAX_QUEUE_BYTES,
            queue_entries: DEFAULT_QUEUE_ENTRIES,
        }
    }
}

pub struct TranscriptRecorder {
    path: PathBuf,
    command_tx: Option<SyncSender<WriterCommand>>,
    event_rx: Receiver<WriterEvent>,
    queued_bytes: Arc<AtomicUsize>,
    worker: Option<JoinHandle<()>>,
    completion_rx: Receiver<std::io::Result<()>>,
    shutdown_result: Option<std::io::Result<()>>,
    shutdown_text_error: Option<std::io::Error>,
    pub entry_count: usize,
    limits: RecorderLimits,
    cap_reached: bool,
    failed: bool,
    backpressure_reported: bool,
    input_decoder: utf8::Decoder,
    output_decoder: utf8::Decoder,
    replacement_seen: u8,
    replacement_pending: u8,
}

impl std::fmt::Debug for TranscriptRecorder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TranscriptRecorder")
            .field("path", &self.path)
            .field("entry_count", &self.entry_count)
            .field("queued_bytes", &self.queued_bytes.load(Ordering::Relaxed))
            .finish()
    }
}

impl TranscriptRecorder {
    pub fn new(
        dir: &Path,
        repo: Option<&str>,
        branch: Option<&str>,
        host: &str,
        profile: &str,
    ) -> std::io::Result<Self> {
        let path = dir.join(format!("transcript_{}.jsonl", super::generate_session_id()));
        let directory = dir.to_path_buf();
        let file_path = path.clone();
        let factory: SinkFactory = Box::new(move || {
            fs::create_dir_all(&directory)?;
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&file_path)?;
            Ok(Box::new(FileSink(file)))
        });
        Self::with_factory(
            path,
            metadata_entry(repo, branch, host, profile),
            RecorderLimits::default(),
            factory,
            crate::app::transcript_event_waker(),
        )
    }

    fn with_factory(
        path: PathBuf,
        metadata: TranscriptEntry,
        limits: RecorderLimits,
        factory: SinkFactory,
        wake: Option<WakeCallback>,
    ) -> std::io::Result<Self> {
        let (command_tx, command_rx) = mpsc::sync_channel(limits.queue_entries.max(1));
        let (event_tx, event_rx) = mpsc::channel();
        let (completion_tx, completion_rx) = mpsc::channel();
        let queued_bytes = Arc::new(AtomicUsize::new(0));
        let worker_queued_bytes = Arc::clone(&queued_bytes);
        let worker = std::thread::Builder::new()
            .name("termeric-transcript".into())
            .spawn(move || {
                let result = transcript_worker(
                    factory,
                    metadata,
                    limits.max_file_bytes,
                    command_rx,
                    event_tx,
                    worker_queued_bytes,
                    wake,
                );
                // Publish only after the worker's writer, sink and queued
                // commands have been dropped, including failure cleanup.
                let _ = completion_tx.send(result);
            })?;

        Ok(Self {
            path,
            command_tx: Some(command_tx),
            event_rx,
            queued_bytes,
            worker: Some(worker),
            completion_rx,
            shutdown_result: None,
            shutdown_text_error: None,
            entry_count: 1,
            limits,
            cap_reached: false,
            failed: false,
            backpressure_reported: false,
            input_decoder: utf8::Decoder::default(),
            output_decoder: utf8::Decoder::default(),
            replacement_seen: 0,
            replacement_pending: 0,
        })
    }

    pub fn record_input(&mut self, data: impl AsRef<[u8]>) -> std::io::Result<TranscriptWrite> {
        self.record_text(true, data.as_ref())
    }

    pub fn record_output(&mut self, data: impl AsRef<[u8]>) -> std::io::Result<TranscriptWrite> {
        self.record_text(false, data.as_ref())
    }

    fn decoder(&mut self, input: bool) -> &mut utf8::Decoder {
        if input {
            &mut self.input_decoder
        } else {
            &mut self.output_decoder
        }
    }

    fn record_text(&mut self, input: bool, data: &[u8]) -> std::io::Result<TranscriptWrite> {
        let kind = if input {
            EntryKind::Input
        } else {
            EntryKind::Output
        };
        if self.cap_reached || self.failed || self.command_tx.is_none() {
            self.decoder(input).clear();
            return self.enqueue(kind, "", None);
        }
        // Borrow valid chunks. Repair produces at most one bounded temporary
        // string and one queue entry, not a queue of tiny replacement fragments.
        let limit = self.limits.max_queue_bytes;
        let Some(decoded) = self.decoder(input).decode(data, limit) else {
            return Ok(self.backpressure());
        };
        if decoded.text.is_empty() {
            return Ok(TranscriptWrite::Written);
        }
        if decoded.replaced {
            self.replacement_notice(input);
        }
        let metadata = decoded
            .replaced
            .then(|| serde_json::json!({ "utf8_replaced": true }));
        let result = self.enqueue(kind, &decoded.text, metadata);
        if !matches!(result, Ok(TranscriptWrite::Written)) {
            self.decoder(input).clear();
        }
        result
    }

    fn replacement_notice(&mut self, input: bool) {
        let flag = if input { 1 } else { 2 };
        if self.replacement_seen & flag == 0 {
            self.replacement_seen |= flag;
            self.replacement_pending |= flag;
        }
    }

    /// Finalize only raw text, without completing pending diff captures or
    /// stopping the worker. EOF, recording stop/replacement and Drop use this
    /// same idempotent path; checkpoint deliberately does not finalize text.
    pub(crate) fn finish_text_streams(&mut self) -> std::io::Result<TranscriptWrite> {
        let mut result = Ok(TranscriptWrite::Written);
        for input in [true, false] {
            if self.decoder(input).finish() {
                self.replacement_notice(input);
                let written = self.enqueue(
                    if input {
                        EntryKind::Input
                    } else {
                        EntryKind::Output
                    },
                    "\u{fffd}",
                    Some(serde_json::json!({ "utf8_replaced": true })),
                );
                result = combine_writes(result, written);
            }
        }
        result
    }

    pub fn record_diff(
        &mut self,
        filename: &str,
        diff_content: &str,
    ) -> std::io::Result<TranscriptWrite> {
        self.enqueue(
            EntryKind::Diff,
            diff_content,
            Some(serde_json::json!({ "filename": filename })),
        )
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Never waits for the writer. A full queue rejects the request rather than
    /// moving the snapshot boundary later while more entries are accepted.
    pub(crate) fn checkpoint(&self) -> std::io::Result<PendingTranscriptCheckpoint> {
        let (reply, receiver) = mpsc::channel();
        let sender = self.command_tx.as_ref().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "transcript writer is stopped",
            )
        })?;
        sender
            .try_send(WriterCommand::Checkpoint(reply))
            .map_err(|error| match error {
                TrySendError::Full(_) => std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "transcript queue is busy; retry export",
                ),
                TrySendError::Disconnected(_) => std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "transcript writer is unavailable",
                ),
            })?;
        Ok(PendingTranscriptCheckpoint {
            path: self.path.clone(),
            reply: receiver,
        })
    }

    #[cfg(test)]
    pub(crate) fn queued_bytes_for_diagnostics(&self) -> usize {
        self.queued_bytes.load(Ordering::Acquire)
    }

    pub fn poll_notices(&mut self) -> Vec<TranscriptNotice> {
        let mut notices = Vec::new();
        let pending = std::mem::take(&mut self.replacement_pending);
        for (flag, input) in [(1, true), (2, false)] {
            if pending & flag != 0 {
                notices.push(TranscriptNotice::TextReplaced { input });
            }
        }
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                WriterEvent::CapReached if !self.cap_reached => {
                    self.cap_reached = true;
                    notices.push(TranscriptNotice::CapReached {
                        max_bytes: self.limits.max_file_bytes,
                    });
                }
                WriterEvent::Failed(error) if !self.failed => {
                    self.failed = true;
                    notices.push(TranscriptNotice::Failed(error));
                }
                _ => {}
            }
        }
        notices
    }

    fn enqueue(
        &mut self,
        kind: EntryKind,
        content: &str,
        metadata: Option<serde_json::Value>,
    ) -> std::io::Result<TranscriptWrite> {
        if self.cap_reached {
            return Ok(TranscriptWrite::DroppedAtCap);
        }
        if self.failed {
            return Ok(TranscriptWrite::DroppedAfterFailure);
        }

        let accounted_bytes = content
            .len()
            .saturating_add(metadata.as_ref().map_or(0, |value| value.to_string().len()))
            .saturating_add(128);
        if !reserve_queue_bytes(
            &self.queued_bytes,
            accounted_bytes,
            self.limits.max_queue_bytes,
        ) {
            return Ok(self.backpressure());
        }

        let command = WriterCommand::Entry {
            entry: TranscriptEntry {
                timestamp: unix_timestamp_ms(),
                kind,
                content: content.to_string(),
                metadata,
            },
            accounted_bytes,
        };
        let Some(command_tx) = &self.command_tx else {
            self.queued_bytes
                .fetch_sub(accounted_bytes, Ordering::AcqRel);
            self.failed = true;
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "transcript writer is stopped",
            ));
        };
        match command_tx.try_send(command) {
            Ok(()) => {
                self.entry_count += 1;
                Ok(TranscriptWrite::Written)
            }
            Err(TrySendError::Full(WriterCommand::Entry {
                accounted_bytes, ..
            })) => {
                self.queued_bytes
                    .fetch_sub(accounted_bytes, Ordering::AcqRel);
                Ok(self.backpressure())
            }
            Err(TrySendError::Disconnected(WriterCommand::Entry {
                accounted_bytes, ..
            })) => {
                self.queued_bytes
                    .fetch_sub(accounted_bytes, Ordering::AcqRel);
                self.failed = true;
                Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "transcript writer stopped unexpectedly",
                ))
            }
            Err(_) => unreachable!("only entry commands are enqueued here"),
        }
    }

    fn backpressure(&mut self) -> TranscriptWrite {
        if self.backpressure_reported {
            TranscriptWrite::DroppedAtBackpressure
        } else {
            self.backpressure_reported = true;
            TranscriptWrite::Backpressure
        }
    }

    pub fn finish(&mut self, timeout: Duration) -> std::io::Result<()> {
        let started = Instant::now();
        if self.command_tx.is_some() {
            self.shutdown_text_error = match self.finish_text_streams() {
                Ok(TranscriptWrite::Written) => None,
                Ok(_) => Some(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "transcript final UTF-8 replacement was not recorded before shutdown",
                )),
                Err(error) => Some(error),
            };
            // Disconnect is ordered after all accepted commands, needs no
            // spare queue slot, and prevents producers from extending shutdown.
            self.command_tx.take();
        }
        if self.shutdown_result.is_none() {
            let result = match self
                .completion_rx
                .recv_timeout(timeout.saturating_sub(started.elapsed()))
            {
                Ok(result) => result,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // Keep the receiver and final-text failure for retries.
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "transcript durable shutdown exceeded timeout",
                    ));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "transcript writer stopped without a completion result",
                )),
            };
            self.shutdown_result =
                Some(result.and_then(|()| self.shutdown_text_error.take().map_or(Ok(()), Err)));
        }
        // A completion confirms persistence and sink cleanup, not that the
        // scheduler has let the final send/return finish. Never block on join.
        if self.worker.as_ref().is_some_and(JoinHandle::is_finished)
            && let Some(worker) = self.worker.take()
        {
            let _ = worker.join();
        }
        match self.shutdown_result.as_ref().unwrap() {
            Ok(()) => Ok(()),
            Err(error) => Err(std::io::Error::new(error.kind(), error.to_string())),
        }
    }
}

fn combine_writes(
    first: std::io::Result<TranscriptWrite>,
    second: std::io::Result<TranscriptWrite>,
) -> std::io::Result<TranscriptWrite> {
    let priority = |value| match value {
        TranscriptWrite::Written => 0,
        TranscriptWrite::DroppedAtBackpressure => 1,
        TranscriptWrite::DroppedAtCap => 2,
        TranscriptWrite::DroppedAfterFailure => 3,
        TranscriptWrite::Backpressure => 4,
    };
    match (first, second) {
        (Err(error), _) | (_, Err(error)) => Err(error),
        (Ok(first), Ok(second)) => Ok(if priority(first) >= priority(second) {
            first
        } else {
            second
        }),
    }
}

impl Drop for TranscriptRecorder {
    fn drop(&mut self) {
        if self.worker.is_some()
            && let Err(error) = self.finish(SHUTDOWN_TIMEOUT)
        {
            log::error!(
                "Transcript {} may be incomplete after bounded shutdown: {}",
                self.path.display(),
                error
            );
        }
    }
}

fn reserve_queue_bytes(counter: &AtomicUsize, amount: usize, maximum: usize) -> bool {
    let mut current = counter.load(Ordering::Acquire);
    loop {
        let Some(next) = current.checked_add(amount) else {
            return false;
        };
        if next > maximum {
            return false;
        }
        match counter.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return true,
            Err(observed) => current = observed,
        }
    }
}

fn metadata_entry(
    repo: Option<&str>,
    branch: Option<&str>,
    host: &str,
    profile: &str,
) -> TranscriptEntry {
    TranscriptEntry {
        timestamp: unix_timestamp_ms(),
        kind: EntryKind::Meta,
        content: String::new(),
        metadata: Some(serde_json::json!({
            "repo": repo,
            "branch": branch,
            "host": host,
            "profile": profile,
            "version": "1.0",
        })),
    }
}

fn unix_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn transcript_worker(
    factory: SinkFactory,
    mut metadata: TranscriptEntry,
    max_file_bytes: usize,
    command_rx: Receiver<WriterCommand>,
    event_tx: mpsc::Sender<WriterEvent>,
    queued_bytes: Arc<AtomicUsize>,
    wake: Option<WakeCallback>,
) -> std::io::Result<()> {
    let sink = match factory() {
        Ok(sink) => sink,
        Err(error) => {
            publish_writer_event(&event_tx, &wake, WriterEvent::Failed(error.to_string()));
            return Err(error);
        }
    };
    let mut writer = BufWriter::new(sink);
    let mut written_bytes = 0usize;
    let mut digest = Sha256::new();
    let mut capped = false;
    let mut dirty = false;
    let mut last_flush = Instant::now();

    redact_entry(&mut metadata);
    if let Err(error) = write_serialized(
        &mut writer,
        &metadata,
        &mut written_bytes,
        &mut digest,
        max_file_bytes,
    ) {
        publish_writer_event(&event_tx, &wake, WriterEvent::Failed(error.to_string()));
        return Err(error);
    }
    if let Err(error) = writer.flush() {
        publish_writer_event(&event_tx, &wake, WriterEvent::Failed(error.to_string()));
        return Err(error);
    }

    loop {
        let wait = FLUSH_INTERVAL.saturating_sub(last_flush.elapsed());
        match command_rx.recv_timeout(wait) {
            Ok(WriterCommand::Entry {
                mut entry,
                accounted_bytes,
            }) => {
                queued_bytes.fetch_sub(accounted_bytes, Ordering::AcqRel);
                if capped {
                    continue;
                }
                redact_entry(&mut entry);
                let line = match serde_json::to_vec(&entry) {
                    Ok(line) => line,
                    Err(error) => {
                        publish_writer_event(
                            &event_tx,
                            &wake,
                            WriterEvent::Failed(format!("serialize transcript: {error}")),
                        );
                        return Err(std::io::Error::other(error));
                    }
                };
                if written_bytes.saturating_add(line.len() + 1) > max_file_bytes {
                    capped = true;
                    publish_writer_event(&event_tx, &wake, WriterEvent::CapReached);
                    continue;
                }
                if line.len() + 1 > MAX_TRANSCRIPT_RECORD_BYTES {
                    let error = std::io::Error::other("transcript record exceeds 16 MiB");
                    publish_writer_event(&event_tx, &wake, WriterEvent::Failed(error.to_string()));
                    return Err(error);
                }
                if let Err(error) = writer
                    .write_all(&line)
                    .and_then(|()| writer.write_all(b"\n"))
                {
                    publish_writer_event(&event_tx, &wake, WriterEvent::Failed(error.to_string()));
                    return Err(error);
                }
                written_bytes += line.len() + 1;
                digest.update(&line);
                digest.update(b"\n");
                dirty = true;
            }
            Ok(WriterCommand::Checkpoint(reply)) => {
                if let Err(error) = writer.flush() {
                    let message = error.to_string();
                    let kind = error.kind();
                    let _ = reply.send(Err(error));
                    publish_writer_event(&event_tx, &wake, WriterEvent::Failed(message.clone()));
                    return Err(std::io::Error::new(kind, message));
                }
                dirty = false;
                last_flush = Instant::now();
                let _ = reply.send(Ok(TranscriptCheckpoint {
                    bytes: written_bytes as u64,
                    sha256: digest.clone().finalize().into(),
                    recording_limit_reached: capped,
                }));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if let Err(error) = writer.flush().and_then(|()| writer.get_mut().sync_data()) {
                    publish_writer_event(&event_tx, &wake, WriterEvent::Failed(error.to_string()));
                    return Err(error);
                }
                return Ok(());
            }
        }

        if dirty && last_flush.elapsed() >= FLUSH_INTERVAL {
            if let Err(error) = writer.flush() {
                publish_writer_event(&event_tx, &wake, WriterEvent::Failed(error.to_string()));
                return Err(error);
            }
            dirty = false;
            last_flush = Instant::now();
        }
    }
}

fn write_serialized(
    writer: &mut BufWriter<Box<dyn TranscriptSink>>,
    entry: &TranscriptEntry,
    written_bytes: &mut usize,
    digest: &mut Sha256,
    max_file_bytes: usize,
) -> std::io::Result<()> {
    let line = serde_json::to_vec(entry)
        .map_err(|error| std::io::Error::other(format!("serialize transcript: {error}")))?;
    if line.len() + 1 > MAX_TRANSCRIPT_RECORD_BYTES.min(max_file_bytes) {
        return Err(std::io::Error::other(
            "initial transcript record exceeds recording limits",
        ));
    }
    writer.write_all(&line)?;
    writer.write_all(b"\n")?;
    *written_bytes += line.len() + 1;
    digest.update(&line);
    digest.update(b"\n");
    Ok(())
}

fn publish_writer_event(
    event_tx: &mpsc::Sender<WriterEvent>,
    wake: &Option<WakeCallback>,
    event: WriterEvent,
) {
    let _ = event_tx.send(event);
    if let Some(wake) = wake {
        wake();
    }
}

#[cfg(test)]
pub(crate) struct TranscriptEnqueueBenchmark {
    pub samples_ms: Vec<f64>,
    pub dropped_entries: Vec<f64>,
}

#[cfg(test)]
struct DelayedDiscardSink;

#[cfg(test)]
impl Write for DelayedDiscardSink {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        std::thread::sleep(Duration::from_millis(2));
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        std::thread::sleep(Duration::from_millis(2));
        Ok(())
    }
}

#[cfg(test)]
impl TranscriptSink for DelayedDiscardSink {}

/// Measure the terminal-facing enqueue path while the injected persistence
/// backend is much slower than the producer. The queue must shed load rather
/// than transferring the sink delay to input or PTY processing.
#[cfg(test)]
pub(crate) fn benchmark_slow_transcript_enqueue() -> std::io::Result<TranscriptEnqueueBenchmark> {
    const SAMPLES: usize = 5;
    const ENTRIES: usize = 1_000;
    let payload = "x".repeat(4_096);
    let mut samples_ms = Vec::with_capacity(SAMPLES);
    let mut dropped_entries = Vec::with_capacity(SAMPLES);

    for sample in 0..SAMPLES {
        let mut recorder = TranscriptRecorder::with_factory(
            PathBuf::from(format!("benchmark-{sample}.jsonl")),
            metadata_entry(None, None, "benchmark", "default"),
            RecorderLimits {
                max_queue_bytes: 64 * 1024,
                queue_entries: 16,
                ..RecorderLimits::default()
            },
            Box::new(|| Ok(Box::new(DelayedDiscardSink))),
            None,
        )?;
        let started = Instant::now();
        let mut dropped = 0usize;
        for entry in 0..ENTRIES {
            let result = if entry % 10 == 0 {
                recorder.record_input("synthetic input")?
            } else {
                recorder.record_output(&payload)?
            };
            if matches!(
                result,
                TranscriptWrite::Backpressure | TranscriptWrite::DroppedAtBackpressure
            ) {
                dropped += 1;
            }
        }
        samples_ms.push(started.elapsed().as_secs_f64() * 1_000.0);
        dropped_entries.push(dropped as f64);
        assert!(
            dropped > 0,
            "slow persistence fixture must apply backpressure"
        );
        recorder.finish(Duration::from_secs(2))?;
    }

    Ok(TranscriptEnqueueBenchmark {
        samples_ms,
        dropped_entries,
    })
}

#[cfg(test)]
impl TranscriptRecorder {
    fn record_command(&mut self, command: &str, exit_code: Option<i32>) {
        let _ = self.enqueue(
            EntryKind::Command,
            command,
            Some(serde_json::json!({ "exit_code": exit_code })),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::sync::Mutex;

    #[test]
    fn checkpoint_is_an_ordered_flushed_prefix_while_recording_continues() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder =
            TranscriptRecorder::new(dir.path(), None, None, "Local", "test").unwrap();
        recorder.record_output("before checkpoint 界").unwrap();
        let pending = recorder.checkpoint().unwrap();
        recorder.record_output("after checkpoint").unwrap();
        let checkpoint = pending
            .reply
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        let mut prefix = Vec::new();
        File::open(&pending.path)
            .unwrap()
            .take(checkpoint.bytes)
            .read_to_end(&mut prefix)
            .unwrap();
        assert_eq!(<[u8; 32]>::from(Sha256::digest(&prefix)), checkpoint.sha256);
        assert!(String::from_utf8_lossy(&prefix).contains("before checkpoint 界"));
        assert!(!String::from_utf8_lossy(&prefix).contains("after checkpoint"));
        assert!(finish_and_read(&mut recorder).contains("after checkpoint"));
        assert!(recorder.checkpoint().is_err());
    }

    #[test]
    fn full_checkpoint_queue_rejects_without_waiting_for_the_writer() {
        let (release, blocked) = mpsc::channel();
        let mut recorder = TranscriptRecorder::with_factory(
            PathBuf::from("checkpoint-fixture"),
            metadata_entry(None, None, "Local", "test"),
            RecorderLimits {
                queue_entries: 1,
                ..RecorderLimits::default()
            },
            Box::new(move || {
                blocked.recv_timeout(Duration::from_secs(2)).unwrap();
                Ok(Box::new(SlowSink {
                    delay: Duration::ZERO,
                    bytes: Arc::new(Mutex::new(Vec::new())),
                }))
            }),
            None,
        )
        .unwrap();
        recorder.record_output("queued before start").unwrap();
        let began = Instant::now();
        let error = recorder
            .checkpoint()
            .err()
            .expect("full queue must reject checkpoint");
        assert!(began.elapsed() < Duration::from_millis(100));
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
        release.send(()).unwrap();
        recorder.finish(Duration::from_secs(2)).unwrap();
    }

    #[test]
    fn checkpoint_flush_failure_is_reported_and_does_not_confirm_a_snapshot() {
        struct FailingFlush {
            flushes: usize,
        }
        impl Write for FailingFlush {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                self.flushes += 1;
                if self.flushes >= 2 {
                    Err(std::io::Error::other("injected checkpoint disk failure"))
                } else {
                    Ok(())
                }
            }
        }
        impl TranscriptSink for FailingFlush {}
        let mut recorder = TranscriptRecorder::with_factory(
            PathBuf::from("failed-checkpoint-fixture"),
            metadata_entry(None, None, "Local", "test"),
            RecorderLimits::default(),
            Box::new(|| Ok(Box::new(FailingFlush { flushes: 0 }))),
            None,
        )
        .unwrap();
        let pending = recorder.checkpoint().unwrap();
        assert!(
            pending
                .reply
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("disk failure")
        );
        recorder.worker.take().unwrap().join().unwrap();
        assert!(matches!(
            recorder.poll_notices().as_slice(),
            [TranscriptNotice::Failed(_)]
        ));
    }

    fn finish_and_read(recorder: &mut TranscriptRecorder) -> String {
        recorder.finish(Duration::from_secs(2)).unwrap();
        fs::read_to_string(recorder.path()).unwrap()
    }

    #[test]
    fn redaction_handles_unicode_at_the_old_byte_cutoff() {
        let secret = format!("sk-{}界 tail", "x".repeat(39));
        assert_eq!(redact_secrets(&secret), "sk-[REDACTED] tail");
    }

    #[test]
    fn redaction_removes_the_entire_long_token() {
        let secret = format!("ghp_{}SECRET_SUFFIX; safe", "a".repeat(200));
        assert_eq!(redact_secrets(&secret), "ghp_[REDACTED]; safe");
    }

    #[test]
    fn redaction_preserves_delimiters_and_unicode_for_every_prefix() {
        for prefix in REDACTION_PATTERNS {
            for delimiter in [" ", "\t", "\n", "\r", "\"", "'", ",", ";", "\u{2003}"] {
                for count in [0, 1, 38, 39, 40, 41, 200] {
                    let input = format!("界🙂 {prefix}{}🙂界{delimiter}safe Ω", "a".repeat(count));
                    let expected = format!("界🙂 {prefix}[REDACTED]{delimiter}safe Ω");
                    assert_eq!(redact_secrets(&input), expected);
                    assert_eq!(redact_secrets(&expected), expected);
                }
            }
            assert_eq!(redact_secrets(prefix), format!("{prefix}[REDACTED]"));
        }
        let safe = "plain 界🙂 text\nwith Unicode whitespace\u{2003}and punctuation;,'\"";
        assert_eq!(redact_secrets(safe), safe);
        assert_eq!(redact_secrets(""), "");
    }

    #[test]
    fn redaction_handles_dense_and_nested_prefixes_without_suffix_leaks() {
        let input = "sk-ghp_not_real,github_pat_sk-also_fake;eyJ.payload.signature ".repeat(16_384);
        let expected = "sk-[REDACTED],github_pat_[REDACTED];eyJ[REDACTED] ".repeat(16_384);
        assert_eq!(redact_secrets(&input), expected);
        let large = format!("xoxb-{}", "Ω".repeat(512 * 1024));
        assert_eq!(redact_secrets(&large), "xoxb-[REDACTED]");
    }

    #[test]
    fn redaction_filters_nested_metadata_values_without_changing_schema() {
        let mut entry = TranscriptEntry {
            timestamp: 123,
            kind: EntryKind::Diff,
            content: "safe content".into(),
            metadata: Some(serde_json::json!({
                "filename": "sk-fabricated",
                "nested": ["gho_fake", {"value": "xoxp-fake", "count": 42}, true, null],
                "safe": "plain 界",
            })),
        };
        redact_entry(&mut entry);
        assert_eq!(entry.timestamp, 123);
        assert_eq!(entry.kind, EntryKind::Diff);
        assert_eq!(entry.content, "safe content");
        assert_eq!(
            entry.metadata.unwrap(),
            serde_json::json!({
                "filename": "sk-[REDACTED]",
                "nested": ["gho_[REDACTED]", {"value": "xoxp-[REDACTED]", "count": 42}, true, null],
                "safe": "plain 界",
            })
        );
    }

    #[test]
    fn recorder_survives_unicode_and_exports_only_redacted_synthetic_values() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = TranscriptRecorder::new(
            dir.path(),
            Some("ghp_FAKE_REPO"),
            Some("sk-FAKE_BRANCH"),
            "xoxb-FAKE_HOST",
            "github_pat_FAKE_PROFILE",
        )
        .unwrap();
        let unicode = format!("sk-{}界 tail", "x".repeat(39));
        recorder.record_input(&unicode).unwrap();
        recorder
            .record_output(format!("eyJ{}FAKE_SUFFIX", "a".repeat(200)))
            .unwrap();
        recorder
            .record_diff("gho_FAKE_FILENAME", "safe diff")
            .unwrap();
        recorder.record_output("safe subsequent output 界").unwrap();
        let contents = finish_and_read(&mut recorder);
        assert!(recorder.poll_notices().is_empty());
        let entries: Vec<TranscriptEntry> = contents
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(entries.len(), 5);
        assert_eq!(
            entries[0].metadata.as_ref().unwrap()["repo"],
            "ghp_[REDACTED]"
        );
        assert_eq!(entries[1].content, "sk-[REDACTED] tail");
        assert_eq!(entries[2].content, "eyJ[REDACTED]");
        assert_eq!(
            entries[3].metadata.as_ref().unwrap()["filename"],
            "gho_[REDACTED]"
        );
        assert_eq!(entries[4].content, "safe subsequent output 界");
        assert!(!contents.contains("FAKE_"));
        assert_eq!(recorder.queued_bytes.load(Ordering::Acquire), 0);
        let markdown = crate::session::export::export_to_markdown(recorder.path()).unwrap();
        assert!(markdown.contains("safe subsequent output 界"));
        assert!(markdown.contains("ghp_[REDACTED]"));
        assert!(!markdown.contains("FAKE_"));
    }

    #[test]
    fn test_redact_secrets_api_keys() {
        let redacted = redact_secrets("My key is sk-abc123def456 and token is ghp_xyzzy789");
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
        let redacted = redact_secrets(r#"Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.payload"#);
        assert!(redacted.contains("eyJ[REDACTED]"));
    }

    #[test]
    fn recorder_batches_entries_and_flushes_them_at_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = TranscriptRecorder::new(
            dir.path(),
            Some("termeric"),
            Some("main"),
            "Local",
            "default",
        )
        .unwrap();
        assert_eq!(
            recorder.record_input("Hello Claude").unwrap(),
            TranscriptWrite::Written
        );
        recorder.record_output("I'll help you with that.").unwrap();
        recorder
            .record_diff("src/main.rs", "+new line\n-old line")
            .unwrap();
        recorder.record_command("cargo test", Some(0));

        let contents = finish_and_read(&mut recorder);
        let entries: Vec<TranscriptEntry> = contents
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(entries.len(), 5);
        assert_eq!(entries[0].kind, EntryKind::Meta);
        assert_eq!(entries[1].kind, EntryKind::Input);
        assert_eq!(entries[1].content, "Hello Claude");
    }

    #[test]
    fn recorder_redacts_on_the_worker_before_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder =
            TranscriptRecorder::new(dir.path(), None, None, "Local", "default").unwrap();
        recorder
            .record_input("export API_KEY=sk-abc123xyz456")
            .unwrap();
        let contents = finish_and_read(&mut recorder);
        assert!(contents.contains("sk-[REDACTED]"));
        assert!(!contents.contains("abc123"));
    }

    #[test]
    fn cap_is_accounted_without_per_entry_metadata_and_reported_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bounded.jsonl");
        let metadata = metadata_entry(None, None, "Local", "default");
        let metadata_bytes = serde_json::to_vec(&metadata).unwrap().len() + 1;
        let file_path = path.clone();
        let (wake_tx, wake_rx) = mpsc::channel();
        let mut recorder = TranscriptRecorder::with_factory(
            path,
            metadata,
            RecorderLimits {
                max_file_bytes: metadata_bytes + 2,
                ..RecorderLimits::default()
            },
            Box::new(move || {
                let file = File::create(&file_path)?;
                Ok(Box::new(FileSink(file)))
            }),
            Some(Arc::new(move || {
                let _ = wake_tx.send(());
            })),
        )
        .unwrap();
        recorder.record_output("too large").unwrap();
        wake_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(
            recorder.poll_notices(),
            vec![TranscriptNotice::CapReached {
                max_bytes: metadata_bytes + 2
            }]
        );
        assert!(recorder.poll_notices().is_empty());
        assert_eq!(
            recorder.record_output("also dropped").unwrap(),
            TranscriptWrite::DroppedAtCap
        );
    }

    struct SlowSink {
        delay: Duration,
        bytes: Arc<Mutex<Vec<u8>>>,
    }

    impl Write for SlowSink {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            std::thread::sleep(self.delay);
            self.bytes.lock().unwrap().extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            std::thread::sleep(self.delay);
            Ok(())
        }
    }

    impl TranscriptSink for SlowSink {}

    struct SyncTrackingSink {
        bytes: Arc<Mutex<Vec<u8>>>,
        syncs: Arc<AtomicUsize>,
    }

    impl Write for SyncTrackingSink {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.bytes.lock().unwrap().extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl TranscriptSink for SyncTrackingSink {
        fn sync_data(&mut self) -> std::io::Result<()> {
            self.syncs.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[test]
    fn clean_shutdown_durably_syncs_the_batched_sink() {
        let storage = Arc::new(Mutex::new(Vec::new()));
        let syncs = Arc::new(AtomicUsize::new(0));
        let sink_storage = Arc::clone(&storage);
        let sink_syncs = Arc::clone(&syncs);
        let mut recorder = TranscriptRecorder::with_factory(
            PathBuf::from("sync-tracking.jsonl"),
            metadata_entry(None, None, "Local", "default"),
            RecorderLimits::default(),
            Box::new(move || {
                Ok(Box::new(SyncTrackingSink {
                    bytes: sink_storage,
                    syncs: sink_syncs,
                }))
            }),
            None,
        )
        .unwrap();

        assert_eq!(
            recorder.record_output("durable output").unwrap(),
            TranscriptWrite::Written
        );
        recorder.finish(Duration::from_secs(2)).unwrap();

        assert_eq!(syncs.load(Ordering::SeqCst), 1);
        let persisted = String::from_utf8(storage.lock().unwrap().clone()).unwrap();
        assert!(persisted.contains("durable output"));
    }

    #[test]
    fn slow_writer_has_bounded_backpressure_without_input_stalls() {
        let storage = Arc::new(Mutex::new(Vec::new()));
        let sink_storage = Arc::clone(&storage);
        let mut recorder = TranscriptRecorder::with_factory(
            PathBuf::from("slow.jsonl"),
            metadata_entry(None, None, "Local", "default"),
            RecorderLimits {
                max_queue_bytes: 1_024,
                queue_entries: 2,
                ..RecorderLimits::default()
            },
            Box::new(move || {
                Ok(Box::new(SlowSink {
                    delay: Duration::from_millis(25),
                    bytes: sink_storage,
                }))
            }),
            None,
        )
        .unwrap();

        let started = Instant::now();
        let results: Vec<_> = (0..1_000)
            .map(|_| recorder.record_output("0123456789").unwrap())
            .collect();
        assert!(started.elapsed() < Duration::from_millis(50));
        assert_eq!(
            results
                .iter()
                .filter(|result| **result == TranscriptWrite::Backpressure)
                .count(),
            1
        );
        assert!(results.contains(&TranscriptWrite::DroppedAtBackpressure));
        recorder.finish(Duration::from_secs(2)).unwrap();
        assert!(!storage.lock().unwrap().is_empty());
    }

    #[test]
    fn shutdown_during_slow_work_returns_at_the_bounded_timeout() {
        let storage = Arc::new(Mutex::new(Vec::new()));
        let sink_storage = Arc::clone(&storage);
        let mut recorder = TranscriptRecorder::with_factory(
            PathBuf::from("slow-shutdown.jsonl"),
            metadata_entry(None, None, "Local", "default"),
            RecorderLimits {
                queue_entries: 2,
                ..RecorderLimits::default()
            },
            Box::new(move || {
                Ok(Box::new(SlowSink {
                    delay: Duration::from_millis(100),
                    bytes: sink_storage,
                }))
            }),
            None,
        )
        .unwrap();
        recorder
            .record_output("queued during slow initialization")
            .unwrap();

        let started = Instant::now();
        let error = recorder.finish(Duration::from_millis(5)).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_millis(50));
    }

    struct FailingSink;

    impl Write for FailingSink {
        fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(
                std::io::ErrorKind::StorageFull,
                "disk full",
            ))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl TranscriptSink for FailingSink {}

    #[test]
    fn worker_failure_is_woken_and_reported_once() {
        let (wake_tx, wake_rx) = mpsc::channel();
        let mut recorder = TranscriptRecorder::with_factory(
            PathBuf::from("failure.jsonl"),
            metadata_entry(None, None, "Local", "default"),
            RecorderLimits::default(),
            Box::new(|| Ok(Box::new(FailingSink))),
            Some(Arc::new(move || {
                let _ = wake_tx.send(());
            })),
        )
        .unwrap();
        wake_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let notices = recorder.poll_notices();
        assert_eq!(notices.len(), 1);
        assert!(
            matches!(&notices[0], TranscriptNotice::Failed(error) if error.contains("disk full"))
        );
        assert!(recorder.poll_notices().is_empty());
        assert_eq!(
            recorder.record_output("dropped").unwrap(),
            TranscriptWrite::DroppedAfterFailure
        );
    }

    #[test]
    fn unavailable_directory_is_reported_asynchronously() {
        let temp = tempfile::tempdir().unwrap();
        let blocked = temp.path().join("not-a-directory");
        fs::write(&blocked, "file").unwrap();
        let (wake_tx, wake_rx) = mpsc::channel();
        let mut recorder = TranscriptRecorder::with_factory(
            blocked.join("transcript.jsonl"),
            metadata_entry(None, None, "Local", "default"),
            RecorderLimits::default(),
            Box::new({
                let blocked = blocked.clone();
                move || {
                    fs::create_dir_all(&blocked)?;
                    unreachable!()
                }
            }),
            Some(Arc::new(move || {
                let _ = wake_tx.send(());
            })),
        )
        .unwrap();
        wake_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(
            recorder.poll_notices().as_slice(),
            [TranscriptNotice::Failed(_)]
        ));
    }
}
