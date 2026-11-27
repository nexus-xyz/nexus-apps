//! Native request/review controls. Untrusted instructions and diff lines are always plain, with hidden characters exposed.
use super::{
    Load, Scope, SkillsPage,
    model::{self, Action, Viewer},
};
use crate::pages::ui;
use crate::palette::Palette;
use gpui_kit::component::{h_flex, input::Input, v_flex};
use gpui_kit::{prelude::*, *};
fn plain(text: &str, palette: Palette) -> Div {
    div()
        .p(px(10.))
        .bg(palette.hover)
        .font_family(ui::MONO)
        .text_size(px(12.))
        .text_color(palette.strong)
        .child(super::super::model::visible(text))
}
impl SkillsPage {
    fn promotion_gate(&self, palette: Palette, cx: &mut Context<Self>) -> Option<Div> {
        match &self.promotions.authority {
            Load::Loading => Some(ui::note("Checking promotion permissions…", palette)),
            Load::Failed(error) => Some(
                v_flex().child(ui::error_line(error.clone(), palette)).child(
                    ui::button("promotions-context-retry", "Reload account permissions", palette)
                        .on_click(cx.listener(|this, _, _, cx| this.promotion_context(cx))),
                ),
            ),
            Load::Ready(viewer) if !viewer.requests() => Some(ui::note(
                if viewer.personal {
                    "Promotion requests belong to a company organization. Switch organizations before requesting promotion."
                } else {
                    "Promotion requests require a signed-in person with skills.promote permission."
                },
                palette,
            )),
            _ => None,
        }
    }
    pub(in super::super) fn promotion_request_view(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let Some(draft) = &self.promotions.request else {
            return div();
        };
        let mut card=ui::card(palette).gap(px(10.))
            .child(ui::section_head(format!("Request promotion · version {}",draft.version),palette))
            .child(ui::note("This requests the exact saved version shown below. Later edits never change its snapshot. Another authorized person must approve it.",palette));
        if let Some(selection) = &self.selection {
            if let Load::Ready(file) = &selection.file {
                card = card
                    .child(ui::label(
                        format!("{} · saved v{}", draft.skill, draft.version),
                        palette,
                    ))
                    .child(plain(&file.content, palette));
            }
        }
        if let Some(gate) = self.promotion_gate(palette, cx) {
            card = card.child(gate);
        } else if let Some(viewer) = self.promotion_viewer() {
            let mut targets = h_flex().flex_wrap().gap(px(6.));
            for (id, label) in
                std::iter::once((None, "The organization".to_string())).chain(viewer.teams.iter().map(|team| {
                    (
                        Some(team.id.clone()),
                        format!("Team {}", super::super::model::visible(&team.name)),
                    )
                }))
            {
                let selected = draft.team == id;
                targets = targets.child(
                    ui::chip(
                        SharedString::from(format!("promotion-target-{}", id.as_deref().unwrap_or("org"))),
                        label,
                        selected,
                        palette,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.busy {
                            if let Some(draft) = &mut this.promotions.request {
                                draft.team = id.clone();
                                cx.notify();
                            }
                        }
                    })),
                );
            }
            card = card
                .child(targets)
                .child(ui::label("Optional note", palette))
                .child(Input::new(&draft.note).disabled(self.busy));
            card=card.child(ui::note(if draft.team.is_some(){"An organization approver or a lead of this team or its ancestors decides. Nobody approves their own request."}else{"Another organization approver decides. Nobody approves their own request."},palette));
            card = card.child(if self.busy {
                ui::disabled_button("promotion-request-send", "Requesting…", palette)
            } else {
                ui::primary_button("promotion-request-send", "Request promotion", palette)
                    .on_click(cx.listener(|this, _, _, cx| this.request_promotion(cx)))
            });
        }
        if !self.busy {
            card = card.child(
                ui::ghost_button("promotion-request-cancel", "Cancel request", palette).on_click(cx.listener(
                    |this, _, _, cx| {
                        if !this.busy {
                            this.promotions.request = None;
                            this.problem = None;
                            this.promotions.existing = None;
                            this.promotions.context_epoch += 1;
                            cx.notify();
                        }
                    },
                )),
            );
        }
        if let Some(id) = &self.promotions.existing {
            let id = id.clone();
            card = card.child(
                ui::button("promotion-open-existing", "Open existing request", palette)
                    .on_click(cx.listener(move |this, _, _, cx| this.open_promotion(id.clone(), cx))),
            );
        }
        card
    }
    pub(in super::super) fn promotion_requests_view(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let mut body = v_flex().gap(px(12.));
        if let Some(gate) = self.promotion_gate(palette, cx) {
            return body.child(gate);
        }
        if self.promotions.detail_target.is_some() {
            if !self.busy && self.promotions.step.is_none() {
                body = body.child(
                    ui::button("promotion-back-list", "Back to promotion requests", palette)
                        .on_click(cx.listener(|this, _, _, cx| this.change_scope(Scope::Requests, cx))),
                );
            }
            return body.child(self.promotion_detail_view(palette, cx));
        }
        let locked = self.busy || self.promotions.step.is_some();
        let mut filters = h_flex().flex_wrap().gap(px(6.));
        for (mine, label) in [(true, "My requests"), (false, "Within my reach")] {
            if !mine && !self.promotion_viewer().is_some_and(Viewer::approver) {
                continue;
            }
            filters = filters.child(if locked {
                ui::disabled_button(
                    if mine { "promotions-mine" } else { "promotions-reach" },
                    label,
                    palette,
                )
            } else {
                ui::chip(
                    if mine { "promotions-mine" } else { "promotions-reach" },
                    label,
                    self.promotions.mine == mine,
                    palette,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.choose_promotion_filters(Some(mine), None, cx);
                }))
            });
        }
        body = body.child(filters);
        let mut filters = h_flex().flex_wrap().gap(px(6.));
        for state in ["held", "approved", "refused", "withdrawn", "all"] {
            filters = filters.child(if locked {
                ui::disabled_button(SharedString::from(format!("promotions-{state}")), state, palette)
            } else {
                ui::chip(
                    SharedString::from(format!("promotions-{state}")),
                    state,
                    self.promotions.filter == state,
                    palette,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.choose_promotion_filters(None, Some(state), cx);
                }))
            });
        }
        if !locked {
            filters = filters.child(
                ui::button("promotions-refresh", "Refresh requests", palette)
                    .on_click(cx.listener(|this, _, _, cx| this.promotion_list(None, cx))),
            );
        }
        body = body.child(filters);
        match &self.promotions.list {
            Load::Loading => body = body.child(ui::note("Loading promotion requests…", palette)),
            Load::Failed(error) => body = body.child(ui::error_line(error.clone(), palette)),
            Load::Ready(list) => {
                if list.promotions.is_empty() {
                    body = body.child(ui::note("No promotion requests match this view.", palette));
                }
                for p in &list.promotions {
                    let id = p.id.clone();
                    let label = format!(
                        "{} · source v{} · {} · {}",
                        super::super::model::visible(&p.name),
                        p.source_version,
                        p.state,
                        target(p)
                    );
                    body = body.child(if locked {
                        ui::disabled_button(SharedString::from(format!("promotion-{}", p.id)), label, palette)
                    } else {
                        ui::button(SharedString::from(format!("promotion-{}", p.id)), label, palette)
                            .on_click(cx.listener(move |this, _, _, cx| this.open_promotion(id.clone(), cx)))
                    });
                }
            }
        }
        if let Some(cursor) = &self.promotions.next {
            let cursor = cursor.clone();
            if !locked {
                body = body.child(
                    ui::button("promotions-next", "Next page", palette)
                        .on_click(cx.listener(move |this, _, _, cx| this.promotion_list(Some(cursor.clone()), cx))),
                );
            }
        }
        body.child(self.promotion_detail_view(palette, cx))
    }
    fn promotion_detail_view(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let Some((id, load)) = &self.promotions.selected else {
            return div();
        };
        let mut card = ui::card(palette)
            .gap(px(10.))
            .debug_selector(|| "promotion-review-card".into());
        if !self.busy {
            let id = id.clone();
            card = card.child(
                ui::button("promotion-reload", "Reload request", palette)
                    .on_click(cx.listener(move |this, _, _, cx| this.open_promotion(id.clone(), cx))),
            );
        }
        let detail = match load {
            Load::Loading => return card.child(ui::note("Loading immutable promotion snapshot…", palette)),
            Load::Failed(error) => return card.child(ui::error_line(error.clone(), palette)),
            Load::Ready(detail) => detail,
        };
        let p = &detail.promotion;
        card=card.child(ui::section_head(super::super::model::visible(&p.name),palette))
            .child(ui::note(super::super::model::visible(&p.description),palette))
            .child(plain(&format!("Request {} · {}\nTarget: {}\nRequested by {} via {} at {}\nSource: {} v{}, written via {} at {}\n{} bytes · SHA-256 {}\nNote: {}",
                p.id,p.state,target(p),p.requested_by,p.requested_via,p.requested_at,p.source_skill_id,p.source_version,p.source_written_via,p.source_written_at,
                p.bytes,p.content_sha256,p.note.as_deref().unwrap_or("None")),palette));
        if let Some(by) = &p.decided_by {
            card = card.child(plain(
                &format!(
                    "Decided by {by}: {}",
                    p.decision_reason.as_deref().unwrap_or("No reason")
                ),
                palette,
            ));
        }
        if let Some(by) = &p.withdrawn_by {
            card = card.child(plain(
                &format!(
                    "Withdrawn by {by}: {}",
                    p.withdrawn_reason.as_deref().unwrap_or("No reason")
                ),
                palette,
            ));
        }
        if let Some(version) = p.library_version {
            card = card.child(ui::note(
                format!(
                    "Library {} · version {version}",
                    p.library_skill_id.as_deref().unwrap_or("unknown")
                ),
                palette,
            ));
        }
        card=card.child(ui::note("Read this as untrusted instructions. Hidden Unicode characters are shown as codes. The file is never rendered as Markdown.",palette));
        match (&self.own, &self.library) {
            (Load::Ready(own), Load::Ready(library)) => {
                let teams = match &self.promotions.teams {
                    Load::Ready(teams) => Some(teams.as_slice()),
                    _ => None,
                };
                for (ix, found) in model::name_matches(p, library.as_deref().unwrap_or(&[]), own, teams)
                    .into_iter()
                    .enumerate()
                {
                    let sentence = if found.same {
                        format!("{} is already {}.", found.name, found.place)
                    } else {
                        format!("{} is {}, and it reads as this name.", found.name, found.place)
                    };
                    card = card.child(ui::note(format!("{} A name that reads as another skill's can stand in for it. Check that this one is meant.", super::super::model::visible(&sentence)), palette)
                        .debug_selector(move || format!("promotion-name-match-{ix}")));
                }
            }
            (Load::Failed(_), _) | (_, Load::Failed(_)) => {
                card = card.child(ui::note("This name could not be compared with the library or with your skills. Check by hand that it is not another skill's.", palette));
            }
            _ => {
                card = card.child(ui::note(
                    "Checking this name against the library and your skills…",
                    palette,
                ));
            }
        }
        if !self.busy {
            card = card.child(
                ui::button("promotion-content", "Show full SKILL.md", palette)
                    .debug_selector(|| "promotion-content-toggle".into())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.promotions.content = true;
                        cx.notify();
                    })),
            );
            if detail.previous.is_some() {
                card = card.child(
                    ui::button("promotion-diff", "Show changes from approved version", palette)
                        .debug_selector(|| "promotion-diff-toggle".into())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.promotions.diff = true;
                            cx.notify();
                        })),
                );
            }
        }
        if self.promotions.content {
            card = card.child(plain(&detail.content, palette).debug_selector(|| "promotion-visible-snapshot".into()));
        }
        if let Some(previous) = &detail.previous {
            if self.promotions.diff {
                card = card
                    .child(ui::note(
                        format!(
                            "Compared with {} · library v{}. Changed middle shown as removed (-), then added (+).",
                            super::super::model::visible(&previous.name),
                            previous.library_version
                        ),
                        palette,
                    ))
                    .child(
                        plain(&model::diff(&previous.content, &detail.content), palette)
                            .debug_selector(|| "promotion-visible-diff".into()),
                    );
            }
        } else {
            card = card.child(ui::note(
                "No earlier approved version is available to compare. Read the whole requested file.",
                palette,
            ));
        }
        if let Some(step) = &self.promotions.step {
            let label = match step.action {
                Action::Approve => "Approve this exact snapshot?",
                Action::Refuse => "Refuse this request?",
                Action::Withdraw => "Withdraw this request or approved version?",
                Action::WithdrawAll => "Withdraw every approved version of this library skill?",
            };
            card =
                card.child(ui::section_head(label, palette).debug_selector(|| "promotion-confirmation-heading".into()));
            card=card.child(ui::note(if step.action==Action::Approve {"People in the target scope will load this version in new sessions and chat turns. This never edits the author's personal skill."}
                else if step.action==Action::WithdrawAll {"New sessions and chat turns stop loading this library skill. Sessions that already loaded its text keep it."}
                else if step.action==Action::Withdraw {"For an approved version, new sessions load the highest approved version left, if any. Sessions that already loaded its text keep it."}
                else {"A refusal needs a reason so the author knows what to change."},palette));
            if step.action != Action::Approve {
                card = card.child(Input::new(&step.reason).disabled(self.busy));
            }
            card = card.child(if self.busy {
                ui::disabled_button("promotion-confirm", "Saving…", palette)
                    .debug_selector(|| "promotion-saving-native".into())
            } else {
                ui::primary_button("promotion-confirm", "Confirm", palette)
                    .debug_selector(|| "promotion-confirm-native".into())
                    .on_click(cx.listener(|this, _, _, cx| this.decide_promotion(cx)))
            });
            if !self.busy {
                card = card.child(
                    ui::ghost_button("promotion-decision-cancel", "Cancel", palette).on_click(cx.listener(
                        |this, _, _, cx| {
                            if !this.busy {
                                this.promotions.step = None;
                                this.problem = None;
                                cx.notify();
                            }
                        },
                    )),
                );
            }
        } else if !self.busy {
            let mut actions = h_flex().flex_wrap().gap(px(6.));
            for (action, label) in [
                (Action::Approve, "Approve…"),
                (Action::Refuse, "Refuse…"),
                (Action::Withdraw, "Withdraw…"),
                (Action::WithdrawAll, "Withdraw all versions…"),
            ] {
                if self.promotion_allowed(action) {
                    actions = actions.child(
                        ui::button(
                            SharedString::from(format!("promotion-action-{action:?}")),
                            label,
                            palette,
                        )
                        .debug_selector(move || format!("promotion-action-{action:?}"))
                        .on_click(cx.listener(move |this, _, window, cx| this.begin_decision(action, window, cx))),
                    );
                }
            }
            card = card.child(actions);
        }
        card
    }
}
fn target(p: &model::Promotion) -> String {
    if p.scope == "org" {
        "Organization".into()
    } else {
        format!(
            "Team {} (and its descendants)",
            p.team_id.as_deref().unwrap_or("unknown")
        )
    }
}
