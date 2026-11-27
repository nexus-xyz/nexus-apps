use super::{
    model::{activity, path_to, visible_rows},
    output_text,
};
use nexus_workbench::crew_fold::fold_crew;
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

#[test]
fn selection_reads_only_the_agents_own_work() {
    let events = vec![
        event(
            1,
            WbEvent::Tool {
                id: "a".into(),
                name: "Agent".into(),
                input: json!({"description":"Inspect code"}),
                parent: None,
            },
        ),
        event(
            2,
            WbEvent::Text {
                id: "child".into(),
                text: "Agent answer".into(),
                parent: Some("a".into()),
            },
        ),
        event(
            3,
            WbEvent::Text {
                id: "lead".into(),
                text: "Lead answer".into(),
                parent: None,
            },
        ),
        event(
            4,
            WbEvent::Text {
                id: "other".into(),
                text: "Other agent".into(),
                parent: Some("b".into()),
            },
        ),
    ];
    let crew = fold_crew(&events, SessionAgent::Claude);
    let child = crew.resolve("a").unwrap();
    assert_eq!(
        activity(&events, child)
            .into_iter()
            .map(|row| row.text)
            .collect::<Vec<_>>(),
        vec!["Agent answer"]
    );
    let lead = activity(&events, crew.lead())
        .into_iter()
        .map(|row| row.text)
        .collect::<Vec<_>>();
    assert!(lead.iter().any(|s| s == "Lead answer"));
    assert!(!lead.iter().any(|s| s == "Agent answer" || s == "Other agent"));
    let order = visible_rows(&crew, false, None).agents;
    assert_eq!(order[0].id, "lead");
    assert_eq!(order[1].id, "a");
}

#[test]
fn command_output_preserves_truncation_and_empty_state() {
    assert_eq!(output_text(&json!({"output":"log", "truncated":true})), "…\nlog");
    assert_eq!(output_text(&json!({"output":"", "truncated":false})), "No output yet.");
}

fn tool(seq: i64, id: &str, parent: Option<&str>) -> StoredEvent {
    event(
        seq,
        WbEvent::Tool {
            id: id.into(),
            name: "Agent".into(),
            input: json!({"description": id}),
            parent: parent.map(str::to_string),
        },
    )
}

#[test]
fn finished_parents_of_running_children_never_fold_away() {
    use nexus_workbench::crew_fold::AgentStatus;
    let mut events = vec![tool(1, "parent", None), tool(2, "child", Some("parent"))];
    for ix in 3..10 {
        events.push(tool(ix, &format!("ended-{ix}"), None));
    }
    let mut crew = fold_crew(&events, SessionAgent::Claude);
    for node in &mut crew.nodes {
        if node.id != "child" {
            node.status = AgentStatus::Done;
            node.ended_at = Some(node.started_at);
        }
    }
    let rows = visible_rows(&crew, false, None);
    let ids: Vec<_> = rows.agents.iter().map(|node| node.id.as_str()).collect();
    assert_eq!(&ids[..3], &["lead", "parent", "child"]);
    assert_eq!(rows.hidden, 3);
    let selected = visible_rows(&crew, false, Some("ended-3"));
    assert!(selected.agents.iter().any(|node| node.id == "ended-3"));
    assert_eq!(selected.hidden, 2);
    assert_eq!(visible_rows(&crew, true, None).hidden, 0);
    let path: Vec<_> = path_to(&crew, crew.resolve("child").unwrap())
        .iter()
        .map(|node| node.id.as_str())
        .collect();
    assert_eq!(path, ["lead", "parent", "child"]);
}

