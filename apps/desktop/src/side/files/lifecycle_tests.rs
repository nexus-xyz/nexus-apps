//! Real Files entities retain only plain state and reject delayed read publication.
use super::{Files, FilesState};
use crate::{
    palette::Palette,
    side::{Place, file_lifetime::Guard, file_view::FileViewState},
};
use futures::channel::oneshot;
use gpui_kit::{AppContext, Context, IntoElement, Render, TestAppContext, Window, WindowAppearance, div};
use nexus_workbench::files;
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
/// Own only this fixture's UUID directory; never touch application stores or settings.
struct TempDir(std::path::PathBuf);
impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("nexus files fixture {}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).expect("create isolated fixture directory");
        Self(path)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Host;
impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}
fn init(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(|cx| {
        cx.set_global(crate::pages::Theme(Palette::for_choice(
            crate::palette::ThemeChoice::Light,
            WindowAppearance::Light,
        )))
    });
}
fn place(root: &Path) -> Place {
    Place::Here(Ok(crate::code::SessionFolder {
        path: root.into(),
        base: None,
    }))
}
#[gpui_kit::test]
fn a_held_list_cannot_publish_after_source_authority_changes(cx: &mut TestAppContext) {
    init(cx);
    let dir = TempDir::new();
    std::fs::write(dir.path().join("original.rs"), "text").unwrap();
    let epoch = Arc::new(AtomicU64::new(1));
    let captured = epoch.clone();
    let guard: Guard = Arc::new(move || captured.load(Ordering::SeqCst) == 1);
    let (send, pending) = oneshot::channel();
    let (host, cx) = cx.add_window_view(|_, _| Host);
    let view = host.update_in(cx, |_, window, cx| {
        cx.new(|cx| Files::fixture_list(place(dir.path()), FilesState::default(), guard, pending, window, cx))
    });
    cx.run_until_parked();
    view.read_with(cx, |view, _| assert!(view.loading && view.files.is_empty()));
    epoch.store(3, Ordering::SeqCst);
    let list = files::list(dir.path()).map(|list| (list.files, list.truncated));
    assert!(send.send((list, HashMap::new())).is_ok());
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert!(!view.is_current());
        assert!(view.files.is_empty());
        assert!(view.rows.is_empty());
    });
}
#[gpui_kit::test]
fn plain_snapshot_recreates_filter_and_open_file_and_retires_strong_old_entities(cx: &mut TestAppContext) {
    init(cx);
    let dir = TempDir::new();
    std::fs::write(dir.path().join("file.rs"), "one\ntwo").unwrap();
    let (host, cx) = cx.add_window_view(|_, _| Host);
    let state = FilesState {
        query: "file".into(),
        all: true,
        expanded: HashSet::from(["src".into()]),
        scroll_top: 0,
        open: Some(FileViewState {
            path: "file.rs".into(),
            line: Some(2),
            source: true,
            zoom: true,
            pdf_page: None,
        }),
    };
    let view = host.update_in(cx, |_, window, cx| {
        cx.new(|cx| Files::restore(place(dir.path()), state.clone(), Arc::new(|| true), window, cx))
    });
    cx.run_until_parked();
    let (old_filter, old_child) = view.read_with(cx, |view, _| {
        (
            view.filter.clone().expect("filter"),
            view.open.as_ref().expect("preview").0.clone(),
        )
    });
    let saved = view.update(cx, |view, cx| view.retain(cx));
    assert_eq!(saved, state);
    view.update_in(cx, |view, window, cx| {
        view.open_at("new.rs".into(), None, window, cx);
        view.refresh(cx);
        assert!(view.is_retired());
        assert!(view.open.is_none());
        assert!(view.filter.is_none());
    });
    old_child.read_with(cx, |view, _| assert!(view.is_retired()));
    let fresh = host.update_in(cx, |_, window, cx| {
        cx.new(|cx| Files::restore(place(dir.path()), saved, Arc::new(|| true), window, cx))
    });
    cx.run_until_parked();
    fresh.read_with(cx, |view, cx| {
        assert!(view.is_current());
        assert_eq!(view.snapshot(cx), state);
        assert_ne!(view.filter.as_ref().unwrap().entity_id(), old_filter.entity_id());
        assert_eq!(view.filter.as_ref().unwrap().read(cx).value().as_ref(), "file");
        assert_ne!(view.open.as_ref().unwrap().0.entity_id(), old_child.entity_id());
    });
}
