//! The one switch for everything agent-related (`[agent_features]`).
//!
//! Off, this window offers no agent: no recognition or badges, no agent
//! commands or shortcuts, no agent view, supervisor, or Agents panel, and
//! programs asking over the control socket are told the features are off.
//! Agents running in terminals are ordinary programs and keep running.
//!
//! Turning the switch off ends nothing the person is in the middle of: while
//! an agent view conversation or the supervisor runs here, however the
//! configuration changed, agent features stay on in this window, with a
//! notice, and turn off once the last of them ends. Then everything else
//! agent-related that is open closes at once.

use super::*;
use crate::agents::config::{AGENT_FEATURES_HELD, AGENT_FEATURES_OFF, AGENT_FEATURES_OFF_REPLY};
use crate::ui::command_palette::palette::PickKind;

impl App {
    /// Whether agent features are on in this window: the configuration has
    /// them on, or they are held on for a conversation. Every agent entry
    /// point asks this.
    pub(super) fn agent_features_enabled(&self) -> bool {
        self.config.agent_features_enabled() || self.agent_features_held
    }

    /// Whether this window's configuration has agent features on, as
    /// opposed to their being held on until its conversations end.
    pub(super) fn agent_features_configured(&self) -> bool {
        self.config.agent_features_enabled()
    }

    /// `Ok` while agent features are on; otherwise the reply a program asking
    /// for one of them gets.
    pub(super) fn require_agent_features(&self) -> Result<(), String> {
        if self.agent_features_enabled() {
            Ok(())
        } else {
            Err(AGENT_FEATURES_OFF_REPLY.to_owned())
        }
    }

    /// Tell the person that what they asked for is an agent feature, and
    /// where to turn agent features on.
    pub(super) fn report_agent_features_off(&mut self) {
        self.toasts.info(AGENT_FEATURES_OFF);
        self.request_redraw(RedrawReason::Action);
    }

    /// Whether an agent view conversation or the supervisor runs in this
    /// window: turning agent features off would have to end it.
    fn agent_conversations_running(&self) -> bool {
        self.supervisor_running() || self.agent_view.conversations().next().is_some()
    }

    /// Whether agent features stay on, held, once `next` replaces this
    /// window's configuration: it turns them off while a conversation runs.
    pub(super) fn holds_agent_features_against(&self, next: &Config) -> bool {
        self.agent_features_enabled()
            && !next.agent_features_enabled()
            && self.agent_conversations_running()
    }

    /// Turn agent features off once the conversations they were held on for
    /// have ended. Returns whether they turned off.
    pub(super) fn release_agent_features(&mut self) -> bool {
        if !self.agent_features_held || self.agent_conversations_running() {
            return false;
        }
        self.agent_features_held = false;
        self.apply_config_side_effects(true);
        true
    }

    /// Bring this window in line with the switch. Off, every agent surface
    /// closes and no terminal keeps an agent's recognition or status.
    pub(super) fn sync_agent_features(&mut self) {
        if self.agent_features_held {
            self.agent_features_held_notice.show(
                &mut self.toasts,
                crate::ui::toast::ToastSeverity::Info,
                AGENT_FEATURES_HELD.to_owned(),
            );
        } else {
            self.agent_features_held_notice.clear(&mut self.toasts);
        }
        if self.agent_features_enabled() {
            return;
        }
        self.close_agent_surfaces();
        self.forget_terminal_agents();
    }

    /// While agent features are off, drop what any terminal still carries
    /// about an agent, such as a pane moved here from a window where they
    /// are on. Returns whether anything changed.
    pub(super) fn forget_terminal_agents(&mut self) -> bool {
        self.tabs.as_mut().is_some_and(TabManager::forget_agents)
    }

    /// Close every agent surface that is open. The agent view sessions left
    /// are blank ones, started ahead of a first message, and end with their
    /// panels.
    fn close_agent_surfaces(&mut self) {
        if self.agent_view.state.is_open() {
            self.close_agent_view();
        }
        if let Some(id) = self.agent_view.state.active
            && self
                .agent_view
                .session(id)
                .is_some_and(crate::agent_view::session::AgentSession::is_blank)
        {
            self.end_agent_session(id);
        }
        // Other tabs' panels stay closed until agent features return; each
        // keeps what its terminal was last told about focus.
        for panel in self.agent_view.tabs.values_mut() {
            panel.open = false;
        }
        self.agents_panel.close();
        if matches!(
            self.command_palette.active_pick(),
            Some(
                PickKind::Agent(_)
                    | PickKind::AgentTerminal
                    | PickKind::AgentChange
                    | PickKind::SupervisorMode
            )
        ) {
            self.command_palette.close();
        }
        if self.context_picker.is_active() {
            let request_id = self.context_picker.request_id();
            self.apply_context_picker_action(
                crate::ui::context_picker::ContextPickerAction::Cancel { request_id },
            );
        }
        if self.diff_review.is_awaiting_current() {
            let request_id = self.diff_review.request_id();
            self.apply_diff_review_action(crate::ui::diff_review::DiffReviewAction::Dismiss {
                request_id,
            });
        }
        self.close_agent_change_review();
        self.close_agent_hook_setup();
        self.request_redraw(RedrawReason::ConfigReload);
    }
}
