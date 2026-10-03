//! Bell and notification handling
//!
//! Handles terminal bell (BEL character) with visual and audio feedback.

mod icon;
mod tracked;

use std::process::{Child, Command};
use std::sync::OnceLock;
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::time::{Duration, Instant};

use termeric_core::NotificationRequest;
#[cfg(test)]
use termeric_core::NotificationSound;

const DESKTOP_COMMAND_QUEUE_CAPACITY: usize = 16;
const MAX_DESKTOP_HELPER_CHILDREN: usize = 8;
const MAX_DELIVERY_ERROR_CHARS: usize = 512;

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
        /// Opaque daemon authority retained only for this notification
        /// generation. Winit cannot currently apply a supplied token to an
        /// existing window, so the app deliberately consumes it without
        /// logging or forwarding it.
        activation_token: Option<String>,
    },
}

#[cfg(test)]
pub(crate) type CapturedNotificationCommand = (&'static str, crate::tabs::TerminalIdentity, String);

struct DesktopCommandRequest {
    program: String,
    args: Vec<String>,
    label: &'static str,
    started: SyncSender<Result<(), String>>,
}

impl DesktopCommandRequest {
    fn new(
        program: String,
        args: Vec<String>,
        label: &'static str,
    ) -> (Self, Receiver<Result<(), String>>) {
        // One acknowledgement cannot block cleanup if its caller is descheduled.
        let (started, result) = mpsc::sync_channel(1);
        (
            Self {
                program,
                args,
                label,
                started,
            },
            result,
        )
    }
}

static DESKTOP_COMMAND_RUNNER: OnceLock<Result<SyncSender<DesktopCommandRequest>, String>> =
    OnceLock::new();

fn desktop_command_sender() -> Result<&'static SyncSender<DesktopCommandRequest>, String> {
    DESKTOP_COMMAND_RUNNER
        .get_or_init(|| {
            let (sender, receiver) = mpsc::sync_channel(DESKTOP_COMMAND_QUEUE_CAPACITY);
            std::thread::Builder::new()
                .name("termeric-desktop-command".into())
                .spawn(move || run_desktop_commands(receiver, Vec::new(), |_| {}))
                .map(|_| sender)
                .map_err(|error| format!("could not start desktop helper worker: {error}"))
        })
        .as_ref()
        .map_err(Clone::clone)
}

