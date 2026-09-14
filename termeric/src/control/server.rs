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
                &ServerFrame::Error {
                    version: protocol::PROTOCOL_VERSION,
                    message: "GUI control client limit reached".into(),
                },
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
    serve_client(stream, expected_token, |request| {
        let (sender, receiver) = mpsc::sync_channel(1);
        proxy
            .send_event(AppEvent::ControlRequest {
                request,
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
    respond: impl FnOnce(Request) -> anyhow::Result<Response>,
) -> anyhow::Result<()> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    validate_peer(&stream)?;
    let request = protocol::read_frame::<ClientFrame>(&mut stream)?;
    if let Err(message) = validate_request(&request, expected_token) {
        protocol::write_frame(
            &mut stream,
            &ServerFrame::Error {
                version: protocol::PROTOCOL_VERSION,
                message,
            },
        )?;
        return Ok(());
    }

    let response = match respond(request.request) {
        Ok(response) => response,
        Err(error) => {
            return protocol::write_frame(
                &mut stream,
                &ServerFrame::Error {
                    version: protocol::PROTOCOL_VERSION,
                    message: bounded_error(&format!("{error:#}")),
                },
            );
        }
    };
    let frame = match response {
        Response::LaunchStatus {
            process_id,
            launch_request_id,
            state,
            terminal_closed,
        } => {
            if process_id == 0 || !protocol::valid_request_id(&launch_request_id) {
                anyhow::bail!("Invalid GUI launch-status identity");
            }
            state.validate().map_err(anyhow::Error::msg)?;
            ServerFrame::LaunchStatus {
                version: protocol::PROTOCOL_VERSION,
                process_id,
                launch_request_id,
                state,
                terminal_closed,
            }
        }
        Response::Inventory(inventory) => {
            inventory.validate()?;
            ServerFrame::Inventory {
                version: protocol::PROTOCOL_VERSION,
                inventory,
            }
        }
        Response::Text {
            process_id,
            window_id,
            tab_id,
            pane_id,
            screen,
            history_lines,
            text,
            truncated,
        } => {
            if process_id == 0
                || process_id > i32::MAX as u32
                || window_id == 0
                || history_lines > protocol::MAX_TEXT_HISTORY_LINES
                || text.len() > protocol::MAX_TERMINAL_TEXT_BYTES as usize
            {
                anyhow::bail!("GUI event loop returned an invalid terminal-text response");
            }
            ServerFrame::Text {
                version: protocol::PROTOCOL_VERSION,
                process_id,
                window_id,
                tab_id,
                pane_id,
                screen,
                history_lines,
                text,
                truncated,
            }
        }
        Response::InputAccepted {
            process_id,
            request_id,
            window_id,
            tab_id,
            pane_id,
            input_bytes,
            wire_bytes,
            paste,
            enter,
            disposition,
            confirmation_id,
        } => {
            validate_action_response(process_id, &request_id, window_id)?;
            let valid_disposition = match (disposition, confirmation_id) {
                (protocol::InputDisposition::Sent, None) => true,
                (protocol::InputDisposition::PendingConfirmation, Some(id)) => id != 0,
                _ => false,
            };
            if input_bytes == 0
                || input_bytes as usize > protocol::MAX_CONTROL_INPUT_BYTES + 1
                || wire_bytes < input_bytes
                || wire_bytes as usize > protocol::MAX_CONTROL_INPUT_BYTES + 13
                || !valid_disposition
            {
                anyhow::bail!("GUI event loop returned an invalid terminal-input response");
            }
            ServerFrame::InputAccepted {
                version: protocol::PROTOCOL_VERSION,
                process_id,
                request_id,
                window_id,
                tab_id,
                pane_id,
                input_bytes,
                wire_bytes,
                paste,
                enter,
                disposition,
                confirmation_id,
            }
        }
        Response::WindowCreated {
            process_id,
            request_id,
            window_id,
        } => {
            if process_id == 0
                || process_id > i32::MAX as u32
                || !protocol::valid_request_id(&request_id)
                || window_id == 0
            {
                anyhow::bail!("GUI event loop returned an invalid window-creation response");
            }
            ServerFrame::WindowCreated {
                version: protocol::PROTOCOL_VERSION,
                process_id,
                request_id,
                window_id,
            }
        }
        Response::WindowLaunched {
            process_id,
            request_id,
            window_id,
            tab_id,
        } => {
            if process_id == 0
                || process_id > i32::MAX as u32
                || !protocol::valid_request_id(&request_id)
                || window_id == 0
            {
                anyhow::bail!("GUI event loop returned an invalid window-launch response");
            }
            ServerFrame::WindowLaunched {
                version: protocol::PROTOCOL_VERSION,
                process_id,
                request_id,
                window_id,
                tab_id,
            }
        }
        Response::TabCreated {
            process_id,
            request_id,
            window_id,
            tab_id,
        } => {
            if process_id == 0
                || process_id > i32::MAX as u32
                || !protocol::valid_request_id(&request_id)
                || window_id == 0
            {
                anyhow::bail!("GUI event loop returned an invalid tab-creation response");
            }
            ServerFrame::TabCreated {
                version: protocol::PROTOCOL_VERSION,
                process_id,
                request_id,
                window_id,
                tab_id,
            }
        }
        Response::PaneCreated {
            process_id,
            request_id,
            window_id,
            tab_id,
            pane_id,
        } => {
            if process_id == 0
                || process_id > i32::MAX as u32
                || !protocol::valid_request_id(&request_id)
                || window_id == 0
                || pane_id == 0
            {
                anyhow::bail!("GUI event loop returned an invalid pane-creation response");
            }
            ServerFrame::PaneCreated {
                version: protocol::PROTOCOL_VERSION,
                process_id,
                request_id,
                window_id,
                tab_id,
                pane_id,
            }
        }
        Response::WindowFocusRequested {
            process_id,
            request_id,
            window_id,
            already_focused,
        } => {
            validate_action_response(process_id, &request_id, window_id)?;
            ServerFrame::WindowFocusRequested {
                version: protocol::PROTOCOL_VERSION,
                process_id,
                request_id,
                window_id,
                already_focused,
            }
        }
        Response::QuickAccessChanged {
            process_id,
            request_id,
            window_id,
            visible,
        } => {
            validate_action_response(process_id, &request_id, window_id)?;
            ServerFrame::QuickAccessChanged {
                version: protocol::PROTOCOL_VERSION,
                process_id,
                request_id,
                window_id,
                visible,
            }
        }
        Response::TabSelected {
            process_id,
            request_id,
            window_id,
            tab_id,
            changed,
        } => {
            validate_action_response(process_id, &request_id, window_id)?;
            ServerFrame::TabSelected {
                version: protocol::PROTOCOL_VERSION,
                process_id,
                request_id,
                window_id,
                tab_id,
                changed,
            }
        }
        Response::PaneFocused {
            process_id,
            request_id,
            window_id,
            tab_id,
            pane_id,
            tab_changed,
            pane_changed,
        } => {
            validate_action_response(process_id, &request_id, window_id)?;
            ServerFrame::PaneFocused {
                version: protocol::PROTOCOL_VERSION,
                process_id,
                request_id,
                window_id,
                tab_id,
                pane_id,
                tab_changed,
                pane_changed,
            }
        }
        Response::WindowCloseRequested {
            process_id,
            request_id,
            window_id,
        } => {
            validate_action_response(process_id, &request_id, window_id)?;
            ServerFrame::WindowCloseRequested {
                version: protocol::PROTOCOL_VERSION,
                process_id,
                request_id,
                window_id,
            }
        }
        Response::TabCloseRequested {
            process_id,
            request_id,
            window_id,
            tab_id,
        } => {
            validate_action_response(process_id, &request_id, window_id)?;
            ServerFrame::TabCloseRequested {
                version: protocol::PROTOCOL_VERSION,
                process_id,
                request_id,
                window_id,
                tab_id,
            }
        }
        Response::PaneClosed {
            process_id,
            request_id,
            window_id,
            tab_id,
            pane_id,
            remaining_panes,
            focused_pane_id,
        } => {
            validate_action_response(process_id, &request_id, window_id)?;
            if remaining_panes == 0 || usize::from(remaining_panes) > protocol::MAX_PANES_PER_TAB {
                anyhow::bail!("GUI event loop returned an invalid pane-close response");
            }
            ServerFrame::PaneClosed {
                version: protocol::PROTOCOL_VERSION,
                process_id,
                request_id,
                window_id,
                tab_id,
                pane_id,
                remaining_panes,
                focused_pane_id,
            }
        }
        Response::TabMoved {
            process_id,
            request_id,
            source_window_id,
            source_tab_id,
            destination_window_id,
            destination_tab_id,
            destination_index,
            source_window_closing,
            changed,
        } => {
            validate_action_response(process_id, &request_id, source_window_id)?;
            if destination_window_id == 0
                || usize::from(destination_index) >= protocol::MAX_TABS_PER_WINDOW
            {
                anyhow::bail!("GUI event loop returned an invalid tab-move response");
            }
            ServerFrame::TabMoved {
                version: protocol::PROTOCOL_VERSION,
                process_id,
                request_id,
                source_window_id,
                source_tab_id,
                destination_window_id,
                destination_tab_id,
                destination_index,
                source_window_closing,
                changed,
            }
        }
        Response::PaneMoved {
            process_id,
            request_id,
            source_window_id,
            source_tab_id,
            source_pane_id,
            destination_window_id,
            destination_tab_id,
            destination_pane_id,
            target_pane_id,
        } => {
            validate_action_response(process_id, &request_id, source_window_id)?;
            if destination_window_id == 0 || destination_pane_id == target_pane_id {
                anyhow::bail!("GUI event loop returned an invalid pane-move response");
            }
            ServerFrame::PaneMoved {
                version: protocol::PROTOCOL_VERSION,
                process_id,
                request_id,
                source_window_id,
                source_tab_id,
                source_pane_id,
                destination_window_id,
                destination_tab_id,
                destination_pane_id,
                target_pane_id,
            }
        }
        Response::WindowResizeRequested {
            process_id,
            request_id,
            window_id,
            requested_width,
            requested_height,
            immediate_width,
            immediate_height,
        } => {
            validate_action_response(process_id, &request_id, window_id)?;
            if !(protocol::MIN_WINDOW_WIDTH..=protocol::MAX_WINDOW_DIMENSION)
                .contains(&requested_width)
                || !(protocol::MIN_WINDOW_HEIGHT..=protocol::MAX_WINDOW_DIMENSION)
                    .contains(&requested_height)
                || immediate_width.is_some() != immediate_height.is_some()
                || immediate_width == Some(0)
                || immediate_height == Some(0)
            {
                anyhow::bail!("GUI event loop returned an invalid window-resize response");
            }
            ServerFrame::WindowResizeRequested {
                version: protocol::PROTOCOL_VERSION,
                process_id,
                request_id,
                window_id,
                requested_width,
                requested_height,
                immediate_width,
                immediate_height,
            }
        }
        Response::DividerResized {
            process_id,
            request_id,
            window_id,
            tab_id,
            divider_id,
            previous_ratio_millis,
            ratio_millis,
            changed,
        } => {
            validate_action_response(process_id, &request_id, window_id)?;
            if !(100..=900).contains(&previous_ratio_millis)
                || !(100..=900).contains(&ratio_millis)
                || changed != (previous_ratio_millis != ratio_millis)
            {
                anyhow::bail!("GUI event loop returned an invalid divider-resize response");
            }
            ServerFrame::DividerResized {
                version: protocol::PROTOCOL_VERSION,
                process_id,
                request_id,
                window_id,
                tab_id,
                divider_id,
                previous_ratio_millis,
                ratio_millis,
                changed,
            }
        }
    };
    protocol::write_frame(&mut stream, &frame)
}

fn validate_action_response(
    process_id: u32,
    request_id: &str,
    window_id: u64,
) -> anyhow::Result<()> {
    if process_id == 0
        || process_id > i32::MAX as u32
        || !protocol::valid_request_id(request_id)
        || window_id == 0
    {
        anyhow::bail!("GUI event loop returned an invalid control-action response");
    }
    Ok(())
}

fn bounded_error(error: &str) -> String {
    const MAX_ERROR_BYTES: usize = 1_024;
    let mut output = String::with_capacity(error.len().min(MAX_ERROR_BYTES));
    for character in error.chars().filter(|character| !character.is_control()) {
        if output.len() + character.len_utf8() > MAX_ERROR_BYTES {
            break;
        }
        output.push(character);
    }
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

#[cfg(target_os = "linux")]
fn validate_peer(stream: &UnixStream) -> anyhow::Result<()> {
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
    Ok(())
}

#[cfg(all(unix, not(target_os = "linux")))]
fn validate_peer(stream: &UnixStream) -> anyhow::Result<()> {
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: getpeereid receives a connected descriptor and valid output
    // pointers and retains neither.
    let result =
        unsafe { libc::getpeereid(std::os::fd::AsRawFd::as_raw_fd(stream), &mut uid, &mut gid) };
    if result != 0 {
        return Err(std::io::Error::last_os_error()).context("authenticate GUI control peer");
    }
    if uid != crate::domain::paths::effective_uid() {
        anyhow::bail!("GUI control peer belongs to a different user");
    }
    Ok(())
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
            serve_client(server, "fixture-token", |request| {
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
            ServerFrame::LaunchStatus {
                version: protocol::PROTOCOL_VERSION,
                process_id: 42,
                launch_request_id: id,
                state: protocol::LaunchState::Exited {
                    exit_code: 17,
                    success: false
                },
                terminal_closed: true,
            }
        );
        worker.join().unwrap();
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
        assert!(validate_action_response(42, "0123456789abcdef0123456789abcdef", 7,).is_ok());
        assert!(validate_action_response(0, "bad", 0).is_err());
    }

    #[test]
    fn authenticated_request_returns_only_the_supplied_bounded_inventory() {
        let (mut probe, _peer) = UnixStream::pair().unwrap();
        if let Err(error) = std::io::Write::write_all(&mut probe, &[0]) {
            if error.kind() == std::io::ErrorKind::PermissionDenied {
                eprintln!("skip GUI control socket exchange: sandbox denies AF_UNIX I/O");
                return;
            }
            panic!("could not probe Unix-domain socket I/O: {error}");
        }
        let (mut client, server) = UnixStream::pair().unwrap();
        let worker = thread::spawn(move || {
            serve_client(server, "secret", |request| {
                assert_eq!(request, Request::Inventory);
                Ok(Response::Inventory(protocol::ProcessInventory {
                    process_id: 42,
                    windows: Vec::new(),
                    truncated: false,
                }))
            })
            .unwrap();
        });
        protocol::write_frame(
            &mut client,
            &ClientFrame {
                version: protocol::PROTOCOL_VERSION,
                token: "secret".into(),
                request: Request::Inventory,
            },
        )
        .unwrap();
        assert!(matches!(
            protocol::read_frame::<ServerFrame>(&mut client).unwrap(),
            ServerFrame::Inventory { inventory, .. }
                if inventory.process_id == 42 && inventory.windows.is_empty()
        ));
        worker.join().unwrap();

        let request_id = "0123456789abcdef0123456789abcdef".to_string();
        let (mut client, server) = UnixStream::pair().unwrap();
        let expected_id = request_id.clone();
        let worker = thread::spawn(move || {
            serve_client(server, "secret", |request| {
                assert_eq!(
                    request,
                    Request::CreateWindow {
                        request_id: expected_id.clone(),
                        source_window: 7,
                    }
                );
                Ok(Response::WindowCreated {
                    process_id: 42,
                    request_id: expected_id,
                    window_id: 9,
                })
            })
            .unwrap();
        });
        protocol::write_frame(
            &mut client,
            &ClientFrame {
                version: protocol::PROTOCOL_VERSION,
                token: "secret".into(),
                request: Request::CreateWindow {
                    request_id: request_id.clone(),
                    source_window: 7,
                },
            },
        )
        .unwrap();
        assert!(matches!(
            protocol::read_frame::<ServerFrame>(&mut client).unwrap(),
            ServerFrame::WindowCreated {
                process_id: 42,
                request_id: returned,
                window_id: 9,
                ..
            } if returned == request_id
        ));
        worker.join().unwrap();

        let request_id = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string();
        let launch = Request::LaunchWindow {
            request_id: request_id.clone(),
            source_window: 7,
            program: "/usr/bin/printf".into(),
            args: vec!["%s".into(), "$HOME;🙂".into()],
            working_directory: Some("/tmp".into()),
            title: Some("exact window".into()),
            environment_policy: termeric_core::pty::EnvironmentPolicy {
                clear: true,
                unset: vec![],
            },
            environment: vec![protocol::EnvironmentVariable {
                name: "WINDOW_MODE".into(),
                value: "direct".into(),
            }],
        };
        let (mut client, server) = UnixStream::pair().unwrap();
        let expected_request = launch.clone();
        let expected_id = request_id.clone();
        let worker = thread::spawn(move || {
            serve_client(server, "secret", |request| {
                assert_eq!(request, expected_request);
                Ok(Response::WindowLaunched {
                    process_id: 42,
                    request_id: expected_id,
                    window_id: 9,
                    tab_id: 0,
                })
            })
            .unwrap();
        });
        protocol::write_frame(
            &mut client,
            &ClientFrame {
                version: protocol::PROTOCOL_VERSION,
                token: "secret".into(),
                request: launch,
            },
        )
        .unwrap();
        assert!(matches!(
            protocol::read_frame::<ServerFrame>(&mut client).unwrap(),
            ServerFrame::WindowLaunched {
                process_id: 42,
                request_id: returned,
                window_id: 9,
                tab_id: 0,
                ..
            } if returned == request_id
        ));
        worker.join().unwrap();

        let request_id = "fedcba9876543210fedcba9876543210".to_string();
        let (mut client, server) = UnixStream::pair().unwrap();
        let expected_id = request_id.clone();
        let worker = thread::spawn(move || {
            serve_client(server, "secret", |request| {
                assert_eq!(
                    request,
                    Request::CreateTab {
                        request_id: expected_id.clone(),
                        window_id: 7,
                    }
                );
                Ok(Response::TabCreated {
                    process_id: 42,
                    request_id: expected_id,
                    window_id: 7,
                    tab_id: 3,
                })
            })
            .unwrap();
        });
        protocol::write_frame(
            &mut client,
            &ClientFrame {
                version: protocol::PROTOCOL_VERSION,
                token: "secret".into(),
                request: Request::CreateTab {
                    request_id: request_id.clone(),
                    window_id: 7,
                },
            },
        )
        .unwrap();
        assert!(matches!(
            protocol::read_frame::<ServerFrame>(&mut client).unwrap(),
            ServerFrame::TabCreated {
                process_id: 42,
                request_id: returned,
                window_id: 7,
                tab_id: 3,
                ..
            } if returned == request_id
        ));
        worker.join().unwrap();

        let request_id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string();
        let launch = Request::LaunchTab {
            request_id: request_id.clone(),
            window_id: 7,
            program: "/usr/bin/printf".into(),
            args: vec!["%s".into(), "$HOME;🙂".into()],
            working_directory: Some("/tmp".into()),
            title: Some("exact".into()),
            environment_policy: termeric_core::pty::EnvironmentPolicy {
                clear: false,
                unset: vec!["PATH".into()],
            },
            environment: vec![protocol::EnvironmentVariable {
                name: "TAB_MODE".into(),
                value: "direct".into(),
            }],
        };
        let (mut client, server) = UnixStream::pair().unwrap();
        let expected_request = launch.clone();
        let expected_id = request_id.clone();
        let worker = thread::spawn(move || {
            serve_client(server, "secret", |request| {
                assert_eq!(request, expected_request);
                Ok(Response::TabCreated {
                    process_id: 42,
                    request_id: expected_id,
                    window_id: 7,
                    tab_id: 4,
                })
            })
            .unwrap();
        });
        protocol::write_frame(
            &mut client,
            &ClientFrame {
                version: protocol::PROTOCOL_VERSION,
                token: "secret".into(),
                request: launch,
            },
        )
        .unwrap();
        assert!(matches!(
            protocol::read_frame::<ServerFrame>(&mut client).unwrap(),
            ServerFrame::TabCreated {
                process_id: 42,
                request_id: returned,
                window_id: 7,
                tab_id: 4,
                ..
            } if returned == request_id
        ));
        worker.join().unwrap();

        let request_id = "1234567890abcdef1234567890abcdef".to_string();
        let (mut client, server) = UnixStream::pair().unwrap();
        let expected_id = request_id.clone();
        let worker = thread::spawn(move || {
            serve_client(server, "secret", |request| {
                assert_eq!(
                    request,
                    Request::SplitPane {
                        request_id: expected_id.clone(),
                        window_id: 7,
                        tab_id: 3,
                        pane_id: 0,
                        direction: protocol::SplitDirection::Vertical,
                    }
                );
                Ok(Response::PaneCreated {
                    process_id: 42,
                    request_id: expected_id,
                    window_id: 7,
                    tab_id: 3,
                    pane_id: 1,
                })
            })
            .unwrap();
        });
        protocol::write_frame(
            &mut client,
            &ClientFrame {
                version: protocol::PROTOCOL_VERSION,
                token: "secret".into(),
                request: Request::SplitPane {
                    request_id: request_id.clone(),
                    window_id: 7,
                    tab_id: 3,
                    pane_id: 0,
                    direction: protocol::SplitDirection::Vertical,
                },
            },
        )
        .unwrap();
        assert!(matches!(
            protocol::read_frame::<ServerFrame>(&mut client).unwrap(),
            ServerFrame::PaneCreated {
                process_id: 42,
                request_id: returned,
                window_id: 7,
                tab_id: 3,
                pane_id: 1,
                ..
            } if returned == request_id
        ));
        worker.join().unwrap();

        let (mut client, server) = UnixStream::pair().unwrap();
        let worker = thread::spawn(move || {
            serve_client(server, "secret", |_| anyhow::bail!("missing\nwindow")).unwrap();
        });
        protocol::write_frame(
            &mut client,
            &ClientFrame {
                version: protocol::PROTOCOL_VERSION,
                token: "secret".into(),
                request: Request::Inventory,
            },
        )
        .unwrap();
        assert!(matches!(
            protocol::read_frame::<ServerFrame>(&mut client).unwrap(),
            ServerFrame::Error { message, .. }
                if message == "missingwindow" && !message.chars().any(char::is_control)
        ));
        worker.join().unwrap();
    }
}
