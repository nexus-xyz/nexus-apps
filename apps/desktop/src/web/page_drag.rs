//! A drag that starts inside a page: an HTML drag and drop (a sortable list, a board's cards), a dragged link, image or
//! selected text. Chromium hands it to the window that shows the page (`start_dragging`), to run as the system's drag.
//! Off screen there is no such window, and without this CEF cancels every such drag at once.
//!
//! The host plays the drag on the page itself instead, as CEF's own off-screen sample does with the system's drag: the
//! page is the drag's source and its target. While it lasts, the mouse's moves go to the page as drag moves, a release
//! drops, and the drag ends. Its data stays inside the page: nothing goes to other apps.
//!
//! CEF calls the handlers and the host's commands on the host's main thread, so the state is thread-local.

use std::cell::RefCell;
use std::collections::HashMap;

use cef::ImplBrowserHost;

use super::protocol::{Button, Cursor, MouseAction};

struct Drag {
    data: cef::DragData,
    /// The operations the page allows (`cef_drag_operations_mask_t`).
    allowed: u32,
    /// Whether the pointer is over the page, as the drag's target.
    entered: bool,
    /// The page's cursor before the drag, put back after it.
    cursor: Cursor,
}

thread_local! {
    static DRAGS: RefCell<HashMap<u32, Drag>> = RefCell::new(HashMap::new());
    /// The operation a drop would do now, as the page last said (`update_drag_cursor`).
    static OPERATIONS: RefCell<HashMap<u32, u32>> = RefCell::new(HashMap::new());
}

/// Page `id` started a drag of `data` at `at` (view pixels). It enters the page with the next move.
pub(super) fn start(id: u32, data: cef::DragData, allowed: u32, _at: (i32, i32), cursor: Cursor) {
    OPERATIONS.with_borrow_mut(|operations| operations.remove(&id));
    DRAGS.with_borrow_mut(|drags| {
        drags.insert(
            id,
            Drag {
                data,
                allowed,
                entered: false,
                cursor,
            },
        )
    });
}

/// What a drop on page `id` would do now.
pub(super) fn operation(id: u32, operation: u32) {
    OPERATIONS.with_borrow_mut(|operations| operations.insert(id, operation));
}

/// Page `id` closed.
pub(super) fn forget(id: u32) {
    DRAGS.with_borrow_mut(|drags| drags.remove(&id));
    OPERATIONS.with_borrow_mut(|operations| operations.remove(&id));
}

/// What the host does with a mouse event during a drag.
pub(super) struct Handled {
    /// Send the event to the page as well: a press or another button ends the drag, and then acts as usual.
    pub(super) pass: bool,
    /// The drag ended: the page's cursor from before it.
    pub(super) restore: Option<Cursor>,
}

/// The mouse did `action` at `event` on page `id`. None: no drag, the page gets it as usual.
pub(super) fn mouse(id: u32, host: &cef::BrowserHost, event: &cef::MouseEvent, action: MouseAction) -> Option<Handled> {
    // Out of the map while CEF runs: the page's handlers may call back into it.
    let mut drag = DRAGS.with_borrow_mut(|drags| drags.remove(&id))?;
    let plan = plan(drag.entered, action);
    for step in &plan.steps {
        match step {
            Step::Enter => host.drag_target_drag_enter(Some(&mut drag.data), Some(event), mask(drag.allowed)),
            Step::Over => host.drag_target_drag_over(Some(event), mask(drag.allowed)),
            Step::Leave => host.drag_target_drag_leave(),
            Step::Drop => host.drag_target_drop(Some(event)),
        }
    }
    drag.entered = plan.entered;
    if !plan.ends {
        DRAGS.with_borrow_mut(|drags| drags.insert(id, drag));
        return Some(Handled {
            pass: false,
            restore: None,
        });
    }
    let done = OPERATIONS.with_borrow_mut(|operations| operations.remove(&id));
    end(host, event.x, event.y, if plan.dropped { done.unwrap_or(0) } else { 0 });
    Some(Handled {
        pass: plan.pass,
        restore: Some(drag.cursor),
    })
}

/// Escape on page `id`: its drag, if any, ends with no drop. Returns the cursor to put back.
pub(super) fn cancel(id: u32, host: &cef::BrowserHost) -> Option<Cursor> {
    let drag = DRAGS.with_borrow_mut(|drags| drags.remove(&id))?;
    OPERATIONS.with_borrow_mut(|operations| operations.remove(&id));
    if drag.entered {
        host.drag_target_drag_leave();
    }
    end(host, 0, 0, 0);
    Some(drag.cursor)
}

