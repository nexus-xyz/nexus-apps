//! One creation receipt survives closure before its side-chat UI exists.
use super::operations::Pending;
use super::*;
use futures::StreamExt;
use std::rc::Rc;

#[derive(Default)]
pub(in crate::workspace) struct Creation {
    pending: Pending<Created>,
    parked: bool,
}
struct Created {
    parent: String,
    parent_draft: String,
    prompt: Option<String>,
    /// What the new side chat's composer holds once it opens (a quoted passage), when no prompt went.
    draft: String,
    source: crate::workspace::runs::Source,
    title: String,
    result: Result<(ThreadSummary, Option<String>), String>,
}
impl Creation {
    pub(in crate::workspace) fn revoke(&self) {
        self.pending.revoke();
    }
    pub(in crate::workspace) fn detach(&mut self) {
        self.parked = true;
        self.pending.detach();
    }
}
impl Workspace {
    pub(super) fn side_chat_runs(&self, parent: &str) -> Option<Runs> {
        #[cfg(test)]
        if let Some(runs) = &self.side.side_chat_fixture_runs {
            return Some(runs.clone());
        }
        self.runs(parent)
    }
    pub(super) fn side_chat_guard(&self) -> Rc<dyn Fn() -> bool> {
        #[cfg(test)]
        if let Some(guard) = &self.side.side_chat_fixture_guard {
            return guard.clone();
        }
        let gateway = self.session_account_gateway();
        let identity = self.account.as_ref().and_then(|account| self.current_identity(account));
        Rc::new(move || {
            identity.as_ref().is_some_and(|identity| {
                gateway
                    .as_ref()
                    .is_some_and(|gateway| gateway.matches_identity(&identity))
            })
        })
    }
    pub(in crate::workspace) fn create_side_chat(
        &mut self,
        prompt: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.start_side_chat(prompt, String::new(), window, cx);
    }

    /// Open a new side chat with `draft` waiting in its composer, unsent (a passage quoted from the toolbar).
    pub(in crate::workspace) fn create_side_chat_with_draft(
        &mut self,
        draft: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.start_side_chat(None, draft, window, cx);
    }

    fn start_side_chat(&mut self, prompt: Option<String>, draft: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(reason) = self.session_account_control_reason() {
            self.composer_note = Some(reason.into());
            cx.notify();
            return;
        }
        let View::Code(view) = &self.view else { return };
        if self.side.side_chat_retired || self.side.side_chat_busy || self.side.side_chat_creation.pending.occupied() {
            return;
        }
        let parent = view.id.clone();
        let Some(runs) = self.side_chat_runs(&parent) else {
            self.side.side_chat_error = Some("Take over the store to open a side chat.".into());
            cx.notify();
            return;
        };
        let guard = self.side_chat_guard();
        self.side.side_chat_creation.pending.guard(guard);
        if !self.side.side_chat_creation.pending.valid() {
            return;
        }
        self.side.connection = None;
        self.side.side_chat_error = None;
        self.watch_side_chat_creation(window, cx);
        let parent_draft = self.composer.read(cx).value().to_string();
        let title = self
            .code_session()
            .map(|session| session.title.clone())
            .unwrap_or_else(|| "Parent session".into());
        let source = runs.source();
        let permit = self.side.side_chat_creation.pending.permit();
        let started = self.side.side_chat_creation.pending.start(
            async move {
                let result = async {
                    permit.check()?;
                    let value = runs
                        .call("wb_side_chat", json!({"threadId":parent}))
                        .await
                        .unwrap_or_else(|_| {
                            Err("The side chat's creation answer was lost. Refresh before trying again.".into())
                        })?;
                    let side: ThreadSummary = serde_json::from_value(value).map_err(|error| error.to_string())?;
                    if !side.side_chat || side.source_thread_id.as_deref() != Some(parent.as_str()) || side.id == parent
                    {
                        return Err("Nexus returned a side chat for a different session.".into());
                    }
                    // The originally authorized /side question sends once, even while the UI is parked.
                    let send_error = if let Some(prompt) = &prompt {
                        permit.check()?;
                        runs.call(
                            "wb_send_files",
                            json!({"threadId":side.id,"text":prompt,"images":[],"files":[]}),
                        )
                        .await
                        .unwrap_or_else(|_| {
                            Err("The first message's answer was lost. Read the side chat before retrying.".into())
                        })
                        .err()
                    } else {
                        None
                    };
                    Ok((side, send_error))
                }
                .await;
                Created {
                    parent,
                    parent_draft,
                    prompt,
                    draft,
                    source,
                    title,
                    result,
                }
            },
            cx,
        );
        if started {
            self.side.side_chat_creation.parked = false;
        }
        self.side.side_chat_busy = started;
        cx.notify();
    }
    pub(super) fn watch_side_chat_creation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut changes = self.side.side_chat_creation.pending.subscribe();
        self.side.side_chat_busy = self.side.side_chat_creation.pending.occupied();
        self.side.side_chat_creation_watch = Some(cx.spawn_in(window, async move |this, cx| {
            while changes.next().await.is_some() {
                if this
                    .update_in(cx, |this, window, cx| this.apply_side_chat_creation(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
    }
    fn apply_side_chat_creation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.side.side_chat_retired || !self.side.side_chat_creation.pending.valid() {
            return;
        }
        let Some(created) = self.side.side_chat_creation.pending.take() else {
            return;
        };
        self.side.side_chat_busy = false;
        match created.result {
            Err(error) => {
                self.side.side_chat_error = Some(error.clone());
                self.composer_note = Some(error.into());
            }
            Ok((summary, error)) => {
                let parent = created.parent;
                let id = summary.id.clone();
                // A first message that went shows at once; one that did not waits in the composer.
                let (input, sent) = if error.is_some() {
                    (created.prompt.unwrap_or_default(), None)
                } else {
                    (created.draft, created.prompt)
                };
                let saved = super::retention::RetainedChat::created(
                    summary,
                    created.title,
                    created.source,
                    input,
                    sent,
                    error,
                    self.side_chat_guard(),
                    !self.side.side_chat_creation.parked,
                );
                self.side.side_chat_for.insert(parent.clone(), id.clone());
                self.side.side_chat_waiting.insert(id, saved);
                if matches!(&self.view,View::Code(view) if view.id==parent) {
                    self.side.side_chat_visible = true;
                    self.side.open = true;
                    if side_question(&created.parent_draft).is_some()
                        && self.composer.read(cx).value().as_ref() == created.parent_draft
                    {
                        self.composer.update(cx, |input, cx| input.set_value("", window, cx));
                        self.composer_empty = true;
                    }
                }
                self.adopt_side_chats(window, cx);
                self.sync_side_chat(window, cx);
            }
        }
        cx.notify();
    }
}
