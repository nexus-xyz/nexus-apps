//! The chat column's width in this window: a share of the room beside the sidebar, the side pane and the
//! page panel, as Settings › Appearance chose (`chat_width.rs`). `render` measures it once a frame from the window's
//! size and the choice, both cheap to read; the chat, a Code session, the new session form and the composer draw with
//! it. Settings' row for the choice is here too: the presets, Custom's two sliders and a preview at this window's size.

use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_desktop_core::chat_width::{CUSTOM_MAX_CH, CUSTOM_SHARE, ChatWidth, GUTTERS, Preset, STANDARD_PX, Widths};

use super::{View, Workspace};
use crate::i18n::tr;
use crate::pages::ui;
use crate::palette::Palette;

/// The preview's width; the window is drawn to this scale.
const PREVIEW_WIDTH: f32 = 280.;
const PREVIEW_HEIGHT: f32 = 120.;
/// Custom's sliders: the share in steps of 5%, the most characters in steps of 4.
const SHARE_STEP: f32 = 5.;
const MAX_CH_STEP: f32 = 4.;

/// The widths at the last frame, the choice and the room they came from, and Settings' sliders while Custom shows.
pub(super) struct ChatColumn {
    choice: ChatWidth,
    widths: Widths,
    room: f32,
    window: f32,
    sliders: Option<Sliders>,
    /// Why the last change was not saved.
    problem: Option<SharedString>,
}

struct Sliders {
    share: Entity<SliderState>,
    max_ch: Entity<SliderState>,
    _changed: [Subscription; 2],
}

impl Default for ChatColumn {
    fn default() -> ChatColumn {
        ChatColumn {
            choice: ChatWidth::default(),
            // Until the first frame: Standard's lane.
            widths: Widths {
                column: STANDARD_PX,
                wide: STANDARD_PX,
                composer: STANDARD_PX,
            },
            room: 0.,
            window: 0.,
            sliders: None,
            problem: None,
        }
    }
}

impl Workspace {
    /// The width the main area has: the window's, less the sidebar, the side pane and the page panel when open.
    fn chat_room(&self, window: &Window) -> f32 {
        let mut room = f32::from(window.viewport_size().width);
        if self.panel {
            return room;
        }
        if self.sidebar_open {
            room -= self.sidebar_room();
        }
        if self.side.open && !self.side.maximized {
            room -= self.side_width(window);
        }
        room
    }

    /// Measure this frame's widths. A new choice keeps the lists' width, so they lay out their rows again; a new room
    /// changes their width, and they do so by themselves.
    pub(super) fn sync_chat_column(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let choice = crate::chat_width::choice(cx);
        let room = self.chat_room(window);
        let widths = crate::chat_width::in_room(room, cx);
        let changed = choice != self.chat_column.choice;
        if changed {
            self.chat_list.list.remeasure();
            if let View::Code(view) = &self.view {
                view.list.remeasure();
            }
        }
        if widths != self.chat_column.widths {
            crate::chat_width::remember(window.window_handle().window_id(), widths);
        }
        self.chat_column.choice = choice;
        self.chat_column.widths = widths;
        self.chat_column.room = room;
        self.chat_column.window = f32::from(window.viewport_size().width);
        self.sync_sliders(changed, window, cx);
    }

