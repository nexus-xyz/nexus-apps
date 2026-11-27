//! The displays and the pointer, from Core Graphics, which any thread may call and which needs no permission: the
//! display the pointer is on (the one recorded) and, about 60 times a second while recording, where the pointer is and
//! whether Shift and the left button are down (src/gesture.rs reads them). The keyboard is never read: that
//! would need the Accessibility or Input Monitoring permission. macOS only.
#![allow(unsafe_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use objc2_core_foundation::{CGPoint, CGRect};
use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayBounds, CGError, CGEvent, CGEventFlags, CGEventSource, CGEventSourceStateID,
    CGGetActiveDisplayList, CGMainDisplayID, CGMouseButton,
};

use super::gesture::Sample;

/// A display, in Core Graphics' global points (from the main display's top-left corner, y down).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Screen {
    /// `screencapture -D`'s number: 1 is the main display, then the others in Core Graphics' order.
    pub index: u32,
    pub bounds: CGRect,
    /// The main display's height, to turn these points into AppKit's (from its bottom-left corner, y up).
    pub main_height: f64,
}

fn contains(rect: CGRect, point: CGPoint) -> bool {
    point.x >= rect.origin.x
        && point.x < rect.origin.x + rect.size.width
        && point.y >= rect.origin.y
        && point.y < rect.origin.y + rect.size.height
}

pub fn pointer() -> CGPoint {
    let event = CGEvent::new(None);
    CGEvent::location(event.as_deref())
}

/// The display the pointer is on (the main display when it is on none).
pub fn under_pointer() -> Option<Screen> {
    let mut ids: [CGDirectDisplayID; 16] = [0; 16];
    let mut count = 0u32;
    // SAFETY: room for 16 displays, and a count to fill in.
    let error = unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count) };
    if error != CGError::Success || count == 0 {
        return None;
    }
    let ids = &ids[..count as usize];
    let at = pointer();
    // The active list starts with the main display, as `screencapture -D 1` does.
    let index = ids
        .iter()
        .position(|id| contains(CGDisplayBounds(*id), at))
        .unwrap_or(0);
    Some(Screen {
        index: index as u32 + 1,
        bounds: CGDisplayBounds(ids[index]),
        main_height: CGDisplayBounds(CGMainDisplayID()).size.height,
    })
}

/// Read the pointer, Shift and the left button every 16 ms until `stop`, handing each reading (in points from the
/// display's top-left corner, seconds since `began`) to `take`.
pub fn sample(screen: Screen, began: Instant, stop: Arc<AtomicBool>, mut take: impl FnMut(Sample)) {
    let state = CGEventSourceStateID::CombinedSessionState;
    while !stop.load(Ordering::SeqCst) {
        let at = pointer();
        take(Sample {
            t: began.elapsed().as_secs_f64(),
            at: [at.x - screen.bounds.origin.x, at.y - screen.bounds.origin.y],
            shift: CGEventSource::flags_state(state).contains(CGEventFlags::MaskShift),
            button: CGEventSource::button_state(state, CGMouseButton::Left),
        });
        std::thread::sleep(Duration::from_millis(16));
    }
}
