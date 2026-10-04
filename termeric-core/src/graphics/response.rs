//! Protocol errors and the responses Kitty clients read back.

use super::*;

#[derive(Debug, Clone)]
pub(crate) struct ProtocolError {
    pub(super) code: &'static str,
    pub(crate) message: String,
}

impl ProtocolError {
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: "EINVAL",
            message: message.into(),
        }
    }
    pub(crate) fn unsupported(message: impl Into<String>) -> Self {
        Self {
            code: "ENOTSUP",
            message: message.into(),
        }
    }
    pub(crate) fn permission_denied(message: impl Into<String>) -> Self {
        Self {
            code: "EPERM",
            message: message.into(),
        }
    }
    pub(crate) fn too_large(message: impl Into<String>) -> Self {
        Self {
            code: "E2BIG",
            message: message.into(),
        }
    }
    pub(super) fn no_space(message: impl Into<String>) -> Self {
        Self {
            code: "ENOSPC",
            message: message.into(),
        }
    }
    pub(super) fn parent_missing(message: impl Into<String>) -> Self {
        Self {
            code: "ENOPARENT",
            message: message.into(),
        }
    }
    pub(super) fn cycle(message: impl Into<String>) -> Self {
        Self {
            code: "ECYCLE",
            message: message.into(),
        }
    }
    pub(super) fn too_deep(message: impl Into<String>) -> Self {
        Self {
            code: "ETOODEEP",
            message: message.into(),
        }
    }
    pub(super) fn io(message: impl Into<String>) -> Self {
        Self {
            code: "EIO",
            message: message.into(),
        }
    }
    pub(super) fn not_found(message: impl Into<String>) -> Self {
        Self {
            code: "ENOENT",
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct GraphicsResponse {
    pub image_id: u32,
    pub image_number: Option<u32>,
    pub placement_id: Option<u32>,
    pub frame_number: Option<u32>,
    pub ok: bool,
    pub error_code: Option<&'static str>,
    pub error: Option<String>,
}

impl GraphicsResponse {
    pub(crate) fn parse_error(message: &str) -> Self {
        Self {
            image_id: 0,
            image_number: None,
            placement_id: None,
            frame_number: None,
            ok: false,
            error_code: Some("EINVAL"),
            error: Some(message.to_owned()),
        }
    }

    pub(crate) fn too_large(message: &str) -> Self {
        Self {
            image_id: 0,
            image_number: None,
            placement_id: None,
            frame_number: None,
            ok: false,
            error_code: Some("E2BIG"),
            error: Some(message.to_owned()),
        }
    }

    pub(crate) fn to_escape_sequence(&self) -> String {
        let mut controls = if self.image_id != 0 || self.image_number.is_none() {
            format!("i={}", self.image_id)
        } else {
            String::new()
        };
        if let Some(image_number) = self.image_number.filter(|number| *number != 0) {
            if !controls.is_empty() {
                controls.push(',');
            }
            controls.push_str(&format!("I={image_number}"));
        }
        if self.image_id != 0
            && let Some(placement_id) = self.placement_id.filter(|id| *id != 0)
        {
            controls.push_str(&format!(",p={placement_id}"));
        }
        if let Some(frame_number) = self.frame_number.filter(|frame| *frame != 0) {
            controls.push_str(&format!(",r={frame_number}"));
        }
        if self.ok {
            format!("\x1b_G{controls};OK\x1b\\")
        } else {
            let code = self.error_code.unwrap_or("EINVAL");
            let message: String = self
                .error
                .as_deref()
                .unwrap_or("graphics error")
                .chars()
                .filter(|character| !character.is_control())
                .take(160)
                .collect();
            format!("\x1b_G{controls};{code}:{message}\x1b\\")
        }
    }
}

pub(super) fn response_for(
    cmd: &KittyCommand,
    result: Result<(), ProtocolError>,
) -> Option<GraphicsResponse> {
    response_fields(
        cmd.image_id.unwrap_or(0),
        cmd.image_number,
        cmd.placement_id,
        cmd.quiet,
        result,
    )
}

pub(super) fn response_for_resolved(
    cmd: &KittyCommand,
    image_id: u32,
    result: Result<(), ProtocolError>,
) -> Option<GraphicsResponse> {
    response_fields(
        image_id,
        cmd.image_number,
        cmd.placement_id,
        cmd.quiet,
        result,
    )
}

pub(super) fn response_fields(
    image_id: u32,
    image_number: Option<u32>,
    placement_id: Option<u32>,
    quiet: u8,
    result: Result<(), ProtocolError>,
) -> Option<GraphicsResponse> {
    let placement_id = placement_id.filter(|value| image_id != 0 && *value != 0);
    let is_error = result.is_err();
    let image_number = image_number.filter(|number| *number != 0);
    if quiet == 2
        || (quiet == 1 && !is_error)
        || (image_id == 0 && image_number.is_none() && placement_id.is_none())
    {
        return None;
    }
    let (ok, error_code, error) = match result {
        Ok(()) => (true, None, None),
        Err(error) => (false, Some(error.code), Some(error.message)),
    };
    Some(GraphicsResponse {
        image_id,
        image_number,
        placement_id,
        frame_number: None,
        ok,
        error_code,
        error,
    })
}

pub(super) fn frame_response_fields(
    image_id: u32,
    image_number: Option<u32>,
    frame_number: Option<u32>,
    quiet: u8,
    result: Result<(), ProtocolError>,
) -> Option<GraphicsResponse> {
    let mut response = response_fields(image_id, image_number, None, quiet, result)?;
    response.frame_number = frame_number.filter(|frame| *frame != 0);
    Some(response)
}

pub(super) fn failure(cmd: &KittyCommand, error: ProtocolError) -> GraphicsCommandResult {
    GraphicsCommandResult {
        response: response_for(cmd, Err(error)),
        cursor_movement: None,
    }
}

pub(super) fn pending_failure(
    pending: PendingImage,
    error: ProtocolError,
) -> GraphicsCommandResult {
    GraphicsCommandResult {
        response: response_fields(
            pending.response_id,
            pending.response_number,
            pending.placement_id,
            pending.quiet,
            Err(error),
        ),
        cursor_movement: None,
    }
}

pub(super) fn pending_frame_failure(
    pending: PendingFrame,
    error: ProtocolError,
) -> GraphicsCommandResult {
    GraphicsCommandResult {
        response: frame_response_fields(
            pending.response_id,
            pending.response_number,
            pending.frame.frame_number,
            pending.quiet,
            Err(error),
        ),
        cursor_movement: None,
    }
}
