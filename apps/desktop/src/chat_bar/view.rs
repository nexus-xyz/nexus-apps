//! The chat bar's view: the composer row (the mark, the question, Auto), then the answer under a hairline with
//! its footer (model and time, Copy, Open in Nexus ⌘↩). The answer streams into a Markdown view of its own, so a
//! streamed word redraws this small window only.

use std::sync::Arc;
use std::time::Instant;

use futures::StreamExt as _;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::text::TextViewState;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{ANSWER_MAX, Body, HEADER, MARGIN, WIDTH};
use crate::agent::{Chunk, Pane, Stream};
use crate::backend::Backend;
use crate::palette::{Palette, ThemeChoice};
use nexus_desktop_core::agent as rules;

pub(super) struct ChatBar {
    gateway: Arc<dyn Backend>,
    pane: Pane,
    composer: Entity<TextareaState>,
    answer: Entity<TextViewState>,
    /// The answer's text and model as last shown, for the window's height and Copy.
    text: String,
    model: Option<String>,
    started: Option<Instant>,
    seconds: Option<u64>,
    asking: Option<Task<()>>,
    /// The window has had the focus: only then does losing it close an idle bar.
    had_focus: bool,
    body: Body,
    window_id: Option<WindowId>,
    _subscriptions: Vec<Subscription>,
}

impl ChatBar {
    pub(super) fn new(
        gateway: Arc<dyn Backend>,
        conversation: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ChatBar {
        // The thread's conversation, not read back: the bar only asks in it.
        let (mut pane, _) = Pane::new(None);
        pane.conversation = conversation;
        pane.surface = Some(super::SURFACE);
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Ask Nexus")
                .submit_on_enter(true)
                .auto_grow(1, 4)
        });
        composer.update(cx, |state, cx| state.focus(window, cx));
        let answer = cx.new(|cx| TextViewState::markdown("", cx));
        let this = cx.entity().downgrade();
        let subscriptions = vec![
            cx.subscribe_in(&composer, window, |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { secondary, shift } = event {
                    if *secondary {
                        this.open_in_nexus(cx);
                    } else if !*shift {
                        this.send(window, cx);
                    }
                }
            }),
            // Esc closes the bar; ⌘↩ opens the thread. Read before the text field's own keys.
            cx.intercept_keystrokes(move |event, window, cx| {
                let keystroke = &event.keystroke;
                if keystroke.key != "escape" || keystroke.modifiers.modified() {
                    return;
                }
                let here = this
                    .update(cx, |bar, _| bar.window_id == Some(window.window_handle().window_id()))
                    .unwrap_or(false);
                if here {
                    super::hide_later(cx);
                    cx.stop_propagation();
                }
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.had_focus = true;
                } else if this.had_focus && this.idle(cx) {
                    super::hide_later(cx);
                }
            }),
        ];
        ChatBar {
            gateway,
            pane,
            composer,
            answer,
            text: String::new(),
            model: None,
            started: None,
            seconds: None,
            asking: None,
            had_focus: false,
            body: Body::Empty,
            window_id: Some(window.window_handle().window_id()),
            _subscriptions: subscriptions,
        }
    }

    /// Nothing typed and nothing asked: a click elsewhere may close it.
    fn idle(&self, cx: &App) -> bool {
        self.body == Body::Empty && self.composer.read(cx).value().trim().is_empty()
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).value().to_string();
        let Some(call) = self.pane.ask(&text, Instant::now()) else {
            return;
        };
        self.composer.update(cx, |state, cx| state.set_value("", window, cx));
        self.text.clear();
        self.model = None;
        self.seconds = None;
        self.started = Some(Instant::now());
        self.answer.update(cx, |state, cx| state.set_text("", cx));
        // Read at each call, as the thread does.
        let base = rules::base_url(crate::desktop_settings::load().agents_api_url.as_deref());
        let Stream { mut chunks, _stop } = self.gateway.agent(base, call);
        self.asking = Some(cx.spawn_in(window, async move |this, cx| {
            // Held until the bar drops this task: then the call stops.
            let _stop = _stop;
            while let Some(chunk) = chunks.next().await {
                if this
                    .update_in(cx, |bar, window, cx| bar.chunk(chunk, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
        self.show(window, cx);
    }

    fn chunk(&mut self, chunk: Chunk, window: &mut Window, cx: &mut Context<Self>) {
        // A notification is for an app window that is not in front: the bar is in front.
        let _ = self.pane.ask_chunk(chunk, Instant::now());
        match &self.pane.draft {
            Some(draft) => {
                self.text = draft.text.clone();
                let text = self.text.clone();
                self.answer.update(cx, |state, cx| state.set_text(&text, cx));
            }
            None => {
                if self.asking.take().is_some()
                    && let Some((text, model)) = super::answer_of(&self.pane)
                {
                    self.answer.update(cx, |state, cx| state.set_text(&text, cx));
                    self.text = text;
                    self.model = model;
                    self.seconds = self.started.map(super::took);
                }
            }
        }
        self.show(window, cx);
    }

    /// The pane changed: the body, and the window's height to fit it.
    fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let body = super::body_of(&self.pane);
        let height = super::height(&body, &self.text);
        self.body = body;
        window.resize(size(px(WIDTH + 2. * MARGIN), px(height)));
        cx.notify();
    }

    fn copy(&mut self, cx: &mut Context<Self>) {
        if !self.text.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.text.clone()));
        }
    }

    fn open_in_nexus(&mut self, cx: &mut Context<Self>) {
        // Nothing asked yet: Open in Nexus only brings the app forward.
        super::open_in_nexus(self.pane.conversation.clone(), cx);
    }

    fn palette(window: &Window) -> Palette {
        Palette::for_choice(ThemeChoice::saved(), window.appearance())
    }

    fn footer(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let quiet = palette.icon;
        let meta = match (&self.model, self.seconds) {
            (Some(model), Some(seconds)) => format!("Auto → {model} · {seconds} s"),
            (Some(model), None) => model.clone(),
            _ => String::new(),
        };
        h_flex()
            .gap(px(12.))
            .text_size(px(12.5))
            .text_color(quiet)
            .child(div().flex_1().child(meta))
            .child(
                div()
                    .id("chat-bar-copy")
                    .cursor_pointer()
                    .text_color(palette.muted)
                    .child("Copy")
                    .on_click(cx.listener(|this, _, _, cx| this.copy(cx))),
            )
            .child(
                h_flex()
                    .id("chat-bar-open")
                    .gap(px(6.))
                    .cursor_pointer()
                    .text_color(palette.foreground)
                    .child("Open in Nexus")
                    .child(
                        div()
                            .font_family("Geist Mono")
                            .text_size(px(11.5))
                            .text_color(quiet)
                            .child("⌘↩"),
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.open_in_nexus(cx))),
            )
    }

    fn lower(&self, palette: Palette, window: &Window, cx: &mut Context<Self>) -> Option<Div> {
        let words = |column: Div| {
            column.child(
                div()
                    .id("chat-bar-answer")
                    .max_h(px(ANSWER_MAX))
                    .overflow_y_scroll()
                    .text_size(px(14.5))
                    .line_height(px(22.))
                    .text_color(palette.foreground)
                    .child(crate::workspace::prose(
                        crate::external_link::text_view(&self.answer).selectable(true),
                        palette,
                    )),
            )
        };
        let column = v_flex().gap(px(12.)).pl(px(34.));
        match &self.body {
            Body::Empty => None,
            Body::Thinking(line) => {
                let rest = crate::pages::agent::quiet_ink(palette);
                let line = SharedString::from(line.clone());
                let shown = if cx.reduce_motion() {
                    div().text_color(rest).child(line).into_any_element()
                } else {
                    crate::motion::shimmer("chat-bar-step", line, rest, palette.strong).into_any_element()
                };
                Some(column.child(div().text_size(px(13.)).line_height(px(22.)).child(shown)))
            }
            Body::Writing => Some(words(column)),
            Body::Answered { needs_you } => {
                let column = words(column);
                let column = if *needs_you {
                    column.child(
                        div()
                            .text_size(px(13.))
                            .text_color(palette.strong)
                            .child("Needs you — open in Nexus"),
                    )
                } else {
                    column
                };
                Some(column.child(self.footer(palette, cx)))
            }
            Body::Failed { title, detail } => Some(
                column
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(palette.foreground)
                            .child(title.clone()),
                    )
                    .child(div().text_size(px(13.)).text_color(palette.muted).child(detail.clone())),
            ),
        }
    }
}

