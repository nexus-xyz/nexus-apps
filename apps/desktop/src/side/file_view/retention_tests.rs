//! Per-instance deferred reads and real FileView continuations, without a user store or editor process.
use super::{Body, Clip, FileView, FileViewState, State, VideoFile};
use crate::{
    palette::Palette,
    side::{Place, file_lifetime::Guard},
    web::WebView,
};
use futures::channel::oneshot;
use gpui_kit::component::input::EditorState;
use gpui_kit::{AppContext, Context, IntoElement, Render, TestAppContext, Window, WindowAppearance, div};
use nexus_workbench::output_files::{self, Preview};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
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
fn place(root: &std::path::Path) -> Place {
    Place::Here(Ok(crate::code::SessionFolder {
        path: root.into(),
        base: None,
    }))
}
fn preview(root: &std::path::Path, path: &str) -> Result<(Preview, bool), String> {
    output_files::read(root, path).map(|read| (read, true))
}

#[gpui_kit::test]
fn held_preview_checks_frozen_authority_before_constructing_a_body(cx: &mut TestAppContext) {
    init(cx);
    let dir = TempDir::new();
    std::fs::write(dir.path().join("note.md"), "# Original source").unwrap();
    let epoch = Arc::new(AtomicU64::new(1));
    let captured = epoch.clone();
    let guard: Guard = Arc::new(move || captured.load(Ordering::SeqCst) == 1);
    let (send, pending) = oneshot::channel();
    let (host, cx) = cx.add_window_view(|_, _| Host);
    let view = host.update_in(cx, |_, window, cx| {
        cx.new(|cx| {
            FileView::fixture_preview(
                place(dir.path()),
                FileViewState {
                    path: "note.md".into(),
                    ..Default::default()
                },
                guard,
                pending,
                window,
                cx,
            )
        })
    });
    cx.run_until_parked();
    view.read_with(cx, |view, _| assert!(matches!(view.state, State::Opening)));
    epoch.store(3, Ordering::SeqCst); // Same visible account can return; this accepted proof cannot.
    assert!(send.send(preview(dir.path(), "note.md")).is_ok());
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert!(!view.is_current());
        assert!(matches!(view.state, State::Opening));
    });
    let fresh = host.update_in(cx, |_, window, cx| {
        cx.new(|cx| FileView::new(place(dir.path()), "note.md".into(), window, cx))
    });
    cx.run_until_parked();
    fresh.read_with(cx, |view, _| match &view.state {
        State::Ready {
            preview,
            body: Body::Markdown { .. },
        } => assert_eq!(preview.text.as_deref(), Some("# Original source")),
        _ => panic!("fresh authorized preview must construct its actual Markdown view"),
    });
}

#[gpui_kit::test]
fn retiring_a_strong_old_view_keeps_selection_but_releases_body_and_blocks_actions(cx: &mut TestAppContext) {
    init(cx);
    let dir = TempDir::new();
    std::fs::write(dir.path().join("source.rs"), "first\nsecond\nthird").unwrap();
    let (host, cx) = cx.add_window_view(|_, _| Host);
    let original = FileViewState {
        path: "source.rs".into(),
        line: Some(2),
        source: true,
        zoom: true,
        pdf_page: None,
    };
    let view = host.update_in(cx, |_, window, cx| {
        cx.new(|cx| FileView::restore(place(dir.path()), original.clone(), Arc::new(|| true), window, cx))
    });
    cx.run_until_parked();
    let saved = view.update(cx, |view, cx| {
        assert!(matches!(
            view.state,
            State::Ready {
                body: Body::Code(_),
                ..
            }
        ));
        let saved = view.snapshot();
        view.retire(cx);
        saved
    });
    assert_eq!(saved, original);
    let records = Arc::new(Mutex::new(Vec::new()));
    let record = records.clone();
    let fixture = crate::editor_choice::Fixture {
        load: Arc::new(|| panic!("a retired view must not even discover editors")),
        launch: Arc::new(move |request, _| {
            record.lock().unwrap().push(request);
            Ok(())
        }),
    };
    view.update_in(cx, |view, window, cx| {
        view.fixture_editor(fixture);
        view.open_in_editor(cx);
        view.read(window, cx);
        view.copy_path(cx);
        view.open_in_app(cx);
        view.open_line(Some(9));
        assert!(view.is_retired());
        assert!(matches!(view.state, State::Failed(_)));
        assert!(view.notice.is_none());
        assert_eq!(view.snapshot().line, Some(2));
    });
    cx.run_until_parked();
    assert!(records.lock().unwrap().is_empty());
    let fresh = host.update_in(cx, |_, window, cx| {
        cx.new(|cx| FileView::restore(place(dir.path()), saved, Arc::new(|| true), window, cx))
    });
    cx.run_until_parked();
    assert_ne!(fresh.entity_id(), view.entity_id());
    fresh.read_with(cx, |view, _| {
        assert!(view.is_current());
        assert_eq!(view.snapshot(), original);
        assert!(matches!(view.state, State::Ready { .. }));
    });
}

