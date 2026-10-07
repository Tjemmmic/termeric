//! Reads and writes that must finish before a deadline, for local protocol
//! frames. A per-read timeout alone can be renewed forever by a peer that
//! trickles one byte at a time; these bound the whole frame instead.

use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How often a read that can be cancelled looks at its liveness flag.
const CANCELLATION_POLL_MILLIS: libc::c_int = 500;

/// The time left before `deadline`, or `TimedOut` with `expired` as its
/// message once it has passed.
pub(crate) fn remaining(deadline: Instant, expired: &'static str) -> io::Result<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        Err(io::Error::new(io::ErrorKind::TimedOut, expired))
    } else {
        Ok(remaining)
    }
}

/// Whether `descriptor` has input (or a hangup) within `timeout_millis`; a
/// negative timeout waits indefinitely. An interrupted wait reads as not
/// ready, so callers simply wait again.
pub(crate) fn readable(descriptor: RawFd, timeout_millis: libc::c_int) -> io::Result<bool> {
    poll(descriptor, libc::POLLIN | libc::POLLHUP, timeout_millis)
}

fn poll(descriptor: RawFd, events: libc::c_short, timeout_millis: libc::c_int) -> io::Result<bool> {
    let mut descriptor = libc::pollfd {
        fd: descriptor,
        events,
        revents: 0,
    };
    // SAFETY: poll receives one initialized descriptor for the duration of the
    // call and does not retain its pointer.
    let result = unsafe { libc::poll(&mut descriptor, 1, timeout_millis) };
    if result < 0 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            return Ok(false);
        }
        return Err(error);
    }
    Ok(result > 0)
}

/// A poll timeout for the time left, rounded up to whole milliseconds so a
/// nearly expired deadline still waits for it rather than spinning.
fn poll_millis(left: Duration) -> libc::c_int {
    left.as_micros()
        .div_ceil(1000)
        .clamp(1, libc::c_int::MAX as u128) as libc::c_int
}

/// Connects to the socket at `path` before `deadline`. Connecting blocks only
/// while the listener's backlog is full, so the deadline is checked on both
/// sides of it.
pub(crate) fn connect_before(
    path: &Path,
    deadline: Instant,
    expired: &'static str,
) -> io::Result<UnixStream> {
    remaining(deadline, expired)?;
    let stream = UnixStream::connect(path)?;
    remaining(deadline, expired)?;
    Ok(stream)
}

/// When a deadline-bounded read must finish.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Deadline {
    /// Everything finishes by this instant.
    At(Instant),
    /// Waiting for the first byte has no limit, so an idle stream may stay
    /// open; once input is ready this budget covers the rest, and later reads
    /// never renew it.
    AfterIdle(Duration),
}

impl Deadline {
    /// Everything finishes within `budget` from now.
    pub(crate) fn within(budget: Duration) -> Self {
        Self::At(Instant::now() + budget)
    }

    /// Starts an idle budget now; a fixed deadline stays as it is.
    fn start(&mut self) {
        if let Self::AfterIdle(budget) = *self {
            *self = Self::At(Instant::now() + budget);
        }
    }

    /// The time left, or `None` while still idle.
    fn left(&self, expired: &'static str) -> io::Result<Option<Duration>> {
        match *self {
            Self::At(deadline) => remaining(deadline, expired).map(Some),
            Self::AfterIdle(_) => Ok(None),
        }
    }
}

/// Reads from a socket or pipe by waiting for input with `poll` and then
/// reading once, so it never changes the descriptor's own timeouts or flags.
/// Nothing else may read the descriptor meanwhile, and the reader itself
/// must not buffer ahead, or a read after readiness could block. With a
/// liveness flag, clearing it ends the read within half a second as
/// `ConnectionAborted`.
pub(crate) struct PollReader<'a, R> {
    reader: &'a mut R,
    deadline: Deadline,
    expired: &'static str,
    alive: Option<&'a AtomicBool>,
}

impl<'a, R: Read + AsRawFd> PollReader<'a, R> {
    /// Reads from `reader` by `deadline`, failing as `TimedOut` with
    /// `expired` as its message after it.
    pub(crate) fn new(reader: &'a mut R, deadline: Deadline, expired: &'static str) -> Self {
        Self {
            reader,
            deadline,
            expired,
            alive: None,
        }
    }

