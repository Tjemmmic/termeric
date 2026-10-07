use super::*;

fn request(session: &str, file_id: &str, name: &str) -> KittyFileTransferCommand {
    KittyFileTransferCommand {
        file_id: file_id.into(),
        name: Some(name.into()),
        reply_uses_8bit_controls: true,
        ..test_transfer_command(KittyFileTransferAction::File, session)
    }
}

#[test]
fn upload_read_handle_stays_on_the_pinned_regular_inode() {
    use std::io::Read;
    use std::os::unix::fs::MetadataExt;

    for symbolic in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("approved");
        let moved = root.path().join("moved");
        let victim = root.path().join("victim");
        std::fs::write(&path, b"approved bytes").unwrap();
        let pinned = openat2_no_symlinks(&path, libc::O_PATH).unwrap();
        let identity = pinned.metadata().unwrap();
        std::fs::rename(&path, &moved).unwrap();
        if symbolic {
            std::fs::write(&victim, b"must not read").unwrap();
            std::os::unix::fs::symlink(&victim, &path).unwrap();
        } else {
            std::fs::write(&path, b"must not read").unwrap();
        }
        let mut file = reopen_upload_regular_file(&pinned).unwrap();
        let opened = file.metadata().unwrap();
        assert_eq!(
            (opened.dev(), opened.ino()),
            (identity.dev(), identity.ino())
        );
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"approved bytes");
    }
}

#[test]
fn upload_worker_rejects_nonregular_and_multiply_linked_sources_before_metadata() {
    for kind in ["directory", "socket", "device", "hardlink"] {
        let root = tempfile::tempdir().unwrap();
        let local = root.path().join("source");
        let mut socket = None;
        let path = match kind {
            "directory" => {
                std::fs::create_dir(&local).unwrap();
                local
            }
            "socket" => {
                socket = Some(std::os::unix::net::UnixListener::bind(&local).unwrap());
                local
            }
            "device" => std::path::PathBuf::from("/dev/null"),
            "hardlink" => {
                std::fs::write(&local, b"must not export").unwrap();
                std::fs::hard_link(&local, root.path().join("alias")).unwrap();
                local
            }
            _ => unreachable!(),
        };
        let pinned = openat2_no_symlinks(&path, libc::O_PATH).unwrap();
        use std::os::fd::AsRawFd;
        // SAFETY: pinned owns a live descriptor; F_GETFL has no third argument.
        let flags = unsafe { libc::fcntl(pinned.as_raw_fd(), libc::F_GETFL) };
        assert!(flags >= 0);
        assert_ne!(
            flags & libc::O_PATH,
            0,
            "{kind} must not receive a data open"
        );
        assert_eq!(
            reopen_upload_regular_file(&pinned).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );
        let (_sender, receiver) = sync_channel(1);
        let mut completions = Vec::new();
        run_upload_worker(
            receiver,
            "type-test".into(),
            vec![UploadPath {
                file_id: "f1".into(),
                requested_name: path.to_str().unwrap().into(),
                path,
            }],
            true,
            Arc::new(AtomicBool::new(false)),
            |completion| {
                completion.acknowledge.send(true).unwrap();
                completions.push(completion);
                true
            },
        );
        assert_eq!(completions.len(), 1, "{kind}");
        assert!(completions[0].terminal);
        assert_eq!(completions[0].bytes, 0);
        assert_eq!(completions[0].frames.len(), 1);
        let frame = String::from_utf8(completions[0].frames[0].clone()).unwrap();
        assert!(frame.contains(";ac=status;id=type-test;fid=f1;"));
        assert_eq!(
            completions[0].frames[0],
            transfer_status(
                "type-test",
                "f1",
                KittyFileTransferStatusCode::Invalid,
                "approved path is not a single-link regular file"
            )
            .canonical_reply()
        );
        drop(socket);
    }
}

