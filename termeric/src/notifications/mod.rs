//! The terminal bell and desktop notifications.
//!
//! The bell flashes the window and can play a sound. Notifications go to the
//! desktop's notification service over D-Bus from one worker thread, which
//! also tracks the Kitty OSC 99 notifications a terminal may close, query,
//! or ask to hear about.

mod icon;
mod tracked;

use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::config::BellConfigFile;
use termeric_core::NotificationRequest;
#[cfg(test)]
use termeric_core::NotificationSound;

const MAX_DELIVERY_ERROR_CHARS: usize = 512;
/// Bells closer together than this are ignored.
const BELL_RATE_LIMIT: Duration = Duration::from_millis(50);
/// A bell sound still playing after this long is stopped and reported, so a
/// hung player cannot silence the bell for good.
const AUDIO_PLAYER_TIMEOUT: Duration = Duration::from_secs(5);
/// How often the waiter checks whether the player has exited.
const AUDIO_PLAYER_POLL: Duration = Duration::from_millis(20);

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum NotificationCompletion {
    Warning(String),
    ProtocolReply {
        origin: crate::tabs::TerminalIdentity,
        data: Vec<u8>,
    },
    Activation {
        origin: crate::tabs::TerminalIdentity,
        focus: bool,
        protocol_reply: Option<Vec<u8>>,
    },
}

