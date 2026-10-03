//! PTY creation and I/O over portable-pty, and the terminal identity exported
//! to child processes.

pub(crate) mod capabilities;
mod child;
mod environment;
mod event;

pub(crate) use capabilities::TERMERIC_TERM;
pub use child::{ChildExitStatus, ChildObserver};
pub use environment::{EnvironmentPolicy, TERMINAL_ID_VARIABLE, terminal_identity_variable};
pub use event::PtyEvent;
pub use portable_pty::PtySize;

use child::PtyChild;
use portable_pty::{CommandBuilder, MasterPty, native_pty_system};
use std::io::{Read, Write};
use std::sync::{Arc, mpsc};
use std::thread;

/// PTY reader backpressure ceiling. A full queue blocks the reader thread and
/// leaves additional output in the operating-system PTY buffer instead of
/// allocating an unbounded process queue while the application is busy.
pub(crate) const PTY_EVENT_QUEUE_CAPACITY: usize = 256;
/// Bootstrap cell pixels used by constructors that do not yet have renderer
/// metrics. Renderer-backed callers replace this with exact physical geometry.
pub const DEFAULT_CELL_PIXEL_WIDTH: u16 = 8;
pub const DEFAULT_CELL_PIXEL_HEIGHT: u16 = 16;

/// Build a complete nonzero PTY size before definitive renderer metrics exist.
pub fn default_pty_size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: cols.saturating_mul(DEFAULT_CELL_PIXEL_WIDTH).max(1),
        pixel_height: rows.saturating_mul(DEFAULT_CELL_PIXEL_HEIGHT).max(1),
    }
}

/// A pseudo-terminal for communicating with a shell process
pub struct Pty {
    /// Must be dropped before the writer: portable-pty's Unix writer emits
    /// LF + VEOF on drop, which must not submit input to a still-live child.
    /// Rust drops struct fields in declaration order.
    child: PtyChild,
    /// Exact TERM identity exported to this child.
    term_name: &'static str,
    /// Exported as [`TERMINAL_ID_VARIABLE`].
    terminal_id: Option<String>,
    /// The master side of the PTY
    master: Box<dyn MasterPty + Send>,
    /// Reader for PTY output
    reader: Box<dyn Read + Send>,
    /// Writer for PTY input
    writer: Box<dyn Write + Send>,
}

/// What a new PTY runs. The default runs the user's shell with the
/// inherited environment.
#[derive(Debug, Clone, Copy, Default)]
pub struct PtyCommand<'a> {
    /// Program to run; `None` runs the user's shell.
    pub program: Option<&'a str>,
    pub args: &'a [String],
    /// Variables set on top of the inherited environment. `PWD` also sets
    /// the working directory when it names one.
    pub env: &'a [(String, String)],
    /// Inherited variables to clear or remove first; `None` keeps them all.
    /// Unix portable-pty still supplies SHELL, so this is not a sandbox.
    pub policy: Option<&'a EnvironmentPolicy>,
}

