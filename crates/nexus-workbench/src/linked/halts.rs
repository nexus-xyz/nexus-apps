//! Halts: the person's Stop and Interrupt
//! halt a session, Stop and Delete also halt and stop every session below it, and a halted session runs no automatic
//! turn until the person writes to it or to a session above it. A halt is a column (`threads.halt`), so it outlives
//! the process and a restart.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use crate::event::{ThreadStatus, TurnStatus, WbEvent};
use crate::store::ThreadRecord;
use crate::{Emit, StopReason, Workbench, now_ms, process, short_id};

/// Why a linked session's message or a result did not reach a halted session, or a halted linked session did not
/// start: what `start` answers, and what a parent's tool call then reports. The person stopped or interrupted it.
pub(crate) const HALTED_BY_PERSON: &str = "The person stopped this session. Only they can start it again.";
/// [`HALTED_BY_PERSON`] for the cascade's halt: the person stopped or deleted a session above it.
pub(crate) const HALTED_ABOVE: &str = "The person stopped a session above this one. Only they can start it again.";

/// What `start` answers for a session halted with `halt`. A halt it can't read is the person's.
pub(crate) fn refusal(halt: Halt) -> &'static str {
    match halt {
        Halt::Parent => HALTED_ABOVE,
        Halt::Person | Halt::Fault => HALTED_BY_PERSON,
    }
}

/// Whether `start`'s error is a halt's refusal.
pub(crate) fn is_refusal(error: &str) -> bool {
    error == HALTED_BY_PERSON || error == HALTED_ABOVE
}

/// `start_session`'s refusal under a halt above the parent.
const STOPPED_ABOVE: &str =
    "The person stopped a session above this one. It starts no sessions until they write to that session.";

/// How many sessions below one the cascade walks at most: far past any tree the limits allow.
const MAX_BELOW: usize = 1024;

/// Why a session may run no automatic turn (`ThreadRecord::halt`). Only a person message clears one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Halt {
    /// The person stopped or interrupted this session.
    Person,
    /// The person stopped or deleted a session above it (the cascade). A person message to it or to any session above
    /// it clears it.
    Parent,
    /// A value this app does not know (a newer app wrote it). It holds as the person's does: a halt never lifts on a
    /// value it can't read.
    Fault,
}

impl Halt {
    pub fn as_str(self) -> &'static str {
        match self {
            Halt::Person => "person",
            Halt::Parent => "parent",
            Halt::Fault => "fault",
        }
    }

    pub fn parse(value: &str) -> Halt {
        match value {
            "person" => Halt::Person,
            "parent" => Halt::Parent,
            _ => Halt::Fault,
        }
    }

    pub fn of(record: &ThreadRecord) -> Option<Halt> {
        record.halt.as_deref().map(Halt::parse)
    }
}

impl Workbench {
    /// The session's halt; None when it is free, or no such session.
    pub fn halt(&self, thread_id: &str) -> crate::Result<Option<Halt>> {
        Ok(self.reader().thread(thread_id)?.as_ref().and_then(Halt::of))
    }

    /// The sessions below `thread_id`, each before the ones it started (depth first), as far as their rows go. A
    /// cycle (only a damaged store has one) is walked once.
    pub(crate) fn descendants(&self, thread_id: &str) -> crate::Result<Vec<ThreadRecord>> {
        let mut found = Vec::new();
        let mut seen: HashSet<String> = HashSet::from([thread_id.to_string()]);
        let mut stack: Vec<ThreadRecord> = self.reader().children(thread_id)?.into_iter().rev().collect();
        while let Some(record) = stack.pop() {
            if found.len() >= MAX_BELOW || !seen.insert(record.summary.id.clone()) {
                continue;
            }
            stack.extend(self.reader().children(&record.summary.id)?.into_iter().rev());
            found.push(record);
        }
        Ok(found)
    }

    /// Saves the session's halt (None clears it), when it changes.
    fn put_halt(&self, record: &ThreadRecord, halt: Option<Halt>) -> crate::Result<()> {
        if Halt::of(record) == halt {
            return Ok(());
        }
        self.store().set_halt(&record.summary.id, halt.map(Halt::as_str))
    }

