use super::*;
use nexus_workbench::crew_fold::{Control, Crew, fold_crew};
use nexus_workbench::event::{SessionAgent, StoredEvent, WbEvent};
use serde_json::json;

fn event(seq: i64, event: WbEvent) -> StoredEvent {
    StoredEvent {
        thread_id: "t".into(),
        seq,
        at: seq * 1000,
        event,
    }
}

fn tool(seq: i64, id: &str, input: serde_json::Value, parent: Option<&str>) -> StoredEvent {
    event(
        seq,
        WbEvent::Tool {
            id: id.into(),
            name: "Agent".into(),
            input,
            parent: parent.map(str::to_string),
        },
    )
}

/// The lead starts "a", which starts "b".
fn nested() -> (Vec<StoredEvent>, Crew) {
    let events = vec![
        tool(
            1,
            "a",
            json!({"description":"Review code","prompt":"Read src and list bugs"}),
            None,
        ),
        tool(2, "b", json!({"description":"Check tests"}), Some("a")),
    ];
    let crew = fold_crew(&events, SessionAgent::Claude);
    (events, crew)
}

fn path<'a>(crew: &'a Crew, id: &str) -> Vec<&'a AgentNode> {
    super::super::model::path_to(crew, crew.resolve(id).unwrap())
}

fn on() -> Control {
    Control {
        available: true,
        reason: None,
    }
}

fn off(reason: &'static str) -> Control {
    Control {
        available: false,
        reason: Some(reason),
    }
}

fn controls(stop: Control, to: Option<MessageTo>) -> Controls {
    Controls {
        stop: stop.clone(),
        background: stop.clone(),
        output: stop,
        message: off("Sub-agents can't take messages. Message the lead instead."),
        message_to: to,
    }
}

#[test]
fn crumbs_go_back_up_to_the_conversation() {
    let (_, crew) = nested();
    let crumbs = crumbs("Fix the build", &path(&crew, "b"));
    let labels: Vec<_> = crumbs.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(labels, ["Fix the build", "Lead", "Review code", "Check tests"]);
    assert_eq!(crumbs[0].target, Target::Conversation);
    assert_eq!(crumbs[1].target, Target::Conversation);
    assert_eq!(crumbs[2].target, Target::Agent("a".into()));
    assert_eq!(crumbs[3].target, Target::Here);
}

#[test]
fn an_untitled_session_reads_your_session() {
    let (_, crew) = nested();
    assert_eq!(crumbs("  ", &path(&crew, "a"))[0].label, "Your session");
}

#[test]
fn facts_name_status_steps_cost_time_and_model() {
    let (_, crew) = nested();
    let mut node = crew.resolve("a").unwrap().clone();
    node.started_at = 0;
    node.ended_at = None;
    node.steps = 14;
    node.cost_usd = Some(0.19);
    node.model = Some("opus".into());
    node.role = None;
    node.status = AgentStatus::Running;
    assert_eq!(
        facts(&node, "Working", 72_000),
        "Working · 14 steps · $0.19 · 1m 12s · opus"
    );
    node.steps = 1;
    node.cost_usd = None;
    node.model = None;
    assert_eq!(facts(&node, "Working", 5_000), "Working · 1 step · 5s");
}

#[test]
fn an_ended_agent_reports_its_own_run_time() {
    let (_, crew) = nested();
    let mut node = crew.resolve("a").unwrap().clone();
    node.status = AgentStatus::Done;
    node.started_at = 0;
    node.ended_at = Some(90_000);
    node.duration_ms = Some(30_000);
    assert_eq!(ran(&node, 500_000), "30s");
    node.duration_ms = None;
    assert_eq!(ran(&node, 500_000), "1m 30s");
}

#[test]
fn inferred_status_says_where_it_was_read() {
    let (_, crew) = nested();
    let mut node = crew.resolve("a").unwrap().clone();
    node.status_source = StatusSource::Inferred;
    assert_eq!(inferred(&node), Some("(read from the session's tool calls)"));
    node.status_source = StatusSource::Reported;
    assert_eq!(inferred(&node), None);
}

#[test]
fn stop_shows_only_for_a_live_controllable_agent() {
    let (_, crew) = nested();
    let mut node = crew.resolve("a").unwrap().clone();
    node.status = AgentStatus::Running;
    node.task_id = Some("task-1".into());
    let can = controls(on(), Some(MessageTo::Lead));
    assert_eq!(stop(&node, &can, true), Stop::Button("task-1".into()));
    // Read-only here: no controls at all.
    assert_eq!(stop(&node, &can, false), Stop::None);
    let cannot = controls(off("Update Claude Code."), Some(MessageTo::Lead));
    assert_eq!(stop(&node, &cannot, true), Stop::Why("Update Claude Code.".into()));
    node.status = AgentStatus::Done;
    assert_eq!(stop(&node, &can, true), Stop::None);
}

#[test]
fn background_needs_its_call_and_a_live_agent() {
    let (_, crew) = nested();
    let mut node = crew.resolve("a").unwrap().clone();
    node.status = AgentStatus::Running;
    let can = controls(on(), Some(MessageTo::Lead));
    assert_eq!(background(&node, &can, true), Some("a".into()));
    assert_eq!(background(&node, &can, false), None);
}

#[test]
fn the_dock_sends_a_sub_agents_message_to_the_lead() {
    let lead = controls(on(), Some(MessageTo::Lead));
    assert_eq!(dock(&lead, true), Dock::Lead);
    assert_eq!(dock(&lead, false), Dock::ReadOnly);
    let mut shell = controls(on(), None);
    shell.message = off("A command takes no messages.");
    assert_eq!(dock(&shell, true), Dock::Note("A command takes no messages."));
    assert_eq!(
        about("Review code", " look at main.rs "),
        "About the sub-agent \"Review code\": look at main.rs"
    );
    assert_eq!(about("Review code", "  "), "");
}

#[test]
fn its_task_is_the_prompt_unless_it_repeats_the_title() {
    let (events, crew) = nested();
    assert_eq!(
        task(&events, crew.resolve("a").unwrap()).as_deref(),
        Some("Read src and list bugs")
    );
    // "b" has only a description, which is its title.
    assert_eq!(task(&events, crew.resolve("b").unwrap()), None);
}

#[test]
fn how_it_ended_shows_once_it_ended() {
    let (_, crew) = nested();
    let mut node = crew.resolve("a").unwrap().clone();
    node.summary = Some("Found 3 bugs".into());
    node.status = AgentStatus::Running;
    assert_eq!(ending(&node), None);
    node.status = AgentStatus::Done;
    assert_eq!(ending(&node), Some(("How it ended", "Found 3 bugs".into())));
    node.status = AgentStatus::Failed;
    assert_eq!(ending(&node).unwrap().0, "How it ended: it failed");
}
