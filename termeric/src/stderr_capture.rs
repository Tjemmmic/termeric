//! Keeping the end of a child's standard error for its error reports.
//!
//! A thread reads the stream to its end, so the child never blocks on a full
//! pipe, and keeps the last bytes of it: what a child says just before it
//! fails is why, while what it logged first usually is not. A report waits
//! only so long for that end: a child's stderr usually closes as soon as it
//! exits, but a command it started outside its process group, or one that
//! outlived the signal sent to the group, can hold the pipe open
//! indefinitely.

use std::collections::VecDeque;
use std::io::Read;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Instant;

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
    /// Starts reading `stream` on a thread called `thread_name`, keeping its
    /// last `limit` bytes.
    pub fn start(
        stream: impl Read + Send + 'static,
        limit: usize,
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
                read_to_end(stream, limit, &target);
            })?;
        Ok(Self {
            kept,
            finished,
            reader: Some(reader),
        })
    }

    /// What has been kept once the stream has ended, or once `deadline` has
    /// passed while something still holds it open. A reader that misses the
    /// deadline goes on until the stream ends, holding no more than `limit`,
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

fn read_to_end(mut stream: impl Read, limit: usize, target: &Mutex<Buffer>) {
    let mut chunk = [0_u8; 4096];
    loop {
        let read = match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(read) => read,
        };
        let mut buffer = target.lock().unwrap_or_else(PoisonError::into_inner);
        buffer.keep(&chunk[..read], limit);
    }
}

impl Buffer {
    /// Adds `chunk`, then drops from the front whatever is past `limit`.
    fn keep(&mut self, chunk: &[u8], limit: usize) {
        self.bytes.extend(chunk);
        let excess = self.bytes.len().saturating_sub(limit);
        self.bytes.drain(..excess);
        self.truncated |= excess > 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::time::Duration;

    fn kept(written: &[u8], limit: usize) -> Kept {
        let mut capture =
            StderrCapture::start(Cursor::new(written.to_vec()), limit, "test").unwrap();
        capture.kept_by(Instant::now() + Duration::from_secs(10))
    }

    #[test]
    fn the_end_is_kept_within_its_bound() {
        // Longer than one read, so the bound holds across reads.
        let written: Vec<u8> = (0..10_000_u32).map(|byte| byte as u8).collect();
        assert_eq!(
            kept(&written, 5_000),
            Kept {
                bytes: written[5_000..].to_vec(),
                truncated: true
            }
        );
        assert_eq!(
            kept(&written, 10_000),
            Kept {
                bytes: written.clone(),
                truncated: false
            }
        );
    }

    #[test]
    fn a_stream_held_open_is_waited_for_only_until_the_deadline() {
        let (reader, mut writer) = std::io::pipe().unwrap();
        let mut capture = StderrCapture::start(reader, 64, "test").unwrap();
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
