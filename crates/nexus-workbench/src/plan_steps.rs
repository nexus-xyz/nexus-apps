//! The agent's plan step by step, for the Workbench's Agents tool, as the Nexus app reads it:
//! the steps of its latest plan (the main agent's TodoWrite; codex.rs writes
//! Codex's plan as one too), each with when it started and when it was done, read from the plan's earlier writes. A
//! step is the same step across writes when its text is the same; two steps with the same text in one write are told
//! apart by their order.

use std::collections::HashMap;

use serde_json::Value;

use crate::event::{StoredEvent, WbEvent};

/// How far a step is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub content: String,
    pub status: StepStatus,
    /// The sentence for while it is being done ("Running the tests"), when the agent gives one.
    pub active_form: Option<String>,
    /// When a write first showed it in progress; None when none did.
    pub started_at: Option<i64>,
    /// When a write first showed it done; None while it is not done.
    pub ended_at: Option<i64>,
    /// The write it started in (else the first one it showed in), for a click on it to jump to in the transcript.
    pub seq: i64,
}

/// An earlier write of the plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revision {
    pub steps: Vec<Step>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    pub steps: Vec<Step>,
    /// When the agent last wrote the plan; None when it never did.
    pub updated_at: Option<i64>,
    /// The writes before the latest, oldest first, at most [`REVISIONS`].
    pub revisions: Vec<Revision>,
}

/// How many earlier plans a long session keeps.
pub const REVISIONS: usize = 100;

/// The todos in a TodoWrite's input (`timeline.ts` `parseTodos`); an item without text is skipped.
fn todos(input: &Value) -> Vec<(String, StepStatus, Option<String>)> {
    let Some(list) = input.get("todos").and_then(Value::as_array) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|item| {
            let content = item.get("content")?.as_str()?.to_string();
            let status = match item.get("status").and_then(Value::as_str) {
                Some("completed") => StepStatus::Completed,
                Some("in_progress") => StepStatus::InProgress,
                _ => StepStatus::Pending,
            };
            let active = item.get("activeForm").and_then(Value::as_str).map(str::to_string);
            Some((content, status, active))
        })
        .collect()
}

/// What the writes so far said about one step's text.
#[derive(Debug, Clone, Copy)]
struct Seen {
    started_at: Option<i64>,
    ended_at: Option<i64>,
    first_seq: i64,
    start_seq: Option<i64>,
}

/// A session's plan kept as its log grows: [`PlanFolder::push`] folds only the events past those folded before.
#[derive(Debug, Default)]
pub struct PlanFolder {
    seen: HashMap<String, Seen>,
    last_seq: Option<i64>,
    plan: Plan,
}

impl PlanFolder {
    pub fn new() -> PlanFolder {
        PlanFolder::default()
    }

    /// Folds events past the last one folded; a gap in their numbers starts over from them.
    pub fn push<'a>(&mut self, events: impl IntoIterator<Item = &'a StoredEvent>) {
        for stored in events {
            if let Some(last) = self.last_seq {
                if stored.seq <= last {
                    continue;
                }
                if stored.seq != last + 1 {
                    *self = PlanFolder::default();
                }
            }
            self.apply(stored);
            self.last_seq = Some(stored.seq);
        }
    }

    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    fn apply(&mut self, stored: &StoredEvent) {
        // A sub-agent's plan is its own.
        let WbEvent::Tool {
            name,
            input,
            parent: None,
            ..
        } = &stored.event
        else {
            return;
        };
        if name != "TodoWrite" {
            return;
        }
        let (seq, at) = (stored.seq, stored.at);
        let todos = todos(input);
        let keys = step_keys(&todos);
        for ((_, status, _), key) in todos.iter().zip(&keys) {
            let seen = self.seen.entry(key.clone()).or_insert(Seen {
                started_at: None,
                ended_at: None,
                first_seq: seq,
                start_seq: None,
            });
            match status {
                // Put back to do: whatever it took so far no longer counts.
                StepStatus::Pending => {
                    seen.started_at = None;
                    seen.ended_at = None;
                    seen.start_seq = None;
                }
                // Not started yet, or done before and taken up again: it starts now.
                StepStatus::InProgress => {
                    if seen.started_at.is_none() || seen.ended_at.is_some() {
                        seen.started_at = Some(at);
                        seen.start_seq = Some(seq);
                    }
                    seen.ended_at = None;
                }
                StepStatus::Completed => {
                    if seen.ended_at.is_none() {
                        seen.ended_at = Some(at);
                    }
                }
            }
        }
        let steps = todos
            .into_iter()
            .zip(&keys)
            .map(|((content, status, active_form), key)| {
                let seen = self.seen[key];
                Step {
                    content,
                    status,
                    active_form,
                    started_at: seen.started_at,
                    ended_at: if status == StepStatus::Completed {
                        seen.ended_at
                    } else {
                        None
                    },
                    seq: seen.start_seq.unwrap_or(seen.first_seq),
                }
            })
            .collect();
        let before = std::mem::take(&mut self.plan);
        let mut revisions = before.revisions;
        if let Some(updated_at) = before.updated_at {
            revisions.push(Revision {
                steps: before.steps,
                updated_at,
            });
            if revisions.len() > REVISIONS {
                revisions.drain(..revisions.len() - REVISIONS);
            }
        }
        self.plan = Plan {
            steps,
            updated_at: Some(at),
            revisions,
        };
    }
}

