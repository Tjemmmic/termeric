//! Kitty OSC 22 mouse-pointer-shape protocol state.

/// Minimum stack depth required by the Kitty pointer-shape protocol.
pub(crate) const POINTER_SHAPE_STACK_DEPTH: usize = 16;

/// Maximum comma-separated entries accepted by one OSC 22 operation.
pub(crate) const MAX_POINTER_SHAPE_LIST_ENTRIES: usize = 64;

/// Maximum OSC 22 payload accepted after the command selector.
pub(crate) const MAX_POINTER_SHAPE_PAYLOAD_BYTES: usize = 1024;

/// Mouse pointer shapes required by the Kitty OSC 22 protocol.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PointerShape {
    Alias,
    Cell,
    Copy,
    Crosshair,
    #[default]
    Default,
    EResize,
    EwResize,
    Grab,
    Grabbing,
    Help,
    Move,
    NResize,
    NeResize,
    NeswResize,
    NoDrop,
    NotAllowed,
    NsResize,
    NwResize,
    NwseResize,
    Pointer,
    Progress,
    SResize,
    SeResize,
    SwResize,
    Text,
    VerticalText,
    WResize,
    Wait,
    ZoomIn,
    ZoomOut,
}

impl PointerShape {
    /// All required protocol shapes in the specification's canonical order.
    pub const ALL: [Self; 30] = [
        Self::Alias,
        Self::Cell,
        Self::Copy,
        Self::Crosshair,
        Self::Default,
        Self::EResize,
        Self::EwResize,
        Self::Grab,
        Self::Grabbing,
        Self::Help,
        Self::Move,
        Self::NResize,
        Self::NeResize,
        Self::NeswResize,
        Self::NoDrop,
        Self::NotAllowed,
        Self::NsResize,
        Self::NwResize,
        Self::NwseResize,
        Self::Pointer,
        Self::Progress,
        Self::SResize,
        Self::SeResize,
        Self::SwResize,
        Self::Text,
        Self::VerticalText,
        Self::WResize,
        Self::Wait,
        Self::ZoomIn,
        Self::ZoomOut,
    ];

    /// Parse one canonical, case-sensitive OSC 22 shape name.
    pub fn from_name(name: &[u8]) -> Option<Self> {
        Some(match name {
            b"alias" => Self::Alias,
            b"cell" => Self::Cell,
            b"copy" => Self::Copy,
            b"crosshair" => Self::Crosshair,
            b"default" => Self::Default,
            b"e-resize" => Self::EResize,
            b"ew-resize" => Self::EwResize,
            b"grab" => Self::Grab,
            b"grabbing" => Self::Grabbing,
            b"help" => Self::Help,
            b"move" => Self::Move,
            b"n-resize" => Self::NResize,
            b"ne-resize" => Self::NeResize,
            b"nesw-resize" => Self::NeswResize,
            b"no-drop" => Self::NoDrop,
            b"not-allowed" => Self::NotAllowed,
            b"ns-resize" => Self::NsResize,
            b"nw-resize" => Self::NwResize,
            b"nwse-resize" => Self::NwseResize,
            b"pointer" => Self::Pointer,
            b"progress" => Self::Progress,
            b"s-resize" => Self::SResize,
            b"se-resize" => Self::SeResize,
            b"sw-resize" => Self::SwResize,
            b"text" => Self::Text,
            b"vertical-text" => Self::VerticalText,
            b"w-resize" => Self::WResize,
            b"wait" => Self::Wait,
            b"zoom-in" => Self::ZoomIn,
            b"zoom-out" => Self::ZoomOut,
            _ => return None,
        })
    }

    /// Return this shape's canonical OSC 22 name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Alias => "alias",
            Self::Cell => "cell",
            Self::Copy => "copy",
            Self::Crosshair => "crosshair",
            Self::Default => "default",
            Self::EResize => "e-resize",
            Self::EwResize => "ew-resize",
            Self::Grab => "grab",
            Self::Grabbing => "grabbing",
            Self::Help => "help",
            Self::Move => "move",
            Self::NResize => "n-resize",
            Self::NeResize => "ne-resize",
            Self::NeswResize => "nesw-resize",
            Self::NoDrop => "no-drop",
            Self::NotAllowed => "not-allowed",
            Self::NsResize => "ns-resize",
            Self::NwResize => "nw-resize",
            Self::NwseResize => "nwse-resize",
            Self::Pointer => "pointer",
            Self::Progress => "progress",
            Self::SResize => "s-resize",
            Self::SeResize => "se-resize",
            Self::SwResize => "sw-resize",
            Self::Text => "text",
            Self::VerticalText => "vertical-text",
            Self::WResize => "w-resize",
            Self::Wait => "wait",
            Self::ZoomIn => "zoom-in",
            Self::ZoomOut => "zoom-out",
        }
    }
}

