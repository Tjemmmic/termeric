//! Raw mode for the terminal a bridge runs in. The domain relay and the
//! serial bridge pass every byte straight through, so their terminal must
//! neither echo nor edit what is typed.

use std::io;
use std::os::fd::RawFd;

/// Holds a terminal in raw mode and restores its settings when dropped.
/// Input that is not a terminal, such as a pipe in automation, is left as
/// it is.
pub(crate) struct RawTerminal {
    descriptor: RawFd,
    original: Option<libc::termios>,
}

impl RawTerminal {
    /// Puts the terminal on `descriptor`, which must stay open while the
    /// guard lives, into raw mode.
    pub(crate) fn enter(descriptor: RawFd) -> io::Result<Self> {
        // SAFETY: zeroed termios is valid output storage for tcgetattr.
        let mut original = unsafe { std::mem::zeroed::<libc::termios>() };
        // SAFETY: descriptor is open and original is writable storage.
        if unsafe { libc::tcgetattr(descriptor, &mut original) } != 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ENOTTY) {
                return Ok(Self {
                    descriptor,
                    original: None,
                });
            }
            return Err(error);
        }
        let mut raw = original;
        // SAFETY: raw is initialized, writable termios storage.
        unsafe { libc::cfmakeraw(&mut raw) };
        // SAFETY: descriptor is an open terminal and raw is initialized.
        if unsafe { libc::tcsetattr(descriptor, libc::TCSANOW, &raw) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            descriptor,
            original: Some(original),
        })
    }
}

impl Drop for RawTerminal {
    fn drop(&mut self) {
        let Some(original) = &self.original else {
            return;
        };
        // SAFETY: the settings came from this descriptor, which the caller
        // keeps open while the guard lives.
        if unsafe { libc::tcsetattr(self.descriptor, libc::TCSANOW, original) } != 0 {
            log::warn!(
                "Could not restore terminal attributes: {}",
                io::Error::last_os_error()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::os::fd::{AsRawFd, FromRawFd};

    fn local_flags(descriptor: RawFd) -> libc::tcflag_t {
        // SAFETY: zeroed termios is valid output storage for tcgetattr.
        let mut value = unsafe { std::mem::zeroed::<libc::termios>() };
        // SAFETY: descriptor is a live terminal and value is writable.
        assert_eq!(unsafe { libc::tcgetattr(descriptor, &mut value) }, 0);
        value.c_lflag
    }

    #[test]
    fn a_terminal_is_raw_while_held_and_restored_after() {
        let (mut master, mut slave) = (-1, -1);
        // SAFETY: both outputs are writable integers; null name, termios,
        // and winsize ask for the kernel's defaults.
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            },
            0
        );
        // SAFETY: each new descriptor is adopted exactly once.
        let (_master, slave) = unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) };
        let cooked = local_flags(slave.as_raw_fd());
        assert_ne!(cooked & libc::ECHO, 0);
        let raw = RawTerminal::enter(slave.as_raw_fd()).unwrap();
        assert_eq!(
            local_flags(slave.as_raw_fd()) & (libc::ECHO | libc::ICANON),
            0
        );
        drop(raw);
        assert_eq!(local_flags(slave.as_raw_fd()), cooked);
    }

    #[test]
    fn input_that_is_not_a_terminal_is_left_alone() {
        let (reader, _writer) = std::os::unix::net::UnixStream::pair().unwrap();
        drop(RawTerminal::enter(reader.as_raw_fd()).unwrap());
    }
}
