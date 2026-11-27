//! Admitted operations finish independently of the presentation that admitted them.
use super::*;

/// Shared only by admitted decisions and their receipts; no presentation or authority.
pub(super) struct BatchProgress {
    remaining: usize,
    errors: Vec<String>,
}
pub(super) type Batch = Rc<std::cell::RefCell<BatchProgress>>;

impl BatchProgress {
    pub(super) fn new(remaining: usize, errors: Vec<String>) -> Self {
        Self { remaining, errors }
    }

    pub(super) fn summary(&self) -> Option<String> {
        (self.remaining == 0 && !self.errors.is_empty()).then(|| {
            format!(
                "{} decision(s) could not be completed: {}",
                self.errors.len(),
                self.errors[0]
            )
        })
    }

    fn complete(&mut self, result: &Result<Value, String>) -> Option<String> {
        self.remaining = self.remaining.saturating_sub(1);
        if let Err(error) = result {
            self.errors.push(error.clone());
        }
        self.summary()
    }
}

pub(super) enum Receipt {
    Action {
        op: &'static str,
        target: Option<String>,
        result: Result<Value, String>,
        shell: bool,
        batch: Option<Batch>,
        /// A setup fix's token (`watch.rs`).
        watch: Option<u64>,
    },
    Delete {
        deleted: Vec<String>,
        error: Option<String>,
    },
    Undo {
        failed: Vec<String>,
        error: Option<String>,
    },
}

impl SharedTerminals {
    pub(super) fn drain_receipts(&mut self, cx: &mut Context<Self>) {
        if !self.is_current() {
            return;
        }
        let waiting = std::mem::take(&mut *self.pending_input.borrow_mut());
        for mut answer in waiting {
            match answer.try_recv() {
                Ok(Some(Ok(_))) => {}
                Ok(Some(Err(error))) => self.error = Some(error),
                Ok(None) => self.pending_input.borrow_mut().push(answer),
                Err(_) => self.error = Some("The session runner stopped.".into()),
            }
        }
        let receipts = std::mem::take(&mut *self.receipts.borrow_mut());
        for receipt in receipts {
            self.pending_actions = self.pending_actions.saturating_sub(1);
            match receipt {
                Receipt::Action {
                    op,
                    target,
                    result,
                    shell,
                    batch,
                    watch,
                } => {
                    if shell {
                        self.open_shell = result.as_ref().ok().and_then(|v| v["id"].as_str().map(str::to_owned));
                    }
                    if let Some(token) = watch {
                        self.watch_started(token, &result, cx);
                    }
                    let summary = batch
                        .as_ref()
                        .and_then(|progress| progress.borrow_mut().complete(&result));
                    self.apply_action(op, target, result, batch.is_none(), cx);
                    if let Some(summary) = summary {
                        self.error = Some(summary);
                    }
                }
                Receipt::Delete { deleted, error } => {
                    self.entries.retain(|e| !deleted.contains(&e.id));
                    if !deleted.is_empty() {
                        self.undo_ids = deleted.clone();
                        cx.emit(SharedTerminalsEvent::Deleted(deleted));
                    }
                    self.error = error;
                }
                Receipt::Undo { failed, error } => {
                    if let Some(error) = error {
                        self.undo_ids = failed.clone();
                        cx.emit(SharedTerminalsEvent::RestoreFailed(failed, error));
                    }
                    self.show_history = true;
                }
            }
        }
        self.busy = self.pending_actions > 0;
    }

    fn apply_action(
        &mut self,
        op: &'static str,
        target: Option<String>,
        result: Result<Value, String>,
        report_error: bool,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(value) => {
                if op == "review_again" {
                    self.show_history = false;
                    self.selected = value["id"].as_str().map(str::to_owned);
                }
                if matches!(op, "execute" | "take") {
                    if let Ok(entry) = serde_json::from_value::<Entry>(value.clone()) {
                        let id = entry.id.clone();
                        if op == "execute" {
                            self.accepted.insert(id.clone());
                        }
                        let known = self.entries.iter().any(|e| e.id == id);
                        if let Some(existing) = self.entries.iter_mut().find(|e| e.id == id) {
                            *existing = entry;
                        } else {
                            self.entries.push(entry);
                        }
                        // A shell the person opened arrives open, to type in; any other new command arrives
                        // closed, and a known one keeps its state when it is accepted or taken.
                        if self.open_shell.as_deref() == Some(id.as_str()) {
                            self.collapsed.remove(&id);
                        } else if !known {
                            self.collapsed.insert(id.clone());
                        }
                        self.inline_selected = Some(id.clone());
                        self.inline_open = true;
                        self.selected = Some(id.clone());
                        if let Some(output) = self.outputs.get(&id) {
                            output
                                .view
                                .update(cx, |view, cx| view.feed(&[], !self.read_only, false, cx));
                        }
                        self.focus_output = Some(id);
                        self.expanded = true;
                        self.show_history = false;
                    }
                }
                if op == "delete" {
                    if let Some(id) = target.as_ref() {
                        self.undo_ids = vec![id.clone()];
                        cx.emit(SharedTerminalsEvent::Deleted(vec![id.clone()]));
                        self.entries.retain(|e| &e.id != id);
                    }
                }
                if op == "decline" {
                    if let Some(entry) = self.entries.iter_mut().find(|e| Some(&e.id) == target.as_ref()) {
                        entry.status = "declined".into();
                    }
                }
                self.editing = None;
            }
            Err(error) => {
                if op == "execute"
                    && let Some(id) = target.as_ref()
                {
                    self.accepted.remove(id);
                }
                if report_error {
                    self.error = Some(error);
                }
            }
        }
    }
}
