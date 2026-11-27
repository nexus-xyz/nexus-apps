//! Stacked replies in a Code session. Select a passage of a finished answer and press Reply (`cite_selection.rs`): a
//! small box opens with the passage over a line to answer it. Enter (or the add button) puts the pair over the
//! composer as a chip, and more can stack. The next send goes once: each passage quoted, its reply under it, then
//! what the composer holds. The person's bubble draws the quotes with a bar ([`segments`]).

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::TextSelection;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::cite;
use std::collections::HashMap;

use super::cite_selection::{BoundsCell, bounds_probe};
use super::glass::FloatingGround;
use super::{View, Workspace};
use crate::palette::Palette;

/// A passage of an answer and the person's reply to it, waiting over the composer.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Reply {
    pub id: u64,
    pub quote: String,
    pub text: String,
}

/// The reply box open over a passage: its session, the passage, and where it opened.
struct Composing {
    thread: String,
    quote: String,
    at: Point<Pixels>,
}

/// Each Code session's stacked replies, and the reply box while it is open.
pub(super) struct Replies {
    stacks: HashMap<String, Vec<Reply>>,
    next: u64,
    composing: Option<Composing>,
    pub(super) input: Entity<TextareaState>,
    /// The reply box's painted bounds: a press inside it leaves the box open.
    pub(super) bounds: BoundsCell,
    /// The chip under the pointer: it shows its reply (`chips.rs`).
    hovered: Option<u64>,
    /// The chip whose reply is being rewritten, and the rewrite.
    editing: Option<u64>,
    edit: Entity<TextareaState>,
    _subscriptions: Vec<Subscription>,
}

impl Replies {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Replies {
        let field = |window: &mut Window, cx: &mut Context<Workspace>| {
            cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("Reply")
                    .submit_on_enter(true)
                    .auto_grow(1, 6)
            })
        };
        let input = field(window, cx);
        let edit = field(window, cx);
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
        Replies {
            stacks: HashMap::new(),
            next: 0,
            composing: None,
            input,
            bounds: Default::default(),
            hovered: None,
            editing: None,
            edit,
            _subscriptions: subscriptions,
        }
    }

    pub(super) fn is_composing(&self) -> bool {
        self.composing.is_some()
    }

    /// Forget every stack (sign-out).
    pub(super) fn clear(&mut self) {
        self.stacks.clear();
        self.composing = None;
        self.hovered = None;
        self.editing = None;
    }
}

/// A chip's fill: the person's own messages' (`bubble`), so a stacked reply reads as their words, apart from the
/// composer's raised box in both themes. The hover fill read as the composer's own (#232325 on #212123 in dark).
pub(super) fn chip_fill(palette: Palette) -> Hsla {
    palette.bubble
}

/// A chip's words: the passage on one line, in curly quotes.
pub(super) fn chip_label(quote: &str) -> String {
    format!(
        "\u{201c}{}\u{201d}",
        quote.split_whitespace().collect::<Vec<_>>().join(" ")
    )
}

/// The message the stack and the composer's text send: each passage as a Markdown quote with its reply under it, then
/// the composer's text. Empty when there is nothing to send.
pub(super) fn message(replies: &[Reply], draft: &str) -> String {
    let mut parts: Vec<String> = replies
        .iter()
        .filter_map(|reply| {
            let quote = cite::quote_block(&reply.quote);
            let text = reply.text.trim();
            match (quote.is_empty(), text.is_empty()) {
                (true, true) => None,
                (true, false) => Some(text.to_string()),
                (false, true) => Some(quote),
                (false, false) => Some(format!("{quote}\n\n{text}")),
            }
        })
        .collect();
    let draft = draft.trim();
    if !draft.is_empty() {
        parts.push(draft.to_string());
    }
    parts.join("\n\n")
}

/// A sent message cut into what it quotes and what it says, in order, for its bubble: runs of `>` lines are quotes
/// (without their `>`), the rest is text. Blank lines between parts go; a fenced code block is never a quote. One
/// text part when the message quotes nothing.
pub(super) fn segments(text: &str) -> Vec<(bool, String)> {
    let mut out: Vec<(bool, Vec<&str>)> = Vec::new();
    let mut fenced = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
        }
        let (quote, body) = match trimmed.strip_prefix('>') {
            Some(rest) if !fenced => (true, rest.strip_prefix(' ').unwrap_or(rest)),
            _ => (false, line),
        };
        match out.last_mut() {
            Some((kind, lines)) if *kind == quote => lines.push(body),
            _ => out.push((quote, vec![body])),
        }
    }
    out.into_iter()
        .map(|(quote, lines)| (quote, lines.join("\n").trim_matches('\n').trim_end().to_string()))
        .filter(|(_, text)| !text.trim().is_empty())
        .collect()
}

impl Workspace {
    fn reply_thread(&self) -> Option<&str> {
        match &self.view {
            View::Code(view) => Some(&view.id),
            _ => None,
        }
    }

