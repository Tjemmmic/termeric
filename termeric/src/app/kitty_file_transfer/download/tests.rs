use super::*;
use flate2::{Compression, write::ZlibEncoder};
use tempfile::tempdir;

#[test]
fn worker_shutdown_cleans_staging_and_does_not_process_queued_finish() {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Exit {
        Cancel,
        DeliveryRejected,
        Disconnect,
        Timeout,
        InvalidData,
        Commit,
    }
    for exit in [
        Exit::Cancel,
        Exit::DeliveryRejected,
        Exit::Disconnect,
        Exit::Timeout,
        Exit::InvalidData,
        Exit::Commit,
    ] {
        for payload in [b"".as_slice(), b"staged payload"] {
            let temp = tempdir().unwrap();
            let destination = temp.path().join("received.bin");
            let unrelated = temp.path().join("keep.bin");
            fs::write(&unrelated, b"keep").unwrap();
            let cancellation = Arc::new(AtomicBool::new(false));
            let (sender, receiver) = sync_channel(4);
            let mut metadata = command(
                KittyFileTransferAction::File,
                "f",
                Some(&destination),
                KittyFileTransferType::Regular,
                &[],
            );
            metadata.size = Some(payload.len() as i64);
            sender.send(metadata).unwrap();
            sender
                .send(command(
                    KittyFileTransferAction::EndData,
                    "f",
                    None,
                    KittyFileTransferType::Regular,
                    payload,
                ))
                .unwrap();
            if exit == Exit::InvalidData {
                sender
                    .send(command(
                        KittyFileTransferAction::Data,
                        "f",
                        None,
                        KittyFileTransferType::Regular,
                        b"unexpected",
                    ))
                    .unwrap();
            }
            if !matches!(exit, Exit::Disconnect | Exit::Timeout) {
                sender
                    .send(command(
                        KittyFileTransferAction::Finish,
                        "",
                        None,
                        KittyFileTransferType::Regular,
                        &[],
                    ))
                    .unwrap();
            }
            // Keep a sender only for the actual inactivity-expiry case.
            let retained_sender = (exit == Exit::Timeout).then_some(sender);
            let mut completions = Vec::new();
            run_worker(
                receiver,
                "session".into(),
                true,
                cancellation.clone(),
                Duration::from_millis(1),
                |completion| {
                    completions.push(completion);
                    if completions.len() == 2 {
                        // Verify that the case actually staged a file,
                        // including an empty file, before shutting down.
                        let staged: Vec<_> = fs::read_dir(temp.path())
                            .unwrap()
                            .map(|entry| entry.unwrap().path())
                            .filter(|path| path != &unrelated)
                            .collect();
                        assert_eq!(staged.len(), 1, "{exit:?}");
                        assert_ne!(staged[0], destination);
                        assert_eq!(fs::read(&staged[0]).unwrap(), payload);
                        assert!(!destination.exists());
                        if exit == Exit::Cancel {
                            cancellation.store(true, Ordering::Release);
                        }
                        if exit == Exit::DeliveryRejected {
                            return false;
                        }
                    }
                    true
                },
            );
            drop(retained_sender);
            assert_eq!(fs::read(&unrelated).unwrap(), b"keep");
            let remaining: Vec<_> = fs::read_dir(temp.path())
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .filter(|path| path != &unrelated)
                .collect();
            if exit == Exit::Commit {
                assert_eq!(remaining, vec![destination.clone()]);
                assert_eq!(fs::read(&destination).unwrap(), payload);
                assert_eq!(completions.len(), 3);
                assert!(
                    matches!(completions[2].disposition, WorkerDisposition::Finished { files: 1, bytes } if bytes == payload.len() as u64)
                );
            } else {
                assert!(remaining.is_empty(), "{exit:?}: {remaining:?}");
                assert!(!destination.exists());
                assert!(completions.iter().all(|completion| !matches!(
                    completion.disposition,
                    WorkerDisposition::Finished { .. }
                )));
                if matches!(exit, Exit::Timeout | Exit::InvalidData) {
                    assert_eq!(completions.len(), 3);
                    let final_reply = &completions[2];
                    assert_eq!(final_reply.reply_uses_8bit_controls, exit == Exit::Timeout);
                    let WorkerDisposition::Status {
                        status,
                        terminal,
                        files,
                        bytes,
                    } = &final_reply.disposition
                    else {
                        panic!("expected terminal error");
                    };
                    assert!(*terminal);
                    assert_eq!(*files, 1);
                    assert_eq!(*bytes, payload.len() as u64);
                    assert_eq!(
                        status.code,
                        if exit == Exit::Timeout {
                            KittyFileTransferStatusCode::TimedOut
                        } else {
                            KittyFileTransferStatusCode::Invalid
                        }
                    );
                } else {
                    assert_eq!(completions.len(), 2);
                }
            }
        }
    }
}

fn command(
    action: KittyFileTransferAction,
    file_id: &str,
    path: Option<&Path>,
    file_type: KittyFileTransferType,
    data: &[u8],
) -> KittyFileTransferCommand {
    let is_metadata = action == KittyFileTransferAction::File;
    KittyFileTransferCommand {
        file_type: if is_metadata {
            file_type
        } else {
            KittyFileTransferType::Regular
        },
        file_id: file_id.into(),
        mtime_ns: is_metadata.then_some(1_700_000_000_123_456_789),
        permissions: is_metadata.then_some(if file_type == KittyFileTransferType::Directory {
            0o750
        } else {
            0o640
        }),
        size: is_metadata.then(|| path.map_or(0, |_| data.len() as i64)),
        name: path.map(|path| path.to_string_lossy().into_owned()),
        status: is_metadata.then(|| file_id.into()),
        data: data.into(),
        ..test_transfer_command(action, "session")
    }
}

fn completed_filesystem_fixture(root: &Path) -> (DownloadTransaction, Vec<CreatedPath>) {
    let cancellation = AtomicBool::new(false);
    let mut transaction = DownloadTransaction::new("session".into());
    for (id, relative, kind, payload) in [
        (
            "d",
            "tree",
            KittyFileTransferType::Directory,
            b"".as_slice(),
        ),
        (
            "c",
            "tree/child",
            KittyFileTransferType::Directory,
            b"".as_slice(),
        ),
        (
            "f",
            "tree/child/file",
            KittyFileTransferType::Regular,
            b"payload".as_slice(),
        ),
        (
            "s",
            "tree/child/symlink",
            KittyFileTransferType::Symlink,
            b"fid:f".as_slice(),
        ),
        (
            "h",
            "tree/child/hardlink",
            KittyFileTransferType::HardLink,
            b"f".as_slice(),
        ),
    ] {
        let path = root.join(relative);
        let mut metadata = command(KittyFileTransferAction::File, id, Some(&path), kind, b"");
        metadata.permissions = Some(0o000);
        if kind == KittyFileTransferType::Regular {
            metadata.size = Some(payload.len() as i64);
        }
        transaction.handle(metadata, &cancellation).unwrap();
        if kind != KittyFileTransferType::Directory {
            transaction
                .handle(
                    command(KittyFileTransferAction::EndData, id, None, kind, payload),
                    &cancellation,
                )
                .unwrap();
        }
    }
    let mut created = Vec::new();
    transaction
        .commit_inner(&cancellation, &mut created)
        .unwrap();
    assert_eq!(created.len(), 5);
    for entry in &created {
        let metadata = entry._identity_guard.as_ref().unwrap().metadata().unwrap();
        assert_eq!(
            (metadata.dev(), metadata.ino()),
            (entry.device, entry.inode)
        );
        if entry.kind == CreatedKind::Directory {
            assert_eq!(metadata.permissions().mode() & 0o777, 0o000);
        }
    }
    (transaction, created)
}

