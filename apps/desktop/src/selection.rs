//! The window's text selection, around GPUI Kit's selection layer (`gpui_kit::base::TextSelection`).

use gpui_kit::base::TextSelection;
use gpui_kit::{DispatchPhase, IntoElement, MouseButton, MouseUpEvent, Styled, canvas};

/// Each frame: a left release ends the drag selection in its capture phase, before the elements drawn after this
/// listener see the release. The kit's layer ends it itself, but last in the release's bubble phase, so a click
/// handler that calls `cx.stop_propagation()` (a button inside a row with its own click) keeps it running, and the
/// pointer then selects text with no button held. Draw it in the root of every window that shows text; the web
/// popup (`side/browser/popup.rs`) shows only a page.
///
/// The kit also clears its "press prepared" flag in that skipped handler, and the flag is private. So after a
/// swallowed release, the next press does not clear selections that text elements keep themselves (Ctrl+A's) first.
/// The swallowed press cleared them already, so this shows only when something was selected in between, until the
/// press after that.
pub fn release_ends_selection() -> impl IntoElement {
    canvas(
        |_, _, _| {},
        |_, _, window, _| {
            window.on_mouse_event(|event: &MouseUpEvent, phase, window, cx| {
                if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                    TextSelection::end(window, cx);
                }
            });
        },
    )
    .absolute()
    .size_0()
}

#[cfg(test)]
mod tests {
    use crate::workspace::message_text::MessageText;
    use core::prelude::v1::test;
    use gpui_kit::prelude::*;
    use gpui_kit::*;

    /// A button that stops its click, as a subagent row's Open does (`workspace/code_view.rs`, `inspect-agent`), above
    /// a line of text that can be selected.
    struct Probe {
        guarded: bool,
    }

    impl Render for Probe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .text_size(px(14.))
                .line_height(px(22.))
                .when(self.guarded, |root| root.child(super::release_ends_selection()))
                .child(
                    div()
                        .id("probe-open")
                        .w(px(60.))
                        .h(px(24.))
                        .child("Open")
                        .on_click(|_, _, cx| cx.stop_propagation()),
                )
                .child(
                    div()
                        .mt(px(12.))
                        .w(px(240.))
                        .child(MessageText::new("probe-text", "conversation text")),
                )
        }
    }

    fn probe(guarded: bool, cx: &mut TestAppContext) -> &mut VisualTestContext {
        cx.update(gpui_kit::init);
        let window = cx.add_window(move |window, cx| {
            let probe = cx.new(|_| Probe { guarded });
            gpui_kit::base::Root::new(probe, window, cx)
        });
        let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx
    }

    /// Clicks Open, then moves over the text with no button held. Returns what got selected.
    fn click_open_then_hover(cx: &mut VisualTestContext) -> String {
        cx.simulate_click(point(px(20.), px(12.)), Modifiers::none());
        cx.simulate_mouse_move(point(px(200.), px(46.)), None, Modifiers::none());
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            gpui_kit::base::TextSelection::selected_text(window, cx)
        })
    }

    #[gpui_kit::test]
    fn release_swallowed_by_a_click_ends_the_selection(cx: &mut TestAppContext) {
        let cx = probe(true, cx);
        assert_eq!(click_open_then_hover(cx), "");
    }

    #[gpui_kit::test]
    fn swallowed_release_without_the_listener_keeps_selecting(cx: &mut TestAppContext) {
        // What the listener fixes: without it the hover after the click selects text. This fails once GPUI Kit ends
        // the drag where a click cannot stop it; then the listener can go.
        let cx = probe(false, cx);
        assert_ne!(click_open_then_hover(cx), "");
    }

    #[gpui_kit::test]
    fn drag_over_text_still_selects_it(cx: &mut TestAppContext) {
        let cx = probe(true, cx);
        cx.simulate_mouse_down(point(px(2.), px(46.)), MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(point(px(200.), px(46.)), Some(MouseButton::Left), Modifiers::none());
        cx.simulate_mouse_up(point(px(200.), px(46.)), MouseButton::Left, Modifiers::none());
        let selected = cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            gpui_kit::base::TextSelection::selected_text(window, cx)
        });
        assert!(selected.starts_with("conversation"), "{selected:?}");
    }
}