    /// The person's Interrupt halts the session: its process, its background sub-agents and
    /// the sessions below it stay. Written under its run's state lock, where a message's turn begins (`begin_turn`): a
    /// parent's message or a result either began its turn before, and the interrupt then stops that turn, or finds the
    /// halt and is refused.
    pub(crate) fn halt_for_interrupt(&self, thread_id: &str) -> crate::Result<()> {
        let live = self.live(thread_id);
        let _state = live.as_ref().map(|live| live.state());
        match self.reader().thread(thread_id)? {
            Some(record) => self.put_halt(&record, Some(Halt::Person)),
            None => Ok(()),
        }
    }

    /// The cascade: with `halt`, the session is halted with it, and every session below it with
    /// no halt of its own as [`Halt::Parent`]; then each running one stops, the deepest first, and each still starting
    /// ends as stopped at once. It walks the rows, so it reaches sessions that are starting or not running, and it runs
    /// whether or not the session itself runs. A person's halt is never weakened to a parent's. With no `halt` (the
    /// parent's `stop_session`), nothing is halted, so its parent may message it again. Returns the sessions below.
    ///
    /// The halts and the walk are written and read under the trees' lock, which `start_session` admits a child under:
    /// a child is admitted before (and found below) or refused after. Both are best effort: a row that can't be read or
    /// written still leaves every process it can reach stopped, the session's own at least, and the error is returned
    /// after.
    pub(crate) fn stop_tree(&self, thread_id: &str, halt: Option<Halt>) -> crate::Result<Vec<ThreadRecord>> {
        let (record, below, marked) = {
            let _admissions = self.trees();
            self.mark_tree(thread_id, halt)
        };
        for record in below.iter().rev() {
            self.stop_one(&record.summary.id, Some(record));
        }
        self.stop_one(thread_id, record.as_ref());
        marked.map(|()| below)
    }

    /// [`Workbench::stop_tree`]'s halts and walk: the session's row (None when there is none, or it can't be read),
    /// the sessions below it, and the first error, if any.
    fn mark_tree(
        &self,
        thread_id: &str,
        halt: Option<Halt>,
    ) -> (Option<ThreadRecord>, Vec<ThreadRecord>, crate::Result<()>) {
        let mut marked = Ok(());
        let record = match self.reader().thread(thread_id) {
            Ok(record) => record,
            Err(error) => {
                marked = Err(error);
                None
            }
        };
        let below = match self.descendants(thread_id) {
            Ok(below) => below,
            Err(error) => {
                marked = marked.and(Err(error));
                Vec::new()
            }
        };
        if halt.is_some() {
            let persisted = self.store().halt_crew(thread_id);
            marked = marked.and(persisted);
        }
        (record, below, marked)
    }

    /// Stops the session's agent, or ends it as stopped while its worktree is still being made: its setup script stops,
    /// and its setup task finds it ended and sends nothing (U3). With no row, only its agent.
    fn stop_one(&self, id: &str, record: Option<&ThreadRecord>) {
        if let Some(live) = self.live(id) {
            self.stop_live(&live, StopReason::Stop, None);
            return;
        }
        let Some(record) = record else {
            return;
        };
        if record.summary.parent_thread_id.is_none() || record.summary.status != ThreadStatus::Starting {
            return;
        }
        self.stop_setup(id);
        match self.end_unstarted(id) {
            Ok(true) => self.child_moved(id),
            Ok(false) => {}
            Err(error) => eprintln!("nexus-workbench: {id}: {error}"),
        }
    }

