use super::{FRAME, LOOK_WIDTH, PAGE_INSET, SCREEN_W, Step, Workspace, preview_screen_w, step_width};
use crate::workspace::gateway_chat::tests::{Probe, account, fixture};
use gpui_kit::{Global, TestAppContext, px};

pub(super) struct OfflinePlanPage;
impl Global for OfflinePlanPage {}

#[test]
fn narrow_desktop_steps_and_actions_fit_inside_the_gutters() {
    for width in [640., 720., 840., 984., 1280.] {
        for step in Step::ALL {
            assert!(step_width(step, px(width)) + 2. * PAGE_INSET <= width);
        }
    }
    let threshold = LOOK_WIDTH + 2. * PAGE_INSET;
    assert_eq!(step_width(Step::Look, px(threshold - 1.)), SCREEN_W + 2. * FRAME);
    assert_eq!(step_width(Step::Look, px(threshold)), LOOK_WIDTH);
    assert_eq!(preview_screen_w(px(threshold)), SCREEN_W);
    assert_eq!(preview_screen_w(px(420.)), 356.);
    assert_eq!(
        step_width(Step::Look, px(420.)),
        preview_screen_w(px(420.)) + 2. * FRAME
    );
}

#[gpui_kit::test]
fn a_resumed_plan_mounts_once_for_each_account(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    workspace.update_in(cx, |workspace, window, cx| {
        cx.set_global(OfflinePlanPage);
        workspace.account = None;
        workspace.begin_onboarding(Step::Plan, false);
        workspace.open_onboarding_plan(window, cx);
        assert!(workspace.onboarding.flow.as_ref().unwrap().plans.is_none());

        let data = crate::paths::test_root().join("resume-onboarding-plan");
        std::fs::create_dir_all(&data).unwrap();
        let (runner, _events) = crate::code_run::test_runner(data, "http://127.0.0.1:9|org-a");
        workspace.runner = Some(std::rc::Rc::new(runner));
        workspace.handle(crate::gateway::Event::Account(account("org-a", "person-a")), window, cx);
        let plan_id = |workspace: &Workspace| {
            workspace
                .onboarding
                .flow
                .as_ref()?
                .plans
                .as_ref()
                .map(|plan| plan.entity_id())
        };
        let mounted = plan_id(workspace).expect("resuming Plan loads the account's plans");
        workspace.handle(crate::gateway::Event::Account(account("org-a", "person-a")), window, cx);
        assert_eq!(plan_id(workspace), Some(mounted));

        workspace.handle(crate::gateway::Event::Account(account("org-a", "person-b")), window, cx);
        let changed = plan_id(workspace).expect("the next person gets a fresh Plan page");
        assert_ne!(changed, mounted);

        workspace.leave_org();
        assert!(plan_id(workspace).is_none());
        workspace.handle(crate::gateway::Event::Account(account("org-b", "person-b")), window, cx);
        assert_ne!(
            plan_id(workspace).expect("the next organization gets a fresh Plan page"),
            changed
        );
    });
}
