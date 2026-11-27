//! The top bar's one meter, as the Nexus app draws it (`workbench/context-meter.tsx`): what the
//! session has cost and how full its context is, as one pill (`$3.42 ▬ 38%`). A click opens the session's facts: the
//! cost, the context window (used and left), who pays, the agent, model, effort and permissions. It reads the session's
//! summary (`cost_usd`, `context_tokens` and `context_window`, as the agent reports them), as the Nexus app does.
//!
//! Context shares use the shared rules; the loaded log supplies estimates, never exact billing figures.

use crate::workspace::MenuTrigger;
use crate::workspace::overlay::{Overlay, OverlaySlot};
use gpui_kit::component::h_flex;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::event::{PermissionMode, SessionAgent, Via};

use super::TopPanel;
use super::git_panel::popover;
use crate::code::CodeSession;
use crate::palette::Palette;
use crate::workspace::View;
use crate::workspace::Workspace;
use crate::workspace::code_dock::menu_box;

/// Dollars at the precision the amount needs, as Nexus UI's `usd`: cents when whole, else four or six places.
pub(in crate::workspace) fn usd(value: f64) -> String {
    let abs = value.abs();
    if abs == 0.0 {
        return "$0.00".into();
    }
    if abs >= 100.0 || (value * 100.0 - (value * 100.0).round()).abs() < 1e-9 {
        let cents = (value * 100.0).round() as i64;
        let whole = (cents / 100).abs().to_string();
        let mut grouped = String::new();
        for (index, digit) in whole.chars().enumerate() {
            if index > 0 && (whole.len() - index) % 3 == 0 {
                grouped.push(',');
            }
            grouped.push(digit);
        }
        let sign = if value < 0.0 { "-" } else { "" };
        return format!("{sign}${grouped}.{:02}", (cents % 100).abs());
    }
    if abs >= 0.01 {
        format!("${value:.4}")
    } else {
        format!("${value:.6}")
    }
}

/// A token count, short: `850`, `1.2K`, `200K`, `1M`.
pub(in crate::workspace) fn compact(value: u64) -> String {
    let short = |value: f64, unit: &str| {
        let rounded = (value * 10.0).round() / 10.0;
        if rounded.fract() == 0.0 {
            format!("{rounded:.0}{unit}")
        } else {
            format!("{rounded:.1}{unit}")
        }
    };
    match value {
        0..1_000 => value.to_string(),
        1_000..999_950 => short(value as f64 / 1e3, "K"),
        _ => short(value as f64 / 1e6, "M"),
    }
}

/// How full the context is, 0 to 1, and as a whole percent; none until the agent reports it.
pub(in crate::workspace) fn share(context: Option<(u64, u64)>) -> Option<(f32, u32)> {
    let (tokens, size) = context.filter(|(_, size)| *size > 0)?;
    let share = (tokens as f64 / size as f64).min(1.0);
    Some((share as f32, (share * 100.0).round() as u32))
}

pub(in crate::workspace) fn agent_name(agent: SessionAgent) -> &'static str {
    nexus_workbench::driver::label(agent)
}

pub(in crate::workspace) fn permissions(agent: SessionAgent, mode: PermissionMode) -> &'static str {
    agent.permission_settings().unwrap_or_else(|| match mode {
        PermissionMode::Default => "Ask first",
        PermissionMode::Auto => "Auto",
        PermissionMode::AcceptEdits => "Accept edits",
        PermissionMode::Plan => "Plan only",
        PermissionMode::BypassPermissions => "Full access",
    })
}

impl Workspace {
    fn toggle_meter(&mut self, id: &str, cx: &mut Context<Self>) {
        match self.overlay.code_top() {
            Some(TopPanel::Meter(open)) if open == id => {
                self.overlay.close_code_top();
            }
            _ => self.open_overlay(Overlay::CodeTop(TopPanel::Meter(id.to_string()))),
        }
        cx.notify();
    }

