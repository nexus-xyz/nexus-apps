//! Sub-agents in a Code transcript, as the Nexus app shows them (`tool-view.tsx`'s `AgentRow`, `work-run.tsx`'s
//! `AgentBody`). Where each block goes is `nexus_workbench::subagent`'s rule; how each sub-agent stands is the shared
//! agent tree's (`crew_fold`). This file turns both into the transcript's flat list of blocks.

use std::collections::HashMap;

use nexus_workbench::crew_fold::{self, AgentNode, AgentStatus, Crew, StatusSource};
use nexus_workbench::event::{SessionAgent, StoredEvent, WbEvent};
use nexus_workbench::subagent::{AgentCall, Flat, Nest};
use serde_json::Value;

use super::Block;

/// A sub-agent's row: what it does, its type, how it stands and how many steps it took.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentRow {
    /// Its Task or Agent call: the key its row opens by.
    pub call_id: String,
    /// What it does (its description, else its prompt's first line); empty when it has neither.
    pub title: String,
    /// Its type ("Explore").
    pub role: Option<String>,
    /// What it was asked: the row's tooltip.
    pub prompt: Option<String>,
    pub status: AgentStatus,
    pub policy: Option<String>,
    /// Tool calls in its whole subtree: its own sub-agents' count too.
    pub steps: u64,
    /// What it does now, while it works.
    pub activity: Option<String>,
    /// What it cost, when its agent reports it.
    pub cost_usd: Option<f64>,
    /// What it handed back: its own summary once it ended, else its call's result. None while it runs, and for a
    /// background agent's launch receipt.
    pub outcome: Option<String>,
    /// It runs past its call (a background agent): its call's result is a launch receipt.
    receipt: bool,
    /// The agent tree knows it, so its status is the tree's.
    tracked: bool,
}

impl AgentRow {
    /// The row of call `id` with `input`, standing as `node` (its node in the agent tree) says.
    pub fn of(id: &str, input: &Value, node: Option<&AgentNode>) -> AgentRow {
        let call = AgentCall::of(input);
        AgentRow {
            call_id: id.to_string(),
            title: call.title,
            role: call.role,
            prompt: call.prompt,
            status: node.map_or(AgentStatus::Running, |node| node.status),
            policy: node
                .and_then(|node| policy_of(node.status, node.reason.as_deref()))
                .map(str::to_string),
            steps: node.map_or(0, |node| node.steps),
            activity: node.and_then(|node| node.last.as_ref()).map(|last| last.text.clone()),
            cost_usd: node.and_then(|node| node.cost_usd).filter(|cost| *cost > 0.),
            // Its own words once it ended, when it reported them.
            outcome: node
                .filter(|node| !node.status.is_live() && node.status_source == StatusSource::Reported)
                .and_then(|node| node.summary.clone()),
            receipt: node.and_then(|node| node.backgrounded).unwrap_or(false),
            tracked: node.is_some(),
        }
    }

    /// Its call's result landed. A background agent's launch receipt is no result.
    pub fn ended(&mut self, output: String, is_error: bool) {
        if self.outcome.is_none() && !self.receipt {
            self.outcome = Some(output).filter(|output| !output.trim().is_empty());
        }
        if !self.tracked {
            self.status = if is_error {
                AgentStatus::Failed
            } else {
                AgentStatus::Done
            };
        }
    }

    /// Running, needing the person or waiting.
    pub fn running(&self) -> bool {
        self.status.is_live()
    }

    /// The line after its name: what it does now while it works, else its type.
    pub fn aside(&self) -> Option<&str> {
        match self.status {
            AgentStatus::Running | AgentStatus::Waiting => self.activity.as_deref().or(self.role.as_deref()),
            _ => self.role.as_deref(),
        }
    }

    /// Its cost when its agent reports one, else how many steps it took (`AgentRow`'s `meta`).
    pub fn meta(&self) -> Option<String> {
        match (self.cost_usd, self.steps) {
            (Some(cost), _) => Some(super::usd(cost)),
            (None, 0) => None,
            (None, 1) => Some("1 step".into()),
            (None, steps) => Some(format!("{steps} steps")),
        }
    }
}

/// The end of a sub-agent's open body.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentEnd {
    pub call_id: String,
    pub running: bool,
    /// Nothing went under it yet: "Starting…" while it runs, else "No steps recorded."
    pub empty: bool,
    /// Its result, shown once it stopped running.
    pub outcome: Option<String>,
}

/// The session's agent tree, from the transcript's events.
pub(super) fn crew(events: &[(i64, i64, WbEvent)]) -> Crew {
    let stored: Vec<StoredEvent> = events
        .iter()
        .map(|(seq, at, event)| StoredEvent {
            thread_id: String::new(),
            seq: *seq,
            at: *at,
            event: event.clone(),
        })
        .collect();
    // Only the controls and the lead's name read the agent; a sub-agent's status and steps do not.
    crew_fold::fold_crew(&stored, SessionAgent::Claude)
}

/// The transcript's blocks in reading order: each sub-agent's row, then its blocks, then its end.
#[cfg(test)]
pub(super) fn flatten(nest: Nest<Block>) -> Vec<Block> {
    let entries = nest.flatten().into_iter().map(|item| match item {
        Flat::Entry { path, entry } => Flat::Entry {
            path,
            entry: (i64::MAX, entry),
        },
        Flat::End { path, call } => Flat::End { path, call },
    });
    flatten_entries(entries).0
}

pub(super) fn flatten_at(nest: Nest<(i64, Block)>) -> (Vec<Block>, Vec<i64>) {
    flatten_entries(nest.flatten())
}

fn flatten_entries(entries: impl IntoIterator<Item = Flat<(i64, Block)>>) -> (Vec<Block>, Vec<i64>) {
    let mut starts = Vec::new();
    let mut rows: HashMap<String, (usize, AgentEnd)> = HashMap::new();
    let mut out: Vec<Block> = Vec::new();
    for item in entries {
        match item {
            Flat::Entry {
                path,
                entry: (seq, entry),
            } => {
                if let Block::Agent(row) = &entry {
                    let end = AgentEnd {
                        call_id: row.call_id.clone(),
                        running: row.running(),
                        empty: true,
                        outcome: row.outcome.clone(),
                    };
                    rows.insert(row.call_id.clone(), (out.len(), end));
                }
                out.push(wrap(path, entry));
                starts.push(seq);
            }
            Flat::End { path, call } => {
                let Some((at, mut end)) = rows.remove(&call) else {
                    continue;
                };
                end.empty = at + 1 == out.len();
                out.push(wrap(path, Block::AgentEnd(Box::new(end))));
                starts.push(i64::MAX);
            }
        }
    }
    (out, starts)
}

fn wrap(path: Vec<String>, block: Block) -> Block {
    if path.is_empty() {
        block
    } else {
        Block::Inside {
            path,
            block: Box::new(block),
        }
    }
}

/// The policy boundary: a refusal is blocked, never a red failure.
pub fn policy_of(status: AgentStatus, reason: Option<&str>) -> Option<&str> {
    if status != AgentStatus::Failed {
        return None;
    }
    let reason = reason?;
    if !reason.get(..6)?.eq_ignore_ascii_case("policy") {
        return None;
    }
    let tail = &reason[6..];
    if tail
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return None;
    }
    Some(tail.trim_start_matches(':').trim())
}

#[cfg(test)]
mod tests;
