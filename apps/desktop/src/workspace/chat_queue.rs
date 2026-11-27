//! Follow-ups queued in a Gateway chat while its answer streams, as the Nexus app's plain sessions (`plain-queue.ts`,
//! `use-plain-queue.ts`, drawn with `queued-chips.tsx`). Enter while an answer streams queues what is in the composer;
//! Ctrl+Enter (Cmd+Enter) queues it and sends it now, which stops the answer. Each queued message goes through the
//! app-owned execution path once the answer before it ends.
//!
//! The rule is `nexus_workbench::chat_queue`'s: a failed or stopped answer holds the queue until Resume, Send now goes
//! next however the answer ends, and a message Gateway never took goes back to the front, held. The queues are
//! kept per chat by the execution owner, including primary close-to-tray/reopen. A new chat's queue waits under its unique pending key until Gateway names the session.
//!
//! The chips are the Code session's Queued chips (`code_dock.rs`), inside the composer's box as the Nexus app draws
//! them for a plain session.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::chat_queue::{self, Item, Outcome, Queues};

use super::code_dock::{icon_button, text_button};
use super::{View, Workspace, glyph};
use crate::attachments::Draft;
use crate::palette::Palette;

/// The window's chat queues, the queued message sent last until Gateway stores it, and a chip's rewrite.
pub(super) struct ChatQueue {
    pub(super) queues: Queues<Draft>,
    /// The turn that sent a queued message, and the message: put back if Gateway never takes it.
    pub(super) sent: Option<(u64, Item<Draft>)>,
    /// The rewrite of a queued message, and its `seq`.
    edit: Entity<InputState>,
    pub(super) editing: Option<u64>,
    _subscription: Subscription,
}

impl ChatQueue {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Workspace>) -> ChatQueue {
        let edit = cx.new(|cx| InputState::new(window, cx));
        let subscription = cx.subscribe_in(&edit, window, |this, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { shift: false, .. } = event {
                this.save_queued_chat(window, cx);
            }
        });
        ChatQueue {
            queues: Queues::default(),
            sent: None,
            edit,
            editing: None,
            _subscription: subscription,
        }
    }
}

impl Workspace {
    /// The chat on screen's queue key: its session id, or none yet for a new chat.
    fn queue_key(&self) -> String {
        self.gateway_queue_key()
            .unwrap_or_else(|| self.current.clone().unwrap_or_default())
    }

    /// Queue what the composer holds behind the answer; `now`: and send it next at once, stopping the answer.
    pub(super) fn enqueue_chat(
        &mut self,
        prompt: String,
        attachments: Draft,
        now: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let _ = self.try_enqueue_chat(prompt, attachments, now, window, cx);
    }
    pub(super) fn try_enqueue_chat(
        &mut self,
        prompt: String,
        attachments: Draft,
        now: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.navigation_allowed() {
            return false;
        }
        self.sync_gateway_chat(window, cx);
        if !self.chat_binding_current(cx) {
            return false;
        }
        self.refresh_gateway_queues(cx);
        let key = self.queue_key();
        let seq = self.chat_queue.queues.enqueue(&key, prompt, attachments);
        self.commit_gateway_queues(cx);
        if now {
            self.send_queued_now(seq, window, cx);
        }
        cx.notify();
        self.chat_binding_current(cx)
    }

    /// Send a queued message now: it stops the answer and goes next; with no answer running, it goes at once.
    fn send_queued_now(&mut self, seq: u64, window: &mut Window, cx: &mut Context<Self>) {
        if !self.navigation_allowed() {
            return;
        }
        self.sync_gateway_chat(window, cx);
        self.refresh_gateway_queues(cx);
        let key = self.queue_key();
        if self.asking.is_some() || self.chat_busy() {
            self.chat_queue.queues.send_now(&key, seq);
            self.commit_gateway_queues(cx);
            self.stop(window, cx);
        } else {
            self.chat_queue.queues.update(&key, |queue| {
                queue.promote(seq);
                queue.resume();
            });
            self.commit_gateway_queues(cx);
            self.send_first_queued(cx);
        }
    }

    /// Resume a held queue: its first message goes now if nothing runs, else when the answer ends.
    pub(super) fn resume_chat_queue(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.navigation_allowed() {
            return;
        }
        self.sync_gateway_chat(window, cx);
        self.refresh_gateway_queues(cx);
        let key = self.queue_key();
        self.chat_queue.queues.update(&key, |queue| queue.resume());
        self.commit_gateway_queues(cx);
        if !self.chat_busy() {
            self.send_first_queued(cx);
        }
        cx.notify();
    }

    fn send_first_queued(&mut self, cx: &mut Context<Self>) {
        if !self.ensure_gateway_execution(cx) {
            return;
        }
        if !self.navigation_allowed() {
            return;
        }
        self.refresh_gateway_queues(cx);
        let key = self.queue_key();
        if let Some(item) = self.chat_queue.queues.update(&key, |queue| queue.take()) {
            self.commit_gateway_queues(cx);
            self.send_queued(item, cx);
        }
    }

