//! Exit notices for watched `terminal_run` commands.
//!
//! A watched command delivers at most one Nexus notice through [`Workbench::steer_notice`] when its process exits,
//! unless an agent `terminal_read` is waiting or already read the ended status. Each session gets at most
//! [`WATCH_MAX_NOTICES`] notices until the person types a message.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use super::{Execution, State, Terminals};
use crate::Workbench;
use crate::event::{SessionAgent, WbEvent};
use crate::harness::harness;

#[path = "watch_queue.rs"]
mod queue;
#[cfg(test)]
#[path = "watch_tests.rs"]
mod tests;

pub(crate) use queue::{Deliver, ExitQueue};

/// Notices per session until the person types a message, so a loop of watched commands cannot run unattended.
pub(crate) const WATCH_MAX_NOTICES: usize = 10;

/// Where a command goes when its notice is due ([`ExitQueue`]).
pub(crate) type Exited = Arc<ExitQueue>;

/// The row for the first notice dropped because the session's budget is spent. Later drops in the same budget period
/// write nothing. It never matches the notice texts other readers look for ([`Workbench::steer_notice`]).
pub(crate) const BUDGET_SPENT: &str =
    "More watched commands ended. The agent is not told about them until you send a message.";

/// One command's exit notice and what the agent's reads saw of it. The execution's state lock guards it.
#[derive(Default)]
pub(super) struct Watch {
    /// `terminal_run` asked for one exit notice.
    pub(super) requested: bool,
    /// The process exited.
    exited: bool,
    /// The notice was claimed for delivery, or cancelled.
    done: bool,
    /// Agent `terminal_read` calls waiting on the command now. While one waits, it returns the end instead.
    readers: usize,
    /// An agent read or retry returned the ended status, so the agent knows already.
    seen_end: bool,
}

impl Watch {
    fn due(&self) -> bool {
        self.requested && self.exited && !self.done && self.readers == 0 && !self.seen_end
    }

    /// The process exited. True when the notice is due now.
    pub(super) fn exit(&mut self) -> bool {
        self.exited = true;
        self.due()
    }

    pub(super) fn saw_end(&mut self) {
        self.seen_end = true;
    }

    /// No notice for this command, whatever happens next.
    pub(super) fn cancel(&mut self) {
        self.done = true;
    }

    /// Marks the notice sent. False when it is not due.
    fn claim(&mut self) -> bool {
        let due = self.due();
        self.done |= due;
        due
    }
}

/// Holds back the exit notice while an agent `terminal_read` waits: that read returns the end itself. A read that
/// returns early, fails or is cancelled releases it on drop, and the notice goes out if the command ended meanwhile.
pub(super) struct ReaderGuard {
    entry: Arc<Execution>,
    runtime: Option<tokio::runtime::Handle>,
    armed: bool,
}

impl ReaderGuard {
    pub(super) fn new(entry: Arc<Execution>) -> Self {
        Self {
            entry,
            runtime: tokio::runtime::Handle::try_current().ok(),
            armed: false,
        }
    }

    pub(super) fn arm(&mut self, state: &mut State) {
        if !self.armed {
            state.watch.readers += 1;
            self.armed = true;
        }
    }
}

impl Drop for ReaderGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let mut state = self.entry.state();
        state.watch.readers = state.watch.readers.saturating_sub(1);
        if state.watch.due() {
            self.entry.notify_exit(self.runtime.as_ref());
        }
    }
}

#[derive(Default)]
struct Counter {
    sent: usize,
    epoch: u64,
    /// A notice was dropped in this budget period, and its row written.
    drop_noted: bool,
}

/// What the budget allows for one notice.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Slot {
    /// Send it. The epoch lets a failed delivery refund the slot.
    Taken(u64),
    /// The budget is spent and this is the period's first drop: say so once.
    FirstDrop,
    /// The budget is spent and the person was told already.
    Dropped,
}

/// Per-session notice budget, reset only when the person types a message.
#[derive(Default)]
pub(super) struct Budget {
    threads: HashMap<String, Counter>,
}

impl Budget {
    /// Reserves one notice slot for `thread`, or reports a drop when the budget is spent.
    pub(super) fn try_take(&mut self, thread: &str) -> Slot {
        let entry = self.threads.entry(thread.into()).or_default();
        if entry.sent < WATCH_MAX_NOTICES {
            entry.sent += 1;
            return Slot::Taken(entry.epoch);
        }
        if std::mem::replace(&mut entry.drop_noted, true) {
            Slot::Dropped
        } else {
            Slot::FirstDrop
        }
    }

    /// Returns a reserved slot when delivery fails, unless a reset happened in between.
    pub(super) fn refund(&mut self, thread: &str, epoch: u64) {
        if let Some(entry) = self.threads.get_mut(thread)
            && entry.epoch == epoch
        {
            entry.sent = entry.sent.saturating_sub(1);
        }
    }