#[test]
fn tool_results_belong_to_their_call_even_without_a_parent() {
    let events = vec![
        tool(1, "agent", None),
        event(
            2,
            WbEvent::Tool {
                id: "read".into(),
                name: "Read".into(),
                input: json!({"file_path":"a.rs"}),
                parent: Some("agent".into()),
            },
        ),
        event(
            3,
            WbEvent::ToolResult {
                id: "read".into(),
                output: "actual output".into(),
                images: vec![],
                is_error: false,
                truncated: false,
                parent: None,
            },
        ),
        event(
            4,
            WbEvent::ToolResult {
                id: "unrelated".into(),
                output: "foreign output".into(),
                images: vec![],
                is_error: true,
                truncated: false,
                parent: None,
            },
        ),
    ];
    let crew = fold_crew(&events, SessionAgent::Claude);
    let rows = activity(&events, crew.resolve("agent").unwrap());
    assert_eq!(rows.len(), 1);
    assert!(rows[0].text.contains("actual output"));
    assert!(!rows[0].text.contains("foreign output"));
    assert!(!rows[0].error);
    assert!(
        activity(&events, crew.lead())
            .iter()
            .all(|row| !row.text.contains("actual output"))
    );
}

#[test]
fn literal_output_cannot_close_its_own_markdown_fence() {
    let source = super::model::literal("~~~\n# literal heading");
    assert!(source.starts_with("~~~~text\n"));
    assert!(source.ends_with("\n~~~~"));
}

#[test]
fn keyboard_selection_stays_in_the_visible_list() {
    use super::model::step;
    let ids = ["lead", "a", "b"];
    assert_eq!(step(&ids, None, "down").as_deref(), Some("lead"));
    assert_eq!(step(&ids, Some("lead"), "up").as_deref(), Some("lead"));
    assert_eq!(step(&ids, Some("a"), "down").as_deref(), Some("b"));
    assert_eq!(step(&ids, Some("b"), "down").as_deref(), Some("b"));
    assert_eq!(step(&ids, Some("a"), "home").as_deref(), Some("lead"));
    assert_eq!(step(&[], None, "end"), None);
}

#[test]
fn policy_blocks_are_not_faults_and_keep_the_rule_name() {
    use crate::code::policy_of;
    use nexus_workbench::crew_fold::AgentStatus;
    assert_eq!(
        policy_of(AgentStatus::Failed, Some("Policy: no-web-search")),
        Some("no-web-search")
    );
    assert_eq!(policy_of(AgentStatus::Failed, Some("policy")), Some(""));
    assert_eq!(policy_of(AgentStatus::Failed, Some("policy_error")), None);
    assert_eq!(policy_of(AgentStatus::Failed, Some("provider error")), None);
    assert_eq!(policy_of(AgentStatus::Running, Some("policy: x")), None);
}

fn shell_node(title: &str, recent: &[&str], summary: Option<&str>) -> nexus_workbench::crew_fold::AgentNode {
    let events = vec![event(
        1,
        WbEvent::Tool {
            id: "sh".into(),
            name: "Bash".into(),
            input: json!({"command": title, "run_in_background": true}),
            parent: None,
        },
    )];
    let crew = fold_crew(&events, SessionAgent::Claude);
    let mut node = crew.nodes.last().unwrap().clone();
    node.title = title.into();
    node.summary = summary.map(str::to_string);
    node.recent = recent
        .iter()
        .map(|text| nexus_workbench::crew_fold::Activity {
            text: text.to_string(),
            at: 0,
        })
        .collect();
    node
}

#[test]
fn ports_come_from_output_then_summary_then_the_command_line() {
    use super::model::port_of;
    assert_eq!(
        port_of(&shell_node("pnpm dev", &["Local: http://localhost:5173/"], None)).as_deref(),
        Some("5173")
    );
    assert_eq!(
        port_of(&shell_node("serve", &["on 127.0.0.1:8080"], None)).as_deref(),
        Some("8080")
    );
    assert_eq!(
        port_of(&shell_node("serve", &["bound 0.0.0.0:9000"], None)).as_deref(),
        Some("9000")
    );
    assert_eq!(
        port_of(&shell_node("serve", &["listening [::]:4000"], None)).as_deref(),
        Some("4000")
    );
    assert_eq!(
        port_of(&shell_node("vite --port 3001", &[], None)).as_deref(),
        Some("3001")
    );
    assert_eq!(
        port_of(&shell_node("vite --port=3002", &[], None)).as_deref(),
        Some("3002")
    );
    assert_eq!(
        port_of(&shell_node("PORT=4321 node app.js", &[], None)).as_deref(),
        Some("4321")
    );
    assert_eq!(
        port_of(&shell_node("x", &[], Some("ready on localhost:3000"))).as_deref(),
        Some("3000")
    );
    // The newest output wins over older lines and over the command line.
    assert_eq!(
        port_of(&shell_node(
            "vite --port 1111",
            &["localhost:2222", "localhost:3333"],
            None
        ))
        .as_deref(),
        Some("3333")
    );
    assert_eq!(port_of(&shell_node("cargo test", &["ok: 12 passed"], None)), None);
    assert_eq!(port_of(&shell_node("APORT=5000 x", &[], None)), None);
}

