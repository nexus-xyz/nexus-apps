//! Stacked replies in a side chat, as in a session (`stacked_replies.rs`, `cite_selection.rs`). Select a passage of a
//! side answer and press Reply: a small box opens with the passage over a line to answer it. Enter puts the pair over
//! the composer as a chip, and more can stack. The next send goes once: each passage quoted, its reply under it, then
//! what the composer holds (`stacked_replies::message`). The person's bubble draws the quotes with a bar.
use super::*;
use crate::workspace::cite_selection::BoundsCell;
use crate::workspace::stacked_replies::{self, Reply};
use gpui_kit::base::TextSelection;
use nexus_workbench::cite;

/// The toolbar and the box sit this far above where the selection ended.
pub(super) const LIFT: f32 = 24.;

/// The side chat's stack, the passage the toolbar offers, and the reply box while it is open.
pub(super) struct Replies {
    pub(super) stack: Vec<Reply>,
    next: u64,
    /// The selected passage the toolbar offers, and the one the person dismissed (it does not come back).
    pub(super) selected: Option<String>,
    dismissed: Option<String>,
    /// The passage the reply box answers, while it is open.
    pub(super) composing: Option<String>,
    /// Where the selection ended: the toolbar and the box open over it.
    pub(super) pointer: Point<Pixels>,
    pressed: bool,
    /// A press on the toolbar: the window's selection clears, and the passage must stay until the click lands.
    button_pressed: bool,
    pub(super) input: Entity<TextareaState>,
    pub(super) edit: Entity<TextareaState>,
    pub(super) editing: Option<u64>,
    pub(super) hovered: Option<u64>,
    pub(super) box_bounds: BoundsCell,
    pub(super) toolbar_bounds: BoundsCell,
}

impl Replies {
    pub(super) fn new(window: &mut Window, cx: &mut Context<SideChat>) -> (Self, Vec<Subscription>) {
        let field = |window: &mut Window, cx: &mut Context<SideChat>| {
            cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("Reply")
                    .submit_on_enter(true)
                    .auto_grow(1, 6)
            })
        };
        let (input, edit) = (field(window, cx), field(window, cx));
        let subscriptions = vec![
            cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift: false, .. } = event {
                    this.add_reply(window, cx);
                }
            }),
            cx.subscribe_in(&edit, window, |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift: false, .. } = event {
                    this.save_reply_edit(window, cx);
                }
            }),
        ];
        let replies = Replies {
            stack: vec![],
            next: 0,
            selected: None,
            dismissed: None,
            composing: None,
            pointer: point(px(0.), px(0.)),
            pressed: false,
            button_pressed: false,
            input,
            edit,
            editing: None,
            hovered: None,
            box_bounds: Default::default(),
            toolbar_bounds: Default::default(),
        };
        (replies, subscriptions)
    }

    /// The stack went with a message.
    pub(super) fn sent(&mut self) {
        self.stack.clear();
        self.editing = None;
        self.hovered = None;
    }

    /// The stack, for a closed window to keep (`retention.rs`).
    pub(super) fn keep(&mut self) -> (Vec<Reply>, u64) {
        (std::mem::take(&mut self.stack), self.next)
    }

    pub(super) fn restore(&mut self, (stack, next): (Vec<Reply>, u64)) {
        self.stack = stack;
        self.next = next;
    }
}

impl SideChat {
    /// The message a send takes: the stack's passages and replies, then what the composer holds.
    pub(super) fn with_replies(&self, typed: String) -> String {
        if self.replies.stack.is_empty() {
            typed
        } else {
            stacked_replies::message(&self.replies.stack, &typed)
        }
    }

    /// The selected passage of a side answer, offered by the toolbar when it can be quoted (`cite::eligible`).
    pub(super) fn refresh_cite(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.replies.button_pressed || self.replies.composing.is_some() {
            return;
        }
        let whole = TextSelection::selected_text(window, cx);
        let mut ixs: Vec<usize> = self.texts.keys().copied().collect();
        ixs.sort_unstable();
        let parts: Vec<String> = ixs
            .into_iter()
            .filter_map(|ix| {
                let selected = self.texts.get(&ix)?.read(cx).selected_text();
                (!selected.trim().is_empty()).then_some(selected)
            })
            .collect();
        let candidate = cite::eligible(parts.iter().map(String::as_str), &whole);
        let next = candidate.filter(|text| {
            window.is_window_active() && !self.replies.pressed && self.replies.dismissed.as_ref() != Some(text)
        });
        if self.replies.selected != next {
            self.replies.selected = next;
            cx.notify();
        }
    }

