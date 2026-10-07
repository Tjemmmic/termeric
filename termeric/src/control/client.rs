//! Discovery client and safe command-line formatting for GUI inventory.

use super::paths::ControlEndpoint;
use super::protocol::{self, ClientFrame, ProcessInventory, Request, Response, ServerFrame};
use crate::deadline_io::{Deadline, PollReader, SocketWriter};
use anyhow::Context;
use serde::Serialize;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

pub(super) const IO_TIMEOUT: Duration = Duration::from_secs(12);

#[derive(Serialize)]
struct InventoryCollection {
    schema_version: u32,
    processes: Vec<ProcessInventory>,
}

pub fn list(json: bool) -> anyhow::Result<String> {
    let mut processes = Vec::new();
    for endpoint in ControlEndpoint::discover_all()? {
        match request_inventory(&endpoint) {
            Ok(inventory) => {
                if inventory.process_id != endpoint.process_id {
                    anyhow::bail!(
                        "GUI control endpoint {} answered for unexpected process {}",
                        endpoint.process_id,
                        inventory.process_id
                    );
                }
                processes.push(inventory);
            }
            Err(error) if endpoint_is_stale(&error, endpoint.process_id) => {
                log::debug!(
                    "Ignoring stale GUI control endpoint for process {}: {error:#}",
                    endpoint.process_id
                );
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "query running GUI process {} through {}",
                        endpoint.process_id,
                        endpoint.paths.socket.display()
                    )
                });
            }
        }
    }
    processes.sort_by_key(|inventory| inventory.process_id);
    if json {
        Ok(serde_json::to_string_pretty(&InventoryCollection {
            schema_version: protocol::JSON_SCHEMA_VERSION,
            processes,
        })?)
    } else {
        Ok(format_human(&processes))
    }
}

/// Toggle the one discoverable quick-access window. `None` means no running
/// process owns one and the caller may safely attempt a guarded launch.
pub fn toggle_existing_quick_access() -> anyhow::Result<Option<String>> {
    toggle_existing_quick_access_at(ControlEndpoint::discover_all()?)
}

fn toggle_existing_quick_access_at(
    endpoints: Vec<ControlEndpoint>,
) -> anyhow::Result<Option<String>> {
    let mut targets = Vec::new();
    let mut incomplete_inventory = false;
    for endpoint in endpoints {
        match request_inventory(&endpoint) {
            Ok(inventory) => {
                if inventory.process_id != endpoint.process_id {
                    anyhow::bail!(
                        "GUI control endpoint {} answered for unexpected process {}",
                        endpoint.process_id,
                        inventory.process_id
                    );
                }
                incomplete_inventory |= inventory.truncated;
                for window in inventory.windows {
                    if window.quick_access {
                        let visible = !(window.visible && window.focused);
                        targets.push((endpoint.clone(), window.id, visible));
                    }
                }
            }
            Err(error) if endpoint_is_stale(&error, endpoint.process_id) => {
                log::debug!(
                    "Ignoring stale GUI control endpoint for process {}: {error:#}",
                    endpoint.process_id
                );
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "query quick-access state from GUI process {}",
                        endpoint.process_id
                    )
                });
            }
        }
    }
    if targets.len() > 1 {
        anyhow::bail!(
            "multiple running quick-access terminals were discovered; close duplicates before toggling"
        );
    }
    let Some((endpoint, window_id, desired_visible)) = targets.pop() else {
        if incomplete_inventory {
            anyhow::bail!(
                "GUI inventory was truncated, so quick-access absence cannot be proven safely"
            );
        }
        return Ok(None);
    };
    let request_id = generate_request_id()?;
    let toggle = Request::SetQuickAccessVisibility {
        request_id: request_id.clone(),
        window_id,
        visible: desired_visible,
    };
    // The request ID makes the retry a replay, never a second toggle.
    let response = match request(&endpoint, &toggle) {
        Ok(response) => response,
        Err(first_error) => request(&endpoint, &toggle).with_context(|| {
            format!(
                "quick-access request {request_id} failed twice; first failure: {first_error:#}"
            )
        })?,
    };
    match response {
        Response::Error { message } => anyhow::bail!(
            "{message} (request {request_id}; retry the same visibility action with a fresh invocation)"
        ),
        response => {
            let response = answer(endpoint.process_id, &toggle, response)?;
            Ok(Some(describe(&toggle, &response)))
        }
    }
}

fn endpoint_is_stale(error: &anyhow::Error, process_id: u32) -> bool {
    error.downcast_ref::<std::io::Error>().is_some_and(|io| {
        matches!(
            io.kind(),
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
        )
    }) || !process_is_alive(process_id)
}

fn request_inventory(endpoint: &ControlEndpoint) -> anyhow::Result<ProcessInventory> {
    match request(endpoint, &Request::Inventory)? {
        Response::Inventory(inventory) => Ok(inventory),
        Response::Error { message } => anyhow::bail!(message),
        _ => anyhow::bail!("invalid GUI control inventory response"),
    }
}

/// `--json` output: the reply as the GUI sent it, beside the schema version.
#[derive(Serialize)]
struct JsonOutput<'a> {
    schema_version: u32,
    #[serde(flatten)]
    response: &'a Response,
}

fn json_output(response: &Response) -> anyhow::Result<String> {
    Ok(serde_json::to_string_pretty(&JsonOutput {
        schema_version: protocol::JSON_SCHEMA_VERSION,
        response,
    })?)
}

