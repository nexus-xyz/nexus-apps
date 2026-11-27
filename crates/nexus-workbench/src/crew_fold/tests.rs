//! crew.test.ts's cases, on the same logs. The perf fixture's and the mock crews' cases stay in TypeScript: they
//! play generators that live there (`perf-fixture.ts`, `mock-crew.ts`); "folds the same in pieces" covers the fold's
//! own part of them here.

use serde_json::{Value, json};

use super::*;
use crate::event::{BackgroundTask, Decision, TaskDetail, TaskStatus, TurnStatus};

struct Log {
    seq: i64,
}

impl Log {
    fn new() -> Log {
        Log { seq: 0 }
    }

    fn at(&mut self, ms: i64, event: WbEvent) -> StoredEvent {
        self.seq += 1;
        StoredEvent {
            thread_id: "t".into(),
            seq: self.seq,
            at: ms,
            event,
        }
    }

    fn tool(&mut self, ms: i64, id: &str, name: &str, input: Value) -> StoredEvent {
        self.tool_of(ms, id, name, input, None)
    }

    fn tool_of(&mut self, ms: i64, id: &str, name: &str, input: Value, parent: Option<&str>) -> StoredEvent {
        self.at(
            ms,
            WbEvent::Tool {
                id: id.into(),
                name: name.into(),
                input,
                parent: parent.map(str::to_string),
            },
        )
    }

    fn result(&mut self, ms: i64, id: &str, output: &str, is_error: bool) -> StoredEvent {
        self.at(
            ms,
            WbEvent::ToolResult {
                id: id.into(),
                output: output.into(),
                images: vec![],
                is_error,
                truncated: false,
                parent: None,
            },
        )
    }

    fn report(&mut self, ms: i64, task_id: &str, status: TaskStatus, extra: Extra) -> StoredEvent {
        self.at(
            ms,
            WbEvent::Task {
                task_id: task_id.into(),
                tool_use_id: extra.tool_use_id.map(str::to_string),
                task_type: extra.task_type.map(str::to_string),
                agent_type: None,
                status,
                description: extra.description.unwrap_or("").into(),
                tool_count: extra.tool_count,
                total_tokens: extra.total_tokens,
                duration_ms: extra.duration_ms,
                detail: extra.detail.map(Box::new),
            },
        )
    }

    fn list(&mut self, ms: i64, tasks: &[(&str, &str)]) -> StoredEvent {
        self.at(
            ms,
            WbEvent::BackgroundTasks {
                tasks: tasks
                    .iter()
                    .map(|(task_id, task_type)| BackgroundTask {
                        task_id: (*task_id).into(),
                        task_type: Some((*task_type).into()),
                        description: (*task_id).into(),
                    })
                    .collect(),
            },
        )
    }

    fn list_described(&mut self, ms: i64, task_id: &str, task_type: &str, description: &str) -> StoredEvent {
        self.at(
            ms,
            WbEvent::BackgroundTasks {
                tasks: vec![BackgroundTask {
                    task_id: task_id.into(),
                    task_type: Some(task_type.into()),
                    description: description.into(),
                }],
            },
        )
    }

    fn ask_of(&mut self, ms: i64, request_id: &str, tool_use_id: &str, parent: Option<&str>) -> StoredEvent {
        self.at(
            ms,
            WbEvent::Approval {
                request_id: request_id.into(),
                tool_use_id: Some(tool_use_id.into()),
                tool: "Bash".into(),
                input: json!({ "command": "touch x" }),
                description: None,
                suggestions: vec![],
                parent: parent.map(str::to_string),
            },
        )
    }

    fn ask(&mut self, ms: i64, request_id: &str, tool_use_id: &str) -> StoredEvent {
        self.ask_of(ms, request_id, tool_use_id, None)
    }

    fn answer(&mut self, ms: i64, request_id: &str) -> StoredEvent {
        self.at(
            ms,
            WbEvent::ApprovalResolved {
                request_id: request_id.into(),
                decision: Decision::Allow,
                message: None,
            },
        )
    }

    fn turn_start(&mut self, ms: i64) -> StoredEvent {
        self.at(ms, WbEvent::TurnStart { turn_id: "u".into() })
    }

    fn turn_end(&mut self, ms: i64, status: TurnStatus) -> StoredEvent {
        self.at(
            ms,
            WbEvent::TurnEnd {
                turn_id: "u".into(),
                status,
                cost_usd: None,
                duration_ms: 0,
                error: None,
                cursor: None,
            },
        )
    }

    fn usage(&mut self, ms: i64, parent: Option<&str>, model: &str, input: u64, output: u64, cost: f64) -> StoredEvent {
        self.at(
            ms,
            WbEvent::Usage {
                model: model.into(),
                input_tokens: input,
                output_tokens: output,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: cost,
                parent: parent.map(str::to_string),
            },
        )
    }

    /// A 2.1.292 background agent as the core logs it: the call, the list, the start report, the launch receipt.
    fn launched(&mut self) -> Vec<StoredEvent> {
        vec![
            self.tool(
                1,
                "toolu_A",
                "Agent",
                json!({ "description": "outer", "subagent_type": "general-purpose", "prompt": "go" }),
            ),
            self.list(2, &[("aA", "local_agent")]),
            self.report(
                3,
                "aA",
                TaskStatus::Running,
                Extra::call("toolu_A").typed("local_agent").said("outer"),
            ),
            self.result(4, "toolu_A", "Async agent launched successfully.", false),
        ]
    }
}

#[derive(Default, Clone)]
struct Extra {
    tool_use_id: Option<&'static str>,
    task_type: Option<&'static str>,
    description: Option<&'static str>,
    tool_count: Option<u64>,
    total_tokens: Option<u64>,
    duration_ms: Option<u64>,
    detail: Option<TaskDetail>,
}

impl Extra {
    fn none() -> Extra {
        Extra::default()
    }
    fn call(id: &'static str) -> Extra {
        Extra {
            tool_use_id: Some(id),
            ..Extra::default()
        }
    }
    fn typed(mut self, kind: &'static str) -> Extra {
        self.task_type = Some(kind);
        self
    }
    fn said(mut self, description: &'static str) -> Extra {
        self.description = Some(description);
        self
    }
    fn tools(mut self, count: u64) -> Extra {
        self.tool_count = Some(count);
        self
    }
    fn tokens(mut self, tokens: u64) -> Extra {
        self.total_tokens = Some(tokens);
        self
    }
    fn took(mut self, ms: u64) -> Extra {
        self.duration_ms = Some(ms);
        self
    }
    fn detail(mut self, detail: TaskDetail) -> Extra {
        self.detail = Some(detail);
        self
    }
}

fn s(value: &str) -> Option<String> {
    Some(value.to_string())
}

fn cat(parts: Vec<Vec<StoredEvent>>) -> Vec<StoredEvent> {
    parts.into_iter().flatten().collect()
}

