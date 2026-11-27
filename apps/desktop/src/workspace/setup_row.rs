//! Setup scripts in a Code session's timeline.

use super::{View, Workspace, glyph};
use crate::palette::Palette;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::{event::SetupStatus, setup_timeline::SetupRow};

impl Workspace {
    pub(super) fn render_setup_row(
        &self,
        ix: usize,
        row: &SetupRow,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let View::Code(view) = &self.view else {
            return div().into_any_element();
        };
        let key = format!("setup:{}", row.id);
        let open = view.open_tools.contains(&key);
        let running = row.status == SetupStatus::Running;
        let color = if row.is_error() {
            palette.destructive
        } else {
            palette.muted
        };
        let stop_key = format!("setup:{}", row.id);
        let stopping = view.moving.contains(&stop_key);
        let row_header = h_flex()
            .id(("setup-row", ix))
            .w_full()
            .gap(px(8.))
            .px(px(8.))
            .py(px(6.))
            .rounded(px(8.))
            .cursor_pointer()
            .hover(|style| style.bg(palette.hover))
            .child(glyph(Lucide::SquareTerminal, palette.faint).size(px(15.)))
            .child(div().text_size(px(13.)).child("Setup script"))
            .child(
                div()
                    .flex_1()
                    .truncate()
                    .font_family("monospace")
                    .text_size(px(12.))
                    .text_color(palette.muted)
                    .child(row.command.clone()),
            )
            .children(row.duration_ms.map(|ms| {
                div()
                    .text_size(px(12.))
                    .text_color(palette.faint)
                    .child(format!("{:.1} s", ms as f64 / 1000.))
            }))
            .child(div().text_size(px(12.)).text_color(color).child(row.tag()))
            .child(
                glyph(
                    if open {
                        Lucide::ChevronDown
                    } else {
                        Lucide::ChevronRight
                    },
                    palette.faint,
                )
                .size(px(14.)),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if let View::Code(view) = &mut this.view {
                    if !view.open_tools.remove(&key) {
                        view.open_tools.insert(key.clone());
                    }
                    view.list.remeasure_items(ix..ix + 1);
                }
                cx.notify();
            }));
        let mut column = v_flex().w_full().gap(px(6.)).child(row_header);
        if open {
            let output = if row.output.trim().is_empty() {
                if running {
                    "Running in the new worktree…".into()
                } else {
                    "No output.".into()
                }
            } else {
                format!("{}{}", if row.clipped { "…\n" } else { "" }, row.output)
            };
            column = column.child(
                div()
                    .id(("setup-output", ix))
                    .max_h(px(260.))
                    .overflow_y_scroll()
                    .when_some(view.setup_scrolls.get(&row.id), |output, scroll| {
                        output.track_scroll(scroll)
                    })
                    .px(px(12.))
                    .py(px(8.))
                    .rounded(px(8.))
                    .bg(palette.raised)
                    .font_family("monospace")
                    .text_size(px(12.))
                    .text_color(color)
                    .child(output),
            );
            if running {
                let thread = view.id.clone();
                column =
                    column.child(
                        h_flex()
                            .gap(px(8.))
                            .justify_between()
                            .child(div().flex_1().text_size(px(12.)).text_color(palette.faint).child(
                                "The agent starts when this ends. You can write now; your messages wait for it.",
                            ))
                            .child(
                                div()
                                    .id(("setup-stop", ix))
                                    .px(px(10.))
                                    .py(px(5.))
                                    .rounded(px(7.))
                                    .bg(palette.hover)
                                    .text_size(px(12.))
                                    .child(if stopping { "Stopping…" } else { "Stop" })
                                    .when(!stopping, |button| {
                                        button.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            let Some(runs) = this.runs(&thread) else {
                                                this.refuse_read_only();
                                                cx.notify();
                                                return;
                                            };
                                            if let View::Code(view) = &mut this.view {
                                                view.moving.insert(stop_key.clone());
                                            }
                                            let pending =
                                                runs.call("wb_setup_stop", serde_json::json!({"threadId": thread}));
                                            let stop_key = stop_key.clone();
                                            cx.spawn(async move |this, cx| {
                                                let result = pending
                                                    .await
                                                    .unwrap_or_else(|_| Err("The Stop answer was lost.".into()));
                                                let _ = this.update(cx, |this, cx| {
                                                    if let Err(error) = result {
                                                        if let View::Code(view) = &mut this.view {
                                                            view.moving.remove(&stop_key);
                                                        }
                                                        this.composer_note = Some(error.into());
                                                    }
                                                    cx.notify();
                                                });
                                            })
                                            .detach();
                                            cx.notify();
                                        }))
                                    }),
                            ),
                    );
            }
            if let Some(problem) = row.problem() {
                column = column.child(div().text_size(px(12.)).text_color(palette.faint).child(problem));
            }
        }
        column.into_any_element()
    }
}
