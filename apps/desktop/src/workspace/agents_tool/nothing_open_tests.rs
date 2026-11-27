//! Headless: the Agents tab of a session with no plan and no agents shows "Nothing open" and the six tools, as the
//! Nexus app's `NothingOpen`. A plan or an agent replaces it. A tool this screen cannot open takes no click.
use super::*;
use crate::workspace::side_pane::Tool;
use crate::workspace::{code_view::CodeView, machine_source::MachineSource};
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, VisualTestContext};

const ID: &str = "nothing-open";

fn agents_tab(events: Vec<WbEvent>, cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(gpui_kit::init);
    let (gateway, _) = crate::gateway::Gateway::start();
    let (_, arrive) = futures::channel::mpsc::unbounded();
    let (_, arrivals) = futures::channel::mpsc::unbounded();
    let (workspace, cx) = cx.add_window_view(|window, cx| {
        let mut workspace = Workspace::with_machine_source(
            Arc::new(gateway),
            arrive,
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
        let events: Vec<StoredEvent> = events
            .into_iter()
            .enumerate()
            .map(|(seq, event)| StoredEvent {
                thread_id: ID.into(),
                seq: seq as i64 + 1,
                at: seq as i64 + 1,
                event,
            })
            .collect();
        let mut state = AgentsTool::resting(ID.into(), cx);
        state.crew = Some(crew_fold::fold_crew(&events, SessionAgent::Claude));
        state.plans.lock().unwrap().push(&events);
        state.has_plan = !state.plans.lock().unwrap().plan().steps.is_empty();
        state.events = Arc::new(events);
        workspace.side.agents = Some(state);
        workspace.side.open = true;
        workspace.side.select_tool(Tool::Agents);
        workspace
    });
    workspace.update_in(cx, |_, window, _| window.activate_window());
    cx.run_until_parked();
    (workspace, cx)
}

fn tool_row(tool: Tool) -> &'static str {
    Box::leak(format!("nothing-open-tool-{}", tool as usize).into_boxed_str())
}

#[gpui_kit::test]
fn an_empty_session_shows_nothing_open_with_the_six_tools(cx: &mut TestAppContext) {
    let (_, cx) = agents_tab(Vec::new(), cx);
    assert!(
        cx.debug_bounds("nothing-open").is_some(),
        "no plan and no agents: Nothing open"
    );
    let rows: Vec<_> = Tool::ALL
        .into_iter()
        .map(|tool| cx.debug_bounds(tool_row(tool)).expect("each tool has a row"))
        .collect();
    assert!(
        rows.windows(2).all(|pair| pair[0].origin.y < pair[1].origin.y),
        "the rows keep the tool order, Browser before Terminal"
    );
}

#[gpui_kit::test]
fn a_plan_replaces_nothing_open(cx: &mut TestAppContext) {
    let plan = WbEvent::Tool {
        id: "todo".into(),
        name: "TodoWrite".into(),
        input: json!({"todos": [{"content": "Read the code", "status": "in_progress"}]}),
        parent: None,
    };
    let (_, cx) = agents_tab(vec![plan], cx);
    assert!(cx.debug_bounds("nothing-open").is_none());
}

#[gpui_kit::test]
fn an_agent_replaces_nothing_open(cx: &mut TestAppContext) {
    let agent = WbEvent::Tool {
        id: "child".into(),
        name: "Agent".into(),
        input: json!({"description": "Inspect layout"}),
        parent: None,
    };
    let (_, cx) = agents_tab(vec![agent], cx);
    assert!(cx.debug_bounds("nothing-open").is_none());
}

#[gpui_kit::test]
fn a_click_opens_the_tool_and_a_tool_not_offered_takes_none(cx: &mut TestAppContext) {
    let (workspace, cx) = agents_tab(Vec::new(), cx);
    // No page is open, so Artifacts is not offered here.
    let artifacts = cx.debug_bounds(tool_row(Tool::Artifacts)).expect("Artifacts has a row");
    cx.simulate_click(artifacts.center(), Modifiers::none());
    workspace.read_with(cx, |workspace, _| assert_eq!(workspace.side_tool(), Tool::Agents));
    let changes = cx.debug_bounds(tool_row(Tool::Changes)).expect("Changes has a row");
    cx.simulate_click(changes.center(), Modifiers::none());
    workspace.read_with(cx, |workspace, _| assert_eq!(workspace.side_tool(), Tool::Changes));
}