#[test]
fn cancellation_after_filesystem_commit_rolls_back_instead_of_finishing() {
    let root = tempdir().unwrap();
    let existing = root.path().join("existing");
    fs::write(&existing, b"keep").unwrap();
    let (transaction, created) = completed_filesystem_fixture(root.path());
    let cancellation = AtomicBool::new(true);
    let result = transaction.complete_commit(Ok(()), &cancellation, &created);
    assert!(
        matches!(result, Err(ref error) if error.code == KittyFileTransferStatusCode::Canceled)
    );
    assert!(!root.path().join("tree").exists());
    assert_eq!(fs::read(&existing).unwrap(), b"keep");
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn commit_completion_preserves_success_and_does_not_revoke_it_later() {
    let root = tempdir().unwrap();
    let (transaction, created) = completed_filesystem_fixture(root.path());
    let cancellation = AtomicBool::new(false);
    let result = transaction
        .complete_commit(Ok(()), &cancellation, &created)
        .unwrap();
    assert!(matches!(
        result,
        TransactionResult::Finished {
            files: 5,
            bytes: 13
        }
    ));
    cancellation.store(true, Ordering::Release);
    drop(transaction);
    assert!(root.path().join("tree").is_dir());
    assert_eq!(
        fs::metadata(root.path().join("tree"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o000
    );
    // Test-only cleanup: cancellation after acceptance does not request
    // this rollback in production.
    rollback_paths(&created);
    assert!(!root.path().join("tree").exists());
}

#[test]
fn commit_failure_preserves_original_error_and_rolls_back_when_canceled() {
    for canceled in [false, true] {
        let root = tempdir().unwrap();
        let (transaction, created) = completed_filesystem_fixture(root.path());
        let cancellation = AtomicBool::new(canceled);
        let result = transaction.complete_commit(
            Err(TransactionError::invalid("f", "original failure")),
            &cancellation,
            &created,
        );
        let Err(error) = result else {
            panic!("failed commit must not finish");
        };
        assert_eq!(error.code, KittyFileTransferStatusCode::Invalid);
        assert_eq!(error.file_id, "f");
        assert_eq!(error.message, "original failure");
        assert!(!root.path().join("tree").exists());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }
}

#[test]
fn directory_final_permissions_do_not_retain_assembly_access() {
    for requested in [0o000, 0o055, 0o555, 0o2750] {
        let root = tempdir().unwrap();
        let directory = root.path().join("tree");
        let child = directory.join("child");
        let file = child.join("file");
        let cancel = AtomicBool::new(false);
        let mut transaction = DownloadTransaction::new("session".into());
        for (id, path) in [("d", &directory), ("c", &child)] {
            let mut metadata = command(
                KittyFileTransferAction::File,
                id,
                Some(path),
                KittyFileTransferType::Directory,
                b"",
            );
            metadata.permissions = Some(requested);
            transaction.handle(metadata, &cancel).unwrap();
        }
        let mut metadata = command(
            KittyFileTransferAction::File,
            "f",
            Some(&file),
            KittyFileTransferType::Regular,
            b"content",
        );
        metadata.data = Box::default();
        transaction.handle(metadata, &cancel).unwrap();
        transaction
            .handle(
                command(
                    KittyFileTransferAction::EndData,
                    "f",
                    None,
                    KittyFileTransferType::Regular,
                    b"content",
                ),
                &cancel,
            )
            .unwrap();
        transaction
            .handle(
                command(
                    KittyFileTransferAction::Finish,
                    "",
                    None,
                    KittyFileTransferType::Regular,
                    b"",
                ),
                &cancel,
            )
            .unwrap();
        // Restore access top-down only after observing the actual final
        // mode, so 000/055 directories are inspectable and removable.
        for path in [&directory, &child] {
            let actual = fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777;
            fs::set_permissions(path, Permissions::from_mode(0o700)).unwrap();
            assert_eq!(actual, requested & 0o777, "{path:?}");
        }
        assert_eq!(fs::read(&file).unwrap(), b"content");
    }
}

#[test]
fn rollback_restores_owned_directory_access_before_removing_children() {
    let root = tempdir().unwrap();
    let parent = root.path().join("parent");
    let child = parent.join("child");
    let file = child.join("file");
    let mut created = Vec::new();
    for path in [&parent, &child] {
        fs::create_dir(path).unwrap();
        record_created(path, CreatedKind::Directory, &mut created).unwrap();
    }
    fs::write(&file, b"payload").unwrap();
    record_created(&file, CreatedKind::File, &mut created).unwrap();
    for path in [&child, &parent] {
        fs::set_permissions(path, Permissions::from_mode(0o000)).unwrap();
    }
    rollback_paths(&created);
    let removed = fs::symlink_metadata(&parent).is_err();
    // A failing implementation must not leave inaccessible test data.
    if !removed {
        fs::set_permissions(&parent, Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&child, Permissions::from_mode(0o700)).unwrap();
        rollback_paths(&created);
    }
    assert!(
        removed,
        "rollback left a finalized restrictive directory tree"
    );
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn unverified_directory_handle_never_changes_replacement_permissions() {
    for symbolic in [false, true] {
        let root = tempdir().unwrap();
        let path = root.path().join("owned");
        let moved = root.path().join("moved");
        let target = root.path().join("target");
        let mut created = Vec::new();
        create_directory(&path, &mut created).unwrap();
        fs::rename(&path, &moved).unwrap();
        let replacement = if symbolic { &target } else { &path };
        fs::create_dir(replacement).unwrap();
        fs::set_permissions(replacement, Permissions::from_mode(0o750)).unwrap();
        if symbolic {
            std::os::unix::fs::symlink(&target, &path).unwrap();
        }
        created[0]._identity_guard = None;
        assert!(ensure_directory_access(&mut created[0]).is_err());
        created[0]._identity_guard = Some(open_directory(replacement).unwrap());
        assert!(ensure_directory_access(&mut created[0]).is_err());
        assert_eq!(
            fs::metadata(replacement).unwrap().permissions().mode() & 0o777,
            0o750
        );
        assert_eq!(
            fs::metadata(&moved).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
}

#[test]
fn opath_directory_access_recovery_ignores_replaced_paths() {
    for symbolic in [false, true] {
        let root = tempdir().unwrap();
        let path = root.path().join("owned");
        let moved = root.path().join("moved");
        let target = root.path().join("target");
        let mut created = Vec::new();
        create_directory(&path, &mut created).unwrap();
        // Force this path even for a privileged test runner that can
        // open unreadable directories without the O_PATH fallback.
        created[0]._identity_guard = Some(
            fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW)
                .open(&path)
                .unwrap(),
        );
        fs::set_permissions(&path, Permissions::from_mode(0o000)).unwrap();
        fs::rename(&path, &moved).unwrap();
        let replacement = if symbolic { &target } else { &path };
        fs::create_dir(replacement).unwrap();
        fs::set_permissions(replacement, Permissions::from_mode(0o750)).unwrap();
        if symbolic {
            std::os::unix::fs::symlink(&target, &path).unwrap();
        }
        let before = fs::metadata(replacement).unwrap();
        ensure_directory_access(&mut created[0]).unwrap();
        assert_eq!(
            fs::metadata(&moved).unwrap().permissions().mode() & 0o777,
            0o700
        );
        // Finalization requires the upgraded usable handle, not merely
        // a successful chmod through the metadata-only handle.
        finalize_directory(&path, 0o555, Some(-1), &created).unwrap();
        let owned = fs::metadata(&moved).unwrap();
        assert_eq!(owned.permissions().mode() & 0o777, 0o555);
        assert_eq!((owned.mtime(), owned.mtime_nsec()), (-1, 999_999_999));
        rollback_paths(&created);
        let after = fs::metadata(replacement).unwrap();
        assert_eq!(after.permissions().mode(), before.permissions().mode());
        assert_eq!(
            (after.mtime(), after.mtime_nsec()),
            (before.mtime(), before.mtime_nsec())
        );
        assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
    }
}

#[test]
fn directory_assembly_access_survives_restrictive_umask() {
    const CHILD: &str = "TERMERIC_TEST_TRANSFER_DIRECTORY_UMASK";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "app::kitty_file_transfer::download::tests::directory_assembly_access_survives_restrictive_umask", "--nocapture"])
                    .env(CHILD, "1")
                    .output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let root = tempdir().unwrap();
    // Change the process-wide umask only in this isolated test process.
    // SAFETY: umask has no pointer arguments; no sibling test runs here.
    let previous = unsafe { libc::umask(0o777) };
    let mut created = Vec::new();
    let path = root.path().join("private");
    let result = create_directory(&path, &mut created);
    // SAFETY: restore the original process umask before assertions/cleanup.
    unsafe { libc::umask(previous) };
    result.unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert!(created[0]._identity_guard.is_some());
    rollback_paths(&created);
    assert!(!path.exists());
}

#[test]
fn directory_finalization_never_changes_a_replacement_path() {
    for symlink_replacement in [false, true] {
        let root = tempdir().unwrap();
        let path = root.path().join("owned");
        let moved = root.path().join("moved");
        let target = root.path().join("target");
        let mut created = Vec::new();
        create_directory(&path, &mut created).unwrap();
        fs::rename(&path, &moved).unwrap();
        let replacement = if symlink_replacement { &target } else { &path };
        fs::create_dir(replacement).unwrap();
        fs::set_permissions(replacement, Permissions::from_mode(0o750)).unwrap();
        apply_mtime(replacement, Some(1_700_000_000_123_456_789), false).unwrap();
        let before = fs::metadata(replacement).unwrap();
        if symlink_replacement {
            std::os::unix::fs::symlink(&target, &path).unwrap();
        }
        finalize_directory(&path, 0o555, Some(-1), &created).unwrap();
        let owned = fs::metadata(&moved).unwrap();
        let after = fs::metadata(replacement).unwrap();
        // Restore cleanup access before asserting, including on the
        // vulnerable path-based implementation used by the red test.
        created[0]
            ._identity_guard
            .as_ref()
            .unwrap()
            .set_permissions(Permissions::from_mode(0o700))
            .unwrap();
        fs::set_permissions(replacement, Permissions::from_mode(0o700)).unwrap();
        assert_eq!(owned.permissions().mode() & 0o777, 0o555);
        assert_eq!((owned.mtime(), owned.mtime_nsec()), (-1, 999_999_999));
        assert_eq!(after.permissions().mode(), before.permissions().mode());
        assert_eq!(
            (after.mtime(), after.mtime_nsec()),
            (before.mtime(), before.mtime_nsec())
        );
        assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
    }
}

#[test]
fn directory_finalization_requires_its_recorded_handle() {
    for invalid in ["unrecorded", "missing", "mismatched", "wrong-kind"] {
        let root = tempdir().unwrap();
        let path = root.path().join("owned");
        let other = root.path().join("other");
        let mut created = Vec::new();
        create_directory(&path, &mut created).unwrap();
        fs::create_dir(&other).unwrap();
        let before = fs::metadata(&path).unwrap();
        let other_before = fs::metadata(&other).unwrap();
        match invalid {
            "unrecorded" => created.clear(),
            "missing" => created[0]._identity_guard = None,
            "mismatched" => created[0]._identity_guard = Some(open_directory(&other).unwrap()),
            "wrong-kind" => created[0].kind = CreatedKind::File,
            _ => unreachable!(),
        }
        assert!(
            finalize_directory(&path, 0o000, Some(-1), &created).is_err(),
            "{invalid}"
        );
        for (path, before) in [(&path, before), (&other, other_before)] {
            let after = fs::metadata(path).unwrap();
            assert_eq!(
                after.permissions().mode(),
                before.permissions().mode(),
                "{invalid}"
            );
            assert_eq!(
                (after.mtime(), after.mtime_nsec()),
                (before.mtime(), before.mtime_nsec()),
                "{invalid}"
            );
        }
    }
}

#[test]
fn directory_rollback_does_not_change_replacement_permissions() {
    for symlink_replacement in [false, true] {
        let root = tempdir().unwrap();
        let path = root.path().join("owned");
        let moved = root.path().join("moved");
        let target = root.path().join("target");
        let mut created = Vec::new();
        create_directory(&path, &mut created).unwrap();
        fs::set_permissions(&path, Permissions::from_mode(0o000)).unwrap();
        fs::rename(&path, &moved).unwrap();
        let replacement = if symlink_replacement { &target } else { &path };
        fs::create_dir(replacement).unwrap();
        fs::set_permissions(replacement, Permissions::from_mode(0o555)).unwrap();
        if symlink_replacement {
            std::os::unix::fs::symlink(&target, &path).unwrap();
        }
        rollback_paths(&created);
        assert_eq!(
            fs::metadata(replacement).unwrap().permissions().mode() & 0o777,
            0o555
        );
        assert_eq!(
            fs::metadata(&moved).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert!(fs::symlink_metadata(&path).is_ok());
        fs::set_permissions(replacement, Permissions::from_mode(0o700)).unwrap();
    }
}

#[test]
fn directory_symlinks_resolve_forward_relative_and_absolute_targets() {
    let root = tempdir().unwrap();
    let directory = root.path().join("tree");
    let file = directory.join("file");
    let relative = root.path().join("links/relative");
    let absolute = root.path().join("absolute");
    let cancel = AtomicBool::new(false);
    let mut transaction = DownloadTransaction::new("session".into());
    // Announce both links before the directory they name. Resolution
    // belongs to finish, once the complete transfer inventory exists.
    for (id, path, target) in [
        ("r", &relative, b"fid:d".as_slice()),
        ("a", &absolute, b"fid_abs:d".as_slice()),
    ] {
        transaction
            .handle(
                command(
                    KittyFileTransferAction::File,
                    id,
                    Some(path),
                    KittyFileTransferType::Symlink,
                    b"",
                ),
                &cancel,
            )
            .unwrap();
        transaction
            .handle(
                command(
                    KittyFileTransferAction::EndData,
                    id,
                    None,
                    KittyFileTransferType::Symlink,
                    target,
                ),
                &cancel,
            )
            .unwrap();
    }
    let mut dir = command(
        KittyFileTransferAction::File,
        "d",
        Some(&directory),
        KittyFileTransferType::Directory,
        b"",
    );
    dir.permissions = Some(0o750);
    transaction.handle(dir, &cancel).unwrap();
    let mut metadata = command(
        KittyFileTransferAction::File,
        "f",
        Some(&file),
        KittyFileTransferType::Regular,
        b"content",
    );
    metadata.data = Box::default();
    metadata.parent = Some("d".into());
    transaction.handle(metadata, &cancel).unwrap();
    transaction
        .handle(
            command(
                KittyFileTransferAction::EndData,
                "f",
                None,
                KittyFileTransferType::Regular,
                b"content",
            ),
            &cancel,
        )
        .unwrap();
    for path in [&directory, &relative, &absolute] {
        assert!(
            fs::symlink_metadata(path).is_err(),
            "published early: {path:?}"
        );
    }
    assert!(matches!(
        transaction
            .handle(
                command(
                    KittyFileTransferAction::Finish,
                    "",
                    None,
                    KittyFileTransferType::Regular,
                    b"",
                ),
                &cancel
            )
            .unwrap(),
        TransactionResult::Finished {
            files: 4,
            bytes: 21
        }
    ));
    assert_eq!(fs::read_link(&relative).unwrap(), PathBuf::from("../tree"));
    assert_eq!(fs::read_link(&absolute).unwrap(), directory);
    for path in [&relative, &absolute] {
        assert!(fs::symlink_metadata(path).unwrap().file_type().is_symlink());
        assert_eq!(fs::read(path.join("file")).unwrap(), b"content");
    }
    assert_eq!(
        fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
        0o750
    );
}

#[test]
fn directory_hardlinks_and_unknown_symlink_ids_fail_with_rollback() {
    for (kind, target) in [
        (KittyFileTransferType::HardLink, b"d".as_slice()),
        (KittyFileTransferType::Symlink, b"fid:unknown".as_slice()),
        (
            KittyFileTransferType::Symlink,
            b"fid_abs:unknown".as_slice(),
        ),
    ] {
        let root = tempdir().unwrap();
        let directory = root.path().join("tree");
        let link = root.path().join("link");
        let cancel = AtomicBool::new(false);
        let mut transaction = DownloadTransaction::new("session".into());
        transaction
            .handle(
                command(
                    KittyFileTransferAction::File,
                    "d",
                    Some(&directory),
                    KittyFileTransferType::Directory,
                    b"",
                ),
                &cancel,
            )
            .unwrap();
        transaction
            .handle(
                command(KittyFileTransferAction::File, "l", Some(&link), kind, b""),
                &cancel,
            )
            .unwrap();
        transaction
            .handle(
                command(KittyFileTransferAction::EndData, "l", None, kind, target),
                &cancel,
            )
            .unwrap();
        let error = transaction
            .handle(
                command(
                    KittyFileTransferAction::Finish,
                    "",
                    None,
                    KittyFileTransferType::Regular,
                    b"",
                ),
                &cancel,
            )
            .err()
            .expect("invalid target must fail");
        assert_eq!(error.file_id, "l");
        assert_eq!(error.code, KittyFileTransferStatusCode::Invalid);
        drop(transaction);
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }
}

#[test]
fn signed_modification_times_survive_transaction_commit() {
    for mtime_ns in [-1_i64, -1_000_000_001, 0, 1_700_000_000_123_456_789] {
        let root = tempdir().unwrap();
        let directory = root.path().join("tree");
        let file = directory.join("file");
        let hardlink = directory.join("hard");
        let symlink = directory.join("symbolic");
        let cancel = AtomicBool::new(false);
        let mut transaction = DownloadTransaction::new("session".into());
        for (id, path, kind, data, timestamp) in [
            (
                "d",
                &directory,
                KittyFileTransferType::Directory,
                b"".as_slice(),
                mtime_ns,
            ),
            (
                "f",
                &file,
                KittyFileTransferType::Regular,
                b"content".as_slice(),
                mtime_ns,
            ),
            (
                "h",
                &hardlink,
                KittyFileTransferType::HardLink,
                b"f".as_slice(),
                mtime_ns,
            ),
            // A distinct link timestamp verifies AT_SYMLINK_NOFOLLOW:
            // setting it must not change the regular file's timestamp.
            (
                "s",
                &symlink,
                KittyFileTransferType::Symlink,
                b"fid:f".as_slice(),
                mtime_ns - 1,
            ),
        ] {
            let mut metadata = command(KittyFileTransferAction::File, id, Some(path), kind, b"");
            metadata.mtime_ns = Some(timestamp);
            metadata.size = Some(if kind == KittyFileTransferType::Regular {
                data.len() as i64
            } else {
                0
            });
            if id != "d" {
                metadata.parent = Some("d".into());
            }
            transaction.handle(metadata, &cancel).unwrap();
            if kind != KittyFileTransferType::Directory {
                transaction
                    .handle(
                        command(KittyFileTransferAction::EndData, id, None, kind, data),
                        &cancel,
                    )
                    .unwrap();
            }
        }
        transaction
            .handle(
                command(
                    KittyFileTransferAction::Finish,
                    "",
                    None,
                    KittyFileTransferType::Regular,
                    b"",
                ),
                &cancel,
            )
            .unwrap();
        for (path, expected) in [
            (&directory, mtime_ns),
            (&file, mtime_ns),
            (&hardlink, mtime_ns),
            (&symlink, mtime_ns - 1),
        ] {
            let metadata = fs::symlink_metadata(path).unwrap();
            assert_eq!(
                metadata.mtime(),
                expected.div_euclid(1_000_000_000),
                "{path:?}"
            );
            assert_eq!(
                metadata.mtime_nsec(),
                expected.rem_euclid(1_000_000_000),
                "{path:?}"
            );
        }
        assert_eq!(fs::read(&file).unwrap(), b"content");
        assert_eq!(
            fs::metadata(&file).unwrap().ino(),
            fs::metadata(&hardlink).unwrap().ino()
        );
        assert_eq!(fs::read_link(&symlink).unwrap(), PathBuf::from("file"));
    }
}

#[test]
fn expanded_creation_budget_counts_implicit_parents_once() {
    let root = tempdir().unwrap();
    let parent = root.path().join("missing-parent");
    let mut paths: HashSet<_> = (0..MAX_KITTY_DOWNLOAD_FILES - 1)
        .map(|index| parent.join(format!("file-{index}")))
        .collect();
    assert!(validate_creation_budget(&paths).is_ok());
    paths.insert(parent.clone());
    assert!(
        validate_creation_budget(&paths).is_ok(),
        "explicit parent must not count twice"
    );
    paths.remove(&parent);
    paths.insert(parent.join("one-too-many"));
    let error = validate_creation_budget(&paths).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::FileTooLarge);
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    fs::create_dir(&parent).unwrap();
    assert!(
        validate_creation_budget(&paths).is_ok(),
        "existing parents do not consume creation budget"
    );
}

#[test]
fn excessive_implicit_tree_is_rejected_before_any_destination_creation() {
    let root = tempdir().unwrap();
    let mut path = root.path().to_owned();
    for _ in 0..=MAX_KITTY_DOWNLOAD_FILES {
        path.push("d");
    }
    let cancellation = AtomicBool::new(false);
    let mut transaction = DownloadTransaction::new("session".into());
    transaction
        .handle(
            command(
                KittyFileTransferAction::File,
                "d",
                Some(&path),
                KittyFileTransferType::Directory,
                b"",
            ),
            &cancellation,
        )
        .unwrap();
    let result = transaction.handle(
        command(
            KittyFileTransferAction::Finish,
            "",
            None,
            KittyFileTransferType::Regular,
            b"",
        ),
        &cancellation,
    );
    assert!(
        matches!(result, Err(ref error) if error.code == KittyFileTransferStatusCode::TooLarge)
    );
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn runtime_creation_budget_bounds_growth_and_preserves_existing_state() {
    let root = tempdir().unwrap();
    // Synthetic bookkeeping exercises the boundary without creating a
    // thousand files or relying on the test runner's descriptor limit.
    let mut created: Vec<_> = (0..MAX_KITTY_DOWNLOAD_FILES - 1)
        .map(|_| CreatedPath {
            path: root.path().join("uncreated"),
            device: 0,
            inode: 0,
            kind: CreatedKind::File,
            _identity_guard: None,
        })
        .collect();
    let owned_start = created.len();
    let first = root.path().join("first");
    let error = create_missing_parents(&first.join("second/file"), &HashSet::new(), &mut created)
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::FileTooLarge);
    assert_eq!(created.len(), MAX_KITTY_DOWNLOAD_FILES);
    assert!(first.is_dir());
    assert!(!first.join("second").exists());
    let blocked = root.path().join("blocked");
    assert_eq!(
        PublicationTarget::open(&blocked)
            .unwrap()
            .create_directory(&mut created)
            .unwrap_err()
            .kind(),
        io::ErrorKind::FileTooLarge
    );
    assert_eq!(
        PublicationTarget::open(&blocked)
            .unwrap()
            .create_symlink(Path::new("target"), &mut created)
            .unwrap_err()
            .kind(),
        io::ErrorKind::FileTooLarge
    );
    assert_eq!(
        create_hard_link(Path::new("unused"), &blocked, &mut created)
            .unwrap_err()
            .kind(),
        io::ErrorKind::FileTooLarge
    );
    assert_eq!(
        ensure_creation_capacity(&created).unwrap_err().kind(),
        io::ErrorKind::FileTooLarge
    );
    assert!(!blocked.exists());
    assert_eq!(created.len(), MAX_KITTY_DOWNLOAD_FILES);
    rollback_paths(&created[owned_start..]);
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn staging_rejects_symlink_ancestors_and_uses_nearest_existing_parent() {
    let root = tempdir().unwrap();
    let real = root.path().join("real");
    fs::create_dir_all(real.join("nested")).unwrap();
    let link = root.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    for directory in [&link, &link.join("nested")] {
        assert!(StagingDirectory::open(directory).is_err());
        assert!(stage_file(&directory.join("destination")).is_err());
    }
    assert_eq!(fs::read_dir(&real).unwrap().count(), 1);
    assert_eq!(fs::read_dir(real.join("nested")).unwrap().count(), 0);
    let staged = stage_file(&real.join("missing/deeper/file")).unwrap();
    let parent = staged._parent.as_ref().unwrap().metadata().unwrap();
    let expected = fs::metadata(&real).unwrap();
    assert_eq!(
        (parent.dev(), parent.ino()),
        (expected.dev(), expected.ino())
    );
    assert!(!real.join("missing").exists());
    drop(staged);
    assert_eq!(fs::read_dir(&real).unwrap().count(), 1);
}

#[test]
fn pinned_staging_cleanup_runs_after_publication_success_and_failure() {
    for collision in [false, true] {
        let root = tempdir().unwrap();
        let parent = root.path().join("parent");
        let moved = root.path().join("moved");
        fs::create_dir(&parent).unwrap();
        let mut staged = stage_file(&parent.join("destination")).unwrap();
        staged.as_file_mut().write_all(b"approved payload").unwrap();
        prepare_staged_file(staged.as_file(), 0o400, Some(-1)).unwrap();
        let identity = staged.as_file().metadata().unwrap();
        let name = staged.path.file_name().unwrap().to_owned();
        fs::rename(&parent, &moved).unwrap();
        fs::create_dir(&parent).unwrap();
        let replacement = parent.join(&name);
        fs::write(&replacement, b"keep replacement").unwrap();
        let destination = root.path().join("published");
        if collision {
            fs::write(&destination, b"keep destination").unwrap();
        }
        let result = PublicationTarget::open(&destination)
            .unwrap()
            .publish(staged);
        if collision {
            assert_eq!(result.unwrap_err().kind(), io::ErrorKind::AlreadyExists);
            assert_eq!(fs::read(&destination).unwrap(), b"keep destination");
        } else {
            let published = result.unwrap();
            let after = published.metadata().unwrap();
            assert_eq!((after.dev(), after.ino()), (identity.dev(), identity.ino()));
            assert_eq!(after.permissions().mode() & 0o777, 0o400);
            assert_eq!((after.mtime(), after.mtime_nsec()), (-1, 999_999_999));
            assert_eq!(fs::read(&destination).unwrap(), b"approved payload");
        }
        assert!(!moved.join(&name).exists());
        assert_eq!(fs::read(&replacement).unwrap(), b"keep replacement");
    }
}

/// Make `root/ancestor/parent` and return the parent.
fn nested_parent(root: &Path) -> PathBuf {
    let parent = root.join("ancestor/parent");
    fs::create_dir_all(&parent).unwrap();
    parent
}

/// Where an ancestor-replacement race left things: the original
/// parent's new location, and the empty directory now reached through
/// its old path.
struct ReplacedParent {
    moved_parent: PathBuf,
    replacement_parent: PathBuf,
}

/// Move `root/ancestor/parent` (with `replace_ancestor`, its ancestor)
/// to `root/moved`, and put a fresh directory at the old path: a real
/// one, or with `symbolic` a symlink to a fresh `root/outside`.
fn replace_parent(root: &Path, replace_ancestor: bool, symbolic: bool) -> ReplacedParent {
    let ancestor = root.join("ancestor");
    let replaced = if replace_ancestor {
        ancestor.clone()
    } else {
        ancestor.join("parent")
    };
    let moved = root.join("moved");
    fs::rename(&replaced, &moved).unwrap();
    let outside = root.join("outside");
    let replacement = if symbolic { &outside } else { &replaced };
    fs::create_dir(replacement).unwrap();
    if symbolic {
        std::os::unix::fs::symlink(&outside, &replaced).unwrap();
    }
    if replace_ancestor {
        fs::create_dir(replacement.join("parent")).unwrap();
        ReplacedParent {
            moved_parent: moved.join("parent"),
            replacement_parent: replacement.join("parent"),
        }
    } else {
        ReplacedParent {
            moved_parent: moved,
            replacement_parent: replacement.clone(),
        }
    }
}

#[test]
fn staging_creation_and_cleanup_retain_replaced_ancestors() {
    for symbolic in [false, true] {
        for replace_ancestor in [false, true] {
            for replace_before_creation in [false, true] {
                let root = tempdir().unwrap();
                let parent = nested_parent(root.path());
                let mut directory = Some(StagingDirectory::open(&parent).unwrap());
                let staged = if replace_before_creation {
                    None
                } else {
                    Some(directory.take().unwrap().create_file().unwrap())
                };
                let ReplacedParent {
                    moved_parent,
                    replacement_parent,
                } = replace_parent(root.path(), replace_ancestor, symbolic);
                let mut staged =
                    staged.unwrap_or_else(|| directory.take().unwrap().create_file().unwrap());
                let original_file = moved_parent.join(staged.path.file_name().unwrap());
                assert!(
                    original_file.is_file(),
                    "staging must stay in pinned directory"
                );
                assert_eq!(fs::read_dir(&replacement_parent).unwrap().count(), 0);
                staged.as_file_mut().write_all(b"approved payload").unwrap();
                assert_eq!(fs::read(&original_file).unwrap(), b"approved payload");
                let replacement_file = replacement_parent.join(staged.path.file_name().unwrap());
                fs::write(&replacement_file, b"keep").unwrap();
                let before = fs::metadata(&replacement_file).unwrap();
                drop(staged);
                assert!(
                    !original_file.exists(),
                    "cleanup must use the retained parent"
                );
                let after = fs::metadata(&replacement_file).unwrap();
                assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
                assert_eq!(fs::read(&replacement_file).unwrap(), b"keep");
            }
        }
    }
}

#[test]
fn directory_creation_rejects_existing_entries_and_symlink_ancestors() {
    for kind in ["file", "directory", "symlink", "ancestor"] {
        let root = tempdir().unwrap();
        let path = root.path().join("destination");
        let outside = root.path().join("outside");
        fs::create_dir(&outside).unwrap();
        match kind {
            "file" => fs::write(&path, b"keep").unwrap(),
            "directory" => fs::create_dir(&path).unwrap(),
            "symlink" | "ancestor" => {
                std::os::unix::fs::symlink(&outside, &path).unwrap();
            }
            _ => unreachable!(),
        }
        let before = fs::symlink_metadata(&path).unwrap();
        let outside_before = fs::metadata(&outside).unwrap();
        let destination = if kind == "ancestor" {
            path.join("new")
        } else {
            path.clone()
        };
        let mut created = Vec::new();
        assert!(
            create_directory(&destination, &mut created).is_err(),
            "{kind}"
        );
        assert!(
            created.is_empty(),
            "no cleanup authority on failed creation"
        );
        let after = fs::symlink_metadata(&path).unwrap();
        assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
        assert_eq!(after.permissions().mode(), before.permissions().mode());
        let outside_after = fs::metadata(&outside).unwrap();
        assert_eq!(
            (outside_after.mtime(), outside_after.mtime_nsec()),
            (outside_before.mtime(), outside_before.mtime_nsec())
        );
        assert!(!outside.join("new").exists());
    }
}

#[test]
fn implicit_directories_keep_logical_rollback_keys_and_private_modes() {
    let root = tempdir().unwrap();
    let existing = root.path().join("existing");
    fs::create_dir(&existing).unwrap();
    fs::set_permissions(&existing, Permissions::from_mode(0o750)).unwrap();
    let parent = existing.join("first/second");
    let mut created = Vec::new();
    create_missing_parents(&parent.join("file"), &HashSet::new(), &mut created).unwrap();
    assert_eq!(created.len(), 2);
    for (entry, path) in created.iter().zip([existing.join("first"), parent]) {
        assert_eq!(entry.path, path);
        let metadata = fs::metadata(&path).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
        assert_eq!(
            (metadata.dev(), metadata.ino()),
            (entry.device, entry.inode)
        );
        assert!(entry._identity_guard.is_some());
    }
    rollback_paths(&created);
    assert!(!existing.join("first").exists());
    assert!(existing.is_dir());
    assert_eq!(
        fs::metadata(&existing).unwrap().permissions().mode() & 0o777,
        0o750
    );
}

#[test]
fn directory_creation_retains_parent_after_ancestor_replacement() {
    for symbolic in [false, true] {
        for replace_ancestor in [false, true] {
            let root = tempdir().unwrap();
            let parent = nested_parent(root.path());
            let path = parent.join("new-directory");
            let destination = PublicationTarget::open(&path).unwrap();
            let ReplacedParent {
                moved_parent,
                replacement_parent,
            } = replace_parent(root.path(), replace_ancestor, symbolic);
            let before = fs::metadata(&replacement_parent).unwrap();
            let mut created = Vec::new();
            destination.create_directory(&mut created).unwrap();
            let moved_directory = moved_parent.join("new-directory");
            let original = fs::metadata(&moved_directory).unwrap();
            assert!(original.is_dir());
            assert_eq!(original.permissions().mode() & 0o777, 0o700);
            assert!(fs::symlink_metadata(&path).is_err());
            assert_eq!(created.len(), 1);
            assert_eq!(created[0].path, path);
            assert_eq!(
                (original.dev(), original.ino()),
                (created[0].device, created[0].inode)
            );
            finalize_directory(&path, 0o555, Some(-1), &created).unwrap();
            let finalized = fs::metadata(&moved_directory).unwrap();
            assert_eq!(finalized.permissions().mode() & 0o777, 0o555);
            assert_eq!(
                (finalized.mtime(), finalized.mtime_nsec()),
                (-1, 999_999_999)
            );
            rollback_paths(&created);
            assert!(
                moved_directory.is_dir(),
                "moved entries may remain after rollback"
            );
            let after = fs::metadata(&replacement_parent).unwrap();
            assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
            assert_eq!(after.permissions().mode(), before.permissions().mode());
            assert_eq!(
                (after.mtime(), after.mtime_nsec()),
                (before.mtime(), before.mtime_nsec())
            );
        }
    }
}

#[test]
fn symbolic_link_publication_preserves_literal_targets_and_rollback_keys() {
    let root = tempdir().unwrap();
    for target in [
        PathBuf::from("missing-relative"),
        PathBuf::from("../outside"),
        root.path().join("missing-absolute"),
    ] {
        let path = root.path().join("link");
        let mut created = Vec::new();
        PublicationTarget::open(&path)
            .unwrap()
            .create_symlink(&target, &mut created)
            .unwrap();
        assert_eq!(fs::read_link(&path).unwrap(), target);
        assert_eq!(created[0].path, path);
        let before = fs::symlink_metadata(&path).unwrap();
        let error = PublicationTarget::open(&path)
            .unwrap()
            .create_symlink(Path::new("different"), &mut created)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(created.len(), 1);
        let after = fs::symlink_metadata(&path).unwrap();
        assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
        assert_eq!(fs::read_link(&path).unwrap(), target);
        rollback_paths(&created);
        assert!(fs::symlink_metadata(&path).is_err());
    }
}

#[test]
fn symbolic_link_publication_never_replaces_existing_entries() {
    for directory in [false, true] {
        let root = tempdir().unwrap();
        let path = root.path().join("existing");
        if directory {
            fs::create_dir(&path).unwrap();
        } else {
            fs::write(&path, b"keep").unwrap();
        }
        let before = fs::symlink_metadata(&path).unwrap();
        let mut created = Vec::new();
        let error = PublicationTarget::open(&path)
            .unwrap()
            .create_symlink(Path::new("elsewhere"), &mut created)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert!(created.is_empty());
        let after = fs::symlink_metadata(&path).unwrap();
        assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
        assert_eq!(after.file_type(), before.file_type());
        if !directory {
            assert_eq!(fs::read(&path).unwrap(), b"keep");
        }
    }
}

#[test]
fn symbolic_link_creation_retains_parent_after_ancestor_replacement() {
    for symbolic in [false, true] {
        for replace_ancestor in [false, true] {
            let root = tempdir().unwrap();
            let parent = nested_parent(root.path());
            let path = parent.join("link");
            let destination = PublicationTarget::open(&path).unwrap();
            let ReplacedParent {
                moved_parent,
                replacement_parent,
            } = replace_parent(root.path(), replace_ancestor, symbolic);
            let before = fs::metadata(&replacement_parent).unwrap();
            let target = root.path().join("target");
            fs::write(&target, b"keep target").unwrap();
            let target_before = fs::metadata(&target).unwrap();
            let mut created = Vec::new();
            destination.create_symlink(&target, &mut created).unwrap();
            let moved_link = moved_parent.join("link");
            assert_eq!(fs::read_link(&moved_link).unwrap(), target);
            assert!(fs::symlink_metadata(replacement_parent.join("link")).is_err());
            assert_eq!(created.len(), 1);
            assert_eq!(created[0].path, path);
            apply_link_mtime(&created[0], Some(-1)).unwrap();
            let linked = fs::symlink_metadata(&moved_link).unwrap();
            assert_eq!(
                (linked.dev(), linked.ino()),
                (created[0].device, created[0].inode)
            );
            assert_eq!((linked.mtime(), linked.mtime_nsec()), (-1, 999_999_999));
            let after = fs::metadata(&replacement_parent).unwrap();
            assert_eq!(
                (after.mtime(), after.mtime_nsec()),
                (before.mtime(), before.mtime_nsec())
            );
            let target_after = fs::metadata(&target).unwrap();
            assert_eq!(
                (target_after.mtime(), target_after.mtime_nsec()),
                (target_before.mtime(), target_before.mtime_nsec())
            );
            assert_eq!(fs::read(&target).unwrap(), b"keep target");
            rollback_paths(&created);
            assert!(fs::symlink_metadata(&moved_link).is_ok());
            assert!(fs::symlink_metadata(&parent).is_ok());
        }
    }
}

#[test]
fn hard_link_refuses_invalid_source_authority() {
    for fault in [
        "unrecorded",
        "missing",
        "foreign",
        "kind",
        "type",
        "unlinked",
    ] {
        let root = tempdir().unwrap();
        let source = root.path().join("source");
        let other = root.path().join("other");
        let destination = root.path().join("destination");
        fs::write(&source, b"source").unwrap();
        fs::write(&other, b"keep").unwrap();
        let before = fs::metadata(&other).unwrap();
        let mut created = Vec::new();
        record_created(&source, CreatedKind::File, &mut created).unwrap();
        match fault {
            "unrecorded" => created.clear(),
            "missing" => created[0]._identity_guard = None,
            "foreign" => created[0]._identity_guard = Some(File::open(&other).unwrap()),
            "kind" => created[0].kind = CreatedKind::Symlink,
            "type" => {
                let directory = File::open(root.path()).unwrap();
                let metadata = directory.metadata().unwrap();
                created[0].device = metadata.dev();
                created[0].inode = metadata.ino();
                created[0]._identity_guard = Some(directory);
            }
            "unlinked" => {
                fs::remove_file(&source).unwrap();
                fs::write(&source, b"replacement").unwrap();
            }
            _ => unreachable!(),
        }
        let count = created.len();
        assert!(
            create_hard_link(&source, &destination, &mut created).is_err(),
            "{fault}"
        );
        assert!(!destination.exists(), "{fault}");
        assert_eq!(
            created.len(),
            count,
            "failure must not grant cleanup authority"
        );
        let after = fs::metadata(&other).unwrap();
        assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
        assert_eq!(
            (after.mtime(), after.mtime_nsec()),
            (before.mtime(), before.mtime_nsec())
        );
        assert_eq!(fs::read(&other).unwrap(), b"keep");
    }
}

#[test]
fn hard_link_refuses_existing_destinations_and_symlink_ancestors() {
    for fault in ["file", "symlink", "ancestor"] {
        let root = tempdir().unwrap();
        let source = root.path().join("source");
        let outside = root.path().join("outside");
        fs::write(&source, b"source").unwrap();
        fs::create_dir(&outside).unwrap();
        let victim = outside.join("victim");
        fs::write(&victim, b"keep").unwrap();
        let mut created = Vec::new();
        record_created(&source, CreatedKind::File, &mut created).unwrap();
        let destination = match fault {
            "file" => victim.clone(),
            "symlink" => {
                let link = root.path().join("link");
                std::os::unix::fs::symlink(&victim, &link).unwrap();
                link
            }
            "ancestor" => {
                let link = root.path().join("link");
                std::os::unix::fs::symlink(&outside, &link).unwrap();
                link.join("new")
            }
            _ => unreachable!(),
        };
        let before = fs::metadata(&victim).unwrap();
        assert!(
            create_hard_link(&source, &destination, &mut created).is_err(),
            "{fault}"
        );
        assert_eq!(created.len(), 1);
        assert!(!outside.join("new").exists());
        let after = fs::metadata(&victim).unwrap();
        assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
        assert_eq!(after.nlink(), before.nlink());
        assert_eq!(fs::read(&victim).unwrap(), b"keep");
    }
}

#[test]
fn hard_link_uses_recorded_source_after_name_replacement() {
    for symbolic in [false, true] {
        let root = tempdir().unwrap();
        let source = root.path().join("source");
        let moved = root.path().join("moved");
        let destination = root.path().join("hardlink");
        let victim = root.path().join("victim");
        fs::write(&source, b"approved payload").unwrap();
        let mut created = Vec::new();
        record_created(&source, CreatedKind::File, &mut created).unwrap();
        let identity = fs::metadata(&source).unwrap();
        fs::rename(&source, &moved).unwrap();
        let replacement = if symbolic { &victim } else { &source };
        fs::write(replacement, b"keep").unwrap();
        apply_mtime(replacement, Some(1_700_000_000_123_456_789), false).unwrap();
        let before = fs::metadata(replacement).unwrap();
        if symbolic {
            std::os::unix::fs::symlink(&victim, &source).unwrap();
        }
        create_hard_link(&source, &destination, &mut created).unwrap();
        apply_link_mtime(created.last().unwrap(), Some(-1)).unwrap();
        let linked = fs::symlink_metadata(&destination).unwrap();
        assert!(linked.is_file());
        assert_eq!(
            (linked.dev(), linked.ino()),
            (identity.dev(), identity.ino())
        );
        assert_eq!(fs::read(&destination).unwrap(), b"approved payload");
        assert_eq!((linked.mtime(), linked.mtime_nsec()), (-1, 999_999_999));
        let after = fs::metadata(replacement).unwrap();
        assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
        assert_eq!(
            (after.mtime(), after.mtime_nsec()),
            (before.mtime(), before.mtime_nsec())
        );
        assert_eq!(fs::read(replacement).unwrap(), b"keep");
        rollback_paths(&created);
        assert!(!destination.exists());
        assert!(fs::symlink_metadata(&source).is_ok());
        assert!(moved.exists());
    }
}

#[test]
fn staging_cleanup_preserves_replacements_before_publication() {
    for replacement_kind in ["file", "link", "directory", "none"] {
        let root = tempdir().unwrap();
        let staged = stage_file(&root.path().join("destination")).unwrap();
        // The procfs path intentionally expires with the retained
        // parent descriptor. Inspect the real entry after drop.
        let source = root.path().join(staged.path.file_name().unwrap());
        if replacement_kind == "none" {
            drop(staged);
            assert!(!source.exists());
            continue;
        }
        let moved = root.path().join("moved");
        fs::rename(&source, &moved).unwrap();
        match replacement_kind {
            "file" => fs::write(&source, b"keep").unwrap(),
            "link" => std::os::unix::fs::symlink("missing", &source).unwrap(),
            "directory" => fs::create_dir(&source).unwrap(),
            _ => unreachable!(),
        }
        let before = fs::symlink_metadata(&source).unwrap();
        drop(staged);
        let after = fs::symlink_metadata(&source).unwrap();
        assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
        assert!(moved.exists());
    }
}

#[test]
fn publication_uses_staging_inode_not_replacement_name() {
    for symbolic in [false, true] {
        for remove_original in [false, true] {
            let root = tempdir().unwrap();
            let mut staged = NamedTempFile::new_in(root.path()).unwrap();
            staged.write_all(b"approved payload").unwrap();
            prepare_staged_file(staged.as_file(), 0o400, Some(-1)).unwrap();
            let identity = staged.as_file().metadata().unwrap();
            let source = staged.path().to_owned();
            if remove_original {
                fs::remove_file(&source).unwrap();
            } else {
                fs::rename(&source, root.path().join("moved")).unwrap();
            }
            let victim = root.path().join("victim");
            if symbolic {
                fs::write(&victim, b"keep victim").unwrap();
                std::os::unix::fs::symlink(&victim, &source).unwrap();
            } else {
                fs::write(&source, b"replacement payload").unwrap();
            }
            let replacement = fs::symlink_metadata(&source).unwrap();
            let destination = root.path().join("published");
            let result = PublicationTarget::open(&destination)
                .unwrap()
                .publish(StagedFile::from(staged));
            if remove_original {
                assert!(
                    result.is_err(),
                    "an unlinked named inode cannot be republished"
                );
                assert!(!destination.exists());
            } else {
                let published = result.unwrap();
                let metadata = fs::symlink_metadata(&destination).unwrap();
                assert!(metadata.is_file());
                assert_eq!(
                    (metadata.dev(), metadata.ino()),
                    (identity.dev(), identity.ino())
                );
                assert_eq!(published.metadata().unwrap().ino(), metadata.ino());
                assert_eq!(metadata.permissions().mode() & 0o777, 0o400);
                assert_eq!((metadata.mtime(), metadata.mtime_nsec()), (-1, 999_999_999));
                assert_eq!(fs::read(&destination).unwrap(), b"approved payload");
            }
            let after = fs::symlink_metadata(&source).unwrap();
            assert_eq!(
                (after.dev(), after.ino()),
                (replacement.dev(), replacement.ino())
            );
            if symbolic {
                assert_eq!(fs::read_link(&source).unwrap(), victim);
                assert_eq!(fs::read(&victim).unwrap(), b"keep victim");
            } else {
                assert_eq!(fs::read(&source).unwrap(), b"replacement payload");
            }
        }
    }
}

#[test]
fn publication_retains_parent_after_ancestor_replacement() {
    for symbolic in [false, true] {
        for replace_ancestor in [false, true] {
            let root = tempdir().unwrap();
            let parent = nested_parent(root.path());
            let target = PublicationTarget::open(&parent.join("published")).unwrap();
            let mut staged = NamedTempFile::new_in(root.path()).unwrap();
            staged.write_all(b"payload").unwrap();
            prepare_staged_file(staged.as_file(), 0o400, Some(-1)).unwrap();
            let identity = staged.as_file().metadata().unwrap();
            let ReplacedParent {
                moved_parent,
                replacement_parent,
            } = replace_parent(root.path(), replace_ancestor, symbolic);
            let before = fs::metadata(&replacement_parent).unwrap();
            let published = target.publish(StagedFile::from(staged)).unwrap();
            let metadata = published.metadata().unwrap();
            assert_eq!(
                (metadata.dev(), metadata.ino()),
                (identity.dev(), identity.ino())
            );
            assert_eq!(metadata.permissions().mode() & 0o777, 0o400);
            assert_eq!((metadata.mtime(), metadata.mtime_nsec()), (-1, 999_999_999));
            assert_eq!(
                fs::read(moved_parent.join("published")).unwrap(),
                b"payload"
            );
            assert!(!replacement_parent.join("published").exists());
            let after = fs::metadata(&replacement_parent).unwrap();
            assert_eq!(
                (after.mtime(), after.mtime_nsec()),
                (before.mtime(), before.mtime_nsec())
            );
        }
    }
}

#[test]
fn publication_refuses_symlink_parents_and_invalid_destinations() {
    let root = tempdir().unwrap();
    let directory = root.path().join("directory");
    fs::create_dir_all(directory.join("nested")).unwrap();
    let link = root.path().join("link");
    std::os::unix::fs::symlink(&directory, &link).unwrap();
    for path in [link.join("file"), link.join("nested/file")] {
        assert!(PublicationTarget::open(&path).is_err());
    }
    for path in [
        PathBuf::from("relative/file"),
        PathBuf::from("/"),
        directory.join("../escape"),
        directory.join("bad\0name"),
        directory.join("x".repeat(256)),
    ] {
        assert!(PublicationTarget::open(&path).is_err(), "{path:?}");
    }
}

#[test]
fn publication_never_overwrites_existing_symlinks() {
    for dangling in [false, true] {
        let root = tempdir().unwrap();
        let victim = root.path().join("victim");
        if !dangling {
            fs::write(&victim, b"keep").unwrap();
        }
        let destination = root.path().join("destination");
        std::os::unix::fs::symlink(&victim, &destination).unwrap();
        let before = fs::symlink_metadata(&destination).unwrap();
        let staged = NamedTempFile::new_in(root.path()).unwrap();
        let staging_path = staged.path().to_owned();
        let error = PublicationTarget::open(&destination)
            .unwrap()
            .publish(StagedFile::from(staged))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert!(!staging_path.exists());
        let after = fs::symlink_metadata(&destination).unwrap();
        assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
        assert_eq!(fs::read_link(&destination).unwrap(), victim);
        if dangling {
            assert!(!victim.exists());
        } else {
            assert_eq!(fs::read(&victim).unwrap(), b"keep");
        }
    }
}

#[test]
fn staged_file_metadata_is_complete_before_publication() {
    for mtime in [None, Some(-1), Some(0), Some(1_700_000_000_123_456_789)] {
        for mode in [0o000, 0o400, 0o640] {
            let root = tempdir().unwrap();
            let destination = root.path().join("published");
            let mut staged = NamedTempFile::new_in(root.path()).unwrap();
            staged.write_all(b"payload").unwrap();
            let before = staged.as_file().metadata().unwrap();
            prepare_staged_file(staged.as_file(), mode, mtime).unwrap();
            assert!(!destination.exists());
            let prepared = staged.as_file().metadata().unwrap();
            let expected = mtime.map_or((before.mtime(), before.mtime_nsec()), |value| {
                (
                    value.div_euclid(1_000_000_000),
                    value.rem_euclid(1_000_000_000),
                )
            });
            assert_eq!((prepared.mtime(), prepared.mtime_nsec()), expected);
            assert_eq!(prepared.permissions().mode() & 0o777, mode);
            let published = PublicationTarget::open(&destination)
                .unwrap()
                .publish(StagedFile::from(staged))
                .unwrap();
            let after = published.metadata().unwrap();
            assert_eq!((after.dev(), after.ino()), (prepared.dev(), prepared.ino()));
            assert_eq!((after.mtime(), after.mtime_nsec()), expected);
            assert_eq!(after.permissions().mode() & 0o777, mode);
        }
    }
}

#[test]
fn staged_file_metadata_never_changes_a_replacement_path() {
    for symlink_replacement in [false, true] {
        let root = tempdir().unwrap();
        let path = root.path().join("staged");
        let moved = root.path().join("moved");
        let target = root.path().join("target");
        let mut file = File::create(&path).unwrap();
        file.write_all(b"ours").unwrap();
        fs::rename(&path, &moved).unwrap();
        let replacement = if symlink_replacement { &target } else { &path };
        fs::write(replacement, b"keep").unwrap();
        apply_mtime(replacement, Some(1_700_000_000_123_456_789), false).unwrap();
        let before = fs::metadata(replacement).unwrap();
        if symlink_replacement {
            std::os::unix::fs::symlink(&target, &path).unwrap();
        }
        prepare_staged_file(&file, 0o400, Some(-1)).unwrap();
        let owned = file.metadata().unwrap();
        let after = fs::metadata(replacement).unwrap();
        assert_eq!(owned.permissions().mode() & 0o777, 0o400);
        assert_eq!((owned.mtime(), owned.mtime_nsec()), (-1, 999_999_999));
        assert_eq!(after.permissions().mode(), before.permissions().mode());
        assert_eq!(
            (after.mtime(), after.mtime_nsec()),
            (before.mtime(), before.mtime_nsec())
        );
        assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
        assert_eq!(fs::read(replacement).unwrap(), b"keep");
        assert_eq!(fs::read(&moved).unwrap(), b"ours");
    }
}

#[test]
fn prepared_file_does_not_change_an_existing_destination() {
    let root = tempdir().unwrap();
    let destination = root.path().join("existing");
    fs::write(&destination, b"keep").unwrap();
    apply_mtime(&destination, Some(1_700_000_000_123_456_789), false).unwrap();
    let before = fs::metadata(&destination).unwrap();
    let mut staged = NamedTempFile::new_in(root.path()).unwrap();
    staged.write_all(b"new").unwrap();
    let staged_path = staged.path().to_path_buf();
    prepare_staged_file(staged.as_file(), 0o000, Some(-1)).unwrap();
    let error = PublicationTarget::open(&destination)
        .unwrap()
        .publish(StagedFile::from(staged))
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    drop(error);
    assert!(!staged_path.exists());
    let after = fs::metadata(&destination).unwrap();
    assert_eq!(after.permissions().mode(), before.permissions().mode());
    assert_eq!(
        (after.mtime(), after.mtime_nsec()),
        (before.mtime(), before.mtime_nsec())
    );
    assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
    assert_eq!(fs::read(&destination).unwrap(), b"keep");
}

#[test]
fn link_timestamps_use_the_retained_inode_after_path_replacement() {
    for symbolic in [false, true] {
        for replacement_symbolic in [false, true] {
            let root = tempdir().unwrap();
            let source = root.path().join("source");
            let path = root.path().join("link");
            let moved = root.path().join("moved");
            let victim = root.path().join("victim");
            fs::write(&source, b"source").unwrap();
            apply_mtime(&source, Some(1_700_000_000_123_456_789), false).unwrap();
            let source_before = fs::metadata(&source).unwrap();
            let kind = if symbolic {
                std::os::unix::fs::symlink(&source, &path).unwrap();
                CreatedKind::Symlink
            } else {
                fs::hard_link(&source, &path).unwrap();
                CreatedKind::File
            };
            let mut created = Vec::new();
            record_created(&path, kind, &mut created).unwrap();
            fs::rename(&path, &moved).unwrap();
            fs::write(&victim, b"keep").unwrap();
            if replacement_symbolic {
                std::os::unix::fs::symlink(&victim, &path).unwrap();
            } else {
                fs::write(&path, b"replacement").unwrap();
            }
            let replacement_before = fs::symlink_metadata(&path).unwrap();
            let victim_before = fs::metadata(&victim).unwrap();
            apply_link_mtime(&created[0], Some(-1)).unwrap();
            let owned = fs::symlink_metadata(&moved).unwrap();
            assert_eq!((owned.mtime(), owned.mtime_nsec()), (-1, 999_999_999));
            for (path, before) in [(&path, replacement_before), (&victim, victim_before)] {
                let after = fs::symlink_metadata(path).unwrap();
                assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
                assert_eq!(
                    (after.mtime(), after.mtime_nsec()),
                    (before.mtime(), before.mtime_nsec())
                );
            }
            let source_after = fs::metadata(&source).unwrap();
            let expected = if symbolic {
                (source_before.mtime(), source_before.mtime_nsec())
            } else {
                (-1, 999_999_999)
            };
            assert_eq!((source_after.mtime(), source_after.mtime_nsec()), expected);
            assert_eq!(fs::read(&victim).unwrap(), b"keep");
            assert_eq!(fs::read(&source).unwrap(), b"source");
        }
    }
}

#[test]
fn link_timestamp_updates_require_the_recorded_identity() {
    for invalid in ["missing", "foreign-link", "wrong-kind", "wrong-inode"] {
        let root = tempdir().unwrap();
        let source = root.path().join("source");
        let first = root.path().join("first");
        let second = root.path().join("second");
        fs::write(&source, b"keep").unwrap();
        std::os::unix::fs::symlink(&source, &first).unwrap();
        std::os::unix::fs::symlink(&source, &second).unwrap();
        let mut created = Vec::new();
        record_created(&first, CreatedKind::Symlink, &mut created).unwrap();
        record_created(&second, CreatedKind::Symlink, &mut created).unwrap();
        let snapshots: Vec<_> = [&source, &first, &second]
            .into_iter()
            .map(|path| (path, fs::symlink_metadata(path).unwrap()))
            .collect();
        match invalid {
            "missing" => created[0]._identity_guard = None,
            "foreign-link" => {
                created[0]._identity_guard = Some(
                    created[1]
                        ._identity_guard
                        .as_ref()
                        .unwrap()
                        .try_clone()
                        .unwrap(),
                )
            }
            "wrong-kind" => created[0].kind = CreatedKind::File,
            "wrong-inode" => created[0].inode ^= 1,
            _ => unreachable!(),
        }
        // Missing timestamps perform no syscall, even without a guard.
        apply_link_mtime(&created[0], None).unwrap();
        assert_eq!(
            apply_link_mtime(&created[0], Some(-1)).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied,
            "{invalid}"
        );
        for (path, before) in snapshots {
            let after = fs::symlink_metadata(path).unwrap();
            assert_eq!(
                (after.mtime(), after.mtime_nsec()),
                (before.mtime(), before.mtime_nsec()),
                "{invalid}: {path:?}"
            );
        }
    }
}

#[test]
fn dangling_link_timestamps_survive_unlink_without_path_recovery() {
    let root = tempdir().unwrap();
    let path = root.path().join("link");
    std::os::unix::fs::symlink("missing", &path).unwrap();
    let mut created = Vec::new();
    record_created(&path, CreatedKind::Symlink, &mut created).unwrap();
    fs::remove_file(&path).unwrap();
    apply_link_mtime(&created[0], Some(-1_000_000_001)).unwrap();
    let metadata = created[0]
        ._identity_guard
        .as_ref()
        .unwrap()
        .metadata()
        .unwrap();
    assert!(metadata.file_type().is_symlink());
    assert_eq!((metadata.mtime(), metadata.mtime_nsec()), (-2, 999_999_999));
    assert!(fs::symlink_metadata(&path).is_err());
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn regular_file_is_staged_and_published_only_on_finish() {
    let root = tempdir().unwrap();
    let path = root.path().join("nested/file.txt");
    let cancel = AtomicBool::new(false);
    let mut transaction = DownloadTransaction::new("session".into());
    let mut metadata = command(
        KittyFileTransferAction::File,
        "f1",
        Some(&path),
        KittyFileTransferType::Regular,
        b"hello",
    );
    metadata.data = Box::default();
    transaction.handle(metadata, &cancel).unwrap();
    assert!(!path.exists());
    let data = command(
        KittyFileTransferAction::EndData,
        "f1",
        None,
        KittyFileTransferType::Regular,
        b"hello",
    );
    transaction.handle(data, &cancel).unwrap();
    assert!(!path.exists());
    let finish = command(
        KittyFileTransferAction::Finish,
        "",
        None,
        KittyFileTransferType::Regular,
        b"",
    );
    assert!(matches!(
        transaction.handle(finish, &cancel).unwrap(),
        TransactionResult::Finished { files: 1, bytes: 5 }
    ));
    assert_eq!(fs::read(&path).unwrap(), b"hello");
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640
    );
}

#[test]
fn zlib_stream_boundaries_are_independent_of_transfer_packets() {
    for payload in [
        Vec::new(),
        b"compressed content".to_vec(),
        vec![b'x'; 128 * 1024],
    ] {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&payload).unwrap();
        let compressed = encoder.finish().unwrap();
        assert!(compressed.len() < 4096);
        for split in 0..=compressed.len() {
            for separate_end in [false, true] {
                let root = tempdir().unwrap();
                let path = root.path().join("file");
                let cancel = AtomicBool::new(false);
                let mut transaction = compressed_transaction(&path, payload.len());
                transaction
                    .handle(
                        command(
                            KittyFileTransferAction::Data,
                            "f",
                            None,
                            KittyFileTransferType::Regular,
                            &compressed[..split],
                        ),
                        &cancel,
                    )
                    .unwrap();
                transaction
                    .handle(
                        command(
                            if separate_end {
                                KittyFileTransferAction::Data
                            } else {
                                KittyFileTransferAction::EndData
                            },
                            "f",
                            None,
                            KittyFileTransferType::Regular,
                            &compressed[split..],
                        ),
                        &cancel,
                    )
                    .unwrap();
                if separate_end {
                    // An empty non-final packet must not complete the
                    // entry or reset a decoder that has reached EOF.
                    transaction
                        .handle(
                            command(
                                KittyFileTransferAction::Data,
                                "f",
                                None,
                                KittyFileTransferType::Regular,
                                b"",
                            ),
                            &cancel,
                        )
                        .unwrap();
                    assert!(!transaction.entries["f"].complete);
                    transaction
                        .handle(
                            command(
                                KittyFileTransferAction::EndData,
                                "f",
                                None,
                                KittyFileTransferType::Regular,
                                b"",
                            ),
                            &cancel,
                        )
                        .unwrap();
                }
                assert!(!path.exists());
                transaction
                    .handle(
                        command(
                            KittyFileTransferAction::Finish,
                            "",
                            None,
                            KittyFileTransferType::Regular,
                            b"",
                        ),
                        &cancel,
                    )
                    .unwrap();
                assert_eq!(fs::read(&path).unwrap(), payload);
            }
        }
    }
}

fn compressed_transaction(path: &Path, size: usize) -> DownloadTransaction {
    let mut transaction = DownloadTransaction::new("session".into());
    let mut metadata = command(
        KittyFileTransferAction::File,
        "f",
        Some(path),
        KittyFileTransferType::Regular,
        b"",
    );
    metadata.size = Some(size as i64);
    metadata.compression = termeric_core::KittyFileTransferCompression::Zlib;
    transaction
        .handle(metadata, &AtomicBool::new(false))
        .unwrap();
    transaction
}

#[test]
fn invalid_zlib_framing_never_publishes_or_retains_staging() {
    use KittyFileTransferAction::{Data, EndData};

    let payload = b"compressed content";
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(payload).unwrap();
    let compressed = encoder.finish().unwrap();
    let mut corrupt = compressed.clone();
    *corrupt.last_mut().unwrap() ^= 1;
    let mut trailing = compressed.clone();
    trailing.extend_from_slice(b"junk");
    let mut concatenated = compressed.clone();
    concatenated.extend_from_slice(&compressed);
    for (name, size, packets) in [
        (
            "truncated",
            payload.len(),
            vec![(EndData, compressed[..compressed.len() - 1].to_vec())],
        ),
        ("checksum", payload.len(), vec![(EndData, corrupt)]),
        (
            "same-packet suffix",
            payload.len(),
            vec![(EndData, trailing)],
        ),
        (
            "same-packet second stream",
            payload.len(),
            vec![(EndData, concatenated)],
        ),
        (
            "later suffix",
            payload.len(),
            vec![(Data, compressed.clone()), (EndData, b"junk".to_vec())],
        ),
        (
            "later second stream",
            payload.len(),
            vec![(Data, compressed.clone()), (Data, compressed.clone())],
        ),
        (
            "missing end_data",
            payload.len(),
            vec![(Data, compressed.clone())],
        ),
        (
            "undersized declaration",
            payload.len() - 1,
            vec![(EndData, compressed.clone())],
        ),
        (
            "oversized declaration",
            payload.len() + 1,
            vec![(Data, compressed.clone()), (EndData, Vec::new())],
        ),
    ] {
        let root = tempdir().unwrap();
        let path = root.path().join("file");
        let cancel = AtomicBool::new(false);
        let mut transaction = compressed_transaction(&path, size);
        let mut failure = None;
        for (action, data) in packets {
            if let Err(error) = transaction.handle(
                command(action, "f", None, KittyFileTransferType::Regular, &data),
                &cancel,
            ) {
                failure = Some(error);
                break;
            }
        }
        if failure.is_none() {
            failure = transaction
                .handle(
                    command(
                        KittyFileTransferAction::Finish,
                        "",
                        None,
                        KittyFileTransferType::Regular,
                        b"",
                    ),
                    &cancel,
                )
                .err();
        }
        assert_eq!(
            failure.expect(name).code,
            KittyFileTransferStatusCode::Invalid,
            "{name}"
        );
        assert!(!path.exists(), "published {name}");
        drop(transaction);
        assert_eq!(
            fs::read_dir(root.path()).unwrap().count(),
            0,
            "retained staging for {name}"
        );
    }
}

#[test]
fn zlib_directory_and_internal_links_commit_together() {
    let root = tempdir().unwrap();
    let directory = root.path().join("tree");
    let file = directory.join("file.txt");
    let symlink = directory.join("alias.txt");
    let hardlink = directory.join("hard.txt");
    let cancel = AtomicBool::new(false);
    let mut transaction = DownloadTransaction::new("session".into());

    let mut dir = command(
        KittyFileTransferAction::File,
        "d",
        Some(&directory),
        KittyFileTransferType::Directory,
        b"",
    );
    dir.size = Some(0);
    transaction.handle(dir, &cancel).unwrap();

    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(b"compressed").unwrap();
    let compressed = encoder.finish().unwrap();
    let mut regular = command(
        KittyFileTransferAction::File,
        "f",
        Some(&file),
        KittyFileTransferType::Regular,
        b"compressed",
    );
    regular.data = Box::default();
    regular.parent = Some("d".into());
    regular.compression = termeric_core::KittyFileTransferCompression::Zlib;
    transaction.handle(regular, &cancel).unwrap();
    let body = command(
        KittyFileTransferAction::EndData,
        "f",
        None,
        KittyFileTransferType::Regular,
        &compressed,
    );
    transaction.handle(body, &cancel).unwrap();

    for (id, path, kind, target) in [
        (
            "s",
            &symlink,
            KittyFileTransferType::Symlink,
            b"fid:f".as_slice(),
        ),
        (
            "h",
            &hardlink,
            KittyFileTransferType::HardLink,
            b"f".as_slice(),
        ),
    ] {
        let mut metadata = command(KittyFileTransferAction::File, id, Some(path), kind, b"");
        metadata.size = Some(0);
        metadata.parent = Some("d".into());
        transaction.handle(metadata, &cancel).unwrap();
        let data = command(KittyFileTransferAction::EndData, id, None, kind, target);
        transaction.handle(data, &cancel).unwrap();
    }
    let finish = command(
        KittyFileTransferAction::Finish,
        "",
        None,
        KittyFileTransferType::Regular,
        b"",
    );
    transaction.handle(finish, &cancel).unwrap();
    assert_eq!(fs::read(&file).unwrap(), b"compressed");
    assert_eq!(fs::read_link(&symlink).unwrap(), PathBuf::from("file.txt"));
    assert_eq!(fs::read(&hardlink).unwrap(), b"compressed");
    assert_eq!(
        fs::metadata(&file).unwrap().ino(),
        fs::metadata(&hardlink).unwrap().ino()
    );
}

#[test]
fn traversal_existing_targets_and_size_mismatch_fail_closed() {
    let root = tempdir().unwrap();
    let existing = root.path().join("existing");
    fs::write(&existing, b"keep").unwrap();
    let cancel = AtomicBool::new(false);
    let mut transaction = DownloadTransaction::new("session".into());
    assert!(
        transaction
            .handle(
                command(
                    KittyFileTransferAction::File,
                    "e",
                    Some(&existing),
                    KittyFileTransferType::Regular,
                    b"x",
                ),
                &cancel,
            )
            .is_err()
    );
    assert_eq!(fs::read(&existing).unwrap(), b"keep");
    assert!(super::resolve_home_relative_path("../escape").is_err());
    assert!(super::resolve_home_relative_path("nested//empty").is_err());
    assert!(super::resolve_home_relative_path("nested/./dot").is_err());

    let path = root.path().join("short");
    let mut metadata = command(
        KittyFileTransferAction::File,
        "short",
        Some(&path),
        KittyFileTransferType::Regular,
        b"1234",
    );
    metadata.data = Box::default();
    transaction.handle(metadata, &cancel).unwrap();
    assert!(
        transaction
            .handle(
                command(
                    KittyFileTransferAction::EndData,
                    "short",
                    None,
                    KittyFileTransferType::Regular,
                    b"12",
                ),
                &cancel,
            )
            .is_err()
    );
    assert!(!path.exists());
}

#[test]
fn cancellation_decompression_limits_and_symlink_races_leave_no_payload() {
    let root = tempdir().unwrap();
    let cancel = AtomicBool::new(false);

    let canceled_path = root.path().join("canceled.bin");
    let mut canceled = DownloadTransaction::new("cancel".into());
    let mut metadata = command(
        KittyFileTransferAction::File,
        "c",
        Some(&canceled_path),
        KittyFileTransferType::Regular,
        b"payload",
    );
    metadata.data = Box::default();
    canceled.handle(metadata, &cancel).unwrap();
    canceled
        .handle(
            command(
                KittyFileTransferAction::EndData,
                "c",
                None,
                KittyFileTransferType::Regular,
                b"payload",
            ),
            &cancel,
        )
        .unwrap();
    cancel.store(true, Ordering::Release);
    assert!(
        canceled
            .handle(
                command(
                    KittyFileTransferAction::Finish,
                    "",
                    None,
                    KittyFileTransferType::Regular,
                    b"",
                ),
                &cancel,
            )
            .is_err()
    );
    drop(canceled);
    assert!(!canceled_path.exists());
    assert!(fs::read_dir(root.path()).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".termeric-transfer-")
    }));

    let bomb_path = root.path().join("bomb.bin");
    let mut bomb = DownloadTransaction::new("bomb".into());
    let mut bomb_metadata = command(
        KittyFileTransferAction::File,
        "b",
        Some(&bomb_path),
        KittyFileTransferType::Regular,
        b"tiny",
    );
    bomb_metadata.data = Box::default();
    bomb_metadata.size = Some(4);
    bomb_metadata.compression = termeric_core::KittyFileTransferCompression::Zlib;
    bomb.handle(bomb_metadata, &AtomicBool::new(false)).unwrap();
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&vec![b'x'; 8 * 1024]).unwrap();
    let compressed = encoder.finish().unwrap();
    let bomb_data = command(
        KittyFileTransferAction::EndData,
        "b",
        None,
        KittyFileTransferType::Regular,
        &compressed,
    );
    assert!(bomb.handle(bomb_data, &AtomicBool::new(false)).is_err());
    assert!(!bomb_path.exists());

    let outside = tempdir().unwrap();
    let raced_parent = root.path().join("raced");
    let raced_path = raced_parent.join("file");
    let mut raced = DownloadTransaction::new("race".into());
    let mut raced_metadata = command(
        KittyFileTransferAction::File,
        "r",
        Some(&raced_path),
        KittyFileTransferType::Regular,
        b"data",
    );
    raced_metadata.data = Box::default();
    raced
        .handle(raced_metadata, &AtomicBool::new(false))
        .unwrap();
    raced
        .handle(
            command(
                KittyFileTransferAction::EndData,
                "r",
                None,
                KittyFileTransferType::Regular,
                b"data",
            ),
            &AtomicBool::new(false),
        )
        .unwrap();
    std::os::unix::fs::symlink(outside.path(), &raced_parent).unwrap();
    assert!(
        raced
            .handle(
                command(
                    KittyFileTransferAction::Finish,
                    "",
                    None,
                    KittyFileTransferType::Regular,
                    b"",
                ),
                &AtomicBool::new(false),
            )
            .is_err()
    );
    assert!(!outside.path().join("file").exists());
}

