use super::*;

fn request(
    program: String,
    args: Vec<String>,
) -> (DesktopCommandRequest, Receiver<Result<(), String>>) {
    DesktopCommandRequest::new(program, args, "fixture")
}

fn worker(
    receiver: Receiver<DesktopCommandRequest>,
    children: Vec<(&'static str, Child)>,
) -> (std::thread::JoinHandle<()>, Receiver<bool>, Receiver<()>) {
    let (observed, observations) = mpsc::channel();
    let (done, finished) = mpsc::channel();
    let handle = std::thread::spawn(move || {
        run_desktop_commands(receiver, children, |idle| {
            let _ = observed.send(idle);
        });
        let _ = done.send(());
    });
    (handle, observations, finished)
}

#[test]
fn empty_desktop_worker_blocks_and_failed_launch_returns_to_untimed_sleep() {
    let (sender, receiver) = mpsc::sync_channel(DESKTOP_COMMAND_QUEUE_CAPACITY);
    let (handle, observations, finished) = worker(receiver, Vec::new());
    let deadline = Duration::from_secs(5);
    assert!(observations.recv_timeout(deadline).unwrap());
    assert!(matches!(
        observations.recv_timeout(Duration::from_millis(160)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    let directory = tempfile::tempdir().unwrap();
    let (request, started) = request(
        directory
            .path()
            .join("absent-helper")
            .to_str()
            .unwrap()
            .into(),
        Vec::new(),
    );
    sender.send(request).unwrap();
    assert!(started.recv_timeout(deadline).unwrap().is_err());
    assert!(observations.recv_timeout(deadline).unwrap());
    assert!(matches!(
        observations.recv_timeout(Duration::from_millis(160)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    drop(sender);
    finished.recv_timeout(deadline).unwrap();
    handle.join().unwrap();
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::io;
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;

    fn gated_child() -> (Child, UnixStream) {
        let (input, release) = UnixStream::pair().unwrap();
        let child = Command::new("/bin/sh")
            .args(["-c", "read line; exit 0"])
            .stdin(std::process::Stdio::from(OwnedFd::from(input)))
            .spawn()
            .unwrap();
        (child, release)
    }

    fn assert_reaped(pid: u32) {
        let mut status = 0;
        // SAFETY: This is our fixture PID and a writable status pointer.
        // WNOHANG cannot block even if the reaping regression recurs.
        let result = unsafe { libc::waitpid(pid as libc::pid_t, &mut status, libc::WNOHANG) };
        let error = io::Error::last_os_error();
        assert_eq!(result, -1, "worker did not reap the fixture child");
        assert_eq!(error.raw_os_error(), Some(libc::ECHILD));
    }

    #[test]
    fn live_child_uses_timed_receive_then_returns_to_idle_without_polling() {
        let (child, release) = gated_child();
        let pid = child.id();
        let (sender, receiver) = mpsc::sync_channel(DESKTOP_COMMAND_QUEUE_CAPACITY);
        let (handle, observations, finished) = worker(receiver, vec![("fixture", child)]);
        let deadline = Duration::from_secs(5);
        assert!(!observations.recv_timeout(deadline).unwrap());
        assert!(!observations.recv_timeout(deadline).unwrap());
        drop(release);
        let end = Instant::now() + deadline;
        loop {
            assert!(Instant::now() < end, "worker never returned to idle");
            if observations
                .recv_timeout(end.saturating_duration_since(Instant::now()))
                .unwrap()
            {
                break;
            }
        }
        assert_reaped(pid);
        assert!(matches!(
            observations.recv_timeout(Duration::from_millis(160)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        drop(sender);
        finished.recv_timeout(deadline).unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn disconnected_worker_retains_live_children_until_reaped() {
        let (child, release) = gated_child();
        let pid = child.id();
        let (sender, receiver) = mpsc::sync_channel(DESKTOP_COMMAND_QUEUE_CAPACITY);
        drop(sender);
        let (handle, observations, finished) = worker(receiver, vec![("fixture", child)]);
        let deadline = Duration::from_secs(5);
        assert!(!observations.recv_timeout(deadline).unwrap());
        assert!(matches!(
            finished.recv_timeout(Duration::from_millis(160)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        drop(release);
        finished.recv_timeout(deadline).unwrap();
        handle.join().unwrap();
        assert_reaped(pid);
    }

    #[test]
    fn queued_requests_get_individual_reap_turns_and_abandoned_acks_do_not_block() {
        let directory = tempfile::tempdir().unwrap();
        let pids = directory.path().join("pids");
        let (sender, receiver) = mpsc::sync_channel(DESKTOP_COMMAND_QUEUE_CAPACITY);
        let mut acknowledgements = Vec::new();
        for index in 0..DESKTOP_COMMAND_QUEUE_CAPACITY {
            let (request, started) = request(
                "/bin/sh".into(),
                vec![
                    "-c".into(),
                    "printf '%s\\n' \"$$\" >> \"$1\"; exit \"$2\"".into(),
                    "fixture".into(),
                    pids.to_str().unwrap().into(),
                    (index % 2).to_string(),
                ],
            );
            sender.send(request).unwrap();
            if index % 2 == 0 {
                // Keep the receiver alive without reading it until shutdown.
                acknowledgements.push(started);
            } else {
                drop(started);
            }
        }
        drop(sender);
        let (handle, observations, finished) = worker(receiver, Vec::new());
        finished.recv_timeout(Duration::from_secs(5)).unwrap();
        handle.join().unwrap();
        // One receive/reap iteration per queued request, then disconnection.
        assert_eq!(
            observations.into_iter().count(),
            DESKTOP_COMMAND_QUEUE_CAPACITY + 1
        );
        for acknowledgement in acknowledgements {
            let result = acknowledgement.recv().unwrap();
            if let Err(error) = result {
                assert!(error.contains("at most 8 desktop helpers"));
            }
        }
        let pids = std::fs::read_to_string(pids).unwrap();
        assert!(!pids.is_empty());
        for pid in pids.lines() {
            assert_reaped(pid.parse().unwrap());
        }
    }
}
