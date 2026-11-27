//! The feedback dialog, as Nexus UI's (`FeedbackProvider` in `components/nexus-ui/feedback.tsx`), over the window. The
//! tray's Send Feedback… and Help › Send Feedback… (macOS) open it (`windows.rs`). Bug, Idea or Other, a message of
//! up to 4,000 characters, Cancel and Send; Ctrl+Enter (⌘↩ on macOS) sends from the text box. What the person typed
//! stays when the dialog closes or the send fails, and clears once it went. The words and the body are `feedback.rs`'s.

use crate::workspace::glass::FloatingGround;
use crate::workspace::overlay::{Overlay, OverlaySlot};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{View, Workspace, glyph};
use crate::feedback::{self, Kind};
use crate::gateway::Verb;
use crate::pages::ui;
use crate::palette::Palette;

const ENDPOINT: &str = "/v1/diagnostics/feedback";

/// Why a send failed, under the message: a channel no one set up on this Gateway is a quiet line nobody in the app can
/// fix; any other failure is the red ✕ and the words, worth another try. The crash prompt shares it.
pub(super) fn problem_line(key: &'static str, problem: SharedString, palette: Palette, cx: &App) -> AnyElement {
    if feedback::unset(&problem) {
        return crate::setup::quiet::info_line(key, Lucide::Info, problem, palette, cx).into_any_element();
    }
    h_flex()
        .items_start()
        .gap(px(8.))
        .text_size(px(13.))
        .text_color(palette.foreground)
        .child(
            div().flex_none().child(
                gpui_kit::component::Icon::new(Lucide::X)
                    .size(px(16.))
                    .text_color(palette.status_failed),
            ),
        )
        .child(div().flex_1().min_w(px(0.)).child(problem))
        .into_any_element()
}

/// The dialog's state, kept while it is closed so the message stays. Whether it is open is `Overlay::Feedback`.
#[derive(Default)]
pub struct FeedbackDialog {
    kind: Kind,
    /// Made the first time it opens: a text box needs the window.
    text: Option<Entity<TextareaState>>,
    /// The message's length in characters, for the counter and Send.
    chars: usize,
    empty: bool,
    busy: bool,
    sent: bool,
    problem: Option<SharedString>,
    _task: Option<Task<()>>,
    _subscription: Option<Subscription>,
}

impl Workspace {
    /// Send Feedback…: open the dialog on its form, the message as the person left it.
    pub(crate) fn open_feedback(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_local() {
            return;
        }
        let text = match self.feedback.text.clone() {
            Some(text) => text,
            None => {
                let kind = self.feedback.kind;
                let text = cx.new(|cx| TextareaState::new(window, cx).placeholder(kind.placeholder()).rows(6));
                self.feedback._subscription = Some(cx.subscribe_in(
                    &text,
                    window,
                    |this, state, event: &InputEvent, window, cx| match event {
                        InputEvent::Change => {
                            let value = state.read(cx).value().to_string();
                            // The text box's `maxLength`: a longer paste is cut to the bound.
                            if let Some(cut) = feedback::bounded(&value) {
                                state.update(cx, |state, cx| state.set_value(cut, window, cx));
                            }
                            let value = state.read(cx).value();
                            this.feedback.chars = value.chars().count();
                            this.feedback.empty = value.trim().is_empty();
                            cx.notify();
                        }
                        InputEvent::PressEnter { secondary: true, .. } => this.send_feedback(window, cx),
                        _ => {}
                    },
                ));
                self.feedback.text = Some(text.clone());
                self.feedback.empty = true;
                text
            }
        };
        self.open_overlay(Overlay::Feedback);
        self.feedback.sent = false;
        self.feedback.problem = None;
        text.update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }

    fn close_feedback(&mut self, cx: &mut Context<Self>) {
        self.overlay.close_feedback();
        cx.notify();
    }

    fn pick_feedback_kind(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        self.feedback.kind = kind;
        if let Some(text) = &self.feedback.text {
            text.update(cx, |state, cx| state.set_placeholder(kind.placeholder(), window, cx));
        }
        cx.notify();
    }

    /// The screen the person is on, as Nexus UI's route for it (`feedback::screen_of`): never a chat's title or id.
    fn feedback_screen(&self) -> Option<String> {
        let org = self.account.as_ref().map(|account| account.org.clone())?;
        let path = match &self.view {
            View::Chat if self.current.is_some() => format!("/app/{org}/sessions/:id"),
            View::Chat => format!("/app/{org}"),
            View::Code(_) | View::Pull(_) => format!("/app/{org}/sessions/:id"),
            View::NewCode => format!("/app/{org}/sessions"),
            View::TeamSessions => format!("/app/{org}/team/sessions"),
            View::Library => format!("/app/{org}/library"),
            View::Inbox => format!("/app/{org}/notifications"),
            View::Docs => "/docs".into(),
            View::Issues => format!("/app/{org}/issues"),
            View::Models => format!("/app/{org}/models"),
            View::Page(id) | View::Settings(super::settings_view::SettingsSection::Page(id)) => {
                format!("/app/{org}/{}", id.web_path())
            }
            View::Settings(super::settings_view::SettingsSection::ApiKeys) => format!("/app/{org}/settings/api-keys"),
            View::Settings(_) => format!("/app/{org}/settings/preferences"),
        };
        Some(feedback::screen_of(&path))
    }

