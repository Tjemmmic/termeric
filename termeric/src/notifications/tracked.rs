//! Bounded, origin-scoped Linux notification handles for Kitty OSC 99.

use crate::tabs::TerminalEventOrigin;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::time::{Duration, Instant};
use termeric_core::{NotificationExpiry, NotificationRequest, NotificationUrgency};

const TRACKED_COMMAND_CAPACITY: usize = 64;
const MAX_TRACKED_NOTIFICATIONS: usize = 64;
const MAX_EXPIRY_CLOSE_ATTEMPTS: u8 = 3;
const EXPIRY_RETRY_DELAY: Duration = Duration::from_secs(1);
const CLOSE_SIGNAL_TIMEOUT: Duration = Duration::from_secs(5);
const DBUS_METHOD_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum InternalIdentifier {
    Named(String),
    Anonymous(u64),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct TrackedKey {
    origin: TerminalEventOrigin,
    identifier: InternalIdentifier,
}

#[derive(Clone, Debug)]
struct TrackedEntry {
    desktop_id: u32,
    expires_at: Option<Instant>,
    expiry_close_attempts: u8,
    report_close: bool,
    reply_uses_8bit_controls: bool,
}

#[derive(Clone, Debug)]
struct ClosingEntry {
    origin: TerminalEventOrigin,
    identifier: String,
    reply_uses_8bit_controls: bool,
    deadline: Instant,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ProtocolReply {
    pub(super) origin: TerminalEventOrigin,
    pub(super) data: Vec<u8>,
}

enum TrackedCommand {
    Show {
        key: TrackedKey,
        request: NotificationRequest,
    },
    Close {
        key: TrackedKey,
    },
    Alive {
        origin: TerminalEventOrigin,
        identifier: String,
        reply_uses_8bit_controls: bool,
    },
    NativeClosed {
        desktop_id: u32,
    },
    BackendFailure(String),
    Shutdown,
}

pub(super) enum TrackedCompletion {
    Failure(String),
    Recovered,
    ProtocolReply(ProtocolReply),
}

trait TrackedNotificationBackend: Send + 'static {
    fn prepare(&mut self, _commands: SyncSender<TrackedCommand>) -> Result<(), String> {
        Ok(())
    }

    fn show(&mut self, request: &NotificationRequest, replaces_id: u32) -> Result<u32, String>;

    fn close(&mut self, desktop_id: u32) -> Result<(), String>;

    fn shutdown(&mut self) {}
}

struct TrackedState<B> {
    backend: B,
    entries: HashMap<TrackedKey, TrackedEntry>,
    native_keys: HashMap<u32, TrackedKey>,
    order: VecDeque<TrackedKey>,
    closing: HashMap<u32, ClosingEntry>,
    closing_order: VecDeque<u32>,
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
        }
    }

    fn show(&mut self, key: TrackedKey, request: NotificationRequest) -> Vec<String> {
        let replaces_id = self.entries.get(&key).map_or(0, |entry| entry.desktop_id);
        let desktop_id = match self.backend.show(&request, replaces_id) {
            Ok(desktop_id) if desktop_id != 0 => desktop_id,
            Ok(_) => return vec!["notification service returned invalid desktop ID 0".into()],
            Err(error) => return vec![format!("could not display tracked notification: {error}")],
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
                reply_uses_8bit_controls: request.reply_uses_8bit_controls,
            },
        );
        self.enforce_capacity()
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

    fn alive_reply(
        &self,
        origin: TerminalEventOrigin,
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

pub(super) struct TrackedNotifications {
    commands: SyncSender<TrackedCommand>,
    completions: Receiver<TrackedCompletion>,
    next_anonymous: u64,
    shutdown: Arc<AtomicBool>,
}

impl TrackedNotifications {
    pub(super) fn new() -> Result<Self, String> {
        Self::new_with_backend(FreedesktopBackend::default())
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
        origin: TerminalEventOrigin,
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
            request,
        })
    }

    pub(super) fn close(
        &self,
        origin: TerminalEventOrigin,
        identifier: String,
    ) -> Result<(), String> {
        self.send(TrackedCommand::Close {
            key: TrackedKey {
                origin,
                identifier: InternalIdentifier::Named(identifier),
            },
        })
    }

    pub(super) fn alive(
        &self,
        origin: TerminalEventOrigin,
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
                    Ok(()) => state.show(key, request),
                    Err(error) => vec![format!(
                        "could not prepare notification close tracking: {error}"
                    )],
                };
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

#[derive(Default)]
struct FreedesktopBackend {
    connection: Option<zbus::blocking::Connection>,
    signal_stop: Option<async_channel::Sender<()>>,
    signal_thread: Option<std::thread::JoinHandle<()>>,
    signal_alive: Option<Arc<AtomicBool>>,
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

    fn show(&mut self, request: &NotificationRequest, replaces_id: u32) -> Result<u32, String> {
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
            let app_name = request.application_name.as_deref().unwrap_or("Termeric");
            let app_icon = request
                .icon_names
                .first()
                .map(|name| super::freedesktop_icon_name(name))
                .or(request.application_name.as_deref())
                .unwrap_or("");
            let summary = escape_markup(request.title.as_deref().unwrap_or("Terminal"));
            let body = escape_markup(&request.body);
            let actions: Vec<&str> = Vec::new();
            let categories = request.notification_types.join(",");
            let mut hints: HashMap<&str, Value<'_>> = HashMap::new();
            if !categories.is_empty() {
                hints.insert("category", Value::from(categories.as_str()));
            }
            if let Some(urgency) = request.urgency {
                let urgency = match urgency {
                    NotificationUrgency::Low => 0_u8,
                    NotificationUrgency::Normal => 1_u8,
                    NotificationUrgency::Critical => 2_u8,
                };
                hints.insert("urgency", Value::from(urgency));
            }
            let expire_timeout = match request.expiry {
                NotificationExpiry::System => -1,
                NotificationExpiry::Never => 0,
                NotificationExpiry::AfterMillis(milliseconds) => milliseconds as i32,
            };
            proxy
                .call(
                    "Notify",
                    &(
                        app_name,
                        replaces_id,
                        app_icon,
                        summary.as_str(),
                        body.as_str(),
                        actions,
                        hints,
                        expire_timeout,
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
    let mut signals = match proxy.receive_signal("NotificationClosed").await {
        Ok(signals) => signals,
        Err(error) => {
            let _ = ready.send(Err(format!(
                "notification close signals are unavailable: {error}"
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
            ListenerEvent::Signal(None) => {
                let _ = commands.send(TrackedCommand::BackendFailure(
                    "notification close signal stream stopped".into(),
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
        closes: Vec<u32>,
        show_error: Option<String>,
        show_delay: Duration,
        close_errors_remaining: usize,
        shutdown_notice: Option<SyncSender<()>>,
    }

    impl TrackedNotificationBackend for FakeBackend {
        fn show(&mut self, request: &NotificationRequest, replaces_id: u32) -> Result<u32, String> {
            if let Some(error) = &self.show_error {
                return Err(error.clone());
            }
            std::thread::sleep(self.show_delay);
            self.shows
                .push((request.identifier.clone(), replaces_id, request.expiry));
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

    fn origin(tab_id: usize, pane_id: u32) -> TerminalEventOrigin {
        TerminalEventOrigin::Pane {
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

    fn key(origin: TerminalEventOrigin, identifier: &str) -> TrackedKey {
        TrackedKey {
            origin,
            identifier: InternalIdentifier::Named(identifier.into()),
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
