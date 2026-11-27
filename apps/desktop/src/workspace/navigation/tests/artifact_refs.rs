//! A Nexus page referenced in a sent message (`artifact_refs.rs`) shows as a chip in the transcript, and a click opens
//! it in the Workbench by the same path the Library and a page card take.
use super::{BackendProbe, View, fixture};
use crate::artifact_refs::{ArtifactRef, Reader, Target, with_block};
use crate::model::{Message, Meta, Role, Stored};
use crate::workspace::transcript::Entry;
use gpui_kit::{Modifiers, TestAppContext, VisualTestContext};

fn draw(cx: &mut VisualTestContext) {
    for _ in 0..3 {
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.simulate_next_frame(cx);
        });
        cx.run_until_parked();
    }
}

fn page() -> ArtifactRef {
    ArtifactRef {
        title: "Fixture plan".into(),
        kind: "Site".into(),
        target: Target::Page("pag_Fixture0Page".into()),
        version: Some(1),
        link: None,
    }
}

#[gpui_kit::test]
fn a_page_referenced_in_a_sent_message_opens_in_the_workbench_on_click(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.new_chat(window, cx);
        assert!(matches!(workspace.view, View::Chat));
        // As Gateway gives the question back: the person's text, then the block the agent read.
        let content = with_block("Read the plan", &[page()], Reader::Chat);
        let message = Message {
            role: Role::User,
            content,
            page: None,
            meta: Meta::default(),
            web_sources: Vec::new(),
            stored: Stored::default(),
        };
        let entry = Entry::from_message(0, message, cx);
        assert_eq!(entry.text, "Read the plan", "the block shows as a chip, not as text");
        workspace.entries.push(entry);
        cx.notify();
    });
    draw(cx);
    let chip = cx
        .debug_bounds("artifact-chip")
        .expect("the referenced page shows as a chip in the transcript");
    cx.simulate_click(chip.center(), Modifiers::none());
    assert_eq!(
        backend.frames.lock().expect("frames").as_slice(),
        ["page pag_Fixture0Page v1 /app/org-a/pages/pag_Fixture0Page"],
        "a click opens the page, at the version it was referenced at, as the Library and page cards do"
    );
    workspace.read_with(cx, |workspace, _| {
        assert_eq!(
            workspace.page_opening,
            Some(("pag_Fixture0Page".to_string(), 1)),
            "the Workbench's Artifacts tab waits for that version"
        );
        assert!(!workspace.page_opening_auto, "the person opened it, so it shows");
    });
}
