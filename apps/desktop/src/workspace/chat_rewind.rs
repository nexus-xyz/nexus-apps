//! Rewind and Edit on a person's message in the open Gateway chat, as the Nexus app's (`editable-prompt.tsx`,
//! `session-view.tsx`'s `rewindMessage`). Rewind asks first, then puts the message back in the composer; Edit opens in
//! the message's place, and Save & send rewinds, then sends the new text with the message's attachments. A rewound
//! message and the later ones stay, dimmed. The rule is `nexus_desktop_core::chat_rewind`; the calls are
//! `Gateway::rewind`'s.

use crate::workspace::glass::FloatingGround;
use std::sync::Arc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_desktop_core::chat_rewind::{self, RestoredFile, RestoredPrompt};

use super::transcript::Entry;
use super::{Workspace, glyph};
use crate::attachments::{self, Incoming, Source};
use crate::gateway::OpenedChat;
use crate::palette::Palette;

/// How much a rewound message fades (`.chat-message-rewound`).
pub(super) const REWOUND_OPACITY: f32 = 0.45;

/// Rewind's confirmation or the editor, open on one message.
pub(super) struct ChatRewind {
    /// The entry it is open on.
    pub entry: usize,
    /// The editor in the message's place; `None` is Rewind's confirmation under it.
    pub editor: Option<(Entity<TextareaState>, Subscription)>,
    pub busy: bool,
    pub error: Option<SharedString>,
    /// The rewind sent, so a late answer restores its prompt without changing another editor.
    pub ask: u64,
}

/// A question an edit sent, until Gateway stores it: a send that fails before then gives the prompt back
/// (`session-view.tsx`: `if (!accepted) restorePrompt(prompt)`).
#[derive(Clone)]
pub(super) struct EditSent {
    pub turn: u64,
    pub text: String,
    pub draft: attachments::Draft,
}

/// The files a rewind gave back, as the composer takes files from outside (`attachments::prepare`).
pub(super) fn incoming(prompt: &RestoredPrompt) -> Vec<Incoming> {
    let file = |file: &RestoredFile, image: bool| Incoming {
        name: file.name.clone(),
        media_type: image.then(|| file.media_type.clone()),
        size: file.bytes.len() as u64,
        source: Source::Bytes(Arc::new(file.bytes.clone())),
    };
    prompt
        .images
        .iter()
        .map(|image| file(image, true))
        .chain(prompt.files.iter().map(|data| file(data, false)))
        .collect()
}

impl Workspace {
    /// Whether an answer or a rewind runs, so Rewind and Edit wait (`session-view.tsx`'s `busy`).
    pub(super) fn chat_busy(&self) -> bool {
        self.asking.is_some() || self.chat_rewind.as_ref().is_some_and(|open| open.busy)
    }

    fn entry_of(&self, entry: usize) -> Option<&Entry> {
        self.entries.iter().find(|candidate| candidate.id == entry)
    }

    /// Ask to rewind to before entry `entry`: its confirmation opens under it.
    pub(super) fn ask_chat_rewind(&mut self, entry: usize, cx: &mut Context<Self>) {
        if self.chat_busy() {
            return;
        }
        self.chat_rewind = Some(ChatRewind {
            entry,
            editor: None,
            busy: false,
            error: None,
            ask: self.rewind_ask,
        });
        cx.notify();
    }

    /// Close the confirmation or the editor, unless its rewind is under way.
    pub(super) fn cancel_chat_rewind(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.chat_rewind.as_ref().is_some_and(|open| !open.busy) {
            let editing = self.chat_rewind.take().is_some_and(|open| open.editor.is_some());
            if editing {
                self.focus_composer(window, cx);
            }
            cx.notify();
        }
    }

