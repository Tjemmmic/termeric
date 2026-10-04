use super::*;

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
