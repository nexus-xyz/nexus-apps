//! The side chat's "/" menu and commands, as the session composer's (`menus.rs`).
use super::super::composer::Section;
use super::super::{SideChat, SideEvent};
use super::{Rpc, summary};
use crate::workspace::slash::Action;
use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use nexus_workbench::event::{AgentCapabilities, SessionAgent};
use serde_json::json;
use std::cell::RefCell;
use std::rc::Rc;

/// A Claude Code side chat whose agent reported `/review` and compaction.
fn chat<'a>(cx: &'a mut TestAppContext, rpc: &Rpc) -> (Entity<SideChat>, &'a mut VisualTestContext) {
    cx.update(gpui_kit::init);
    let mut side = summary("menu-side", "parent");
    side.agent = SessionAgent::Claude;
    side.capabilities = Some(Box::new(AgentCapabilities {
        commands: Some(vec!["review".into(), "rename".into()]),
        compact: true,
        ..Default::default()
    }));
    cx.add_window_view(|window, cx| SideChat::new(side, "Parent".into(), rpc.runs("machine"), window, cx))
}

fn send(chat: &Entity<SideChat>, cx: &mut VisualTestContext, text: &str) {
    chat.update_in(cx, |chat, window, cx| {
        chat.input.update(cx, |input, cx| input.set_value(text, window, cx));
        chat.send(false, window, cx);
    });
    cx.run_until_parked();
}

fn text(chat: &Entity<SideChat>, cx: &mut VisualTestContext) -> String {
    chat.read_with(cx, |chat, cx| chat.input.read(cx).value().to_string())
}

#[gpui_kit::test]
fn the_menu_lists_the_side_chats_commands_then_its_agents(cx: &mut TestAppContext) {
    let rpc = Rpc::default();
    let (chat, cx) = chat(cx, &rpc);
    let names: Vec<String> = chat.read_with(cx, |chat, _| {
        chat.slash_list().into_iter().map(|command| command.name).collect()
    });
    for name in [
        "model",
        "effort",
        "permissions",
        "compact",
        "restart",
        "terminal",
        "review",
    ] {
        assert!(names.iter().any(|listed| listed == name), "{name} in {names:?}");
    }
    // No unique name and no side chat of its own; the agent's /rename stays replaced.
    for name in ["name", "rename", "btw", "side"] {
        assert!(!names.iter().any(|listed| listed == name), "{name} in {names:?}");
    }
}

#[gpui_kit::test]
fn enter_completes_the_highlighted_command_and_esc_closes_the_menu(cx: &mut TestAppContext) {
    let rpc = Rpc::default();
    let (chat, cx) = chat(cx, &rpc);
    chat.update_in(cx, |chat, window, cx| {
        chat.input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.set_value("/mod", window, cx);
        });
        assert!(chat.slash_rows(window, cx).is_some(), "the menu opens on /mod");
        assert!(chat.menu_pick(true, window, cx));
    });
    assert_eq!(text(&chat, cx), "/model ");
    chat.update_in(cx, |chat, window, cx| {
        chat.input.update(cx, |input, cx| input.set_value("/re", window, cx));
        assert!(chat.menu_dismiss(window, cx));
        assert!(chat.slash_rows(window, cx).is_none(), "closed for this text");
        assert!(!chat.menu_pick(true, window, cx), "Enter sends again");
    });
    assert_eq!(rpc.count("wb_send_files"), 0);
}

#[gpui_kit::test]
fn a_nexus_command_opens_the_side_chats_own_menu_and_sends_nothing(cx: &mut TestAppContext) {
    let rpc = Rpc::default();
    let (chat, cx) = chat(cx, &rpc);
    send(&chat, cx, "/model");
    assert_eq!(rpc.count("wb_send_files"), 0);
    assert_eq!(text(&chat, cx), "");
    chat.read_with(cx, |chat, _| assert_eq!(chat.settings, Some(Section::Model)));
    send(&chat, cx, "/model gpt");
    chat.read_with(cx, |chat, _| {
        assert_eq!(
            chat.note.as_deref(),
            Some("/model opens a Nexus control and accepts no arguments.")
        );
    });
    assert_eq!(text(&chat, cx), "/model gpt", "a refused command stays in the composer");
}

#[gpui_kit::test]
fn compact_and_restart_reach_the_side_thread(cx: &mut TestAppContext) {
    let rpc = Rpc::default();
    let (chat, cx) = chat(cx, &rpc);
    send(&chat, cx, "/compact keep the tests");
    let compact = rpc.take("wb_compact");
    assert_eq!(compact.args["threadId"], "menu-side");
    assert_eq!(compact.args["instructions"], "keep the tests");
    compact.answer.send(Ok(json!(null))).unwrap();
    cx.run_until_parked();
    send(&chat, cx, "/restart");
    let stop = rpc.take("wb_stop");
    assert_eq!(stop.args, json!({"threadId": "menu-side", "restart": true}));
    assert_eq!(rpc.count("wb_send_files"), 0);
}

#[gpui_kit::test]
fn a_session_tool_opens_in_the_workspace_and_an_agent_command_goes_to_the_agent(cx: &mut TestAppContext) {
    let rpc = Rpc::default();
    let (chat, cx) = chat(cx, &rpc);
    let heard = Rc::new(RefCell::new(Vec::new()));
    let _subscription = cx.update({
        let heard = heard.clone();
        let chat = chat.clone();
        move |_, cx| {
            cx.subscribe(&chat, move |_, event: &SideEvent, _| {
                if let SideEvent::Slash(action) = event {
                    heard.borrow_mut().push(*action);
                }
            })
        }
    });
    send(&chat, cx, "/terminal");
    assert_eq!(*heard.borrow(), [Action::Terminal]);
    assert_eq!(rpc.count("wb_send_files"), 0);
    send(&chat, cx, "/review the diff");
    assert_eq!(rpc.take("wb_send_files").args["text"], "/review the diff");
}
