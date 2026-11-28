//! Code sessions, read from the Nexus app's own event log (`workbench.sqlite3`, through `nexus-workbench`'s store), so
//! the sidebar and the transcript show the same sessions as the Nexus app, live while it runs them.
//!
//! Reading needs no lock. Running sessions does: `code_run.rs` takes the store's lock (`run/workbench.lock`) when the
//! Nexus app does not hold it, and this reader then follows what this build's own agents write.

use std::collections::{BTreeMap, HashMap, HashSet};

use nexus_desktop_core::page_links;
use nexus_workbench::crew_fold::{CrewCounts, fold_crew};
use nexus_workbench::event::{
    Decision, Effort, PermissionMode, PermissionSuggestion, SessionAgent, StoredEvent, ThreadStatus, TurnStatus,
    WbEvent,
};
use nexus_workbench::interrupt::interrupted_since_message;
use nexus_workbench::queued_delivery::Queue;
use nexus_workbench::session_scope::SessionScope;
use nexus_workbench::store::Store;
use nexus_workbench::subagent::{self, Nest, Slot};
use serde_json::Value;

#[cfg(test)]
mod overview_tests;
mod subagents;
pub use subagents::{AgentEnd, AgentRow, policy_of};

use crate::side::browser::auto_preview::{self, Signal};

/// How many events the transcript reads at a time: the latest ones as a session opens, then a page more each time the
/// reader scrolls near its top (the Nexus app's `wb_thread_page` pages, `store.ts`'s `loadOlder`).
pub const EVENTS: usize = 600;

/// The local machine name used by Workbench surfaces.
pub const THIS_COMPUTER: &str = if cfg!(target_os = "macos") {
    "This Mac"
} else {
    "This computer"
};

/// A session's transcript as read: its blocks from event `start` to the end.
#[derive(Debug, Clone, PartialEq)]
pub struct Transcript {
    pub queue: Queue,
    /// When the agent read each of the person's messages, by its `User` event (`WbEvent::Read`): the bubble's "Read".
    /// A message sent while nothing ran is read too; the Nexus agent and older logs say nothing.
    pub reads: BTreeMap<i64, i64>,
    pub context_estimate: nexus_workbench::context_breakdown::Estimate,
    pub blocks: Vec<Block>,
    /// The event each block starts at, by block: where a jump to an event lands ([`locate`]).
    pub starts: Vec<i64>,
    /// The last event's number, to tell when it changed.
    pub last: i64,
    /// The first event read; None while the session has none.
    pub start: Option<i64>,
    /// Older events exist before `start`.
    pub more: bool,
    /// What the turn still running has cost so far; None when no turn runs.
    pub turn_cost: Option<f64>,
    /// Server-start signals from the same event snapshot, for the local session's Browser.
    pub signals: Vec<Signal>,
    /// A turn ended interrupted since the person's last message (`nexus_workbench::interrupt`).
    pub interrupted: bool,
    /// What its sub-agents and background commands do, as the events read show (`nexus_workbench::crew_fold`).
    pub crew: CrewCounts,
}

impl Transcript {
    /// The transcript of `events`, oldest first, that hold every event from the first one's to the session's end.
    pub fn of(events: Vec<(i64, i64, WbEvent)>) -> Transcript {
        let queue = Queue::of(&events);
        let reads = reads(&events);
        let context_estimate =
            nexus_workbench::context_breakdown::Estimate::of(events.iter().map(|(_, _, event)| event));
        let start = events.first().map(|event| event.0);
        let last = events.last().map(|event| event.0).unwrap_or(0);
        let signals = auto_preview::signals(events.iter().map(|(seq, _, event)| (*seq, event)));
        let stored: Vec<StoredEvent> = events
            .into_iter()
            .map(|(seq, at, event)| StoredEvent {
                thread_id: String::new(),
                seq,
                at,
                event,
            })
            .collect();
        // The agent only names the lead and its controls; the counts read the same for each.
        let crew = fold_crew(&stored, SessionAgent::Claude).counts;
        let interrupted = interrupted_since_message(&stored);
        let (blocks, starts, turn_cost) =
            read_blocks(stored.into_iter().map(|stored| (stored.seq, stored.at, stored.event)));
        Transcript {
            queue,
            reads,
            signals,
            context_estimate,
            blocks,
            starts,
            turn_cost,
            last,
            start,
            // Event numbers start at 1 in each session.
            more: start.is_some_and(|start| start > 1),
            interrupted,
            crew,
        }
    }
}

/// When the agent read each message: the first `Read` of its `User` event, at that event's time.
fn reads(events: &[(i64, i64, WbEvent)]) -> BTreeMap<i64, i64> {
    let mut reads = BTreeMap::new();
    for (_, at, event) in events {
        if let WbEvent::Read { seq } = event {
            reads.entry(*seq).or_insert(*at);
        }
    }
    reads
}

pub struct CodeStore {
    data: std::path::PathBuf,
    store: Store,
    scope: SessionScope,
    include_local: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodeProject {
    pub id: String,
    pub name: String,
    pub branch: Option<String>,
    pub pinned: bool,
    /// The repository's remote URLs, `origin` first: a prompt link names its project by one of them.
    pub remotes: Vec<String>,
    pub sessions: Vec<CodeSession>,
    /// The server it is on (its machine id, `machines.rs`); None on this computer.
    pub machine: Option<String>,
    /// When it was added (ms since 1970): its place in the sidebar, the newest first.
    pub added_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodeSession {
    pub id: String,
    /// A linked session's whole summary; None for a session the person started.
    pub linked: Option<std::sync::Arc<nexus_workbench::event::ThreadSummary>>,
    pub title: String,
    /// Its unique name (`nexus_workbench::session_name`), which Claude Code gets as `--name`; None: no name.
    pub name: Option<String>,
    pub status: Status,
    /// Milliseconds since 1970.
    pub updated_at: i64,
    pub created_at: i64,
    /// When the person last made it or sent it a message (`ThreadSummary::acted`): its place in its project.
    pub acted_at: i64,
    pub agent: SessionAgent,
    pub saved_provider: bool,
    /// None: the agent's default model.
    pub model: Option<String>,
    pub mode: PermissionMode,
    /// None: the model's default effort.
    pub effort: Option<Effort>,
    /// Fast mode, as saved.
    pub fast: bool,
    /// Pinned from its menu: first in its project.
    pub pinned: bool,
    /// Its worktree's branch, when it has one.
    pub branch: Option<String>,
    /// Snoozed until this time (ms); in the sidebar's "Snoozed" group until then.
    pub snoozed_until: Option<i64>,
    /// A merge of its pull request settled it: in the sidebar's "Settled" group.
    pub settled: bool,
    /// Archived: only in the sidebar's "Archived" group.
    pub archived: bool,
    pub project_id: String,
    /// The latest pull request it opened (in its worktree or its project's folder), or the one it checked out to
    /// review.
    pub pull_request: Option<String>,
    /// A review of someone else's pull request: the app opens no pull request from it.
    pub review: bool,
    /// What it cost so far, as the agent reports it, and how full its context is: the top bar's meter.
    pub cost_usd: f64,
    pub context: Option<(u64, u64)>,
    pub pending: Option<nexus_workbench::event::Pending>,
    pub via: nexus_workbench::event::Via,
    pub account: Option<String>,
    pub connection: Option<Box<nexus_workbench::event::SessionConnection>>,
    /// The server it runs on (its machine id, `machines.rs`); None on this computer.
    pub machine: Option<String>,
    /// What its sidebar mark reads: its status and its last completion (`workspace/seen.rs`).
    pub activity: nexus_workbench::attention::Activity,
    /// The person's plan stopped its last turn at a usage limit, and nothing runs: the row over the composer
    /// (`nexus_workbench::limit_stop`).
    pub limited: Option<nexus_workbench::event::LimitStop>,
    /// Its last turn was interrupted (the Continue row over the composer, `nexus_workbench::interrupt`).
    pub interrupted: bool,
    /// Its agent process runs: the summary carries its capabilities, so "Stop all" has a process to end.
    pub live: bool,
    /// The session that started it, as a linked session: the sidebar nests it under that one
    /// (`nexus_workbench::session_tree`).
    pub parent: Option<String>,
    /// The session that handed its work to this one (`handoff_session`): the sidebar says "from a Codex session".
    pub handoff_from: Option<nexus_workbench::event::HandoffFrom>,
    /// Its agent can move a running command to the background (`capabilities.background` holds "shell").
    pub background: bool,
    /// Its agent left work running past its turn (`nexus_workbench::attention::busy_in_background`).
    pub background_busy: bool,
    /// What that work is, oldest first; empty from an older host (`ThreadSummary::background`).
    pub background_work: Vec<nexus_workbench::event::BackgroundWork>,
    /// How many things that work is in all, past the list's cap too (`ThreadSummary::background_total`).
    pub background_total: usize,
    /// The linked sessions its agent waits on.
    pub waits_on: Vec<String>,
    pub source: Option<String>,
    pub has_worktree: bool,
    pub side_chat: bool,
    pub compact: bool,
    pub orchestration: bool,
    pub at_depth_limit: bool,
    pub agent_started: bool,
    /// What the crew line under the composer reads (`nexus_workbench::crew_chips`).
    pub crew: nexus_workbench::crew_chips::Facts,
    pub start_choices: Vec<nexus_workbench::session_choice::StartChoice>,
}

impl CodeSession {
    /// How many things its agent left running that the sidebar can name: its background work and the linked
    /// sessions it waits on. 0 when only an older host's flag says it works in the background.
    pub fn background_count(&self) -> usize {
        self.background_total.max(self.background_work.len()) + self.waits_on.len()
    }

    /// Ordinary streaming activity does not move a row; older hosts fall back to their activity time.
    pub fn list_activity(&self) -> i64 {
        self.activity.listed_at.unwrap_or(self.updated_at)
    }

    pub(crate) fn of(thread: nexus_workbench::event::ThreadSummary) -> CodeSession {
        CodeSession {
            saved_provider: thread.provider.is_some(),
            acted_at: thread.acted(),
            source: thread.source_thread_id.clone(),
            has_worktree: thread.worktree.is_some(),
            side_chat: thread.side_chat,
            start_choices: thread.start_choices.clone(),
            compact: thread.capabilities.as_ref().is_some_and(|caps| caps.compact),
            orchestration: thread.orchestration,
            at_depth_limit: thread.at_depth_limit,
            agent_started: thread.capabilities.is_some(),
            linked: thread
                .parent_thread_id
                .is_some()
                .then(|| std::sync::Arc::new(thread.clone())),
            machine: None,
            activity: nexus_workbench::attention::Activity::of(&thread),
            limited: nexus_workbench::limit_stop::shown(thread.limited, thread.status),
            interrupted: thread.status == ThreadStatus::Interrupted,
            live: thread.capabilities.is_some(),
            parent: thread.parent_thread_id.clone(),
            handoff_from: thread.handoff_from.clone(),
            background: thread
                .capabilities
                .as_ref()
                .is_some_and(|capabilities| capabilities.background.iter().any(|kind| kind == "shell")),
            crew: nexus_workbench::crew_chips::Facts::of(&thread),
            background_busy: nexus_workbench::attention::busy_in_background(&thread),
            background_work: thread.background.clone(),
            background_total: thread.background_total as usize,
            waits_on: thread.waits_on.clone(),
            title: if thread.title.trim().is_empty() {
                "New session".into()
            } else {
                thread.title.clone()
            },
            name: thread.name.clone(),
            id: thread.id,
            status: Status::of(thread.status),
            updated_at: thread.updated_at,
            created_at: thread.created_at,
            agent: thread.agent,
            model: thread.model.clone(),
            mode: thread.permission_mode,
            effort: thread.effort,
            fast: thread.fast,
            pinned: thread.pinned,
            branch: thread
                .worktree
                .as_ref()
                .filter(|worktree| !worktree.removed)
                .map(|worktree| worktree.branch.clone()),
            snoozed_until: thread.aside.snoozed_until,
            settled: thread.aside.settled_at.is_some(),
            archived: thread.archived,
            // A review's, else the latest it opened, in its worktree or its project's folder.
            pull_request: match &thread.worktree {
                Some(worktree) => worktree
                    .review
                    .as_ref()
                    .map(|review| review.url.clone())
                    .or_else(|| worktree.pull_request.clone()),
                None => thread.pull_requests.last().cloned(),
            },
            review: thread
                .worktree
                .as_ref()
                .is_some_and(|worktree| worktree.review.is_some()),
            project_id: thread.project_id,
            cost_usd: thread.cost_usd,
            context: thread.context_tokens.zip(thread.context_window),
            pending: thread.pending,
            account: thread.account,
            connection: thread.capabilities.and_then(|caps| caps.connection),
            via: thread.via,
        }
    }

    pub fn picker_values(&self) -> nexus_workbench::picker_values::Values {
        nexus_workbench::picker_values::Values::with_pending(
            self.model.as_deref(),
            self.effort,
            self.mode,
            self.pending.as_ref(),
        )
    }

    /// Fast mode as the composer shows it: a change the agent has not answered yet, over the saved one.
    pub fn fast_shown(&self) -> bool {
        self.pending
            .as_ref()
            .and_then(|pending| pending.fast)
            .unwrap_or(self.fast)
    }

    /// Which sidebar group it sits in at `now` (ms), out of its project; None in its project.
    pub fn aside(&self, now: i64) -> Option<Aside> {
        if self.snoozed_until.is_some_and(|until| until > now) {
            Some(Aside::Snoozed)
        } else if self.settled {
            Some(Aside::Settled)
        } else {
            None
        }
    }

    /// The group put aside the sidebar shows this session in, if any, when it is `open` on screen. An open snoozed
    /// session stays in its project while it is on screen; a settled one stays under "Settled", so
    /// opening it does not undo the settle.
    pub fn shown_aside(&self, now: i64, open: bool) -> Option<Aside> {
        match self.aside(now) {
            Some(Aside::Snoozed) if open => None,
            aside => aside,
        }
    }
}

/// The sidebar's groups under the projects, for sessions put aside (`workbench/snooze.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Aside {
    Snoozed,
    Settled,
}

impl Aside {
    pub fn label(self) -> &'static str {
        match self {
            Aside::Snoozed => "Snoozed",
            Aside::Settled => "Settled",
        }
    }
}

/// Where a session works: its worktree, or its project's folder.
pub use nexus_workbench::reader::SessionFolder;

/// The sidebar's mark for a session (`workbench/sidebar-mark.tsx`): its shape says the state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Running,
    NeedsYou,
    Failed,
    Idle,
    /// The turn ended, and work its agent started still runs (`nexus_workbench::attention::Mark::Background`).
    Background,
    /// [`Status::Background`], and a reply the person has not read.
    BackgroundUnread,
}

impl Status {
    pub fn of(status: ThreadStatus) -> Status {
        match status {
            ThreadStatus::Running | ThreadStatus::Starting => Status::Running,
            ThreadStatus::Approval => Status::NeedsYou,
            ThreadStatus::Error => Status::Failed,
            ThreadStatus::Idle | ThreadStatus::Interrupted => Status::Idle,
        }
    }
}

