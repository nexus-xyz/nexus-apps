//! Inline command approval, activity and history, using the existing Settings and apps-pane components.
use super::*;

impl SharedTerminals {
    /// Decline, Edit and Run for a request. A closed request in the tray leaves Edit out (`with_edit`): opening it shows
    /// the whole command, which is where an edit belongs.
    pub(super) fn render_approval_actions(
        &self,
        entry: &Entry,
        with_edit: bool,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = entry.id.clone();
        let command = entry.command.clone();
        let mut actions = h_flex().gap(px(8.)).flex_wrap();
        // Agent-owned output requires an explicit handoff; approved commands accept inline input immediately.
        if !self.busy
            && !self.read_only
            && entry.attached()
            && self
                .entries
                .iter()
                .any(|current| current.id == entry.id && current.needs_attention())
        {
            match entry.status.as_str() {
                "awaitingUser" => {
                    let (execute, decline, edit) = (id.clone(), id.clone(), id.clone());
                    let expected = command.clone();
                    let input = self
                        .editing
                        .as_ref()
                        .filter(|(editing, _)| editing == &id)
                        .map(|(_, input)| input.clone());
                    actions = actions
                        .child(
                            ui::ghost_button(SharedString::from(format!("native-decline-{id}")), "Decline", palette)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.perform("decline", json!({"id":decline}), cx)
                                })),
                        )
                        .when(with_edit, |actions| {
                            actions.child(
                                ui::ghost_button(SharedString::from(format!("native-edit-{id}")), "Edit", palette)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        let command = command.clone();
                                        let input =
                                            cx.new(|cx| TextareaState::new(window, cx).rows(4).default_value(command));
                                        input.update(cx, |input, cx| input.focus(window, cx));
                                        this.pending_rows.expand(&edit);
                                        this.collapsed.remove(&edit);
                                        this.editing = Some((edit.clone(), input));
                                        cx.notify();
                                    })),
                            )
                        })
                        .child(
                            ui::primary_button(SharedString::from(format!("native-run-{id}")), "Run", palette)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let command = input
                                        .as_ref()
                                        .map(|input| input.read(cx).value().to_string())
                                        .unwrap_or_else(|| expected.clone());
                                    this.perform(
                                        "execute",
                                        json!({"id":execute,"command":command,"expected_command":expected}),
                                        cx,
                                    );
                                })),
                        );
                }
                _ => {}
            }
        }
        actions.into_any_element()
    }
}
/// The command a card shows, which a person can select and copy. A press on it takes the focus from the command's
/// shell, so Ctrl+C copies and does not interrupt the command.
pub(super) fn command_text(id: &str, command: &str, palette: Palette) -> Stateful<Div> {
    div()
        .id(SharedString::from(format!("command-text-container-{id}")))
        .debug_selector(|| "command-text".into())
        .w_full()
        .min_w(px(0.))
        .overflow_x_scroll()
        .overflow_y_scroll()
        .max_h(px(220.))
        .p(px(12.))
        .rounded(px(8.))
        .bg(palette.background)
        .font_family(ui::MONO)
        .text_size(px(12.))
        .line_height(px(18.))
        .child(crate::workspace::message_text::MessageText::new(
            SharedString::from(format!("command-text-{id}")),
            command.to_owned(),
        ))
}
