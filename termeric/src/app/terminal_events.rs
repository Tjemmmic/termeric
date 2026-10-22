//! Events a terminal raises for the window: titles, bells, notifications,
//! clipboard requests, color queries, and the other app-mediated replies.

use super::clipboard::{ClipboardAccess, ClipboardError};
use super::*;
use base64::Engine as _;

/// The OSC 52 reply carrying `text` from `selection`: standard, padded
/// base64, as xterm sends and every OSC 52 client decodes.
fn clipboard_reply(selection: termeric_core::ClipboardRequest, text: &str) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    format!("\x1b]52;{};{}\x1b\\", selection.osc_code(), encoded)
}

fn dynamic_color_reply(
    target: termeric_core::DynamicColorTarget,
    color: termeric_core::Color,
) -> String {
    use termeric_core::DynamicColorTarget;
    let specification = format!(
        "rgb:{:04x}/{:04x}/{:04x}",
        u16::from(color.r) * 257,
        u16::from(color.g) * 257,
        u16::from(color.b) * 257
    );
    match target {
        DynamicColorTarget::Palette(index) => format!("\x1b]4;{index};{specification}\x1b\\"),
        DynamicColorTarget::Foreground => format!("\x1b]10;{specification}\x1b\\"),
        DynamicColorTarget::Background => format!("\x1b]11;{specification}\x1b\\"),
        DynamicColorTarget::Cursor => format!("\x1b]12;{specification}\x1b\\"),
    }
}

impl App {
    /// Handle a terminal event.
    pub(super) fn handle_terminal_event(&mut self, routed: RoutedTerminalEvent) {
        let RoutedTerminalEvent { origin, event } = routed;
        match event {
            TerminalEvent::Bell => {
                if let Some(error) = self.bell.ring() {
                    self.toasts.warning(error);
                }
            }
            TerminalEvent::Notification(req) => {
                if !self.should_present_terminal_notification(origin, req.occasion) {
                    log::debug!(
                        "Suppressed OSC notification from {origin:?} for {:?} occasion",
                        req.occasion
                    );
                    return;
                }
                let Some(origin) = self
                    .tabs
                    .as_ref()
                    .and_then(|tabs| tabs.identity_for_origin(origin))
                else {
                    return;
                };
                if let Some(error) = self.notifications.notify_terminal(origin, req) {
                    self.toasts.warning(error);
                }
            }
            TerminalEvent::NotificationClose { identifier } => {
                let Some(origin) = self
                    .tabs
                    .as_ref()
                    .and_then(|tabs| tabs.identity_for_origin(origin))
                else {
                    return;
                };
                if let Some(error) = self.notifications.close_terminal(origin, identifier) {
                    self.toasts.warning(error);
                }
            }
            TerminalEvent::NotificationAliveQuery {
                identifier,
                reply_uses_8bit_controls,
            } => {
                let Some(origin) = self
                    .tabs
                    .as_ref()
                    .and_then(|tabs| tabs.identity_for_origin(origin))
                else {
                    return;
                };
                if let Some(error) =
                    self.notifications
                        .query_alive(origin, identifier, reply_uses_8bit_controls)
                {
                    self.toasts.warning(error);
                }
            }
            TerminalEvent::KittyFileTransfer(command) => {
                self.handle_kitty_file_transfer(origin, command);
            }
            TerminalEvent::TitleChanged(title) => {
                // The program keeps its exact title for title reports; the
                // tab bar and window title drop what could break or reorder
                // the line around it.
                let title = title
                    .chars()
                    .filter(|character| !crate::safe_text::is_unsafe_display_char(*character))
                    .collect();
                let updated = self
                    .tabs
                    .as_mut()
                    .is_some_and(|tabs| tabs.set_title_for_origin(origin, title));
                if updated {
                    // Inactive-tab events leave the native title unchanged
                    // because this always derives from the current active tab.
                    self.sync_native_window_title();
                } else {
                    log::debug!("Ignored OSC title for stale origin {:?}", origin);
                }
            }
            TerminalEvent::ClipboardRequest {
                selection: which,
                reply_uses_8bit_controls,
            } => {
                // OSC 52 read: echo the clipboard contents back to the PTY
                // as a base64 reply. Denial, unavailability, and an oversized
                // clipboard deliberately return an empty payload so clients do
                // not hang and no clipboard content leaks.
                let text = if self.config.security.osc52.allow_read {
                    self.clipboard
                        .text(which, ClipboardAccess::Implicit)
                        .ok()
                        .filter(|text| text.len() <= self.config.security.osc52.max_bytes)
                        .unwrap_or_default()
                } else {
                    log::debug!("Denied OSC 52 clipboard read from {:?}", origin);
                    String::new()
                };
                let canonical = clipboard_reply(which, &text);
                if let Some(tabs) = &mut self.tabs {
                    let reply = termeric_core::Terminal::frame_pty_reply_for_mode(
                        canonical.as_bytes(),
                        reply_uses_8bit_controls,
                    );
                    if let Err(error) = tabs.write_to_origin(origin, &reply) {
                        log::warn!(
                            "Could not return clipboard reply to {:?}: {}",
                            origin,
                            error
                        );
                    }
                }
            }
            TerminalEvent::ClipboardWrite { selection, text } => {
                if !self.config.security.osc52.allow_write {
                    log::debug!("Denied OSC 52 {:?} write from {:?}", selection, origin);
                } else if text.len() > self.config.security.osc52.max_bytes {
                    log::warn!("Rejected oversized OSC 52 write from {:?}", origin);
                } else {
                    match self
                        .clipboard
                        .set_text(selection, &text, ClipboardAccess::Implicit)
                    {
                        Ok(()) => {}
                        Err(error @ ClipboardError::Unavailable(_)) => {
                            if self.clipboard.first_missing_write() {
                                log::warn!(
                                    "Dropped OSC 52 {selection:?} write from {origin:?}: {error}"
                                );
                            }
                        }
                        Err(error) => log::error!("Failed to set clipboard: {error}"),
                    }
                }
            }
            TerminalEvent::DynamicColor(operation) => {
                use termeric_core::DynamicColorOperation;
                match operation {
                    DynamicColorOperation::Query {
                        target,
                        reply_uses_8bit_controls,
                    } => {
                        let color = self.dynamic_color_for_origin(origin, target);
                        if let Some(color) = color {
                            let canonical = dynamic_color_reply(target, color);
                            if let Some(tabs) = &mut self.tabs {
                                let reply = termeric_core::Terminal::frame_pty_reply_for_mode(
                                    canonical.as_bytes(),
                                    reply_uses_8bit_controls,
                                );
                                if let Err(error) = tabs.write_to_origin(origin, &reply) {
                                    log::warn!(
                                        "Could not return dynamic-color reply to {:?}: {}",
                                        origin,
                                        error
                                    );
                                }
                            }
                        }
                    }
                    operation => {
                        if let Some(terminal) = self
                            .tabs
                            .as_mut()
                            .and_then(|tabs| tabs.terminal_for_origin_mut(origin))
                        {
                            terminal.apply_dynamic_color(operation);
                        }
                    }
                }
            }
            TerminalEvent::PointerShapeChanged => {
                if self.active_terminal_origin() == Some(origin) {
                    self.refresh_pointer_cursor();
                }
            }
            TerminalEvent::PtyWrite(_) => {
                // Consumed by the originating tab/pane poller. Reaching App
                // would be a routing invariant violation.
                log::error!(
                    "Terminal protocol reply escaped its PTY owner: {:?}",
                    origin
                );
            }
        }
    }