fn node<'a>(crew: &'a Crew, id: &str) -> &'a AgentNode {
    crew.resolve(id).unwrap_or_else(|| panic!("no node {id}"))
}

fn order(crew: &Crew) -> Vec<&str> {
    crew.order().collect()
}

use AgentStatus::*;
use StatusSource::*;

// crew: what tasks.ts got wrong

#[test]
fn a_2_1_292_launch_result_does_not_end_its_agent() {
    let mut log = Log::new();
    let launched = log.launched();
    let crew = fold_crew(&launched, SessionAgent::Claude);
    assert_eq!(order(&crew), ["toolu_A"]);
    let agent = node(&crew, "toolu_A");
    assert_eq!(
        (
            agent.status,
            agent.status_source,
            agent.backgrounded,
            agent.task_id.as_deref()
        ),
        (Running, Reported, Some(true), Some("aA"))
    );
    assert_eq!(node(&crew, "aA").id, "toolu_A");
    let more = vec![
        log.list(5, &[]),
        log.report(
            6,
            "aA",
            TaskStatus::Completed,
            Extra::call("toolu_A").said("outer done"),
        ),
    ];
    let ended = fold_crew(&cat(vec![launched, more]), SessionAgent::Claude);
    let agent = node(&ended, "toolu_A");
    assert_eq!(
        (
            agent.status,
            agent.status_source,
            agent.ended_at,
            agent.summary.as_deref()
        ),
        (Done, Reported, Some(5), Some("outer done"))
    );
}

#[test]
fn a_running_monitor_watcher_stays_running_when_its_call_answers_with_no_command_id() {
    let mut log = Log::new();
    let watch = vec![
        log.tool(
            1,
            "toolu_M",
            "Monitor",
            json!({ "command": "bash poll.sh", "description": "new PRs", "timeout_ms": 1_800_000 }),
        ),
        log.list(2, &[("bM", "local_bash")]),
        log.report(
            2,
            "bM",
            TaskStatus::Running,
            Extra::call("toolu_M").typed("local_bash").said("new PRs"),
        ),
        log.result(
            3,
            "toolu_M",
            "Monitor started (task bM, expires in 30m unless the source ends first).",
            false,
        ),
    ];
    let crew = fold_crew(&watch, SessionAgent::Claude);
    let shell = node(&crew, "bM");
    assert_eq!(
        (shell.kind, shell.status, shell.status_source),
        (AgentKind::Shell, Running, Reported)
    );
    assert_eq!(
        (
            crew.counts.shells_running,
            crew.counts.agents_running,
            crew.counts.failed
        ),
        (1, 0, 0)
    );
    let more = vec![
        log.list(9, &[]),
        log.report(
            9,
            "bM",
            TaskStatus::Completed,
            Extra::call("toolu_M").said("watch ended"),
        ),
    ];
    let ended = fold_crew(&cat(vec![watch, more]), SessionAgent::Claude);
    assert_eq!(
        (node(&ended, "bM").status, node(&ended, "bM").status_source),
        (Done, Reported)
    );
}

#[test]
fn a_monitor_reported_without_the_background_flag_then_listed_stays_running() {
    let mut log = Log::new();
    let watch = vec![
        log.tool(
            1,
            "toolu_N",
            "Monitor",
            json!({ "command": "bash poll.sh", "description": "new PRs" }),
        ),
        log.report(
            2,
            "bN",
            TaskStatus::Running,
            Extra::call("toolu_N").typed("local_bash").said("new PRs"),
        ),
        log.list(2, &[("bN", "local_bash")]),
        log.result(
            3,
            "toolu_N",
            "Monitor started (task bN, expires in 30m unless the source ends first).",
            false,
        ),
    ];
    let crew = fold_crew(&watch, SessionAgent::Claude);
    assert_eq!(
        (node(&crew, "bN").kind, node(&crew, "bN").status),
        (AgentKind::Shell, Running)
    );
    assert_eq!((crew.counts.shells_running, crew.counts.failed), (1, 0));
}

#[test]
fn a_reported_background_command_whose_call_answers_with_an_error_fails() {
    let mut log = Log::new();
    let events = vec![
        log.tool(1, "toolu_E", "Monitor", json!({ "command": "bash poll.sh" })),
        log.list(2, &[("bE", "local_bash")]),
        log.report(2, "bE", TaskStatus::Running, Extra::call("toolu_E").typed("local_bash")),
        log.result(3, "toolu_E", "Monitor could not start", true),
    ];
    let crew = fold_crew(&events, SessionAgent::Claude);
    let shell = node(&crew, "bE");
    assert_eq!(
        (shell.kind, shell.status, shell.status_source),
        (AgentKind::Shell, Failed, Inferred)
    );
}

#[test]
fn a_background_command_nothing_reported_fails_when_its_call_answers_with_no_id() {
    let mut log = Log::new();
    let events = vec![
        log.tool(
            1,
            "toolu_X",
            "Bash",
            json!({ "command": "sleep 5", "run_in_background": true }),
        ),
        log.result(2, "toolu_X", "spawn failed", false),
    ];
    let crew = fold_crew(&events, SessionAgent::Claude);
    let shell = node(&crew, "toolu_X");
    assert_eq!(
        (shell.kind, shell.status, shell.status_source),
        (AgentKind::Shell, Failed, Inferred)
    );
}

#[test]
fn a_foreground_commands_report_makes_no_node_and_a_listing_promotes_it() {
    let mut log = Log::new();
    let foreground = vec![
        log.tool(1, "toolu_B", "Bash", json!({ "command": "npm test" })),
        log.report(
            2,
            "bq",
            TaskStatus::Running,
            Extra::call("toolu_B").typed("local_bash").said("npm test"),
        ),
    ];
    assert!(order(&fold_crew(&foreground, SessionAgent::Claude)).is_empty());
    let done = log.report(3, "bq", TaskStatus::Completed, Extra::none());
    assert!(
        order(&fold_crew(
            &cat(vec![foreground.clone(), vec![done]]),
            SessionAgent::Claude
        ))
        .is_empty()
    );
    let mut log = Log { seq: 2 };
    let moved = fold_crew(
        &cat(vec![foreground, vec![log.list(3, &[("bq", "local_bash")])]]),
        SessionAgent::Claude,
    );
    assert_eq!(order(&moved), ["toolu_B"]);
    let shell = node(&moved, "bq");
    assert_eq!(
        (
            shell.kind,
            shell.title.as_str(),
            shell.status,
            shell.backgrounded,
            shell.call_id.as_deref()
        ),
        (AgentKind::Shell, "npm test", Running, Some(true), Some("toolu_B"))
    );
}

#[test]
fn a_restart_notice_stops_every_running_node() {
    let mut log = Log::new();
    let mut events = log.launched();
    events.push(log.at(9, WbEvent::Notice {
        text: "Agent restarted. Your next message resumes the conversation in a new process, which reads MCP servers, skills and settings again.".into(),
    }));
    let crew = fold_crew(&events, SessionAgent::Claude);
    let agent = node(&crew, "toolu_A");
    assert_eq!(
        (agent.status, agent.status_source, agent.ended_at),
        (Stopped, Inferred, Some(9))
    );
}