/// One thing the transcript shows, made from the session's events.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Setup(Box<nexus_workbench::setup_timeline::SetupRow>),
    /// What the person sent. `seq` is its event; `from`: a linked session wrote it, not the person; `rewound`: a
    /// later rewind or revert took it back (`revert.ts`'s `isReverted`), so it shows dimmed and cannot be rewound to.
    /// `at`: when it was sent (ms since 1970). `images`: how many images went with it.
    User {
        seq: i64,
        at: i64,
        text: String,
        from: Option<nexus_workbench::event::Sender>,
        rewound: bool,
        images: usize,
        image_data: Vec<nexus_workbench::event::Image>,
    },
    /// A turn's end (`timeline.ts`'s `turn` item): its line, and "Restore to here" (`revert-turn.tsx`). `seq` is the
    /// `turnEnd` event, the point a revert goes back to. `tokens`, `cost_usd` and `model` add up its `usage` events
    /// (the session's own agent names the model). `reverted`: a later revert undid it. `at`: when the turn ended (ms
    /// since 1970), the answer's time.
    Turn {
        seq: i64,
        at: i64,
        status: TurnStatus,
        duration_ms: u64,
        cost_usd: Option<f64>,
        tokens: u64,
        model: Option<String>,
        reverted: bool,
    },
    Text(String),
    /// The agent's thinking, and how long it took when the log says (from the event before it to its own).
    Thinking(String, Option<u64>),
    Tool {
        /// The tool call: a stable key when sub-agent steps move this row.
        call_id: String,
        name: String,
        summary: String,
        /// What a failed call said (its result's text, trimmed); None while it has not failed.
        error: Option<String>,
        /// A file tool's call (its event) and the file it names: what Changes follows (`follow_edits.rs`).
        edit: Option<(i64, String)>,
        /// The images its result returned (a screenshot), shown under its row (`tool-images.tsx`).
        images: Vec<crate::code_media::ToolImage>,
        /// What the call took and gave back, for its opened row (`ToolBody`) and "Move to background".
        call: Box<ToolCall>,
    },
    /// An error: a turn's (`WbEvent::TurnEnd`'s `error`) or one the session recorded (`WbEvent::Error`), such as an
    /// agent that could not start. The second field is what the agent printed (its last stderr lines), shown under it.
    Error(String, Option<String>),
    /// An event Gateway's history did not keep (over its per-event cap): its place, with a placeholder.
    Omitted,
    /// A notice the session recorded (`WbEvent::Notice`): Claude Code compacted the conversation, the agent restarted,
    /// and the like. It shows as a divider at its place, so the person sees where the conversation changed.
    Notice(String),
    /// A sub-agent: a Task or Agent call (`tool-view.tsx`'s `AgentRow`). What it did follows it as [`Block::Inside`]
    /// blocks, then its [`Block::AgentEnd`]; they show while its row is open.
    Agent(Box<AgentRow>),
    /// The end of a sub-agent's open body (`work-run.tsx`'s `AgentBody`): "Starting…" or "No steps recorded." when it
    /// did nothing yet, what it is writing, and its result once it ended. Always inside its own call.
    AgentEnd(Box<AgentEnd>),
    /// A block a sub-agent made, under the call that started it (`timeline.ts`'s `place`). `path`: the sub-agent calls
    /// above it, outermost first; it shows when each of their rows is open.
    Inside {
        path: Vec<String>,
        block: Box<Block>,
    },
    /// A Nexus page the session published (`page_links.rs`), as a card after the call or the answer that named it,
    /// once per turn: a later write of the same page in the turn updates the first card (`pagesWritten`). `turn` is the
    /// turn's `turnStart` event (0 when it is not read); `seq` the event that last wrote the page; `written`: a call
    /// wrote it (a page tool's answer, or the CLI's `View it:` line), so the side pane opens it on its own
    /// (`workspace/code_pages.rs`). A page an answer only links to shows its card and opens nothing.
    Page {
        page_id: String,
        title: Option<String>,
        turn: i64,
        seq: i64,
        written: bool,
    },
    /// The agent asked to use a tool; `answer` is None until the person (or the agent's stop) answered.
    Approval {
        request_id: String,
        tool: String,
        summary: String,
        description: Option<String>,
        /// What the tool is given: a question's questions, a plan's Markdown.
        input: Value,
        /// The rules the agent offers to allow calls like it from now on ("Always allow" sends one by its index).
        suggestions: Vec<PermissionSuggestion>,
        answer: Option<Decision>,
        /// Denied because the plan went to a new session ([`MOVED_ON`]), not because the person refused it.
        moved: bool,
        /// What the answer said: a question's receipt (`questionReceipt`).
        message: Option<String>,
    },
    /// A message the person sent while the agent worked, waiting behind its turn (`queued.ts`). `seq` is its `User`
    /// event. The dock shows it as a chip, not the transcript; these come last, in the order they wait.
    Queued {
        seq: i64,
        text: String,
    },
    /// The agent's plan (its latest TodoWrite; Codex's plan comes as one too), when it shows above the composer
    /// (`timeline.ts` `planVisible`). The dock shows it, not the transcript; it comes last.
    Plan(Vec<Todo>),
}

impl Block {
    /// The block itself, out of its sub-agent's [`Block::Inside`].
    pub fn inner(&self) -> &Block {
        match self {
            Block::Inside { block, .. } => block.inner(),
            block => block,
        }
    }

    /// The sub-agent calls above it, outermost first; empty at the top.
    pub fn path(&self) -> &[String] {
        match self {
            Block::Inside { path, .. } => path,
            _ => &[],
        }
    }
}

/// A tool call as the agent made it: its id, its input, and its result once in.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ToolCall {
    pub id: String,
    pub input: Value,
    pub output: Option<ToolOutput>,
}

/// A call's result: its text (the core caps it at 64 KiB), and whether it failed or was cut.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ToolOutput {
    pub handoff: Option<HandedOff>,
    pub text: String,
    pub error: bool,
    pub truncated: bool,
}

/// The successor named in a completed Nexus handoff tool result.
#[derive(Debug, Clone, PartialEq)]
pub struct HandedOff {
    pub session_id: String,
    pub title: String,
    pub agent: SessionAgent,
}

impl HandedOff {
    fn parse(tool: &str, text: &str, failed: bool) -> Option<Self> {
        if failed || !matches!(tool, "handoff_session" | "mcp__nexus_app__handoff_session") {
            return None;
        }
        let answer: Value = serde_json::from_str(text).ok()?;
        Some(Self {
            session_id: answer.get("session_id")?.as_str()?.to_owned(),
            title: answer.get("title")?.as_str()?.to_owned(),
            agent: SessionAgent::parse(answer.get("agent")?.as_str()?)?,
        })
    }
}

/// One step of the agent's plan.
#[derive(Debug, Clone, PartialEq)]
pub struct Todo {
    pub content: String,
    pub status: TodoStatus,
    /// What the agent says while it does the step ("Running the tests"); Codex gives none.
    pub active_form: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

/// The todos in a TodoWrite's input (`timeline.ts` `parseTodos`); anything malformed is skipped.
fn parse_todos(input: &Value) -> Vec<Todo> {
    let Some(list) = input.get("todos").and_then(Value::as_array) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|item| {
            let content = item.get("content")?.as_str()?.to_string();
            let status = match item.get("status").and_then(Value::as_str) {
                Some("completed") => TodoStatus::Completed,
                Some("in_progress") => TodoStatus::InProgress,
                _ => TodoStatus::Pending,
            };
            let active_form = item.get("activeForm").and_then(Value::as_str).map(str::to_string);
            Some(Todo {
                content,
                status,
                active_form,
            })
        })
        .collect()
}

/// The plan as the agent last wrote it, and whether it shows: something in it is not done yet, or it was finished in
/// the turn still running (`timeline.ts` `planVisible`). A rewind clears it.
#[derive(Default)]
struct PlanFold {
    todos: Vec<Todo>,
    /// The turn that wrote `todos`, counted from the log's start; None outside a turn.
    written_in: Option<usize>,
    turns: usize,
    running: bool,
}

impl PlanFold {
    fn apply(&mut self, event: &WbEvent) {
        match event {
            WbEvent::TurnStart { .. } => {
                self.turns += 1;
                self.running = true;
            }
            WbEvent::TurnEnd { .. } => self.running = false,
            WbEvent::Rewind { .. } => {
                self.todos.clear();
                self.written_in = None;
            }
            WbEvent::Tool {
                name,
                input,
                parent: None,
                ..
            } if name == "TodoWrite" => {
                self.todos = parse_todos(input);
                self.written_in = self.running.then_some(self.turns);
            }
            _ => {}
        }
    }

    fn visible(self) -> Option<Vec<Todo>> {
        let open = self.todos.iter().any(|todo| todo.status != TodoStatus::Completed);
        let this_turn = self.running && self.written_in == Some(self.turns);
        (!self.todos.is_empty() && (open || this_turn)).then_some(self.todos)
    }
}

impl CodeStore {
    /// The concrete local reader's immutable locator, independent of the visible route or current runner.
    pub(crate) fn store_source(&self) -> (&std::path::Path, &str) {
        (&self.data, self.scope.as_str())
    }
    /// The store for the sessions of `scope` (`<gateway>|<org>`), or `None` when the Nexus app has none on this computer.
    pub fn open(scope: impl Into<SessionScope>) -> Result<Option<CodeStore>, String> {
        match crate::paths::data_dir() {
            Some(dir) => Self::open_in(&dir, scope),
            None => Ok(None),
        }
    }

    /// [`CodeStore::open`] on the store in the data folder `dir`.
    pub fn open_in(dir: &std::path::Path, scope: impl Into<SessionScope>) -> Result<Option<CodeStore>, String> {
        Ok(crate::local_store::open_in(dir)?.map(|store| CodeStore {
            data: dir.to_path_buf(),
            store,
            scope: scope.into(),
            include_local: false,
        }))
    }

    /// [`CodeStore::open_in`] for reads only. Use it on the UI thread: it neither migrates nor writes, so it never waits
    /// for a running session's write lock.
    pub fn open_reading_in(dir: &std::path::Path, scope: impl Into<SessionScope>) -> Result<Option<CodeStore>, String> {
        Ok(crate::local_store::open_reading_in(dir)?.map(|store| CodeStore {
            data: dir.to_path_buf(),
            store,
            scope: scope.into(),
            include_local: false,
        }))
    }

    /// The desktop may show its own account-free ledger alongside the current organization.
    pub fn include_local(&mut self) {
        self.include_local = !self.scope.is_local();
    }

    pub fn is_local_session(&self, thread: &str) -> bool {
        self.store
            .scope_of(thread)
            .ok()
            .flatten()
            .is_some_and(|scope| SessionScope::decode(&scope).is_local())
    }

    /// Session `thread_id` as it is stored, when it is in this scope, with the live fields the workbench last pushed
    /// (`live_fields.rs`). One indexed row, so it may run on the UI thread.
    pub fn summary(&self, thread_id: &str) -> Result<Option<nexus_workbench::event::ThreadSummary>, String> {
        Ok(self
            .store
            .thread(thread_id)
            .map_err(|error| error.to_string())?
            .filter(|record| {
                record.scope == self.scope.as_str()
                    || (self.include_local && SessionScope::decode(&record.scope).is_local())
            })
            .map(|record| live(record.summary)))
    }

    /// Scoped, indexed sequence baseline for a newly admitted Files owner; no event bodies are read.
    pub fn event_sequence(&self, thread_id: &str) -> Result<Option<i64>, String> {
        if self.summary(thread_id)?.is_none() {
            return Ok(None);
        }
        self.store
            .reader()
            .latest_event_seq(thread_id)
            .map(Some)
            .map_err(|error| error.to_string())
    }

    /// Where a session works, as the Nexus app finds it (`nexus_workbench::reader::folder`).
    pub fn folder(&self, thread_id: &str) -> Result<SessionFolder, String> {
        nexus_workbench::reader::folder(&self.store, thread_id)
    }

    /// The pull request linked to a session, as its topbar shows it: a review's, else the one opened from the app.
    pub fn pull_request(&self, thread_id: &str) -> Option<String> {
        let record = self.store.thread(thread_id).ok()??;
        CodeSession::of(record.summary).pull_request
    }

    /// A project's own folder, for the Workbench on the New session screen.
    pub fn project_folder(&self, project_id: &str) -> Result<std::path::PathBuf, String> {
        self.store
            .project(project_id)
            .map_err(|error| error.to_string())?
            .map(|project| project.path.into())
            .ok_or_else(|| "This project is gone.".to_string())
    }

    #[cfg(test)]
    pub fn projects(&self) -> Result<Vec<CodeProject>, String> {
        self.reader().projects()
    }

    /// A Send reader for the store's scans: transcripts, projects and search. Use it on the background executor only;
    /// debug builds assert that (`ui_thread`). The single-row reads above may run on the UI thread.
    pub fn reader(&self) -> CodeReader {
        CodeReader {
            reader: self.store.reader(),
            scope: self.scope.clone(),
            include_local: self.include_local,
        }
    }
}

/// A stored summary with the live fields the workbench last pushed for it (`live_fields.rs`).
fn live(mut thread: nexus_workbench::event::ThreadSummary) -> nexus_workbench::event::ThreadSummary {
    crate::live_fields::apply(&mut thread);
    thread
}

pub struct CodeReader {
    scope: SessionScope,
    include_local: bool,
    reader: nexus_workbench::store::Reader,
}

impl CodeReader {
    /// Point validation against this concrete scoped reader. No replacement store or runner is resolved.
    pub(crate) fn matches_session(&self, session: &str, created: i64, project: &str, folder: &SessionFolder) -> bool {
        let Some(record) = self.reader.thread(session).ok().flatten() else {
            return false;
        };
        if record.scope != self.scope.as_str()
            || record.summary.created_at != created
            || record.summary.project_id != project
        {
            return false;
        }
        let worktree = if record.summary.side_chat {
            let Some(source) = record.summary.source_thread_id.as_deref() else {
                return false;
            };
            let Some(parent) = self
                .reader
                .thread(source)
                .ok()
                .flatten()
                .filter(|parent| parent.scope == self.scope.as_str())
            else {
                return false;
            };
            parent.summary.worktree
        } else {
            record.summary.worktree
        };
        match worktree {
            Some(worktree) => {
                !worktree.removed
                    && std::path::Path::new(&worktree.path) == folder.path
                    && folder.base.as_deref() == Some(worktree.base.as_str())
            }
            None => {
                folder.base.is_none()
                    && self
                        .reader
                        .project_path(project)
                        .ok()
                        .flatten()
                        .is_some_and(|path| std::path::Path::new(&path) == folder.path)
            }
        }
    }