/// Tell the page, as the drag's source, that the drag ended with `operation` (0: none).
fn end(host: &cef::BrowserHost, x: i32, y: i32, operation: u32) {
    host.drag_source_ended_at(x, y, mask(operation));
    host.drag_source_system_drag_ended();
}

/// A mask from its bits. CEF's C enum is a `c_int` on Windows and a `c_uint` on Linux and macOS, so the bits convert
/// here, in one place, and stay `u32` everywhere else.
#[allow(clippy::unnecessary_cast)]
pub(super) fn mask(bits: u32) -> cef::DragOperationsMask {
    cef::DragOperationsMask::from(cef::sys::cef_drag_operations_mask_t(bits as _))
}

/// A mask's bits, as `u32` on every platform (see `mask`).
#[allow(clippy::unnecessary_cast)]
pub(super) fn bits(mask: &cef::DragOperationsMask) -> u32 {
    mask.as_ref().0 as u32
}

/// A call on the page as the drag's target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Enter,
    Over,
    Leave,
    Drop,
}

#[derive(Debug, PartialEq, Eq)]
struct Plan {
    steps: Vec<Step>,
    /// Whether the pointer is over the page after it.
    entered: bool,
    ends: bool,
    dropped: bool,
    pass: bool,
}

/// What a mouse `action` does to a drag, `entered` on the page or not.
fn plan(entered: bool, action: MouseAction) -> Plan {
    let leave = if entered { vec![Step::Leave] } else { Vec::new() };
    match action {
        MouseAction::Move => Plan {
            steps: vec![if entered { Step::Over } else { Step::Enter }],
            entered: true,
            ends: false,
            dropped: false,
            pass: false,
        },
        MouseAction::Leave => Plan {
            steps: leave,
            entered: false,
            ends: false,
            dropped: false,
            pass: false,
        },
        MouseAction::Up {
            button: Button::Left, ..
        } => Plan {
            steps: if entered { vec![Step::Drop] } else { Vec::new() },
            entered: false,
            ends: true,
            dropped: entered,
            pass: false,
        },
        // Another button, or a press: the drag lost its release. It ends, and the event acts as usual.
        MouseAction::Down { .. } | MouseAction::Up { .. } => Plan {
            steps: leave,
            entered: false,
            ends: true,
            dropped: false,
            pass: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    const LEFT_UP: MouseAction = MouseAction::Up {
        button: Button::Left,
        clicks: 1,
    };

    #[test]
    fn a_drag_enters_with_the_first_move_and_drops_on_the_release() {
        let first = plan(false, MouseAction::Move);
        assert_eq!(
            (first.steps.as_slice(), first.entered, first.ends),
            (&[Step::Enter][..], true, false)
        );
        let next = plan(true, MouseAction::Move);
        assert_eq!(next.steps, vec![Step::Over]);
        let release = plan(true, LEFT_UP);
        assert_eq!(
            release,
            Plan {
                steps: vec![Step::Drop],
                entered: false,
                ends: true,
                dropped: true,
                pass: false
            }
        );
    }

    #[test]
    fn a_drag_off_the_page_leaves_it_and_a_release_there_drops_nothing() {
        let off = plan(true, MouseAction::Leave);
        assert_eq!(
            (off.steps.as_slice(), off.entered, off.ends),
            (&[Step::Leave][..], false, false)
        );
        // Released off the page (`on_mouse_up_out`): the drag ends with no drop.
        let release = plan(false, LEFT_UP);
        assert_eq!((release.steps.len(), release.ends, release.dropped), (0, true, false));
        // Back over the page: it enters again.
        assert_eq!(plan(false, MouseAction::Move).steps, vec![Step::Enter]);
    }

    #[test]
    fn a_press_ends_the_drag_and_still_reaches_the_page() {
        let press = plan(
            true,
            MouseAction::Down {
                button: Button::Right,
                clicks: 1,
            },
        );
        assert_eq!(
            (press.steps.as_slice(), press.ends, press.dropped, press.pass),
            (&[Step::Leave][..], true, false, true)
        );
        let other = plan(
            false,
            MouseAction::Up {
                button: Button::Middle,
                clicks: 1,
            },
        );
        assert!(other.ends && other.pass && !other.dropped);
    }

    #[test]
    fn a_masks_bits_come_back_as_they_went_in() {
        for value in [0, 1, 2, 16, 1 | 2 | 16, u32::MAX >> 1] {
            assert_eq!(bits(&mask(value)), value);
        }
    }
}
