//! Sub-agents (Claude Code's Task / Agent tool) in a session's timeline. A sub-agent's own work arrives with `parent`
//! set to the tool call that started it. Its calls, thinking and text sit under that call, never in the main answer.
//! The Nexus app folds this in TypeScript (the earlier desktop app's TypeScript's `place`, and `subagent.ts`); this is
//! the same rule for Nexus GPUI, with their tests (`subagent/tests.rs`).
//!
//! [`Nest`] holds a timeline's entries as a tree: an entry whose `parent` names a call it holds goes under that call,
//! and every other entry goes at the top. [`Nest::flatten`] hands the tree out in reading order, each entry with the
//! calls above it.

use std::collections::HashMap;

use serde_json::Value;

use crate::crew_fold::first_line;

/// Whether a call starts a sub-agent (the Nexus web app kind "agent").
pub fn is_agent_tool(name: &str) -> bool {
    matches!(name, "Task" | "Agent")
}

/// How a sub-agent's call reads (the Nexus web app for an Agent call).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCall {
    /// What it does: its description, else its prompt's first line. Empty when it has neither.
    pub title: String,
    /// Its type (`subagent_type`, such as "Explore").
    pub role: Option<String>,
    /// What it was asked.
    pub prompt: Option<String>,
}

impl AgentCall {
    pub fn of(input: &Value) -> AgentCall {
        let text = |key: &str| {
            input
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
        };
        let prompt = text("prompt");
        AgentCall {
            title: text("description")
                .or_else(|| prompt.as_deref().map(|prompt| first_line(prompt, 80)))
                .unwrap_or_default(),
            role: text("subagent_type"),
            prompt,
        }
    }
}

/// Where an entry sits in a [`Nest`]: hand it back to [`Nest::get_mut`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Slot(usize);

/// What [`Nest::flatten`] hands out, in reading order.
#[derive(Debug, Clone, PartialEq)]
pub enum Flat<B> {
    /// An entry, with the calls above it, outermost first (empty at the top).
    Entry { path: Vec<String>, entry: B },
    /// The end of what went under call `call` (placed with [`Nest::place_call`]), even when nothing did. `path`: the
    /// calls above that call's entries, `call` last.
    End { path: Vec<String>, call: String },
}

/// A timeline's entries, each sub-agent's under the call that started it.
#[derive(Debug)]
pub struct Nest<B> {
    entries: Vec<Option<B>>,
    top: Vec<usize>,
    children: Vec<Vec<usize>>,
    /// Each call placed so far, by its id: a later entry whose `parent` names one goes under it.
    calls: HashMap<String, usize>,
    /// The call each entry is, by its slot.
    call_of: HashMap<usize, String>,
}

impl<B> Default for Nest<B> {
    fn default() -> Self {
        Nest {
            entries: Vec::new(),
            top: Vec::new(),
            children: Vec::new(),
            calls: HashMap::new(),
            call_of: HashMap::new(),
        }
    }
}

impl<B> Nest<B> {
    /// The call an entry with `parent` goes under: the parent's call when this nest holds it. A parent it does not
    /// hold (its call is on an older page, or was never shown) leaves the entry at the top, as `timeline.ts` does.
    pub fn owner(&self, parent: Option<&str>) -> Option<Slot> {
        parent.and_then(|parent| self.calls.get(parent)).map(|&ix| Slot(ix))
    }

    /// How many entries were ever placed, removed ones included: a reader compares two counts to see whether anything
    /// was placed between them.
    pub fn placed(&self) -> usize {
        self.entries.len()
    }

    /// Places `entry`: under its sub-agent's call (`parent`), else at the top.
    pub fn place(&mut self, entry: B, parent: Option<&str>) -> Slot {
        let owner = self.owner(parent);
        let ix = self.entries.len();
        self.entries.push(Some(entry));
        self.children.push(Vec::new());
        match owner {
            Some(Slot(owner)) => self.children[owner].push(ix),
            None => self.top.push(ix),
        }
        Slot(ix)
    }

    /// Places a tool call: as [`Nest::place`], and later entries whose `parent` is `id` go under it.
    pub fn place_call(&mut self, id: &str, entry: B, parent: Option<&str>) -> Slot {
        let slot = self.place(entry, parent);
        self.calls.insert(id.to_string(), slot.0);
        self.call_of.insert(slot.0, id.to_string());
        slot
    }

    pub fn get_mut(&mut self, slot: Slot) -> Option<&mut B> {
        self.entries.get_mut(slot.0).and_then(Option::as_mut)
    }

    /// The last entry among the ones that share an owner: the top's (`None`), or a call's.
    pub fn last_mut(&mut self, owner: Option<Slot>) -> Option<&mut B> {
        let list = match owner {
            Some(Slot(owner)) => &self.children[owner],
            None => &self.top,
        };
        let &ix = list.iter().rev().find(|&&ix| self.entries[ix].is_some())?;
        self.entries[ix].as_mut()
    }

    /// Takes an entry out, with everything under it.
    pub fn remove(&mut self, slot: Slot) {
        let mut stack = vec![slot.0];
        while let Some(ix) = stack.pop() {
            if let Some(entry) = self.entries.get_mut(ix) {
                *entry = None;
                stack.extend(self.children[ix].iter().copied());
            }
        }
    }

    /// Adds entries at the top, after the others.
    pub fn extend_top(&mut self, entries: impl IntoIterator<Item = B>) {
        for entry in entries {
            self.place(entry, None);
        }
    }

    /// Every entry in reading order: each call, then what went under it and its end, then the next entry.
    pub fn flatten(mut self) -> Vec<Flat<B>> {
        let mut out = Vec::with_capacity(self.entries.len());
        let mut path: Vec<String> = Vec::new();
        let top = std::mem::take(&mut self.top);
        self.walk(&top, &mut path, &mut out);
        out
    }

    fn walk(&mut self, list: &[usize], path: &mut Vec<String>, out: &mut Vec<Flat<B>>) {
        for &ix in list {
            let Some(entry) = self.entries[ix].take() else {
                continue;
            };
            out.push(Flat::Entry {
                path: path.clone(),
                entry,
            });
            let Some(call) = self.call_of.get(&ix).cloned() else {
                continue;
            };
            let children = std::mem::take(&mut self.children[ix]);
            path.push(call.clone());
            self.walk(&children, path, out);
            out.push(Flat::End {
                path: path.clone(),
                call,
            });
            path.pop();
        }
    }
}

#[cfg(test)]
mod tests;