    /// The side conversations belonging to one parent, read on the background executor.
    pub fn side_chats(&self, parent: &str) -> Result<Vec<nexus_workbench::event::ThreadSummary>, String> {
        Ok(self
            .store()
            .threads(self.scope.as_str())
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|record| record.summary)
            .filter(|summary| summary.side_chat && summary.source_thread_id.as_deref() == Some(parent))
            .collect())
    }

    /// The store, for a read that scans: every read here may take milliseconds, so it never runs on the UI thread.
    #[track_caller]
    fn store(&self) -> &nexus_workbench::store::Reader {
        crate::ui_thread::assert_off("A Code store scan");
        &self.reader
    }

    /// The sessions whose prompts or answers hold every word of `typed` (3+ letters each), as (session id, the event, a line
    /// around the match with the match between `\u{2}` and `\u{3}`), the latest first (`wb_search`).
    pub fn search(&self, typed: &str, limit: usize) -> Result<Vec<(String, i64, String)>, String> {
        let additional_scope = self.include_local.then_some(SessionScope::local_text());
        Ok(self
            .store()
            .search_scopes(self.scope.as_str(), additional_scope, typed, limit)?
            .into_iter()
            .map(|hit| (hit.thread_id, hit.seq, hit.snippet))
            .collect())
    }

    fn visible_threads(&self) -> Result<Vec<nexus_workbench::store::ThreadRecord>, String> {
        let mut records = self.store().threads(self.scope.as_str())?;
        if self.include_local {
            records.extend(self.store().threads(SessionScope::local_text())?);
        }
        Ok(records)
    }

    /// The sidebar's active projects and archived sessions, from one scoped thread snapshot.
    /// A project read failure still leaves the archived sessions available.
    pub fn overview(&self) -> (Result<Vec<CodeProject>, String>, Result<Vec<CodeSession>, String>) {
        let threads = match self.visible_threads() {
            Ok(threads) => threads,
            Err(error) => return (Err(error.to_string()), Err(error.to_string())),
        };
        let (archived, active): (Vec<_>, Vec<_>) = threads
            .into_iter()
            .map(|record| live(record.summary))
            .partition(|thread| thread.archived);
        let mut archived: Vec<_> = archived
            .into_iter()
            .filter(|thread| !thread.side_chat)
            .map(CodeSession::of)
            .collect();
        archived.sort_by(list_order);
        let projects = self
            .store()
            .projects()
            .map(|projects| group(projects, active, None))
            .map_err(|error| error.to_string());
        (projects, Ok(archived))
    }

    #[cfg(test)]
    pub fn projects(&self) -> Result<Vec<CodeProject>, String> {
        self.overview().0
    }

    /// The agent's slash commands, as it last reported them (its `commands` event, carried past any page): None until
    /// it has.
    pub fn commands(&self, thread_id: &str) -> Option<Vec<String>> {
        let page = self.store().events_page(thread_id, None, Some(1)).ok()?;
        page.carried
            .into_iter()
            .chain(page.events)
            .rev()
            .find_map(|stored| match stored.event {
                WbEvent::Commands { commands } => Some(commands),
                _ => None,
            })
    }

    /// The session as the store keeps it and its latest page with the events carried past it: what the Connection pane
    /// reads while the Nexus app runs the sessions (`nexus_workbench::connection_view`).
    pub fn connection_read(
        &self,
        thread_id: &str,
    ) -> Result<
        (
            Option<nexus_workbench::event::ThreadSummary>,
            Vec<nexus_workbench::event::StoredEvent>,
        ),
        String,
    > {
        let page = self
            .store()
            .events_page(thread_id, None, Some(1))
            .map_err(|error| error.to_string())?;
        let summary = self
            .reader
            .thread(thread_id)?
            .filter(|record| {
                record.scope == self.scope.as_str()
                    || (self.include_local && SessionScope::decode(&record.scope).is_local())
            })
            .map(|record| record.summary);
        Ok((summary, page.carried.into_iter().chain(page.events).collect()))
    }

    /// One bounded, metadata-only completion page from the original scoped session, on a worker.
    pub fn completed_turn_page(
        &self,
        thread_id: &str,
        after: i64,
    ) -> Result<Option<nexus_workbench::store::TurnEndPage>, String> {
        let store = self.store();
        if store
            .thread(thread_id)
            .map_err(|error| error.to_string())?
            .is_none_or(|record| record.scope != self.scope.as_str())
        {
            return Ok(None);
        }
        store
            .turn_end_page(thread_id, after)
            .map(Some)
            .map_err(|error| error.to_string())
    }

    /// Whether the session logged an event after number `seq`: an indexed read of at most the new events, so the poll
    /// can skip reading and parsing the transcript when nothing happened.
    pub fn has_events_after(&self, thread_id: &str, seq: i64) -> bool {
        // A read that fails counts as news: the full read then shows why.
        self.store()
            .events_after(thread_id, seq)
            .map_or(true, |events| !events.is_empty())
    }

    /// Agent events for the read-only Workbench, read off the UI thread.
    pub fn agent_events(
        &self,
        id: &str,
        after: i64,
    ) -> Result<
        (
            Vec<nexus_workbench::event::StoredEvent>,
            Option<nexus_workbench::event::ThreadSummary>,
        ),
        String,
    > {
        let (events, summaries) = self.agent_tree_events(id, after)?;
        Ok((events, summaries.into_iter().find(|summary| summary.id == id)))
    }

    /// The crew needs every summary from the same scoped snapshot, without reading child transcripts.
    pub fn agent_tree_events(
        &self,
        id: &str,
        after: i64,
    ) -> Result<
        (
            Vec<nexus_workbench::event::StoredEvent>,
            Vec<nexus_workbench::event::ThreadSummary>,
        ),
        String,
    > {
        let events = self.store().events_after(id, after).map_err(|e| e.to_string())?;
        let scope = self
            .store()
            .scope_of(id)?
            .filter(|scope| self.include_local && SessionScope::decode(scope).is_local());
        let summaries = self
            .store()
            .threads(scope.as_deref().unwrap_or(self.scope.as_str()))?
            .into_iter()
            .map(|record| record.summary)
            .collect();
        Ok((events, summaries))
    }

    /// The session's transcript from event `start` to its end. With no `start`, the latest page; with `older`, the
    /// page before `start` too. A fixed start keeps the blocks above the reader in place as new events arrive.
    pub fn transcript(&self, thread_id: &str, start: Option<i64>, older: bool) -> Result<Transcript, String> {
        let events = match start {
            None => self.reader.events_page(thread_id, None, Some(EVENTS))?.events,
            Some(start) => {
                let from = if older {
                    let page = self.reader.events_page(thread_id, Some(start), Some(EVENTS))?;
                    page.events.first().map_or(start, |event| event.seq)
                } else {
                    start
                };
                self.reader.events_after(thread_id, from - 1)?
            }
        };
        Ok(Transcript::of(
            events
                .into_iter()
                .map(|stored| (stored.seq, stored.at, stored.event))
                .collect(),
        ))
    }
}

/// Newest attention first, with a stable tie-breaker across store refreshes.
pub fn list_order(a: &CodeSession, b: &CodeSession) -> std::cmp::Ordering {
    b.list_activity().cmp(&a.list_activity()).then_with(|| a.id.cmp(&b.id))
}

/// A project's sessions in the sidebar: pinned first, then the one the person last made or wrote to.
/// What the agent does (a finished turn, an approval, an error) never moves a row. The id breaks a tie.
pub fn place_order(a: &CodeSession, b: &CodeSession) -> std::cmp::Ordering {
    b.pinned
        .cmp(&a.pinned)
        .then_with(|| b.acted_at.cmp(&a.acted_at))
        .then_with(|| a.id.cmp(&b.id))
}

/// The projects in the sidebar: pinned first, then the newest added. Activity never moves one. The id
/// breaks a tie, so this computer's, the servers' and the cloud's projects keep one order however they arrive.
pub fn project_order(a: &CodeProject, b: &CodeProject) -> std::cmp::Ordering {
    b.pinned
        .cmp(&a.pinned)
        .then_with(|| b.added_at.cmp(&a.added_at))
        .then_with(|| a.id.cmp(&b.id))
}

/// `projects` with their sessions out of `threads`, in [`project_order`] with their sessions in [`place_order`];
/// archived ones left out. `machine`: the server they are on (None: this computer).
pub fn group(
    projects: Vec<nexus_workbench::event::Project>,
    threads: impl IntoIterator<Item = nexus_workbench::event::ThreadSummary>,
    machine: Option<&str>,
) -> Vec<CodeProject> {
    let mut sessions: HashMap<String, Vec<CodeSession>> = HashMap::new();
    for thread in threads {
        if thread.archived || thread.side_chat {
            continue;
        }
        let mut session = CodeSession::of(thread);
        session.machine = machine.map(str::to_string);
        sessions.entry(session.project_id.clone()).or_default().push(session);
    }
    for list in sessions.values_mut() {
        list.sort_by(place_order);
    }
    let mut projects: Vec<CodeProject> = projects
        .into_iter()
        .filter(|project| !project.archived)
        .map(|project| CodeProject {
            sessions: sessions.remove(&project.id).unwrap_or_default(),
            id: project.id,
            name: project.name,
            branch: project.branch,
            pinned: project.pinned,
            remotes: project.remotes,
            machine: machine.map(str::to_string),
            added_at: project.added_at,
        })
        .collect();
    projects.sort_by(project_order);
    projects
}

/// The blocks a session's events (with their numbers) make: what the person sent, the agent's text and thinking, one
/// line per tool call (marked when its result was an error), each question to use a tool with its answer, and a
/// failed turn's error. Messages still waiting behind the running turn come last, as [`Block::Queued`]; messages taken
/// back are left out, and rewritten ones show their new text.
#[cfg(test)]
pub fn blocks(events: impl IntoIterator<Item = (i64, WbEvent)>) -> Vec<Block> {
    blocks_at(events.into_iter().map(|(seq, event)| (seq, 0, event)))
}

/// [`blocks`] from events with their times (ms since 1970): a message shows when it was sent.
pub fn blocks_at(events: impl IntoIterator<Item = (i64, i64, WbEvent)>) -> Vec<Block> {
    read_blocks(events).0
}

/// Where an event shows, using its block's first event or the closest earlier event.
pub fn locate(starts: &[i64], seq: i64) -> Option<usize> {
    starts.iter().position(|&start| start == seq).or_else(|| {
        starts
            .iter()
            .enumerate()
            .filter(|(_, start)| **start <= seq)
            .max_by_key(|(_, start)| **start)
            .map(|(index, _)| index)
    })
}

/// Nested blocks and their starting events, kept together when the tree changes reading order.
pub fn blocks_and_starts(events: impl IntoIterator<Item = (i64, i64, WbEvent)>) -> (Vec<Block>, Vec<i64>) {
    let (blocks, starts, _) = read_blocks(events);
    (blocks, starts)
}

