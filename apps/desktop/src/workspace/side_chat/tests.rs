use super::{SideChat, own_blocks, side_question};
use crate::code::Block;
use crate::workspace::runs::Runs;
use gpui_kit::{
    AppContext, Context, Entity, Focusable, InteractiveElement, IntoElement, Modifiers, MouseButton, MouseDownEvent,
    MouseUpEvent, ParentElement, Render, Styled, TestAppContext, VisualTestContext, Window, div, px,
};
use nexus_workbench::event::{StoredEvent, ThreadSummary, WbEvent};

fn summary() -> ThreadSummary {
    serde_json::from_value(serde_json::json!({
        "id": "side", "projectId": "p", "title": "A question", "agent": "codex", "model": null,
        "permissionMode": "default", "via": "nexus", "status": "idle", "costUsd": 0.0,
        "nexusSessions": [], "createdAt": 100, "updatedAt": 100, "archived": false,
        "sideChat": true, "sourceThreadId": "parent"
    }))
    .expect("side chat summary")
}

fn event(seq: i64, at: i64, event: WbEvent) -> StoredEvent {
    StoredEvent {
        thread_id: "side".into(),
        seq,
        at,
        event,
    }
}

fn said(seq: i64, at: i64, text: &str, parent: Option<&str>) -> StoredEvent {
    event(
        seq,
        at,
        WbEvent::Text {
            id: format!("text-{seq}"),
            text: text.into(),
            parent: parent.map(str::to_string),
        },
    )
}

#[test]
fn side_questions_keep_the_question_and_require_a_complete_command() {
    assert_eq!(
        side_question("/btw Explain this\nwithout edits"),
        Some("Explain this\nwithout edits")
    );
    assert_eq!(side_question("/side"), Some(""));
    assert_eq!(side_question("/sideways hello"), None);
    assert_eq!(side_question("ordinary message"), None);
}

#[test]
fn copied_parent_messages_do_not_appear_in_the_side_conversation() {
    let mut events = vec![said(1, 10, "Parent response", None)];
    assert!(own_blocks(&events, 100).is_empty());
    events.push(event(
        2,
        110,
        WbEvent::User {
            text: "My question".into(),
            images: vec![],
            from: None,
        },
    ));
    events.push(said(3, 120, "Side answer", None));
    let blocks = own_blocks(&events, 100);
    assert!(
        matches!(&blocks[..], [Block::User { text, .. }, Block::Text(answer)] if text == "My question" && answer == "Side answer")
    );
}

struct Panel {
    chat: Entity<SideChat>,
    width: f32,
}
impl Render for Panel {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .debug_selector(|| "side-panel".into())
            .w(px(self.width))
            .h(px(640.))
            .child(self.chat.clone())
    }
}
fn panel(cx: &mut TestAppContext) -> (Entity<Panel>, &mut VisualTestContext) {
    cx.update(gpui_kit::init);
    cx.add_window_view(|window, cx| Panel {
        chat: cx.new(|cx| SideChat::new(summary(), "Parent session".into(), Runs::isolated(), window, cx)),
        width: 620.,
    })
}
fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}
fn click(cx: &mut VisualTestContext, selector: &'static str) {
    draw(cx);
    let center = cx.debug_bounds(selector).expect("control is drawn").center();
    cx.simulate_click(center, Modifiers::default());
    draw(cx);
}

#[gpui_kit::test]
fn suggestions_fill_the_side_draft_without_sending_and_menu_escape_restores_focus(cx: &mut TestAppContext) {
    let (panel, cx) = panel(cx);
    let chat = panel.read_with(cx, |panel, _| panel.chat.clone());
    click(cx, "side-suggestion-0");
    chat.read_with(cx, |chat, cx| {
        assert_eq!(chat.input.read(cx).value().as_ref(), "Summarize the approach");
        assert!(!chat.sending);
    });
    click(cx, "side-chat-options");
    assert!(chat.read_with(cx, |chat, _| chat.menu_open));
    click(cx, "side-chat-options");
    assert!(
        !chat.read_with(cx, |chat, _| chat.menu_open),
        "second click closes the menu"
    );
    click(cx, "side-chat-options");
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(!chat.read_with(cx, |chat, _| chat.menu_open));
    cx.update(|window, cx| assert!(chat.read(cx).input.focus_handle(cx).is_focused(window)));
}