#[cfg(test)]
pub(crate) type CapturedNotificationCommand = (&'static str, crate::tabs::TerminalIdentity, String);

/// Turns delivery failures into bounded messages for the person. Every
/// failure is logged; the person is shown each one once, until a delivery
/// succeeds again.
struct FailureReport {
    prefix: &'static str,
    last: Option<String>,
}

impl FailureReport {
    const fn new(prefix: &'static str) -> Self {
        Self { prefix, last: None }
    }

    /// Logs `detail`, and returns the message to show for it, or `None` when
    /// it repeats the last one shown.
    fn report(&mut self, detail: &str) -> Option<String> {
        let message = crate::safe_text::truncate_with_ellipsis(
            &format!("{}: {detail}", self.prefix),
            MAX_DELIVERY_ERROR_CHARS,
        );
        log::warn!("{message}");
        if self.last.as_deref() == Some(message.as_str()) {
            return None;
        }
        self.last = Some(message.clone());
        Some(message)
    }

    fn recovered(&mut self) {
        self.last = None;
    }
}

/// Bell state tracker
pub struct Bell {
    config: BellConfigFile,
    rate_limit: Duration,
    /// Time of last bell
    last_bell: Option<Instant>,
    /// When the visual bell started, while it is showing
    visual_start: Option<Instant>,
    audio_command: (String, Vec<String>),
    audio_timeout: Duration,
    /// The waiter for the sound playing now, which returns how it ended. One
    /// at a time: a bell during it stays silent.
    audio_player: Option<JoinHandle<Result<(), String>>>,
    audio_failure: FailureReport,
}

impl Bell {
    /// Create a new bell handler
    pub fn new(config: BellConfigFile) -> Self {
        Self {
            config,
            rate_limit: BELL_RATE_LIMIT,
            last_bell: None,
            visual_start: None,
            audio_command: (
                "paplay".into(),
                vec!["/usr/share/sounds/freedesktop/stereo/bell.oga".into()],
            ),
            audio_timeout: AUDIO_PLAYER_TIMEOUT,
            audio_player: None,
            audio_failure: FailureReport::new("Audio bell unavailable"),
        }
    }

    /// Trigger the bell. Returns a failure to show the person, if any.
    pub fn ring(&mut self) -> Option<String> {
        if self
            .last_bell
            .is_some_and(|last| last.elapsed() < self.rate_limit)
        {
            return None;
        }
        self.last_bell = Some(Instant::now());

        if self.config.visual {
            self.visual_start = Some(Instant::now());
        }

        if self.config.audio
            && let Some(report) = self.play_audio()
        {
            return Some(report);
        }

        log::debug!("Bell triggered");
        None
    }

    /// Check if visual bell should be displayed
    pub fn is_visual_active(&self) -> bool {
        self.visual_start.is_some()
    }

    /// Update bell state (call each frame)
    pub fn update(&mut self) {
        if let Some(start) = self.visual_start
            && start.elapsed().as_millis() >= u128::from(self.config.visual_duration_ms)
        {
            self.visual_start = None;
        }
    }

    /// Get visual bell intensity (0.0 to 1.0 based on time remaining)
    pub fn visual_intensity(&self) -> f32 {
        if let Some(start) = self.visual_start {
            let elapsed = start.elapsed().as_millis() as f32;
            let duration = self.config.visual_duration_ms as f32;
            (1.0 - (elapsed / duration)).max(0.0)
        } else {
            0.0
        }
    }

    /// Starts the bell sound unless one is still playing. Returns a failure
    /// to show: how the last sound ended, or why this one could not start. A
    /// sound that played through is what clears an earlier failure.
    fn play_audio(&mut self) -> Option<String> {
        if self
            .audio_player
            .as_ref()
            .is_some_and(|player| !player.is_finished())
        {
            return None;
        }
        let mut failure = None;
        if let Some(player) = self.audio_player.take() {
            match player.join() {
                Ok(Ok(())) => self.audio_failure.recovered(),
                Ok(Err(error)) => failure = Some(error),
                Err(_) => failure = Some("the audio bell waiter panicked".to_owned()),
            }
        }
        let (program, args) = &self.audio_command;
        match start_audio_player(program, args, self.audio_timeout) {
            Ok(player) => self.audio_player = Some(player),
            Err(error) => failure = Some(error),
        }
        failure.and_then(|error| self.audio_failure.report(&error))
    }

    /// Update configuration
    pub fn set_config(&mut self, config: BellConfigFile) {
        // Turning audio back on may report a player failure again.
        if config.audio && !self.config.audio {
            self.audio_failure.recovered();
        }
        self.config = config;
    }

    #[cfg(test)]
    pub(crate) fn set_audio_command_for_test(&mut self, program: &str, args: &[&str]) {
        self.audio_command = (
            program.into(),
            args.iter().map(|arg| (*arg).into()).collect(),
        );
    }
}

impl Default for Bell {
    fn default() -> Self {
        Self::new(BellConfigFile::default())
    }
}

/// Plays one bell sound. A waiter thread owns the player: it reaps it, even
/// after the `Bell` is gone, and stops it if it is still running after
/// `timeout`. The player's stdio is null, so its messages never reach the
/// terminal Termeric was started from.
fn start_audio_player(
    program: &str,
    args: &[String],
    timeout: Duration,
) -> Result<JoinHandle<Result<(), String>>, String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let name = program.to_owned();
    let (started, result) = mpsc::sync_channel(1);
    // Create the waiter before spawning its player, so a thread creation
    // failure cannot leave a process nobody reaps.
    let waiter = std::thread::Builder::new()
        .name("termeric-bell".into())
        .spawn(move || match command.spawn() {
            Ok(player) => {
                let _ = started.send(Ok(()));
                wait_for_audio_player(player, &name, timeout)
            }
            Err(error) => {
                let _ = started.send(Err(error));
                Ok(())
            }
        })
        .map_err(|error| format!("could not start the audio bell waiter: {error}"))?;
    match result.recv() {
        Ok(Ok(())) => Ok(waiter),
        Ok(Err(error)) => Err(format!("could not start {program}: {error}")),
        Err(_) => Err(format!(
            "the audio bell waiter stopped before starting {program}"
        )),
    }
}

/// Waits for `player` to exit, stopping it once `timeout` has passed, and
/// returns a failure unless it played through.
fn wait_for_audio_player(
    mut player: Child,
    program: &str,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        match player.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(format!("{program} exited unsuccessfully ({status})")),
            Ok(None) if Instant::now() >= deadline => break,
            Ok(None) => std::thread::sleep(AUDIO_PLAYER_POLL),
            Err(error) => {
                log::warn!("Could not check on the audio bell player {program}: {error}");
                break;
            }
        }
    }
    player
        .kill()
        .and_then(|()| player.wait())
        .map_err(|error| format!("could not stop {program}: {error}"))?;
    Err(format!(
        "{program} was still playing after {timeout:?} and was stopped"
    ))
}

/// Notification manager for system notifications
pub struct Notifications {
    /// Last notification time (for rate limiting)
    last_notification: Option<Instant>,
    /// Minimum time between notifications
    rate_limit: Duration,
    delivery_failure: FailureReport,
    /// The desktop notification worker, started on first use.
    tracked: Option<tracked::TrackedNotifications>,
}

