//! Nexus UI's motion (`ui/src/app/app/nexus-ui.css`, `chat/conversation.css`): the same durations and curves, so a
//! message lands here as it does on the web. Only what arrives now moves; what loads with a session does not.

mod dot_strip;
mod field;
mod hover;
#[cfg(test)]
mod loops_capped;
mod mark;
mod presence;
mod reveal;
mod shimmer;
mod solid;
mod spin;
mod tween;
mod typing;
mod unfurl;
mod width;

use std::time::Duration;

pub use dot_strip::dot_strip;
pub use field::field;
pub use hover::hover_fill;
pub use mark::breathing_mark;
pub use presence::{Phase, presence, presence_for, presence_of, presence_of_for};
pub use reveal::reveal;
pub use shimmer::shimmer;
pub use solid::{Solid, solid};
pub use spin::{Arrival, SpinningMark};
pub use tween::{Tween, TweenExt};
pub use typing::TypingDots;
pub use width::{collapse_width, reveal_width};

use gpui_kit::base::input::CursorBlink;
use gpui_kit::component::Theme;
use gpui_kit::{
    Animation, AnimationExt as _, App, ElementId, IntoElement, ParentElement as _, SharedString, Styled, Window, px,
};

/// Popover and menu open (`--nx-dur-ui`).
pub const UI: Duration = Duration::from_millis(160);
/// Anything that closes or leaves (`--nx-dur-ui-exit`).
pub const UI_EXIT: Duration = Duration::from_millis(120);
/// Toast, row insert, a message landing (`--nx-dur-ui-long`).
pub const UI_LONG: Duration = Duration::from_millis(200);
/// Sidebar collapse (`--nx-dur-panel`).
pub const PANEL: Duration = Duration::from_millis(280);
/// A menu or popover opening (`UI`), and closing: quicker, as a menu that is done with gets out of the way (Zeron's
/// `MENU_IN` and `MENU_OUT`).
pub const MENU_IN: Duration = UI;
pub const MENU_OUT: Duration = Duration::from_millis(100);
/// A fill or a color following the pointer (Zeron's `HOVER_FADE`, Tailwind's 150ms `transition-colors`).
pub const HOVER: Duration = Duration::from_millis(150);
/// A group folding or unfolding, a chevron turning (Zeron's `COLLAPSE`).
pub const COLLAPSE: Duration = Duration::from_millis(180);
/// The working dot's breath (`--nx-dur-route-pulse`).
pub const PULSE: Duration = Duration::from_millis(2400);
/// The brand's calm working mark (`motion.breathe` in brand v5's `tokens.json`): opacity 1 to .62 and back, in-out.
pub const BREATHE: Duration = Duration::from_millis(3200);

/// How long a loop for a short wait holds off before it starts (a stopping command's shimmer): a wait that ends
/// sooner shows no loop, so it never flickers.
pub const HOLD_OFF: Duration = Duration::from_millis(400);

/// The frame rate for a loop that never ends (a spinner, a breathing dot, a loading pulse): 30 a second, as the
/// shimmer, the thinking dot and the working dot already run, and as Zeron's shared pulse clock ticks (33ms). An
/// uncapped loop asks for a frame at the display's rate, 60 to 120 a second, and each frame draws again the whole view
/// it sits in: for the working spinner, the stop button and the activity dot that is the whole main area, transcript
/// included, for as long as an agent works.
pub const LOOP_FPS: f32 = 30.;

/// The focused caret becomes steady after a short pause in typing or movement. An unfocused window draws no caret
/// and does not need a blink timer either. The policy is app-wide (GPUI Kit's theme).
pub fn caret_follows_focus(window: &Window, cx: &mut App) {
    let this = window.window_handle().window_id();
    let any_active = window.is_window_active() || cx.active_window().is_some_and(|w| w.window_id() != this);
    let blink = if any_active {
        CursorBlink::default().with_idle_timeout(Duration::from_secs(2))
    } else {
        CursorBlink::steady()
    };
    if Theme::global(cx).cursor_blink != blink {
        Theme::update(cx, |theme| theme.cursor_blink = blink);
    }
}

/// `--nx-ease-out`, `cubic-bezier(0.16, 1, 0.3, 1)`: anything that opens, arrives or moves in.
pub fn ease_out(t: f32) -> f32 {
    cubic_bezier(0.16, 1.0, 0.3, 1.0, t)
}

/// `cubic-bezier(0.4, 0, 0.2, 1)`, Tailwind's and Zeron's standard curve: a color following the pointer.
pub fn standard(t: f32) -> f32 {
    cubic_bezier(0.4, 0.0, 0.2, 1.0, t)
}

