//! Rewind and Edit on a person's side message, as on a session's (`code_view.rs`, `user-message.tsx`). Rewind goes back
//! to before the message and puts it in the composer; Edit rewrites it in place and sends it in its stead. The later
//! messages stay, dimmed; files stay as they are. Both ask the engine (`wb_rewind_message`), so a side chat on a
//! server takes them too.
use super::*;
use crate::workspace::code_view::{EditCall, edit_calls, restored_draft};
use nexus_workbench::event::Image;

/// Rewind's confirmation under message `seq`.
pub(super) struct Rewinding {
    pub(super) seq: i64,
    pub(super) busy: bool,
    pub(super) error: Option<String>,
}

/// Message `seq` rewritten in place.
pub(super) struct Editing {
    pub(super) seq: i64,
    pub(super) input: Entity<TextareaState>,
    images: Vec<Image>,
    pub(super) busy: bool,
    pub(super) error: Option<String>,
    _subscription: Subscription,
}

impl Editing {
    pub(super) fn empty(&self, cx: &App) -> bool {
        self.input.read(cx).value().trim().is_empty() && self.images.is_empty()
    }
}

impl SideChat {
    /// Whether a message takes Rewind and Edit: the person's own, still in the conversation, while the side chat is
    /// reachable (`can_act_on`). While the agent runs they show, but disabled.
    pub(super) fn can_edit(&self, linked: bool, rewound: bool) -> bool {
        self.can_compose() && !linked && !rewound
    }

    fn user_message(&self, seq: i64) -> Option<(String, Vec<Image>)> {
        self.blocks.iter().find_map(|block| match block.inner() {
            Block::User {
                seq: at,
                text,
                image_data,
                ..
            } if *at == seq => Some((text.clone(), image_data.clone())),
            _ => None,
        })
    }

    pub(super) fn ask_rewind(&mut self, seq: i64, cx: &mut Context<Self>) {
        if self.summary.status.busy() {
            return;
        }
        self.editing = None;
        self.rewinding = Some(Rewinding {
            seq,
            busy: false,
            error: None,
        });
        cx.notify();
    }

    pub(super) fn cancel_rewind(&mut self, cx: &mut Context<Self>) {
        if !self.rewinding.as_ref().is_some_and(|rewinding| rewinding.busy) {
            self.rewinding = None;
            cx.notify();
        }
    }

    /// Rewind to before the message being confirmed: the message goes back to the composer, under what is typed.
    pub(super) fn confirm_rewind(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(seq) = self.rewinding.as_ref().filter(|r| !r.busy).map(|r| r.seq) else {
            return;
        };
        let Some((text, images)) = self.user_message(seq) else {
            return;
        };
        if let Some(rewinding) = &mut self.rewinding {
            rewinding.busy = true;
            rewinding.error = None;
        }
        let (thread, permit) = (self.summary.id.clone(), self.pending.permit());
        let rewind = self
            .runs
            .call("wb_rewind_message", json!({"threadId": thread, "seq": seq}));
        cx.spawn_in(window, async move |this, cx| {
            let answer = rewind.await.unwrap_or_else(|_| Err("The rewind was cancelled.".into()));
            let _ = this.update_in(cx, |this, window, cx| {
                if permit.check().is_err() || this.retired {
                    return;
                }
                match answer {
                    Ok(_) => {
                        this.rewinding = None;
                        this.put_back(&text, &images, seq, window, cx);
                        this.read_now();
                    }
                    Err(error) => {
                        if let Some(rewinding) = &mut this.rewinding {
                            rewinding.busy = false;
                            rewinding.error = Some(error);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// A rewound message back in the composer, under what is already typed, its images with it.
    fn put_back(&mut self, text: &str, images: &[Image], seq: i64, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.input.read(cx).value().to_string();
        let value = restored_draft(&typed, text);
        self.input.update(cx, |input, cx| input.set_value(value, window, cx));
        crate::attachments::append_restored(
            &mut self.draft,
            crate::attachments::restored_images(&self.summary.id, seq, images),
        );
        self.input.focus_handle(cx).focus(window, cx);
    }

    /// Read the store now, rather than at the next poll (`active`).
    pub(super) fn read_now(&self) {
        if let Some(wake) = &self.wake {
            let _ = wake.unbounded_send(());
        }
    }

    /// Edit message `seq` in place: its text goes in an editor where the message was.
    pub(super) fn start_edit(&mut self, seq: i64, window: &mut Window, cx: &mut Context<Self>) {
        if self.summary.status.busy() {
            return;
        }
        let Some((text, images)) = self.user_message(seq) else {
            return;
        };
        let input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(2, 12));
        input.update(cx, |state, cx| {
            state.set_value(text, window, cx);
            state.focus(window, cx);
        });
        // Ctrl+Enter (Cmd+Enter on a Mac) sends, as the session's editor; Enter is a new line.
        let subscription = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { secondary: true, .. } = event {
                this.send_edit(window, cx);
            }
        });
        self.rewinding = None;
        self.editing = Some(Editing {
            seq,
            input,
            images,
            busy: false,
            error: None,
            _subscription: subscription,
        });
        cx.notify();
    }

    pub(super) fn cancel_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.editing.as_ref().is_some_and(|editing| editing.busy) {
            self.editing = None;
            self.input.focus_handle(cx).focus(window, cx);
            cx.notify();
        }
    }

    /// Send the edited message: rewind to before it, then send the new text. A failed rewind keeps the editor open
    /// with the error; a send that fails after the rewind puts the text back in the composer.
    pub(super) fn send_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.summary.status.busy() {
            return;
        }
        let thread = self.summary.id.clone();
        let Some(editing) = self.editing.as_mut().filter(|editing| !editing.busy) else {
            return;
        };
        let text = editing.input.read(cx).value().to_string();
        let images = editing.images.clone();
        let Some([EditCall::Rewind { seq, .. }, EditCall::Send { text, .. }]) =
            edit_calls(&thread, editing.seq, &text, images.len())
        else {
            return;
        };
        editing.busy = true;
        editing.error = None;
        let (runs, permit) = (self.runs.clone(), self.pending.permit());
        let rewind = runs.call("wb_rewind_message", json!({"threadId": thread, "seq": seq}));
        cx.spawn_in(window, async move |this, cx| {
            let rewind = rewind.await.unwrap_or_else(|_| Err("The rewind was cancelled.".into()));
            let rewound = rewind.is_ok();
            let answer = match rewind {
                Ok(_) => runs
                    .call("wb_send", json!({"threadId": thread, "text": text, "images": images}))
                    .await
                    .unwrap_or_else(|_| Err("The edited message's answer was lost.".into())),
                Err(error) => Err(error),
            };
            let _ = this.update_in(cx, |this, window, cx| {
                if permit.check().is_err() || this.retired {
                    return;
                }
                match answer {
                    Ok(_) => {
                        this.editing = None;
                        this.input.focus_handle(cx).focus(window, cx);
                        this.read_now();
                    }
                    Err(error) if rewound => {
                        this.editing = None;
                        this.put_back(&text, &images, seq, window, cx);
                        this.problem = Some(error);
                        this.read_now();
                    }
                    Err(error) => {
                        if let Some(editing) = &mut this.editing {
                            editing.busy = false;
                            editing.error = Some(error);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}
