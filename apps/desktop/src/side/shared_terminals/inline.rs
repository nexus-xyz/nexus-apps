//! The existing command cards in one bounded, collapsible group above the composer, and under it the cards of the
//! accepted commands that just ended (`finished.rs`).
use super::*;

impl SharedTerminals {
    pub(super) fn render_inline(&mut self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let attention = self.entries.iter().any(Entry::needs_attention);
        let opacity = self.inline_motion.sample(attention, cx);
        if attention {
            self.closing_rows = self.pending_rows.clone();
            self.closing_entries = self
                .entries
                .iter()
                .filter(|entry| entry.needs_attention())
                .cloned()
                .collect();
        }
        let pending = if attention {
            self.entries.iter().filter(|entry| entry.needs_attention()).count()
        } else {
            self.closing_entries.len()
        };
        let compact = self.inline_limit < crate::command_tray::REVIEW_MIN;
        let shown = self.inline_open && !compact;
        let label = format!(
            "{} need{} permission",
            super::list::commands(pending),
            if pending == 1 { "s" } else { "" }
        );
        let body_opacity = self.group_motion.sample(shown, cx);
        let summary = ui::focusable(h_flex().id("command-group-toggle"), palette)
            .track_focus(&self.inline_focus)
            .flex_1()
            .min_w(px(0.))
            .h(px(22.))
            .gap(px(8.))
            .cursor_pointer()
            .rounded(px(6.))
            .child(glyph(
                if shown {
                    Lucide::ChevronDown
                } else {
                    Lucide::ChevronRight
                },
                palette.icon,
            ))
            .when(pending > 0, |head| head.child(super::approval::attention_mark(palette)))
            .child(
                div()
                    .min_w(px(0.))
                    .truncate()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(label),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                if compact {
                    this.review_inline(cx);
                    cx.emit(SharedTerminalsEvent::OpenCommands);
                } else {
                    this.inline_open = !this.inline_open;
                }
                this.inline_focus.focus(window, cx);
                cx.notify();
            }));
        let commands = ui::ghost_button("command-group-pane", "Commands", palette)
            .on_click(cx.listener(|_, _, _, cx| cx.emit(SharedTerminalsEvent::OpenCommands)));
        let header = crate::command_tray::decision_header(
            summary.into_any_element(),
            self.render_bulk_actions(palette, cx),
            commands.into_any_element(),
        );
        let header_height = crate::command_tray::HEADER;
        let mut content = None;
        if shown || self.group_motion.visible() {
            let source = if attention {
                &self.entries
            } else {
                &self.closing_entries
            };
            let entries: Vec<_> = newest_first(source)
                .into_iter()
                .filter(|entry| entry.needs_attention())
                .collect();
            let rows = if attention {
                &self.pending_rows
            } else {
                &self.closing_rows
            };
            let available = (self.inline_limit - header_height).max(0.);
            let mut cards: Vec<AnyElement> = entries
                .into_iter()
                .map(|entry| {
                    let card = if rows.is_open(&entry.id) {
                        self.render_pending_command(entry, available, palette, cx)
                    } else {
                        self.render_pending_row(entry, palette, cx)
                    };
                    div()
                        .id(SharedString::from(format!("pending-command-container-{}", entry.id)))
                        .w_full()
                        .flex_none()
                        .child(card)
                        .into_any_element()
                })
                .collect();
            if let Some(error) = &self.error {
                cards.insert(0, ui::error_line(error.clone(), palette).into_any_element());
            }
            let body = crate::command_tray::request_list(available, &self.inline_scroll, cards).opacity(body_opacity);
            content = Some(body.into_any_element());
        }
        let tray = crate::command_tray::frame(
            "command-tray",
            self.inline_limit,
            header.into_any_element(),
            content,
            None,
            palette,
        );
        // The requests, while any wait or their tray fades out; under them, the accepted commands that just ended.
        let requests = (attention || self.inline_motion.visible()).then(|| {
            div()
                .id("command-tray-presence")
                .w_full()
                .opacity(opacity)
                .relative()
                .top(px(6. * (1. - opacity)))
                .child(tray)
        });
        let finished = self.render_finished(palette, cx);
        v_flex()
            .w_full()
            .gap(px(8.))
            .children(requests)
            .children(finished)
            .into_any_element()
    }
}
