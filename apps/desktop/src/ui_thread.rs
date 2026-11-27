//! Which thread draws the windows. GPUI runs every frame, spinners included, on that one thread, so a store scan or a
//! parse there freezes the app. Debug builds assert that scans stay off it (`CodeReader`).

use std::cell::Cell;

thread_local! {
    static UI: Cell<bool> = const { Cell::new(false) };
}

/// Marks the calling thread as the UI thread. Call it once, from the application's run closure.
pub fn mark() {
    UI.with(|ui| ui.set(true));
}

/// Whether the calling thread is the UI thread. Tests never mark one, so this is false there.
pub fn is_current() -> bool {
    UI.with(Cell::get)
}

/// Panics in a debug build when called on the UI thread: `what` must run on the background executor.
#[track_caller]
pub fn assert_off(what: &str) {
    debug_assert!(
        !is_current(),
        "{what} ran on the UI thread: run it on the background executor"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_marked_thread_is_the_ui_thread() {
        std::thread::spawn(|| {
            assert!(!is_current());
            assert_off("a read");
            mark();
            assert!(is_current());
        })
        .join()
        .unwrap();
        assert!(!is_current());
    }

    #[test]
    #[cfg(debug_assertions)]
    fn a_scan_on_the_ui_thread_panics_in_debug() {
        let panicked = std::thread::spawn(|| {
            mark();
            assert_off("a scan");
        })
        .join();
        assert!(panicked.is_err());
    }
}
