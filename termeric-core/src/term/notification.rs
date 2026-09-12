//! Bounded terminal-local state for Kitty's OSC 99 notification protocol.

use crate::event::{
    NotificationExpiry, NotificationOccasion, NotificationRequest, NotificationSound,
    NotificationUrgency,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::collections::VecDeque;

pub const MAX_NOTIFICATION_TITLE_BYTES: usize = 4 * 1024;
pub const MAX_NOTIFICATION_BODY_BYTES: usize = 16 * 1024;
pub const MAX_PENDING_NOTIFICATIONS: usize = 16;
pub const MAX_PENDING_NOTIFICATION_BYTES: usize = 256 * 1024;
pub const MAX_NOTIFICATION_IDENTIFIER_BYTES: usize = 128;
pub const MAX_NOTIFICATION_METADATA_VALUE_BYTES: usize = 256;
pub const MAX_NOTIFICATION_TYPES: usize = 8;
pub const MAX_NOTIFICATION_ICON_NAMES: usize = 8;
pub const MAX_NOTIFICATION_ICON_BYTES: usize = 192 * 1024;
pub const MAX_NOTIFICATION_BUTTONS: usize = 8;
pub const MAX_NOTIFICATION_BUTTON_LABEL_BYTES: usize = 256;
pub const MAX_NOTIFICATION_BUTTON_PAYLOAD_BYTES: usize = 4 * 1024;
pub const MAX_OSC99_DECODED_CHUNK_BYTES: usize = 2 * 1024;
pub const MAX_OSC99_ENCODED_CHUNK_BYTES: usize = 4 * 1024;
const MAX_OSC99_METADATA_BYTES: usize = 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PayloadKind {
    #[default]
    Title,
    Body,
    Query,
    Alive,
    Buttons,
    Icon,
    Close,
    Unsupported,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Metadata {
    identifier: Option<String>,
    payload: PayloadKind,
    done: bool,
    encoded: bool,
    application_name: Option<String>,
    notification_types: Vec<String>,
    icon_names: Vec<String>,
    icon_cache_id: Option<String>,
    urgency: Option<NotificationUrgency>,
    expiry: Option<NotificationExpiry>,
    occasion: Option<NotificationOccasion>,
    sound: Option<NotificationSound>,
    report_close: Option<bool>,
    report_activation: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct PendingField {
    decoded: Vec<u8>,
    encoded_tail: Vec<u8>,
}

impl PendingField {
    fn append_plain(&mut self, bytes: &[u8], limit: usize) -> Result<(), ()> {
        self.finish_encoded(limit)?;
        let text = std::str::from_utf8(bytes).map_err(|_| ())?;
        if text.chars().any(is_unsafe_plain_character)
            || self.decoded.len().saturating_add(bytes.len()) > limit
        {
            return Err(());
        }
        self.decoded.extend_from_slice(bytes);
        Ok(())
    }

    fn append_encoded(&mut self, bytes: &[u8], done: bool, limit: usize) -> Result<(), ()> {
        if !valid_base64_bytes(bytes)
            || self.encoded_tail.len().saturating_add(bytes.len()) > limit.saturating_mul(2)
        {
            return Err(());
        }
        self.encoded_tail.extend_from_slice(bytes);
        if done || bytes.ends_with(b"=") {
            self.finish_encoded(limit)?;
        } else {
            self.finish_complete_groups(limit)?;
        }
        Ok(())
    }

    fn finish_complete_groups(&mut self, limit: usize) -> Result<(), ()> {
        let complete_len = self.encoded_tail.len() / 4 * 4;
        if complete_len == 0 {
            return Ok(());
        }
        let decoded = STANDARD
            .decode(&self.encoded_tail[..complete_len])
            .map_err(|_| ())?;
        if self.decoded.len().saturating_add(decoded.len()) > limit {
            return Err(());
        }
        self.decoded.extend_from_slice(&decoded);
        self.encoded_tail.drain(..complete_len);
        Ok(())
    }

    fn finish_encoded(&mut self, limit: usize) -> Result<(), ()> {
        if self.encoded_tail.is_empty() {
            return Ok(());
        }
        let decoded = STANDARD
            .decode(&self.encoded_tail)
            .or_else(|_| {
                base64::engine::general_purpose::STANDARD_NO_PAD.decode(&self.encoded_tail)
            })
            .map_err(|_| ())?;
        if self.decoded.len().saturating_add(decoded.len()) > limit {
            return Err(());
        }
        self.decoded.extend_from_slice(&decoded);
        self.encoded_tail.clear();
        Ok(())
    }

    fn retained_bytes(&self) -> usize {
        self.decoded.len() + self.encoded_tail.len()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct PendingNotification {
    identifier: Option<String>,
    title: PendingField,
    body: PendingField,
    buttons: PendingField,
    icon: PendingField,
    application_name: Option<String>,
    notification_types: Vec<String>,
    icon_names: Vec<String>,
    icon_cache_id: Option<String>,
    urgency: Option<NotificationUrgency>,
    expiry: NotificationExpiry,
    occasion: NotificationOccasion,
    sound: NotificationSound,
    report_close: bool,
    report_activation: bool,
}

impl PendingNotification {
    fn retained_bytes(&self) -> usize {
        self.identifier.as_ref().map_or(0, String::len)
            + self.title.retained_bytes()
            + self.body.retained_bytes()
            + self.buttons.retained_bytes()
            + self.icon.retained_bytes()
            + self.application_name.as_ref().map_or(0, String::len)
            + self
                .notification_types
                .iter()
                .map(String::len)
                .sum::<usize>()
            + self.icon_names.iter().map(String::len).sum::<usize>()
            + self.icon_cache_id.as_ref().map_or(0, String::len)
    }

    fn apply_metadata(&mut self, metadata: Metadata) -> Result<(), ()> {
        if let Some(application_name) = metadata.application_name {
            self.application_name = Some(application_name);
        }
        append_unique_bounded(
            &mut self.notification_types,
            metadata.notification_types,
            MAX_NOTIFICATION_TYPES,
        )?;
        append_unique_bounded(
            &mut self.icon_names,
            metadata.icon_names,
            MAX_NOTIFICATION_ICON_NAMES,
        )?;
        if let Some(icon_cache_id) = metadata.icon_cache_id {
            self.icon_cache_id = Some(icon_cache_id);
        }
        if metadata.urgency.is_some() {
            self.urgency = metadata.urgency;
        }
        if let Some(expiry) = metadata.expiry {
            self.expiry = expiry;
        }
        if let Some(occasion) = metadata.occasion {
            self.occasion = occasion;
        }
        if let Some(sound) = metadata.sound {
            self.sound = sound;
        }
        if let Some(report_close) = metadata.report_close {
            self.report_close = report_close;
        }
        if let Some(report_activation) = metadata.report_activation {
            self.report_activation = report_activation;
        }
        Ok(())
    }

    fn finish(mut self) -> Option<NotificationRequest> {
        self.title
            .finish_encoded(MAX_NOTIFICATION_TITLE_BYTES)
            .ok()?;
        self.body.finish_encoded(MAX_NOTIFICATION_BODY_BYTES).ok()?;
        self.buttons
            .finish_encoded(MAX_NOTIFICATION_BUTTON_PAYLOAD_BYTES)
            .ok()?;
        self.icon.finish_encoded(MAX_NOTIFICATION_ICON_BYTES).ok()?;
        let mut title = String::from_utf8(self.title.decoded).ok()?;
        let mut body = String::from_utf8(self.body.decoded).ok()?;
        let buttons = parse_buttons(self.buttons.decoded)?;
        let icon_data =
            (!self.icon.decoded.is_empty()).then(|| self.icon.decoded.into_boxed_slice());
        if title.is_empty() {
            if body.is_empty() {
                return None;
            }
            title = std::mem::take(&mut body);
        }
        Some(NotificationRequest {
            title: Some(title),
            body,
            identifier: self.identifier,
            application_name: self.application_name,
            notification_types: self.notification_types,
            icon_names: self.icon_names,
            icon_cache_id: self.icon_cache_id,
            icon_data,
            urgency: self.urgency,
            expiry: self.expiry,
            occasion: self.occasion,
            sound: self.sound,
            report_close: self.report_close,
            report_activation: self.report_activation,
            buttons,
            reply_uses_8bit_controls: false,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Osc99Action {
    None,
    Query(String),
    AliveQuery(String),
    Notify(NotificationRequest),
    Close(String),
}

/// Pending OSC 99 notifications are global to one terminal owner. The deque
/// provides deterministic least-recently-updated eviction.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct NotificationProtocol {
    pending: VecDeque<PendingNotification>,
}

impl NotificationProtocol {
    pub(super) fn handle(&mut self, params: &[&[u8]]) -> Osc99Action {
        // The metadata and payload separators are mandatory, including for an
        // empty payload. `params` begins after the OSC command number.
        if params.len() < 2 {
            return Osc99Action::None;
        }
        let identifier_hint = identifier_hint(params[0]);
        let metadata = match parse_metadata(params[0]) {
            Ok(metadata) => metadata,
            Err(()) => {
                if let Some(identifier) = identifier_hint.as_deref() {
                    self.remove(identifier);
                }
                return Osc99Action::None;
            }
        };
        let payload = match join_payload(&params[1..], metadata.encoded) {
            Some(payload) => payload,
            None => {
                if let Some(identifier) = metadata.identifier.as_deref() {
                    self.remove(identifier);
                }
                return Osc99Action::None;
            }
        };

        if metadata.payload == PayloadKind::Query {
            if !metadata.done || !payload.is_empty() {
                return Osc99Action::None;
            }
            return Osc99Action::Query(metadata.identifier.unwrap_or_else(|| "0".into()));
        }
        if metadata.payload == PayloadKind::Alive {
            if !metadata.done || !payload.is_empty() {
                return Osc99Action::None;
            }
            return Osc99Action::AliveQuery(metadata.identifier.unwrap_or_else(|| "0".into()));
        }
        if metadata.payload == PayloadKind::Close {
            let Some(identifier) = metadata.identifier else {
                return Osc99Action::None;
            };
            if !metadata.done || !payload.is_empty() {
                self.remove(&identifier);
                return Osc99Action::None;
            }
            self.remove(&identifier);
            return Osc99Action::Close(identifier);
        }
        if !matches!(
            metadata.payload,
            PayloadKind::Title | PayloadKind::Body | PayloadKind::Buttons | PayloadKind::Icon
        ) {
            return Osc99Action::None;
        }
        if metadata.payload == PayloadKind::Icon && !metadata.encoded {
            if let Some(identifier) = metadata.identifier.as_deref() {
                self.remove(identifier);
            }
            return Osc99Action::None;
        }
        if !metadata.done && metadata.identifier.is_none() {
            return Osc99Action::None;
        }

        let identifier = metadata.identifier.clone();
        let payload_kind = metadata.payload;
        let encoded = metadata.encoded;
        let done = metadata.done;
        let mut notification = identifier
            .as_deref()
            .and_then(|identifier| self.take(identifier))
            .unwrap_or_else(|| PendingNotification {
                identifier,
                ..PendingNotification::default()
            });
        if notification.apply_metadata(metadata).is_err() {
            return Osc99Action::None;
        }
        let (field, limit) = match payload_kind {
            PayloadKind::Title => (&mut notification.title, MAX_NOTIFICATION_TITLE_BYTES),
            PayloadKind::Body => (&mut notification.body, MAX_NOTIFICATION_BODY_BYTES),
            PayloadKind::Buttons => (
                &mut notification.buttons,
                MAX_NOTIFICATION_BUTTON_PAYLOAD_BYTES,
            ),
            PayloadKind::Icon => (&mut notification.icon, MAX_NOTIFICATION_ICON_BYTES),
            _ => unreachable!(),
        };
        let appended = if encoded {
            field.append_encoded(&payload, done, limit)
        } else {
            field.append_plain(&payload, limit)
        };
        if appended.is_err() {
            return Osc99Action::None;
        }

        if done {
            return notification
                .finish()
                .map_or(Osc99Action::None, Osc99Action::Notify);
        }
        self.pending.push_back(notification);
        self.enforce_limits();
        Osc99Action::None
    }

    pub(super) fn clear(&mut self) {
        self.pending.clear();
    }

    fn take(&mut self, identifier: &str) -> Option<PendingNotification> {
        let index = self
            .pending
            .iter()
            .position(|notification| notification.identifier.as_deref() == Some(identifier))?;
        self.pending.remove(index)
    }

    fn remove(&mut self, identifier: &str) {
        let _ = self.take(identifier);
    }

    fn enforce_limits(&mut self) {
        while self.pending.len() > MAX_PENDING_NOTIFICATIONS
            || self.retained_bytes() > MAX_PENDING_NOTIFICATION_BYTES
        {
            self.pending.pop_front();
        }
    }

    fn retained_bytes(&self) -> usize {
        self.pending
            .iter()
            .map(PendingNotification::retained_bytes)
            .sum()
    }

    #[cfg(test)]
    pub(super) fn pending_len(&self) -> usize {
        self.pending.len()
    }

    #[cfg(test)]
    pub(super) fn pending_bytes(&self) -> usize {
        self.retained_bytes()
    }
}

fn parse_metadata(bytes: &[u8]) -> Result<Metadata, ()> {
    if bytes.len() > MAX_OSC99_METADATA_BYTES {
        return Err(());
    }
    let mut metadata = Metadata {
        done: true,
        ..Metadata::default()
    };
    if bytes.is_empty() {
        return Ok(metadata);
    }

    let mut seen = [false; 128];
    for field in bytes.split(|byte| *byte == b':') {
        let Some(equal) = field.iter().position(|byte| *byte == b'=') else {
            return Err(());
        };
        let (key, value_with_equal) = field.split_at(equal);
        let value = &value_with_equal[1..];
        if key.len() != 1
            || !key[0].is_ascii_alphabetic()
            || value.is_empty()
            || !value.iter().all(|byte| valid_metadata_value_byte(*byte))
        {
            return Err(());
        }
        let key = key[0];
        let singular = matches!(
            key,
            b'i' | b'p' | b'd' | b'e' | b'c' | b'o' | b'a' | b'f' | b'u' | b'w' | b's' | b'g'
        );
        if singular && std::mem::replace(&mut seen[key as usize], true) {
            return Err(());
        }
        match key {
            b'i' => {
                if !valid_identifier(value) {
                    return Err(());
                }
                metadata.identifier = Some(String::from_utf8(value.to_vec()).map_err(|_| ())?);
            }
            b'p' => {
                metadata.payload = match value {
                    b"title" => PayloadKind::Title,
                    b"body" => PayloadKind::Body,
                    b"?" => PayloadKind::Query,
                    b"alive" => PayloadKind::Alive,
                    b"buttons" => PayloadKind::Buttons,
                    b"icon" => PayloadKind::Icon,
                    b"close" => PayloadKind::Close,
                    _ => PayloadKind::Unsupported,
                };
            }
            b'd' => metadata.done = parse_bool(value)?,
            b'e' => metadata.encoded = parse_bool(value)?,
            b'f' => metadata.application_name = Some(decode_metadata_text(value)?),
            b't' => {
                if metadata.notification_types.len() >= MAX_NOTIFICATION_TYPES {
                    return Err(());
                }
                metadata
                    .notification_types
                    .push(decode_metadata_text(value)?);
            }
            b'n' => {
                if metadata.icon_names.len() >= MAX_NOTIFICATION_ICON_NAMES {
                    return Err(());
                }
                let name = decode_metadata_text(value)?;
                if name.starts_with('.') || name.contains('/') || name.contains('\\') {
                    return Err(());
                }
                metadata.icon_names.push(name);
            }
            b'g' => {
                if !valid_identifier(value) {
                    return Err(());
                }
                metadata.icon_cache_id = Some(String::from_utf8(value.to_vec()).map_err(|_| ())?);
            }
            b'u' => {
                metadata.urgency = Some(match value {
                    b"0" => NotificationUrgency::Low,
                    b"1" => NotificationUrgency::Normal,
                    b"2" => NotificationUrgency::Critical,
                    _ => return Err(()),
                });
            }
            b'w' => {
                metadata.expiry = Some(match value {
                    b"-1" => NotificationExpiry::System,
                    b"0" => NotificationExpiry::Never,
                    _ => {
                        if !value.iter().all(u8::is_ascii_digit) {
                            return Err(());
                        }
                        let text = std::str::from_utf8(value).map_err(|_| ())?;
                        let milliseconds = text.parse::<u32>().map_err(|_| ())?;
                        if milliseconds == 0 || milliseconds > i32::MAX as u32 {
                            return Err(());
                        }
                        NotificationExpiry::AfterMillis(milliseconds)
                    }
                });
            }
            b's' => {
                metadata.sound = Some(match decode_metadata_text(value)?.as_str() {
                    "system" => NotificationSound::System,
                    "silent" => NotificationSound::Silent,
                    "error" => NotificationSound::Error,
                    "warn" | "warning" => NotificationSound::Warning,
                    "info" => NotificationSound::Information,
                    "question" => NotificationSound::Question,
                    _ => return Err(()),
                });
            }
            b'c' => metadata.report_close = Some(parse_bool(value)?),
            b'a' => metadata.report_activation = Some(parse_activation_policy(value)?),
            b'o' => {
                metadata.occasion = Some(match value {
                    b"always" => NotificationOccasion::Always,
                    b"unfocused" => NotificationOccasion::Unfocused,
                    b"invisible" => NotificationOccasion::Invisible,
                    _ => return Err(()),
                });
            }
            // Unknown keys are deliberately ignored for protocol extension.
            _ => {}
        }
    }
    Ok(metadata)
}

fn identifier_hint(metadata: &[u8]) -> Option<String> {
    metadata
        .split(|byte| *byte == b':')
        .filter_map(|field| field.strip_prefix(b"i="))
        .find(|value| valid_identifier(value))
        .and_then(|value| String::from_utf8(value.to_vec()).ok())
}

fn parse_bool(value: &[u8]) -> Result<bool, ()> {
    match value {
        b"0" => Ok(false),
        b"1" => Ok(true),
        _ => Err(()),
    }
}

fn parse_activation_policy(value: &[u8]) -> Result<bool, ()> {
    let mut report = false;
    let mut report_seen = false;
    let mut focus_seen = false;
    for action in value.split(|byte| *byte == b',') {
        match action {
            b"report" | b"-report" if !report_seen => {
                report = action == b"report";
                report_seen = true;
            }
            b"focus" | b"-focus" if !focus_seen => {
                // Focus policy is parsed so otherwise valid applications are
                // not rejected, but it remains unadvertised and is not acted
                // on until native activation-token handling is implemented.
                focus_seen = true;
            }
            _ => return Err(()),
        }
    }
    Ok(report)
}

fn parse_buttons(bytes: Vec<u8>) -> Option<Vec<String>> {
    if bytes.is_empty() {
        return Some(Vec::new());
    }
    let text = String::from_utf8(bytes).ok()?;
    let mut buttons = Vec::new();
    for label in text.split('\u{2028}') {
        if label.is_empty()
            || label.len() > MAX_NOTIFICATION_BUTTON_LABEL_BYTES
            || label.chars().any(is_unsafe_plain_character)
            || buttons.len() >= MAX_NOTIFICATION_BUTTONS
        {
            return None;
        }
        buttons.push(label.to_owned());
    }
    Some(buttons)
}

fn valid_identifier(identifier: &[u8]) -> bool {
    !identifier.is_empty()
        && identifier.len() <= MAX_NOTIFICATION_IDENTIFIER_BYTES
        && identifier
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'+' | b'.'))
}

fn valid_metadata_value_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'-' | b'_'
                | b'/'
                | b'\\'
                | b'+'
                | b'.'
                | b','
                | b'('
                | b')'
                | b'{'
                | b'}'
                | b'['
                | b']'
                | b'*'
                | b'&'
                | b'^'
                | b'%'
                | b'$'
                | b'#'
                | b'@'
                | b'!'
                | b'`'
                | b'~'
                | b'?'
                | b'='
        )
}

fn decode_metadata_text(value: &[u8]) -> Result<String, ()> {
    if value.len() > MAX_NOTIFICATION_METADATA_VALUE_BYTES.saturating_mul(2)
        || !valid_base64_bytes(value)
    {
        return Err(());
    }
    let decoded = STANDARD
        .decode(value)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(value))
        .map_err(|_| ())?;
    if decoded.is_empty() || decoded.len() > MAX_NOTIFICATION_METADATA_VALUE_BYTES {
        return Err(());
    }
    let text = String::from_utf8(decoded).map_err(|_| ())?;
    if text.chars().any(is_unsafe_plain_character) {
        return Err(());
    }
    Ok(text)
}

fn append_unique_bounded(
    destination: &mut Vec<String>,
    incoming: Vec<String>,
    limit: usize,
) -> Result<(), ()> {
    for value in incoming {
        if destination.contains(&value) {
            continue;
        }
        if destination.len() >= limit {
            return Err(());
        }
        destination.push(value);
    }
    Ok(())
}

fn join_payload(parts: &[&[u8]], encoded: bool) -> Option<Vec<u8>> {
    let limit = if encoded {
        MAX_OSC99_ENCODED_CHUNK_BYTES
    } else {
        MAX_OSC99_DECODED_CHUNK_BYTES
    };
    let mut payload = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        let separator = usize::from(index > 0);
        if payload
            .len()
            .saturating_add(separator)
            .saturating_add(part.len())
            > limit
        {
            return None;
        }
        if index > 0 {
            payload.push(b';');
        }
        payload.extend_from_slice(part);
    }
    Some(payload)
}

fn valid_base64_bytes(bytes: &[u8]) -> bool {
    let padding_start = bytes
        .iter()
        .position(|byte| *byte == b'=')
        .unwrap_or(bytes.len());
    bytes[..padding_start]
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/'))
        && bytes[padding_start..].iter().all(|byte| *byte == b'=')
        && bytes.len().saturating_sub(padding_start) <= 2
}

fn is_unsafe_plain_character(character: char) -> bool {
    matches!(character as u32, 0x00..=0x1f | 0x7f..=0x9f)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts<'a>(metadata: &'a [u8], payload: &'a [u8]) -> [&'a [u8]; 2] {
        [metadata, payload]
    }

    #[test]
    fn simple_plain_and_base64_notifications_finish_exactly() {
        let mut protocol = NotificationProtocol::default();
        assert_eq!(
            protocol.handle(&parts(b"", b"hello")),
            Osc99Action::Notify(NotificationRequest {
                title: Some("hello".into()),
                body: String::new(),
                ..NotificationRequest::default()
            })
        );
        assert_eq!(
            protocol.handle(&parts(b"p=body:e=1", b"aMOpbGxvIPCfjLE")),
            Osc99Action::Notify(NotificationRequest {
                title: Some("héllo 🌱".into()),
                body: String::new(),
                ..NotificationRequest::default()
            })
        );
    }

    #[test]
    fn interleaved_identifiers_and_both_base64_chunk_styles_do_not_mix() {
        let mut protocol = NotificationProtocol::default();
        assert_eq!(
            protocol.handle(&parts(b"i=first:d=0:e=1", b"aGVs")),
            Osc99Action::None
        );
        assert_eq!(
            protocol.handle(&parts(b"i=second:d=0", b"other")),
            Osc99Action::None
        );
        assert_eq!(
            protocol.handle(&parts(b"i=first:d=0:e=1", b"bG8=")),
            Osc99Action::None
        );
        assert_eq!(
            protocol.handle(&parts(b"i=first:p=body:e=1", b"d29ybGQ=")),
            Osc99Action::Notify(NotificationRequest {
                title: Some("hello".into()),
                body: "world".into(),
                identifier: Some("first".into()),
                ..NotificationRequest::default()
            })
        );
        assert_eq!(
            protocol.handle(&parts(b"i=second", b" title")),
            Osc99Action::Notify(NotificationRequest {
                title: Some("other title".into()),
                body: String::new(),
                identifier: Some("second".into()),
                ..NotificationRequest::default()
            })
        );
    }

    #[test]
    fn rich_metadata_survives_chunking_and_is_typed() {
        let mut protocol = NotificationProtocol::default();
        assert_eq!(
            protocol.handle(&parts(
                b"i=job:d=0:f=YnVpbGQtYWdlbnQ=:t=YnVpbGQuY29tcGxldGU=:n=d2FybmluZw==:u=2",
                b"Build",
            )),
            Osc99Action::None
        );
        assert_eq!(
            protocol.handle(&parts(
                b"i=other:d=0:f=bW9uaXRvcg==:n=aW5mbw==:u=0",
                b"Other",
            )),
            Osc99Action::None
        );
        assert_eq!(
            protocol.handle(&parts(
                b"i=job:p=body:t=Y2k=:t=Y2k=:n=bXktYXBw",
                b"finished",
            )),
            Osc99Action::Notify(NotificationRequest {
                title: Some("Build".into()),
                body: "finished".into(),
                identifier: Some("job".into()),
                application_name: Some("build-agent".into()),
                notification_types: vec!["build.complete".into(), "ci".into()],
                icon_names: vec!["warning".into(), "my-app".into()],
                icon_cache_id: None,
                icon_data: None,
                urgency: Some(NotificationUrgency::Critical),
                expiry: NotificationExpiry::System,
                occasion: NotificationOccasion::Always,
                sound: NotificationSound::System,
                report_close: false,
                report_activation: false,
                buttons: Vec::new(),
                reply_uses_8bit_controls: false,
            })
        );
        assert_eq!(
            protocol.handle(&parts(b"i=other", b" done")),
            Osc99Action::Notify(NotificationRequest {
                title: Some("Other done".into()),
                body: String::new(),
                identifier: Some("other".into()),
                application_name: Some("monitor".into()),
                notification_types: Vec::new(),
                icon_names: vec!["info".into()],
                icon_cache_id: None,
                icon_data: None,
                urgency: Some(NotificationUrgency::Low),
                expiry: NotificationExpiry::System,
                occasion: NotificationOccasion::Always,
                sound: NotificationSound::System,
                report_close: false,
                report_activation: false,
                buttons: Vec::new(),
                reply_uses_8bit_controls: false,
            })
        );
    }

    #[test]
    fn identifiers_expiry_and_explicit_close_have_strict_lifecycle() {
        let mut protocol = NotificationProtocol::default();
        assert_eq!(
            protocol.handle(&parts(b"i=job:d=0:w=250", b"Build")),
            Osc99Action::None
        );
        assert_eq!(
            protocol.handle(&parts(b"i=job:p=body", b"done")),
            Osc99Action::Notify(NotificationRequest {
                title: Some("Build".into()),
                body: "done".into(),
                identifier: Some("job".into()),
                expiry: NotificationExpiry::AfterMillis(250),
                ..NotificationRequest::default()
            })
        );
        assert_eq!(
            protocol.handle(&parts(b"i=never:w=0", b"stay")),
            Osc99Action::Notify(NotificationRequest {
                title: Some("stay".into()),
                identifier: Some("never".into()),
                expiry: NotificationExpiry::Never,
                ..NotificationRequest::default()
            })
        );
        assert_eq!(
            protocol.handle(&parts(b"i=system:w=-1", b"default")),
            Osc99Action::Notify(NotificationRequest {
                title: Some("default".into()),
                identifier: Some("system".into()),
                ..NotificationRequest::default()
            })
        );

        assert_eq!(
            protocol.handle(&parts(b"i=pending:d=0", b"stale")),
            Osc99Action::None
        );
        assert_eq!(
            protocol.handle(&parts(b"i=pending:p=close", b"")),
            Osc99Action::Close("pending".into())
        );
        assert_eq!(protocol.pending_len(), 0);
        assert_eq!(protocol.handle(&parts(b"p=close", b"")), Osc99Action::None);
        assert_eq!(
            protocol.handle(&parts(b"i=invalid-close:d=0", b"stale")),
            Osc99Action::None
        );
        assert_eq!(
            protocol.handle(&parts(b"i=invalid-close:p=close", b"not-empty")),
            Osc99Action::None
        );
        assert_eq!(protocol.pending_len(), 0);
        assert_eq!(
            protocol.handle(&parts(b"i=invalid-close:d=0", b"stale")),
            Osc99Action::None
        );
        assert_eq!(
            protocol.handle(&parts(b"i=invalid-close:p=close:d=0", b"")),
            Osc99Action::None
        );
        assert_eq!(protocol.pending_len(), 0);
        assert_eq!(
            protocol.handle(&parts(b"i=bad:w=2147483648", b"ignored")),
            Osc99Action::None
        );
        assert_eq!(
            protocol.handle(&parts(b"i=bad:w=+1", b"ignored")),
            Osc99Action::None
        );
    }

    #[test]
    fn close_reporting_and_alive_queries_are_strict_and_typed() {
        let mut protocol = NotificationProtocol::default();
        assert_eq!(
            protocol.handle(&parts(b"i=job:d=0:c=1", b"Build")),
            Osc99Action::None
        );
        assert_eq!(
            protocol.handle(&parts(b"i=job:p=body", b"done")),
            Osc99Action::Notify(NotificationRequest {
                title: Some("Build".into()),
                body: "done".into(),
                identifier: Some("job".into()),
                report_close: true,
                ..NotificationRequest::default()
            })
        );
        assert_eq!(
            protocol.handle(&parts(b"i=probe:p=alive", b"")),
            Osc99Action::AliveQuery("probe".into())
        );
        assert_eq!(
            protocol.handle(&parts(b"p=alive", b"")),
            Osc99Action::AliveQuery("0".into())
        );
        assert_eq!(
            protocol.handle(&parts(b"i=probe:p=alive:d=0", b"")),
            Osc99Action::None
        );
        assert_eq!(
            protocol.handle(&parts(b"i=probe:p=alive", b"payload")),
            Osc99Action::None
        );
        assert_eq!(
            protocol.handle(&parts(b"i=bad:c=1:c=0", b"ignored")),
            Osc99Action::None
        );
        assert_eq!(
            protocol.handle(&parts(b"i=bad:c=2", b"ignored")),
            Osc99Action::None
        );
    }

    #[test]
    fn activation_policy_and_buttons_are_bounded_typed_and_chunkable() {
        let mut protocol = NotificationProtocol::default();
        assert_eq!(
            protocol.handle(&parts(
                b"i=actions:d=0:p=buttons:a=report,-focus",
                "Open\u{2028}Ignore".as_bytes(),
            )),
            Osc99Action::None
        );
        assert_eq!(
            protocol.handle(&parts(b"i=actions", b"Build finished")),
            Osc99Action::Notify(NotificationRequest {
                title: Some("Build finished".into()),
                identifier: Some("actions".into()),
                report_activation: true,
                buttons: vec!["Open".into(), "Ignore".into()],
                ..NotificationRequest::default()
            })
        );

        assert_eq!(
            protocol.handle(&parts(
                b"i=encoded:d=0:p=buttons:e=1:a=-focus,report",
                b"WWVz4oCoTm8=",
            )),
            Osc99Action::None
        );
        assert_eq!(
            protocol.handle(&parts(b"i=encoded", b"Question")),
            Osc99Action::Notify(NotificationRequest {
                title: Some("Question".into()),
                identifier: Some("encoded".into()),
                report_activation: true,
                buttons: vec!["Yes".into(), "No".into()],
                ..NotificationRequest::default()
            })
        );
        assert_eq!(
            protocol.handle(&parts(b"i=no-report:a=-report,focus", b"Quiet")),
            Osc99Action::Notify(NotificationRequest {
                title: Some("Quiet".into()),
                identifier: Some("no-report".into()),
                report_activation: false,
                ..NotificationRequest::default()
            })
        );

        for metadata in [
            b"i=bad:a=report,report".as_slice(),
            b"i=bad:a=report,-report".as_slice(),
            b"i=bad:a=unknown".as_slice(),
            b"i=bad:a=focus,-focus".as_slice(),
        ] {
            assert_eq!(
                protocol.handle(&parts(metadata, b"ignored")),
                Osc99Action::None
            );
        }
        assert_eq!(
            protocol.handle(&parts(b"i=empty:p=buttons:d=0", "One\u{2028}".as_bytes())),
            Osc99Action::None
        );
        assert_eq!(
            protocol.handle(&parts(b"i=empty", b"ignored")),
            Osc99Action::None
        );
        let too_many = (0..=MAX_NOTIFICATION_BUTTONS)
            .map(|index| format!("button-{index}"))
            .collect::<Vec<_>>()
            .join("\u{2028}");
        assert_eq!(
            protocol.handle(&parts(b"i=many:p=buttons:d=0", too_many.as_bytes())),
            Osc99Action::None
        );
        assert_eq!(
            protocol.handle(&parts(b"i=many", b"ignored")),
            Osc99Action::None
        );
        let oversized_label = "x".repeat(MAX_NOTIFICATION_BUTTON_LABEL_BYTES + 1);
        assert_eq!(
            protocol.handle(&parts(b"i=long:p=buttons:d=0", oversized_label.as_bytes(),)),
            Osc99Action::None
        );
        assert_eq!(
            protocol.handle(&parts(b"i=long", b"ignored")),
            Osc99Action::None
        );
        assert_eq!(
            protocol.handle(&parts(b"i=control:p=buttons:d=0:e=1", b"T25l4oCoVHdvCg==",)),
            Osc99Action::None
        );
        assert_eq!(
            protocol.handle(&parts(b"i=control", b"ignored")),
            Osc99Action::None
        );
    }

    #[test]
    fn sound_policy_is_typed_base64_only_and_bounded() {
        let cases = [
            ("c3lzdGVt", NotificationSound::System),
            ("c2lsZW50", NotificationSound::Silent),
            ("ZXJyb3I=", NotificationSound::Error),
            ("d2Fybg", NotificationSound::Warning),
            ("d2FybmluZw==", NotificationSound::Warning),
            ("aW5mbw==", NotificationSound::Information),
            ("cXVlc3Rpb24=", NotificationSound::Question),
        ];
        for (index, (encoded, sound)) in cases.into_iter().enumerate() {
            let mut protocol = NotificationProtocol::default();
            let metadata = format!("i=sound-{index}:s={encoded}");
            assert_eq!(
                protocol.handle(&parts(metadata.as_bytes(), b"Alert")),
                Osc99Action::Notify(NotificationRequest {
                    title: Some("Alert".into()),
                    identifier: Some(format!("sound-{index}")),
                    sound,
                    ..NotificationRequest::default()
                })
            );
        }

        let oversized = STANDARD.encode(vec![b'x'; MAX_NOTIFICATION_METADATA_VALUE_BYTES + 1]);
        for metadata in [
            "i=bad:s=system".to_string(),
            "i=bad:s=dW5rbm93bg==".to_string(),
            "i=bad:s=YmFkCg==".to_string(),
            "i=bad:s=c3lzdGVt:s=c2lsZW50".to_string(),
            format!("i=bad:s={oversized}"),
        ] {
            let mut protocol = NotificationProtocol::default();
            assert_eq!(
                protocol.handle(&parts(metadata.as_bytes(), b"ignored")),
                Osc99Action::None
            );
        }
    }

    #[test]
    fn occasion_policy_is_typed_fragmented_and_fail_closed() {
        let mut protocol = NotificationProtocol::default();
        assert_eq!(
            protocol.handle(&parts(b"i=hidden:d=0:o=invisible", b"Build")),
            Osc99Action::None
        );
        assert_eq!(
            protocol.handle(&parts(b"i=other:d=0:o=unfocused", b"Other")),
            Osc99Action::None
        );
        assert_eq!(
            protocol.handle(&parts(b"i=hidden:p=body", b"done")),
            Osc99Action::Notify(NotificationRequest {
                title: Some("Build".into()),
                body: "done".into(),
                identifier: Some("hidden".into()),
                occasion: NotificationOccasion::Invisible,
                ..NotificationRequest::default()
            })
        );
        assert_eq!(
            protocol.handle(&parts(b"i=other", b" ready")),
            Osc99Action::Notify(NotificationRequest {
                title: Some("Other ready".into()),
                identifier: Some("other".into()),
                occasion: NotificationOccasion::Unfocused,
                ..NotificationRequest::default()
            })
        );
        assert_eq!(
            protocol.handle(&parts(b"o=always", b"Always")),
            Osc99Action::Notify(NotificationRequest {
                title: Some("Always".into()),
                ..NotificationRequest::default()
            })
        );

        for metadata in [
            b"i=bad:o=unknown".as_slice(),
            b"i=bad:o=always:o=unfocused".as_slice(),
            b"i=bad:o=".as_slice(),
        ] {
            let mut rejected = NotificationProtocol::default();
            assert_eq!(
                rejected.handle(&parts(b"i=bad:d=0", b"stale")),
                Osc99Action::None
            );
            assert_eq!(
                rejected.handle(&parts(metadata, b"ignored")),
                Osc99Action::None
            );
            assert_eq!(
                rejected.handle(&parts(b"i=bad", b"fresh")),
                Osc99Action::Notify(NotificationRequest {
                    title: Some("fresh".into()),
                    identifier: Some("bad".into()),
                    ..NotificationRequest::default()
                })
            );
        }
    }

    #[test]
    fn transmitted_icons_are_encoded_bounded_and_cache_identified() {
        let mut protocol = NotificationProtocol::default();
        assert_eq!(
            protocol.handle(&parts(b"i=icon:d=0:p=icon:e=1:g=shared-icon", b"AA")),
            Osc99Action::None
        );
        assert_eq!(
            protocol.handle(&parts(b"i=icon:d=0:p=icon:e=1", b"EC")),
            Osc99Action::None
        );
        assert_eq!(
            protocol.handle(&parts(b"i=icon", b"With icon")),
            Osc99Action::Notify(NotificationRequest {
                title: Some("With icon".into()),
                identifier: Some("icon".into()),
                icon_cache_id: Some("shared-icon".into()),
                icon_data: Some(vec![0, 1, 2].into_boxed_slice()),
                ..NotificationRequest::default()
            })
        );

        let mut padded = NotificationProtocol::default();
        assert_eq!(
            padded.handle(&parts(b"i=padded:d=0:p=icon:e=1:g=cached", b"AA==")),
            Osc99Action::None
        );
        assert_eq!(
            padded.handle(&parts(b"i=padded:d=0:p=icon:e=1", b"AQ==")),
            Osc99Action::None
        );
        assert_eq!(
            padded.handle(&parts(b"i=padded", b"Padded")),
            Osc99Action::Notify(NotificationRequest {
                title: Some("Padded".into()),
                identifier: Some("padded".into()),
                icon_cache_id: Some("cached".into()),
                icon_data: Some(vec![0, 1].into_boxed_slice()),
                ..NotificationRequest::default()
            })
        );

        let mut cached = NotificationProtocol::default();
        assert_eq!(
            cached.handle(&parts(b"i=reuse:g=shared-icon", b"Reuse")),
            Osc99Action::Notify(NotificationRequest {
                title: Some("Reuse".into()),
                identifier: Some("reuse".into()),
                icon_cache_id: Some("shared-icon".into()),
                ..NotificationRequest::default()
            })
        );

        for metadata in [
            b"i=plain:d=0:p=icon".as_slice(),
            b"i=bad-g:d=0:p=icon:e=1:g=../escape".as_slice(),
            b"i=path-name:d=0:n=L3RtcC9pY29uLnBuZw==".as_slice(),
        ] {
            let mut rejected = NotificationProtocol::default();
            assert_eq!(
                rejected.handle(&parts(metadata, b"AA==")),
                Osc99Action::None
            );
        }

        let oversized = vec![0_u8; MAX_NOTIFICATION_ICON_BYTES + 1];
        let oversized = STANDARD.encode(oversized);
        let mut rejected = NotificationProtocol::default();
        assert_eq!(
            rejected.handle(&parts(b"i=large:d=0:p=icon:e=1", oversized.as_bytes(),)),
            Osc99Action::None
        );
    }

    #[test]
    fn transmitted_icon_aggregate_byte_limit_is_exact() {
        let exact_encoded = STANDARD.encode(vec![0_u8; MAX_NOTIFICATION_ICON_BYTES]);
        let mut exact = PendingField::default();
        for chunk in exact_encoded
            .as_bytes()
            .chunks(MAX_OSC99_ENCODED_CHUNK_BYTES)
        {
            exact
                .append_encoded(chunk, false, MAX_NOTIFICATION_ICON_BYTES)
                .unwrap();
        }
        exact.finish_encoded(MAX_NOTIFICATION_ICON_BYTES).unwrap();
        assert_eq!(exact.decoded.len(), MAX_NOTIFICATION_ICON_BYTES);

        let mut protocol = NotificationProtocol::default();
        for chunk in exact_encoded
            .as_bytes()
            .chunks(MAX_OSC99_ENCODED_CHUNK_BYTES)
        {
            assert_eq!(
                protocol.handle(&parts(b"i=exact:d=0:p=icon:e=1:g=exact-cache", chunk)),
                Osc99Action::None
            );
        }
        match protocol.handle(&parts(b"i=exact", b"Exact limit")) {
            Osc99Action::Notify(request) => {
                assert_eq!(request.icon_cache_id.as_deref(), Some("exact-cache"));
                assert_eq!(
                    request.icon_data.as_deref().map(<[u8]>::len),
                    Some(MAX_NOTIFICATION_ICON_BYTES)
                );
            }
            action => panic!("expected exact-limit notification, got {action:?}"),
        }

        let oversized_encoded = STANDARD.encode(vec![0_u8; MAX_NOTIFICATION_ICON_BYTES + 1]);
        let mut oversized = PendingField::default();
        let mut rejected = false;
        for chunk in oversized_encoded
            .as_bytes()
            .chunks(MAX_OSC99_ENCODED_CHUNK_BYTES)
        {
            if oversized
                .append_encoded(chunk, false, MAX_NOTIFICATION_ICON_BYTES)
                .is_err()
            {
                rejected = true;
                break;
            }
        }
        assert!(
            rejected
                || oversized
                    .finish_encoded(MAX_NOTIFICATION_ICON_BYTES)
                    .is_err()
        );
    }

    #[test]
    fn malformed_or_over_limit_metadata_retires_only_its_assembly() {
        let mut protocol = NotificationProtocol::default();
        assert_eq!(
            protocol.handle(&parts(b"i=bad:d=0:t=b25l", b"stale")),
            Osc99Action::None
        );
        for index in 2..=MAX_NOTIFICATION_TYPES {
            let metadata = format!("i=bad:d=0:t={}", STANDARD.encode(format!("type-{index}")));
            assert_eq!(
                protocol.handle(&parts(metadata.as_bytes(), b"")),
                Osc99Action::None
            );
        }
        assert_eq!(protocol.pending_len(), 1);
        assert_eq!(
            protocol.handle(&parts(b"i=bad:d=0:t=b3ZlcmZsb3c=", b"")),
            Osc99Action::None
        );
        assert_eq!(protocol.pending_len(), 0);
        assert_eq!(
            protocol.handle(&parts(b"i=bad", b"fresh")),
            Osc99Action::Notify(NotificationRequest {
                title: Some("fresh".into()),
                body: String::new(),
                identifier: Some("bad".into()),
                ..NotificationRequest::default()
            })
        );

        assert_eq!(
            protocol.handle(&parts(b"i=duplicate:f=YQ==:f=Yg==", b"ignored")),
            Osc99Action::None
        );
        assert_eq!(
            protocol.handle(&parts(b"i=control:f=Cg==", b"ignored")),
            Osc99Action::None
        );
        assert_eq!(
            protocol.handle(&parts(b"i=urgency:u=3", b"ignored")),
            Osc99Action::None
        );

        let oversized = STANDARD.encode(vec![b'x'; MAX_NOTIFICATION_METADATA_VALUE_BYTES + 1]);
        let metadata = format!("i=large:f={oversized}");
        assert_eq!(
            protocol.handle(&parts(metadata.as_bytes(), b"ignored")),
            Osc99Action::None
        );
        assert_eq!(protocol.pending_len(), 0);
    }

    #[test]
    fn malformed_continuation_retires_only_its_identifier() {
        let mut protocol = NotificationProtocol::default();
        assert_eq!(
            protocol.handle(&parts(b"i=bad:d=0", b"stale")),
            Osc99Action::None
        );
        assert_eq!(
            protocol.handle(&parts(b"i=good:d=0", b"kept")),
            Osc99Action::None
        );
        assert_eq!(protocol.pending_len(), 2);
        assert_eq!(
            protocol.handle(&parts(b"i=bad:e=1", b"%%%")),
            Osc99Action::None
        );
        assert_eq!(protocol.pending_len(), 1);
        assert_eq!(
            protocol.handle(&parts(b"i=bad", b"fresh")),
            Osc99Action::Notify(NotificationRequest {
                title: Some("fresh".into()),
                body: String::new(),
                identifier: Some("bad".into()),
                ..NotificationRequest::default()
            })
        );
        assert_eq!(
            protocol.handle(&parts(b"i=good", b" value")),
            Osc99Action::Notify(NotificationRequest {
                title: Some("kept value".into()),
                body: String::new(),
                identifier: Some("good".into()),
                ..NotificationRequest::default()
            })
        );
    }

    #[test]
    fn pending_count_and_bytes_are_hard_bounded() {
        let mut protocol = NotificationProtocol::default();
        let payload = vec![b'x'; MAX_OSC99_DECODED_CHUNK_BYTES];
        for index in 0..=MAX_PENDING_NOTIFICATIONS {
            let metadata = format!("i=item-{index}:p=body:d=0");
            assert_eq!(
                protocol.handle(&parts(metadata.as_bytes(), &payload)),
                Osc99Action::None
            );
        }
        assert_eq!(protocol.pending_len(), MAX_PENDING_NOTIFICATIONS);
        assert!(protocol.pending_bytes() <= MAX_PENDING_NOTIFICATION_BYTES);
        assert_eq!(
            protocol.handle(&parts(b"i=item-0", b"gone")),
            Osc99Action::Notify(NotificationRequest {
                title: Some("gone".into()),
                body: String::new(),
                identifier: Some("item-0".into()),
                ..NotificationRequest::default()
            })
        );

        let mut aggregate = NotificationProtocol::default();
        for index in 0..MAX_PENDING_NOTIFICATIONS {
            let metadata = format!("i=large-{index}:p=body:d=0");
            for _ in 0..MAX_NOTIFICATION_BODY_BYTES / MAX_OSC99_DECODED_CHUNK_BYTES {
                assert_eq!(
                    aggregate.handle(&parts(metadata.as_bytes(), &payload)),
                    Osc99Action::None
                );
            }
        }
        assert!(aggregate.pending_len() < MAX_PENDING_NOTIFICATIONS);
        assert!(aggregate.pending_bytes() <= MAX_PENDING_NOTIFICATION_BYTES);
    }

    #[test]
    fn malformed_controls_metadata_and_field_overflow_fail_closed() {
        let mut protocol = NotificationProtocol::default();
        assert_eq!(
            protocol.handle(&parts(b"", b"line\nfeed")),
            Osc99Action::None
        );
        assert_eq!(
            protocol.handle(&parts(b"i=bad=:p=?", b"")),
            Osc99Action::None
        );
        assert_eq!(
            protocol.handle(&parts(b"c=2", b"no-close-report")),
            Osc99Action::None
        );
        assert_eq!(
            protocol.handle(&parts(b"o=sometimes", b"unsupported-filter")),
            Osc99Action::None
        );

        let chunk = vec![b'x'; MAX_OSC99_DECODED_CHUNK_BYTES];
        for _ in 0..MAX_NOTIFICATION_TITLE_BYTES / chunk.len() {
            assert_eq!(
                protocol.handle(&parts(b"i=overflow:d=0", &chunk)),
                Osc99Action::None
            );
        }
        assert_eq!(
            protocol.handle(&parts(b"i=overflow", b"x")),
            Osc99Action::None
        );
        assert_eq!(protocol.pending_len(), 0);
    }
}