    /// Ends the read once `alive` is cleared.
    pub(crate) fn while_alive(mut self, alive: &'a AtomicBool) -> Self {
        self.alive = Some(alive);
        self
    }

    /// Whether the read may go on: still alive and not past its deadline.
    /// A caller checks again after a frame completes, so a frame finished
    /// after cancellation or its deadline is not delivered.
    pub(crate) fn check(&self) -> io::Result<()> {
        if self
            .alive
            .is_some_and(|alive| !alive.load(Ordering::Acquire))
        {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "deadline-bounded read cancelled",
            ));
        }
        self.deadline.left(self.expired).map(drop)
    }
}

impl<R: Read + AsRawFd> Read for PollReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        loop {
            self.check()?;
            let mut timeout = self.deadline.left(self.expired)?.map_or(-1, poll_millis);
            if self.alive.is_some() && !(0..=CANCELLATION_POLL_MILLIS).contains(&timeout) {
                timeout = CANCELLATION_POLL_MILLIS;
            }
            if readable(self.reader.as_raw_fd(), timeout)? {
                self.deadline.start();
                self.check()?;
                return self.reader.read(buffer);
            }
        }
    }
}

/// Writes to a socket before one deadline that covers every write and the
/// final flush, so a peer draining slowly cannot stretch a frame. It never
/// changes the socket's own timeouts or flags, which other threads holding a
/// clone of the descriptor would see.
pub(crate) struct SocketWriter<'a> {
    socket: &'a UnixStream,
    deadline: Instant,
    expired: &'static str,
}

impl<'a> SocketWriter<'a> {
    /// Writes to `socket` before `deadline`, failing as `TimedOut` with
    /// `expired` as its message after it.
    pub(crate) fn new(socket: &'a UnixStream, deadline: Instant, expired: &'static str) -> Self {
        Self {
            socket,
            deadline,
            expired,
        }
    }
}

impl Write for SocketWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let socket = self.socket;
        write_before(self.deadline, socket.as_raw_fd(), self.expired, || {
            send_nonblocking(socket, bytes)
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        remaining(self.deadline, self.expired)?;
        let mut socket = self.socket;
        socket.flush()
    }
}

/// One nonblocking send on a socket whose descriptor other threads share.
/// `MSG_DONTWAIT` applies to this call only: setting `O_NONBLOCK` would change
/// the concurrent reader's clone too, and a blocking send can keep making
/// progress past `SO_SNDTIMEO`.
pub(crate) fn send_nonblocking(socket: &UnixStream, bytes: &[u8]) -> io::Result<usize> {
    // SAFETY: the borrowed socket and byte slice stay live for this call;
    // send reads at most bytes.len() bytes and retains no pointer.
    let sent = unsafe {
        libc::send(
            socket.as_raw_fd(),
            bytes.as_ptr().cast(),
            bytes.len(),
            libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
        )
    };
    if sent < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(sent as usize)
    }
}

