//! Voice mode's pane (Nexus UI's `voice-pane.tsx`): beside the thread, or over it in a narrow window.
//! The mark at 120 px with its voice ring, one word under it, then the last two turns as captions (You quiet, Nexus
//! larger), and Mute · Type · End at the foot. End is the only filled button. Type opens a box under the captions.
//! Esc or End leaves voice and keeps the conversation: the summary card and the writes the call asked for land in the
//! thread ([`VoiceEvent::Ended`]). While Nexus speaks, Space talks over it. The pane itself takes the focus when it
//! opens, not a button, so Space never presses End by accident.
//!
//! The pane runs the call (`call.rs`): it does each [`Command`] (the ears, the ask, the voice, the timers) and tells
//! the call what came of it. It redraws only when what it shows changed; the level goes to the mark, which draws
//! its own frames (`mark.rs`).

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use futures::StreamExt as _;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_desktop_core::agent::{self as rules, Message};

use super::ask::AskReading;
use super::call::{Call, Command, Timer, VoiceReply, VoiceState};
use super::ears::{Heard, Hearing};
use super::mark::VoiceMark;
use super::speaker::Speaker;
use super::spoken::{VoiceSummary, summarize};

/// How often the pane looks whether the voice still speaks.
const SPEAKING_POLL: Duration = Duration::from_millis(100);
/// Without word events the level pulses this often, slower than 3 Hz (`browser-voice.ts`'s `PULSE_MS`).
const PULSE: Duration = Duration::from_millis(350);
/// A voice may report silence for a moment after it starts: an end sooner than this is not trusted.
const SPEECH_GRACE: Duration = Duration::from_millis(300);
/// The pane's width beside the thread.
pub const WIDTH: f32 = 400.;

/// The ears and the agent the call uses: the Gateway backend in the app, fakes in the tests.
pub trait Senses {
    /// Open the microphone and the speech service.
    fn hear(&self) -> Hearing;
    /// One call to the agent API.
    fn ask(&self, call: crate::agent::Call) -> crate::agent::Stream;
}

/// The app's senses: the backend's ears, and the agent API the Assistant asks.
pub struct BackendSenses(pub(crate) crate::workspace::gateway_access::AccountGateway);

impl Senses for BackendSenses {
    fn hear(&self) -> Hearing {
        self.0.hear()
    }

    fn ask(&self, call: crate::agent::Call) -> crate::agent::Stream {
        // Read at each call, as the Assistant reads the setting.
        let base = rules::base_url(crate::desktop_settings::load().agents_api_url.as_deref());
        self.0.agent(base, call)
    }
}

/// What the pane tells the Assistant.
#[derive(Clone, Debug, PartialEq)]
pub enum VoiceEvent {
    /// The call asked in this conversation, or started it.
    Kept(String),
    /// The call ended and nothing is still coming: its card, if anything was said, and the writes it asked for.
    Ended {
        summary: Option<VoiceSummary>,
        writes: Vec<Message>,
    },
    /// Switch to text: the call ends and the caret goes to the composer.
    SwitchToText,
}

/// What the pane shows, to tell whether it must redraw.
#[derive(Clone, PartialEq)]
struct Shown {
    state: VoiceState,
    heard: String,
    said: String,
    problem: Option<String>,
    muted: bool,
}

