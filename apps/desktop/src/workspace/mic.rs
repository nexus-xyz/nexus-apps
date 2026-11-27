//! Dictation in the composer, as the Nexus app's (`ui/src/components/nexus-ui/dictation/mic-button.tsx`,
//! `kit/src/dictation.ts`): the mic button grows into a pill while it listens, with a blinking dot, the voice's level
//! as bars, a timer and a stop button. The words go in where the caret was, live, and stay in the composer to read
//! over before sending. Gateway's voice token says where to stream ([`crate::dictation`]).

use crate::workspace::overlay::{Overlay, OverlaySlot};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Position, TextareaState};
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::Workspace;
use crate::dictation::{DictationEvent, compose};
use crate::microphones::{Choice, Kind, Microphones};
use crate::motion::{self, TweenExt as _};
use crate::palette::Palette;

/// The bars in the pill.
pub(super) const BARS: usize = 22;
/// The pill's width while listening, and how long it takes to grow from the button.
const PILL: f32 = 188.;
const GROW: Duration = Duration::from_millis(600);
/// Said once per device, the first time the person dictates, so they know where their voice goes.
const PRIVACY: &str = "While you dictate, your voice goes to our speech-to-text provider. It keeps no audio.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mic {
    Idle,
    /// Asked for the token and the microphone; nothing heard yet.
    Starting,
    Listening,
    /// The microphone or the service said no; the button shows it until the next try.
    Refused,
}

/// A dictation's state in the window.
pub(super) struct MicState {
    pub mic: Mic,
    pub id: u64,
    pub handle: Option<crate::dictation::Dictation>,
    pub levels: VecDeque<f32>,
    pub started: Option<Instant>,
    /// The composer's text on either side of the caret when dictation began.
    pub before: String,
    pub after: String,
    /// The composer's text as dictation last set it. After a stop, late words land only while it is unchanged.
    pub last: String,
    /// Whether the note was shown alongside a dictation that started, in this run or on this device before. Read from
    /// disk once, at the first dictation.
    pub told_privacy: Option<bool>,
    pub _tick: Option<Task<()>>,
    /// The system's microphones, read each time the menu opens; None while they are read.
    pub microphones: Option<Microphones>,
    /// The chosen microphone, as saved on this computer; None for the system default.
    pub choice: Option<Choice>,
    /// What the note under the composer offers after its words, while the note is dictation's.
    pub link: Option<NoteLink>,
    pub _listing: Option<Task<()>>,
    /// Where the words go when not in the composer: a stacked reply's box (`stacked_replies.rs`).
    pub target: Option<Entity<TextareaState>>,
}

/// The action a dictation note ends with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NoteLink {
    /// Open the microphone menu: no microphone, or the chosen one is not connected.
    Choose,
    /// Open the system's microphone permission (or, on Linux, its sound settings): the microphone was refused.
    Privacy,
}

impl NoteLink {
    fn label(self) -> &'static str {
        match self {
            NoteLink::Choose => "Choose a microphone",
            NoteLink::Privacy => "Open System Settings",
        }
    }
}

/// The menu's width: the longest names (PipeWire's descriptions) truncate, not the menu.
const MENU_WIDTH: f32 = 264.;

/// The note when the chosen microphone is not connected and dictation uses the default.
fn fell_back_note(wanted: &str, using: Option<&str>) -> String {
    format!(
        "{wanted} isn't connected. Using {}.",
        using.unwrap_or("the system default")
    )
}

/// The link a dictation problem's note offers.
fn problem_link(problem: crate::dictation::Problem) -> Option<NoteLink> {
    use crate::dictation::Problem;
    match problem {
        Problem::NoMicrophone => Some(NoteLink::Choose),
        Problem::Denied => Some(NoteLink::Privacy),
        Problem::Unavailable | Problem::Failed => None,
    }
}

fn kind_icon(kind: Kind) -> Lucide {
    match kind {
        Kind::BuiltIn => Lucide::Laptop,
        Kind::Headset => Lucide::Headphones,
        Kind::Usb => Lucide::Usb,
        Kind::Other => Lucide::Mic,
    }
}

