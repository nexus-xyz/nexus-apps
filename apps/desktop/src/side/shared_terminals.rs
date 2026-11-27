//! Agent command activity lives in the conversation. Human Workbench shells use `TerminalSet`.
use super::terminal::Terminal;
mod approval;
mod batch;
#[cfg(test)]
mod copy_tests;
mod dispatch;
mod finished;
mod inline;
mod list;
mod pane;
#[cfg(test)]
mod pane_tests;
mod receipts;
mod render;
mod retention;
#[cfg(test)]
mod retention_tests;
mod rows;
mod view;
mod watch;
pub(crate) use dispatch::Dispatch;
pub(crate) use retention::RetainedSharedTerminals;
// The Commands pane's parts, which a session's shells use before it starts (`terminals/commands.rs`).
pub(crate) use pane::{frame as pane_frame, item as pane_item};
pub(crate) use rows::{ROW, ROW_RADIUS, folder_slot, row_button};
pub(crate) type Guard = std::sync::Arc<dyn Fn() -> bool + Send + Sync>;
use crate::pages::ui;
use crate::workspace::glyph;
use crate::{code_run::CodeRunner, palette::Palette};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    id: String,
    name: String,
    /// A shell the person types into, not one command: its `command` is only the words "Interactive shell".
    #[serde(default)]
    interactive: bool,
    command: String,
    reason: Option<String>,
    cwd: String,
    status: String,
    #[serde(default)]
    created_at: i64,
    private: bool,
    #[serde(default)]
    owner: String,
    #[serde(default)]
    closed: bool,
    exit_code: Option<u32>,
    #[serde(default)]
    deleted: bool,
    #[serde(default = "workbench_route")]
    route: String,
    #[serde(default)]
    restored: bool,
    #[serde(default)]
    input_count: u64,
    #[serde(default)]
    stop_requested: bool,
    #[serde(default)]
    bypass_code: Option<String>,
    #[serde(default)]
    bypass_reason: Option<String>,
}
/// How often a read that changed nothing still redraws the view, so a row's age ("now", "2m") moves on.
const AGES_REFRESH: Duration = Duration::from_secs(10);

fn workbench_route() -> String {
    "workbench".into()
}

fn newest_first(entries: &[Entry]) -> Vec<&Entry> {
    newest_first_places(entries)
        .into_iter()
        .map(|at| &entries[at])
        .collect()
}

/// The places in `entries`, newest first; of two at the same time, the later one first.
fn newest_first_places(entries: &[Entry]) -> Vec<usize> {
    let mut ordered: Vec<usize> = (0..entries.len()).collect();
    ordered.sort_by_key(|&at| std::cmp::Reverse((entries[at].created_at, at)));
    ordered
}

impl Entry {
    fn needs_attention(&self) -> bool {
        self.status == "awaitingUser" && self.attached()
    }
    fn active(&self) -> bool {
        matches!(
            self.status.as_str(),
            "awaitingUser" | "starting" | "running" | "providerPending"
        )
    }
    fn attached(&self) -> bool {
        self.route == "workbench" && !self.restored
    }
    fn accepts_input(&self, read_only: bool) -> bool {
        !read_only && self.attached() && !self.stop_requested && self.owner == "user" && self.status == "running"
    }
    fn status(&self) -> String {
        if self.stop_requested && self.status == "running" {
            return "Stopping".into();
        }
        match (self.status.as_str(), self.exit_code) {
            ("awaitingUser", _) => "Needs you".into(),
            // Stopped by a person: its exit code (often 1 or 130) is the stop's, not a failure.
            ("interrupted", _) => "Stopped".into(),
            (_, Some(code)) => format!("Exit {code}"),
            ("providerPending", _) => "Provider command pending".into(),
            ("starting", _) => "Starting".into(),
            ("running", _) => "Running".into(),
            ("unknown", _) => "Unknown outcome".into(),
            ("cancelled", _) => "Cancelled".into(),
            ("denied", _) => "Denied".into(),
            ("failed", None) if self.route == "workbench" => "Failed to start".into(),
            ("failed", None) => "Provider reported failure".into(),
            ("completed", None) => "Finished · exit code unavailable".into(),
            ("declined", _) => "Declined".into(),
            _ => "Stopped".into(),
        }
    }
}
#[derive(Deserialize)]
struct Read {
    bytes: Vec<u8>,
    next: u64,
    truncated: bool,
}
struct Output {
    view: Entity<Terminal>,
    cursor: u64,
}

