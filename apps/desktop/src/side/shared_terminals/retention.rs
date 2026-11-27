//! Window-free command presentation and independently owned operation receipts.
use super::super::terminal::RetainedSharedTerminal;
use super::*;
use std::sync::{Arc, atomic::Ordering};

pub(crate) struct RetainedSharedTerminals {
    runner: Rc<CodeRunner>,
    dispatch: Dispatch,
    thread: String,
    owner_guard: Guard,
    owner_fence: Arc<std::sync::Mutex<Guard>>,
    editing: Option<(String, String)>,
    outputs: HashMap<String, (RetainedSharedTerminal, u64)>,
    inline_open: bool,
    pending_rows: approval::PendingRows,
    inline_limit: f32,
    inline_selected: Option<String>,
    inline_initialized: bool,
    show_history: bool,
    history_allowed: bool,
    delete_confirmation: Option<Vec<String>>,
    focus_output: Option<String>,
    pending_input: Rc<std::cell::RefCell<Vec<crate::code_run::Answer<Value>>>>,
    entries: Vec<Entry>,
    selected: Option<String>,
    collapsed: HashSet<String>,
    inline_expanded: HashSet<String>,
    action_error: Option<(String, String)>,
    submitted_inputs: HashMap<String, (u64, Vec<String>)>,
    expanded: bool,
    busy: bool,
    error: Option<String>,
    monitor: bool,
    read_only: bool,
    registry_revision: u64,
    query: String,
    seen: Option<HashSet<String>>,
    arrived: HashMap<String, Instant>,
    changed: HashMap<String, Instant>,
    accepted: HashSet<String>,
    finished: Vec<finished::Finished>,
    reveal: Option<String>,
    open_shell: Option<String>,
    receipts: Rc<std::cell::RefCell<Vec<receipts::Receipt>>>,
    pending_actions: usize,
    undo_ids: Vec<String>,
}

impl SharedTerminals {
    pub(crate) fn is_current(&self) -> bool {
        self.presentation.load(Ordering::SeqCst) && (self.owner_guard)() && self.presentation.load(Ordering::SeqCst)
    }

    /// Freeze the admitted source proof before giving this presentation to an owner.
    /// Conjunction prevents replacing an old proof with a newer source epoch.
    pub(crate) fn set_owner_guard(&mut self, guard: Guard) {
        let mut fence = self.owner_fence.lock().expect("Commands continuation fence");
        let previous = fence.clone();
        *fence = Arc::new(move || previous() && guard());
    }

    fn presentation_guard(&self) -> Guard {
        let alive = self.presentation.clone();
        let owner = self.owner_guard.clone();
        Arc::new(move || alive.load(Ordering::SeqCst) && owner() && alive.load(Ordering::SeqCst))
    }

    pub(super) fn terminal(&self, thread: String, op: String, args: Value) -> crate::code_run::Answer<Value> {
        (self.dispatch)(thread, op, args, self.presentation_guard())
    }

    pub(super) fn output_sender(&self, execution: String) -> Rc<dyn Fn(&str, Value)> {
        let valid = self.presentation_guard();
        let owner = self.owner_guard.clone();
        let dispatch = self.dispatch.clone();
        let thread = self.thread.clone();
        let pending = self.pending_input.clone();
        let read_only = self.read_only;
        Rc::new(move |op, mut args| {
            if read_only || !valid() {
                return;
            }
            args["id"] = json!(execution);
            // Admission belongs to this presentation; the resulting receipt belongs to the owner.
            pending
                .borrow_mut()
                .push(dispatch(thread.clone(), op.into(), args, owner.clone()));
        })
    }

    pub(crate) fn retire(&mut self, cx: &mut Context<Self>) {
        self.presentation.store(false, Ordering::SeqCst);
        self.poll = None;
        self.polling = false;
        self.monitor = false;
        self._search_events = None;
        self.search = None;
        // Retired strong terminal entity clones are inert too.
        for output in self.outputs.values() {
            output.view.update(cx, |view, _| {
                let _ = view.retain_shared();
            });
        }
        self.outputs.clear();
        cx.notify();
    }