pub struct VoicePane {
    call: Call,
    senses: Rc<dyn Senses>,
    speaker: Box<dyn Speaker>,
    mark: Entity<VoiceMark>,
    level: Rc<Cell<f32>>,
    conversation: Option<String>,
    focus: FocusHandle,
    typing: Option<Entity<InputState>>,
    /// The executor's clock and the wall clock when the call started: the call's times are epoch ms.
    origin: (Instant, i64),
    landed: bool,
    _hearing: Option<Task<()>>,
    _asking: Option<Task<()>>,
    _speaking: Option<Task<()>>,
    _yield: Option<Task<()>>,
    _retry: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<VoiceEvent> for VoicePane {}

impl VoicePane {
    /// Open the pane and start the call in `conversation`. The microphone is asked for now, and only now.
    pub fn new(
        senses: Rc<dyn Senses>,
        speaker: Box<dyn Speaker>,
        conversation: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> VoicePane {
        let level = Rc::new(Cell::new(0.));
        let mark = cx.new(|cx| VoiceMark::new(level.clone(), cx));
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let wall = chrono::Utc::now().timestamp_millis();
        let mut pane = VoicePane {
            call: Call::new(wall),
            senses,
            speaker,
            mark,
            level,
            conversation,
            focus,
            typing: None,
            origin: (cx.background_executor().now(), wall),
            landed: false,
            _hearing: None,
            _asking: None,
            _speaking: None,
            _yield: None,
            _retry: None,
            _subscriptions: Vec::new(),
        };
        pane.act(cx, |call, _| call.start());
        pane
    }

    /// Epoch ms now, on the executor's clock so the tests can move it.
    fn now(&self, cx: &App) -> i64 {
        let (instant, wall) = self.origin;
        wall + cx
            .background_executor()
            .now()
            .saturating_duration_since(instant)
            .as_millis() as i64
    }

    #[cfg(test)]
    pub fn state(&self) -> VoiceState {
        self.call.state()
    }

    /// The call is over: the pane no longer shows, though an answer still coming may land.
    pub fn ended(&self) -> bool {
        self.call.ended()
    }

    fn shown(&self) -> Shown {
        Shown {
            state: self.call.state(),
            heard: self.call.heard.clone(),
            said: self.call.said.clone(),
            problem: self.call.problem.clone(),
            muted: self.call.muted(),
        }
    }

    /// Change the call, do what it asks, and redraw what changed.
    fn act(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut Call, i64)) {
        let before = self.shown();
        let now = self.now(cx);
        change(&mut self.call, now);
        loop {
            let commands = self.call.take_commands();
            if commands.is_empty() {
                break;
            }
            for command in commands {
                self.run(command, cx);
            }
        }
        if let Some(level) = self.call.take_level() {
            self.level.set(level);
        }
        let state = self.call.state();
        self.mark.update(cx, |mark, cx| mark.set_state(state, cx));
        if self.shown() != before {
            cx.notify();
        }
        if !self.landed
            && let Some(record) = self.call.record()
        {
            self.landed = true;
            let writes = record
                .messages
                .iter()
                .filter(|message| matches!(message, Message::PendingAction { .. }))
                .cloned()
                .collect();
            cx.emit(VoiceEvent::Ended {
                summary: summarize(&record),
                writes,
            });
        }
    }