#[gpui_kit::test]
fn editor_dispatch_freezes_folder_path_line_and_reads_current_per_instance_preference(cx: &mut TestAppContext) {
    init(cx);
    let dir = TempDir::new();
    std::fs::write(dir.path().join("nested file.rs"), "first\nsecond").unwrap();
    let choice = Arc::new(Mutex::new(Some("zed".to_string())));
    let records = Arc::new(Mutex::new(Vec::new()));
    let selected = choice.clone();
    let record = records.clone();
    let fixture = crate::editor_choice::Fixture {
        load: Arc::new(move || {
            (
                vec![
                    nexus_workbench::editors::Editor {
                        id: "zed",
                        name: "Zed",
                        kind: "editor",
                    },
                    nexus_workbench::editors::Editor {
                        id: "code",
                        name: "Code",
                        kind: "editor",
                    },
                ],
                selected.lock().unwrap().clone(),
                String::new(),
            )
        }),
        launch: Arc::new(move |request, valid| {
            assert!(valid());
            record.lock().unwrap().push(request);
            Ok(())
        }),
    };
    let (host, cx) = cx.add_window_view(|_, _| Host);
    let view = host.update_in(cx, |_, window, cx| {
        cx.new(|cx| {
            FileView::restore(
                place(dir.path()),
                FileViewState {
                    path: "nested file.rs".into(),
                    line: Some(2),
                    source: true,
                    ..Default::default()
                },
                Arc::new(|| true),
                window,
                cx,
            )
        })
    });
    cx.run_until_parked();
    view.update(cx, |view, cx| {
        view.fixture_editor(fixture);
        view.open_in_editor(cx);
    });
    cx.run_until_parked();
    *choice.lock().unwrap() = Some("code".into());
    view.update(cx, |view, cx| view.open_in_editor(cx));
    cx.run_until_parked();
    let records = records.lock().unwrap();
    assert_eq!(records.len(), 2);
    for record in records.iter() {
        assert_eq!(record.root, dir.path());
        assert_eq!(record.file, "nested file.rs");
        assert_eq!(record.line, Some(2));
    }
    assert_eq!(records[0].editor.as_deref(), Some("zed"));
    assert_eq!(records[1].editor.as_deref(), Some("code"));
}

#[gpui_kit::test]
fn editor_queued_before_retirement_cannot_discover_or_launch_afterward(cx: &mut TestAppContext) {
    init(cx);
    let dir = TempDir::new();
    std::fs::write(dir.path().join("source.rs"), "text").unwrap();
    let (host, cx) = cx.add_window_view(|_, _| Host);
    let view = host.update_in(cx, |_, window, cx| {
        cx.new(|cx| FileView::new(place(dir.path()), "source.rs".into(), window, cx))
    });
    cx.run_until_parked();
    view.update(cx, |view, cx| {
        assert!(matches!(view.state, State::Ready { .. }));
        view.fixture_editor(crate::editor_choice::Fixture {
            load: Arc::new(|| panic!("queued retired work must refuse before preference discovery")),
            launch: Arc::new(|_, _| panic!("queued retired work must not launch")),
        });
        view.open_in_editor(cx);
        view.retire(cx);
    });
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert!(view.is_retired());
        assert!(view.notice.is_none());
    });
}

#[gpui_kit::test]
fn held_video_result_cannot_create_a_player_after_authority_loss(cx: &mut TestAppContext) {
    init(cx);
    let dir = TempDir::new();
    let path = dir.path().join("clip.mp4");
    std::fs::write(&path, b"isolated fixture video").unwrap();
    let live = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let current = live.clone();
    let (send, pending) = oneshot::channel();
    let (host, cx) = cx.add_window_view(|_, _| Host);
    let view = host.update_in(cx, |_, window, cx| {
        cx.new(|cx| {
            let mut view = FileView::unread(
                place(dir.path()),
                FileViewState {
                    path: "clip.mp4".into(),
                    ..Default::default()
                },
                Arc::new(move || current.load(Ordering::SeqCst)),
            );
            view.pending_video = Some(pending);
            view.read(window, cx);
            view
        })
    });
    cx.run_until_parked();
    view.read_with(cx, |view, _| assert!(matches!(view.state, State::Video(Clip::Loading))));
    live.store(false, Ordering::SeqCst);
    assert!(send.send(Ok(VideoFile { path, copy: false })).is_ok());
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert!(
            matches!(view.state, State::Video(Clip::Loading)),
            "revoked result must not create a native player"
        )
    });
    let (send, pending) = oneshot::channel();
    let fresh = host.update_in(cx, |_, window, cx| {
        cx.new(|cx| {
            let mut view = FileView::unread(
                place(dir.path()),
                FileViewState {
                    path: "clip.mp4".into(),
                    ..Default::default()
                },
                Arc::new(|| true),
            );
            view.pending_video = Some(pending);
            view.read(window, cx);
            view
        })
    });
    cx.run_until_parked();
    assert!(send.send(Err(Clip::Failed("expected decoder error".into()))).is_ok());
    cx.run_until_parked();
    fresh.read_with(cx, |view, _| {
        assert!(
            matches!(&view.state, State::Video(Clip::Failed(error)) if error.as_ref() == "expected decoder error"),
            "current video completion must still publish its result"
        )
    });
}