/// Fixed-capacity screen-local stack. Overflow evicts the oldest entry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct PointerShapeStack {
    shapes: [PointerShape; POINTER_SHAPE_STACK_DEPTH],
    len: usize,
}

impl PointerShapeStack {
    pub(super) fn current(&self) -> Option<PointerShape> {
        self.len.checked_sub(1).map(|index| self.shapes[index])
    }

    pub(super) fn set(&mut self, shape: PointerShape) {
        if self.len == 0 {
            self.len = 1;
        }
        self.shapes[self.len - 1] = shape;
    }

    pub(super) fn push(&mut self, shape: PointerShape) {
        if self.len == POINTER_SHAPE_STACK_DEPTH {
            self.shapes.rotate_left(1);
            self.shapes[POINTER_SHAPE_STACK_DEPTH - 1] = shape;
        } else {
            self.shapes[self.len] = shape;
            self.len += 1;
        }
    }

    pub(super) fn pop(&mut self) {
        self.len = self.len.saturating_sub(1);
    }

    pub(super) fn clear(&mut self) {
        self.len = 0;
    }
}

/// Parse an entire non-empty shape list before the caller mutates state.
pub(super) fn parse_shape_list(
    payload: &[u8],
) -> Option<([PointerShape; MAX_POINTER_SHAPE_LIST_ENTRIES], usize)> {
    if payload.is_empty() || payload.len() > MAX_POINTER_SHAPE_PAYLOAD_BYTES {
        return None;
    }
    let mut shapes = [PointerShape::Default; MAX_POINTER_SHAPE_LIST_ENTRIES];
    let mut len = 0;
    for name in payload.split(|byte| *byte == b',') {
        let slot = shapes.get_mut(len)?;
        *slot = PointerShape::from_name(name)?;
        len += 1;
    }
    Some((shapes, len))
}

/// Validate a query token without accepting arbitrary control/string data.
pub(super) fn valid_query_name(name: &[u8]) -> bool {
    !name.is_empty()
        && name.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_required_shapes_round_trip() {
        assert_eq!(PointerShape::ALL.len(), 30);
        for shape in PointerShape::ALL {
            assert_eq!(
                PointerShape::from_name(shape.name().as_bytes()),
                Some(shape)
            );
        }
    }

    #[test]
    fn shape_names_are_strict_and_case_sensitive() {
        assert_eq!(PointerShape::from_name(b"Pointer"), None);
        assert_eq!(PointerShape::from_name(b"arrow"), None);
        assert_eq!(PointerShape::from_name(b"pointer\0"), None);
        assert_eq!(PointerShape::from_name(&[0xff]), None);
    }

    #[test]
    fn stack_set_push_pop_and_overflow_are_bounded() {
        let mut stack = PointerShapeStack::default();
        stack.set(PointerShape::Text);
        stack.push(PointerShape::Pointer);
        stack.set(PointerShape::Wait);
        assert_eq!(stack.current(), Some(PointerShape::Wait));
        stack.pop();
        assert_eq!(stack.current(), Some(PointerShape::Text));

        stack.clear();
        for index in 0..=POINTER_SHAPE_STACK_DEPTH {
            stack.push(PointerShape::ALL[index]);
        }
        assert_eq!(stack.len, POINTER_SHAPE_STACK_DEPTH);
        stack.pop();
        assert_eq!(
            stack.current(),
            Some(PointerShape::ALL[POINTER_SHAPE_STACK_DEPTH - 1])
        );
    }

    #[test]
    fn list_parser_is_atomic_and_bounded() {
        let (shapes, len) = parse_shape_list(b"text,pointer,wait").unwrap();
        assert_eq!(
            &shapes[..len],
            &[
                PointerShape::Text,
                PointerShape::Pointer,
                PointerShape::Wait
            ]
        );
        assert!(parse_shape_list(b"text,,wait").is_none());
        assert!(parse_shape_list(b"text,missing").is_none());
        assert!(parse_shape_list(&vec![b'x'; MAX_POINTER_SHAPE_PAYLOAD_BYTES + 1]).is_none());
        let too_many = vec!["text"; MAX_POINTER_SHAPE_LIST_ENTRIES + 1].join(",");
        assert!(parse_shape_list(too_many.as_bytes()).is_none());
    }

    #[test]
    fn query_name_validation_accepts_only_protocol_alphabet() {
        assert!(valid_query_name(b"__current__"));
        assert!(valid_query_name(b"no-such-name"));
        assert!(!valid_query_name(b""));
        assert!(!valid_query_name(b"Pointer"));
        assert!(!valid_query_name(b"pointer;title"));
        assert!(!valid_query_name(&[0xff]));
    }
}