/// Fold nested blocks, their first events, and the running turn's cost.
fn read_blocks(events: impl IntoIterator<Item = (i64, i64, WbEvent)>) -> (Vec<Block>, Vec<i64>, Option<f64>) {
    let mut running = false;
    let mut before: Option<i64> = None;
    let events: Vec<(i64, i64, WbEvent)> = events.into_iter().collect();
    // The sub-agents' status, steps and activity, as the Nexus app's agent tree reads them (`crew_fold`).
    let crew = subagents::crew(&events);
    // A sub-agent's blocks go under the call that started it (`subagent.rs`).
    let mut blocks: Nest<(i64, Block)> = Nest::default();
    let mut turns: Vec<Slot> = Vec::new();
    // What the running turn spent so far (`timeline.ts`'s turnCost, turnTokens, turnModel).
    let mut spent = (0f64, 0u64, None::<String>);
    let mut tools: HashMap<String, Slot> = HashMap::new();
    let mut setups: HashMap<String, Slot> = HashMap::new();
    let mut approvals: HashMap<String, Slot> = HashMap::new();
    let mut users: HashMap<i64, Slot> = HashMap::new();
    let mut taken_back: Vec<(i64, i64)> = Vec::new();
    let mut queue = Queue::default();
    let mut plan = PlanFold::default();
    let mut turn = 0;
    // The blocks placed when the running turn began: a turn that placed none drew nothing. And whether the person
    // wrote since the last turn ended: the workbench logs their message before the turn it starts.
    let mut turn_from = 0;
    let mut asked = false;
    // The bypass codes whose notice is already shown.
    let mut bypasses: HashSet<String> = HashSet::new();
    // The error the last turn ended with, while only notices follow it: an error event with its words joins it.
    let mut turn_error: Option<Slot> = None;
    for (seq, at, event) in events {
        let since = before
            .filter(|before| *before > 0 && at >= *before)
            .map(|before| (at - before) as u64);
        before = Some(at);
        match &event {
            WbEvent::TurnStart { .. } => running = true,
            WbEvent::TurnEnd { .. } => running = false,
            _ => {}
        }
        if !matches!(
            event,
            WbEvent::TurnEnd { .. } | WbEvent::Notice { .. } | WbEvent::Error { .. }
        ) {
            turn_error = None;
        }
        // A message sent into the turn, or handed the next one, shows where the agent read it, not where it was sent:
        // mid-answer it would read as part of what came before.
        let read_now = match &event {
            WbEvent::Read { seq: read } if queue.sending.iter().any(|message| message.seq == *read) => Some(*read),
            _ => None,
        };
        queue.apply(seq, at, &event);
        plan.apply(&event);
        if let Some(read) = read_now {
            move_to_end(&mut blocks, &mut users, read);
        }
        match event {
            WbEvent::Setup {
                id,
                command,
                status,
                output,
                exit_code,
                duration_ms,
            } => {
                let index = *setups.entry(id.clone()).or_insert_with(|| {
                    blocks.place(
                        (
                            seq,
                            Block::Setup(Box::new(nexus_workbench::setup_timeline::SetupRow::new(&id))),
                        ),
                        None,
                    )
                });
                if let Some((_, Block::Setup(row))) = blocks.get_mut(index) {
                    row.apply(&command, status, &output, exit_code, duration_ms);
                }
            }
            WbEvent::User { text, from, images } if !text.trim().is_empty() || !images.is_empty() => {
                asked = true;
                let slot = blocks.place(
                    (
                        seq,
                        Block::User {
                            seq,
                            at,
                            text,
                            from,
                            rewound: false,
                            images: images.len(),
                            image_data: images,
                        },
                    ),
                    None,
                );
                users.insert(seq, slot);
            }
            WbEvent::TurnStart { .. } => {
                spent = (0., 0, None);
                turn = seq;
                turn_from = blocks.placed();
            }
            WbEvent::Usage {
                model,
                input_tokens,
                output_tokens,
                cost_usd,
                parent,
                ..
            } => {
                spent.0 += cost_usd;
                spent.1 += input_tokens + output_tokens;
                if parent.is_none() {
                    spent.2 = Some(model);
                }
            }
            // What a rewind or a revert takes back: the messages after `after`, before the event itself.
            WbEvent::Rewind { before, .. } => taken_back.push((before - 1, seq)),
            WbEvent::Revert { turn, .. } => taken_back.push((turn, seq)),
            WbEvent::Text { text, parent, .. } if !text.trim().is_empty() => {
                // A sub-agent's text stays under its call, never in the main answer.
                let owner = blocks.owner(parent.as_deref());
                match blocks.last_mut(owner).map(|(_, block)| block) {
                    // Consecutive text from one turn (or one sub-agent) reads as one answer.
                    Some(Block::Text(previous)) => {
                        previous.push_str("\n\n");
                        previous.push_str(&text);
                    }
                    _ => {
                        blocks.place((seq, Block::Text(text)), parent.as_deref());
                    }
                }
            }
            WbEvent::Thinking { text, parent, .. } if !text.trim().is_empty() => {
                blocks.place(
                    (seq, Block::Thinking(text, since.filter(|_| at > 0))),
                    parent.as_deref(),
                );
            }
            WbEvent::Tool {
                id,
                name,
                input,
                parent,
            } => {
                let slot = if subagent::is_agent_tool(&name) {
                    let row = AgentRow::of(&id, &input, crew.node_of_call(&id));
                    blocks.place_call(&id, (seq, Block::Agent(Box::new(row))), parent.as_deref())
                } else {
                    let block = Block::Tool {
                        call_id: id.clone(),
                        summary: summarize(&input),
                        edit: edited_file(&name, &input).map(|path| (seq, path)),
                        name,
                        error: None,
                        images: Vec::new(),
                        call: Box::new(ToolCall {
                            id: id.clone(),
                            input,
                            output: None,
                        }),
                    };
                    blocks.place((seq, block), parent.as_deref())
                };
                tools.insert(id, slot);
            }
            WbEvent::ToolResult {
                id, output, is_error, ..
            } if matches!(
                tools
                    .get(&id)
                    .and_then(|slot| blocks.get_mut(*slot).map(|(_, block)| block)),
                Some(Block::Agent(_))
            ) =>
            {
                if let Some(Block::Agent(row)) = tools
                    .get(&id)
                    .and_then(|slot| blocks.get_mut(*slot).map(|(_, block)| block))
                {
                    row.ended(output, is_error);
                }
            }
            WbEvent::ToolResult {
                id,
                output,
                images,
                is_error,
                truncated,
                parent,
            } => {
                if let Some(Block::Tool {
                    images: shown,
                    call,
                    name,
                    ..
                }) = tools
                    .get(&id)
                    .and_then(|ix| blocks.get_mut(*ix).map(|(_, block)| block))
                {
                    *shown = crate::code_media::tool_images(images);
                    call.output = Some(ToolOutput {
                        handoff: HandedOff::parse(name, &output, is_error),
                        text: output.clone(),
                        error: is_error,
                        truncated,
                    });
                }
                if is_error
                    && let Some(Block::Tool { error, .. }) = tools
                        .get(&id)
                        .and_then(|ix| blocks.get_mut(*ix).map(|(_, block)| block))
                {
                    // Claude Code wraps its own refusals in a tag the person need not read.
                    let output = output.trim();
                    let output = output
                        .strip_prefix("<tool_use_error>")
                        .and_then(|inner| inner.strip_suffix("</tool_use_error>"))
                        .unwrap_or(output);
                    *error = Some(output.trim().chars().take(2000).collect());
                }
                if !is_error {
                    let tool = match tools.get(&id).and_then(|ix| blocks.get_mut(*ix)) {
                        Some((_, Block::Tool { name, .. })) => name.clone(),
                        _ => String::new(),
                    };
                    if let Some(page) = page_links::page_in_result(&tool, &output) {
                        blocks.place(
                            (
                                seq,
                                Block::Page {
                                    page_id: page.page_id,
                                    title: page.label,
                                    turn,
                                    seq,
                                    written: true,
                                },
                            ),
                            parent.as_deref(),
                        );
                    }
                }
            }
            WbEvent::TurnEnd {
                status,
                cost_usd,
                duration_ms,
                error,
                ..
            } => {
                let cost_usd = cost_usd.or((spent.0 > 0.).then_some(spent.0));
                let error = error.filter(|error| !error.trim().is_empty());
                // A turn no message of the person's started, that drew nothing and completed, has no footer: Claude
                // Code answers a sub-agent's late notice with an empty result, which the workbench logs as a turn of
                // its own (`Workbench::handle`, "A result with no turn open"). Its footer would be a second mark under
                // the answer before it. What it spent (the sub-agent's late usage) goes to that answer's footer. The
                // person's own turn always keeps its footer, with Restore and Fork, and so does a stopped or failed
                // one, which says so.
                let quiet = !std::mem::take(&mut asked)
                    && blocks.placed() == turn_from
                    && status == TurnStatus::Completed
                    && error.is_none();
                let spent_any = cost_usd.is_some_and(|cost| cost > 0.) || spent.1 > 0;
                let before = turns.last().copied();
                if quiet
                    && spent_any
                    && let Some(Block::Turn {
                        cost_usd: before_cost,
                        tokens: before_tokens,
                        ..
                    }) = before.and_then(|slot| blocks.get_mut(slot)).map(|(_, block)| block)
                {
                    if let Some(cost) = cost_usd.filter(|cost| *cost > 0.) {
                        *before_cost = Some(before_cost.unwrap_or(0.) + cost);
                    }
                    *before_tokens += spent.1;
                    spent.2 = None;
                } else if !quiet || spent_any {
                    let turn = Block::Turn {
                        seq,
                        at,
                        status,
                        duration_ms,
                        cost_usd,
                        tokens: spent.1,
                        model: spent.2.take(),
                        reverted: false,
                    };
                    turns.push(blocks.place((seq, turn), None));
                }
                turn_error = error.map(|error| blocks.place((seq, Block::Error(error, None)), None));
            }
            // An agent that failed (`Workbench::finish`: it could not start, or it stopped) says why, with what it
            // printed. A launch that failed ends its turn with the same words first (`Workbench::start`'s `fail`): the
            // error shows once, with the detail under it.
            WbEvent::Error { text, detail } if !text.trim().is_empty() => {
                let detail = detail.filter(|detail| !detail.trim().is_empty());
                match turn_error
                    .take()
                    .and_then(|slot| blocks.get_mut(slot))
                    .map(|(_, block)| block)
                {
                    Some(Block::Error(said, shown)) if said.trim() == text.trim() => {
                        if shown.is_none() {
                            *shown = detail;
                        }
                    }
                    _ => {
                        blocks.place((seq, Block::Error(text, detail)), None);
                    }
                }
            }
            WbEvent::Notice { text } if text == crate::machines::OMITTED => {
                // The missing event could have separated two distinct errors.
                turn_error = None;
                blocks.place((seq, Block::Omitted), None);
            }
            WbEvent::Approval {
                request_id,
                tool,
                input,
                description,
                suggestions,
                ..
            } => {
                let approval = Block::Approval {
                    request_id: request_id.clone(),
                    summary: summarize(&input),
                    tool,
                    description: description.filter(|text| !text.trim().is_empty()),
                    input,
                    suggestions,
                    answer: None,
                    moved: false,
                    message: None,
                };
                approvals.insert(request_id, blocks.place((seq, approval), None));
            }
            WbEvent::ApprovalResolved {
                request_id,
                decision,
                message,
            } => {
                if let Some(Block::Approval {
                    answer,
                    moved,
                    message: said,
                    ..
                }) = approvals
                    .get(&request_id)
                    .and_then(|slot| blocks.get_mut(*slot).map(|(_, block)| block))
                {
                    *answer = Some(decision);
                    *moved = message.as_deref() == Some(MOVED_ON);
                    *said = message;
                }
            }
            // A command the agent ran outside the shared terminal: the first notice of each code says why, and the
            // repeats are left out. An agent with its own shell bypasses on every command, which would bury the
            // conversation. The Terminal tab lists each command with its reason, but not under the background service.
            WbEvent::Notice { text }
                if text
                    .strip_prefix(nexus_workbench::terminal::BYPASS_NOTICE)
                    .and_then(|rest| rest.split_once(')'))
                    .is_some_and(|(code, _)| !bypasses.insert(code.to_string())) => {}
            WbEvent::Notice { text } | WbEvent::AgentSwitched { text, .. } if !text.trim().is_empty() => {
                blocks.place((seq, Block::Notice(text)), None);
            }
            WbEvent::QueueEdit { seq, text } => {
                if let Some(Block::User { text: previous, .. }) = users
                    .get(&seq)
                    .and_then(|slot| blocks.get_mut(*slot).map(|(_, block)| block))
                {
                    *previous = text;
                }
            }
            _ => {}
        }
    }
    let undone = |seq: i64| taken_back.iter().any(|&(after, at)| seq > after && seq < at);
    for (seq, slot) in &users {
        if let Some(Block::User { rewound, .. }) = blocks.get_mut(*slot).map(|(_, block)| block) {
            *rewound = undone(*seq);
        }
    }
    for slot in turns {
        if let Some(Block::Turn { seq, reverted, .. }) = blocks.get_mut(slot).map(|(_, block)| block) {
            *reverted = undone(*seq);
        }
    }
    // On its way and not read yet, a message goes last, after the work it was sent during.
    for seq in queue.sending.iter().map(|message| message.seq) {
        move_to_end(&mut blocks, &mut users, seq);
    }
    // Taken back, a message leaves the conversation; still waiting, it moves to the dock.
    let gone: HashSet<Slot> = queue
        .cancelled
        .iter()
        .chain(queue.waiting.iter().map(|(seq, _)| seq))
        .filter_map(|seq| users.get(seq).copied())
        .collect();
    for slot in gone {
        blocks.remove(slot);
    }
    blocks.extend_top(
        queue
            .waiting
            .into_iter()
            .map(|(seq, text)| (seq, Block::Queued { seq, text })),
    );
    blocks.extend_top(plan.visible().map(|plan| (i64::MAX, Block::Plan(plan))));
    let (blocks, starts) = subagents::flatten_at(blocks);
    let (blocks, starts) = place_pages(blocks, starts);
    (blocks, starts, running.then_some(spent.0))
}

/// Moves the person's message `seq` to the end of the transcript's top level, where the next block will follow it.
fn move_to_end(blocks: &mut Nest<(i64, Block)>, users: &mut HashMap<i64, Slot>, seq: i64) {
    let Some(slot) = users.get(&seq).copied() else {
        return;
    };
    let Some(entry) = blocks.get_mut(slot).map(|entry| entry.clone()) else {
        return;
    };
    blocks.remove(slot);
    users.insert(seq, blocks.place(entry, None));
}

/// A tool call in one line: the command, file, pattern or address it was given.
/// The file a file tool's input names, as the Nexus app's `fileAuthors` reads it.
fn edited_file(name: &str, input: &Value) -> Option<String> {
    if !nexus_workbench::changes_source::FILE_TOOLS.contains(&name) {
        return None;
    }
    ["file_path", "notebook_path", "path"]
        .iter()
        .find_map(|key| input.get(key).and_then(Value::as_str))
        .filter(|path| !path.is_empty())
        .map(str::to_string)
}

fn summarize(input: &Value) -> String {
    let line = [
        "command",
        "file_path",
        "path",
        "pattern",
        "url",
        "query",
        "description",
        "prompt",
    ]
    .iter()
    .find_map(|key| input.get(key).and_then(Value::as_str))
    .unwrap_or("");
    let line = line.lines().next().unwrap_or("").trim();
    let mut short: String = line.chars().take(90).collect();
    if line.chars().count() > 90 {
        short.push('…');
    }
    short
}

/// Where the agent's work in progress ends: the last block drawn, past the person's messages. A message sent into the
/// running turn lands after the call the agent is in, and does not end that call.
pub fn work_tail(blocks: &[Block]) -> Option<usize> {
    blocks
        .iter()
        .rposition(|block| !matches!(block, Block::Queued { .. } | Block::Plan(_) | Block::User { .. }))
}

/// The question the agent waits on, if any: the latest unanswered one.
pub fn pending_approval(blocks: &[Block]) -> Option<&Block> {
    blocks
        .iter()
        .rev()
        .find(|block| matches!(block, Block::Approval { answer: None, .. }))
}

/// What the planning session is told when its plan goes to a new session (`plan.ts`'s `MOVED_ON`); its plan's row
/// then reads "Moved to a new session".
pub const MOVED_ON: &str =
    "The user is implementing this plan in a new session. Do not implement it here. Stop and wait.";

/// A message's time as the Nexus app shows it under the message (Nexus UI's `relativeMessageTime`): "Just now",
/// "5 minutes ago", "1 hour ago", "3 days ago".
pub fn message_time(at_ms: i64, now_ms: i64) -> String {
    let seconds = (now_ms - at_ms).max(0) / 1000;
    if seconds < 60 {
        return "Just now".into();
    }
    let (size, unit) = [
        (31_536_000, "year"),
        (2_592_000, "month"),
        (86_400, "day"),
        (3_600, "hour"),
        (60, "minute"),
    ]
    .into_iter()
    .find(|(size, _)| seconds >= *size)
    .unwrap_or((60, "minute"));
    let count = seconds / size;
    format!("{count} {unit}{} ago", if count == 1 { "" } else { "s" })
}

/// A message's exact time, for its tooltip (`MessageTime`'s `dateStyle: "full", timeStyle: "short"`):
/// "Friday, November 6, 2026 at 3:04 PM".
pub fn message_time_exact(at_ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(at_ms)
        .map(|at| {
            at.with_timezone(&chrono::Local)
                .format("%A, %B %-d, %Y at %-I:%M %p")
                .to_string()
        })
        .unwrap_or_default()
}

/// Nexus UI's `milliseconds`: "850 ms", "2.4 s", "3 min 5 s".
pub(crate) fn duration(ms: u64) -> String {
    if ms < 1000 {
        return format!("{ms} ms");
    }
    let seconds = ms as f64 / 1000.;
    if seconds < 60. {
        format!("{seconds:.1} s")
    } else {
        format!("{} min {} s", (seconds / 60.).floor(), (seconds % 60.).round())
    }
}

/// Nexus UI's `usd`: cents when they say it all, else four places, six under a cent.
pub(crate) fn usd(value: f64) -> String {
    let abs = value.abs();
    if abs == 0. {
        "$0.00".into()
    } else if abs >= 100. || (value * 100. - (value * 100.).round()).abs() < 1e-9 {
        format!("${value:.2}")
    } else if abs >= 0.01 {
        format!("${value:.4}")
    } else {
        format!("${value:.6}")
    }
}

/// Nexus UI's `compact`: 1.2k, 34k, 1.5M.
pub(crate) fn compact(n: u64) -> String {
    let short = |value: f64, places: usize| {
        let text = format!("{value:.places$}");
        text.strip_suffix(".0").map(str::to_string).unwrap_or(text)
    };
    match n {
        0..1000 => n.to_string(),
        1000..1_000_000 => format!("{}k", short(n as f64 / 1000., if n < 10_000 { 1 } else { 0 })),
        _ => format!("{}M", short(n as f64 / 1_000_000., 1)),
    }
}

