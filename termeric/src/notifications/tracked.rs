//! Bounded, origin-scoped Linux notification handles for Kitty OSC 99.

use crate::tabs::TerminalIdentity;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::time::{Duration, Instant};
use termeric_core::{
    NotificationExpiry, NotificationRequest, NotificationSound, NotificationUrgency,
};

const TRACKED_COMMAND_CAPACITY: usize = 64;
const MAX_TRACKED_NOTIFICATIONS: usize = 64;
const MAX_EXPIRY_CLOSE_ATTEMPTS: u8 = 3;
const EXPIRY_RETRY_DELAY: Duration = Duration::from_secs(1);
const CLOSE_SIGNAL_TIMEOUT: Duration = Duration::from_secs(5);
const DBUS_METHOD_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_NATIVE_ACTION_KEY_BYTES: usize = 64;
const MAX_ACTIVATION_TOKEN_BYTES: usize = 4 * 1024;
const DEFAULT_ACTION_KEY: &str = "default";
const BUTTON_ACTION_KEY_PREFIX: &str = "termeric-button-";

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum InternalIdentifier {
    Named(String),
    Anonymous(u64),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct TrackedKey {
    origin: TerminalIdentity,
    identifier: InternalIdentifier,
}

#[derive(Clone)]
struct TrackedEntry {
    desktop_id: u32,
    expires_at: Option<Instant>,
    expiry_close_attempts: u8,
    report_close: bool,
    report_activation: bool,
    focus_on_activation: bool,
    button_action_keys: Vec<String>,
    activation_handled: bool,
    activation_token: Option<String>,
    reply_uses_8bit_controls: bool,
}

#[derive(Clone, Debug)]
struct ClosingEntry {
    origin: TerminalIdentity,
    identifier: String,
    reply_uses_8bit_controls: bool,
    deadline: Instant,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ProtocolReply {
    pub(super) origin: TerminalIdentity,
    pub(super) data: Vec<u8>,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct ActivationCompletion {
    pub(super) origin: TerminalIdentity,
    pub(super) focus: bool,
    pub(super) protocol_reply: Option<Vec<u8>>,
    pub(super) activation_token: Option<String>,
}

impl std::fmt::Debug for ActivationCompletion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ActivationCompletion")
            .field("origin", &self.origin)
            .field("focus", &self.focus)
            .field("protocol_reply", &self.protocol_reply)
            .field("activation_token_present", &self.activation_token.is_some())
            .finish()
    }
}

enum TrackedCommand {
    Show {
        key: TrackedKey,
        request: Box<NotificationRequest>,
    },
    /// A notification nothing will replace, close, query, or report on.
    ShowUntracked {
        request: Box<NotificationRequest>,
    },
    Close {
        key: TrackedKey,
    },
    Alive {
        origin: TerminalIdentity,
        identifier: String,
        reply_uses_8bit_controls: bool,
    },
    NativeClosed {
        desktop_id: u32,
    },
    ActionInvoked {
        desktop_id: u32,
        action_key: String,
    },
    ActivationToken {
        desktop_id: u32,
        token: String,
    },
    BackendFailure(String),
    Shutdown,
}

pub(super) enum TrackedCompletion {
    Failure(String),
    Recovered,
    ProtocolReply(ProtocolReply),
    Activation(ActivationCompletion),
}

trait TrackedNotificationBackend: Send + 'static {
    fn prepare(&mut self, _commands: SyncSender<TrackedCommand>) -> Result<(), String> {
        Ok(())
    }

    fn show(
        &mut self,
        request: &NotificationRequest,
        replaces_id: u32,
        button_action_keys: &[String],
    ) -> Result<u32, String>;

    fn close(&mut self, desktop_id: u32) -> Result<(), String>;

    fn take_warning(&mut self) -> Option<String> {
        None
    }

    fn shutdown(&mut self) {}
}

struct TrackedState<B> {
    backend: B,
    entries: HashMap<TrackedKey, TrackedEntry>,
    native_keys: HashMap<u32, TrackedKey>,
    order: VecDeque<TrackedKey>,
    closing: HashMap<u32, ClosingEntry>,
    closing_order: VecDeque<u32>,
    next_action_generation: u64,
}

impl<B: TrackedNotificationBackend> TrackedState<B> {
    fn new(backend: B) -> Self {
        Self {
            backend,
            entries: HashMap::new(),
            native_keys: HashMap::new(),
            order: VecDeque::new(),
            closing: HashMap::new(),
            closing_order: VecDeque::new(),
            next_action_generation: 1,
        }
    }

    fn show_untracked(&mut self, request: &NotificationRequest) -> Vec<String> {
        let result = self.backend.show(request, 0, &[]);
        let mut warnings = self.backend.take_warning().into_iter().collect::<Vec<_>>();
        if let Err(error) = result {
            warnings.push(format!("could not display notification: {error}"));
        }
        warnings
    }

    fn show(&mut self, key: TrackedKey, request: NotificationRequest) -> Vec<String> {
        let replaces_id = self.entries.get(&key).map_or(0, |entry| entry.desktop_id);
        let button_action_keys = self.allocate_button_action_keys(request.buttons.len());
        let desktop_result = self
            .backend
            .show(&request, replaces_id, &button_action_keys);
        let mut warnings = self.backend.take_warning().into_iter().collect::<Vec<_>>();
        let desktop_id = match desktop_result {
            Ok(desktop_id) if desktop_id != 0 => desktop_id,
            Ok(_) => {
                warnings.push("notification service returned invalid desktop ID 0".into());
                return warnings;
            }
            Err(error) => {
                warnings.push(format!("could not display tracked notification: {error}"));
                return warnings;
            }
        };
        let displayed_at = Instant::now();
        let expires_at = match request.expiry {
            NotificationExpiry::AfterMillis(milliseconds) => {
                displayed_at.checked_add(Duration::from_millis(u64::from(milliseconds)))
            }
            NotificationExpiry::System | NotificationExpiry::Never => None,
        };
        if replaces_id != 0 && replaces_id != desktop_id {
            self.native_keys.remove(&replaces_id);
        }
        self.order.retain(|candidate| candidate != &key);
        self.order.push_back(key.clone());
        if let Some(previous) = self.native_keys.get(&desktop_id).cloned()
            && previous != key
        {
            self.remove(&previous);
        }
        self.native_keys.insert(desktop_id, key.clone());
        self.closing.remove(&desktop_id);
        self.closing_order
            .retain(|candidate| *candidate != desktop_id);
        self.entries.insert(
            key,
            TrackedEntry {
                desktop_id,
                expires_at,
                expiry_close_attempts: 0,
                report_close: request.report_close,
                report_activation: request.report_activation,
                focus_on_activation: request.focus_on_activation,
                button_action_keys,
                activation_handled: false,
                activation_token: None,
                reply_uses_8bit_controls: request.reply_uses_8bit_controls,
            },
        );
        warnings.extend(self.enforce_capacity());
        warnings
    }

    fn allocate_button_action_keys(&mut self, count: usize) -> Vec<String> {
        if count == 0 {
            return Vec::new();
        }
        let generation = self.next_action_generation;
        self.next_action_generation = self.next_action_generation.wrapping_add(1).max(1);
        (1..=count)
            .map(|index| button_action_key(generation, index))
            .collect()
    }

    fn close(&mut self, key: &TrackedKey) -> Vec<String> {
        let Some(desktop_id) = self.entries.get(key).map(|entry| entry.desktop_id) else {
            return Vec::new();
        };
        match self.backend.close(desktop_id) {
            Ok(()) => self.retire_after_terminal_close(key, Instant::now()),
            Err(error) => vec![format!("could not close tracked notification: {error}")],
        }
    }

