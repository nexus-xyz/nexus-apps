//! The pending linked-session decision, before other dock questions.
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::session_choice::{ChoiceKey, Preference, StartChoice, shortcut};

use super::code_dock::{MODES, agent_name, answer_button, card, card_head};
use super::{View, Workspace};
use crate::palette::Palette;

pub(super) struct StartCard {
    request: Option<String>,
    focus: FocusHandle,
    sending: bool,
    details: bool,
    options: bool,
    problem: Option<SharedString>,
}
impl StartCard {
    pub fn new(cx: &mut App) -> Self {
        Self {
            request: None,
            focus: cx.focus_handle(),
            sending: false,
            details: false,
            options: false,
            problem: None,
        }
    }
}
impl Workspace {
    pub(super) fn follow_session_start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let request = self
            .code_session()
            .and_then(|s| s.start_choices.first())
            .map(|c| c.id.clone());
        let View::Code(view) = &mut self.view else { return };
        let card = &mut view.start_card;
        if card.request == request {
            return;
        }
        let restore_focus = request.is_none() && card.focus.contains_focused(window, cx);
        card.request = request;
        card.sending = false;
        card.problem = None;
        card.details = false;
        card.options = false;
        if card.request.is_some() {
            card.focus.focus(window, cx);
        } else if restore_focus {
            self.focus_composer(window, cx);
        }
    }

    fn answer_session_start(&mut self, choice: &'static str, remember: bool, cx: &mut Context<Self>) {
        let Some(session) = self.code_session() else { return };
        let Some(request) = session.start_choices.first() else {
            return;
        };
        if remember && !request.can_remember {
            return;
        }
        let id = session.id.clone();
        let request = request.id.clone();
        let Some(runs) = self.runs(&id) else { return };
        let View::Code(view) = &mut self.view else { return };
        if view.start_card.sending {
            return;
        }
        view.start_card.sending = true;
        view.start_card.problem = None;
        let read = runs.call(
            "wb_session_start_answer",
            serde_json::json!({
                "threadId": id, "requestId": request, "choice": choice, "remember": remember
            }),
        );
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = read
                .await
                .unwrap_or_else(|_| Err("The session choice's answer was lost.".into()));
            let _ = this.update(cx, |this, cx| {
                let View::Code(view) = &mut this.view else { return };
                if view.id != id || view.start_card.request.as_deref() != Some(&request) {
                    return;
                }
                // Keep the controls locked after success until the summary removes this request.
                if let Err(problem) = result {
                    view.start_card.sending = false;
                    view.start_card.problem = Some(problem.into());
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn render_session_start(
        &self,
        view: &super::code_view::CodeView,
        choice: &StartChoice,
        count: usize,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = &view.start_card;
        let sending = state.sending;
        let mode = MODES
            .iter()
            .find(|(m, _, _)| *m == choice.permission_mode)
            .map(|(_, label, _)| *label)
            .unwrap_or("Ask first");
        card(palette)
            .id("session-start-card")
            .track_focus(&state.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                let mods = event.keystroke.modifiers;
                // This card has no composing input. A menu consumes its key before bubble listeners run.
                let choice = shortcut(ChoiceKey {
                    key: &event.keystroke.key,
                    control: mods.control,
                    command: mods.platform,
                    alt: mods.alt,
                    shift: mods.shift,
                    ..Default::default()
                })
                .map(|choice| if choice == Preference::New { "new" } else { "here" });
                if let Some(choice) = choice {
                    cx.stop_propagation();
                    this.answer_session_start(choice, false, cx);
                }
            }))
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(div().flex_1().child(card_head(
                        Lucide::GitBranch,
                        "Start this in its own session?".into(),
                        palette,
                    )))
                    .when(count > 1, |row| row.child(format!("{count} requests")))
                    .child(
                        answer_button("start-dismiss", "×", false, !sending, palette)
                            .on_click(cx.listener(|this, _, _, cx| this.answer_session_start("here", false, cx))),
                    ),
            )
            .child(div().font_weight(FontWeight::MEDIUM).child(choice.title.clone()))
            .child(div().text_color(palette.muted).child(format!(
                "{}{} · {} · {mode}",
                agent_name(choice.agent),
                choice.payer().map(|payer| format!(" on {payer}")).unwrap_or_default(),
                choice.model.as_deref().unwrap_or("Default model")
            )))
            .child(
                div()
                    .text_color(palette.muted)
                    .child(format!("New worktree in {}", choice.workspace)),
            )
            .child(
                answer_button("start-details", "Task details", false, !sending, palette).on_click(cx.listener(
                    |this, _, _, cx| {
                        if let View::Code(view) = &mut this.view {
                            view.start_card.details = !view.start_card.details;
                            cx.notify();
                        }
                    },
                )),
            )
            .when(state.details, |element| {
                element.child(
                    div()
                        .id("start-prompt")
                        .max_h(px(180.))
                        .overflow_y_scroll()
                        .child(choice.prompt.clone()),
                )
            })
            .child(
                answer_button("start-options", "Don’t ask again ▾", false, !sending, palette).on_click(cx.listener(
                    |this, _, _, cx| {
                        if let View::Code(view) = &mut this.view {
                            view.start_card.options = !view.start_card.options;
                            cx.notify();
                        }
                    },
                )),
            )
            .when(state.options, |element| {
                element.child(
                    v_flex()
                        .gap(px(4.))
                        .child(
                            answer_button(
                                "start-always-new",
                                "Always start a new session",
                                false,
                                !sending && choice.can_remember,
                                palette,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.answer_session_start("new", true, cx))),
                        )
                        .child(
                            answer_button(
                                "start-always-here",
                                "Always do it here",
                                false,
                                !sending && choice.can_remember,
                                palette,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.answer_session_start("here", true, cx))),
                        )
                        .child(
                            answer_button("start-settings", "Settings", false, !sending, palette).on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.open_settings(
                                        super::SettingsSection::Page(crate::pages::PageId::Subsessions),
                                        window,
                                        cx,
                                    )
                                }),
                            ),
                        ),
                )
            })
            .child(
                h_flex()
                    .gap(px(8.))
                    .justify_end()
                    .child(
                        answer_button("start-here", "Do it here  Esc", false, !sending, palette)
                            .on_click(cx.listener(|this, _, _, cx| this.answer_session_start("here", false, cx))),
                    )
                    .child(
                        answer_button(
                            "start-new",
                            if sending {
                                "Saving…"
                            } else {
                                "New session  Ctrl/⌘↵"
                            },
                            true,
                            !sending,
                            palette,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.answer_session_start("new", false, cx))),
                    ),
            )
            .when_some(state.problem.clone(), |element, problem| {
                element.child(div().text_color(palette.destructive).child(problem))
            })
            .into_any_element()
    }
}