    /// Delete's cascade: as Stop's, then it waits until neither the session nor any session below it runs, a first
    /// prompt or a parent's message on its way included. Returns the sessions below.
    pub(crate) async fn stop_tree_and_wait(&self, thread_id: &str) -> crate::Result<Vec<ThreadRecord>> {
        let below = self.stop_tree(thread_id, Some(Halt::Person))?;
        // Windows has no process group to end once the agent is gone (`process::end_leftovers`): the shells, MCP
        // servers and dev servers it started live on, and hold the session's folder open. A delete keeps nothing of
        // the run, so the deleted session's whole tree ends now (`taskkill /T`), while the agent still leads it.
        if cfg!(windows)
            && let Some(live) = self.live(thread_id)
        {
            let pid = {
                let state = live.state();
                if state.reaped { None } else { state.pid }
            };
            process::kill(pid);
        }
        let ids: Vec<&str> = std::iter::once(thread_id)
            .chain(below.iter().map(|record| record.summary.id.as_str()))
            .collect();
        // A first prompt or a parent's message on its way has either started its agent, which is stopped below, or
        // finds its session ended or halted.
        for id in &ids {
            for gate in [first_prompt_gate(id), message_gate(id)] {
                let gate = self.creation_gate(&gate);
                drop(gate.lock().await);
            }
        }
        let deadline = Instant::now() + process::GRACE + process::TERM_GRACE + Duration::from_secs(1);
        loop {
            let running: Vec<_> = ids.iter().filter_map(|id| self.live(id)).collect();
            if running.is_empty() {
                return Ok(below);
            }
            if Instant::now() >= deadline {
                return Err("The agent is still stopping. Try again in a moment.".into());
            }
            for live in running {
                self.stop_live(&live, StopReason::Stop, None);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// After a delete: the sessions it started become sessions of the person's own, roots at depth
    /// 0, with their halts cleared and their worktrees kept. Every descendant becomes a root.
    /// Orchestration stays as it was.
    pub(crate) fn free_children(&self, parent_id: &str) -> crate::Result<()> {
        // Show every freed descendant with its new root link and depth.
        let freed = self.descendants(parent_id)?;
        self.store().free_children(parent_id)?;
        for record in freed {
            self.emit_summary(&record.summary.id);
        }
        Ok(())
    }

    /// A person message to the session: its own halt is cleared, and the cascade's on the
    /// sessions below it. A halt the person put on one of those stays.
    pub(crate) fn person_clears(&self, record: &ThreadRecord) -> crate::Result<()> {
        self.store().clear_halts(&record.summary.id)
    }

    /// `start`'s check, under the lock of the running agents: a halted linked session does not start.
    /// Its halt; None when it may start.
    pub(crate) fn halted_link(&self, thread_id: &str) -> crate::Result<Option<Halt>> {
        Ok(self
            .reader()
            .thread(thread_id)?
            .filter(|record| record.summary.parent_thread_id.is_some())
            .as_ref()
            .and_then(Halt::of))
    }

    /// A start [`Workbench::halted_link`] refused: its message ends as a stopped turn, which its parent gets as a
    /// result. What `start` answers.
    pub(crate) fn refuse_halted(&self, thread_id: &str, halt: Halt) -> String {
        match self.end_unstarted(thread_id) {
            Ok(true) => self.child_moved(thread_id),
            Ok(false) => {}
            Err(error) => eprintln!("nexus-workbench: {thread_id}: {error}"),
        }
        refusal(halt).into()
    }

    /// `begin_turn`'s check for a linked session's message or a result to a running agent, under its run's state lock
    /// (where Interrupt writes its halt): why the halted session takes no such message. A halt that can't be read
    /// counts as the person's.
    pub(crate) fn message_refusal(&self, thread_id: &str) -> Option<&'static str> {
        match self.reader().thread(thread_id) {
            Ok(record) => record.as_ref().and_then(Halt::of).map(refusal),
            Err(_) => Some(refusal(Halt::Fault)),
        }
    }

    /// A message `start` gave no turn: when a halt refused it, its row is taken back (`QueueCancel`), so it reads as
    /// not sent, and a revert never counts it as read. Returns the error.
    pub(crate) fn not_taken(&self, thread_id: &str, seq: i64, error: String) -> String {
        if is_refusal(&error)
            && let Err(failed) = self.append(thread_id, WbEvent::QueueCancel { seq })
        {
            eprintln!("nexus-workbench: {thread_id}: {failed}");
        }
        error
    }

    /// The turn of the person's own message begins, under its run's state lock: the session's halt goes, so an
    /// Interrupt after the message was sent holds only until then. The halts below it
    /// went when the message was logged ([`Workbench::person_clears`]).
    pub(crate) fn person_turn_begins(&self, thread_id: &str) {
        let cleared = match self.reader().thread(thread_id) {
            Ok(Some(record)) => self.put_halt(&record, None),
            Ok(None) => Ok(()),
            Err(error) => Err(error),
        };
        if let Err(error) = cleared {
            eprintln!("nexus-workbench: {thread_id}: {error}");
        }
    }

    /// Why `start_session` refuses for `parent_id`: a halt on it or on any session above it, read now. None when there
    /// is none.
    pub(crate) fn start_refusal(&self, parent_id: &str) -> crate::Result<Option<&'static str>> {
        // A parent deleted since the caller read it: its children would have no one to answer to.
        let Some(parent) = self.reader().thread(parent_id)? else {
            return Ok(Some("That session was deleted, so it starts no sessions."));
        };
        if parent.summary.archived {
            return Ok(Some("An archived session cannot start another session."));
        }
        Ok(match Halt::of(&parent) {
            Some(Halt::Person | Halt::Fault) => {
                Some("The person stopped this session. It starts no sessions until they write to it.")
            }
            Some(Halt::Parent) => Some(STOPPED_ABOVE),
            None if self.ancestors(&parent)?.iter().any(|above| above.halt.is_some()) => Some(STOPPED_ABOVE),
            None => None,
        })
    }

    /// A session whose agent does not run ends as stopped: a turn that ends interrupted after its last message (or
    /// with none, a child stopped before its first prompt), and the status Interrupted. Nothing when it runs, a turn
    /// is open, or its log already ends a turn after its last message. Checked and written under the store's lock, so
    /// the cascade and a refused start never both end it.
    pub(crate) fn end_unstarted(&self, thread_id: &str) -> crate::Result<bool> {
        if self.is_live(thread_id) {
            return Ok(false);
        }
        let store = self.store();
        let Some(mut record) = store.thread(thread_id)? else {
            return Ok(false);
        };
        let last = self.reader().last_seqs(thread_id, record.summary.created_at)?;
        if last.turn_start > last.turn_end || (last.turn_end.is_some() && last.user <= last.turn_end) {
            return Ok(false);
        }
        let turn_id = format!("turn_{}", short_id());
        let shown = self.shown(thread_id);
        for event in [
            WbEvent::TurnStart {
                turn_id: turn_id.clone(),
            },
            WbEvent::TurnEnd {
                turn_id,
                status: TurnStatus::Interrupted,
                cost_usd: None,
                duration_ms: 0,
                error: None,
                cursor: None,
            },
        ] {
            let stored = store.append(thread_id, now_ms(), &event)?;
            if shown {
                self.inner.host.emit(Emit::Event(stored));
            }
        }
        record.summary.status = ThreadStatus::Interrupted;
        record.summary.updated_at = now_ms();
        store.save_thread(&record)?;
        drop(store);
        self.emit_summary(thread_id);
        Ok(true)
    }

    /// The session's agent began a turn of its own (Claude Code answering a background sub-agent). In a halted session
    /// or a tree past its spend cap, it is interrupted at once, with no new halt. A halt
    /// that can't be read counts as one.
    pub(crate) fn own_turn_begun(&self, thread_id: &str) {
        let halted = match self.reader().thread(thread_id) {
            Ok(record) => record.is_some_and(|record| record.halt.is_some()),
            Err(_) => true,
        };
        if halted {
            if let Err(error) = self.stop_turn(thread_id, crate::InterruptSource::Halt) {
                eprintln!("nexus-workbench: {thread_id}: {error}");
            }
        } else {
            self.quiet_if_tripped(thread_id);
        }
    }
}

/// The lock a linked session's first prompt holds from its last check until its agent is registered: Delete waits for
/// it, so a child it ended never starts after its halt is cleared.
pub(crate) fn first_prompt_gate(thread_id: &str) -> String {
    format!("first-prompt:{thread_id}")
}

/// The lock a parent's message to a linked session (`message_session`) holds until `start` has taken it or refused
/// it: one message at a time per child, and Delete waits for it.
pub(crate) fn message_gate(thread_id: &str) -> String {
    format!("message:{thread_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_halt_it_cannot_read_holds() {
        for halt in [Halt::Person, Halt::Parent, Halt::Fault] {
            assert_eq!(Halt::parse(halt.as_str()), halt);
        }
        assert_eq!(Halt::parse("paused-by-a-newer-app"), Halt::Fault);
        assert_eq!(Halt::parse(""), Halt::Fault);
    }
}
