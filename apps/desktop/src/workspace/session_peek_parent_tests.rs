//! A cross-project subsession's parent line wraps inside the hover card, up to `PARENT_LINES` lines.
use super::*;
use core::prelude::v1::test;

struct Preview {
    text: String,
    palette: Palette,
}

impl Render for Preview {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // A sidebar row truncates its text. The card must explicitly restore wrapping.
        div().whitespace_nowrap().child(frame(
            v_flex()
                .w_full()
                .p(px(12.))
                .child(parent_line(self.text.clone(), self.palette)),
            false,
            self.palette,
            window,
        ))
    }
}

/// The line's bounds in a card that shows `text`.
fn drawn(text: &str, cx: &mut TestAppContext) -> (Bounds<Pixels>, Bounds<Pixels>) {
    cx.update(gpui_kit::init);
    let text = text.to_string();
    let (_, cx) = cx.add_window_view(|_, _| Preview {
        text,
        palette: Palette::for_choice(crate::palette::ThemeChoice::Dark, WindowAppearance::Dark),
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let card = cx.debug_bounds("session-peek-card").expect("the card");
    let line = cx.debug_bounds("session-peek-parent").expect("the parent line");
    (card, line)
}

#[gpui_kit::test]
fn a_long_parent_title_wraps_inside_the_card(cx: &mut TestAppContext) {
    let text = super::super::cross_project::subsession_line(
        Some("Move the gateway's invite codes to the audited routes and check the deploys"),
        Some("nexus-web"),
    );
    let (card, line) = drawn(&text, cx);
    assert!(line.size.height > px(18.), "it wraps: {line:?}");
    assert!(
        line.left() >= card.left() && line.right() <= card.right(),
        "it stays inside the card: {line:?}"
    );
}

#[gpui_kit::test]
fn a_very_long_parent_title_stops_at_three_lines(cx: &mut TestAppContext) {
    let title = "Move the gateway's invite codes to the audited routes ".repeat(6);
    let text = super::super::cross_project::subsession_line(Some(&title), Some("nexus-web"));
    let (_, line) = drawn(&text, cx);
    assert!(
        line.size.height <= px(18. * PARENT_LINES as f32),
        "at most {PARENT_LINES} lines: {line:?}"
    );
}

#[gpui_kit::test]
fn a_short_line_takes_one_line(cx: &mut TestAppContext) {
    let (_, line) = drawn("Subsession of “Fix it” in api", cx);
    assert_eq!(line.size.height, px(18.));
}