    pub(crate) fn retain(&mut self, cx: &mut Context<Self>) -> RetainedSharedTerminals {
        let editing = self
            .editing
            .take()
            .map(|(id, input)| (id, input.read(cx).value().to_string()));
        let outputs = std::mem::take(&mut self.outputs)
            .into_iter()
            .map(|(id, output)| {
                let saved = output.view.update(cx, |view, _| view.retain_shared());
                (id, (saved, output.cursor))
            })
            .collect();
        for card in &mut self.finished {
            card.set_hover(false, Instant::now());
        }
        let saved = RetainedSharedTerminals {
            runner: self.runner.clone(),
            dispatch: self.dispatch.clone(),
            thread: self.thread.clone(),
            owner_guard: self.owner_guard.clone(),
            owner_fence: self.owner_fence.clone(),
            editing,
            outputs,
            inline_open: std::mem::take(&mut self.inline_open),
            pending_rows: std::mem::take(&mut self.pending_rows),
            inline_limit: std::mem::take(&mut self.inline_limit),
            inline_selected: std::mem::take(&mut self.inline_selected),
            inline_initialized: std::mem::take(&mut self.inline_initialized),
            show_history: std::mem::take(&mut self.show_history),
            history_allowed: std::mem::take(&mut self.history_allowed),
            delete_confirmation: std::mem::take(&mut self.delete_confirmation),
            focus_output: std::mem::take(&mut self.focus_output),
            pending_input: std::mem::take(&mut self.pending_input),
            entries: std::mem::take(&mut self.entries),
            selected: std::mem::take(&mut self.selected),
            collapsed: std::mem::take(&mut self.collapsed),
            inline_expanded: std::mem::take(&mut self.inline_expanded),
            action_error: std::mem::take(&mut self.action_error),
            submitted_inputs: std::mem::take(&mut self.submitted_inputs),
            expanded: std::mem::take(&mut self.expanded),
            busy: std::mem::take(&mut self.busy),
            error: std::mem::take(&mut self.error),
            monitor: std::mem::take(&mut self.monitor),
            read_only: std::mem::take(&mut self.read_only),
            registry_revision: std::mem::take(&mut self.registry_revision),
            query: std::mem::take(&mut self.query),
            seen: std::mem::take(&mut self.seen),
            arrived: std::mem::take(&mut self.arrived),
            changed: std::mem::take(&mut self.changed),
            accepted: std::mem::take(&mut self.accepted),
            finished: std::mem::take(&mut self.finished),
            reveal: std::mem::take(&mut self.reveal),
            open_shell: std::mem::take(&mut self.open_shell),
            receipts: std::mem::take(&mut self.receipts),
            pending_actions: std::mem::take(&mut self.pending_actions),
            undo_ids: std::mem::take(&mut self.undo_ids),
        };
        self.retire(cx);
        saved
    }

    pub(crate) fn restore(
        saved: RetainedSharedTerminals,
        guard: Guard,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self::new_dispatched(
            saved.runner,
            saved.dispatch,
            saved.thread,
            saved.owner_guard.clone(),
            window,
            cx,
        );
        view.owner_guard = saved.owner_guard;
        view.owner_fence = saved.owner_fence;
        view.set_owner_guard(guard);
        view.inline_open = saved.inline_open;
        view.pending_rows = saved.pending_rows;
        view.inline_limit = saved.inline_limit;
        view.inline_selected = saved.inline_selected;
        view.inline_initialized = saved.inline_initialized;
        view.show_history = saved.show_history;
        view.history_allowed = saved.history_allowed;
        view.delete_confirmation = saved.delete_confirmation;
        view.focus_output = saved.focus_output;
        view.pending_input = saved.pending_input;
        view.entries = saved.entries;
        view.selected = saved.selected;
        view.collapsed = saved.collapsed;
        view.inline_expanded = saved.inline_expanded;
        view.action_error = saved.action_error;
        view.submitted_inputs = saved.submitted_inputs;
        view.expanded = saved.expanded;
        view.busy = saved.busy;
        view.error = saved.error;
        view.monitor = saved.monitor;
        view.read_only = saved.read_only;
        view.registry_revision = saved.registry_revision;
        view.query = saved.query;
        view.seen = saved.seen;
        view.arrived = saved.arrived;
        view.changed = saved.changed;
        view.accepted = saved.accepted;
        view.finished = saved.finished;
        view.reveal = saved.reveal;
        view.open_shell = saved.open_shell;
        view.receipts = saved.receipts;
        view.pending_actions = saved.pending_actions;
        view.undo_ids = saved.undo_ids;
        view.editing = saved.editing.map(|(id, text)| {
            (
                id,
                cx.new(|cx| TextareaState::new(window, cx).rows(4).default_value(text)),
            )
        });
        for (id, (display, cursor)) in saved.outputs {
            let send = view.output_sender(id.clone());
            let terminal = cx.new(|cx| Terminal::restore_shared(display, send, cx));
            view.outputs.insert(id, Output { view: terminal, cursor });
        }
        view.last_draw = Instant::now();
        view.start_poll(cx);
        view
    }
}