impl Pty {
    /// Open a PTY of `size` and start `command` on it.
    pub fn spawn(size: PtySize, command: PtyCommand<'_>) -> Result<Self, PtyError> {
        let PtyCommand {
            program,
            args,
            env,
            policy,
        } = command;
        let inherit = EnvironmentPolicy::default();
        let policy = policy.unwrap_or(&inherit);
        policy
            .validate()
            .map_err(|error| PtyError::Creation(error.into()))?;
        if env.iter().any(|(key, _)| policy.unset.contains(key)) {
            return Err(PtyError::Creation(
                "Environment variable cannot be both set and removed".into(),
            ));
        }
        let pty_system = native_pty_system();

        let pair = pty_system.openpty(size)?;

        // Build the command for the shell
        let mut cmd = match program {
            Some(program) => CommandBuilder::new(program),
            None => CommandBuilder::new_default_prog(),
        };
        if !args.is_empty() {
            cmd.args(args);
        }
        if policy.clear {
            cmd.env_clear();
        }
        for name in &policy.unset {
            cmd.env_remove(name);
        }

        // Advertise the dedicated identity only when its compiled terminfo
        // entry is discoverable. Compatibility mode is the safe fallback.
        let terminal_environment = capabilities::terminal_environment();
        cmd.env("TERM", terminal_environment.term);
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "Termeric");
        cmd.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));
        if let Some(terminfo) = terminal_environment.terminfo {
            cmd.env("TERMINFO", terminfo);
        }
        if let Some(terminfo_dirs) = terminal_environment.terminfo_dirs {
            cmd.env("TERMINFO_DIRS", terminfo_dirs);
        }
        // Always replaced, so a terminal started from another Termeric
        // terminal never inherits its parent's identity.
        cmd.env_remove(TERMINAL_ID_VARIABLE);
        let terminal_id = environment::new_terminal_id();
        if let Some(id) = &terminal_id {
            cmd.env(TERMINAL_ID_VARIABLE, id);
        }

        // Set additional environment variables (e.g., PWD for session restore)
        for (key, value) in env {
            if !terminal_identity_variable(key) {
                cmd.env(key, value);
            }
        }

        // If PWD is set, also set the working directory
        if let Some((_, pwd)) = env.iter().find(|(k, _)| k == "PWD") {
            let path = std::path::Path::new(pwd);
            if path.is_dir() {
                cmd.cwd(path);
            }
        }

        // Spawn the shell on the slave side
        let child = PtyChild::new(pair.slave.spawn_command(cmd)?);

        // Drop the slave - we only need the master
        drop(pair.slave);

        // Get reader and writer
        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;

        Ok(Self {
            term_name: terminal_environment.term,
            terminal_id,
            master: pair.master,
            reader,
            writer,
            child,
        })
    }

    /// Exact TERM identity exported to the child process.
    pub fn term_name(&self) -> &'static str {
        self.term_name
    }

    /// Resize the PTY
    pub fn resize(&self, size: PtySize) -> Result<(), PtyError> {
        if self.master.get_size()? != size {
            self.master.resize(size)?;
        }
        Ok(())
    }

    /// Read the kernel's current PTY geometry (`TIOCGWINSZ` on Unix).
    pub fn size(&self) -> Result<PtySize, PtyError> {
        Ok(self.master.get_size()?)
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

    /// Observe this exact child across reader detachment and terminal cleanup.
    pub fn child_observer(&self) -> ChildObserver {
        self.child.observer()
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
                writer: Some(self.writer),
                child: self.child,
                terminal_id: self.terminal_id,
            },
            self.reader,
        )
    }
}

/// Handle to the PTY after the reader has been taken
pub struct PtyHandle {
    // Keep child shutdown ahead of master/writer release, as in Pty above.
    child: PtyChild,
    master: Box<dyn MasterPty + Send>,
    writer: Option<Box<dyn Write + Send>>,
    terminal_id: Option<String>,
}

impl PtyHandle {
    /// The identity exported to this terminal's processes as
    /// [`TERMINAL_ID_VARIABLE`].
    pub fn terminal_id(&self) -> Option<&str> {
        self.terminal_id.as_deref()
    }

    /// Observe this exact child across pane/window ownership changes and cleanup.
    pub fn child_observer(&self) -> ChildObserver {
        self.child.observer()
    }

    /// Resize the PTY
    pub fn resize(&self, size: PtySize) -> Result<(), PtyError> {
        if self.master.get_size()? != size {
            self.master.resize(size)?;
        }
        Ok(())
    }

    /// Read the kernel's current PTY geometry (`TIOCGWINSZ` on Unix).
    pub fn size(&self) -> Result<PtySize, PtyError> {
        Ok(self.master.get_size()?)
    }