    /// Custom's sliders exist while Settings › Appearance shows them, and follow a choice made elsewhere.
    fn sync_sliders(&mut self, changed: bool, window: &mut Window, cx: &mut Context<Self>) {
        let shown = matches!(self.view, View::Settings(section)
                if section.dest() == super::settings_registry::Dest::Preferences)
            && self.chat_column.choice.preset == Preset::Custom;
        if !shown {
            self.chat_column.sliders = None;
            return;
        }
        let (share, max_ch) = self.chat_column.choice.custom_values();
        match &self.chat_column.sliders {
            Some(sliders) if changed => {
                sliders
                    .share
                    .update(cx, |state, cx| state.set_value(share * 100., window, cx));
                sliders
                    .max_ch
                    .update(cx, |state, cx| state.set_value(max_ch, window, cx));
            }
            Some(_) => {}
            None => {
                let share = cx.new(|_| {
                    SliderState::new()
                        .min(CUSTOM_SHARE.0 * 100.)
                        .max(CUSTOM_SHARE.1 * 100.)
                        .step(SHARE_STEP)
                        .default_value(share * 100.)
                });
                let max_ch = cx.new(|_| {
                    SliderState::new()
                        .min(CUSTOM_MAX_CH.0)
                        .max(CUSTOM_MAX_CH.1)
                        .step(MAX_CH_STEP)
                        .default_value(max_ch)
                });
                let changed = [
                    cx.subscribe(&share, |this, _, event: &SliderEvent, cx| {
                        this.on_custom_slider(event, cx)
                    }),
                    cx.subscribe(&max_ch, |this, _, event: &SliderEvent, cx| {
                        this.on_custom_slider(event, cx)
                    }),
                ];
                self.chat_column.sliders = Some(Sliders {
                    share,
                    max_ch,
                    _changed: changed,
                });
            }
        }
    }

    /// A slider moves: every window follows it; let go, the choice is saved.
    fn on_custom_slider(&mut self, event: &SliderEvent, cx: &mut Context<Self>) {
        let Some(sliders) = &self.chat_column.sliders else {
            return;
        };
        let next = ChatWidth::custom(
            sliders.share.read(cx).value().start() / 100.,
            sliders.max_ch.read(cx).value().start(),
        );
        match event {
            SliderEvent::Change(_) => crate::chat_width::preview(next, cx),
            SliderEvent::Release(_) => self.save_chat_width(next, cx),
        }
    }

    fn save_chat_width(&mut self, next: ChatWidth, cx: &mut Context<Self>) {
        self.chat_column.problem = crate::chat_width::set(next, cx).err().map(SharedString::from);
        cx.notify();
    }

    /// The conversation's column with its 24px gutters.
    pub(super) fn column_width(&self) -> f32 {
        self.chat_column.widths.column + GUTTERS
    }

    /// The composer's column with its 24px gutters.
    pub(super) fn composer_width(&self) -> f32 {
        self.chat_column.widths.composer + GUTTERS
    }

    /// The widths at the last frame, without the gutters.
    pub(super) fn chat_widths(&self) -> Widths {
        self.chat_column.widths
    }

    /// Settings › Appearance's "Conversation width": the presets, Custom's sliders, and a preview at this window's size.
    pub(super) fn render_chat_width(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let column = &self.chat_column;
        let choice = column.choice;
        let writable = crate::desktop_settings::writable();
        let presets = Preset::ALL.into_iter().map(|preset| {
            let picked = choice.preset == preset;
            ui::chip(
                SharedString::from(format!("chat-width-{preset:?}")),
                preset.label(),
                picked,
                palette,
            )
            .when(writable && !picked, |chip| {
                chip.on_click(cx.listener(move |this, _, _, cx| {
                    let next = match preset {
                        // Custom starts from what it held last, else its defaults.
                        Preset::Custom => {
                            let (share, max_ch) = this.chat_column.choice.custom_values();
                            ChatWidth::custom(share, max_ch)
                        }
                        preset => ChatWidth::preset(preset),
                    };
                    this.save_chat_width(next, cx);
                }))
            })
        });
        let ch = crate::chat_width::ch(cx);
        let summary = tr("Preview at this window's size: column {px} px, about {chars} characters a line.")
            .replace("{px}", &column.widths.column.round().to_string())
            .replace("{chars}", &(column.widths.column / ch).round().to_string());
        v_flex()
            .gap(px(10.))
            .child(
                v_flex()
                    .gap(px(2.))
                    .child(div().text_size(px(13.)).child(tr("Conversation width")))
                    .child(ui::note(
                        tr("How wide the conversation and the composer are. Standard keeps one width, as most chat apps \
                            do. The wider presets grow with the window."),
                        palette,
                    )),
            )
            .child(h_flex().flex_wrap().gap(px(6.)).children(presets))
            .when_some(column.sliders.as_ref(), |row, sliders| {
                let (share, max_ch) = choice.custom_values();
                row.child(slider_row(
                    tr("Share of the room"),
                    format!("{}%", (share * 100.).round()),
                    &sliders.share,
                    palette,
                ))
                .child(slider_row(
                    tr("Maximum characters"),
                    format!("{max_ch}"),
                    &sliders.max_ch,
                    palette,
                ))
            })
            .child(self.render_chat_preview(palette))
            .child(ui::note(summary, palette))
            .when(!writable, |row| {
                row.child(ui::note(crate::desktop_settings::READ_ONLY, palette))
            })
            .when_some(column.problem.clone(), |row, problem| {
                row.child(ui::error_line(problem, palette))
            })
    }