#[test]
fn upload_worker_rejects_fifo_without_waiting_for_a_writer() {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;

    let root = tempfile::tempdir().unwrap();
    let fifo = root.path().join("fifo");
    let name = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    // SAFETY: name is a live NUL-terminated path in our private tempdir.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let worker_path = fifo.clone();
    let (done_sender, done_receiver) = sync_channel(1);
    let worker = std::thread::spawn(move || {
        let (_sender, receiver) = sync_channel(1);
        let mut completions = Vec::new();
        run_upload_worker(
            receiver,
            "fifo-test".into(),
            vec![UploadPath {
                file_id: "f1".into(),
                requested_name: worker_path.to_str().unwrap().into(),
                path: worker_path,
            }],
            true,
            Arc::new(AtomicBool::new(false)),
            |completion| {
                completion.acknowledge.send(true).unwrap();
                completions.push(completion);
                true
            },
        );
        done_sender.send(completions).unwrap();
    });
    let mut completed_without_writer = true;
    let completions = match done_receiver.recv_timeout(Duration::from_secs(2)) {
        Ok(completions) => completions,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            completed_without_writer = false;
            // Release the old blocking open before failing the regression.
            let _rescue = std::fs::OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&fifo)
                .unwrap();
            done_receiver.recv_timeout(Duration::from_secs(2)).unwrap()
        }
        Err(error) => panic!("worker disconnected: {error}"),
    };
    worker.join().unwrap();
    assert!(
        completed_without_writer,
        "upload worker waited for a FIFO writer"
    );
    assert_eq!(completions.len(), 1);
    assert!(completions[0].terminal);
    assert_eq!(completions[0].bytes, 0);
    assert_eq!(completions[0].frames.len(), 1);
    let frame = String::from_utf8(completions[0].frames[0].clone()).unwrap();
    assert!(frame.contains(";ac=status;id=fifo-test;fid=f1;"));
    assert!(!frame.contains(";ac=file;"));
    let error = std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&fifo)
        .unwrap_err();
    assert_eq!(
        error.raw_os_error(),
        Some(libc::ENXIO),
        "no FIFO reader may remain"
    );
}

#[test]
fn upload_worker_opens_after_consent_and_streams_bounded_canonical_frames() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("approved.txt");
    std::fs::write(&path, b"approved contents").unwrap();
    let path_name = path.to_str().unwrap().to_string();
    let (sender, receiver) = sync_channel(2);
    sender
        .send(request("upload-test", "f1", &path_name))
        .unwrap();
    drop(sender);
    let mut completions = Vec::new();
    run_upload_worker(
        receiver,
        "upload-test".into(),
        vec![UploadPath {
            file_id: "f1".into(),
            requested_name: path_name.clone(),
            path,
        }],
        true,
        Arc::new(AtomicBool::new(false)),
        |completion| {
            completion.acknowledge.send(true).unwrap();
            completions.push(completion);
            true
        },
    );

    assert_eq!(completions.len(), 2);
    assert_eq!(completions[0].frames.len(), 2);
    let metadata = String::from_utf8(completions[0].frames[0].clone()).unwrap();
    assert!(metadata.starts_with("\u{1b}]5113;ac=file;id=upload-test;fid=f1;"));
    assert!(metadata.contains(";sz=17;"));
    assert!(!metadata.contains("approved contents"));
    let ready = String::from_utf8(completions[0].frames[1].clone()).unwrap();
    assert!(ready.contains(";ac=status;id=upload-test;"));
    assert_eq!(completions[1].frames.len(), 1);
    let payload = String::from_utf8(completions[1].frames[0].clone()).unwrap();
    assert!(payload.contains(";ac=end_data;id=upload-test;fid=f1;"));
    assert!(payload.contains("d=YXBwcm92ZWQgY29udGVudHM="));
    assert_eq!(completions[1].files, 1);
    assert_eq!(completions[1].bytes, 17);
    assert!(completions[1].reply_uses_8bit_controls);
}