#[gpui_kit::test]
fn the_composer_fits_narrow_panels_and_only_side_answers_can_be_brought_back(cx: &mut TestAppContext) {
    let (panel, cx) = panel(cx);
    let chat = panel.read_with(cx, |panel, _| panel.chat.clone());
    for width in [620., 320., 240.] {
        panel.update(cx, |panel, cx| {
            panel.width = width;
            cx.notify();
        });
        draw(cx);
        let pane = cx.debug_bounds("side-panel").expect("panel is drawn");
        let composer = cx.debug_bounds("side-chat-composer").expect("composer is drawn");
        assert!(composer.left() >= pane.left() && composer.right() <= pane.right());
        assert!(
            composer.bottom() <= pane.bottom(),
            "composer stays in the panel at {width}px"
        );
    }
    chat.update(cx, |chat, cx| {
        chat.events = vec![said(1, 10, "Parent answer", None)];
        assert!(chat.is_empty());
        assert!(!chat.has_answer());
        chat.events.push(said(2, 110, "Subagent answer", Some("tool")));
        assert!(!chat.has_answer());
        chat.events.push(said(3, 120, "Side answer", None));
        assert!(chat.has_answer());
        cx.notify();
    });
}

#[gpui_kit::test]
fn bring_back_sits_in_the_latest_answers_actions_and_becomes_a_queued_note(cx: &mut TestAppContext) {
    let (panel, cx) = panel(cx);
    let chat = panel.read_with(cx, |panel, _| panel.chat.clone());
    let turn = |seq| Block::Turn {
        seq,
        at: 0,
        status: nexus_workbench::event::TurnStatus::Completed,
        duration_ms: 1,
        cost_usd: None,
        tokens: 0,
        model: None,
        reverted: false,
    };
    chat.update(cx, |chat, cx| {
        chat.blocks = vec![
            Block::Text("First".into()),
            turn(2),
            Block::Text("Second".into()),
            turn(4),
            Block::Text("Working".into()),
        ];
        for (ix, text) in [(0, "First"), (2, "Second"), (4, "Working")] {
            chat.texts.insert(
                ix,
                cx.new(|cx| gpui_kit::component::text::TextViewState::markdown(text, cx)),
            );
        }
        chat.events = vec![said(1, 120, "Second", None)];
        assert_eq!(
            chat.bring_back_turn(),
            Some(3),
            "the last turn with an answer carries it"
        );
        cx.notify();
    });
    draw(cx);
    let action = cx.debug_bounds("side-bring-back").expect("Bring back is drawn");
    let row = cx.debug_bounds("side-turn-3").expect("the latest turn's row is drawn");
    assert!(action.top() < row.bottom() && action.bottom() > row.top());
    chat.update(cx, |chat, cx| {
        chat.brought_back = true;
        cx.notify();
    });
    draw(cx);
    assert!(cx.debug_bounds("side-bring-back").is_none());
    assert!(cx.debug_bounds("side-brought-back").is_some());
}

#[gpui_kit::test]
fn tool_groups_and_output_expand_without_changing_the_side_draft(cx: &mut TestAppContext) {
    let (panel, cx) = panel(cx);
    let chat = panel.read_with(cx, |panel, _| panel.chat.clone());
    chat.update(cx, |chat, cx| {
        let tool = |id: &str| Block::Tool {
            call_id: id.into(),
            name: "Read".into(),
            summary: "pipeline.md".into(),
            error: None,
            edit: None,
            images: vec![],
            call: Box::new(crate::code::ToolCall {
                id: id.into(),
                input: serde_json::json!({"file_path":"pipeline.md"}),
                output: Some(crate::code::ToolOutput {
                    text: "first\nsecond\nthird\nfourth\nfifth\nsixth".into(),
                    ..Default::default()
                }),
            }),
        };
        chat.blocks = vec![tool("read-1"), tool("read-2"), Block::Text("Done".into())];
        chat.texts.insert(
            2,
            cx.new(|cx| gpui_kit::component::text::TextViewState::markdown("Done", cx)),
        );
        chat.events = vec![event(
            1,
            110,
            WbEvent::User {
                text: "Trace it".into(),
                images: vec![],
                from: None,
            },
        )];
        cx.notify();
    });
    draw(cx);
    assert!(
        cx.debug_bounds("side-tool-0").is_none(),
        "completed routine tools start folded"
    );
    click(cx, "side-work-0");
    click(cx, "side-tool-0");
    assert!(cx.debug_bounds("side-output-all-0").is_some());
    click(cx, "side-output-all-0");
    chat.read_with(cx, |chat, cx| {
        assert!(chat.history.open.contains("read-1"));
        assert!(chat.history.all_output.contains("read-1"));
        assert!(chat.input.read(cx).value().is_empty());
        assert!(!chat.sending);
    });
    assert!(cx.debug_bounds("side-output-all-0").is_none());
}

