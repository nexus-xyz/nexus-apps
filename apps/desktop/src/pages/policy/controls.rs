use crate::workspace::MenuTrigger;
use crate::workspace::menu_item_hover;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{h_flex, v_flex};

use super::super::{Load, ui};
use crate::palette::Palette;

use super::*;

pub(super) fn add_control_button(palette: Palette, live: bool) -> Stateful<Div> {
    if live {
        ui::button("policy-add-control", "Add control", palette)
    } else {
        ui::disabled_button("policy-add-control", "Add control", palette)
    }
    .gap(px(6.))
    .child(ui::icon(Lucide::ChevronDown, palette.icon).size(px(14.)))
}

impl PolicyPage {
    pub(super) fn render_controls(&self, palette: Palette, cx: &mut Context<Self>) -> Option<Div> {
        let can = self.can_control();
        let list = match &self.controls {
            Load::Ready(list) => list.as_slice(),
            _ => &[],
        };
        if !can && list.is_empty() && !matches!(self.controls, Load::Failed(_)) {
            return None;
        }
        let menu_open = self.control_menu && can;
        let actions = h_flex().relative().when(can, |actions| {
            actions
                .child(
                    add_control_button(palette, true)
                        .when(menu_open, |button| button.bg(palette.selected))
                        .menu_trigger(
                            menu_open,
                            cx,
                            |this| this.control_menu,
                            |this, _, cx| {
                                this.control_menu = !this.control_menu;
                                cx.notify();
                            },
                        ),
                )
                .when(menu_open, |actions| {
                    actions.child(deferred(
                        div().absolute().top(px(ui::CONTROL_HEIGHT)).right(px(0.)).child(
                            div().pt(px(4.)).child(
                                v_flex()
                                    .id("policy-control-menu")
                                    .occlude()
                                    .w(px(240.))
                                    .p(px(4.))
                                    .gap(px(1.))
                                    .rounded(px(12.))
                                    .bg(palette.menu)
                                    .shadow(palette.pop_shadow(palette.border))
                                    .text_size(px(13.))
                                    .text_color(palette.foreground)
                                    .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                                        this.control_menu = false;
                                        cx.notify();
                                    }))
                                    .children(self.control_kinds().into_iter().map(|kind| {
                                        h_flex()
                                            .id(SharedString::from(format!("policy-control-{}", kind.key())))
                                            .h(px(30.))
                                            .px(px(8.))
                                            .gap(px(8.))
                                            .rounded(px(8.))
                                            .cursor_pointer()
                                            .hover(|style| menu_item_hover(style, palette))
                                            .child(ui::icon(kind.icon(), palette.icon).size(px(16.)))
                                            .child(div().flex_1().child(kind.action()))
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.control_menu = false;
                                                this.open_control_form(kind, window, cx);
                                            }))
                                    })),
                            ),
                        ),
                    ))
                })
        });
        let head = h_flex()
            .justify_between()
            .child(ui::section_head("Operator controls", palette))
            .child(actions);
        let now = now_secs();
        let rows: AnyElement = match &self.controls {
            Load::Loading => ui::skeleton(1, palette).into_any_element(),
            Load::Failed(error) => {
                ui::error_line(format!("Active controls could not be loaded: {error}"), palette).into_any_element()
            }
            Load::Ready(list) if self.visible_controls(list).is_empty() => ui::note(
                if self.team.is_some() {
                    "Nothing is paused, limited or overridden on this team or under it."
                } else {
                    "Nothing is paused, limited or overridden."
                },
                palette,
            )
            .into_any_element(),
            Load::Ready(list) => v_flex()
                .children(self.visible_controls(list).into_iter().map(|control| {
                    let tag = match control.kind.as_str() {
                        "pause" => "Paused",
                        "limit" => "Limited",
                        "override" => "Override",
                        other => other,
                    };
                    let who = if control.level == "organization" {
                        "The whole organization".to_string()
                    } else {
                        let level = LEVELS
                            .iter()
                            .find(|(key, _)| *key == control.level)
                            .map(|(_, label)| *label)
                            .unwrap_or("");
                        format!("{level} {}", self.name_of(&control.subject))
                    };
                    let models = (!control.models.is_empty()).then(|| {
                        format!(
                            "{} {}",
                            if control.kind == "limit" { "Only" } else { "Allowed" },
                            control
                                .models
                                .iter()
                                .map(|id| self.model_name(id).unwrap_or_else(|| id.clone()))
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    });
                    let when = format!(
                        "{}{}",
                        control
                            .created_by
                            .as_deref()
                            .map(|by| format!("Set by {}, ", self.name_of(by)))
                            .unwrap_or_default(),
                        time_left(&control.expires_at, now)
                    );
                    let confirming = self.confirm_end.as_deref() == Some(control.id.as_str());
                    let id = control.id.clone();
                    ui::row(palette)
                        .child(
                            ui::badge(tag, palette)
                                .when(control.kind == "pause", |badge| badge.text_color(palette.destructive)),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w(px(0.))
                                .gap(px(2.))
                                .child(div().text_size(px(13.)).text_color(palette.strong).child(who))
                                .children(models.map(|text| ui::note(text, palette).text_size(px(12.))))
                                .when(!control.reason.is_empty(), |column| {
                                    column.child(
                                        div()
                                            .text_size(px(13.))
                                            .text_color(palette.foreground)
                                            .child(control.reason.clone()),
                                    )
                                })
                                .child(div().text_size(px(12.)).text_color(palette.faint).child(when)),
                        )
                        .when(can, |row| {
                            row.child(
                                ui::danger_button(
                                    SharedString::from(format!("policy-end-{}", control.id)),
                                    if confirming { "End it" } else { "End now" },
                                    palette,
                                )
                                .when(!self.ending, |button| {
                                    button.on_click(cx.listener(move |this, _, _, cx| this.end_control(id.clone(), cx)))
                                }),
                            )
                        })
                }))
                .into_any_element(),
        };
        Some(
            v_flex()
                .child(head)
                .children(
                    self.control_form
                        .as_ref()
                        .map(|form| self.render_control_form(form, palette, cx)),
                )
                .when_some(self.control_problem.clone(), |column, problem| {
                    column.child(ui::error_line(problem, palette))
                })
                .child(rows),
        )
    }

    pub(super) fn render_control_form(&self, form: &ControlForm, palette: Palette, cx: &mut Context<Self>) -> Div {
        let levels: &[(&'static str, &'static str)] = if self.team.is_some() { &TEAM_LEVELS } else { &LEVELS };
        let levels = h_flex()
            .gap(px(6.))
            .children(levels.iter().copied().map(|(key, label)| {
                ui::chip(
                    SharedString::from(format!("policy-level-{key}")),
                    label,
                    form.level == key,
                    palette,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    // In a team's view, the team level is the team itself.
                    let own_team = this.team.as_ref().map(|team| team.id.clone()).filter(|_| key == "team");
                    if let Some(form) = this.control_form.as_mut() {
                        form.level = key;
                        form.subject = own_team.unwrap_or_default();
                        form.confirm = false;
                    }
                    cx.notify();
                }))
            }));
        let choices = match form.level {
            "team" => Some(&self.teams),
            "person" => Some(&self.members),
            _ => None,
        };
        let subject: Option<AnyElement> = match (form.level, choices) {
            ("key", _) => Some(code_field(&self.key_subject, palette).into_any_element()),
            (_, Some(Load::Ready(list))) if !list.is_empty() => Some(
                option_list("policy-subjects", palette)
                    .children(list.iter().map(|choice| {
                        let value = choice.value.clone();
                        option_row(
                            SharedString::from(format!("policy-subject-{}", choice.value)),
                            choice.label.clone(),
                            String::new(),
                            form.subject == choice.value,
                            palette,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(form) = this.control_form.as_mut() {
                                form.subject = value.clone();
                                form.confirm = false;
                            }
                            cx.notify();
                        }))
                    }))
                    .into_any_element(),
            ),
            (_, Some(Load::Loading)) => Some(ui::note("Reading the list…", palette).into_any_element()),
            (_, Some(Load::Failed(error))) => {
                Some(ui::error_line(format!("The list could not be read: {error}"), palette).into_any_element())
            }
            (_, Some(_)) => Some(ui::note("Nobody to pick here.", palette).into_any_element()),
            _ => None,
        };
        let models = (form.kind != ControlKind::Pause).then(|| {
            let catalog = match &self.catalog {
                Load::Ready(models) => models.as_slice(),
                _ => &[],
            };
            v_flex()
                .gap(px(4.))
                .child(ui::label(
                    if form.kind == ControlKind::Limit {
                        "Only these models"
                    } else {
                        "Allow these models"
                    },
                    palette,
                ))
                .child(
                    option_list("policy-control-models", palette).children(catalog.iter().map(|model| {
                        let id = model.id.clone();
                        option_row(
                            SharedString::from(format!("policy-control-model-{}", model.id)),
                            model.name.clone(),
                            model.id.clone(),
                            form.models.contains(&model.id),
                            palette,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(form) = this.control_form.as_mut() {
                                if let Some(at) = form.models.iter().position(|entry| entry == &id) {
                                    form.models.remove(at);
                                } else {
                                    form.models.push(id.clone());
                                }
                            }
                            cx.notify();
                        }))
                    })),
                )
        });
        let presets = EXPIRY.into_iter().chain([(AT_A_TIME, "At a time…")]);
        let ends = h_flex().gap(px(6.)).children(presets.map(|(secs, label)| {
            ui::chip(
                SharedString::from(format!("policy-ends-{secs}")),
                label,
                form.expiry == secs,
                palette,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(form) = this.control_form.as_mut() {
                    form.expiry = secs;
                }
                cx.notify();
            }))
        }));
        let send_label = if form.busy {
            "Saving…".to_string()
        } else if form.confirm {
            "Pause now".to_string()
        } else {
            form.kind.action().to_string()
        };
        let send = if form.kind == ControlKind::Pause {
            ui::danger_button("policy-control-send", send_label, palette)
        } else {
            ui::primary_button("policy-control-send", send_label, palette)
        }
        .when(!form.busy, |button| {
            button.on_click(cx.listener(|this, _, _, cx| this.send_control(cx)))
        });
        ui::card(palette)
            .mb(px(10.))
            .gap(px(10.))
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(palette.strong)
                    .child(match &self.team {
                        // "Pause… in Core": the web's dialog title.
                        Some(team) => format!("{} in {}", form.kind.action(), team.name),
                        None => form.kind.action().to_string(),
                    }),
            )
            .child(ui::note(form.kind.note(), palette))
            .child(ui::label("Applies to", palette))
            .child(levels)
            .children(subject)
            .children(models)
            .child(ui::label("Reason", palette))
            .child(field(&self.reason, palette))
            .child(ui::label("Ends", palette))
            .child(ends)
            .when(form.expiry == AT_A_TIME, |card| {
                card.child(
                    h_flex()
                        .gap(px(8.))
                        .child(div().w(px(200.)).flex_none().child(code_field(&self.end_at, palette)))
                        .child(ui::note(
                            format!("Year, month, day and a 24-hour time, in {}.", local_zone()),
                            palette,
                        )),
                )
            })
            .when_some(form.problem.clone(), |card, problem| {
                card.child(ui::error_line(problem, palette))
            })
            .when(form.confirm, |card| {
                card.child(ui::note("Requests it covers stop at once.", palette))
            })
            .child(
                h_flex()
                    .gap(px(8.))
                    .justify_end()
                    .child(
                        ui::button("policy-control-cancel", "Cancel", palette).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.control_form = None;
                                cx.notify();
                            },
                        )),
                    )
                    .child(send),
            )
    }
}