    fn dynamic_color_for_origin(
        &self,
        origin: crate::tabs::TerminalEventOrigin,
        target: termeric_core::DynamicColorTarget,
    ) -> Option<termeric_core::Color> {
        use termeric_core::DynamicColorTarget;
        let terminal = self.tabs.as_ref()?.terminal_for_origin(origin)?;
        let dynamic = terminal.dynamic_colors();
        let theme_color = |color: crate::config::theme::Color| {
            termeric_core::Color::rgb(color.r, color.g, color.b)
        };
        Some(match target {
            DynamicColorTarget::Palette(index) => dynamic.palette(index).unwrap_or_else(|| {
                if index < 16 {
                    theme_color(self.theme.ansi_color(index))
                } else {
                    termeric_core::ansi::color_256(index)
                }
            }),
            DynamicColorTarget::Foreground => dynamic
                .foreground()
                .unwrap_or_else(|| theme_color(self.theme.ui.foreground)),
            DynamicColorTarget::Background => dynamic
                .background()
                .unwrap_or_else(|| theme_color(self.theme.ui.background)),
            DynamicColorTarget::Cursor => dynamic
                .cursor()
                .unwrap_or_else(|| theme_color(self.theme.ui.cursor)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_color_queries_use_xterm_16_bit_rgb_and_st() {
        let color = termeric_core::Color::rgb(255, 128, 0);
        assert_eq!(
            dynamic_color_reply(termeric_core::DynamicColorTarget::Palette(3), color),
            "\x1b]4;3;rgb:ffff/8080/0000\x1b\\"
        );
        assert_eq!(
            dynamic_color_reply(termeric_core::DynamicColorTarget::Cursor, color),
            "\x1b]12;rgb:ffff/8080/0000\x1b\\"
        );
    }

    #[test]
    fn clipboard_replies_use_standard_padded_base64() {
        use termeric_core::ClipboardRequest;
        for (text, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            // The standard alphabet's last two digits, not URL-safe - and _.
            ("~~~???", "fn5+Pz8/"),
            ("é", "w6k="),
        ] {
            assert_eq!(
                clipboard_reply(ClipboardRequest::Clipboard, text),
                format!("\x1b]52;c;{encoded}\x1b\\")
            );
        }
        assert_eq!(
            clipboard_reply(ClipboardRequest::Primary, "hi"),
            "\x1b]52;p;aGk=\x1b\\"
        );
    }

    #[test]
    fn app_mediated_osc_replies_use_the_origin_terminal_c1_mode() {
        let mut terminal = termeric_core::Terminal::new(8, 4);
        let color = dynamic_color_reply(
            termeric_core::DynamicColorTarget::Background,
            termeric_core::Color::rgb(0x20, 0x18, 0x14),
        );
        assert_eq!(
            terminal.frame_pty_reply(color.as_bytes()),
            b"\x1b]11;rgb:2020/1818/1414\x1b\\"
        );

        terminal.process(b"\x1b G");
        let captured_mode = terminal.uses_8bit_controls();
        terminal.process(b"\x1b F");
        assert!(!terminal.uses_8bit_controls());
        assert_eq!(
            termeric_core::Terminal::frame_pty_reply_for_mode(color.as_bytes(), captured_mode),
            b"\x9d11;rgb:2020/1818/1414\x9c"
        );
        assert_eq!(
            termeric_core::Terminal::frame_pty_reply_for_mode(b"\x1b]52;c;\x1b\\", captured_mode,),
            b"\x9d52;c;\x9c"
        );
    }
}
