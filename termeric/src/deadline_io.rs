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
    let [ready] = ready_to_read([descriptor], timeout_millis)?;
    Ok(ready)
}

/// Which of `descriptors` a read would not block on (input, end of file, a
/// hangup, or an error the read will report) within `timeout_millis`; a
/// negative timeout waits indefinitely. An interrupted wait reads as none
/// ready.
#[cfg(target_os = "linux")]
pub(crate) fn ready_to_read<const N: usize>(
    descriptors: [RawFd; N],
    timeout_millis: libc::c_int,
) -> io::Result<[bool; N]> {
    poll_ready(descriptors, timeout_millis)
}

/// [`ready_to_read`] with `poll`.
fn poll_ready<const N: usize>(
    descriptors: [RawFd; N],
    timeout_millis: libc::c_int,
) -> io::Result<[bool; N]> {
    let mut polls = descriptors.map(|fd| libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    });
    // SAFETY: poll receives `N` initialized descriptors for the duration of
    // the call and does not retain the pointer.
    let result = unsafe { libc::poll(polls.as_mut_ptr(), N as libc::nfds_t, timeout_millis) };
    if result < 0 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            return Ok([false; N]);
        }
        return Err(error);
    }
    Ok(polls.map(|poll| {
        poll.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0
    }))
}

/// Which of `descriptors` a read would not block on within `timeout_millis`.
/// macOS's `poll` does not support devices, and terminals and serial ports
/// are devices, so a wait that includes one uses `select`, which supports
/// every kind of descriptor (and reports a hangup or an error as readable,
/// for the read to tell). Everything else uses `poll`, which, unlike
/// `select`, takes descriptors beyond `FD_SETSIZE`.
#[cfg(target_os = "macos")]
pub(crate) fn ready_to_read<const N: usize>(
    descriptors: [RawFd; N],
    timeout_millis: libc::c_int,
) -> io::Result<[bool; N]> {
    let is_device = |descriptor: RawFd| {
        // SAFETY: an all-zero stat is valid output storage for fstat.
        let mut status = unsafe { std::mem::zeroed::<libc::stat>() };
        // SAFETY: fstat writes the status of the open descriptor.
        let result = unsafe { libc::fstat(descriptor, &mut status) };
        result == 0 && status.st_mode & libc::S_IFMT == libc::S_IFCHR
    };
    if descriptors.iter().copied().any(is_device) {
        select_ready(descriptors, timeout_millis)
    } else {
        poll_ready(descriptors, timeout_millis)
    }
}

/// [`ready_to_read`] with `select`, for terminals and devices on macOS.
#[cfg(target_os = "macos")]
fn select_ready<const N: usize>(
    descriptors: [RawFd; N],
    timeout_millis: libc::c_int,
) -> io::Result<[bool; N]> {
    if let Some(fd) = descriptors
        .iter()
        .find(|fd| !(0..libc::FD_SETSIZE as RawFd).contains(*fd))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("descriptor {fd} is out of select's range"),
        ));
    }
    // SAFETY: an all-zero fd_set is the empty set, and FD_SET receives
    // descriptors checked to lie within FD_SETSIZE.
    let mut readable = unsafe { std::mem::zeroed::<libc::fd_set>() };
    for fd in descriptors {
        unsafe { libc::FD_SET(fd, &mut readable) };
    }
    let mut timeout = (timeout_millis >= 0).then(|| libc::timeval {
        tv_sec: (timeout_millis / 1000).into(),
        tv_usec: (timeout_millis % 1000) * 1000,
    });
    let highest = descriptors.iter().copied().max().unwrap_or(-1);
    // SAFETY: the set and the optional timeout outlive the call, and nfds is
    // one more than the highest descriptor in the set.
    let result = unsafe {
        libc::select(
            highest + 1,
            &mut readable,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            timeout.as_mut().map_or(std::ptr::null_mut(), |timeout| {
                timeout as *mut libc::timeval
            }),
        )
    };
    if result < 0 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            return Ok([false; N]);
        }
        return Err(error);
    }
    // SAFETY: FD_ISSET reads the set select filled in, for checked descriptors.
    Ok(descriptors.map(|fd| unsafe { libc::FD_ISSET(fd, &readable) }))
}

