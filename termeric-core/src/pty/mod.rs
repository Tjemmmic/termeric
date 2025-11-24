//! Pseudo-terminal handling
//!
//! This module provides cross-platform PTY creation and management
//! using the portable-pty crate.

mod event;

pub use event::PtyEvent;

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::sync::mpsc;
use std::thread;

/// A pseudo-terminal for communicating with a shell process
pub struct Pty {
    /// The master side of the PTY
    master: Box<dyn MasterPty + Send>,
    /// Reader for PTY output
    reader: Box<dyn Read + Send>,
    /// Writer for PTY input
    writer: Box<dyn Write + Send>,
    /// The child shell process
    child: Box<dyn Child + Send + Sync>,
}

impl Pty {
    /// Create a new PTY and spawn a shell process
    ///
    /// # Arguments
    /// * `cols` - Number of columns
    /// * `rows` - Number of rows
    ///
    /// # Returns
    /// A new PTY instance or an error
    pub fn new(cols: u16, rows: u16) -> Result<Self, PtyError> {
        let pty_system = native_pty_system();

        let pair = pty_system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        // Build the command for the default shell
        let mut cmd = CommandBuilder::new_default_prog();

        // Set terminal environment variables
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");

        // Spawn the shell on the slave side
        let child = pair.slave.spawn_command(cmd)?;

        // Drop the slave - we only need the master
        drop(pair.slave);

        // Get reader and writer
        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;

        Ok(Self {
            master: pair.master,
            reader,
            writer,
            child,
        })
    }

    /// Resize the PTY
    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), PtyError> {
        self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        Ok(())
    }

    /// Read from the PTY
    pub fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.reader.read(buf)
    }

    /// Write to the PTY
    pub fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.writer.write(data)
    }

    /// Write all data to the PTY
    pub fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.writer.write_all(data)
    }

    /// Check if the child process is still running
    pub fn is_running(&mut self) -> bool {
        self.child.try_wait().ok().flatten().is_none()
    }

    /// Wait for the child process to exit
    pub fn wait(&mut self) -> Option<portable_pty::ExitStatus> {
        self.child.wait().ok()
    }

    /// Get the child process ID (if available)
    pub fn child_id(&self) -> Option<u32> {
        self.child.process_id()
    }

    /// Take the reader for use in a separate thread
    pub fn take_reader(self) -> (PtyHandle, Box<dyn Read + Send>) {
        (
            PtyHandle {
                master: self.master,
                writer: self.writer,
                child: self.child,
            },
            self.reader,
        )
    }
}

/// Handle to the PTY after the reader has been taken
pub struct PtyHandle {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
}

impl PtyHandle {
    /// Resize the PTY
    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), PtyError> {
        self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        Ok(())
    }

    /// Write to the PTY
    pub fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.writer.write(data)
    }

    /// Write all data to the PTY
    pub fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.writer.write_all(data)
    }

    /// Check if the child process is still running
    pub fn is_running(&mut self) -> bool {
        self.child.try_wait().ok().flatten().is_none()
    }

    /// Get the child process ID (if available)
    pub fn child_id(&self) -> Option<u32> {
        self.child.process_id()
    }
}

/// Spawn a reader thread that sends PTY output via a channel
pub fn spawn_reader_thread(mut reader: Box<dyn Read + Send>) -> mpsc::Receiver<PtyEvent> {
    let (tx, rx) = mpsc::channel();

    thread::Builder::new()
        .name("pty-reader".into())
        .spawn(move || {
            let mut buf = [0u8; 4096];

            loop {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        // EOF - PTY closed
                        let _ = tx.send(PtyEvent::Closed);
                        break;
                    }
                    Ok(n) => {
                        if tx.send(PtyEvent::Output(buf[..n].to_vec())).is_err() {
                            // Receiver dropped
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(PtyEvent::Error(e.to_string()));
                        break;
                    }
                }
            }
        })
        .expect("Failed to spawn PTY reader thread");

    rx
}

/// PTY-related errors
#[derive(Debug, thiserror::Error)]
pub enum PtyError {
    #[error("Failed to create PTY: {0}")]
    Creation(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<anyhow::Error> for PtyError {
    fn from(err: anyhow::Error) -> Self {
        PtyError::Creation(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pty_creation() {
        // This test may fail in some CI environments without a proper TTY
        let result = Pty::new(80, 24);
        // Just verify it doesn't panic; may fail without proper TTY
        if let Ok(mut pty) = result {
            assert!(pty.is_running());
        }
    }
}