/// Send one `termeric control` request to GUI process `process_id` and print
/// its reply. An action gets `request_id`, or a fresh one, so a retry with the
/// same ID replays it instead of acting twice.
pub fn send(
    process_id: u32,
    mut request: Request,
    request_id: Option<String>,
    json: bool,
) -> anyhow::Result<String> {
    if let Some(slot) = request.request_id_mut() {
        *slot = match request_id {
            Some(request_id) if protocol::valid_request_id(&request_id) => request_id,
            Some(_) => anyhow::bail!("--request-id must be exactly 32 lowercase hex digits"),
            None => generate_request_id()?,
        };
    }
    request.canonicalize();
    let Some(id) = request.request_id().map(str::to_owned) else {
        return print(&request, &ask(process_id, &request)?, json);
    };
    // Typed text must never be typed twice, so a failed send is final; any
    // other action replays under its ID.
    let input = matches!(request, Request::SendText { .. });
    let response = reply(process_id, &request).with_context(|| {
        if input {
            format!(
                "request {id} did not complete; retry only with `--request-id {id}` so input cannot be duplicated"
            )
        } else {
            format!(
                "request {id} did not complete; retry with `--request-id {id}` to replay the same logical action"
            )
        }
    })?;
    match response {
        Response::Error { message } if input => anyhow::bail!(
            "{message} (request {id} is terminal; use a new request ID after correcting the failure)"
        ),
        Response::Error { message } => {
            anyhow::bail!("{message} (request {id}; retry the same logical action with this ID)")
        }
        response => print(&request, &answer(process_id, &request, response)?, json),
    }
}

/// `response` as `--json` output or as text for a person.
pub(super) fn print(request: &Request, response: &Response, json: bool) -> anyhow::Result<String> {
    if json {
        json_output(response)
    } else {
        Ok(describe(request, response))
    }
}

/// Send `request` to GUI process `process_id`; a refusal is an error, and
/// the reply must answer this request.
fn ask(process_id: u32, request: &Request) -> anyhow::Result<Response> {
    answer(process_id, request, reply(process_id, request)?)
}

/// The GUI's validated reply to `request`, which may be a refusal.
fn reply(process_id: u32, request: &Request) -> anyhow::Result<Response> {
    if process_id == 0 || process_id > i32::MAX as u32 {
        anyhow::bail!("process IDs must be nonzero runtime IDs");
    }
    request.validate().map_err(anyhow::Error::msg)?;
    request_within(&endpoint_for(process_id)?, request, IO_TIMEOUT)
}

/// `response` from process `process_id` as the answer to `request`: a
/// refusal becomes an error, and a reply to anything else is refused.
pub(super) fn answer(
    process_id: u32,
    request: &Request,
    response: Response,
) -> anyhow::Result<Response> {
    if let Response::Error { message } = response {
        anyhow::bail!(message);
    }
    if !answers(request, process_id, &response) {
        anyhow::bail!("the GUI's reply does not answer this request");
    }
    Ok(response)
}