/// How long ago, as the sidebar's mono ages say it: `now`, `5m`, `3h`, `2d`, `4w`.
/// How long ago, as the Nexus app's `shortAgo` (`status.tsx`): "now", "5m", "3h", "6d", then the
/// local date ("Oct 26").
pub fn age(updated_at_ms: i64, now_ms: i64) -> String {
    let minutes = (now_ms - updated_at_ms).max(0) / 60_000;
    match minutes {
        0 => "now".into(),
        1..=59 => format!("{minutes}m"),
        60..=1439 => format!("{}h", minutes / 60),
        1440..=10079 => format!("{}d", minutes / 1440),
        _ => chrono::DateTime::from_timestamp_millis(updated_at_ms)
            .map(|at| at.with_timezone(&chrono::Local).format("%b %-d").to_string())
            .unwrap_or_default(),
    }
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// Claude Code's own "Default (recommended)" entry: the picker's "Default" row already stands for it.
pub fn is_default_entry(model: &nexus_workbench::agent_models::Model) -> bool {
    model.id == "default"
}

/// The model "Default" runs, by name ("Opus 5.5"), as the Nexus app's chip shows it: what the agent's default entry
/// resolves to, else the agent's first model. A resolved id the agent also lists under its own name keeps that name
/// (agy's "gemini-3.8-flash" is "Gemini 3.8 Flash"); [`model_label`] spells Claude's ids only.
pub fn default_model_name(models: &[nexus_workbench::agent_models::Model]) -> Option<String> {
    let resolved = models
        .iter()
        .find(|model| is_default_entry(model))
        .and_then(|model| model.resolved_id.clone());
    match resolved {
        Some(id) if model_label(&id) == id => Some(
            models
                .iter()
                .find(|model| !is_default_entry(model) && model.id == id)
                .map_or(id, |model| model.name.clone()),
        ),
        Some(id) => Some(model_label(&id)),
        None => models
            .iter()
            .find(|model| !is_default_entry(model))
            .map(|model| model.name.clone()),
    }
}

/// A model id as a name (`modelLabel`): "claude-opus-5-5" (or "claude-opus-5-5-20261101") is "Opus 5.5". The rule is
/// shared with the model keys' step (`nexus_workbench::cycle`).
pub fn model_label(id: &str) -> String {
    nexus_workbench::cycle::model_label(id)
}

/// `blocks` with a card after each answer for the pages it links to, and one card per page in a turn: a later one
/// (a new version, or a link to a page already written) updates the first.
fn place_pages(blocks: Vec<Block>, starts: Vec<i64>) -> (Vec<Block>, Vec<i64>) {
    let mut placed: Vec<Block> = Vec::with_capacity(blocks.len());
    let mut placed_starts = Vec::with_capacity(starts.len());
    let mut cards: HashMap<(Vec<String>, String), usize> = HashMap::new();
    for (block, start) in blocks.into_iter().zip(starts) {
        let (path, block) = match block {
            Block::Inside { path, block } => (path, *block),
            block => (Vec::new(), block),
        };
        match block {
            Block::User { .. } | Block::Turn { .. } => {
                cards.retain(|(owner, _), _| owner != &path);
                placed.push(in_agent(&path, block));
            }
            Block::Page { .. } => place_page(&mut placed, &mut cards, &path, block),
            Block::Text(text) => {
                let linked = page_links::page_links_in(&text);
                placed.push(in_agent(&path, Block::Text(text)));
                for page in linked {
                    let card = Block::Page {
                        page_id: page.page_id,
                        title: page.label,
                        turn: 0,
                        seq: 0,
                        written: false,
                    };
                    place_page(&mut placed, &mut cards, &path, card);
                }
            }
            block => placed.push(in_agent(&path, block)),
        }
        placed_starts.resize(placed.len(), start);
    }
    (placed, placed_starts)
}

fn in_agent(path: &[String], block: Block) -> Block {
    if path.is_empty() {
        block
    } else {
        Block::Inside {
            path: path.to_vec(),
            block: Box::new(block),
        }
    }
}

/// Add the page card `card`, or update the turn's card for the same page.
fn place_page(
    placed: &mut Vec<Block>,
    cards: &mut HashMap<(Vec<String>, String), usize>,
    path: &[String],
    card: Block,
) {
    let Block::Page {
        page_id,
        title,
        turn,
        seq,
        written,
    } = card
    else {
        return;
    };
    let key = (path.to_vec(), page_id.clone());
    let first = cards
        .get(&key)
        .and_then(|ix| placed.get_mut(*ix))
        .map(|block| match block {
            Block::Inside { block, .. } => block.as_mut(),
            block => block,
        });
    let Some(Block::Page {
        title: first_title,
        turn: first_turn,
        seq: first_seq,
        written: first_written,
        ..
    }) = first
    else {
        cards.insert(key, placed.len());
        placed.push(in_agent(
            path,
            Block::Page {
                page_id,
                title,
                turn,
                seq,
                written,
            },
        ));
        return;
    };
    // A call's title is the page's; a link's text only names a page that has none yet.
    if written {
        (*first_turn, *first_seq, *first_written) = (turn, seq, true);
        if title.is_some() {
            *first_title = title;
        }
    } else if first_title.is_none() {
        *first_title = title;
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn setup_updates_keep_one_row_before_the_queued_user_message() {
        let setup = |status: &str, output: &str, exit: Option<i32>| {
            serde_json::from_value(serde_json::json!({"kind":"setup","id":"setup_t","command":"npm ci","status":status,"output":output,"exitCode":exit,"durationMs":4200})).unwrap()
        };
        let user = serde_json::from_value(serde_json::json!({"kind":"user","text":"Fix it"})).unwrap();
        let blocks = super::blocks_at(vec![
            (1, 1, setup("running", "", None)),
            (2, 2, user),
            (3, 3, setup("running", "added packages\n", None)),
            (4, 4, setup("failed", "oops\n", Some(1))),
        ]);
        assert_eq!(blocks.len(), 2);
        let super::Block::Setup(row) = &blocks[0] else {
            panic!("setup stays first")
        };
        assert_eq!(row.output, "added packages\noops\n");
        assert_eq!(row.status, nexus_workbench::event::SetupStatus::Failed);
        assert!(matches!(blocks[1], super::Block::User { .. }));
    }

    /// The user messages and agent texts, in the order the transcript shows them.
    fn reading_order(transcript: &super::Transcript) -> Vec<String> {
        transcript
            .blocks
            .iter()
            .filter_map(|block| match block {
                super::Block::User { text, .. } => Some(format!("user:{text}")),
                super::Block::Text(text) => Some(format!("agent:{text}")),
                _ => None,
            })
            .collect()
    }

    fn event(value: serde_json::Value) -> nexus_workbench::event::WbEvent {
        serde_json::from_value(value).expect("the event fixtures follow the wire contract")
    }

    #[test]
    fn a_message_sent_into_the_turn_shows_where_the_agent_read_it() {
        use nexus_workbench::event::WbEvent;
        let mut events = vec![
            (1, 1, event(json!({"kind":"user","text":"start"}))),
            (2, 2, event(json!({"kind":"turnStart","turnId":"t"}))),
            (3, 3, event(json!({"kind":"text","id":"m","text":"working on it"}))),
            (4, 4, event(json!({"kind":"user","text":"use staging"}))),
            (5, 5, WbEvent::QueueSteer { seq: 4 }),
            (
                6,
                6,
                event(json!({"kind":"text","id":"m","text":"still on the last step"})),
            ),
        ];
        // Not read yet: last, after the work it was sent during.
        assert_eq!(
            reading_order(&super::Transcript::of(events.clone())),
            [
                "user:start",
                "agent:working on it",
                "agent:still on the last step",
                "user:use staging"
            ]
        );
        events.push((7, 7, WbEvent::Read { seq: 4 }));
        events.push((
            8,
            8,
            event(json!({"kind":"text","id":"m","text":"switching to staging"})),
        ));
        // Read: where the agent took it in, before what it did with it.
        assert_eq!(
            reading_order(&super::Transcript::of(events)),
            [
                "user:start",
                "agent:working on it",
                "agent:still on the last step",
                "user:use staging",
                "agent:switching to staging"
            ]
        );
    }

    #[test]
    fn a_queued_message_handed_the_next_turn_shows_after_the_turn_before() {
        use nexus_workbench::event::WbEvent;
        let end =
            json!({"kind":"turnEnd","turnId":"t","status":"completed","durationMs":1,"costUsd":null,"error":null});
        let transcript = super::Transcript::of(vec![
            (1, 1, event(json!({"kind":"user","text":"start"}))),
            (2, 2, event(json!({"kind":"turnStart","turnId":"t"}))),
            (3, 3, event(json!({"kind":"text","id":"m","text":"first answer"}))),
            (4, 4, event(json!({"kind":"user","text":"then the changelog"}))),
            (5, 5, event(json!({"kind":"text","id":"m","text":"first answer, more"}))),
            (6, 6, event(end)),
            (7, 7, event(json!({"kind":"turnStart","turnId":"u"}))),
            (8, 8, WbEvent::Read { seq: 4 }),
            (9, 9, event(json!({"kind":"text","id":"m","text":"changelog done"}))),
        ]);
        assert_eq!(
            reading_order(&transcript),
            [
                "user:start",
                "agent:first answer",
                "agent:first answer, more",
                "user:then the changelog",
                "agent:changelog done"
            ]
        );
    }

    #[test]
    fn image_only_queued_messages_remain_visible_after_the_agent_reads_them() {
        let user: nexus_workbench::event::WbEvent =
            serde_json::from_value(serde_json::json!({"kind":"user","text":"start"})).unwrap();
        let image: nexus_workbench::event::WbEvent = serde_json::from_value(
            serde_json::json!({"kind":"user","text":"","images":[{"mediaType":"image/png","data":"","name":null}]}),
        )
        .unwrap();
        let transcript = super::Transcript::of(vec![
            (1, 1, user),
            (2, 2, image),
            (3, 3, nexus_workbench::event::WbEvent::QueueSteer { seq: 2 }),
            (4, 4, nexus_workbench::event::WbEvent::Read { seq: 2 }),
        ]);
        assert!(
            transcript
                .blocks
                .iter()
                .any(|block| matches!(block, super::Block::User { seq: 2, images: 1, .. }))
        );
        assert_eq!(
            transcript.queue.delivery[&2],
            nexus_workbench::queued_delivery::Delivery::Read { at: 4 }
        );
    }

    #[test]
    fn every_message_the_agent_read_has_its_first_read_time() {
        use nexus_workbench::event::WbEvent;
        let transcript = super::Transcript::of(vec![
            (1, 100, event(json!({"kind":"user","text":"start"}))),
            (2, 200, event(json!({"kind":"turnStart","turnId":"t"}))),
            (3, 300, WbEvent::Read { seq: 1 }),
            (4, 400, event(json!({"kind":"user","text":"also this"}))),
            (5, 500, WbEvent::QueueSteer { seq: 4 }),
            (6, 600, event(json!({"kind":"user","text":"not read yet"}))),
            (7, 700, WbEvent::Read { seq: 4 }),
            (8, 800, WbEvent::Read { seq: 4 }),
        ]);
        // A message sent while nothing ran is read too, not only one sent into a turn; a second Read keeps the first time.
        assert_eq!(transcript.reads, BTreeMap::from([(1, 300), (4, 700)]));
        assert!(!transcript.queue.sending.iter().any(|message| message.seq == 4));
    }

    use super::*;
    use serde_json::json;

    fn ordered_thread(
        id: &str,
        project: &str,
        updated: i64,
        listed: Option<i64>,
    ) -> nexus_workbench::event::ThreadSummary {
        serde_json::from_value(json!({
            "id": id, "projectId": project, "title": id, "agent": "claude", "model": null,
            "permissionMode": "default", "via": "nexus", "status": "running", "costUsd": 0.0,
            "nexusSessions": [], "contextTokens": null, "contextWindow": null,
            "createdAt": 1, "updatedAt": updated, "listedAt": listed, "archived": false
        }))
        .unwrap()
    }

    fn ordered_project(id: &str) -> nexus_workbench::event::Project {
        serde_json::from_value(json!({ "id": id, "name": id, "path": ".", "branch": null, "addedAt": 1 })).unwrap()
    }

    #[test]
    fn page_cards_keep_their_agent_path_and_event_positions() {
        let text = || Block::Text("[Draft](https://gw.example.com/app/acme/pages/pag_1)".into());
        let nested = || in_agent(&["agent".into()], text());
        let (blocks, starts) = place_pages(vec![nested(), nested(), text()], vec![1, 2, 3]);
        assert_eq!(starts, vec![1, 1, 2, 3, 3]);
        assert!(
            matches!(&blocks[1], Block::Inside {path, block} if path == &["agent"] && matches!(block.as_ref(), Block::Page { page_id, .. } if page_id == "pag_1"))
        );
        assert!(matches!(&blocks[4], Block::Page { page_id, .. } if page_id == "pag_1"));
    }

    fn added_project(id: &str, added_at: i64) -> nexus_workbench::event::Project {
        let mut project = ordered_project(id);
        project.added_at = added_at;
        project
    }

    #[test]
    fn agent_work_never_moves_a_session_but_a_sent_message_and_a_pin_do() {
        use nexus_workbench::event::ThreadStatus;
        let mut a = ordered_thread("a", "p", 20, Some(20));
        a.acted_at = Some(20);
        let mut b = ordered_thread("b", "p", 10, Some(10));
        b.acted_at = Some(10);
        let ids = |a: &nexus_workbench::event::ThreadSummary, b: &nexus_workbench::event::ThreadSummary| {
            group(vec![ordered_project("p")], [a.clone(), b.clone()], None)[0]
                .sessions
                .iter()
                .map(|s| s.id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&a, &b), ["a", "b"]);
        // b's agent streams, finishes its turn, asks for approval, fails and is interrupted: b stays where it is.
        for status in [
            ThreadStatus::Running,
            ThreadStatus::Idle,
            ThreadStatus::Approval,
            ThreadStatus::Error,
            ThreadStatus::Interrupted,
        ] {
            b.status = status;
            b.updated_at += 100;
            b.listed_at = Some(b.updated_at);
            assert_eq!(ids(&a, &b), ["a", "b"], "{status:?} moved it");
        }
        // The person sends b a message: it goes to the top.
        b.acted_at = Some(30);
        assert_eq!(ids(&a, &b), ["b", "a"]);
        // A pin still comes first.
        a.pinned = true;
        assert_eq!(ids(&a, &b), ["a", "b"]);
    }

    #[test]
    fn project_order_ignores_activity_and_keeps_pinned_projects_first() {
        // p has the newer work and the newer message; q was added later.
        let mut a = ordered_thread("a", "p", 999, Some(999));
        a.acted_at = Some(999);
        let b = ordered_thread("b", "q", 5, Some(5));
        let projects = || vec![added_project("p", 1), added_project("q", 2)];
        let ids = |p| {
            group(p, [a.clone(), b.clone()], None)
                .iter()
                .map(|p| p.id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(projects()), ["q", "p"]);
        let mut pinned = projects();
        pinned[0].pinned = true;
        assert_eq!(ids(pinned), ["p", "q"]);
    }

    #[test]
    fn an_open_settled_session_stays_settled_and_an_open_snoozed_one_comes_back() {
        let mut session = CodeSession::of(ordered_thread("a", "p", 10, None));
        assert_eq!(session.shown_aside(100, true), None, "an active session");
        session.settled = true;
        assert_eq!(session.shown_aside(100, false), Some(Aside::Settled));
        assert_eq!(
            session.shown_aside(100, true),
            Some(Aside::Settled),
            "open, still settled"
        );
        session.snoozed_until = Some(200);
        assert_eq!(session.shown_aside(100, false), Some(Aside::Snoozed));
        assert_eq!(
            session.shown_aside(100, true),
            None,
            "open, a snoozed one is in its project"
        );
    }

    #[test]
    fn older_hosts_fall_back_to_creation_and_equal_times_have_stable_ids() {
        // No actedAt on the wire: the session was made at 1, whatever its agent did since.
        let a = CodeSession::of(ordered_thread("a", "p", 10, None));
        let b = CodeSession::of(ordered_thread("b", "p", 20, None));
        assert_eq!((a.acted_at, b.acted_at), (1, 1));
        assert_eq!(place_order(&a, &b), std::cmp::Ordering::Less);
        // Recents and the archive still list the latest attention first.
        assert_eq!(a.list_activity(), 10);
        assert_eq!(list_order(&a, &b), std::cmp::Ordering::Greater);
        let mut tied = b.clone();
        tied.activity.listed_at = Some(10);
        assert_eq!(list_order(&a, &tied), std::cmp::Ordering::Less);
        let q = group(vec![ordered_project("q"), ordered_project("p")], [], None);
        assert_eq!(q.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["p", "q"]);
    }

    #[test]
    fn a_folder_session_shows_the_latest_pull_request_its_agent_opened() {
        let mut thread = ordered_thread("a", "p", 10, None);
        assert!(thread.worktree.is_none());
        assert_eq!(CodeSession::of(thread.clone()).pull_request, None);
        thread.pull_requests = vec![
            "https://github.com/o/r/pull/1".into(),
            "https://github.com/o/r/pull/2".into(),
        ];
        assert_eq!(
            CodeSession::of(thread).pull_request.as_deref(),
            Some("https://github.com/o/r/pull/2")
        );
    }

    #[test]
    fn a_transcript_keeps_preview_signals_and_main_event_positions_from_one_snapshot() {
        let read = Transcript::of(vec![
            (
                1,
                0,
                WbEvent::User {
                    text: "Start the server".into(),
                    images: Vec::new(),
                    from: None,
                },
            ),
            (2, 0, WbEvent::TurnStart { turn_id: "t".into() }),
            (
                3,
                0,
                WbEvent::Tool {
                    id: "server".into(),
                    name: "Bash".into(),
                    input: json!({"command": "npm run dev"}),
                    parent: None,
                },
            ),
            (
                4,
                0,
                WbEvent::ToolResult {
                    id: "server".into(),
                    output: "Local: http://localhost:5173".into(),
                    images: Vec::new(),
                    is_error: false,
                    truncated: false,
                    parent: None,
                },
            ),
            (
                5,
                0,
                WbEvent::TurnEnd {
                    turn_id: "t".into(),
                    status: TurnStatus::Interrupted,
                    cost_usd: None,
                    duration_ms: 1,
                    error: None,
                    cursor: None,
                },
            ),
        ]);
        assert_eq!(
            read.signals,
            [
                Signal::Turn(2),
                Signal::Server {
                    seq: 4,
                    url: "http://localhost:5173/".into()
                },
            ]
        );
        assert_eq!(read.starts, [1, 3, 5]);
        assert_eq!(read.blocks.len(), read.starts.len());
        assert!(read.interrupted);
    }

    #[test]
    fn a_jump_lands_on_the_block_that_holds_its_event() {
        let user = |text: &str| WbEvent::User {
            text: text.into(),
            images: Vec::new(),
            from: None,
        };
        let text = |text: &str| WbEvent::Text {
            id: "m".into(),
            text: text.into(),
            parent: None,
        };
        let end = || -> WbEvent {
            serde_json::from_value(json!({
                "kind": "turnEnd", "turnId": "t", "status": "completed", "costUsd": null, "durationMs": 1, "error": null
            }))
            .unwrap()
        };
        let events = vec![
            (3, 0, user("deploy the gateway")),
            (4, 0, WbEvent::TurnStart { turn_id: "t".into() }),
            (5, 0, text("Deploying.")),
            (6, 0, text("Deployed.")),
            (7, 0, end()),
        ];
        let (blocks, starts) = blocks_and_starts(events);
        assert_eq!(blocks.len(), starts.len());
        assert_eq!(starts, [3, 5, 7]);
        // The message, an answer's first text, and its later text merged into the same block.
        assert_eq!(locate(&starts, 3), Some(0));
        assert_eq!(locate(&starts, 5), Some(1));
        assert_eq!(locate(&starts, 6), Some(1));
        // An event that draws nothing lands on the block before it; none before the first.
        assert_eq!(locate(&starts, 4), Some(0));
        assert_eq!(locate(&starts, 2), None);
    }

    #[test]
    fn empty_turn_after_an_answer_draws_no_second_footer() {
        let end = |status, cost: f64, duration_ms| WbEvent::TurnEnd {
            turn_id: "t".into(),
            status,
            cost_usd: Some(cost),
            duration_ms,
            error: None,
            cursor: None,
        };
        let start = || WbEvent::TurnStart { turn_id: "t".into() };
        // Each footer's cost.
        let footers = |events: Vec<WbEvent>| {
            let read = Transcript::of(
                events
                    .into_iter()
                    .enumerate()
                    .map(|(i, event)| (i as i64 + 1, 0, event))
                    .collect(),
            );
            read.blocks
                .iter()
                .filter_map(|block| match block {
                    Block::Turn { cost_usd, .. } => Some(cost_usd.unwrap_or(0.)),
                    _ => None,
                })
                .collect::<Vec<f64>>()
        };
        let answer = || WbEvent::Text {
            id: "m".into(),
            text: "Here's the plan.".into(),
            parent: None,
        };
        // The answer, then the empty result Claude Code sends for a sub-agent's late notice: one footer. When the
        // notice carries the sub-agent's late cost, the answer's footer takes it.
        let answer_then = |late: f64| {
            footers(vec![
                start(),
                answer(),
                end(TurnStatus::Completed, 3.5, 11_351),
                start(),
                end(TurnStatus::Completed, late, 0),
            ])
        };
        assert_eq!(answer_then(0.), vec![3.5]);
        assert_eq!(answer_then(0.25), vec![3.75]);
        // An empty turn with no footer before it keeps its own when it cost something; one that did not complete
        // always does: the footer says so.
        assert_eq!(footers(vec![start(), end(TurnStatus::Completed, 0.2, 900)]), vec![0.2]);
        assert_eq!(footers(vec![start(), end(TurnStatus::Interrupted, 0., 0)]), vec![0.]);
        // A person's message that drew no answer and cost nothing keeps its footer.
        let ask = WbEvent::User {
            text: "go".into(),
            images: Vec::new(),
            from: None,
        };
        assert_eq!(footers(vec![ask, start(), end(TurnStatus::Completed, 0., 0)]), vec![0.]);
    }

    #[test]
    fn a_transcript_knows_an_interrupt_since_the_message_and_what_still_runs() {
        let end = |status| WbEvent::TurnEnd {
            turn_id: "t".into(),
            status,
            cost_usd: None,
            duration_ms: 1,
            error: None,
            cursor: None,
        };
        let user = || WbEvent::User {
            text: "go".into(),
            images: Vec::new(),
            from: None,
        };
        let task = WbEvent::Task {
            task_id: "a1".into(),
            tool_use_id: None,
            task_type: Some("local_agent".into()),
            agent_type: Some("Explore".into()),
            status: nexus_workbench::event::TaskStatus::Running,
            description: "Look around".into(),
            tool_count: None,
            total_tokens: None,
            duration_ms: None,
            detail: None,
        };
        let read = |events: Vec<WbEvent>| {
            Transcript::of(
                events
                    .into_iter()
                    .enumerate()
                    .map(|(i, event)| (i as i64 + 1, 0, event))
                    .collect(),
            )
        };
        let stopped = read(vec![
            user(),
            WbEvent::TurnStart { turn_id: "t".into() },
            task.clone(),
            end(TurnStatus::Interrupted),
        ]);
        assert!(stopped.interrupted);
        assert_eq!((stopped.crew.agents_running, stopped.crew.shells_running), (1, 0));
        let moved_on = read(vec![user(), end(TurnStatus::Interrupted), user()]);
        assert!(!moved_on.interrupted);
        assert!(!read(vec![user(), end(TurnStatus::Completed)]).interrupted);
    }

    #[test]
    fn the_plan_shows_while_open_or_finished_this_turn() {
        let start = || WbEvent::TurnStart { turn_id: "t".into() };
        let end = || -> WbEvent {
            serde_json::from_value(json!({
                "kind": "turnEnd", "turnId": "t", "status": "completed", "costUsd": null, "durationMs": 1, "error": null
            }))
            .unwrap()
        };
        let todo = |statuses: &[&str]| WbEvent::Tool {
            id: "w".into(),
            name: "TodoWrite".into(),
            input: json!({ "todos": statuses.iter().enumerate().map(|(ix, status)| json!({
                "content": format!("step {ix}"), "status": status, "activeForm": format!("doing {ix}")
            })).collect::<Vec<_>>() }),
            parent: None,
        };
        let plan = |events: Vec<WbEvent>| {
            blocks(events.into_iter().enumerate().map(|(ix, event)| (ix as i64, event)))
                .into_iter()
                .find_map(|block| match block {
                    Block::Plan(todos) => Some(todos),
                    _ => None,
                })
        };
        let open = plan(vec![start(), todo(&["completed", "in_progress", "pending"])]).unwrap();
        assert_eq!(open.len(), 3);
        assert_eq!(open[1].status, TodoStatus::InProgress);
        assert_eq!(open[1].active_form.as_deref(), Some("doing 1"));
        // Finished in the running turn: still shown; once that turn ends, hidden.
        assert!(plan(vec![start(), todo(&["completed"])]).is_some());
        assert!(plan(vec![start(), todo(&["completed"]), end()]).is_none());
        assert!(plan(vec![start(), todo(&["completed"]), end(), start()]).is_none());
        // Unfinished, it stays through later turns.
        assert!(plan(vec![start(), todo(&["pending"]), end(), start()]).is_some());
        assert!(plan(vec![start()]).is_none());
    }

    #[test]
    fn a_tools_images_stay_on_its_row_whether_it_failed_or_not() {
        const PIXEL: &str =
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
        let call = |id: &str| WbEvent::Tool {
            id: id.into(),
            name: "mcp__playwright__browser_take_screenshot".into(),
            input: json!({}),
            parent: None,
        };
        let result = |id: &str, is_error: bool| WbEvent::ToolResult {
            id: id.into(),
            output: "shot".into(),
            images: vec![nexus_workbench::event::Image {
                media_type: "image/png".into(),
                data: PIXEL.into(),
                name: None,
            }],
            is_error,
            truncated: false,
            parent: None,
        };
        let shown = blocks(vec![
            (1, call("a")),
            (2, result("a", false)),
            (3, call("b")),
            (4, result("b", true)),
        ]);
        let images: Vec<_> = shown
            .iter()
            .map(|block| match block {
                Block::Tool { images, error, .. } => (images.len(), error.is_some()),
                _ => panic!("only tool rows"),
            })
            .collect();
        assert_eq!(images, [(1, false), (1, true)]);
        // The call keeps its id, input and result for its opened row.
        let Block::Tool { call, .. } = &shown[1] else {
            panic!("a tool row")
        };
        assert_eq!(call.id, "b");
        assert_eq!(
            call.output.as_ref().map(|output| (output.text.as_str(), output.error)),
            Some(("shot", true))
        );
    }

    #[test]
    fn a_thought_says_how_long_it_took_and_a_running_turn_what_it_cost() {
        let start = WbEvent::TurnStart { turn_id: "t".into() };
        let thought = WbEvent::Thinking {
            id: "h".into(),
            text: "Look in src first.".into(),
            parent: None,
        };
        let usage: WbEvent = serde_json::from_value(json!({
            "kind": "usage", "model": "m", "inputTokens": 10, "outputTokens": 5, "cacheReadTokens": 0,
            "cacheWriteTokens": 0, "costUsd": 0.84
        }))
        .unwrap();
        let end: WbEvent = serde_json::from_value(json!({
            "kind": "turnEnd", "turnId": "t", "status": "completed", "costUsd": null, "durationMs": 1, "error": null
        }))
        .unwrap();
        let mut events = vec![(1, 1_000, start), (2, 15_200, thought), (3, 15_300, usage)];
        let running = Transcript::of(events.clone());
        assert_eq!(
            running.blocks[0],
            Block::Thinking("Look in src first.".into(), Some(14_200))
        );
        assert_eq!(running.turn_cost, Some(0.84));
        events.push((4, 16_000, end));
        let ended = Transcript::of(events);
        assert_eq!(ended.turn_cost, None);
    }

    #[test]
    fn ages_read_short() {
        let now = 10_000_000_000;
        assert_eq!(age(now, now), "now");
        assert_eq!(age(now - 5 * 60_000, now), "5m");
        assert_eq!(age(now - 3 * 3_600_000, now), "3h");
        assert_eq!(age(now - 2 * 86_400_000, now), "2d");
        // Older than a week: the local date, whose day depends on the time zone.
        assert!(age(now - 30 * 86_400_000, now).contains(char::is_alphabetic));
    }

    fn tool(id: &str, name: &str) -> WbEvent {
        WbEvent::Tool {
            id: id.into(),
            name: name.into(),
            input: json!({}),
            parent: None,
        }
    }

    fn result(id: &str, output: &str, is_error: bool) -> WbEvent {
        WbEvent::ToolResult {
            id: id.into(),
            output: output.into(),
            images: Vec::new(),
            is_error,
            truncated: false,
            parent: None,
        }
    }

    fn answer(text: &str) -> WbEvent {
        WbEvent::Text {
            id: "a".into(),
            text: text.into(),
            parent: None,
        }
    }

    fn wrote(id: &str, title: &str) -> String {
        json!({ "page": { "id": id, "title": title }, "version": { "version": 1 } }).to_string()
    }

    fn card(page_id: &str, title: Option<&str>, turn: i64, seq: i64, written: bool) -> Block {
        Block::Page {
            page_id: page_id.into(),
            title: title.map(str::to_string),
            turn,
            seq,
            written,
        }
    }

    fn cards(blocks: &[Block]) -> Vec<Block> {
        blocks
            .iter()
            .filter(|block| matches!(block, Block::Page { .. }))
            .cloned()
            .collect()
    }

    #[test]
    fn inserted_and_deduplicated_page_cards_keep_later_event_starts_aligned() {
        let user = |text: &str| WbEvent::User {
            text: text.into(),
            images: Vec::new(),
            from: None,
        };
        let events = vec![
            (1, 0, user("Make a page")),
            (2, 0, WbEvent::TurnStart { turn_id: "t".into() }),
            (3, 0, answer("[Draft](https://gw.example.com/app/acme/pages/pag_1)")),
            (4, 0, tool("p", "mcp__nexus__createPage")),
            (5, 0, result("p", &wrote("pag_1", "Published"), false)),
            (
                6,
                0,
                WbEvent::TurnEnd {
                    turn_id: "t".into(),
                    status: TurnStatus::Completed,
                    cost_usd: None,
                    duration_ms: 1,
                    error: None,
                    cursor: None,
                },
            ),
            (7, 0, user("Next task")),
        ];
        let (blocks, starts) = blocks_and_starts(events);
        assert_eq!(blocks.len(), starts.len());
        assert_eq!(starts, [1, 3, 3, 4, 6, 7]);
        assert_eq!(cards(&blocks), [card("pag_1", Some("Published"), 2, 5, true)]);
        assert!(matches!(
            blocks[locate(&starts, 6).unwrap()],
            Block::Turn { seq: 6, .. }
        ));
        assert!(matches!(
            blocks[locate(&starts, 7).unwrap()],
            Block::User { seq: 7, .. }
        ));
    }

    #[test]
    fn a_page_a_call_wrote_shows_as_a_card_after_it() {
        let start = WbEvent::TurnStart { turn_id: "t".into() };
        let blocks = blocks(vec![
            (1, start),
            (2, tool("c", "mcp__nexus__createPage")),
            (3, result("c", &wrote("pag_1", "Q3 spend"), false)),
            (4, tool("b", "Bash")),
            (
                5,
                result(
                    "b",
                    "Published.\nView it: https://gw.example.com/app/acme/pages/pag_2\n",
                    false,
                ),
            ),
            // A failed call and another tool's answer write nothing.
            (6, tool("x", "mcp__nexus__createPage")),
            (7, result("x", &wrote("pag_3", "x"), true)),
            (8, tool("y", "mcp__nexus__listTeams")),
            (9, result("y", &wrote("pag_4", "x"), false)),
        ]);
        assert!(matches!(&blocks[0], Block::Tool { name, .. } if name == "mcp__nexus__createPage"));
        assert_eq!(blocks[1], card("pag_1", Some("Q3 spend"), 1, 3, true));
        assert_eq!(blocks[3], card("pag_2", None, 1, 5, true));
        assert_eq!(cards(&blocks).len(), 2);
    }

    #[test]
    fn a_later_version_in_the_turn_updates_the_first_card() {
        let start = || WbEvent::TurnStart { turn_id: "t".into() };
        let end = || -> WbEvent {
            serde_json::from_value(json!({
                "kind": "turnEnd", "turnId": "t", "status": "completed", "costUsd": null, "durationMs": 1, "error": null
            }))
            .unwrap()
        };
        let blocks = blocks(vec![
            (1, start()),
            (2, tool("a", "mcp__nexus__createPage")),
            (3, result("a", &wrote("pag_1", "Draft"), false)),
            (4, tool("b", "mcp__nexus__writePageVersion")),
            (5, result("b", &wrote("pag_1", "Final"), false)),
            (6, end()),
            // The next turn shows its own card.
            (7, start()),
            (8, tool("c", "mcp__nexus__writePageVersion")),
            (9, result("c", &wrote("pag_1", "Final, v3"), false)),
        ]);
        assert_eq!(
            cards(&blocks),
            vec![
                card("pag_1", Some("Final"), 1, 5, true),
                card("pag_1", Some("Final, v3"), 7, 9, true)
            ]
        );
        assert_eq!(blocks[1], card("pag_1", Some("Final"), 1, 5, true));
    }

    /// A compaction, automatic or `/compact`, shows as a divider where it happened, as any notice does. An
    /// empty notice shows nothing.
    #[test]
    fn a_notice_shows_at_its_place_in_the_conversation() {
        let user = |text: &str| WbEvent::User {
            text: text.into(),
            images: Vec::new(),
            from: None,
        };
        let start = || WbEvent::TurnStart { turn_id: "t".into() };
        let end = || WbEvent::TurnEnd {
            turn_id: "t".into(),
            status: TurnStatus::Completed,
            cost_usd: None,
            duration_ms: 1,
            error: None,
            cursor: None,
        };
        let notice = |text: &str| WbEvent::Notice { text: text.into() };
        let compacted = "Claude Code compacted the conversation to fit its context window.";
        let bypass = format!(
            "{}native_execution_unintercepted): The provider bypassed shared terminal routing; the cause is unknown.",
            nexus_workbench::terminal::BYPASS_NOTICE
        );
        let blocks = blocks(vec![
            (1, user("Refactor the parser")),
            (2, start()),
            (3, answer("Done.")),
            (4, end()),
            (5, notice(compacted)),
            (6, notice("  ")),
            // The first bypass of a code shows; its repeats do not.
            (7, notice(&bypass)),
            (8, notice(&bypass)),
            (9, user("Now add tests")),
            (10, start()),
        ]);
        let at = |found: &dyn Fn(&Block) -> bool| blocks.iter().position(|block| found(block));
        let answered = at(&|block| matches!(block, Block::Text(text) if text == "Done.")).expect("the answer");
        let divider = at(&|block| block == &Block::Notice(compacted.into())).expect("the notice");
        let next = at(&|block| matches!(block, Block::User { text, .. } if text == "Now add tests"))
            .expect("the next message");
        assert!(answered < divider && divider < next, "{blocks:#?}");
        assert_eq!(
            blocks.iter().filter(|block| matches!(block, Block::Notice(_))).count(),
            2,
            "the compaction and one bypass: {blocks:#?}"
        );
    }

    /// An agent that exits ends its turn as failed, with no words, then says why (`Workbench::finish`): the reason and
    /// what the agent printed show after the failed turn.
    #[test]
    fn failed_agent_shows_its_reason_and_what_it_printed() {
        let reason = "Antigravity could not start (exit 1).";
        let printed = "bwrap: Can't chdir to /home/ada/src/app: No such file or directory";
        let blocks = blocks(vec![
            (
                1,
                WbEvent::User {
                    text: "Fix the build".into(),
                    images: Vec::new(),
                    from: None,
                },
            ),
            (2, WbEvent::TurnStart { turn_id: "t".into() }),
            (
                3,
                WbEvent::TurnEnd {
                    turn_id: "t".into(),
                    status: TurnStatus::Error,
                    cost_usd: None,
                    duration_ms: 0,
                    error: None,
                    cursor: None,
                },
            ),
            (
                4,
                WbEvent::Error {
                    text: reason.into(),
                    detail: Some(printed.into()),
                },
            ),
        ]);
        let failed = blocks
            .iter()
            .position(|block| {
                matches!(
                    block,
                    Block::Turn {
                        status: TurnStatus::Error,
                        ..
                    }
                )
            })
            .expect("the failed turn");
        let error = blocks
            .iter()
            .position(|block| block == &Block::Error(reason.into(), Some(printed.into())))
            .expect("the reason, with what the agent printed");
        assert!(failed < error, "{blocks:#?}");
    }

    /// An error with no detail (a refused sign-in, say) shows its text alone. A blank detail counts as none.
    #[test]
    fn error_without_detail_shows_its_text() {
        let error = |detail: Option<&str>| WbEvent::Error {
            text: "Antigravity is not signed in. Run `agy` to sign in.".into(),
            detail: detail.map(Into::into),
        };
        for detail in [None, Some(" \n ")] {
            assert_eq!(
                blocks(vec![(1, error(detail))]),
                vec![Block::Error(
                    "Antigravity is not signed in. Run `agy` to sign in.".into(),
                    None
                )]
            );
        }
    }

    /// A launch that fails ends its turn with the reason, then records the same reason with what the agent printed
    /// (`Workbench::start`'s `fail`): it shows once, with the detail. The same words show again in a later turn, or
    /// after a message of the person's: only notices may come between a turn's error and the event that joins it.
    #[test]
    fn turn_error_and_error_event_with_the_same_words_show_once() {
        let reason = "Codex could not start: the folder is gone.";
        let start = || WbEvent::TurnStart { turn_id: "t".into() };
        let end = || WbEvent::TurnEnd {
            turn_id: "t".into(),
            status: TurnStatus::Error,
            cost_usd: None,
            duration_ms: 0,
            error: Some(reason.into()),
            cursor: None,
        };
        let error = || WbEvent::Error {
            text: reason.into(),
            detail: Some("exit 1".into()),
        };
        let blocks = blocks(vec![
            (1, start()),
            (2, end()),
            (
                3,
                WbEvent::Notice {
                    text: "The agent could not start, so it did not read your last message.".into(),
                },
            ),
            (4, error()),
            (5, start()),
            (6, error()),
            (7, end()),
            (
                8,
                WbEvent::User {
                    text: "Try again".into(),
                    images: Vec::new(),
                    from: None,
                },
            ),
            (9, error()),
            (10, start()),
            (11, end()),
            (
                12,
                WbEvent::Notice {
                    text: crate::machines::OMITTED.into(),
                },
            ),
            (13, error()),
        ]);
        let errors: Vec<&Block> = blocks
            .iter()
            .filter(|block| matches!(block, Block::Error(..)))
            .collect();
        let shown = Block::Error(reason.into(), Some("exit 1".into()));
        let alone = Block::Error(reason.into(), None);
        assert_eq!(
            errors,
            vec![&shown, &shown, &alone, &shown, &alone, &shown],
            "{blocks:#?}"
        );
        assert_eq!(blocks[blocks.len() - 2..], [Block::Omitted, shown]);
    }

    #[test]
    fn an_answer_shows_a_card_for_each_page_it_links_to() {
        let start = WbEvent::TurnStart { turn_id: "t".into() };
        let blocks = blocks(vec![
            (1, start),
            (2, tool("a", "mcp__nexus__createPage")),
            (3, result("a", &wrote("pag_1", "Q3 spend"), false)),
            (
                4,
                answer("Done: [the chart](https://gw.example.com/app/acme/pages/pag_1)."),
            ),
            (
                5,
                answer("Also https://gw.example.com/app/acme/pages/pag_2 and https://example.com/"),
            ),
        ]);
        // Consecutive text still reads as one answer; the page the call wrote keeps its one card and its title.
        assert!(matches!(&blocks[2], Block::Text(text) if text.contains("Also")));
        assert_eq!(blocks[3], card("pag_2", None, 0, 0, false));
        assert_eq!(
            cards(&blocks),
            vec![
                card("pag_1", Some("Q3 spend"), 1, 3, true),
                card("pag_2", None, 0, 0, false)
            ]
        );
    }

    #[test]
    fn messages_sent_during_a_turn_wait_in_the_dock() {
        let user = |text: &str| WbEvent::User {
            text: text.into(),
            images: Vec::new(),
            from: None,
        };
        let start = || WbEvent::TurnStart { turn_id: "t".into() };
        let end: WbEvent = serde_json::from_value(json!({
            "kind": "turnEnd", "turnId": "t", "status": "completed", "costUsd": null, "durationMs": 1, "error": null
        }))
        .unwrap();
        let events = vec![
            (1, user("first")),
            (2, start()),
            (3, user("second")),
            (4, user("third")),
            (5, user("fourth")),
            (6, WbEvent::QueueCancel { seq: 4 }),
            (
                7,
                WbEvent::QueueEdit {
                    seq: 5,
                    text: "fourth, rewritten".into(),
                },
            ),
            (8, end),
            (9, start()),
        ];
        assert_eq!(
            blocks(events),
            // "second" took the next turn: it shows after the turn it waited for, not in the middle of it.
            vec![
                said(1, "first"),
                Block::Turn {
                    seq: 8,
                    at: 0,
                    status: TurnStatus::Completed,
                    duration_ms: 1,
                    cost_usd: None,
                    tokens: 0,
                    model: None,
                    reverted: false,
                },
                said(3, "second"),
                Block::Queued {
                    seq: 5,
                    text: "fourth, rewritten".into()
                },
            ]
        );
    }

    fn said(seq: i64, text: &str) -> Block {
        Block::User {
            seq,
            at: 0,
            text: text.into(),
            from: None,
            rewound: false,
            images: 0,
            image_data: Vec::new(),
        }
    }

    #[test]
    fn image_only_user_events_keep_their_source_images() {
        let image = nexus_workbench::event::Image {
            media_type: "image/png".into(),
            data: "aGVsbG8=".into(),
            name: Some("shot.png".into()),
        };
        let blocks = blocks(vec![(
            1,
            WbEvent::User {
                text: String::new(),
                from: None,
                images: vec![image.clone()],
            },
        )]);
        assert!(
            matches!(&blocks[..], [Block::User { text, images: 1, image_data, .. }] if text.is_empty() && image_data == &vec![image])
        );
    }

    #[test]
    fn a_linked_sessions_message_keeps_its_sender() {
        let from = nexus_workbench::event::Sender::Session {
            thread_id: "t_child".into(),
            title: "Builder".into(),
        };
        let blocks = blocks([(
            3,
            WbEvent::User {
                text: "The work is ready.".into(),
                images: Vec::new(),
                from: Some(from.clone()),
            },
        )]);
        assert!(matches!(&blocks[..], [Block::User { from: Some(sender), .. }] if *sender == from));
    }

    #[test]
    fn a_rewind_marks_the_messages_it_took_back() {
        let user = |text: &str| WbEvent::User {
            text: text.into(),
            images: Vec::new(),
            from: None,
        };
        let events = vec![
            (1, user("first")),
            (2, user("second")),
            (3, user("third")),
            (
                4,
                WbEvent::Rewind {
                    before: 2,
                    cursor: None,
                },
            ),
            (5, user("again")),
        ];
        let rewound: Vec<(i64, bool)> = blocks(events)
            .into_iter()
            .filter_map(|block| match block {
                Block::User { seq, rewound, .. } => Some((seq, rewound)),
                _ => None,
            })
            .collect();
        assert_eq!(rewound, vec![(1, false), (2, true), (3, true), (5, false)]);
    }

    #[test]
    fn turns_end_with_their_line_and_a_revert_marks_the_ones_it_undid() {
        let user = |text: &str| WbEvent::User {
            text: text.into(),
            images: Vec::new(),
            from: None,
        };
        let start = || WbEvent::TurnStart { turn_id: "t".into() };
        let usage = |model: &str, cost: f64, parent: Option<&str>| WbEvent::Usage {
            model: model.into(),
            input_tokens: 30_000,
            output_tokens: 11_000,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_usd: cost,
            parent: parent.map(str::to_string),
        };
        let end = |status: &str| -> WbEvent {
            serde_json::from_value(json!({
                "kind": "turnEnd", "turnId": "t", "status": status, "costUsd": null, "durationMs": 2400, "error": null
            }))
            .unwrap()
        };
        let events = vec![
            (1, 1_000, user("first")),
            (2, 1_001, start()),
            (3, 1_002, usage("anthropic/claude-opus-5-5", 0.18, None)),
            (4, 1_003, usage("claude-haiku-5-5", 0.0, Some("task"))),
            (5, 1_004, end("completed")),
            (6, 2_000, user("second")),
            (7, 2_001, start()),
            (8, 2_002, end("interrupted")),
            (9, 3_000, WbEvent::Revert { turn: 5, files: 2 }),
        ];
        let blocks = blocks_at(events);
        type TurnFacts = (i64, TurnStatus, u64, Option<f64>, u64, Option<String>, bool);
        let turns: Vec<TurnFacts> = blocks
            .iter()
            .filter_map(|block| match block {
                Block::Turn {
                    seq,
                    at: _,
                    status,
                    duration_ms,
                    cost_usd,
                    tokens,
                    model,
                    reverted,
                } => Some((
                    *seq,
                    *status,
                    *duration_ms,
                    *cost_usd,
                    *tokens,
                    model.clone(),
                    *reverted,
                )),
                _ => None,
            })
            .collect();
        assert_eq!(
            turns,
            vec![
                (
                    5,
                    TurnStatus::Completed,
                    2400,
                    Some(0.18),
                    82_000,
                    Some("anthropic/claude-opus-5-5".into()),
                    false
                ),
                (8, TurnStatus::Interrupted, 2400, None, 0, None, true),
            ]
        );
        // An answer's time is when its turn ended.
        let ended: Vec<i64> = blocks
            .iter()
            .filter_map(|block| match block {
                Block::Turn { at, .. } => Some(*at),
                _ => None,
            })
            .collect();
        assert_eq!(ended, vec![1_004, 2_002]);
        assert!(matches!(
            blocks[0],
            Block::User {
                at: 1_000,
                rewound: false,
                ..
            }
        ));
        assert!(matches!(
            blocks[2],
            Block::User {
                at: 2_000,
                rewound: true,
                ..
            }
        ));
    }

    #[test]
    fn a_plan_moved_to_a_new_session_says_so() {
        let ask = WbEvent::Approval {
            request_id: "r".into(),
            tool_use_id: None,
            tool: "ExitPlanMode".into(),
            input: json!({ "plan": "# Plan" }),
            description: None,
            suggestions: Vec::new(),
            parent: None,
        };
        let moved = |message: Option<&str>| {
            let resolved = WbEvent::ApprovalResolved {
                request_id: "r".into(),
                decision: Decision::Deny,
                message: message.map(str::to_string),
            };
            blocks(vec![(1, ask.clone()), (2, resolved)])
                .into_iter()
                .find_map(|block| match block {
                    Block::Approval { moved, .. } => Some(moved),
                    _ => None,
                })
        };
        assert_eq!(moved(Some(MOVED_ON)), Some(true));
        assert_eq!(moved(Some("not like this")), Some(false));
        assert_eq!(moved(None), Some(false));
    }

    #[test]
    fn message_times_read_as_the_nexus_app_shows_them() {
        let now = 10_000_000_000;
        assert_eq!(message_time(now - 20_000, now), "Just now");
        assert_eq!(message_time(now - 60_000, now), "1 minute ago");
        assert_eq!(message_time(now - 5 * 60_000, now), "5 minutes ago");
        assert_eq!(message_time(now - 3 * 3_600_000, now), "3 hours ago");
        assert_eq!(message_time(now - 86_400_000, now), "1 day ago");
        assert_eq!(message_time(now - 40 * 86_400_000, now), "1 month ago");
        assert_eq!(message_time(now + 5_000, now), "Just now");
        assert!(message_time_exact(now).contains(" at "));
    }

    #[test]
    fn durations_counts_and_costs_read_as_nexus_ui_says_them() {
        assert_eq!(duration(850), "850 ms");
        assert_eq!(duration(185_000), "3 min 5 s");
        assert_eq!(compact(1_500_000), "1.5M");
        assert_eq!(compact(34_000), "34k");
        assert_eq!(usd(0.004), "$0.004000");
    }

    #[test]
    fn tool_calls_read_as_one_line() {
        assert_eq!(summarize(&json!({ "command": "cargo test\n--all" })), "cargo test");
        assert_eq!(summarize(&json!({ "file_path": "src/main.rs" })), "src/main.rs");
        assert_eq!(summarize(&json!({})), "");
    }

    #[test]
    fn model_labels() {
        assert_eq!(model_label("claude-opus-5-5"), "Opus 5.5");
        assert_eq!(model_label("claude-haiku-5-5-20261101"), "Haiku 5.5");
        assert_eq!(model_label("gpt-5"), "Gpt 5");
        assert_eq!(model_label("sonnet"), "sonnet");
    }

    #[test]
    fn a_transcript_snapshot_keeps_browser_signals_with_its_page_bounds() {
        let event = serde_json::from_value(json!({
            "kind": "text", "id": "ready", "text": "Local: http://localhost:5173/", "parent": null
        }))
        .unwrap();
        let read = Transcript::of(vec![(700, 0, event)]);
        assert_eq!((read.start, read.last, read.more), (Some(700), 700, true));
        assert_eq!(
            read.signals,
            vec![Signal::Server {
                seq: 700,
                url: "http://localhost:5173/".into()
            }]
        );
        assert_eq!(read.blocks.len(), 1);
    }

    #[test]
    fn a_long_session_opens_at_its_end_reads_older_pages_and_keeps_its_start() {
        let store = CodeStore {
            data: std::path::PathBuf::new(),
            store: Store::in_memory().unwrap(),
            scope: "gateway|org".into(),
            include_local: false,
        };
        // A message and its turn's end: a page starts at a message after a finished turn (`page.rs`'s quiet start).
        let turn = |text: String| {
            store
                .store
                .append(
                    "thread",
                    1,
                    &WbEvent::User {
                        text,
                        from: None,
                        images: Vec::new(),
                    },
                )
                .unwrap();
            store
                .store
                .append(
                    "thread",
                    1,
                    &WbEvent::TurnEnd {
                        turn_id: "turn".into(),
                        status: TurnStatus::Completed,
                        cost_usd: None,
                        duration_ms: 10,
                        error: None,
                        cursor: None,
                    },
                )
                .unwrap()
        };
        let turns = EVENTS + 25;
        for n in 1..=turns {
            turn(format!("Message {n}"));
        }
        let reader = store.reader();
        let text = |block: &Block| match block {
            Block::User { text, .. } => text.clone(),
            _ => String::new(),
        };

        // It opens at its end: the latest page, with more before it.
        let latest = reader.transcript("thread", None, false).unwrap();
        assert_eq!(latest.blocks.len(), EVENTS);
        assert!(latest.more);
        let start = latest.start.unwrap();
        assert_eq!(text(&latest.blocks[0]), format!("Message {}", (start + 1) / 2));

        // A new turn keeps the start: the blocks above the reader stay where they are.
        let added = turn("New".into());
        let grown = reader.transcript("thread", Some(start), false).unwrap();
        assert_eq!((grown.start, grown.last), (Some(start), added.seq));
        assert_eq!(&grown.blocks[..EVENTS], &latest.blocks[..]);

        // Each older read adds the page before, down to the first message.
        let older = reader.transcript("thread", Some(start), true).unwrap();
        assert!(older.start.unwrap() < start);
        assert!(older.more);
        assert_eq!(
            &older.blocks[older.blocks.len() - grown.blocks.len()..],
            &grown.blocks[..]
        );
        let first = reader.transcript("thread", older.start, true).unwrap();
        assert_eq!(first.start, Some(1));
        assert!(!first.more);
        assert_eq!(first.blocks.len(), (turns + 1) * 2);
        assert_eq!(text(&first.blocks[0]), "Message 1");
    }

    #[test]
    fn reader_runs_off_thread_and_sees_new_writes() {
        let store = CodeStore {
            data: std::path::PathBuf::new(),
            store: Store::in_memory().unwrap(),
            scope: "gateway|org".into(),
            include_local: false,
        };
        let append = |text: &str| {
            store
                .store
                .append(
                    "thread",
                    1,
                    &WbEvent::User {
                        text: text.into(),
                        from: None,
                        images: Vec::new(),
                    },
                )
                .unwrap()
        };
        let first = append("First message");
        let reader = store.reader();
        let (reader, Transcript { blocks, last: seq, .. }) = std::thread::spawn(move || {
            let read = reader.transcript("thread", None, false).unwrap();
            (reader, read)
        })
        .join()
        .unwrap();
        assert_eq!(seq, first.seq);
        assert!(matches!(&blocks[..], [Block::User { text, .. }] if text == "First message"));
        let second = append("Second message");
        store
            .store
            .append(
                "thread",
                2,
                &WbEvent::Commands {
                    commands: vec!["review".into()],
                },
            )
            .unwrap();
        let end = store
            .store
            .append(
                "thread",
                3,
                &WbEvent::TurnEnd {
                    turn_id: "turn".into(),
                    status: TurnStatus::Interrupted,
                    cost_usd: None,
                    duration_ms: 10,
                    error: None,
                    cursor: None,
                },
            )
            .unwrap();
        std::thread::spawn(move || {
            assert!(reader.has_events_after("thread", seq));
            let (events, summary) = reader.agent_events("thread", seq).unwrap();
            assert_eq!(events.first().unwrap().seq, second.seq);
            assert_eq!(events.last().unwrap().seq, end.seq);
            assert!(summary.is_none());
            let Transcript { blocks, last, .. } = reader.transcript("thread", None, false).unwrap();
            assert_eq!(last, end.seq);
            // The second message was sent during the turn and takes the next one: it shows after the turn's end.
            assert!(matches!(&blocks[..], [Block::User { .. }, Block::Turn { status: TurnStatus::Interrupted, .. }, Block::User { seq, text, .. }] if *seq == second.seq && text == "Second message"));
            assert_eq!(reader.commands("thread"), Some(vec!["review".into()]));
        })
        .join()
        .unwrap();
    }

    #[test]
    fn a_side_chat_reader_keeps_the_parent_and_organization_scope() {
        let store = CodeStore {
            data: std::path::PathBuf::new(),
            store: Store::in_memory().unwrap(),
            scope: "gateway|org".into(),
            include_local: false,
        };
        let insert = |id: &str, scope: &str, parent: Option<&str>, side_chat: bool| {
            let mut summary = ordered_thread(id, "p", 1, None);
            summary.side_chat = side_chat;
            summary.source_thread_id = parent.map(str::to_owned);
            store
                .store
                .insert_thread(&nexus_workbench::store::ThreadRecord {
                    summary,
                    native_id: format!("native-{id}"),
                    native_started: false,
                    scope: scope.into(),
                    fork_from: None,
                    seed: None,
                    automatic_title: false,
                    delivered_seq: 0,
                    halt: None,
                })
                .unwrap();
        };
        insert("parent", "gateway|org", None, false);
        insert("mine", "gateway|org", Some("parent"), true);
        insert("another-parent", "gateway|org", Some("other"), true);
        insert("another-org", "gateway|other-org", Some("parent"), true);
        insert("ordinary-fork", "gateway|org", Some("parent"), false);
        let reader = store.reader();
        let chats = std::thread::spawn(move || reader.side_chats("parent").unwrap())
            .join()
            .unwrap();
        assert_eq!(chats.into_iter().map(|chat| chat.id).collect::<Vec<_>>(), ["mine"]);
    }

    #[test]
    fn a_summary_is_one_row_of_this_scope_only() {
        let store = CodeStore {
            data: std::path::PathBuf::new(),
            store: Store::in_memory().unwrap(),
            scope: "gateway|org".into(),
            include_local: false,
        };
        let insert = |id: &str, scope: &str| {
            let summary = serde_json::from_value(serde_json::json!({
                "id": id, "projectId": "p", "title": "Deploy the docs", "agent": "claude", "model": null,
                "permissionMode": "default", "via": "nexus", "status": "idle", "costUsd": 0.0, "nexusSessions": [],
                "contextTokens": null, "contextWindow": null, "createdAt": 1, "updatedAt": 1, "archived": false,
            }))
            .unwrap();
            store
                .store
                .insert_thread(&nexus_workbench::store::ThreadRecord {
                    summary,
                    native_id: format!("native-{id}"),
                    native_started: false,
                    scope: scope.into(),
                    fork_from: None,
                    seed: None,
                    automatic_title: false,
                    delivered_seq: 0,
                    halt: None,
                })
                .unwrap();
        };
        insert("mine", "gateway|org");
        insert("theirs", "gateway|other-org");
        assert_eq!(
            store.summary("mine").unwrap().map(|summary| summary.title),
            Some("Deploy the docs".into())
        );
        assert_eq!(store.summary("theirs").unwrap(), None);
        assert_eq!(store.summary("gone").unwrap(), None);
    }

    #[test]
    fn news_is_any_event_after_the_last_one_seen() {
        let store = CodeStore {
            data: std::path::PathBuf::new(),
            store: Store::in_memory().unwrap(),
            scope: "gateway|org".into(),
            include_local: false,
        };
        let reader = store.reader();
        let event = || WbEvent::TurnStart { turn_id: "t".into() };
        // A session with nothing stored has nothing after anything.
        assert!(!reader.has_events_after("thread", 0));
        let first = store.store.append("thread", 1, &event()).unwrap();
        assert!(reader.has_events_after("thread", 0));
        assert!(!reader.has_events_after("thread", first.seq));
        let second = store.store.append("thread", 2, &event()).unwrap();
        assert!(reader.has_events_after("thread", first.seq));
        assert!(!reader.has_events_after("thread", second.seq));
        // Another session's events are not this one's.
        assert!(!reader.has_events_after("other", 0));
    }
    #[test]
    fn nested_agent_blocks_keep_event_keys_and_running_turn_cost() {
        let event = |value| serde_json::from_value::<WbEvent>(value).expect("event");
        let transcript = Transcript::of(vec![
            (1, 1, WbEvent::TurnStart { turn_id: "t".into() }),
            (
                2,
                2,
                event(serde_json::json!({ "kind": "tool", "id": "agent", "name": "Task",
                "input": { "description": "Inspect", "prompt": "Inspect" }, "parent": null })),
            ),
            (
                3,
                3,
                event(serde_json::json!({ "kind": "text", "id": "answer", "text": "Main", "parent": null })),
            ),
            (
                4,
                4,
                event(serde_json::json!({ "kind": "text", "id": "nested", "text": "Nested", "parent": "agent" })),
            ),
            (
                5,
                5,
                event(serde_json::json!({ "kind": "usage", "inputTokens": 10,
                "outputTokens": 4, "cacheReadTokens": 0, "cacheWriteTokens": 0, "costUsd": 0.25, "model": "test" })),
            ),
        ]);
        assert_eq!(transcript.turn_cost, Some(0.25));
        let nested = locate(&transcript.starts, 4).expect("nested event");
        let main = locate(&transcript.starts, 3).expect("main event");
        assert!(nested < main, "nested text moves beneath its agent");
        assert_eq!(transcript.blocks[nested].path(), ["agent"]);
        assert!(matches!(transcript.blocks[nested].inner(), Block::Text(text) if text == "Nested"));
        assert!(matches!(transcript.blocks[main], Block::Text(ref text) if text == "Main"));
    }
}

#[cfg(test)]
mod handoff_tests {
    use super::{HandedOff, SessionAgent};
    #[test]
    fn a_handoff_call_names_the_new_session_for_either_agent_and_nothing_else_does() {
        let answer = r#"{"session_id":"t_new","title":"Fix login","agent":"codex","status":"starting"}"#;
        let expected = Some(HandedOff {
            session_id: "t_new".into(),
            title: "Fix login".into(),
            agent: SessionAgent::Codex,
        });
        assert_eq!(
            HandedOff::parse("mcp__nexus_app__handoff_session", answer, false),
            expected
        );
        assert_eq!(HandedOff::parse("handoff_session", answer, false), expected);
        assert_eq!(HandedOff::parse("mcp__nexus_app__start_session", answer, false), None);
        assert_eq!(
            HandedOff::parse("handoff_session", answer, true),
            None,
            "a refusal is a plain row"
        );
        assert_eq!(HandedOff::parse("handoff_session", "not json", false), None);
    }
}
