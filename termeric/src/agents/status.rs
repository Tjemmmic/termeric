//! What an agent is doing, as reported by its hooks.

use std::path::PathBuf;
use std::time::Instant;

use crate::control::protocol::AgentEventKind;

/// The agent's current turn, from the person's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentActivity {
    /// A conversation is open and nothing is in progress.
    Idle,
    Working,
    /// Waiting for the person, for example to approve a tool.
    NeedsInput,
    Finished,
    Failed,
}

impl AgentActivity {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Working => "Working",
            Self::NeedsInput => "Needs you",
            Self::Finished => "Done",
            Self::Failed => "Failed",
        }
    }

    /// Whether this state is worth interrupting the person for.
    pub fn wants_attention(self) -> bool {
        matches!(self, Self::NeedsInput | Self::Finished | Self::Failed)
    }
}

/// What a tab shows for its agents, from least to most urgent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AgentBadge {
    Working,
    Done,
    Failed,
    NeedsYou,
}

impl AgentBadge {
    /// Words for screen readers, following the tab title.
    pub fn description(self) -> &'static str {
        match self {
            Self::Working => "agent working",
            Self::Done => "agent done",
            Self::Failed => "agent failed",
            Self::NeedsYou => "agent needs you",
        }
    }
}

/// One terminal's reported agent state. It exists only while the agent's
/// hooks are reporting, and travels with the terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentStatus {
    /// The catalog agent that reported.
    pub agent_id: String,
    pub activity: AgentActivity,
    /// Why the agent needs the person, or why its turn failed.
    pub detail: Option<String>,
    /// The agent's most recent complete answer.
    pub last_message: Option<String>,
    /// The agent's own conversation log, when it reported one.
    pub transcript_path: Option<PathBuf>,
    /// When `activity` last changed.
    pub since: Instant,
    /// Whether the person has looked at the terminal since `activity` asked
    /// for attention.
    pub seen: bool,
    /// The terminal's foreground process group when the agent last
    /// reported: the agent itself, since its hooks run inside its turn.
    pub process_group: Option<i32>,
}

/// One report from an agent's hooks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentReport {
    pub agent_id: String,
    pub event: AgentEventKind,
    pub detail: Option<String>,
    pub last_message: Option<String>,
    pub transcript_path: Option<PathBuf>,
}

