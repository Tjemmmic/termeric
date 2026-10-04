//! Same-user Unix-socket server for GUI inventory requests.

use super::paths::{ControlPaths, remove_safe_file};
use super::protocol::{self, ClientFrame, Request, Response, ServerFrame};
use crate::app::AppEvent;
use anyhow::Context;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use winit::event_loop::EventLoopProxy;

const MAX_CLIENTS: usize = 16;
const IO_TIMEOUT: Duration = Duration::from_secs(12);
const EVENT_LOOP_TIMEOUT: Duration = Duration::from_secs(10);

pub struct ControlServer {
    paths: ControlPaths,
    shutdown: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl ControlServer {
    pub fn start(proxy: EventLoopProxy<AppEvent>) -> anyhow::Result<Self> {
        let paths = ControlPaths::discover_current()?;
        prepare_endpoint(&paths)?;
        match Self::start_prepared(paths.clone(), proxy) {
            Ok(server) => Ok(server),
            Err(error) => {
                for (path, socket) in [(&paths.socket, true), (&paths.token, false)] {
                    let _ = remove_safe_file(path, socket);
                }
                Err(error)
            }
        }
    }

    fn start_prepared(
        paths: ControlPaths,
        proxy: EventLoopProxy<AppEvent>,
    ) -> anyhow::Result<Self> {
        let token = paths.load_or_create_token()?;
        let listener = UnixListener::bind(&paths.socket)
            .with_context(|| format!("bind GUI control socket {}", paths.socket.display()))?;
        fs::set_permissions(&paths.socket, fs::Permissions::from_mode(0o600))?;
        let metadata = fs::symlink_metadata(&paths.socket)?;
        if metadata.uid() != crate::domain::paths::effective_uid()
            || metadata.permissions().mode() & 0o777 != 0o600
        {
            anyhow::bail!("GUI control socket could not be secured to mode 0600");
        }

        let shutdown = Arc::new(AtomicBool::new(false));
        let server_shutdown = Arc::clone(&shutdown);
        let clients = Arc::new(AtomicUsize::new(0));
        let thread = thread::Builder::new()
            .name("termeric-gui-control".into())
            .spawn(move || run(listener, token, proxy, server_shutdown, clients))?;
        log::info!(
            "GUI control endpoint listening at {}",
            paths.socket.display()
        );
        Ok(Self {
            paths,
            shutdown,
            thread: Some(thread),
        })
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        // A local connection wakes the blocking accept without relying on a
        // polling loop. The server notices shutdown before spawning a client.
        let _ = UnixStream::connect(&self.paths.socket);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        for (path, socket) in [(&self.paths.socket, true), (&self.paths.token, false)] {
            if let Err(error) = remove_safe_file(path, socket) {
                log::warn!("Could not remove GUI control endpoint: {error:#}");
            }
        }
    }
}

struct ClientGuard(Arc<AtomicUsize>);

impl Drop for ClientGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn run(
    listener: UnixListener,
    token: String,
    proxy: EventLoopProxy<AppEvent>,
    shutdown: Arc<AtomicBool>,
    clients: Arc<AtomicUsize>,
) {
    while !shutdown.load(Ordering::Acquire) {
        let (stream, _) = match listener.accept() {
            Ok(accepted) => accepted,
            Err(error) => {
                if !shutdown.load(Ordering::Acquire) {
                    log::warn!("GUI control listener stopped: {error}");
                }
                break;
            }
        };
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        if clients.fetch_add(1, Ordering::AcqRel) >= MAX_CLIENTS {
            clients.fetch_sub(1, Ordering::AcqRel);
            let mut stream = stream;
            let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
            let _ = protocol::write_frame(
                &mut stream,
                &ServerFrame::error("GUI control client limit reached"),
            );
            continue;
        }
        let token = token.clone();
        let proxy = proxy.clone();
        let guard = ClientGuard(Arc::clone(&clients));
        if let Err(error) = thread::Builder::new()
            .name("termeric-gui-control-client".into())
            .spawn(move || {
                let _guard = guard;
                if let Err(error) = handle_client(stream, &token, &proxy) {
                    log::debug!("GUI control client disconnected: {error:#}");
                }
            })
        {
            log::warn!("Could not start GUI control client worker: {error}");
        }
    }
}

fn handle_client(
    stream: UnixStream,
    expected_token: &str,
    proxy: &EventLoopProxy<AppEvent>,
) -> anyhow::Result<()> {
    serve_client(stream, expected_token, |request, peer_process| {
        let (sender, receiver) = mpsc::sync_channel(1);
        proxy
            .send_event(AppEvent::ControlRequest {
                request,
                peer_process,
                respond_to: sender,
            })
            .map_err(|_| anyhow::anyhow!("GUI event loop is no longer available"))?;
        receiver
            .recv_timeout(EVENT_LOOP_TIMEOUT)
            .context("GUI event loop did not answer control request")
            .and_then(|result| result.map_err(anyhow::Error::msg))
    })
}

fn serve_client(
    mut stream: UnixStream,
    expected_token: &str,
    respond: impl FnOnce(Request, Option<u32>) -> anyhow::Result<Response>,
) -> anyhow::Result<()> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let peer_process = validate_peer(&stream)?;
    let request = protocol::read_frame::<ClientFrame>(&mut stream)?;
    if let Err(message) = validate_request(&request, expected_token) {
        return protocol::write_frame(&mut stream, &ServerFrame::error(bounded_error(&message)));
    }