fn run_desktop_commands(
    receiver: Receiver<DesktopCommandRequest>,
    mut children: Vec<(&'static str, Child)>,
    mut before_receive: impl FnMut(bool),
) {
    loop {
        reap_desktop_commands(&mut children);
        let idle = children.is_empty();
        // The observer is a no-op in production. Tests observe this exact
        // receive boundary, not an estimate of whole-application CPU usage.
        before_receive(idle);
        let received = if idle {
            receiver
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
        } else {
            receiver.recv_timeout(Duration::from_millis(50))
        };
        match received {
            Ok(request) => {
                // A child may have exited while receive was blocked. Reap
                // before admission so a completed child cannot occupy a slot.
                // One request per iteration also prevents queue traffic from
                // postponing cleanup indefinitely.
                reap_desktop_commands(&mut children);
                start_desktop_command(request, &mut children);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // Accepted requests have drained. Do not abandon remaining
                // children or kill desktop applications during teardown.
                for (label, mut child) in children {
                    report_desktop_exit(label, child.wait());
                }
                return;
            }
        }
    }
}

fn report_desktop_exit(label: &str, result: std::io::Result<std::process::ExitStatus>) {
    match result {
        Ok(status) if !status.success() => {
            log::warn!("{label} helper exited unsuccessfully: {status}");
        }
        Err(error) => log::warn!("Could not reap {label} helper: {error}"),
        Ok(_) => {}
    }
}

fn reap_desktop_commands(children: &mut Vec<(&'static str, Child)>) {
    let mut index = 0;
    while index < children.len() {
        let result = match children[index].1.try_wait() {
            Ok(None) => {
                index += 1;
                continue;
            }
            Ok(Some(status)) => Ok(status),
            Err(error) => Err(error),
        };
        report_desktop_exit(children[index].0, result);
        children.swap_remove(index);
    }
}

fn start_desktop_command(
    request: DesktopCommandRequest,
    children: &mut Vec<(&'static str, Child)>,
) {
    if children.len() >= MAX_DESKTOP_HELPER_CHILDREN {
        let _ = request.started.send(Err(format!(
            "at most {MAX_DESKTOP_HELPER_CHILDREN} desktop helpers may run at once"
        )));
        return;
    }
    match Command::new(&request.program).args(&request.args).spawn() {
        Ok(child) => {
            let _ = request.started.send(Ok(()));
            children.push((request.label, child));
        }
        Err(error) => {
            let _ = request.started.send(Err(error.to_string()));
        }
    }
}

fn spawn_desktop_command(
    program: &str,
    args: &[String],
    label: &'static str,
) -> Result<(), String> {
    let runner = desktop_command_sender()?;
    let (request, result) = DesktopCommandRequest::new(program.to_owned(), args.to_vec(), label);
    match runner.try_send(request) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => {
            return Err("desktop helper queue is busy".into());
        }
        Err(TrySendError::Disconnected(_)) => {
            return Err("desktop helper worker stopped".into());
        }
    }
    result
        .recv()
        .map_err(|_| "desktop helper worker stopped before launch".to_string())?
        .map_err(|error| format!("could not start {program}: {error}"))
}

fn bounded_delivery_error(prefix: &str, detail: &str) -> String {
    crate::safe_text::truncate_with_ellipsis(
        &format!("{prefix}: {detail}"),
        MAX_DELIVERY_ERROR_CHARS,
    )
}

fn report_new_failure(slot: &mut Option<String>, message: String) -> Option<String> {
    if slot.as_deref() == Some(message.as_str()) {
        return None;
    }
    *slot = Some(message.clone());
    Some(message)
}

/// Bell configuration
#[derive(Debug, Clone)]
pub struct BellConfig {
    /// Enable visual bell (screen flash)
    pub visual: bool,
    /// Enable audio bell
    pub audio: bool,
    /// Duration of visual bell flash in milliseconds
    pub visual_duration_ms: u64,
    /// Minimum time between bells to prevent spam
    pub rate_limit_ms: u64,
}

impl Default for BellConfig {
    fn default() -> Self {
        Self {
            visual: true,
            audio: false, // Audio can be annoying
            visual_duration_ms: 100,
            rate_limit_ms: 50,
        }
    }
}

/// Bell state tracker
pub struct Bell {
    /// Configuration
    config: BellConfig,
    /// Time of last bell
    last_bell: Option<Instant>,
    /// Whether visual bell is currently active
    visual_active: bool,
    /// When visual bell started
    visual_start: Option<Instant>,
    audio_command: (String, Vec<String>),
    last_audio_failure: Option<String>,
}

impl Bell {
    /// Create a new bell handler
    pub fn new(config: BellConfig) -> Self {
        Self {
            config,
            last_bell: None,
            visual_active: false,
            visual_start: None,
            audio_command: default_audio_command(),
            last_audio_failure: None,
        }
    }

    /// Trigger the bell
    pub fn ring(&mut self) -> Option<String> {
        // Rate limit
        if let Some(last) = self.last_bell {
            if last.elapsed().as_millis() < self.config.rate_limit_ms as u128 {
                return None;
            }
        }

        self.last_bell = Some(Instant::now());

        // Visual bell
        if self.config.visual {
            self.visual_active = true;
            self.visual_start = Some(Instant::now());
        }

        // Audio bell
        if self.config.audio {
            if let Err(error) = self.play_audio() {
                let message = bounded_delivery_error("Audio bell unavailable", &error);
                log::warn!("{message}");
                return report_new_failure(&mut self.last_audio_failure, message);
            }
            self.last_audio_failure = None;
        }

        log::debug!("Bell triggered");
        None
    }

    /// Check if visual bell should be displayed
    pub fn is_visual_active(&self) -> bool {
        self.visual_active
    }

    /// Update bell state (call each frame)
    pub fn update(&mut self) {
        if let Some(start) = self.visual_start {
            if start.elapsed().as_millis() >= self.config.visual_duration_ms as u128 {
                self.visual_active = false;
                self.visual_start = None;
            }
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

    /// Play audio bell
    fn play_audio(&self) -> Result<(), String> {
        let (program, args) = &self.audio_command;
        spawn_desktop_command(program, args, "audio bell")
    }

    /// Update configuration
    pub fn set_config(&mut self, config: BellConfig) {
        self.config = config;
    }

    /// Enable/disable visual bell
    pub fn set_visual(&mut self, enabled: bool) {
        self.config.visual = enabled;
    }

    /// Enable/disable audio bell
    pub fn set_audio(&mut self, enabled: bool) {
        self.config.audio = enabled;
        self.last_audio_failure = None;
    }

    #[cfg(test)]
    pub(crate) fn set_audio_command_for_test(&mut self, program: impl Into<String>) {
        self.audio_command = (program.into(), Vec::new());
    }
}

fn default_audio_command() -> (String, Vec<String>) {
    (
        "paplay".into(),
        vec!["/usr/share/sounds/freedesktop/stereo/bell.oga".into()],
    )
}

impl Default for Bell {
    fn default() -> Self {
        Self::new(BellConfig::default())
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
    last_delivery_failure: Option<String>,
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
            last_delivery_failure: None,
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
        let message = bounded_delivery_error("Desktop notification unavailable", error);
        log::warn!("{message}");
        report_new_failure(&mut self.last_delivery_failure, message)
    }

    /// Drain asynchronous lifecycle failures for visible app reporting.
    pub fn drain_completions(&mut self) -> Vec<NotificationCompletion> {
        self.tracked
            .as_ref()
            .map_or_else(Vec::new, tracked::TrackedNotifications::drain_completions)
            .into_iter()
            .filter_map(|completion| match completion {
                tracked::TrackedCompletion::Recovered => {
                    self.last_delivery_failure = None;
                    None
                }
                tracked::TrackedCompletion::Failure(error) => {
                    let message =
                        bounded_delivery_error("Desktop notification unavailable", &error);
                    report_new_failure(&mut self.last_delivery_failure, message)
                        .map(NotificationCompletion::Warning)
                }
                tracked::TrackedCompletion::ProtocolReply(reply) => {
                    Some(NotificationCompletion::ProtocolReply {
                        origin: reply.origin,
                        data: reply.data,
                    })
                }
                tracked::TrackedCompletion::Activation(activation) => {
                    Some(NotificationCompletion::Activation {
                        origin: activation.origin,
                        focus: activation.focus,
                        protocol_reply: activation.protocol_reply,
                        activation_token: activation.activation_token,
                    })
                }
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
mod desktop_worker_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bells_inside_the_rate_limit_are_ignored() {
        let mut bell = Bell::new(BellConfig {
            rate_limit_ms: 60_000,
            ..Default::default()
        });
        bell.ring();
        assert!(bell.is_visual_active());
        let first = bell.last_bell;
        bell.ring();
        assert_eq!(bell.last_bell, first);

        bell.set_config(BellConfig {
            rate_limit_ms: 0,
            ..Default::default()
        });
        bell.ring();
        assert_ne!(bell.last_bell, first);
    }

    #[test]
    fn test_visual_intensity() {
        let bell = Bell::new(BellConfig::default());
        assert_eq!(bell.visual_intensity(), 0.0);
    }

    #[test]
    fn audio_helper_failures_are_bounded_deduplicated_and_recoverable() {
        let mut bell = Bell::new(BellConfig {
            visual: false,
            audio: true,
            rate_limit_ms: 0,
            ..BellConfig::default()
        });
        bell.set_audio_command_for_test("/definitely/missing/termeric-audio-helper");
        let first = bell.ring().unwrap();
        assert!(first.contains("Audio bell unavailable"));
        assert!(first.contains("termeric-audio-helper"));
        assert!(bell.ring().is_none());

        bell.set_audio_command_for_test("/bin/true");
        assert!(bell.ring().is_none());
        bell.set_audio_command_for_test("/definitely/missing/termeric-audio-helper");
        assert!(bell.ring().is_some());
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
        let error = bounded_delivery_error("Desktop notification unavailable", &"x".repeat(1_024));
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