#[test]
fn upload_worker_stops_when_a_completion_is_dropped_unanswered() {
    // A completion for a window that has closed is dropped with its
    // acknowledgement sender; the worker must stop rather than wait forever.
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("approved.txt");
    std::fs::write(&path, b"never sent").unwrap();
    let path_name = path.to_str().unwrap().to_string();
    let (sender, receiver) = sync_channel(1);
    sender.send(request("dropped", "f1", &path_name)).unwrap();
    let (done, finished) = sync_channel(1);
    let worker = std::thread::spawn(move || {
        let mut emitted = 0;
        run_upload_worker(
            receiver,
            "dropped".into(),
            vec![UploadPath {
                file_id: "f1".into(),
                requested_name: path_name,
                path,
            }],
            false,
            Arc::new(AtomicBool::new(false)),
            |completion| {
                emitted += 1;
                drop(completion);
                true
            },
        );
        done.send(emitted).unwrap();
    });
    let emitted = finished
        .recv_timeout(Duration::from_secs(10))
        .expect("the worker kept waiting for a dropped completion");
    assert_eq!(emitted, 1, "nothing follows an undelivered completion");
    worker.join().unwrap();
    drop(sender);
}

#[test]
fn upload_worker_refuses_symlinks_without_reading_their_targets() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target.txt");
    let link = temp.path().join("link.txt");
    std::fs::write(&target, b"must not leak").unwrap();
    symlink(&target, &link).unwrap();
    let link_name = link.to_str().unwrap().to_string();
    let (_sender, receiver) = sync_channel(1);
    let mut completions = Vec::new();
    run_upload_worker(
        receiver,
        "symlink-test".into(),
        vec![UploadPath {
            file_id: "f1".into(),
            requested_name: link_name.clone(),
            path: link,
        }],
        false,
        Arc::new(AtomicBool::new(false)),
        |completion| {
            completion.acknowledge.send(true).unwrap();
            completions.push(completion);
            true
        },
    );

    assert_eq!(completions.len(), 1);
    assert!(completions[0].terminal);
    assert_eq!(completions[0].bytes, 0);
    let error = String::from_utf8(completions[0].frames[0].clone()).unwrap();
    assert!(error.contains(";ac=status;id=symlink-test;fid=f1;"));
    assert!(!error.contains("must not leak"));

    let real_directory = temp.path().join("real-directory");
    let linked_directory = temp.path().join("linked-directory");
    std::fs::create_dir(&real_directory).unwrap();
    std::fs::write(real_directory.join("nested.txt"), b"also must not leak").unwrap();
    symlink(&real_directory, &linked_directory).unwrap();
    let nested = linked_directory.join("nested.txt");
    let nested_name = nested.to_str().unwrap().to_string();
    let (_sender, receiver) = sync_channel(1);
    let mut ancestor_completions = Vec::new();
    run_upload_worker(
        receiver,
        "ancestor-test".into(),
        vec![UploadPath {
            file_id: "f2".into(),
            requested_name: nested_name,
            path: nested,
        }],
        false,
        Arc::new(AtomicBool::new(false)),
        |completion| {
            completion.acknowledge.send(true).unwrap();
            ancestor_completions.push(completion);
            true
        },
    );
    assert_eq!(ancestor_completions.len(), 1);
    assert!(ancestor_completions[0].terminal);
    let error = String::from_utf8(ancestor_completions[0].frames[0].clone()).unwrap();
    assert!(error.contains(";ac=status;id=ancestor-test;fid=f2;"));
    assert!(!error.contains("also must not leak"));
}

#[test]
fn upload_worker_rejects_aggregate_size_before_emitting_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("oversized.bin");
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(MAX_KITTY_UPLOAD_BYTES + 1).unwrap();
    drop(file);
    let path_name = path.to_str().unwrap().to_string();
    let (_sender, receiver) = sync_channel(1);
    let mut completions = Vec::new();
    run_upload_worker(
        receiver,
        "oversized-test".into(),
        vec![UploadPath {
            file_id: "f1".into(),
            requested_name: path_name,
            path,
        }],
        false,
        Arc::new(AtomicBool::new(false)),
        |completion| {
            completion.acknowledge.send(true).unwrap();
            completions.push(completion);
            true
        },
    );

    assert_eq!(completions.len(), 1);
    assert!(completions[0].terminal);
    assert_eq!(completions[0].bytes, 0);
    let error = String::from_utf8(completions[0].frames[0].clone()).unwrap();
    assert!(error.contains("st=RUZCSUc6"));
    assert!(!error.contains("ac=file"));
}