    /// Write to the PTY
    pub fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.writer
            .as_mut()
            .ok_or_else(detached_writer_error)?
            .write(data)
    }

    /// Write all data to the PTY
    pub fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.writer
            .as_mut()
            .ok_or_else(detached_writer_error)?
            .write_all(data)
    }

    /// Detach the PTY writer for an independently owned input worker. The
    /// master/child control handle remains usable for resize and lifecycle
    /// operations, but direct writes then fail explicitly.
    pub fn take_writer(&mut self) -> Option<Box<dyn Write + Send>> {
        self.writer.take()
    }

    /// Check if the child process is still running
    pub fn is_running(&mut self) -> bool {
        self.child.try_wait().ok().flatten().is_none()
    }

    /// Get the child process ID (if available)
    pub fn child_id(&self) -> Option<u32> {
        self.child.process_id()
    }

    /// The process group in the terminal's foreground: the program the
    /// person is interacting with, which may be a child of the shell.
    #[cfg(unix)]
    pub fn foreground_process_group(&self) -> Option<i32> {
        self.master.process_group_leader()
    }

    #[cfg(not(unix))]
    pub fn foreground_process_group(&self) -> Option<i32> {
        None
    }
}

fn detached_writer_error() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::BrokenPipe,
        "PTY writer is owned by another worker",
    )
}

/// Spawn a reader thread that sends PTY output via a channel.
/// Returns the receiver channel. If the thread fails to spawn, the channel
/// is still returned (but with `PtyEvent::Error` already queued) so callers
/// can observe the failure through their normal event path instead of
/// aborting the whole process.
pub fn spawn_reader_thread(reader: Box<dyn Read + Send>) -> mpsc::Receiver<PtyEvent> {
    spawn_reader_thread_with_waker(reader, None)
}

