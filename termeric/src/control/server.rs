//! Same-user Unix-socket server for every GUI control request: one
//! authenticated request and reply per connection, answered by the GUI event
//! loop.

use super::paths::ControlEndpoint;
use super::protocol::{self, ClientFrame, Request, Response, ServerFrame};
use super::{EVENT_LOOP_TIMEOUT, IO_TIMEOUT};
use crate::app::AppEvent;
use crate::local_socket::{self, OwnedFile};
use anyhow::Context;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use winit::event_loop::EventLoopProxy;

const MAX_CLIENTS: usize = 16;

pub struct ControlServer {
    endpoint: ControlEndpoint,
    shutdown: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl ControlServer {
    pub fn start(proxy: EventLoopProxy<AppEvent>) -> anyhow::Result<Self> {
        let endpoint = ControlEndpoint::discover_current()?;
        prepare_endpoint(&endpoint)?;
        match Self::start_prepared(endpoint.clone(), proxy) {
            Ok(server) => Ok(server),
            Err(error) => {
                let _ = remove_endpoint(&endpoint);
                Err(error)
            }
        }
    }

    fn start_prepared(
        endpoint: ControlEndpoint,
        proxy: EventLoopProxy<AppEvent>,
    ) -> anyhow::Result<Self> {
        let paths = &endpoint.paths;
        let token = paths.load_or_create_token()?;
        let listener = UnixListener::bind(&paths.socket)
            .with_context(|| format!("bind GUI control socket {}", paths.socket.display()))?;
        fs::set_permissions(&paths.socket, fs::Permissions::from_mode(0o600))?;
        let metadata = fs::symlink_metadata(&paths.socket)?;
        if metadata.uid() != local_socket::effective_uid()
            || metadata.permissions().mode() & 0o777 != 0o600
        {
            anyhow::bail!("GUI control socket could not be secured to mode 0600");
        }

        let shutdown = Arc::new(AtomicBool::new(false));
        let server_shutdown = Arc::clone(&shutdown);
        let thread = thread::Builder::new()
            .name("termeric-gui-control".into())
            .spawn(move || run(listener, token, proxy, server_shutdown))?;
        log::info!(
            "GUI control endpoint listening at {}",
            paths.socket.display()
        );
        Ok(Self {
            endpoint,
            shutdown,
            thread: Some(thread),
        })
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        // The server notices shutdown before spawning a client.
        local_socket::wake(&self.endpoint.paths.socket);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        for removed in remove_endpoint(&self.endpoint) {
            if let Err(error) = removed {
                log::warn!("Could not remove GUI control endpoint: {error:#}");
            }
        }
    }
}

/// Removes the endpoint's socket and its token, each whatever became of the
/// other.
fn remove_endpoint(endpoint: &ControlEndpoint) -> [anyhow::Result<()>; 2] {
    [
        local_socket::remove_owned(&endpoint.paths.socket, OwnedFile::Socket),
        local_socket::remove_owned(&endpoint.paths.token, OwnedFile::Regular),
    ]
}

fn run(
    listener: UnixListener,
    token: String,
    proxy: EventLoopProxy<AppEvent>,
    shutdown: Arc<AtomicBool>,
) {
    let served = local_socket::accept_clients(
        &listener,
        &shutdown,
        &local_socket::ClientSlots::new(MAX_CLIENTS),
        |mut stream| {
            let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
            let _ = protocol::write_frame(
                &mut stream,
                &ServerFrame::error("GUI control client limit reached"),
            );
        },
        |stream, slot| {
            let token = token.clone();
            let proxy = proxy.clone();
            if let Err(error) = thread::Builder::new()
                .name("termeric-gui-control-client".into())
                .spawn(move || {
                    let _slot = slot;
                    if let Err(error) = handle_client(stream, &token, &proxy) {
                        log::debug!("GUI control client disconnected: {error:#}");
                    }
                })
            {
                log::warn!("Could not start GUI control client worker: {error}");
            }
            Ok(())
        },
    );
    if let Err(error) = served
        && !shutdown.load(Ordering::Acquire)
    {
        log::warn!("GUI control listener stopped: {error:#}");
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
    let peer_process = local_socket::same_user_peer(&stream, "GUI control peer")?;
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
    local_socket::authenticate(
        "GUI control",
        protocol::PROTOCOL_VERSION,
        frame.version,
        &frame.token,
        expected_token,
    )?;
    frame.request.validate()
}

fn prepare_endpoint(endpoint: &ControlEndpoint) -> anyhow::Result<()> {
    if local_socket::clear_stale_socket(&endpoint.paths.socket)? == local_socket::SocketPath::InUse
    {
        anyhow::bail!(
            "GUI control endpoint for process {} is already active",
            endpoint.process_id
        );
    }
    local_socket::remove_owned(&endpoint.paths.token, OwnedFile::Regular)
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