#[test]
fn upload_worker_completes_empty_files_in_mixed_batches() {
    let root = tempfile::tempdir().unwrap();
    let payloads: [&[u8]; 3] = [b"", b"payload", b""];
    let (sender, receiver) = sync_channel(payloads.len());
    let mut paths = Vec::new();
    for (index, payload) in payloads.iter().enumerate() {
        let file_id = format!("f{index}");
        let path = root.path().join(&file_id);
        std::fs::write(&path, payload).unwrap();
        let name = path.to_str().unwrap().to_string();
        sender.send(request("empty-test", &file_id, &name)).unwrap();
        paths.push(UploadPath {
            file_id,
            requested_name: name,
            path,
        });
    }
    drop(sender);
    let mut completions = Vec::new();
    run_upload_worker(
        receiver,
        "empty-test".into(),
        paths,
        true,
        Arc::new(AtomicBool::new(false)),
        |completion| {
            completion.acknowledge.send(true).unwrap();
            completions.push(completion);
            true
        },
    );
    assert_eq!(
        completions.len(),
        4,
        "metadata plus one completion per file"
    );
    assert_eq!(completions[0].frames.len(), 4);
    let mut bytes = 0;
    for (index, payload) in payloads.iter().enumerate() {
        bytes += payload.len() as u64;
        let completion = &completions[index + 1];
        assert_eq!(
            completion.frames,
            vec![
                KittyFileTransferReply::Data {
                    id: "empty-test".into(),
                    file_id: format!("f{index}"),
                    data: (*payload).into(),
                    last: true,
                }
                .canonical_reply()
            ]
        );
        assert_eq!(completion.files, index + 1);
        assert_eq!(completion.total_files, 3);
        assert_eq!(completion.bytes, bytes);
        assert!(completion.reply_uses_8bit_controls);
        assert!(!completion.terminal);
        assert!(completion.error.is_none());
    }
}

#[test]
fn upload_worker_stops_after_unacknowledged_or_canceled_empty_completion() {
    for cancel_on_empty in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (sender, receiver) = sync_channel(2);
        let mut paths = Vec::new();
        for (file_id, payload) in [
            ("empty", b"".as_slice()),
            ("later", b"must not send".as_slice()),
        ] {
            let path = root.path().join(file_id);
            std::fs::write(&path, payload).unwrap();
            let name = path.to_str().unwrap().to_string();
            sender.send(request("empty-stop", file_id, &name)).unwrap();
            paths.push(UploadPath {
                file_id: file_id.into(),
                requested_name: name,
                path,
            });
        }
        drop(sender);
        let cancellation = Arc::new(AtomicBool::new(false));
        let mut completions = Vec::new();
        run_upload_worker(
            receiver,
            "empty-stop".into(),
            paths,
            false,
            cancellation.clone(),
            |completion| {
                let empty_completion = completion.files == 1;
                if empty_completion && cancel_on_empty {
                    cancellation.store(true, Ordering::Release);
                }
                completion
                    .acknowledge
                    .send(!empty_completion || cancel_on_empty)
                    .unwrap();
                completions.push(completion);
                true
            },
        );
        assert_eq!(completions.len(), 2, "later payload must not be delivered");
        assert_eq!(
            completions[1].frames,
            vec![
                KittyFileTransferReply::Data {
                    id: "empty-stop".into(),
                    file_id: "empty".into(),
                    data: Box::default(),
                    last: true,
                }
                .canonical_reply()
            ]
        );
        assert_eq!(completions[1].bytes, 0);
        assert!(!completions[1].reply_uses_8bit_controls);
    }
}

