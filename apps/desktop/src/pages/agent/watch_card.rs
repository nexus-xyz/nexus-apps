//! The proposal card in the thread (brand v5 A3·b): the person asked "tell me when…", and Nexus proposes the
//! watch as a notice (`topic: "watches"`, `state: "proposed"`). The card says what it would do (When, Checks, Until,
//! Tells you) and offers Keep and Not now. Nothing is kept and nothing runs until Keep. The card keeps the proposal's
//! cadence and channels: Nexus UI's card lets the person change them first, and the Watches view edits them after.

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{AgentPage, body, card, card_title};
use crate::agent::Call;
use crate::pages::assistant_views::{Reply, call, pill, primary_pill, quiet_pill};
use crate::palette::Palette;
use nexus_desktop_core::agent_watches::{self as words, Proposal, Read};

/// Where one proposal stands.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Stand {
    Working,
    Kept,
    Declined,
    Refused(String),
}

impl AgentPage {
    /// The card under the proposal's own line.
    pub(super) fn watch_card(
        &mut self,
        key: (usize, usize),
        proposal: &Proposal,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let now = chrono::Utc::now();
        let until = (now + chrono::Duration::days(i64::from(proposal.days))).to_rfc3339();
        let id = key.0 * 100 + key.1;
        let stand = self.watch_stands.get(&key).cloned();
        let mut card = card(palette)
            .child(card_title("Watch", palette))
            .child(body(
                match words::place(&proposal.target) {
                    Some(place) => format!("{} · {place}", words::name(&proposal.target)),
                    None => words::name(&proposal.target),
                },
                palette,
            ))
            .child(fact("When", words::when(&proposal.target, proposal.every), palette))
            .child(fact(
                "Checks",
                words::checks(&proposal.target, proposal.cadence),
                palette,
            ))
            .child(fact(
                "Until",
                words::until_text(&proposal.target, proposal.every, &until, now.timestamp_millis()),
                palette,
            ))
            .child(fact("Tells you", words::tells_text(&proposal.tells), palette));
        card = match stand {
            Some(Stand::Kept) => card.child(note(
                "Kept. I’ll tell you when it happens. Pause or delete it in Watches.",
                palette,
            )),
            Some(Stand::Declined) => card.child(note("Not kept. Nothing runs.", palette)),
            other => {
                let working = other == Some(Stand::Working);
                let proposal = proposal.clone();
                let keep = if working {
                    quiet_pill(("agent-watch-keep", id), "Keep", palette).into_any_element()
                } else {
                    primary_pill(("agent-watch-keep", id), "Keep", palette)
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.keep_watch(key, proposal.clone(), window, cx)),
                        )
                        .into_any_element()
                };
                let decline = if working {
                    quiet_pill(("agent-watch-decline", id), "Not now", palette).into_any_element()
                } else {
                    pill(("agent-watch-decline", id), "Not now", palette)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.watch_stands.insert(key, Stand::Declined);
                            cx.notify();
                        }))
                        .into_any_element()
                };
                card.child(note("Nothing runs until you keep it.", palette))
                    .children(match other {
                        Some(Stand::Refused(text)) => Some(body(text, palette)),
                        _ => None,
                    })
                    .child(h_flex().gap(px(8.)).child(keep).child(decline))
            }
        };
        card.into_any_element()
    }

    /// Keep: `POST /agent/watches` with the proposal. The card says what happened.
    fn keep_watch(&mut self, key: (usize, usize), proposal: Proposal, window: &mut Window, cx: &mut Context<Self>) {
        let Some(made) = Call::keep_watch(&proposal) else {
            return;
        };
        self.watch_stands.insert(key, Stand::Working);
        let task = call(&self.ctx, made, window, cx, move |this, reply: Reply, _, _| {
            let stand = match words::watch_read(reply.status, reply.body.as_ref()) {
                Read::Ok(_) => Stand::Kept,
                Read::Off => Stand::Refused(words::error_text(Some("watches_off")).into()),
                Read::Failed { error, .. } => Stand::Refused(words::error_text(error.as_deref()).into()),
            };
            this.watch_stands.insert(key, stand);
        });
        self._watch_keeps.push(task);
        cx.notify();
    }
}

fn fact(label: &'static str, value: String, palette: Palette) -> Div {
    h_flex()
        .w_full()
        .gap(px(12.))
        .child(
            div()
                .w(px(76.))
                .flex_none()
                .text_size(px(13.))
                .text_color(palette.faint)
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .text_size(px(13.))
                .text_color(palette.foreground)
                .child(value),
        )
}

fn note(text: &'static str, palette: Palette) -> Div {
    v_flex().child(div().text_size(px(12.5)).text_color(palette.muted).child(text))
}