    let frame = match respond(request.request, peer_process) {
        // The GUI may already have acted on the request, so a reply that
        // fails validation is reported as such rather than dropping the
        // connection, which the client would read as a request that never
        // completed.
        Ok(response) => match response.validate() {
            Ok(()) => ServerFrame::new(response),
            Err(error) => ServerFrame::error(bounded_error(&format!(
                "the GUI's reply was invalid, so the request may already have taken effect: {error:#}"
            ))),
        },
        Err(error) => ServerFrame::error(bounded_error(&format!("{error:#}"))),
    };
    protocol::write_frame(&mut stream, &frame)
}

fn bounded_error(error: &str) -> String {
    let output = crate::safe_text::single_line_within(error, protocol::MAX_ERROR_MESSAGE_BYTES);
    if output.is_empty() {
        "GUI control request failed".into()
    } else {
        output
    }
}

fn validate_request(frame: &ClientFrame, expected_token: &str) -> Result<(), String> {
    if frame.version != protocol::PROTOCOL_VERSION {
        return Err(format!(
            "protocol version {} is incompatible with server version {}",
            frame.version,
            protocol::PROTOCOL_VERSION
        ));
    }
    if frame.token != expected_token {
        return Err("GUI control authentication failed".into());
    }
    frame.request.validate()
}

fn prepare_endpoint(paths: &ControlPaths) -> anyhow::Result<()> {
    if paths.socket.exists() && UnixStream::connect(&paths.socket).is_ok() {
        anyhow::bail!(
            "GUI control endpoint for process {} is already active",
            paths.process_id
        );
    }
    remove_safe_file(&paths.socket, true)?;
    remove_safe_file(&paths.token, false)
}

/// Refuses a peer of another user, and returns the peer's process ID when
/// the kernel reports one (a peer in another PID namespace reads as 0).
fn validate_peer(stream: &UnixStream) -> anyhow::Result<Option<u32>> {
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: the connected descriptor and initialized output storage remain
    // valid for the duration of getsockopt.
    let result = unsafe {
        libc::getsockopt(
            std::os::fd::AsRawFd::as_raw_fd(stream),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error()).context("authenticate GUI control peer");
    }
    if credentials.uid != crate::domain::paths::effective_uid() {
        anyhow::bail!("GUI control peer belongs to a different user");
    }
    Ok(u32::try_from(credentials.pid).ok().filter(|pid| *pid > 0))
}

