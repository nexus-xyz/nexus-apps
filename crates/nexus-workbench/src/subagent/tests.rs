//! `subagent.test.ts`'s "fold, a sub-agent's work", for [`Nest`].
use serde_json::json;

use super::*;

/// What each test places: (kind, id) pairs, kept readable.
type E = (&'static str, &'static str);

fn entries(flat: Vec<Flat<E>>) -> Vec<(Vec<String>, String)> {
    flat.into_iter()
        .map(|item| match item {
            Flat::Entry { path, entry } => (path, format!("{}:{}", entry.0, entry.1)),
            Flat::End { path, call } => (path, format!("end:{call}")),
        })
        .collect()
}

fn path(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| id.to_string()).collect()
}

/// The log of `subagent.test.ts`: a Task whose sub-agent thinks, reads, starts its own sub-agent and writes, then the
/// main answer goes on.
fn sample() -> Nest<E> {
    let mut nest = Nest::default();
    nest.place(("user", "go"), None);
    nest.place(("text", "I'll ask a sub-agent."), None);
    nest.place_call("task_1", ("tool", "task_1"), None);
    nest.place(("thinking", "s1"), Some("task_1"));
    nest.place_call("read_1", ("tool", "read_1"), Some("task_1"));
    nest.place_call("task_2", ("tool", "task_2"), Some("task_1"));
    nest.place_call("grep_1", ("tool", "grep_1"), Some("task_2"));
    nest.place(("text", "It is read in src/config.ts."), Some("task_1"));
    nest.place(("text", "Found it."), None);
    nest
}

#[test]
fn nests_every_call_thought_and_text_of_the_sub_agent_under_its_task_call() {
    let flat = entries(sample().flatten());
    let top: Vec<&str> = flat
        .iter()
        .filter(|(path, entry)| path.is_empty() && !entry.starts_with("end:"))
        .map(|(_, entry)| entry.as_str())
        .collect();
    assert_eq!(
        top,
        ["user:go", "text:I'll ask a sub-agent.", "tool:task_1", "text:Found it."]
    );
    let under: Vec<&str> = flat
        .iter()
        .filter(|(path, _)| *path == ["task_1"])
        .map(|(_, entry)| entry.as_str())
        .collect();
    assert_eq!(
        under,
        [
            "thinking:s1",
            "tool:read_1",
            "tool:task_2",
            "text:It is read in src/config.ts.",
            "end:task_1"
        ]
    );
    let inner: Vec<&str> = flat
        .iter()
        .filter(|(path, _)| *path == ["task_1", "task_2"])
        .map(|(_, entry)| entry.as_str())
        .collect();
    // A call's end sits with its own work: `end:read_1` under read_1, `end:grep_1` under grep_1.
    assert_eq!(inner, ["tool:grep_1", "end:task_2"]);
}

#[test]
fn hands_out_each_call_then_its_work_then_what_follows() {
    let flat = entries(sample().flatten());
    let order: Vec<&str> = flat.iter().map(|(_, entry)| entry.as_str()).collect();
    assert_eq!(
        order,
        [
            "user:go",
            "text:I'll ask a sub-agent.",
            "tool:task_1",
            "thinking:s1",
            "tool:read_1",
            "end:read_1",
            "tool:task_2",
            "tool:grep_1",
            "end:grep_1",
            "end:task_2",
            "text:It is read in src/config.ts.",
            "end:task_1",
            "text:Found it.",
        ]
    );
    assert_eq!(flat[7].0, path(&["task_1", "task_2"]));
}

#[test]
fn keeps_the_sub_agents_text_out_of_the_main_answer() {
    let mut nest = sample();
    // The main answer's last entry is its own text, not the sub-agent's.
    assert_eq!(nest.last_mut(None).map(|entry| entry.1), Some("Found it."));
    let task = nest.owner(Some("task_1"));
    assert_eq!(
        nest.last_mut(task).map(|entry| entry.1),
        Some("It is read in src/config.ts.")
    );
}

#[test]
fn a_parent_it_does_not_hold_leaves_the_entry_at_the_top() {
    let mut nest: Nest<E> = Nest::default();
    // Its Task call is on an older page.
    nest.place_call("read_9", ("tool", "read_9"), Some("task_9"));
    assert_eq!(nest.owner(Some("task_9")), None);
    assert_eq!(nest.owner(None), None);
    let flat = entries(nest.flatten());
    assert_eq!(flat[0], (Vec::new(), "tool:read_9".to_string()));
}

#[test]
fn an_entry_taken_out_takes_its_work_with_it() {
    let mut nest = sample();
    let task = nest.owner(Some("task_1")).expect("held");
    nest.remove(task);
    let order: Vec<String> = entries(nest.flatten()).into_iter().map(|(_, entry)| entry).collect();
    assert_eq!(order, ["user:go", "text:I'll ask a sub-agent.", "text:Found it."]);
}

#[test]
fn an_agent_call_reads_its_description_type_and_prompt() {
    let call = AgentCall::of(&json!({
        "description": "Find the config",
        "subagent_type": "Explore",
        "prompt": "Where is the config read?"
    }));
    assert_eq!(
        call,
        AgentCall {
            title: "Find the config".into(),
            role: Some("Explore".into()),
            prompt: Some("Where is the config read?".into()),
        }
    );
    // No description: the prompt's first line names it.
    assert_eq!(
        AgentCall::of(&json!({ "prompt": "Look around\nthen report" })).title,
        "Look around"
    );
    assert_eq!(AgentCall::of(&json!({})).title, "");
    assert!(is_agent_tool("Task") && is_agent_tool("Agent") && !is_agent_tool("Read"));
}