/// A rendered decision cannot switch terminals or approve a changed command.
fn valid_decision(entries: &[Entry], op: &str, args: &Value) -> bool {
    let Some(id) = args.get("id").and_then(Value::as_str) else {
        return false;
    };
    entries.iter().any(|entry| {
        entry.id == id
            && entry.status == "awaitingUser"
            && (op == "decline" || args.get("command").and_then(Value::as_str) == Some(entry.command.as_str()))
    })
}

pub enum SharedTerminalsEvent {
    OpenCommands,
    /// What the tray above the composer shows changed while it may not be drawn: a finished card came or went.
    Activity,
    Deleted(Vec<String>),
    RestoreFailed(Vec<String>, String),
    /// A setup fix's command (`run_watched`) ended: well when it exited 0.
    FixEnded {
        token: u64,
        ok: bool,
        code: Option<u32>,
        last_line: Option<String>,
    },
}
impl EventEmitter<SharedTerminalsEvent> for SharedTerminals {}

pub struct SharedTerminals {
    owner_guard: Guard,
    owner_fence: std::sync::Arc<std::sync::Mutex<Guard>>,
    presentation: std::sync::Arc<std::sync::atomic::AtomicBool>,
    receipts: Rc<std::cell::RefCell<Vec<receipts::Receipt>>>,
    pending_actions: usize,
    undo_ids: Vec<String>,
    inline_open: bool,
    inline_motion: crate::command_tray::Motion,
    group_motion: crate::command_tray::Motion,
    closing_entries: Vec<Entry>,
    pending_rows: approval::PendingRows,
    closing_rows: approval::PendingRows,
    inline_limit: f32,
    inline_focus: FocusHandle,
    inline_scroll: ScrollHandle,
    pane_list: pane::PaneList,
    inline_selected: Option<String>,
    inline_initialized: bool,
    show_history: bool,
    history_allowed: bool,
    delete_confirmation: Option<Vec<String>>,
    editing: Option<(String, Entity<TextareaState>)>,
    focus_output: Option<String>,
    pending_input: Rc<std::cell::RefCell<Vec<crate::code_run::Answer<Value>>>>,
    runner: Rc<CodeRunner>,
    /// Where the pane's actions go: the runner's own workbench, or a server's host.
    dispatch: Dispatch,
    thread: String,
    entries: Vec<Entry>,
    selected: Option<String>,
    collapsed: HashSet<String>,
    inline_expanded: HashSet<String>,
    action_error: Option<(String, String)>,
    outputs: HashMap<String, Output>,
    submitted_inputs: HashMap<String, (u64, Vec<String>)>,
    expanded: bool,
    busy: bool,
    error: Option<String>,
    last_draw: Instant,
    /// When a read last redrew the view. A read that changed nothing redraws only after `AGES_REFRESH`, so the rows'
    /// ages stay current without a redraw every 50ms.
    last_poll_draw: Instant,
    poll: Option<Task<()>>,
    polling: bool,
    monitor: bool,
    read_only: bool,
    registry_revision: u64,
    /// The Commands tab's search field, made on first draw, and its words as typed.
    search: Option<Entity<gpui_kit::component::input::InputState>>,
    _search_events: Option<Subscription>,
    query: String,
    /// The Commands list's own focus: Up, Down, Enter, Space, Escape and `/` work while it has it.
    list_focus: FocusHandle,
    /// The commands the first read found: only ones that arrive after it move in.
    seen: Option<HashSet<String>>,
    /// When each command arrived, and when its state last changed, for their short entrances.
    arrived: HashMap<String, Instant>,
    changed: HashMap<String, Instant>,
    /// Requests the person accepted that have not ended yet, and the cards of those that have (`finished.rs`).
    accepted: HashSet<String>,
    finished: Vec<finished::Finished>,
    /// The command a finished card's click opens in the Commands tab.
    reveal: Option<String>,
    /// The shell the person last opened with New shell: the one command that arrives open.
    open_shell: Option<String>,
    /// A setup fix's commands (`watch.rs`): the token the next run reports under, and each running one's.
    watch_next: Option<u64>,
    watched: HashMap<String, u64>,
    /// Each open command's output well: its height, its drag and the maximized one (`side::well`).
    wells: crate::side::well::Wells,
    /// Whether each open row cuts its command's first line, as its last paint measured it (`rows::cut_probe`).
    /// The details repeat the command only when the row does not show it whole.
    cut: Rc<std::cell::RefCell<HashMap<String, bool>>>,
}
impl SharedTerminals {
    pub(crate) fn has_modal(&self) -> bool {
        self.delete_confirmation.is_some() || self.editing.is_some()
    }

    pub fn new(runner: Rc<CodeRunner>, thread: String, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let guard = runner.source_guard();
        Self::new_guarded(runner, thread, guard, _window, cx)
    }