#[gpui_kit::test]
fn side_menus_switch_without_closing_or_changing_the_draft(cx: &mut TestAppContext) {
    use super::composer::Section;
    // Keep the real Codex catalog branch, but answer its instance-local transport on the test thread.
    let pending = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let reads = pending.clone();
    let runs = Runs::deferred(
        "settings-catalog",
        std::sync::Arc::new(move |name, args| {
            let (send, receive) = futures::channel::oneshot::channel();
            reads.lock().unwrap().push((name, args, send));
            receive
        }),
    );
    let (panel, cx) = panel(cx);
    let chat = panel.read_with(cx, |panel, _| panel.chat.clone());
    cx.update(|window, cx| {
        chat.update(cx, |chat, cx| {
            assert_eq!(chat.summary.agent, nexus_workbench::event::SessionAgent::Codex);
            chat.runs = runs;
            chat.input
                .update(cx, |input, cx| input.set_value("Keep this draft", window, cx));
            chat.settings(Section::Effort, window, cx);
            assert_eq!(chat.settings, Some(Section::Effort));
            chat.settings(Section::Model, window, cx);
            assert_eq!(chat.settings, Some(Section::Model), "the other chip switches menus");
            chat.settings(Section::Model, window, cx);
            assert_eq!(chat.settings, None, "the same chip closes its menu");
            assert_eq!(chat.input.read(cx).value().as_ref(), "Keep this draft");
        })
    });
    click(cx, "side-effort");
    assert_eq!(chat.read_with(cx, |chat, _| chat.settings), Some(Section::Effort));
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert_eq!(chat.read_with(cx, |chat, _| chat.settings), None);
    cx.run_until_parked();
    let (name, args, send) = {
        let mut pending = pending.lock().unwrap();
        assert_eq!(pending.len(), 1, "section switches share the pending catalog read");
        pending.pop().unwrap()
    };
    assert_eq!(name, "wb_agent_models");
    assert_eq!(args, serde_json::json!({}));
    assert!(chat.read_with(cx, |chat, _| chat.models_loading));
    send.send(Ok(serde_json::json!([{
        "agent": "codex", "models": [{"id": "codex-test", "name": "Codex test", "efforts": []}]
    }])))
    .expect("the catalog task is still waiting");
    cx.run_until_parked();
    chat.read_with(cx, |chat, cx| {
        assert!(
            !chat.models_loading,
            "the actual catalog task consumed the reply before teardown"
        );
        assert!(chat.catalog_problem.is_none());
        assert_eq!(chat.models.len(), 1);
        assert_eq!(chat.models[0].id, "codex-test");
        assert_eq!(chat.settings, None);
        assert_eq!(chat.input.read(cx).value().as_ref(), "Keep this draft");
    });
    assert!(pending.lock().unwrap().is_empty());
}

#[gpui_kit::test]
fn delete_asks_inside_the_menu_and_keep_closes_it(cx: &mut TestAppContext) {
    let (panel, cx) = panel(cx);
    let chat = panel.read_with(cx, |panel, _| panel.chat.clone());
    click(cx, "side-chat-options");
    click(cx, "side-menu-delete");
    assert!(chat.read_with(cx, |chat, _| chat.menu_open && chat.deleting));
    assert!(cx.debug_bounds("confirm-side-delete").is_some());
    assert!(
        cx.debug_bounds("side-menu-close").is_none(),
        "the confirmation takes the menu's place"
    );
    click(cx, "keep-side-chat");
    assert!(chat.read_with(cx, |chat, _| !chat.menu_open && !chat.deleting));
    assert!(!chat.read_with(cx, |chat, _| chat.sending), "Keep deletes nothing");
}

