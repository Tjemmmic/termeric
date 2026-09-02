//! Bell and notification handling
//!
//! Handles terminal bell (BEL character) with visual and audio feedback.

use std::process::{Child, Command};
use std::sync::OnceLock;
use std::sync::mpsc::{self, SyncSender, TryRecvError, TrySendError};
use std::time::{Duration, Instant};

const DESKTOP_COMMAND_QUEUE_CAPACITY: usize = 16;
const MAX_DESKTOP_HELPER_CHILDREN: usize = 8;
const MAX_DELIVERY_ERROR_CHARS: usize = 512;

struct DesktopCommandRequest {
    program: String,
    args: Vec<String>,
    label: &'static str,
    started: SyncSender<Result<(), String>>,
}

static DESKTOP_COMMAND_RUNNER: OnceLock<Result<SyncSender<DesktopCommandRequest>, String>> =
    OnceLock::new();

fn desktop_command_sender() -> Result<&'static SyncSender<DesktopCommandRequest>, String> {
    DESKTOP_COMMAND_RUNNER
        .get_or_init(|| {
            let (sender, receiver) = mpsc::sync_channel(DESKTOP_COMMAND_QUEUE_CAPACITY);
            std::thread::Builder::new()
                .name("termeric-desktop-command".into())
                .spawn(move || {
                    let mut children: Vec<(&'static str, Child)> = Vec::new();
                    loop {
                        match receiver.recv_timeout(Duration::from_millis(50)) {
                            Ok(request) => start_desktop_command(request, &mut children),
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                        loop {
                            match receiver.try_recv() {
                                Ok(request) => start_desktop_command(request, &mut children),
                                Err(TryRecvError::Empty) => break,
                                Err(TryRecvError::Disconnected) => return,
                            }
                        }
                        let mut index = 0;
                        while index < children.len() {
                            let completed = match children[index].1.try_wait() {
                                Ok(Some(status)) => {
                                    if !status.success() {
                                        log::warn!(
                                            "{} helper exited unsuccessfully: {}",
                                            children[index].0,
                                            status
                                        );
                                    }
                                    true
                                }
                                Ok(None) => false,
                                Err(error) => {
                                    log::warn!(
                                        "Could not reap {} helper: {}",
                                        children[index].0,
                                        error
                                    );
                                    true
                                }
                            };
                            if completed {
                                children.swap_remove(index);
                            } else {
                                index += 1;
                            }
                        }
                    }
                })
                .map(|_| sender)
                .map_err(|error| format!("could not start desktop helper worker: {error}"))
        })
        .as_ref()
        .map_err(Clone::clone)
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
    let (started, result) = mpsc::sync_channel(0);
    let request = DesktopCommandRequest {
        program: program.to_owned(),
        args: args.to_vec(),
        label,
        started,
    };
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
    let message = format!("{prefix}: {detail}");
    if message.chars().count() <= MAX_DELIVERY_ERROR_CHARS {
        return message;
    }
    message
        .chars()
        .take(MAX_DELIVERY_ERROR_CHARS.saturating_sub(1))
        .chain(std::iter::once('…'))
        .collect()
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
    audio_command: Option<(String, Vec<String>)>,
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
        let Some((program, args)) = &self.audio_command else {
            return Err("no audio helper is available on this platform".into());
        };
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
        self.audio_command = Some((program.into(), Vec::new()));
    }
}

fn default_audio_command() -> Option<(String, Vec<String>)> {
    #[cfg(target_os = "linux")]
    return Some((
        "paplay".into(),
        vec!["/usr/share/sounds/freedesktop/stereo/bell.oga".into()],
    ));
    #[cfg(target_os = "macos")]
    return Some((
        "afplay".into(),
        vec!["/System/Library/Sounds/Tink.aiff".into()],
    ));
    #[cfg(target_os = "windows")]
    return Some((
        "powershell".into(),
        vec!["-c".into(), "[console]::beep(800,100)".into()],
    ));
    #[allow(unreachable_code)]
    None
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
    command: Option<String>,
    last_delivery_failure: Option<String>,
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
            command: default_notification_command(),
            last_delivery_failure: None,
        }
    }

    /// Send a desktop notification
    pub fn notify(&mut self, title: &str, body: &str) -> Option<String> {
        if !self.enabled {
            return None;
        }

        // Rate limit
        if let Some(last) = self.last_notification {
            if last.elapsed() < self.rate_limit {
                return None;
            }
        }

        self.last_notification = Some(Instant::now());

        let Some(command) = &self.command else {
            log::info!("Notification: {} - {}", title, body);
            return None;
        };
        let args = notification_args(title, body);
        if let Err(error) = spawn_desktop_command(command, &args, "desktop notification") {
            let message = bounded_delivery_error("Desktop notification unavailable", &error);
            log::warn!("{message}");
            return report_new_failure(&mut self.last_delivery_failure, message);
        }
        self.last_delivery_failure = None;

        log::debug!("Sent notification: {} - {}", title, body);
        None
    }

    #[cfg(test)]
    pub(crate) fn set_command_for_test(&mut self, program: impl Into<String>) {
        self.command = Some(program.into());
        self.rate_limit = Duration::ZERO;
    }
}

fn default_notification_command() -> Option<String> {
    #[cfg(target_os = "linux")]
    return Some("notify-send".into());
    #[cfg(target_os = "macos")]
    return Some("osascript".into());
    #[allow(unreachable_code)]
    None
}

fn notification_args(title: &str, body: &str) -> Vec<String> {
    #[cfg(target_os = "linux")]
    return vec!["--app-name=Termeric".into(), title.into(), body.into()];
    #[cfg(target_os = "macos")]
    return vec![
        "-e".into(),
        format!(
            "display notification \"{}\" with title \"{}\"",
            body.replace('"', "\\\""),
            title.replace('"', "\\\"")
        ),
    ];
    #[allow(unreachable_code)]
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bell_rate_limit() {
        let mut bell = Bell::new(BellConfig {
            rate_limit_ms: 100,
            ..Default::default()
        });

        bell.ring();
        assert!(bell.is_visual_active());

        // Immediate second ring should be rate limited
        // (can't easily test timing in unit tests)
    }

    #[test]
    fn test_visual_intensity() {
        let bell = Bell::new(BellConfig::default());
        assert_eq!(bell.visual_intensity(), 0.0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn helper_spawn_failures_are_bounded_deduplicated_and_recoverable() {
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

        let mut notifications = Notifications::new();
        notifications.set_command_for_test("/definitely/missing/termeric-notify-helper");
        let first = notifications.notify("title", "body").unwrap();
        assert!(first.contains("Desktop notification unavailable"));
        assert!(first.contains("termeric-notify-helper"));
        assert!(notifications.notify("title", "body").is_none());

        notifications.set_command_for_test("/bin/true");
        assert!(notifications.notify("title", "body").is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn delivery_errors_have_a_hard_character_ceiling() {
        let mut notifications = Notifications::new();
        notifications.set_command_for_test(format!("/missing/{}", "x".repeat(1_024)));
        let error = notifications.notify("title", "body").unwrap();
        assert!(error.chars().count() <= MAX_DELIVERY_ERROR_CHARS);
        assert!(error.ends_with('…'));
    }
}