#[test]
fn a_stopped_parent_keeps_its_running_background_child() {
    let mut log = Log::new();
    let mut events = log.launched();
    events.push(log.tool_of(
        5,
        "toolu_H",
        "Agent",
        json!({ "description": "helper", "run_in_background": true }),
        Some("toolu_A"),
    ));
    events.push(log.report(
        6,
        "aH",
        TaskStatus::Running,
        Extra::call("toolu_H").typed("local_agent"),
    ));
    events.push(log.report(7, "aA", TaskStatus::Stopped, Extra::none()));
    let crew = fold_crew(&events, SessionAgent::Claude);
    assert_eq!(node(&crew, "toolu_A").status, Stopped);
    let helper = node(&crew, "toolu_H");
    assert_eq!(
        (helper.status, helper.parent_id.as_deref(), helper.depth),
        (Running, Some("toolu_A"), 2)
    );
    assert_eq!(node(&crew, "toolu_A").children, ["toolu_H"]);
}

#[test]
fn two_approvals_on_one_agent_answering_one_keeps_it_needing_you() {
    let mut log = Log::new();
    let mut events = log.launched();
    events.push(log.tool_of(5, "c1", "Bash", json!({ "command": "a" }), Some("toolu_A")));
    events.push(log.tool_of(5, "c2", "Bash", json!({ "command": "b" }), Some("toolu_A")));
    events.push(log.ask(6, "r1", "c1"));
    events.push(log.ask(7, "r2", "c2"));
    let asking = fold_crew(&events, SessionAgent::Claude);
    assert_eq!(node(&asking, "toolu_A").status, Needs);
    let requests: Vec<&str> = node(&asking, "toolu_A")
        .needs
        .iter()
        .map(|need| need.request_id.as_str())
        .collect();
    assert_eq!(requests, ["r1", "r2"]);
    assert_eq!(
        asking.approvals,
        [
            ("r1".to_string(), "toolu_A".to_string()),
            ("r2".into(), "toolu_A".into())
        ]
    );
    assert_eq!(asking.counts.needs, 1);
    let mut one = events.clone();
    one.push(log.answer(8, "r1"));
    assert_eq!(node(&fold_crew(&one, SessionAgent::Claude), "toolu_A").status, Needs);
    one.push(log.answer(9, "r2"));
    let both = fold_crew(&one, SessionAgent::Claude);
    assert_eq!(node(&both, "toolu_A").status, Running);
    assert!(node(&both, "toolu_A").needs.is_empty());
    assert!(both.needs.is_empty());
}

#[test]
fn an_approval_names_its_owner_through_its_tools_parent() {
    let mut log = Log::new();
    let mut events = log.launched();
    events.push(log.tool_of(
        5,
        "toolu_I",
        "Agent",
        json!({ "description": "inner" }),
        Some("toolu_A"),
    ));
    events.push(log.tool_of(6, "c1", "Bash", json!({ "command": "touch x" }), Some("toolu_I")));
    events.push(log.ask(7, "r1", "c1"));
    events.push(log.tool(8, "c2", "Bash", json!({ "command": "rm -rf build" })));
    events.push(log.ask(9, "r2", "c2"));
    let crew = fold_crew(&events, SessionAgent::Claude);
    let owner = |request: &str| {
        crew.approvals
            .iter()
            .find(|(id, _)| id == request)
            .map(|(_, node)| node.as_str())
    };
    assert_eq!(owner("r1"), Some("toolu_I"));
    assert_eq!(owner("r2"), Some(LEAD_ID));
    assert_eq!(crew.needs, ["toolu_I", LEAD_ID]);
    assert_eq!(node(&crew, "toolu_A").status, Running);
}

// crew: what the recordings show

#[test]
fn a_hidden_sub_agents_foreground_command_makes_no_node() {
    let mut log = Log::new();
    let mut events = log.launched();
    events.push(log.report(
        5,
        "bab3",
        TaskStatus::Running,
        Extra::call("toolu_hidden").typed("local_bash").said("for i in 1 2 3"),
    ));
    events.push(log.report(9, "bab3", TaskStatus::Stopped, Extra::none()));
    let crew = fold_crew(&events, SessionAgent::Claude);
    assert_eq!(order(&crew), ["toolu_A"]);
    assert_eq!((crew.counts.shells_running, crew.counts.total), (0, 1));
    // A background one is listed, before or after its start report.
    let mut log = Log::new();
    let mut events = log.launched();
    events.push(log.report(
        5,
        "b7",
        TaskStatus::Running,
        Extra::call("toolu_unseen").typed("local_bash").said("tail -f log"),
    ));
    events.push(log.list(6, &[("b7", "local_bash")]));
    let listed = fold_crew(&events, SessionAgent::Claude);
    let shell = node(&listed, "b7");
    assert_eq!(
        (shell.kind, shell.status, shell.title.as_str()),
        (AgentKind::Shell, Running, "tail -f log")
    );
}

#[test]
fn links_a_listed_task_to_its_call_before_the_start_report() {
    let mut log = Log::new();
    let call = log.tool(1, "toolu_A", "Agent", json!({ "description": "outer", "prompt": "go" }));
    let listed = log.list_described(2, "aA", "local_agent", "outer");
    let crew = fold_crew(&[call.clone(), listed], SessionAgent::Claude);
    assert_eq!(order(&crew), ["toolu_A"]);
    assert_eq!(
        (node(&crew, "aA").id.as_str(), node(&crew, "aA").backgrounded),
        ("toolu_A", Some(true))
    );
    // Two calls with the list's description: no guess, a node of its own until the report links it.
    let mut log = Log { seq: 1 };
    let twice = fold_crew(
        &[
            call,
            log.tool(2, "toolu_B", "Agent", json!({ "description": "outer" })),
            log.list_described(3, "aA", "local_agent", "outer"),
        ],
        SessionAgent::Claude,
    );
    assert_eq!(order(&twice), ["toolu_A", "toolu_B", "aA"]);
}

#[test]
fn links_a_background_command_listed_under_its_command_line() {
    let mut log = Log::new();
    let events = vec![
        log.tool(
            1,
            "toolu_S",
            "Bash",
            json!({ "command": "sleep 5", "run_in_background": true }),
        ),
        log.list_described(2, "b1", "local_bash", "sleep 5"),
    ];
    let crew = fold_crew(&events, SessionAgent::Claude);
    assert_eq!(order(&crew), ["toolu_S"]);
    assert_eq!(crew.counts.shells_running, 1);
    assert_eq!(node(&crew, "b1").id, "toolu_S");
}