/// Spawn a PTY reader and notify an external event loop after each queued
/// event. The callback runs only after the channel send succeeds, so waking
/// always means there is work available to drain.
pub fn spawn_reader_thread_with_waker(
    mut reader: Box<dyn Read + Send>,
    waker: Option<Arc<dyn Fn() + Send + Sync>>,
) -> mpsc::Receiver<PtyEvent> {
    let (tx, rx) = mpsc::sync_channel(PTY_EVENT_QUEUE_CAPACITY);

    let result = thread::Builder::new()
        .name("pty-reader".into())
        .spawn(move || {
            let mut buf = [0u8; 4096];

            loop {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        // EOF - PTY closed
                        let _ = tx.send(PtyEvent::Closed);
                        if let Some(wake) = &waker {
                            wake();
                        }
                        break;
                    }
                    Ok(n) => {
                        if tx.send(PtyEvent::Output(buf[..n].to_vec())).is_err() {
                            // Receiver dropped
                            break;
                        }
                        if let Some(wake) = &waker {
                            wake();
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(PtyEvent::Error(e.to_string()));
                        if let Some(wake) = &waker {
                            wake();
                        }
                        break;
                    }
                }
            }
        });

    if let Err(e) = result {
        log::error!("Failed to spawn PTY reader thread: {}", e);
        // The tx we would have moved into the thread is dropped, so the
        // channel is already closed. Callers will see `Closed`/`Error` via
        // their usual pump without crashing the process.
    }

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
    use super::child::ChildObservation;
    use super::*;
    use std::io::Cursor;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// Runs only when explicitly selected in a child PTY. Reports no inherited
    /// values or credentials; the normal workspace invocation is a no-op.
    #[cfg(unix)]
    #[test]
    fn environment_policy_child_probe() {
        let Ok(mode) = std::env::var("TERMERIC_POLICY_PROBE") else {
            return;
        };
        assert_eq!(std::env::var("EMPTY_VALUE").unwrap(), "");
        assert_eq!(std::env::var("EXACT_VALUE").unwrap(), "= two words $HOME;λ");
        assert_eq!(std::env::var("TERM_PROGRAM").unwrap(), "Termeric");
        assert_eq!(std::env::var("COLORTERM").unwrap(), "truecolor");
        assert!(std::env::var_os("SHELL").is_some());
        assert_eq!(
            std::env::var_os("PWD").unwrap(),
            std::env::current_dir().unwrap().as_os_str()
        );
        if mode == "inherit" {
            assert_eq!(
                std::env::var_os("PATH").is_some(),
                std::env::var("EXPECTED_PATH_PRESENT").unwrap() == "true"
            );
        } else {
            assert!(std::env::var_os("PATH").is_none());
        }
        if mode == "clear" {
            assert!(
                std::env::vars_os().all(|(name, _)| name.to_str().is_some_and(|name| {
                    terminal_identity_variable(name)
                        || matches!(
                            name,
                            "SHELL"
                                | "PWD"
                                | "TERMERIC_POLICY_PROBE"
                                | "EXPECTED_PATH_PRESENT"
                                | "EMPTY_VALUE"
                                | "EXACT_VALUE"
                        )
                })),
                "cleared child retained an unexpected variable (names/values withheld)"
            );
        }
        println!(
            "TERMINAL_ID={}",
            std::env::var(TERMINAL_ID_VARIABLE).unwrap()
        );
        println!("ENVIRONMENT_POLICY_PROBE_OK");
    }

    #[cfg(unix)]
    #[test]
    fn environment_policy_reaches_real_child_without_changing_parent_or_siblings() {
        let original_path = std::env::var_os("PATH");
        let original_home = std::env::var_os("HOME");
        let executable = std::env::current_exe().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let args = [
            "--exact".into(),
            "pty::tests::environment_policy_child_probe".into(),
            "--nocapture".into(),
        ];
        for (mode, policy) in [
            ("inherit", EnvironmentPolicy::default()),
            (
                "unset",
                EnvironmentPolicy {
                    clear: false,
                    unset: vec!["PATH".into()],
                },
            ),
            (
                "clear",
                EnvironmentPolicy {
                    clear: true,
                    unset: vec![],
                },
            ),
            ("inherit", EnvironmentPolicy::default()),
        ] {
            let env = [
                ("TERMERIC_POLICY_PROBE".into(), mode.into()),
                (
                    "EXPECTED_PATH_PRESENT".into(),
                    original_path.is_some().to_string(),
                ),
                ("PWD".into(), directory.path().to_str().unwrap().into()),
                ("EMPTY_VALUE".into(), String::new()),
                ("EXACT_VALUE".into(), "= two words $HOME;λ".into()),
                ("TERM".into(), "must-not-override-identity".into()),
                (
                    TERMINAL_ID_VARIABLE.into(),
                    "must-not-override-identity".into(),
                ),
            ];
            let pty = Pty::spawn(
                default_pty_size(80, 24),
                PtyCommand {
                    program: executable.to_str(),
                    args: &args,
                    env: &env,
                    policy: Some(&policy),
                },
            )
            .unwrap();
            let observer = pty.child_observer();
            let (handle, reader) = pty.take_reader();
            let rx = spawn_reader_thread(reader);
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let mut output = Vec::new();
            loop {
                let timeout = deadline.saturating_duration_since(std::time::Instant::now());
                match rx
                    .recv_timeout(timeout)
                    .expect("environment child did not finish")
                {
                    PtyEvent::Output(bytes) => {
                        output.extend(bytes);
                        assert!(output.len() < 16384);
                    }
                    PtyEvent::Closed | PtyEvent::Error(_) => break,
                }
            }
            while observer.poll().unwrap().exit.is_none() {
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(
                observer.poll().unwrap().exit.unwrap().success,
                "environment fixture failed: {}",
                String::from_utf8_lossy(&output)
            );
            let output = String::from_utf8_lossy(&output);
            assert!(output.contains("ENVIRONMENT_POLICY_PROBE_OK"));
            let id = handle.terminal_id().expect("a terminal identity");
            assert!(id.starts_with(&format!("{}.", std::process::id())));
            assert!(
                output.contains(&format!("TERMINAL_ID={id}")),
                "the child saw another terminal identity"
            );
            assert_eq!(std::env::var_os("PATH"), original_path);
            assert_eq!(std::env::var_os("HOME"), original_home);
        }
    }

    struct CountingByteReader {
        reads: Arc<AtomicUsize>,
    }

    #[cfg(unix)]
    #[test]
    fn child_observation_survives_wait_detach_and_output_drain() {
        for code in [0, 7] {
            let args = vec![
                "-c".into(),
                format!("printf FINAL_CHILD_OUTPUT; exit {code}"),
            ];
            let mut pty = Pty::spawn(
                default_pty_size(80, 24),
                PtyCommand {
                    program: Some("/bin/sh"),
                    args: &args,
                    ..PtyCommand::default()
                },
            )
            .unwrap();
            let observer = pty.child_observer();
            assert_eq!(pty.wait().unwrap().exit_code(), code);
            assert!(!pty.is_running());
            let expected = ChildExitStatus {
                exit_code: code,
                success: code == 0,
            };
            assert_eq!(observer.poll().unwrap().exit, Some(expected));
            let (handle, reader) = pty.take_reader();
            let rx = spawn_reader_thread(reader);
            let mut output = Vec::new();
            loop {
                match rx.recv_timeout(Duration::from_secs(2)) {
                    Ok(PtyEvent::Output(bytes)) => output.extend(bytes),
                    Ok(PtyEvent::Closed | PtyEvent::Error(_)) => break,
                    other => panic!("reader did not finish: {other:?}"),
                }
            }
            assert_eq!(output, b"FINAL_CHILD_OUTPUT");
            assert!(!observer.poll().unwrap().terminal_closed);
            drop(handle);
            for _ in 0..3 {
                assert_eq!(
                    observer.poll().unwrap(),
                    ChildObservation {
                        exit: Some(expected),
                        terminal_closed: true
                    }
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn terminal_child_shutdown_precedes_writer_drop_for_both_owner_forms() {
        struct DropProbe {
            writer: Option<Box<dyn Write + Send>>,
            observer: ChildObserver,
            report: mpsc::Sender<ChildObservation>,
        }
        impl Write for DropProbe {
            fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
                self.writer.as_mut().unwrap().write(data)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                self.writer.as_mut().unwrap().flush()
            }
        }
        impl Drop for DropProbe {
            fn drop(&mut self) {
                let before = self.observer.poll().unwrap();
                drop(self.writer.take());
                // portable-pty's Unix writer destructor emits LF + VEOF.
                // If it runs first, wait for the ready shell's read to consume
                // that LF, making the old race deterministic without sleeps.
                let deadline = std::time::Instant::now() + Duration::from_secs(2);
                while !before.terminal_closed && self.observer.poll().unwrap().exit.is_none() {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "writer EOF did not reach ready child"
                    );
                    std::thread::yield_now();
                }
                self.report.send(before).unwrap();
            }
        }
        for split in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let submitted = directory.path().join("submitted-input");
            let mut pty = Pty::spawn(
                default_pty_size(80, 24),
                PtyCommand {
                    program: Some("/bin/sh"),
                    args: &[
                        "-c".into(),
                        "printf READY; IFS= read -r value; printf '%s' \"$value\" > \"$1\"".into(),
                        "shutdown-fixture".into(),
                        submitted.to_str().unwrap().into(),
                    ],
                    ..PtyCommand::default()
                },
            )
            .unwrap();
            let observer = pty.child_observer();
            let receiver = spawn_reader_thread(pty.master.try_clone_reader().unwrap());
            let mut output = Vec::new();
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while output != b"READY" {
                let event = receiver
                    .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                    .expect("child readiness missing");
                if let PtyEvent::Output(bytes) = event {
                    output.extend(bytes);
                } else {
                    panic!("child closed before readiness");
                }
            }
            // This canonical input has not been submitted with Enter. Writer
            // destruction must never supply the missing newline during close.
            pty.write_all(b"unsubmitted fixture input").unwrap();
            assert!(!submitted.exists());
            let (report, received) = mpsc::channel();
            let writer = std::mem::replace(&mut pty.writer, Box::new(std::io::sink()));
            pty.writer = Box::new(DropProbe {
                writer: Some(writer),
                observer: observer.clone(),
                report,
            });
            if split {
                let (handle, reader) = pty.take_reader();
                drop(handle);
                drop(reader);
            } else {
                drop(pty);
            }
            let before_writer_drop = received.recv_timeout(Duration::from_secs(2)).unwrap();
            let exit = observer.poll().unwrap();
            assert!(
                before_writer_drop.terminal_closed,
                "split={split}: writer dropped before child shutdown; final status: {exit:?}"
            );
            assert!(exit.terminal_closed);
            assert!(exit.exit.is_some_and(|exit| !exit.success));
            assert!(
                !submitted.exists(),
                "closing submitted buffered input, split={split}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn child_observer_does_not_keep_a_closed_terminal_child_alive() {
        let pty = Pty::spawn(
            default_pty_size(80, 24),
            PtyCommand {
                program: Some("/bin/sh"),
                args: &["-c".into(), "read value".into()],
                ..PtyCommand::default()
            },
        )
        .unwrap();
        let (handle, reader) = pty.take_reader();
        let observer = handle.child_observer();
        assert_eq!(
            observer.poll().unwrap(),
            ChildObservation {
                exit: None,
                terminal_closed: false
            }
        );
        drop(handle);
        drop(reader);
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            let observation = observer.poll().unwrap();
            assert!(observation.terminal_closed);
            if let Some(exit) = observation.exit {
                assert!(!exit.success);
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "closed terminal child remained alive"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    impl Read for CountingByteReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            buffer[0] = b'x';
            Ok(1)
        }
    }

    #[test]
    fn default_command_runs_the_users_shell_at_the_requested_size() {
        let mut pty = Pty::spawn(default_pty_size(80, 24), PtyCommand::default()).unwrap();
        assert!(pty.is_running());
        assert_eq!(pty.size().unwrap(), default_pty_size(80, 24));
    }

    #[cfg(unix)]
    #[test]
    fn child_identity_cannot_be_overridden_by_untrusted_extra_environment() {
        let args = vec![
            "-c".to_string(),
            "printf 'TERM=%s COLORTERM=%s' \"$TERM\" \"$COLORTERM\"".to_string(),
        ];
        let extra = vec![
            ("TERM".to_string(), "not-a-real-terminal".to_string()),
            ("COLORTERM".to_string(), "false".to_string()),
            ("TERMINFO".to_string(), "/untrusted/database".to_string()),
        ];
        let pty = Pty::spawn(
            default_pty_size(80, 24),
            PtyCommand {
                program: Some("/bin/sh"),
                args: &args,
                env: &extra,
                ..PtyCommand::default()
            },
        )
        .unwrap();
        let term_name = pty.term_name();
        let (_handle, reader) = pty.take_reader();
        let rx = spawn_reader_thread(reader);
        let mut output = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            match rx.recv_timeout(std::time::Duration::from_millis(20)) {
                Ok(PtyEvent::Output(bytes)) => output.extend(bytes),
                Ok(PtyEvent::Closed | PtyEvent::Error(_)) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        let output = String::from_utf8_lossy(&output);
        assert!(
            output.contains(&format!("TERM={term_name} COLORTERM=truecolor")),
            "{output:?}"
        );
        assert!(!output.contains("not-a-real-terminal"), "{output:?}");
    }

    #[cfg(unix)]
    #[test]
    fn kernel_winsize_preserves_nonzero_pixels_at_spawn_and_resize() {
        let initial = PtySize {
            cols: 83,
            rows: 27,
            pixel_width: 913,
            pixel_height: 513,
        };
        let args = vec!["-c".to_string(), "sleep 1".to_string()];
        let pty = Pty::spawn(
            initial,
            PtyCommand {
                program: Some("/bin/sh"),
                args: &args,
                ..PtyCommand::default()
            },
        )
        .expect("PTY helper should spawn");
        assert_eq!(pty.size().expect("TIOCGWINSZ at startup"), initial);

        let resized = PtySize {
            cols: 41,
            rows: 19,
            pixel_width: 451,
            pixel_height: 361,
        };
        pty.resize(resized).expect("TIOCSWINSZ should succeed");
        assert_eq!(pty.size().expect("TIOCGWINSZ after resize"), resized);
        // The second call is an exact no-op and must not emit another SIGWINCH.
        pty.resize(resized)
            .expect("duplicate size should be accepted");
        assert_eq!(pty.size().unwrap(), resized);
    }

    #[cfg(unix)]
    #[test]
    fn child_winsize_probe_observes_startup_and_resize_pixels() {
        let python = ["/usr/bin/python3", "/bin/python3"]
            .into_iter()
            .find(|path| std::path::Path::new(path).is_file());
        let Some(python) = python else {
            return;
        };
        let probe = concat!(
            "import fcntl,struct,sys,termios\n",
            "def size(): return struct.unpack('HHHH', fcntl.ioctl(0, termios.TIOCGWINSZ, b'\\0'*8))\n",
            "print('startup=%r' % (size(),), flush=True)\n",
            "sys.stdin.readline()\n",
            "print('resized=%r' % (size(),), flush=True)\n",
        );
        let args = vec!["-c".to_string(), probe.to_string()];
        let initial = PtySize {
            cols: 83,
            rows: 27,
            pixel_width: 913,
            pixel_height: 513,
        };
        let pty = Pty::spawn(
            initial,
            PtyCommand {
                program: Some(python),
                args: &args,
                ..PtyCommand::default()
            },
        )
        .unwrap();
        let (mut handle, reader) = pty.take_reader();
        let rx = spawn_reader_thread(reader);
        let mut output = String::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !output.contains("startup=(27, 83, 913, 513)") && std::time::Instant::now() < deadline
        {
            match rx.recv_timeout(std::time::Duration::from_millis(20)) {
                Ok(PtyEvent::Output(bytes)) => output.push_str(&String::from_utf8_lossy(&bytes)),
                Ok(PtyEvent::Closed | PtyEvent::Error(_)) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        assert!(output.contains("startup=(27, 83, 913, 513)"), "{output:?}");

        let resized = PtySize {
            cols: 41,
            rows: 19,
            pixel_width: 451,
            pixel_height: 361,
        };
        handle.resize(resized).unwrap();
        handle.write_all(b"\n").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !output.contains("resized=(19, 41, 451, 361)") && std::time::Instant::now() < deadline
        {
            match rx.recv_timeout(std::time::Duration::from_millis(20)) {
                Ok(PtyEvent::Output(bytes)) => output.push_str(&String::from_utf8_lossy(&bytes)),
                Ok(PtyEvent::Closed | PtyEvent::Error(_)) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        assert!(output.contains("resized=(19, 41, 451, 361)"), "{output:?}");
    }

    #[test]
    fn reader_wakes_after_queueing_output_and_close() {
        let (wake_tx, wake_rx) = mpsc::channel();
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            wake_tx.send(()).unwrap();
        });
        let rx =
            spawn_reader_thread_with_waker(Box::new(Cursor::new(b"hello".to_vec())), Some(wake));

        assert!(matches!(rx.recv().unwrap(), PtyEvent::Output(data) if data == b"hello"));
        assert!(matches!(rx.recv().unwrap(), PtyEvent::Closed));
        // The reader deliberately queues each event before waking. Receiving
        // the event can therefore race ahead of its callback; wait for both
        // callbacks instead of sampling an atomic at that intermediate point.
        wake_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        wake_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(
            wake_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn reader_applies_backpressure_at_the_bounded_event_queue() {
        let reads = Arc::new(AtomicUsize::new(0));
        let rx = spawn_reader_thread(Box::new(CountingByteReader {
            reads: Arc::clone(&reads),
        }));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while reads.load(Ordering::SeqCst) < PTY_EVENT_QUEUE_CAPACITY + 1
            && std::time::Instant::now() < deadline
        {
            std::thread::yield_now();
        }

        assert_eq!(reads.load(Ordering::SeqCst), PTY_EVENT_QUEUE_CAPACITY + 1);
        std::thread::sleep(std::time::Duration::from_millis(10));
        assert_eq!(
            reads.load(Ordering::SeqCst),
            PTY_EVENT_QUEUE_CAPACITY + 1,
            "reader should block on the 257th send until the app drains output"
        );
        drop(rx);
    }
}
