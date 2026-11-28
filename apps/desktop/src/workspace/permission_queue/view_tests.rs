//! Render the production permission queue, including its actual wrapping action footer.
use super::*;
use crate::workspace::{Auth, machine_source::MachineSource};
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, VisualTestContext};
use std::sync::Arc;

struct QueueView {
    workspace: Entity<Workspace>,
    width: f32,
    limit: f32,
}
impl Render for QueueView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Palette::for_choice(crate::palette::ThemeChoice::Dark, window.appearance());
        let queue = self.workspace.update(cx, |workspace, cx| {
            let View::Code(view) = &workspace.view else {
                unreachable!()
            };
            workspace
                .render_permission_queue(view, palette, self.limit, cx)
                .unwrap()
        });
        div().w(px(self.width)).child(queue)
    }
}

fn fixture(cx: &mut TestAppContext) -> (Entity<QueueView>, &mut VisualTestContext) {
    cx.update(gpui_kit::init);
    let (gateway, _) = crate::gateway::Gateway::start();
    let (_, events) = futures::channel::mpsc::unbounded();
    let (_, arrivals) = futures::channel::mpsc::unbounded();
    cx.add_window_view(|window, cx| {
        let workspace = cx.new(|cx| {
            let mut workspace = Workspace::with_machine_source(
                Arc::new(gateway),
                events,
                arrivals,
                false,
                MachineSource::isolated(),
                window,
                cx,
            );
            workspace._code_poll = Task::ready(());
            workspace.runner = None;
            workspace.auth = Auth::SignedIn;
            let mut view = CodeView::new("tray-test".into(), window, cx);
            view.blocks = (0..100)
                .map(|index| Block::Approval {
                    request_id: format!("request-{index}"),
                    tool: "Bash".into(),
                    summary: format!("Command {index}"),
                    description: Some("A long reason\n".repeat(100)),
                    input: json!({"command": "echo command\n".repeat(100), "cwd": "/work"}),
                    suggestions: vec![],
                    answer: None,
                    moved: false,
                    message: None,
                })
                .collect();
            view.dock.permissions.follow(&view.blocks);
            workspace.view = View::Code(view);
            workspace
        });
        QueueView {
            workspace,
            width: 680.,
            limit: 300.,
        }
    })
}

#[gpui_kit::test]
fn real_permission_actions_fit_after_resize_and_collapsed_requests_stay_visible(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (queue, cx) = fixture(cx);
    for (width, limit) in [(680., 300.), (320., 180.)] {
        queue.update(cx, |queue, cx| {
            queue.width = width;
            queue.limit = limit;
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let tray = cx.debug_bounds("permission-tray").unwrap();
        let actions = cx.debug_bounds("permission-actions").unwrap();
        assert!(tray.size.height <= px(limit));
        assert!(actions.bottom() <= tray.bottom(), "{actions:?} must fit {tray:?}");
        assert!(actions.right() <= tray.right());
        assert!(actions.size.height > px(28.));
        // Collapsed rows carry their own decisions; the open row's are in the footer.
        assert!(cx.debug_bounds("permission-quick-allow-request-0").is_none());
        assert!(cx.debug_bounds("permission-quick-allow-request-1").is_some());
        assert!(cx.debug_bounds("permission-quick-deny-request-1").is_some());
    }
    let workspace = queue.read_with(cx, |queue, _| queue.workspace.clone());
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.toggle_permission_group(false, false, window, cx);
        let View::Code(view) = &workspace.view else {
            unreachable!()
        };
        assert!(view.dock.permissions.collapsed);
        assert_eq!(view.dock.permissions.pending(&view.blocks).len(), 100);
        assert!(view.dock.permissions.current(&view.blocks).is_none());
    });
    cx.run_until_parked();
    queue.update(cx, |_, cx| cx.notify());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("permission-tray").unwrap().size.height <= px(crate::command_tray::HEADER));
    assert!(cx.debug_bounds("permission-actions").is_none());
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.toggle_permission_group(true, true, window, cx);
        let View::Code(view) = &workspace.view else {
            unreachable!()
        };
        assert!(view.dock.permissions.reviewing);
        assert_eq!(view.dock.permissions.open.as_deref(), Some("request-0"));
    });
}

/// One Bash request whose command fills the tray, with a rule to remember.
fn remember_fixture(cx: &mut TestAppContext) -> (Entity<QueueView>, &mut VisualTestContext) {
    let (queue, cx) = fixture(cx);
    let workspace = queue.read_with(cx, |queue, _| queue.workspace.clone());
    workspace.update(cx, |workspace, cx| {
        let View::Code(view) = &mut workspace.view else {
            unreachable!()
        };
        view.blocks = vec![Block::Approval {
            request_id: "request-0".into(),
            tool: "Bash".into(),
            summary: "Check out the account".into(),
            description: None,
            input: json!({"command": "git checkout account\n".repeat(40), "cwd": "/work"}),
            suggestions: vec![nexus_workbench::event::PermissionSuggestion {
                kind: "addRules".into(),
                label: "label".into(),
                raw: json!({"type": "addRules", "destination": "session",
                    "rules": [{"toolName": "Bash", "ruleContent": "git checkout:*"}]}),
            }],
            answer: None,
            moved: false,
            message: None,
        }];
        view.dock.permissions = PermissionQueue::default();
        view.dock.permissions.follow(&view.blocks);
        cx.notify();
    });
    (queue, cx)
}

#[gpui_kit::test]
fn remember_shows_its_rules_inside_the_tray(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (queue, cx) = remember_fixture(cx);
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    // Allow always… sits under the long command: scroll to it, as a person does, before the click.
    let workspace = queue.read_with(cx, |queue, _| queue.workspace.clone());
    workspace.update(cx, |workspace, cx| {
        let View::Code(view) = &workspace.view else {
            unreachable!()
        };
        view.dock.permissions.scroll.scroll_to_bottom();
        cx.notify();
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let remember = cx
        .debug_bounds("permission-always-request-0")
        .expect("Allow always… is drawn");
    cx.simulate_click(remember.center(), Modifiers::default());
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let tray = cx.debug_bounds("permission-tray").unwrap();
    let rule = cx
        .debug_bounds("permission-rule-request-0-0")
        .expect("the click opens the rules");
    assert!(
        rule.top() >= tray.top() && rule.bottom() <= tray.bottom(),
        "the rule {rule:?} must show inside the tray {tray:?}"
    );
}