    fn expire_due(&mut self, now: Instant) -> Vec<String> {
        let due = self
            .order
            .iter()
            .filter(|key| {
                self.entries
                    .get(*key)
                    .and_then(|entry| entry.expires_at)
                    .is_some_and(|deadline| deadline <= now)
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut failures = Vec::new();
        for key in due {
            let Some(desktop_id) = self.entries.get(&key).map(|entry| entry.desktop_id) else {
                continue;
            };
            match self.backend.close(desktop_id) {
                Ok(()) => failures.extend(self.retire_after_terminal_close(&key, now)),
                Err(error) => {
                    let Some(entry) = self.entries.get_mut(&key) else {
                        continue;
                    };
                    entry.expiry_close_attempts = entry.expiry_close_attempts.saturating_add(1);
                    failures.push(format!("could not expire tracked notification: {error}"));
                    if entry.expiry_close_attempts >= MAX_EXPIRY_CLOSE_ATTEMPTS {
                        self.remove(&key);
                    } else {
                        entry.expires_at = now.checked_add(EXPIRY_RETRY_DELAY);
                    }
                }
            }
        }
        failures
    }

    fn reap_unconfirmed_closes(&mut self, now: Instant) -> (Vec<String>, Vec<ProtocolReply>) {
        let due = self
            .closing_order
            .iter()
            .filter(|desktop_id| {
                self.closing
                    .get(desktop_id)
                    .is_some_and(|entry| entry.deadline <= now)
            })
            .copied()
            .collect::<Vec<_>>();
        let mut failures = Vec::new();
        let mut replies = Vec::new();
        for desktop_id in due {
            let Some(entry) = self.remove_closing(desktop_id) else {
                continue;
            };
            failures.push(format!(
                "notification service did not confirm close for desktop ID {desktop_id}"
            ));
            replies.push(close_reply(&entry, true));
        }
        (failures, replies)
    }

    fn native_closed(&mut self, desktop_id: u32) -> Option<ProtocolReply> {
        if let Some(entry) = self.remove_closing(desktop_id) {
            return Some(close_reply(&entry, false));
        }
        let key = self.native_keys.get(&desktop_id)?.clone();
        let entry = self.remove(&key)?;
        entry
            .report_close
            .then(|| close_reply_for(&key, entry.reply_uses_8bit_controls, false))
    }

    fn set_activation_token(&mut self, desktop_id: u32, token: String) {
        if token.is_empty()
            || token.len() > MAX_ACTIVATION_TOKEN_BYTES
            || token.chars().any(char::is_control)
        {
            return;
        }
        let Some(key) = self.native_keys.get(&desktop_id) else {
            return;
        };
        let Some(entry) = self.entries.get_mut(key) else {
            return;
        };
        if entry.focus_on_activation && entry.activation_token.is_none() {
            entry.activation_token = Some(token);
        }
    }

    fn action_invoked(
        &mut self,
        desktop_id: u32,
        action_key: &str,
    ) -> Option<ActivationCompletion> {
        let key = self.native_keys.get(&desktop_id)?.clone();
        let entry = self.entries.get_mut(&key)?;
        if entry.activation_handled {
            return None;
        }
        let button = entry
            .button_action_keys
            .iter()
            .position(|candidate| candidate == action_key)
            .map(|index| index + 1);
        if action_key != DEFAULT_ACTION_KEY && button.is_none() {
            return None;
        }
        if !entry.report_activation && !entry.focus_on_activation {
            return None;
        }
        entry.activation_handled = true;
        let protocol_reply = entry
            .report_activation
            .then(|| activation_reply_for(&key, entry.reply_uses_8bit_controls, button).data);
        Some(ActivationCompletion {
            origin: key.origin,
            focus: entry.focus_on_activation,
            protocol_reply,
            activation_token: entry.activation_token.take(),
        })
    }

    fn alive_reply(
        &self,
        origin: TerminalIdentity,
        identifier: &str,
        reply_uses_8bit_controls: bool,
    ) -> ProtocolReply {
        let mut identifiers = self
            .entries
            .keys()
            .filter_map(|key| match (&key.origin, &key.identifier) {
                (candidate, InternalIdentifier::Named(identifier)) if *candidate == origin => {
                    Some(identifier.as_str())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        identifiers.sort_unstable();
        let canonical = format!(
            "\x1b]99;i={identifier}:p=alive;{}\x1b\\",
            identifiers.join(",")
        );
        ProtocolReply {
            origin,
            data: termeric_core::Terminal::frame_pty_reply_for_mode(
                canonical.as_bytes(),
                reply_uses_8bit_controls,
            ),
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        self.entries
            .values()
            .filter_map(|entry| entry.expires_at)
            .chain(self.closing.values().map(|entry| entry.deadline))
            .min()
    }

    fn enforce_capacity(&mut self) -> Vec<String> {
        let mut failures = Vec::new();
        while self.entries.len() > MAX_TRACKED_NOTIFICATIONS {
            let Some(key) = self.order.pop_front() else {
                break;
            };
            let Some(desktop_id) = self.entries.get(&key).map(|entry| entry.desktop_id) else {
                continue;
            };
            match self.backend.close(desktop_id) {
                Ok(()) => failures.extend(self.retire_after_terminal_close(&key, Instant::now())),
                Err(error) => {
                    self.remove(&key);
                    failures.push(format!(
                        "could not close evicted tracked notification: {error}"
                    ));
                }
            }
        }
        failures
    }

    fn retire_after_terminal_close(&mut self, key: &TrackedKey, now: Instant) -> Vec<String> {
        let Some(entry) = self.remove(key) else {
            return Vec::new();
        };
        if !entry.report_close {
            return Vec::new();
        }
        let closing = ClosingEntry {
            origin: key.origin,
            identifier: client_identifier(key),
            reply_uses_8bit_controls: entry.reply_uses_8bit_controls,
            deadline: now.checked_add(CLOSE_SIGNAL_TIMEOUT).unwrap_or(now),
        };
        self.closing_order
            .retain(|candidate| *candidate != entry.desktop_id);
        self.closing_order.push_back(entry.desktop_id);
        self.closing.insert(entry.desktop_id, closing);
        self.enforce_closing_capacity()
    }

    fn enforce_closing_capacity(&mut self) -> Vec<String> {
        let mut failures = Vec::new();
        while self.closing.len() > MAX_TRACKED_NOTIFICATIONS {
            let Some(desktop_id) = self.closing_order.pop_front() else {
                break;
            };
            if self.closing.remove(&desktop_id).is_some() {
                failures.push("notification close-report tracking capacity exceeded".into());
            }
        }
        failures
    }

    fn remove(&mut self, key: &TrackedKey) -> Option<TrackedEntry> {
        let entry = self.entries.remove(key)?;
        self.native_keys.remove(&entry.desktop_id);
        self.order.retain(|candidate| candidate != key);
        Some(entry)
    }

    fn remove_closing(&mut self, desktop_id: u32) -> Option<ClosingEntry> {
        let entry = self.closing.remove(&desktop_id)?;
        self.closing_order
            .retain(|candidate| *candidate != desktop_id);
        Some(entry)
    }
}

fn client_identifier(key: &TrackedKey) -> String {
    match &key.identifier {
        InternalIdentifier::Named(identifier) => identifier.clone(),
        InternalIdentifier::Anonymous(_) => "0".into(),
    }
}

fn close_reply(entry: &ClosingEntry, untracked: bool) -> ProtocolReply {
    let canonical = if untracked {
        format!("\x1b]99;i={}:p=close;untracked\x1b\\", entry.identifier)
    } else {
        format!("\x1b]99;i={}:p=close;\x1b\\", entry.identifier)
    };
    ProtocolReply {
        origin: entry.origin,
        data: termeric_core::Terminal::frame_pty_reply_for_mode(
            canonical.as_bytes(),
            entry.reply_uses_8bit_controls,
        ),
    }
}

fn close_reply_for(
    key: &TrackedKey,
    reply_uses_8bit_controls: bool,
    untracked: bool,
) -> ProtocolReply {
    close_reply(
        &ClosingEntry {
            origin: key.origin,
            identifier: client_identifier(key),
            reply_uses_8bit_controls,
            deadline: Instant::now(),
        },
        untracked,
    )
}

fn activation_reply_for(
    key: &TrackedKey,
    reply_uses_8bit_controls: bool,
    button: Option<usize>,
) -> ProtocolReply {
    let payload = button.map_or_else(String::new, |button| button.to_string());
    let canonical = format!("\x1b]99;i={};{payload}\x1b\\", client_identifier(key));
    ProtocolReply {
        origin: key.origin,
        data: termeric_core::Terminal::frame_pty_reply_for_mode(
            canonical.as_bytes(),
            reply_uses_8bit_controls,
        ),
    }
}

fn button_action_key(generation: u64, index: usize) -> String {
    format!("{BUTTON_ACTION_KEY_PREFIX}{generation}-{index}")
}

/// The `Notify` call's text, category, urgency, and expiry for a request;
/// icons and actions are resolved separately.
#[derive(Debug, PartialEq, Eq)]
struct FreedesktopNotification<'a> {
    app_name: &'a str,
    summary: String,
    body: String,
    category: Option<String>,
    urgency: Option<u8>,
    expire_timeout: i32,
}

fn freedesktop_notification(request: &NotificationRequest) -> FreedesktopNotification<'_> {
    FreedesktopNotification {
        app_name: request.application_name.as_deref().unwrap_or("Termeric"),
        summary: escape_markup(request.title.as_deref().unwrap_or("Terminal")),
        body: escape_markup(&request.body),
        category: (!request.notification_types.is_empty())
            .then(|| request.notification_types.join(",")),
        urgency: request.urgency.map(|urgency| match urgency {
            NotificationUrgency::Low => 0,
            NotificationUrgency::Normal => 1,
            NotificationUrgency::Critical => 2,
        }),
        expire_timeout: match request.expiry {
            NotificationExpiry::System => -1,
            NotificationExpiry::Never => 0,
            NotificationExpiry::AfterMillis(milliseconds) => milliseconds as i32,
        },
    }
}

fn freedesktop_actions(
    request: &NotificationRequest,
    button_action_keys: &[String],
) -> Vec<String> {
    debug_assert_eq!(request.buttons.len(), button_action_keys.len());
    let mut actions = Vec::with_capacity(request.buttons.len().saturating_mul(2).saturating_add(2));
    if request.report_activation || request.focus_on_activation {
        actions.push(DEFAULT_ACTION_KEY.into());
        actions.push("Activate".into());
    }
    for (action_key, label) in button_action_keys.iter().zip(&request.buttons) {
        actions.push(action_key.clone());
        actions.push(label.clone());
    }
    actions
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FreedesktopSoundHint {
    Suppress,
    Name(&'static str),
}

fn freedesktop_sound_hint(sound: NotificationSound) -> Option<FreedesktopSoundHint> {
    match sound {
        NotificationSound::System => None,
        NotificationSound::Silent => Some(FreedesktopSoundHint::Suppress),
        NotificationSound::Error => Some(FreedesktopSoundHint::Name("dialog-error")),
        NotificationSound::Warning => Some(FreedesktopSoundHint::Name("dialog-warning")),
        NotificationSound::Information => Some(FreedesktopSoundHint::Name("dialog-information")),
        NotificationSound::Question => Some(FreedesktopSoundHint::Name("dialog-question")),
    }
}

fn freedesktop_image_data_value(
    icon: &super::icon::DecodedNotificationIcon,
) -> zbus::zvariant::Value<'_> {
    zbus::zvariant::Value::from((
        icon.width,
        icon.height,
        icon.rowstride,
        true,
        8_i32,
        4_i32,
        icon.data.as_slice(),
    ))
}

fn freedesktop_app_icon(
    request: &NotificationRequest,
    named_icon: Option<&str>,
    has_transmitted_icon: bool,
) -> String {
    if let Some(named_icon) = named_icon {
        return named_icon.to_owned();
    }
    if has_transmitted_icon {
        return String::new();
    }
    request
        .icon_names
        .iter()
        .find(|name| super::icon::is_safe_icon_name(name))
        .map(|name| super::freedesktop_icon_name(name))
        .or(request
            .application_name
            .as_deref()
            .filter(|name| super::icon::is_safe_icon_name(name)))
        .unwrap_or("")
        .to_owned()
}

pub(super) struct TrackedNotifications {
    commands: SyncSender<TrackedCommand>,
    completions: Receiver<TrackedCompletion>,
    next_anonymous: u64,
    shutdown: Arc<AtomicBool>,
}

impl TrackedNotifications {
    /// Use the real frontend/queue with no worker or native notification backend.
    #[cfg(test)]
    pub(super) fn capture_commands_for_test()
    -> (Self, impl Fn() -> Vec<super::CapturedNotificationCommand>) {
        let (commands, receiver) = mpsc::sync_channel(TRACKED_COMMAND_CAPACITY);
        let (_completion_sender, completions) = mpsc::sync_channel(TRACKED_COMMAND_CAPACITY);
        let tracked = Self {
            commands,
            completions,
            next_anonymous: 1,
            shutdown: Arc::new(AtomicBool::new(false)),
        };
        let capture = move || {
            receiver
                .try_iter()
                .filter_map(|command| {
                    Some(match command {
                        TrackedCommand::ShowUntracked { .. } => return None,
                        TrackedCommand::Show { ref key, .. }
                        | TrackedCommand::Close { ref key } => {
                            let kind = if matches!(command, TrackedCommand::Show { .. }) {
                                "show"
                            } else {
                                "close"
                            };
                            let InternalIdentifier::Named(identifier) = &key.identifier else {
                                panic!("expected named fixture request")
                            };
                            (kind, key.origin, identifier.clone())
                        }
                        TrackedCommand::Alive {
                            origin, identifier, ..
                        } => ("alive", origin, identifier),
                        _ => panic!("unexpected fixture command"),
                    })
                })
                .collect()
        };
        (tracked, capture)
    }

    pub(super) fn new() -> Result<Self, String> {
        Self::new_with_backend(FreedesktopBackend::default())
    }

    /// A real worker whose notification service refuses every notification.
    #[cfg(test)]
    pub(super) fn failing_for_test(error: &str) -> Result<Self, String> {
        Self::new_with_backend(FailingBackend(error.to_owned()))
    }

    fn new_with_backend<B: TrackedNotificationBackend>(backend: B) -> Result<Self, String> {
        let (commands, receiver) = mpsc::sync_channel(TRACKED_COMMAND_CAPACITY);
        let (completion_sender, completions) = mpsc::sync_channel(TRACKED_COMMAND_CAPACITY);
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_shutdown = Arc::clone(&shutdown);
        let worker_commands = commands.clone();
        std::thread::Builder::new()
            .name("termeric-notification-lifecycle".into())
            .spawn(move || {
                tracked_worker(
                    receiver,
                    worker_commands,
                    completion_sender,
                    worker_shutdown,
                    TrackedState::new(backend),
                );
            })
            .map_err(|error| format!("could not start notification lifecycle worker: {error}"))?;
        Ok(Self {
            commands,
            completions,
            next_anonymous: 1,
            shutdown,
        })
    }

    pub(super) fn show(
        &mut self,
        origin: TerminalIdentity,
        request: NotificationRequest,
    ) -> Result<(), String> {
        let identifier = request.identifier.as_ref().map_or_else(
            || {
                let value = self.next_anonymous;
                self.next_anonymous = self.next_anonymous.wrapping_add(1).max(1);
                InternalIdentifier::Anonymous(value)
            },
            |identifier| InternalIdentifier::Named(identifier.clone()),
        );
        self.send(TrackedCommand::Show {
            key: TrackedKey { origin, identifier },
            request: Box::new(request),
        })
    }

    pub(super) fn show_untracked(&self, request: NotificationRequest) -> Result<(), String> {
        self.send(TrackedCommand::ShowUntracked {
            request: Box::new(request),
        })
    }

    pub(super) fn close(&self, origin: TerminalIdentity, identifier: String) -> Result<(), String> {
        self.send(TrackedCommand::Close {
            key: TrackedKey {
                origin,
                identifier: InternalIdentifier::Named(identifier),
            },
        })
    }

    pub(super) fn alive(
        &self,
        origin: TerminalIdentity,
        identifier: String,
        reply_uses_8bit_controls: bool,
    ) -> Result<(), String> {
        self.send(TrackedCommand::Alive {
            origin,
            identifier,
            reply_uses_8bit_controls,
        })
    }

    pub(super) fn drain_completions(&self) -> Vec<TrackedCompletion> {
        self.completions.try_iter().collect()
    }

    fn send(&self, command: TrackedCommand) -> Result<(), String> {
        match self.commands.try_send(command) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err("notification lifecycle queue is busy".into()),
            Err(TrySendError::Disconnected(_)) => {
                Err("notification lifecycle worker stopped".into())
            }
        }
    }
}

impl Drop for TrackedNotifications {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        let _ = self.commands.try_send(TrackedCommand::Shutdown);
    }
}

fn tracked_worker<B: TrackedNotificationBackend>(
    commands: Receiver<TrackedCommand>,
    command_sender: SyncSender<TrackedCommand>,
    completions: SyncSender<TrackedCompletion>,
    shutdown: Arc<AtomicBool>,
    mut state: TrackedState<B>,
) {
    let mut last_failure = None;
    loop {
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        let now = Instant::now();
        if state
            .next_deadline()
            .is_some_and(|deadline| deadline <= now)
        {
            publish_operation(&completions, &mut last_failure, state.expire_due(now));
            let (failures, replies) = state.reap_unconfirmed_closes(now);
            publish_operation(&completions, &mut last_failure, failures);
            for reply in replies {
                publish_reply(&completions, reply);
            }
        }
        let wait = state
            .next_deadline()
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
            .unwrap_or(Duration::from_secs(60));
        match commands.recv_timeout(wait) {
            Ok(TrackedCommand::Show { key, request }) => {
                let failures = match state.backend.prepare(command_sender.clone()) {
                    Ok(()) => state.show(key, *request),
                    Err(error) => vec![format!(
                        "could not prepare notification close tracking: {error}"
                    )],
                };
                publish_operation(&completions, &mut last_failure, failures);
            }
            Ok(TrackedCommand::ShowUntracked { request }) => {
                let failures = state.show_untracked(&request);
                publish_operation(&completions, &mut last_failure, failures);
            }
            Ok(TrackedCommand::Close { key }) => {
                let failures = state.close(&key);
                publish_operation(&completions, &mut last_failure, failures);
            }
            Ok(TrackedCommand::Alive {
                origin,
                identifier,
                reply_uses_8bit_controls,
            }) => publish_reply(
                &completions,
                state.alive_reply(origin, &identifier, reply_uses_8bit_controls),
            ),
            Ok(TrackedCommand::NativeClosed { desktop_id }) => {
                if let Some(reply) = state.native_closed(desktop_id) {
                    publish_reply(&completions, reply);
                }
                publish_operation(&completions, &mut last_failure, Vec::new());
            }
            Ok(TrackedCommand::ActionInvoked {
                desktop_id,
                action_key,
            }) => {
                if let Some(activation) = state.action_invoked(desktop_id, &action_key) {
                    publish_activation(&completions, activation);
                }
                publish_operation(&completions, &mut last_failure, Vec::new());
            }
            Ok(TrackedCommand::ActivationToken { desktop_id, token }) => {
                state.set_activation_token(desktop_id, token);
            }
            Ok(TrackedCommand::BackendFailure(error)) => {
                publish_operation(&completions, &mut last_failure, vec![error]);
            }
            Ok(TrackedCommand::Shutdown) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    drop(commands);
    state.backend.shutdown();
}

fn publish_reply(completions: &SyncSender<TrackedCompletion>, reply: ProtocolReply) {
    if completions
        .send(TrackedCompletion::ProtocolReply(reply))
        .is_ok()
    {
        wake_app();
    }
}

fn publish_activation(
    completions: &SyncSender<TrackedCompletion>,
    activation: ActivationCompletion,
) {
    if completions
        .send(TrackedCompletion::Activation(activation))
        .is_ok()
    {
        wake_app();
    }
}

fn publish_operation(
    completions: &SyncSender<TrackedCompletion>,
    last_failure: &mut Option<String>,
    failures: Vec<String>,
) {
    if failures.is_empty() {
        if last_failure.is_some() && completions.try_send(TrackedCompletion::Recovered).is_ok() {
            *last_failure = None;
            wake_app();
        }
        return;
    }
    for error in failures {
        if last_failure.as_deref() == Some(error.as_str()) {
            continue;
        }
        if completions
            .try_send(TrackedCompletion::Failure(error.clone()))
            .is_ok()
        {
            *last_failure = Some(error);
            wake_app();
        }
    }
}

fn wake_app() {
    if let Some(wake) = crate::app::pty_event_waker() {
        wake();
    }
}

#[cfg(test)]
struct FailingBackend(String);

#[cfg(test)]
impl TrackedNotificationBackend for FailingBackend {
    fn show(&mut self, _: &NotificationRequest, _: u32, _: &[String]) -> Result<u32, String> {
        Err(self.0.clone())
    }

    fn close(&mut self, _: u32) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Default)]
struct FreedesktopBackend {
    connection: Option<zbus::blocking::Connection>,
    signal_stop: Option<async_channel::Sender<()>>,
    signal_thread: Option<std::thread::JoinHandle<()>>,
    signal_alive: Option<Arc<AtomicBool>>,
    last_warning: Option<String>,
}

impl FreedesktopBackend {
    fn connection(&mut self) -> Result<&zbus::blocking::Connection, String> {
        if self.connection.is_none() {
            let connection = zbus::blocking::connection::Builder::session()
                .map_err(|error| format!("session bus address is unavailable: {error}"))?
                .method_timeout(DBUS_METHOD_TIMEOUT)
                .build()
                .map_err(|error| format!("session bus connection failed: {error}"))?;
            self.connection = Some(connection);
        }
        Ok(self
            .connection
            .as_ref()
            .expect("connection was initialized"))
    }

    fn stop(&mut self) {
        if let Some(alive) = self.signal_alive.take() {
            alive.store(false, Ordering::Release);
        }
        if let Some(stop) = self.signal_stop.take() {
            let _ = stop.try_send(());
        }
        if let Some(connection) = self.connection.take() {
            let _ = connection.close();
        }
        if let Some(thread) = self.signal_thread.take()
            && thread.join().is_err()
        {
            log::warn!("Notification close-signal listener panicked during shutdown");
        }
    }
}

impl TrackedNotificationBackend for FreedesktopBackend {
    fn prepare(&mut self, commands: SyncSender<TrackedCommand>) -> Result<(), String> {
        if self.signal_thread.is_some() {
            if self
                .signal_alive
                .as_ref()
                .is_some_and(|alive| alive.load(Ordering::Acquire))
            {
                return Ok(());
            }
            self.stop();
        }
        let connection = self.connection()?.clone();
        let async_connection = connection.inner().clone();
        let (stop_sender, stop_receiver) = async_channel::bounded(1);
        let (ready_sender, ready_receiver) = mpsc::sync_channel(0);
        let signal_alive = Arc::new(AtomicBool::new(true));
        let listener_alive = Arc::clone(&signal_alive);
        let thread = std::thread::Builder::new()
            .name("termeric-notification-signals".into())
            .spawn(move || {
                pollster::block_on(notification_signal_listener(
                    async_connection,
                    commands,
                    stop_receiver,
                    ready_sender,
                ));
                listener_alive.store(false, Ordering::Release);
            })
            .map_err(|error| format!("could not start notification signal listener: {error}"))?;
        self.signal_stop = Some(stop_sender);
        self.signal_thread = Some(thread);
        self.signal_alive = Some(signal_alive);
        match ready_receiver.recv_timeout(DBUS_METHOD_TIMEOUT) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => {
                self.stop();
                Err(error)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.stop();
                Err("notification signal subscription timed out".into())
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                self.stop();
                Err("notification signal listener stopped during startup".into())
            }
        }
    }

    fn show(
        &mut self,
        request: &NotificationRequest,
        replaces_id: u32,
        button_action_keys: &[String],
    ) -> Result<u32, String> {
        let (transmitted_icon, warning) = super::icon::resolve_transmitted_icon(
            request.icon_cache_id.as_deref(),
            request.icon_data.as_deref(),
        );
        self.last_warning = warning;
        let named_icon = super::icon::first_available_icon_name(&request.icon_names);
        let connection = self.connection()?;
        (|| {
            use std::collections::HashMap;
            use zbus::blocking::Proxy;
            use zbus::zvariant::Value;

            let proxy = Proxy::new(
                connection,
                "org.freedesktop.Notifications",
                "/org/freedesktop/Notifications",
                "org.freedesktop.Notifications",
            )
            .map_err(|error| format!("notification interface is unavailable: {error}"))?;
            let notification = freedesktop_notification(request);
            let app_icon =
                freedesktop_app_icon(request, named_icon.as_deref(), transmitted_icon.is_some());
            let actions = freedesktop_actions(request, button_action_keys);
            let mut hints: HashMap<&str, Value<'_>> = HashMap::new();
            if let Some(category) = notification.category.as_deref() {
                hints.insert("category", Value::from(category));
            }
            if let Some(urgency) = notification.urgency {
                hints.insert("urgency", Value::from(urgency));
            }
            match freedesktop_sound_hint(request.sound) {
                None => {}
                Some(FreedesktopSoundHint::Suppress) => {
                    hints.insert("suppress-sound", Value::from(true));
                }
                Some(FreedesktopSoundHint::Name(name)) => {
                    hints.insert("sound-name", Value::from(name));
                }
            }
            if named_icon.is_none()
                && let Some(icon) = transmitted_icon.as_ref()
            {
                hints.insert("image-data", freedesktop_image_data_value(icon));
            }
            proxy
                .call(
                    "Notify",
                    &(
                        notification.app_name,
                        replaces_id,
                        app_icon.as_str(),
                        notification.summary.as_str(),
                        notification.body.as_str(),
                        actions,
                        hints,
                        notification.expire_timeout,
                    ),
                )
                .map_err(|error| format!("Notify call failed: {error}"))
        })()
    }

    fn close(&mut self, desktop_id: u32) -> Result<(), String> {
        let connection = self.connection()?;
        (|| {
            let proxy = zbus::blocking::Proxy::new(
                connection,
                "org.freedesktop.Notifications",
                "/org/freedesktop/Notifications",
                "org.freedesktop.Notifications",
            )
            .map_err(|error| format!("notification interface is unavailable: {error}"))?;
            proxy
                .call("CloseNotification", &(desktop_id,))
                .map_err(|error| format!("CloseNotification call failed: {error}"))
        })()
    }

    fn take_warning(&mut self) -> Option<String> {
        self.last_warning.take()
    }

    fn shutdown(&mut self) {
        self.stop();
    }
}

async fn notification_signal_listener(
    connection: zbus::Connection,
    commands: SyncSender<TrackedCommand>,
    stop: async_channel::Receiver<()>,
    ready: SyncSender<Result<(), String>>,
) {
    use futures_lite::{StreamExt, future};

    let proxy = match zbus::Proxy::new(
        &connection,
        "org.freedesktop.Notifications",
        "/org/freedesktop/Notifications",
        "org.freedesktop.Notifications",
    )
    .await
    {
        Ok(proxy) => proxy,
        Err(error) => {
            let _ = ready.send(Err(format!(
                "notification interface is unavailable: {error}"
            )));
            return;
        }
    };
    // A single stream preserves service ordering when activation and closure
    // are emitted back-to-back for one user action.
    let mut signals = match proxy.receive_all_signals().await {
        Ok(signals) => signals,
        Err(error) => {
            let _ = ready.send(Err(format!(
                "notification lifecycle signals are unavailable: {error}"
            )));
            return;
        }
    };
    if ready.send(Ok(())).is_err() {
        return;
    }

    enum ListenerEvent {
        Stop,
        Signal(Option<zbus::Message>),
    }

    loop {
        let event = future::race(
            async {
                let _ = stop.recv().await;
                ListenerEvent::Stop
            },
            async { ListenerEvent::Signal(signals.next().await) },
        )
        .await;
        match event {
            ListenerEvent::Stop => break,
            ListenerEvent::Signal(Some(message)) => {
                match message.header().member().map(|member| member.as_str()) {
                    Some("NotificationClosed") => {
                        let (desktop_id, _reason): (u32, u32) = match message.body().deserialize() {
                            Ok(body) => body,
                            Err(error) => {
                                if commands
                                    .send(TrackedCommand::BackendFailure(format!(
                                        "invalid notification close signal: {error}"
                                    )))
                                    .is_err()
                                {
                                    break;
                                }
                                continue;
                            }
                        };
                        if commands
                            .send(TrackedCommand::NativeClosed { desktop_id })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Some("ActionInvoked") => {
                        let (desktop_id, action_key): (u32, String) =
                            match message.body().deserialize() {
                                Ok(body) => body,
                                Err(error) => {
                                    if commands
                                        .send(TrackedCommand::BackendFailure(format!(
                                            "invalid notification action signal: {error}"
                                        )))
                                        .is_err()
                                    {
                                        break;
                                    }
                                    continue;
                                }
                            };
                        if action_key.len() > MAX_NATIVE_ACTION_KEY_BYTES {
                            continue;
                        }
                        if commands
                            .send(TrackedCommand::ActionInvoked {
                                desktop_id,
                                action_key,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Some("ActivationToken") => {
                        let (desktop_id, token): (u32, String) = match message.body().deserialize()
                        {
                            Ok(body) => body,
                            Err(_) => continue,
                        };
                        // Treat daemon-provided tokens as opaque, short-lived
                        // authority. Never log or forward their contents.
                        if commands
                            .send(TrackedCommand::ActivationToken { desktop_id, token })
                            .is_err()
                        {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            ListenerEvent::Signal(None) => {
                let _ = commands.send(TrackedCommand::BackendFailure(
                    "notification lifecycle signal stream stopped".into(),
                ));
                break;
            }
        }
    }
}

fn escape_markup(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeBackend {
        next_id: u32,
        shows: Vec<(Option<String>, u32, NotificationExpiry)>,
        shown_action_keys: Vec<Vec<String>>,
        shown_sounds: Vec<NotificationSound>,
        closes: Vec<u32>,
        show_error: Option<String>,
        show_delay: Duration,
        close_errors_remaining: usize,
        shutdown_notice: Option<SyncSender<()>>,
    }

    impl TrackedNotificationBackend for FakeBackend {
        fn show(
            &mut self,
            request: &NotificationRequest,
            replaces_id: u32,
            button_action_keys: &[String],
        ) -> Result<u32, String> {
            if let Some(error) = &self.show_error {
                return Err(error.clone());
            }
            std::thread::sleep(self.show_delay);
            self.shows
                .push((request.identifier.clone(), replaces_id, request.expiry));
            self.shown_action_keys.push(button_action_keys.to_vec());
            self.shown_sounds.push(request.sound);
            if replaces_id != 0 {
                return Ok(replaces_id);
            }
            self.next_id += 1;
            Ok(self.next_id)
        }

        fn close(&mut self, desktop_id: u32) -> Result<(), String> {
            self.closes.push(desktop_id);
            if self.close_errors_remaining > 0 {
                self.close_errors_remaining -= 1;
                return Err("synthetic close failure".into());
            }
            Ok(())
        }

        fn shutdown(&mut self) {
            if let Some(notice) = self.shutdown_notice.take() {
                let _ = notice.send(());
            }
        }
    }

    fn origin(tab_id: usize, pane_id: u32) -> TerminalIdentity {
        TerminalIdentity {
            tab_id,
            pane_id: crate::panes::PaneId(pane_id),
        }
    }

    fn request(identifier: &str, expiry: NotificationExpiry) -> NotificationRequest {
        NotificationRequest {
            title: Some("title".into()),
            identifier: Some(identifier.into()),
            expiry,
            ..NotificationRequest::default()
        }
    }

    fn reporting_request(identifier: Option<&str>, use_8bit_controls: bool) -> NotificationRequest {
        NotificationRequest {
            title: Some("title".into()),
            identifier: identifier.map(str::to_owned),
            report_close: true,
            reply_uses_8bit_controls: use_8bit_controls,
            ..NotificationRequest::default()
        }
    }

    fn action_request(identifier: Option<&str>, use_8bit_controls: bool) -> NotificationRequest {
        NotificationRequest {
            title: Some("title".into()),
            identifier: identifier.map(str::to_owned),
            report_activation: true,
            buttons: vec!["Open".into(), "Ignore".into()],
            reply_uses_8bit_controls: use_8bit_controls,
            ..NotificationRequest::default()
        }
    }

    fn key(origin: TerminalIdentity, identifier: &str) -> TrackedKey {
        TrackedKey {
            origin,
            identifier: InternalIdentifier::Named(identifier.into()),
        }
    }

    fn reported_activation(
        origin: TerminalIdentity,
        data: impl Into<Vec<u8>>,
    ) -> ActivationCompletion {
        ActivationCompletion {
            origin,
            focus: false,
            protocol_reply: Some(data.into()),
            activation_token: None,
        }
    }

    #[test]
    fn replacement_and_close_are_exactly_origin_scoped() {
        let mut state = TrackedState::new(FakeBackend::default());
        assert!(
            state
                .show(
                    key(origin(1, 1), "same"),
                    request("same", Default::default())
                )
                .is_empty()
        );
        assert!(
            state
                .show(
                    key(origin(1, 2), "same"),
                    request("same", Default::default())
                )
                .is_empty()
        );
        assert!(
            state
                .show(
                    key(origin(1, 1), "same"),
                    request("same", Default::default())
                )
                .is_empty()
        );
        assert_eq!(state.backend.shows[0].1, 0);
        assert_eq!(state.backend.shows[1].1, 0);
        assert_eq!(state.backend.shows[2].1, 1);
        assert_eq!(state.entries.len(), 2);

        assert!(state.close(&key(origin(1, 2), "same")).is_empty());
        assert_eq!(state.backend.closes, vec![2]);
        assert!(state.entries.contains_key(&key(origin(1, 1), "same")));
        assert!(!state.entries.contains_key(&key(origin(1, 2), "same")));
        assert!(state.close(&key(origin(9, 9), "same")).is_empty());
        assert_eq!(state.backend.closes, vec![2]);
    }

    #[test]
    fn expiry_retries_are_bounded_and_eventually_release_state() {
        let now = Instant::now();
        let mut state = TrackedState::new(FakeBackend {
            close_errors_remaining: usize::MAX,
            ..FakeBackend::default()
        });
        let expiry = NotificationExpiry::AfterMillis(10);
        assert!(
            state
                .show(key(origin(1, 1), "expire"), request("expire", expiry))
                .is_empty()
        );
        assert_eq!(state.backend.shows[0].2, expiry);
        for attempt in 1..=MAX_EXPIRY_CLOSE_ATTEMPTS {
            let failures = state.expire_due(now + Duration::from_secs(u64::from(attempt)));
            assert_eq!(failures.len(), 1);
        }
        assert!(state.entries.is_empty());
        assert_eq!(
            state.backend.closes.len(),
            usize::from(MAX_EXPIRY_CLOSE_ATTEMPTS)
        );
    }

    #[test]
    fn expiry_clock_starts_after_the_backend_confirms_display() {
        let mut state = TrackedState::new(FakeBackend {
            show_delay: Duration::from_millis(20),
            ..FakeBackend::default()
        });
        assert!(
            state
                .show(
                    key(origin(1, 1), "slow"),
                    request("slow", NotificationExpiry::AfterMillis(5)),
                )
                .is_empty()
        );

        assert!(state.expire_due(Instant::now()).is_empty());
        assert!(state.backend.closes.is_empty());
        assert!(state.entries.contains_key(&key(origin(1, 1), "slow")));
    }

    #[test]
    fn native_close_reports_once_and_alive_is_origin_scoped() {
        let first_origin = origin(1, 1);
        let second_origin = origin(2, 1);
        let mut state = TrackedState::new(FakeBackend::default());
        assert!(
            state
                .show(
                    key(first_origin, "alpha"),
                    reporting_request(Some("alpha"), false),
                )
                .is_empty()
        );
        assert!(
            state
                .show(
                    key(first_origin, "beta"),
                    request("beta", NotificationExpiry::System),
                )
                .is_empty()
        );
        assert!(
            state
                .show(
                    key(second_origin, "alpha"),
                    reporting_request(Some("alpha"), false),
                )
                .is_empty()
        );

        assert_eq!(
            state.alive_reply(first_origin, "probe", false).data,
            b"\x1b]99;i=probe:p=alive;alpha,beta\x1b\\"
        );
        assert_eq!(
            state.native_closed(1),
            Some(ProtocolReply {
                origin: first_origin,
                data: b"\x1b]99;i=alpha:p=close;\x1b\\".to_vec(),
            })
        );
        assert_eq!(state.native_closed(1), None);
        assert_eq!(
            state.alive_reply(first_origin, "probe", false).data,
            b"\x1b]99;i=probe:p=alive;beta\x1b\\"
        );
        assert_eq!(
            state.alive_reply(second_origin, "probe", false).data,
            b"\x1b]99;i=probe:p=alive;alpha\x1b\\"
        );
    }

    #[test]
    fn anonymous_close_report_uses_zero_and_captured_eight_bit_framing() {
        let event_origin = origin(1, 1);
        let mut state = TrackedState::new(FakeBackend::default());
        assert!(
            state
                .show(
                    TrackedKey {
                        origin: event_origin,
                        identifier: InternalIdentifier::Anonymous(1),
                    },
                    reporting_request(None, true),
                )
                .is_empty()
        );

        assert_eq!(
            state.native_closed(1),
            Some(ProtocolReply {
                origin: event_origin,
                data: b"\x9d99;i=0:p=close;\x9c".to_vec(),
            })
        );
        assert_eq!(
            state.alive_reply(event_origin, "probe", true).data,
            b"\x9d99;i=probe:p=alive;\x9c"
        );
    }

    #[test]
    fn activation_and_buttons_report_once_without_consuming_close_state() {
        let event_origin = origin(1, 1);
        let mut state = TrackedState::new(FakeBackend::default());
        let mut request = action_request(Some("job"), false);
        request.report_close = true;
        assert!(state.show(key(event_origin, "job"), request).is_empty());
        state.set_activation_token(1, "unused-without-focus".into());

        assert_eq!(state.action_invoked(1, "unknown"), None);
        assert_eq!(state.action_invoked(999, DEFAULT_ACTION_KEY), None);
        assert_eq!(
            state.action_invoked(1, &button_action_key(1, 2)),
            Some(reported_activation(event_origin, b"\x1b]99;i=job;2\x1b\\"))
        );
        assert_eq!(state.action_invoked(1, DEFAULT_ACTION_KEY), None);
        assert_eq!(
            state.native_closed(1),
            Some(ProtocolReply {
                origin: event_origin,
                data: b"\x1b]99;i=job:p=close;\x1b\\".to_vec(),
            })
        );
    }

    #[test]
    fn every_bounded_button_index_has_an_exact_revision_scoped_reply() {
        let event_origin = origin(1, 1);
        const BUTTON_COUNT: usize = 8;
        for selected in 1..=BUTTON_COUNT {
            let mut state = TrackedState::new(FakeBackend::default());
            let request = NotificationRequest {
                title: Some("title".into()),
                identifier: Some("job".into()),
                report_activation: true,
                buttons: (1..=BUTTON_COUNT)
                    .map(|index| format!("Button {index}"))
                    .collect(),
                ..NotificationRequest::default()
            };
            assert!(state.show(key(event_origin, "job"), request).is_empty());
            assert_eq!(
                state.action_invoked(1, &button_action_key(1, selected)),
                Some(reported_activation(
                    event_origin,
                    format!("\x1b]99;i=job;{selected}\x1b\\")
                ))
            );
        }
    }

    #[test]
    fn anonymous_default_activation_uses_zero_and_captured_eight_bit_framing() {
        let event_origin = origin(1, 1);
        let mut state = TrackedState::new(FakeBackend::default());
        assert!(
            state
                .show(
                    TrackedKey {
                        origin: event_origin,
                        identifier: InternalIdentifier::Anonymous(1),
                    },
                    action_request(None, true),
                )
                .is_empty()
        );

        assert_eq!(
            state.action_invoked(1, DEFAULT_ACTION_KEY),
            Some(reported_activation(event_origin, b"\x9d99;i=0;\x9c"))
        );
    }

    #[test]
    fn focus_only_activation_is_exact_once_and_needs_no_protocol_reply() {
        let event_origin = origin(4, 7);
        let tracked_key = key(event_origin, "focus");
        let mut state = TrackedState::new(FakeBackend::default());
        let request = NotificationRequest {
            title: Some("title".into()),
            identifier: Some("focus".into()),
            focus_on_activation: true,
            ..NotificationRequest::default()
        };
        assert!(state.show(tracked_key, request).is_empty());
        assert_eq!(
            state.action_invoked(1, DEFAULT_ACTION_KEY),
            Some(ActivationCompletion {
                origin: event_origin,
                focus: true,
                protocol_reply: None,
                activation_token: None,
            })
        );
        assert_eq!(state.action_invoked(1, DEFAULT_ACTION_KEY), None);
    }

    #[test]
    fn activation_tokens_are_bounded_opaque_and_replacement_scoped() {
        let event_origin = origin(4, 7);
        let tracked_key = key(event_origin, "focus");
        let mut state = TrackedState::new(FakeBackend::default());
        let request = NotificationRequest {
            title: Some("title".into()),
            identifier: Some("focus".into()),
            report_activation: true,
            focus_on_activation: true,
            ..NotificationRequest::default()
        };
        assert!(state.show(tracked_key.clone(), request.clone()).is_empty());
        state.set_activation_token(999, "unmatched".into());
        state.set_activation_token(1, "bad\ntoken".into());
        state.set_activation_token(1, "x".repeat(MAX_ACTIVATION_TOKEN_BYTES + 1));
        state.set_activation_token(1, "opaque-token".into());
        state.set_activation_token(1, "ignored-second-token".into());
        assert_eq!(
            state.action_invoked(1, DEFAULT_ACTION_KEY),
            Some(ActivationCompletion {
                origin: event_origin,
                focus: true,
                protocol_reply: Some(b"\x1b]99;i=focus;\x1b\\".to_vec()),
                activation_token: Some("opaque-token".into()),
            })
        );

        assert!(state.show(tracked_key, request).is_empty());
        assert_eq!(
            state.action_invoked(1, DEFAULT_ACTION_KEY),
            Some(ActivationCompletion {
                origin: event_origin,
                focus: true,
                protocol_reply: Some(b"\x1b]99;i=focus;\x1b\\".to_vec()),
                activation_token: None,
            })
        );
    }

    #[test]
    fn replacement_updates_action_authority_only_after_success() {
        let event_origin = origin(1, 1);
        let tracked_key = key(event_origin, "job");
        let mut state = TrackedState::new(FakeBackend::default());
        assert!(
            state
                .show(tracked_key.clone(), action_request(Some("job"), false))
                .is_empty()
        );
        state.backend.show_error = Some("synthetic replacement failure".into());
        assert_eq!(
            state
                .show(
                    tracked_key.clone(),
                    request("job", NotificationExpiry::System),
                )
                .len(),
            1
        );
        assert!(state.action_invoked(1, DEFAULT_ACTION_KEY).is_some());

        state.backend.show_error = None;
        assert!(
            state
                .show(tracked_key, request("job", NotificationExpiry::System),)
                .is_empty()
        );
        assert_eq!(state.action_invoked(1, DEFAULT_ACTION_KEY), None);
    }

    #[test]
    fn successful_replacement_rejects_the_previous_revision_button_keys() {
        let event_origin = origin(1, 1);
        let tracked_key = key(event_origin, "job");
        let mut state = TrackedState::new(FakeBackend::default());
        assert!(
            state
                .show(tracked_key.clone(), action_request(Some("job"), false))
                .is_empty()
        );
        let old_key = state.backend.shown_action_keys[0][0].clone();

        assert!(
            state
                .show(tracked_key, action_request(Some("job"), false))
                .is_empty()
        );
        let current_key = state.backend.shown_action_keys[1][0].clone();
        assert_ne!(old_key, current_key);
        assert_eq!(state.action_invoked(1, &old_key), None);
        assert_eq!(
            state.action_invoked(1, &current_key),
            Some(reported_activation(event_origin, b"\x1b]99;i=job;1\x1b\\"))
        );
    }

    #[test]
    fn freedesktop_actions_use_private_keys_and_separate_labels() {
        let request = action_request(Some("job"), false);
        let action_keys = vec![button_action_key(7, 1), button_action_key(7, 2)];
        assert!(
            action_keys
                .iter()
                .all(|key| key.len() <= MAX_NATIVE_ACTION_KEY_BYTES)
        );
        assert_eq!(
            freedesktop_actions(&request, &action_keys),
            vec![
                "default",
                "Activate",
                "termeric-button-7-1",
                "Open",
                "termeric-button-7-2",
                "Ignore",
            ]
        );

        let no_report = NotificationRequest {
            title: Some("title".into()),
            buttons: vec!["Still visible".into()],
            ..NotificationRequest::default()
        };
        assert_eq!(
            freedesktop_actions(&no_report, &[button_action_key(8, 1)]),
            vec!["termeric-button-8-1", "Still visible"]
        );

        let focus_only = NotificationRequest {
            title: Some("title".into()),
            focus_on_activation: true,
            ..NotificationRequest::default()
        };
        assert_eq!(
            freedesktop_actions(&focus_only, &[]),
            vec!["default", "Activate"]
        );
    }

    #[test]
    fn rich_notification_metadata_becomes_freedesktop_notify_fields() {
        let request = NotificationRequest {
            title: Some("Build <b>done</b>; $(touch nope)".into()),
            body: "Finished `without a shell` & <i>quietly</i>".into(),
            application_name: Some("build agent".into()),
            notification_types: vec!["build.complete".into(), "ci".into()],
            urgency: Some(NotificationUrgency::Critical),
            expiry: NotificationExpiry::AfterMillis(5_000),
            ..NotificationRequest::default()
        };
        assert_eq!(
            freedesktop_notification(&request),
            FreedesktopNotification {
                app_name: "build agent",
                summary: "Build &lt;b&gt;done&lt;/b&gt;; $(touch nope)".into(),
                body: "Finished `without a shell` &amp; &lt;i&gt;quietly&lt;/i&gt;".into(),
                category: Some("build.complete,ci".into()),
                urgency: Some(2),
                expire_timeout: 5_000,
            }
        );
        assert_eq!(
            freedesktop_notification(&NotificationRequest {
                body: "Body".into(),
                ..NotificationRequest::default()
            }),
            FreedesktopNotification {
                app_name: "Termeric",
                summary: "Terminal".into(),
                body: "Body".into(),
                category: None,
                urgency: None,
                expire_timeout: -1,
            }
        );
    }

    #[test]
    fn freedesktop_sound_hints_are_typed_and_deterministic() {
        assert_eq!(freedesktop_sound_hint(NotificationSound::System), None);
        assert_eq!(
            freedesktop_sound_hint(NotificationSound::Silent),
            Some(FreedesktopSoundHint::Suppress)
        );
        for (sound, name) in [
            (NotificationSound::Error, "dialog-error"),
            (NotificationSound::Warning, "dialog-warning"),
            (NotificationSound::Information, "dialog-information"),
            (NotificationSound::Question, "dialog-question"),
        ] {
            assert_eq!(
                freedesktop_sound_hint(sound),
                Some(FreedesktopSoundHint::Name(name))
            );
        }
    }

    #[test]
    fn freedesktop_image_data_has_the_standard_exact_tuple_signature() {
        let icon = crate::notifications::icon::DecodedNotificationIcon {
            width: 2,
            height: 1,
            rowstride: 8,
            data: vec![1, 2, 3, 4, 5, 6, 7, 8],
        };
        let value = freedesktop_image_data_value(&icon);
        assert_eq!(value.value_signature().to_string(), "(iiibiiay)");
    }

    #[test]
    fn freedesktop_local_names_win_before_transmitted_fallback_without_paths() {
        let request = NotificationRequest {
            icon_names: vec!["warning".into()],
            application_name: Some("org.example.App".into()),
            ..NotificationRequest::default()
        };
        assert_eq!(
            freedesktop_app_icon(&request, Some("dialog-warning"), true),
            "dialog-warning"
        );
        assert_eq!(freedesktop_app_icon(&request, None, true), "");
        assert_eq!(
            freedesktop_app_icon(&request, None, false),
            "dialog-warning"
        );

        let unsafe_request = NotificationRequest {
            icon_names: vec!["/tmp/untrusted.png".into()],
            application_name: Some("../untrusted.png".into()),
            ..NotificationRequest::default()
        };
        assert_eq!(freedesktop_app_icon(&unsafe_request, None, false), "");
    }

    #[test]
    fn untracked_notifications_are_shown_without_keeping_state() {
        let mut state = TrackedState::new(FakeBackend::default());
        let untracked = NotificationRequest {
            title: Some("<b>Build</b>".into()),
            body: "done".into(),
            ..NotificationRequest::default()
        };
        assert!(state.show_untracked(&untracked).is_empty());
        assert_eq!(state.backend.shows.len(), 1);
        assert_eq!(state.backend.shows[0].1, 0, "never replaces a notification");
        assert!(state.entries.is_empty());
        assert!(state.native_keys.is_empty());

        state.backend.show_error = Some("synthetic show failure".into());
        let failures = state.show_untracked(&untracked);
        assert_eq!(failures.len(), 1);
        assert!(failures[0].contains("synthetic show failure"));
    }

    #[test]
    fn replacement_uses_only_the_latest_successful_close_report_policy() {
        let event_origin = origin(1, 1);
        let tracked_key = key(event_origin, "job");
        let mut state = TrackedState::new(FakeBackend::default());
        assert!(
            state
                .show(tracked_key.clone(), reporting_request(Some("job"), false),)
                .is_empty()
        );
        assert!(
            state
                .show(tracked_key, request("job", NotificationExpiry::System),)
                .is_empty()
        );
        assert_eq!(state.backend.shows[1].1, 1);
        assert_eq!(state.native_closed(1), None);
    }

    #[test]
    fn terminal_close_waits_for_signal_and_timeout_reports_untracked() {
        let event_origin = origin(1, 1);
        let mut state = TrackedState::new(FakeBackend::default());
        let first = key(event_origin, "first");
        assert!(
            state
                .show(first.clone(), reporting_request(Some("first"), false),)
                .is_empty()
        );
        assert!(state.close(&first).is_empty());
        assert!(!state.entries.contains_key(&first));
        assert!(state.closing.contains_key(&1));
        assert_eq!(
            state.alive_reply(event_origin, "probe", false).data,
            b"\x1b]99;i=probe:p=alive;\x1b\\"
        );
        assert_eq!(
            state.native_closed(1),
            Some(ProtocolReply {
                origin: event_origin,
                data: b"\x1b]99;i=first:p=close;\x1b\\".to_vec(),
            })
        );

        let second = key(event_origin, "second");
        assert!(
            state
                .show(second.clone(), reporting_request(Some("second"), false),)
                .is_empty()
        );
        assert!(state.close(&second).is_empty());
        let deadline = state.closing[&2].deadline;
        let (failures, replies) = state.reap_unconfirmed_closes(deadline);
        assert_eq!(failures.len(), 1);
        assert_eq!(
            replies,
            vec![ProtocolReply {
                origin: event_origin,
                data: b"\x1b]99;i=second:p=close;untracked\x1b\\".to_vec(),
            }]
        );
        assert!(state.closing.is_empty());
        assert_eq!(state.native_closed(999), None);

        let third = key(event_origin, "third");
        let mut expiring = reporting_request(Some("third"), false);
        expiring.expiry = NotificationExpiry::AfterMillis(1);
        assert!(state.show(third, expiring).is_empty());
        let expiry = state.entries.values().next().unwrap().expires_at.unwrap();
        assert!(state.expire_due(expiry).is_empty());
        assert!(state.entries.is_empty());
        assert!(state.closing.contains_key(&3));
        assert_eq!(
            state.native_closed(3),
            Some(ProtocolReply {
                origin: event_origin,
                data: b"\x1b]99;i=third:p=close;\x1b\\".to_vec(),
            })
        );
    }

    #[test]
    fn capacity_evicts_the_oldest_native_handle() {
        let mut state = TrackedState::new(FakeBackend::default());
        for index in 0..=MAX_TRACKED_NOTIFICATIONS {
            let identifier = format!("item-{index}");
            assert!(
                state
                    .show(
                        key(origin(1, 1), &identifier),
                        request(&identifier, Default::default()),
                    )
                    .is_empty()
            );
        }
        assert_eq!(state.entries.len(), MAX_TRACKED_NOTIFICATIONS);
        assert_eq!(state.backend.closes, vec![1]);
        assert!(!state.entries.contains_key(&key(origin(1, 1), "item-0")));
    }

    #[test]
    fn pending_close_reports_have_a_visible_hard_ceiling() {
        let event_origin = origin(1, 1);
        let mut state = TrackedState::new(FakeBackend::default());
        let mut failures = Vec::new();
        for index in 0..=MAX_TRACKED_NOTIFICATIONS {
            let identifier = format!("closing-{index}");
            let tracked_key = key(event_origin, &identifier);
            assert!(
                state
                    .show(
                        tracked_key.clone(),
                        reporting_request(Some(&identifier), false),
                    )
                    .is_empty()
            );
            failures.extend(state.close(&tracked_key));
        }

        assert_eq!(state.closing.len(), MAX_TRACKED_NOTIFICATIONS);
        assert!(!state.closing.contains_key(&1));
        assert!(state.closing.contains_key(&2));
        assert_eq!(
            failures,
            vec!["notification close-report tracking capacity exceeded"]
        );
    }

    #[test]
    fn backend_failures_preserve_existing_replacement_authority() {
        let mut state = TrackedState::new(FakeBackend::default());
        let tracked_key = key(origin(1, 1), "stable");
        assert!(
            state
                .show(tracked_key.clone(), request("stable", Default::default()),)
                .is_empty()
        );
        state.backend.show_error = Some("synthetic show failure".into());
        assert_eq!(
            state
                .show(tracked_key.clone(), request("stable", Default::default()),)
                .len(),
            1
        );
        assert_eq!(state.entries[&tracked_key].desktop_id, 1);

        state.backend.show_error = None;
        state.backend.close_errors_remaining = 1;
        assert_eq!(state.close(&tracked_key).len(), 1);
        assert!(state.entries.contains_key(&tracked_key));
    }

    #[test]
    fn markup_escaping_keeps_kitty_plain_text_literal() {
        assert_eq!(
            escape_markup("<b>A&B</b> > done"),
            "&lt;b&gt;A&amp;B&lt;/b&gt; &gt; done"
        );
    }

    #[test]
    fn worker_failures_are_deduplicated_until_recovery() {
        let (sender, receiver) = mpsc::sync_channel(8);
        let mut last_failure = None;

        publish_operation(&sender, &mut last_failure, vec!["same failure".into()]);
        publish_operation(&sender, &mut last_failure, vec!["same failure".into()]);
        assert!(matches!(
            receiver.try_recv(),
            Ok(TrackedCompletion::Failure(error)) if error == "same failure"
        ));
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));

        publish_operation(&sender, &mut last_failure, Vec::new());
        assert!(matches!(
            receiver.try_recv(),
            Ok(TrackedCompletion::Recovered)
        ));

        publish_operation(&sender, &mut last_failure, vec!["same failure".into()]);
        assert!(matches!(
            receiver.try_recv(),
            Ok(TrackedCompletion::Failure(error)) if error == "same failure"
        ));
    }

    #[test]
    fn a_full_completion_queue_does_not_lose_recovery_state() {
        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .try_send(TrackedCompletion::Failure("queued".into()))
            .unwrap();
        let mut last_failure = Some("active".into());

        publish_operation(&sender, &mut last_failure, Vec::new());
        assert_eq!(last_failure.as_deref(), Some("active"));
        assert!(matches!(
            receiver.try_recv(),
            Ok(TrackedCompletion::Failure(error)) if error == "queued"
        ));

        publish_operation(&sender, &mut last_failure, Vec::new());
        assert!(last_failure.is_none());
        assert!(matches!(
            receiver.try_recv(),
            Ok(TrackedCompletion::Recovered)
        ));
    }

    #[test]
    fn dropping_the_owner_stops_the_worker_without_native_io() {
        let (shutdown_sender, shutdown_receiver) = mpsc::sync_channel(0);
        let tracked = TrackedNotifications::new_with_backend(FakeBackend {
            shutdown_notice: Some(shutdown_sender),
            ..FakeBackend::default()
        })
        .unwrap();

        drop(tracked);

        assert!(
            shutdown_receiver
                .recv_timeout(Duration::from_secs(1))
                .is_ok()
        );
    }

    #[test]
    fn signal_listener_failures_cross_the_bounded_worker_channel() {
        let tracked = TrackedNotifications::new_with_backend(FakeBackend::default()).unwrap();
        tracked
            .commands
            .send(TrackedCommand::BackendFailure(
                "synthetic signal disconnect".into(),
            ))
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let completions = tracked.drain_completions();
            if completions.iter().any(
                |completion| matches!(completion, TrackedCompletion::Failure(error) if error == "synthetic signal disconnect"),
            ) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "worker did not surface the synthetic signal failure"
            );
            std::thread::yield_now();
        }
    }
}
