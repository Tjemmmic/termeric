//! Owning an agent process and its line-based pipes.
//!
//! The agent runs as its own process group with piped stdin, stdout, and
//! stderr. Three threads keep the UI thread from ever blocking on it: one
//! writes queued input, one splits stdout into lines, and one keeps the end of
//! stderr for error reports. The reader wakes the event loop after each line;
//! the panel drains lines with [`AgentProcess::try_next`].

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::line_reader::read_bounded_line;
use crate::process_group::{PipedChild, Signal};
use crate::stderr_capture::StderrCapture;

use super::event::ExitReport;

/// Longest single protocol line kept. Claude Code already shortens large tool
/// output; a longer line is reported as [`ProcessOutput::Oversized`] and
/// dropped, holding at most this much of it while it is read.
pub const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;
/// Capacity the line buffer keeps between lines, so one huge line doesn't
/// hold its memory until the agent exits.
const RETAINED_LINE_CAPACITY: usize = 64 * 1024;
/// Lines waiting for the panel before the reader stops reading, which in turn
/// makes the agent wait instead of growing memory.
const OUTPUT_CAPACITY: usize = 4096;
/// Input lines waiting to be written.
const INPUT_CAPACITY: usize = 256;
/// How much of stderr is kept for error reports.
const STDERR_TAIL_BYTES: usize = 16 * 1024;
/// How long a closing agent may take to exit after SIGTERM.
const TERMINATE_GRACE: Duration = Duration::from_secs(2);
/// How long to wait for the exit status after stdout closes.
const EXIT_WAIT: Duration = Duration::from_secs(5);
/// How long the exit report waits for stderr to close once the agent has
/// exited or been killed.
const STDERR_CLOSE_WAIT: Duration = Duration::from_secs(1);

/// Everything needed to start an agent process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// Added to Termeric's own environment.
    pub env: Vec<(String, String)>,
    /// Removed from Termeric's own environment.
    pub env_remove: Vec<String>,
}

/// What the agent wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcessOutput {
    Line(String),
    /// A line longer than [`MAX_LINE_BYTES`] was dropped.
    Oversized,
    /// The agent closed its output and ended. Always the last output.
    Exited(ExitReport),
}

/// Why a line could not be sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendError {
    /// The agent is not reading fast enough; try again later.
    Busy,
    /// The agent's input is closed.
    Closed,
}

pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// A running agent process.
pub struct AgentProcess {
    child: Arc<Mutex<Child>>,
    process_id: u32,
    input: Option<SyncSender<String>>,
    output: Receiver<ProcessOutput>,
    stopped: Arc<AtomicBool>,
    exited: bool,
}

impl std::fmt::Debug for AgentProcess {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentProcess")
            .field("process_id", &self.process_id)
            .field("exited", &self.exited)
            .finish_non_exhaustive()
    }
}

impl AgentProcess {
    /// Starts the agent. `waker` runs on a background thread after each output.
    pub fn spawn(spec: &LaunchSpec, waker: Waker) -> std::io::Result<Self> {
        let mut command = Command::new(&spec.program);
        command.args(&spec.args).current_dir(&spec.cwd);
        for name in &spec.env_remove {
            command.env_remove(name);
        }
        for (name, value) in &spec.env {
            command.env(name, value);
        }
        // Its own group, so closing the agent also closes the commands it
        // started, and a terminal's Ctrl+C never reaches it.
        let PipedChild {
            child,
            stdin,
            stdout,
            stderr,
        } = crate::process_group::spawn_piped(&mut command)?;
        let process_id = child.id();
        let child = Arc::new(Mutex::new(child));
        let stopped = Arc::new(AtomicBool::new(false));
        let (input, output) = match serve_pipes(stdin, stdout, stderr, waker, &child, &stopped) {
            Ok(pipes) => pipes,
            Err(error) => {
                // Nothing would read its output or reap it.
                abandon(&child);
                return Err(error);
            }
        };
        Ok(Self {
            child,
            process_id,
            input: Some(input),
            output,
            stopped,
            exited: false,
        })
    }

    /// Queues one line (which must end in a newline) for the agent. A line
    /// that cannot be queued is handed back with the reason.
    pub fn send(&self, line: String) -> Result<(), (SendError, String)> {
        debug_assert!(line.ends_with('\n'));
        let Some(input) = self.input.as_ref() else {
            return Err((SendError::Closed, line));
        };
        input.try_send(line).map_err(|error| match error {
            TrySendError::Full(line) => (SendError::Busy, line),
            TrySendError::Disconnected(line) => (SendError::Closed, line),
        })
    }

