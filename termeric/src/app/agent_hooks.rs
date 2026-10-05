//! Setting up and removing the hooks through which agents report their
//! state. Every change is shown before it is made, and a file edited in the
//! meantime is left alone.

use super::*;
use crate::agents::setup::{self, HookState, HookedAgent};
use crate::ui::info_overlay::InfoOverlayChoice;
use std::path::Path;

const APPLY_CHOICE: &str = "agent-hooks-apply";
const CANCEL_CHOICE: &str = "agent-hooks-cancel";

/// One settings file to rewrite.
#[derive(Debug)]
pub(super) struct HookFileChange {
    name: String,
    agent_id: &'static str,
    path: PathBuf,
    previous: String,
    next: String,
}

/// A confirmation dialog awaiting the person's answer.
#[derive(Debug)]
pub(super) struct PendingHookChange {
    request_id: u64,
    install: bool,
    changes: Vec<HookFileChange>,
}

/// What the Settings page shows about agent status hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HookSetupSummary {
    /// No installed agent supports status hooks.
    Unavailable,
    NotSetUp,
    SetUp,
    /// Set up for some agents, or for another Termeric executable.
    Incomplete,
}

impl HookSetupSummary {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Unavailable => "No supported agent installed",
            Self::NotSetUp => "Not set up",
            Self::SetUp => "Set up",
            Self::Incomplete => "Needs updating",
        }
    }
}

/// The environment an agent is started with when no profile is named,
/// which decides where it reads its settings.
fn launch_env(
    agent: &crate::agents::AgentDefinition,
) -> std::collections::BTreeMap<String, String> {
    agent
        .launch(None, false)
        .map_or_else(|_| agent.env.clone(), |launch| launch.env)
}

fn termeric_executable() -> Result<PathBuf, String> {
    std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .map_err(|error| format!("Could not locate the Termeric executable: {error}"))
}

impl App {
    /// Agents that could report their state: enabled, installed, and with a
    /// known hook format; each with its name and settings file.
    fn hookable_agents(&self) -> Vec<(HookedAgent, String, Option<PathBuf>)> {
        setup::HOOKED_AGENTS
            .into_iter()
            .filter_map(|hooked| {
                let agent = self.agents.get(hooked.agent_id)?;
                (agent.enabled && agent.installed_path().is_some()).then(|| {
                    (
                        hooked,
                        agent.name.clone(),
                        hooked.settings_path(&launch_env(agent)),
                    )
                })
            })
            .collect()
    }

    /// Each hookable agent's name and how its installed hooks compare with
    /// this Termeric's; `None` when this executable cannot be located.
    fn agent_hook_states(&self) -> Option<Vec<(String, HookState)>> {
        let executable = termeric_executable().ok()?;
        Some(
            self.hookable_agents()
                .into_iter()
                .map(|(hooked, name, path)| {
                    let state = path
                        .and_then(|path| setup::read_settings(&path).ok())
                        .and_then(|text| setup::hook_state(hooked, &text, &executable).ok())
                        .unwrap_or(HookState::NotInstalled);
                    (name, state)
                })
                .collect(),
        )
    }

    pub(super) fn agent_hook_summary(&self) -> HookSetupSummary {
        let Some(states) = self.agent_hook_states() else {
            return HookSetupSummary::Unavailable;
        };
        if states.is_empty() {
            HookSetupSummary::Unavailable
        } else if states
            .iter()
            .all(|(_, state)| *state == HookState::Installed)
        {
            HookSetupSummary::SetUp
        } else if states
            .iter()
            .all(|(_, state)| *state == HookState::NotInstalled)
        {
            HookSetupSummary::NotSetUp
        } else {
            HookSetupSummary::Incomplete
        }
    }

    /// Once per process, point out hooks that were set up but no longer
    /// match this Termeric: after it moved, or after an update changed the
    /// events it listens to. Agents then report nothing, or too little, and
    /// nothing else would say why. Agents never set up are not mentioned.
    pub(super) fn warn_about_outdated_agent_hooks(&mut self) {
        static CHECKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !self.agent_features_enabled() {
            return;
        }
        if CHECKED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let outdated: Vec<String> = self
            .agent_hook_states()
            .unwrap_or_default()
            .into_iter()
            .filter(|(_, state)| *state == HookState::Outdated)
            .map(|(name, _)| name)
            .collect();
        if !outdated.is_empty() {
            self.toasts.warning(format!(
                "Status hooks for {} need updating; run Set Up Agent Status…",
                outdated.join(" and ")
            ));
        }
    }

    /// The rewrites that would install (or remove) Termeric's hooks. Files
    /// already in the wanted state are skipped.
    fn plan_hook_changes(&self, install: bool) -> Result<Vec<HookFileChange>, String> {
        let executable = termeric_executable()?;
        let agents: Vec<(HookedAgent, String, Option<PathBuf>)> = if install {
            self.hookable_agents()
        } else {
            // Removal also cleans up after agents that were since disabled
            // or uninstalled.
            setup::HOOKED_AGENTS
                .into_iter()
                .map(|hooked| match self.agents.get(hooked.agent_id) {
                    Some(agent) => (
                        hooked,
                        agent.name.clone(),
                        hooked.settings_path(&launch_env(agent)),
                    ),
                    None => (
                        hooked,
                        hooked.agent_id.to_string(),
                        hooked.settings_path(&Default::default()),
                    ),
                })
                .collect()
        };
        let mut changes = Vec::new();
        for (hooked, name, path) in agents {
            let Some(path) = path else {
                continue;
            };
            let previous = setup::read_settings(&path).map_err(|error| format!("{error:#}"))?;
            // Already current: leave the file, and its formatting, alone.
            if install
                && setup::hook_state(hooked, &previous, &executable).ok()
                    == Some(HookState::Installed)
            {
                continue;
            }
            let next = setup::rewrite(hooked, &previous, &executable, install)
                .map_err(|error| format!("{} could not be updated: {error:#}", path.display()))?;
            if next != previous && (install || !previous.is_empty()) {
                changes.push(HookFileChange {
                    name,
                    agent_id: hooked.agent_id,
                    path,
                    previous,
                    next,
                });
            }
        }
        Ok(changes)
    }

