//! Desktop notifications on Linux: the freedesktop notification service over
//! D-Bus, and the signals it sends when a notification closes or a person
//! activates it.

use super::tracked::{
    DEFAULT_ACTION_KEY, TrackedCommand, TrackedNotificationBackend, without_bidi_formatting,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::time::Duration;
use termeric_core::{
    NotificationExpiry, NotificationRequest, NotificationSound, NotificationUrgency,
};
use zbus::zvariant::Value;

const DBUS_METHOD_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_NATIVE_ACTION_KEY_BYTES: usize = 64;
const NOTIFICATIONS_SERVICE: &str = "org.freedesktop.Notifications";
const NOTIFICATIONS_PATH: &str = "/org/freedesktop/Notifications";

/// The freedesktop icon name for one of the names Kitty defines.
fn freedesktop_icon_name(name: &str) -> &str {
    match name {
        "error" => "dialog-error",
        "warn" | "warning" => "dialog-warning",
        "info" => "dialog-information",
        "question" => "dialog-question",
        "help" => "help-browser",
        "file-manager" => "system-file-manager",
        "system-monitor" => "utilities-system-monitor",
        "text-editor" => "accessories-text-editor",
        _ => name,
    }
}

/// The first of a request's icon names the icon theme has, in its
/// freedesktop spelling.
fn first_available_icon_name(names: &[String]) -> Option<String> {
    first_available_icon_name_with(names, |mapped| {
        freedesktop_icons::lookup(mapped)
            .with_size(256)
            .find()
            .is_some()
    })
}

/// An icon name, never a path, so a program cannot point the notification
/// service at a file.
fn is_safe_icon_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && !name.contains('/') && !name.contains('\\')
}

fn first_available_icon_name_with(
    names: &[String],
    mut available: impl FnMut(&str) -> bool,
) -> Option<String> {
    names
        .iter()
        .filter(|name| is_safe_icon_name(name))
        .find_map(|name| {
            let mapped = freedesktop_icon_name(name);
            available(mapped).then(|| mapped.to_owned())
        })
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
        summary: escape_markup(&without_bidi_formatting(
            request.title.as_deref().unwrap_or("Terminal"),
        )),
        body: escape_markup(&without_bidi_formatting(&request.body)),
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
        .find(|name| is_safe_icon_name(name))
        .map(|name| freedesktop_icon_name(name))
        .or(request
            .application_name
            .as_deref()
            .filter(|name| is_safe_icon_name(name)))
        .unwrap_or("")
        .to_owned()
}

#[derive(Default)]
pub(super) struct FreedesktopBackend {
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

    fn proxy(&mut self) -> Result<zbus::blocking::Proxy<'_>, String> {
        zbus::blocking::Proxy::new(
            self.connection()?,
            NOTIFICATIONS_SERVICE,
            NOTIFICATIONS_PATH,
            NOTIFICATIONS_SERVICE,
        )
        .map_err(|error| format!("notification interface is unavailable: {error}"))
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
        let named_icon = first_available_icon_name(&request.icon_names);
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
        self.proxy()?
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
    }

    fn close(&mut self, desktop_id: u32) -> Result<(), String> {
        self.proxy()?
            .call("CloseNotification", &(desktop_id,))
            .map_err(|error| format!("CloseNotification call failed: {error}"))
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
        NOTIFICATIONS_SERVICE,
        NOTIFICATIONS_PATH,
        NOTIFICATIONS_SERVICE,
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
    use crate::notifications::tracked::button_action_key;

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
            title: Some("Build <b>done</b>;\u{202e} $(touch nope)".into()),
            body: "Finished `without a shell` &\n<i>quietly</i>\u{2066}".into(),
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
                body: "Finished `without a shell` &amp;\n&lt;i&gt;quietly&lt;/i&gt;".into(),
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
        for (urgency, level) in [
            (NotificationUrgency::Low, 0),
            (NotificationUrgency::Normal, 1),
            (NotificationUrgency::Critical, 2),
        ] {
            let request = NotificationRequest {
                urgency: Some(urgency),
                ..NotificationRequest::default()
            };
            assert_eq!(freedesktop_notification(&request).urgency, Some(level));
        }
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
    fn markup_escaping_keeps_kitty_plain_text_literal() {
        assert_eq!(
            escape_markup("<b>A&B</b> > done"),
            "&lt;b&gt;A&amp;B&lt;/b&gt; &gt; done"
        );
    }

    #[test]
    fn kitty_icon_names_map_to_freedesktop_icons() {
        assert_eq!(freedesktop_icon_name("error"), "dialog-error");
        assert_eq!(freedesktop_icon_name("warn"), "dialog-warning");
        assert_eq!(freedesktop_icon_name("warning"), "dialog-warning");
        assert_eq!(freedesktop_icon_name("info"), "dialog-information");
        assert_eq!(freedesktop_icon_name("question"), "dialog-question");
        assert_eq!(freedesktop_icon_name("help"), "help-browser");
        assert_eq!(freedesktop_icon_name("file-manager"), "system-file-manager");
        assert_eq!(
            freedesktop_icon_name("system-monitor"),
            "utilities-system-monitor"
        );
        assert_eq!(
            freedesktop_icon_name("text-editor"),
            "accessories-text-editor"
        );
        assert_eq!(freedesktop_icon_name("org.example.App"), "org.example.App");
    }

    #[test]
    fn local_icon_names_keep_protocol_order_and_universal_aliases() {
        let names = vec![
            "/tmp/untrusted.png".to_string(),
            "warning".to_string(),
            "second".to_string(),
        ];
        let mut checked = Vec::new();
        let selected = first_available_icon_name_with(&names, |name| {
            checked.push(name.to_owned());
            name == "second"
        });
        assert_eq!(selected.as_deref(), Some("second"));
        assert_eq!(checked, ["dialog-warning", "second"]);

        let selected = first_available_icon_name_with(&names, |_| true);
        assert_eq!(selected.as_deref(), Some("dialog-warning"));
    }
}