    /// The open session's stacked replies.
    pub(super) fn pending_replies(&self) -> &[Reply] {
        self.reply_thread()
            .and_then(|thread| self.replies.stacks.get(thread))
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// The open session's stack went with a message.
    pub(super) fn replies_sent(&mut self, thread: &str) {
        if let Some(stack) = self.replies.stacks.remove(thread) {
            let gone = |id: Option<u64>| id.is_some_and(|id| stack.iter().any(|reply| reply.id == id));
            if gone(self.replies.editing) {
                self.replies.editing = None;
            }
            if gone(self.replies.hovered) {
                self.replies.hovered = None;
            }
        }
    }

    /// Open the reply box over `quote`, at `at`; `listen`: dictate the reply.
    pub(super) fn begin_reply(
        &mut self,
        quote: String,
        at: Point<Pixels>,
        listen: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(thread) = self.reply_thread().map(str::to_string) else {
            return;
        };
        if quote.trim().is_empty() {
            return;
        }
        let agent = self
            .code_session()
            .map_or("the agent", |session| super::code_dock::agent_name(session.agent));
        let placeholder: SharedString = format!("Reply to {agent}").into();
        self.replies.composing = Some(Composing { thread, quote, at });
        self.replies.input.update(cx, |input, cx| {
            input.set_placeholder(placeholder, window, cx);
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        if listen {
            self.start_dictation_into(Some(self.replies.input.clone()), window, cx);
        }
        cx.notify();
    }

    /// Close the reply box; what it held goes.
    pub(super) fn close_reply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.replies.composing.take().is_none() {
            return;
        }
        if self.dictation.target.is_some() {
            self.cancel_dictation();
        }
        self.focus_composer(window, cx);
        cx.notify();
    }

    /// Put the reply box's passage and reply on the stack, over the composer.
    fn add_reply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(composing) = self.replies.composing.take() else {
            return;
        };
        if self.dictation.target.is_some() {
            self.cancel_dictation();
        }
        let text = self.replies.input.read(cx).value().trim().to_string();
        self.replies.next += 1;
        let reply = Reply {
            id: self.replies.next,
            quote: composing.quote,
            text,
        };
        self.replies.stacks.entry(composing.thread).or_default().push(reply);
        self.replies
            .input
            .update(cx, |input, cx| input.set_value("", window, cx));
        TextSelection::clear(window, cx);
        self.focus_composer(window, cx);
        cx.notify();
    }

    /// A press outside the reply box closes it. True when the press is inside it.
    pub(super) fn reply_box_pressed(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.replies.is_composing() {
            return false;
        }
        if self
            .replies
            .bounds
            .get()
            .is_some_and(|bounds| bounds.contains(&position))
        {
            return true;
        }
        self.close_reply(window, cx);
        false
    }

    /// The reply box over the passage: the passage, then the reply, the microphone and the add button.
    pub(super) fn render_reply_box(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let composing = self.replies.composing.as_ref()?;
        if self.reply_thread() != Some(composing.thread.as_str()) {
            return None;
        }
        let listening = self.dictation.busy() && self.dictation.target.is_some();
        let icon = |id: &'static str, icon: Lucide, color: Hsla| {
            div()
                .id(id)
                .flex_none()
                .size(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .hover(|style| style.bg(palette.hover))
                .child(Icon::new(icon).size(px(16.)).text_color(color))
        };
        let card = v_flex()
            .debug_selector(|| "reply-box".into())
            .relative()
            .w(px(320.))
            .gap(px(10.))
            .pt(px(12.))
            .pb(px(10.))
            .pl(px(16.))
            .pr(px(10.))
            .rounded(px(14.))
            .border_1()
            .border_color(palette.border)
            .shadow_lg()
            // On Frost and Glass the passage under the box blurs; it read through the see-through fill.
            .floating_ground(14., palette.menu, true, &palette)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.close_reply(window, cx);
                }
            }))
            .child(
                h_flex()
                    .gap(px(8.))
                    .pr(px(6.))
                    .child(
                        div()
                            .flex_none()
                            .w(px(2.))
                            .h(px(18.))
                            .rounded(px(1.))
                            .bg(palette.border),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .text_size(px(13.))
                            .text_color(palette.muted)
                            .child(composing.quote.split_whitespace().collect::<Vec<_>>().join(" ")),
                    ),
            )
            .child(
                h_flex()
                    .items_end()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_size(px(14.))
                            .child(Textarea::new(&self.replies.input).appearance(false)),
                    )
                    .child(
                        icon(
                            "reply-mic",
                            if listening { Lucide::MicOff } else { Lucide::Mic },
                            if listening { palette.foreground } else { palette.muted },
                        )
                        .tooltip(move |window, cx| {
                            Tooltip::new(if listening { "Stop dictating" } else { "Dictate" }).build(window, cx)
                        })
                        .on_click(cx.listener(|this, _, window, cx| {
                            if this.dictation.busy() {
                                this.stop_dictation(cx);
                            } else {
                                this.start_dictation_into(Some(this.replies.input.clone()), window, cx);
                            }
                        })),
                    )
                    .child(
                        icon("reply-add", Lucide::MessageCirclePlus, palette.muted)
                            .tooltip(|window, cx| Tooltip::new("Add to message (Enter)").build(window, cx))
                            .on_click(cx.listener(|this, _, window, cx| this.add_reply(window, cx))),
                    ),
            )
            .child(bounds_probe(self.replies.bounds.clone()));
        Some(
            deferred(
                anchored()
                    .position(composing.at)
                    .anchor(Anchor::BottomLeft)
                    .snap_to_window_with_margin(px(8.))
                    .child(card),
            )
            .with_priority(3)
            .into_any_element(),
        )
    }
}

mod chips;

#[cfg(test)]
#[path = "stacked_replies_tests.rs"]
mod tests;