    /// The pill's words: the session's cost, and while a turn runs, its cost so far ("$0.42 this turn").
    pub(in crate::workspace) fn meter_texts(&self, session: &CodeSession) -> (String, Option<String>) {
        let turn = match &self.view {
            View::Code(view) if view.id == session.id && self.code_running() => {
                view.turn_cost.filter(|cost| *cost > 0.)
            }
            _ => None,
        }
        .map(|cost| format!("{} this turn", usd(cost)));
        (usd(session.cost_usd), turn)
    }

    /// The pill, with its card under it while open. A tight bar leaves out the turn's cost (`with_turn`).
    pub(super) fn render_meter(
        &self,
        session: &CodeSession,
        with_turn: bool,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = matches!(self.overlay.code_top(), Some(TopPanel::Meter(id)) if *id == session.id);
        let share = share(session.context);
        let id = session.id.clone();
        // The amount as a quiet text button: the context share moved into the card and
        // the tooltip, so the bar holds one number.
        // While a turn runs, its cost so far shows beside the session's, so the bar still ticks mid-turn.
        let (cost, turn) = self.meter_texts(session);
        let turn = turn.filter(|_| with_turn);
        let tip: SharedString = match share {
            Some((_, percent)) => format!("Session cost {cost} · context {percent}%").into(),
            None => format!("Session cost {cost}").into(),
        };
        let pill = h_flex()
            .id("code-meter")
            .flex_none()
            .h(px(28.))
            .px(px(8.))
            .rounded(px(6.))
            .when(open, |pill| pill.bg(palette.selected))
            .text_size(px(12.))
            .line_height(px(16.))
            .text_color(palette.muted)
            .cursor_pointer()
            .hover(|style| style.bg(palette.hover).text_color(palette.foreground))
            .when(!open, |pill| {
                pill.tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            })
            .menu_trigger(
                open,
                cx,
                {
                    let id = id.clone();
                    move |this| matches!(this.overlay.code_top(), Some(TopPanel::Meter(open)) if *open == id)
                },
                move |this, _, cx| this.toggle_meter(&id, cx),
            )
            .child(cost)
            .children(turn.map(|turn| div().ml(px(6.)).text_color(palette.faint).child(format!("· {turn}"))));
        div()
            .relative()
            .child(pill)
            .when(open, |meter| meter.child(self.render_meter_card(session, palette, cx)))
            .into_any_element()
    }