    fn run(&mut self, command: Command, cx: &mut Context<Self>) {
        match command {
            Command::Open { opening } => {
                let Hearing { mut events, _stop } = self.senses.hear();
                self._hearing = Some(cx.spawn(async move |this, cx| {
                    // Held while the ears are open: dropping this task closes the microphone.
                    let _stop = _stop;
                    while let Some(heard) = events.next().await {
                        if this.update(cx, |pane, cx| pane.heard(opening, heard, cx)).is_err() {
                            break;
                        }
                    }
                }));
            }
            Command::Close => self._hearing = None,
            Command::Ask { turn, words } => {
                let body = rules::ask_body(&words, self.conversation.as_deref());
                let crate::agent::Stream { mut chunks, _stop } = self.senses.ask(crate::agent::Call::Ask { body });
                self._asking = Some(cx.spawn(async move |this, cx| {
                    let _stop = _stop;
                    let mut reading = AskReading::default();
                    while let Some(chunk) = chunks.next().await {
                        let last = matches!(chunk, crate::agent::Chunk::End | crate::agent::Chunk::Broken);
                        if let Some(conversation) = reading.chunk(chunk)
                            && this.update(cx, |pane, cx| pane.keep(conversation, cx)).is_err()
                        {
                            return;
                        }
                        if last {
                            break;
                        }
                    }
                    let reply = reading.finish();
                    if let Err(error) = this.update(cx, |pane, cx| pane.answered(turn, reply, cx)) {
                        eprintln!("nexus-desktop-gpui: voice: the answer came after the pane closed: {error}");
                    }
                }));
            }
            Command::Speak { turn, text } => {
                if !self.speaker.speak(&text) {
                    self._speaking = None;
                    self.call.spoken(turn);
                    return;
                }
                self.call.word(turn);
                self._speaking = Some(cx.spawn(async move |this, cx| {
                    let started = cx.background_executor().now();
                    let mut pulsed = started;
                    loop {
                        cx.background_executor().timer(SPEAKING_POLL).await;
                        let now = cx.background_executor().now();
                        let go = this.update(cx, |pane, cx| {
                            let speaking = pane.speaker.speaking() || now.duration_since(started) < SPEECH_GRACE;
                            if !speaking {
                                pane.act(cx, |call, _| call.spoken(turn));
                                return false;
                            }
                            if now.duration_since(pulsed) >= PULSE {
                                pulsed = now;
                                pane.act(cx, |call, _| call.word(turn));
                            }
                            true
                        });
                        if !go.unwrap_or(false) {
                            break;
                        }
                    }
                }));
            }
            Command::Hush => {
                self.speaker.stop();
                self._speaking = None;
            }
            Command::Later { timer, ms } => {
                let task = cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(Duration::from_millis(ms)).await;
                    this.update(cx, |pane, cx| pane.act(cx, |call, now| call.fired(timer, now)))
                        .ok();
                });
                match timer {
                    Timer::Yield(_) => self._yield = Some(task),
                    Timer::Retry(_) => self._retry = Some(task),
                }
            }
        }
    }

    fn heard(&mut self, opening: u64, heard: Heard, cx: &mut Context<Self>) {
        self.act(cx, |call, now| match heard {
            Heard::Opened => call.ears_opened(opening, now),
            Heard::Failed(problem) => call.ears_failed(opening, problem),
            Heard::Words(words) => call.heard_words(&words),
            Heard::Turn(said) => call.heard_turn(&said, now),
            Heard::Level(value) => call.heard_level(value, now),
            Heard::Lost => call.heard_lost(),
        });
    }

    fn answered(&mut self, turn: u64, reply: VoiceReply, cx: &mut Context<Self>) {
        self.act(cx, |call, now| call.answered(turn, reply, now));
    }

    fn keep(&mut self, conversation: String, cx: &mut Context<Self>) {
        if self.conversation.as_deref() != Some(conversation.as_str()) {
            self.conversation = Some(conversation.clone());
            cx.emit(VoiceEvent::Kept(conversation));
        }
    }

    pub fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        self.act(cx, |call, now| call.toggle_mute(now));
    }

    /// Talk over Nexus (Space).
    pub fn interrupt(&mut self, cx: &mut Context<Self>) {
        self.act(cx, |call, _| call.interrupt());
    }

    pub fn retry(&mut self, cx: &mut Context<Self>) {
        self.act(cx, |call, _| call.retry());
    }

    /// Leave voice (Esc or End). What was said lands in the thread once an answer still coming has landed.
    pub fn end(&mut self, cx: &mut Context<Self>) {
        self.typing = None;
        self.act(cx, |call, now| call.end(now));
        cx.notify();
    }

    /// Type: a box under the captions, its words asked as a turn.
    fn toggle_typing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.typing.take().is_some() {
            self._subscriptions.clear();
            window.focus(&self.focus, cx);
            cx.notify();
            return;
        }
        let field = cx.new(|cx| InputState::new(window, cx).placeholder("Type to Nexus"));
        field.update(cx, |state, cx| state.focus(window, cx));
        self._subscriptions = vec![
            cx.subscribe_in(&field, window, |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.send_typed(window, cx);
                }
            }),
        ];
        self.typing = Some(field);
        cx.notify();
    }

    fn send_typed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(field) = &self.typing else { return };
        let words = field.read(cx).value().trim().to_string();
        if words.is_empty() {
            return;
        }
        field.update(cx, |state, cx| state.set_value("", window, cx));
        self.act(cx, |call, now| call.type_words(&words, now));
    }

    /// Type, as the tests type it.
    #[cfg(test)]
    pub fn type_words(&mut self, words: &str, cx: &mut Context<Self>) {
        self.act(cx, |call, now| call.type_words(words, now));
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.modified() {
            return;
        }
        match keystroke.key.as_str() {
            "escape" => {
                if self
                    .typing
                    .as_ref()
                    .is_some_and(|field| field.focus_handle(cx).is_focused(window))
                {
                    self.toggle_typing(window, cx);
                } else {
                    self.end(cx);
                }
                cx.stop_propagation();
            }
            // Space talks over Nexus, only with the pane itself focused: in the box or on a button it is theirs.
            "space" if self.focus.is_focused(window) && self.call.state() == VoiceState::Speaking => {
                self.interrupt(cx);
                cx.stop_propagation();
            }
            _ => {}
        }
    }
}

