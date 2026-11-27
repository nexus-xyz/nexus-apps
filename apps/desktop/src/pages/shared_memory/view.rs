use gpui_kit::component::input::Textarea;
use gpui_kit::component::scroll::{Scrollbar, ScrollbarMode};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_agent_memory::Scope;
use nexus_workbench::event::SessionAgent;

use super::{ForgetConfirmation, MemoryRow, SharedMemory};
use crate::pages::{palette, ui};
use crate::palette::Palette;

const ROW_HEIGHT: f32 = 54.;
const VISIBLE_ROWS: usize = 8;

impl Render for SharedMemory {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        let enabled = self.snapshot.enabled;
        let switch = ui::switch("shared-memory", enabled, self.busy || self.store.is_err(), palette)
            .debug_selector(|| "shared-memory-switch".into())
            .on_click(cx.listener(|this, _, _, cx| {
                if !this.busy && this.store.is_ok() {
                    this.toggle(cx);
                }
            }));
        let setting = ui::setting_row(
            "Shared memory",
            "Share personal and repository notes between local coding-agent sessions.",
            switch,
            palette,
        );

        v_flex()
            .w_full()
            .gap_3()
            .child(ui::group([Some(setting.into_any_element())], palette))
            .when(self.confirming_off, |page| page.child(self.turn_off(palette, cx)))
            .when(enabled, |page| {
                page.child(self.exchange_controls(palette, cx))
                    .child(self.entry_controls(palette, cx))
            })
            .when(self.snapshot.skipped > 0, |page| {
                page.child(ui::footnote(
                    format!("Skipped {} unreadable shared memory file(s).", self.snapshot.skipped),
                    palette,
                ))
            })
            .when_some(self.error.clone(), |page, error| {
                page.child(ui::error_line(error, palette))
            })
            .when_some(self.notice.clone(), |page, notice| {
                page.child(ui::footnote(notice, palette))
            })
    }
}

