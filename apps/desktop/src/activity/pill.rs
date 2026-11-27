use std::time::Instant;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{
    Control, Item, control, placement,
    state::{PEEK, RETRACT, Reveal},
};
use crate::pages::ui;
use crate::palette::{Palette, ThemeChoice};

pub(super) struct Pill {
    pub items: Vec<Item>,
    pub(super) reveal: Reveal,
    focus: FocusHandle,
    _hover: Option<Task<()>>,
    zoom: f32,
}

impl Pill {
    pub fn new(items: Vec<Item>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            items,
            reveal: Reveal::default(),
            focus: cx.focus_handle(),
            _hover: None,
            zoom: gpui_kit::ui_zoom(),
        }
    }

    pub fn set_items(&mut self, items: Vec<Item>, window: &mut Window, cx: &mut Context<Self>) {
        self.items = items;
        self.resize(window, cx);
        cx.notify();
    }

    fn resize(&self, window: &mut Window, cx: &App) {
        let (width, height) = if self.reveal.open {
            (
                placement::EXPANDED,
                placement::HEADER + self.items.len().min(3) as f32 * 144.,
            )
        } else {
            (placement::COMPACT, placement::HEADER)
        };
        let height = window
            .display(cx)
            .map(|display| height.min((f32::from(display.visible_bounds().size.height) - 20.) / gpui_kit::ui_zoom()))
            .unwrap_or(height);
        placement::resize(window, width, height, cx.reduce_motion());
    }

    fn hover(&mut self, hovered: bool, window: &mut Window, cx: &mut Context<Self>) {
        let now = Instant::now();
        self.reveal.hover(hovered, now);
        let delay = if hovered { PEEK } else { RETRACT };
        let due = now + delay;
        self._hover = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.reveal.tick(due) {
                    this.resize(window, cx);
                    cx.notify();
                }
            });
        }));
    }

    fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reveal.toggle();
        self._hover = None;
        self.resize(window, cx);
        cx.notify();
    }

    fn details(&self, item: &Item, palette: Palette, cx: &mut Context<Self>) -> Div {
        let status = &item.status;
        let id = item.id.clone();
        let take = item.id.clone();
        let stop = item.id.clone();
        v_flex()
            .h(px(144.))
            .px(px(16.))
            .py(px(12.))
            .gap(px(5.))
            .border_t_1()
            .border_color(palette.border)
            .child(
                h_flex()
                    .gap(px(7.))
                    .child(Icon::new(Lucide::Globe).size(px(13.)))
                    .child(div().text_size(px(11.)).child(format!(
                        "{} · {}",
                        status.agent,
                        if status.paused { "Paused" } else { "Nexus browser" }
                    ))),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(palette.muted)
                    .truncate()
                    .child(status.title.clone()),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(palette.muted)
                    .truncate()
                    .child(status.action.clone()),
            )
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(palette.faint)
                    .truncate()
                    .child(format!("{} · Browser tabs only", status.site)),
            )
            .child(
                h_flex()
                    .gap(px(4.))
                    .mt(px(5.))
                    .child(
                        button(
                            format!("activity-pause-{}", item.id),
                            if status.paused { Lucide::Play } else { Lucide::Pause },
                            if status.paused { "Resume" } else { "Pause" },
                            palette,
                        )
                        .on_click(cx.listener(move |_, _, _, cx| control(&id, Control::Pause, cx))),
                    )
                    .child(
                        button(
                            format!("activity-takeover-{}", item.id),
                            Lucide::Hand,
                            "Take over",
                            palette,
                        )
                        .on_click(cx.listener(move |_, _, _, cx| control(&take, Control::TakeOver, cx))),
                    )
                    .child(
                        button(format!("activity-stop-{}", item.id), Lucide::Square, "Stop", palette)
                            .on_click(cx.listener(move |_, _, _, cx| control(&stop, Control::Stop, cx))),
                    ),
            )
    }
}