#[test]
fn cost_shares_add_up_to_the_total_and_group_the_rest() {
    use super::model::cost_shares;
    let events: Vec<_> = (1..=6).map(|ix| tool(ix, &format!("a{ix}"), None)).collect();
    let mut crew = fold_crew(&events, SessionAgent::Claude);
    for (ix, node) in crew.nodes.iter_mut().enumerate().skip(1) {
        node.cost_usd = Some(ix as f64);
    }
    // Sub-agents spent 1+2+3+4+5+6 = 21 of 30: the lead 9.
    let shares = cost_shares(&crew, 30.);
    let sum: f64 = shares.iter().map(|s| s.usd).sum();
    assert!((sum - 30.).abs() < 1e-9);
    assert_eq!(shares[0].label, "Lead");
    assert_eq!(shares[0].route, Some(0));
    assert_eq!(shares[0].usd, 9.);
    // The biggest four on their own, in their route hues, then the rest together.
    assert_eq!(shares[1].label, "a6");
    assert_eq!(shares[1].route, Some(1));
    assert_eq!(shares[2].route, Some(5));
    let rest = shares.last().unwrap();
    assert_eq!(rest.route, None);
    assert_eq!(rest.usd, 3.);
    assert_eq!(shares.len(), 6);
    // Nothing priced: no slices.
    let bare = fold_crew(&events, SessionAgent::Claude);
    assert!(cost_shares(&bare, 0.).is_empty());
    // Sub-agents priced past the total: the lead takes nothing, never less.
    assert!(cost_shares(&crew, 10.).iter().all(|s| s.label != "Lead"));
}

#[test]
fn the_tab_counts_live_sub_agents_and_rings_for_the_one_waiting() {
    use super::model::{live_count, waiting_route};
    use nexus_workbench::crew_fold::AgentStatus;
    let events: Vec<_> = (1..=3).map(|ix| tool(ix, &format!("a{ix}"), None)).collect();
    let mut crew = fold_crew(&events, SessionAgent::Claude);
    assert_eq!(live_count(&crew), 3);
    assert_eq!(waiting_route(&crew), None);
    crew.nodes[1].status = AgentStatus::Done;
    crew.nodes[2].status = AgentStatus::Needs;
    assert_eq!(live_count(&crew), 2);
    assert_eq!(waiting_route(&crew), Some(2));
}

#[test]
fn peek_shows_the_last_three_steps() {
    let mut events = vec![tool(1, "agent", None)];
    for ix in 2..7 {
        events.push(event(
            ix,
            WbEvent::Text {
                id: format!("t{ix}"),
                text: format!("step {ix}"),
                parent: Some("agent".into()),
            },
        ));
    }
    let crew = fold_crew(&events, SessionAgent::Claude);
    assert_eq!(
        super::model::peek(&events, crew.resolve("agent").unwrap()),
        ["step 4", "step 5", "step 6"]
    );
}

#[test]
fn folding_a_branch_hides_its_descendants_but_keeps_its_row() {
    let crew = fold_crew(
        &[
            tool(1, "parent", None),
            tool(2, "child", Some("parent")),
            tool(3, "peer", None),
        ],
        SessionAgent::Claude,
    );
    let collapsed = ["parent".to_string()].into_iter().collect();
    let rows = super::model::folded_rows(&crew, true, None, &collapsed);
    let ids: Vec<_> = rows.agents.iter().map(|node| node.id.as_str()).collect();
    assert_eq!(ids, ["lead", "parent", "peer"]);
    let open = super::model::folded_rows(&crew, true, None, &Default::default());
    assert!(open.agents.iter().any(|node| node.id == "child"));
}
