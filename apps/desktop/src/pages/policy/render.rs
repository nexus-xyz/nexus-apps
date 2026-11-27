use gpui_kit::component::{h_flex, v_flex};

use super::super::{Load, palette, ui};

use super::*;

fn pattern_toggle(
    patterns: bool,
    palette: crate::palette::Palette,
    cx: Option<&mut Context<PolicyPage>>,
) -> Stateful<Div> {
    let label = if patterns {
        "Edit with lists"
    } else {
        "Edit as patterns"
    };
    match cx {
        Some(cx) => ui::button("policy-patterns", label, palette).on_click(cx.listener(|page, _, _, cx| {
            page.patterns = !page.patterns;
            cx.notify();
        })),
        None => {
            ui::disabled_button("policy-patterns", label, palette).debug_selector(|| "policy-patterns-locked".into())
        }
    }
}

pub(super) fn locked(palette: crate::palette::Palette) -> AnyElement {
    v_flex()
        .gap(px(12.))
        .child(
            h_flex()
                .gap(px(8.))
                .flex_wrap()
                .child(pattern_toggle(false, palette, None))
                .child(super::controls::add_control_button(palette, false)),
        )
        .children([Kind::Chat, Kind::Claude, Kind::Codex].map(|kind| super::card::locked(kind, palette)))
        .child(crate::pages::general::locked_chat_controls(palette))
        .into_any_element()
}

impl Render for PolicyPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Put a freshly read rule in the pattern fields: setting a field needs the window.
        for ix in 0..self.cards.len() {
            if !self.cards[ix].reset {
                continue;
            }
            self.cards[ix].reset = false;
            if let Load::Ready(view) = &self.cards[ix].view {
                let (allow, deny) = (view.allow.join(", "), view.deny.join(", "));
                self.cards[ix]
                    .allow
                    .update(cx, |state, cx| state.set_value(allow, window, cx));
                self.cards[ix]
                    .deny
                    .update(cx, |state, cx| state.set_value(deny, window, cx));
            }
        }
        let palette = palette(cx);
        let toggle = pattern_toggle(self.patterns, palette, Some(cx));
        // A team's Models section: its controls, then its allowed models. No page lead and no test.
        if self.team.is_some() {
            let column = v_flex()
                .w_full()
                .gap(px(10.))
                .children(self.render_controls(palette, cx));
            return match &self.catalog {
                Load::Loading => column.child(ui::skeleton(3, palette)),
                Load::Failed(error) => column.child(ui::failed(
                    format!("The model catalog could not be loaded: {error}"),
                    palette,
                )),
                Load::Ready(_) => column
                    .when(!self.can_manage(), |column| {
                        column.child(ui::note(
                            "Only the organization's owners and admins, and the leads of this team or a team above \
                             it, can change this.",
                            palette,
                        ))
                    })
                    .when(self.can_manage(), |column| {
                        column.child(h_flex().justify_end().child(toggle))
                    })
                    .child(self.render_card(0, palette, cx))
                    .child(self.render_test(palette, cx)),
            };
        }
        let column = v_flex().w_full().child(
            h_flex()
                .justify_between()
                .gap(px(12.))
                .child(ui::lead(
                    "Which models people and agents can call through the Gateway.",
                    palette,
                ))
                .child(div().mb(px(14.)).child(toggle)),
        );
        let banner = h_flex()
            .gap(px(6.))
            .p(px(10.))
            .mb(px(4.))
            .rounded(px(8.))
            .bg(palette.hover)
            .text_size(px(13.))
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(palette.strong)
                    .child("A block always wins."),
            )
            .child(
                div()
                    .text_color(palette.muted)
                    .child("If any rule blocks a model, no other rule can allow it back."),
            );
        // Who can't change the organization's rules sees one notice that names who can (board P11).
        let notice = if !self.can_manage() && self.team.is_none() {
            let org = if self.ctx.org_name.is_empty() {
                "our organization"
            } else {
                &self.ctx.org_name
            };
            crate::setup::role_notice(
                "policy:org",
                "Owners and admins change model rules.",
                "policy.manage",
                format!("Could you change {org}'s model rules in Nexus?"),
                &self.ctx.fixer.access(cx),
            )
            .render(&self.ctx.fixer, palette, cx)
            .mb(px(12.))
            .into_any_element()
        } else {
            ui::settings_notice(
                if self.can_manage() { "Organization rules" } else { "Read-only model rules" },
                if self.can_manage() { "Review the affected scope before applying model changes. Agent rules can narrow organization access." }
                else { "Ask an owner or admin to change model rules. Your current permissions still govern temporary controls." },
                palette,
            ).mb(px(12.)).into_any_element()
        };
        let column = column
            .child(notice)
            .child(banner.flex_wrap())
            .children(self.render_controls(palette, cx));
        match &self.catalog {
            Load::Loading => {
                return column
                    .child(ui::skeleton(4, palette))
                    .children(self.shared_settings.clone());
            }
            Load::Failed(error) => {
                return column
                    .child(ui::failed(
                        format!("The model catalog could not be loaded: {error}"),
                        palette,
                    ))
                    .child(
                        h_flex().justify_center().child(
                            ui::button("policy-catalog-retry", "Try again", palette)
                                .on_click(cx.listener(|this, _, _, cx| this.load_catalog(cx))),
                        ),
                    )
                    .children(self.shared_settings.clone());
            }
            Load::Ready(_) => {}
        }
        let read_only = (!self.can_manage()).then(|| {
            ui::note(
                "Only organization admins can change this. You can read it and test it.",
                palette,
            )
            .mt(px(10.))
        });
        column
            .children(read_only)
            .child(div().mt(px(16.)).child(self.render_card(0, palette, cx)))
            .child(
                h_flex()
                    .items_start()
                    .flex_wrap()
                    .gap(px(12.))
                    .mt(px(12.))
                    .child(self.render_card(1, palette, cx))
                    .child(self.render_card(2, palette, cx)),
            )
            .children(self.shared_settings.clone())
            .child(self.render_test(palette, cx))
    }
}
