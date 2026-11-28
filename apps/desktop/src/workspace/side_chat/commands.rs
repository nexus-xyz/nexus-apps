//! Existing Workbench mutations, with results owned independently of the window.
use super::operations::{Completion, Effect, Outgoing};
use super::*;
use futures::{Future, StreamExt};
use nexus_workbench::attachment_upload::CANCELLED;

impl SideChat {
    pub(super) fn can_command(&self) -> bool {
        !self.retired && self.ready && self.pending.valid() && !self.sending && !self.pending.occupied()
    }
    pub(super) fn launch(
        &mut self,
        effect: Effect,
        work: impl Future<Output = Result<Value, String>> + 'static,
        cx: &mut Context<Self>,
    ) {
        if !self.can_command() {
            return;
        }
        if self.pending.start(
            async move {
                Completion {
                    effect,
                    result: work.await,
                }
            },
            cx,
        ) {
            self.sending = true;
            self.problem = None;
            self.note = None;
            cx.notify();
        }
    }
    pub(super) fn watch_commands(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut changes = self.pending.subscribe();
        self._completion = Some(cx.spawn_in(window, async move |this, cx| {
            while changes.next().await.is_some() {
                if this
                    .update_in(cx, |this, window, cx| this.apply_commands(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
    }
    pub(super) fn apply_commands(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.retired || !self.pending.valid() {
            return;
        }
        let Some(Completion { effect, result }) = self.pending.take() else {
            return;
        };
        self.sending = false;
        if matches!(effect, Effect::Send { .. }) {
            self.uploading = None;
        }
        match result {
            Err(error) => match effect {
                Effect::Send { text, draft } => {
                    self.echo_settled(&text, false);
                    self.unsent(text, draft, error, window, cx);
                }
                _ => self.problem = Some(error),
            },
            Ok(value) => match effect {
                Effect::Send { text, .. } => {
                    // The store has the message now: read it at once rather than at the next poll.
                    self.echo_settled(&text, true);
                    if let Some(wake) = &self.wake {
                        let _ = wake.unbounded_send(());
                    }
                }
                Effect::Command {
                    close,
                    note,
                    handing_back,
                } => {
                    if let Ok(summary) = serde_json::from_value::<ThreadSummary>(value.clone())
                        && summary.id == self.summary.id
                        && summary.side_chat
                        && summary.source_thread_id == self.summary.source_thread_id
                    {
                        self.summary = summary;
                    }
                    self.note = if handing_back { None } else { note };
                    if handing_back {
                        self.brought_back = true;
                    }
                    if close {
                        self.poll = None;
                        cx.emit(SideEvent::Closed(value != false));
                    }
                }
                Effect::Rewrite { seq, text } => {
                    if self
                        .rewrite
                        .as_ref()
                        .is_some_and(|edit| edit.seq == seq && edit.input.read(cx).value().as_ref() == text)
                    {
                        self.rewrite = None;
                    }
                }
                Effect::Secret => self.secret.update(cx, |input, cx| input.set_value("", window, cx)),
            },
        }
        // The next message in the line goes once this answer is in, as the session's `outbox`.
        self.flush_outbox(window, cx);
        cx.notify();
    }

    /// Whether the composer takes a message: a send or another command in flight does not stop it.
    pub(super) fn can_compose(&self) -> bool {
        !self.retired && self.ready && self.pending.valid()
    }

    /// Send what is in the composer, as the session composer does (`code_view.rs`, `submit_code`): the text and files
    /// leave the composer at once, and a message the core refuses comes back to it (`unsent`). Messages leave in the
    /// order they were sent, one after the other. `queued`: while the turn runs, the message waits for it to end (a
    /// Queued chip) instead of joining it.
    pub(super) fn send(&mut self, queued: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_compose() || self.request.is_some() || self.secret_id.is_some() {
            return;
        }
        let text = self.input.read(cx).value().to_string();
        if text.trim().is_empty() && self.draft.is_empty() {
            return;
        }
        let draft = std::mem::take(&mut self.draft);
        self.input.update(cx, |input, cx| input.set_value("", window, cx));
        self.brought_back = false;
        self.problem = None;
        self.note = None;
        self.echo_sent(&text, queued);
        self.scroll.to_end();
        self.outbox.push_back(Outgoing { text, draft, queued });
        self.flush_outbox(window, cx);
        cx.notify();
    }

    /// Send the oldest message in the line when nothing else is in flight.
    pub(super) fn flush_outbox(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_command() {
            return;
        }
        let Some(Outgoing {
            text,
            mut draft,
            queued,
        }) = self.outbox.pop_front()
        else {
            return;
        };
        // Files over one piece go up in pieces; their chips follow the upload (`uploads.rs`).
        draft.upload = draft
            .sends_in_pieces()
            .then(|| std::sync::Arc::new(nexus_workbench::attachment_upload::UploadRun::default()));
        let effect = Effect::Send {
            text: text.clone(),
            draft: draft.clone(),
        };
        let (runs, thread, permit) = (self.runs.clone(), self.summary.id.clone(), self.pending.permit());
        let (sent, back) = (draft.clone(), text.clone());
        self.launch(effect, async move {
            permit.check()?;
            let files = crate::attachments::sendable_files(&thread, &draft,
                |chunk| {
                    let permit=permit.clone();let runs=runs.clone();
                    async move { permit.check()?; let read=runs.call("wb_attachment_chunk",serde_json::to_value(chunk).unwrap_or(Value::Null));
                        read.await.map_err(|_|"The upload answer was lost.".to_string())?.and_then(|value|serde_json::from_value(value).map_err(|error|error.to_string())) }
                },
                |id| {
                    let runs=runs.clone();let permit=permit.clone();let thread=thread.clone();
                    async move { permit.check()?; runs.call("wb_attachment_cancel",json!({"uploadId":id,"threadId":thread})).await.map_err(|_|"The upload cancellation was lost.".to_string())?.map(drop) }
                }).await?;
            permit.check()?;
            let mut args = json!({"threadId":thread,"text":text,"images":crate::attachments::session_images(&draft),"files":files});
            // Only when set: an older host ignores it, and the message joins the turn (`Runs::send`).
            if queued {
                args["queued"] = Value::Bool(true);
            }
            runs.call("wb_send_files",args)
                .await.unwrap_or_else(|_|Err("The send answer was lost. Read the current state before retrying.".into()))
        },cx);
        if !self.sending {
            // The send could not start: the message goes back, never lost.
            self.echo_settled(&back, false);
            self.unsent(back, sent, "The side chat is unavailable.".into(), window, cx);
            return;
        }
        if sent.upload.is_some() {
            self.uploading = Some(sent);
            self.watch_upload(window, cx);
        }
    }

    /// A message the core did not take: its text and files go back to the composer, ahead of what was typed since,
    /// with the reason, as the session's `unsent` (`workspace/uploads.rs`). A cancelled upload says only that.
    pub(super) fn unsent(
        &mut self,
        text: String,
        draft: crate::attachments::Draft,
        error: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.draft = super::super::uploads::ahead(draft, std::mem::take(&mut self.draft));
        let typed = self.input.read(cx).value().to_string();
        if let Some(value) = super::super::uploads::restored(&text, &typed) {
            self.input.update(cx, |input, cx| input.set_value(value, window, cx));
        }
        if error == CANCELLED {
            self.note = Some(CANCELLED.into());
        } else {
            self.problem = Some(format!("{error} The message was not sent; it is back in the composer."));
        }
    }
    pub(super) fn command(
        &mut self,
        name: &'static str,
        args: Value,
        close: bool,
        note: Option<&str>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_command() {
            return;
        }
        if matches!(name, "wb_interrupt" | "wb_stop" | "wb_restart" | "wb_side_chat_close") {
            self.suspend_partials();
        }
        let (runs, permit) = (self.runs.clone(), self.pending.permit());
        let effect = Effect::Command {
            close,
            note: note.map(str::to_string),
            handing_back: name == "wb_send_back",
        };
        self.launch(
            effect,
            async move {
                permit.check()?;
                runs.call(name, args).await.unwrap_or_else(|_| {
                    Err("The side chat command's answer was lost. Read the current state before retrying.".into())
                })
            },
            cx,
        );
    }
    pub(super) fn answer_secret(&mut self, provide: bool, _: &mut Window, cx: &mut Context<Self>) {
        if !self.can_command() {
            return;
        }
        let (Runs::Here(runner), Some(id)) = (&self.runs, &self.secret_id) else {
            return;
        };
        let value = self.secret.read(cx).value().to_string();
        if provide && value.is_empty() {
            return;
        }
        let (runner, id, permit) = (runner.clone(), id.clone(), self.pending.permit());
        self.launch(
            Effect::Secret,
            async move {
                permit.check()?;
                runner
                    .answer_secret(id, provide.then_some(value))
                    .await
                    .unwrap_or_else(|_| Err("The secret answer was lost.".into()))
                    .map(|()| Value::Null)
            },
            cx,
        );
    }
}