    /// Edit entry `entry` in place: its text goes in an editor where the message was.
    pub(super) fn start_chat_edit(&mut self, entry: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.chat_busy() {
            return;
        }
        let Some(text) = self.entry_of(entry).map(|entry| entry.text.to_string()) else {
            return;
        };
        let input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(3, 12));
        input.update(cx, |state, cx| {
            state.set_value(text, window, cx);
            state.focus(window, cx);
        });
        // Ctrl+Enter (Cmd+Enter on a Mac) sends, as the Nexus app's editor; Enter is a new line.
        let subscription = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { secondary: true, .. } = event {
                this.send_chat_edit(window, cx);
            }
        });
        self.chat_rewind = Some(ChatRewind {
            entry,
            editor: Some((input, subscription)),
            busy: false,
            error: None,
            ask: self.rewind_ask,
        });
        cx.notify();
    }

    /// Rewind's Rewind: the message goes back to the composer.
    pub(super) fn confirm_chat_rewind(&mut self, cx: &mut Context<Self>) {
        self.start_rewind(None, cx);
    }

    /// The editor's Save & send: rewind, then send the edited text with the message's attachments.
    pub(super) fn send_chat_edit(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self
            .chat_rewind
            .as_ref()
            .and_then(|open| open.editor.as_ref())
            .map(|(input, _)| input.read(cx).value().to_string())
        else {
            return;
        };
        self.start_rewind(Some(text), cx);
    }

    /// Send the rewind open now: `edit` is the editor's text, `None` for Rewind.
    fn start_rewind(&mut self, edit: Option<String>, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        let busy = self.chat_busy();
        let Some(open) = self.chat_rewind.as_ref().filter(|open| !open.busy) else {
            return;
        };
        let entry = open.entry;
        if busy {
            if let Some(open) = &mut self.chat_rewind {
                open.error = Some(chat_rewind::BUSY.into());
            }
            cx.notify();
            return;
        }
        let Some((message, text, refs)) = self.entry_of(entry).and_then(|entry| {
            Some((
                entry.stored.id.clone()?,
                entry.text.to_string(),
                entry.stored.attachments.clone(),
            ))
        }) else {
            return;
        };
        let Some(session) = self.current.clone() else {
            return;
        };
        if let Some(edit) = &edit
            && !chat_rewind::can_send_edit(edit, refs.len(), false)
        {
            return;
        }
        self.cached_chat.mutated(Some(&session));
        self.kept_chats.mutated();
        self.rewind_ask += 1;
        let ask = self.rewind_ask;
        if let Some(open) = &mut self.chat_rewind {
            open.busy = true;
            open.error = None;
            open.ask = ask;
        }
        self.rewind_edit = edit.is_some();
        let attachments = refs.into_iter().map(|attachment| attachment.id).collect();
        account_gateway.rewind(ask, session, message, edit.unwrap_or(text), attachments);
        cx.notify();
    }

    /// Gateway answered rewind `ask`: on success the chat shows as Gateway keeps it now, and the prompt goes back to
    /// the composer (Rewind) or is sent (Edit). A failure keeps the confirmation or the editor open, with why.
    pub(super) fn chat_rewound(
        &mut self,
        ask: u64,
        outcome: Result<(OpenedChat, RestoredPrompt), (String, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        let active = self
            .chat_rewind
            .as_ref()
            .is_some_and(|open| open.ask == ask && open.busy);
        let (chat, prompt) = match outcome {
            Ok(done) => done,
            Err((error, text)) => {
                if active && let Some(open) = &mut self.chat_rewind {
                    open.busy = false;
                    open.error = Some(error.into());
                } else {
                    self.put_back_prompt(&text, attachments::Draft::default(), window, cx);
                    self.composer_note = Some(error.into());
                }
                return;
            }
        };
        let session = chat.id;
        let same_chat = active && self.current.as_deref() == Some(session.as_str());
        let edit = if active {
            self.chat_rewind = None;
            std::mem::take(&mut self.rewind_edit) && same_chat
        } else {
            false
        };
        if same_chat {
            self.entries = self.entries_from(chat.messages, cx);
            self.notice = None;
            self.suggestion = None;
            self.close_page(window, cx);
        }
        account_gateway.list_sessions();
        let files = incoming(&prompt);
        if files.is_empty() {
            self.give_back(edit, prompt.text, attachments::Draft::default(), None, window, cx);
            return;
        }
        // The images' thumbnails are made off the UI thread, as an attach's.
        self.attaching += 1;
        cx.spawn_in(window, async move |this, cx| {
            let prepared = cx
                .background_executor()
                .spawn(async move { attachments::prepare(files) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.attaching -= 1;
                let mut draft = attachments::Draft::default();
                let problem = attachments::merge(&mut draft, prepared);
                // The person can leave while thumbnails are prepared too. Restore instead of sending elsewhere.
                let edit = edit && this.current.as_deref() == Some(session.as_str());
                this.give_back(edit, prompt.text, draft, problem, window, cx);
            });
        })
        .detach();
    }

    /// After a rewind: send the edit, or put the prompt back in the composer.
    pub(super) fn give_back(
        &mut self,
        edit: bool,
        text: String,
        draft: attachments::Draft,
        problem: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut rejected = false;
        if edit {
            let refused = self.gateway_payload_submission(&text, &draft);
            if self.asking.is_none()
                && self.ensure_gateway_execution(cx)
                && self.try_ask(text.clone(), draft.clone(), cx)
            {
                self.edit_sent = Some(EditSent {
                    turn: self.turn,
                    text,
                    draft,
                });
                self.commit_gateway_queues(cx);
                self.focus_composer(window, cx);
            } else {
                self.draft_refused(refused, window, cx);
                rejected = true;
            }
        } else {
            self.put_back_prompt(&text, draft, window, cx);
        }
        self.composer_note = if rejected {
            Some("This message wasn't sent. Its draft was kept in the original account.".into())
        } else {
            problem.map(Into::into)
        };
        cx.notify();
    }

    /// Put a prompt back in the composer, under what is typed, its attachments after the ones attached
    /// (`mergeRestoredPrompt`).
    pub(super) fn put_back_prompt(
        &mut self,
        text: &str,
        draft: attachments::Draft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let problem = attachments::merge(
            &mut self.draft,
            attachments::Prepared {
                images: draft.images,
                files: draft.files,
                ..Default::default()
            },
        );
        self.put_back(text, window, cx);
        if problem.is_some() {
            self.composer_note = problem.map(Into::into);
        }
    }

    /// An edit's question that Gateway never stored (failed or stopped first) goes back to the composer.
    pub(super) fn edit_not_sent(&mut self, turn: u64, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(sent) = self.edit_sent.take_if(|sent| sent.turn == turn) {
            self.put_back_prompt(&sent.text, sent.draft, window, cx);
        }
    }

    /// Rewind and Edit beside a question's Copy (`editable-prompt.tsx`), while it is stored, not rewound and not being
    /// edited. They show, dimmed, while an answer or a rewind runs.
    pub(super) fn render_prompt_actions(
        &self,
        entry: &Entry,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let editing = self
            .chat_rewind
            .as_ref()
            .is_some_and(|open| open.entry == entry.id && open.editor.is_some());
        if entry.stored.id.is_none() || entry.stored.rewound || editing {
            return None;
        }
        let enabled = chat_rewind::can_act(true, false, self.chat_busy());
        let id = entry.id;
        let action = |name: &'static str, icon: Lucide, tip: &'static str| {
            div()
                .id((name, id))
                .size(px(28.))
                .rounded(px(8.))
                .flex()
                .items_center()
                .justify_center()
                .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
                .child(glyph(icon, palette.faint))
                .map(|button| {
                    if enabled {
                        button
                            .cursor_pointer()
                            .hover(|style| style.bg(palette.hover))
                            .active(|style| style.opacity(0.7))
                    } else {
                        button.opacity(0.4)
                    }
                })
        };
        let rewind = action("prompt-rewind", Lucide::RotateCcw, "Rewind to this message").when(enabled, |button| {
            button.on_click(cx.listener(move |this, _, _, cx| this.ask_chat_rewind(id, cx)))
        });
        let edit = action("prompt-edit", Lucide::Pencil, "Edit this message").when(enabled, |button| {
            button.on_click(cx.listener(move |this, _, window, cx| this.start_chat_edit(id, window, cx)))
        });
        Some(h_flex().gap(px(2.)).child(rewind).child(edit))
    }

    /// The editor in a question's place, when it is open on `entry`: the text, what sending does, the attachments
    /// that stay, then Cancel and Save & send. Esc cancels.
    pub(super) fn render_prompt_editor(
        &self,
        entry: &Entry,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let open = self.chat_rewind.as_ref().filter(|open| open.entry == entry.id)?;
        let (input, _) = open.editor.as_ref()?;
        let busy = open.busy;
        let sendable = chat_rewind::can_send_edit(&input.read(cx).value(), entry.stored.attachments.len(), busy);
        Some(
            v_flex()
                .w(relative(0.8))
                .gap(px(8.))
                .p(px(12.))
                .rounded(px(12.))
                .bg(palette.bubble)
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        cx.stop_propagation();
                        this.cancel_chat_rewind(window, cx);
                    }
                }))
                .child(
                    div()
                        .text_size(px(14.))
                        .line_height(px(22.))
                        // The field pads its own text; pulled out by that much, it lines up with the note under it.
                        .mx(px(-10.))
                        .child(Textarea::new(input).appearance(false)),
                )
                .child(note(chat_rewind::EDIT_NOTE, palette.muted))
                .children(
                    chat_rewind::attachments_note(&entry.stored.attachments).map(|said| note(said, palette.muted)),
                )
                .children(open.error.clone().map(|error| note(error, palette.destructive)))
                .child(
                    h_flex()
                        .justify_end()
                        .gap(px(8.))
                        .child(
                            super::code_dock::answer_button("chat-edit-cancel", "Cancel", false, !busy, palette)
                                .on_click(cx.listener(|this, _, window, cx| this.cancel_chat_rewind(window, cx))),
                        )
                        .child(
                            super::code_dock::answer_button(
                                "chat-edit-send",
                                if busy { "Sending…" } else { "Save & send" },
                                true,
                                sendable,
                                palette,
                            )
                            .when(sendable, |button| {
                                button.on_click(cx.listener(|this, _, window, cx| this.send_chat_edit(window, cx)))
                            }),
                        ),
                ),
        )
    }

    /// Rewind's confirmation under a question, when it is open on `entry`: what it does and leaves, then Cancel and
    /// Rewind.
    pub(super) fn render_rewind_confirm(
        &self,
        entry: &Entry,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let open = self
            .chat_rewind
            .as_ref()
            .filter(|open| open.entry == entry.id && open.editor.is_none())?;
        let busy = open.busy;
        Some(
            v_flex()
                .w(px(320.))
                .gap(px(8.))
                .p(px(12.))
                .rounded(px(12.))
                .floating_ground(12., palette.pop, false, &palette)
                .shadow_md()
                .child(
                    div()
                        .text_size(px(13.5))
                        .font_weight(FontWeight::MEDIUM)
                        .child("Rewind to before this message?"),
                )
                .child(note(chat_rewind::REWIND_NOTE, palette.muted))
                .children(open.error.clone().map(|error| note(error, palette.destructive)))
                .child(
                    h_flex()
                        .justify_end()
                        .gap(px(8.))
                        .child(
                            super::code_dock::answer_button("chat-rewind-cancel", "Cancel", false, !busy, palette)
                                .on_click(cx.listener(|this, _, window, cx| this.cancel_chat_rewind(window, cx))),
                        )
                        .child(
                            super::code_dock::answer_button(
                                "chat-rewind-confirm",
                                if busy { "Rewinding…" } else { "Rewind" },
                                true,
                                !busy,
                                palette,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.confirm_chat_rewind(cx))),
                        ),
                ),
        )
    }
}

fn note(text: impl Into<SharedString>, color: Hsla) -> Div {
    div()
        .text_size(px(12.5))
        .line_height(px(18.))
        .text_color(color)
        .child(text.into())
}

#[cfg(test)]
mod tests {
    use super::incoming;
    use crate::attachments::Source;
    use nexus_desktop_core::chat_rewind::{RestoredFile, RestoredPrompt};

    /// A rewind's files reach the composer as an attach's would: images by their type, data files by their name.
    #[test]
    fn a_rewinds_files_go_back_as_attached_files() {
        let file = |name: &str, media_type: &str| RestoredFile {
            name: name.into(),
            media_type: media_type.into(),
            bytes: b"abc".to_vec(),
        };
        let prompt = RestoredPrompt {
            text: "Original".into(),
            images: vec![file("picture.png", "image/png")],
            files: vec![file("notes.csv", "text/csv")],
        };
        let files = incoming(&prompt);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].media_type.as_deref(), Some("image/png"));
        assert_eq!(files[1].media_type, None);
        assert_eq!(files[1].name, "notes.csv");
        assert_eq!(files[1].size, 3);
        assert!(matches!(&files[1].source, Source::Bytes(bytes) if bytes.as_slice() == b"abc"));
    }
}
