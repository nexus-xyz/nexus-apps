//! Move-only parked authority; a retained lease revokes synchronously when discarded.
use super::super::View;
use super::*;
impl Workspace {
    pub(in crate::workspace) fn revoke_gateway_chat(&mut self) {
        if let Some(binding) = self.chat_execution.take() {
            binding.revoke();
            self.asking = None;
            self.chat_queue.queues = Queues::default();
            self.chat_queue.sent = None;
            self.chat_queue.editing = None;
            self.edit_sent = None;
            if let Some(entry) = self.entries.last_mut().filter(|entry| entry.streaming) {
                entry.streaming = false;
                entry.writing_page = false;
                entry.searching_web = false;
                entry.stopped = true;
            }
        }
        self._chat_execution = None;
        self.chat_execution_at = None;
    }
    /// Native closure ends this attachment even when a stale callback retains its Workspace.
    /// A primary owner already moved into Retained has no binding here and keeps running.
    pub(crate) fn retire_gateway_chat_view(&mut self) {
        self.revoke_gateway_chat();
        self.navigation_retired = true;
        self.gateway_account_authority = None;
        self.pending_account_authority = None;
    }
    pub(in crate::workspace) fn retain_gateway_chat(&mut self, cx: &mut Context<Self>) -> Option<Retained> {
        if !self.chat_binding_current(cx) {
            self.revoke_gateway_chat();
            return None;
        }
        let binding = self.chat_execution.take()?;
        let selected = matches!(self.view, View::Chat)
            .then_some(self.chat_execution_at)
            .flatten();
        let draft = self.detach_draft();
        let mut state = binding.state.borrow_mut();
        if !state.permits(binding.incarnation) {
            return None;
        }
        if let Some(run) = selected.and_then(|key| state.runs.get_mut(&key)) {
            run.draft = draft;
        }
        state.rewrite = self.chat_queue.editing.and_then(|seq| {
            let key = selected
                .and_then(|key| state.runs.get(&key))
                .map(|run| run.queue_key.clone())?;
            Some((key, seq, self.gateway_rewrite_text(cx)))
        });
        state.incarnation = state.incarnation.wrapping_add(1);
        drop(state);
        self._chat_execution = None;
        self.chat_execution_at = None;
        Some(Retained {
            owner: Some(binding.owner),
            state: binding.state,
            selected,
        })
    }
    pub(in crate::workspace) fn adopt_gateway_chat(
        &mut self,
        mut saved: Option<Retained>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<u64> {
        let saved = saved.as_mut()?;
        let valid = {
            let mut state = saved.state.borrow_mut();
            state.current() && Some(&state.identity) == self.terminal_identity.as_ref()
        };
        if !valid {
            return None;
        }
        let owner = saved.owner.take()?;
        let selected = saved.selected;
        self.bind_gateway_chat(owner, window, cx);
        selected
    }
    pub(in crate::workspace) fn restore_gateway_selection(
        &mut self,
        selected: Option<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(self.view, View::Chat) || !self.chat_binding_current(cx) {
            return;
        }
        let Some(key) = selected else {
            return;
        };
        let Some(binding) = &self.chat_execution else {
            return;
        };
        if !binding.state.borrow().runs.contains_key(&key) {
            return;
        }
        self.chat_execution_at = Some(key);
        self.sync_gateway_chat(window, cx);
    }
    pub(in crate::workspace) fn gateway_chat_answering(&self, id: &str) -> bool {
        if !self.navigation_allowed() {
            return false;
        }
        self.chat_execution.as_ref().is_some_and(|binding| {
            let mut state = binding.state.borrow_mut();
            state.permits(binding.incarnation)
                && state
                    .runs
                    .values()
                    .any(|run| run.running && run.id.as_deref() == Some(id))
        })
    }
    pub(in crate::workspace) fn cancel_gateway_chat(&mut self, id: &str) -> bool {
        if !self.navigation_allowed() {
            return false;
        }
        let Some(binding) = &self.chat_execution else {
            return false;
        };
        let mut state = binding.state.borrow_mut();
        if !state.permits(binding.incarnation) {
            return false;
        }
        let keys = state
            .runs
            .iter()
            .filter(|(_, run)| run.id.as_deref() == Some(id))
            .map(|(key, _)| *key)
            .collect::<Vec<_>>();
        for key in keys {
            if let Some(run) = state.runs.remove(&key) {
                if run.running {
                    state.backend.stop(run.turn);
                    state.launched.remove(&run.turn);
                }
            }
        }
        state.queues.clear(id);
        self.chat_queue.queues.clear(id);
        true
    }
}