impl MicState {
    pub fn new() -> MicState {
        MicState {
            mic: Mic::Idle,
            id: 0,
            handle: None,
            levels: VecDeque::from(vec![0.; BARS]),
            started: None,
            before: String::new(),
            after: String::new(),
            last: String::new(),
            told_privacy: None,
            _tick: None,
            microphones: None,
            choice: crate::paths::dictation_microphone(),
            link: None,
            _listing: None,
            target: None,
        }
    }

    pub fn busy(&self) -> bool {
        matches!(self.mic, Mic::Starting | Mic::Listening)
    }

    /// Whether this dictation shows the privacy note: it has not yet been shown with a dictation that started, in this
    /// run or on this device (`told_on_device`, read once).
    fn privacy_note_due(&mut self, told_on_device: impl FnOnce() -> bool) -> bool {
        !*self.told_privacy.get_or_insert_with(told_on_device)
    }

    /// The dictation started listening with the note up: the person has been told. True only the first time, when the
    /// device should remember it. A dictation that fails before it listens (no microphone, denied, no service) does
    /// not count, so the next one that streams voice still shows the note (as Nexus UI marks it only on `listening`).
    fn privacy_note_heard(&mut self) -> bool {
        if self.told_privacy == Some(false) {
            self.told_privacy = Some(true);
            true
        } else {
            false
        }
    }
}

