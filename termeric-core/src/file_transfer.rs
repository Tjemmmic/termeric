//! Bounded Kitty OSC 5113 file-transfer wire types.
//!
//! Filesystem authority belongs to the embedding application. The core only
//! validates and decodes one command at a time, captures reply framing, and
//! provides a canonical status serializer.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::collections::HashSet;

pub(crate) const MAX_KITTY_TRANSFER_ID_BYTES: usize = 64;
pub const MAX_KITTY_TRANSFER_FILE_ID_BYTES: usize = 64;
pub const MAX_KITTY_TRANSFER_PATH_BYTES: usize = 4_096;
pub(crate) const MAX_KITTY_TRANSFER_STATUS_BYTES: usize = 1_024;
pub const MAX_KITTY_TRANSFER_DATA_BYTES: usize = 4_096;
pub(crate) const MAX_KITTY_TRANSFER_OSC_BYTES: usize = 16 * 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KittyFileTransferAction {
    Send,
    File,
    Data,
    EndData,
    Receive,
    Cancel,
    Status,
    Finish,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum KittyFileTransferCompression {
    #[default]
    None,
    Zlib,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum KittyFileTransferType {
    #[default]
    Regular,
    Directory,
    Symlink,
    HardLink,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum KittyFileTransmissionType {
    #[default]
    Simple,
    Rsync,
}

#[derive(Clone, PartialEq, Eq)]
pub struct KittyFileTransferCommand {
    pub action: KittyFileTransferAction,
    pub compression: KittyFileTransferCompression,
    pub file_type: KittyFileTransferType,
    pub transmission_type: KittyFileTransmissionType,
    pub id: String,
    pub file_id: String,
    /// Kitty quiet level. Zero requests success and error acknowledgements.
    pub quiet: u8,
    pub mtime_ns: Option<i64>,
    pub permissions: Option<u32>,
    pub size: Option<i64>,
    pub name: Option<String>,
    pub status: Option<String>,
    pub parent: Option<String>,
    pub data: Box<[u8]>,
    /// Whether a bypass credential was supplied. Its decoded value is never
    /// retained or exposed beyond the parser.
    pub bypass_present: bool,
    pub reply_uses_8bit_controls: bool,
}

impl std::fmt::Debug for KittyFileTransferCommand {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("KittyFileTransferCommand")
            .field("action", &self.action)
            .field("compression", &self.compression)
            .field("file_type", &self.file_type)
            .field("transmission_type", &self.transmission_type)
            .field("id", &self.id)
            .field("file_id", &self.file_id)
            .field("quiet", &self.quiet)
            .field("mtime_ns", &self.mtime_ns)
            .field("permissions", &self.permissions)
            .field("size", &self.size)
            .field("name_present", &self.name.is_some())
            .field("status", &self.status)
            .field("parent", &self.parent)
            .field("data_bytes", &self.data.len())
            .field("bypass_present", &self.bypass_present)
            .field("reply_uses_8bit_controls", &self.reply_uses_8bit_controls)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KittyFileTransferParseError {
    pub id: Option<String>,
    pub file_id: Option<String>,
    pub message: String,
}

impl KittyFileTransferParseError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            id: None,
            file_id: None,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KittyFileTransferStatusCode {
    Ok,
    Started,
    Canceled,
    Progress,
    Invalid,
    PermissionDenied,
    AlreadyExists,
    TooLarge,
    Busy,
    Io,
    TimedOut,
}

impl KittyFileTransferStatusCode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Started => "STARTED",
            Self::Canceled => "CANCELED",
            Self::Progress => "PROGRESS",
            Self::Invalid => "EINVAL",
            Self::PermissionDenied => "EPERM",
            Self::AlreadyExists => "EEXIST",
            Self::TooLarge => "EFBIG",
            Self::Busy => "EBUSY",
            Self::Io => "EIO",
            Self::TimedOut => "ETIMEDOUT",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KittyFileTransferStatus {
    pub id: String,
    pub file_id: String,
    pub code: KittyFileTransferStatusCode,
    pub message: Option<String>,
    pub name: Option<String>,
    pub size: Option<i64>,
    pub transmission_type: KittyFileTransmissionType,
}

impl KittyFileTransferStatus {
    pub fn canonical_reply(&self) -> Vec<u8> {
        let mut fields = vec!["ac=status".to_string(), format!("id={}", self.id)];
        if !self.file_id.is_empty() {
            fields.push(format!("fid={}", self.file_id));
        }
        if self.transmission_type == KittyFileTransmissionType::Rsync {
            fields.push("tt=rsync".into());
        }
        if let Some(size) = self.size {
            fields.push(format!("sz={size}"));
        }
        if let Some(name) = self.name.as_deref() {
            fields.push(format!("n={}", STANDARD.encode(name.as_bytes())));
        }
        let mut status = self.code.as_str().to_string();
        if let Some(message) = self.message.as_deref().filter(|value| !value.is_empty()) {
            status.push(':');
            status.extend(
                message
                    .chars()
                    .filter(|character| !character.is_control())
                    .take(512),
            );
        }
        fields.push(format!("st={}", STANDARD.encode(status.as_bytes())));
        format!("\x1b]5113;{}\x1b\\", fields.join(";")).into_bytes()
    }
}

/// A terminal-to-child OSC 5113 command used by consented host reads.
///
/// Debug output intentionally reports only sizes and field presence so file
/// names and payload bytes cannot leak through ordinary diagnostics.
#[derive(Clone, PartialEq, Eq)]
pub enum KittyFileTransferReply {
    File {
        id: String,
        file_id: String,
        actual_file_id: String,
        name: String,
        mtime_ns: i64,
        permissions: u32,
        size: u64,
    },
    Data {
        id: String,
        file_id: String,
        data: Box<[u8]>,
        last: bool,
    },
}

impl std::fmt::Debug for KittyFileTransferReply {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::File {
                id,
                file_id,
                actual_file_id,
                name,
                mtime_ns,
                permissions,
                size,
            } => formatter
                .debug_struct("KittyFileTransferReply::File")
                .field("id", id)
                .field("file_id", file_id)
                .field("actual_file_id", actual_file_id)
                .field("name_bytes", &name.len())
                .field("mtime_ns", mtime_ns)
                .field("permissions", permissions)
                .field("size", size)
                .finish(),
            Self::Data {
                id,
                file_id,
                data,
                last,
            } => formatter
                .debug_struct("KittyFileTransferReply::Data")
                .field("id", id)
                .field("file_id", file_id)
                .field("data_bytes", &data.len())
                .field("last", last)
                .finish(),
        }
    }
}

impl KittyFileTransferReply {
    pub fn canonical_reply(&self) -> Vec<u8> {
        let fields = match self {
            Self::File {
                id,
                file_id,
                actual_file_id,
                name,
                mtime_ns,
                permissions,
                size,
            } => vec![
                "ac=file".to_string(),
                format!("id={id}"),
                format!("fid={file_id}"),
                format!("mod={mtime_ns}"),
                format!("prm={permissions}"),
                format!("sz={size}"),
                format!("n={}", STANDARD.encode(name.as_bytes())),
                format!("st={}", STANDARD.encode(actual_file_id.as_bytes())),
            ],
            Self::Data {
                id,
                file_id,
                data,
                last,
            } => vec![
                format!("ac={}", if *last { "end_data" } else { "data" }),
                format!("id={id}"),
                format!("fid={file_id}"),
                format!("d={}", STANDARD.encode(data)),
            ],
        };
        format!("\x1b]5113;{}\x1b\\", fields.join(";")).into_bytes()
    }
}

pub(crate) fn parse_kitty_file_transfer(
    params: &[&[u8]],
    reply_uses_8bit_controls: bool,
) -> Result<KittyFileTransferCommand, KittyFileTransferParseError> {
    let total = params
        .iter()
        .try_fold(0usize, |total, value| total.checked_add(value.len() + 1))
        .ok_or_else(|| KittyFileTransferParseError::new("command is too large"))?;
    if total > MAX_KITTY_TRANSFER_OSC_BYTES {
        return Err(KittyFileTransferParseError::new("command is too large"));
    }

    let mut action = None;
    let mut compression = KittyFileTransferCompression::None;
    let mut file_type = KittyFileTransferType::Regular;
    let mut transmission_type = KittyFileTransmissionType::Simple;
    let mut id = None;
    let mut file_id = None;
    let mut quiet = 0;
    let mut mtime_ns = None;
    let mut permissions = None;
    let mut size = None;
    let mut name = None;
    let mut status = None;
    let mut parent = None;
    let mut data = Box::<[u8]>::default();
    let mut bypass_present = false;
    let mut seen = HashSet::new();

    for field in params {
        let Some(split) = field.iter().position(|byte| *byte == b'=') else {
            continue;
        };
        let key = &field[..split];
        let value = &field[split + 1..];
        let known = matches!(
            key,
            b"ac"
                | b"zip"
                | b"ft"
                | b"tt"
                | b"id"
                | b"fid"
                | b"pw"
                | b"q"
                | b"mod"
                | b"prm"
                | b"sz"
                | b"n"
                | b"st"
                | b"pr"
                | b"d"
        );
        if known && !seen.insert(key.to_vec()) {
            return Err(parse_error_with_ids(
                &id,
                &file_id,
                format!("duplicate field {}", String::from_utf8_lossy(key)),
            ));
        }
        match key {
            b"ac" => {
                action = Some(match value {
                    b"send" => KittyFileTransferAction::Send,
                    b"file" => KittyFileTransferAction::File,
                    b"data" => KittyFileTransferAction::Data,
                    b"end_data" => KittyFileTransferAction::EndData,
                    b"receive" => KittyFileTransferAction::Receive,
                    b"cancel" => KittyFileTransferAction::Cancel,
                    b"status" => KittyFileTransferAction::Status,
                    b"finish" => KittyFileTransferAction::Finish,
                    _ => {
                        return Err(parse_error_with_ids(&id, &file_id, "unknown action"));
                    }
                });
            }
            b"zip" => {
                compression = match value {
                    b"none" => KittyFileTransferCompression::None,
                    b"zlib" => KittyFileTransferCompression::Zlib,
                    _ => {
                        return Err(parse_error_with_ids(&id, &file_id, "unknown compression"));
                    }
                };
            }
            b"ft" => {
                file_type = match value {
                    b"regular" => KittyFileTransferType::Regular,
                    b"directory" => KittyFileTransferType::Directory,
                    b"symlink" => KittyFileTransferType::Symlink,
                    b"link" => KittyFileTransferType::HardLink,
                    _ => {
                        return Err(parse_error_with_ids(&id, &file_id, "unknown file type"));
                    }
                };
            }
            b"tt" => {
                transmission_type = match value {
                    b"simple" => KittyFileTransmissionType::Simple,
                    b"rsync" => KittyFileTransmissionType::Rsync,
                    _ => {
                        return Err(parse_error_with_ids(
                            &id,
                            &file_id,
                            "unknown transmission type",
                        ));
                    }
                };
            }
            b"id" => {
                id = Some(
                    parse_safe_id(value, MAX_KITTY_TRANSFER_ID_BYTES, "id")
                        .map_err(|error| contextualize(error, &id, &file_id))?,
                )
            }
            b"fid" => {
                file_id = Some(
                    parse_safe_id(value, MAX_KITTY_TRANSFER_FILE_ID_BYTES, "file id")
                        .map_err(|error| contextualize(error, &id, &file_id))?,
                )
            }
            b"pw" => {
                if value.len() > 4_096 {
                    return Err(parse_error_with_ids(&id, &file_id, "bypass is too large"));
                }
                bypass_present = !value.is_empty();
            }
            b"q" => {
                quiet = parse_number::<u8>(value, "quiet level")
                    .map_err(|error| contextualize(error, &id, &file_id))?;
                if quiet > 2 {
                    return Err(parse_error_with_ids(
                        &id,
                        &file_id,
                        "quiet level must be 0, 1, or 2",
                    ));
                }
            }
            b"mod" => {
                mtime_ns = Some(
                    parse_number::<i64>(value, "mtime")
                        .map_err(|error| contextualize(error, &id, &file_id))?,
                )
            }
            b"prm" => {
                permissions = Some(
                    parse_number::<u32>(value, "permissions")
                        .map_err(|error| contextualize(error, &id, &file_id))?,
                )
            }
            b"sz" => {
                size = Some(
                    parse_number::<i64>(value, "size")
                        .map_err(|error| contextualize(error, &id, &file_id))?,
                )
            }
            b"n" => {
                name = Some(
                    parse_base64_text(value, MAX_KITTY_TRANSFER_PATH_BYTES, "name")
                        .map_err(|error| contextualize(error, &id, &file_id))?,
                )
            }
            b"st" => {
                status = Some(
                    parse_base64_text(value, MAX_KITTY_TRANSFER_STATUS_BYTES, "status")
                        .map_err(|error| contextualize(error, &id, &file_id))?,
                )
            }
            b"pr" => {
                parent = Some(
                    parse_safe_id(value, MAX_KITTY_TRANSFER_FILE_ID_BYTES, "parent")
                        .map_err(|error| contextualize(error, &id, &file_id))?,
                )
            }
            b"d" => {
                data = decode_base64(value, MAX_KITTY_TRANSFER_DATA_BYTES, "data")
                    .map_err(|error| contextualize(error, &id, &file_id))?
                    .into();
            }
            _ => {}
        }
    }

    let id = id.ok_or_else(|| KittyFileTransferParseError::new("missing session id"))?;
    if id.is_empty() {
        return Err(KittyFileTransferParseError::new("empty session id"));
    }
    let action = action
        .ok_or_else(|| parse_error_with_ids(&Some(id.clone()), &file_id, "missing action"))?;
    Ok(KittyFileTransferCommand {
        action,
        compression,
        file_type,
        transmission_type,
        id,
        file_id: file_id.unwrap_or_default(),
        quiet,
        mtime_ns,
        permissions,
        size,
        name,
        status,
        parent,
        data,
        bypass_present,
        reply_uses_8bit_controls,
    })
}

fn contextualize(
    mut error: KittyFileTransferParseError,
    id: &Option<String>,
    file_id: &Option<String>,
) -> KittyFileTransferParseError {
    error.id.clone_from(id);
    error.file_id.clone_from(file_id);
    error
}

fn parse_error_with_ids(
    id: &Option<String>,
    file_id: &Option<String>,
    message: impl Into<String>,
) -> KittyFileTransferParseError {
    KittyFileTransferParseError {
        id: id.clone(),
        file_id: file_id.clone(),
        message: message.into(),
    }
}

/// Whether `value` is a transfer identifier Termeric accepts: at most `limit`
/// bytes from a conservative ASCII set, so it is safe to echo in replies and
/// to use as a metadata node key.
pub fn is_safe_transfer_id(value: &[u8], limit: usize) -> bool {
    value.len() <= limit
        && value.iter().all(|byte| {
            matches!(
                byte,
                b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z' | b'_' | b':' | b'.' | b'/' | b'@' | b'-'
            )
        })
}

fn parse_safe_id(
    value: &[u8],
    limit: usize,
    label: &str,
) -> Result<String, KittyFileTransferParseError> {
    if !is_safe_transfer_id(value, limit) {
        return Err(KittyFileTransferParseError::new(format!("invalid {label}")));
    }
    Ok(String::from_utf8(value.to_vec()).expect("safe id is ASCII"))
}

fn parse_number<T>(value: &[u8], label: &str) -> Result<T, KittyFileTransferParseError>
where
    T: std::str::FromStr,
{
    std::str::from_utf8(value)
        .ok()
        .and_then(|value| value.parse::<T>().ok())
        .ok_or_else(|| KittyFileTransferParseError::new(format!("invalid {label}")))
}

fn parse_base64_text(
    value: &[u8],
    limit: usize,
    label: &str,
) -> Result<String, KittyFileTransferParseError> {
    let decoded = decode_base64(value, limit, label)?;
    String::from_utf8(decoded)
        .map_err(|_| KittyFileTransferParseError::new(format!("{label} is not UTF-8")))
}

fn decode_base64(
    value: &[u8],
    limit: usize,
    label: &str,
) -> Result<Vec<u8>, KittyFileTransferParseError> {
    if value.len() > limit.saturating_add(2) / 3 * 4 + 4 {
        return Err(KittyFileTransferParseError::new(format!(
            "{label} is too large"
        )));
    }
    let decoded = STANDARD
        .decode(value)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(value))
        .map_err(|_| KittyFileTransferParseError::new(format!("invalid {label} base64")))?;
    if decoded.len() > limit {
        return Err(KittyFileTransferParseError::new(format!(
            "{label} is too large"
        )));
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(value: &[u8]) -> Vec<&[u8]> {
        value.split(|byte| *byte == b';').collect()
    }

    #[test]
    fn parses_signed_modification_times_without_losing_nanoseconds() {
        for timestamp in [i64::MIN, -1_000_000_001, -1, 0, 1, i64::MAX] {
            let input = format!("ac=file;id=session;fid=f;mod={timestamp}");
            let command = parse_kitty_file_transfer(&fields(input.as_bytes()), false).unwrap();
            assert_eq!(command.mtime_ns, Some(timestamp));
        }
    }

    #[test]
    fn parses_complete_bounded_command_and_forgets_bypass_secret() {
        let name = STANDARD.encode("/tmp/root/file.txt");
        let data = STANDARD.encode(b"hello");
        let input = format!(
            "ac=file;id=req-1;fid=f1;ft=regular;tt=simple;zip=zlib;q=0;mod=9;prm=420;sz=5;n={name};st=MA==;pr=parent;d={data};pw=c2VjcmV0"
        );
        let parts = fields(input.as_bytes());
        let command = parse_kitty_file_transfer(&parts, true).unwrap();
        assert_eq!(command.action, KittyFileTransferAction::File);
        assert_eq!(command.compression, KittyFileTransferCompression::Zlib);
        assert_eq!(command.file_type, KittyFileTransferType::Regular);
        assert_eq!(command.id, "req-1");
        assert_eq!(command.file_id, "f1");
        assert_eq!(command.name.as_deref(), Some("/tmp/root/file.txt"));
        assert_eq!(&*command.data, b"hello");
        assert!(command.bypass_present);
        assert!(command.reply_uses_8bit_controls);
        let debug = format!("{command:?}");
        assert!(!debug.contains("secret"));
        assert!(!debug.contains("hello"));
        assert!(!debug.contains("/tmp/root/file.txt"));
    }

    #[test]
    fn rejects_duplicate_invalid_and_oversized_fields() {
        for input in [
            b"ac=send;id=a;id=b".as_slice(),
            b"ac=send;id=bad!".as_slice(),
            b"ac=other;id=a".as_slice(),
            b"ac=data;id=a;d=%%%".as_slice(),
            b"ac=send;id=a;q=3".as_slice(),
        ] {
            let parts = fields(input);
            assert!(
                parse_kitty_file_transfer(&parts, false).is_err(),
                "{input:?}"
            );
        }
        let huge = vec![b'x'; MAX_KITTY_TRANSFER_OSC_BYTES + 1];
        assert!(parse_kitty_file_transfer(&[b"ac=send", b"id=a", &huge], false).is_err());

        let contextual = fields(b"ac=data;id=session;fid=file;q=invalid");
        let error = parse_kitty_file_transfer(&contextual, false).unwrap_err();
        assert_eq!(error.id.as_deref(), Some("session"));
        assert_eq!(error.file_id.as_deref(), Some("file"));
    }

    #[test]
    fn status_reply_is_canonical_bounded_and_base64_safe() {
        let reply = KittyFileTransferStatus {
            id: "request".into(),
            file_id: "file".into(),
            code: KittyFileTransferStatusCode::Started,
            message: Some("ready\nnow".into()),
            name: Some("/tmp/a b".into()),
            size: Some(12),
            transmission_type: KittyFileTransmissionType::Simple,
        }
        .canonical_reply();
        let text = String::from_utf8(reply).unwrap();
        assert!(text.starts_with("\u{1b}]5113;ac=status;id=request;fid=file;sz=12;"));
        assert!(text.contains("n=L3RtcC9hIGI="));
        assert!(text.contains("st=U1RBUlRFRDpyZWFkeW5vdw=="));
        assert!(text.ends_with("\u{1b}\\"));
    }

    #[test]
    fn canonical_host_read_replies_encode_paths_and_payload_without_debug_leaks() {
        let metadata = KittyFileTransferReply::File {
            id: "session".into(),
            file_id: "requested".into(),
            actual_file_id: "actual".into(),
            name: "/home/user/private.txt".into(),
            mtime_ns: 12,
            permissions: 0o640,
            size: 3,
        };
        let encoded = String::from_utf8(metadata.canonical_reply()).unwrap();
        assert!(encoded.starts_with("\u{1b}]5113;ac=file;id=session;fid=requested;"));
        assert!(encoded.contains(";mod=12;prm=416;sz=3;"));
        assert!(encoded.contains("n=L2hvbWUvdXNlci9wcml2YXRlLnR4dA=="));
        assert!(encoded.contains("st=YWN0dWFs"));
        assert!(!format!("{metadata:?}").contains("private.txt"));

        let data = KittyFileTransferReply::Data {
            id: "session".into(),
            file_id: "actual".into(),
            data: Box::from(b"secret".as_slice()),
            last: true,
        };
        assert_eq!(
            String::from_utf8(data.canonical_reply()).unwrap(),
            "\u{1b}]5113;ac=end_data;id=session;fid=actual;d=c2VjcmV0\u{1b}\\"
        );
        assert!(!format!("{data:?}").contains("secret"));
    }
}