/// Whether `response` from process `process_id` is the reply to `request`:
/// the same process, replay identity, and targets, and results consistent
/// with what was asked.
fn answers(request: &Request, process_id: u32, response: &Response) -> bool {
    if response.process_id() != Some(process_id) || response.request_id() != request.request_id() {
        return false;
    }
    match (request, response) {
        (Request::Inventory, Response::Inventory(_))
        | (Request::ListAgents { .. }, Response::Agents { .. })
        | (Request::AgentEvent { .. }, Response::AgentEventRecorded { .. })
        | (Request::CreateWindow { .. }, Response::WindowCreated { .. })
        | (Request::JoinLaunch { .. }, Response::WindowCreated { .. })
        | (Request::LaunchWindow { .. }, Response::WindowLaunched { .. }) => true,
        (
            Request::LaunchStatus { launch_request_id },
            Response::LaunchStatus {
                launch_request_id: returned,
                ..
            },
        ) => returned == launch_request_id,
        (
            Request::ReadText {
                window_id,
                tab_id,
                pane_id,
                history_lines,
                max_bytes,
            },
            Response::Text {
                window_id: returned_window,
                tab_id: returned_tab,
                pane_id: returned_pane,
                history_lines: returned_history,
                text,
                ..
            },
        ) => {
            (returned_window, returned_tab, returned_pane) == (window_id, tab_id, pane_id)
                && returned_history <= history_lines
                && text.len() <= *max_bytes as usize
        }
        (
            Request::SendText {
                window_id,
                tab_id,
                pane_id,
                text,
                paste,
                enter,
                ..
            },
            Response::InputAccepted {
                window_id: returned_window,
                tab_id: returned_tab,
                pane_id: returned_pane,
                input_bytes,
                wire_bytes,
                paste: returned_paste,
                enter: returned_enter,
                ..
            },
        ) => {
            (returned_window, returned_tab, returned_pane) == (window_id, tab_id, pane_id)
                && *input_bytes as usize == text.len() + usize::from(*enter)
                && (returned_paste, returned_enter) == (paste, enter)
                // A bracketed paste adds its 12 delimiter bytes.
                && (wire_bytes == input_bytes || (*paste && *wire_bytes == input_bytes + 12))
        }
        (
            Request::CreateTab { window_id, .. } | Request::LaunchTab { window_id, .. },
            Response::TabCreated {
                window_id: returned_window,
                ..
            },
        )
        | (
            Request::FocusWindow { window_id, .. },
            Response::WindowFocusRequested {
                window_id: returned_window,
                ..
            },
        )
        | (
            Request::CloseWindow { window_id, .. },
            Response::WindowCloseRequested {
                window_id: returned_window,
                ..
            },
        ) => returned_window == window_id,
        (
            Request::SetQuickAccessVisibility {
                window_id, visible, ..
            },
            Response::QuickAccessChanged {
                window_id: returned_window,
                visible: returned_visible,
                ..
            },
        ) => (returned_window, returned_visible) == (window_id, visible),
        (
            Request::SplitPane {
                window_id, tab_id, ..
            },
            Response::PaneCreated {
                window_id: returned_window,
                tab_id: returned_tab,
                ..
            },
        )
        | (
            Request::SelectTab {
                window_id, tab_id, ..
            },
            Response::TabSelected {
                window_id: returned_window,
                tab_id: returned_tab,
                ..
            },
        )
        | (
            Request::CloseTab {
                window_id, tab_id, ..
            },
            Response::TabCloseRequested {
                window_id: returned_window,
                tab_id: returned_tab,
                ..
            },
        ) => (returned_window, returned_tab) == (window_id, tab_id),
        (
            Request::FocusPane {
                window_id,
                tab_id,
                pane_id,
                ..
            },
            Response::PaneFocused {
                window_id: returned_window,
                tab_id: returned_tab,
                pane_id: returned_pane,
                ..
            },
        )
        | (
            Request::ClosePane {
                window_id,
                tab_id,
                pane_id,
                ..
            },
            Response::PaneClosed {
                window_id: returned_window,
                tab_id: returned_tab,
                pane_id: returned_pane,
                ..
            },
        )
        | (
            Request::SubmitToAgent {
                window_id,
                tab_id,
                pane_id,
                ..
            },
            Response::AgentInputAccepted {
                window_id: returned_window,
                tab_id: returned_tab,
                pane_id: returned_pane,
                ..
            },
        ) => (returned_window, returned_tab, returned_pane) == (window_id, tab_id, pane_id),
        (
            Request::MoveTab {
                source_window_id,
                tab_id,
                destination_window_id,
                destination_index,
                ..
            },
            Response::TabMoved {
                source_window_id: returned_source,
                source_tab_id,
                destination_window_id: returned_destination,
                destination_index: returned_index,
                ..
            },
        ) => {
            (returned_source, source_tab_id) == (source_window_id, tab_id)
                && (returned_destination, returned_index)
                    == (destination_window_id, destination_index)
        }
        (
            Request::MovePane {
                source_window_id,
                source_tab_id,
                pane_id,
                destination_window_id,
                destination_tab_id,
                target_pane_id,
                ..
            },
            Response::PaneMoved {
                source_window_id: returned_source_window,
                source_tab_id: returned_source_tab,
                source_pane_id,
                destination_window_id: returned_destination_window,
                destination_tab_id: returned_destination_tab,
                target_pane_id: returned_target,
                ..
            },
        ) => {
            (returned_source_window, returned_source_tab, source_pane_id)
                == (source_window_id, source_tab_id, pane_id)
                && (
                    returned_destination_window,
                    returned_destination_tab,
                    returned_target,
                ) == (destination_window_id, destination_tab_id, target_pane_id)
        }
        (
            Request::ResizeWindow {
                window_id,
                width,
                height,
                ..
            },
            Response::WindowResizeRequested {
                window_id: returned_window,
                requested_width,
                requested_height,
                ..
            },
        ) => (returned_window, requested_width, requested_height) == (window_id, width, height),
        (
            Request::ResizeDivider {
                window_id,
                tab_id,
                divider_id,
                ratio_millis,
                ..
            },
            Response::DividerResized {
                window_id: returned_window,
                tab_id: returned_tab,
                divider_id: returned_divider,
                ratio_millis: returned_ratio,
                ..
            },
        ) => {
            (
                returned_window,
                returned_tab,
                returned_divider,
                returned_ratio,
            ) == (window_id, tab_id, divider_id, ratio_millis)
        }
        (
            Request::ReadConversation {
                window_id,
                session_id,
            },
            Response::Conversation {
                window_id: returned_window,
                session_id: returned_session,
                ..
            },
        )
        | (
            Request::AnswerApproval {
                window_id,
                session_id,
                ..
            },
            Response::ApprovalAnswered {
                window_id: returned_window,
                session_id: returned_session,
                ..
            },
        )
        | (
            Request::SubmitToSession {
                window_id,
                session_id,
                ..
            },
            Response::SessionInputAccepted {
                window_id: returned_window,
                session_id: returned_session,
                ..
            },
        ) => (returned_window, returned_session) == (window_id, session_id),
        _ => false,
    }
}

