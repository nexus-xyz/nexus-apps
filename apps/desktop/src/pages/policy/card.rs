use gpui_kit::component::{h_flex, v_flex};

use super::super::{Load, ui};
use crate::palette::Palette;

use super::*;

fn modes(kind: Kind) -> [(Mode, &'static str); 3] {
    [
        (Mode::All, if kind == Kind::Chat { "All models" } else { "All" }),
        (Mode::Only, "Only these…"),
        (Mode::Except, "All except…"),
    ]
}

fn model_default_control(ix: usize, label: String, enabled: bool, palette: Palette) -> Stateful<Div> {
    let id = SharedString::from(format!("policy-default-{ix}"));
    if enabled {
        ui::button(id, label, palette)
    } else {
        ui::disabled_button(id, label, palette)
    }
}

pub(super) fn locked(kind: Kind, palette: Palette) -> Div {
    let ix = match kind {
        Kind::Chat => 0,
        Kind::Claude => 1,
        Kind::Codex => 2,
    };
    v_flex()
        .gap(px(8.))
        .child(ui::section_head(kind.title(), palette))
        .child(ui::tabs(palette).children(modes(kind).map(|(_, label)| {
            ui::disabled_button(SharedString::from(format!("policy-mode-{ix}-{label}")), label, palette)
        })))
        .child(
            h_flex()
                .gap(px(10.))
                .child(ui::label("Default model", palette))
                .child(model_default_control(ix, "Choose model".into(), false, palette)),
        )
}

impl PolicyPage {
    pub(super) fn render_card(&self, ix: usize, palette: Palette, cx: &mut Context<Self>) -> Div {
        let card = &self.cards[ix];
        let kind = card.kind;
        let title = if self.team.is_some() {
            "Allowed models"
        } else {
            kind.title()
        };
        let line = match (kind, &self.team) {
            // The web's team card: "…within what the organization, Platform and Core allow".
            (_, Some(team)) if team.path.is_empty() => {
                "The models this team may use, within what the organization allows.".to_string()
            }
            (_, Some(team)) => format!(
                "The models this team may use, within what the organization, {} allow.",
                team.path
                    .iter()
                    .map(|(_, name)| name.as_str())
                    .collect::<Vec<_>>()
                    .join(" and ")
            ),
            (Kind::Chat, None) => format!(
                "Applies to everyone and everything in {}. Agent rules below can only narrow it.",
                if self.ctx.org_name.is_empty() {
                    "the organization"
                } else {
                    &self.ctx.org_name
                }
            ),
            (Kind::Claude, None) => {
                "For nexus claude runs. Keep a Haiku model allowed: Claude Code uses one in the background.".into()
            }
            (Kind::Codex, None) => "For nexus codex runs.".into(),
        };
        let frame = ui::card(palette).flex_1().min_w(px(240.)).gap(px(10.));
        let view = match &card.view {
            Load::Loading => {
                return frame
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(palette.strong)
                            .child(title),
                    )
                    .child(ui::skeleton(2, palette));
            }
            Load::Failed(error) => {
                return frame
                    .child(ui::error_line(format!("{title} could not be loaded: {error}"), palette))
                    .child(
                        div().child(
                            ui::button(SharedString::from(format!("policy-retry-{ix}")), "Try again", palette)
                                .on_click(cx.listener(move |this, _, _, cx| this.load_card(ix, true, cx))),
                        ),
                    );
            }
            Load::Ready(view) => view,
        };
        let can_manage = self.can_manage();
        let enabled = can_manage && !card.saving;
        let supported = self.supported(kind);
        let allowed = view.allowed.len();
        let blocked = supported.len().saturating_sub(allowed);
        let inherits = kind != Kind::Chat && !view.custom;
        let effective = view.effective_defaults.get(kind.key()).cloned().unwrap_or((None, None));
        let draft = self.draft(ix, cx);
        let dirty = draft.differs(view, kind);

        let tag = if view.custom {
            if view.migrated {
                "From the agent policy"
            } else {
                "Own rule"
            }
        } else {
            "Inherited"
        };
        let meta = match kind {
            Kind::Chat => format!(
                "Allowed {allowed} · nexus/auto runs {}",
                effective.0.as_deref().map(short_id).unwrap_or("nothing")
            ),
            _ => format!("{allowed} allowed"),
        };
        let head = h_flex()
            .flex_wrap()
            .gap(px(8.))
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(palette.strong)
                    .child(title),
            )
            .child(ui::badge(tag, palette))
            .child(div().flex_1())
            .child(div().text_size(px(12.)).text_color(palette.faint).child(meta));

        let body: AnyElement = if self.show_patterns(ix) {
            v_flex()
                .gap(px(8.))
                .child(ui::label("Allow (patterns split by commas; empty inherits)", palette))
                .child(
                    code_field(&card.allow, palette)
                        .font_family(ui::MONO)
                        .when(!enabled, |field| field.opacity(0.6)),
                )
                .child(ui::label("Block (always wins)", palette))
                .child(
                    code_field(&card.deny, palette)
                        .font_family(ui::MONO)
                        .when(!enabled, |field| field.opacity(0.6)),
                )
                .into_any_element()
        } else {
            // A segmented control: one choice of three.
            let focus_ids = match kind {
                Kind::Chat => ["policy-chat-all", "policy-chat-only", "policy-chat-except"],
                Kind::Claude => ["policy-claude-all", "policy-claude-only", "policy-claude-except"],
                Kind::Codex => ["policy-codex-all", "policy-codex-only", "policy-codex-except"],
            };
            let switcher =
                ui::tabs(palette).children(modes(kind).into_iter().enumerate().map(|(at, (mode, label))| {
                    ui::tab(
                        SharedString::from(format!("policy-mode-{ix}-{label}")),
                        label,
                        card.mode == mode,
                        palette,
                    )
                    .track_focus(&self.ctx.settings_fields.focus(focus_ids[at], cx))
                    .when(enabled, |tab| {
                        tab.on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                            let next = match event.keystroke.key.as_str() {
                                "left" | "up" => (at + 2) % 3,
                                "right" | "down" => (at + 1) % 3,
                                _ => return,
                            };
                            let card = &mut this.cards[ix];
                            card.picked.clear();
                            card.mode = [Mode::All, Mode::Only, Mode::Except][next];
                            card.adding = false;
                            this.ctx.settings_fields.focus(focus_ids[next], cx).focus(window, cx);
                            cx.stop_propagation();
                            cx.notify();
                        }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let card = &mut this.cards[ix];
                            if card.mode != mode {
                                card.picked.clear();
                                card.mode = mode;
                                card.adding = false;
                            }
                            cx.notify();
                        }))
                    })
                }));
            let below: AnyElement = if card.mode == Mode::All {
                if inherits {
                    ui::note(
                        format!(
                            "Follows Organization: {allowed} models{}.",
                            if blocked > 0 {
                                format!(", {blocked} blocked")
                            } else {
                                String::new()
                            }
                        ),
                        palette,
                    )
                    .into_any_element()
                } else {
                    div().into_any_element()
                }
            } else {
                let blocking = card.mode == Mode::Except;
                let chips = card.picked.iter().map(|id| {
                    let known = self.model_name(id).is_some();
                    let remove = id.clone();
                    h_flex()
                        .gap(px(4.))
                        .pl(px(8.))
                        .pr(px(if can_manage { 4. } else { 8. }))
                        .py(px(2.))
                        .rounded(px(6.))
                        .bg(if blocking {
                            palette.destructive.opacity(0.12)
                        } else {
                            ui::inset(palette)
                        })
                        .text_size(px(12.))
                        .text_color(if blocking {
                            palette.destructive
                        } else {
                            palette.foreground
                        })
                        .when(!known, |chip| chip.font_family(ui::MONO))
                        .child(if known { short_id(id).to_string() } else { id.clone() })
                        .when(can_manage, |chip| {
                            chip.child(
                                div()
                                    .id(SharedString::from(format!("policy-unpick-{ix}-{id}")))
                                    .px(px(4.))
                                    .rounded(px(4.))
                                    .cursor_pointer()
                                    .text_color(palette.faint)
                                    .hover(|style| style.bg(palette.hover).text_color(palette.strong))
                                    .child("×")
                                    .when(!card.saving, |button| {
                                        button.on_click(cx.listener(move |this, _, _, cx| {
                                            this.cards[ix].picked.retain(|entry| entry != &remove);
                                            cx.notify();
                                        }))
                                    }),
                            )
                        })
                });
                let unpicked: Vec<Model> = supported
                    .iter()
                    .filter(|model| !card.picked.contains(&model.id))
                    .cloned()
                    .collect();
                let add = (can_manage && !unpicked.is_empty()).then(|| {
                    ui::button(SharedString::from(format!("policy-add-{ix}")), "+ Add model", palette).when(
                        !card.saving,
                        |button| {
                            button.on_click(cx.listener(move |this, _, _, cx| {
                                this.cards[ix].adding = !this.cards[ix].adding;
                                this.cards[ix].picking_default = false;
                                cx.notify();
                            }))
                        },
                    )
                });
                let empty = card.picked.is_empty().then(|| {
                    ui::note(
                        if blocking {
                            "No model blocked yet."
                        } else {
                            "No model yet: nothing is allowed."
                        },
                        palette,
                    )
                });
                let list = (card.adding && add.is_some()).then(|| {
                    option_list(SharedString::from(format!("policy-add-list-{ix}")), palette).children(
                        unpicked.iter().map(|model| {
                            let id = model.id.clone();
                            option_row(
                                SharedString::from(format!("policy-add-{ix}-{}", model.id)),
                                model.name.clone(),
                                model.id.clone(),
                                false,
                                palette,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.cards[ix].picked.push(id.clone());
                                cx.notify();
                            }))
                        }),
                    )
                });
                v_flex()
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap(px(6.))
                            .children(chips)
                            .children(add)
                            .children(empty),
                    )
                    .children(list)
                    .into_any_element()
            };
            v_flex()
                .gap(px(10.))
                .child(h_flex().child(switcher))
                .child(below)
                .into_any_element()
        };

        let default_label = if card.default_model.is_empty() {
            match (&effective.0, view.defaults.get(kind.key())) {
                (Some(model), None) => format!("Inherited ({model})"),
                _ => "Inherited".to_string(),
            }
        } else {
            self.model_name(&card.default_model)
                .unwrap_or_else(|| card.default_model.clone())
        };
        let default_row = h_flex()
            .flex_wrap()
            .gap(px(10.))
            .child(ui::label("Default model", palette))
            .child(
                model_default_control(ix, default_label, enabled, palette).when(enabled, |button| {
                    button.on_click(cx.listener(move |this, _, _, cx| {
                        this.cards[ix].picking_default = !this.cards[ix].picking_default;
                        this.cards[ix].adding = false;
                        cx.notify();
                    }))
                }),
            );
        let default_list = (card.picking_default && enabled).then(|| {
            let inherited = option_row(
                SharedString::from(format!("policy-default-{ix}-none")),
                "Inherited",
                effective
                    .0
                    .clone()
                    .filter(|_| !view.defaults.contains_key(kind.key()))
                    .unwrap_or_default(),
                card.default_model.is_empty(),
                palette,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.cards[ix].default_model.clear();
                this.cards[ix].picking_default = false;
                cx.notify();
            }));
            option_list(SharedString::from(format!("policy-default-list-{ix}")), palette)
                .child(inherited)
                .children(supported.iter().map(|model| {
                    let id = model.id.clone();
                    option_row(
                        SharedString::from(format!("policy-default-{ix}-{}", model.id)),
                        model.name.clone(),
                        model.id.clone(),
                        card.default_model == model.id,
                        palette,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.cards[ix].default_model = id.clone();
                        this.cards[ix].picking_default = false;
                        cx.notify();
                    }))
                }))
        });

        let reviewed = card.reviewed.as_ref() == Some(&draft);
        let scope = self
            .team
            .as_ref()
            .map(|team| team.name.as_str())
            .unwrap_or(&self.ctx.org_name);
        let review = (dirty && reviewed).then(|| {
            ui::settings_notice(
                "Review changes",
                format!(
                    "Applies to {}. Allowed patterns: {}. Blocked patterns: {}. Default model: {}.",
                    if scope.is_empty() { "this organization" } else { scope },
                    if draft.only && draft.allow.is_empty() {
                        "no models".to_string()
                    } else if draft.allow.is_empty() {
                        "all supported models".to_string()
                    } else {
                        draft.allow.join(", ")
                    },
                    if draft.deny.is_empty() {
                        "none".to_string()
                    } else {
                        draft.deny.join(", ")
                    },
                    if draft.default_model.is_empty() {
                        "inherited"
                    } else {
                        &draft.default_model
                    }
                ),
                palette,
            )
        });
        let foot = ((dirty && can_manage) || card.outcome.is_some()).then(|| {
            v_flex()
                .gap(px(6.))
                .pt(px(4.))
                .when_some(card.outcome.as_ref(), |foot, outcome| {
                    foot.child(if outcome.error {
                        ui::error_line(outcome.text.clone(), palette)
                    } else {
                        ui::note(outcome.text.clone(), palette).text_color(palette.foreground)
                    })
                    .children(outcome.notes.iter().map(|note| ui::note(note.clone(), palette)))
                })
                .when(dirty && can_manage, |foot| {
                    foot.child(
                        h_flex()
                            .gap(px(8.))
                            .justify_end()
                            .child(
                                ui::button(SharedString::from(format!("policy-discard-{ix}")), "Discard", palette)
                                    .when(!card.saving, |button| {
                                        button.on_click(cx.listener(move |this, _, _, cx| {
                                            this.reset_card(ix);
                                            cx.notify();
                                        }))
                                    }),
                            )
                            .child(
                                ui::primary_button(
                                    SharedString::from(format!("policy-save-{ix}")),
                                    if card.saving {
                                        "Saving…"
                                    } else if reviewed {
                                        "Apply changes"
                                    } else {
                                        "Review changes"
                                    },
                                    palette,
                                )
                                .when(!card.saving, |button| {
                                    button.on_click(cx.listener(move |this, _, _, cx| this.save(ix, cx)))
                                }),
                            ),
                    )
                })
        });

        frame
            .child(head)
            .child(ui::note(line, palette))
            .child(body)
            .child(self.ctx.settings_fields.field(
                match kind {
                    Kind::Chat => "default-model-chat",
                    Kind::Claude => "default-model-claude",
                    Kind::Codex => "default-model-codex",
                },
                v_flex().child(default_row).children(default_list),
                None,
                cx,
            ))
            .when_some(effective.1.clone(), |frame, note| frame.child(ui::note(note, palette)))
            .children(review)
            .children(foot)
    }
}
