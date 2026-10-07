//! The guardrail confirmation's keys and actions: confirming or cancelling a
//! held destructive write.

use super::*;

impl App {
    /// The guardrail card's geometry, shared by drawing, pointer hits, and
    /// accessibility.
    pub(super) fn guardrail_layout(&self) -> crate::ui::guardrail::GuardrailLayout {
        let metrics = self.ui_metrics();
        crate::ui::guardrail::GuardrailLayout::calculate(
            metrics.width,
            metrics.height,
            metrics.cell_width,
            metrics.cell_height,
            self.guardrail.animation(),
        )
    }

    /// Ctrl+Enter confirms the guarded write; Enter or Escape cancels it.
    pub(super) fn handle_guardrail_key(&mut self, key: &winit::keyboard::Key) {
        use winit::keyboard::{Key, NamedKey};
        let request_id = self.guardrail.request_id();
        let action = match key {
            Key::Named(NamedKey::Enter) if self.modifiers.control_key() => {
                crate::terminal::guardrails::GuardrailAction::Confirm { request_id }
            }
            Key::Named(NamedKey::Enter | NamedKey::Escape) => {
                crate::terminal::guardrails::GuardrailAction::Cancel { request_id }
            }
            _ => crate::terminal::guardrails::GuardrailAction::None,
        };
        self.apply_guardrail_action(action);
        self.request_redraw(RedrawReason::KeyboardInput);
    }

    pub(super) fn apply_guardrail_action(
        &mut self,
        action: crate::terminal::guardrails::GuardrailAction,
    ) {
        use crate::terminal::guardrails::GuardrailAction;
        match action {
            GuardrailAction::Confirm { request_id } => {
                let Some(writes) = self.guardrail.confirm(request_id) else {
                    return;
                };
                let focused_origin = self.active_terminal_origin();
                let result = self
                    .tabs
                    .as_mut()
                    .ok_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            "originating terminal no longer exists",
                        )
                    })
                    .and_then(|tabs| {
                        // Validate the entire captured route before sending any
                        // member. A vanished pane must not produce a partial
                        // broadcast or fall back to the current focus.
                        let writes = writes
                            .into_iter()
                            .map(|write| {
                                let origin = write.resolve_origin(tabs).ok_or_else(|| {
                                    std::io::Error::new(
                                        std::io::ErrorKind::NotFound,
                                        "one or more originating terminals no longer exist",
                                    )
                                })?;
                                Ok((origin, write.data))
                            })
                            .collect::<std::io::Result<Vec<_>>>()?;
                        if writes
                            .iter()
                            .any(|(origin, _)| tabs.origin_read_only(*origin) != Some(false))
                        {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::PermissionDenied,
                                "one or more originating panes are read-only",
                            ));
                        }
                        let affects_focused_input = writes
                            .iter()
                            .any(|(origin, _)| Some(*origin) == focused_origin);
                        for (origin, data) in writes {
                            tabs.write_user_to_origin(origin, &data)?;
                        }
                        Ok(affects_focused_input)
                    });
                if let Err(error) = result {
                    log::error!("Guarded command was not sent: {error}");
                    self.toasts
                        .error(format!("Guarded command was not sent: {error}"));
                } else if matches!(result, Ok(true)) {
                    self.clear_terminal_input_tracking();
                    self.reset_cursor_blink();
                }
            }
            GuardrailAction::Cancel { request_id } => {
                self.guardrail.dismiss(request_id);
            }
            GuardrailAction::None => {}
        }
    }
}
