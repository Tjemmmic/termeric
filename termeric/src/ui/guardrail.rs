//! Geometry of the guardrail confirmation card.

use crate::terminal::guardrails::GuardrailAction;
use crate::ui::geometry::Bounds;

/// Guardrail modal geometry shared by rendering, pointer hit testing, and
/// accessibility. Confirm and Cancel are deliberately separate shapes so the
/// safety decision never depends on color alone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GuardrailLayout {
    pub card: Bounds,
    pub confirm_button: Bounds,
    pub cancel_button: Bounds,
    pub close_button: Bounds,
    pub padding: f32,
    pub title_height: f32,
    pub command_height: f32,
    pub reason_height: f32,
    pub context_height: f32,
    pub footer_height: f32,
}

impl GuardrailLayout {
    pub fn calculate(
        window_width: f32,
        window_height: f32,
        cell_width: f32,
        cell_height: f32,
        animation: f32,
    ) -> Self {
        let padding = cell_width * 2.0;
        let title_height = cell_height * 2.5;
        let command_height = cell_height * 2.0;
        let reason_height = cell_height * 1.8;
        let context_height = cell_height * 1.5;
        let footer_height = cell_height * 2.6;
        let card_height = padding
            + title_height
            + command_height
            + reason_height
            + context_height
            + footer_height
            + padding;
        let card_width = (cell_width * 50.0).max(400.0).min(window_width * 0.7);
        let card_x = (window_width - card_width) / 2.0;
        let card_y = (window_height - card_height) / 2.0 + (1.0 - animation) * -20.0;
        let button_gap = cell_width;
        let button_height = cell_height * 1.7;
        let available_button_width = card_width - padding * 2.0 - button_gap;
        let button_width = (available_button_width / 2.0).max(1.0);
        let button_y = card_y + card_height - padding - button_height;
        let cancel_button = Bounds {
            x: card_x + padding,
            y: button_y,
            width: button_width,
            height: button_height,
        };
        let confirm_button = Bounds {
            x: cancel_button.x + button_width + button_gap,
            y: button_y,
            width: button_width,
            height: button_height,
        };
        let close_size = cell_height * 1.5;
        Self {
            card: Bounds {
                x: card_x,
                y: card_y,
                width: card_width,
                height: card_height,
            },
            confirm_button,
            cancel_button,
            close_button: Bounds {
                x: card_x + card_width - padding - close_size / 2.0,
                y: card_y + padding - close_size / 2.0,
                width: close_size,
                height: close_size,
            },
            padding,
            title_height,
            command_height,
            reason_height,
            context_height,
            footer_height,
        }
    }

    pub fn hit_test(&self, x: f32, y: f32, request_id: u64) -> GuardrailAction {
        if self.confirm_button.contains(x, y) {
            GuardrailAction::Confirm { request_id }
        } else if self.cancel_button.contains(x, y)
            || self.close_button.contains(x, y)
            || !self.card.contains(x, y)
        {
            GuardrailAction::Cancel { request_id }
        } else {
            GuardrailAction::None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_layout_has_disjoint_explicit_decision_targets() {
        let layout = GuardrailLayout::calculate(1200.0, 800.0, 10.0, 20.0, 1.0);
        assert!(!layout.cancel_button.contains(
            layout.confirm_button.x + layout.confirm_button.width / 2.0,
            layout.confirm_button.y + layout.confirm_button.height / 2.0,
        ));
        assert_eq!(
            layout.hit_test(
                layout.cancel_button.x + layout.cancel_button.width / 2.0,
                layout.cancel_button.y + layout.cancel_button.height / 2.0,
                7,
            ),
            GuardrailAction::Cancel { request_id: 7 }
        );
        assert_eq!(
            layout.hit_test(
                layout.confirm_button.x + layout.confirm_button.width / 2.0,
                layout.confirm_button.y + layout.confirm_button.height / 2.0,
                7,
            ),
            GuardrailAction::Confirm { request_id: 7 }
        );
        assert_eq!(
            layout.hit_test(layout.card.x - 1.0, layout.card.y - 1.0, 7),
            GuardrailAction::Cancel { request_id: 7 }
        );
    }
}