impl Workspace {
    pub(super) fn toggle_dictation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.dictation.mic {
            Mic::Idle | Mic::Refused => self.start_dictation_into(None, window, cx),
            Mic::Starting | Mic::Listening => self.stop_dictation(cx),
        }
    }

    /// The text field dictation writes into: `target`, or the composer.
    fn dictation_field(&self) -> Entity<TextareaState> {
        self.dictation.target.clone().unwrap_or_else(|| self.composer.clone())
    }

    /// Start dictating into `target`, or into the composer when it is None.
    pub(super) fn start_dictation_into(
        &mut self,
        target: Option<Entity<TextareaState>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(account) = self.account_gateway() else { return };
        if let Some(mut handle) = self.dictation.handle.take() {
            handle.cancel();
        }
        self.dictation.target = target;
        let (value, cursor) = {
            let state = self.dictation_field().read(cx);
            (state.value().to_string(), state.cursor())
        };
        let cursor = cursor.min(value.len());
        self.overlay.close_mic_menu();
        let state = &mut self.dictation;
        state.link = None;
        state.id += 1;
        state.mic = Mic::Starting;
        state.before = value[..cursor].to_string();
        state.after = value[cursor..].to_string();
        state.last = value.clone();
        state.levels = VecDeque::from(vec![0.; BARS]);
        state.started = None;
        // Remembered on this device only once the dictation listens (`on_dictation`).
        self.mic_note = state
            .privacy_note_due(crate::paths::dictation_notice_told)
            .then(|| PRIVACY.into());
        state.handle = account.dictate(state.id);
        if state.handle.is_none() {
            state.mic = Mic::Idle;
            return;
        }
        // The timer moves once a second; the window draws again for it.
        state._tick = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(500)).await;
                let Ok(busy) = this.update(cx, |this, cx| {
                    cx.notify();
                    this.dictation.busy()
                }) else {
                    break;
                };
                if !busy {
                    break;
                }
            }
        }));
        match &self.dictation.target {
            Some(field) => field.update(cx, |field, cx| field.focus(window, cx)),
            None => self.focus_composer(window, cx),
        }
        cx.notify();
    }

    /// Stop listening and keep the last words. The button is back at once: the service's last words (up to a few
    /// seconds) land in the composer after it, unless the person typed in the meantime.
    pub(super) fn stop_dictation(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = &mut self.dictation.handle {
            handle.stop();
        }
        if self.dictation.busy() {
            self.dictation.mic = Mic::Idle;
            self.mic_note = None;
        }
        cx.notify();
    }

    /// End a dictation now, keeping what is already in the composer (on send, or a new chat).
    pub(super) fn cancel_dictation(&mut self) {
        if let Some(mut handle) = self.dictation.handle.take() {
            handle.cancel();
        }
        if self.dictation.busy() {
            self.dictation.mic = Mic::Idle;
        }
        self.dictation.target = None;
        self.mic_note = None;
    }

    pub(super) fn on_dictation(&mut self, id: u64, event: DictationEvent, window: &mut Window, cx: &mut Context<Self>) {
        if id != self.dictation.id || self.dictation.handle.is_none() {
            return;
        }
        match event {
            // A stop while starting wins over the service's late hello.
            DictationEvent::Listening if self.dictation.mic == Mic::Starting => {
                self.dictation.mic = Mic::Listening;
                self.dictation.started = Some(Instant::now());
                // Voice streams from now, with the note up: it has been shown, so this device remembers it.
                if self.mic_note.as_ref().is_some_and(|note| note.as_ref() == PRIVACY)
                    && self.dictation.privacy_note_heard()
                {
                    crate::paths::mark_dictation_notice_told();
                }
            }
            DictationEvent::Listening => {}
            // The privacy note, when it is up, is the one that must be read; the fallback waits for the next time.
            DictationEvent::FellBack { wanted, using }
                if !self.mic_note.as_ref().is_some_and(|note| note.as_ref() == PRIVACY) =>
            {
                self.mic_note = Some(fell_back_note(&wanted, using.as_deref()).into());
                self.dictation.link = Some(NoteLink::Choose);
            }
            DictationEvent::FellBack { .. } => {}
            DictationEvent::Level(level) => {
                let levels = &mut self.dictation.levels;
                levels.pop_front();
                levels.push_back(level);
            }
            DictationEvent::Text(words) => {
                // Stopped, and the person has typed since: the late words would overwrite them.
                let field = self.dictation_field();
                if !self.dictation.busy() && *field.read(cx).value() != *self.dictation.last {
                    return;
                }
                let text = compose(&self.dictation.before, &words, &self.dictation.after);
                // The caret sits after the words, before what followed it.
                let head = &text[..text.len() - self.dictation.after.len()];
                let line = head.matches('\n').count() as u32;
                let character = head.rsplit('\n').next().unwrap_or("").chars().count() as u32;
                field.update(cx, |state, cx| {
                    state.set_value(text.clone(), window, cx);
                    state.set_cursor_position(Position { line, character }, window, cx);
                });
                if self.dictation.target.is_none() {
                    self.composer_empty = text.trim().is_empty();
                }
                self.suggestion = None;
                self.dictation.last = text;
            }
            DictationEvent::Finishing => {}
            DictationEvent::Ended => {
                self.dictation.mic = Mic::Idle;
                self.dictation.handle = None;
                self.mic_note = None;
            }
            // After a stop, a failure only loses the last words: no need to say so.
            DictationEvent::Failed(_) if !self.dictation.busy() => self.dictation.handle = None,
            DictationEvent::Failed(problem) => {
                self.dictation.mic = Mic::Refused;
                self.dictation.handle = None;
                self.mic_note = Some(problem.message().into());
                self.dictation.link = problem_link(problem);
            }
        }
        cx.notify();
    }

    /// Open the microphone menu and read the system's microphones for it, off the UI thread.
    pub(super) fn open_mic_menu(&mut self, cx: &mut Context<Self>) {
        if self.dictation.busy() {
            return;
        }
        self.overlay.close_attach_menu();
        self.overlay.close_model_menu();
        self.open_overlay(Overlay::MicMenu);
        self.dictation.microphones = None;
        let listing = cx.background_executor().spawn(async { crate::microphones::list() });
        self.dictation._listing = Some(cx.spawn(async move |this, cx| {
            let microphones = listing.await;
            let _ = this.update(cx, |this, cx| {
                this.dictation.microphones = Some(microphones);
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(super) fn close_mic_menu(&mut self) {
        self.overlay.close_mic_menu();
        self.dictation._listing = None;
    }

    /// Use `choice` from now on (None: the system default), and save it on this computer.
    fn choose_microphone(&mut self, choice: Option<Choice>, cx: &mut Context<Self>) {
        self.close_mic_menu();
        if let Err(problem) = crate::paths::save_dictation_microphone(choice.as_ref()) {
            self.composer_note = Some(problem.into());
        }
        self.dictation.choice = choice;
        // A note that asked for a microphone has its answer.
        if self.dictation.link == Some(NoteLink::Choose) {
            self.mic_note = None;
            self.dictation.link = None;
            if self.dictation.mic == Mic::Refused {
                self.dictation.mic = Mic::Idle;
            }
        }
        cx.notify();
    }

    fn open_sound_settings(&mut self, cx: &mut Context<Self>) {
        self.close_mic_menu();
        if !crate::microphones::open_sound_settings() {
            self.composer_note = Some("Nexus couldn't open the sound settings.".into());
        }
        cx.notify();
    }

    fn follow_note_link(&mut self, link: NoteLink, cx: &mut Context<Self>) {
        match link {
            NoteLink::Choose => self.open_mic_menu(cx),
            NoteLink::Privacy => {
                if !crate::microphones::open_privacy_settings() {
                    self.composer_note = Some("Nexus couldn't open the system settings.".into());
                }
                cx.notify();
            }
        }
    }

    /// The link at the end of dictation's note under the composer, while that note shows.
    pub(super) fn render_mic_note_link(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.mic_note.as_ref()?;
        let link = self.dictation.link?;
        Some(
            div()
                .id("mic-note-link")
                .text_color(palette.foreground)
                .underline()
                .cursor_pointer()
                .child(link.label())
                .on_click(cx.listener(move |this, _, _, cx| this.follow_note_link(link, cx)))
                .into_any_element(),
        )
    }

    /// The mic and, at its right, the chevron that opens the microphone menu (above them, right-aligned).
    fn render_mic_split(&self, button: Stateful<Div>, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let open = self.overlay.mic_menu();
        let chevron = div()
            .id("mic-choose")
            .w(px(16.))
            .h(px(30.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .text_color(if open { palette.foreground } else { palette.faint })
            .cursor_pointer()
            .hover(|style| style.text_color(palette.strong))
            .when(!open, |chevron| {
                chevron.tooltip(|window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("Choose a microphone").build(window, cx)
                })
            })
            // Opens only: while the menu is open the window's backdrop covers this, and a press there closes it.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.open_mic_menu(cx);
                }),
            )
            .child(Icon::new(Lucide::ChevronDown).size(px(12.)).with_tween(
                "mic-chevron",
                if open { 1. } else { 0. },
                motion::Tween::new(motion::COLLAPSE, motion::ease_out),
                |icon, turn| icon.rotate(radians(std::f32::consts::PI * turn)),
            ));
        let split = h_flex()
            .id("mic-split")
            .flex_none()
            .rounded(px(8.))
            .when(open, |split| split.bg(palette.control_fill(true)))
            .child(button)
            .child(chevron);
        div()
            .relative()
            .flex_none()
            .child(split)
            .when_some(motion::presence("mic-menu", open, cx), |frame, phase| {
                let menu = phase.menu("mic-menu", 4., self.render_mic_menu(palette, cx));
                frame.child(super::code_dock::menu_layer(menu, Anchor::BottomRight))
            })
            .into_any_element()
    }

    /// The microphone menu: the system default, each microphone, and the system's sound settings.
    fn render_mic_menu(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let state = &self.dictation;
        let chosen = state.choice.as_ref().map(|choice| choice.id.as_str());
        let row = |id: ElementId, icon: Lucide, name: SharedString, sub: Option<SharedString>, checked: bool| {
            h_flex()
                .id(id)
                .min_h(px(30.))
                .py(px(4.))
                .px(px(8.))
                .gap(px(8.))
                .rounded(px(8.))
                .cursor_pointer()
                .hover(|style| super::code_dock::menu_item_hover(style, palette))
                .child(
                    div()
                        .w(px(15.))
                        .flex_none()
                        .children(checked.then(|| super::glyph(Lucide::Check, palette.foreground).size(px(15.)))),
                )
                .child(super::glyph(icon, palette.icon).size(px(15.)))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().truncate().child(name))
                        .children(
                            sub.map(|sub| div().text_size(px(11.)).text_color(palette.faint).truncate().child(sub)),
                        ),
                )
        };
        let quiet = |text: &'static str| {
            div()
                .px(px(8.))
                .py(px(6.))
                .text_size(px(12.))
                .text_color(palette.faint)
                .child(text)
        };
        let default_name = state
            .microphones
            .as_ref()
            .and_then(|microphones| microphones.default.clone())
            .map(SharedString::from);
        let mut menu = super::code_dock::menu_box("mic-menu", MENU_WIDTH, palette)
            .text_size(px(13.))
            .child(
                div()
                    .px(px(8.))
                    .pt(px(6.))
                    .pb(px(2.))
                    .text_size(px(11.))
                    .text_color(palette.faint)
                    .child("Microphone"),
            )
            .child(
                row(
                    "mic-default".into(),
                    Lucide::Mic,
                    "System default".into(),
                    default_name,
                    chosen.is_none(),
                )
                .on_click(cx.listener(|this, _, _, cx| this.choose_microphone(None, cx))),
            );
        match &state.microphones {
            None => menu = menu.child(quiet("Looking for microphones…")),
            Some(microphones) => {
                for (ix, microphone) in microphones.list.iter().enumerate() {
                    let choice = Choice {
                        id: microphone.id.clone(),
                        name: microphone.name.clone(),
                    };
                    menu = menu.child(
                        row(
                            ("mic-device", ix).into(),
                            kind_icon(microphone.kind),
                            microphone.name.clone().into(),
                            None,
                            chosen == Some(microphone.id.as_str()),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.choose_microphone(Some(choice.clone()), cx))),
                    );
                }
                // The chosen one is unplugged: it stays chosen, and says so.
                if let Some(choice) = state
                    .choice
                    .as_ref()
                    .filter(|choice| microphones.find(&choice.id).is_none())
                {
                    let again = choice.clone();
                    menu = menu.child(
                        row(
                            "mic-missing".into(),
                            Lucide::MicOff,
                            choice.name.clone().into(),
                            Some("Not connected".into()),
                            true,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.choose_microphone(Some(again.clone()), cx))),
                    );
                }
                if microphones.list.is_empty() {
                    menu = menu.child(quiet("No other microphones found."));
                }
            }
        }
        menu.child(div().h(px(1.)).mx(px(6.)).my(px(4.)).bg(palette.border))
            .child(
                row(
                    "mic-sound-settings".into(),
                    Lucide::SlidersHorizontal,
                    "Sound settings…".into(),
                    None,
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| this.open_sound_settings(cx))),
            )
    }

    pub(super) fn render_mic(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        if let Some(reason) = self.account_control_reason() {
            return div()
                .id("mic")
                .size(px(30.))
                .flex()
                .items_center()
                .justify_center()
                .text_color(palette.faint)
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(reason).build(window, cx))
                .child(Icon::new(Lucide::MicOff).size(px(16.)))
                .into_any_element();
        }
        // A dictation that ends folds its pill back into the button and fades out, as a menu leaves, instead of
        // vanishing; the button then fades back in.
        let phase = motion::presence("mic-pill", self.dictation.busy(), cx);
        let state = &self.dictation;
        let leaving = matches!(phase, Some(motion::Phase::Leaving { .. }));
        match state.mic {
            Mic::Idle | Mic::Refused if leaving => self.render_mic_pill(true, palette, cx),
            Mic::Idle | Mic::Refused => {
                let refused = state.mic == Mic::Refused;
                let tip: SharedString = match &state.choice {
                    Some(choice) => format!("Dictate · {}", choice.name).into(),
                    None => "Dictate".into(),
                };
                let button = div()
                    .id("mic")
                    .size(px(30.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(8.))
                    .text_color(if refused { palette.destructive } else { palette.muted })
                    .cursor_pointer()
                    .hover(|style| style.bg(palette.control_fill(false)).text_color(palette.strong))
                    .active(|style| style.bg(palette.control_fill(true)))
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                    })
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_dictation(window, cx)))
                    .child(Icon::new(if refused { Lucide::MicOff } else { Lucide::Mic }).size(px(16.)));
                let button = self.render_mic_split(button, palette, cx);
                if state.id == 0 {
                    // No dictation yet: the button loads with the composer and does not move.
                    button
                } else {
                    // Back after a dictation: it fades in once, where the pill folded away.
                    motion::fade_in(div().child(button), ("mic-back", state.id as usize)).into_any_element()
                }
            }
            _ => self.render_mic_pill(false, palette, cx),
        }
    }

    /// The listening pill: it grows out of the 30px button, and when `leaving`, folds back into it and fades out. A
    /// leaving pill takes no clicks.
    fn render_mic_pill(&self, leaving: bool, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let state = &self.dictation;
        let seconds = state.started.map_or(0, |at| at.elapsed().as_secs());
        let timer = format!("{}:{:02}", seconds / 60, seconds % 60);
        let starting = state.mic == Mic::Starting;
        let bars = h_flex()
            .gap(px(2.))
            .h(px(16.))
            .items_center()
            .children(state.levels.iter().enumerate().map(|(ix, level)| {
                let height = 3. + (level * 1.8).min(1.) * 13.;
                let bar = div().w(px(2.)).h(px(height)).rounded_full().bg(palette.muted);
                if starting {
                    // Before the first sound, the bars breathe in a wave.
                    bar.with_animation(
                        ("mic-breathe", ix),
                        Animation::new(Duration::from_millis(1200))
                            .repeat()
                            .with_max_fps(crate::motion::LOOP_FPS),
                        move |bar, delta| {
                            let phase = (delta + ix as f32 / BARS as f32) * std::f32::consts::TAU;
                            bar.h(px(3. + (phase.sin() + 1.) * 2.5))
                        },
                    )
                    .into_any_element()
                } else {
                    bar.into_any_element()
                }
            }));
        let dot = div()
            .size(px(7.))
            .flex_none()
            .rounded_full()
            .bg(palette.destructive)
            .with_animation(
                "mic-dot",
                Animation::new(Duration::from_millis(1600))
                    .repeat()
                    .with_max_fps(crate::motion::LOOP_FPS),
                |dot, delta| dot.opacity(if delta < 0.5 { 1. } else { 0.35 }),
            );
        let stop = div()
            .id("mic-stop")
            .size(px(24.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(palette.strong)
            .when(!leaving, |stop| {
                stop.cursor_pointer()
                    .hover(|style| style.opacity(0.85))
                    .on_click(cx.listener(|this, _, _, cx| this.stop_dictation(cx)))
            })
            .child(
                div()
                    .size(px(8.))
                    .rounded(px(2.))
                    .border(px(1.5))
                    .border_color(palette.background),
            );
        // Grows out of the 30px button over `GROW`; leaving, folds back over `MENU_OUT`, from wherever it is.
        let (target, tween) = if leaving {
            (0., motion::Tween::new(motion::MENU_OUT, motion::ease_out))
        } else {
            (1., motion::Tween::new(GROW, motion::ease_out).from(0.))
        };
        h_flex()
            .id("mic-pill")
            .relative()
            .h(px(32.))
            .flex_none()
            .overflow_hidden()
            .pl(px(12.))
            .pr(px(3.))
            .gap(px(8.))
            .rounded_full()
            .bg(palette.selected)
            .child(dot)
            .child(div().flex_1().min_w(px(0.)).overflow_hidden().child(bars))
            .child(
                div()
                    .flex_none()
                    .font_family("Geist Mono")
                    .text_size(px(11.))
                    .text_color(palette.muted)
                    .child(timer),
            )
            .child(stop)
            .when(leaving, |pill| {
                pill.child(div().id("mic-leaving").absolute().inset_0().occlude())
            })
            .when_some(state.choice.as_ref().filter(|_| !leaving), |pill, choice| {
                let tip: SharedString = format!("Listening on {}", choice.name).into();
                pill.tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                })
            })
            .with_tween(("mic-width", state.id as usize), target, tween, move |pill, grown| {
                let pill = pill.w(px(30. + (PILL - 30.) * grown));
                if leaving { pill.opacity(grown) } else { pill }
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // `gpui_kit::*` brings GPUI's own `test` attribute, which expands to `#[test]` again.
    use core::prelude::v1::test;

    #[test]
    fn a_dictation_that_fails_before_listening_still_owes_the_privacy_note() {
        let mut state = MicState::new();
        let mut reads = 0;
        // First press on a device that never showed it: the note shows.
        assert!(state.privacy_note_due(|| {
            reads += 1;
            false
        }));
        // The service says no (Denied, NoMicrophone, Unavailable): nothing marks it, so the next press shows it again.
        assert!(state.privacy_note_due(|| unreachable!("the device is read once")));
        assert_eq!(reads, 1);
        // That one listens: told now, and the device remembers it once.
        assert!(state.privacy_note_heard());
        assert!(!state.privacy_note_heard());
        assert!(!state.privacy_note_due(|| unreachable!()));
    }

    #[test]
    fn a_device_that_was_told_shows_no_note_and_writes_nothing() {
        let mut state = MicState::new();
        assert!(!state.privacy_note_due(|| true));
        assert!(!state.privacy_note_heard());
    }
}