#[test]
fn duplicate_ids_paths_oversized_declarations_and_external_symlinks_are_bounded() {
    let root = tempdir().unwrap();
    let first = root.path().join("first");
    let second = root.path().join("second");
    let cancel = AtomicBool::new(false);
    let mut transaction = DownloadTransaction::new("bounds".into());

    let mut oversized = command(
        KittyFileTransferAction::File,
        "large",
        Some(&first),
        KittyFileTransferType::Regular,
        b"",
    );
    oversized.size = Some((MAX_KITTY_DOWNLOAD_BYTES + 1) as i64);
    assert!(transaction.handle(oversized, &cancel).is_err());
    let mut transaction = DownloadTransaction::new("duplicates".into());

    let mut file = command(
        KittyFileTransferAction::File,
        "f",
        Some(&first),
        KittyFileTransferType::Regular,
        b"one",
    );
    file.data = Box::default();
    transaction.handle(file.clone(), &cancel).unwrap();
    assert!(transaction.handle(file, &cancel).is_err());

    let mut duplicate_path = command(
        KittyFileTransferAction::File,
        "other",
        Some(&first),
        KittyFileTransferType::Regular,
        b"one",
    );
    duplicate_path.data = Box::default();
    assert!(transaction.handle(duplicate_path, &cancel).is_err());

    transaction
        .handle(
            command(
                KittyFileTransferAction::EndData,
                "f",
                None,
                KittyFileTransferType::Regular,
                b"one",
            ),
            &cancel,
        )
        .unwrap();
    let mut link = command(
        KittyFileTransferAction::File,
        "s",
        Some(&second),
        KittyFileTransferType::Symlink,
        b"",
    );
    link.size = Some(0);
    transaction.handle(link, &cancel).unwrap();
    transaction
        .handle(
            command(
                KittyFileTransferAction::EndData,
                "s",
                None,
                KittyFileTransferType::Symlink,
                b"path:../outside-target",
            ),
            &cancel,
        )
        .unwrap();
    transaction
        .handle(
            command(
                KittyFileTransferAction::Finish,
                "",
                None,
                KittyFileTransferType::Regular,
                b"",
            ),
            &cancel,
        )
        .unwrap();
    assert_eq!(
        fs::read_link(second).unwrap(),
        PathBuf::from("../outside-target")
    );
}

