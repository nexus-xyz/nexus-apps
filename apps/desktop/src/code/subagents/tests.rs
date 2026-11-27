//! `subagent.test.ts`, for the Code transcript's blocks.
use nexus_workbench::crew_fold::AgentStatus;
use nexus_workbench::event::WbEvent;
use serde_json::{Value, json};

use super::super::{Block, blocks};

fn event(value: Value) -> WbEvent {
    serde_json::from_value(value).expect("event")
}

fn task(id: &str, parent: Option<&str>) -> WbEvent {
    event(json!({
        "kind": "tool", "id": id, "name": "Task", "parent": parent,
        "input": { "description": "Find the config", "subagent_type": "Explore", "prompt": "Where is the config read?" }
    }))
}

fn tool(id: &str, name: &str, parent: &str) -> WbEvent {
    event(
        json!({ "kind": "tool", "id": id, "name": name, "input": { "file_path": "/r/src/config.ts" }, "parent": parent }),
    )
}

fn text(id: &str, text: &str, parent: Option<&str>) -> WbEvent {
    event(json!({ "kind": "text", "id": id, "text": text, "parent": parent }))
}

fn result(id: &str, output: &str, is_error: bool) -> WbEvent {
    event(json!({ "kind": "toolResult", "id": id, "output": output, "isError": is_error, "truncated": false }))
}

fn fold(events: Vec<WbEvent>) -> Vec<Block> {
    blocks(events.into_iter().enumerate().map(|(ix, event)| (ix as i64 + 1, event)))
}

/// Each block as one line: its path, then what it is.
fn lines(blocks: &[Block]) -> Vec<String> {
    fn line(block: &Block) -> String {
        match block {
            Block::User { text, .. } => format!("user {text}"),
            Block::Text(text) => format!("text {text}"),
            Block::Thinking(text, _) => format!("thinking {text}"),
            Block::Tool { name, .. } => format!("tool {name}"),
            Block::Agent(row) => format!("agent {}", row.call_id),
            Block::AgentEnd(end) => format!("end {}", end.call_id),
            other => format!("{other:?}"),
        }
    }
    blocks
        .iter()
        .map(|block| match block {
            Block::Inside { path, block } => format!("{} > {}", path.join("/"), line(block)),
            block => line(block),
        })
        .collect()
}

fn agent<'a>(blocks: &'a [Block], id: &str) -> &'a super::AgentRow {
    blocks
        .iter()
        .find_map(|block| match block {
            Block::Agent(row) if row.call_id == id => Some(row.as_ref()),
            Block::Inside { block, .. } => match block.as_ref() {
                Block::Agent(row) if row.call_id == id => Some(row.as_ref()),
                _ => None,
            },
            _ => None,
        })
        .expect("the agent's row")
}

fn end<'a>(blocks: &'a [Block], id: &str) -> &'a super::AgentEnd {
    blocks
        .iter()
        .find_map(|block| match block {
            Block::Inside { block, .. } => match block.as_ref() {
                Block::AgentEnd(end) if end.call_id == id => Some(end.as_ref()),
                _ => None,
            },
            _ => None,
        })
        .expect("the agent's end")
}

fn sample() -> Vec<Block> {
    fold(vec![
        event(json!({ "kind": "user", "text": "go", "images": [] })),
        text("m1:0", "I'll ask a sub-agent.", None),
        task("task_1", None),
        event(json!({ "kind": "thinking", "id": "s1", "text": "Look in src first.", "parent": "task_1" })),
        tool("read_1", "Read", "task_1"),
        result("read_1", "export const x = 1", false),
        task("task_2", Some("task_1")),
        tool("grep_1", "Grep", "task_2"),
        text("s2:0", "It is read in src/config.ts.", Some("task_1")),
        result("task_1", "src/config.ts", false),
        text("m2:0", "Found it.", None),
    ])
}

#[test]
fn nests_every_call_thought_and_text_of_the_sub_agent_under_its_task_call() {
    assert_eq!(
        lines(&sample()),
        [
            "user go",
            "text I'll ask a sub-agent.",
            "agent task_1",
            "task_1 > thinking Look in src first.",
            "task_1 > tool Read",
            "task_1 > agent task_2",
            "task_1/task_2 > tool Grep",
            "task_1/task_2 > end task_2",
            "task_1 > text It is read in src/config.ts.",
            "task_1 > end task_1",
            "text Found it.",
        ]
    );
}

#[test]
fn keeps_the_sub_agents_text_out_of_the_main_answer() {
    let texts: Vec<String> = sample()
        .into_iter()
        .filter_map(|block| match block {
            Block::Text(text) => Some(text),
            _ => None,
        })
        .collect();
    assert_eq!(texts, ["I'll ask a sub-agent.", "Found it."]);
}

