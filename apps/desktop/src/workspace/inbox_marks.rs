//! Read and Done for session notices, separate from Gateway's notification marks.

use std::collections::HashMap;
use std::path::PathBuf;

use gpui_kit::{App, BorrowAppContext, Global};
use nexus_workbench::session_notices::Marks;
use sha2::Digest;

#[derive(Default)]
struct NoticeStore(HashMap<String, Marks>);
impl Global for NoticeStore {}

fn file(scope: &str) -> Option<PathBuf> {
    if cfg!(test) || scope.starts_with("volatile:") || !crate::managed::local_cache(crate::managed::policy()) {
        return None;
    }
    let digest: String = sha2::Sha256::digest(scope.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    crate::paths::seen_file().map(|file| file.with_file_name(format!("inbox-sessions-{digest}.json")))
}

fn load(scope: &str, now: i64) -> Marks {
    file(scope)
        .and_then(|file| std::fs::read_to_string(file).ok())
        .map(|text| Marks::parse(&text, now))
        .unwrap_or_else(|| Marks::new(now))
}

fn save(scope: &str, marks: &Marks) {
    let Some(file) = file(scope) else { return };
    let Some(parent) = file.parent() else { return };
    let Ok(text) = serde_json::to_string(marks) else { return };
    let written = std::fs::create_dir_all(parent).and_then(|()| std::fs::write(file, text));
    if let Err(error) = written {
        eprintln!("nexus-desktop-gpui: Inbox marks could not be saved: {error}");
    }
}

pub(super) fn ensure(scope: Option<&str>, cx: &mut App) {
    let Some(scope) = scope else { return };
    if !cx.has_global::<NoticeStore>() {
        cx.set_global(NoticeStore::default());
    }
    if cx.global::<NoticeStore>().0.contains_key(scope) {
        return;
    }
    let marks = load(scope, chrono::Local::now().timestamp_millis());
    save(scope, &marks);
    cx.update_global::<NoticeStore, _>(|store, _| {
        store.0.insert(scope.into(), marks);
    });
}

pub(super) fn read(scope: Option<&str>, cx: &App) -> Marks {
    scope
        .and_then(|scope| cx.try_global::<NoticeStore>().and_then(|store| store.0.get(scope)))
        .cloned()
        .unwrap_or_else(|| Marks::new(chrono::Local::now().timestamp_millis()))
}

pub(super) fn mark(scope: Option<&str>, id: &str, done: bool, cx: &mut App) {
    mark_each(scope, &[id], done, cx);
}

/// Mark several notices with one read and one write of the file.
pub(super) fn mark_each(scope: Option<&str>, ids: &[&str], done: bool, cx: &mut App) {
    let Some(scope) = scope else { return };
    if ids.is_empty() {
        return;
    }
    ensure(Some(scope), cx);
    let now = chrono::Local::now().timestamp_millis();
    let kept = load(scope, now);
    cx.update_global::<NoticeStore, _>(|store, _| {
        let marks = store.0.entry(scope.into()).or_insert_with(|| Marks::new(now));
        marks.merge(&kept);
        for id in ids {
            marks.mark(id, done, now);
        }
        save(scope, marks);
    });
}