impl Default for Notifications {
    fn default() -> Self {
        Self::new()
    }
}

impl Notifications {
    /// Create a new notification manager
    pub fn new() -> Self {
        Self {
            last_notification: None,
            rate_limit: Duration::from_secs(1),
            delivery_failure: FailureReport::new("Desktop notification unavailable"),
            tracked: None,
        }
    }

    /// Send a desktop notification
    pub fn notify(&mut self, title: &str, body: &str) -> Option<String> {
        self.notify_untracked(NotificationRequest {
            title: Some(title.to_owned()),
            body: body.to_owned(),
            ..NotificationRequest::default()
        })
    }

    /// Show a notification nothing will replace, close, or report on. These
    /// are rate limited because any program can trigger one.
    fn notify_untracked(&mut self, request: NotificationRequest) -> Option<String> {
        if self
            .last_notification
            .is_some_and(|last| last.elapsed() < self.rate_limit)
        {
            return None;
        }
        self.last_notification = Some(Instant::now());
        let delivered = match self.tracked() {
            Ok(tracked) => tracked.show_untracked(request),
            Err(report) => return report,
        };
        self.delivered(delivered)
    }

    /// Route a terminal-owned notification, using tracked native handles when
    /// its identity, expiry, reporting, button, sound, or transmitted-icon
    /// policy requires bounded worker state.
    pub fn notify_terminal(
        &mut self,
        origin: crate::tabs::TerminalIdentity,
        request: NotificationRequest,
    ) -> Option<String> {
        if !requires_tracked_notification(&request) {
            return self.notify_untracked(request);
        }
        let delivered = match self.tracked() {
            Ok(tracked) => tracked.show(origin, request),
            Err(report) => return report,
        };
        self.delivered(delivered)
    }

    /// Query the exact origin's currently live named OSC 99 notifications.
    pub fn query_alive(
        &mut self,
        origin: crate::tabs::TerminalIdentity,
        identifier: String,
        reply_uses_8bit_controls: bool,
    ) -> Option<String> {
        let delivered = match self.tracked() {
            Ok(tracked) => tracked.alive(origin, identifier, reply_uses_8bit_controls),
            Err(report) => return report,
        };
        self.delivered(delivered)
    }

    /// Close one exact origin-owned tracked notification.
    pub fn close_terminal(
        &mut self,
        origin: crate::tabs::TerminalIdentity,
        identifier: String,
    ) -> Option<String> {
        let closed = self.tracked.as_ref()?.close(origin, identifier);
        self.delivered(closed)
    }

    /// The desktop notification worker, started on first use. On failure,
    /// returns the error to report, unless it repeats the last one.
    fn tracked(&mut self) -> Result<&mut tracked::TrackedNotifications, Option<String>> {
        if self.tracked.is_none() {
            match tracked::TrackedNotifications::new() {
                Ok(tracked) => self.tracked = Some(tracked),
                Err(error) => return Err(self.report_failure(&error)),
            }
        }
        Ok(self.tracked.as_mut().expect("worker was started"))
    }

    fn report_failure(&mut self, error: &str) -> Option<String> {
        self.delivery_failure.report(error)
    }

    /// The failure to show for handing a request to the worker. A request
    /// that gets through clears the last failure, so a later one, such as
    /// another busy queue, is shown again.
    fn delivered(&mut self, handed_off: Result<(), String>) -> Option<String> {
        match handed_off {
            Ok(()) => {
                self.delivery_failure.recovered();
                None
            }
            Err(error) => self.report_failure(&error),
        }
    }

    /// Drain asynchronous lifecycle failures for visible app reporting.
    pub fn drain_completions(&mut self) -> Vec<NotificationCompletion> {
        self.tracked
            .as_ref()
            .map_or_else(Vec::new, tracked::TrackedNotifications::drain_completions)
            .into_iter()
            .filter_map(|completion| match completion {
                tracked::TrackedCompletion::Recovered => {
                    self.delivery_failure.recovered();
                    None
                }
                tracked::TrackedCompletion::Failure(error) => self
                    .delivery_failure
                    .report(&error)
                    .map(NotificationCompletion::Warning),
                tracked::TrackedCompletion::Completed(completion) => Some(completion),
            })
            .collect()
    }