#[gpui_kit::test]
fn queued_messages_wait_over_the_composer_not_in_the_history(cx: &mut TestAppContext) {
    let (panel, cx) = panel(cx);
    let chat = panel.read_with(cx, |panel, _| panel.chat.clone());
    chat.update(cx, |chat, cx| {
        chat.blocks = vec![Block::Queued {
            seq: 7,
            text: "Then check the docs".into(),
        }];
        cx.notify();
    });
    draw(cx);
    let queued = cx.debug_bounds("side-steer-7").expect("queued chip is drawn");
    let composer = cx.debug_bounds("side-chat-composer").expect("composer");
    assert!(queued.bottom() <= composer.top(), "the chip sits over the composer");
}

#[gpui_kit::test]
fn composer_surface_and_actions_stay_inside_the_shell(cx: &mut TestAppContext) {
    let (panel, cx) = panel(cx);
    for width in [840., 480., 320., 240.] {
        panel.update(cx, |panel, cx| {
            panel.width = width;
            cx.notify();
        });
        draw(cx);
        let shell = cx.debug_bounds("side-chat-composer").expect("composer");
        // The glass covers the shell's whole box, border included, so its corners match the border's: inside the
        // border, its 16px corners would leave a sliver of what lies under the box at each corner.
        let surface = cx.debug_bounds("composer-surface").expect("anchored surface");
        assert_eq!(surface, shell);
        let send = cx.debug_bounds("side-send").expect("send button");
        assert_eq!(send.size.width, px(28.));
        assert_eq!(send.size.height, px(28.));
        assert!(send.right() <= shell.right());
        let toolbar = cx.debug_bounds("composer-toolbar").expect("shared toolbar");
        assert!(toolbar.left() >= shell.left() && toolbar.right() <= shell.right());
        assert!(toolbar.bottom() <= shell.bottom());
    }
}

#[gpui_kit::test]
fn command_permission_uses_a_card_and_keeps_the_draft(cx: &mut TestAppContext) {
    let (panel, cx) = panel(cx);
    let chat = panel.read_with(cx, |panel, _| panel.chat.clone());
    cx.update(|window, cx| {
        chat.update(cx, |chat, cx| {
            chat.input
                .update(cx, |input, cx| input.set_value("Keep this draft", window, cx));
            chat.blocks = vec![Block::Approval {
                request_id: "permission-1".into(),
                tool: "Bash".into(),
                summary: "Run tests".into(),
                description: Some("Check the pipeline".into()),
                input: serde_json::json!({"command":"cargo test\n--workspace"}),
                suggestions: vec![],
                answer: None,
                moved: false,
                message: None,
            }];
            chat.request = Some("permission-1".into());
            cx.notify();
        })
    });
    draw(cx);
    assert!(cx.debug_bounds("side-permission-card").is_some());
    assert!(cx.debug_bounds("side-chat-composer").is_some());
    assert!(cx.debug_bounds("side-allow").is_some());
    assert!(cx.debug_bounds("side-deny").is_some());
    assert_eq!(
        crate::workspace::permission_queue::payload("Bash", &serde_json::json!({"command":"cargo test\n--workspace"})),
        "cargo test\n--workspace"
    );
    chat.read_with(cx, |chat, cx| {
        assert_eq!(chat.input.read(cx).value().as_ref(), "Keep this draft")
    });
    chat.update(cx, |chat, _| chat.request = Some("different-request".into()));
    click(cx, "side-allow");
    assert!(
        !chat.read_with(cx, |chat, _| chat.sending),
        "a stale card must not approve a different request"
    );
}