#[test]
fn fifo_symlink_commit_does_not_wait_for_a_target_writer() {
    use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};

    let root = tempdir().unwrap();
    let fifo = root.path().join("pipe");
    let link = root.path().join("link");
    let fifo_name = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    // SAFETY: fifo_name is a live NUL-terminated path in a private tempdir.
    assert_eq!(unsafe { libc::mkfifo(fifo_name.as_ptr(), 0o600) }, 0);
    let before = fs::symlink_metadata(&fifo).unwrap();
    let worker_link = link.clone();
    let (sender, receiver) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let cancel = AtomicBool::new(false);
        let mut transaction = DownloadTransaction::new("session".into());
        transaction
            .handle(
                command(
                    KittyFileTransferAction::File,
                    "s",
                    Some(&worker_link),
                    KittyFileTransferType::Symlink,
                    b"",
                ),
                &cancel,
            )
            .unwrap();
        transaction
            .handle(
                command(
                    KittyFileTransferAction::EndData,
                    "s",
                    None,
                    KittyFileTransferType::Symlink,
                    b"path:pipe",
                ),
                &cancel,
            )
            .unwrap();
        let result = transaction.handle(
            command(
                KittyFileTransferAction::Finish,
                "",
                None,
                KittyFileTransferType::Regular,
                b"",
            ),
            &cancel,
        );
        sender.send(result).unwrap();
    });
    let observed = receiver.recv_timeout(Duration::from_secs(2));
    let completed_without_writer = observed.is_ok();
    // Rescue the old blocking implementation before asserting, so a
    // failing regression never strands its worker or FIFO endpoint.
    let result = match observed {
        Ok(result) => result,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            let _rescue = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&fifo)
                .unwrap();
            receiver.recv_timeout(Duration::from_secs(2)).unwrap()
        }
        Err(error) => panic!("transfer worker disconnected: {error}"),
    };
    worker.join().unwrap();
    result.unwrap();
    assert!(
        completed_without_writer,
        "symlink bookkeeping waited for a FIFO writer"
    );
    assert_eq!(fs::read_link(&link).unwrap(), PathBuf::from("pipe"));
    let after = fs::symlink_metadata(&fifo).unwrap();
    assert!(after.file_type().is_fifo());
    assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
    assert_eq!(
        (before.mtime(), before.mtime_nsec()),
        (after.mtime(), after.mtime_nsec())
    );
    let no_reader = fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&fifo)
        .unwrap_err();
    assert_eq!(no_reader.raw_os_error(), Some(libc::ENXIO));
}