/// Each step's key: its text, and for the second and later step with that text in the write, its count too.
fn step_keys(todos: &[(String, StepStatus, Option<String>)]) -> Vec<String> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    todos
        .iter()
        .map(|(content, _, _)| {
            let count = counts.entry(content).or_insert(0);
            let key = if *count == 0 {
                content.clone()
            } else {
                format!("{content}\u{0}{count}")
            };
            *count += 1;
            key
        })
        .collect()
}

/// The plan of a whole log at once.
pub fn derive_plan(events: &[StoredEvent]) -> Plan {
    let mut folder = PlanFolder::new();
    for stored in events {
        folder.apply(stored);
    }
    folder.plan
}

/// How long a step took, or has taken by `now` while it is in progress; None when its start is not known.
pub fn step_time(step: &Step, now: i64) -> Option<i64> {
    let started = step.started_at?;
    match step.status {
        StepStatus::InProgress => Some((now - started).max(0)),
        StepStatus::Completed => step.ended_at.map(|ended| (ended - started).max(0)),
        StepStatus::Pending => None,
    }
}

/// How long the plan has taken: from its first step's start to its last step's end once all are done, else to `now`.
/// None before any step started.
pub fn plan_time(plan: &Plan, now: i64) -> Option<i64> {
    let first = plan.steps.iter().filter_map(|step| step.started_at).min()?;
    let done = plan.steps.iter().all(|step| step.status == StepStatus::Completed);
    let end = if done {
        plan.steps
            .iter()
            .map(|step| step.ended_at.unwrap_or(0))
            .max()
            .unwrap_or(0)
    } else {
        now
    };
    Some((end - first).max(0))
}

