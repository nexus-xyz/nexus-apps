//! Session logs for agents the `nexus` CLI runs in a terminal (`nexus claude`, `nexus codex`). The CLI
//! opens its own store and runs the same session sync as the desktop app: it reads the agent's transcript, turns each
//! line into [`WbEvent`]s, and appends them here. The workbench never starts these agents.
//!
//! A thread is made only while session sync can bind it, so a CLI log is never an unowned local copy. Each thread
//! remembers how far its transcript was read, written with the events in one transaction: a resumed conversation
//! continues the same thread from there, and a crash never logs a line twice.

use std::path::Path;

use crate::event::{PermissionMode, SessionAgent, ThreadStatus, ThreadSummary, Via, WbEvent};
use crate::store::ThreadRecord;
use crate::{Result, Workbench, now_ms};

/// A conversation the CLI runs: the agent's own id for it, and where it runs.
#[derive(Debug, Clone)]
pub struct CliRun<'a> {
    pub agent: SessionAgent,
    /// The agent's conversation id (Claude Code's session id, Codex's thread id).
    pub native_id: &'a str,
    pub cwd: &'a Path,
    pub model: Option<&'a str>,
    pub via: Via,
}

/// A CLI conversation's thread, and how many bytes of its transcript are already in the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliThread {
    pub id: String,
    pub read_to: u64,
}

impl Workbench {
    /// The thread of this conversation: the one made before for it, or a new one bound to the current session-sync
    /// identity. None when nothing could sync it: no backend, `AllowLocalCache` off, an in-memory store, or a thread
    /// made for another person.
    pub fn cli_thread(&self, run: &CliRun) -> Result<Option<CliThread>> {
        let scope = self.config().scope;
        let Some(identity) = self.session_sync_identity(&scope) else {
            return Ok(None);
        };
        let store = self.store();
        if let Some(found) = store.cli_transcript(&scope, run.agent, run.native_id)? {
            let owned = store
                .sync_target(&found.id)?
                .is_some_and(|target| target.identity == identity);
            return Ok(owned.then_some(found));
        }
        let now = now_ms();
        let project = store.add_project(run.cwd, now)?;
        let record = ThreadRecord {
            summary: ThreadSummary {
                name: None,
                id: format!("t_{}", uuid::Uuid::new_v4().simple()),
                title: format!("{} in {}", label(run.agent), project.name),
                project_id: project.id,
                agent: run.agent,
                model: run.model.map(String::from),
                permission_mode: PermissionMode::Default,
                effort: None,
                fast: false,
                via: run.via,
                provider: None,
                account: None,
                status: ThreadStatus::Idle,
                cost_usd: 0.0,
                nexus_sessions: vec![],
                context_tokens: None,
                context_window: None,
                created_at: now,
                updated_at: now,
                listed_at: Some(now),
                acted_at: Some(now),
                archived: false,
                pinned: false,
                worktree: None,
                pull_requests: Vec::new(),
                pending: None,
                capabilities: None,
                parent_thread_id: None,
                source_thread_id: None,
                side_chat: false,
                spawned_by: None,
                group_id: None,
                depth: 0,
                orchestration: false,
                waits_on: vec![],
                pending_results: 0,
                start_choices: vec![],
                halt: None,
                crew_paused: false,
                crew_tripped: false,
                crew_spend_trip: None,
                in_background: false,
                background: vec![],
                background_total: 0,
                wake: None,
                at_depth_limit: false,
                handoff_from: None,
                limited: None,
                aside: Default::default(),
            },
            native_id: run.native_id.to_string(),
            native_started: true,
            scope,
            fork_from: None,
            seed: None,
            automatic_title: true,
            delivered_seq: 0,
            halt: None,
        };
        let made = store.insert_cli_thread(&record, &identity)?;
        Ok(made.then_some(CliThread {
            id: record.summary.id,
            read_to: 0,
        }))
    }

    /// Appends `events` (`(at, event)`, in order) and records that the transcript is read up to `read_to`, in one
    /// transaction. The first prompt names the thread.
    pub fn cli_append(&self, thread: &str, events: &[(i64, WbEvent)], read_to: u64) -> Result<()> {
        let title = events.iter().find_map(|(_, event)| match event {
            WbEvent::User { text, .. } if !text.trim().is_empty() => {
                Some(crate::title_of(&crate::mentions::plain(text)))
            }
            _ => None,
        });
        self.store().cli_append(thread, events, read_to, title.as_deref())
    }

    /// Appends live events that no transcript records (a phone's notices, the agent's approvals from its app-server),
    /// and leaves the transcript's mark where it is.
    pub fn cli_events(&self, thread: &str, events: &[(i64, WbEvent)]) -> Result<()> {
        self.store().cli_events(thread, events)
    }

    /// How many logged events Gateway has not confirmed yet, for the person signed in now: what a closing run reports
    /// as waiting for the next one.
    pub fn cli_unsynced(&self) -> Result<u64> {
        let Some(backend) = self.inner.host.session_sync() else {
            return Ok(0);
        };
        self.store().cli_unsynced(backend.identity())
    }
}

fn label(agent: SessionAgent) -> &'static str {
    crate::harness::harness(agent).log_title()
}

#[cfg(test)]
#[path = "cli_log/tests.rs"]
mod tests;