    fn render_meter_card(&self, session: &CodeSession, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let head = |title: &'static str, value: String| {
            h_flex()
                .justify_between()
                .gap(px(12.))
                .child(
                    div()
                        .text_color(palette.foreground)
                        .font_weight(FontWeight::MEDIUM)
                        .child(title),
                )
                .child(
                    div()
                        .font_family("monospace")
                        .text_size(px(12.))
                        .text_color(palette.muted)
                        .child(value),
                )
        };
        let quiet = |text: String| div().text_size(px(12.)).text_color(palette.faint).child(text);
        let model = session
            .model
            .as_deref()
            .map(|model| model[model.rfind('/').map_or(0, |at| at + 1)..].to_string())
            .unwrap_or_else(|| "Organization default".into());
        let facts = [
            (
                "Who pays",
                if session.saved_provider {
                    match session.agent {
                        SessionAgent::Opencode => "Your model server",
                        _ => "Your API key",
                    }
                    .to_string()
                } else {
                    match session.via {
                        Via::Subscription => crate::pages::subscriptions::plan_payer(session.agent),
                        Via::Nexus => "Nexus Gateway · API".to_string(),
                    }
                },
            ),
            ("Agent", agent_name(session.agent).to_string()),
            ("Model", model),
            (
                "Effort",
                session
                    .effort
                    .map_or("Default".into(), |effort| effort.as_str().to_string()),
            ),
            ("Permissions", permissions(session.agent, session.mode).to_string()),
        ];
        let mut card = menu_box("code-meter-card", 320., palette)
            .p(px(12.))
            .gap(px(12.))
            .text_size(px(13.))
            .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                if matches!(this.overlay.code_top(), Some(TopPanel::Meter(_))) {
                    this.overlay.close_code_top();
                    cx.notify();
                }
            }))
            .child(
                v_flex()
                    .gap(px(4.))
                    .child(head("Cost", usd(session.cost_usd)))
                    .child(quiet(if session.saved_provider {
                        "The saved provider handles billing. This session sends no usage to Nexus Gateway.".into()
                    } else if session.via == Via::Subscription {
                        "Your plan pays for model calls; this is the agent's estimate.".into()
                    } else {
                        "Reported by the agent. Gateway billing is in Usage.".into()
                    })),
            );
        card = match (session.context, share(session.context)) {
            (Some((tokens, size)), Some((share, percent))) => card.child(
                v_flex()
                    .gap(px(6.))
                    .child(head(
                        "Context window",
                        format!("{} of {} tokens ({percent}%)", compact(tokens), compact(size)),
                    ))
                    .child(meter_bar(share, relative(1.), palette).h(px(6.)))
                    .child(quiet(format!(
                        "{} used · {} left",
                        grouped(tokens),
                        grouped(size.saturating_sub(tokens))
                    ))),
            ),
            _ => card.child(quiet("The agent has not reported context usage.".into())),
        };
        if let (Some((tokens, _)), crate::workspace::View::Code(view)) = (session.context, &self.view)
            && view.id == session.id
            && view.loaded
        {
            let parts = view.context_estimate.parts(tokens);
            card = card.child(
                v_flex()
                    .gap(px(6.))
                    .child(quiet(if view.more {
                        "Estimated from loaded history. Load older messages for more detail.".into()
                    } else if view.context_estimate.since_compaction {
                        "Estimated since the last compaction or clear.".into()
                    } else {
                        "Estimated from this session's messages and tool calls.".into()
                    }))
                    .children(parts.into_iter().map(|part| {
                        h_flex()
                            .justify_between()
                            .gap(px(12.))
                            .text_size(px(12.))
                            .child(div().flex_1().text_color(palette.muted).child(part.label))
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(palette.foreground)
                                    .child(grouped(part.tokens)),
                            )
                    })),
            );
        }
        card = card.child(
            v_flex()
                .gap(px(6.))
                .text_size(px(12.))
                .children(facts.into_iter().map(|(term, value)| {
                    h_flex()
                        .justify_between()
                        .gap(px(12.))
                        .child(div().flex_none().text_color(palette.muted).child(term))
                        .child(
                            div()
                                .min_w(px(0.))
                                .truncate()
                                .text_color(palette.foreground)
                                .child(value),
                        )
                })),
        );
        popover(card, palette)
    }
}

/// A track with its filled share: at least a sliver, so a fresh session's bar still reads as one.
fn meter_bar(share: f32, width: impl Into<Length>, palette: Palette) -> Div {
    div()
        .w(width.into())
        .h(px(4.))
        .rounded(px(2.))
        .bg(palette.selected)
        .child(
            div()
                .h_full()
                .rounded(px(2.))
                .bg(palette.muted)
                .w(relative(share.max(0.02))),
        )
}

/// A whole number with thousands grouped: `12,345`.
fn grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{compact, grouped, share, usd};

    #[test]
    fn cost_reads_as_nexus_ui_writes_it() {
        assert_eq!(usd(0.0), "$0.00");
        assert_eq!(usd(3.42), "$3.42");
        assert_eq!(usd(1234.5), "$1,234.50");
        assert_eq!(usd(0.12345), "$0.1235");
        assert_eq!(usd(0.001234), "$0.001234");
    }

    #[test]
    fn the_share_is_rounded_and_capped() {
        assert_eq!(share(None), None);
        assert_eq!(share(Some((10, 0))), None);
        assert_eq!(share(Some((76_000, 200_000))), Some((0.38, 38)));
        assert_eq!(share(Some((300_000, 200_000))).map(|(_, percent)| percent), Some(100));
    }

    #[test]
    fn token_counts_are_short() {
        assert_eq!(compact(850), "850");
        assert_eq!(compact(1_234), "1.2K");
        assert_eq!(compact(200_000), "200K");
        assert_eq!(compact(1_000_000), "1M");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }
}