/// One line (or the terminal text) a person reads for `response`.
fn describe(request: &Request, response: &Response) -> String {
    match response {
        Response::Text {
            text, truncated, ..
        } => {
            if *truncated {
                format!("[older terminal text truncated]\n{text}")
            } else {
                text.clone()
            }
        }
        Response::LaunchStatus {
            process_id,
            launch_request_id,
            state,
            terminal_closed,
        } => {
            let result = match state {
                protocol::LaunchState::Running {} => "running".to_string(),
                protocol::LaunchState::Exited { exit_code, success } => {
                    format!("exited with portable code {exit_code} (success: {success})")
                }
            };
            format!(
                "Launch {launch_request_id} in process {process_id}: {result}; terminal closed: {terminal_closed}."
            )
        }
        Response::InputAccepted {
            process_id,
            request_id,
            window_id,
            tab_id,
            pane_id,
            input_bytes,
            wire_bytes,
            confirmation_id,
            ..
        } => match confirmation_id {
            Some(confirmation_id) => format!(
                "Input for pane {pane_id}, tab {tab_id}, window {window_id}, process {process_id} awaits visible destructive-command confirmation {confirmation_id} (request {request_id})."
            ),
            None => format!(
                "Sent {input_bytes} UTF-8 input bytes to pane {pane_id}, tab {tab_id}, window {window_id}, process {process_id} ({wire_bytes} PTY bytes; request {request_id})."
            ),
        },
        Response::WindowCreated {
            process_id,
            request_id,
            window_id,
        } => format!("Created window {window_id} in process {process_id} (request {request_id})."),
        Response::WindowLaunched {
            process_id,
            request_id,
            window_id,
            tab_id,
        } => format!(
            "Launched tab {tab_id} in new window {window_id} of process {process_id} (request {request_id})."
        ),
        Response::TabCreated {
            process_id,
            request_id,
            window_id,
            tab_id,
        } => {
            let verb = if matches!(request, Request::LaunchTab { .. }) {
                "Launched"
            } else {
                "Created"
            };
            format!(
                "{verb} tab {tab_id} in window {window_id} of process {process_id} (request {request_id})."
            )
        }
        Response::PaneCreated {
            process_id,
            request_id,
            window_id,
            tab_id,
            pane_id,
        } => format!(
            "Created pane {pane_id} in tab {tab_id}, window {window_id}, process {process_id} (request {request_id})."
        ),
        Response::WindowFocusRequested {
            process_id,
            request_id,
            window_id,
            already_focused,
        } => {
            if *already_focused {
                format!(
                    "Window {window_id} in process {process_id} was already focused (request {request_id})."
                )
            } else {
                format!(
                    "Requested focus for window {window_id} in process {process_id} (request {request_id})."
                )
            }
        }
        Response::QuickAccessChanged {
            process_id,
            request_id,
            window_id,
            visible,
        } => format!(
            "Quick-access terminal in process {process_id}, window {window_id} is now {} (request {request_id}).",
            if *visible { "visible" } else { "hidden" }
        ),
        Response::TabSelected {
            process_id,
            request_id,
            window_id,
            tab_id,
            changed,
        } => format!(
            "{} tab {tab_id} in window {window_id}, process {process_id} (request {request_id}).",
            if *changed {
                "Selected"
            } else {
                "Already selected"
            }
        ),
        Response::PaneFocused {
            process_id,
            request_id,
            window_id,
            tab_id,
            pane_id,
            tab_changed,
            pane_changed,
        } => format!(
            "Focused pane {pane_id} in tab {tab_id}, window {window_id}, process {process_id} (tab changed: {tab_changed}; pane changed: {pane_changed}; request {request_id})."
        ),
        Response::WindowCloseRequested {
            process_id,
            request_id,
            window_id,
        } => format!(
            "Scheduled window {window_id} in process {process_id} to close (request {request_id})."
        ),
        Response::TabCloseRequested {
            process_id,
            request_id,
            window_id,
            tab_id,
        } => format!(
            "Scheduled tab {tab_id} in window {window_id}, process {process_id} to close (request {request_id})."
        ),
        Response::PaneClosed {
            process_id,
            request_id,
            window_id,
            tab_id,
            pane_id,
            remaining_panes,
            focused_pane_id,
        } => format!(
            "Closed pane {pane_id} in tab {tab_id}, window {window_id}, process {process_id}; {remaining_panes} pane(s) remain and pane {focused_pane_id} is focused (request {request_id})."
        ),
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
            if *changed {
                format!(
                    "Moved tab {source_tab_id} from window {source_window_id} to tab {destination_tab_id} at index {destination_index} in window {destination_window_id}, process {process_id}; source window closing: {source_window_closing} (request {request_id})."
                )
            } else {
                format!(
                    "Tab {source_tab_id} was already at index {destination_index} in window {source_window_id}, process {process_id} (request {request_id})."
                )
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
        } => format!(
            "Moved pane {source_pane_id} from tab {source_tab_id}, window {source_window_id} to pane {destination_pane_id} beside pane {target_pane_id} in tab {destination_tab_id}, window {destination_window_id}, process {process_id} (request {request_id})."
        ),
        Response::WindowResizeRequested {
            process_id,
            request_id,
            window_id,
            requested_width,
            requested_height,
            immediate_width,
            immediate_height,
        } => match (immediate_width, immediate_height) {
            (Some(applied_width), Some(applied_height)) => format!(
                "Requested {requested_width}x{requested_height} physical pixels for window {window_id}, process {process_id}; the platform immediately reported {applied_width}x{applied_height} (request {request_id})."
            ),
            _ => format!(
                "Requested {requested_width}x{requested_height} physical pixels for window {window_id}, process {process_id}; await the native resize event for the applied size (request {request_id})."
            ),
        },
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
            if *changed {
                format!(
                    "Resized divider {divider_id} in tab {tab_id}, window {window_id}, process {process_id} from {previous_ratio_millis} to {ratio_millis} thousandths (request {request_id})."
                )
            } else {
                format!(
                    "Divider {divider_id} in tab {tab_id}, window {window_id}, process {process_id} was already at {ratio_millis} thousandths (request {request_id})."
                )
            }
        }
        Response::Inventory(inventory) => format_human(std::slice::from_ref(inventory)),
        // Agent replies are read by the MCP server, not printed here.
        Response::AgentEventRecorded { .. }
        | Response::Agents { .. }
        | Response::Conversation { .. }
        | Response::ApprovalAnswered { .. }
        | Response::SessionInputAccepted { .. }
        | Response::AgentInputAccepted { .. }
        | Response::Error { .. } => format!("{response:?}"),
    }
}

/// One launch's state, as `control launch-status` prints it.
pub fn launch_status(
    process_id: u32,
    launch_request_id: String,
    json: bool,
) -> anyhow::Result<String> {
    send(
        process_id,
        Request::LaunchStatus { launch_request_id },
        None,
        json,
    )
}

