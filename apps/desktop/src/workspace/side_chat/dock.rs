//! The side chat dock, as the session's: notes, a secret or question card, the command tray, the queued chips, and
//! the composer.
use super::*;
use crate::palette::Palette;
use crate::workspace::code_dock::{self, answer_button, reason_field};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
impl SideChat {
    pub(super) fn render_dock(&self, window: &Window, p: Palette, cx: &mut Context<Self>) -> Div {
        let id = self.summary.id.clone();
        let line = |text: String, color| div().px(px(4.)).text_size(px(12.)).text_color(color).child(text);
        let mut dock = v_flex().px(px(12.)).pb(px(12.)).pt(px(8.)).gap(px(8.)).flex_none();
        if !self.ready {
            dock = dock.child(line("Reconnecting to the side chat…".into(), p.muted));
        }
        if let Some(problem) = &self.problem {
            dock = dock.child(line(problem.clone(), p.destructive));
        }
        if let Some(note) = &self.note {
            dock = dock.child(line(note.clone(), p.muted));
        }
        if self.summary.pending.is_some() {
            dock = dock.child(line("Settings changes await the agent.".into(), p.faint));
        }
        if self.secret_id.is_some() {
            let mut card = v_flex()
                .w_full()
                .p(px(12.))
                .gap(px(8.))
                .rounded(px(16.))
                .bg(p.raised)
                .border_1()
                .border_color(p.border);
            if let Runs::Here(runner) = &self.runs
                && let Some(secret) = runner.secret_request(&id)
            {
                card = card
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(secret.name),
                    )
                    .child(div().text_size(px(13.)).text_color(p.muted).child(secret.reason));
            }
            dock = dock.child(
                card.child(h_flex().child(reason_field(&self.secret, p))).child(
                    h_flex()
                        .justify_end()
                        .gap(px(8.))
                        .child(
                            answer_button("side-secret-decline", "Decline", false, !self.sending, p)
                                .on_click(cx.listener(|this, _, window, cx| this.answer_secret(false, window, cx))),
                        )
                        .child(
                            answer_button("side-secret-provide", "Provide privately", true, !self.sending, p)
                                .on_click(cx.listener(|this, _, window, cx| this.answer_secret(true, window, cx))),
                        ),
                ),
            );
        } else if let Some(block @ Block::Approval { tool, .. }) = code::pending_approval(&self.blocks)
            && tool != code_dock::QUESTION_TOOL
            && tool != code_dock::PLAN_TOOL
        {
            dock = dock
                .child(self.render_tool_permission(block, p, cx))
                .children(self.render_queued(p, cx))
                .child(self.render_message_composer(window, p, cx));
        } else if let Some(Block::Approval {
            tool,
            input,
            description,
            ..
        }) = code::pending_approval(&self.blocks)
        {
            let question = tool == code_dock::QUESTION_TOOL;
            let mut card = v_flex()
                .w_full()
                .p(px(12.))
                .gap(px(8.))
                .rounded(px(16.))
                .bg(p.raised)
                .border_1()
                .border_color(p.border)
                .text_size(px(13.))
                .child(div().text_size(px(14.)).font_weight(FontWeight::MEDIUM).child(
                    description.clone().unwrap_or_else(|| {
                        if question {
                            "The agent asks you".into()
                        } else {
                            format!("Allow {tool}?")
                        }
                    }),
                ));
            if question {
                for (ix, question) in code_dock::questions_of(input).iter().enumerate() {
                    card = card.child(div().text_color(p.foreground).child(question.question.clone()));
                    if let Some(field) = self.answers.get(ix) {
                        for (option, description) in &question.options {
                            let option = option.clone();
                            let field = field.clone();
                            let multi = question.multi;
                            card = card.child(
                                code_dock::menu_row(
                                    SharedString::from(format!("side-question-{ix}-{option}")),
                                    option.clone().into(),
                                    description.clone().map(Into::into),
                                    false,
                                    p,
                                )
                                .on_click(cx.listener(move |_, _, window, cx| {
                                    field.update(cx, |field, cx| {
                                        let current = field.value().to_string();
                                        field.set_value(
                                            if multi && !current.trim().is_empty() {
                                                format!("{current}; {option}")
                                            } else {
                                                option.clone()
                                            },
                                            window,
                                            cx,
                                        );
                                    });
                                })),
                            );
                        }
                        card = card.child(h_flex().child(reason_field(field, p)));
                    }
                }
            } else if tool == code_dock::PLAN_TOOL {
                card = card.child(div().text_color(p.muted).child(code_dock::plan_of(input).to_string()));
            } else {
                card = card.child(div().text_color(p.muted).child(input.to_string()));
            }
            dock = dock.child(
                card.child(
                    h_flex()
                        .justify_end()
                        .gap(px(8.))
                        .child(
                            answer_button(
                                "side-deny",
                                if question { "Skip" } else { "Deny" },
                                false,
                                !self.sending,
                                p,
                            )
                            .on_click(cx.listener(|this, _, window, cx| this.answer("deny", window, cx))),
                        )
                        .child(
                            answer_button(
                                "side-allow",
                                if question { "Send answers" } else { "Allow" },
                                true,
                                !self.sending,
                                p,
                            )
                            .on_click(cx.listener(|this, _, window, cx| this.answer("allow", window, cx))),
                        ),
                ),
            );
        } else {
            dock = dock
                .children(self.render_queued(p, cx))
                .child(self.render_message_composer(window, p, cx));
        }
        dock
    }
}
