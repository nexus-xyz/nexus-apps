use gpui_kit::component::{h_flex, v_flex};

use super::super::{Load, ui};
use crate::palette::Palette;

use super::*;

impl PolicyPage {
    pub(super) fn render_test(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let picker = |name: &'static str, label: String, cx: &mut Context<Self>| {
            ui::chip(
                SharedString::from(format!("policy-test-{name}")),
                label,
                self.test_open == Some(name),
                palette,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.test_open = if this.test_open == Some(name) { None } else { Some(name) };
                cx.notify();
            }))
        };
        let who = self
            .test_as
            .as_ref()
            .map(|choice| choice.label.clone())
            .unwrap_or_else(|| "You".into());
        let sentence = match &self.team {
            Some(team) => h_flex()
                .flex_wrap()
                .gap(px(8.))
                .text_size(px(13.))
                .text_color(palette.foreground)
                .child(format!("Test in team {}: can it use", team.slug))
                .child(picker("model", self.test_model.clone(), cx).font_family(ui::MONO))
                .child("?"),
            None => h_flex()
                .flex_wrap()
                .gap(px(8.))
                .text_size(px(13.))
                .text_color(palette.foreground)
                .child("Can")
                .child(picker("person", who, cx))
                .child("use")
                .child(picker("model", self.test_model.clone(), cx).font_family(ui::MONO))
                .child("in")
                .child(picker("agent", self.test_agent.place().into(), cx))
                .child("?"),
        };
        let list: Option<Stateful<Div>> = match self.test_open {
            Some("person") => {
                let people = match (&self.members, self.can_manage()) {
                    (Load::Ready(list), true) => list.clone(),
                    _ => Vec::new(),
                };
                Some(
                    option_list("policy-test-people", palette)
                        .child(
                            option_row("policy-test-person-you", "You", "", self.test_as.is_none(), palette).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.test_as = None;
                                    this.test_open = None;
                                    this.run_test(cx);
                                    cx.notify();
                                }),
                            ),
                        )
                        .children(people.into_iter().map(|choice| {
                            let picked = self
                                .test_as
                                .as_ref()
                                .is_some_and(|current| current.value == choice.value);
                            option_row(
                                SharedString::from(format!("policy-test-person-{}", choice.value)),
                                choice.label.clone(),
                                String::new(),
                                picked,
                                palette,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.test_as = Some(choice.clone());
                                this.test_open = None;
                                this.run_test(cx);
                                cx.notify();
                            }))
                        })),
                )
            }
            Some("model") => {
                let mut ids = vec![("nexus/auto".to_string(), String::new())];
                if let Load::Ready(models) = &self.catalog {
                    ids.extend(models.iter().map(|model| (model.id.clone(), model.name.clone())));
                }
                Some(
                    option_list("policy-test-models", palette).children(ids.into_iter().map(|(id, name)| {
                        let picked = self.test_model == id;
                        let value = id.clone();
                        option_row(
                            SharedString::from(format!("policy-test-model-{id}")),
                            id,
                            name,
                            picked,
                            palette,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.test_model = value.clone();
                            this.test_open = None;
                            this.run_test(cx);
                            cx.notify();
                        }))
                    })),
                )
            }
            Some("agent") => Some(
                option_list("policy-test-agents", palette).children(Kind::ALL.into_iter().map(|kind| {
                    option_row(
                        SharedString::from(format!("policy-test-agent-{}", kind.key())),
                        kind.place(),
                        String::new(),
                        self.test_agent == kind,
                        palette,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.test_agent = kind;
                        this.test_open = None;
                        this.run_test(cx);
                        cx.notify();
                    }))
                })),
            ),
            _ => None,
        };
        let result: AnyElement = match &self.test {
            Load::Loading => ui::note("Asking Gateway…", palette).into_any_element(),
            Load::Failed(error) => h_flex()
                .gap(px(8.))
                .child(ui::error_line(error.clone(), palette))
                .child(
                    ui::button("policy-test-retry", "Try again", palette).on_click(cx.listener(|this, _, _, cx| {
                        this.run_test(cx);
                        cx.notify();
                    })),
                )
                .into_any_element(),
            Load::Ready(decision) => {
                let target = if decision.allowed {
                    decision.model.clone().unwrap_or_else(|| decision.requested.clone())
                } else {
                    decision.rule.clone()
                };
                v_flex()
                    .gap(px(4.))
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap(px(6.))
                            .text_size(px(13.))
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(if decision.allowed {
                                        palette.strong
                                    } else {
                                        palette.destructive
                                    })
                                    .child(if decision.allowed { "Allowed" } else { "Blocked" }),
                            )
                            .child(
                                div()
                                    .text_color(palette.muted)
                                    .child(if decision.allowed { "as" } else { "by" }),
                            )
                            .child(div().font_family(ui::MONO).text_color(palette.foreground).child(target))
                            .when_some(decision.resolved_from.clone(), |line, from| {
                                line.child(div().text_color(palette.faint).child(format!("from {from}")))
                            }),
                    )
                    .when(!decision.reason.is_empty(), |column| {
                        column.child(ui::note(decision.reason.clone(), palette))
                    })
                    .children(
                        decision
                            .warnings
                            .iter()
                            .map(|warning| ui::note(warning.clone(), palette)),
                    )
                    .into_any_element()
            }
        };
        v_flex()
            .child(ui::section_head("Test", palette))
            .child(ui::note("Ask the policy before someone hits it.", palette).mb(px(10.)))
            .child(sentence)
            .children(list)
            .child(div().mt(px(10.)).child(result))
    }
}