/// A CSS `cubic-bezier(x1, y1, x2, y2)` at time `t`: solve the curve's x for `t` (Newton, then bisection), and read its
/// y there.
pub fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let curve = |a: f32, b: f32, s: f32| {
        let u = 1.0 - s;
        3.0 * u * u * s * a + 3.0 * u * s * s * b + s * s * s
    };
    let slope = |a: f32, b: f32, s: f32| {
        let u = 1.0 - s;
        3.0 * u * u * a + 6.0 * u * s * (b - a) + 3.0 * s * s * (1.0 - b)
    };
    let mut s = t;
    for _ in 0..8 {
        let error = curve(x1, x2, s) - t;
        let d = slope(x1, x2, s);
        if error.abs() < 1e-5 {
            return curve(y1, y2, s);
        }
        if d.abs() < 1e-6 {
            break;
        }
        s = (s - error / d).clamp(0.0, 1.0);
    }
    let (mut low, mut high) = (0.0, 1.0);
    s = t;
    for _ in 0..24 {
        let x = curve(x1, x2, s);
        if (x - t).abs() < 1e-5 {
            break;
        }
        if x < t {
            low = s;
        } else {
            high = s;
        }
        s = (low + high) / 2.0;
    }
    curve(y1, y2, s)
}

/// `chat-land` (a question just sent) and `chat-rise` (an answer starting): fade in while moving up `distance` pixels.
pub fn arrive<E: IntoElement + Styled + 'static>(
    element: E,
    id: impl Into<ElementId>,
    distance: f32,
) -> impl IntoElement {
    element.with_animation(
        id,
        Animation::new(UI_LONG).with_easing(ease_out),
        move |element, delta| element.opacity(delta).relative().top(px(distance * (1.0 - delta))),
    )
}

/// A section reveal (`motion.slow` in brand v5's `tokens.json`).
pub const SLOW: Duration = Duration::from_millis(600);

/// One part of a screen that arrives in steps (the sign-in): it rises 8 px and fades in over `SLOW` on the standard
/// curve, `delay` after the screen appeared. Less motion shows it in place (GPUI's one-shot animations end at once).
pub fn rise<E: IntoElement + Styled + 'static>(
    element: E,
    id: impl Into<ElementId>,
    delay: Duration,
) -> impl IntoElement {
    let length = delay + SLOW;
    element.with_animation(id, Animation::new(length), move |element, delta| {
        let into = length.as_secs_f32() * delta - delay.as_secs_f32();
        let t = cubic_bezier(0.2, 0.7, 0.2, 1., (into / SLOW.as_secs_f32()).clamp(0., 1.));
        element.opacity(t).relative().top(px(8. * (1. - t)))
    })
}

/// A title that changed on screen (its agent named it, or the person renamed it): the new title rises 4 px into place
/// as it fades in (`chat-rise`). `since` is when it changed; a title that loads with its view, or changed while out
/// of sight, does not move. Each title has its own animation, so the next change plays again.
pub fn retitle<E: IntoElement + Styled + 'static>(
    element: E,
    title: &str,
    since: Option<std::time::Instant>,
) -> gpui_kit::AnyElement {
    use std::hash::{Hash, Hasher};
    if !since.is_some_and(|at| at.elapsed() < RETITLE_WINDOW) {
        return element.into_any_element();
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    title.hash(&mut hasher);
    arrive(element, ElementId::NamedInteger("retitle".into(), hasher.finish()), 4.).into_any_element()
}

/// How long after a title changes its motion may start: a frame that draws it late still plays the whole motion.
const RETITLE_WINDOW: Duration = Duration::from_millis(600);

/// The size an agent's live mark takes beside a line of text (`solid`, `breathing_mark`): a little over the line's
/// 14 px text, so the 3D icon's depth and the breathing layers read.
pub const LIVE_MARK: f32 = 18.;

/// What an answer shows before its first word: one of the 3D icons (`solid`, picked from `turn`) beside a plain
/// "Thinking…", the row's one moving thing. `id` names the row; `turn` names the answer, so its icon holds.
pub fn thinking(id: impl Into<SharedString>, turn: u64, palette: crate::palette::Palette) -> impl IntoElement {
    let id: SharedString = id.into();
    gpui_kit::component::h_flex()
        .gap(px(8.))
        .child(solid(id, Solid::pick(turn), LIVE_MARK, palette.foreground))
        .child(gpui_kit::div().text_color(palette.muted).child("Thinking…"))
}

/// A received message popping in, as in Messages: it fades in and rises 6 px with a small overshoot, then settles.
/// GPUI draws no scale on a box, so the overshoot stands in for the bubble's grow. Under less motion it shows in place.
pub fn pop<E: IntoElement + Styled + 'static>(element: E, id: impl Into<ElementId>) -> impl IntoElement {
    element.with_animation(id, Animation::new(POP).with_easing(back_out), |element, t| {
        element.opacity(t.clamp(0., 1.)).relative().top(px(6. * (1. - t)))
    })
}