/// Runs a nonblocking `attempt` to write to `descriptor` until it makes
/// progress, waiting for writability in between, all before `deadline`.
/// Readiness, interruption, and partial progress never extend the deadline;
/// once it passes the error is `TimedOut` with `expired` as its message.
pub(crate) fn write_before(
    deadline: Instant,
    descriptor: RawFd,
    expired: &'static str,
    mut attempt: impl FnMut() -> io::Result<usize>,
) -> io::Result<usize> {
    loop {
        remaining(deadline, expired)?;
        match attempt() {
            Ok(count) => return Ok(count),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error),
        }
        // A HUP or ERR surfaces from the next attempt.
        poll(
            descriptor,
            libc::POLLOUT,
            poll_millis(remaining(deadline, expired)?),
        )?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    const EXPIRED: &str = "fixture deadline elapsed";

    /// Writes `bytes` to `peer` one at a time, `gap` apart, until the reader
    /// goes away.
    fn trickle(mut peer: UnixStream, bytes: Vec<u8>, gap: Duration) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            for byte in bytes {
                thread::sleep(gap);
                if peer.write_all(&[byte]).is_err() {
                    return;
                }
            }
        })
    }

    fn timed_out(error: io::Error) -> bool {
        error.kind() == io::ErrorKind::TimedOut && error.to_string() == EXPIRED
    }

    #[test]
    fn socket_deadlines_cover_trickled_reads_and_write_backpressure() {
        // Each byte arrives well within any per-read timeout, but the header
        // never completes before the shared deadline.
        let (mut stream, peer) = UnixStream::pair().unwrap();
        let sender = trickle(peer, vec![0; 3], Duration::from_millis(30));
        let deadline = Instant::now() + Duration::from_millis(80);
        let mut reader = PollReader::new(&mut stream, Deadline::At(deadline), EXPIRED);
        assert!(timed_out(reader.read_exact(&mut [0; 4]).unwrap_err()));
        assert!(Instant::now() >= deadline);
        drop(stream);
        sender.join().unwrap();

        let (stream, _peer) = UnixStream::pair().unwrap();
        let deadline = Instant::now() + Duration::from_millis(40);
        let mut writer = SocketWriter::new(&stream, deadline, EXPIRED);
        let chunk = [0; 8192];
        let error = loop {
            if let Err(error) = writer.write_all(&chunk) {
                break error;
            }
        };
        assert!(timed_out(error));
        assert!(timed_out(writer.flush().unwrap_err()));
    }

    #[test]
    fn socket_deadlines_leave_the_socket_policy_alone() {
        let (mut stream, mut peer) = UnixStream::pair().unwrap();
        // SAFETY: F_GETFL only reads flags from the live fixture socket.
        let flags = unsafe { libc::fcntl(stream.as_raw_fd(), libc::F_GETFL) };
        let deadline = Instant::now() + Duration::from_secs(1);
        peer.write_all(b"ping").unwrap();
        let mut received = [0; 4];
        PollReader::new(&mut stream, Deadline::At(deadline), EXPIRED)
            .read_exact(&mut received)
            .unwrap();
        assert_eq!(&received, b"ping");
        let mut writer = SocketWriter::new(&stream, deadline, EXPIRED);
        writer.write_all(b"pong").unwrap();
        writer.flush().unwrap();
        peer.read_exact(&mut received).unwrap();
        assert_eq!(&received, b"pong");
        assert_eq!(stream.read_timeout().unwrap(), None);
        assert_eq!(stream.write_timeout().unwrap(), None);
        // SAFETY: the same live descriptor is retained across the exchange.
        assert_eq!(
            unsafe { libc::fcntl(stream.as_raw_fd(), libc::F_GETFL) },
            flags
        );
    }

    #[test]
    fn idle_time_is_not_a_budget_but_trickled_input_is() {
        let (mut stream, mut peer) = UnixStream::pair().unwrap();
        let mut reader = PollReader::new(
            &mut stream,
            Deadline::AfterIdle(Duration::from_millis(60)),
            EXPIRED,
        );
        // An idle wait three times the budget long is not a timeout.
        let sender = thread::spawn(move || {
            thread::sleep(Duration::from_millis(180));
            peer.write_all(b"ab").unwrap();
            peer
        });
        let mut first = [0; 2];
        reader.read_exact(&mut first).unwrap();
        assert_eq!(&first, b"ab");
        let peer = sender.join().unwrap();
        // Once input has started, trickling cannot renew the budget.
        let sender = trickle(peer, vec![0; 20], Duration::from_millis(10));
        assert!(timed_out(reader.read_exact(&mut [0; 20]).unwrap_err()));
        // Closing the socket releases the trickling writer.
        drop(stream);
        sender.join().unwrap();
    }

    #[test]
    fn poll_reader_deadline_also_bounds_pipes() {
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "printf 'ab'; sleep 1"])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let started = Instant::now();
        let mut reader = PollReader::new(
            &mut stdout,
            Deadline::within(Duration::from_millis(40)),
            EXPIRED,
        );
        assert!(timed_out(reader.read_exact(&mut [0; 4]).unwrap_err()));
        assert!(started.elapsed() < Duration::from_secs(1));
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn clearing_liveness_cancels_an_idle_read() {
        let (mut stream, _peer) = UnixStream::pair().unwrap();
        let alive = AtomicBool::new(true);
        thread::scope(|scope| {
            let reader = scope.spawn(|| {
                PollReader::new(
                    &mut stream,
                    Deadline::AfterIdle(Duration::from_secs(5)),
                    EXPIRED,
                )
                .while_alive(&alive)
                .read(&mut [0; 1])
                .unwrap_err()
                .kind()
            });
            thread::sleep(Duration::from_millis(20));
            alive.store(false, Ordering::Release);
            assert_eq!(reader.join().unwrap(), io::ErrorKind::ConnectionAborted);
        });
    }
}