#[gpui_kit::test]
fn held_pdf_draw_cannot_publish_after_revocation_and_a_fresh_draw_still_works(cx: &mut TestAppContext) {
    init(cx);
    let dir = TempDir::new();
    let bytes = super::super::pdf::tests::pdf(3);
    std::fs::write(dir.path().join("pages.pdf"), &bytes).unwrap();
    let live = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let current = live.clone();
    let (host, cx) = cx.add_window_view(|_, _| Host);
    let view = host.update_in(cx, |_, window, cx| {
        cx.new(|cx| {
            FileView::restore(
                place(dir.path()),
                FileViewState {
                    path: "pages.pdf".into(),
                    pdf_page: Some(2),
                    ..Default::default()
                },
                Arc::new(move || current.load(Ordering::SeqCst)),
                window,
                cx,
            )
        })
    });
    cx.run_until_parked();
    let (send, pending) = oneshot::channel();
    view.update_in(cx, |view, window, cx| {
        view.width = 330.;
        view.pending_pdf = Some(pending);
        view.draw_pdf(window, cx);
    });
    cx.run_until_parked();
    live.store(false, Ordering::SeqCst);
    assert!(
        send.send(super::super::pdf::draw(Arc::new(bytes.clone()), 1, 306., 1.))
            .is_ok()
    );
    cx.run_until_parked();
    view.read_with(cx, |view, _| match &view.state {
        State::Ready {
            body: Body::Pdf(pdf), ..
        } => {
            assert!(pdf.drawn.is_none());
            assert!(pdf.pages.is_none());
        }
        _ => panic!("original loaded PDF"),
    });
    let fresh = host.update_in(cx, |_, window, cx| {
        cx.new(|cx| {
            FileView::restore(
                place(dir.path()),
                FileViewState {
                    path: "pages.pdf".into(),
                    pdf_page: Some(99),
                    ..Default::default()
                },
                Arc::new(|| true),
                window,
                cx,
            )
        })
    });
    cx.run_until_parked();
    fresh.update_in(cx, |view, window, cx| {
        view.width = 330.;
        view.draw_pdf(window, cx);
    });
    cx.run_until_parked();
    fresh.read_with(cx, |view, _| match &view.state {
        State::Ready {
            body: Body::Pdf(pdf), ..
        } => {
            assert_eq!(pdf.page, 3, "restored page clamps to actual document");
            assert!(pdf.drawn.is_none());
        }
        _ => panic!("fresh loaded PDF"),
    });
    fresh.update_in(cx, |view, window, cx| view.draw_pdf(window, cx));
    cx.run_until_parked();
    fresh.read_with(cx, |view, _| match &view.state {
        State::Ready {
            body: Body::Pdf(pdf), ..
        } => {
            assert!(pdf.drawn.is_some());
            assert_eq!(pdf.page, 3);
            assert_eq!(pdf.pages, Some(3));
        }
        _ => panic!("fresh drawn PDF"),
    });
}

#[gpui_kit::test]
fn retiring_an_html_preview_releases_its_actual_cef_owner_even_with_old_child_handles(cx: &mut TestAppContext) {
    init(cx);
    let dir = TempDir::new();
    std::fs::write(dir.path().join("document.html"), "<p>Original source</p>").unwrap();
    let (saved, probe) = crate::web::fixture_page(78559);
    let (host, cx) = cx.add_window_view(|_, _| Host);
    let (view, old_page) = host.update_in(cx, |_, window, cx| {
        let page = cx.new(|cx| WebView::restore(saved, window, cx));
        let old_page = page.clone();
        let view = cx.new(|cx| {
            let mut view = FileView::unread(
                place(dir.path()),
                FileViewState {
                    path: "document.html".into(),
                    ..Default::default()
                },
                Arc::new(|| true),
            );
            let source = cx.new(|cx| EditorState::new(window, cx));
            view.state = State::Ready {
                preview: output_files::read(dir.path(), "document.html").unwrap(),
                body: Body::Html { page, source },
            };
            view
        });
        (view, old_page)
    });
    assert!(probe.alive());
    view.update(cx, |view, cx| view.retire(cx));
    cx.run_until_parked();
    assert!(
        !probe.alive(),
        "retired native document ownership must not survive an old entity handle"
    );
    assert!(!old_page.read_with(cx, |page, _| page.can_transfer()));
    view.read_with(cx, |view, _| {
        assert!(view.is_retired() && matches!(view.state, State::Failed(_)))
    });
}
