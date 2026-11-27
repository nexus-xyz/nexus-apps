//! Images sent with a linked session's message show under it, and a click opens them full size, as the Nexus app's
//! `session-message.tsx` (`MessageImages`). Headless: the fixture of `tool_row_editor_tests.rs`, with one linked message.

use super::*;
use crate::code::Transcript;
use crate::workspace::code_view::CodeView;
use crate::workspace::machine_source::MachineSource;
use crate::workspace::overlay::OverlaySlot;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use core::prelude::v1::test;
use nexus_workbench::event::{Image, Sender, WbEvent};
use std::sync::Arc;

const SESSION: &str = "linked-images";

fn png() -> Image {
    let mut bytes = std::io::Cursor::new(Vec::new());
    ::image::DynamicImage::new_rgb8(32, 20)
        .write_to(&mut bytes, ::image::ImageFormat::Png)
        .unwrap();
    Image {
        media_type: "image/png".into(),
        data: STANDARD.encode(bytes.get_ref()),
        name: Some("chart.png".into()),
    }
}

fn fixture(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(gpui_kit::init);
    let (gateway, _) = crate::gateway::Gateway::start();
    let (_, events) = futures::channel::mpsc::unbounded();
    let (_, arrivals) = futures::channel::mpsc::unbounded();
    let (workspace, cx) = cx.add_window_view(|window, cx| {
        let mut workspace = Workspace::with_machine_source(
            Arc::new(gateway),
            events,
            arrivals,
            false,
            MachineSource::isolated(),
            window,
            cx,
        );
        workspace.auth = crate::workspace::Auth::SignedIn;
        workspace._code_poll = Task::ready(());
        workspace.runner = None;
        workspace.code = None;
        let mut view = CodeView::new(SESSION.into(), window, cx);
        view.loaded = true;
        workspace.view = View::Code(view);
        workspace.side.fixture_session(SESSION);
        workspace
    });
    workspace.update_in(cx, |_, window, _| window.activate_window());
    cx.run_until_parked();
    let transcript = Transcript::of(vec![(
        1,
        1,
        WbEvent::User {
            text: "Here are the two charts you asked for.".into(),
            images: vec![png(), png()],
            from: Some(Sender::Session {
                thread_id: "child".into(),
                title: "Chart builder".into(),
            }),
        },
    )]);
    workspace.update(cx, |workspace, cx| {
        let mut session = crate::workspace::session_keys::tests::session(SESSION);
        session.project_id = "proj".into();
        workspace.projects = vec![crate::code::CodeProject {
            id: "proj".into(),
            name: "proj".into(),
            branch: None,
            pinned: false,
            remotes: Vec::new(),
            sessions: vec![session],
            machine: None,
            added_at: 0,
        }];
        if let View::Code(view) = &mut workspace.view {
            view.set(transcript, cx);
        }
        cx.notify();
    });
    (workspace, cx)
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

#[gpui_kit::test]
fn a_linked_messages_images_show_under_it_and_open_full_size(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture(cx);
    cx.run_until_parked();
    draw(cx);
    let first = cx.debug_bounds("tool-image-0-0").expect("the first image shows");
    let second = cx.debug_bounds("tool-image-0-1").expect("the second image shows");
    assert!(first.right() <= second.left(), "the images sit side by side");
    assert!(workspace.read_with(cx, |workspace, _| workspace.overlay.image_viewer().is_none()));
    cx.simulate_click(second.center(), Modifiers::default());
    cx.run_until_parked();
    assert!(
        workspace.read_with(cx, |workspace, _| workspace.overlay.image_viewer().is_some()),
        "a click opens the image full size"
    );
}