#[test]
fn rollback_identity_guard_pins_the_symlink_not_its_target() {
    let root = tempdir().unwrap();
    let target = root.path().join("target");
    let link = root.path().join("link");
    fs::write(&target, b"keep").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let mut created = Vec::new();
    record_created(&link, CreatedKind::Symlink, &mut created).unwrap();
    let guard = created[0]
        ._identity_guard
        .as_ref()
        .expect("pin the symlink inode");
    let pinned = guard.metadata().unwrap();
    assert!(pinned.file_type().is_symlink());
    assert_eq!(
        (pinned.dev(), pinned.ino()),
        (created[0].device, created[0].inode)
    );
    // Unlink/recreate cannot reuse the still-pinned original inode.
    fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink("missing", &link).unwrap();
    rollback_paths(&created);
    assert_eq!(fs::read_link(&link).unwrap(), PathBuf::from("missing"));
    assert_eq!(fs::read(&target).unwrap(), b"keep");
    let mut replacement = Vec::new();
    record_created(&link, CreatedKind::Symlink, &mut replacement).unwrap();
    assert!(
        replacement[0]._identity_guard.is_some(),
        "pin dangling links too"
    );
    rollback_paths(&replacement);
    assert!(fs::symlink_metadata(&link).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"keep");
}

