//! A linked session's message in a Code session's transcript, as the Nexus app's `session-message.tsx` draws it. It
//! never looks like a message the person wrote: "Received message from <its session>", its text as plain text, and
//! Copy. A batch of its children's results shows one card per child instead, with the child's title, agent and how it
//! ended. The batch rule is `nexus_workbench::session_results`; this file only draws it.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::event::Sender;
use nexus_workbench::session_results::{ReadResult, own_results, session_results};

use super::{View, Workspace, glyph};
use crate::i18n::tr;
use crate::palette::Palette;

impl Workspace {
    /// The message `text` (event `seq`) that session `from` wrote into the session on screen.
    pub(super) fn render_session_message(
        &self,
        seq: i64,
        text: &str,
        from: &Sender,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let View::Code(view) = &self.view else {
            return div().into_any_element();
        };
        let Sender::Session { thread_id, title } = from;
        let results = own_results(session_results(text), thread_id, Some(view.id.as_str()), |id| {
            self.session_by_id(id).map(|session| session.parent.as_deref())
        });
        let head = match &results {
            Some(_) => from_line(palette).child(tr("Received results from subsessions")),
            None => from_line(palette)
                .child(tr("Received message from "))
                .child(session_link(
                    ("session-from", seq as u64),
                    thread_id,
                    title,
                    palette,
                    cx,
                )),
        };
        let body: AnyElement = match results {
            Some(results) => v_flex()
                .gap(px(12.))
                .children(
                    results
                        .iter()
                        .enumerate()
                        .map(|(index, result)| result_card(seq, index, result, palette, cx)),
                )
                .into_any_element(),
            None => message_body(text.to_string()).into_any_element(),
        };
        let group: SharedString = format!("session-message-{seq}").into();
        let copied = view.copied == Some(seq);
        let copy = {
            let text = text.to_string();
            div()
                .id(("session-message-copy", seq as u64))
                .size(px(24.))
                .rounded(px(6.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|style| style.bg(palette.hover))
                .active(|style| style.opacity(0.7))
                .tooltip(move |window, cx| {
                    Tooltip::new(if copied { tr("Copied") } else { tr("Copy agent message") }).build(window, cx)
                })
                .child(glyph(if copied { Lucide::Check } else { Lucide::Copy }, palette.faint))
                .on_click(cx.listener(move |this, _, _, cx| this.copy_message(seq, text.clone(), cx)))
        };
        v_flex()
            .group(group.clone())
            .w_full()
            .py(px(8.))
            .child(head)
            .child(body)
            .child(
                h_flex()
                    .pt(px(4.))
                    .when(!copied, |row| {
                        row.opacity(0.).group_hover(group.clone(), |style| style.opacity(1.))
                    })
                    .child(copy),
            )
            .into_any_element()
    }
}

/// One child's report: its title (opening it), agent and how it ended, then its text.
fn result_card(
    seq: i64,
    index: usize,
    result: &ReadResult,
    palette: Palette,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let id = SharedString::from(format!("session-result-{seq}-{index}"));
    let meta = format!(
        " · {} · {}",
        super::code_dock::agent_name(result.agent),
        result.status_label()
    );
    v_flex()
        .child(
            from_line(palette)
                .child(session_link(id, &result.session_id, &result.title, palette, cx))
                .child(meta),
        )
        .child(message_body(result.text.clone()))
        .into_any_element()
}

/// The small muted line over a message or a report (`.wb-session-message-from`).
fn from_line(palette: Palette) -> Div {
    h_flex().flex_wrap().mb(px(8.)).text_xs().text_color(palette.muted)
}

/// A session's title, underlined, opening that session.
fn session_link(
    id: impl Into<ElementId>,
    thread: &str,
    title: &str,
    palette: Palette,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let thread = thread.to_string();
    div()
        .id(id)
        .text_color(palette.foreground)
        .underline()
        .cursor_pointer()
        .child(title.to_string())
        .on_click(cx.listener(move |this, _, window, cx| this.open_code(&thread, window, cx)))
}

/// The text as written, never as markup (`.wb-session-message-body`).
fn message_body(text: String) -> impl IntoElement {
    div().w_full().text_size(px(14.)).line_height(px(22.)).child(text)
}

#[cfg(test)]
#[path = "session_message_tests.rs"]
mod tests;