    /// The next output, if any has arrived.
    pub fn try_next(&mut self) -> Option<ProcessOutput> {
        if self.exited {
            return None;
        }
        match self.output.try_recv() {
            Ok(output) => {
                if matches!(output, ProcessOutput::Exited(_)) {
                    self.exited = true;
                }
                Some(output)
            }
            Err(TryRecvError::Empty) => None,
            // The reader always reports the exit before it ends.
            Err(TryRecvError::Disconnected) => {
                self.exited = true;
                None
            }
        }
    }

    /// The agent's process id, which leads its process group.
    pub fn process_id(&self) -> u32 {
        self.process_id
    }

    #[cfg(test)]
    pub fn has_exited(&self) -> bool {
        self.exited
    }

    /// Ends the agent and everything it started, without waiting for it.
    /// Its exit is still reported through [`AgentProcess::try_next`].
    pub fn terminate(&mut self) {
        self.input = None;
        if self.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        signal_agent_group(&self.child, Signal::Terminate);
        let child = Arc::clone(&self.child);
        let _ = thread::Builder::new()
            .name("agent-view-reaper".into())
            .spawn(move || {
                if wait_for_exit(&child, TERMINATE_GRACE).is_none() {
                    signal_agent_group(&child, Signal::Kill);
                    let _ = wait_for_exit(&child, Duration::MAX);
                }
            });
    }
}

impl AgentProcess {
    /// Ends the agent and runs `then` on a background thread once it has
    /// exited, as reported by its output reader, which reaps it first.
    pub fn finish_then(mut self, then: impl FnOnce() + Send + 'static) {
        let (_, closed) = mpsc::sync_channel(0);
        let output = std::mem::replace(&mut self.output, closed);
        let exited = self.exited;
        self.terminate();
        drop(self);
        let _ = thread::Builder::new()
            .name("agent-view-finish".into())
            .spawn(move || {
                if !exited {
                    // Draining also keeps the reader from blocking on a full
                    // queue before it can report the exit.
                    for output in output.iter() {
                        if matches!(output, ProcessOutput::Exited(_)) {
                            break;
                        }
                    }
                }
                then();
            });
    }
}

impl Drop for AgentProcess {
    fn drop(&mut self) {
        self.terminate();
    }
}

/// Starts the threads that serve the agent's pipes: one writes the input
/// queued for it, one keeps the end of its stderr, and one reads its output
/// lines and finally reaps it. Returns the input queue and the output.
fn serve_pipes(
    stdin: ChildStdin,
    stdout: ChildStdout,
    stderr: ChildStderr,
    waker: Waker,
    child: &Arc<Mutex<Child>>,
    stopped: &Arc<AtomicBool>,
) -> std::io::Result<(SyncSender<String>, Receiver<ProcessOutput>)> {
    let (input, input_queue) = mpsc::sync_channel::<String>(INPUT_CAPACITY);
    thread::Builder::new()
        .name("agent-view-input".into())
        .spawn(move || write_input(stdin, input_queue))?;

    let stderr = StderrCapture::start(stderr, STDERR_TAIL_BYTES, "agent-view-stderr")?;

    let (output_sender, output) = mpsc::sync_channel(OUTPUT_CAPACITY);
    let reader_child = Arc::clone(child);
    let reader_stopped = Arc::clone(stopped);
    thread::Builder::new()
        .name("agent-view-output".into())
        .spawn(move || {
            read_output(
                stdout,
                &output_sender,
                &waker,
                &reader_child,
                &reader_stopped,
                stderr,
            );
        })?;
    Ok((input, output))
}

/// Kills an agent that cannot be served, and everything in its group, and
/// reaps it. SIGKILL cannot be caught, so it exits at once; the wait is
/// bounded all the same, as it holds up the thread that started the agent.
fn abandon(child: &Mutex<Child>) {
    signal_agent_group(child, Signal::Kill);
    if wait_for_exit(child, EXIT_WAIT).is_none() {
        log::warn!("agent view: an agent that could not be served did not exit when killed");
    }
}