fn button(id: String, icon: Lucide, label: &'static str, palette: Palette) -> Stateful<Div> {
    ui::focusable(
        h_flex()
            .id(SharedString::from(id))
            .gap(px(5.))
            .h(px(26.))
            .px(px(8.))
            .rounded_full()
            .text_size(px(10.))
            .cursor_pointer()
            .hover(|style| style.bg(palette.selected))
            .role(gpui_kit::accesskit::Role::Button)
            .aria_label(label)
            .child(Icon::new(icon).size(px(12.)))
            .child(label),
        palette,
    )
}

impl Render for Pill {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.zoom != gpui_kit::ui_zoom() {
            self.zoom = gpui_kit::ui_zoom();
            self.resize(_window, cx);
        }
        let palette = cx
            .try_global::<crate::pages::Theme>()
            .map(|theme| theme.0)
            .unwrap_or_else(|| Palette::for_choice(ThemeChoice::System, _window.appearance()));
        let first = self.items.first();
        let agent = first
            .map(|item| item.status.agent.clone())
            .unwrap_or_else(|| "Agent".into());
        let paused = first.is_some_and(|item| item.status.paused);
        let stop = first.map(|item| item.id.clone());
        let count = self.items.len();
        let open = self.reveal.open;
        v_flex()
            .id("browser-activity")
            .track_focus(&self.focus)
            .w_full()
            .h_full()
            .rounded(px(19.))
            .overflow_hidden()
            .bg(palette.menu)
            .border_1()
            .border_color(palette.border)
            .text_color(palette.foreground)
            .font_family("DM Sans")
            .on_hover(cx.listener(|this, hovered: &bool, window, cx| this.hover(*hovered, window, cx)))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.reveal.retract();
                    this._hover = None;
                    this.resize(window, cx);
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .child(
                h_flex()
                    .h(px(placement::HEADER))
                    .flex_none()
                    .px(px(7.))
                    .gap(px(5.))
                    .child(
                        ui::focusable(
                            h_flex()
                                .id("activity-expand")
                                .debug_selector(|| "activity-expand".into())
                                .flex_1()
                                .min_w(px(0.))
                                .h(px(28.))
                                .px(px(7.))
                                .gap(px(7.))
                                .rounded_full()
                                .cursor_pointer()
                                .role(gpui_kit::accesskit::Role::Button)
                                .aria_label(if open {
                                    "Retract browser access details"
                                } else {
                                    "Expand browser access details"
                                })
                                .child(Icon::new(Lucide::Globe).size(px(14.)))
                                .child(div().text_size(px(11.)).truncate().child(agent))
                                .child(div().text_size(px(10.)).text_color(palette.muted).child(if paused {
                                    "· Paused"
                                } else {
                                    "· Browser"
                                }))
                                .when(count > 1, |row| {
                                    row.child(div().text_size(px(10.)).child(format!("+{}", count - 1)))
                                })
                                .child(
                                    Icon::new(if open { Lucide::ChevronUp } else { Lucide::ChevronDown }).size(px(10.)),
                                ),
                            palette,
                        )
                        .on_click(cx.listener(|this, _, window, cx| this.toggle(window, cx))),
                    )
                    .child(
                        ui::focusable(
                            h_flex()
                                .id("activity-quick-stop")
                                .w(px(25.))
                                .h(px(24.))
                                .justify_center()
                                .border_l_1()
                                .border_color(palette.border)
                                .cursor_pointer()
                                .role(gpui_kit::accesskit::Role::Button)
                                .aria_label("Stop this agent and revoke its browser access")
                                .child(Icon::new(Lucide::Square).size(px(11.))),
                            palette,
                        )
                        .on_click(cx.listener(move |_, _, _, cx| {
                            if let Some(id) = &stop {
                                control(id, Control::Stop, cx);
                            }
                        })),
                    ),
            )
            .when(open, |column| {
                column.child(
                    v_flex()
                        .id("activity-sessions")
                        .flex_1()
                        .overflow_y_scroll()
                        .children(self.items.iter().map(|item| self.details(item, palette, cx))),
                )
            })
    }
}