/// A message popping in (`pop`).
pub const POP: Duration = Duration::from_millis(260);

/// `cubic-bezier(0.34, 1.56, 0.64, 1)`, a back-out curve: past the end by about 10%, then back.
pub fn back_out(t: f32) -> f32 {
    cubic_bezier(0.34, 1.56, 0.64, 1., t)
}

/// Fade out only: what a cross-fade leaves (the receipt that was).
pub fn fade_out<E: IntoElement + Styled + 'static>(element: E, id: impl Into<ElementId>) -> impl IntoElement {
    element.with_animation(id, Animation::new(UI).with_easing(ease_out), |element, delta| {
        element.opacity(1. - delta)
    })
}

/// `chat-fade`: fade in only (a popover, the jump-to-latest button).
pub fn fade_in<E: IntoElement + Styled + 'static>(element: E, id: impl Into<ElementId>) -> impl IntoElement {
    element.with_animation(id, Animation::new(UI).with_easing(ease_out), |element, delta| {
        element.opacity(delta)
    })
}

/// A dragged size coming to rest on its last whole step (a shell's well on its last line): as quick as an exit.
pub const SETTLE: Duration = UI_EXIT;
/// How long a scrollbar stays after the last wheel turn, before it fades out.
pub const SCROLLBAR_LINGER: Duration = Duration::from_millis(800);
/// How far a shell joining a split slides in from the right.
const SLIDE_IN: f32 = 16.;

/// A control that shows only when it is wanted (a resize grip, a scrollbar): its opacity eases to `opacity` over
/// `HOVER`, on the standard curve, and turns around from where it is.
pub fn fade_to<E: IntoElement + Styled + 'static>(
    element: E,
    id: impl Into<ElementId>,
    opacity: f32,
) -> impl IntoElement {
    element.with_tween(id, opacity, Tween::new(HOVER, standard), |element, value| {
        element.opacity(value)
    })
}

/// A box whose height changes: it eases to `height` over `duration`, ease out, from where it is drawn now. Use `PANEL`
/// for a box that takes the pane or gives it back (a maximized well), `SETTLE` for a drag's last step, and
/// `Duration::ZERO` while a drag moves it, so it follows the pointer. Keep the same `id` for all three: the element
/// must stay in the tree while it drags, or the element under it loses its press. A box that loads with its view sits
/// still.
pub fn ease_height<E: IntoElement + Styled + 'static>(
    element: E,
    id: impl Into<ElementId>,
    height: f32,
    duration: Duration,
) -> impl IntoElement {
    element.with_tween(id, height, Tween::new(duration, ease_out), |element, value| {
        element.h(px(value))
    })
}

/// A shell joining a split: it slides in `SLIDE_IN` px from the right and fades in over `PANEL`, ease out.
pub fn slide_in<E: IntoElement + Styled + 'static>(element: E, id: impl Into<ElementId>) -> impl IntoElement {
    element.with_animation(id, Animation::new(PANEL).with_easing(ease_out), |element, delta| {
        element.opacity(delta).relative().left(px(SLIDE_IN * (1. - delta)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui_kit::test]
    fn an_active_window_sets_a_finite_caret_blink_timeout(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::VisualContext as _;

        cx.update(gpui_kit::init);
        let cx = cx.add_empty_window();
        let handle = cx.window_handle();
        handle.update(cx, |_, window, _| window.activate_window()).unwrap();
        cx.run_until_parked();
        handle
            .update(cx, |_, window, cx| {
                assert!(window.is_window_active());
                caret_follows_focus(window, cx);
                let blink = Theme::global(cx).cursor_blink;
                assert_eq!(blink.interval(), CursorBlink::default().interval());
                assert_eq!(blink.idle_timeout(), Some(Duration::from_secs(2)));
            })
            .unwrap();
    }

    #[test]
    fn the_curve_starts_and_ends_in_place() {
        assert!(ease_out(0.0).abs() < 1e-3);
        assert!((ease_out(1.0) - 1.0).abs() < 1e-3);
    }

    #[test]
    fn ease_out_moves_most_of_the_way_early() {
        // cubic-bezier(0.16, 1, 0.3, 1) is about 83% of the way at a quarter of the time.
        assert!((ease_out(0.25) - 0.826).abs() < 0.01);
        assert!(ease_out(0.5) > ease_out(0.25));
    }

    #[test]
    fn linear_is_identity() {
        for t in [0.1, 0.4, 0.7] {
            assert!((cubic_bezier(1.0 / 3.0, 1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0, t) - t).abs() < 1e-3);
        }
    }
}
