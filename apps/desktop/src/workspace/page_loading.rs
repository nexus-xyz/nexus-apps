//! What the page panel shows while a page opens: the outline of a page, breathing, under the see-through page view
//! ([`crate::web::WebView::veil`]), so the pane never shows the engine's white or an empty field. It appears only
//! after [`APPEAR_AFTER`], so a page that opens fast shows no loader at all. Past [`SLOW_AFTER`] it says the page is
//! still loading. When the page shows, the outline fades out over it.
//!
//! It is its own view, so its breathing redraws only itself, at [`motion::LOOP_FPS`] (render rules 4 and 5).
//!
//! A page that did not load, twice, shows [`failed`] in its place instead, as Nexus UI's `PageBlocked`.

use std::time::{Duration, Instant};

use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::motion;
use crate::tokens::radius;

/// A loader that flashes on for a fast open is worse than none.
pub(super) const APPEAR_AFTER: Duration = Duration::from_millis(150);
/// When the panel says the page is slow, and shows a page that drew but never finished loading.
pub(super) const SLOW_AFTER: Duration = Duration::from_secs(4);

pub(super) struct PageLoading {
    since: Instant,
    /// The page's last picture covers the outline: when the page shows, there is nothing to fade.
    poster: bool,
    slow: bool,
    /// The page shows: the outline fades out over it.
    leaving: bool,
}

impl PageLoading {
    pub(super) fn new(poster: bool) -> Self {
        PageLoading {
            since: Instant::now(),
            poster,
            slow: false,
            leaving: false,
        }
    }

    pub(super) fn set_slow(&mut self, cx: &mut Context<Self>) {
        if !self.slow && !self.leaving {
            self.slow = true;
            cx.notify();
        }
    }

    pub(super) fn leave(&mut self, cx: &mut Context<Self>) {
        if !self.leaving {
            self.leaving = true;
            cx.notify();
        }
    }
}

/// The outline's opacity `elapsed` after the page started opening: none before [`APPEAR_AFTER`], then a fade in over
/// [`motion::UI`].
pub(super) fn appear(elapsed: Duration) -> f32 {
    let into = elapsed.saturating_sub(APPEAR_AFTER).as_secs_f32();
    motion::ease_out((into / motion::UI.as_secs_f32()).clamp(0., 1.))
}

/// An outline of a page: an eyebrow, a title, three lines and two cards.
fn outline(palette: crate::palette::Palette) -> Div {
    let bar = |width: f32, height: f32, color: Hsla| {
        div()
            .w(relative(width))
            .h(px(height))
            .flex_none()
            .rounded(px(radius::INLINE))
            .bg(color)
    };
    let card = || {
        div()
            .flex_1()
            .h(px(160.))
            .rounded(px(radius::CARD))
            .border_1()
            .border_color(palette.border)
            .bg(palette.raised)
    };
    div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(bar(0.28, 9., palette.hover))
        .child(div().h(px(4.)))
        .child(bar(0.72, 24., palette.selected))
        .child(div().h(px(4.)))
        .child(bar(0.92, 10., palette.hover))
        .child(bar(0.84, 10., palette.hover))
        .child(bar(0.6, 10., palette.hover))
        .child(div().flex().gap(px(14.)).mt(px(10.)).child(card()).child(card()))
}

/// What shows in place of a page that did not load, tried twice: the engine's own error page names the sandbox host,
/// which means nothing to the reader, so this says where the page is served from and offers `retry`.
pub(super) fn failed(host: Option<&str>, palette: crate::palette::Palette, retry: impl IntoElement) -> Div {
    let from = match host {
        Some(host) => format!("Nexus serves pages from {host}."),
        None => "Nexus serves pages from their own host.".to_string(),
    };
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap(px(10.))
        .px(px(30.))
        .bg(palette.background)
        .child(
            div()
                .text_size(px(15.))
                .text_color(palette.foreground)
                .child("This Artifact didn't load"),
        )
        .child(
            div()
                .max_w(px(420.))
                .text_center()
                .text_size(px(13.))
                .text_color(palette.muted)
                .child(format!("{from} Your network, a VPN or another app may be blocking it.")),
        )
        .child(retry)
}

impl Render for PageLoading {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = crate::pages::palette(cx);
        let layer = div().absolute().top_0().left_0().size_full().bg(palette.background);
        if self.leaving {
            if self.poster {
                return div().absolute().into_any_element();
            }
            // Still: a loop must not run under a page that shows.
            let still = layer.px(px(30.)).py(px(28.)).child(outline(palette).opacity(0.7));
            return motion::fade_out(still, "page-loading-leave").into_any_element();
        }
        let breathing = outline(palette).with_animation(
            "page-loading-breath",
            Animation::new(motion::PULSE)
                .repeat()
                .with_max_fps(motion::LOOP_FPS)
                .with_easing(pulsating_between(0.45, 0.85)),
            |outline, delta| outline.opacity(delta),
        );
        let slow = self.slow.then(|| {
            div()
                .absolute()
                .left(px(30.))
                .right(px(30.))
                .bottom(px(28.))
                .text_size(px(13.))
                .text_color(palette.muted)
                .child("Still loading. The page is large, or the network is slow.")
        });
        let since = self.since;
        let content = v_flex().size_full().px(px(30.)).py(px(28.)).child(breathing);
        // The outline waits, then fades in: one animation whose length covers the wait.
        let content = content.with_animation(
            "page-loading-appear",
            Animation::new(APPEAR_AFTER + motion::UI),
            move |content, _| content.opacity(appear(since.elapsed())),
        );
        layer.child(content).children(slow).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn a_fast_open_shows_no_loader() {
        assert_eq!(appear(Duration::ZERO), 0.);
        assert_eq!(appear(APPEAR_AFTER), 0.);
        assert!(appear(APPEAR_AFTER + motion::UI / 2) > 0.);
        assert!((appear(APPEAR_AFTER + motion::UI) - 1.).abs() < 1e-3);
        assert!((appear(Duration::from_secs(10)) - 1.).abs() < 1e-3);
    }
}
