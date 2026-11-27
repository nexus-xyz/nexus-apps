//! A Code session start is named by its draft view (`draft-N`): only its own answer moves it.
use super::{View, Workspace, code_view::CodeView, machine_source::MachineSource};
use crate::code_run::{CodeEvent, NewSession};
use gpui_kit::{Entity, Task, TestAppContext, VisualTestContext};
use nexus_workbench::event::{SessionAgent, Via};

/// A window whose draft view is starting a session; answers the draft's id.
fn starting(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext, String) {
    cx.update(gpui_kit::init);
    let gateway = crate::gateway::Gateway::for_view_test(crate::code_run::runtime());
    let (_, events) = futures::channel::mpsc::unbounded();
    let (_, arrivals) = futures::channel::mpsc::unbounded();
    let (workspace, cx) = cx.add_window_view(|window, cx| {
        let mut workspace = Workspace::with_machine_source(
            std::sync::Arc::new(gateway),
            events,
            arrivals,
            false,
            MachineSource::isolated(),
            window,
            cx,
        );
        workspace._code_poll = Task::ready(());
        workspace
    });
    let draft = workspace.update_in(cx, |workspace, window, cx| {
        let view = CodeView::pending(
            NewSession {
                project_id: "project".into(),
                agent: SessionAgent::Claude,
                model: None,
                permission_mode: "default".into(),
                effort: None,
                via: Via::Subscription,
                provider: None,
                worktree: false,
                review: None,
                base: None,
                text: "Fix the login".into(),
                attachments: Default::default(),
            },
            window,
            cx,
        );
        let draft = view.id.clone();
        workspace.code_form.starting = Some(draft.clone());
        workspace.view = View::Code(view);
        draft
    });
    (workspace, cx, draft)
}

fn open_id(workspace: &Workspace) -> (String, bool) {
    match &workspace.view {
        View::Code(view) => (
            view.id.clone(),
            view.pending.as_ref().is_some_and(|pending| pending.failed.is_some()),
        ),
        _ => panic!("the Code view stays open"),
    }
}

/// Approving a tool in another session fails while this one starts: before, the start read it as its own failure and
/// offered Try again, which made a second session once the first one landed.
#[gpui_kit::test]
fn another_commands_failure_keeps_the_start_going(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let (workspace, cx, draft) = starting(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.on_code_events(vec![CodeEvent::Failed("Could not approve.".into())], window, cx);
        let (id, failed) = open_id(workspace);
        assert_eq!(id, draft);
        assert!(!failed, "the start has not failed: no Try again");
        assert_eq!(workspace.code_form.starting.as_deref(), Some(draft.as_str()));
        assert_eq!(workspace.composer_note.as_deref(), Some("Could not approve."));
    });
}

/// Two windows start a session each. The first one's answer reaches both: only its own window takes it.
#[gpui_kit::test]
fn a_start_answer_for_another_draft_leaves_this_one(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let (workspace, cx, draft) = starting(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        let theirs = CodeEvent::Created {
            draft: "draft-elsewhere".into(),
            id: "their-session".into(),
        };
        assert!(!workspace.waits_for(&theirs), "another window's start goes there");
        workspace.on_code_events(vec![theirs], window, cx);
        assert_eq!(
            open_id(workspace).0,
            draft,
            "this view keeps waiting for its own session"
        );
        assert_eq!(workspace.code_form.starting.as_deref(), Some(draft.as_str()));
        workspace.on_code_events(
            vec![CodeEvent::StartFailed {
                draft: "draft-elsewhere".into(),
                error: "No room on the machine.".into(),
            }],
            window,
            cx,
        );
        assert!(!open_id(workspace).1, "another start's failure is not this one's");
        assert_eq!(workspace.code_form.starting.as_deref(), Some(draft.as_str()));

        let ours = CodeEvent::Created {
            draft: draft.clone(),
            id: "my-session".into(),
        };
        assert!(workspace.waits_for(&ours));
        workspace.on_code_events(vec![ours], window, cx);
        assert_eq!(open_id(workspace).0, "my-session");
        assert_eq!(workspace.code_form.starting, None);
    });
}

/// Its own failure: the view offers Try again and Edit, and New session takes a start again.
#[gpui_kit::test]
fn its_own_failure_ends_the_start(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let (workspace, cx, draft) = starting(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.on_code_events(
            vec![CodeEvent::StartFailed {
                draft: draft.clone(),
                error: "Sign in to Nexus to start Claude.".into(),
            }],
            window,
            cx,
        );
        assert_eq!(open_id(workspace), (draft.clone(), true));
        assert_eq!(workspace.code_form.starting, None);
    });
}

/// A start refused for want of a Nexus sign-in waits for the next sign-in's account, and only for that one.
#[gpui_kit::test]
fn a_start_without_a_sign_in_waits_for_the_next_sign_in(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let (workspace, cx, draft) = starting(cx);
    let key = crate::setup::FixKey::from(super::nexus_sign_in::START_KEY);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.on_code_events(
            vec![CodeEvent::StartFailed {
                draft: draft.clone(),
                error: "Sign in to Nexus to start Claude Code.".into(),
            }],
            window,
            cx,
        );
        assert!(crate::setup::state::waiting(&key, cx));
        // An account read with no sign-in before it (a poll) resumes nothing.
        workspace.account_after_sign_in(window, cx);
    });
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, window, cx| {
        assert!(crate::setup::state::waiting(&key, cx));
        workspace.setup.signed_in = true;
        workspace.account_after_sign_in(window, cx);
    });
    cx.run_until_parked();
    workspace.update(cx, |_, cx| assert!(!crate::setup::state::waiting(&key, cx)));
}

/// Any other failure waits for nothing.
#[gpui_kit::test]
fn another_failure_waits_for_no_sign_in(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let (workspace, cx, draft) = starting(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.on_code_events(
            vec![CodeEvent::StartFailed {
                draft: draft.clone(),
                error: "No room on the machine.".into(),
            }],
            window,
            cx,
        );
        let key = crate::setup::FixKey::from(super::nexus_sign_in::START_KEY);
        assert!(!crate::setup::state::waiting(&key, cx));
    });
}
