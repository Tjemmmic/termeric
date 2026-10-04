//! Raw PTYs and synthetic worker events only: no GUI, file workers, or providers.
use super::*;
use crate::app::Requester;
use crate::app::tests::{
    current_harvest_capture_routes, fenced_harvest_capture, harvest_rerun_capture_fixture_in,
    kitty_capture_command, split_harvest_capture_owner,
};
use crate::panes::PaneId;
use std::path::Path;

const TRANSITIONS: &[&str] = &[
    "split",
    "collapse",
    "nonzero-collapse",
    "removed",
    "resplit-replacement",
    "closed",
    "closing",
    "pane-closed",
    "read-only",
];

fn change_owner_layout(
    app: &mut App,
    directory: &Path,
    owner: TerminalIdentity,
    transition: &str,
) -> Vec<(TerminalEventOrigin, PathBuf)> {
    // Fixture readiness pumps housekeeping. Hold session state aside so tests
    // can exercise a queued callback/choice *before* the next normal prune.
    let transfers = std::mem::take(&mut app.kitty_file_transfers);
    if transition == "nonzero-collapse" {
        app.close_pane_for(Requester::Control, owner.tab_id as u64, 0)
            .unwrap();
        app.close_pane_for(Requester::Control, owner.tab_id as u64, 1)
            .unwrap();
    } else {
        let sibling = split_harvest_capture_owner(app, directory);
        match transition {
            "collapse" => {
                app.close_pane_for(Requester::Control, owner.tab_id as u64, sibling.0)
                    .unwrap();
            }
            "removed" | "resplit-replacement" => {
                app.close_pane_for(Requester::Control, owner.tab_id as u64, owner.pane_id.0)
                    .unwrap();
                if transition == "resplit-replacement" {
                    split_harvest_capture_owner(app, directory);
                }
            }
            _ => {}
        }
    }
    let captures = current_harvest_capture_routes(app, directory);
    app.kitty_file_transfers = transfers;
    let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
    match transition {
        "closed" => tab.closed = true,
        "closing" => tab.closing_at = Some(Instant::now()),
        "pane-closed" => tab.panes.get_pane_mut(owner.pane_id).unwrap().closed = true,
        "read-only" => tab.panes.get_pane_mut(owner.pane_id).unwrap().read_only = true,
        _ => {}
    }
    captures
}

fn survives(transition: &str) -> bool {
    matches!(
        transition,
        "split" | "collapse" | "nonzero-collapse" | "read-only"
    )
}

#[test]
fn consent_and_pruning_follow_only_the_same_terminal_owner() {
    for upload in [false, true] {
        for prune_first in [false, true] {
            for &transition in TRANSITIONS {
                let (mut app, directory, _, _) =
                    harvest_rerun_capture_fixture_in(transition == "nonzero-collapse");
                let owner = app
                    .tabs
                    .as_ref()
                    .unwrap()
                    .active_tab()
                    .unwrap()
                    .focused_terminal_identity();
                let origin = app.kitty_transfer_origin(owner).unwrap();
                let mut command = kitty_capture_command(if upload {
                    KittyFileTransferAction::Receive
                } else {
                    KittyFileTransferAction::Send
                });
                command.size = upload.then_some(1);
                app.handle_kitty_file_transfer(origin, command);
                if upload {
                    let mut path = kitty_capture_command(KittyFileTransferAction::File);
                    path.file_id = "fixture".into();
                    path.name = Some(directory.path().join("not-opened").display().to_string());
                    app.handle_kitty_file_transfer(origin, path);
                }
                let request_id = app.info_overlay.request_id();
                assert!(app.info_overlay.is_awaiting(request_id));
                let captures = change_owner_layout(&mut app, directory.path(), owner, transition);
                if prune_first {
                    assert_eq!(
                        app.prune_stale_kitty_file_transfers(),
                        !survives(transition),
                        "{upload} {transition}"
                    );
                }
                // Without a native window, a surviving owner's Allow is refused
                // before worker creation. A removed owner's Allow emits no reply.
                let choice = match (upload, prune_first) {
                    (true, true) => "kitty-upload-deny",
                    (false, true) => "kitty-download-deny",
                    (true, false) => "kitty-upload-allow",
                    (false, false) => "kitty-download-allow",
                };
                let handled = if upload {
                    app.handle_kitty_upload_choice(request_id, choice)
                } else {
                    app.handle_kitty_download_choice(request_id, choice)
                };
                assert_eq!(handled, !prune_first || survives(transition));
                assert!(!app.handle_kitty_download_choice(request_id, choice));
                assert!(!app.handle_kitty_upload_choice(request_id, choice));
                let route = app.kitty_transfer_origin(owner);
                let bytes = fenced_harvest_capture(&mut app, &captures);
                for ((candidate, _), data) in captures.iter().zip(bytes) {
                    assert_eq!(
                        !data.is_empty(),
                        survives(transition) && Some(*candidate) == route,
                        "{upload} {transition}"
                    );
                }
                assert!(app.kitty_file_transfers.pending.is_none());
                assert!(app.kitty_file_transfers.pending_upload.is_none());
                assert!(app.kitty_file_transfers.active.is_empty());
                assert!(app.kitty_file_transfers.active_uploads.is_empty());
                assert!(!directory.path().join("not-opened").exists());
            }
        }
    }
}

