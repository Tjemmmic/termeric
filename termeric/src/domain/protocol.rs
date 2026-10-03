//! Versioned, length-delimited local-domain wire protocol.

use super::model::{
    DomainSnapshot, DomainSummary, DomainTopology, LivePane, LiveWorkspace, SpawnSpec,
    SplitDirection,
};
use anyhow::Context;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::io::{Read, Write};

pub const PROTOCOL_VERSION: u32 = 16;
pub const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_STREAM_CHUNK_BYTES: usize = 64 * 1024;
pub const MAX_FILE_UPLOAD_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_UPLOAD_NAME_BYTES: usize = 255;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UploadKind {
    File,
    Directory { entries: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UploadRetention {
    Automatic,
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadSummary {
    pub upload_id: String,
    pub domain: String,
    pub pane: u32,
    pub name: String,
    pub total_bytes: u64,
    pub bytes_received: u64,
    pub kind: UploadKind,
    pub retention: UploadRetention,
    pub active: bool,
    pub expires_in_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientFrame {
    Hello {
        version: u32,
        token: String,
    },
    Request {
        request: Request,
    },
    Input {
        data: Vec<u8>,
    },
    UploadChunk {
        offset: u64,
        data: Vec<u8>,
    },
    UploadCommit,
    /// Abort the current upload before publication. The server removes its
    /// private temporary file and reports how many bytes it received.
    UploadCancel,
    Resize {
        cols: u16,
        rows: u16,
        pixel_width: u16,
        pixel_height: u16,
    },
    Detach,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerFrame {
    Hello {
        version: u32,
    },
    Response {
        response: Response,
    },
    Output {
        sequence: u64,
        data: Vec<u8>,
    },
    /// ASCII escape-stream fragment from the daemon's decoded graphics
    /// namespace. This is distinct from live PTY output and always precedes
    /// `SnapshotComplete` for an attachment.
    SnapshotOutput {
        data: String,
    },
    SnapshotComplete,
    Closed {
        exit_message: String,
    },
    TopologyChanged {
        revision: u64,
        domain: DomainTopology,
    },
    DomainClosed {
        domain: String,
        message: String,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Request {
    ListDomains,
    ListWorkspaces {
        domain: String,
    },
    ListPanes {
        domain: String,
    },
    Describe {
        domain: String,
    },
    CreateDomain {
        name: String,
        spawn: SpawnSpec,
    },
    SpawnTab {
        domain: String,
        spawn: SpawnSpec,
    },
    SplitPane {
        domain: String,
        pane: u32,
        direction: SplitDirection,
        spawn: SpawnSpec,
    },
    ResizeSplit {
        domain: String,
        split: u32,
        ratio_millis: u16,
    },
    RenameTab {
        domain: String,
        tab: u32,
        title: Option<String>,
    },
    SetTabPinned {
        domain: String,
        tab: u32,
        pinned: bool,
    },
    MoveTab {
        domain: String,
        tab: u32,
        index: u16,
    },
    SwapPaneNext {
        domain: String,
        pane: u32,
    },
    BalanceTab {
        domain: String,
        tab: u32,
    },
    ApplyPaneLayout {
        domain: String,
        tab: u32,
        preset: super::model::LivePaneLayoutPreset,
    },
    MovePane {
        domain: String,
        pane: u32,
        tab: u32,
        /// Exact destination anchor. `None` preserves the domain CLI's
        /// historical focused-pane behavior.
        target: Option<u32>,
        direction: SplitDirection,
    },
    FocusPane {
        domain: String,
        pane: u32,
    },
    ClosePane {
        domain: String,
        pane: u32,
    },
    CloseTab {
        domain: String,
        tab: u32,
    },
    SendText {
        domain: String,
        pane: u32,
        data: Vec<u8>,
    },
    SendPaste {
        domain: String,
        pane: u32,
        text: String,
    },
    GetText {
        domain: String,
        pane: u32,
        max_bytes: usize,
    },
    ListUploads,
    CancelUpload {
        upload_id: String,
    },
    UploadFile {
        domain: String,
        pane: u32,
        name: String,
        total_bytes: u64,
        /// Random client-generated identity used only for bounded retry and
        /// idempotent completion replay within this authenticated authority.
        upload_id: String,
        /// The byte stream is either the file itself or Termeric's bounded,
        /// deterministic directory archive.
        kind: UploadKind,
        retention: UploadRetention,
        /// Refuse to create a new transfer when the retained record is absent.
        resume_only: bool,
    },
    Attach {
        domain: String,
        pane: u32,
    },
    SubscribeTopology {
        domain: String,
    },
    KillDomain {
        domain: String,
    },
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Response {
    Domains {
        domains: Vec<DomainSummary>,
    },
    Workspaces {
        workspaces: Vec<LiveWorkspace>,
    },
    Panes {
        panes: Vec<LivePane>,
    },
    Domain {
        domain: DomainSnapshot,
    },
    Text {
        text: String,
        truncated: bool,
    },
    UploadReady {
        max_chunk_bytes: usize,
        offset: u64,
        /// Lowercase SHA-256 of the retained prefix `[0, offset)`.
        prefix_sha256: String,
    },
    FileUploaded {
        path: String,
        bytes: u64,
    },
    DirectoryUploaded {
        path: String,
        bytes: u64,
        entries: u32,
    },
    UploadCancelled {
        bytes_received: u64,
    },
    UploadBusy {
        retry_after_ms: u16,
    },
    Uploads {
        uploads: Vec<UploadSummary>,
    },
    Attached {
        snapshot: Vec<u8>,
        sequence: u64,
    },
    TopologySubscribed {
        revision: u64,
        domain: DomainTopology,
    },
    Ok {
        message: String,
    },
}

pub fn validate_upload_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty()
        || name.len() > MAX_UPLOAD_NAME_BYTES
        || matches!(name, "." | "..")
        || name.contains(['/', '\\'])
        || name.chars().any(crate::safe_text::is_unsafe_display_char)
    {
        anyhow::bail!("upload name must be one safe UTF-8 file-name component");
    }
    Ok(())
}

pub fn validate_upload_id(upload_id: &str) -> anyhow::Result<()> {
    if upload_id.len() != 32
        || !upload_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        anyhow::bail!("upload id must be exactly 32 lowercase hexadecimal characters");
    }
    Ok(())
}

pub fn validate_sha256(digest: &str) -> anyhow::Result<()> {
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        anyhow::bail!("SHA-256 digest must be exactly 64 lowercase hexadecimal characters");
    }
    Ok(())
}

pub fn write_frame<T: Serialize>(writer: &mut impl Write, frame: &T) -> anyhow::Result<()> {
    let payload = serde_json::to_vec(frame).context("serialize domain protocol frame")?;
    if payload.len() > MAX_FRAME_BYTES {
        anyhow::bail!("domain protocol frame exceeds {MAX_FRAME_BYTES} bytes");
    }
    writer
        .write_all(&(payload.len() as u32).to_be_bytes())
        .context("write domain protocol frame length")?;
    writer
        .write_all(&payload)
        .context("write domain protocol frame payload")?;
    writer.flush().context("flush domain protocol frame")?;
    Ok(())
}

pub fn read_frame<T: DeserializeOwned>(reader: &mut impl Read) -> anyhow::Result<T> {
    let mut length = [0u8; 4];
    reader
        .read_exact(&mut length)
        .context("read domain protocol frame length")?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        anyhow::bail!("invalid domain protocol frame length {length}");
    }
    let mut payload = vec![0u8; length];
    reader
        .read_exact(&mut payload)
        .context("read domain protocol frame payload")?;
    serde_json::from_slice(&payload).context("decode domain protocol frame")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_frame_round_trips_through_the_versioned_wire_shape() {
        for frame in [
            ClientFrame::Resize {
                cols: 101,
                rows: 37,
                pixel_width: 1_111,
                pixel_height: 777,
            },
            ClientFrame::Request {
                request: Request::SendText {
                    domain: "daily".into(),
                    pane: 7,
                    data: vec![0, 1, 2, 255],
                },
            },
            ClientFrame::Request {
                request: Request::SendPaste {
                    domain: "daily".into(),
                    pane: 7,
                    text: "'/tmp/artifact.bin' ".into(),
                },
            },
            ClientFrame::Request {
                request: Request::ClosePane {
                    domain: "daily".into(),
                    pane: 7,
                },
            },
            ClientFrame::Request {
                request: Request::CloseTab {
                    domain: "daily".into(),
                    tab: 3,
                },
            },
            ClientFrame::Request {
                request: Request::SubscribeTopology {
                    domain: "daily".into(),
                },
            },
            ClientFrame::Request {
                request: Request::ResizeSplit {
                    domain: "daily".into(),
                    split: 4,
                    ratio_millis: 375,
                },
            },
            ClientFrame::Request {
                request: Request::RenameTab {
                    domain: "daily".into(),
                    tab: 3,
                    title: Some("Build λ".into()),
                },
            },
            ClientFrame::Request {
                request: Request::SetTabPinned {
                    domain: "daily".into(),
                    tab: 3,
                    pinned: true,
                },
            },
            ClientFrame::Request {
                request: Request::MoveTab {
                    domain: "daily".into(),
                    tab: 3,
                    index: 1,
                },
            },
            ClientFrame::Request {
                request: Request::SwapPaneNext {
                    domain: "daily".into(),
                    pane: 7,
                },
            },
            ClientFrame::Request {
                request: Request::BalanceTab {
                    domain: "daily".into(),
                    tab: 3,
                },
            },
            ClientFrame::Request {
                request: Request::ApplyPaneLayout {
                    domain: "daily".into(),
                    tab: 3,
                    preset: super::super::model::LivePaneLayoutPreset::Grid,
                },
            },
            ClientFrame::Request {
                request: Request::ApplyPaneLayout {
                    domain: "daily".into(),
                    tab: 3,
                    preset: super::super::model::LivePaneLayoutPreset::Tall,
                },
            },
            ClientFrame::Request {
                request: Request::ApplyPaneLayout {
                    domain: "daily".into(),
                    tab: 3,
                    preset: super::super::model::LivePaneLayoutPreset::Fat,
                },
            },
            ClientFrame::Request {
                request: Request::MovePane {
                    domain: "daily".into(),
                    pane: 7,
                    tab: 4,
                    target: Some(9),
                    direction: SplitDirection::Horizontal,
                },
            },
            ClientFrame::Request {
                request: Request::UploadFile {
                    domain: "daily".into(),
                    pane: 7,
                    name: "artifact.bin".into(),
                    total_bytes: 131_072,
                    upload_id: "0123456789abcdef0123456789abcdef".into(),
                    kind: UploadKind::Directory { entries: 42 },
                    retention: UploadRetention::Manual,
                    resume_only: true,
                },
            },
            ClientFrame::Request {
                request: Request::ListUploads,
            },
            ClientFrame::Request {
                request: Request::CancelUpload {
                    upload_id: "0123456789abcdef0123456789abcdef".into(),
                },
            },
            ClientFrame::UploadChunk {
                offset: 65_536,
                data: vec![0, 1, 2, 255],
            },
            ClientFrame::UploadCommit,
            ClientFrame::UploadCancel,
        ] {
            let mut wire = Vec::new();
            write_frame(&mut wire, &frame).unwrap();
            assert_eq!(
                read_frame::<ClientFrame>(&mut wire.as_slice()).unwrap(),
                frame
            );
        }

        for frame in [
            ServerFrame::Response {
                response: Response::TopologySubscribed {
                    revision: 8,
                    domain: DomainTopology {
                        domain_id: "daily".into(),
                        active_tab: 0,
                        tabs: Vec::new(),
                    },
                },
            },
            ServerFrame::TopologyChanged {
                revision: 9,
                domain: DomainTopology {
                    domain_id: "daily".into(),
                    active_tab: 0,
                    tabs: Vec::new(),
                },
            },
            ServerFrame::DomainClosed {
                domain: "daily".into(),
                message: "terminated".into(),
            },
            ServerFrame::SnapshotOutput {
                data: "\x1b_TG;end\x1b\\".into(),
            },
            ServerFrame::Response {
                response: Response::UploadReady {
                    max_chunk_bytes: MAX_STREAM_CHUNK_BYTES,
                    offset: 65_536,
                    prefix_sha256:
                        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
                },
            },
            ServerFrame::Response {
                response: Response::FileUploaded {
                    path: "/tmp/artifact.bin".into(),
                    bytes: 131_072,
                },
            },
            ServerFrame::Response {
                response: Response::DirectoryUploaded {
                    path: "/tmp/project".into(),
                    bytes: 131_072,
                    entries: 42,
                },
            },
            ServerFrame::Response {
                response: Response::UploadCancelled {
                    bytes_received: 65_536,
                },
            },
            ServerFrame::Response {
                response: Response::UploadBusy {
                    retry_after_ms: 100,
                },
            },
            ServerFrame::Response {
                response: Response::Uploads {
                    uploads: vec![UploadSummary {
                        upload_id: "0123456789abcdef0123456789abcdef".into(),
                        domain: "daily".into(),
                        pane: 7,
                        name: "artifact.bin".into(),
                        total_bytes: 131_072,
                        bytes_received: 65_536,
                        kind: UploadKind::File,
                        retention: UploadRetention::Manual,
                        active: false,
                        expires_in_seconds: 86_399,
                    }],
                },
            },
            ServerFrame::SnapshotComplete,
        ] {
            let mut wire = Vec::new();
            write_frame(&mut wire, &frame).unwrap();
            assert_eq!(
                read_frame::<ServerFrame>(&mut wire.as_slice()).unwrap(),
                frame
            );
        }
    }

    #[test]
    fn upload_names_are_one_safe_portable_component() {
        for valid in ["artifact.bin", "release λ.txt", "-leading-option"] {
            validate_upload_name(valid).unwrap();
        }
        for invalid in [
            "",
            ".",
            "..",
            "../escape",
            "nested/file",
            "windows\\path",
            "line\nbreak",
            "reverse\u{202e}txt",
        ] {
            assert!(validate_upload_name(invalid).is_err(), "{invalid:?}");
        }
        assert!(validate_upload_name(&"x".repeat(MAX_UPLOAD_NAME_BYTES + 1)).is_err());
    }

    #[test]
    fn sha256_digests_use_one_canonical_wire_shape() {
        validate_sha256("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
            .unwrap();
        for invalid in ["", "abc", &"A".repeat(64), &"0".repeat(63), &"g".repeat(64)] {
            assert!(validate_sha256(invalid).is_err());
        }
    }

    #[test]
    fn upload_ids_are_exact_lowercase_random_token_shapes() {
        for valid in [
            "0123456789abcdef0123456789abcdef",
            "ffffffffffffffffffffffffffffffff",
        ] {
            validate_upload_id(valid).unwrap();
        }
        for invalid in [
            "",
            "0123456789abcdef",
            "0123456789ABCDEF0123456789ABCDEF",
            "gggggggggggggggggggggggggggggggg",
            "0123456789abcdef0123456789abcdef0",
        ] {
            assert!(validate_upload_id(invalid).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn malformed_empty_oversized_and_truncated_frames_are_rejected() {
        for wire in [
            0u32.to_be_bytes().to_vec(),
            ((MAX_FRAME_BYTES + 1) as u32).to_be_bytes().to_vec(),
            [4u32.to_be_bytes().as_slice(), b"{}"].concat(),
            [1u32.to_be_bytes().as_slice(), b"{"].concat(),
        ] {
            assert!(read_frame::<ClientFrame>(&mut wire.as_slice()).is_err());
        }
    }

    #[test]
    fn seeded_byte_mutations_never_bypass_length_or_json_validation() {
        let mut valid = Vec::new();
        write_frame(
            &mut valid,
            &ClientFrame::Hello {
                version: PROTOCOL_VERSION,
                token: "token".into(),
            },
        )
        .unwrap();
        for index in 0..valid.len() {
            let mut mutated = valid.clone();
            mutated[index] ^= 0xff;
            let _ = read_frame::<ClientFrame>(&mut mutated.as_slice());
        }
    }
}
