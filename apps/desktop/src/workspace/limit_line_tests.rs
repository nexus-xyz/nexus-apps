use super::*;
use crate::code_run::AgentAccess;
use crate::gateway::Account;
use crate::palette::ThemeChoice;
use crate::workspace::machine_source::MachineSource;
use core::prelude::v1::test;
use nexus_client::local_agents::{LocalAgent, Login};
use nexus_workbench::api::AgentInfo;
use nexus_workbench::event::SessionAgent;
use serde_json::json;
use std::sync::Arc;

#[test]
fn only_this_accounts_budget_read_again_lifts_the_hold() {
    let used_up = Budget::Exhausted { resets_at: None };
    assert!(limit_lifted(true, &Budget::Remaining, false), "raised");
    assert!(limit_lifted(true, &used_up, false), "another payer");
    assert!(!limit_lifted(true, &used_up, true), "still used up");
    assert!(!limit_lifted(false, &Budget::Remaining, false), "another account's");
    assert!(!limit_lifted(true, &Budget::Unknown, false), "not read yet");
}

/// The budget is used up: the strip is its amber line, a send is held without a note and keeps its text, and it goes
/// once the person picks their plan to pay. In a chat the line sits over the composer.
#[gpui_kit::test]
fn a_send_held_for_a_used_up_budget_goes_once_another_payer_is_picked(cx: &mut gpui_kit::TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    cx.update(gpui_kit::init);
    let (gateway, _gateway_events) = crate::gateway::Gateway::start();
    let (_send, events) = futures::channel::mpsc::unbounded();
    let (_arrive, arrivals) = futures::channel::mpsc::unbounded();
    let (workspace, cx) = cx.add_window_view(|window, cx| {
        Workspace::with_machine_source(
            Arc::new(gateway),
            events,
            arrivals,
            false,
            MachineSource::isolated(),
            window,
            cx,
        )
    });
    let held: FixKey = HELD.into();
    workspace.update_in(cx, |workspace, window, cx| {
        let account = Account::from_json(&json!({"tenant":{"id":"org-a","slug":"org-a","name":"Acme","kind":"company"},"user":{"id":"person-a","personId":"person-a","name":"A","email":"a@example.test"},"role":"owner","permissions":[]})).expect("account");
        workspace.account = Some(account);
        workspace.readiness_for_this_account();
        workspace.readiness.budget = Budget::Exhausted {
            resets_at: Some("2026-12-01T12:00:00Z".into()),
        };
        workspace.auth = crate::workspace::Auth::SignedIn;
        workspace.view = View::NewCode;
        workspace.cloud_ui.on = false;
        workspace.code_form.project = None;
        workspace.code_form.agent = SessionAgent::Claude;
        workspace.code_form.via = Some(Via::Nexus);
        workspace.code_form.access = Some(AgentAccess {
            subscriptions: true,
            answer: Some(json!({ "agents": [{ "agent": "claude", "routes": [
                { "route": "gateway", "access": "byok", "available": true },
                { "route": "subscription", "available": true },
            ] }] })),
        });
        workspace.code_agents = vec![AgentInfo {
            agent: SessionAgent::Claude,
            path: Some("/fixture/claude".into()),
            version: None,
            models: None,
            problem: None,
        }];
        workspace.readiness.local = Some(Ok(vec![LocalAgent {
            agent: "claude".into(),
            installed: true,
            path: Some("/fixture/claude".into()),
            version: None,
            login: Some(Login::Plan),
            plan: None,
        }]));
        let stop = workspace.budget_stop().expect("the budget blocks");
        let Stop::Budget { resumes, .. } = &stop else {
            panic!("a budget");
        };
        assert!(resumes.is_some(), "it says when calls resume");
        let palette = Palette::for_choice(ThemeChoice::Dark, gpui_kit::WindowAppearance::Dark);
        assert!(workspace.render_start_blockers(palette, cx).is_some());
        assert!(workspace.render_limit_line(palette, cx).is_none(), "the strip says it in a new session");

        workspace
            .composer
            .update(cx, |input, cx| input.set_value("Rerun the sweep with batch 4", window, cx));
        workspace.composer_empty = false;
        workspace.submit_code("Rerun the sweep with batch 4".into(), false, window, cx);
        assert_eq!(workspace.composer_note, None, "the line says why: no note under it");
        assert!(state::waiting(&held, cx));
        assert_eq!(
            workspace.composer.read(cx).value().as_ref(),
            "Rerun the sweep with batch 4"
        );

        // Read again, still used up: nothing goes.
        workspace.limit_check(cx);
    });
    cx.run_until_parked();
    cx.update(|_, cx| assert!(state::waiting(&held, cx)));

    workspace.update_in(cx, |workspace, _, cx| {
        // Use Max plan: the plan pays, so the budget no longer blocks.
        workspace.code_form.set_via(Some(Via::Subscription));
        assert!(workspace.budget_line().is_none());
        workspace.limit_check(cx);
    });
    cx.run_until_parked();
    cx.update(|_, cx| assert!(!state::waiting(&held, cx), "the held send went"));

    workspace.update_in(cx, |workspace, _, cx| {
        // A chat pays through Gateway: its line sits over the composer.
        workspace.view = View::Chat;
        let palette = Palette::for_choice(ThemeChoice::Dark, gpui_kit::WindowAppearance::Dark);
        assert!(workspace.render_limit_line(palette, cx).is_some());
    });
}