fn install_download(app: &mut App, key: TransferKey) -> (Receiver<WorkerCommand>, Arc<AtomicBool>) {
    let (sender, receiver) = sync_channel(4);
    let cancellation = Arc::new(AtomicBool::new(false));
    let progress_toast_id = app.toasts.persistent_info("synthetic download");
    app.kitty_file_transfers.tokens.insert(key.clone(), 42);
    app.kitty_file_transfers.active.insert(
        42,
        ActiveDownload {
            key,
            sender,
            cancellation: Arc::clone(&cancellation),
            progress_toast_id,
        },
    );
    (receiver, cancellation)
}

fn install_upload(
    app: &mut App,
    key: TransferKey,
) -> (Receiver<UploadWorkerCommand>, Arc<AtomicBool>) {
    let (sender, receiver) = sync_channel(4);
    let cancellation = Arc::new(AtomicBool::new(false));
    let progress_toast_id = app.toasts.persistent_info("synthetic upload");
    app.kitty_file_transfers
        .upload_tokens
        .insert(key.clone(), 42);
    app.kitty_file_transfers.active_uploads.insert(
        42,
        ActiveUpload {
            key,
            sender,
            cancellation: Arc::clone(&cancellation),
            progress_toast_id,
            files_sent: 0,
            total_files: 1,
            bytes_sent: 0,
        },
    );
    (receiver, cancellation)
}

#[test]
fn asynchronous_status_and_file_data_never_follow_a_replacement_owner() {
    for upload in [false, true] {
        for eight_bit in [false, true] {
            for &transition in TRANSITIONS {
                let (mut app, directory, _, _) =
                    harvest_rerun_capture_fixture_in(transition == "nonzero-collapse");
                let owner = app
                    .tabs
                    .as_ref()
                    .unwrap()
                    .active_tab()
                    .unwrap()
                    .focused_terminal_identity();
                let key = TransferKey {
                    origin: owner,
                    id: "capture-session".into(),
                };
                // Keep synthetic command receivers alive without starting workers.
                let (download_receiver, download_cancel) = install_download(&mut app, key.clone());
                let (upload_receiver, upload_cancel) = install_upload(&mut app, key);
                let captures = change_owner_layout(&mut app, directory.path(), owner, transition);
                let status = transfer_status(
                    "capture-session",
                    "fixture",
                    KittyFileTransferStatusCode::Ok,
                    "fixture-status",
                );
                let canonical = if upload {
                    b"\x1b]5113;ac=end_data;id=capture-session;fid=fixture;d=Zml4dHVyZQ==\x1b\\"
                        .to_vec()
                } else {
                    status.canonical_reply()
                };
                let expected = Terminal::frame_pty_reply_for_mode(&canonical, eight_bit);
                if upload {
                    for replay in [false, true] {
                        let (acknowledge, received) = sync_channel(1);
                        app.handle_kitty_file_upload_completed(
                            42,
                            UploadWorkerCompletion {
                                frames: vec![canonical.clone()],
                                terminal: true,
                                files: 1,
                                total_files: 1,
                                bytes: 7,
                                error: None,
                                reply_uses_8bit_controls: eight_bit,
                                acknowledge: Some(acknowledge),
                            },
                        );
                        assert_eq!(
                            received.try_recv().unwrap(),
                            !replay && survives(transition),
                            "{transition}"
                        );
                    }
                    assert!(upload_cancel.load(Ordering::Acquire));
                    assert!(app.kitty_file_transfers.upload_tokens.is_empty());
                } else {
                    for _ in 0..2 {
                        app.handle_kitty_file_transfer_completed(
                            42,
                            WorkerCompletion {
                                reply_uses_8bit_controls: eight_bit,
                                disposition: WorkerDisposition::Status {
                                    status: status.clone(),
                                    terminal: true,
                                    files: 1,
                                    bytes: 7,
                                },
                            },
                        );
                    }
                    assert!(download_cancel.load(Ordering::Acquire));
                    assert!(app.kitty_file_transfers.tokens.is_empty());
                }
                let route = app.kitty_transfer_origin(owner);
                let bytes = fenced_harvest_capture(&mut app, &captures);
                for ((candidate, _), data) in captures.iter().zip(bytes) {
                    let wanted = if survives(transition) && Some(*candidate) == route {
                        expected.as_slice()
                    } else {
                        &[]
                    };
                    assert_eq!(
                        data, wanted,
                        "upload={upload}, S8={eight_bit}, {transition}"
                    );
                }
                // Pruning independently cancels the other direction if its owner disappeared.
                assert_eq!(
                    app.prune_stale_kitty_file_transfers(),
                    !survives(transition)
                );
                app.shutdown_kitty_file_transfers();
                assert!(download_cancel.load(Ordering::Acquire));
                assert!(upload_cancel.load(Ordering::Acquire));
                assert!(download_receiver.try_recv().is_err());
                assert!(upload_receiver.try_recv().is_err());
            }
        }
    }
}

