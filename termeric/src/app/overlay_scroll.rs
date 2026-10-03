//! The scrolling keys the info overlay and diff review share.

use winit::keyboard::{Key, ModifiersState, NamedKey};

/// How far one key scrolls a two-axis text viewport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OverlayScroll {
    Rows(isize),
    Columns(isize),
    Top,
    Bottom,
    LineStart,
    LineEnd,
}

/// Columns one horizontal arrow press scrolls.
const COLUMN_STEP: isize = 4;

/// Arrows scroll one row or a few columns, Page Up/Down a viewport of rows,
/// Ctrl+Home/End to the first or last row, and Home/End to the start or end
/// of the lines.
pub(super) fn overlay_scroll_for_key(
    key: &Key,
    modifiers: ModifiersState,
    viewport_rows: usize,
) -> Option<OverlayScroll> {
    let Key::Named(named) = key else {
        return None;
    };
    let page = viewport_rows as isize;
    Some(match named {
        NamedKey::ArrowUp => OverlayScroll::Rows(-1),
        NamedKey::ArrowDown => OverlayScroll::Rows(1),
        NamedKey::PageUp => OverlayScroll::Rows(-page),
        NamedKey::PageDown => OverlayScroll::Rows(page),
        NamedKey::ArrowLeft => OverlayScroll::Columns(-COLUMN_STEP),
        NamedKey::ArrowRight => OverlayScroll::Columns(COLUMN_STEP),
        NamedKey::Home if modifiers.control_key() => OverlayScroll::Top,
        NamedKey::End if modifiers.control_key() => OverlayScroll::Bottom,
        NamedKey::Home => OverlayScroll::LineStart,
        NamedKey::End => OverlayScroll::LineEnd,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_map_to_rows_columns_and_ends() {
        let plain = ModifiersState::empty();
        let ctrl = ModifiersState::CONTROL;
        for (key, modifiers, scroll) in [
            (NamedKey::ArrowUp, plain, Some(OverlayScroll::Rows(-1))),
            (NamedKey::ArrowDown, plain, Some(OverlayScroll::Rows(1))),
            (NamedKey::PageUp, plain, Some(OverlayScroll::Rows(-7))),
            (NamedKey::PageDown, plain, Some(OverlayScroll::Rows(7))),
            (NamedKey::ArrowLeft, plain, Some(OverlayScroll::Columns(-4))),
            (NamedKey::ArrowRight, plain, Some(OverlayScroll::Columns(4))),
            (NamedKey::Home, ctrl, Some(OverlayScroll::Top)),
            (NamedKey::End, ctrl, Some(OverlayScroll::Bottom)),
            (NamedKey::Home, plain, Some(OverlayScroll::LineStart)),
            (NamedKey::End, plain, Some(OverlayScroll::LineEnd)),
            (NamedKey::Enter, plain, None),
        ] {
            assert_eq!(
                overlay_scroll_for_key(&Key::Named(key), modifiers, 7),
                scroll,
                "{key:?} {modifiers:?}"
            );
        }
        assert_eq!(
            overlay_scroll_for_key(&Key::Character("j".into()), plain, 7),
            None
        );
    }
}