    /// Send: the kind, the message, the screen and this build; the form becomes the thanks once it went.
    fn send_feedback(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        let Some(text) = self.feedback.text.clone() else {
            return;
        };
        let message = text.read(cx).value().to_string();
        if self.feedback.busy || message.trim().is_empty() || self.account.is_none() {
            return;
        }
        let kind = self.feedback.kind;
        let screen = self.feedback_screen();
        self.feedback.busy = true;
        self.feedback.problem = None;
        cx.notify();
        let gateway = account_gateway.clone();
        self.feedback._task = Some(cx.spawn_in(window, async move |this, cx| {
            // The OS version is read once, off the UI thread.
            let os_version = cx.background_spawn(async { crate::crash::os_version() }).await;
            let body = feedback::body(kind, &message, screen, &os_version);
            let sent = gateway.request(Verb::Post, ENDPOINT.into(), Some(body)).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.feedback.busy = false;
                match sent {
                    Ok(_) => {
                        this.feedback.sent = true;
                        text.update(cx, |state, cx| state.set_value("", window, cx));
                        this.feedback.chars = 0;
                        this.feedback.empty = true;
                    }
                    Err(error) => {
                        this.feedback.problem = Some(feedback::failure(&error, "Feedback and crash reports").into())
                    }
                }
                cx.notify();
            });
        }));
    }

    /// The dialog over the window, while it is open.
    pub(super) fn render_feedback(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let state = &self.feedback;
        if !self.overlay.feedback() {
            return None;
        }
        let head = |title: &'static str| {
            h_flex()
                .child(
                    div()
                        .flex_1()
                        .text_size(px(16.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(palette.strong)
                        .child(title),
                )
                .child(
                    div()
                        .id("feedback-close")
                        .size(px(24.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(|style| style.bg(palette.hover))
                        .child(glyph(Lucide::X, palette.icon))
                        .on_click(cx.listener(|this, _, _, cx| this.close_feedback(cx))),
                )
        };
        let note = |text: String| div().text_size(px(13.)).text_color(palette.muted).child(text);
        let dialog = v_flex()
            .id("feedback-dialog")
            .occlude()
            .w(px(460.))
            .max_w(relative(0.94))
            .p(px(20.))
            .gap(px(12.))
            .rounded(px(14.))
            .floating_ground(14., palette.pop, false, &palette)
            .shadow(palette.pop_shadow(palette.border))
            .text_color(palette.foreground)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.close_feedback(cx);
                    cx.stop_propagation();
                }
            }));
        let dialog = if state.sent {
            let email = self.account.as_ref().map(|account| account.email.as_str());
            dialog
                .child(head(feedback::SENT_TITLE))
                .child(note(feedback::sent_note(email)))
                .child(
                    h_flex().justify_end().child(
                        ui::primary_button("feedback-done", "Done", palette)
                            .on_click(cx.listener(|this, _, _, cx| this.close_feedback(cx))),
                    ),
                )
        } else {
            let text = state.text.as_ref()?;
            let can_send = !state.busy && !state.empty && self.account.is_some();
            let send_label = if state.busy { "Sending…" } else { "Send" };
            dialog
                .child(head(feedback::TITLE))
                .child(note(feedback::NOTE.into()))
                .child(h_flex().child(ui::tabs(palette).children(Kind::ALL.map(|kind| {
                    ui::tab(
                        ("feedback-kind", kind as usize),
                        kind.label(),
                        kind == state.kind,
                        palette,
                    )
                    .on_click(cx.listener(move |this, _, window, cx| this.pick_feedback_kind(kind, window, cx)))
                }))))
                .child(
                    div()
                        .p(px(8.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(palette.border)
                        .text_size(px(13.))
                        .child(Textarea::new(text).appearance(false).disabled(state.busy)),
                )
                .children(
                    feedback::count(state.chars)
                        .map(|count| div().text_size(px(12.)).text_color(palette.faint).child(count)),
                )
                .child(div().text_size(px(12.)).text_color(palette.faint).child(feedback::HINT))
                .when_some(state.problem.clone(), |dialog, problem| {
                    dialog.child(problem_line("feedback:send", problem, palette, cx))
                })
                .child(
                    h_flex()
                        .gap(px(8.))
                        .justify_end()
                        .child(
                            ui::button("feedback-cancel", "Cancel", palette)
                                .on_click(cx.listener(|this, _, _, cx| this.close_feedback(cx))),
                        )
                        .child(if can_send {
                            ui::primary_button("feedback-send", send_label, palette)
                                .on_click(cx.listener(|this, _, window, cx| this.send_feedback(window, cx)))
                        } else {
                            ui::disabled_button("feedback-send", send_label, palette)
                        }),
                )
        };
        Some(
            deferred(
                div()
                    .id("feedback-overlay")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .occlude()
                    .bg(palette.scrim)
                    .flex()
                    .items_center()
                    .justify_center()
                    // A click outside closes it, as Nexus UI's dialog; the message stays.
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            if !this.feedback.busy {
                                this.close_feedback(cx);
                            }
                        }),
                    )
                    .child(dialog),
            )
            .with_priority(3)
            .into_any_element(),
        )
    }
}