#[test]
fn hands_out_the_current_node_that_needs_you_after_it_changes_otherwise() {
    let mut log = Log::new();
    let mut folder = CrewFolder::new(SessionAgent::Claude);
    let mut events = vec![
        log.turn_start(1),
        log.tool(2, "toolu_A", "Agent", json!({ "description": "a" })),
        log.tool_of(3, "c1", "Bash", json!({ "command": "a" }), Some("toolu_A")),
        log.ask(4, "r1", "c1"),
    ];
    assert_eq!(folder.sync(&events).needs, ["toolu_A"]);
    events.push(log.tool_of(5, "c2", "Read", json!({ "file_path": "/x" }), Some("toolu_A")));
    let crew = folder.sync(&events);
    let agent = node(crew, &crew.needs[0]);
    assert_eq!(agent.steps, 2);
    assert_eq!(agent.last.as_ref().map(|last| last.text.as_str()), Some("Read /x"));
}

#[test]
fn starts_a_new_run_fresh_no_duration_or_summary_from_the_run_before() {
    let mut log = Log::new();
    let mut events = log.launched();
    events.push(log.report(
        5,
        "aA",
        TaskStatus::Completed,
        Extra::call("toolu_A").said("first run").took(6167),
    ));
    let crew = fold_crew(&events, SessionAgent::Claude);
    assert_eq!(
        (
            node(&crew, "toolu_A").duration_ms,
            node(&crew, "toolu_A").summary.as_deref()
        ),
        (Some(6167), Some("first run"))
    );
    events.push(log.report(9, "aA", TaskStatus::Running, Extra::call("toolu_A")));
    let again = fold_crew(&events, SessionAgent::Claude);
    let agent = node(&again, "toolu_A");
    assert_eq!(
        (
            agent.status,
            agent.duration_ms,
            agent.summary.as_deref(),
            agent.ended_at
        ),
        (Running, None, None, None)
    );
}

#[test]
fn counts_a_hidden_sub_agents_reported_tool_calls_in_its_parents_steps() {
    let mut log = Log::new();
    let mut events = log.launched();
    events.push(log.tool_of(
        5,
        "toolu_W",
        "Agent",
        json!({ "description": "worker", "run_in_background": false }),
        Some("toolu_A"),
    ));
    events.push(log.report(6, "aW", TaskStatus::Running, Extra::call("toolu_W")));
    events.push(log.report(9, "aW", TaskStatus::Completed, Extra::call("toolu_W").tools(3)));
    let crew = fold_crew(&events, SessionAgent::Claude);
    assert_eq!(
        (node(&crew, "toolu_W").tool_count, node(&crew, "toolu_W").steps),
        (3, 3)
    );
    assert_eq!(
        (node(&crew, "toolu_A").tool_count, node(&crew, "toolu_A").steps),
        (1, 4)
    );
    assert_eq!(crew.lead().steps, 5);
    // Its frames show later: what was reported is not counted twice.
    events.push(log.tool_of(10, "c1", "Read", json!({ "file_path": "x" }), Some("toolu_W")));
    let shown = fold_crew(&events, SessionAgent::Claude);
    assert_eq!(
        (node(&shown, "toolu_W").tool_count, node(&shown, "toolu_W").steps),
        (3, 3)
    );
}

#[test]
fn keeps_a_codex_child_running_when_the_leads_turn_is_cut_short() {
    let mut log = Log::new();
    let child = vec![
        log.turn_start(1),
        log.tool(2, "call_1", "Agent", json!({ "description": "core", "prompt": "" })),
        log.report(
            2,
            "thr_1",
            TaskStatus::Running,
            Extra::call("call_1").typed("local_agent"),
        ),
        log.turn_end(3, TurnStatus::Interrupted),
    ];
    let crew = fold_crew(&child, SessionAgent::Codex);
    assert_eq!(
        (node(&crew, "call_1").status, node(&crew, "call_1").status_source),
        (Running, Reported)
    );
    assert_eq!(crew.lead().status, Waiting);
    let mut log = Log::new();
    let failed = fold_crew(
        &[log.turn_start(1), log.turn_end(2, TurnStatus::Error)],
        SessionAgent::Claude,
    );
    assert_eq!(failed.lead().status, Failed);
    let mut log = Log::new();
    let launch = fold_crew(
        &[
            log.tool(
                1,
                "a",
                "Agent",
                json!({ "description": "d", "run_in_background": true }),
            ),
            log.result(2, "a", "boom", true),
        ],
        SessionAgent::Claude,
    );
    assert_eq!(
        (node(&launch, "a").status, node(&launch, "a").status_source),
        (Failed, Inferred)
    );
}

// crew: the core's fields

fn detail() -> TaskDetail {
    TaskDetail::default()
}

/// A foreground sub-agent of `outer` whose frames Claude Code never forwards: only its reports reach the log.
fn hidden(log: &mut Log) -> Vec<StoredEvent> {
    let mut events = log.launched();
    events.push(
        log.report(
            5,
            "aH",
            TaskStatus::Running,
            Extra::call("toolu_H")
                .typed("local_agent")
                .said("hidden")
                .detail(TaskDetail {
                    parent_task_id: s("aA"),
                    depth: Some(2),
                    backgrounded: Some(false),
                    run_id: s("rH"),
                    ..detail()
                }),
        ),
    );
    events
}

fn progress(
    log: &mut Log,
    ms: i64,
    task_id: &str,
    detail: TaskDetail,
    count: Option<u64>,
    tokens: Option<u64>,
) -> StoredEvent {
    log.report(
        ms,
        task_id,
        TaskStatus::Running,
        Extra {
            tool_count: count,
            total_tokens: tokens,
            detail: Some(TaskDetail {
                progress: Some(true),
                ..detail
            }),
            ..Extra::default()
        },
    )
}

#[test]
fn places_a_hidden_agent_under_the_task_that_started_it() {
    let mut log = Log::new();
    let mut events = hidden(&mut log);
    events.push(log.report(6, "aH", TaskStatus::Completed, Extra::call("toolu_H").tools(3)));
    let crew = fold_crew(&events, SessionAgent::Claude);
    let agent = node(&crew, "toolu_H");
    assert_eq!(
        (
            agent.parent_id.as_deref(),
            agent.depth,
            agent.backgrounded,
            agent.run_id.as_deref(),
            agent.status
        ),
        (Some("toolu_A"), 2, Some(false), Some("rH"), Done)
    );
    assert_eq!(node(&crew, "toolu_A").children, ["toolu_H"]);
    assert_eq!(node(&crew, "toolu_A").steps, 3);
}

#[test]
fn moves_a_task_listed_before_its_start_under_the_task_its_start_names() {
    let mut log = Log::new();
    let mut events = hidden(&mut log);
    events.push(log.list(6, &[("aA", "local_agent"), ("aX", "local_agent")]));
    let listed = fold_crew(&events, SessionAgent::Claude);
    assert_eq!(
        (node(&listed, "aX").parent_id.as_deref(), node(&listed, "aX").depth),
        (Some(LEAD_ID), 1)
    );
    events.push(
        log.report(
            8,
            "aX",
            TaskStatus::Running,
            Extra::call("toolu_X")
                .typed("local_agent")
                .said("aX")
                .detail(TaskDetail {
                    parent_task_id: s("aH"),
                    depth: Some(3),
                    backgrounded: Some(true),
                    ..detail()
                }),
        ),
    );
    let crew = fold_crew(&events, SessionAgent::Claude);
    let moved = node(&crew, "aX");
    assert_eq!((moved.parent_id.as_deref(), moved.depth), (Some("toolu_H"), 3));
    assert!(node(&crew, "toolu_H").children.contains(&moved.id));
    assert!(!crew.lead().children.contains(&moved.id));
}