/// Every running GUI process, by process ID.
pub fn running_processes() -> anyhow::Result<Vec<u32>> {
    Ok(ControlEndpoint::discover_all()?
        .into_iter()
        .map(|endpoint| endpoint.process_id)
        .collect())
}

fn endpoint_for(process_id: u32) -> anyhow::Result<ControlEndpoint> {
    ControlEndpoint::discover_all()?
        .into_iter()
        .find(|endpoint| endpoint.process_id == process_id)
        .ok_or_else(|| anyhow::anyhow!("running GUI process {process_id} was not found"))
}

/// Every terminal running an agent in GUI process `process_id`, and whether
/// the list was cut short.
pub fn list_agents(
    process_id: u32,
    supervisor: Option<&str>,
) -> anyhow::Result<(Vec<protocol::AgentSummary>, bool)> {
    let listing = Request::ListAgents {
        supervisor: supervisor.map(str::to_owned),
    };
    match ask(process_id, &listing)? {
        Response::Agents {
            agents, truncated, ..
        } => Ok((agents, truncated)),
        _ => unreachable!("ask returns only a reply that answers the request"),
    }
}

/// Paste `text` into the agent in one pane and submit it, as the supervisor
/// when `supervisor` carries its token. Returns whether it waits for the
/// person's approval.
pub fn submit_to_agent(
    process_id: u32,
    window_id: u64,
    tab_id: u64,
    pane_id: u32,
    agent_id: &str,
    text: &str,
    supervisor: Option<&str>,
) -> anyhow::Result<bool> {
    let submission = Request::SubmitToAgent {
        request_id: generate_request_id()?,
        window_id,
        tab_id,
        pane_id,
        agent_id: agent_id.into(),
        text: text.into(),
        supervisor: supervisor.map(str::to_owned),
    };
    match ask(process_id, &submission)? {
        Response::AgentInputAccepted {
            pending_approval, ..
        } => Ok(pending_approval),
        _ => unreachable!("ask returns only a reply that answers the request"),
    }
}

/// One agent view session's recent conversation, and whether older parts
/// were left out.
pub fn read_conversation(
    process_id: u32,
    window_id: u64,
    session_id: u64,
) -> anyhow::Result<(String, bool)> {
    let reading = Request::ReadConversation {
        window_id,
        session_id,
    };
    match ask(process_id, &reading)? {
        Response::Conversation {
            text, truncated, ..
        } => Ok((text, truncated)),
        _ => unreachable!("ask returns only a reply that answers the request"),
    }
}

/// Answer one pending approval in an agent view session. Returns whether it
/// was answered, rather than kept as a suggestion for the person.
pub fn answer_approval(
    process_id: u32,
    window_id: u64,
    session_id: u64,
    approval_id: &str,
    allow: bool,
    reason: &str,
    supervisor: Option<&str>,
) -> anyhow::Result<bool> {
    let answer = Request::AnswerApproval {
        request_id: generate_request_id()?,
        window_id,
        session_id,
        approval_id: approval_id.into(),
        allow,
        reason: reason.into(),
        supervisor: supervisor.map(str::to_owned),
    };
    match ask(process_id, &answer)? {
        Response::ApprovalAnswered { answered, .. } => Ok(answered),
        _ => unreachable!("ask returns only a reply that answers the request"),
    }
}

/// Send a message to an agent view session. Returns whether it waits for the
/// person's approval.
pub fn submit_to_session(
    process_id: u32,
    window_id: u64,
    session_id: u64,
    text: &str,
    supervisor: Option<&str>,
) -> anyhow::Result<bool> {
    let submission = Request::SubmitToSession {
        request_id: generate_request_id()?,
        window_id,
        session_id,
        text: text.into(),
        supervisor: supervisor.map(str::to_owned),
    };
    match ask(process_id, &submission)? {
        Response::SessionInputAccepted {
            pending_approval, ..
        } => Ok(pending_approval),
        _ => unreachable!("ask returns only a reply that answers the request"),
    }
}

/// How long an agent hook waits for the GUI. A hook runs inside the agent's
/// turn, so a busy or stuck GUI must not hold the agent up for long.
const AGENT_EVENT_TIMEOUT: Duration = Duration::from_secs(2);

/// Deliver one agent hook report to the GUI process that owns its terminal.
pub fn report_agent_event(process_id: u32, report: Request) -> anyhow::Result<()> {
    report.validate().map_err(anyhow::Error::msg)?;
    let endpoint =
        ControlEndpoint::from_directory(crate::local_socket::runtime_directory(), process_id)?;
    let response = request_within(&endpoint, &report, AGENT_EVENT_TIMEOUT)?;
    answer(process_id, &report, response).map(drop)
}

/// Asks a running Termeric on `display` to open a window of `tabs`, so the
/// new window shares its process. Processes are asked in turn; one that
/// refuses (another display, build, or environment, too many windows, an
/// older protocol) passes to the next. Returns the process and window that opened it, or `None`.
pub fn join_running(
    display: &str,
    build: &str,
    environment: &str,
    activation_token: Option<String>,
    tabs: Vec<protocol::JoinTab>,
) -> anyhow::Result<Option<(u32, u64)>> {
    let mut endpoints = ControlEndpoint::discover_all()?;
    endpoints.sort_by_key(|endpoint| endpoint.process_id);
    for endpoint in endpoints {
        if !process_is_alive(endpoint.process_id) {
            continue;
        }
        let join = Request::JoinLaunch {
            request_id: generate_request_id()?,
            display: display.to_owned(),
            activation_token: activation_token.clone(),
            build: build.to_owned(),
            environment: environment.to_owned(),
            tabs: tabs.clone(),
        };
        let outcome = request(&endpoint, &join)
            .and_then(|response| answer(endpoint.process_id, &join, response));
        match outcome {
            Ok(Response::WindowCreated { window_id, .. }) => {
                return Ok(Some((endpoint.process_id, window_id)));
            }
            Ok(response) => unreachable!("a joining launch is answered by a window: {response:?}"),
            Err(error) => log::debug!(
                "Termeric {} did not open the window: {error:#}",
                endpoint.process_id
            ),
        }
    }
    Ok(None)
}

