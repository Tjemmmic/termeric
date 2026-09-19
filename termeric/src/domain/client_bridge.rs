//! Scoped, bidirectional forwarding between SSH stdio and the local daemon.
use super::relay_workers::Completion;
use super::{DomainCloser, copy_with_flush, stdin_ready};
use anyhow::Context;
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

pub(super) fn clone_input(descriptor: RawFd) -> std::io::Result<std::fs::File> {
    // Keep negotiation and forwarding unbuffered: a StdinLock could prefetch
    // request bytes that descriptor readiness would no longer see afterward.
    // SAFETY: fcntl duplicates an open descriptor without changing/closing it;
    // a successful new CLOEXEC descriptor is adopted exactly once below.
    let duplicate = unsafe { libc::fcntl(descriptor, libc::F_DUPFD_CLOEXEC, 0) };
    if duplicate < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { std::fs::File::from_raw_fd(duplicate) })
}

struct InputFailure<'a> {
    alive: &'a AtomicBool,
    closer: &'a DomainCloser,
    armed: bool,
}

impl Drop for InputFailure<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.alive.store(false, Ordering::Release);
            self.closer.close();
        }
    }
}

pub(super) fn run<I: Read + AsRawFd>(
    mut stream: UnixStream,
    output: &mut impl Write,
    input: impl FnOnce() -> I + Send,
) -> anyhow::Result<()> {
    let mut writer = stream.try_clone()?;
    let closer = DomainCloser::Local(stream.try_clone()?);
    let alive = AtomicBool::new(true);
    thread::scope(|scope| -> anyhow::Result<()> {
        // This guard drops before scope joins on every exit, including spawn
        // failure and output panic, waking socket writes and cancelling input.
        let completion = Completion {
            alive: &alive,
            closer: &closer,
        };
        let input_worker = thread::Builder::new()
            .name("termeric-domain-ssh-input".into())
            .spawn_scoped(scope, || -> anyhow::Result<()> {
                let mut failure = InputFailure {
                    alive: &alive,
                    closer: &closer,
                    armed: true,
                };
                let mut input = input();
                match copy_input(&mut input, &mut writer, &alive) {
                    Ok(true) => {
                        if !alive.load(Ordering::Acquire) {
                            return Ok(());
                        }
                        // EOF is not a failure: allow already-requested daemon
                        // replies to drain before the output side closes.
                        writer
                            .shutdown(Shutdown::Write)
                            .context("half-close SSH bridge input")?;
                        failure.armed = false;
                        Ok(())
                    }
                    Ok(false) => Ok(()),
                    Err(_) if !alive.load(Ordering::Acquire) => {
                        // Output-side shutdown interrupted I/O.
                        Ok(())
                    }
                    Err(error) => Err(error).context("forward SSH input to local domain"),
                }
            })
            .context("start SSH bridge input worker")?;
        let output_result = copy_with_flush(&mut stream, output)
            .context("forward remote domain frames to SSH stdout")
            .and_then(|_| output.flush().context("flush SSH bridge output"));
        drop(completion);
        let input_result = input_worker
            .join()
            .map_err(|_| anyhow::anyhow!("SSH bridge input worker panicked"))?;
        input_result?;
        output_result
    })
}

/// true means input EOF; false means output-side cancellation. Do not consume
/// or forward more input after cancellation is observed.
fn copy_input(
    input: &mut (impl Read + AsRawFd),
    writer: &mut impl Write,
    alive: &AtomicBool,
) -> std::io::Result<bool> {
    let mut bytes = [0; 16 * 1024];
    while alive.load(Ordering::Acquire) {
        if !stdin_ready(input.as_raw_fd(), 500)? {
            continue;
        }
        if !alive.load(Ordering::Acquire) {
            break;
        }
        let count = match input.read(&mut bytes) {
            Ok(0) => return Ok(true),
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if !alive.load(Ordering::Acquire) {
            break;
        }
        writer.write_all(&bytes[..count])?;
        writer.flush()?;
    }
    Ok(false)
}

#[cfg(test)]
#[path = "client_bridge_tests.rs"]
mod tests;
