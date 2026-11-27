//! Each command toggles independently inside one scrolling approval list.
use super::*;

#[derive(Clone, Default)]
pub(super) struct PendingRows {
    open: HashSet<String>,
}
impl PendingRows {
    pub(super) fn is_open(&self, id: &str) -> bool {
        self.open.contains(id)
    }
    pub(super) fn expand(&mut self, id: &str) {
        self.open.insert(id.to_owned());
    }
    fn toggle(&mut self, id: &str) {
        if !self.open.remove(id) {
            self.open.insert(id.to_owned());
        }
    }
    pub(super) fn follow(&mut self, entries: &[Entry]) {
        self.open
            .retain(|id| entries.iter().any(|entry| &entry.id == id && entry.needs_attention()));
    }
}

impl SharedTerminals {
    /// An open request in the tray: its decisions on top, then why the agent asked, the whole command and its folder.
    pub(super) fn render_pending_command(
        &self,
        entry: &Entry,
        limit: f32,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let header = h_flex()
            .w_full()
            .gap(px(8.))
            .flex_wrap()
            .child(self.render_pending_toggle(entry, true, palette, cx))
            .child(
                div()
                    .flex_none()
                    .min_h(px(28.))
                    .child(self.render_approval_actions(entry, true, palette, cx)),
            );
        // The tray shows its error once, above the requests (`inline.rs`), not again in each open card.
        let mut body = v_flex()
            .w_full()
            .min_w(px(0.))
            .gap(px(8.))
            .children(entry.reason.as_ref().map(|reason| ui::note(reason.clone(), palette)));
        if let Some((id, input)) = &self.editing
            && id == &entry.id
        {
            body = body.child(Textarea::new(input));
        } else {
            body = body.child(
                div()
                    .id("pending-command-code")
                    .w_full()
                    .min_w(px(0.))
                    .overflow_x_scroll()
                    .font_family(ui::MONO)
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .child(entry.command.clone()),
            );
        }
        body = body.child(
            div()
                .text_size(px(11.))
                .text_color(palette.muted)
                .child(entry.cwd.clone()),
        );
        let body = if cx.reduce_motion() {
            body.into_any_element()
        } else {
            crate::motion::arrive(body, SharedString::from(format!("pending-open-{}", entry.id)), -4.)
                .into_any_element()
        };
        crate::command_tray::review_card(limit, header.into_any_element(), body, palette)
    }

    /// A request's own line: the mark the Commands tab gives a request, its name, and while closed the command's
    /// first line, so a person sees what would run before they decide.
    fn render_pending_toggle(
        &self,
        entry: &Entry,
        open: bool,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let id = entry.id.clone();
        let (line, more) = super::list::first_line(&entry.command);
        let label = super::list::label(&entry.name, &entry.command);
        ui::focusable(
            h_flex().id(SharedString::from(format!("pending-command-{id}"))),
            palette,
        )
        .flex_1()
        .min_w(px(120.))
        .h(px(28.))
        .gap(px(8.))
        .px(px(6.))
        .rounded(px(6.))
        .cursor_pointer()
        .hover(|style| style.bg(palette.hover))
        .active(|style| style.opacity(0.8))
        .child(
            glyph(
                if open {
                    Lucide::ChevronDown
                } else {
                    Lucide::ChevronRight
                },
                palette.faint,
            )
            .size(px(12.))
            .flex_none(),
        )
        .child(attention_mark(palette))
        .children(label.clone().map(|name| {
            div()
                .flex_none()
                .max_w(relative(0.45))
                .min_w(px(0.))
                .truncate()
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(palette.foreground)
                .child(name)
        }))
        // With no name of its own, the command's first line is the request's title, open or closed.
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .font_family(ui::MONO)
                .text_size(px(12.))
                .text_color(if label.is_some() {
                    palette.faint
                } else {
                    palette.foreground
                })
                .when(!open || label.is_none(), |text| text.child(line.to_string())),
        )
        .when(!open && more > 0, |row| {
            row.child(
                div()
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(palette.faint)
                    .child(format!("+{more}")),
            )
        })
        .when(!open, |row| {
            let command = super::list::tooltip(&entry.command);
            row.tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(command.clone()).build(window, cx))
        })
        .on_click(cx.listener(move |this, _, _, cx| {
            this.pending_rows.toggle(&id);
            cx.notify();
        }))
    }

    /// A closed request: one line on the page's fill, with Decline and Run at its end. Edit opens the request.
    pub(super) fn render_pending_row(&self, entry: &Entry, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let row = h_flex()
            .w_full()
            .min_w(px(0.))
            .min_h(px(36.))
            .gap(px(8.))
            .flex_wrap()
            .px(px(2.))
            .py(px(4.))
            .rounded(px(10.))
            .bg(palette.background)
            .child(self.render_pending_toggle(entry, false, palette, cx))
            .child(
                div()
                    .flex_none()
                    .min_h(px(28.))
                    .child(self.render_approval_actions(entry, false, palette, cx)),
            );
        let fresh = self
            .arrived
            .get(&entry.id)
            .is_some_and(|at| at.elapsed() < Duration::from_millis(400));
        if fresh && !cx.reduce_motion() {
            crate::motion::arrive(row, SharedString::from(format!("pending-arrive-{}", entry.id)), 4.)
                .into_any_element()
        } else {
            row.into_any_element()
        }
    }
}

/// A request's mark: a filled dot in a soft ring, as the tray's summary and the Commands tab draw one.
pub(super) fn attention_mark(palette: Palette) -> Div {
    div()
        .size(px(14.))
        .flex_none()
        .rounded_full()
        .bg(palette.primary.opacity(0.14))
        .flex()
        .items_center()
        .justify_center()
        .child(div().size(px(8.)).rounded_full().bg(palette.primary))
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    fn pending(id: &str) -> Entry {
        serde_json::from_value(
            json!({"id":id,"name":"Command","command":"pwd","cwd":"/repo","status":"awaitingUser","private":false}),
        )
        .unwrap()
    }
    #[test]
    fn requests_start_collapsed_and_toggle_independently_without_opening_new_arrivals() {
        let mut rows = PendingRows::default();
        let mut entries = vec![pending("one"), pending("two")];
        rows.follow(&entries);
        assert!(!rows.is_open("one") && !rows.is_open("two"));
        rows.toggle("one");
        rows.toggle("two");
        assert!(rows.is_open("one") && rows.is_open("two"));
        rows.toggle("one");
        entries.push(pending("three"));
        rows.follow(&entries);
        assert!(!rows.is_open("one") && rows.is_open("two") && !rows.is_open("three"));
        entries[1].status = "running".into();
        rows.follow(&entries);
        assert!(!rows.is_open("two"));
        entries[1].status = "awaitingUser".into();
        rows.follow(&entries);
        assert!(!rows.is_open("two"));
    }
}
