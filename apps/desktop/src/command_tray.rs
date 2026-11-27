//! A shared height budget for command activity above the composer.
mod motion;
pub use motion::Motion;

use gpui_kit::prelude::*;
use gpui_kit::*;
use std::{cell::Cell, rc::Rc};

pub const HEADER: f32 = 44.;
pub const REVIEW_MIN: f32 = 180.;

#[derive(Default)]
pub struct Layout {
    pane: Rc<Cell<f32>>,
    composer: Rc<Cell<f32>>,
    notices: Rc<Cell<f32>>,
}

impl Layout {
    pub fn pane_height(&self) -> f32 {
        self.pane.get().max(160.)
    }

    pub fn primary_limit(&self, groups: usize) -> f32 {
        (self.pane_height() - self.notices.get() - 28. - (HEADER + 8.) * groups as f32)
            .max(0.)
            .min(self.pane_height() * 0.5)
    }

    pub fn budget(&self, groups: usize) -> f32 {
        budget(
            self.pane_height(),
            self.composer.get().max(104.) + self.notices.get(),
            groups,
        )
    }

    pub fn clear_notices(&self) {
        self.notices.set(0.);
    }

    pub fn measure_notices<T: 'static>(&self, cx: &Context<T>) -> AnyElement {
        measure_height(self.notices.clone(), cx)
    }

    /// Measure the actual pane and composer, including their padding, after layout.
    pub fn measure<T: 'static>(&self, composer: bool, cx: &Context<T>) -> AnyElement {
        measure_height(
            if composer {
                self.composer.clone()
            } else {
                self.pane.clone()
            },
            cx,
        )
    }
}

fn measure_height<T: 'static>(height: Rc<Cell<f32>>, cx: &Context<T>) -> AnyElement {
    let owner = cx.weak_entity();
    canvas(
        move |bounds, _, cx| {
            let next = f32::from(bounds.size.height).round();
            if (height.get() - next).abs() >= 1. {
                height.set(next);
                let owner = owner.clone();
                cx.defer(move |cx| {
                    let _ = owner.update(cx, |_, cx| cx.notify());
                });
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .size_full()
    .into_any_element()
}

pub fn budget(pane: f32, composer: f32, groups: usize) -> f32 {
    if groups == 0 {
        return 0.;
    }
    let reserve = 160_f32.min(pane * 0.25);
    let available = (pane - composer - reserve - 28.).max(0.);
    let total = 300_f32.min(pane * 0.35).min(available);
    ((total - 8. * groups.saturating_sub(1) as f32) / groups as f32).max(HEADER)
}

/// Both command routes use the same outer bound. Only the middle region can shrink or scroll.
pub fn frame(
    id: &'static str,
    limit: f32,
    header: AnyElement,
    body: Option<AnyElement>,
    footer: Option<AnyElement>,
    palette: crate::palette::Palette,
) -> AnyElement {
    gpui_kit::component::v_flex()
        .id(id)
        .debug_selector(move || id.into())
        // Children consume their scroll first; the tray never passes it to the transcript.
        .on_scroll(|_, _, cx| cx.stop_propagation())
        .w_full()
        .min_w(px(0.))
        .min_h(px(0.))
        .max_h(px(limit))
        .overflow_hidden()
        .px(px(10.))
        .py(px(6.))
        .gap(px(8.))
        .rounded(px(16.))
        .bg(palette.raised)
        // The composer's lift, not a hairline: the tray sits on the transcript as the composer under it does.
        .shadow(crate::pages::ui::lift(palette))
        .child(div().flex_none().child(header))
        .children(body)
        .children(footer)
        .into_any_element()
}

/// Keep bulk decisions beside the permission summary, before the Commands button.
pub fn decision_header(summary: AnyElement, actions: AnyElement, commands: AnyElement) -> Div {
    gpui_kit::component::h_flex()
        .w_full()
        .min_w(px(0.))
        .gap(px(8.))
        .child(div().flex_1().min_w(px(0.)).child(summary))
        .child(
            div()
                .debug_selector(|| "command-bulk-actions".into())
                .flex_none()
                .child(actions),
        )
        .child(
            div()
                .debug_selector(|| "command-pane-action".into())
                .flex_none()
                .child(commands),
        )
}

/// Every request, including expanded cards, belongs to this one scrolling list.
pub fn request_list(limit: f32, scroll: &ScrollHandle, cards: Vec<AnyElement>) -> Stateful<Div> {
    gpui_kit::component::v_flex()
        .id("inline-command-list")
        .debug_selector(|| "inline-command-list".into())
        .w_full()
        .min_w(px(0.))
        .min_h(px(0.))
        .max_h(px(limit))
        .overflow_y_scroll()
        .track_scroll(scroll)
        .gap(px(6.))
        .children(cards)
}

/// A command's decision header scrolls with its card; its long text has a separate reading region.
pub fn review_card(limit: f32, header: AnyElement, body: AnyElement, palette: crate::palette::Palette) -> AnyElement {
    gpui_kit::component::v_flex()
        .id("command-review-card")
        .debug_selector(|| "command-review-card".into())
        .w_full()
        .min_w(px(0.))
        .min_h(px(0.))
        .max_h(px(limit.max(0.)))
        .overflow_hidden()
        .rounded(px(10.))
        .bg(palette.background)
        .child(
            div()
                .debug_selector(|| "command-review-header".into())
                .flex_none()
                .px(px(2.))
                .py(px(4.))
                .child(header),
        )
        .child(
            gpui_kit::component::v_flex()
                .w_full()
                .min_w(px(0.))
                .min_h(px(0.))
                .max_h(px((limit - 48.).max(0.)))
                .on_scroll(|_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .id("command-review-text")
                        .debug_selector(|| "command-review-text".into())
                        .w_full()
                        .min_w(px(0.))
                        .min_h(px(0.))
                        .max_h(px((limit - 48.).max(0.)))
                        .overflow_y_scroll()
                        .px(px(28.))
                        .pt(px(2.))
                        .pb(px(10.))
                        .child(body),
                ),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests;
