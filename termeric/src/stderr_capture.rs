//! Keeping a bounded part of a child's standard error for its error reports.
//!
//! A thread reads the stream to its end, so the child never blocks on a full
//! pipe, and keeps either the start or the end of it. A report waits only so
//! long for that end: a child's stderr usually closes as soon as it exits,
//! but a command it started outside its process group, or one that outlived
//! the signal sent to the group, can hold the pipe open indefinitely.

use std::collections::VecDeque;
use std::io::Read;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Instant;

/// Which part of the stream is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keep {
    /// The first this many bytes.
    First(usize),
    /// The last this many bytes.
    Last(usize),
}

/// What was kept of the stream.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Kept {
    pub bytes: Vec<u8>,
    /// Some of what was written is not in `bytes`.
    pub truncated: bool,
}

/// A child's standard error, read on a thread of its own.
#[derive(Debug)]
pub struct StderrCapture {
    kept: Arc<Mutex<Buffer>>,
    /// Disconnects once the reader has stopped reading.
    finished: Receiver<()>,
    /// `None` once joined.
    reader: Option<JoinHandle<()>>,
}

#[derive(Debug, Default)]
struct Buffer {
    bytes: VecDeque<u8>,
    truncated: bool,
}

impl StderrCapture {
    /// Starts reading `stream` on a thread called `thread_name`.
    pub fn start(
        stream: impl Read + Send + 'static,
        keep: Keep,
        thread_name: &str,
    ) -> std::io::Result<Self> {
        let kept = Arc::new(Mutex::new(Buffer::default()));
        let target = Arc::clone(&kept);
        let (reading, finished) = mpsc::channel::<()>();
        let reader = thread::Builder::new()
            .name(thread_name.into())
            .spawn(move || {
                // Dropped however reading ends, which disconnects `finished`.
                let _reading = reading;
                read_to_end(stream, keep, &target);
            })?;
        Ok(Self {
            kept,
            finished,
            reader: Some(reader),
        })
    }

    /// What has been kept once the stream has ended, or once `deadline` has
    /// passed while something still holds it open. A reader that misses the
    /// deadline goes on until the stream ends, holding no more than `keep`,
    /// and a later call can still wait for it.
    pub fn kept_by(&mut self, deadline: Instant) -> Kept {
        if let Some(reader) = self.reader.take() {
            let wait = deadline.saturating_duration_since(Instant::now());
            match self.finished.recv_timeout(wait) {
                Err(RecvTimeoutError::Timeout) => self.reader = Some(reader),
                // Nothing is ever sent: only the reader's end disconnects.
                Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                    let _ = reader.join();
                }
            }
        }
        let buffer = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        Kept {
            bytes: buffer.bytes.iter().copied().collect(),
            truncated: buffer.truncated,
        }
    }
}

fn read_to_end(mut stream: impl Read, keep: Keep, target: &Mutex<Buffer>) {
    let mut chunk = [0_u8; 4096];
    loop {
        let read = match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(read) => read,
        };
        let mut buffer = target.lock().unwrap_or_else(PoisonError::into_inner);
        buffer.keep(&chunk[..read], keep);
    }
}

impl Buffer {
    fn keep(&mut self, chunk: &[u8], keep: Keep) {
        match keep {
            Keep::First(limit) => {
                let retained = chunk.len().min(limit.saturating_sub(self.bytes.len()));
                self.bytes.extend(&chunk[..retained]);
                self.truncated |= retained < chunk.len();
            }
            Keep::Last(limit) => {
                self.bytes.extend(chunk);
                let excess = self.bytes.len().saturating_sub(limit);
                self.bytes.drain(..excess);
                self.truncated |= excess > 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::time::Duration;

    fn kept(written: &[u8], keep: Keep) -> Kept {
        let mut capture =
            StderrCapture::start(Cursor::new(written.to_vec()), keep, "test").unwrap();
        capture.kept_by(Instant::now() + Duration::from_secs(10))
    }

    #[test]
    fn the_start_or_the_end_is_kept_within_its_bound() {
        // Longer than one read, so the bound holds across reads.
        let written: Vec<u8> = (0..10_000_u32).map(|byte| byte as u8).collect();
        assert_eq!(
            kept(&written, Keep::First(5_000)),
            Kept {
                bytes: written[..5_000].to_vec(),
                truncated: true
            }
        );
        assert_eq!(
            kept(&written, Keep::Last(5_000)),
            Kept {
                bytes: written[5_000..].to_vec(),
                truncated: true
            }
        );
        for keep in [Keep::First(10_000), Keep::Last(10_000)] {
            assert_eq!(
                kept(&written, keep),
                Kept {
                    bytes: written.clone(),
                    truncated: false
                }
            );
        }
    }

    #[test]
    fn a_stream_held_open_is_waited_for_only_until_the_deadline() {
        let (reader, mut writer) = std::io::pipe().unwrap();
        let mut capture = StderrCapture::start(reader, Keep::Last(64), "test").unwrap();
        std::io::Write::write_all(&mut writer, b"so far").unwrap();
        let wait = Duration::from_millis(200);
        let started = Instant::now();
        capture.kept_by(started + wait);
        assert!(started.elapsed() >= wait);
        assert!(
            capture.reader.is_some(),
            "a reader still reading was joined"
        );

        // Once the stream ends, the reader is waited for and joined.
        std::io::Write::write_all(&mut writer, b", and the end").unwrap();
        drop(writer);
        let started = Instant::now();
        let all = capture.kept_by(started + Duration::from_secs(10));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(all.bytes, b"so far, and the end");
        assert!(capture.reader.is_none());
    }
}
