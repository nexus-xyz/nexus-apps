//! History keeps its consent in the shared settings file. This epoch only invalidates displayed and pending reads.

use std::path::Path;
use std::rc::{Rc, Weak};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use file_watch::{RecommendedWatcher, RecursiveMode, Watcher};
use futures::{
    StreamExt as _,
    channel::mpsc::{UnboundedSender, unbounded},
};
use gpui_kit::*;

use super::{HistoryRevision, read_ticket};

pub(super) struct SettingsWatch {
    _watcher: RecommendedWatcher,
    _task: Task<()>,
}

/// Pages own the watcher. The global only shares it, and the last closed page stops watching.
struct Watching(Weak<SettingsWatch>);
impl Global for Watching {}

pub(super) fn watch_settings(cx: &mut App) -> Option<Rc<SettingsWatch>> {
    watch_file(crate::desktop_settings::file()?, cx)
}

fn watch_file(file: std::path::PathBuf, cx: &mut App) -> Option<Rc<SettingsWatch>> {
    if cx.has_global::<Watching>()
        && let Some(watcher) = cx.global::<Watching>().0.upgrade()
    {
        return Some(watcher);
    }
    let dir = file.parent()?.to_path_buf();
    let (signal, task) = listen(cx);
    let mut watcher = file_watch::recommended_watcher(move |event: file_watch::Result<file_watch::Event>| {
        if match event {
            Ok(event) => affects_settings(&event, &file),
            Err(_) => true,
        } {
            signal.notify();
        }
    })
    .ok()?;
    // Watch the directory: replacing settings.json atomically must not detach the next write's watcher.
    watcher.watch(&dir, RecursiveMode::NonRecursive).ok()?;
    let watcher = Rc::new(SettingsWatch {
        _watcher: watcher,
        _task: task,
    });
    cx.set_global(Watching(Rc::downgrade(&watcher)));
    Some(watcher)
}

fn affects_settings(event: &file_watch::Event, file: &Path) -> bool {
    event.need_rescan() || (!event.kind.is_access() && event.paths.iter().any(|path| path == file))
}

/// Invalidate immediately on the watcher thread, before an old scan can finish on the UI executor.
pub(super) struct ChangeSignal {
    revision: Arc<AtomicU64>,
    sender: UnboundedSender<()>,
}
impl ChangeSignal {
    pub fn notify(&self) {
        self.revision.fetch_add(1, Ordering::SeqCst);
        let _ = self.sender.unbounded_send(());
    }
}

/// Every write invalidates, even if an off -> on change finishes before the UI reads the final saved preference.
pub(super) fn listen(cx: &mut App) -> (ChangeSignal, Task<()>) {
    let revision = read_ticket(cx).revision;
    let (sender, mut changes) = unbounded();
    let signal = ChangeSignal {
        revision: revision.clone(),
        sender,
    };
    let task = cx.spawn(async move |cx| {
        while changes.next().await.is_some() {
            while changes.try_recv().is_ok() {}
            cx.update(|cx| cx.set_global(HistoryRevision(revision.clone())));
        }
    });
    (signal, task)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    use file_watch::event::{AccessKind, ModifyKind, RenameMode};

    #[gpui_kit::test]
    fn the_last_page_releases_the_shared_settings_watcher(cx: &mut TestAppContext) {
        let file = crate::desktop_settings::TempFile::new("history-watcher-lifetime");
        file.update(|settings| {
            settings.local_session_history = Some(false);
            Ok(())
        })
        .unwrap();
        let (first, second) = cx.update(|cx| {
            (
                watch_file(file.path().to_path_buf(), cx).expect("watch isolated settings directory"),
                watch_file(file.path().to_path_buf(), cx).expect("reuse watcher"),
            )
        });
        assert!(Rc::ptr_eq(&first, &second));
        let weak = Rc::downgrade(&first);
        drop(first);
        assert!(
            weak.upgrade().is_some(),
            "another history or privacy page still owns the watcher"
        );
        drop(second);
        assert!(
            weak.upgrade().is_none(),
            "the app global must not keep a closed page's watcher alive"
        );
        let fresh = cx.update(|cx| watch_file(file.path().to_path_buf(), cx).expect("reopen watcher"));
        assert!(weak.upgrade().is_none());
        drop(fresh);
    }

    #[test]
    fn only_saved_settings_changes_invalidate_history() {
        let file = Path::new("/tmp/nexus-history/settings.json");
        let write = file_watch::Event::new(file_watch::EventKind::Modify(ModifyKind::Any)).add_path(file.into());
        assert!(affects_settings(&write, file));
        let rename = file_watch::Event::new(file_watch::EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
            .add_path(file.with_extension("tmp"))
            .add_path(file.into());
        assert!(
            affects_settings(&rename, file),
            "atomic replacement also revokes old reads"
        );
        let read = file_watch::Event::new(file_watch::EventKind::Access(AccessKind::Any)).add_path(file.into());
        assert!(
            !affects_settings(&read, file),
            "reading consent must not trigger another read"
        );
        let unrelated = file_watch::Event::new(file_watch::EventKind::Modify(ModifyKind::Any))
            .add_path(file.with_file_name("other.json"));
        assert!(!affects_settings(&unrelated, file));
    }
}