    /// Show what setting up (or removing) agent status hooks would change,
    /// and wait for the person to confirm.
    pub(super) fn open_agent_hook_setup(&mut self, install: bool) {
        self.open_agent_hook_setup_because(install, None);
    }

    /// Show the setup (or removal) dialog, opening with `reason` when
    /// something else asked for it.
    pub(super) fn open_agent_hook_setup_because(&mut self, install: bool, reason: Option<&str>) {
        let changes = match self.plan_hook_changes(install) {
            Ok(changes) => changes,
            Err(error) => {
                self.toasts.error(error);
                return;
            }
        };
        if changes.is_empty() {
            self.toasts
                .info(match (install, self.hookable_agents().is_empty()) {
                    (true, true) => "Agent status needs Claude Code or Codex to be installed",
                    (true, false) => "Agent status is already set up",
                    (false, _) => "No agent status hooks are installed",
                });
            return;
        }
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let shown = |path: &Path| match home
            .as_deref()
            .and_then(|home| path.strip_prefix(home).ok())
        {
            Some(relative) => format!("~/{}", relative.display()),
            None => path.display().to_string(),
        };
        let mut lines: Vec<String> = reason
            .into_iter()
            .flat_map(|reason| [reason.to_owned(), String::new()])
            .collect();
        lines.extend(if install {
            vec![
                "Termeric adds hooks that report when an agent is working, needs you,".into(),
                "or is done. They do nothing when the agent runs elsewhere.".into(),
                String::new(),
            ]
        } else {
            vec![
                "Termeric removes its hooks. Your other hooks and settings stay.".into(),
                String::new(),
            ]
        });
        for change in &changes {
            lines.push(format!("  {}: {}", change.name, shown(&change.path)));
        }
        lines.push(String::new());
        lines.push(
            "Each file's previous version is kept beside it, ending in .termeric-backup.".into(),
        );
        if install && changes.iter().any(|change| change.agent_id == "codex") {
            lines.push("Codex asks you to trust the new hooks the next time it starts.".into());
        }
        lines.push(String::new());
        let apply_line = lines.len();
        let (title, action) = if install {
            ("Set up agent status", "Set Up")
        } else {
            ("Remove agent status hooks", "Remove")
        };
        lines.push(format!("  {}  {action}", &action[..1]));
        lines.push("  C  Cancel".into());
        let shortcut = action.chars().next().map(|c| c.to_ascii_lowercase());
        let request_id = self.info_overlay.show_choices(
            title,
            lines,
            vec![
                InfoOverlayChoice::new(APPLY_CHOICE, action, apply_line, shortcut),
                InfoOverlayChoice::new(CANCEL_CHOICE, "Cancel", apply_line + 1, Some('c')),
            ],
        );
        self.pending_hook_change = Some(PendingHookChange {
            request_id,
            install,
            changes,
        });
    }

    /// Answer the confirmation dialog. Returns whether the choice was its.
    pub(super) fn handle_agent_hook_choice(&mut self, request_id: u64, choice_id: &str) -> bool {
        if !matches!(choice_id, APPLY_CHOICE | CANCEL_CHOICE)
            || self
                .pending_hook_change
                .as_ref()
                .is_none_or(|pending| pending.request_id != request_id)
        {
            return false;
        }
        self.info_overlay.dismiss(request_id);
        let Some(pending) = self.pending_hook_change.take() else {
            return true;
        };
        if choice_id == APPLY_CHOICE {
            self.apply_hook_changes(pending);
        }
        true
    }

    /// Close the setup dialog without changing any file.
    pub(super) fn close_agent_hook_setup(&mut self) {
        if let Some(pending) = self.pending_hook_change.take() {
            self.info_overlay.dismiss(pending.request_id);
        }
    }

    pub(super) fn cancel_pending_hook_change(&mut self, request_id: u64) {
        if self
            .pending_hook_change
            .as_ref()
            .is_some_and(|pending| pending.request_id == request_id)
        {
            self.pending_hook_change = None;
        }
    }

    fn apply_hook_changes(&mut self, pending: PendingHookChange) {
        let mut done = false;
        for change in pending.changes {
            match setup::replace_if_unchanged(&change.path, &change.previous, &change.next) {
                Ok(true) => done = true,
                Ok(false) => self.toasts.warning(format!(
                    "{} changed while you were deciding; nothing was written to it",
                    change.path.display()
                )),
                Err(error) => self.toasts.error(format!("{error:#}")),
            }
        }
        // Files that could not be written already have their own warning.
        if done {
            self.toasts.success(if pending.install {
                "Agent status is set up"
            } else {
                "Removed agent status hooks"
            });
        }
        self.sync_agent_settings_rows();
    }
}