/// A pill at the pane's foot (`.voice-pane-button`): a hairline, and filled for End.
fn pill(id: &'static str, filled: bool, palette: crate::palette::Palette) -> Stateful<Div> {
    h_flex()
        .id(id)
        .debug_selector(move || id.to_string())
        .flex_none()
        .min_w(px(88.))
        .h(px(40.))
        .px(px(18.))
        .gap(px(6.))
        .justify_center()
        .rounded_full()
        .text_size(px(14.))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .map(|pill| {
            if filled {
                pill.bg(palette.primary)
                    .text_color(palette.primary_foreground)
                    .hover(|style| style.opacity(0.9))
            } else {
                pill.border_1()
                    .border_color(palette.border)
                    .text_color(palette.strong)
                    .hover(|style| style.bg(palette.hover))
            }
        })
}

impl Render for VoicePane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = crate::pages::palette(cx);
        let state = self.call.state();
        let label = |text: &'static str| div().text_size(px(11.)).text_color(palette.faint).child(text);
        let link = |id: &'static str, text: &'static str| {
            div()
                .id(id)
                .debug_selector(move || id.to_string())
                .text_color(palette.muted)
                .underline()
                .cursor_pointer()
                .hover(|style| style.text_color(palette.strong))
                .child(text)
        };
        let problem = self.call.problem.clone().map(|problem| {
            // The system refused the microphone: the way there, as dictation's note has it (`workspace/mic.rs`).
            let denied = problem == super::call::DENIED;
            h_flex()
                .id("voice-problem")
                .debug_selector(|| "voice-problem".into())
                .flex_wrap()
                .gap_x(px(8.))
                .gap_y(px(4.))
                .text_size(px(13.))
                .text_color(palette.faint)
                .child(problem)
                .when(denied, |line| {
                    line.child(
                        crate::setup::quiet::link("voice-open-privacy", "Open System Settings", palette)
                            .debug_selector(|| "voice-open-privacy".into())
                            .on_click(cx.listener(|this, _, _, cx| {
                                if !crate::microphones::open_privacy_settings() {
                                    this.call.problem = Some("Nexus couldn't open the system settings.".into());
                                }
                                cx.notify();
                            })),
                    )
                })
                .when(state == VoiceState::PoorConnection, |line| {
                    line.child(link("voice-switch-to-text", "Switch to text?").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.end(cx);
                            cx.emit(VoiceEvent::SwitchToText);
                        },
                    )))
                })
                .when(state == VoiceState::CantHear, |line| {
                    line.child(
                        link("voice-try-again", "Try again").on_click(cx.listener(|this, _, _, cx| this.retry(cx))),
                    )
                })
                .when(denied, |line| {
                    line.child(
                        link("voice-type-instead", "Type instead").on_click(cx.listener(|this, _, _, cx| {
                            this.end(cx);
                            cx.emit(VoiceEvent::SwitchToText);
                        })),
                    )
                })
        });
        let muted = self.call.muted();
        v_flex()
            .id("voice-pane")
            .debug_selector(|| "voice-pane".into())
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .size_full()
            .pt(px(12.))
            .px(px(24.))
            .pb(px(20.))
            .bg(crate::frost::cover(&palette))
            .text_color(palette.foreground)
            .child(
                h_flex()
                    .min_h(px(32.))
                    .justify_between()
                    .child(div().text_size(px(13.)).text_color(palette.faint).child("Voice"))
                    .child(
                        div()
                            .id("voice-esc")
                            .px(px(6.))
                            .py(px(1.))
                            .rounded(px(4.))
                            .border_1()
                            .border_color(palette.border)
                            .text_size(px(11.))
                            .text_color(palette.faint)
                            .tooltip(|window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(
                                    "Esc leaves voice and keeps the conversation",
                                )
                                .build(window, cx)
                            })
                            .child("esc"),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h(px(0.))
                    .items_center()
                    .justify_center()
                    .gap(px(4.))
                    .child(self.mark.clone())
                    .child(
                        div()
                            .id("voice-word")
                            .debug_selector(|| "voice-word".into())
                            .text_size(px(15.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(palette.strong)
                            .child(state.word()),
                    ),
            )
            .child(
                v_flex()
                    .id("voice-captions")
                    .gap(px(10.))
                    .min_h(px(132.))
                    .pb(px(20.))
                    .when(!self.call.heard.is_empty(), |captions| {
                        captions.child(
                            v_flex()
                                .id("voice-caption-you")
                                .debug_selector(|| "voice-caption-you".into())
                                .child(label("You"))
                                .child(
                                    div()
                                        .text_size(px(13.5))
                                        .line_height(relative(1.45))
                                        .text_color(palette.muted)
                                        .child(self.call.heard.clone()),
                                ),
                        )
                    })
                    .when(!self.call.said.is_empty(), |captions| {
                        captions.child(
                            v_flex()
                                .id("voice-caption-nexus")
                                .debug_selector(|| "voice-caption-nexus".into())
                                .child(label("Nexus"))
                                .child(
                                    div()
                                        .text_size(px(16.))
                                        .line_height(relative(1.45))
                                        .text_color(palette.strong)
                                        .child(self.call.said.clone()),
                                ),
                        )
                    })
                    .children(problem)
                    .when(muted && self.call.problem.is_none(), |captions| {
                        captions.child(
                            div()
                                .text_size(px(13.))
                                .text_color(palette.faint)
                                .child("Muted. Unmute to talk."),
                        )
                    })
                    .children(self.typing.clone().map(|field| {
                        h_flex()
                            .id("voice-type-box")
                            .debug_selector(|| "voice-type-box".into())
                            .gap(px(8.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .h(px(36.))
                                    .px(px(12.))
                                    .flex()
                                    .items_center()
                                    .rounded_full()
                                    .bg(palette.raised)
                                    .border_1()
                                    .border_color(palette.border)
                                    .text_size(px(14.))
                                    .child(Input::new(&field).appearance(false)),
                            )
                            .child(
                                pill("voice-type-send", false, palette)
                                    .min_w(px(0.))
                                    .h(px(36.))
                                    .on_click(cx.listener(|this, _, window, cx| this.send_typed(window, cx)))
                                    .child("Send"),
                            )
                    })),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(px(8.))
                    .justify_between()
                    .child(
                        pill("voice-mute", false, palette)
                            .when(muted, |pill| pill.bg(palette.hover))
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_mute(cx)))
                            .child(
                                Icon::new(if muted { Lucide::MicOff } else { Lucide::Mic })
                                    .size(px(15.))
                                    .text_color(palette.strong),
                            )
                            .child(if muted { "Unmute" } else { "Mute" }),
                    )
                    .child(
                        pill("voice-type", false, palette)
                            .when(self.typing.is_some(), |pill| pill.bg(palette.hover))
                            .on_click(cx.listener(|this, _, window, cx| this.toggle_typing(window, cx)))
                            .child("Type"),
                    )
                    .child(
                        pill("voice-end", true, palette)
                            .on_click(cx.listener(|this, _, _, cx| this.end(cx)))
                            .child("End"),
                    ),
            )
    }
}

#[cfg(test)]
#[path = "pane_tests.rs"]
mod tests;