#[test]
fn moves_a_listed_tasks_whole_subtree_with_its_steps_and_depths() {
    let mut log = Log::new();
    let mut events = hidden(&mut log);
    events.push(log.list(
        6,
        &[("aA", "local_agent"), ("aX", "local_agent"), ("aY", "local_agent")],
    ));
    events.push(
        log.report(
            7,
            "aY",
            TaskStatus::Running,
            Extra::call("toolu_Y")
                .typed("local_agent")
                .said("aY")
                .detail(TaskDetail {
                    parent_task_id: s("aX"),
                    depth: Some(4),
                    ..detail()
                }),
        ),
    );
    events.push(log.tool_of(8, "tR", "Read", json!({ "file_path": "y.ts" }), Some("toolu_Y")));
    events.push(
        log.report(
            9,
            "aX",
            TaskStatus::Running,
            Extra::call("toolu_X")
                .typed("local_agent")
                .said("aX")
                .detail(TaskDetail {
                    parent_task_id: s("aH"),
                    depth: Some(3),
                    ..detail()
                }),
        ),
    );
    let crew = fold_crew(&events, SessionAgent::Claude);
    let x = node(&crew, "aX");
    assert_eq!((x.parent_id.as_deref(), x.depth, x.steps), (Some("toolu_H"), 3, 1));
    assert_eq!((node(&crew, "aY").depth, node(&crew, "aY").steps), (4, 1));
    assert_eq!(node(&crew, "toolu_H").steps, 1);
    assert_eq!(crew.lead().steps, 2);
}

#[test]
fn starts_a_command_moved_to_the_background_when_its_call_was_made() {
    let mut log = Log::new();
    let events = vec![
        log.tool(
            100,
            "toolu_F",
            "Bash",
            json!({ "command": "for i in 1 2 3; do sleep 1; done" }),
        ),
        log.list(900, &[("b1", "local_bash")]),
        log.report(
            1000,
            "b1",
            TaskStatus::Running,
            Extra::call("toolu_F")
                .typed("local_bash")
                .said("loop")
                .detail(TaskDetail {
                    depth: Some(1),
                    backgrounded: Some(true),
                    run_id: s("r"),
                    ..detail()
                }),
        ),
    ];
    let crew = fold_crew(&events, SessionAgent::Claude);
    let shell = node(&crew, "toolu_F");
    assert_eq!(
        (shell.kind, shell.call_id.as_deref(), shell.started_at, shell.status),
        (AgentKind::Shell, Some("toolu_F"), 100, Running)
    );
}

#[test]
fn gives_an_approval_to_the_agent_it_names_by_call_or_by_task_id() {
    for parent in ["toolu_H", "aH"] {
        let mut log = Log::new();
        let mut events = hidden(&mut log);
        events.push(log.ask_of(6, "r1", "toolu_never", Some(parent)));
        let crew = fold_crew(&events, SessionAgent::Claude);
        assert_eq!(node(&crew, "toolu_H").status, Needs, "{parent}");
        assert!(crew.lead().needs.is_empty());
    }
}

#[test]
fn reads_a_progress_row_as_an_update() {
    let mut log = Log::new();
    let mut events = log.launched();
    events.push(progress(
        &mut log,
        5,
        "aZ",
        TaskDetail {
            activity: s("Reading"),
            ..detail()
        },
        None,
        None,
    ));
    assert_eq!(order(&fold_crew(&events, SessionAgent::Claude)), ["toolu_A"]);

    let mut log = Log::new();
    let mut events = hidden(&mut log);
    events.push(progress(
        &mut log,
        6,
        "aH",
        TaskDetail {
            activity: s("Running git log"),
            model: s("claude-haiku-4-5"),
            ..detail()
        },
        Some(2),
        Some(23_000),
    ));
    let crew = fold_crew(&events, SessionAgent::Claude);
    let agent = node(&crew, "toolu_H");
    assert_eq!(
        (
            agent.title.as_str(),
            agent.context_tokens,
            agent.tool_count,
            agent.model.as_deref(),
            agent.status
        ),
        ("hidden", Some(23_000), 2, Some("claude-haiku-4-5"), Running)
    );
    assert_eq!(
        agent.last.as_ref().map(|last| last.text.as_str()),
        Some("Running git log")
    );

    let mut log = Log::new();
    let mut events = log.launched();
    events.push(log.tool_of(5, "t1", "Read", json!({ "file_path": "a.ts" }), Some("toolu_A")));
    events.push(progress(
        &mut log,
        6,
        "aA",
        TaskDetail {
            activity: s("Reading a.ts"),
            ..detail()
        },
        Some(1),
        None,
    ));
    let shown = fold_crew(&events, SessionAgent::Claude);
    assert_eq!(
        node(&shown, "toolu_A").last.as_ref().map(|last| last.text.as_str()),
        Some("Read a.ts")
    );

    let mut log = Log::new();
    let mut events = hidden(&mut log);
    events.push(log.report(6, "aH", TaskStatus::Completed, Extra::call("toolu_H").tokens(9)));
    events.push(progress(
        &mut log,
        7,
        "aH",
        TaskDetail {
            activity: s("late"),
            ..detail()
        },
        Some(5),
        Some(50),
    ));
    let ended = fold_crew(&events, SessionAgent::Claude);
    let agent = node(&ended, "toolu_H");
    assert_eq!(
        (agent.status, agent.context_tokens, agent.tool_count),
        (Done, Some(9), 0)
    );
}

