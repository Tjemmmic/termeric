//! What the integration tests share for speaking Termeric's local protocols.
//! The crate is binary-only, so tests cannot import its protocol types; they
//! spell the wire format out by hand, which also pins it. Both protocols
//! frame each message as a 4-byte big-endian length followed by that many
//! bytes of JSON.

// Each test binary compiles this module and uses only part of it.
#![allow(dead_code)]

use serde_json::Value;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The longest frame a test accepts: the domain protocol's limit, the
/// larger of the two.
const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

/// `value` as one frame.
pub fn frame(value: &Value) -> Vec<u8> {
    let payload = serde_json::to_vec(value).unwrap();
    [
        u32::try_from(payload.len())
            .unwrap()
            .to_be_bytes()
            .as_slice(),
        &payload,
    ]
    .concat()
}

/// Writes `value` as one frame.
pub fn write_frame(writer: &mut impl Write, value: &Value) {
    writer.write_all(&frame(value)).unwrap();
}

/// Reads one frame.
pub fn read_frame(reader: &mut impl Read) -> Value {
    let mut length = [0; 4];
    reader.read_exact(&mut length).unwrap();
    let length = u32::from_be_bytes(length) as usize;
    assert!(
        (1..=MAX_FRAME_BYTES).contains(&length),
        "frame length {length}"
    );
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload).unwrap();
    serde_json::from_slice(&payload).unwrap()
}

/// Every frame in `wire`, which must end on a frame boundary.
pub fn read_frames(mut wire: &[u8]) -> Vec<Value> {
    let mut frames = Vec::new();
    while !wire.is_empty() {
        frames.push(read_frame(&mut wire));
    }
    frames
}

/// The token tests write beside a stand-in GUI's socket. A real GUI writes a
/// random one there, readable only by its user, and refuses any request
/// that does not carry it.
pub const GUI_TOKEN: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

/// A stand-in for a running Termeric window process: the control socket and
/// token a GUI with this test's process ID would create, in a private
/// runtime directory. A command run with `XDG_RUNTIME_DIR` set to `root`
/// finds it under that process ID, and the test answers as the GUI would.
pub struct StandInGui {
    pub root: tempfile::TempDir,
    pub listener: UnixListener,
    /// Where the token is; the socket is beside it.
    pub token: PathBuf,
}

impl StandInGui {
    pub fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let directory = root.path().join("termeric");
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let stem = format!("gui-v1-{}", std::process::id());
        let socket = directory.join(format!("{stem}.sock"));
        let listener = UnixListener::bind(&socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        fs::set_permissions(socket, fs::Permissions::from_mode(0o600)).unwrap();
        let token = directory.join(format!("{stem}.token"));
        fs::write(&token, GUI_TOKEN).unwrap();
        fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).unwrap();
        Self {
            root,
            listener,
            token,
        }
    }

    /// The next connection a command makes, which must come within five
    /// seconds; `missing` says what it means when none does.
    pub fn accept(&self, missing: &str) -> UnixStream {
        let deadline = Instant::now() + Duration::from_secs(5);
        let stream = loop {
            match self.listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "{missing}");
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("accept failed: {error}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream
    }

    /// Checks no command has connected since the last one was accepted.
    pub fn nothing_requested(&self) {
        assert!(matches!(
            self.listener.accept(),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
        ));
    }
}