impl SharedMemory {
    fn turn_off(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        v_flex()
            .gap_2()
            .child(ui::footnote(
                "Export the entries in this view before turning off shared memory.",
                palette,
            ))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        ui::button("memory-export-first", "Export and turn off", palette)
                            .on_click(cx.listener(|this, _, _, cx| this.export_markdown(true, cx))),
                    )
                    .child(
                        ui::button("memory-turn-off", "Turn off", palette)
                            .on_click(cx.listener(|this, _, _, cx| this.set_enabled(false, cx))),
                    )
                    .child(
                        ui::ghost_button("memory-stay-on", "Cancel", palette).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.confirming_off = false;
                                cx.notify();
                            },
                        )),
                    ),
            )
    }

    fn exchange_controls(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let ready = !self.busy;
        let checkout = self.snapshot.checkout.as_deref();
        let scope = checkout
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "Personal entries".into());
        let native = |agent: SessionAgent, importing: bool, cx: &mut Context<Self>| {
            let label = match agent {
                SessionAgent::Claude => "Claude Code",
                _ => "Codex",
            };
            let available = ready && (agent != SessionAgent::Claude || checkout.is_some());
            ui::button(
                SharedString::from(format!("memory-{importing}-{label}")),
                label,
                palette,
            )
            .when(!available, |button| button.opacity(0.5))
            .on_click(cx.listener(move |this, _, _, cx| {
                if available {
                    this.exchange(agent, importing, cx);
                }
            }))
        };
        ui::group(
            [
                Some(
                    ui::setting_row(
                        "Repository",
                        scope,
                        h_flex()
                            .gap_2()
                            .when(checkout.is_some(), |row| {
                                row.child(
                                    ui::ghost_button("memory-personal", "Personal only", palette)
                                        .on_click(cx.listener(|this, _, _, cx| this.select_repository(None, cx))),
                                )
                            })
                            .child(
                                ui::button("memory-repository", "Choose folder", palette).on_click(cx.listener(
                                    |this, _, _, cx| {
                                        if !this.busy {
                                            this.browse(cx);
                                        }
                                    },
                                )),
                            ),
                        palette,
                    )
                    .into_any_element(),
                ),
                Some(
                    ui::setting_row(
                        "Import from",
                        "Claude Code import needs a repository.",
                        h_flex()
                            .gap_2()
                            .child(native(SessionAgent::Claude, true, cx))
                            .child(native(SessionAgent::Codex, true, cx)),
                        palette,
                    )
                    .into_any_element(),
                ),
                Some(
                    ui::setting_row(
                        "Export to",
                        "Codex loads every repository block in every session on its default profile.",
                        h_flex()
                            .gap_2()
                            .child(native(SessionAgent::Claude, false, cx))
                            .child(native(SessionAgent::Codex, false, cx))
                            .child(ui::button("memory-markdown", "Markdown", palette).on_click(cx.listener(
                                |this, _, _, cx| {
                                    if !this.busy {
                                        this.export_markdown(false, cx);
                                    }
                                },
                            ))),
                        palette,
                    )
                    .into_any_element(),
                ),
            ],
            palette,
        )
    }

    fn confirm_forget_all(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        v_flex()
            .gap_2()
            .child(ui::footnote(
                "Delete all personal and repository notes in Nexus Shared memory on this computer?",
                palette,
            ))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        ui::ghost_button("memory-keep-all", "Cancel", palette).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.confirming_forget = None;
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        ui::button("memory-confirm-forget-all", "Forget all entries", palette)
                            .debug_selector(|| "memory-confirm-forget-all".into())
                            .on_click(cx.listener(|this, _, _, cx| this.forget_all(cx))),
                    ),
            )
    }

    fn entry_controls(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let count = self.snapshot.entries.len();
        let head = ui::group_head("Entries", palette)
            .child(
                ui::button("memory-add-personal", "Add personal note", palette)
                    .on_click(cx.listener(|this, _, window, cx| this.open_editor(None, String::new(), window, cx))),
            )
            .child(
                ui::ghost_button("memory-refresh", "Refresh", palette)
                    .on_click(cx.listener(|this, _, _, cx| this.operate(|_, _| Ok(None), cx))),
            )
            .child(
                ui::ghost_button("memory-forget-all", "Forget all", palette)
                    .debug_selector(|| "memory-forget-all".into())
                    .on_click(cx.listener(|this, _, _, cx| {
                        if !this.busy {
                            this.confirming_forget = Some(ForgetConfirmation::All);
                            cx.notify();
                        }
                    })),
            );
        let mut list = v_flex()
            .child(head)
            .when(self.confirming_forget == Some(ForgetConfirmation::All), |list| {
                list.child(self.confirm_forget_all(palette, cx))
            })
            .when(count == 0, |list| {
                list.child(ui::footnote("No shared memory entries here yet.", palette))
            });
        if count > 0 {
            let overflow = count > VISIBLE_ROWS;
            let entries = uniform_list(
                "memory-entries",
                count,
                cx.processor(move |this, rows: std::ops::Range<usize>, _, cx| {
                    rows.filter_map(|index| this.snapshot.entries.get(index))
                        .map(|row| this.entry_row(row, palette, cx))
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.entries_scroll)
            .h(px(ROW_HEIGHT * count.min(VISIBLE_ROWS) as f32))
            .when(overflow, |entries| entries.pr_3());
            list = list.child(div().relative().child(entries).when(overflow, |pane| {
                pane.child(
                    div()
                        .absolute()
                        .inset_0()
                        .debug_selector(|| "memory-entries-scrollbar".into())
                        .child(
                            Scrollbar::vertical(&self.entries_scroll)
                                .id("memory-entries-scrollbar")
                                .mode(ScrollbarMode::Always)
                                .viewport_from_layout(),
                        ),
                )
            }));
        }
        if let Some((_, editor)) = &self.editor {
            list = list.child(
                v_flex().gap_2().child(Textarea::new(editor)).child(
                    h_flex()
                        .gap_2()
                        .child(
                            ui::primary_button("memory-save", "Save", palette)
                                .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                        )
                        .child(
                            ui::ghost_button("memory-edit-cancel", "Cancel", palette).on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.editor = None;
                                    cx.notify();
                                },
                            )),
                        ),
                ),
            );
        }
        list
    }

    fn entry_row(&self, row: &MemoryRow, palette: Palette, cx: &mut Context<Self>) -> Div {
        let id = row.entry.id;
        let scope = if row.entry.scope == Scope::Personal {
            "Personal"
        } else {
            "Repository"
        };
        let confirming = self.confirming_forget == Some(ForgetConfirmation::Entry(id));
        ui::row(palette)
            .h(px(ROW_HEIGHT))
            .w_full()
            .child(ui::two_lines(row.preview.clone(), scope, palette).flex_1())
            .when(!confirming, |row| {
                row.child(
                    ui::ghost_button(SharedString::from(format!("edit-{id}")), "Edit", palette)
                        .on_click(cx.listener(move |this, _, window, cx| this.edit(id, window, cx))),
                )
                .child(
                    ui::ghost_button(SharedString::from(format!("forget-{id}")), "Forget", palette).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.confirming_forget = Some(ForgetConfirmation::Entry(id));
                            cx.notify();
                        }),
                    ),
                )
            })
            .when(confirming, |row| {
                row.child(
                    ui::ghost_button(SharedString::from(format!("cancel-{id}")), "Cancel", palette).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.confirming_forget = None;
                            cx.notify();
                        }),
                    ),
                )
                .child(
                    ui::button(SharedString::from(format!("confirm-{id}")), "Forget entry", palette)
                        .on_click(cx.listener(move |this, _, _, cx| this.forget(id, cx))),
                )
            })
    }
}