#[test]
fn starts_a_new_run_fresh_and_never_revives_a_run_that_ended() {
    let mut log = Log::new();
    let mut first = hidden(&mut log);
    first.push(log.report(
        6,
        "aH",
        TaskStatus::Stopped,
        Extra::call("toolu_H").said("cut").detail(TaskDetail {
            run_id: s("rH"),
            reason: s("worker_restart"),
            ..detail()
        }),
    ));
    let crew = fold_crew(&first, SessionAgent::Claude);
    let agent = node(&crew, "toolu_H");
    assert_eq!(
        (agent.status, agent.reason.as_deref(), agent.summary.as_deref()),
        (Stopped, Some("worker_restart"), Some("cut"))
    );
    let rerun_of = |log: &mut Log, run: &str| {
        log.report(
            7,
            "aH",
            TaskStatus::Running,
            Extra::call("toolu_H").typed("local_agent").detail(TaskDetail {
                run_id: s(run),
                ..detail()
            }),
        )
    };
    let mut repeat = first.clone();
    repeat.push(rerun_of(&mut log, "rH"));
    assert_eq!(
        node(&fold_crew(&repeat, SessionAgent::Claude), "toolu_H").status,
        Stopped
    );
    let mut log = Log {
        seq: first.len() as i64,
    };
    let mut rerun = first.clone();
    rerun.push(rerun_of(&mut log, "rH2"));
    let crew = fold_crew(&rerun, SessionAgent::Claude);
    let agent = node(&crew, "toolu_H");
    assert_eq!(
        (
            agent.status,
            agent.run_id.as_deref(),
            agent.reason.as_deref(),
            agent.summary.as_deref(),
            agent.parent_id.as_deref()
        ),
        (Running, Some("rH2"), None, None, Some("toolu_A"))
    );
    // A late end of the run before stays out of the new one.
    rerun.push(log.report(
        8,
        "aH",
        TaskStatus::Failed,
        Extra::call("toolu_H").detail(TaskDetail {
            run_id: s("rH"),
            ..detail()
        }),
    ));
    assert_eq!(
        node(&fold_crew(&rerun, SessionAgent::Claude), "toolu_H").status,
        Running
    );
}

#[test]
fn forgets_why_the_last_run_ended_when_a_new_one_starts() {
    let mut log = Log::new();
    let mut events = hidden(&mut log);
    events.push(log.report(
        6,
        "aH",
        TaskStatus::Stopped,
        Extra::call("toolu_H").detail(TaskDetail {
            run_id: s("rH"),
            reason: s("worker_restart"),
            ..detail()
        }),
    ));
    let crew = fold_crew(&events, SessionAgent::Claude);
    let agent = node(&crew, "toolu_H");
    assert_eq!(
        (agent.reason.as_deref(), agent.summary.as_deref(), agent.duration_ms),
        (Some("worker_restart"), None, None)
    );
    events.push(log.report(
        7,
        "aH",
        TaskStatus::Running,
        Extra::call("toolu_H").typed("local_agent").detail(TaskDetail {
            run_id: s("rH2"),
            ..detail()
        }),
    ));
    let rerun = fold_crew(&events, SessionAgent::Claude);
    assert_eq!(
        (
            node(&rerun, "toolu_H").status,
            node(&rerun, "toolu_H").reason.as_deref()
        ),
        (Running, None)
    );
}

#[test]
fn adds_up_a_codex_childs_own_model_calls_and_keeps_the_leads_out() {
    let mut log = Log::new();
    let events = vec![
        log.tool(
            1,
            "call_1",
            "Agent",
            json!({ "description": "core crate", "prompt": "" }),
        ),
        log.report(
            2,
            "thr_c",
            TaskStatus::Running,
            Extra::call("call_1")
                .typed("local_agent")
                .said("core crate")
                .detail(TaskDetail {
                    depth: Some(1),
                    model: s("gpt-6-astra"),
                    ..detail()
                }),
        ),
        log.usage(3, None, "gpt-5", 1000, 50, 0.01),
        log.usage(4, Some("call_1"), "gpt-6-astra", 500, 20, 0.02),
        log.usage(5, Some("call_1"), "gpt-6-astra", 700, 30, 0.03),
    ];
    let crew = fold_crew(&events, SessionAgent::Codex);
    let child = node(&crew, "call_1");
    assert_eq!(
        (child.model.as_deref(), child.tokens, child.context_tokens),
        (Some("gpt-6-astra"), Some(1250), Some(730))
    );
    assert!((child.cost_usd.expect("a cost") - 0.05).abs() < 1e-6);
    assert_eq!((crew.lead().tokens, crew.lead().cost_usd), (None, None));
}

#[test]
fn keeps_the_steps_of_a_listed_tasks_children_when_it_links_to_its_call() {
    let mut log = Log::new();
    let events = vec![
        log.list(1, &[("aL", "local_agent")]),
        log.report(
            2,
            "aC",
            TaskStatus::Running,
            Extra::call("toolu_C")
                .typed("local_agent")
                .said("child")
                .detail(TaskDetail {
                    parent_task_id: s("aL"),
                    depth: Some(2),
                    ..detail()
                }),
        ),
        log.tool_of(3, "t1", "Read", json!({ "file_path": "a.ts" }), Some("toolu_C")),
        log.tool(
            4,
            "toolu_L",
            "Agent",
            json!({ "description": "listed one", "prompt": "go" }),
        ),
        log.report(
            5,
            "aL",
            TaskStatus::Running,
            Extra::call("toolu_L").typed("local_agent").said("listed one"),
        ),
    ];
    let crew = fold_crew(&events, SessionAgent::Claude);
    assert_eq!(order(&crew), ["toolu_C", "toolu_L"]);
    assert_eq!(node(&crew, "toolu_L").children, ["toolu_C"]);
    assert_eq!(
        (
            node(&crew, "toolu_C").parent_id.as_deref(),
            node(&crew, "toolu_C").depth
        ),
        (Some("toolu_L"), 2)
    );
    assert_eq!(node(&crew, "toolu_L").steps, 1);
    assert_eq!(crew.lead().steps, 2);
}

#[test]
fn keeps_a_listed_tasks_own_logged_calls_when_it_links_to_its_call() {
    let mut log = Log::new();
    let events = vec![
        log.list(1, &[("aL", "local_agent")]),
        log.tool_of(2, "t1", "Read", json!({ "file_path": "a.ts" }), Some("aL")),
        log.tool(
            3,
            "toolu_L",
            "Agent",
            json!({ "description": "listed one", "prompt": "go" }),
        ),
        log.report(
            4,
            "aL",
            TaskStatus::Running,
            Extra::call("toolu_L").typed("local_agent").said("listed one"),
        ),
    ];
    let crew = fold_crew(&events, SessionAgent::Claude);
    assert_eq!(
        (node(&crew, "toolu_L").tool_count, node(&crew, "toolu_L").steps),
        (1, 1)
    );
}

// crew: the tree

