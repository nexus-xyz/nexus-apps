use super::{Reply, chip_fill, chip_label, message, segments};
use crate::palette::{Palette, ThemeChoice};
use crate::workspace::{View, Workspace, code_view::CodeView, machine_source::MachineSource};
use gpui_kit::prelude::*;
use gpui_kit::{
    Context, Entity, Focusable, IntoElement, Modifiers, ParentElement, Render, Styled, TestAppContext,
    VisualTestContext, Window, WindowAppearance, div, point, px,
};

#[test]
fn a_chip_stands_apart_from_the_composer_in_both_themes() {
    for choice in [ThemeChoice::Light, ThemeChoice::Dark] {
        let palette = Palette::for_choice(choice, WindowAppearance::Light);
        // The composer's box is `raised`; the old chip fill (`hover`) was 0.008 off it in dark.
        let apart = (chip_fill(palette).l - palette.raised.l).abs();
        assert!(
            apart >= 0.025,
            "{choice:?}: the chip's fill is only {apart} off the composer's"
        );
    }
}

fn reply(id: u64, quote: &str, text: &str) -> Reply {
    Reply {
        id,
        quote: quote.into(),
        text: text.into(),
    }
}

#[test]
fn a_stack_sends_each_passage_quoted_over_its_reply_then_the_draft() {
    let stack = [
        reply(1, "The brand README is stale.", "Yes, we should fix this"),
        reply(2, "\u{201c}Gentle contrast\u{201d} needs a floor.", " Sounds good \n"),
    ];
    assert_eq!(
        message(&stack, ""),
        "> The brand README is stale.\n\nYes, we should fix this\n\n\
         > \u{201c}Gentle contrast\u{201d} needs a floor.\n\nSounds good"
    );
    assert_eq!(
        message(&stack[..1], "  And ship it.\n"),
        "> The brand README is stale.\n\nYes, we should fix this\n\nAnd ship it."
    );
}

#[test]
fn a_passage_without_a_reply_goes_alone_and_an_empty_stack_leaves_the_draft() {
    assert_eq!(
        message(&[reply(1, "line one\nline two", "  ")], ""),
        "> line one\n> line two"
    );
    assert_eq!(message(&[], " draft "), "draft");
    assert_eq!(message(&[], ""), "");
}

#[test]
fn a_chip_shows_the_passage_on_one_line_in_curly_quotes() {
    assert_eq!(
        chip_label("The brand\n README  is stale."),
        "\u{201c}The brand README is stale.\u{201d}"
    );
}

#[test]
fn a_bubble_splits_quotes_from_replies_in_order() {
    let sent = message(
        &[
            reply(1, "The brand README is stale.", "Yes, we should fix this"),
            reply(2, "About ten other docs.", "Yes, let's do as you propose"),
        ],
        "",
    );
    assert_eq!(
        segments(&sent),
        vec![
            (true, "The brand README is stale.".to_string()),
            (false, "Yes, we should fix this".to_string()),
            (true, "About ten other docs.".to_string()),
            (false, "Yes, let's do as you propose".to_string()),
        ]
    );
}

#[test]
fn a_bubble_keeps_paragraphs_and_never_quotes_inside_a_code_fence() {
    assert_eq!(
        segments("> a\n>\n> b\n\nfirst\n\nsecond"),
        vec![(true, "a\n\nb".to_string()), (false, "first\n\nsecond".to_string())]
    );
    assert_eq!(
        segments("```\n> not a quote\n```"),
        vec![(false, "```\n> not a quote\n```".to_string())]
    );
    assert_eq!(segments("plain"), vec![(false, "plain".to_string())]);
}

/// Draws the open session's chips, as the composer's top does.
struct Host(Entity<Workspace>);

impl Render for Host {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Palette::for_choice(ThemeChoice::Dark, window.appearance());
        self.0.update(cx, |workspace, cx| {
            div().w(px(600.)).children(workspace.render_reply_chips(palette, cx))
        })
    }
}

fn workspace(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(gpui_kit::init);
    let (gateway, _gateway_events) = crate::gateway::Gateway::start();
    let (_send, events) = futures::channel::mpsc::unbounded();
    let (_arrive, arrivals) = futures::channel::mpsc::unbounded();
    let (host, cx) = cx.add_window_view(|window, cx| {
        let workspace = cx.new(|cx| {
            let mut workspace = Workspace::with_machine_source(
                std::sync::Arc::new(gateway),
                events,
                arrivals,
                false,
                MachineSource::isolated(),
                window,
                cx,
            );
            workspace.view = View::Code(CodeView::new("session-a".into(), window, cx));
            workspace
        });
        // The chips are drawn inline: a change in the workspace draws the host again.
        cx.observe(&workspace, |_, _, cx| cx.notify()).detach();
        Host(workspace)
    });
    let workspace = host.read_with(cx, |host, _| host.0.clone());
    (workspace, cx)
}

