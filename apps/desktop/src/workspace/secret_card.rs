//! An agent's request for a secret, in its session's dock in place of the composer (the Nexus app's
//! `secret-request-card.tsx`): what the secret is and why, a password field, Decline and Provide.
//!
//! The value stays in the card's masked field until Provide hands it to the runner (`secret_request.rs`), which
//! writes it to a private file and gives the agent only the path. It never enters the transcript, a draft, the
//! notification or a log. Decline ends the request with no file. The field empties after either answer, and when the
//! card changes to another request.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_desktop_mcp::secret_request::SecretRequest;

use super::code_dock::{agent_name, answer_button, card, card_head};
use super::{View, Workspace};
use crate::palette::Palette;

/// The card's state, kept with the open session's dock.
pub(super) struct SecretCard {
    /// The password field.
    field: Entity<InputState>,
    /// The request the field was typed for.
    for_id: Option<String>,
    sending: bool,
    problem: Option<SharedString>,
}

impl SecretCard {
    pub fn new(window: &mut Window, cx: &mut Context<Workspace>) -> (SecretCard, Subscription) {
        let field = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("Paste the secret…")
        });
        // Enter provides it, as the Nexus app's form submits; typing turns Provide on.
        let subscription = cx.subscribe_in(
            &field,
            window,
            |this: &mut Workspace, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.answer_secret(true, window, cx),
                InputEvent::Change => cx.notify(),
                _ => {}
            },
        );
        let card = SecretCard {
            field,
            for_id: None,
            sending: false,
            problem: None,
        };
        (card, subscription)
    }
}

impl Workspace {
    /// The open session's request for a secret, while this build runs its agent.
    pub(super) fn open_secret_request(&self) -> Option<SecretRequest> {
        let View::Code(view) = &self.view else {
            return None;
        };
        self.runner.as_ref()?.secret_request(&view.id)
    }

    /// The card follows the open session's request: a new one starts with an empty field, which takes the focus.
    pub(super) fn follow_secret_request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let request = self.open_secret_request().map(|request| request.id);
        let View::Code(view) = &mut self.view else {
            return;
        };
        let card = &mut view.dock.secret;
        if card.for_id == request {
            return;
        }
        card.for_id = request.clone();
        card.sending = false;
        card.problem = None;
        card.field.update(cx, |state, cx| {
            state.set_value("", window, cx);
            if request.is_some() {
                state.focus(window, cx);
            }
        });
        cx.notify();
    }

    /// Provide the value typed (`provide`), or decline.
    fn answer_secret(&mut self, provide: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(request) = self.open_secret_request() else {
            return;
        };
        let (View::Code(view), Some(runner)) = (&mut self.view, &self.runner) else {
            return;
        };
        let card = &mut view.dock.secret;
        let value = card.field.read(cx).value().to_string();
        if card.sending || (provide && value.is_empty()) {
            return;
        }
        card.sending = true;
        card.problem = None;
        let answered = runner.answer_secret(request.id, provide.then_some(value));
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = answered
                .await
                .unwrap_or_else(|_| Err("Nexus stopped running Code sessions here.".into()));
            let _ = this.update_in(cx, |this, window, cx| {
                let View::Code(view) = &mut this.view else {
                    return;
                };
                let card = &mut view.dock.secret;
                card.sending = false;
                match result {
                    Ok(()) => card.field.update(cx, |state, cx| state.set_value("", window, cx)),
                    Err(error) => card.problem = Some(error.into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn render_secret_card(
        &self,
        view: &super::code_view::CodeView,
        request: &SecretRequest,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let card_state = &view.dock.secret;
        let name = self
            .code_session()
            .map(|session| agent_name(session.agent))
            .unwrap_or("The agent");
        let typed = !card_state.field.read(cx).value().is_empty();
        let sending = card_state.sending;
        card(palette)
            .child(card_head(
                Lucide::KeyRound,
                format!("{name} asks for a secret"),
                palette,
            ))
            .child(
                v_flex()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(request.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(palette.muted)
                            .child(request.reason.clone()),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .h(px(32.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(palette.border)
                    .bg(palette.background)
                    .text_size(px(13.))
                    .child(Input::new(&card_state.field).appearance(false).disabled(sending)),
            )
            .child(div().text_size(px(12.5)).text_color(palette.faint).child(
                "The agent gets a private file that holds it, never the value. The file is deleted when the agent \
                 stops, or after 15 minutes.",
            ))
            .when_some(card_state.problem.clone(), |card, problem| {
                card.child(div().text_size(px(13.)).text_color(palette.destructive).child(problem))
            })
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.5))
                            .text_color(palette.faint)
                            .child("This agent is waiting for your answer."),
                    )
                    .child(
                        answer_button("secret-decline", "Decline", false, !sending, palette)
                            .on_click(cx.listener(|this, _, window, cx| this.answer_secret(false, window, cx))),
                    )
                    .child(
                        answer_button(
                            "secret-provide",
                            if sending { "Providing…" } else { "Provide" },
                            true,
                            typed && !sending,
                            palette,
                        )
                        .on_click(cx.listener(|this, _, window, cx| this.answer_secret(true, window, cx))),
                    ),
            )
            .into_any_element()
    }
}
