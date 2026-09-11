//! Bounded terminal-local state for Kitty's OSC 99 notification protocol.

use crate::event::{NotificationRequest, NotificationUrgency};
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
pub const MAX_OSC99_DECODED_CHUNK_BYTES: usize = 2 * 1024;
pub const MAX_OSC99_ENCODED_CHUNK_BYTES: usize = 4 * 1024;
const MAX_OSC99_METADATA_BYTES: usize = 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PayloadKind {
    #[default]
    Title,
    Body,
    Query,
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
    urgency: Option<NotificationUrgency>,
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
        }
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
    identifier: String,
    title: PendingField,
    body: PendingField,
    application_name: Option<String>,
    notification_types: Vec<String>,
    icon_names: Vec<String>,
    urgency: Option<NotificationUrgency>,
}

impl PendingNotification {
    fn retained_bytes(&self) -> usize {
        self.identifier.len()
            + self.title.retained_bytes()
            + self.body.retained_bytes()
            + self.application_name.as_ref().map_or(0, String::len)
            + self
                .notification_types
                .iter()
                .map(String::len)
                .sum::<usize>()
            + self.icon_names.iter().map(String::len).sum::<usize>()
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
        if metadata.urgency.is_some() {
            self.urgency = metadata.urgency;
        }
        Ok(())
    }

    fn finish(mut self) -> Option<NotificationRequest> {
        self.title
            .finish_encoded(MAX_NOTIFICATION_TITLE_BYTES)
            .ok()?;
        self.body.finish_encoded(MAX_NOTIFICATION_BODY_BYTES).ok()?;
        let mut title = String::from_utf8(self.title.decoded).ok()?;
        let mut body = String::from_utf8(self.body.decoded).ok()?;
        if title.is_empty() {
            if body.is_empty() {
                return None;
            }
            title = std::mem::take(&mut body);
        }
        Some(NotificationRequest {
            title: Some(title),
            body,
            application_name: self.application_name,
            notification_types: self.notification_types,
            icon_names: self.icon_names,
            urgency: self.urgency,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Osc99Action {
    None,
    Query(String),
    Notify(NotificationRequest),
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
        if !matches!(metadata.payload, PayloadKind::Title | PayloadKind::Body) {
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
                identifier: identifier.unwrap_or_default(),
                ..PendingNotification::default()
            });
        if notification.apply_metadata(metadata).is_err() {
            return Osc99Action::None;
        }
        let (field, limit) = match payload_kind {
            PayloadKind::Title => (&mut notification.title, MAX_NOTIFICATION_TITLE_BYTES),
            PayloadKind::Body => (&mut notification.body, MAX_NOTIFICATION_BODY_BYTES),
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
            .position(|notification| notification.identifier == identifier)?;
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
            b'i' | b'p' | b'd' | b'e' | b'c' | b'o' | b'a' | b'f' | b'u'
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
                metadata.icon_names.push(decode_metadata_text(value)?);
            }
            b'u' => {
                metadata.urgency = Some(match value {
                    b"0" => NotificationUrgency::Low,
                    b"1" => NotificationUrgency::Normal,
                    b"2" => NotificationUrgency::Critical,
                    _ => return Err(()),
                });
            }
            b'c' if value == b"0" => {}
            b'c' => return Err(()),
            b'o' if value == b"always" => {}
            b'o' => return Err(()),
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
                ..NotificationRequest::default()
            })
        );
        assert_eq!(
            protocol.handle(&parts(b"i=second", b" title")),
            Osc99Action::Notify(NotificationRequest {
                title: Some("other title".into()),
                body: String::new(),
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
                application_name: Some("build-agent".into()),
                notification_types: vec!["build.complete".into(), "ci".into()],
                icon_names: vec!["warning".into(), "my-app".into()],
                urgency: Some(NotificationUrgency::Critical),
            })
        );
        assert_eq!(
            protocol.handle(&parts(b"i=other", b" done")),
            Osc99Action::Notify(NotificationRequest {
                title: Some("Other done".into()),
                body: String::new(),
                application_name: Some("monitor".into()),
                notification_types: Vec::new(),
                icon_names: vec!["info".into()],
                urgency: Some(NotificationUrgency::Low),
            })
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
                ..NotificationRequest::default()
            })
        );
        assert_eq!(
            protocol.handle(&parts(b"i=good", b" value")),
            Osc99Action::Notify(NotificationRequest {
                title: Some("kept value".into()),
                body: String::new(),
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
            protocol.handle(&parts(b"c=1", b"no-close-report")),
            Osc99Action::None
        );
        assert_eq!(
            protocol.handle(&parts(b"o=unfocused", b"unsupported-filter")),
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
