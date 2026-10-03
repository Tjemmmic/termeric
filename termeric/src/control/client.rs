//! Discovery client and safe command-line formatting for GUI inventory.

use super::paths::ControlPaths;
use super::protocol::{self, ClientFrame, ProcessInventory, Request, ServerFrame};
use anyhow::Context;
use serde::Serialize;
use std::os::unix::net::UnixStream;
use std::time::Duration;

pub(super) const IO_TIMEOUT: Duration = Duration::from_secs(12);

#[derive(Serialize)]
struct InventoryCollection {
    schema_version: u32,
    processes: Vec<ProcessInventory>,
}

pub fn list(json: bool) -> anyhow::Result<String> {
    let mut processes = Vec::new();
    for endpoint in ControlPaths::discover_all()? {
        match request_inventory(&endpoint) {
            Ok(inventory) => {
                inventory.validate()?;
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
                        endpoint.socket.display()
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
    toggle_existing_quick_access_at(ControlPaths::discover_all()?)
}

fn toggle_existing_quick_access_at(endpoints: Vec<ControlPaths>) -> anyhow::Result<Option<String>> {
    let mut targets = Vec::new();
    let mut incomplete_inventory = false;
    for endpoint in endpoints {
        match request_inventory(&endpoint) {
            Ok(inventory) => {
                inventory.validate()?;
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
    let request_frame = Request::SetQuickAccessVisibility {
        request_id: request_id.clone(),
        window_id,
        visible: desired_visible,
    };
    let frame = match request(&endpoint, request_frame.clone()) {
        Ok(frame) => frame,
        Err(first_error) => request(&endpoint, request_frame).with_context(|| {
            format!(
                "quick-access request {request_id} failed twice; first failure: {first_error:#}"
            )
        })?,
    };
    match frame {
        ServerFrame::QuickAccessChanged {
            version,
            process_id,
            request_id: returned_request,
            window_id: returned_window,
            visible: returned_visible,
        } if version == protocol::PROTOCOL_VERSION
            && process_id == endpoint.process_id
            && returned_request == request_id
            && returned_window == window_id
            && returned_visible == desired_visible =>
        {
            Ok(Some(format!(
                "Quick-access terminal in process {process_id}, window {window_id} is now {} (request {request_id}).",
                if returned_visible {
                    "visible"
                } else {
                    "hidden"
                }
            )))
        }
        ServerFrame::Error { message, .. } => anyhow::bail!(
            "{message} (request {request_id}; retry the same visibility action with a fresh invocation)"
        ),
        frame => anyhow::bail!("invalid quick-access visibility response {frame:?}"),
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

fn request_inventory(endpoint: &ControlPaths) -> anyhow::Result<ProcessInventory> {
    match request(endpoint, Request::Inventory)? {
        ServerFrame::Inventory { version, inventory } if version == protocol::PROTOCOL_VERSION => {
            Ok(inventory)
        }
        ServerFrame::Error { message, .. } => anyhow::bail!(message),
        frame => anyhow::bail!("invalid GUI control inventory response {frame:?}"),
    }
}

pub fn get_text(
    process_id: u32,
    window_id: u64,
    tab_id: u64,
    pane_id: u32,
    history_lines: u16,
    max_bytes: u32,
    json: bool,
) -> anyhow::Result<String> {
    if process_id == 0 || process_id > i32::MAX as u32 || window_id == 0 {
        anyhow::bail!("process and window IDs must be nonzero runtime IDs");
    }
    let endpoint = ControlPaths::discover_all()?
        .into_iter()
        .find(|endpoint| endpoint.process_id == process_id)
        .ok_or_else(|| anyhow::anyhow!("running GUI process {process_id} was not found"))?;
    let request_frame = Request::ReadText {
        window_id,
        tab_id,
        pane_id,
        history_lines,
        max_bytes,
    };
    request_frame.validate().map_err(anyhow::Error::msg)?;
    match request(&endpoint, request_frame)? {
        ServerFrame::Text {
            version,
            process_id: returned_process,
            window_id: returned_window,
            tab_id: returned_tab,
            pane_id: returned_pane,
            screen,
            history_lines: returned_history,
            text,
            truncated,
        } if version == protocol::PROTOCOL_VERSION
            && returned_process == process_id
            && returned_window == window_id
            && returned_tab == tab_id
            && returned_pane == pane_id
            && returned_history <= history_lines
            && text.len() <= max_bytes as usize =>
        {
            if json {
                Ok(serde_json::to_string_pretty(&TextOutput {
                    schema_version: protocol::JSON_SCHEMA_VERSION,
                    process_id,
                    window_id,
                    tab_id,
                    pane_id,
                    screen,
                    history_lines: returned_history,
                    text,
                    truncated,
                })?)
            } else if truncated {
                Ok(format!("[older terminal text truncated]\n{text}"))
            } else {
                Ok(text)
            }
        }
        ServerFrame::Error { message, .. } => anyhow::bail!(message),
        frame => anyhow::bail!("invalid GUI control terminal-text response {frame:?}"),
    }
}

pub fn launch_status(
    process_id: u32,
    launch_request_id: String,
    json: bool,
) -> anyhow::Result<String> {
    let query = Request::LaunchStatus {
        launch_request_id: launch_request_id.clone(),
    };
    query.validate().map_err(anyhow::Error::msg)?;
    let endpoint = ControlPaths::discover_all()?
        .into_iter()
        .find(|endpoint| endpoint.process_id == process_id)
        .ok_or_else(|| anyhow::anyhow!("GUI process {process_id} was not found"))?;
    format_launch_status(
        request(&endpoint, query)?,
        process_id,
        &launch_request_id,
        json,
    )
}

pub(super) fn format_launch_status(
    frame: ServerFrame,
    process_id: u32,
    launch_request_id: &str,
    json: bool,
) -> anyhow::Result<String> {
    match frame {
        ServerFrame::LaunchStatus {
            version,
            process_id: returned_process,
            launch_request_id: returned_id,
            state,
            terminal_closed,
        } if version == protocol::PROTOCOL_VERSION
            && process_id == returned_process
            && launch_request_id == returned_id =>
        {
            state.validate().map_err(anyhow::Error::msg)?;
            if json {
                Ok(serde_json::to_string_pretty(&serde_json::json!({
                    "schema_version": protocol::JSON_SCHEMA_VERSION,
                    "process_id": process_id,
                    "launch_request_id": launch_request_id,
                    "state": state,
                    "terminal_closed": terminal_closed,
                }))?)
            } else {
                let result = match state {
                    protocol::LaunchState::Running {} => "running".to_string(),
                    protocol::LaunchState::Exited { exit_code, success } => {
                        format!("exited with portable code {exit_code} (success: {success})")
                    }
                };
                Ok(format!(
                    "Launch {launch_request_id} in process {process_id}: {result}; terminal closed: {terminal_closed}."
                ))
            }
        }
        ServerFrame::Error { version, message } if version == protocol::PROTOCOL_VERSION => {
            anyhow::bail!(message)
        }
        _ => anyhow::bail!("Invalid GUI launch-status response"),
    }
}

pub fn send_text(
    process_id: u32,
    window_id: u64,
    tab_id: u64,
    pane_id: u32,
    text: String,
    paste: bool,
    enter: bool,
    request_id: Option<String>,
    json: bool,
) -> anyhow::Result<String> {
    let (endpoint, request_id) = mutation_target(process_id, window_id, request_id)?;
    let expected_input_bytes = text.len().saturating_add(usize::from(enter));
    let request_frame = Request::SendText {
        request_id: request_id.clone(),
        window_id,
        tab_id,
        pane_id,
        text,
        paste,
        enter,
    };
    request_frame.validate().map_err(anyhow::Error::msg)?;
    let frame = request(&endpoint, request_frame).with_context(|| {
        format!(
            "send-text request {request_id} did not complete; retry only with `--request-id {request_id}` so input cannot be duplicated"
        )
    })?;
    match frame {
        ServerFrame::InputAccepted {
            version,
            process_id: returned_process,
            request_id: returned_request,
            window_id: returned_window,
            tab_id: returned_tab,
            pane_id: returned_pane,
            input_bytes,
            wire_bytes,
            paste: returned_paste,
            enter: returned_enter,
            disposition,
            confirmation_id,
        } if version == protocol::PROTOCOL_VERSION
            && returned_process == process_id
            && returned_request == request_id
            && returned_window == window_id
            && returned_tab == tab_id
            && returned_pane == pane_id
            && input_bytes as usize == expected_input_bytes
            && returned_paste == paste
            && returned_enter == enter
            && (wire_bytes == input_bytes || (paste && wire_bytes == input_bytes + 12))
            && matches!(
                (disposition, confirmation_id),
                (protocol::InputDisposition::Sent, None)
                    | (protocol::InputDisposition::PendingConfirmation, Some(_))
            ) =>
        {
            if json {
                Ok(serde_json::to_string_pretty(&InputOutput {
                    schema_version: protocol::JSON_SCHEMA_VERSION,
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
                })?)
            } else if let Some(confirmation_id) = confirmation_id {
                Ok(format!(
                    "Input for pane {pane_id}, tab {tab_id}, window {window_id}, process {process_id} awaits visible destructive-command confirmation {confirmation_id} (request {request_id})."
                ))
            } else {
                Ok(format!(
                    "Sent {input_bytes} UTF-8 input bytes to pane {pane_id}, tab {tab_id}, window {window_id}, process {process_id} ({wire_bytes} PTY bytes; request {request_id})."
                ))
            }
        }
        ServerFrame::Error { message, .. } => anyhow::bail!(
            "{message} (request {request_id} is terminal; use a new request ID after correcting the failure)"
        ),
        frame => anyhow::bail!("invalid GUI control terminal-input response {frame:?}"),
    }
}

pub fn new_window(
    process_id: u32,
    source_window: u64,
    request_id: Option<String>,
    json: bool,
) -> anyhow::Result<String> {
    if process_id == 0 || process_id > i32::MAX as u32 || source_window == 0 {
        anyhow::bail!("process and source-window IDs must be nonzero runtime IDs");
    }
    let request_id = match request_id {
        Some(request_id) if protocol::valid_request_id(&request_id) => request_id,
        Some(_) => anyhow::bail!("--request-id must be exactly 32 lowercase hex digits"),
        None => generate_request_id()?,
    };
    let endpoint = ControlPaths::discover_all()?
        .into_iter()
        .find(|endpoint| endpoint.process_id == process_id)
        .ok_or_else(|| anyhow::anyhow!("running GUI process {process_id} was not found"))?;
    let frame = request(
        &endpoint,
        Request::CreateWindow {
            request_id: request_id.clone(),
            source_window,
        },
    )
    .with_context(|| {
        format!(
            "new-window request {request_id} did not complete; retry with `--request-id {request_id}` to avoid duplicate creation"
        )
    })?;
    match frame {
        ServerFrame::WindowCreated {
            version,
            process_id: returned_process,
            request_id: returned_request,
            window_id,
        } if version == protocol::PROTOCOL_VERSION
            && returned_process == process_id
            && returned_request == request_id
            && window_id != 0 =>
        {
            if json {
                Ok(serde_json::to_string_pretty(&WindowCreatedOutput {
                    schema_version: protocol::JSON_SCHEMA_VERSION,
                    process_id,
                    request_id,
                    window_id,
                })?)
            } else {
                Ok(format!(
                    "Created window {window_id} in process {process_id} (request {request_id})."
                ))
            }
        }
        ServerFrame::Error { message, .. } => anyhow::bail!(
            "{message} (request {request_id}; retry the same logical action with this ID)"
        ),
        frame => anyhow::bail!("invalid GUI control window-creation response {frame:?}"),
    }
}

pub fn launch_window(
    process_id: u32,
    source_window: u64,
    program: String,
    args: Vec<String>,
    working_directory: Option<String>,
    title: Option<String>,
    environment: Vec<protocol::EnvironmentVariable>,
    environment_policy: termeric_core::pty::EnvironmentPolicy,
    request_id: Option<String>,
    json: bool,
) -> anyhow::Result<String> {
    if process_id == 0 || process_id > i32::MAX as u32 || source_window == 0 {
        anyhow::bail!("process and source-window IDs must be nonzero runtime IDs");
    }
    let request_id = match request_id {
        Some(request_id) if protocol::valid_request_id(&request_id) => request_id,
        Some(_) => anyhow::bail!("--request-id must be exactly 32 lowercase hex digits"),
        None => generate_request_id()?,
    };
    let mut request_frame = Request::LaunchWindow {
        request_id: request_id.clone(),
        source_window,
        program,
        args,
        working_directory,
        title,
        environment,
        environment_policy,
    };
    request_frame.canonicalize();
    request_frame.validate().map_err(anyhow::Error::msg)?;
    let endpoint = ControlPaths::discover_all()?
        .into_iter()
        .find(|endpoint| endpoint.process_id == process_id)
        .ok_or_else(|| anyhow::anyhow!("running GUI process {process_id} was not found"))?;
    let frame = request(&endpoint, request_frame).with_context(|| {
        format!(
            "launch-window request {request_id} did not complete; retry with `--request-id {request_id}` to avoid duplicate creation"
        )
    })?;
    match frame {
        ServerFrame::WindowLaunched {
            version,
            process_id: returned_process,
            request_id: returned_request,
            window_id,
            tab_id,
        } if version == protocol::PROTOCOL_VERSION
            && returned_process == process_id
            && returned_request == request_id
            && window_id != 0 =>
        {
            if json {
                Ok(serde_json::to_string_pretty(&WindowLaunchedOutput {
                    schema_version: protocol::JSON_SCHEMA_VERSION,
                    process_id,
                    request_id,
                    window_id,
                    tab_id,
                })?)
            } else {
                Ok(format!(
                    "Launched tab {tab_id} in new window {window_id} of process {process_id} (request {request_id})."
                ))
            }
        }
        ServerFrame::Error { message, .. } => anyhow::bail!(
            "{message} (request {request_id}; retry the same logical action with this ID)"
        ),
        frame => anyhow::bail!("invalid GUI control window-launch response {frame:?}"),
    }
}

pub fn new_tab(
    process_id: u32,
    window_id: u64,
    request_id: Option<String>,
    json: bool,
) -> anyhow::Result<String> {
    if process_id == 0 || process_id > i32::MAX as u32 || window_id == 0 {
        anyhow::bail!("process and window IDs must be nonzero runtime IDs");
    }
    let request_id = match request_id {
        Some(request_id) if protocol::valid_request_id(&request_id) => request_id,
        Some(_) => anyhow::bail!("--request-id must be exactly 32 lowercase hex digits"),
        None => generate_request_id()?,
    };
    let endpoint = ControlPaths::discover_all()?
        .into_iter()
        .find(|endpoint| endpoint.process_id == process_id)
        .ok_or_else(|| anyhow::anyhow!("running GUI process {process_id} was not found"))?;
    let frame = request(
        &endpoint,
        Request::CreateTab {
            request_id: request_id.clone(),
            window_id,
        },
    )
    .with_context(|| {
        format!(
            "new-tab request {request_id} did not complete; retry with `--request-id {request_id}` to avoid duplicate creation"
        )
    })?;
    match frame {
        ServerFrame::TabCreated {
            version,
            process_id: returned_process,
            request_id: returned_request,
            window_id: returned_window,
            tab_id,
        } if version == protocol::PROTOCOL_VERSION
            && returned_process == process_id
            && returned_request == request_id
            && returned_window == window_id =>
        {
            if json {
                Ok(serde_json::to_string_pretty(&TabCreatedOutput {
                    schema_version: protocol::JSON_SCHEMA_VERSION,
                    process_id,
                    request_id,
                    window_id,
                    tab_id,
                })?)
            } else {
                Ok(format!(
                    "Created tab {tab_id} in window {window_id} of process {process_id} (request {request_id})."
                ))
            }
        }
        ServerFrame::Error { message, .. } => anyhow::bail!(
            "{message} (request {request_id}; retry the same logical action with this ID)"
        ),
        frame => anyhow::bail!("invalid GUI control tab-creation response {frame:?}"),
    }
}

pub fn launch_tab(
    background: bool,
    process_id: u32,
    window_id: u64,
    program: String,
    args: Vec<String>,
    working_directory: Option<String>,
    title: Option<String>,
    environment: Vec<protocol::EnvironmentVariable>,
    environment_policy: termeric_core::pty::EnvironmentPolicy,
    request_id: Option<String>,
    json: bool,
) -> anyhow::Result<String> {
    if process_id == 0 || process_id > i32::MAX as u32 || window_id == 0 {
        anyhow::bail!("process and window IDs must be nonzero runtime IDs");
    }
    let request_id = match request_id {
        Some(request_id) if protocol::valid_request_id(&request_id) => request_id,
        Some(_) => anyhow::bail!("--request-id must be exactly 32 lowercase hex digits"),
        None => generate_request_id()?,
    };
    let mut request_frame = Request::LaunchTab {
        background,
        request_id: request_id.clone(),
        window_id,
        program,
        args,
        working_directory,
        title,
        environment,
        environment_policy,
    };
    request_frame.canonicalize();
    request_frame.validate().map_err(anyhow::Error::msg)?;
    let endpoint = ControlPaths::discover_all()?
        .into_iter()
        .find(|endpoint| endpoint.process_id == process_id)
        .ok_or_else(|| anyhow::anyhow!("running GUI process {process_id} was not found"))?;
    let frame = request(&endpoint, request_frame).with_context(|| {
        format!(
            "launch-tab request {request_id} did not complete; retry with `--request-id {request_id}` to avoid duplicate creation"
        )
    })?;
    match frame {
        ServerFrame::TabCreated {
            version,
            process_id: returned_process,
            request_id: returned_request,
            window_id: returned_window,
            tab_id,
        } if version == protocol::PROTOCOL_VERSION
            && returned_process == process_id
            && returned_request == request_id
            && returned_window == window_id =>
        {
            if json {
                Ok(serde_json::to_string_pretty(&TabCreatedOutput {
                    schema_version: protocol::JSON_SCHEMA_VERSION,
                    process_id,
                    request_id,
                    window_id,
                    tab_id,
                })?)
            } else {
                Ok(format!(
                    "Launched tab {tab_id} in window {window_id} of process {process_id} (request {request_id})."
                ))
            }
        }
        ServerFrame::Error { message, .. } => anyhow::bail!(
            "{message} (request {request_id}; retry the same logical action with this ID)"
        ),
        frame => anyhow::bail!("invalid GUI control tab-launch response {frame:?}"),
    }
}

pub fn split_pane(
    process_id: u32,
    window_id: u64,
    tab_id: u64,
    pane_id: u32,
    direction: protocol::SplitDirection,
    request_id: Option<String>,
    json: bool,
) -> anyhow::Result<String> {
    if process_id == 0 || process_id > i32::MAX as u32 || window_id == 0 {
        anyhow::bail!("process and window IDs must be nonzero runtime IDs");
    }
    let request_id = match request_id {
        Some(request_id) if protocol::valid_request_id(&request_id) => request_id,
        Some(_) => anyhow::bail!("--request-id must be exactly 32 lowercase hex digits"),
        None => generate_request_id()?,
    };
    let endpoint = ControlPaths::discover_all()?
        .into_iter()
        .find(|endpoint| endpoint.process_id == process_id)
        .ok_or_else(|| anyhow::anyhow!("running GUI process {process_id} was not found"))?;
    let frame = request(
        &endpoint,
        Request::SplitPane {
            request_id: request_id.clone(),
            window_id,
            tab_id,
            pane_id,
            direction,
        },
    )
    .with_context(|| {
        format!(
            "split-pane request {request_id} did not complete; retry with `--request-id {request_id}` to avoid duplicate creation"
        )
    })?;
    match frame {
        ServerFrame::PaneCreated {
            version,
            process_id: returned_process,
            request_id: returned_request,
            window_id: returned_window,
            tab_id: returned_tab,
            pane_id: created_pane,
        } if version == protocol::PROTOCOL_VERSION
            && returned_process == process_id
            && returned_request == request_id
            && returned_window == window_id
            && returned_tab == tab_id
            && created_pane != 0 =>
        {
            if json {
                Ok(serde_json::to_string_pretty(&PaneCreatedOutput {
                    schema_version: protocol::JSON_SCHEMA_VERSION,
                    process_id,
                    request_id,
                    window_id,
                    tab_id,
                    pane_id: created_pane,
                })?)
            } else {
                Ok(format!(
                    "Created pane {created_pane} in tab {tab_id}, window {window_id}, process {process_id} (request {request_id})."
                ))
            }
        }
        ServerFrame::Error { message, .. } => anyhow::bail!(
            "{message} (request {request_id}; retry the same logical action with this ID)"
        ),
        frame => anyhow::bail!("invalid GUI control pane-creation response {frame:?}"),
    }
}

pub fn focus_window(
    process_id: u32,
    window_id: u64,
    request_id: Option<String>,
    json: bool,
) -> anyhow::Result<String> {
    let (endpoint, request_id) = mutation_target(process_id, window_id, request_id)?;
    let frame = request(
        &endpoint,
        Request::FocusWindow {
            request_id: request_id.clone(),
            window_id,
        },
    )
    .with_context(|| ambiguous_mutation("focus-window", &request_id))?;
    match frame {
        ServerFrame::WindowFocusRequested {
            version,
            process_id: returned_process,
            request_id: returned_request,
            window_id: returned_window,
            already_focused,
        } if version == protocol::PROTOCOL_VERSION
            && returned_process == process_id
            && returned_request == request_id
            && returned_window == window_id =>
        {
            if json {
                Ok(serde_json::to_string_pretty(&WindowFocusOutput {
                    schema_version: protocol::JSON_SCHEMA_VERSION,
                    process_id,
                    request_id,
                    window_id,
                    already_focused,
                })?)
            } else if already_focused {
                Ok(format!(
                    "Window {window_id} in process {process_id} was already focused (request {request_id})."
                ))
            } else {
                Ok(format!(
                    "Requested focus for window {window_id} in process {process_id} (request {request_id})."
                ))
            }
        }
        ServerFrame::Error { message, .. } => mutation_error(message, &request_id),
        frame => anyhow::bail!("invalid GUI control window-focus response {frame:?}"),
    }
}

pub fn select_tab(
    process_id: u32,
    window_id: u64,
    tab_id: u64,
    request_id: Option<String>,
    json: bool,
) -> anyhow::Result<String> {
    let (endpoint, request_id) = mutation_target(process_id, window_id, request_id)?;
    let frame = request(
        &endpoint,
        Request::SelectTab {
            request_id: request_id.clone(),
            window_id,
            tab_id,
        },
    )
    .with_context(|| ambiguous_mutation("select-tab", &request_id))?;
    match frame {
        ServerFrame::TabSelected {
            version,
            process_id: returned_process,
            request_id: returned_request,
            window_id: returned_window,
            tab_id: returned_tab,
            changed,
        } if version == protocol::PROTOCOL_VERSION
            && returned_process == process_id
            && returned_request == request_id
            && returned_window == window_id
            && returned_tab == tab_id =>
        {
            if json {
                Ok(serde_json::to_string_pretty(&TabSelectedOutput {
                    schema_version: protocol::JSON_SCHEMA_VERSION,
                    process_id,
                    request_id,
                    window_id,
                    tab_id,
                    changed,
                })?)
            } else {
                let state = if changed {
                    "Selected"
                } else {
                    "Already selected"
                };
                Ok(format!(
                    "{state} tab {tab_id} in window {window_id}, process {process_id} (request {request_id})."
                ))
            }
        }
        ServerFrame::Error { message, .. } => mutation_error(message, &request_id),
        frame => anyhow::bail!("invalid GUI control tab-selection response {frame:?}"),
    }
}

pub fn focus_pane(
    process_id: u32,
    window_id: u64,
    tab_id: u64,
    pane_id: u32,
    request_id: Option<String>,
    json: bool,
) -> anyhow::Result<String> {
    let (endpoint, request_id) = mutation_target(process_id, window_id, request_id)?;
    let frame = request(
        &endpoint,
        Request::FocusPane {
            request_id: request_id.clone(),
            window_id,
            tab_id,
            pane_id,
        },
    )
    .with_context(|| ambiguous_mutation("focus-pane", &request_id))?;
    match frame {
        ServerFrame::PaneFocused {
            version,
            process_id: returned_process,
            request_id: returned_request,
            window_id: returned_window,
            tab_id: returned_tab,
            pane_id: returned_pane,
            tab_changed,
            pane_changed,
        } if version == protocol::PROTOCOL_VERSION
            && returned_process == process_id
            && returned_request == request_id
            && returned_window == window_id
            && returned_tab == tab_id
            && returned_pane == pane_id =>
        {
            if json {
                Ok(serde_json::to_string_pretty(&PaneFocusedOutput {
                    schema_version: protocol::JSON_SCHEMA_VERSION,
                    process_id,
                    request_id,
                    window_id,
                    tab_id,
                    pane_id,
                    tab_changed,
                    pane_changed,
                })?)
            } else {
                Ok(format!(
                    "Focused pane {pane_id} in tab {tab_id}, window {window_id}, process {process_id} (tab changed: {tab_changed}; pane changed: {pane_changed}; request {request_id})."
                ))
            }
        }
        ServerFrame::Error { message, .. } => mutation_error(message, &request_id),
        frame => anyhow::bail!("invalid GUI control pane-focus response {frame:?}"),
    }
}

pub fn close_window(
    process_id: u32,
    window_id: u64,
    request_id: Option<String>,
    json: bool,
) -> anyhow::Result<String> {
    let (endpoint, request_id) = mutation_target(process_id, window_id, request_id)?;
    let frame = request(
        &endpoint,
        Request::CloseWindow {
            request_id: request_id.clone(),
            window_id,
        },
    )
    .with_context(|| ambiguous_mutation("close-window", &request_id))?;
    match frame {
        ServerFrame::WindowCloseRequested {
            version,
            process_id: returned_process,
            request_id: returned_request,
            window_id: returned_window,
        } if version == protocol::PROTOCOL_VERSION
            && returned_process == process_id
            && returned_request == request_id
            && returned_window == window_id =>
        {
            if json {
                Ok(serde_json::to_string_pretty(&WindowCloseOutput {
                    schema_version: protocol::JSON_SCHEMA_VERSION,
                    process_id,
                    request_id,
                    window_id,
                    scheduled: true,
                })?)
            } else {
                Ok(format!(
                    "Scheduled window {window_id} in process {process_id} to close (request {request_id})."
                ))
            }
        }
        ServerFrame::Error { message, .. } => mutation_error(message, &request_id),
        frame => anyhow::bail!("invalid GUI control window-close response {frame:?}"),
    }
}

pub fn close_tab(
    process_id: u32,
    window_id: u64,
    tab_id: u64,
    request_id: Option<String>,
    json: bool,
) -> anyhow::Result<String> {
    let (endpoint, request_id) = mutation_target(process_id, window_id, request_id)?;
    let frame = request(
        &endpoint,
        Request::CloseTab {
            request_id: request_id.clone(),
            window_id,
            tab_id,
        },
    )
    .with_context(|| ambiguous_mutation("close-tab", &request_id))?;
    match frame {
        ServerFrame::TabCloseRequested {
            version,
            process_id: returned_process,
            request_id: returned_request,
            window_id: returned_window,
            tab_id: returned_tab,
        } if version == protocol::PROTOCOL_VERSION
            && returned_process == process_id
            && returned_request == request_id
            && returned_window == window_id
            && returned_tab == tab_id =>
        {
            if json {
                Ok(serde_json::to_string_pretty(&TabCloseOutput {
                    schema_version: protocol::JSON_SCHEMA_VERSION,
                    process_id,
                    request_id,
                    window_id,
                    tab_id,
                    scheduled: true,
                })?)
            } else {
                Ok(format!(
                    "Scheduled tab {tab_id} in window {window_id}, process {process_id} to close (request {request_id})."
                ))
            }
        }
        ServerFrame::Error { message, .. } => mutation_error(message, &request_id),
        frame => anyhow::bail!("invalid GUI control tab-close response {frame:?}"),
    }
}

pub fn close_pane(
    process_id: u32,
    window_id: u64,
    tab_id: u64,
    pane_id: u32,
    request_id: Option<String>,
    json: bool,
) -> anyhow::Result<String> {
    let (endpoint, request_id) = mutation_target(process_id, window_id, request_id)?;
    let frame = request(
        &endpoint,
        Request::ClosePane {
            request_id: request_id.clone(),
            window_id,
            tab_id,
            pane_id,
        },
    )
    .with_context(|| ambiguous_mutation("close-pane", &request_id))?;
    match frame {
        ServerFrame::PaneClosed {
            version,
            process_id: returned_process,
            request_id: returned_request,
            window_id: returned_window,
            tab_id: returned_tab,
            pane_id: returned_pane,
            remaining_panes,
            focused_pane_id,
        } if version == protocol::PROTOCOL_VERSION
            && returned_process == process_id
            && returned_request == request_id
            && returned_window == window_id
            && returned_tab == tab_id
            && returned_pane == pane_id
            && remaining_panes != 0
            && usize::from(remaining_panes) <= protocol::MAX_PANES_PER_TAB =>
        {
            if json {
                Ok(serde_json::to_string_pretty(&PaneCloseOutput {
                    schema_version: protocol::JSON_SCHEMA_VERSION,
                    process_id,
                    request_id,
                    window_id,
                    tab_id,
                    pane_id,
                    remaining_panes,
                    focused_pane_id,
                })?)
            } else {
                Ok(format!(
                    "Closed pane {pane_id} in tab {tab_id}, window {window_id}, process {process_id}; {remaining_panes} pane(s) remain and pane {focused_pane_id} is focused (request {request_id})."
                ))
            }
        }
        ServerFrame::Error { message, .. } => mutation_error(message, &request_id),
        frame => anyhow::bail!("invalid GUI control pane-close response {frame:?}"),
    }
}

pub fn move_tab(
    process_id: u32,
    source_window_id: u64,
    tab_id: u64,
    destination_window_id: u64,
    destination_index: u16,
    request_id: Option<String>,
    json: bool,
) -> anyhow::Result<String> {
    let (endpoint, request_id) = mutation_target(process_id, source_window_id, request_id)?;
    if destination_window_id == 0 || usize::from(destination_index) > protocol::MAX_TABS_PER_WINDOW
    {
        anyhow::bail!("destination window ID and tab index are outside control limits");
    }
    let frame = request(
        &endpoint,
        Request::MoveTab {
            request_id: request_id.clone(),
            source_window_id,
            tab_id,
            destination_window_id,
            destination_index,
        },
    )
    .with_context(|| ambiguous_mutation("move-tab", &request_id))?;
    match frame {
        ServerFrame::TabMoved {
            version,
            process_id: returned_process,
            request_id: returned_request,
            source_window_id: returned_source_window,
            source_tab_id,
            destination_window_id: returned_destination_window,
            destination_tab_id,
            destination_index: returned_destination_index,
            source_window_closing,
            changed,
        } if version == protocol::PROTOCOL_VERSION
            && returned_process == process_id
            && returned_request == request_id
            && returned_source_window == source_window_id
            && source_tab_id == tab_id
            && returned_destination_window == destination_window_id
            && returned_destination_index == destination_index =>
        {
            if json {
                Ok(serde_json::to_string_pretty(&TabMovedOutput {
                    schema_version: protocol::JSON_SCHEMA_VERSION,
                    process_id,
                    request_id,
                    source_window_id,
                    source_tab_id,
                    destination_window_id,
                    destination_tab_id,
                    destination_index,
                    source_window_closing,
                    changed,
                })?)
            } else if changed {
                Ok(format!(
                    "Moved tab {tab_id} from window {source_window_id} to tab {destination_tab_id} at index {destination_index} in window {destination_window_id}, process {process_id}; source window closing: {source_window_closing} (request {request_id})."
                ))
            } else {
                Ok(format!(
                    "Tab {tab_id} was already at index {destination_index} in window {source_window_id}, process {process_id} (request {request_id})."
                ))
            }
        }
        ServerFrame::Error { message, .. } => mutation_error(message, &request_id),
        frame => anyhow::bail!("invalid GUI control tab-move response {frame:?}"),
    }
}

pub fn move_pane(
    process_id: u32,
    source_window_id: u64,
    source_tab_id: u64,
    pane_id: u32,
    destination_window_id: u64,
    destination_tab_id: u64,
    target_pane_id: u32,
    direction: protocol::SplitDirection,
    request_id: Option<String>,
    json: bool,
) -> anyhow::Result<String> {
    let (endpoint, request_id) = mutation_target(process_id, source_window_id, request_id)?;
    if destination_window_id == 0 {
        anyhow::bail!("destination window ID must be nonzero");
    }
    let frame = request(
        &endpoint,
        Request::MovePane {
            request_id: request_id.clone(),
            source_window_id,
            source_tab_id,
            pane_id,
            destination_window_id,
            destination_tab_id,
            target_pane_id,
            direction,
        },
    )
    .with_context(|| ambiguous_mutation("move-pane", &request_id))?;
    match frame {
        ServerFrame::PaneMoved {
            version,
            process_id: returned_process,
            request_id: returned_request,
            source_window_id: returned_source_window,
            source_tab_id: returned_source_tab,
            source_pane_id,
            destination_window_id: returned_destination_window,
            destination_tab_id: returned_destination_tab,
            destination_pane_id,
            target_pane_id: returned_target_pane,
        } if version == protocol::PROTOCOL_VERSION
            && returned_process == process_id
            && returned_request == request_id
            && returned_source_window == source_window_id
            && returned_source_tab == source_tab_id
            && source_pane_id == pane_id
            && returned_destination_window == destination_window_id
            && returned_destination_tab == destination_tab_id
            && returned_target_pane == target_pane_id
            && destination_pane_id != target_pane_id =>
        {
            if json {
                Ok(serde_json::to_string_pretty(&PaneMovedOutput {
                    schema_version: protocol::JSON_SCHEMA_VERSION,
                    process_id,
                    request_id,
                    source_window_id,
                    source_tab_id,
                    source_pane_id: pane_id,
                    destination_window_id,
                    destination_tab_id,
                    destination_pane_id,
                    target_pane_id,
                })?)
            } else {
                Ok(format!(
                    "Moved pane {pane_id} from tab {source_tab_id}, window {source_window_id} to pane {destination_pane_id} beside pane {target_pane_id} in tab {destination_tab_id}, window {destination_window_id}, process {process_id} (request {request_id})."
                ))
            }
        }
        ServerFrame::Error { message, .. } => mutation_error(message, &request_id),
        frame => anyhow::bail!("invalid GUI control pane-move response {frame:?}"),
    }
}

pub fn resize_window(
    process_id: u32,
    window_id: u64,
    width: u32,
    height: u32,
    request_id: Option<String>,
    json: bool,
) -> anyhow::Result<String> {
    let (endpoint, request_id) = mutation_target(process_id, window_id, request_id)?;
    let request_frame = Request::ResizeWindow {
        request_id: request_id.clone(),
        window_id,
        width,
        height,
    };
    request_frame.validate().map_err(anyhow::Error::msg)?;
    let frame = request(&endpoint, request_frame)
        .with_context(|| ambiguous_mutation("resize-window", &request_id))?;
    match frame {
        ServerFrame::WindowResizeRequested {
            version,
            process_id: returned_process,
            request_id: returned_request,
            window_id: returned_window,
            requested_width,
            requested_height,
            immediate_width,
            immediate_height,
        } if version == protocol::PROTOCOL_VERSION
            && returned_process == process_id
            && returned_request == request_id
            && returned_window == window_id
            && requested_width == width
            && requested_height == height
            && immediate_width.is_some() == immediate_height.is_some()
            && immediate_width != Some(0)
            && immediate_height != Some(0) =>
        {
            if json {
                Ok(serde_json::to_string_pretty(&WindowResizeOutput {
                    schema_version: protocol::JSON_SCHEMA_VERSION,
                    process_id,
                    request_id,
                    window_id,
                    requested_width,
                    requested_height,
                    immediate_width,
                    immediate_height,
                })?)
            } else if let (Some(applied_width), Some(applied_height)) =
                (immediate_width, immediate_height)
            {
                Ok(format!(
                    "Requested {width}x{height} physical pixels for window {window_id}, process {process_id}; the platform immediately reported {applied_width}x{applied_height} (request {request_id})."
                ))
            } else {
                Ok(format!(
                    "Requested {width}x{height} physical pixels for window {window_id}, process {process_id}; await the native resize event for the applied size (request {request_id})."
                ))
            }
        }
        ServerFrame::Error { message, .. } => mutation_error(message, &request_id),
        frame => anyhow::bail!("invalid GUI control window-resize response {frame:?}"),
    }
}

pub fn resize_divider(
    process_id: u32,
    window_id: u64,
    tab_id: u64,
    divider_id: u64,
    ratio_millis: u16,
    request_id: Option<String>,
    json: bool,
) -> anyhow::Result<String> {
    let (endpoint, request_id) = mutation_target(process_id, window_id, request_id)?;
    let request_frame = Request::ResizeDivider {
        request_id: request_id.clone(),
        window_id,
        tab_id,
        divider_id,
        ratio_millis,
    };
    request_frame.validate().map_err(anyhow::Error::msg)?;
    let frame = request(&endpoint, request_frame)
        .with_context(|| ambiguous_mutation("resize-divider", &request_id))?;
    match frame {
        ServerFrame::DividerResized {
            version,
            process_id: returned_process,
            request_id: returned_request,
            window_id: returned_window,
            tab_id: returned_tab,
            divider_id: returned_divider,
            previous_ratio_millis,
            ratio_millis: returned_ratio,
            changed,
        } if version == protocol::PROTOCOL_VERSION
            && returned_process == process_id
            && returned_request == request_id
            && returned_window == window_id
            && returned_tab == tab_id
            && returned_divider == divider_id
            && returned_ratio == ratio_millis
            && (100..=900).contains(&previous_ratio_millis)
            && changed == (previous_ratio_millis != ratio_millis) =>
        {
            if json {
                Ok(serde_json::to_string_pretty(&DividerResizeOutput {
                    schema_version: protocol::JSON_SCHEMA_VERSION,
                    process_id,
                    request_id,
                    window_id,
                    tab_id,
                    divider_id,
                    previous_ratio_millis,
                    ratio_millis,
                    changed,
                })?)
            } else if changed {
                Ok(format!(
                    "Resized divider {divider_id} in tab {tab_id}, window {window_id}, process {process_id} from {previous_ratio_millis} to {ratio_millis} thousandths (request {request_id})."
                ))
            } else {
                Ok(format!(
                    "Divider {divider_id} in tab {tab_id}, window {window_id}, process {process_id} was already at {ratio_millis} thousandths (request {request_id})."
                ))
            }
        }
        ServerFrame::Error { message, .. } => mutation_error(message, &request_id),
        frame => anyhow::bail!("invalid GUI control divider-resize response {frame:?}"),
    }
}

fn mutation_target(
    process_id: u32,
    window_id: u64,
    request_id: Option<String>,
) -> anyhow::Result<(ControlPaths, String)> {
    if process_id == 0 || process_id > i32::MAX as u32 || window_id == 0 {
        anyhow::bail!("process and window IDs must be nonzero runtime IDs");
    }
    let request_id = match request_id {
        Some(request_id) if protocol::valid_request_id(&request_id) => request_id,
        Some(_) => anyhow::bail!("--request-id must be exactly 32 lowercase hex digits"),
        None => generate_request_id()?,
    };
    let endpoint = ControlPaths::discover_all()?
        .into_iter()
        .find(|endpoint| endpoint.process_id == process_id)
        .ok_or_else(|| anyhow::anyhow!("running GUI process {process_id} was not found"))?;
    Ok((endpoint, request_id))
}

fn ambiguous_mutation(operation: &str, request_id: &str) -> String {
    format!(
        "{operation} request {request_id} did not complete; retry with `--request-id {request_id}` to replay the same logical action"
    )
}

fn mutation_error(message: String, request_id: &str) -> anyhow::Result<String> {
    anyhow::bail!("{message} (request {request_id}; retry the same logical action with this ID)")
}

#[derive(Serialize)]
struct TextOutput {
    schema_version: u32,
    process_id: u32,
    window_id: u64,
    tab_id: u64,
    pane_id: u32,
    screen: protocol::TextScreen,
    history_lines: u16,
    text: String,
    truncated: bool,
}

#[derive(Serialize)]
struct InputOutput {
    schema_version: u32,
    process_id: u32,
    request_id: String,
    window_id: u64,
    tab_id: u64,
    pane_id: u32,
    input_bytes: u32,
    wire_bytes: u32,
    paste: bool,
    enter: bool,
    disposition: protocol::InputDisposition,
    confirmation_id: Option<u64>,
}

#[derive(Serialize)]
struct WindowCreatedOutput {
    schema_version: u32,
    process_id: u32,
    request_id: String,
    window_id: u64,
}

#[derive(Serialize)]
struct WindowLaunchedOutput {
    schema_version: u32,
    process_id: u32,
    request_id: String,
    window_id: u64,
    tab_id: u64,
}

#[derive(Serialize)]
struct TabCreatedOutput {
    schema_version: u32,
    process_id: u32,
    request_id: String,
    window_id: u64,
    tab_id: u64,
}

#[derive(Serialize)]
struct PaneCreatedOutput {
    schema_version: u32,
    process_id: u32,
    request_id: String,
    window_id: u64,
    tab_id: u64,
    pane_id: u32,
}

#[derive(Serialize)]
struct WindowFocusOutput {
    schema_version: u32,
    process_id: u32,
    request_id: String,
    window_id: u64,
    already_focused: bool,
}

#[derive(Serialize)]
struct TabSelectedOutput {
    schema_version: u32,
    process_id: u32,
    request_id: String,
    window_id: u64,
    tab_id: u64,
    changed: bool,
}

#[derive(Serialize)]
struct PaneFocusedOutput {
    schema_version: u32,
    process_id: u32,
    request_id: String,
    window_id: u64,
    tab_id: u64,
    pane_id: u32,
    tab_changed: bool,
    pane_changed: bool,
}

#[derive(Serialize)]
struct WindowCloseOutput {
    schema_version: u32,
    process_id: u32,
    request_id: String,
    window_id: u64,
    scheduled: bool,
}

#[derive(Serialize)]
struct TabCloseOutput {
    schema_version: u32,
    process_id: u32,
    request_id: String,
    window_id: u64,
    tab_id: u64,
    scheduled: bool,
}

#[derive(Serialize)]
struct PaneCloseOutput {
    schema_version: u32,
    process_id: u32,
    request_id: String,
    window_id: u64,
    tab_id: u64,
    pane_id: u32,
    remaining_panes: u16,
    focused_pane_id: u32,
}

#[derive(Serialize)]
struct TabMovedOutput {
    schema_version: u32,
    process_id: u32,
    request_id: String,
    source_window_id: u64,
    source_tab_id: u64,
    destination_window_id: u64,
    destination_tab_id: u64,
    destination_index: u16,
    source_window_closing: bool,
    changed: bool,
}

#[derive(Serialize)]
struct PaneMovedOutput {
    schema_version: u32,
    process_id: u32,
    request_id: String,
    source_window_id: u64,
    source_tab_id: u64,
    source_pane_id: u32,
    destination_window_id: u64,
    destination_tab_id: u64,
    destination_pane_id: u32,
    target_pane_id: u32,
}

#[derive(Serialize)]
struct WindowResizeOutput {
    schema_version: u32,
    process_id: u32,
    request_id: String,
    window_id: u64,
    requested_width: u32,
    requested_height: u32,
    immediate_width: Option<u32>,
    immediate_height: Option<u32>,
}

#[derive(Serialize)]
struct DividerResizeOutput {
    schema_version: u32,
    process_id: u32,
    request_id: String,
    window_id: u64,
    tab_id: u64,
    divider_id: u64,
    previous_ratio_millis: u16,
    ratio_millis: u16,
    changed: bool,
}

/// Every running GUI process, by process ID.
pub(super) fn running_processes() -> anyhow::Result<Vec<u32>> {
    Ok(ControlPaths::discover_all()?
        .into_iter()
        .map(|endpoint| endpoint.process_id)
        .collect())
}

fn endpoint_for(process_id: u32) -> anyhow::Result<ControlPaths> {
    ControlPaths::discover_all()?
        .into_iter()
        .find(|endpoint| endpoint.process_id == process_id)
        .ok_or_else(|| anyhow::anyhow!("running GUI process {process_id} was not found"))
}

/// Every terminal running an agent in GUI process `process_id`, and whether
/// the list was cut short.
pub(super) fn list_agents(
    process_id: u32,
    supervisor: Option<&str>,
) -> anyhow::Result<(Vec<protocol::AgentSummary>, bool)> {
    let listing = Request::ListAgents {
        supervisor: supervisor.map(str::to_owned),
    };
    listing.validate().map_err(anyhow::Error::msg)?;
    match request(&endpoint_for(process_id)?, listing)? {
        ServerFrame::Agents {
            version,
            process_id: returned,
            agents,
            truncated,
        } if version == protocol::PROTOCOL_VERSION && returned == process_id => {
            protocol::validate_agent_summaries(&agents)?;
            Ok((agents, truncated))
        }
        ServerFrame::Error { message, .. } => anyhow::bail!(message),
        _ => anyhow::bail!("Invalid agent list response"),
    }
}

/// Paste `text` into the agent in one pane and submit it, as the supervisor
/// when `supervisor` carries its token. Returns whether it waits for the
/// person's approval.
pub(super) fn submit_to_agent(
    process_id: u32,
    window_id: u64,
    tab_id: u64,
    pane_id: u32,
    agent_id: &str,
    text: &str,
    supervisor: Option<&str>,
) -> anyhow::Result<bool> {
    let request_id = generate_request_id()?;
    let submission = Request::SubmitToAgent {
        request_id: request_id.clone(),
        window_id,
        tab_id,
        pane_id,
        agent_id: agent_id.into(),
        text: text.into(),
        supervisor: supervisor.map(str::to_owned),
    };
    submission.validate().map_err(anyhow::Error::msg)?;
    match request(&endpoint_for(process_id)?, submission)? {
        ServerFrame::AgentInputAccepted {
            version,
            process_id: returned,
            request_id: returned_id,
            window_id: returned_window,
            tab_id: returned_tab,
            pane_id: returned_pane,
            pending_approval,
        } if version == protocol::PROTOCOL_VERSION
            && returned == process_id
            && returned_id == request_id
            && (returned_window, returned_tab, returned_pane) == (window_id, tab_id, pane_id) =>
        {
            Ok(pending_approval)
        }
        ServerFrame::Error { message, .. } => anyhow::bail!(message),
        _ => anyhow::bail!("Invalid agent input response"),
    }
}

/// One agent view session's recent conversation, and whether older parts
/// were left out.
pub(super) fn read_conversation(
    process_id: u32,
    window_id: u64,
    session_id: u64,
) -> anyhow::Result<(String, bool)> {
    let reading = Request::ReadConversation {
        window_id,
        session_id,
    };
    reading.validate().map_err(anyhow::Error::msg)?;
    match request(&endpoint_for(process_id)?, reading)? {
        ServerFrame::Conversation {
            version,
            process_id: returned,
            window_id: returned_window,
            session_id: returned_session,
            text,
            truncated,
        } if version == protocol::PROTOCOL_VERSION
            && returned == process_id
            && (returned_window, returned_session) == (window_id, session_id) =>
        {
            Ok((text, truncated))
        }
        ServerFrame::Error { message, .. } => anyhow::bail!(message),
        _ => anyhow::bail!("Invalid conversation response"),
    }
}

/// Answer one pending approval in an agent view session. Returns whether it
/// was answered, rather than kept as a suggestion for the person.
pub(super) fn answer_approval(
    process_id: u32,
    window_id: u64,
    session_id: u64,
    approval_id: &str,
    allow: bool,
    reason: &str,
    supervisor: Option<&str>,
) -> anyhow::Result<bool> {
    let request_id = generate_request_id()?;
    let answer = Request::AnswerApproval {
        request_id: request_id.clone(),
        window_id,
        session_id,
        approval_id: approval_id.into(),
        allow,
        reason: reason.into(),
        supervisor: supervisor.map(str::to_owned),
    };
    answer.validate().map_err(anyhow::Error::msg)?;
    match request(&endpoint_for(process_id)?, answer)? {
        ServerFrame::ApprovalAnswered {
            version,
            process_id: returned,
            request_id: returned_id,
            window_id: returned_window,
            session_id: returned_session,
            answered,
        } if version == protocol::PROTOCOL_VERSION
            && returned == process_id
            && returned_id == request_id
            && (returned_window, returned_session) == (window_id, session_id) =>
        {
            Ok(answered)
        }
        ServerFrame::Error { message, .. } => anyhow::bail!(message),
        _ => anyhow::bail!("Invalid approval response"),
    }
}

/// Send a message to an agent view session. Returns whether it waits for the
/// person's approval.
pub(super) fn submit_to_session(
    process_id: u32,
    window_id: u64,
    session_id: u64,
    text: &str,
    supervisor: Option<&str>,
) -> anyhow::Result<bool> {
    let request_id = generate_request_id()?;
    let submission = Request::SubmitToSession {
        request_id: request_id.clone(),
        window_id,
        session_id,
        text: text.into(),
        supervisor: supervisor.map(str::to_owned),
    };
    submission.validate().map_err(anyhow::Error::msg)?;
    match request(&endpoint_for(process_id)?, submission)? {
        ServerFrame::SessionInputAccepted {
            version,
            process_id: returned,
            request_id: returned_id,
            window_id: returned_window,
            session_id: returned_session,
            pending_approval,
        } if version == protocol::PROTOCOL_VERSION
            && returned == process_id
            && returned_id == request_id
            && (returned_window, returned_session) == (window_id, session_id) =>
        {
            Ok(pending_approval)
        }
        ServerFrame::Error { message, .. } => anyhow::bail!(message),
        _ => anyhow::bail!("Invalid session input response"),
    }
}

/// How long an agent hook waits for the GUI. A hook runs inside the agent's
/// turn, so a busy or stuck GUI must not hold the agent up for long.
const AGENT_EVENT_TIMEOUT: Duration = Duration::from_secs(2);

/// Deliver one agent hook report to the GUI process that owns its terminal.
pub(super) fn report_agent_event(process_id: u32, report: Request) -> anyhow::Result<()> {
    report.validate().map_err(anyhow::Error::msg)?;
    let endpoint = ControlPaths::from_directory(
        crate::domain::paths::discover_runtime_directory(),
        process_id,
    )?;
    match request_within(&endpoint, report, AGENT_EVENT_TIMEOUT)? {
        ServerFrame::AgentEventRecorded {
            version,
            process_id: returned,
        } if version == protocol::PROTOCOL_VERSION && returned == process_id => Ok(()),
        ServerFrame::Error { message, .. } => anyhow::bail!(message),
        _ => anyhow::bail!("Invalid agent-event response"),
    }
}

/// Asks a running Termeric on `display` to open a window of `tabs`, so the
/// new window shares its process. Processes are asked in turn; one that
/// refuses (another display, too many windows, an older protocol) passes to
/// the next. Returns the process and window that opened it, or `None`.
pub fn join_running(
    display: &str,
    build: &str,
    activation_token: Option<String>,
    tabs: Vec<protocol::JoinTab>,
) -> anyhow::Result<Option<(u32, u64)>> {
    let mut endpoints = ControlPaths::discover_all()?;
    endpoints.sort_by_key(|endpoint| endpoint.process_id);
    for endpoint in endpoints {
        if !process_is_alive(endpoint.process_id) {
            continue;
        }
        let request_id = generate_request_id()?;
        let frame = request(
            &endpoint,
            Request::JoinLaunch {
                request_id: request_id.clone(),
                display: display.to_owned(),
                activation_token: activation_token.clone(),
                build: build.to_owned(),
                tabs: tabs.clone(),
            },
        );
        match frame {
            Ok(ServerFrame::WindowCreated {
                version,
                process_id,
                request_id: returned,
                window_id,
            }) if version == protocol::PROTOCOL_VERSION
                && process_id == endpoint.process_id
                && returned == request_id
                && window_id != 0 =>
            {
                return Ok(Some((process_id, window_id)));
            }
            Ok(ServerFrame::Error { message, .. }) => log::debug!(
                "Termeric {} did not open the window: {message}",
                endpoint.process_id
            ),
            Ok(frame) => log::debug!(
                "Termeric {} answered a joining launch with {frame:?}",
                endpoint.process_id
            ),
            Err(error) => log::debug!(
                "Termeric {} did not answer a joining launch: {error:#}",
                endpoint.process_id
            ),
        }
    }
    Ok(None)
}

fn request(endpoint: &ControlPaths, request: Request) -> anyhow::Result<ServerFrame> {
    request_within(endpoint, request, IO_TIMEOUT)
}

fn request_within(
    endpoint: &ControlPaths,
    request: Request,
    timeout: Duration,
) -> anyhow::Result<ServerFrame> {
    let token = endpoint.read_token()?;
    let mut stream = UnixStream::connect(&endpoint.socket)
        .with_context(|| format!("connect to {}", endpoint.socket.display()))?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    validate_peer(&stream)?;
    protocol::write_frame(
        &mut stream,
        &ClientFrame {
            version: protocol::PROTOCOL_VERSION,
            token,
            request,
        },
    )?;
    protocol::read_frame(&mut stream)
}

pub(super) fn generate_request_id() -> anyhow::Result<String> {
    termeric_core::hex::random(16).context("generate GUI control request ID")
}

#[cfg(target_os = "linux")]
pub(super) fn validate_peer(stream: &UnixStream) -> anyhow::Result<()> {
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
        return Err(std::io::Error::last_os_error()).context("authenticate GUI control server");
    }
    if credentials.uid != crate::domain::paths::effective_uid() {
        anyhow::bail!("GUI control server belongs to a different user");
    }
    Ok(())
}

#[cfg(all(unix, not(target_os = "linux")))]
pub(super) fn validate_peer(stream: &UnixStream) -> anyhow::Result<()> {
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: getpeereid receives a connected descriptor and valid output
    // pointers and retains neither.
    let result =
        unsafe { libc::getpeereid(std::os::fd::AsRawFd::as_raw_fd(stream), &mut uid, &mut gid) };
    if result != 0 {
        return Err(std::io::Error::last_os_error()).context("authenticate GUI control server");
    }
    if uid != crate::domain::paths::effective_uid() {
        anyhow::bail!("GUI control server belongs to a different user");
    }
    Ok(())
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
                    let direction = match divider.direction {
                        super::protocol::SplitDirection::Horizontal => "horizontal",
                        super::protocol::SplitDirection::Vertical => "vertical",
                    };
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
        let frame = ServerFrame::LaunchStatus {
            version: protocol::PROTOCOL_VERSION,
            process_id: 42,
            launch_request_id: id.clone(),
            state: protocol::LaunchState::Exited {
                exit_code: 7,
                success: false,
            },
            terminal_closed: true,
        };
        let json = format_launch_status(frame.clone(), 42, &id, true).unwrap();
        let json: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(json["state"]["exit_code"], 7);
        assert_eq!(json["state"]["success"], false);
        assert_eq!(json["terminal_closed"], true);
        for private in ["program", "args", "environment", "cwd", "text"] {
            assert!(json.get(private).is_none());
        }
        assert!(format_launch_status(frame.clone(), 43, &id, false).is_err());
        assert!(format_launch_status(frame.clone(), 42, &"b".repeat(32), false).is_err());
        let mut incompatible = frame.clone();
        if let ServerFrame::LaunchStatus { version, .. } = &mut incompatible {
            *version -= 1;
        }
        assert!(format_launch_status(incompatible, 42, &id, false).is_err());
        assert!(
            format_launch_status(frame, 42, &id, false)
                .unwrap()
                .contains("portable code 7")
        );
    }
    use super::*;
    use crate::control::protocol::{
        DividerInventory, DividerKind, PaneInventory, SplitDirection, TabInventory, TabKind,
        WindowInventory,
    };
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;
    use std::thread;

    #[test]
    fn quick_access_discovery_and_toggle_cross_authenticated_socket() {
        let temp = tempfile::tempdir().unwrap();
        let endpoint =
            ControlPaths::from_directory(temp.path().join("runtime"), std::process::id()).unwrap();
        let token = endpoint.load_or_create_token().unwrap();
        let listener = match UnixListener::bind(&endpoint.socket) {
            Ok(listener) => listener,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!("skip quick-access socket round trip: sandbox denies AF_UNIX");
                return;
            }
            Err(error) => panic!("could not bind quick-access test socket: {error}"),
        };
        fs::set_permissions(&endpoint.socket, fs::Permissions::from_mode(0o600)).unwrap();
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
                        &ServerFrame::Inventory {
                            version: protocol::PROTOCOL_VERSION,
                            inventory: ProcessInventory {
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
                            },
                        },
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
                        &ServerFrame::QuickAccessChanged {
                            version: protocol::PROTOCOL_VERSION,
                            process_id: std::process::id(),
                            request_id,
                            window_id,
                            visible,
                        },
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
            ControlPaths::from_directory(temp.path().join("runtime"), std::process::id()).unwrap();
        let token = endpoint.load_or_create_token().unwrap();
        let listener = match UnixListener::bind(&endpoint.socket) {
            Ok(listener) => listener,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!("skip quick-access retry round trip: sandbox denies AF_UNIX");
                return;
            }
            Err(error) => panic!("could not bind quick-access retry test socket: {error}"),
        };
        fs::set_permissions(&endpoint.socket, fs::Permissions::from_mode(0o600)).unwrap();
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
                        &ServerFrame::Inventory {
                            version: protocol::PROTOCOL_VERSION,
                            inventory: ProcessInventory {
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
                            },
                        },
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
                    &ServerFrame::QuickAccessChanged {
                        version: protocol::PROTOCOL_VERSION,
                        process_id: std::process::id(),
                        request_id: request_id.clone(),
                        window_id: *window_id,
                        visible: *visible,
                    },
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
        assert_eq!(output["schema_version"], 3);
        assert_eq!(output["processes"], serde_json::json!([]));
    }

    #[test]
    fn generated_request_ids_are_canonical_and_action_output_is_versioned() {
        let request_id = generate_request_id().unwrap();
        assert!(protocol::valid_request_id(&request_id));
        let output = serde_json::to_value(WindowCreatedOutput {
            schema_version: protocol::JSON_SCHEMA_VERSION,
            process_id: 42,
            request_id: request_id.clone(),
            window_id: 9,
        })
        .unwrap();
        assert_eq!(output["schema_version"], 3);
        assert_eq!(output["request_id"], request_id);
        assert_eq!(output["window_id"], 9);

        let output = serde_json::to_value(WindowLaunchedOutput {
            schema_version: protocol::JSON_SCHEMA_VERSION,
            process_id: 42,
            request_id: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
            window_id: 10,
            tab_id: 0,
        })
        .unwrap();
        assert_eq!(output["schema_version"], 3);
        assert_eq!(output["window_id"], 10);
        assert_eq!(output["tab_id"], 0);

        let output = serde_json::to_value(TabCreatedOutput {
            schema_version: protocol::JSON_SCHEMA_VERSION,
            process_id: 42,
            request_id: "fedcba9876543210fedcba9876543210".into(),
            window_id: 9,
            tab_id: 3,
        })
        .unwrap();
        assert_eq!(output["schema_version"], 3);
        assert_eq!(output["window_id"], 9);
        assert_eq!(output["tab_id"], 3);

        let output = serde_json::to_value(PaneCreatedOutput {
            schema_version: protocol::JSON_SCHEMA_VERSION,
            process_id: 42,
            request_id: "1234567890abcdef1234567890abcdef".into(),
            window_id: 9,
            tab_id: 3,
            pane_id: 1,
        })
        .unwrap();
        assert_eq!(output["schema_version"], 3);
        assert_eq!(output["tab_id"], 3);
        assert_eq!(output["pane_id"], 1);

        let window_focus = serde_json::to_value(WindowFocusOutput {
            schema_version: protocol::JSON_SCHEMA_VERSION,
            process_id: 42,
            request_id: "a".repeat(32),
            window_id: 9,
            already_focused: false,
        })
        .unwrap();
        assert_eq!(window_focus["already_focused"], false);

        let tab_selection = serde_json::to_value(TabSelectedOutput {
            schema_version: protocol::JSON_SCHEMA_VERSION,
            process_id: 42,
            request_id: "b".repeat(32),
            window_id: 9,
            tab_id: 3,
            changed: true,
        })
        .unwrap();
        assert_eq!(tab_selection["changed"], true);

        let pane_focus = serde_json::to_value(PaneFocusedOutput {
            schema_version: protocol::JSON_SCHEMA_VERSION,
            process_id: 42,
            request_id: "c".repeat(32),
            window_id: 9,
            tab_id: 3,
            pane_id: 1,
            tab_changed: true,
            pane_changed: false,
        })
        .unwrap();
        assert_eq!(pane_focus["tab_changed"], true);
        assert_eq!(pane_focus["pane_changed"], false);

        let window_close = serde_json::to_value(WindowCloseOutput {
            schema_version: protocol::JSON_SCHEMA_VERSION,
            process_id: 42,
            request_id: "d".repeat(32),
            window_id: 9,
            scheduled: true,
        })
        .unwrap();
        assert_eq!(window_close["scheduled"], true);

        let tab_close = serde_json::to_value(TabCloseOutput {
            schema_version: protocol::JSON_SCHEMA_VERSION,
            process_id: 42,
            request_id: "e".repeat(32),
            window_id: 9,
            tab_id: 3,
            scheduled: true,
        })
        .unwrap();
        assert_eq!(tab_close["scheduled"], true);

        let pane_close = serde_json::to_value(PaneCloseOutput {
            schema_version: protocol::JSON_SCHEMA_VERSION,
            process_id: 42,
            request_id: "f".repeat(32),
            window_id: 9,
            tab_id: 3,
            pane_id: 1,
            remaining_panes: 1,
            focused_pane_id: 0,
        })
        .unwrap();
        assert_eq!(pane_close["remaining_panes"], 1);
        assert_eq!(pane_close["focused_pane_id"], 0);

        let tab_move = serde_json::to_value(TabMovedOutput {
            schema_version: protocol::JSON_SCHEMA_VERSION,
            process_id: 42,
            request_id: "0".repeat(32),
            source_window_id: 9,
            source_tab_id: 3,
            destination_window_id: 10,
            destination_tab_id: 7,
            destination_index: 2,
            source_window_closing: false,
            changed: true,
        })
        .unwrap();
        assert_eq!(tab_move["destination_tab_id"], 7);
        assert_eq!(tab_move["destination_index"], 2);
        assert_eq!(tab_move["changed"], true);

        let pane_move = serde_json::to_value(PaneMovedOutput {
            schema_version: protocol::JSON_SCHEMA_VERSION,
            process_id: 42,
            request_id: "1".repeat(32),
            source_window_id: 9,
            source_tab_id: 3,
            source_pane_id: 1,
            destination_window_id: 10,
            destination_tab_id: 7,
            destination_pane_id: 5,
            target_pane_id: 2,
        })
        .unwrap();
        assert_eq!(pane_move["destination_pane_id"], 5);
        assert_eq!(pane_move["target_pane_id"], 2);

        let window_resize = serde_json::to_value(WindowResizeOutput {
            schema_version: protocol::JSON_SCHEMA_VERSION,
            process_id: 42,
            request_id: "2".repeat(32),
            window_id: 9,
            requested_width: 900,
            requested_height: 600,
            immediate_width: None,
            immediate_height: None,
        })
        .unwrap();
        assert_eq!(window_resize["requested_width"], 900);
        assert!(window_resize["immediate_width"].is_null());

        let divider_resize = serde_json::to_value(DividerResizeOutput {
            schema_version: protocol::JSON_SCHEMA_VERSION,
            process_id: 42,
            request_id: "3".repeat(32),
            window_id: 9,
            tab_id: 3,
            divider_id: 11,
            previous_ratio_millis: 500,
            ratio_millis: 650,
            changed: true,
        })
        .unwrap();
        assert_eq!(divider_resize["divider_id"], 11);
        assert_eq!(divider_resize["previous_ratio_millis"], 500);
    }
}