    /// Resets `thread`'s budget after the person types a message.
    pub(super) fn reset(&mut self, thread: &str) {
        let entry = self.threads.entry(thread.into()).or_default();
        entry.sent = 0;
        entry.drop_noted = false;
        entry.epoch = entry.epoch.wrapping_add(1);
    }

    pub(super) fn remove(&mut self, thread: &str) {
        self.threads.remove(thread);
    }

    pub(super) fn clear(&mut self) {
        self.threads.clear();
    }

    #[cfg(test)]
    fn tracks(&self, thread: &str) -> bool {
        self.threads.contains_key(thread)
    }
}

impl Terminals {
    fn watch_budget<T>(&self, use_budget: impl FnOnce(&mut Budget) -> T) -> T {
        use_budget(&mut self.0.lock().unwrap_or_else(|p| p.into_inner()).watch_budget)
    }

    /// The person typed a message in session `thread`: it gets its full notice budget again.
    pub(crate) fn reset_watch_budget(&self, thread: &str) {
        self.watch_budget(|budget| budget.reset(thread));
    }

    /// Whether session `thread` has a watched command still running: the idle reaper keeps its agent alive longer.
    pub(crate) fn has_watched_running(&self, scope: &str, thread: &str) -> bool {
        let registry = self.0.lock().unwrap_or_else(|p| p.into_inner());
        registry.entries.iter().any(|entry| {
            entry.scope == scope && entry.thread == thread && {
                let state = entry.state();
                state.watch.requested && !state.watch.done && state.info.runs()
            }
        })
    }
}

/// Snapshot of a watched command taken when it exits, ready to format as a notice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WatchExit {
    pub(crate) scope: String,
    pub(crate) thread: String,
    pub(crate) id: String,
    pub(crate) status: String,
    pub(crate) exit_code: Option<u32>,
    /// The name the agent may see (`Info::for_agent`), cleaned for a sentence by [`notice_name`]. Empty: none.
    pub(crate) name: String,
}

/// The characters of a command's name a notice keeps.
const NAME_MAX_CHARS: usize = 60;

/// Cleans a command's name for a sentence: its first line, control, invisible and text-direction characters as
/// spaces, runs of whitespace as one, trimmed, and at most [`NAME_MAX_CHARS`] characters with "..." when cut. A double
/// quote becomes a single quote, so the name cannot break the quotes around it. Names are already cut to 48 characters
/// when they are staged or renamed; this cap keeps the notice short if that ever changes.
pub(crate) fn notice_name(raw: &str) -> String {
    let line = raw.split('\n').next().unwrap_or_default();
    let spaced: String = line
        .chars()
        .map(|ch| match ch {
            '"' => '\'',
            // Zero-width characters, the direction marks and embeddings, the direction isolates, and the BOM.
            '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}' => ' ',
            ch if ch.is_control() => ' ',
            ch => ch,
        })
        .collect();
    let name = spaced.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.chars().count() <= NAME_MAX_CHARS {
        return name;
    }
    let cut: String = name.chars().take(NAME_MAX_CHARS).collect();
    format!("{}...", cut.trim_end())
}

/// Error returned when an agent whose driver cannot steer mid-turn passes `watch: true` to `terminal_run`.
pub(crate) fn unsupported_error() -> String {
    let supported: Vec<&str> = SessionAgent::ALL
        .into_iter()
        .filter(|agent| harness(*agent).session_features().steers_mid_turn)
        .map(|agent| harness(agent).label())
        .collect();
    let support_sentence = match supported.as_slice() {
        [] => "No agents support it yet.".to_string(),
        [only] => format!("Only {only} supports it."),
        [first @ .., last] => format!("Only {} and {last} support it.", first.join(", ")),
    };
    format!(
        "terminal_watch_unsupported: This agent does not support watch on terminal_run. \
         {support_sentence} Read the command with terminal_read instead."
    )
}

/// Which command ended and how, as a sentence. Only `process::Control::Stop` ends a command `interrupted`: the
/// person's Stop or Close (`stop` and `close` ops), a sign-in or scope change (`Terminals::clear`,
/// `Terminals::context`), or deleting its session (`close_thread`, which cancels the notice first). Stop hangs up and
/// then kills the process, and a process killed by a signal reports code 1, which says nothing: the sentence leaves
/// the code out. `terminal_interrupt` only sends Ctrl-C, so the command's own exit decides its status and code. One
/// whose exit status could not be read ends `failed` with none.
fn ended(exit: &WatchExit) -> String {
    let command = match exit.name.as_str() {
        "" => exit.id.clone(),
        name => format!("\"{name}\" ({})", exit.id),
    };
    let how = match (exit.exit_code, exit.status.as_str()) {
        (Some(_), "interrupted") => "was stopped".to_string(),
        (Some(code), _) => format!("exited with code {code}"),
        (None, _) => "ended without an exit code".to_string(),
    };
    format!("The watched command {command} {how}.")
}