impl Render for ChatBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Self::palette(window);
        let thinking = matches!(self.body, Body::Thinking(_) | Body::Writing);
        let lower = self.lower(palette, window, cx);
        let fill = palette.raised.opacity(0.96);
        div().size_full().p(px(MARGIN)).child(
            v_flex()
                .w(px(WIDTH))
                .rounded(px(22.))
                .bg(fill)
                .border_1()
                .border_color(palette.border)
                .shadow(vec![BoxShadow {
                    color: black().opacity(0.28),
                    offset: point(px(0.), px(16.)),
                    blur_radius: px(40.),
                    spread_radius: px(-12.),
                    inset: false,
                }])
                .child(
                    h_flex()
                        .h(px(HEADER))
                        .px(px(18.))
                        .gap(px(12.))
                        .child(crate::pages::agent::avatar("chat-bar-mark", 22., thinking, palette))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .text_size(px(17.))
                                .child(Textarea::new(&self.composer).appearance(false)),
                        )
                        .child(div().text_size(px(13.)).text_color(palette.muted).child("Auto")),
                )
                .when_some(lower, |card, lower| {
                    card.child(div().h(px(1.)).mx(px(18.)).bg(palette.border))
                        .child(div().pt(px(2.)).pb(px(16.)).pr(px(18.)).pl(px(18.)).child(lower))
                }),
        )
    }
}