#[gpui_kit::test]
fn rename_opens_from_the_menu_or_a_double_click_and_enter_keeps_the_title(cx: &mut TestAppContext) {
    // The title save goes to an instance-local transport answered on the test thread, not the shared runtime.
    let pending = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let calls = pending.clone();
    let runs = Runs::deferred(
        "rename",
        std::sync::Arc::new(move |name, args| {
            let (send, receive) = futures::channel::oneshot::channel();
            calls.lock().unwrap().push((name, args, send));
            receive
        }),
    );
    let (panel, cx) = panel(cx);
    let chat = panel.read_with(cx, |panel, _| panel.chat.clone());
    chat.update(cx, |chat, _| chat.runs = runs);
    click(cx, "side-chat-options");
    click(cx, "side-menu-rename");
    assert!(chat.read_with(cx, |chat, _| !chat.menu_open && chat.renaming.is_some()));
    assert!(cx.debug_bounds("side-chat-title-field").is_some());
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(
        chat.read_with(cx, |chat, _| chat.renaming.is_none()),
        "Escape drops the field"
    );
    cx.update(|window, cx| assert!(chat.read(cx).input.focus_handle(cx).is_focused(window)));

    let center = cx.debug_bounds("side-chat-title").expect("title is drawn").center();
    cx.simulate_event(MouseDownEvent {
        position: center,
        button: MouseButton::Left,
        modifiers: Modifiers::default(),
        click_count: 2,
        first_mouse: false,
    });
    cx.simulate_event(MouseUpEvent {
        position: center,
        button: MouseButton::Left,
        modifiers: Modifiers::default(),
        click_count: 2,
    });
    draw(cx);
    let field = chat.read_with(cx, |chat, _| chat.renaming.as_ref().map(|r| r.input.clone()));
    let field = field.expect("a double-click renames");
    cx.update(|window, cx| field.update(cx, |state, cx| state.set_value("Why that file", window, cx)));
    cx.simulate_keystrokes("enter");
    draw(cx);
    let (name, args, send) = {
        let mut pending = pending.lock().unwrap();
        assert_eq!(pending.len(), 1, "Enter saves the title once");
        pending.pop().unwrap()
    };
    assert_eq!(name, "wb_thread_update");
    assert_eq!(args, serde_json::json!({"threadId":"side","title":"Why that file"}));
    send.send(Ok(serde_json::json!(true)))
        .expect("the save is still waiting");
    cx.run_until_parked();
    chat.read_with(cx, |chat, _| {
        assert!(chat.renaming.is_none());
        assert!(!chat.sending);
        assert_eq!(chat.summary.title, "Why that file");
    });
}

#[gpui_kit::test]
fn side_partial_updates_reuse_markdown_and_replace_or_clear_old_content(cx: &mut TestAppContext) {
    let (panel, cx) = panel(cx);
    let chat = panel.read_with(cx, |panel, _| panel.chat.clone());
    let chunks = [
        "# Héllo",
        "\n\nFirst **para",
        "graph**.\n\n",
        "- one\n- two\n\n",
        "```rust\n",
        "let x = 1;\n```\n",
    ];
    let mut text = String::new();
    let mut original = None;
    for chunk in chunks {
        text.push_str(chunk);
        chat.update(cx, |chat, cx| assert!(chat.set_stream(Some(text.clone()), cx)));
        cx.run_until_parked();
        chat.read_with(cx, |chat, _| {
            let id = chat.streamed.as_ref().expect("partial").1.entity_id();
            assert_eq!(
                *original.get_or_insert(id),
                id,
                "a growing answer keeps its Markdown entity"
            );
        });
    }
    let expected = cx.update(|_, cx| cx.new(|cx| gpui_kit::component::text::TextViewState::markdown(&text, cx)));
    cx.run_until_parked();
    let rendered = expected.read_with(cx, |view, _| view.rendered_text().as_str().to_string());
    chat.update(cx, |chat, cx| {
        assert!(
            !chat.set_stream(Some(text.clone()), cx),
            "unchanged partials do no work"
        );
        let (_, view) = chat.streamed.as_ref().expect("partial");
        assert_eq!(view.read(cx).rendered_text().as_str(), rendered);
        view.update(cx, |view, cx| view.select_all(cx));
        assert!(chat.set_stream(Some("Replacement".into()), cx));
    });
    cx.run_until_parked();
    chat.update(cx, |chat, cx| {
        let (source, view) = chat.streamed.as_ref().expect("replacement");
        assert_eq!(source, "Replacement");
        assert_eq!(Some(view.entity_id()), original);
        // The pinned Markdown parser includes the paragraph's line break in its plain-text projection.
        assert_eq!(view.read(cx).rendered_text().as_str(), "Replacement\n");
        assert!(
            view.read(cx).selected_text().is_empty(),
            "a replacement clears the old selection"
        );
        assert!(chat.set_stream(None, cx));
        assert!(chat.streamed.is_none());
        assert!(!chat.set_stream(None, cx));
        assert!(chat.set_stream(Some("Next answer".into()), cx));
        assert_ne!(Some(chat.streamed.as_ref().unwrap().1.entity_id()), original);
    });
}