fn write_input(mut stdin: ChildStdin, queue: Receiver<String>) {
    // Ends when the sender is dropped (input closed) or the agent stops
    // reading; dropping `stdin` then gives the agent end-of-file.
    for line in queue {
        if stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.flush())
            .is_err()
        {
            break;
        }
    }
}

/// The agent's next output line from `reader`, read through `line`: blank
/// lines are skipped, a line over `limit` bytes becomes
/// [`ProcessOutput::Oversized`] and reading goes on, and a last line without
/// a newline still counts. `None` once the output ends or fails.
fn next_output(
    reader: &mut impl BufRead,
    limit: usize,
    line: &mut Vec<u8>,
) -> Option<ProcessOutput> {
    loop {
        let read = read_bounded_line(reader, limit, line).ok()??;
        let next = if read.oversized {
            Some(ProcessOutput::Oversized)
        } else {
            let text = String::from_utf8_lossy(line);
            let text = text.trim_end_matches('\r');
            (!text.trim().is_empty()).then(|| ProcessOutput::Line(text.to_owned()))
        };
        if line.capacity() > RETAINED_LINE_CAPACITY {
            line.clear();
            line.shrink_to(RETAINED_LINE_CAPACITY);
        }
        if next.is_some() {
            return next;
        }
    }
}

fn read_output(
    stdout: ChildStdout,
    output: &SyncSender<ProcessOutput>,
    waker: &Waker,
    child: &Mutex<Child>,
    stopped: &AtomicBool,
    mut stderr: StderrCapture,
) {
    let mut reader = BufReader::with_capacity(64 * 1024, stdout);
    let mut line = Vec::new();
    while let Some(next) = next_output(&mut reader, MAX_LINE_BYTES, &mut line) {
        if output.send(next).is_err() {
            return;
        }
        waker();
    }
    let status = wait_for_exit(child, EXIT_WAIT);
    if status.is_none() {
        // Output closed but the process lives on; it cannot be driven.
        signal_agent_group(child, Signal::Kill);
    }
    // The agent's last words may still be on their way through stderr, so
    // the report waits for it to close. Its group has been signalled by now,
    // so it usually closes at once; the wait is bounded because a command
    // that left the group, or ignores the signal, can hold it open.
    let stderr_tail =
        String::from_utf8_lossy(&stderr.kept_by(Instant::now() + STDERR_CLOSE_WAIT).bytes)
            .trim()
            .to_owned();
    let report = match status {
        Some(status) => ExitReport {
            success: status.success(),
            status: if stopped.load(Ordering::Acquire) {
                "closed".to_owned()
            } else {
                status.to_string()
            },
            stderr_tail,
        },
        None => ExitReport {
            success: false,
            status: "stopped responding".to_owned(),
            stderr_tail,
        },
    };
    let _ = output.send(ProcessOutput::Exited(report));
    waker();
}

fn wait_for_exit(child: &Mutex<Child>, limit: Duration) -> Option<std::process::ExitStatus> {
    let started = Instant::now();
    loop {
        match reap(child) {
            Ok(Some(status)) => return Some(status),
            Ok(None) if started.elapsed() < limit => thread::sleep(Duration::from_millis(20)),
            Ok(None) | Err(()) => return None,
        }
    }
}

// The agent leads its own process group, which `crate::process_group` may
// signal only while the agent is unreaped. Every reap therefore happens
// here, under the child's lock, and so does every group signal.

/// Signals the agent and everything in its group, unless it was reaped.
fn signal_agent_group(child: &Mutex<Child>, signal: Signal) {
    let Ok(child) = child.lock() else {
        return;
    };
    crate::process_group::signal(&child, signal);
}

/// Reaps the agent once it has exited, first ending the commands it left
/// running in its group. `Ok(None)` while it runs.
fn reap(child: &Mutex<Child>) -> Result<Option<std::process::ExitStatus>, ()> {
    let mut child = child.lock().map_err(|_| ())?;
    crate::process_group::reap(&mut child, Signal::Terminate).map_err(|_| ())
}

/// Whether a program was found, once the search has finished.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lookup {
    Found(PathBuf),
    Missing,
    /// The login shell's `PATH` is still being asked for; `waker` runs
    /// when the answer is in.
    Searching,
}

