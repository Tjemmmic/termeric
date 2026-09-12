//! Bounded, origin-scoped Linux notification handles for Kitty OSC 99.

use crate::tabs::TerminalEventOrigin;
use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::time::{Duration, Instant};
use termeric_core::{NotificationExpiry, NotificationRequest, NotificationUrgency};

const TRACKED_COMMAND_CAPACITY: usize = 64;
const MAX_TRACKED_NOTIFICATIONS: usize = 64;
const MAX_EXPIRY_CLOSE_ATTEMPTS: u8 = 3;
const EXPIRY_RETRY_DELAY: Duration = Duration::from_secs(1);
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
}

enum TrackedCommand {
    Show {
        key: TrackedKey,
        request: NotificationRequest,
    },
    Close {
        key: TrackedKey,
    },
}

enum TrackedCompletion {
    Failure(String),
    Recovered,
}

trait TrackedNotificationBackend: Send + 'static {
    fn show(&mut self, request: &NotificationRequest, replaces_id: u32) -> Result<u32, String>;

    fn close(&mut self, desktop_id: u32) -> Result<(), String>;
}

struct TrackedState<B> {
    backend: B,
    entries: HashMap<TrackedKey, TrackedEntry>,
    order: VecDeque<TrackedKey>,
}

impl<B: TrackedNotificationBackend> TrackedState<B> {
    fn new(backend: B) -> Self {
        Self {
            backend,
            entries: HashMap::new(),
            order: VecDeque::new(),
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
        self.order.retain(|candidate| candidate != &key);
        self.order.push_back(key.clone());
        self.entries.insert(
            key,
            TrackedEntry {
                desktop_id,
                expires_at,
                expiry_close_attempts: 0,
            },
        );
        self.enforce_capacity()
    }

    fn close(&mut self, key: &TrackedKey) -> Vec<String> {
        let Some(desktop_id) = self.entries.get(key).map(|entry| entry.desktop_id) else {
            return Vec::new();
        };
        match self.backend.close(desktop_id) {
            Ok(()) => {
                self.remove(key);
                Vec::new()
            }
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
                Ok(()) => self.remove(&key),
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

    fn next_expiry(&self) -> Option<Instant> {
        self.entries
            .values()
            .filter_map(|entry| entry.expires_at)
            .min()
    }

    fn enforce_capacity(&mut self) -> Vec<String> {
        let mut failures = Vec::new();
        while self.entries.len() > MAX_TRACKED_NOTIFICATIONS {
            let Some(key) = self.order.pop_front() else {
                break;
            };
            let Some(entry) = self.entries.remove(&key) else {
                continue;
            };
            if let Err(error) = self.backend.close(entry.desktop_id) {
                failures.push(format!(
                    "could not close evicted tracked notification: {error}"
                ));
            }
        }
        failures
    }

    fn remove(&mut self, key: &TrackedKey) {
        self.entries.remove(key);
        self.order.retain(|candidate| candidate != key);
    }
}

pub(super) struct TrackedNotifications {
    commands: SyncSender<TrackedCommand>,
    completions: Receiver<TrackedCompletion>,
    next_anonymous: u64,
}

impl TrackedNotifications {
    pub(super) fn new() -> Result<Self, String> {
        Self::new_with_backend(FreedesktopBackend::default())
    }

    fn new_with_backend<B: TrackedNotificationBackend>(backend: B) -> Result<Self, String> {
        let (commands, receiver) = mpsc::sync_channel(TRACKED_COMMAND_CAPACITY);
        let (completion_sender, completions) = mpsc::sync_channel(TRACKED_COMMAND_CAPACITY);
        std::thread::Builder::new()
            .name("termeric-notification-lifecycle".into())
            .spawn(move || {
                tracked_worker(receiver, completion_sender, TrackedState::new(backend));
            })
            .map_err(|error| format!("could not start notification lifecycle worker: {error}"))?;
        Ok(Self {
            commands,
            completions,
            next_anonymous: 1,
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

    pub(super) fn drain_completions(&self) -> Vec<Result<(), String>> {
        self.completions
            .try_iter()
            .map(|completion| match completion {
                TrackedCompletion::Failure(error) => Err(error),
                TrackedCompletion::Recovered => Ok(()),
            })
            .collect()
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

fn tracked_worker<B: TrackedNotificationBackend>(
    commands: Receiver<TrackedCommand>,
    completions: SyncSender<TrackedCompletion>,
    mut state: TrackedState<B>,
) {
    let mut last_failure = None;
    loop {
        let now = Instant::now();
        if state.next_expiry().is_some_and(|deadline| deadline <= now) {
            publish_operation(&completions, &mut last_failure, state.expire_due(now));
        }
        let wait = state
            .next_expiry()
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
            .unwrap_or(Duration::from_secs(60));
        match commands.recv_timeout(wait) {
            Ok(TrackedCommand::Show { key, request }) => {
                let failures = state.show(key, request);
                publish_operation(&completions, &mut last_failure, failures);
            }
            Ok(TrackedCommand::Close { key }) => {
                let failures = state.close(&key);
                publish_operation(&completions, &mut last_failure, failures);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
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

    fn invalidate_after<T>(&mut self, result: Result<T, String>) -> Result<T, String> {
        if result.is_err() {
            self.connection = None;
        }
        result
    }
}

impl TrackedNotificationBackend for FreedesktopBackend {
    fn show(&mut self, request: &NotificationRequest, replaces_id: u32) -> Result<u32, String> {
        let connection = self.connection()?;
        let result = (|| {
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
        })();
        self.invalidate_after(result)
    }

    fn close(&mut self, desktop_id: u32) -> Result<(), String> {
        let connection = self.connection()?;
        let result = (|| {
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
        })();
        self.invalidate_after(result)
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
}
