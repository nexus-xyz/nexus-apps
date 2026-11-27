//! What happened to each line's fix, by its key: Ask was pressed, a terminal step runs or failed, and the blocked
//! actions that wait to resume when the fix is done. App-wide, so a page and the session that opened it share it.
//! A change redraws the windows: these are rare, one per press or per command's end.

use std::collections::HashMap;
use std::time::Instant;

use gpui_kit::{App, Global, Window};

use super::FixKey;

/// The Ask button was pressed: what it says now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asked {
    pub label: String,
    pub at: Instant,
}

/// A terminal step's state (`Fix::RunInTerminal`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Run {
    /// Running in the Workbench terminal: the line waits.
    Running,
    /// It ended with an error: the line shows the red ✕, the last line of output and Try again.
    Failed { code: Option<u32>, last: Option<String> },
    /// It runs where this window cannot see its end (the Commands window, detached): the line offers Check again.
    Unwatched,
}

type Resume = Box<dyn FnOnce(&mut Window, &mut App)>;

#[derive(Default)]
struct Fixes {
    asked: HashMap<FixKey, Asked>,
    runs: HashMap<FixKey, Run>,
    resumes: HashMap<FixKey, Vec<Resume>>,
}

impl Global for Fixes {}

/// What Ask said for `key`, once pressed.
pub fn asked(key: &FixKey, cx: &App) -> Option<Asked> {
    cx.try_global::<Fixes>()?.asked.get(key).cloned()
}

pub(crate) fn set_asked(key: FixKey, label: String, cx: &mut App) {
    cx.default_global::<Fixes>().asked.insert(
        key,
        Asked {
            label,
            at: Instant::now(),
        },
    );
    cx.refresh_windows();
}

/// The terminal step of `key`, while it runs or after it failed.
pub fn run_state(key: &FixKey, cx: &App) -> Option<Run> {
    cx.try_global::<Fixes>()?.runs.get(key).cloned()
}

pub(crate) fn set_run(key: FixKey, run: Run, cx: &mut App) {
    cx.default_global::<Fixes>().runs.insert(key, run);
    cx.refresh_windows();
}

/// Run `resume` when the fix of `key` is done ([`done`]): the blocked action goes on by itself (send the held message,
/// start the agent, open the link again). A call site registers it when the person presses its fix, and calls [`done`]
/// when it sees the fix landed (a settings change, a sign-in). A terminal step that exits 0 calls it itself.
pub fn on_done(key: impl Into<FixKey>, cx: &mut App, resume: impl FnOnce(&mut Window, &mut App) + 'static) {
    cx.default_global::<Fixes>()
        .resumes
        .entry(key.into())
        .or_default()
        .push(Box::new(resume));
}

/// Whether something waits on `key`'s fix.
pub fn waiting(key: &FixKey, cx: &App) -> bool {
    cx.try_global::<Fixes>()
        .is_some_and(|fixes| fixes.resumes.get(key).is_some_and(|list| !list.is_empty()))
}

/// The fix of `key` is done: what waited on it resumes, once, and the line's Ask and run state go.
pub fn done(key: &FixKey, window: &mut Window, cx: &mut App) {
    let fixes = cx.default_global::<Fixes>();
    fixes.asked.remove(key);
    fixes.runs.remove(key);
    let resumes = fixes.resumes.remove(key).unwrap_or_default();
    for resume in resumes {
        resume(window, cx);
    }
    cx.refresh_windows();
}

/// Drop what waits on `key` without running it: the person picked another way, or left.
pub fn forget(key: &FixKey, cx: &mut App) {
    let fixes = cx.default_global::<Fixes>();
    fixes.asked.remove(key);
    fixes.runs.remove(key);
    fixes.resumes.remove(key);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    #[gpui_kit::test]
    fn a_done_fix_resumes_once_and_forgets_its_state(cx: &mut gpui_kit::TestAppContext) {
        let window = cx.add_empty_window();
        let key: FixKey = "start:no-key".into();
        let resumed = Rc::new(Cell::new(0));
        window.update(|_, cx| {
            let count = resumed.clone();
            on_done(key.clone(), cx, move |_, _| count.set(count.get() + 1));
            set_run(key.clone(), Run::Running, cx);
            set_asked(key.clone(), "Asked · in Ana's inbox".into(), cx);
            assert!(waiting(&key, cx));
        });
        window.update(|window, cx| done(&key, window, cx));
        window.update(|window, cx| {
            assert_eq!(resumed.get(), 1);
            assert!(!waiting(&key, cx));
            assert_eq!(run_state(&key, cx), None);
            assert_eq!(asked(&key, cx), None);
            done(&key, window, cx);
        });
        assert_eq!(resumed.get(), 1, "a second done runs nothing");
    }

    #[gpui_kit::test]
    fn a_forgotten_fix_never_resumes(cx: &mut gpui_kit::TestAppContext) {
        let window = cx.add_empty_window();
        let key: FixKey = "pr:gh-auth".into();
        let resumed = Rc::new(Cell::new(false));
        window.update(|_, cx| {
            let flag = resumed.clone();
            on_done(key.clone(), cx, move |_, _| flag.set(true));
            forget(&key, cx);
        });
        window.update(|window, cx| done(&key, window, cx));
        assert!(!resumed.get());
    }
}