fn request(endpoint: &ControlEndpoint, request: &Request) -> anyhow::Result<Response> {
    request_within(endpoint, request, IO_TIMEOUT)
}

/// One exchange on `endpoint`: the GUI's reply, checked to be of this
/// protocol version and consistent, though it may be a refusal.
fn request_within(
    endpoint: &ControlEndpoint,
    request: &Request,
    timeout: Duration,
) -> anyhow::Result<Response> {
    let token = endpoint.paths.read_token()?;
    let mut stream = UnixStream::connect(&endpoint.paths.socket)
        .with_context(|| format!("connect to {}", endpoint.paths.socket.display()))?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    validate_peer(&stream)?;
    protocol::write_frame(
        &mut stream,
        &ClientFrame {
            version: protocol::PROTOCOL_VERSION,
            token,
            request: request.clone(),
        },
    )?;
    accept(protocol::read_frame(&mut stream)?)
}

/// Sends `request` with `token` on a connected, validated `stream` and reads
/// the reply, all before `deadline`, so a peer trickling single frame bytes
/// cannot stretch the exchange; past it, fails with `expired`.
pub(super) fn exchange_before(
    stream: &mut UnixStream,
    token: &str,
    request: &Request,
    deadline: Instant,
    expired: &'static str,
) -> anyhow::Result<Response> {
    protocol::write_frame(
        &mut SocketWriter::new(stream, deadline, expired),
        &ClientFrame {
            version: protocol::PROTOCOL_VERSION,
            token: token.to_owned(),
            request: request.clone(),
        },
    )?;
    accept(protocol::read_frame(&mut PollReader::new(
        stream,
        Deadline::At(deadline),
        expired,
    ))?)
}

/// The reply in `frame` once it is of this protocol version and consistent;
/// a refusal stays a reply.
pub(super) fn accept(frame: ServerFrame) -> anyhow::Result<Response> {
    if frame.version != protocol::PROTOCOL_VERSION {
        anyhow::bail!(
            "GUI control protocol version {} is incompatible with client version {}",
            frame.version,
            protocol::PROTOCOL_VERSION
        );
    }
    frame.response.validate()?;
    Ok(frame.response)
}

pub(super) fn generate_request_id() -> anyhow::Result<String> {
    termeric_core::hex::random(16).context("generate GUI control request ID")
}

/// Refuses a GUI control server of another user.
pub(super) fn validate_peer(stream: &UnixStream) -> anyhow::Result<()> {
    crate::local_socket::same_user_peer(stream, "GUI control server").map(drop)
}