#[test]
fn nests_a_codex_grandchild_under_its_parent_child() {
    let mut log = Log::new();
    let events = vec![
        log.tool(
            1,
            "call_outer",
            "Agent",
            json!({ "description": "outer", "prompt": "" }),
        ),
        log.report(
            1,
            "thr_outer",
            TaskStatus::Running,
            Extra::call("call_outer").typed("local_agent").said("outer"),
        ),
        log.tool_of(
            2,
            "call_inner",
            "Agent",
            json!({ "description": "inner", "prompt": "" }),
            Some("call_outer"),
        ),
        log.report(
            2,
            "thr_inner",
            TaskStatus::Running,
            Extra::call("call_inner").typed("local_agent").said("inner"),
        ),
        log.tool_of(
            3,
            "exec-1",
            "Bash",
            json!({ "command": "touch codex-inner.txt" }),
            Some("call_inner"),
        ),
        log.ask(3, "exec-1#0", "exec-1"),
    ];
    let crew = fold_crew(&events, SessionAgent::Codex);
    assert_eq!(crew.provider, SessionAgent::Codex);
    let lead = crew.lead();
    assert_eq!(
        (lead.id.as_str(), lead.kind, lead.title.as_str(), lead.depth),
        (LEAD_ID, AgentKind::Lead, "Codex", 0)
    );
    assert_eq!(lead.children, ["call_outer"]);
    let outer = node(&crew, "call_outer");
    assert_eq!(
        (outer.depth, outer.parent_id.as_deref(), outer.tool_count, outer.steps),
        (1, Some(LEAD_ID), 1, 2)
    );
    assert_eq!(outer.children, ["call_inner"]);
    let inner = node(&crew, "call_inner");
    assert_eq!(
        (
            inner.depth,
            inner.parent_id.as_deref(),
            inner.tool_count,
            inner.steps,
            inner.status
        ),
        (2, Some("call_outer"), 1, 1, Needs)
    );
    assert_eq!(lead.steps, 3);
    assert_eq!(
        crew.node_of_call("call_inner").and_then(|node| node.task_id.as_deref()),
        Some("thr_inner")
    );
    assert!(crew.node_of_call("exec-1").is_none());
}

#[test]
fn keeps_the_latest_four_actions_oldest_first() {
    let mut log = Log::new();
    let mut events = vec![log.tool(1, "a", "Agent", json!({ "description": "d" }))];
    for (index, name) in ["one", "two", "three", "four", "five"].iter().enumerate() {
        events.push(log.tool_of(
            2 + index as i64,
            &format!("c{index}"),
            "Grep",
            json!({ "pattern": name }),
            Some("a"),
        ));
    }
    let crew = fold_crew(&events, SessionAgent::Claude);
    let agent = node(&crew, "a");
    let recent: Vec<&str> = agent.recent.iter().map(|entry| entry.text.as_str()).collect();
    assert_eq!(recent, ["Search two", "Search three", "Search four", "Search five"]);
    assert_eq!(
        agent.last,
        Some(Activity {
            text: "Search five".into(),
            at: 6
        })
    );
}

#[test]
fn gives_the_lead_its_state() {
    let mut log = Log::new();
    let mut events = vec![
        log.turn_start(1),
        log.tool(
            2,
            "a",
            "Agent",
            json!({ "description": "d", "run_in_background": true }),
        ),
        log.result(3, "a", "launched", false),
    ];
    assert_eq!(fold_crew(&events, SessionAgent::Claude).lead().status, Running);
    events.push(log.turn_end(4, TurnStatus::Completed));
    let between = fold_crew(&events, SessionAgent::Claude);
    assert_eq!(between.lead().status, Waiting);
    assert_eq!((between.counts.agents_running, between.counts.total), (1, 1));
    events.push(log.report(5, "ta", TaskStatus::Completed, Extra::call("a")));
    assert_eq!(fold_crew(&events, SessionAgent::Claude).lead().status, Done);
    let mut log = Log::new();
    let cut = [log.turn_start(1), log.turn_end(2, TurnStatus::Interrupted)];
    assert_eq!(fold_crew(&cut, SessionAgent::Claude).lead().status, Stopped);
    let mut log = Log::new();
    let asking = [
        log.turn_start(1),
        log.tool(2, "c", "Bash", json!({ "command": "x" })),
        log.ask(3, "r", "c"),
    ];
    assert_eq!(fold_crew(&asking, SessionAgent::Claude).lead().status, Needs);
}

#[test]
fn counts_every_node_but_the_lead_by_how_it_stands() {
    let mut log = Log::new();
    let events = vec![
        log.tool(1, "a", "Agent", json!({ "description": "a" })),
        log.tool(2, "b", "Agent", json!({ "description": "b" })),
        log.result(3, "b", "boom", true),
        log.tool(
            4,
            "s",
            "Bash",
            json!({ "command": "npm run dev", "run_in_background": true }),
        ),
        log.result(5, "s", "Command running in background with ID: bash_1", false),
    ];
    assert_eq!(
        fold_crew(&events, SessionAgent::Claude).counts,
        CrewCounts {
            agents_running: 1,
            shells_running: 1,
            needs: 0,
            done: 0,
            failed: 1,
            stopped: 0,
            total: 3
        }
    );
}

// crew: controls

fn controls_crew() -> Crew {
    let mut log = Log::new();
    fold_crew(
        &[
            log.tool(1, "a", "Agent", json!({ "description": "d" })),
            log.tool(2, "s", "Bash", json!({ "command": "x", "run_in_background": true })),
            log.tool(3, "b", "Agent", json!({ "description": "e" })),
            log.result(4, "b", "ok", false),
        ],
        SessionAgent::Claude,
    )
}

fn capabilities(stop: &[&str], background: &[&str], output: &[&str]) -> AgentCapabilities {
    let names = |kinds: &[&str]| kinds.iter().map(|kind| kind.to_string()).collect();
    AgentCapabilities {
        stop: names(stop),
        background: names(background),
        output: names(output),
        ..AgentCapabilities::default()
    }
}

#[test]
fn offers_nothing_before_the_core_says_what_the_agent_accepts_but_the_leads_composer() {
    let crew = controls_crew();
    let controls = controls_of(node(&crew, "a"), None);
    assert_eq!(controls.stop, off("This Nexus can't control agents yet."));
    assert_eq!(
        (controls.message, controls.message_to),
        (
            off("Sub-agents can't take messages. Message the lead instead."),
            Some(MessageTo::Lead)
        )
    );
    let lead = controls_of(crew.lead(), None);
    assert_eq!((lead.message, lead.message_to), (ON, Some(MessageTo::Own)));
    assert_eq!(controls_of(node(&crew, "s"), None).message_to, None);
}

#[test]
fn offers_what_the_capabilities_name_for_a_live_node() {
    let crew = controls_crew();
    let all = capabilities(&["agent", "shell"], &["shell"], &["shell"]);
    let agent = controls_of(node(&crew, "a"), Some(&all));
    assert!(agent.stop.available);
    assert_eq!(agent.background, off("Only a command can move to the background."));
    assert_eq!(agent.output, off("Only a background command has output to read."));
    let shell = controls_of(node(&crew, "s"), Some(&all));
    assert_eq!(shell.background, off("It already runs in the background."));
    assert!(shell.output.available);
    assert_eq!(
        controls_of(node(&crew, "b"), Some(&all)).stop,
        off("This agent has finished.")
    );
    assert_eq!(
        controls_of(crew.lead(), Some(&all)).stop,
        off("Use the session's Interrupt or Stop.")
    );
    assert_eq!(
        controls_of(node(&crew, "a"), Some(&capabilities(&["shell"], &[], &[]))).stop,
        off("Update Claude Code to 2.1.292 or later to stop one sub-agent.")
    );
    // A task kind the core names by its own type (a workflow): never offered, as the core would refuse it.
    let mut log = Log::new();
    let flows = fold_crew(
        &[
            log.report(
                1,
                "w1",
                TaskStatus::Running,
                Extra::call("w").typed("local_workflow").said("Release"),
            ),
            log.report(2, "a1", TaskStatus::Running, Extra::call("r").typed("local_agent")),
        ],
        SessionAgent::Claude,
    );
    assert_eq!(
        controls_of(node(&flows, "w"), Some(&all)).stop,
        off("Nexus can't stop this kind of task.")
    );
    let workflows = capabilities(&["local_workflow"], &["shell"], &["shell"]);
    assert!(controls_of(node(&flows, "w"), Some(&workflows)).stop.available);
    assert!(controls_of(node(&flows, "r"), Some(&all)).stop.available);
    let mut log = Log::new();
    let codex = fold_crew(
        &[log.tool(1, "call_1", "Agent", json!({ "description": "core", "prompt": "" }))],
        SessionAgent::Codex,
    );
    assert_eq!(
        controls_of(node(&codex, "call_1"), Some(&capabilities(&["agent"], &[], &[]))).background,
        off("Codex sub-agents always run beside the session.")
    );
}