    /// A press in the history: in the reply box it is the box's; on the toolbar it keeps the passage; elsewhere it
    /// closes the box and starts a new selection.
    pub(super) fn cite_pointer_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let inside = |cell: &BoundsCell| cell.get().is_some_and(|bounds| bounds.contains(&event.position));
        if self.replies.composing.is_some() {
            if inside(&self.replies.box_bounds) {
                return;
            }
            self.close_reply(window, cx);
        }
        if self.replies.selected.is_some() && inside(&self.replies.toolbar_bounds) {
            self.replies.button_pressed = event.button == MouseButton::Left;
            return;
        }
        self.replies.button_pressed = false;
        self.replies.pressed = event.button == MouseButton::Left;
        self.replies.dismissed = None;
        self.replies.selected = None;
        self.replies.pointer = event.position;
        cx.notify();
    }

    pub(super) fn cite_pointer_up(&mut self, event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.button != MouseButton::Left {
            return;
        }
        if self.replies.button_pressed {
            cx.defer_in(window, |this, window, cx| {
                this.replies.button_pressed = false;
                this.refresh_cite(window, cx);
            });
            return;
        }
        if !self.replies.pressed {
            return;
        }
        self.replies.pressed = false;
        self.replies.pointer = event.position + point(px(0.), px(6.));
        cx.defer_in(window, |this, window, cx| this.refresh_cite(window, cx));
    }

    /// Esc on the toolbar's passage: it goes, and does not come back for the same selection.
    pub(super) fn dismiss_cite(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(selected) = self.replies.selected.take() else {
            return false;
        };
        self.replies.dismissed = Some(selected);
        cx.notify();
        true
    }

    /// Reply: the box opens over the passage, its field focused.
    pub(super) fn reply_to_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(quote) = self.replies.selected.take() else {
            return;
        };
        self.replies.dismissed = Some(quote.clone());
        if quote.trim().is_empty() {
            return;
        }
        let agent = crate::workspace::code_dock::agent_name(self.summary.agent);
        let placeholder: SharedString = format!("Reply to {agent}").into();
        self.replies.composing = Some(quote);
        self.replies.input.update(cx, |input, cx| {
            input.set_placeholder(placeholder, window, cx);
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    /// Close the reply box; what it held goes.
    pub(super) fn close_reply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.replies.composing.take().is_some() {
            self.input.focus_handle(cx).focus(window, cx);
            cx.notify();
        }
    }

    /// Put the box's passage and reply on the stack, over the composer.
    pub(super) fn add_reply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(quote) = self.replies.composing.take() else {
            return;
        };
        let text = self.replies.input.read(cx).value().trim().to_string();
        self.replies.next += 1;
        let id = self.replies.next;
        self.replies.stack.push(Reply { id, quote, text });
        self.replies
            .input
            .update(cx, |input, cx| input.set_value("", window, cx));
        TextSelection::clear(window, cx);
        self.input.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    /// Rewrite chip `id`'s reply in place.
    pub(super) fn edit_reply(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self.replies.stack.iter().find(|r| r.id == id).map(|r| r.text.clone()) else {
            return;
        };
        self.replies.editing = Some(id);
        self.replies.edit.update(cx, |edit, cx| {
            edit.set_value(text, window, cx);
            edit.focus(window, cx);
        });
        cx.notify();
    }

    /// Keep the rewrite. An empty one leaves the passage without a reply.
    pub(super) fn save_reply_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.replies.editing.take() else {
            return;
        };
        let text = self.replies.edit.read(cx).value().trim().to_string();
        if let Some(reply) = self.replies.stack.iter_mut().find(|r| r.id == id) {
            reply.text = text;
        }
        self.input.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    /// Drop the rewrite; the reply stays as it was.
    pub(super) fn cancel_reply_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.replies.editing.take().is_some() {
            self.input.focus_handle(cx).focus(window, cx);
            cx.notify();
        }
    }

    pub(super) fn remove_reply(&mut self, id: u64, cx: &mut Context<Self>) {
        self.replies.stack.retain(|reply| reply.id != id);
        if self.replies.editing == Some(id) {
            self.replies.editing = None;
        }
        if self.replies.hovered == Some(id) {
            self.replies.hovered = None;
        }
        cx.notify();
    }

    pub(super) fn reply_hovered(&mut self, id: u64, hovered: bool, cx: &mut Context<Self>) {
        let next = if hovered {
            Some(id)
        } else if self.replies.hovered == Some(id) {
            None
        } else {
            return;
        };
        if self.replies.hovered != next {
            self.replies.hovered = next;
            cx.notify();
        }
    }
}