#[test]
fn failed_created_path_inspection_never_deletes_a_replacement() {
    for kind in [
        CreatedKind::File,
        CreatedKind::Directory,
        CreatedKind::Symlink,
    ] {
        let root = tempdir().unwrap();
        let path = root.path().join("entry");
        let moved = root.path().join("original");
        let target = root.path().join("target");
        fs::write(&target, b"keep target").unwrap();
        let create = |path: &Path| match kind {
            CreatedKind::File => fs::write(path, b"keep file"),
            CreatedKind::Directory => fs::create_dir(path),
            CreatedKind::Symlink => std::os::unix::fs::symlink(&target, path),
        };
        create(&path).unwrap();
        let mut created = Vec::new();
        let error = record_created_with(&path, kind, &mut created, |observed| {
            // Model a failed metadata observation while another actor
            // moves the original and replaces its pathname.
            fs::rename(observed, &moved).unwrap();
            create(observed).unwrap();
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "injected inspection failure",
            ))
        })
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(created.is_empty());
        assert!(
            fs::symlink_metadata(&path).is_ok(),
            "replacement was deleted"
        );
        assert!(fs::symlink_metadata(&moved).is_ok(), "original was deleted");
        assert_eq!(fs::read(&target).unwrap(), b"keep target");
        rollback_paths(&created);
        assert!(fs::symlink_metadata(&path).is_ok());
    }
}