/// Run alone with `cargo test --release side_partial_stream_benchmark -- --ignored --nocapture --test-threads=1`.
/// This measures update plus completion of parsing on GPUI's test executor, without GPU presentation or network.
#[gpui_kit::test]
#[ignore = "performance measurement: run alone in an optimized test binary"]
fn side_partial_stream_benchmark(cx: &mut TestAppContext) {
    use gpui_kit::component::text::TextViewState;
    use std::time::Instant;

    assert!(!cfg!(debug_assertions), "use cargo test --release");
    let (panel, cx) = panel(cx);
    let chat = panel.read_with(cx, |panel, _| panel.chat.clone());
    let chunks: Vec<_> = (0..256)
        .map(|n| format!("## Step {n}\n\nA **streamed** paragraph with `code`, héllo and an emoji 🦀.\n\n"))
        .collect();
    let final_text = chunks.concat();
    let expected = cx.update(|_, cx| cx.new(|cx| TextViewState::markdown(&final_text, cx)));
    cx.run_until_parked();
    let expected = expected.read_with(cx, |view, _| view.rendered_text().as_str().to_string());
    let mut samples = Vec::new();
    // Alternate order each round. The first round warms allocations and is omitted.
    for round in 0..8 {
        for reuse in if round % 2 == 0 { [false, true] } else { [true, false] } {
            chat.update(cx, |chat, cx| {
                chat.set_stream(None, cx);
            });
            cx.run_until_parked();
            let mut text = String::new();
            let mut entities = std::collections::HashSet::new();
            let start = Instant::now();
            let mut update_ms = 0.;
            for chunk in &chunks {
                text.push_str(chunk);
                let update = Instant::now();
                chat.update(cx, |chat, cx| {
                    if reuse {
                        assert!(chat.set_stream(Some(text.clone()), cx));
                    } else {
                        // The original live-delta and restored-snapshot implementation.
                        let view = cx.new(|cx| TextViewState::markdown(&text, cx));
                        chat.streamed = Some((text.clone(), view));
                    }
                    entities.insert(chat.streamed.as_ref().unwrap().1.entity_id());
                });
                update_ms += update.elapsed().as_secs_f64() * 1000.;
                // Include parsing before the next update, as with 50ms Workbench deltas.
                cx.run_until_parked();
            }
            let elapsed_ms = start.elapsed().as_secs_f64() * 1000.;
            chat.read_with(cx, |chat, cx| {
                let (source, view) = chat.streamed.as_ref().expect("complete stream");
                assert_eq!(source, &final_text);
                assert_eq!(view.read(cx).rendered_text().as_str(), expected);
            });
            assert_eq!(entities.len(), if reuse { 1 } else { chunks.len() });
            if round > 0 {
                samples.push(serde_json::json!({
                    "round": round,
                    "mode": if reuse { "reuse" } else { "recreate" },
                    "updates": chunks.len(), "final_bytes": final_text.len(),
                    "entities": entities.len(), "update_ms": update_ms, "parse_complete_ms": elapsed_ms
                }));
            }
        }
    }
    println!(
        "side_partial_stream_benchmark={}",
        serde_json::json!({"samples": samples})
    );
}
#[path = "keys_tests.rs"]
mod keys_tests;
#[path = "paste_tests.rs"]
mod paste_tests;
