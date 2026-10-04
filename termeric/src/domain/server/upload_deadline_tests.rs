//! Local socket/file fixtures for deadline retention; no shells or providers.
use super::test_support::{DOMAIN, state};
use super::*;
use sha2::{Digest, Sha256};

const UPLOAD: &str = "22222222222222222222222222222222";

fn start(
    state: &SharedState,
    total: u64,
    timeout: Duration,
) -> (UnixStream, thread::JoinHandle<anyhow::Result<Response>>) {
    let (mut server, mut client) = UnixStream::pair().unwrap();
    server.set_read_timeout(Some(timeout)).unwrap();
    server
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    client
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let state = Arc::clone(state);
    let worker = thread::spawn(move || {
        receive_file_upload(
            &mut server,
            state,
            DOMAIN,
            0,
            "result.bin",
            total,
            UPLOAD,
            UploadKind::File,
        )
    });
    assert!(
        matches!(protocol::read_frame::<ServerFrame>(&mut client).unwrap(),
        ServerFrame::Response { response: Response::UploadReady { offset: 3, prefix_sha256, .. } }
            if prefix_sha256 == format!("{:x}", Sha256::digest(b"abc")))
    );
    (client, worker)
}

#[test]
fn trickled_upload_chunk_and_commit_release_capacity_without_publishing_and_can_resume() {
    for pending_commit in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let staging = directory.path().join(".termeric-upload-fixture");
        let target = directory.path().join("result.bin");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&staging)
            .unwrap();
        file.write_all(b"abc").unwrap();
        let metadata = file.metadata().unwrap();
        drop(file);
        let state = state();
        let total = if pending_commit { 3 } else { 6 };
        state.lock().unwrap().upload_records.insert(
            UPLOAD.into(),
            UploadRecord {
                descriptor: UploadDescriptor {
                    domain: DOMAIN.into(),
                    pane: 0,
                    name: "result.bin".into(),
                    total_bytes: total,
                    kind: UploadKind::File,
                },
                state: UploadRecordState::Partial {
                    temporary_path: staging.clone(),
                    target_path: target.clone(),
                    bytes_received: 3,
                    device: metadata.dev(),
                    inode: metadata.ino(),
                    active: false,
                },
                updated: Instant::now(),
            },
        );
        let (mut client, worker) = start(&state, total, Duration::from_millis(150));
        let mut encoded = Vec::new();
        protocol::write_frame(
            &mut encoded,
            &if pending_commit {
                ClientFrame::UploadCommit
            } else {
                ClientFrame::UploadChunk {
                    offset: 3,
                    data: b"def".to_vec(),
                }
            },
        )
        .unwrap();
        let writer = thread::spawn(move || {
            for byte in encoded {
                if client.write_all(&[byte]).is_err() {
                    return;
                }
                thread::sleep(Duration::from_millis(10));
            }
            if !pending_commit {
                let _ = protocol::write_frame(&mut client, &ClientFrame::UploadCommit);
            }
        });
        let result = worker.join().unwrap();
        writer.join().unwrap();
        let error = result.expect_err("trickled upload renewed the frame deadline");
        let io = error.downcast_ref::<std::io::Error>().unwrap();
        assert!(matches!(
            io.kind(),
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
        ));
        assert!(
            !target.exists(),
            "a partial or missing commit must never publish"
        );
        assert_eq!(fs::read(&staging).unwrap(), b"abc");
        {
            let guard = state.lock().unwrap();
            assert_eq!(guard.active_uploads, 0);
            assert!(guard.upload_ids_in_flight.is_empty());
            assert!(matches!(
                guard.upload_records[UPLOAD].state,
                UploadRecordState::Partial {
                    bytes_received: 3,
                    active: false,
                    ..
                }
            ));
        }
        let (mut client, worker) = start(&state, total, Duration::from_secs(2));
        if !pending_commit {
            protocol::write_frame(
                &mut client,
                &ClientFrame::UploadChunk {
                    offset: 3,
                    data: b"def".to_vec(),
                },
            )
            .unwrap();
        }
        protocol::write_frame(&mut client, &ClientFrame::UploadCommit).unwrap();
        assert!(
            matches!(worker.join().unwrap().unwrap(), Response::FileUploaded { bytes, .. } if bytes == total)
        );
        assert_eq!(
            fs::read(&target).unwrap(),
            if pending_commit {
                &b"abc"[..]
            } else {
                &b"abcdef"[..]
            }
        );
        assert!(!staging.exists());
        assert_eq!(state.lock().unwrap().active_uploads, 0);
        prune_expired_upload_records(&state, true);
    }
}