/// Finds `program` the way a login shell would: on Termeric's own `PATH`
/// first, then on the `PATH` the person's login shell sets up, which a
/// desktop-launched Termeric may not have inherited. Never waits for the
/// login shell: the first lookup that needs it asks in the background.
pub fn find_program(program: &str, waker: &Waker) -> Lookup {
    if let Some(path) = crate::agents::find_executable(program) {
        return Lookup::Found(path);
    }
    if program.contains('/') {
        return Lookup::Missing;
    }
    match LOGIN_PATH.get(waker, login_shell_path) {
        None => Lookup::Searching,
        Some(paths) => paths
            .and_then(|paths| crate::agents::find_executable_in(program, &paths))
            .map_or(Lookup::Missing, Lookup::Found),
    }
}

static LOGIN_PATH: LoginPath = LoginPath::new();

/// The login shell's `PATH`, asked for once per run on a thread of its own.
struct LoginPath {
    state: Mutex<LoginPathState>,
}

enum LoginPathState {
    Unknown,
    /// Asking; these run when the answer is in.
    Asking(Vec<Waker>),
    Known(Option<OsString>),
}

impl LoginPath {
    const fn new() -> Self {
        Self {
            state: Mutex::new(LoginPathState::Unknown),
        }
    }

    /// The answer, or `None` while `ask` runs, after which `waker` runs.
    fn get(
        &'static self,
        waker: &Waker,
        ask: fn() -> Option<OsString>,
    ) -> Option<Option<OsString>> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        match &mut *state {
            LoginPathState::Known(path) => return Some(path.clone()),
            LoginPathState::Asking(wakers) => wakers.push(Arc::clone(waker)),
            LoginPathState::Unknown => {
                *state = LoginPathState::Asking(vec![Arc::clone(waker)]);
                let spawned = thread::Builder::new()
                    .name("agent-view-login-path".into())
                    .spawn(move || self.finish(ask()));
                if spawned.is_err() {
                    *state = LoginPathState::Known(None);
                    return Some(None);
                }
            }
        }
        None
    }

    fn finish(&self, path: Option<OsString>) {
        let previous = std::mem::replace(
            &mut *self.state.lock().unwrap_or_else(|error| error.into_inner()),
            LoginPathState::Known(path),
        );
        if let LoginPathState::Asking(wakers) = previous {
            for waker in wakers {
                waker();
            }
        }
    }
}