#[cfg(test)]
mod tests {
    #[test]
    fn authenticated_launch_status_round_trip_does_not_invoke_a_mutation() {
        use super::*;
        let id = "c".repeat(32);
        let expected_id = id.clone();
        let (mut client, server) = UnixStream::pair().unwrap();
        let worker = thread::spawn(move || {
            serve_client(server, "fixture-token", |request, _| {
                assert!(request.request_id().is_none());
                assert_eq!(
                    request,
                    Request::LaunchStatus {
                        launch_request_id: expected_id.clone()
                    }
                );
                Ok(Response::LaunchStatus {
                    process_id: 42,
                    launch_request_id: expected_id,
                    state: protocol::LaunchState::Exited {
                        exit_code: 17,
                        success: false,
                    },
                    terminal_closed: true,
                })
            })
            .unwrap();
        });
        protocol::write_frame(
            &mut client,
            &ClientFrame {
                version: protocol::PROTOCOL_VERSION,
                token: "fixture-token".into(),
                request: Request::LaunchStatus {
                    launch_request_id: id.clone(),
                },
            },
        )
        .unwrap();
        assert_eq!(
            protocol::read_frame::<ServerFrame>(&mut client).unwrap(),
            ServerFrame::new(Response::LaunchStatus {
                process_id: 42,
                launch_request_id: id,
                state: protocol::LaunchState::Exited {
                    exit_code: 17,
                    success: false
                },
                terminal_closed: true,
            })
        );
        worker.join().unwrap();
    }
    #[test]
    fn agent_events_round_trip_and_invalid_identities_are_refused() {
        use super::*;
        let event = Request::AgentEvent {
            terminal_id: "4242.0123456789abcdef0123456789abcdef".into(),
            agent_id: "codex".into(),
            event: protocol::AgentEventKind::NeedsInput,
            detail: Some("Approve shell: cargo test".into()),
            last_message: None,
            transcript_path: Some("/home/me/.codex/sessions/rollout.jsonl".into()),
        };
        let expected = event.clone();
        let (mut client, server) = UnixStream::pair().unwrap();
        let worker = thread::spawn(move || {
            serve_client(server, "fixture-token", |request, _| {
                assert!(request.request_id().is_none(), "no replay identity");
                assert_eq!(request, expected);
                Ok(Response::AgentEventRecorded { process_id: 4242 })
            })
            .unwrap();
        });
        protocol::write_frame(
            &mut client,
            &ClientFrame {
                version: protocol::PROTOCOL_VERSION,
                token: "fixture-token".into(),
                request: event,
            },
        )
        .unwrap();
        assert_eq!(
            protocol::read_frame::<ServerFrame>(&mut client).unwrap(),
            ServerFrame::new(Response::AgentEventRecorded { process_id: 4242 })
        );
        worker.join().unwrap();

        let invalid = |terminal_id: &str, agent_id: &str, path: Option<&str>| Request::AgentEvent {
            terminal_id: terminal_id.into(),
            agent_id: agent_id.into(),
            event: protocol::AgentEventKind::Finished,
            detail: None,
            last_message: None,
            transcript_path: path.map(Into::into),
        };
        for request in [
            invalid("4242", "codex", None),
            invalid("pid.abc", "codex", None),
            invalid("4242.xyz", "codex", None),
            invalid("4242.0123456789abcdef0123456789abcdef", "co dex", None),
            invalid("4242.0123456789abcdef0123456789abcdef", "", None),
            invalid(
                "4242.0123456789abcdef0123456789abcdef",
                "codex",
                Some("relative/path"),
            ),
        ] {
            assert!(request.validate().is_err(), "{request:?}");
        }
    }

    use super::*;

    #[test]
    fn negotiation_requires_exact_version_and_secret() {
        let valid = ClientFrame {
            version: protocol::PROTOCOL_VERSION,
            token: "expected".into(),
            request: Request::Inventory,
        };
        assert_eq!(validate_request(&valid, "expected"), Ok(()));
        assert!(
            validate_request(
                &ClientFrame {
                    version: 99,
                    ..valid.clone()
                },
                "expected"
            )
            .is_err()
        );
        assert!(
            validate_request(
                &ClientFrame {
                    token: "wrong".into(),
                    ..valid
                },
                "expected"
            )
            .is_err()
        );
    }

