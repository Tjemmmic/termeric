//! A complete-frame write budget for the attached relay only, not the bridge.
use super::DomainWriter;
use crate::domain::io_util;
use std::io::Write;
use std::os::fd::{AsRawFd, RawFd};
use std::time::{Duration, Instant};

pub(super) struct Writer {
    inner: DomainWriter,
    timeout: Duration,
    deadline: Option<Instant>,
}

impl Writer {
    pub(super) fn new(inner: DomainWriter) -> std::io::Result<Self> {
        Self::with_timeout(inner, super::RESPONSE_TIMEOUT)
    }

    fn with_timeout(inner: DomainWriter, timeout: Duration) -> std::io::Result<Self> {
        if let DomainWriter::Ssh(pipe) = &inner {
            // This owned stdin pipe is distinct from SSH stdout and has no
            // other writer. Socket clones instead need per-send nonblocking.
            let fd = pipe.as_raw_fd();
            // SAFETY: fd belongs to the live owned pipe; F_GETFL/F_SETFL only
            // read/change its status flags, preserving all flags except adding
            // nonblocking writes. No borrowed descriptor is closed here.
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
            {
                return Err(std::io::Error::last_os_error());
            }
        }
        Ok(Self {
            inner,
            timeout,
            deadline: None,
        })
    }

    fn descriptor(&self) -> RawFd {
        match &self.inner {
            DomainWriter::Local(stream) => stream.as_raw_fd(),
            DomainWriter::Ssh(pipe) => pipe.as_raw_fd(),
        }
    }
}

const EXPIRED: &str = "domain relay frame write deadline elapsed";

impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        // The deadline covers the whole frame, from its first byte to flush.
        let deadline = *self
            .deadline
            .get_or_insert_with(|| Instant::now() + self.timeout);
        let descriptor = self.descriptor();
        let inner = &mut self.inner;
        io_util::write_before(deadline, descriptor, EXPIRED, || match inner {
            DomainWriter::Local(stream) => io_util::send_nonblocking(stream, bytes),
            DomainWriter::Ssh(pipe) => pipe.write(bytes),
        })
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(deadline) = self.deadline {
            io_util::remaining(deadline, EXPIRED)?;
            self.inner.flush()?;
            io_util::remaining(deadline, EXPIRED)?;
            self.deadline = None;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "client_relay_frame_writer_tests.rs"]
mod tests;