    pub(crate) fn new_guarded(
        runner: Rc<CodeRunner>,
        thread: String,
        guard: Guard,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let dispatch = dispatch::runner(runner.clone());
        Self::new_dispatched(runner, dispatch, thread, guard, window, cx)
    }

    /// The Commands of a session on server `machine`: its host runs the commands, and this pane shows them and sends
    /// the person's actions there. `guard` is the window's proof that it still shows that server's session.
    pub(crate) fn new_server(
        runner: Rc<CodeRunner>,
        machines: std::sync::Arc<crate::machines::Machines>,
        machine: String,
        thread: String,
        guard: Guard,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let dispatch = dispatch::server(machines, machine);
        Self::new_dispatched(runner, dispatch, thread, guard, window, cx)
    }

    fn new_dispatched(
        runner: Rc<CodeRunner>,
        dispatch: Dispatch,
        thread: String,
        guard: Guard,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let source = runner.source_guard();
        let guard: Guard = std::sync::Arc::new(move || source() && guard());
        let owner_fence = std::sync::Arc::new(std::sync::Mutex::new(guard));
        let fence = owner_fence.clone();
        let guard: Guard = std::sync::Arc::new(move || {
            let proof = fence.lock().ok().map(|proof| proof.clone());
            proof.is_some_and(|proof| proof())
        });
        let mut view = Self {
            owner_guard: guard,
            owner_fence,
            presentation: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            receipts: Rc::default(),
            pending_actions: 0,
            undo_ids: Vec::new(),
            inline_open: true,
            inline_motion: Default::default(),
            group_motion: Default::default(),
            closing_entries: Vec::new(),
            pending_rows: Default::default(),
            closing_rows: Default::default(),
            inline_limit: 300.,
            inline_focus: cx.focus_handle(),
            inline_scroll: ScrollHandle::new(),
            pane_list: pane::PaneList::new(),
            inline_selected: None,
            inline_initialized: false,
            show_history: false,
            history_allowed: false,
            delete_confirmation: None,
            editing: None,
            focus_output: None,
            pending_input: Rc::default(),
            runner,
            dispatch,
            thread,
            entries: vec![],
            selected: None,
            collapsed: HashSet::new(),
            inline_expanded: HashSet::new(),
            action_error: None,
            outputs: HashMap::new(),
            submitted_inputs: HashMap::new(),
            expanded: true,
            busy: false,
            error: None,
            last_draw: Instant::now(),
            last_poll_draw: Instant::now(),
            poll: None,
            polling: false,
            monitor: false,
            read_only: false,
            registry_revision: 0,
            search: None,
            _search_events: None,
            query: String::new(),
            list_focus: cx.focus_handle(),
            seen: None,
            arrived: HashMap::new(),
            changed: HashMap::new(),
            accepted: HashSet::new(),
            finished: Vec::new(),
            reveal: None,
            open_shell: None,
            watch_next: None,
            watched: HashMap::new(),
            wells: Default::default(),
            cut: Rc::default(),
        };
        view.start_poll(cx);
        view
    }
    pub fn has_command_attention(&self) -> bool {
        self.entries.iter().any(Entry::needs_attention) || self.inline_motion.visible() || self.has_cards()
    }
    pub fn set_inline_limit(&mut self, limit: f32, cx: &mut Context<Self>) {
        if self.inline_limit != limit {
            self.inline_limit = limit;
            cx.notify();
        }
    }
    pub fn show_pending(&mut self, cx: &mut Context<Self>) {
        // A finished card's click: its command, open, in the tab that holds it.
        if let Some(id) = self.reveal.take() {
            self.show_history =
                !(self.lingering(&id) || self.entries.iter().any(|entry| entry.id == id && entry.active()));
            self.collapsed.remove(&id);
            self.selected = Some(id);
            cx.notify();
            return;
        }
        self.show_history = false;
        if let Some(id) = self.inline_selected.clone() {
            self.collapsed.remove(&id);
            self.selected = Some(id);
        }
        cx.notify();
    }
    pub fn review_inline(&mut self, cx: &mut Context<Self>) {
        self.inline_selected = self
            .entries
            .iter()
            .find(|entry| entry.status == "awaitingUser")
            .map(|entry| entry.id.clone());
        self.inline_open = true;
        self.inline_scroll.set_offset(point(px(0.), px(0.)));
        cx.notify();
    }
    pub fn allow_history(&mut self, allowed: bool, cx: &mut Context<Self>) {
        if self.history_allowed != allowed {
            self.history_allowed = allowed;
            cx.notify();
        }
    }
    /// Record what arrived and what changed state since the last read. The first read only records.
    fn note_motion(&mut self, entries: &[Entry]) {
        let now = Instant::now();
        match &mut self.seen {
            None => self.seen = Some(entries.iter().map(|entry| entry.id.clone()).collect()),
            Some(seen) => {
                let old: HashMap<&str, &Entry> = self.entries.iter().map(|old| (old.id.as_str(), old)).collect();
                for entry in entries {
                    if seen.insert(entry.id.clone()) {
                        self.arrived.insert(entry.id.clone(), now);
                    } else if old
                        .get(entry.id.as_str())
                        .is_some_and(|old| list::tone(old) != list::tone(entry))
                    {
                        self.changed.insert(entry.id.clone(), now);
                    }
                }
            }
        }
        let fresh = Duration::from_secs(1);
        self.arrived.retain(|_, at| at.elapsed() < fresh);
        self.changed.retain(|_, at| at.elapsed() < fresh);
    }
    pub fn registry_revision(&self) -> u64 {
        self.registry_revision
    }
    pub fn monitor(&mut self, active: bool, cx: &mut Context<Self>) {
        self.monitor = active;
        if active {
            self.start_poll(cx);
        }
    }
    pub fn execution_status(&self, id: &str) -> Option<String> {
        self.entries.iter().find(|e| e.id == id).map(|e| e.status.clone())
    }
    /// Whether a command runs or waits now (`Entry::active`): the Workbench's Commands tab shows its dot.
    pub fn has_active(&self) -> bool {
        any_active(&self.entries)
    }
    pub fn ready_execution(&self, id: &str) -> bool {
        ready_execution(&self.entries, id)
    }
    /// Follow agent: the agent's command `id`, open in its list. A running one shows in Active, not History.
    pub fn select_execution(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.ready_execution(id) {
            if self.entries.iter().any(|e| e.id == id && e.status == "running") {
                self.show_history = false;
            }
            self.collapsed.remove(id);
            self.selected = Some(id.into());
            self.expanded = true;
            self.last_draw = Instant::now();
            self.start_poll(cx);
            cx.notify();
        }
    }
    fn start_poll(&mut self, cx: &mut Context<Self>) {
        if self.polling || !self.is_current() {
            return;
        }
        self.polling = true;
        self.poll = Some(cx.spawn(async move |this, cx| {
            loop {
                let Ok(request) = this.update(cx, |this, cx| {
                    if !this.is_current() {
                        return None;
                    }
                    // A receipt or an answer to the person's input changes what the pane shows.
                    if !this.receipts.borrow().is_empty() || !this.pending_input.borrow().is_empty() {
                        this.drain_receipts(cx);
                        cx.notify();
                    }
                    // An accepted command keeps the reads going until it ends, so its finished card shows.
                    (this.monitor
                        || this.pending_actions > 0
                        || !this.pending_input.borrow().is_empty()
                        || !this.accepted.is_empty()
                        || this.last_draw.elapsed() < Duration::from_secs(2))
                    .then(|| this.terminal(this.thread.clone(), "list".into(), json!({})))
                }) else {
                    break;
                };
                let Some(request) = request else {
                    let _ = this.update(cx, |this, _| this.polling = false);
                    break;
                };
                let result = request
                    .await
                    .unwrap_or_else(|_| Err("The session runner stopped.".into()));
                let _ = this.update(cx, |this, cx| {
                    if !this.is_current() {
                        return;
                    }
                    match result.and_then(|v| serde_json::from_value::<Vec<Entry>>(v).map_err(|e| e.to_string())) {
                        Ok(entries) => {
                            let entries: Vec<_> = entries
                                .into_iter()
                                .filter(|e| {
                                    !e.deleted
                                        && (!this.read_only
                                            || (!e.private
                                                && !e.closed
                                                && e.status != "awaitingUser"
                                                && e.status != "declined"))
                                })
                                .collect();
                            let changed = this.entries != entries;
                            if changed {
                                this.registry_revision += 1;
                            }
                            let before = (this.inline_selected.clone(), this.selected.clone());
                            // Every command arrives closed: its row's own buttons decide it, take it or stop it. Only
                            // a shell the person opened with New shell arrives open, to type in. After that, a
                            // command keeps the state the person left it in when it is accepted, runs or ends.
                            if changed && !this.read_only {
                                let known: HashSet<&str> = this.entries.iter().map(|old| old.id.as_str()).collect();
                                for entry in &entries {
                                    if !known.contains(entry.id.as_str())
                                        && this.open_shell.as_deref() != Some(entry.id.as_str())
                                    {
                                        this.collapsed.insert(entry.id.clone());
                                    }
                                }
                            }
                            this.inline_selected = next_inline_selection(
                                &this.entries,
                                &entries,
                                this.inline_selected.as_deref(),
                                this.inline_initialized,
                            );
                            this.inline_initialized = entries.iter().any(Entry::active);
                            this.pending_rows.follow(&entries);
                            this.note_motion(&entries);
                            let finished = this.note_finished(&entries);
                            if finished {
                                cx.emit(SharedTerminalsEvent::Activity);
                            }
                            for ended in this.ended_watches(&entries, cx) {
                                cx.emit(ended);
                            }
                            this.entries = entries;
                            if changed {
                                let ids: HashMap<&str, bool> = this
                                    .entries
                                    .iter()
                                    .map(|entry| (entry.id.as_str(), entry.private))
                                    .collect();
                                this.collapsed.retain(|id| ids.contains_key(id.as_str()));
                                this.inline_expanded.retain(|id| ids.contains_key(id.as_str()));
                                this.outputs
                                    .retain(|id, _| ids.get(id.as_str()).is_some_and(|private| !private));
                            }
                            if !this.read_only && !this.entries.iter().any(|e| Some(&e.id) == this.selected.as_ref()) {
                                this.selected = this
                                    .entries
                                    .iter()
                                    .find(|e| e.status == "awaitingUser")
                                    .or_else(|| this.entries.last())
                                    .map(|e| e.id.clone());
                            }
                            // A read that changed nothing does not redraw: with a command running the reads come
                            // every 50ms, and each redraw lays out the pane. The ages still move on a slow beat.
                            if changed
                                || finished
                                || before != (this.inline_selected.clone(), this.selected.clone())
                                || this.last_poll_draw.elapsed() >= AGES_REFRESH
                            {
                                this.last_poll_draw = Instant::now();
                                cx.notify();
                            }
                        }
                        Err(error) => {
                            this.error = Some(error);
                            cx.notify();
                        }
                    }
                });
                let Ok(reads) = this.update(cx, |this, _| {
                    if !this.is_current() {
                        return Vec::new();
                    }
                    this.entries
                        .iter()
                        .filter(|e| {
                            e.attached()
                                && !e.private
                                && !matches!(e.status.as_str(), "awaitingUser" | "declined")
                                && (e.status == "running" || !this.collapsed.contains(&e.id))
                        })
                        .map(|entry| {
                            let id = entry.id.clone();
                            let cursor = this.outputs.get(&id).map_or(0, |o| o.cursor);
                            let writable = entry.accepts_input(this.read_only);
                            (
                                id.clone(),
                                writable,
                                this.terminal(
                                    this.thread.clone(),
                                    "read".into(),
                                    json!({"id":id,"after":cursor,"max_bytes":65536}),
                                ),
                            )
                        })
                        .collect::<Vec<_>>()
                }) else {
                    break;
                };
                for (id, writable, read) in reads {
                    let result = read.await.unwrap_or_else(|_| Err("The session runner stopped.".into()));
                    let _ = this.update(cx, |this, cx| {
                        if !this.is_current() {
                            return;
                        }
                        match result.and_then(|v| serde_json::from_value::<Read>(v).map_err(|e| e.to_string())) {
                            Ok(read) => {
                                // The output view redraws itself when it takes bytes; the pane redraws only when the
                                // row gains its output.
                                if !this.outputs.contains_key(&id) {
                                    let send = this.output_sender(id.clone());
                                    let view = cx.new(|cx| Terminal::attached(send, cx));
                                    this.outputs.insert(id.clone(), Output { view, cursor: 0 });
                                    cx.notify();
                                }
                                let output = this.outputs.get_mut(&id).unwrap();
                                let applied = output
                                    .view
                                    .update(cx, |view, cx| view.feed(&read.bytes, writable, read.truncated, cx));
                                if applied {
                                    output.cursor = read.next;
                                }
                            }
                            Err(error) => {
                                this.error = Some(error);
                                cx.notify();
                            }
                        }
                    });
                }
                let submissions = this
                    .update(cx, |this, _| {
                        if !this.is_current() {
                            return Vec::new();
                        }
                        this.entries
                            .iter()
                            .filter(|entry| {
                                !entry.private
                                    && entry.input_count > 0
                                    && !this.collapsed.contains(&entry.id)
                                    && this
                                        .submitted_inputs
                                        .get(&entry.id)
                                        .is_none_or(|(count, _)| *count != entry.input_count)
                            })
                            .map(|entry| {
                                (
                                    entry.id.clone(),
                                    entry.input_count,
                                    this.terminal(this.thread.clone(), "submissions".into(), json!({"id":entry.id})),
                                )
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                for (id, count, request) in submissions {
                    if let Ok(Ok(value)) = request.await {
                        let inputs = value["inputs"]
                            .as_array()
                            .map(|values| {
                                values
                                    .iter()
                                    .filter_map(|value| value.as_str().map(String::from))
                                    .collect()
                            })
                            .unwrap_or_default();
                        let _ = this.update(cx, |this, cx| {
                            if !this.is_current() {
                                return;
                            }
                            this.submitted_inputs.insert(id, (count, inputs));
                            cx.notify();
                        });
                    }
                }
                let running = this
                    .update(cx, |this, _| this.entries.iter().any(|e| e.status == "running"))
                    .unwrap_or(false);
                cx.background_executor()
                    .timer(Duration::from_millis(if running { 50 } else { 700 }))
                    .await;
            }
        }));
    }
    pub fn open_shell(&mut self, cx: &mut Context<Self>) {
        self.show_history = false;
        self.perform(
            "stage_shell",
            json!({"request_id":uuid::Uuid::new_v4().to_string()}),
            cx,
        );
    }
    pub fn run(&mut self, command: String, name: Option<String>, cx: &mut Context<Self>) {
        self.show_history = false;
        self.perform(
            "request_user",
            json!({"command":command,"name":name,"request_id":uuid::Uuid::new_v4().to_string()}),
            cx,
        );
    }
    fn perform(&mut self, op: &'static str, args: Value, cx: &mut Context<Self>) {
        self.perform_with_batch(op, args, None, cx);
    }

    fn perform_with_batch(
        &mut self,
        op: &'static str,
        args: Value,
        batch: Option<receipts::Batch>,
        cx: &mut Context<Self>,
    ) {
        if self.busy || self.read_only || !self.is_current() {
            return;
        }
        if op == "decline" && !valid_decision(&self.entries, op, &args) {
            return;
        }
        // Running a request the person was asked about: it gets a finished card when it ends.
        if op == "execute"
            && let Some(id) = args["id"].as_str()
            && self
                .entries
                .iter()
                .any(|entry| entry.id == id && entry.status == "awaitingUser")
        {
            self.accepted.insert(id.to_owned());
        }
        self.busy = true;
        self.error = None;
        self.action_error = None;
        let target = args["id"].as_str().map(str::to_owned);
        self.pending_actions += 1;
        self.last_draw = Instant::now();
        self.start_poll(cx);
        let dispatch = self.dispatch.clone();
        let thread = self.thread.clone();
        let guard = self.owner_guard.clone();
        let receipts = self.receipts.clone();
        let watch = self.watch_next.take();
        let request = dispatch(
            thread.clone(),
            if op == "review_again" { "request_user" } else { op }.into(),
            args,
            guard.clone(),
        );
        cx.spawn(async move |_, _| {
            let mut result = request
                .await
                .unwrap_or_else(|_| Err("The session runner stopped.".into()));
            let staged = matches!(op, "request_user" | "stage_shell");
            if staged {
                if let Ok(value) = result {
                    result = dispatch(
                        thread,
                        "execute".into(),
                        json!({"id":value["id"],"command":value["command"]}),
                        guard.clone(),
                    )
                    .await
                    .unwrap_or_else(|_| Err("The session runner stopped.".into()));
                }
            }
            if guard() {
                receipts.borrow_mut().push(receipts::Receipt::Action {
                    op: if staged { "execute" } else { op },
                    target,
                    result,
                    shell: op == "stage_shell",
                    batch,
                    watch,
                });
            }
        })
        .detach();
        cx.notify();
    }
    fn delete_declined(&mut self, cx: &mut Context<Self>) {
        if self.busy || self.read_only || !self.is_current() {
            return;
        }
        let ids = self.delete_confirmation.take().unwrap_or_default();
        self.receipt_batch(ids, false, cx);
    }
    pub fn undo_delete(&mut self, ids: Vec<String>, cx: &mut Context<Self>) {
        if self.read_only || !self.is_current() {
            return;
        }
        self.undo_ids.retain(|id| !ids.contains(id));
        self.receipt_batch(ids, true, cx);
    }
    fn receipt_batch(&mut self, ids: Vec<String>, undo: bool, cx: &mut Context<Self>) {
        let requests: Vec<_> = ids
            .iter()
            .map(|id| {
                (self.dispatch)(
                    self.thread.clone(),
                    if undo { "restore" } else { "delete" }.into(),
                    json!({"id":id}),
                    self.owner_guard.clone(),
                )
            })
            .collect();
        self.pending_actions += 1;
        self.busy = true;
        let receipts = self.receipts.clone();
        let guard = self.owner_guard.clone();
        cx.spawn(async move |_, _| {
            let mut succeeded = Vec::new();
            let mut failed = Vec::new();
            let mut error = None;
            for (id, request) in ids.into_iter().zip(requests) {
                match request
                    .await
                    .unwrap_or_else(|_| Err("The session runner stopped.".into()))
                {
                    Ok(_) => succeeded.push(id),
                    Err(e) => {
                        failed.push(id);
                        error = Some(e);
                    }
                }
            }
            if guard() {
                receipts.borrow_mut().push(if undo {
                    receipts::Receipt::Undo { failed, error }
                } else {
                    receipts::Receipt::Delete {
                        deleted: succeeded,
                        error,
                    }
                });
            }
        })
        .detach();
        self.last_draw = Instant::now();
        self.start_poll(cx);
    }
}

fn any_active(entries: &[Entry]) -> bool {
    entries.iter().any(Entry::active)
}

fn ready_execution(entries: &[Entry], id: &str) -> bool {
    entries.iter().any(|entry| {
        entry.id == id
            && entry.owner == "agent"
            && !entry.closed
            && !entry.private
            && !matches!(entry.status.as_str(), "awaitingUser" | "declined")
    })
}
/// Preserve a deliberate row collapse; move review on only when its request settles or disappears.
fn next_inline_selection(
    previous: &[Entry],
    entries: &[Entry],
    selected: Option<&str>,
    initialized: bool,
) -> Option<String> {
    let was_pending = previous
        .iter()
        .any(|entry| Some(entry.id.as_str()) == selected && entry.status == "awaitingUser");
    let still_pending = entries
        .iter()
        .any(|entry| Some(entry.id.as_str()) == selected && entry.status == "awaitingUser");
    let removed = selected.is_some_and(|id| !entries.iter().any(|entry| entry.id == id && entry.active()));
    if !initialized || (was_pending && !still_pending) || removed {
        entries
            .iter()
            .find(|entry| entry.status == "awaitingUser")
            .map(|entry| entry.id.clone())
    } else {
        selected.map(str::to_owned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    #[test]
    fn decisions_bind_the_rendered_id_and_entire_command() {
        let make = |id: &str, command: &str| {
            serde_json::from_value::<Entry>(json!({
                "id":id,"name":"test","command":command,"reason":null,"cwd":"/work",
                "status":"awaitingUser","private":false,"exitCode":null
            }))
            .unwrap()
        };
        let mut entries = vec![
            make("one", "echo one && echo two\nprintf three"),
            make("two", "echo other"),
        ];
        assert!(valid_decision(
            &entries,
            "execute",
            &json!({"id":"one","command":"echo one && echo two\nprintf three"})
        ));
        assert!(!valid_decision(
            &entries,
            "execute",
            &json!({"id":"two","command":"echo one && echo two\nprintf three"})
        ));
        assert!(!valid_decision(
            &entries,
            "execute",
            &json!({"id":"one","command":"echo one"})
        ));
        assert!(!valid_decision(&entries, "decline", &json!({"id":"missing"})));
        entries[0].status = "running".into();
        assert!(!valid_decision(&entries, "decline", &json!({"id":"one"})));
        assert!(valid_decision(&entries, "decline", &json!({"id":"two"})));
    }

    #[test]
    fn requests_and_real_exit_codes_keep_their_meaning() {
        let mut entry: Entry =
            serde_json::from_value(json!({"id":"one","name":"tests","command":"npm test","reason":null,
            "cwd":"/work","status":"awaitingUser","private":false,"exitCode":null}))
            .unwrap();
        assert_eq!(entry.status(), "Needs you");
        entry.status = "failed".into();
        entry.exit_code = Some(7);
        assert_eq!(entry.status(), "Exit 7");
        entry.status = "completed".into();
        entry.exit_code = Some(0);
        assert_eq!(entry.status(), "Exit 0");
    }
}

#[cfg(test)]
mod navigation_tests {
    use super::*;
    use core::prelude::v1::test;
    fn entry(id: &str, owner: &str, status: &str) -> Entry {
        serde_json::from_value(json!({"id":id,"name":"Command","command":"true","cwd":"/repo","status":status,"private":false,"closed":false,"owner":owner})).unwrap()
    }
    #[test]
    fn only_attached_pending_commands_need_attention() {
        for status in [
            "running",
            "starting",
            "providerPending",
            "completed",
            "failed",
            "declined",
        ] {
            assert!(!entry("one", "agent", status).needs_attention(), "{status}");
        }
        let mut pending = entry("one", "agent", "awaitingUser");
        assert!(pending.needs_attention());
        pending.restored = true;
        assert!(!pending.needs_attention());
    }

    #[test]
    fn the_commands_tab_dot_shows_only_while_a_command_runs_or_waits() {
        assert!(!any_active(&[]));
        let ended = ["completed", "failed", "declined", "cancelled"].map(|status| entry(status, "agent", status));
        assert!(!any_active(&ended), "finished commands are history, not activity");
        for status in ["running", "starting", "awaitingUser", "providerPending"] {
            let mut entries = ended.to_vec();
            entries.push(entry(status, "agent", status));
            assert!(any_active(&entries), "{status}");
        }
    }

    #[test]
    fn history_is_newest_first_even_after_restore_or_out_of_order_arrival() {
        let mut entries = vec![
            entry("middle", "agent", "completed"),
            entry("new", "agent", "declined"),
            entry("old", "agent", "failed"),
        ];
        entries[0].created_at = 20;
        entries[1].created_at = 30;
        entries[2].created_at = 10;
        assert_eq!(
            newest_first(&entries).iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            ["new", "middle", "old"]
        );
        entries[2].created_at = 30;
        assert_eq!(newest_first(&entries)[0].id, "old");
        entries[2].restored = true;
        assert_eq!(newest_first(&entries)[0].id, "old");
    }

    #[test]
    fn output_only_previews_cannot_take_input_after_user_handoff() {
        let mut execution = entry("one", "agent", "running");
        assert!(!execution.accepts_input(false));
        execution.owner = "user".into();
        assert!(execution.accepts_input(false));
        assert!(!execution.accepts_input(true));
        execution.status = "completed".into();
        assert!(!execution.accepts_input(false));
    }
    #[test]
    fn only_the_exact_ready_agent_execution_can_navigate() {
        let entries = vec![
            entry("human", "user", "running"),
            entry("waiting", "agent", "awaitingUser"),
            entry("agent", "agent", "running"),
        ];
        assert!(ready_execution(&entries, "agent"));
        for id in ["human", "waiting", "unknown"] {
            assert!(!ready_execution(&entries, id));
        }
    }
    #[test]
    fn private_or_closed_executions_are_not_navigation_targets() {
        let mut execution = entry("one", "agent", "completed");
        execution.private = true;
        assert!(!ready_execution(&[execution.clone()], "one"));
        execution.private = false;
        execution.closed = true;
        assert!(!ready_execution(&[execution], "one"));
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;
    use core::prelude::v1::test;
    #[test]
    fn restored_and_provider_owned_history_never_accepts_inline_input() {
        let mut entry: Entry = serde_json::from_value(json!({"id":"one","name":"shell","command":"sh","cwd":"/work","status":"running","owner":"user","private":false,"restored":true})).unwrap();
        assert!(!entry.accepts_input(false));
        entry.restored = false;
        entry.route = "externalObserved".into();
        assert!(!entry.accepts_input(false));
        entry.route = "workbench".into();
        assert!(entry.accepts_input(false));
        entry.stop_requested = true;
        assert!(!entry.accepts_input(false));
        assert_eq!(entry.status(), "Stopping");
        entry.status = "unknown".into();
        assert_eq!(entry.status(), "Unknown outcome");
        assert!(entry.exit_code.is_none());
    }
    #[test]
    fn command_review_advances_on_settlement_but_preserves_deliberate_collapse() {
        let entry = |id: &str, owner: &str, status: &str| -> Entry {
            serde_json::from_value(json!({"id":id,"name":"command","command":"echo test",
                "reason":null,"cwd":"/work","status":status,"private":false,"owner":owner,"exitCode":null}))
            .unwrap()
        };
        let previous = vec![
            entry("one", "agent", "awaitingUser"),
            entry("two", "agent", "awaitingUser"),
        ];
        let entries = vec![entry("one", "agent", "running"), entry("two", "agent", "awaitingUser")];
        assert_eq!(
            next_inline_selection(&previous, &entries, Some("one"), true).as_deref(),
            Some("two")
        );
        assert_eq!(next_inline_selection(&previous, &entries, None, true), None);
        assert_eq!(
            next_inline_selection(&[], &previous, None, false).as_deref(),
            Some("one")
        );
        for status in ["awaitingUser", "starting", "running", "providerPending"] {
            assert!(
                entry("active", "agent", status).active(),
                "{status} must keep the inline group mounted"
            );
        }
        for status in ["completed", "failed", "unknown", "declined"] {
            assert!(!entry("ended", "agent", status).active());
        }
    }
}