#[test]
fn upload_worker_emits_empty_completion_once_before_rejecting_duplicate() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("empty");
    std::fs::write(&path, b"").unwrap();
    let name = path.to_str().unwrap().to_string();
    let (sender, receiver) = sync_channel(2);
    for _ in 0..2 {
        sender.send(request("empty-once", "f", &name)).unwrap();
    }
    drop(sender);
    let mut completions = Vec::new();
    run_upload_worker(
        receiver,
        "empty-once".into(),
        vec![UploadPath {
            file_id: "f".into(),
            requested_name: name,
            path,
        }],
        false,
        Arc::new(AtomicBool::new(false)),
        |completion| {
            completion.acknowledge.send(true).unwrap();
            completions.push(completion);
            true
        },
    );
    assert_eq!(completions.len(), 3);
    assert_eq!(
        completions[1].frames,
        vec![
            KittyFileTransferReply::Data {
                id: "empty-once".into(),
                file_id: "f".into(),
                data: Box::default(),
                last: true,
            }
            .canonical_reply()
        ]
    );
    assert_eq!(
        completions[2].frames,
        vec![
            transfer_status(
                "empty-once",
                "f",
                KittyFileTransferStatusCode::Invalid,
                "file was already sent or its path did not match approval"
            )
            .canonical_reply()
        ]
    );
    assert!(completions[2].terminal);
    assert_eq!(completions[2].files, 1);
    assert_eq!(completions[2].bytes, 0);
}

#[test]
fn upload_worker_batches_large_files_behind_acknowledgements() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("multi-chunk.bin");
    let size = termeric_core::MAX_KITTY_TRANSFER_DATA_BYTES * 9 + 1;
    std::fs::write(&path, vec![0x5a; size]).unwrap();
    let path_name = path.to_str().unwrap().to_string();
    let (sender, receiver) = sync_channel(1);
    sender
        .send(request("batch-test", "f1", &path_name))
        .unwrap();
    drop(sender);
    let mut completions = Vec::new();
    run_upload_worker(
        receiver,
        "batch-test".into(),
        vec![UploadPath {
            file_id: "f1".into(),
            requested_name: path_name,
            path,
        }],
        false,
        Arc::new(AtomicBool::new(false)),
        |completion| {
            completion.acknowledge.send(true).unwrap();
            completions.push(completion);
            true
        },
    );

    assert_eq!(completions.len(), 3);
    assert_eq!(completions[1].frames.len(), UPLOAD_BATCH_CHUNKS);
    assert_eq!(completions[1].bytes, (UPLOAD_BATCH_CHUNKS * 4096) as u64);
    assert_eq!(completions[1].files, 0);
    assert_eq!(completions[2].frames.len(), 2);
    assert_eq!(completions[2].bytes, size as u64);
    assert_eq!(completions[2].files, 1);
    assert!(
        String::from_utf8(completions[2].frames[1].clone())
            .unwrap()
            .contains("ac=end_data")
    );
}