    /// A queued message goes, through the chat's own send path.
    pub(super) fn send_queued(&mut self, item: Item<Draft>, cx: &mut Context<Self>) {
        let key = self.queue_key();
        if self.chat_queue.editing == Some(item.seq) {
            self.chat_queue.editing = None;
        }
        if self.try_ask(item.text.clone(), item.payload.clone(), cx) {
            self.chat_queue.sent = Some((self.turn, item));
            self.commit_gateway_queues(cx);
        } else if self.chat_binding_current(cx) {
            // Same authorized owner, refused admission: preserve the exact item, held, without retrying.
            self.refresh_gateway_queues(cx);
            self.chat_queue.queues.update(&key, |queue| queue.put_back(item));
            self.commit_gateway_queues(cx);
        }
    }

    /// The chat's answer to `turn` ended: the queue sends its next message, or holds.
    pub(super) fn chat_answer_ended(&mut self, turn: u64, outcome: Outcome, cx: &mut Context<Self>) {
        let key = self.queue_key();
        let next = super::live_chat::settle_receipt(
            &mut self.chat_queue.queues,
            &mut self.chat_queue.sent,
            &key,
            turn,
            outcome,
        );
        if let Some(item) = next {
            self.send_queued(item, cx);
        }
    }

    /// Gateway stored the question asked in `turn`: a queued message sent then is taken.
    pub(super) fn chat_question_stored(&mut self, turn: u64) {
        self.chat_queue.sent.take_if(|(sent, _)| *sent == turn);
    }

    /// Gateway named the new chat: its queue moves to the session's id.
    pub(super) fn chat_queue_named(&mut self, id: &str) {
        self.chat_queue.queues.rename("", id);
    }

    /// The chat on screen goes (another chat, or a new one); an answer still running was stopped. What waits in its
    /// queue is held for when it opens again, since no answer will end to send it. A new chat's has no session to
    /// come back to, so it goes.
    pub(super) fn chat_queue_leave(&mut self) {
        self.chat_queue.editing = None;
        let key = self.queue_key();
        if let Some((_, item)) = self.chat_queue.sent.take() {
            self.chat_queue.queues.update(&key, |queue| queue.put_back(item));
        }
        if key.is_empty() {
            self.chat_queue.queues.clear(&key);
        } else {
            self.chat_queue.queues.hold(&key);
        }
    }

    fn edit_queued_chat(&mut self, seq: u64, text: String, window: &mut Window, cx: &mut Context<Self>) {
        if !self.navigation_allowed() {
            return;
        }
        self.sync_gateway_chat(window, cx);
        self.refresh_gateway_queues(cx);
        self.chat_queue.editing = Some(seq);
        self.chat_queue.edit.update(cx, |state, cx| {
            state.set_value(text, window, cx);
            state.focus(window, cx);
        });
        cx.notify();
    }