#[test]
fn counts_the_sub_agents_steps_its_own_sub_agents_included() {
    let blocks = sample();
    let row = agent(&blocks, "task_1");
    assert_eq!(row.steps, 3);
    assert_eq!(row.meta().as_deref(), Some("3 steps"));
    assert_eq!(
        (row.title.as_str(), row.role.as_deref()),
        ("Find the config", Some("Explore"))
    );
    assert_eq!(row.status, AgentStatus::Done);
    // Its result shows at the end of its body once it ended.
    let end = end(&blocks, "task_1");
    assert_eq!(
        (end.running, end.empty, end.outcome.as_deref()),
        (false, false, Some("src/config.ts"))
    );
}

#[test]
fn a_running_sub_agent_with_no_steps_yet_says_so() {
    let blocks = fold(vec![
        event(json!({ "kind": "turnStart", "turnId": "u1" })),
        task("task_1", None),
    ]);
    let row = agent(&blocks, "task_1");
    assert!(row.running());
    assert_eq!(row.meta(), None);
    let end = end(&blocks, "task_1");
    assert!(end.running && end.empty && end.outcome.is_none());
}

#[test]
fn stops_an_unreported_foreground_agent_when_its_turn_is_cut_short() {
    let blocks = fold(vec![
        event(json!({ "kind": "turnStart", "turnId": "u1" })),
        task("task_1", None),
        tool("read_1", "Read", "task_1"),
        event(json!({
            "kind": "turnEnd", "turnId": "u1", "status": "interrupted", "costUsd": null, "durationMs": 1, "error": null
        })),
    ]);
    assert_eq!(agent(&blocks, "task_1").status, AgentStatus::Stopped);
}

#[test]
fn shows_failed_when_its_call_failed() {
    let blocks = fold(vec![task("task_1", None), result("task_1", "boom", true)]);
    assert_eq!(agent(&blocks, "task_1").status, AgentStatus::Failed);
    assert_eq!(end(&blocks, "task_1").outcome.as_deref(), Some("boom"));
}

#[test]
fn never_shows_a_background_agents_launch_receipt_and_shows_its_own_summary_once_it_ended() {
    let report = |status: &str, description: &str| {
        event(json!({
            "kind": "task", "taskId": "a1", "toolUseId": "task_1", "taskType": "local_agent", "agentType": null,
            "status": status, "description": description, "toolCount": null, "totalTokens": null, "durationMs": null
        }))
    };
    let launch = result("task_1", "Async agent launched successfully.", false);
    let listed = event(json!({
        "kind": "backgroundTasks", "tasks": [{ "taskId": "a1", "taskType": "local_agent", "description": "Find the config" }]
    }));
    let running = fold(vec![
        task("task_1", None),
        listed,
        report("running", ""),
        launch.clone(),
    ]);
    assert!(agent(&running, "task_1").running());
    assert_eq!(end(&running, "task_1").outcome, None);
    let done = fold(vec![
        task("task_1", None),
        report("running", ""),
        launch,
        report("completed", "The config is read in src/config.ts."),
    ]);
    assert_eq!(agent(&done, "task_1").status, AgentStatus::Done);
    assert_eq!(
        end(&done, "task_1").outcome.as_deref(),
        Some("The config is read in src/config.ts.")
    );
}

#[test]
fn a_sub_agents_work_whose_call_is_on_an_older_page_shows_at_the_top() {
    let blocks = fold(vec![
        tool("read_1", "Read", "task_0"),
        text("s:0", "Seen", Some("task_0")),
    ]);
    assert_eq!(lines(&blocks), ["tool Read", "text Seen"]);
}

#[test]
fn failed_tool_keeps_its_call_key_when_an_earlier_agent_adds_steps() {
    let mut events = vec![
        task("first", None),
        task("second", None),
        tool("failed", "Bash", "second"),
        result("failed", "failed command", true),
    ];
    let failed = |blocks: Vec<Block>| {
        blocks
            .into_iter()
            .enumerate()
            .find_map(|(ix, block)| match block {
                Block::Inside { block, .. } => match *block {
                    Block::Tool {
                        call_id,
                        error: Some(_),
                        ..
                    } => Some((ix, call_id)),
                    _ => None,
                },
                _ => None,
            })
            .expect("failed tool")
    };
    let before = failed(fold(events.clone()));
    events.push(tool("later", "Read", "first"));
    let after = failed(fold(events));
    assert_ne!(before.0, after.0, "an earlier agent inserts before this row");
    assert_eq!(before.1, "failed");
    assert_eq!(after.1, before.1, "the error's open state keeps the same call key");
}

#[test]
fn nested_blocks_keep_their_event_numbers_after_the_tree_reorders_them() {
    let events = vec![
        (1, 0, task("first", None)),
        (2, 0, task("second", None)),
        (3, 0, tool("late", "Read", "second")),
        (4, 0, tool("earlier", "Read", "first")),
    ];
    let (blocks, starts) = super::super::blocks_and_starts(events);
    assert_eq!(blocks.len(), starts.len());
    let first = super::super::locate(&starts, 4).expect("first agent's late event");
    let second = super::super::locate(&starts, 3).expect("second agent's event");
    assert!(first < second, "nesting changes reading order, not event provenance");
    assert_eq!(blocks[first].path(), ["first"]);
    assert_eq!(blocks[second].path(), ["second"]);
    assert_eq!(super::super::locate(&starts, 5), Some(first));
}