#[test]
fn upload_worker_truncation_retains_only_acknowledged_progress() {
    let chunk_size = termeric_core::MAX_KITTY_TRANSFER_DATA_BYTES;
    let batch_bytes = UPLOAD_BATCH_CHUNKS * chunk_size;
    for prior_size in [0, 7] {
        for acknowledged_batches in [0, 1, 2] {
            for buffered_bytes in [0, 17] {
                let temp = tempfile::tempdir().unwrap();
                let prior = temp.path().join("prior.bin");
                let path = temp.path().join("truncated.bin");
                std::fs::write(&prior, vec![0x41; prior_size]).unwrap();
                std::fs::write(&path, vec![0x5a; batch_bytes * 3]).unwrap();
                let (sender, receiver) = sync_channel(2);
                let mut paths = Vec::new();
                for (file_id, source) in [("prior", &prior), ("current", &path)] {
                    let name = source.to_str().unwrap().to_string();
                    sender.send(request("truncate", file_id, &name)).unwrap();
                    paths.push(UploadPath {
                        file_id: file_id.into(),
                        requested_name: name,
                        path: source.clone(),
                    });
                }
                drop(sender);
                let mut completions = Vec::new();
                run_upload_worker(
                    receiver,
                    "truncate".into(),
                    paths,
                    true,
                    Arc::new(AtomicBool::new(false)),
                    |completion| {
                        // Mutate the same inode at an exact delivery boundary,
                        // with no timing race or dependence on worker speed.
                        if completions.len() == 1 + acknowledged_batches {
                            std::fs::OpenOptions::new()
                                .write(true)
                                .open(&path)
                                .unwrap()
                                .set_len(
                                    (acknowledged_batches * batch_bytes + buffered_bytes) as u64,
                                )
                                .unwrap();
                        }
                        completion.acknowledge.send(true).unwrap();
                        completions.push(completion);
                        true
                    },
                );
                assert_eq!(completions.len(), 3 + acknowledged_batches);
                for (index, batch) in completions[2..2 + acknowledged_batches].iter().enumerate() {
                    assert_eq!(
                        batch.frames,
                        vec![
                            KittyFileTransferReply::Data {
                                id: "truncate".into(),
                                file_id: "current".into(),
                                data: vec![0x5a; chunk_size].into_boxed_slice(),
                                last: false,
                            }
                            .canonical_reply();
                            UPLOAD_BATCH_CHUNKS
                        ]
                    );
                    assert_eq!(batch.bytes, (prior_size + (index + 1) * batch_bytes) as u64);
                    assert_eq!(batch.files, 1);
                    assert!(!batch.terminal);
                    assert!(batch.error.is_none());
                }
                let failure = completions.last().unwrap();
                let message = "approved file changed or ended while being read";
                assert_eq!(
                    failure.frames,
                    vec![
                        transfer_status(
                            "truncate",
                            "current",
                            KittyFileTransferStatusCode::Io,
                            message
                        )
                        .canonical_reply()
                    ]
                );
                assert!(failure.terminal);
                assert!(failure.reply_uses_8bit_controls);
                assert_eq!(failure.error.as_deref(), Some(message));
                assert_eq!(failure.total_files, 2);
                assert_eq!(failure.files, 1);
                assert_eq!(
                    failure.bytes,
                    (prior_size + acknowledged_batches * batch_bytes) as u64
                );
            }
        }
    }
}

#[test]
fn upload_paths_are_lexical_bounded_and_home_relative() {
    assert!(resolve_kitty_upload_path("../secret").is_err());
    assert!(resolve_kitty_upload_path("/tmp//secret").is_err());
    assert!(resolve_kitty_upload_path("/tmp/./secret").is_err());
    assert!(resolve_kitty_upload_path("/tmp/line\nbreak").is_err());
    for unsafe_display in ['\u{202e}', '\u{061c}', '\u{2028}', '\u{2029}'] {
        assert!(resolve_kitty_upload_path(&format!("/tmp/a{unsafe_display}b")).is_err());
    }
    assert!(
        resolve_kitty_upload_path("/tmp/secret")
            .unwrap()
            .is_absolute()
    );
    let relative = resolve_kitty_upload_path("project/file").unwrap();
    assert!(relative.is_absolute());
    assert!(relative.ends_with("project/file"));
    let home = directories::BaseDirs::new()
        .unwrap()
        .home_dir()
        .to_path_buf();
    assert_eq!(resolve_home_relative_path("~").unwrap(), home);
    assert_eq!(
        resolve_home_relative_path("~/a/b").unwrap(),
        home.join("a/b")
    );
    assert_eq!(
        resolve_home_relative_path("/~/a").unwrap(),
        PathBuf::from("/~/a")
    );
    for rejected in ["", "/", "~/", "~//etc", "/a/../b", "a/\0b"] {
        assert!(
            resolve_home_relative_path(rejected).is_err(),
            "{rejected:?}"
        );
    }
    let debug = format!(
        "{:?}",
        UploadPath {
            file_id: "f1".into(),
            requested_name: "/tmp/private-name".into(),
            path: PathBuf::from("/tmp/private-name"),
        }
    );
    assert!(!debug.contains("private-name"));
}
