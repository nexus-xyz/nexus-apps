//! Headless: in a pane too narrow for the tabs' names, the Agents tab, icon only, still holds its count of live
//! sub-agents. At a fixed 30px the count spilled onto the next tab.
use super::*;
use crate::workspace::{code_view::CodeView, machine_source::MachineSource};
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, VisualTestContext};

const ID: &str = "compact-tabs";

fn narrow_pane_with_a_live_subagent(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
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
        workspace._code_poll = Task::ready(());
        workspace.code = None;
        workspace.runner = None;
        workspace.auth = crate::workspace::Auth::SignedIn;
        let mut view = CodeView::new(ID.into(), window, cx);
        view.loaded = true;
        workspace.view = View::Code(view);
        workspace.side.fixture_session(ID);
        let mut state = AgentsTool::new(ID.into(), SessionAgent::Claude, cx);
        let events = vec![StoredEvent {
            thread_id: ID.into(),
            seq: 1,
            at: 1,
            event: WbEvent::Tool {
                id: "child".into(),
                name: "Agent".into(),
                input: json!({"description": "Inspect layout"}),
                parent: None,
            },
        }];
        state.crew = Some(crew_fold::fold_crew(&events, SessionAgent::Claude));
        state.events = Arc::new(events);
        state.next = Instant::now() + Duration::from_secs(3600);
        workspace.side.agents = Some(state);
        workspace.side.open = true;
        workspace.side.select_tool(super::super::side_pane::Tool::Files);
        workspace.side.fixture_width(0.);
        workspace
    });
    workspace.update_in(cx, |_, window, _| window.activate_window());
    cx.run_until_parked();
    (workspace, cx)
}

#[gpui_kit::test]
fn a_compact_agents_tab_holds_its_count(cx: &mut TestAppContext) {
    let (workspace, cx) = narrow_pane_with_a_live_subagent(cx);
    workspace.update(cx, |workspace, _| {
        let crew = workspace.crew().expect("the session has a crew");
        assert_eq!(model::live_count(crew), 1, "one sub-agent is live");
    });
    let agents = super::super::side_pane::Tool::Agents as usize;
    let tab = cx
        .debug_bounds(Box::leak(format!("side-tab-{agents}").into_boxed_str()))
        .expect("the Agents tab draws");
    let count = cx
        .debug_bounds("side-tab-agents-count")
        .expect("the Agents tab shows its count");
    assert!(
        f32::from(tab.size.width) < 70.,
        "the narrow pane draws Agents icon only: {tab:?}"
    );
    assert!(
        tab.contains(&count.origin) && tab.contains(&count.bottom_right()),
        "the count sits inside its tab: tab {tab:?}, count {count:?}"
    );
}
