//! Native views are replaceable projections; only the matching attachment can issue commands.
use super::super::View;
use super::*;
impl Binding {
    pub(in crate::workspace) fn revoke(&self) {
        self.state.borrow_mut().revoke();
    }
}
impl Workspace {
    pub(in crate::workspace) fn chat_binding_current(&self, cx: &App) -> bool {
        if !self.navigation_allowed() {
            return false;
        }
        self.chat_execution.as_ref().is_some_and(|binding| {
            let state = binding.owner.read(cx).state.clone();
            let mut state = state.borrow_mut();
            self.terminal_identity.as_ref() == Some(&state.identity) && state.permits(binding.incarnation)
        })
    }
    pub(in crate::workspace) fn bind_gateway_chat(
        &mut self,
        owner: Entity<Owner>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let incarnation = {
            let mut state = owner.read(cx).state.borrow_mut();
            state.incarnation = state.incarnation.wrapping_add(1);
            state.incarnation
        };
        self.chat_execution = Some(Binding {
            state: owner.read(cx).state.clone(),
            owner: owner.clone(),
            incarnation,
        });
        self._chat_execution = Some(self.observe_gateway_chat(&owner, cx));
    }
    pub(in crate::workspace) fn ensure_gateway_execution(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(account) = self.account_gateway() else {
            return false;
        };
        if !self.navigation_allowed() || !matches!(self.auth, super::super::Auth::SignedIn) {
            return false;
        }
        let Some(identity) = self.terminal_identity.clone() else {
            return false;
        };
        if !identity.matches_credential(self.gateway.url(), self.gateway.marker().as_deref())
            || self
                .account
                .as_ref()
                .and_then(|account| self.current_identity(account))
                .as_ref()
                != Some(&identity)
        {
            if let Some(binding) = &self.chat_execution {
                binding.revoke();
            }
            self.mark_gateway_revoked(cx);
            return false;
        }
        let Some(authority) = self
            .gateway_account_authority
            .as_ref()
            .filter(|authority| authority.current())
        else {
            if let Some(binding) = &self.chat_execution {
                binding.revoke();
            }
            self.mark_gateway_revoked(cx);
            return false;
        };
        if self.chat_execution.is_none() {
            let Some((gateway, events)) = account.execution_window(authority) else {
                return false;
            };
            let owner = cx.new(|cx| Owner::new(gateway, events, identity, cx));
            let incarnation = owner.read(cx).state.borrow().incarnation;
            self._chat_execution = Some(self.observe_gateway_chat(&owner, cx));
            self.chat_execution = Some(Binding {
                state: owner.read(cx).state.clone(),
                owner,
                incarnation,
            });
        }
        let current = self.chat_binding_current(cx);
        if !current {
            self.mark_gateway_revoked(cx);
        }
        current
    }
    /// Project `owner`'s changes as they come. `observe_in` needs a `Window`, and the first ask often has none (a
    /// fetch's callback): through the workspace's own window, its first tokens do not wait for a frame to subscribe.
    fn observe_gateway_chat(&self, owner: &Entity<Owner>, cx: &mut Context<Self>) -> Subscription {
        let (this, window) = (cx.weak_entity(), self.window_handle);
        let app: &mut App = cx;
        app.observe(owner, move |_, cx| {
            if let Err(error) = window
                .update(cx, |_, window, cx| {
                    this.update(cx, |this, cx| this.sync_gateway_chat(window, cx))
                })
                .and_then(|result| result)
            {
                eprintln!("chat projection window is unavailable: {error:#}");
            }
        })
    }
    pub(in crate::workspace) fn mark_gateway_revoked(&mut self, cx: &mut Context<Self>) {
        self.asking = None;
        self.chat_queue.queues = Queues::default();
        self.chat_queue.sent = None;
        if let Some(entry) = self.entries.last_mut().filter(|entry| entry.streaming) {
            entry.streaming = false;
            entry.writing_page = false;
            entry.searching_web = false;
            entry.stopped = true;
            cx.notify();
        }
    }
    pub(in crate::workspace) fn next_gateway_turn(&mut self, cx: &mut Context<Self>) -> Option<u64> {
        if !self.ensure_gateway_execution(cx) {
            return None;
        }
        let binding = self.chat_execution.as_ref()?;
        let mut state = binding.state.borrow_mut();
        if self
            .chat_execution_at
            .and_then(|key| state.runs.get(&key))
            .is_some_and(|run| run.running)
        {
            return None;
        }
        Some(state.turn())
    }
    pub(in crate::workspace) fn launch_gateway_chat(
        &mut self,
        prompt: String,
        attachments: Draft,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.chat_binding_current(cx) {
            return false;
        }
        let binding = self.chat_execution.as_ref().expect("current chat execution").clone();
        let choices = self.chat_choices();
        let draft = self.detach_draft();
        let key = self.chat_execution_at.unwrap_or(self.turn);
        let run = Run {
            key,
            turn: self.turn,
            id: self.current.clone(),
            title: self.title.clone(),
            entries: self.entries.iter().map(Message::capture).collect(),
            model: self.model.clone(),
            session_web_search: choices.web_search,
            web_search_flag: self.choices.web_search,
            panel: self.panel,
            choices,
            archived: self.chat_archived,
            running: true,
            accepted: false,
            queue_key: self.current.clone().unwrap_or_else(|| format!("pending-chat:{key}")),
            sent: self.chat_queue.sent.clone(),
            edit: self.edit_sent.clone(),
            restore_edit: None,
            finished: None,
            draft,
        };
        self.chat_execution_at = Some(key);
        binding.owner.update(cx, |owner, cx| {
            let mut state = owner.state.borrow_mut();
            if !state.permits(binding.incarnation) {
                return false;
            }
            state.next_entry = state
                .next_entry
                .max(run.entries.iter().map(|entry| entry.id).max().unwrap_or(0));
            state.queues.rename("", &run.queue_key);
            state.launch(&run, prompt, attachments);
            state.runs.insert(key, run);
            let current = state.current();
            cx.notify();
            current
        })
    }
    pub(in crate::workspace) fn refresh_gateway_queues(&mut self, cx: &App) {
        if !self.chat_binding_current(cx) {
            return;
        }
        let binding = self.chat_execution.as_ref().expect("current chat execution");
        let state = binding.owner.read(cx).state.borrow();
        self.chat_queue.queues = state.queues.clone();
        if let Some(run) = self.chat_execution_at.and_then(|key| state.runs.get(&key)) {
            self.chat_queue.sent = run.sent.clone();
        }
    }
    pub(in crate::workspace) fn commit_gateway_queues(&mut self, cx: &mut Context<Self>) {
        if !self.chat_binding_current(cx) {
            return;
        }
        let binding = self.chat_execution.as_ref().expect("current chat execution").clone();
        let queues = self.chat_queue.queues.clone();
        let sent = self.chat_queue.sent.clone();
        let edit = self.edit_sent.clone();
        let at = self.chat_execution_at;
        binding.owner.update(cx, |owner, cx| {
            let mut state = owner.state.borrow_mut();
            if !state.permits(binding.incarnation) {
                return;
            }
            state.queues = queues;
            if let Some(run) = at.and_then(|key| state.runs.get_mut(&key)) {
                run.sent = sent;
                if !run.accepted {
                    run.edit = edit;
                }
            }
            cx.notify();
        });
    }
    pub(in crate::workspace) fn gateway_queue_key(&self) -> Option<String> {
        if !self.navigation_allowed() {
            return None;
        }
        let binding = self.chat_execution.as_ref()?;
        let mut state = binding.state.borrow_mut();
        if !state.permits(binding.incarnation) {
            return None;
        }
        self.chat_execution_at
            .and_then(|key| state.runs.get(&key))
            .map(|run| run.queue_key.clone())
    }
    pub(in crate::workspace) fn capture_gateway_choices(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.view, View::Chat) || !self.chat_binding_current(cx) {
            return;
        }
        let Some(key) = self.chat_execution_at else {
            return;
        };
        let binding = self.chat_execution.as_ref().expect("current execution").clone();
        let model = self.model.clone();
        let baseline = binding.state.borrow().runs.get(&key).map(|run| run.session_web_search);
        let (choices, flag) = self.pending_chat_choices(baseline);
        self.choices.web_search = flag;
        let mut state = binding.state.borrow_mut();
        if !state.permits(binding.incarnation) {
            return;
        }
        if let Some(run) = state.runs.get_mut(&key) {
            run.model = model;
            run.choices = choices;
            run.web_search_flag = flag;
        }
    }
    pub(in crate::workspace) fn capture_gateway_draft(&self) {
        if !matches!(self.view, View::Chat) || !self.navigation_allowed() {
            return;
        }
        let Some(binding) = &self.chat_execution else {
            return;
        };
        let mut state = binding.state.borrow_mut();
        if !state.permits(binding.incarnation) {
            return;
        }
        if let Some(run) = self
            .chat_execution_at
            .and_then(|key| state.runs.get_mut(&key))
            .filter(|run| run.id.is_none())
        {
            run.draft = self.detach_draft();
        }
    }
    pub(in crate::workspace) fn detach_gateway_chat(&mut self) -> bool {
        if !self.navigation_allowed() {
            return false;
        }
        let Some(binding) = self.chat_execution.as_ref() else {
            return false;
        };
        if !binding.state.borrow_mut().permits(binding.incarnation) {
            return false;
        }
        let Some(key) = self.chat_execution_at.take() else {
            return false;
        };
        let draft = self.detach_draft();
        let edit = self.edit_sent.take();
        if let Some(run) = binding.state.borrow_mut().runs.get_mut(&key) {
            run.draft = draft;
            if !run.accepted {
                run.edit = edit;
            }
        }
        self.asking = None;
        self.chat_queue.sent = None;
        self.chat_queue.editing = None;
        self.chat_projected = None;
        true
    }
    pub(in crate::workspace) fn resume_gateway_chat(
        &mut self,
        id: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.chat_binding_current(cx) {
            return false;
        }
        let binding = self.chat_execution.as_ref().expect("current execution");
        let key = binding
            .owner
            .read(cx)
            .state
            .borrow()
            .runs
            .values()
            .filter(|run| run.id.as_deref() == id)
            .map(|run| run.key)
            .max();
        let Some(key) = key else {
            return false;
        };
        self.chat_execution_at = Some(key);
        self.sync_gateway_chat(window, cx);
        true
    }
    pub(in crate::workspace) fn sync_gateway_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.chat_binding_current(cx) {
            if self.navigation_allowed()
                && self
                    .chat_execution
                    .as_ref()
                    .is_some_and(|binding| binding.state.borrow().revoked)
            {
                self.mark_gateway_revoked(cx);
            }
            return;
        }
        let binding = self.chat_execution.as_ref().expect("current execution").clone();
        let (run, queues, rewrite, next_entry, named) = {
            let state = binding.owner.read(cx).state.borrow();
            (
                self.chat_execution_at
                    .and_then(|key| state.runs.get(&key))
                    .map(Run::without_entries),
                state.queues.clone(),
                state.rewrite.clone(),
                state.next_entry,
                state
                    .runs
                    .values()
                    .filter(|run| {
                        run.id
                            .as_ref()
                            .is_some_and(|id| !self.sessions.iter().any(|session| session.id == *id))
                    })
                    .filter_map(|run| {
                        run.id.as_ref().map(|id| crate::model::SessionSummary {
                            id: id.clone(),
                            title: run.title.to_string(),
                            day: Some(crate::model::today()),
                            at_ms: Some(crate::code::now_ms()),
                        })
                    })
                    .collect::<Vec<_>>(),
            )
        };
        // What the sidebar and the tray show, and the message count, to tell a streamed delta (the main area alone
        // draws again) from a change. A new message, a new or renamed session, or a finished turn draws everything.
        let shown_before = (
            self.sessions.len(),
            self.current.clone(),
            self.asking,
            self.title.clone(),
            self.entries.len(),
        );
        let mut streamed = run
            .as_ref()
            .is_some_and(|run| run.finished.is_none() && run.restore_edit.is_none())
            && rewrite.is_none();
        for summary in named {
            if !self.sessions.iter().any(|session| session.id == summary.id) {
                self.sessions.insert(0, summary);
            }
        }
        self.chat_queue.queues = queues;
        self.next_entry = self.next_entry.max(next_entry);
        if let Some(run) = run.filter(|_| matches!(self.view, View::Chat)) {
            let old_id = self.current.clone();
            self.current = run.id.clone();
            self.title = run.title;
            self.asking = run.running.then_some(run.turn);
            self.turn = run.turn;
            self.model = run.model;
            self.choices.team = Some(run.choices.team.unwrap_or_default());
            self.choices.web_search = run.web_search_flag;
            self.choices.session_web_search = Some(run.session_web_search);
            self.chat_archived = run.archived;
            self.chat_queue.sent = run.sent;
            self.edit_sent = run.edit;
            self.opening = false;
            let follow = super::super::transcript::near_bottom(&self.scroll);
            let shown = self.project_gateway_entries(&binding.state, run.key, cx);
            if follow && shown.grew {
                self.scroll.scroll_to_bottom();
            }
            if shown.streamed {
                self.measure_delta(run.turn);
            }
            if old_id != self.current {
                if let Some(id) = self.current.clone() {
                    self.file_new_chat(&id);
                }
            }
            // Hydrate this source's global composer slot before merging an unaccepted send receipt.
            self.sync_draft(window, cx);
            if let Some((turn, success, page)) = run.finished {
                binding
                    .state
                    .borrow_mut()
                    .runs
                    .get_mut(&run.key)
                    .expect("projected run")
                    .finished = None;
                if let Some(account) = self.account_gateway() {
                    account.list_sessions();
                    account.load_pays();
                }
                if let Some(id) = self.current.clone() {
                    self.read_chat(id.clone());
                    if success && let Some(account) = self.account_gateway() {
                        account.suggest(id);
                    }
                }
                if success {
                    if let Some(page) = page {
                        self.open_page_block(&page, true, cx);
                    }
                }
                self.measure_answered(turn, success, window, cx);
            }
            if let Some(edit) = run.restore_edit {
                binding
                    .state
                    .borrow_mut()
                    .runs
                    .get_mut(&run.key)
                    .expect("projected run")
                    .restore_edit = None;
                self.put_back_prompt(&edit.text, edit.draft, window, cx);
            }
            if let Some((key, seq, text)) = rewrite.filter(|(key, _, _)| run.queue_key == *key) {
                binding.state.borrow_mut().rewrite = None;
                let _ = key;
                self.restore_gateway_rewrite(seq, text, window, cx);
            }
            self.await_first_token(window, cx);
        } else {
            // The entries may change while no run shows: the next projection applies every message.
            self.chat_projected = None;
            streamed = false;
        }
        if streamed
            && shown_before
                == (
                    self.sessions.len(),
                    self.current.clone(),
                    self.asking,
                    self.title.clone(),
                    self.entries.len(),
                )
        {
            self.notify_main(cx);
        } else {
            cx.notify();
        }
    }
    pub(in crate::workspace) fn gateway_chat_metadata(
        &mut self,
        id: &str,
        chat: &crate::gateway::OpenedChat,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.chat_binding_current(cx) {
            return false;
        }
        let Some(key) = self.chat_execution_at else {
            return false;
        };
        let binding = self.chat_execution.as_ref().expect("current execution").clone();
        let mut state = binding.state.borrow_mut();
        let Some(run) = state.runs.get_mut(&key) else {
            return false;
        };
        if run.id.as_deref() != Some(id) {
            return false;
        }
        // Stored history may still end at A while the queue is already streaming B.
        if run.running {
            return true;
        }
        let local = run
            .entries
            .iter()
            .map(|entry| (entry.role, entry.meta != crate::model::Meta::default()))
            .collect::<Vec<_>>();
        for (entry, meta) in run
            .entries
            .iter_mut()
            .zip(crate::model::meta_for(&local, &chat.messages))
        {
            if let Some(meta) = meta {
                entry.meta = meta;
                entry.touch();
            }
        }
        for (entry, data) in self.entries.iter_mut().zip(&run.entries) {
            data.apply(entry, cx);
        }
        let durable = !run.running
            && run.restore_edit.is_none()
            && run.finished.is_none()
            && run
                .entries
                .last()
                .is_some_and(|entry| !entry.streaming && !entry.stopped && entry.error.is_none());
        if durable {
            // A successful authoritative read takes over after the terminal receipt was presented.
            // Future navigation/edits must read Gateway instead of resurrecting an old completed model.
            state.runs.remove(&key);
            self.chat_execution_at = None;
        }
        cx.notify();
        true
    }
    pub(in crate::workspace) fn stop_gateway_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.chat_binding_current(cx) {
            return false;
        }
        let Some(key) = self.chat_execution_at else {
            return false;
        };
        let binding = self.chat_execution.as_ref().expect("execution").clone();
        let stopped = binding.owner.update(cx, |owner, cx| {
            let mut state = owner.state.borrow_mut();
            let Some(mut run) = state.runs.remove(&key) else {
                return false;
            };
            let stopped = run.running;
            if stopped {
                state.backend.stop(run.turn);
                state.launched.remove(&run.turn);
                state.settle(&mut run, Outcome::Stopped);
            }
            state.runs.insert(key, run);
            cx.notify();
            stopped
        });
        if stopped {
            self.sync_gateway_chat(window, cx);
        }
        stopped
    }
}
