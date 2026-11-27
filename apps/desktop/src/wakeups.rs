//! What keeps the app from waking while nothing changes: the Code store's change signal, and how often the
//! polls that remain run.

use std::time::Duration;

use crate::gpui::{App, AsyncApp};
use file_watch::{RecommendedWatcher, RecursiveMode, Watcher};
use futures::channel::mpsc::{UnboundedReceiver, unbounded};

/// How often a poll runs while no Nexus window has the focus. A window that gets the focus reads again at once
/// (`Workspace::activated`), so this only bounds how stale a window nobody is looking at gets.
pub const UNFOCUSED: Duration = Duration::from_secs(30);

/// `focused` while a Nexus window has the focus, else `UNFOCUSED`.
pub fn pace(focused: Duration, cx: &AsyncApp) -> Duration {
    cx.update(|cx| paced(focused, cx))
}

fn paced(focused: Duration, cx: &App) -> Duration {
    if cx.active_window().is_some() {
        focused
    } else {
        focused.max(UNFOCUSED)
    }
}

/// A signal each time the Code store's files change on disk (`workbench.sqlite3` and its `-wal`), whoever writes them.
/// The watcher stops when it is dropped. `None` when the folder can't be watched: the caller keeps polling.
pub fn store_changes() -> Option<(RecommendedWatcher, UnboundedReceiver<()>)> {
    let dir = crate::paths::data_dir()?;
    let (tx, rx) = unbounded();
    let mut watcher = file_watch::recommended_watcher(move |event: file_watch::Result<file_watch::Event>| {
        let Ok(event) = event else {
            return;
        };
        let store = event.paths.iter().any(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("workbench.sqlite3"))
        });
        if store && !event.kind.is_access() {
            let _ = tx.unbounded_send(());
        }
    })
    .ok()?;
    watcher.watch(&dir, RecursiveMode::NonRecursive).ok()?;
    Some((watcher, rx))
}