    /// A small drawing of this window: the sidebar, the side pane, and the column and composer in the room between.
    fn render_chat_preview(&self, palette: Palette) -> impl IntoElement {
        let column = &self.chat_column;
        let scale = if column.window > 0. {
            PREVIEW_WIDTH / column.window
        } else {
            0.
        };
        let sidebar = if self.sidebar_open {
            self.sidebar_room() * scale
        } else {
            0.
        };
        let side = (column.window - column.room) * scale - sidebar;
        let line = |width: f32| {
            div()
                .h(px(4.))
                .w(px(width))
                .rounded(px(2.))
                .bg(palette.faint.opacity(0.5))
        };
        let text = column.widths.column * scale;
        h_flex()
            .w(px(PREVIEW_WIDTH))
            .h(px(PREVIEW_HEIGHT))
            .rounded(px(8.))
            .border_1()
            .border_color(palette.border)
            .bg(palette.background)
            .overflow_hidden()
            .when(sidebar > 0., |frame| {
                frame.child(div().flex_none().w(px(sidebar)).h_full().bg(palette.sidebar))
            })
            .child(
                v_flex()
                    .flex_1()
                    .h_full()
                    .items_center()
                    .justify_between()
                    .py(px(12.))
                    .child(
                        v_flex()
                            .w(px(text))
                            .gap(px(5.))
                            .child(line(text))
                            .child(line(text))
                            .child(line(text * 0.7))
                            .child(
                                div()
                                    .h(px(14.))
                                    .w(px(column.widths.wide * scale))
                                    .ml(px(-column.widths.bleed() * scale))
                                    .rounded(px(3.))
                                    .bg(palette.hover),
                            )
                            .child(line(text * 0.85)),
                    )
                    .child(
                        div()
                            .w(px(column.widths.composer * scale))
                            .h(px(14.))
                            .rounded(px(5.))
                            .border_1()
                            .border_color(palette.border)
                            .bg(palette.raised),
                    ),
            )
            .when(side > 0.5, |frame| {
                frame.child(
                    div()
                        .flex_none()
                        .w(px(side))
                        .h_full()
                        .border_l_1()
                        .border_color(palette.border)
                        .bg(palette.sidebar),
                )
            })
    }
}

/// A slider with its name on the left and its value on the right.
fn slider_row(name: SharedString, value: String, state: &Entity<SliderState>, palette: Palette) -> impl IntoElement {
    h_flex()
        .gap(px(12.))
        .child(
            div()
                .w(px(140.))
                .text_size(px(13.))
                .text_color(palette.muted)
                .child(name),
        )
        .child(div().flex_1().max_w(px(260.)).child(Slider::new(state).horizontal()))
        .child(
            div()
                .w(px(44.))
                .text_size(px(13.))
                .text_color(palette.strong)
                .child(value),
        )
}