fn process_is_alive(process_id: u32) -> bool {
    // SAFETY: signal zero performs a liveness/permission probe and does not
    // deliver a signal. The PID came from a strictly validated filename.
    let result = unsafe { libc::kill(process_id as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn format_human(processes: &[ProcessInventory]) -> String {
    if processes.is_empty() {
        return "No running Termeric GUI processes.".into();
    }
    let mut output = String::new();
    for (process_index, process) in processes.iter().enumerate() {
        if process_index > 0 {
            output.push('\n');
        }
        output.push_str(&format!("process {}", process.process_id));
        if process.truncated {
            output.push_str(" [truncated]");
        }
        output.push('\n');
        for window in &process.windows {
            output.push_str(&format!(
                "  window {} {}x{}",
                window.id, window.pixel_width, window.pixel_height
            ));
            if window.focused {
                output.push_str(" [focused]");
            }
            if !window.visible {
                output.push_str(" [hidden]");
            }
            if window.quick_access {
                output.push_str(" [quick-access]");
            }
            if window.truncated {
                output.push_str(" [truncated]");
            }
            if let Some(workspace) = &window.workspace_id {
                output.push_str(&format!(" workspace={workspace:?}"));
            }
            output.push('\n');
            for tab in &window.tabs {
                output.push_str(&format!(
                    "    tab {} [{}] title={:?}",
                    tab.id,
                    tab_kind_name(&tab.kind),
                    tab.title
                ));
                if tab.active {
                    output.push_str(" [active]");
                }
                if tab.pinned {
                    output.push_str(" [pinned]");
                }
                if tab.activity {
                    output.push_str(" [activity]");
                }
                if tab.closed {
                    output.push_str(" [closed]");
                }
                if tab.truncated {
                    output.push_str(" [truncated]");
                }
                if let Some(domain) = &tab.domain_id {
                    output.push_str(&format!(" domain={domain:?}"));
                }
                output.push('\n');
                for pane in &tab.panes {
                    output.push_str(&format!(
                        "      pane {} {}x{}",
                        pane.id, pane.cols, pane.rows
                    ));
                    if pane.focused {
                        output.push_str(" [focused]");
                    }
                    if pane.closed {
                        output.push_str(" [closed]");
                    }
                    if let Some(title) = &pane.title {
                        output.push_str(&format!(" title={title:?}"));
                    }
                    output.push('\n');
                }
                for divider in &tab.dividers {
                    let direction = divider.direction.name();
                    output.push_str(&format!(
                        "      divider {} {direction} ratio={}",
                        divider.id, divider.ratio_millis
                    ));
                    if divider.kind == super::protocol::DividerKind::LiveDomain {
                        output.push_str(" [live-domain]");
                    }
                    output.push('\n');
                }
            }
        }
    }
    output.pop();
    output
}

fn tab_kind_name(kind: &super::protocol::TabKind) -> &'static str {
    match kind {
        super::protocol::TabKind::Shell => "shell",
        super::protocol::TabKind::Profile => "profile",
        super::protocol::TabKind::Plugin => "plugin",
        super::protocol::TabKind::LiveDomain => "live-domain",
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn launch_status_output_validates_identity_and_separates_child_from_cli_result() {
        use super::*;
        let id = "a".repeat(32);
        let query = Request::LaunchStatus {
            launch_request_id: id.clone(),
        };
        let status = Response::LaunchStatus {
            process_id: 42,
            launch_request_id: id,
            state: protocol::LaunchState::Exited {
                exit_code: 7,
                success: false,
            },
            terminal_closed: true,
        };
        let json = print(&query, &answer(42, &query, status.clone()).unwrap(), true).unwrap();
        let json: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(json["result"], "launch_status");
        assert_eq!(json["state"]["exit_code"], 7);
        assert_eq!(json["state"]["success"], false);
        assert_eq!(json["terminal_closed"], true);
        for private in ["program", "args", "environment", "cwd", "text"] {
            assert!(json.get(private).is_none());
        }
        assert!(answer(43, &query, status.clone()).is_err());
        let other = Request::LaunchStatus {
            launch_request_id: "b".repeat(32),
        };
        assert!(answer(42, &other, status.clone()).is_err());
        assert!(
            accept(ServerFrame {
                version: protocol::PROTOCOL_VERSION + 1,
                response: status.clone(),
            })
            .is_err()
        );
        assert!(
            print(&query, &status, false)
                .unwrap()
                .contains("portable code 7")
        );
    }
    use super::*;
    use crate::control::protocol::{
        DividerInventory, DividerKind, PaneInventory, TabInventory, TabKind, WindowInventory,
    };
    use crate::panes::SplitDirection;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;
    use std::thread;

    #[test]
    fn quick_access_discovery_and_toggle_cross_authenticated_socket() {
        let temp = tempfile::tempdir().unwrap();
        let endpoint =
            ControlEndpoint::from_directory(temp.path().join("runtime"), std::process::id())
                .unwrap();
        let token = endpoint.paths.load_or_create_token().unwrap();
        let listener = match UnixListener::bind(&endpoint.paths.socket) {
            Ok(listener) => listener,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!("skip quick-access socket round trip: sandbox denies AF_UNIX");
                return;
            }
            Err(error) => panic!("could not bind quick-access test socket: {error}"),
        };
        fs::set_permissions(&endpoint.paths.socket, fs::Permissions::from_mode(0o600)).unwrap();
        let expected_token = token.clone();
        let server = thread::spawn(move || {
            for request_index in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let frame = protocol::read_frame::<ClientFrame>(&mut stream).unwrap();
                assert_eq!(frame.version, protocol::PROTOCOL_VERSION);
                assert_eq!(frame.token, expected_token);
                if request_index == 0 {
                    assert_eq!(frame.request, Request::Inventory);
                    protocol::write_frame(
                        &mut stream,
                        &ServerFrame::new(Response::Inventory(ProcessInventory {
                            process_id: std::process::id(),
                            windows: vec![WindowInventory {
                                id: 9,
                                focused: true,
                                visible: true,
                                quick_access: true,
                                workspace_id: None,
                                pixel_width: 1_200,
                                pixel_height: 500,
                                tabs: Vec::new(),
                                truncated: false,
                            }],
                            truncated: false,
                        })),
                    )
                    .unwrap();
                } else {
                    let Request::SetQuickAccessVisibility {
                        request_id,
                        window_id,
                        visible,
                    } = frame.request
                    else {
                        panic!("expected quick-access toggle request")
                    };
                    assert_eq!(window_id, 9);
                    assert!(!visible);
                    protocol::write_frame(
                        &mut stream,
                        &ServerFrame::new(Response::QuickAccessChanged {
                            process_id: std::process::id(),
                            request_id,
                            window_id,
                            visible,
                        }),
                    )
                    .unwrap();
                }
            }
        });

        let output = toggle_existing_quick_access_at(vec![endpoint]).unwrap();
        assert!(
            output
                .as_deref()
                .is_some_and(|text| text.contains("now hidden"))
        );
        server.join().unwrap();
    }

    #[test]
    fn quick_access_visibility_retry_reuses_the_idempotent_request() {
        let temp = tempfile::tempdir().unwrap();
        let endpoint =
            ControlEndpoint::from_directory(temp.path().join("runtime"), std::process::id())
                .unwrap();
        let token = endpoint.paths.load_or_create_token().unwrap();
        let listener = match UnixListener::bind(&endpoint.paths.socket) {
            Ok(listener) => listener,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!("skip quick-access retry round trip: sandbox denies AF_UNIX");
                return;
            }
            Err(error) => panic!("could not bind quick-access retry test socket: {error}"),
        };
        fs::set_permissions(&endpoint.paths.socket, fs::Permissions::from_mode(0o600)).unwrap();
        let server = thread::spawn(move || {
            let mut first_visibility_request = None;
            for request_index in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                let frame = protocol::read_frame::<ClientFrame>(&mut stream).unwrap();
                assert_eq!(frame.version, protocol::PROTOCOL_VERSION);
                assert_eq!(frame.token, token);
                if request_index == 0 {
                    assert_eq!(frame.request, Request::Inventory);
                    protocol::write_frame(
                        &mut stream,
                        &ServerFrame::new(Response::Inventory(ProcessInventory {
                            process_id: std::process::id(),
                            windows: vec![WindowInventory {
                                id: 9,
                                focused: false,
                                visible: false,
                                quick_access: true,
                                workspace_id: None,
                                pixel_width: 1_200,
                                pixel_height: 500,
                                tabs: Vec::new(),
                                truncated: false,
                            }],
                            truncated: false,
                        })),
                    )
                    .unwrap();
                    continue;
                }

                let Request::SetQuickAccessVisibility {
                    request_id,
                    window_id,
                    visible,
                } = &frame.request
                else {
                    panic!("expected quick-access visibility request")
                };
                assert_eq!(*window_id, 9);
                assert!(*visible);
                if request_index == 1 {
                    first_visibility_request = Some(frame.request);
                    // Simulate an ambiguous failure after the server received
                    // the request but before the client received a response.
                    continue;
                }
                assert_eq!(first_visibility_request.as_ref(), Some(&frame.request));
                protocol::write_frame(
                    &mut stream,
                    &ServerFrame::new(Response::QuickAccessChanged {
                        process_id: std::process::id(),
                        request_id: request_id.clone(),
                        window_id: *window_id,
                        visible: *visible,
                    }),
                )
                .unwrap();
            }
        });

        let output = toggle_existing_quick_access_at(vec![endpoint]).unwrap();
        assert!(
            output
                .as_deref()
                .is_some_and(|text| text.contains("now visible"))
        );
        server.join().unwrap();
    }

    #[test]
    fn human_inventory_is_deterministic_and_marks_state() {
        let output = format_human(&[ProcessInventory {
            process_id: 42,
            windows: vec![WindowInventory {
                id: 9,
                focused: true,
                visible: true,
                quick_access: false,
                workspace_id: Some("daily".into()),
                pixel_width: 1_200,
                pixel_height: 800,
                tabs: vec![TabInventory {
                    id: 3,
                    title: "build".into(),
                    kind: TabKind::Profile,
                    active: true,
                    pinned: true,
                    activity: false,
                    closed: false,
                    domain_id: None,
                    panes: vec![
                        PaneInventory {
                            id: 0,
                            title: None,
                            focused: true,
                            closed: false,
                            cols: 120,
                            rows: 40,
                        },
                        PaneInventory {
                            id: 1,
                            title: None,
                            focused: false,
                            closed: false,
                            cols: 119,
                            rows: 40,
                        },
                    ],
                    dividers: vec![DividerInventory {
                        id: 11,
                        kind: DividerKind::Local,
                        direction: SplitDirection::Vertical,
                        ratio_millis: 500,
                    }],
                    truncated: false,
                }],
                truncated: false,
            }],
            truncated: false,
        }]);
        assert_eq!(
            output,
            "process 42\n  window 9 1200x800 [focused] workspace=\"daily\"\n    tab 3 [profile] title=\"build\" [active] [pinned]\n      pane 0 120x40 [focused]\n      pane 1 119x40\n      divider 11 vertical ratio=500"
        );
        assert_eq!(format_human(&[]), "No running Termeric GUI processes.");
    }

    #[test]
    fn empty_json_inventory_keeps_an_explicit_schema_version() {
        let output = serde_json::to_value(InventoryCollection {
            schema_version: protocol::JSON_SCHEMA_VERSION,
            processes: Vec::new(),
        })
        .unwrap();
        assert_eq!(output["schema_version"], 1);
        assert_eq!(output["processes"], serde_json::json!([]));
    }

    #[test]
    fn json_output_is_the_reply_beside_the_schema_version() {
        let request_id = generate_request_id().unwrap();
        assert!(protocol::valid_request_id(&request_id));
        let output: serde_json::Value = serde_json::from_str(
            &json_output(&Response::TabCloseRequested {
                process_id: 42,
                request_id: request_id.clone(),
                window_id: 9,
                tab_id: 3,
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            output,
            serde_json::json!({
                "schema_version": 1,
                "result": "tab_close_requested",
                "process_id": 42,
                "request_id": request_id,
                "window_id": 9,
                "tab_id": 3,
            })
        );
    }

    #[test]
    fn a_reply_answers_only_the_request_it_names() {
        let request_id = "a".repeat(32);
        let select = Request::SelectTab {
            request_id: request_id.clone(),
            window_id: 9,
            tab_id: 3,
        };
        let selected = |process_id, request_id: &str, tab_id| Response::TabSelected {
            process_id,
            request_id: request_id.into(),
            window_id: 9,
            tab_id,
            changed: true,
        };
        assert!(answers(&select, 42, &selected(42, &request_id, 3)));
        assert!(!answers(&select, 43, &selected(42, &request_id, 3)));
        assert!(!answers(&select, 42, &selected(42, &"b".repeat(32), 3)));
        assert!(!answers(&select, 42, &selected(42, &request_id, 4)));
        let closed = Response::TabCloseRequested {
            process_id: 42,
            request_id: request_id.clone(),
            window_id: 9,
            tab_id: 3,
        };
        assert!(!answers(&select, 42, &closed));
        assert!(
            answer(
                42,
                &select,
                Response::Error {
                    message: "tab not found".into()
                }
            )
            .unwrap_err()
            .to_string()
            .contains("tab not found")
        );
    }
}