    /// Deliver through a worker whose notification service refuses every
    /// notification, without a rate limit.
    #[cfg(test)]
    pub(crate) fn fail_delivery_for_test(&mut self, error: &str) {
        self.tracked = Some(tracked::TrackedNotifications::failing_for_test(error).unwrap());
        self.rate_limit = Duration::ZERO;
    }

    /// Wait until the worker has handled everything sent to it so far, and
    /// return what [`Self::drain_completions`] gave meanwhile. The worker
    /// takes commands in order, so once it answers a query sent last, it has
    /// handled the rest, including ones that produce nothing to report.
    #[cfg(test)]
    pub(crate) fn settle_for_test(&mut self) -> Vec<NotificationCompletion> {
        let probe = crate::tabs::TerminalIdentity {
            tab_id: usize::MAX,
            pane_id: crate::panes::PaneId(u32::MAX),
        };
        self.tracked
            .as_ref()
            .expect("a delivery worker is running")
            .alive(probe, "termeric-settle-probe".into(), false)
            .expect("the worker takes the probe");
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut settled = Vec::new();
        loop {
            for completion in self.drain_completions() {
                match completion {
                    NotificationCompletion::ProtocolReply { origin, .. } if origin == probe => {
                        return settled;
                    }
                    other => settled.push(other),
                }
            }
            assert!(
                Instant::now() < deadline,
                "the notification worker did not settle"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[cfg(test)]
    pub(crate) fn capture_tracked_commands_for_test(
        &mut self,
    ) -> impl Fn() -> Vec<CapturedNotificationCommand> + use<> {
        let (tracked, capture) = tracked::TrackedNotifications::capture_commands_for_test();
        self.tracked = Some(tracked);
        capture
    }

    #[cfg(test)]
    pub(crate) fn has_delivery_state_for_test(&self) -> bool {
        self.last_notification.is_some() || self.tracked.is_some()
    }
}

fn requires_tracked_notification(request: &NotificationRequest) -> bool {
    request.identifier.is_some()
        || !matches!(request.expiry, termeric_core::NotificationExpiry::System)
        || request.report_close
        || request.report_activation
        || request.focus_on_activation
        || !request.buttons.is_empty()
        || !matches!(request.sound, termeric_core::NotificationSound::System)
        || request.icon_cache_id.is_some()
        || request.icon_data.is_some()
}

fn freedesktop_icon_name(name: &str) -> &str {
    match name {
        "error" => "dialog-error",
        "warn" | "warning" => "dialog-warning",
        "info" => "dialog-information",
        "question" => "dialog-question",
        "help" => "help-browser",
        "file-manager" => "system-file-manager",
        "system-monitor" => "utilities-system-monitor",
        "text-editor" => "accessories-text-editor",
        _ => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audio_bell() -> Bell {
        let mut bell = Bell::new(BellConfigFile {
            visual: false,
            audio: true,
            ..BellConfigFile::default()
        });
        bell.rate_limit = Duration::ZERO;
        bell
    }

    #[test]
    fn bells_inside_the_rate_limit_are_ignored() {
        let mut bell = Bell {
            rate_limit: Duration::from_secs(60),
            ..Bell::default()
        };
        bell.ring();
        assert!(bell.is_visual_active());
        let first = bell.last_bell;
        bell.ring();
        assert_eq!(bell.last_bell, first);

        bell.rate_limit = Duration::ZERO;
        bell.ring();
        assert_ne!(bell.last_bell, first);
    }

    #[test]
    fn test_visual_intensity() {
        let bell = Bell::default();
        assert_eq!(bell.visual_intensity(), 0.0);
    }

    #[test]
    fn audio_player_failures_are_bounded_deduplicated_and_recoverable() {
        let mut bell = audio_bell();
        bell.set_audio_command_for_test("/definitely/missing/termeric-audio-helper", &[]);
        let first = bell.ring().unwrap();
        assert!(first.contains("Audio bell unavailable"));
        assert!(first.contains("termeric-audio-helper"));
        assert!(bell.ring().is_none());

        // Starting a player is not enough: one that plays through clears
        // the failure, so the next one is shown again.
        bell.set_audio_command_for_test("/bin/true", &[]);
        assert!(bell.ring().is_none());
        wait_for_player(&bell);
        bell.set_audio_command_for_test("/definitely/missing/termeric-audio-helper", &[]);
        assert!(bell.ring().is_some());
    }

    /// Waits for the playing sound's waiter to finish, without joining it, so
    /// the next bell is the one that collects how it ended.
    fn wait_for_player(bell: &Bell) {
        let player = bell.audio_player.as_ref().expect("a sound is playing");
        assert!(
            crate::test_support::poll_until(Duration::from_secs(10), || player.is_finished()),
            "the audio bell waiter did not finish"
        );
    }

    /// Lines the fake player appended to `log`, once there are `count`.
    fn wait_for_lines(log: &std::path::Path, count: usize) -> Vec<String> {
        let read = || {
            std::fs::read_to_string(log)
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        assert!(
            crate::test_support::poll_until(Duration::from_secs(10), || read().len() >= count),
            "the fake player did not start"
        );
        read()
    }

    fn assert_reaped(pid: &str) {
        let pid: libc::pid_t = pid.parse().unwrap();
        let mut status = 0;
        // SAFETY: The PID names a fake player this process started, status
        // is writable, and WNOHANG never blocks.
        let result = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
        let error = std::io::Error::last_os_error();
        assert_eq!(result, -1, "the player was not reaped by its waiter");
        assert_eq!(error.raw_os_error(), Some(libc::ECHILD));
    }

    /// A fake player: it appends its PID and where its stdin, stdout, and
    /// stderr lead to the log in `$1`, then plays until `$2` exists. Each link
    /// is read before it is written: dash applies `>&3` to the shell itself
    /// while the command runs, so its own stdout would read as the log.
    const FAKE_PLAYER: &str = r#"exec 3>>"$1"
echo "$$" >&3
for fd in 0 1 2; do target=$(readlink "/proc/$$/fd/$fd"); echo "$target" >&3; done
exec 3>&-
while [ ! -e "$2" ]; do sleep 0.01; done"#;

    #[test]
    fn one_sound_plays_at_a_time_without_stdio_and_is_reaped() {
        let directory = tempfile::tempdir().unwrap();
        let log = directory.path().join("player.log");
        let release = directory.path().join("release");
        let mut bell = audio_bell();
        bell.set_audio_command_for_test(
            "/bin/sh",
            &[
                "-c",
                FAKE_PLAYER,
                "player",
                log.to_str().unwrap(),
                release.to_str().unwrap(),
            ],
        );
        assert!(bell.ring().is_none());
        let lines = wait_for_lines(&log, 4);
        assert_eq!(lines[1..], ["/dev/null", "/dev/null", "/dev/null"]);
        let waiter = bell.audio_player.as_ref().unwrap().thread().id();

        // A bell during the sound stays silent instead of starting another.
        assert!(bell.ring().is_none());
        assert_eq!(bell.audio_player.as_ref().unwrap().thread().id(), waiter);
        bell.update();
        assert!(bell.audio_player.is_some());

        std::fs::write(&release, b"").unwrap();
        wait_for_player(&bell);
        assert_reaped(&lines[0]);
        assert!(bell.ring().is_none());
        assert_ne!(bell.audio_player.as_ref().unwrap().thread().id(), waiter);
        let lines = wait_for_lines(&log, 8);
        assert_ne!(lines[4], lines[0]);
        wait_for_player(&bell);
        assert_reaped(&lines[4]);
    }

    #[test]
    fn a_hung_player_is_stopped_reaped_and_reported_once() {
        let directory = tempfile::tempdir().unwrap();
        let pid = directory.path().join("pid");
        let mut bell = audio_bell();
        bell.audio_timeout = Duration::from_secs(1);
        bell.set_audio_command_for_test(
            "/bin/sh",
            &[
                "-c",
                r#"echo "$$" > "$1"; exec sleep 30"#,
                "player",
                pid.to_str().unwrap(),
            ],
        );
        assert!(bell.ring().is_none());
        wait_for_player(&bell);
        let first = std::fs::read_to_string(&pid).unwrap();
        assert_reaped(first.trim());

        let report = bell.ring().unwrap();
        assert!(report.contains("Audio bell unavailable"), "{report}");
        assert!(report.contains("was still playing after 1s"), "{report}");
        // That bell started another player, which hangs the same way.
        wait_for_player(&bell);
        let second = std::fs::read_to_string(&pid).unwrap();
        assert_ne!(second, first);
        assert_reaped(second.trim());
        assert!(bell.ring().is_none(), "the same failure is shown once");
        wait_for_player(&bell);
    }

    /// Delivery runs on the notification worker, so failures arrive later.
    fn next_warning(notifications: &mut Notifications) -> Option<String> {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            for completion in notifications.drain_completions() {
                if let NotificationCompletion::Warning(warning) = completion {
                    return Some(warning);
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    }

    #[test]
    fn plain_notification_failures_are_reported_once() {
        let mut notifications = Notifications::new();
        notifications.fail_delivery_for_test("synthetic notification service failure");
        assert!(notifications.notify("title", "body").is_none());
        let warning = next_warning(&mut notifications).unwrap();
        assert!(warning.contains("Desktop notification unavailable"));
        assert!(warning.contains("synthetic notification service failure"));

        assert!(notifications.notify("title", "body").is_none());
        assert!(next_warning(&mut notifications).is_none());
    }

    #[test]
    fn a_busy_queue_is_reported_again_after_a_request_gets_through() {
        let mut notifications = Notifications::new();
        let drain = notifications.capture_tracked_commands_for_test();
        notifications.rate_limit = Duration::ZERO;
        let fill = |notifications: &mut Notifications| {
            (0..1_000).find_map(|_| notifications.notify("title", "body"))
        };
        let busy = fill(&mut notifications).expect("the queue fills");
        assert!(busy.contains("queue is busy"), "{busy}");
        assert!(fill(&mut notifications).is_none(), "shown once while busy");

        drain();
        assert!(notifications.notify("title", "body").is_none());
        assert_eq!(fill(&mut notifications), Some(busy));
    }

    #[test]
    fn delivery_errors_have_a_hard_character_ceiling() {
        let mut report = FailureReport::new("Desktop notification unavailable");
        let error = report.report(&"x".repeat(1_024)).unwrap();
        assert_eq!(error.chars().count(), MAX_DELIVERY_ERROR_CHARS);
        assert!(error.ends_with('…'));
    }

    #[test]
    fn kitty_icon_names_map_to_freedesktop_icons() {
        assert_eq!(freedesktop_icon_name("error"), "dialog-error");
        assert_eq!(freedesktop_icon_name("warn"), "dialog-warning");
        assert_eq!(freedesktop_icon_name("warning"), "dialog-warning");
        assert_eq!(freedesktop_icon_name("info"), "dialog-information");
        assert_eq!(freedesktop_icon_name("question"), "dialog-question");
        assert_eq!(freedesktop_icon_name("help"), "help-browser");
        assert_eq!(freedesktop_icon_name("file-manager"), "system-file-manager");
        assert_eq!(
            freedesktop_icon_name("system-monitor"),
            "utilities-system-monitor"
        );
        assert_eq!(
            freedesktop_icon_name("text-editor"),
            "accessories-text-editor"
        );
        assert_eq!(freedesktop_icon_name("org.example.App"), "org.example.App");
    }

    #[test]
    fn anonymous_nondefault_sound_uses_the_bounded_tracked_route() {
        let ordinary = NotificationRequest {
            title: Some("Title".into()),
            ..NotificationRequest::default()
        };
        assert!(!requires_tracked_notification(&ordinary));
        for sound in [
            NotificationSound::Silent,
            NotificationSound::Error,
            NotificationSound::Warning,
            NotificationSound::Information,
            NotificationSound::Question,
        ] {
            assert!(requires_tracked_notification(&NotificationRequest {
                title: Some("Title".into()),
                sound,
                ..NotificationRequest::default()
            }));
        }
    }

    #[test]
    fn focus_actions_use_the_bounded_tracked_route() {
        let focus = NotificationRequest {
            title: Some("Title".into()),
            focus_on_activation: true,
            ..NotificationRequest::default()
        };
        assert!(requires_tracked_notification(&focus));
    }

    #[test]
    fn anonymous_transmitted_icons_use_the_bounded_tracked_route() {
        assert!(requires_tracked_notification(&NotificationRequest {
            title: Some("Title".into()),
            icon_data: Some(vec![1, 2, 3].into_boxed_slice()),
            ..NotificationRequest::default()
        }));
        assert!(requires_tracked_notification(&NotificationRequest {
            title: Some("Title".into()),
            icon_cache_id: Some("cached".into()),
            ..NotificationRequest::default()
        }));
    }
}