/// How many steps are done.
pub fn done_count(plan: &Plan) -> usize {
    plan.steps
        .iter()
        .filter(|step| step.status == StepStatus::Completed)
        .count()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    struct Log(i64);

    impl Log {
        fn at(&mut self, ms: i64, event: WbEvent) -> StoredEvent {
            self.0 += 1;
            StoredEvent {
                thread_id: "t".into(),
                seq: self.0,
                at: ms,
                event,
            }
        }

        fn write_of(&mut self, ms: i64, steps: &[(&str, &str)], parent: Option<&str>) -> StoredEvent {
            let todos: Vec<Value> = steps
                .iter()
                .map(|(content, status)| json!({ "content": content, "status": status, "activeForm": format!("Doing {content}") }))
                .collect();
            let id = format!("w{}", self.0);
            self.at(
                ms,
                WbEvent::Tool {
                    id,
                    name: "TodoWrite".into(),
                    input: json!({ "todos": todos }),
                    parent: parent.map(str::to_string),
                },
            )
        }

        fn write(&mut self, ms: i64, steps: &[(&str, &str)]) -> StoredEvent {
            self.write_of(ms, steps, None)
        }

        fn text(&mut self, ms: i64) -> StoredEvent {
            self.at(
                ms,
                WbEvent::Text {
                    id: "m".into(),
                    text: "hi".into(),
                    parent: None,
                },
            )
        }
    }

    fn times(plan: &Plan) -> Vec<(Option<i64>, Option<i64>)> {
        plan.steps.iter().map(|step| (step.started_at, step.ended_at)).collect()
    }

    #[test]
    fn keeps_only_the_latest_hundred_earlier_plans_during_a_long_session() {
        let mut log = Log(0);
        let events: Vec<StoredEvent> = (0..1000)
            .map(|index| log.write(index + 1, &[("Fix", "in_progress")]))
            .collect();
        let mut folder = PlanFolder::new();
        for stored in &events {
            folder.push([stored]);
            assert!(folder.plan().revisions.len() <= REVISIONS);
        }
        let updated: Vec<i64> = folder
            .plan()
            .revisions
            .iter()
            .map(|revision| revision.updated_at)
            .collect();
        assert_eq!(updated, (900..1000).collect::<Vec<i64>>());
    }

    #[test]
    fn has_no_steps_before_the_agent_writes_a_plan() {
        let mut log = Log(0);
        assert_eq!(derive_plan(&[log.text(1)]), Plan::default());
    }

    #[test]
    fn times_each_step_from_the_write_that_started_it_to_the_write_that_finished_it() {
        let mut log = Log(0);
        let first = log.write(1_000, &[("Read", "in_progress"), ("Fix", "pending")]);
        let second = log.write(4_000, &[("Read", "completed"), ("Fix", "in_progress")]);
        let third = log.write(9_000, &[("Read", "completed"), ("Fix", "completed")]);
        let plan = derive_plan(&[first.clone(), second.clone(), third]);
        assert_eq!(plan.updated_at, Some(9_000));
        assert_eq!(
            plan.steps,
            [
                Step {
                    content: "Read".into(),
                    status: StepStatus::Completed,
                    active_form: Some("Doing Read".into()),
                    started_at: Some(1_000),
                    ended_at: Some(4_000),
                    seq: first.seq,
                },
                Step {
                    content: "Fix".into(),
                    status: StepStatus::Completed,
                    active_form: Some("Doing Fix".into()),
                    started_at: Some(4_000),
                    ended_at: Some(9_000),
                    seq: second.seq,
                },
            ]
        );
        let spent: Vec<Option<i64>> = plan.steps.iter().map(|step| step_time(step, 99_000)).collect();
        assert_eq!(spent, [Some(3_000), Some(5_000)]);
        assert_eq!(plan_time(&plan, 99_000), Some(8_000));
        assert_eq!(done_count(&plan), 2);
    }

    #[test]
    fn counts_a_step_in_progress_up_to_now_and_the_plan_too_while_a_step_is_open() {
        let mut log = Log(0);
        let plan = derive_plan(&[log.write(
            1_000,
            &[("Read", "completed"), ("Fix", "in_progress"), ("Test", "pending")],
        )]);
        // Done in one write: when it started is not known.
        assert_eq!((plan.steps[0].started_at, plan.steps[0].ended_at), (None, Some(1_000)));
        assert_eq!(step_time(&plan.steps[0], 5_000), None);
        assert_eq!(step_time(&plan.steps[1], 5_000), Some(4_000));
        assert_eq!(step_time(&plan.steps[2], 5_000), None);
        assert_eq!(plan_time(&plan, 5_000), Some(4_000));
        assert_eq!(done_count(&plan), 1);
    }

    #[test]
    fn restarts_a_step_put_back_to_do_and_one_taken_up_again_after_it_was_done() {
        let mut log = Log(0);
        let plan = derive_plan(&[
            log.write(1_000, &[("Fix", "in_progress"), ("Test", "in_progress")]),
            log.write(2_000, &[("Fix", "pending"), ("Test", "completed")]),
            log.write(3_000, &[("Fix", "in_progress"), ("Test", "in_progress")]),
        ]);
        assert_eq!(times(&plan), [(Some(3_000), None), (Some(3_000), None)]);
    }

    #[test]
    fn times_two_steps_with_the_same_text_apart() {
        let mut log = Log(0);
        let plan = derive_plan(&[
            log.write(1_000, &[("Test", "in_progress"), ("Test", "pending")]),
            log.write(3_000, &[("Test", "completed"), ("Test", "in_progress")]),
            log.write(7_000, &[("Test", "completed"), ("Test", "completed")]),
        ]);
        assert_eq!(times(&plan), [(Some(1_000), Some(3_000)), (Some(3_000), Some(7_000))]);
    }

    #[test]
    fn follows_the_latest_write_only_and_leaves_out_a_sub_agents_plan() {
        let mut log = Log(0);
        let plan = derive_plan(&[
            log.write(1_000, &[("Old step", "in_progress")]),
            log.write(2_000, &[("New step", "pending")]),
            log.write_of(3_000, &[("Sub-agent step", "in_progress")], Some("agent_1")),
        ]);
        let steps: Vec<&str> = plan.steps.iter().map(|step| step.content.as_str()).collect();
        assert_eq!(steps, ["New step"]);
        assert_eq!(plan_time(&plan, 9_000), None);
    }

    #[test]
    fn keeps_the_plan_until_the_next_write_and_starts_over_on_a_gap() {
        let mut log = Log(0);
        let mut folder = PlanFolder::new();
        folder.push(&[log.write(1_000, &[("Read", "in_progress")])]);
        let first = folder.plan().clone();
        folder.push(&[log.text(2_000)]);
        assert_eq!(folder.plan(), &first);
        folder.push(&[log.write(3_000, &[("Read", "completed")])]);
        let step = &folder.plan().steps[0];
        assert_eq!(
            (step.status, step.started_at, step.ended_at),
            (StepStatus::Completed, Some(1_000), Some(3_000))
        );
        // A page that does not follow on: the plan starts over from it.
        let mut later = Log(10);
        folder.push(&[later.write(5_000, &[("Other", "pending")])]);
        let steps: Vec<&str> = folder.plan().steps.iter().map(|step| step.content.as_str()).collect();
        assert_eq!(steps, ["Other"]);
        assert!(folder.plan().revisions.is_empty());
    }
}