/// The notice the agent gets: the command's name, id and how it ended, never its output.
pub(crate) fn format_notice(exit: &WatchExit) -> String {
    format!("{} Read its output with terminal_read.", ended(exit))
}

/// The transcript row for a notice the agent could not take, written for the person. It never matches the notice
/// texts other readers look for ([`Workbench::steer_notice`]).
pub(crate) fn undelivered_notice(exit: &WatchExit) -> String {
    format!(
        "{} The agent was not told: the session was halted or its agent was not running. Send a message to tell it.",
        ended(exit)
    )
}

impl Execution {
    /// Queues the due notice for delivery on `runtime`: `steer_notice` needs Tokio. Called while the caller holds this
    /// execution's state lock, in the hold that published its end ([`ExitQueue`]). `process::start` takes `runtime`
    /// from its caller, and every caller runs on Tokio: the agent's `terminal_run` through the orchestra MCP server
    /// (`terminal::tools`), and the person's Run through `Workbench::terminal_guarded`, which the app calls from its
    /// runner's runtime (`code_run::CodeRunner::terminal_guarded`) and nexus-host from its server (`SERVICE_CALL`).
    /// Should one ever lack a runtime, the delivery writes the undelivered row instead of steering.
    pub(super) fn notify_exit(self: &Arc<Self>, runtime: Option<&tokio::runtime::Handle>) {
        let Some(queue) = &self.exited else {
            return;
        };
        queue.push(self.clone(), runtime);
    }

    /// Claims the exit notice if it is due, and takes its snapshot. Only the first call after the exit gets it.
    pub(super) fn take_watch_exit(&self) -> Option<WatchExit> {
        let mut state = self.state();
        if state.info.interactive || !state.watch.claim() {
            return None;
        }
        Some(WatchExit {
            scope: self.scope.clone(),
            thread: self.thread.clone(),
            id: state.info.id.clone(),
            status: state.info.status.clone(),
            exit_code: state.info.exit_code,
            // Through `for_agent`, so a private command is named "Private terminal" by the one rule that redacts it.
            name: notice_name(&state.info.for_agent().name),
        })
    }
}

impl Workbench {
    /// Delivers a watched command's exit notice to its session if scope, read state and budget allow it. A notice
    /// the agent cannot take leaves a row in the transcript, so the person sees that the agent was not told.
    pub(crate) fn deliver_terminal_watch(&self, entry: Arc<Execution>) {
        let Some(exit) = entry.take_watch_exit() else {
            return;
        };
        if exit.scope != self.config().scope || !self.has_thread(&exit.thread) {
            return;
        }
        let thread_id = &exit.thread;
        // `steer_notice` needs Tokio. Without it the notice is not sent, and the row says so.
        if tokio::runtime::Handle::try_current().is_err() {
            eprintln!(
                "nexus-workbench: {thread_id}: no Tokio runtime, so the exit notice of watched command {} was not sent",
                exit.id
            );
            self.note_watch(thread_id, undelivered_notice(&exit));
            return;
        }
        if catch_unwind(AssertUnwindSafe(|| self.send_watch(&exit))).is_err() {
            eprintln!(
                "nexus-workbench: {thread_id}: sending the exit notice of watched command {} panicked",
                exit.id
            );
            self.note_watch(thread_id, undelivered_notice(&exit));
        }
    }

    /// Sends the notice within the session's budget, or writes the row that says why it was not sent.
    fn send_watch(&self, exit: &WatchExit) {
        let terminals = &self.inner.terminals;
        let thread_id = &exit.thread;
        let epoch = match terminals.watch_budget(|budget| budget.try_take(thread_id)) {
            Slot::Taken(epoch) => epoch,
            Slot::FirstDrop => {
                eprintln!(
                    "nexus-workbench: {thread_id}: the exit notice of watched command {} was dropped: the session's \
                     {WATCH_MAX_NOTICES} notices are spent",
                    exit.id
                );
                self.note_watch(thread_id, BUDGET_SPENT.into());
                return;
            }
            Slot::Dropped => return,
        };
        let Err(error) = self.steer_notice(thread_id, &format_notice(exit)) else {
            return;
        };
        terminals.watch_budget(|budget| budget.refund(thread_id, epoch));
        eprintln!(
            "nexus-workbench: {thread_id}: the exit notice of watched command {} was not delivered: {error}",
            exit.id
        );
        if !self.has_thread(thread_id) {
            terminals.watch_budget(|budget| budget.remove(thread_id));
            return;
        }
        self.note_watch(thread_id, undelivered_notice(exit));
    }

    /// Writes a watch row the agent is not sent, for the person to see.
    fn note_watch(&self, thread_id: &str, text: String) {
        if let Err(error) = self.append(thread_id, WbEvent::Notice { text }) {
            eprintln!("nexus-workbench: {thread_id}: {error}");
        }
    }
}
