//! Existing Workbench mutations, with results owned independently of the window.
use super::operations::{Completion, Effect};
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
            self.draft.upload = None;
            if result.as_ref().is_err_and(|error| error == CANCELLED) {
                self.note = Some(CANCELLED.into());
                cx.notify();
                return;
            }
        }
        match result {
            Err(error) => self.problem = Some(error),
            Ok(value) => match effect {
                Effect::Send { text, revision, files } => {
                    if self.draft_revision == revision && self.input.read(cx).value().as_ref() == text {
                        self.input.update(cx, |input, cx| input.set_value("", window, cx));
                    }
                    for id in files {
                        self.draft.remove(id);
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
                        && let Some(rewrite) = self.rewrite.take()
                    {
                        rewrite.saved();
                    }
                }
                Effect::Secret => self.secret.update(cx, |input, cx| input.set_value("", window, cx)),
            },
        }
        cx.notify();
    }
    pub(super) fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_command() || self.request.is_some() || self.secret_id.is_some() {
            return;
        }
        let text = self.input.read(cx).value().to_string();
        if text.trim().is_empty() && self.draft.is_empty() {
            return;
        }
        if text != self.draft_text {
            self.draft_text = text.clone();
            self.draft_revision = self.draft_revision.wrapping_add(1);
        }
        self.begin_upload();
        let draft = self.draft.clone();
        let effect = Effect::Send {
            text: text.clone(),
            revision: self.draft_revision,
            files: draft.all().map(|f| f.id).collect(),
        };
        let (runs, thread, permit) = (self.runs.clone(), self.summary.id.clone(), self.pending.permit());
        self.brought_back = false;
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
            runs.call("wb_send_files",json!({"threadId":thread,"text":text,"images":crate::attachments::session_images(&draft),"files":files}))
                .await.unwrap_or_else(|_|Err("The send answer was lost. Read the current state before retrying.".into()))
        },cx);
        if self.sending {
            self.watch_upload(window, cx);
        } else {
            self.draft.upload = None;
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