#[test]
fn created_path_type_mismatches_never_gain_rollback_authority() {
    let kinds = [
        CreatedKind::File,
        CreatedKind::Directory,
        CreatedKind::Symlink,
    ];
    for actual in kinds {
        let root = tempdir().unwrap();
        let path = root.path().join("entry");
        let target = root.path().join("target");
        fs::write(&target, b"keep target").unwrap();
        match actual {
            CreatedKind::File => fs::write(&path, b"keep file").unwrap(),
            CreatedKind::Directory => fs::create_dir(&path).unwrap(),
            CreatedKind::Symlink => std::os::unix::fs::symlink(&target, &path).unwrap(),
        }
        let before = fs::symlink_metadata(&path).unwrap();
        let target_before = fs::metadata(&target).unwrap();
        for expected in kinds.into_iter().filter(|kind| *kind != actual) {
            let mut created = Vec::new();
            let error = record_created(&path, expected, &mut created).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
            assert!(created.is_empty());
            rollback_paths(&created);
            let after = fs::symlink_metadata(&path).unwrap();
            assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
            assert_eq!(after.permissions().mode(), before.permissions().mode());
            let target_after = fs::metadata(&target).unwrap();
            assert_eq!(
                target_after.permissions().mode(),
                target_before.permissions().mode()
            );
            assert_eq!(fs::read(&target).unwrap(), b"keep target");
        }
    }
}