    /// Serves one authenticated `request` whose GUI answer is `answer`, and
    /// returns the frame the client reads.
    fn exchange(
        request: Request,
        answer: impl FnOnce(Request) -> anyhow::Result<Response> + Send + 'static,
    ) -> ServerFrame {
        let (mut client, server) = UnixStream::pair().unwrap();
        let expected = request.clone();
        let worker = thread::spawn(move || {
            serve_client(server, "secret", |received, _| {
                assert_eq!(received, expected);
                answer(received)
            })
            .unwrap();
        });
        protocol::write_frame(
            &mut client,
            &ClientFrame {
                version: protocol::PROTOCOL_VERSION,
                token: "secret".into(),
                request,
            },
        )
        .unwrap();
        let frame = protocol::read_frame::<ServerFrame>(&mut client).unwrap();
        worker.join().unwrap();
        frame
    }

    #[test]
    fn authenticated_requests_carry_exactly_the_gui_reply() {
        let request_id = || "0123456789abcdef0123456789abcdef".to_string();
        let environment_policy = termeric_core::pty::EnvironmentPolicy {
            clear: false,
            unset: vec!["PATH".into()],
        };
        let environment = vec![protocol::EnvironmentVariable {
            name: "MODE".into(),
            value: "direct".into(),
        }];
        for (request, response) in [
            (
                Request::Inventory,
                Response::Inventory(protocol::ProcessInventory {
                    process_id: 42,
                    windows: Vec::new(),
                    truncated: false,
                }),
            ),
            (
                Request::CreateWindow {
                    request_id: request_id(),
                    source_window: 7,
                },
                Response::WindowCreated {
                    process_id: 42,
                    request_id: request_id(),
                    window_id: 9,
                },
            ),
            (
                Request::LaunchWindow {
                    request_id: request_id(),
                    source_window: 7,
                    program: "/usr/bin/printf".into(),
                    args: vec!["%s".into(), "$HOME;🙂".into()],
                    working_directory: Some("/tmp".into()),
                    title: Some("exact window".into()),
                    environment: environment.clone(),
                    environment_policy: environment_policy.clone(),
                },
                Response::WindowLaunched {
                    process_id: 42,
                    request_id: request_id(),
                    window_id: 9,
                    tab_id: 0,
                },
            ),
            (
                Request::LaunchTab {
                    background: false,
                    request_id: request_id(),
                    window_id: 7,
                    program: "/usr/bin/printf".into(),
                    args: vec!["%s".into(), "$HOME;🙂".into()],
                    working_directory: Some("/tmp".into()),
                    title: Some("exact".into()),
                    environment,
                    environment_policy,
                },
                Response::TabCreated {
                    process_id: 42,
                    request_id: request_id(),
                    window_id: 7,
                    tab_id: 4,
                },
            ),
            (
                Request::SplitPane {
                    request_id: request_id(),
                    window_id: 7,
                    tab_id: 3,
                    pane_id: 0,
                    direction: crate::panes::SplitDirection::Vertical,
                },
                Response::PaneCreated {
                    process_id: 42,
                    request_id: request_id(),
                    window_id: 7,
                    tab_id: 3,
                    pane_id: 1,
                },
            ),
        ] {
            let reply = response.clone();
            assert_eq!(
                exchange(request, move |_| Ok(reply)),
                ServerFrame::new(response)
            );
        }

        // A GUI failure becomes one bounded, single-line error.
        assert_eq!(
            exchange(Request::Inventory, |_| anyhow::bail!("missing\nwindow")),
            ServerFrame::error("missingwindow")
        );
    }

    #[test]
    fn an_invalid_gui_reply_is_reported_as_possibly_applied_not_dropped() {
        const REQUEST_ID: &str = "0123456789abcdef0123456789abcdef";
        let split = Request::SplitPane {
            request_id: REQUEST_ID.into(),
            window_id: 7,
            tab_id: 3,
            pane_id: 0,
            direction: crate::panes::SplitDirection::Vertical,
        };
        let frame = exchange(split, |_| {
            Ok(Response::PaneCreated {
                process_id: 42,
                request_id: REQUEST_ID.into(),
                window_id: 0,
                tab_id: 3,
                pane_id: 1,
            })
        });
        assert!(matches!(
            frame.response,
            Response::Error { message } if message.contains("may already have taken effect")
        ));
    }
}