// crew: one event's work

/// Every case's log above, one after another, renumbered as one session's.
fn long_log() -> Vec<StoredEvent> {
    let mut all = Vec::new();
    let mut log = Log::new();
    let mut parts = vec![log.launched(), hidden(&mut Log::new())];
    let mut more = Log::new();
    parts.push(vec![
        more.turn_start(10),
        more.tool(11, "x1", "Agent", json!({ "description": "x" })),
        more.tool_of(12, "x2", "Bash", json!({ "command": "a" }), Some("x1")),
        more.ask(13, "rx", "x2"),
        more.tool(
            14,
            "xs",
            "Bash",
            json!({ "command": "tail", "run_in_background": true }),
        ),
        more.result(15, "xs", "Command running in background with ID: bx", false),
        more.tool(16, "xo", "BashOutput", json!({ "bash_id": "bx" })),
        more.result(17, "xo", "<status>completed</status> all good", false),
        more.answer(18, "rx"),
        more.turn_end(19, TurnStatus::Interrupted),
    ]);
    for part in parts {
        for mut stored in part {
            stored.seq = all.len() as i64 + 1;
            all.push(stored);
        }
    }
    all
}

#[test]
fn folds_the_same_in_pieces_as_the_whole_log_at_any_point() {
    let events = long_log();
    for cut in 1..=events.len() {
        let whole = fold_crew(&events[..cut], SessionAgent::Claude);
        let mut folder = CrewFolder::new(SessionAgent::Claude);
        for piece in events[..cut].chunks(3) {
            folder.push(piece);
        }
        assert_eq!(folder.crew(), &whole, "at {cut}");
        assert_eq!(folder.applied, cut);
        assert_eq!(folder.resets, 0);
    }
}

#[test]
fn folds_each_event_once_however_the_log_is_read() {
    let events = long_log();
    let mut folder = CrewFolder::new(SessionAgent::Claude);
    for end in 1..=events.len() {
        // Each read hands the whole log so far; only the new events fold.
        folder.sync(&events[..end]);
    }
    assert_eq!(folder.applied, events.len());
    assert_eq!(folder.resets, 0);
    // A second reader folds nothing.
    folder.sync(&events);
    assert_eq!(folder.applied, events.len());
    assert_eq!(folder.crew(), &fold_crew(&events, SessionAgent::Claude));
}

#[test]
fn starts_over_once_on_a_log_that_does_not_continue_the_one_folded() {
    let events = long_log();
    let mut folder = CrewFolder::new(SessionAgent::Claude);
    folder.sync(&events);
    // Another session's log, or one the store started over: its numbers do not go on from the last folded.
    let other: Vec<StoredEvent> = events[..5].to_vec();
    let crew = folder.sync(&other).clone();
    assert_eq!(folder.resets, 1);
    assert_eq!(crew, fold_crew(&other, SessionAgent::Claude));
    // A page with a gap starts over too.
    let mut folder = CrewFolder::new(SessionAgent::Claude);
    folder.push(&events[..3]);
    folder.push(&events[5..]);
    assert_eq!(folder.resets, 1);
}

#[test]
fn the_crew_of_an_empty_log_is_its_lead() {
    let crew = fold_crew(&[], SessionAgent::Codex);
    let lead = crew.lead();
    assert_eq!(
        (lead.kind, lead.provider, lead.status, lead.title.as_str()),
        (AgentKind::Lead, SessionAgent::Codex, Done, "Codex")
    );
    assert_eq!(crew.order().count(), 0);
}

// The helpers crew.ts exports.

#[test]
fn labels_the_pill_and_the_time() {
    let counts = |agents, shells, total| CrewCounts {
        agents_running: agents,
        shells_running: shells,
        total,
        ..CrewCounts::default()
    };
    assert_eq!(pill_label(&counts(2, 0, 2)), "2 agents running");
    assert_eq!(pill_label(&counts(1, 1, 2)), "1 agent and 1 command running");
    assert_eq!(pill_label(&counts(0, 1, 1)), "1 command running");
    assert_eq!(pill_label(&counts(0, 0, 3)), "3 tasks");
    assert_eq!(pill_label(&counts(0, 0, 1)), "1 task");
    assert_eq!(elapsed(12_400), "12s");
    assert_eq!(elapsed(184_000), "3m 04s");
    assert_eq!(elapsed(3_900_000), "1h 05m");
    assert_eq!(elapsed(-5), "0s");
}

#[test]
fn reads_a_background_commands_id() {
    assert_eq!(
        shell_id("Command running in background with ID: b546").as_deref(),
        Some("b546")
    );
    assert_eq!(shell_id("id:  bash_1-x more").as_deref(), Some("bash_1-x"));
    assert_eq!(shell_id("taskid: nope"), None);
    assert_eq!(shell_id("Monitor started (task bM)"), None);
}

#[test]
fn reads_tool_calls_in_words() {
    let words = |name: &str, input: Value| {
        let line = tool_line(name, &input);
        [line.verb, line.detail].join(" ")
    };
    assert_eq!(words("Bash", json!({ "command": "npm test\nmore" })), "Run npm test");
    assert_eq!(
        words("WebFetch", json!({ "url": "https://example.com/a?b=1" })),
        "Fetch example.com/a"
    );
    assert_eq!(
        words("WebFetch", json!({ "url": "https://example.com/" })),
        "Fetch example.com"
    );
    assert_eq!(
        words("TodoWrite", json!({ "todos": [{ "status": "completed" }, {}] })),
        "Update the plan 1 of 2 done"
    );
    assert_eq!(
        words("mcp__nexus__create_page", json!({ "title": "Plan" })),
        "nexus · create page Plan"
    );
    assert_eq!(words("Custom", json!({ "n": 1, "what": "  thing " })), "Custom thing");
    assert_eq!(untagged("<status>done</status>\n  ok <x"), "done ok <x");
    assert_eq!(first_line("  abcdef\nz", 4), "abc…");
}
