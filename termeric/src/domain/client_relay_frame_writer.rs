//! A complete-frame write budget for the attached relay only, not the bridge.
use super::DomainWriter;
use std::io::{ErrorKind, Write};
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

    fn remaining(&self) -> std::io::Result<Duration> {
        let remaining = self
            .deadline
            .expect("frame write started")
            .saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            Err(std::io::Error::new(
                ErrorKind::TimedOut,
                "domain relay frame write deadline elapsed",
            ))
        } else {
            Ok(remaining)
        }
    }
}

impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        self.deadline
            .get_or_insert_with(|| Instant::now() + self.timeout);
        loop {
            self.remaining()?;
            let result = match &mut self.inner {
                DomainWriter::Local(stream) => {
                    // SAFETY: live owned socket and readable byte slice; send
                    // retains no pointer and never changes shared socket flags.
                    let count = unsafe {
                        libc::send(
                            stream.as_raw_fd(),
                            bytes.as_ptr().cast(),
                            bytes.len(),
                            libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
                        )
                    };
                    if count < 0 {
                        Err(std::io::Error::last_os_error())
                    } else {
                        Ok(count as usize)
                    }
                }
                DomainWriter::Ssh(pipe) => pipe.write(bytes),
            };
            match result {
                Ok(count) => return Ok(count),
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                Err(error) => return Err(error),
            }
            let timeout = self
                .remaining()?
                .as_millis()
                .saturating_add(1)
                .min(libc::c_int::MAX as u128) as libc::c_int;
            let mut poll = libc::pollfd {
                fd: self.descriptor(),
                events: libc::POLLOUT,
                revents: 0,
            };
            // SAFETY: one initialized pollfd references this owned transport;
            // timeout is finite/nonnegative, and poll retains no pointer.
            let ready = unsafe { libc::poll(&mut poll, 1, timeout) };
            if ready < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() != ErrorKind::Interrupted {
                    return Err(error);
                }
            }
            // Readiness, EINTR, or partial progress never renews the deadline.
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.deadline.is_some() {
            self.remaining()?;
            self.inner.flush()?;
            self.remaining()?;
            self.deadline = None;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "client_relay_frame_writer_tests.rs"]
mod tests;