#[gpui_kit::test]
fn replies_stack_per_session_and_the_box_closes_without_adding(cx: &mut TestAppContext) {
    let (workspace, cx) = workspace(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        let at = point(px(40.), px(80.));
        workspace.begin_reply("The brand README is stale.".into(), at, false, window, cx);
        assert!(workspace.replies.is_composing());
        assert!(workspace.replies.input.read(cx).focus_handle(cx).is_focused(window));
        workspace
            .replies
            .input
            .update(cx, |input, cx| input.set_value("Yes, we should fix this", window, cx));
        workspace.add_reply(window, cx);
        assert!(!workspace.replies.is_composing());

        // A box closed with Escape or a press outside adds nothing.
        workspace.begin_reply("Gentle contrast needs a floor.".into(), at, false, window, cx);
        workspace
            .replies
            .input
            .update(cx, |input, cx| input.set_value("dropped", window, cx));
        workspace.close_reply(window, cx);

        workspace.begin_reply("About ten other docs.".into(), at, false, window, cx);
        workspace.add_reply(window, cx);
        let stack: Vec<(String, String)> = workspace
            .pending_replies()
            .iter()
            .map(|reply| (reply.quote.clone(), reply.text.clone()))
            .collect();
        assert_eq!(
            stack,
            vec![
                ("The brand README is stale.".into(), "Yes, we should fix this".into()),
                ("About ten other docs.".into(), String::new()),
            ]
        );
        assert!(workspace.composer_send_enabled(), "a stack alone can be sent");

        // Another session has its own stack; this one's waits for it.
        workspace.view = View::Code(CodeView::new("session-b".into(), window, cx));
        assert!(workspace.pending_replies().is_empty());
        workspace.view = View::Code(CodeView::new("session-a".into(), window, cx));
        let first = workspace.pending_replies()[0].id;
        workspace.remove_reply(first, cx);
        assert_eq!(workspace.pending_replies().len(), 1);
        workspace.replies_sent("session-a");
        assert!(workspace.pending_replies().is_empty());
    });
}

/// Stack one passage with `reply`, and return its chip's id.
fn stack(workspace: &mut Workspace, reply: &str, window: &mut Window, cx: &mut Context<Workspace>) -> u64 {
    workspace.begin_reply(
        "The brand README is stale.".into(),
        point(px(40.), px(80.)),
        false,
        window,
        cx,
    );
    workspace
        .replies
        .input
        .update(cx, |input, cx| input.set_value(reply, window, cx));
    workspace.add_reply(window, cx);
    workspace.pending_replies().last().expect("stacked").id
}

#[gpui_kit::test]
fn a_chip_shows_its_reply_under_the_pointer_and_hides_it_when_it_leaves(cx: &mut TestAppContext) {
    let (workspace, cx) = workspace(cx);
    // Reduced motion settles the open and close in one frame.
    cx.update(|_, cx| cx.set_reduce_motion(true));
    workspace.update_in(cx, |workspace, window, cx| {
        stack(workspace, "Yes, we should fix this", window, cx);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("stacked-reply-text").is_none(), "closed at rest");
    assert!(cx.debug_bounds("stacked-reply-edit").is_none(), "no pencil at rest");
    let chip = cx.debug_bounds("stacked-reply").expect("the chip is drawn");
    cx.simulate_mouse_move(chip.center(), None, Modifiers::none());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let text = cx
        .debug_bounds("stacked-reply-text")
        .expect("the reply opens under the pointer");
    assert!(text.top() >= chip.bottom() - px(1.), "under the passage");
    assert!(cx.debug_bounds("stacked-reply-edit").is_some(), "the pencil shows");
    cx.simulate_mouse_move(point(px(590.), px(400.)), None, Modifiers::none());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("stacked-reply-text").is_none(),
        "closed once the pointer leaves"
    );
}

#[gpui_kit::test]
fn a_reply_is_rewritten_in_place_kept_with_save_and_dropped_with_cancel(cx: &mut TestAppContext) {
    let (workspace, cx) = workspace(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        let id = stack(workspace, "Yes, we should fix this", window, cx);
        let text = |workspace: &Workspace| workspace.pending_replies()[0].text.clone();

        workspace.edit_reply(id, window, cx);
        assert!(workspace.replies.edit.read(cx).focus_handle(cx).is_focused(window));
        assert_eq!(
            workspace.replies.edit.read(cx).value().as_ref(),
            "Yes, we should fix this"
        );
        workspace
            .replies
            .edit
            .update(cx, |edit, cx| edit.set_value("Fix it in this PR.", window, cx));
        workspace.save_reply_edit(window, cx);
        assert_eq!(text(workspace), "Fix it in this PR.");
        assert_eq!(workspace.replies.editing, None);

        workspace.edit_reply(id, window, cx);
        workspace
            .replies
            .edit
            .update(cx, |edit, cx| edit.set_value("dropped", window, cx));
        workspace.cancel_reply_edit(window, cx);
        assert_eq!(
            text(workspace),
            "Fix it in this PR.",
            "Cancel keeps the reply as it was"
        );

        // An empty rewrite leaves the passage without a reply; it still sends, quoted alone.
        workspace.edit_reply(id, window, cx);
        workspace
            .replies
            .edit
            .update(cx, |edit, cx| edit.set_value("  ", window, cx));
        workspace.save_reply_edit(window, cx);
        assert_eq!(text(workspace), "");
        assert_eq!(message(workspace.pending_replies(), ""), "> The brand README is stale.");

        // Removing the chip being edited ends the edit.
        workspace.edit_reply(id, window, cx);
        workspace.remove_reply(id, cx);
        assert_eq!(workspace.replies.editing, None);
    });
}