#[test]
fn path_collection_and_active_commands_match_across_split_and_collapse() {
    let (mut app, directory, entry, _) = harvest_rerun_capture_fixture_in(false);
    let owner = TerminalIdentity {
        tab_id: entry.source.tab_id,
        pane_id: PaneId(0),
    };
    let mut receive = kitty_capture_command(KittyFileTransferAction::Receive);
    receive.size = Some(2);
    app.handle_kitty_file_transfer(app.kitty_transfer_origin(owner).unwrap(), receive);
    let sibling = split_harvest_capture_owner(&mut app, directory.path());
    current_harvest_capture_routes(&mut app, directory.path());
    let route = TerminalEventOrigin {
        tab_id: owner.tab_id,
        pane_id: owner.pane_id,
    };
    // The owner's route names its pane, so splitting keeps it valid.
    assert_eq!(
        app.tabs.as_ref().unwrap().identity_for_origin(route),
        Some(owner)
    );
    assert!(!app.prune_stale_kitty_file_transfers());
    for index in 0..2 {
        if index == 1 {
            app.close_pane_for(Requester::Control, owner.tab_id as u64, sibling.0)
                .unwrap();
        }
        let mut path = kitty_capture_command(KittyFileTransferAction::File);
        path.file_id = format!("fixture-{index}");
        path.name = Some(directory.path().join(&path.file_id).display().to_string());
        app.handle_kitty_file_transfer(app.kitty_transfer_origin(owner).unwrap(), path);
    }
    assert_eq!(
        app.kitty_file_transfers
            .pending_upload
            .as_ref()
            .unwrap()
            .paths
            .len(),
        2
    );
    assert!(app.info_overlay.is_awaiting(app.info_overlay.request_id()));
    assert_eq!(
        app.tabs.as_ref().unwrap().identity_for_origin(route),
        Some(owner)
    );
    app.deny_pending_kitty_upload(None, "fixture cleanup");

    let key = TransferKey {
        origin: owner,
        id: "capture-session".into(),
    };
    let (download, download_cancel) = install_download(&mut app, key.clone());
    let sibling = split_harvest_capture_owner(&mut app, directory.path());
    current_harvest_capture_routes(&mut app, directory.path());
    assert!(!app.prune_stale_kitty_file_transfers());
    app.handle_kitty_file_transfer(
        app.kitty_transfer_origin(owner).unwrap(),
        kitty_capture_command(KittyFileTransferAction::Data),
    );
    assert_eq!(
        download.try_recv().unwrap().command.action,
        KittyFileTransferAction::Data
    );
    // A sibling using the identical session ID cannot cancel the owner's transfer.
    app.handle_kitty_file_transfer(
        TerminalEventOrigin {
            tab_id: owner.tab_id,
            pane_id: sibling,
        },
        kitty_capture_command(KittyFileTransferAction::Cancel),
    );
    assert!(!download_cancel.load(Ordering::Acquire));
    app.close_pane_for(Requester::Control, owner.tab_id as u64, sibling.0)
        .unwrap();
    app.handle_kitty_file_transfer(
        app.kitty_transfer_origin(owner).unwrap(),
        kitty_capture_command(KittyFileTransferAction::Cancel),
    );
    assert!(download_cancel.load(Ordering::Acquire));
    assert!(app.kitty_file_transfers.tokens.is_empty());

    let (upload, upload_cancel) = install_upload(&mut app, key);
    let sibling = split_harvest_capture_owner(&mut app, directory.path());
    current_harvest_capture_routes(&mut app, directory.path());
    let mut path = kitty_capture_command(KittyFileTransferAction::File);
    path.file_id = "fixture".into();
    path.name = Some(directory.path().join("fixture").display().to_string());
    app.handle_kitty_file_transfer(app.kitty_transfer_origin(owner).unwrap(), path);
    assert_eq!(upload.try_recv().unwrap().command.file_id, "fixture");
    app.close_pane_for(Requester::Control, owner.tab_id as u64, sibling.0)
        .unwrap();
    app.handle_kitty_file_transfer(
        app.kitty_transfer_origin(owner).unwrap(),
        kitty_capture_command(KittyFileTransferAction::Cancel),
    );
    assert!(upload_cancel.load(Ordering::Acquire));
    assert!(app.kitty_file_transfers.upload_tokens.is_empty());
}
