//! The terminal bell and desktop notifications.
//!
//! The bell flashes the window and can play a sound. Notifications go to the
//! desktop's notification service over D-Bus from one worker thread, which
//! also tracks the Kitty OSC 99 notifications a terminal may close, query,
//! or ask to hear about.

mod icon;
mod tracked;

use std::process::{Child, Command};
use std::time::{Duration, Instant};

use crate::config::BellConfigFile;
use termeric_core::NotificationRequest;
#[cfg(test)]
use termeric_core::NotificationSound;

const MAX_DELIVERY_ERROR_CHARS: usize = 512;
/// Bells closer together than this are ignored.
const BELL_RATE_LIMIT: Duration = Duration::from_millis(50);

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

/// Turns delivery failures into bounded, logged messages for the person,
/// reporting each one once until a delivery succeeds again.
struct FailureReport {
    prefix: &'static str,
    last: Option<String>,
}

impl FailureReport {
    const fn new(prefix: &'static str) -> Self {
        Self { prefix, last: None }
    }

    /// The message to show for `detail`, or `None` when it repeats the last.
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
    /// The sound playing now. One at a time: a bell during it stays silent.
    audio_player: Option<Child>,
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

        if self.config.audio {
            if let Err(error) = self.play_audio() {
                return self.audio_failure.report(&error);
            }
            self.audio_failure.recovered();
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
        self.reap_audio_player();
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

    /// Start the bell sound unless one is still playing. The spawn does not
    /// wait for the sound; `update` and the next bell reap the player.
    fn play_audio(&mut self) -> Result<(), String> {
        if self.reap_audio_player() {
            return Ok(());
        }
        let (program, args) = &self.audio_command;
        let player = Command::new(program)
            .args(args)
            .spawn()
            .map_err(|error| format!("could not start {program}: {error}"))?;
        self.audio_player = Some(player);
        Ok(())
    }

    /// Reaps a finished sound player. Returns whether one is still playing.
    fn reap_audio_player(&mut self) -> bool {
        let Some(player) = &mut self.audio_player else {
            return false;
        };
        match player.try_wait() {
            Ok(None) => return true,
            Ok(Some(status)) if !status.success() => {
                log::warn!("Audio bell player exited unsuccessfully: {status}");
            }
            Ok(Some(_)) => {}
            Err(error) => log::warn!("Could not reap the audio bell player: {error}"),
        }
        self.audio_player = None;
        false
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

/// Notification manager for system notifications
pub struct Notifications {
    /// Whether notifications are enabled
    enabled: bool,
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
            enabled: true,
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
        if !self.enabled
            || self
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
        delivered
            .err()
            .and_then(|error| self.report_failure(&error))
    }

    /// Route a terminal-owned notification, using tracked native handles when
    /// its identity, expiry, reporting, button, sound, or transmitted-icon
    /// policy requires bounded worker state.
    pub fn notify_terminal(
        &mut self,
        origin: crate::tabs::TerminalIdentity,
        request: NotificationRequest,
    ) -> Option<String> {
        if !self.enabled {
            return None;
        }
        if !requires_tracked_notification(&request) {
            return self.notify_untracked(request);
        }
        let delivered = match self.tracked() {
            Ok(tracked) => tracked.show(origin, request),
            Err(report) => return report,
        };
        delivered
            .err()
            .and_then(|error| self.report_failure(&error))
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
        delivered
            .err()
            .and_then(|error| self.report_failure(&error))
    }

    /// Close one exact origin-owned tracked notification.
    pub fn close_terminal(
        &mut self,
        origin: crate::tabs::TerminalIdentity,
        identifier: String,
    ) -> Option<String> {
        let closed = self.tracked.as_ref()?.close(origin, identifier);
        closed.err().and_then(|error| self.report_failure(&error))
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

        bell.set_audio_command_for_test("/bin/true", &[]);
        assert!(bell.ring().is_none());
        bell.audio_player.as_mut().unwrap().wait().unwrap();
        bell.set_audio_command_for_test("/definitely/missing/termeric-audio-helper", &[]);
        assert!(bell.ring().is_some());
    }

    #[test]
    fn a_playing_sound_is_not_overlapped_and_a_finished_one_is_reaped() {
        let mut bell = audio_bell();
        bell.set_audio_command_for_test("/bin/sh", &["-c", "sleep 30"]);
        assert!(bell.ring().is_none());
        let playing = bell.audio_player.as_ref().unwrap().id();
        assert!(bell.ring().is_none());
        assert_eq!(bell.audio_player.as_ref().unwrap().id(), playing);

        let player = bell.audio_player.as_mut().unwrap();
        player.kill().unwrap();
        // Wait without reaping, so update() is the one that reaps it.
        let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::uninit();
        // SAFETY: `info` is writable for waitid, and WNOWAIT leaves the
        // exited child for try_wait to reap.
        let waited = unsafe {
            libc::waitid(
                libc::P_PID,
                playing as libc::id_t,
                info.as_mut_ptr(),
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        assert_eq!(waited, 0);
        bell.update();
        assert!(bell.audio_player.is_none());

        assert!(bell.ring().is_none());
        let next = bell.audio_player.as_mut().unwrap();
        assert_ne!(next.id(), playing);
        next.kill().unwrap();
        next.wait().unwrap();
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
