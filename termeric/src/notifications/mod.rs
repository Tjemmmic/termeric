//! Bell and notification handling
//!
//! Handles terminal bell (BEL character) with visual and audio feedback.

use std::time::{Duration, Instant};

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
}

impl Bell {
    /// Create a new bell handler
    pub fn new(config: BellConfig) -> Self {
        Self {
            config,
            last_bell: None,
            visual_active: false,
            visual_start: None,
        }
    }

    /// Trigger the bell
    pub fn ring(&mut self) {
        // Rate limit
        if let Some(last) = self.last_bell {
            if last.elapsed().as_millis() < self.config.rate_limit_ms as u128 {
                return;
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
            self.play_audio();
        }

        log::debug!("Bell triggered");
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
    fn play_audio(&self) {
        // Platform-specific audio bell
        #[cfg(target_os = "linux")]
        {
            // Try to use the system bell
            let _ = std::process::Command::new("paplay")
                .arg("/usr/share/sounds/freedesktop/stereo/bell.oga")
                .spawn();
        }

        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("afplay")
                .arg("/System/Library/Sounds/Tink.aiff")
                .spawn();
        }

        #[cfg(target_os = "windows")]
        {
            // Windows has built-in beep functionality
            // Using PowerShell as a simple fallback
            let _ = std::process::Command::new("powershell")
                .args(["-c", "[console]::beep(800,100)"])
                .spawn();
        }
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
    }
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
        }
    }

    /// Send a desktop notification
    pub fn notify(&mut self, title: &str, body: &str) {
        if !self.enabled {
            return;
        }

        // Rate limit
        if let Some(last) = self.last_notification {
            if last.elapsed() < self.rate_limit {
                return;
            }
        }

        self.last_notification = Some(Instant::now());

        // Send notification using platform-specific method
        #[cfg(target_os = "linux")]
        {
            let _ = std::process::Command::new("notify-send")
                .args(["--app-name=Termeric", title, body])
                .spawn();
        }

        #[cfg(target_os = "macos")]
        {
            let script = format!(
                "display notification \"{}\" with title \"{}\"",
                body.replace('"', "\\\""),
                title.replace('"', "\\\"")
            );
            let _ = std::process::Command::new("osascript")
                .args(["-e", &script])
                .spawn();
        }

        #[cfg(target_os = "windows")]
        {
            // Windows notifications require more complex setup
            // For now, just log
            log::info!("Notification: {} - {}", title, body);
        }

        log::debug!("Sent notification: {} - {}", title, body);
    }
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
}
