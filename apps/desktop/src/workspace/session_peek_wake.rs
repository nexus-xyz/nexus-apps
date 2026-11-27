//! The hover card's wake-up (`session_peek.rs`): when the session's agent goes on by itself (`ThreadSummary::wake`),
//! and why, in its own words. The state line says when; this section says why. Under a louder state it starts with
//! "Also scheduled", as the background section starts with "Also in background". Read-only, as the rest of the card.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::event::Wake;

use super::glyph;
use super::sidebar::hint;
use crate::code::Status;
use crate::i18n::tr;
use crate::palette::Palette;

/// How long until `at` from `now` (epoch milliseconds), rounded up to the minute: "in 18m", "in 1h 5m". None once due.
pub(super) fn until(at: i64, now: i64) -> Option<String> {
    let left = at - now;
    if left <= 0 {
        return None;
    }
    let minutes = (left + 59_999) / 60_000;
    let text = match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h {m}m"),
    };
    Some(tr("in {time}").replace("{time}", &text))
}

/// The local time of day of `at`, as "14:32".
fn clock(at: i64) -> String {
    chrono::DateTime::from_timestamp_millis(at)
        .map(|at| at.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_default()
}

/// The state line's words for a scheduled session: "Wakes at 14:32 · in 18m", or "Wakes now" once due.
pub(super) fn state_text(wake: &Wake, now: i64, clock: impl Fn(i64) -> String) -> String {
    match until(wake.at, now) {
        Some(left) => tr("Wakes at {time} · {left}")
            .replace("{time}", &clock(wake.at))
            .replace("{left}", &left),
        None => tr("Wakes now").to_string(),
    }
}

/// The state line's word for a scheduled session, styled as the card's other state words.
pub(super) fn state_word(wake: &Wake, now: i64, palette: Palette) -> AnyElement {
    div()
        .flex_none()
        .text_size(px(11.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(palette.muted)
        .child(state_text(wake, now, clock))
        .into_any_element()
}

/// The section for `session`'s wake-up under its `mark`; None when it has none.
pub(super) fn section(wake: Option<&Wake>, mark: Status, now: i64, palette: Palette) -> Option<Div> {
    let wake = wake?;
    let scheduled = matches!(mark, Status::Scheduled | Status::ScheduledUnread);
    let note = |text: SharedString| div().text_size(px(11.)).text_color(palette.faint).child(text);
    let reason = match wake.reason.trim() {
        "" => tr("The agent set a wake-up").to_string(),
        reason => reason.to_string(),
    };
    // The state line says when for a scheduled session; under a louder one, this line does.
    let when = (!scheduled).then(|| state_text(wake, now, clock));
    Some(
        v_flex()
            .gap(px(6.))
            .pt(px(8.))
            .border_t_1()
            .border_color(palette.border)
            .when(!scheduled, |list| list.child(note(tr("Also scheduled"))))
            .child(
                h_flex()
                    .gap(px(8.))
                    .min_w(px(0.))
                    .child(glyph(Lucide::AlarmClock, palette.faint).size(px(14.)))
                    .child(div().flex_1().min_w(px(0.)).truncate().child(reason))
                    .when_some(when, |row, when| row.child(hint(when, palette))),
            )
            .child(note(tr("The agent goes on by itself then."))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    fn wake(at: i64) -> Wake {
        Wake {
            at,
            set_at: 0,
            reason: "watching CI run".into(),
        }
    }

    #[test]
    fn says_how_long_until_it_wakes_rounded_up_to_the_minute() {
        let now = 1_000_000;
        assert_eq!(until(now + 1, now).as_deref(), Some("in 1m"));
        assert_eq!(until(now + 18 * 60_000, now).as_deref(), Some("in 18m"));
        assert_eq!(until(now + 18 * 60_000 + 1, now).as_deref(), Some("in 19m"));
        assert_eq!(until(now + 60 * 60_000, now).as_deref(), Some("in 1h"));
        assert_eq!(until(now + 65 * 60_000, now).as_deref(), Some("in 1h 5m"));
        assert_eq!(until(now, now), None);
        assert_eq!(until(now - 1, now), None);
    }

    #[test]
    fn the_state_line_says_when_and_then_now() {
        let at = 10 * 60_000;
        let clock = |_| "14:32".to_string();
        assert_eq!(state_text(&wake(at), 0, clock), "Wakes at 14:32 · in 10m");
        assert_eq!(state_text(&wake(at), at, clock), "Wakes now");
    }

    #[test]
    fn no_wake_up_has_no_section() {
        let palette = Palette::for_choice(crate::palette::ThemeChoice::Dark, WindowAppearance::Dark);
        assert!(section(None, Status::Idle, 0, palette).is_none());
        assert!(section(Some(&wake(60_000)), Status::Scheduled, 0, palette).is_some());
    }
}