/// Waits up to `timeout_millis` for `descriptor`, a socket or a pipe, to
/// take more output. Only sockets and pipes come here, which `poll` supports
/// on macOS too.
fn writable(descriptor: RawFd, timeout_millis: libc::c_int) -> io::Result<()> {
    let mut descriptor = libc::pollfd {
        fd: descriptor,
        events: libc::POLLOUT,
        revents: 0,
    };
    // SAFETY: poll receives one initialized descriptor for the duration of the
    // call and does not retain its pointer.
    let result = unsafe { libc::poll(&mut descriptor, 1, timeout_millis) };
    if result < 0 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
    Ok(())
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
/// changes the socket's own timeouts, which other threads holding a clone of
/// the descriptor would see; on macOS it makes the socket nonblocking for
/// the length of each send ([`send_nonblocking`]), so a thread reading a
/// clone reads through [`WaitingReader`] or [`PollReader`].
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
#[cfg(target_os = "linux")]
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

/// One nonblocking send on a socket whose descriptor other threads share.
/// macOS's `send` ignores `MSG_DONTWAIT` (its manual names only `MSG_OOB` and
/// `MSG_DONTROUTE`) and would block on a full socket, so the descriptor is
/// made nonblocking for this one call and then put back as it was. A reader
/// sharing it only reads after `ready_to_read` says it can (`PollReader`), so
/// the moment doesn't change what it sees. A full Unix socket may also refuse
/// with `ENOBUFS`, which is the same as would-block here. SIGPIPE needs no
/// flag: Rust's runtime ignores it, so a closed peer is `EPIPE`.
#[cfg(target_os = "macos")]
pub(crate) fn send_nonblocking(socket: &UnixStream, bytes: &[u8]) -> io::Result<usize> {
    let descriptor = socket.as_raw_fd();
    // SAFETY: F_GETFL reads the flags of the live borrowed descriptor.
    let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    let was_blocking = flags & libc::O_NONBLOCK == 0;
    // SAFETY: as above, setting flags on the same live descriptor.
    if was_blocking
        && unsafe { libc::fcntl(descriptor, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the borrowed socket and byte slice stay live for this call;
    // send reads at most bytes.len() bytes and retains no pointer.
    let sent = unsafe { libc::send(descriptor, bytes.as_ptr().cast(), bytes.len(), 0) };
    let result = if sent < 0 {
        let error = io::Error::last_os_error();
        Err(if error.raw_os_error() == Some(libc::ENOBUFS) {
            io::Error::from(io::ErrorKind::WouldBlock)
        } else {
            error
        })
    } else {
        Ok(sent as usize)
    };
    // SAFETY: as above; the flags read before the send are put back.
    if was_blocking && unsafe { libc::fcntl(descriptor, libc::F_SETFL, flags) } < 0 {
        return Err(io::Error::last_os_error());
    }
    result
}

/// Blocking reads that stay blocking while another thread's
/// [`send_nonblocking`] has, on macOS, made the shared socket nonblocking for
/// a moment: a read that would block waits until there is something to read
/// and reads again, instead of failing.
pub(crate) struct WaitingReader<'a, R: Read + AsRawFd>(pub(crate) &'a mut R);

impl<R: Read + AsRawFd> Read for WaitingReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        read_waiting(self.0, buffer)
    }
}

/// One [`WaitingReader`] read.
pub(crate) fn read_waiting(
    reader: &mut (impl Read + AsRawFd),
    buffer: &mut [u8],
) -> io::Result<usize> {
    loop {
        match reader.read(buffer) {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                ready_to_read([reader.as_raw_fd()], -1)?;
            }
            result => return result,
        }
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
        writable(descriptor, poll_millis(remaining(deadline, expired)?))?;
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
