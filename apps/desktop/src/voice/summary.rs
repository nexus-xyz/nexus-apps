//! The card that lands in the thread when a call ends (Nexus UI's `voice-summary-card.tsx`): what Nexus did, or
//! what was asked, the call's length, and the transcript, folded. Raised with a hairline, as every card in the thread.
//! A write the call asked for lands after it as its own approval card; this one only names it.

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::call::Who;
use super::spoken::VoiceSummary;
use crate::palette::Palette;

/// The lead line over the list.
pub fn lead(summary: &VoiceSummary) -> &'static str {
    if summary.acted {
        "Here's what we did:"
    } else {
        "Here's what we talked about:"
    }
}

/// When the call started, on this computer's clock.
fn started(at: i64) -> Option<String> {
    let at = chrono::DateTime::from_timestamp_millis(at)?;
    Some(at.with_timezone(&chrono::Local).format("%H:%M").to_string())
}

/// The card for the thread's entry `ix`. The transcript opens and folds on a click, kept while the thread is drawn.
pub fn card(ix: usize, summary: &VoiceSummary, palette: Palette, window: &mut Window, cx: &mut App) -> AnyElement {
    let open = window.use_keyed_state(("voice-summary-open", ix), cx, |_, _| false);
    let shown = *open.read(cx);
    let faint = |text: String| div().text_size(px(12.5)).text_color(palette.faint).child(text);
    let transcript = summary.turns.iter().enumerate().map(|(n, turn)| {
        h_flex()
            .id(("voice-summary-turn", ix * 1000 + n))
            .items_start()
            .gap(px(6.))
            .child(
                div()
                    .flex_none()
                    .text_size(px(12.))
                    .text_color(palette.faint)
                    .child(if turn.who == Who::You { "You" } else { "Nexus" }),
            )
            .child(div().flex_1().min_w(px(0.)).child(turn.text.clone()))
    });
    v_flex()
        .id(("voice-summary", ix))
        .w_full()
        .max_w(px(440.))
        .px(px(14.))
        .py(px(12.))
        .rounded(px(12.))
        .bg(palette.raised)
        .border_1()
        .border_color(palette.border)
        .text_size(px(14.))
        .line_height(relative(1.45))
        .text_color(palette.foreground)
        .child(div().text_color(palette.strong).child(lead(summary)))
        .child(faint(format!("Voice · {}", summary.length)).mt(px(2.)).mb(px(6.)))
        .children(summary.done.iter().map(|line| {
            h_flex()
                .items_start()
                .gap(px(8.))
                .pl(px(4.))
                .child(div().flex_none().text_color(palette.faint).child("•"))
                .child(div().flex_1().min_w(px(0.)).child(line.clone()))
        }))
        .child(
            h_flex()
                .id(("voice-summary-transcript", ix))
                .mt(px(8.))
                .gap(px(8.))
                .text_size(px(13.))
                .text_color(palette.muted)
                .cursor_pointer()
                .hover(|style| style.text_color(palette.strong))
                .on_click(move |_, _, cx| {
                    open.update(cx, |open, cx| {
                        *open = !*open;
                        cx.notify();
                    })
                })
                .child(if shown { "▾ Transcript" } else { "▸ Transcript" })
                .children(started(summary.started_at).map(|time| div().text_color(palette.faint).child(time))),
        )
        .when(shown, |card| {
            card.child(v_flex().mt(px(8.)).gap(px(6.)).children(transcript))
        })
        .into_any_element()
}
