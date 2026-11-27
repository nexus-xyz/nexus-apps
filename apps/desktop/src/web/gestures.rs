//! Pointer input that a browser window handles itself, and that an off-screen page must get from the app: the mouse's
//! back and forward buttons, and the cursor the page asks for. A touchpad pinch is `WebView::pinch`.

use gpui_kit::{CursorStyle, MouseButton, NavigationDirection};

use super::protocol::Cursor;

/// The history step of a mouse's back (-1) or forward (1) button, as in a browser.
pub(super) fn history_step(button: MouseButton) -> Option<i32> {
    match button {
        MouseButton::Navigate(NavigationDirection::Back) => Some(-1),
        MouseButton::Navigate(NavigationDirection::Forward) => Some(1),
        _ => None,
    }
}

/// The window's cursor for the one the page asks for.
pub(super) fn cursor_style(cursor: Cursor) -> CursorStyle {
    match cursor {
        Cursor::Arrow => CursorStyle::Arrow,
        Cursor::Hand => CursorStyle::PointingHand,
        Cursor::Text => CursorStyle::IBeam,
        Cursor::VerticalText => CursorStyle::IBeamCursorForVerticalLayout,
        Cursor::Crosshair => CursorStyle::Crosshair,
        Cursor::Grab => CursorStyle::OpenHand,
        Cursor::Grabbing => CursorStyle::ClosedHand,
        Cursor::NotAllowed => CursorStyle::OperationNotAllowed,
        Cursor::Copy => CursorStyle::DragCopy,
        Cursor::Alias => CursorStyle::DragLink,
        Cursor::ContextMenu => CursorStyle::ContextualMenu,
        Cursor::ResizeEast => CursorStyle::ResizeRight,
        Cursor::ResizeWest => CursorStyle::ResizeLeft,
        Cursor::ResizeEastWest => CursorStyle::ResizeLeftRight,
        Cursor::ResizeNorth => CursorStyle::ResizeUp,
        Cursor::ResizeSouth => CursorStyle::ResizeDown,
        Cursor::ResizeNorthSouth => CursorStyle::ResizeUpDown,
        Cursor::ResizeNorthWestSouthEast => CursorStyle::ResizeUpLeftDownRight,
        Cursor::ResizeNorthEastSouthWest => CursorStyle::ResizeUpRightDownLeft,
        Cursor::ResizeColumn => CursorStyle::ResizeColumn,
        Cursor::ResizeRow => CursorStyle::ResizeRow,
    }
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    #[test]
    fn back_and_forward_buttons_step_through_history() {
        assert_eq!(history_step(MouseButton::Navigate(NavigationDirection::Back)), Some(-1));
        assert_eq!(
            history_step(MouseButton::Navigate(NavigationDirection::Forward)),
            Some(1)
        );
        assert_eq!(history_step(MouseButton::Left), None);
        assert_eq!(history_step(MouseButton::Right), None);
    }

    #[test]
    fn a_canvas_s_grab_cursors_show() {
        assert_eq!(cursor_style(Cursor::Grab), CursorStyle::OpenHand);
        assert_eq!(cursor_style(Cursor::Grabbing), CursorStyle::ClosedHand);
        assert_eq!(cursor_style(Cursor::ResizeColumn), CursorStyle::ResizeColumn);
        assert_eq!(cursor_style(Cursor::Arrow), CursorStyle::Arrow);
    }
}