    /// Save the rewrite of the queued message; one left with nothing to send is not saved (take it back instead).
    fn save_queued_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.navigation_allowed() {
            return;
        }
        self.sync_gateway_chat(window, cx);
        self.refresh_gateway_queues(cx);
        let Some(seq) = self.chat_queue.editing else {
            return;
        };
        let key = self.queue_key();
        let text = self.chat_queue.edit.read(cx).value().to_string();
        let item = self
            .chat_queue
            .queues
            .get(&key)
            .and_then(|queue| queue.items().iter().find(|item| item.seq == seq));
        let attachments = item.map_or(0, |item| attachment_count(&item.payload));
        if !chat_queue::can_save(&text, attachments) {
            return;
        }
        if item.is_some_and(|item| item.text.trim() != text.trim()) {
            self.chat_queue.queues.update(&key, |queue| queue.edit(seq, text));
            self.commit_gateway_queues(cx);
        }
        self.chat_queue.editing = None;
        self.focus_composer(window, cx);
        cx.notify();
    }

    pub(super) fn gateway_rewrite_text(&self, cx: &App) -> String {
        self.chat_queue.edit.read(cx).value().to_string()
    }
    pub(super) fn restore_gateway_rewrite(
        &mut self,
        seq: u64,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = self.queue_key();
        if self
            .chat_queue
            .queues
            .get(&key)
            .is_some_and(|queue| queue.items().iter().any(|item| item.seq == seq))
        {
            self.edit_queued_chat(seq, text, window, cx);
        } else if !text.trim().is_empty() {
            let current = self.composer.read(cx).value().to_string();
            let merged = if current.is_empty() {
                text
            } else {
                format!("{current}\n\n{text}")
            };
            self.composer
                .update(cx, |input, cx| input.set_value(merged, window, cx));
        }
    }

    /// The Queued chips over the composer's text, while the chat answers or its queue is held.
    pub(super) fn render_chat_queue(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !matches!(self.view, View::Chat) {
            return None;
        }
        let queue = self
            .chat_queue
            .queues
            .get(&self.queue_key())
            .filter(|queue| !queue.is_empty())?;
        let held = queue.held();
        if !held && self.asking.is_none() && !self.chat_busy() {
            return None;
        }
        let count = queue.items().len();
        let rows = queue.items().iter().enumerate().map(|(ix, item)| {
            let seq = item.seq;
            let attachments = attachment_count(&item.payload);
            if self.chat_queue.editing == Some(seq) {
                let text = self.chat_queue.edit.read(cx).value().to_string();
                let ok = chat_queue::can_save(&text, attachments);
                return h_flex()
                    .id(("chat-queued", ix))
                    .w_full()
                    .min_h(px(34.))
                    .px(px(10.))
                    .gap(px(8.))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(palette.faint)
                    .bg(palette.background)
                    .text_size(px(13.))
                    .child(div().text_color(palette.faint).child("Queued"))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .child(Input::new(&self.chat_queue.edit).appearance(false)),
                    )
                    .child(
                        text_button(("chat-queued-cancel", ix), "Cancel", false, palette).on_click(cx.listener(
                            |this, _, window, cx| {
                                this.chat_queue.editing = None;
                                this.focus_composer(window, cx);
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        text_button(("chat-queued-save", ix), "Save", true, palette)
                            .when(!ok, |button| button.opacity(0.4))
                            .tooltip(move |window, cx| {
                                let tip = if ok {
                                    "Save (Enter)"
                                } else {
                                    "A message needs some text. Take it back instead."
                                };
                                gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
                            })
                            .on_click(cx.listener(|this, _, window, cx| this.save_queued_chat(window, cx))),
                    )
                    .into_any_element();
            }
            let edit = item.text.clone();
            h_flex()
                .id(("chat-queued", ix))
                .w_full()
                .h(px(32.))
                .px(px(10.))
                .gap(px(8.))
                .rounded(px(10.))
                .border_1()
                .border_color(palette.border)
                .bg(palette.background)
                .text_size(px(13.))
                .child(
                    div()
                        .flex_none()
                        .font_family("Geist Mono")
                        .text_size(px(11.5))
                        .text_color(palette.faint)
                        .child((ix + 1).to_string()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .child(chat_queue::chip_text(&item.text, attachments)),
                )
                .child(
                    text_button(("chat-queued-send-now", ix), "Send now", false, palette)
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new("Send now: stops this answer and sends it")
                                .build(window, cx)
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.send_queued_now(seq, window, cx);
                            cx.notify();
                        })),
                )
                .child(
                    icon_button(("chat-queued-edit", ix), Lucide::Pencil, palette)
                        .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Edit").build(window, cx))
                        .on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.edit_queued_chat(seq, edit.clone(), window, cx)
                            }),
                        ),
                )
                .child(
                    icon_button(("chat-queued-take-back", ix), Lucide::X, palette)
                        .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Take back").build(window, cx))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if !this.navigation_allowed() {
                                return;
                            }
                            this.refresh_gateway_queues(cx);
                            let key = this.queue_key();
                            this.chat_queue.queues.update(&key, |queue| queue.remove(seq));
                            this.commit_gateway_queues(cx);
                            this.focus_composer(window, cx);
                            cx.notify();
                        })),
                )
                .into_any_element()
        });
        let head = h_flex()
            .gap(px(6.))
            .text_size(px(12.))
            .text_color(palette.muted)
            .child(glyph(Lucide::ListOrdered, palette.faint))
            .child(format!("{} · {count}", if held { "Held" } else { "Queued" }))
            .child(div().text_color(palette.faint).child(if held {
                "the last answer did not finish, so these wait for you"
            } else {
                "sent in order when this answer ends"
            }))
            .when(held, |head| {
                head.child(div().flex_1()).child(
                    text_button("chat-queue-resume", "Resume", true, palette)
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new("Send the next queued message").build(window, cx)
                        })
                        .on_click(cx.listener(|this, _, window, cx| this.resume_chat_queue(window, cx))),
                )
            });
        Some(
            v_flex()
                .id("chat-queue")
                .w_full()
                .pt(px(10.))
                .px(px(12.))
                .gap(px(4.))
                .child(head)
                // A long queue scrolls (about five chips show) instead of pushing the conversation off the window.
                .child(
                    v_flex()
                        .id("chat-queue-rows")
                        .w_full()
                        .max_h(px(168.))
                        .overflow_y_scroll()
                        .gap(px(4.))
                        .children(rows),
                )
                .into_any_element(),
        )
    }
}

fn attachment_count(draft: &Draft) -> usize {
    draft.images.len() + draft.files.len()
}