#[test]
fn rollback_does_not_remove_a_replaced_path() {
    let root = tempdir().unwrap();
    let path = root.path().join("published");
    fs::write(&path, b"ours").unwrap();
    let mut created = Vec::new();
    record_created(&path, CreatedKind::File, &mut created).unwrap();
    fs::remove_file(&path).unwrap();
    fs::write(&path, b"replacement").unwrap();

    rollback_paths(&created);

    assert_eq!(fs::read(path).unwrap(), b"replacement");
}

#[test]
fn idle_worker_expires_with_original_reply_framing_and_no_files() {
    let (_sender, receiver) = sync_channel(1);
    let mut completions = Vec::new();
    run_worker(
        receiver,
        "idle-session".into(),
        true,
        Arc::new(AtomicBool::new(false)),
        Duration::from_millis(1),
        |completion| {
            completions.push(completion);
            true
        },
    );

    assert_eq!(completions.len(), 1);
    let completion = completions.pop().unwrap();
    assert!(completion.reply_uses_8bit_controls);
    let WorkerDisposition::Status {
        status,
        terminal,
        files,
        bytes,
    } = completion.disposition
    else {
        panic!("expected timeout status");
    };
    assert!(terminal);
    assert_eq!(files, 0);
    assert_eq!(bytes, 0);
    assert_eq!(status.id, "idle-session");
    assert_eq!(status.code, KittyFileTransferStatusCode::TimedOut);
}