impl AgentReport {
    /// A report built from wire values, with text made safe to display:
    /// control characters are dropped, and the detail becomes one line.
    pub fn from_wire(
        agent_id: String,
        event: AgentEventKind,
        detail: Option<String>,
        last_message: Option<String>,
        transcript_path: Option<String>,
    ) -> Self {
        let detail = detail
            .map(|text| {
                text.split(|c: char| c.is_whitespace() || c.is_control())
                    .filter(|word| !word.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .filter(|text| !text.is_empty());
        let last_message = last_message
            .map(|text| {
                text.chars()
                    .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
                    .collect::<String>()
            })
            .filter(|text| !text.trim().is_empty());
        Self {
            agent_id,
            event,
            detail,
            last_message,
            transcript_path: transcript_path.map(PathBuf::from),
        }
    }
}

impl AgentStatus {
    /// The status after `report`, or `None` once the conversation has ended.
    /// A report from a different agent replaces the status outright.
    pub fn after(current: Option<&Self>, report: AgentReport, now: Instant) -> Option<Self> {
        let current = current.filter(|status| status.agent_id == report.agent_id);
        let activity = match report.event {
            AgentEventKind::SessionEnded => return None,
            // Only a turn in progress stops; a result keeps showing until seen.
            AgentEventKind::Stopped => match current.map(|status| status.activity) {
                Some(AgentActivity::Working | AgentActivity::NeedsInput) | None => {
                    AgentActivity::Idle
                }
                Some(activity) => activity,
            },
            AgentEventKind::SessionStarted => AgentActivity::Idle,
            AgentEventKind::Working => AgentActivity::Working,
            AgentEventKind::NeedsInput => AgentActivity::NeedsInput,
            AgentEventKind::Finished => AgentActivity::Finished,
            AgentEventKind::Failed => AgentActivity::Failed,
        };
        let changed = current.is_none_or(|status| status.activity != activity);
        // A new conversation forgets the previous one's answer.
        let keep_previous = report.event != AgentEventKind::SessionStarted;
        Some(Self {
            agent_id: report.agent_id,
            activity,
            detail: report
                .detail
                .filter(|_| matches!(activity, AgentActivity::NeedsInput | AgentActivity::Failed)),
            last_message: report.last_message.or_else(|| {
                current
                    .filter(|_| keep_previous)
                    .and_then(|status| status.last_message.clone())
            }),
            transcript_path: report
                .transcript_path
                .or_else(|| current.and_then(|status| status.transcript_path.clone())),
            since: if changed {
                now
            } else {
                current.map_or(now, |status| status.since)
            },
            seen: !changed && current.is_some_and(|status| status.seen),
            process_group: current.and_then(|status| status.process_group),
        })
    }

    /// The person answered the agent from its terminal: a waiting agent is
    /// back at work. There is no hook for an approved request, so this is
    /// how the badge learns of it.
    pub fn answered(&mut self, now: Instant) {
        if self.activity == AgentActivity::NeedsInput {
            self.activity = AgentActivity::Working;
            self.detail = None;
            self.since = now;
        }
    }

    /// Whether the terminal should draw attention to itself.
    pub fn needs_attention(&self) -> bool {
        self.activity.wants_attention() && !self.seen
    }

    /// The tab badge for this terminal. A finished or failed turn stops
    /// showing once seen; a request keeps showing until it is answered.
    pub fn badge(&self) -> Option<AgentBadge> {
        match self.activity {
            AgentActivity::Idle => None,
            AgentActivity::Working => Some(AgentBadge::Working),
            AgentActivity::NeedsInput => Some(AgentBadge::NeedsYou),
            AgentActivity::Finished => (!self.seen).then_some(AgentBadge::Done),
            AgentActivity::Failed => (!self.seen).then_some(AgentBadge::Failed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(event: AgentEventKind) -> AgentReport {
        AgentReport {
            agent_id: "codex".into(),
            event,
            detail: None,
            last_message: None,
            transcript_path: None,
        }
    }

    #[test]
    fn a_turn_moves_through_working_needs_you_and_done() {
        let start = Instant::now();
        let started = AgentStatus::after(None, report(AgentEventKind::SessionStarted), start);
        let started = started.unwrap();
        assert_eq!(started.activity, AgentActivity::Idle);
        assert!(!started.needs_attention());

        let working =
            AgentStatus::after(Some(&started), report(AgentEventKind::Working), start).unwrap();
        let needs = AgentStatus::after(
            Some(&working),
            AgentReport {
                detail: Some("Approve Bash".into()),
                ..report(AgentEventKind::NeedsInput)
            },
            start,
        )
        .unwrap();
        assert_eq!(needs.activity, AgentActivity::NeedsInput);
        assert_eq!(needs.detail.as_deref(), Some("Approve Bash"));
        assert!(needs.needs_attention());

        let done = AgentStatus::after(
            Some(&needs),
            AgentReport {
                last_message: Some("All tests pass.".into()),
                transcript_path: Some("/tmp/rollout.jsonl".into()),
                ..report(AgentEventKind::Finished)
            },
            start,
        )
        .unwrap();
        assert_eq!(done.detail, None, "a finished turn keeps no stale request");
        assert_eq!(done.last_message.as_deref(), Some("All tests pass."));

        // The next turn keeps the previous answer until it has its own.
        let next = AgentStatus::after(Some(&done), report(AgentEventKind::Working), start).unwrap();
        assert_eq!(next.last_message.as_deref(), Some("All tests pass."));
        assert_eq!(
            next.transcript_path.as_deref(),
            Some(std::path::Path::new("/tmp/rollout.jsonl"))
        );
        assert!(
            AgentStatus::after(Some(&next), report(AgentEventKind::SessionEnded), start).is_none()
        );
    }

    #[test]
    fn wire_text_loses_control_characters() {
        let report = AgentReport::from_wire(
            "codex".into(),
            AgentEventKind::NeedsInput,
            Some("Approve\x1b[31m\n  rm -rf\tbuild".into()),
            Some("line one\r\nline\x07 two\n".into()),
            None,
        );
        assert_eq!(report.detail.as_deref(), Some("Approve [31m rm -rf build"));
        assert_eq!(report.last_message.as_deref(), Some("line one\nline two\n"));
        let empty = AgentReport::from_wire(
            "codex".into(),
            AgentEventKind::Finished,
            Some(" \n ".into()),
            Some("\n".into()),
            None,
        );
        assert_eq!((empty.detail, empty.last_message), (None, None));
    }

    #[test]
    fn badges_show_requests_until_answered_and_results_until_seen() {
        let now = Instant::now();
        let status = |event, seen| AgentStatus {
            seen,
            ..AgentStatus::after(None, report(event), now).unwrap()
        };
        assert_eq!(status(AgentEventKind::SessionStarted, false).badge(), None);
        assert_eq!(
            status(AgentEventKind::Working, false).badge(),
            Some(AgentBadge::Working)
        );
        assert_eq!(
            status(AgentEventKind::NeedsInput, true).badge(),
            Some(AgentBadge::NeedsYou)
        );
        assert_eq!(
            status(AgentEventKind::Finished, false).badge(),
            Some(AgentBadge::Done)
        );
        assert_eq!(status(AgentEventKind::Finished, true).badge(), None);
        assert_eq!(status(AgentEventKind::Failed, true).badge(), None);
        assert!(AgentBadge::NeedsYou > AgentBadge::Failed);
        assert!(AgentBadge::Failed > AgentBadge::Done);
        assert!(AgentBadge::Done > AgentBadge::Working);
    }

    #[test]
    fn stopping_ends_a_turn_in_progress_but_keeps_results() {
        let now = Instant::now();
        let working = AgentStatus::after(None, report(AgentEventKind::Working), now).unwrap();
        let stopped =
            AgentStatus::after(Some(&working), report(AgentEventKind::Stopped), now).unwrap();
        assert_eq!(stopped.activity, AgentActivity::Idle);
        assert_eq!(stopped.badge(), None);
        let done = AgentStatus::after(
            None,
            AgentReport {
                last_message: Some("answer".into()),
                ..report(AgentEventKind::Finished)
            },
            now,
        )
        .unwrap();
        let later = AgentStatus::after(Some(&done), report(AgentEventKind::Stopped), now).unwrap();
        assert_eq!(later.activity, AgentActivity::Finished);
        assert!(later.needs_attention(), "an unseen result stays news");
        assert_eq!(later.last_message.as_deref(), Some("answer"));
    }

    #[test]
    fn seen_lasts_until_the_state_changes() {
        let now = Instant::now();
        let mut done = AgentStatus::after(None, report(AgentEventKind::Finished), now).unwrap();
        done.seen = true;
        assert!(!done.needs_attention());
        let repeated = AgentStatus::after(Some(&done), report(AgentEventKind::Finished), now);
        assert!(repeated.unwrap().seen, "a repeated report is not news");
        let failed = AgentStatus::after(Some(&done), report(AgentEventKind::Failed), now).unwrap();
        assert!(failed.needs_attention());
    }

    #[test]
    fn another_agent_or_a_new_conversation_starts_fresh() {
        let now = Instant::now();
        let done = AgentStatus::after(
            None,
            AgentReport {
                last_message: Some("old answer".into()),
                ..report(AgentEventKind::Finished)
            },
            now,
        )
        .unwrap();
        let restarted =
            AgentStatus::after(Some(&done), report(AgentEventKind::SessionStarted), now).unwrap();
        assert_eq!(restarted.last_message, None);
        let other = AgentStatus::after(
            Some(&done),
            AgentReport {
                agent_id: "claude-code".into(),
                ..report(AgentEventKind::Working)
            },
            now,
        )
        .unwrap();
        assert_eq!(other.agent_id, "claude-code");
        assert_eq!(other.last_message, None);
    }
}