/// Asks the person's login shell for its `PATH`.
fn login_shell_path() -> Option<OsString> {
    let shell = std::env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into());
    let mut child = Command::new(shell)
        .args(["-l", "-c", "printf '%s' \"$PATH\""])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut text = Vec::new();
        let _ = stdout.read_to_end(&mut text);
        let _ = sender.send(text);
    });
    // A slow or interactive profile must not keep the answer away forever.
    let text = receiver.recv_timeout(Duration::from_secs(3)).ok();
    let _ = child.kill();
    let _ = child.wait();
    let text = String::from_utf8(text?).ok()?;
    // Profiles may print before the command runs; the path is the end.
    let path = text.rsplit('\n').next()?.trim().to_owned();
    (!path.is_empty()).then(|| path.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_lines_skip_blanks_survive_an_oversized_line_and_keep_an_unterminated_end() {
        let limit = 4 * RETAINED_LINE_CAPACITY;
        let large = "y".repeat(2 * RETAINED_LINE_CAPACITY);
        let input = format!(
            "first\r\n\n  \r\n{}\n{large}\nλ last",
            "x".repeat(limit + 1)
        );
        let mut reader = BufReader::with_capacity(4096, input.as_bytes());
        let mut line = Vec::new();
        let mut next = || {
            let output = next_output(&mut reader, limit, &mut line);
            // A huge line's memory is given back once it is handled.
            assert!(line.capacity() <= RETAINED_LINE_CAPACITY);
            output
        };
        assert_eq!(next(), Some(ProcessOutput::Line("first".into())));
        assert_eq!(next(), Some(ProcessOutput::Oversized));
        assert_eq!(next(), Some(ProcessOutput::Line(large.clone())));
        assert_eq!(next(), Some(ProcessOutput::Line("λ last".into())));
        assert_eq!(next(), None);
    }

    #[test]
    fn the_login_path_is_asked_for_once_without_waiting() {
        static PATH: LoginPath = LoginPath::new();
        let (sender, receiver) = mpsc::channel::<()>();
        let sender = Mutex::new(sender);
        let woken: Waker = Arc::new(move || {
            let _ = sender.lock().unwrap().send(());
        });
        fn ask() -> Option<OsString> {
            thread::sleep(Duration::from_millis(50));
            Some("/login/bin".into())
        }
        assert_eq!(PATH.get(&woken, ask), None);
        // A second lookup while asking waits for the same answer.
        assert_eq!(PATH.get(&woken, ask), None);
        receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(PATH.get(&woken, ask), Some(Some("/login/bin".into())));
    }

    fn shell(script: &str) -> LaunchSpec {
        LaunchSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            cwd: std::env::temp_dir(),
            env: vec![("AGENT_VIEW_TEST".into(), "set".into())],
            env_remove: vec!["TERMERIC_TERMINAL_ID".into()],
        }
    }

    fn collect(process: &mut AgentProcess, limit: Duration) -> Vec<ProcessOutput> {
        let started = Instant::now();
        let mut outputs = Vec::new();
        while started.elapsed() < limit {
            match process.try_next() {
                Some(output) => {
                    let done = matches!(output, ProcessOutput::Exited(_));
                    outputs.push(output);
                    if done {
                        break;
                    }
                }
                None => thread::sleep(Duration::from_millis(5)),
            }
        }
        outputs
    }

    fn noop_waker() -> Waker {
        Arc::new(|| {})
    }

    #[test]
    fn lines_round_trip_and_the_exit_is_reported_last() {
        let mut process = AgentProcess::spawn(
            &shell("read line; echo \"got $line $AGENT_VIEW_TEST\"; echo '' ; echo done; echo oops >&2; exit 3"),
            noop_waker(),
        )
        .unwrap();
        process.send("hello\n".into()).unwrap();
        let outputs = collect(&mut process, Duration::from_secs(10));
        assert_eq!(outputs[0], ProcessOutput::Line("got hello set".into()));
        assert_eq!(outputs[1], ProcessOutput::Line("done".into()));
        let ProcessOutput::Exited(report) = &outputs[2] else {
            panic!("{outputs:?}")
        };
        assert!(!report.success);
        assert!(report.status.contains('3'), "{}", report.status);
        assert_eq!(report.stderr_tail, "oops");
        assert!(process.has_exited());
        assert_eq!(process.try_next(), None);
    }

    fn exit_report(outputs: &[ProcessOutput]) -> &ExitReport {
        let Some(ProcessOutput::Exited(report)) = outputs.last() else {
            panic!("{outputs:?}")
        };
        report
    }

    #[test]
    fn a_crash_reports_what_its_group_writes_to_stderr_as_it_ends() {
        // The agent exits at once, and a command it started writes the
        // reason as it does, ignoring the signal that ends the group. The
        // command waits for the end of a pipe only the agent writes to, so
        // it speaks once the agent has gone, however slowly either runs.
        let mut process = AgentProcess::spawn(
            &shell(
                "pipe=$(mktemp -u) && mkfifo \"$pipe\" || exit 1
                (trap '' TERM; exec 3<\"$pipe\"; rm \"$pipe\"; cat <&3; echo 'last words' >&2) </dev/null >/dev/null &
                exec 3>\"$pipe\"
                echo 'first words' >&2; exit 4",
            ),
            noop_waker(),
        )
        .unwrap();
        let outputs = collect(&mut process, Duration::from_secs(10));
        let report = exit_report(&outputs);
        assert!(report.status.contains('4'), "{}", report.status);
        assert_eq!(report.stderr_tail, "first words\nlast words");
    }

    #[test]
    fn a_command_holding_stderr_open_delays_the_exit_report_only_briefly() {
        // The holder keeps the agent's stderr open until the test kills it.
        // It ignores SIGTERM from the moment it is forked, since the agent
        // ignores it first: set inside the holder instead, the agent could
        // exit and its group be signalled before the holder got to it.
        let started = Instant::now();
        let mut process = AgentProcess::spawn(
            &shell(
                "trap '' TERM; (exec sleep 600) </dev/null >/dev/null & echo $!
                echo 'last words' >&2; exit 4",
            ),
            noop_waker(),
        )
        .unwrap();
        let outputs = collect(&mut process, Duration::from_secs(30));
        let elapsed = started.elapsed();
        let Some(ProcessOutput::Line(holder)) = outputs.first() else {
            panic!("{outputs:?}")
        };
        let holder: libc::pid_t = holder.parse().unwrap();
        // The report came while stderr was still open, so the wait for it
        // to close gave up instead of lasting as long as the holder.
        assert!(
            crate::test_support::process_is_alive(holder),
            "the holder ended before the exit report"
        );
        // SAFETY: the holder still runs, as just checked: it ignored the
        // group's SIGTERM and sleeps until killed, so its id is its own.
        unsafe {
            libc::kill(holder, libc::SIGKILL);
        }
        let report = exit_report(&outputs);
        assert!(report.status.contains('4'), "{}", report.status);
        assert_eq!(report.stderr_tail, "last words");
        // The wait starts after the spawn, so the report cannot come sooner.
        assert!(elapsed >= STDERR_CLOSE_WAIT, "{elapsed:?}");
    }

    #[test]
    fn terminating_closes_the_agent_and_its_children() {
        let mut process =
            AgentProcess::spawn(&shell("sleep 30 & echo started; wait"), noop_waker()).unwrap();
        let started = Instant::now();
        while process.try_next() != Some(ProcessOutput::Line("started".into())) {
            assert!(started.elapsed() < Duration::from_secs(10));
            thread::sleep(Duration::from_millis(5));
        }
        process.terminate();
        let outputs = collect(&mut process, Duration::from_secs(10));
        let Some(ProcessOutput::Exited(report)) = outputs.last() else {
            panic!("{outputs:?}")
        };
        assert_eq!(report.status, "closed");
        assert_eq!(
            process.send("x\n".into()),
            Err((SendError::Closed, "x\n".into()))
        );
    }

    #[test]
    fn an_agent_that_cannot_be_served_is_killed_with_its_group_and_reaped() {
        // Both ignore SIGTERM, so only a kill ends them.
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "trap '' TERM; sleep 30 & echo $!; wait"]);
        let crate::process_group::PipedChild { child, stdout, .. } =
            crate::process_group::spawn_piped(&mut command).unwrap();
        let mut line = String::new();
        BufReader::new(stdout).read_line(&mut line).unwrap();
        let started: libc::pid_t = line.trim().parse().unwrap();
        let child = Mutex::new(child);
        abandon(&child);
        assert_eq!(
            crate::process_group::leader_state(&child.lock().unwrap()),
            crate::process_group::Leader::Reaped
        );
        assert!(
            crate::test_support::process_ends(started),
            "the command the agent started, {started}, still runs"
        );
    }

    #[test]
    fn commands_an_exited_agent_left_running_are_ended() {
        let mut process = AgentProcess::spawn(
            &shell("sleep 30 >/dev/null 2>&1 & echo $!; exit 0"),
            noop_waker(),
        )
        .unwrap();
        let outputs = collect(&mut process, Duration::from_secs(10));
        let Some(ProcessOutput::Line(orphan)) = outputs.first() else {
            panic!("{outputs:?}")
        };
        let orphan: libc::pid_t = orphan.parse().unwrap();
        assert!(matches!(outputs.last(), Some(ProcessOutput::Exited(_))));
        let started = Instant::now();
        // SAFETY: signal 0 only checks that the process exists.
        while unsafe { libc::kill(orphan, 0) } == 0 {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "the agent's leftover command {orphan} still runs"
            );
            thread::sleep(Duration::from_millis(20));
        }
        // Closing afterwards signals nothing: the group's id may be reused.
        process.terminate();
    }

    #[test]
    fn the_waker_runs_for_each_output() {
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&count);
        let mut process = AgentProcess::spawn(
            &shell("echo a; echo b"),
            Arc::new(move || {
                counter.fetch_add(1, Ordering::Relaxed);
            }),
        )
        .unwrap();
        let outputs = collect(&mut process, Duration::from_secs(10));
        assert_eq!(outputs.len(), 3);
        assert!(count.load(Ordering::Relaxed) >= 3);
    }

    #[test]
    fn programs_resolve_by_path_or_explicit_location() {
        // None of these needs the login shell, so none waits for it.
        let waker: Waker = Arc::new(|| {});
        assert_eq!(
            find_program("/bin/sh", &waker),
            Lookup::Found("/bin/sh".into())
        );
        assert!(matches!(find_program("sh", &waker), Lookup::Found(_)));
        assert_eq!(
            find_program("/definitely/not/here", &waker),
            Lookup::Missing
        );
    }
}
