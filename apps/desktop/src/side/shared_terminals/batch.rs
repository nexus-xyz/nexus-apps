//! Bulk decisions target a rendered snapshot, then recheck each request before sending it.
use super::*;

#[derive(Clone)]
pub(super) struct PendingDecision {
    id: String,
    command: String,
    expected: String,
}

impl PendingDecision {
    fn current(&self, entries: &[Entry]) -> bool {
        entries
            .iter()
            .any(|entry| entry.id == self.id && entry.needs_attention() && entry.command == self.expected)
    }
    fn args(&self) -> Value {
        json!({"id":self.id,"command":self.command,"expected_command":self.expected})
    }
}

pub(super) fn snapshot(entries: &[Entry]) -> Vec<PendingDecision> {
    newest_first(entries)
        .into_iter()
        .filter(|entry| entry.needs_attention())
        .map(|entry| PendingDecision {
            id: entry.id.clone(),
            command: entry.command.clone(),
            expected: entry.command.clone(),
        })
        .collect()
}

impl SharedTerminals {
    pub(super) fn render_bulk_actions(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let pending = snapshot(&self.entries);
        let mut actions = h_flex().flex_none().min_h(px(32.)).gap(px(8.));
        if !self.read_only && !pending.is_empty() {
            if self.busy {
                actions = actions.child(ui::note("Applying decisions…", palette));
            } else {
                let declined = pending.clone();
                actions = actions
                    .child(
                        ui::ghost_button("commands-decline-all", "Decline all", palette).on_click(cx.listener(
                            move |this, _, _, cx| this.perform_pending_batch("decline", declined.clone(), cx),
                        )),
                    )
                    .child(ui::primary_button("commands-run-all", "Run all", palette).on_click(
                        cx.listener(move |this, _, _, cx| this.perform_pending_batch("execute", pending.clone(), cx)),
                    ));
            }
        }
        actions.into_any_element()
    }

    pub(super) fn perform_pending_batch(
        &mut self,
        op: &'static str,
        mut pending: Vec<PendingDecision>,
        cx: &mut Context<Self>,
    ) {
        if self.busy || self.read_only || pending.is_empty() {
            return;
        }
        if op == "execute"
            && let Some((id, input)) = &self.editing
        {
            if let Some(decision) = pending.iter_mut().find(|decision| &decision.id == id) {
                decision.command = input.read(cx).value().to_string();
            }
        }
        if !self.is_current() {
            return;
        }
        let (current, changed): (Vec<_>, Vec<_>) = pending
            .into_iter()
            .partition(|decision| decision.current(&self.entries));
        let errors = changed
            .iter()
            .map(|_| "A command changed or was already decided. Review the remaining requests.".to_string())
            .collect();
        let progress = Rc::new(std::cell::RefCell::new(receipts::BatchProgress::new(
            current.len(),
            errors,
        )));
        self.error = progress.borrow().summary();
        // Each rendered request is admitted once. Its receipt survives presentation retirement.
        for decision in current {
            self.busy = false;
            self.perform_with_batch(op, decision.args(), Some(progress.clone()), cx);
        }
        self.busy = self.pending_actions > 0;
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    fn pending(id: &str) -> Entry {
        serde_json::from_value(
            json!({"id":id,"name":"Command","command":"pwd","cwd":"/repo","status":"awaitingUser","private":false}),
        )
        .unwrap()
    }
    #[test]
    fn snapshot_excludes_new_arrivals_and_requests_that_are_not_pending() {
        let mut entries = vec![pending("one"), pending("two"), pending("active")];
        entries[2].status = "running".into();
        let batch = snapshot(&entries);
        entries.push(pending("new"));
        assert_eq!(batch.len(), 2);
        assert!(
            batch
                .iter()
                .all(|decision| decision.id != "new" && decision.current(&entries))
        );
        entries[0].status = "declined".into();
        assert!(
            !batch
                .iter()
                .find(|decision| decision.id == "one")
                .unwrap()
                .current(&entries)
        );
    }
    #[test]
    fn edited_commands_preserve_the_expected_original_and_changed_requests_are_rejected() {
        let mut entries = vec![pending("one")];
        let mut decision = snapshot(&entries).remove(0);
        decision.command = "pwd -P".into();
        assert!(decision.current(&entries));
        assert_eq!(decision.args()["expected_command"], "pwd");
        assert_eq!(decision.args()["command"], "pwd -P");
        entries[0].command = "git status".into();
        assert!(!decision.current(&entries));
        entries[0].command = "pwd".into();
        entries[0].restored = true;
        assert!(!decision.current(&entries));
    }
}
