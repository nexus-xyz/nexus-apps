//! The prompt for a client that asks for outside access (the Nexus app's `ConsentPrompt`, `outside-access.tsx`), over
//! the window. Nothing runs for the client before the person approves. Deny or a click outside denies; Allow
//! read only grants reading; Allow read and act shows only when the client asked to act. A prompt nobody answers in
//! two minutes denies (`nexus_desktop_mcp::outside`).

use crate::workspace::glass::FloatingGround;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_desktop_mcp::outside::ConsentRequest;
use nexus_desktop_mcp::outside::grants::Scope;

use super::Workspace;
use crate::palette::Palette;

/// The prompt's title and what it says under it.
fn words(request: &ConsentRequest) -> (String, &'static str) {
    let what = match request.scope {
        Scope::Act => "read, start, message and stop your Code sessions",
        Scope::Read => "read your Code sessions",
    };
    (
        format!("Allow “{}” to {what}?", request.client),
        "A program on this computer asks for it. The name is what it says about itself: approve only a request you \
         just started. Sessions it starts follow your New session choices and never go above their permission mode. \
         Revoke it any time in Settings, MCP servers.",
    )
}

impl Workspace {
    /// The first prompt waiting, while this build serves outside access.
    fn outside_request(&self) -> Option<ConsentRequest> {
        self.runner.as_ref()?.outside().pending().into_iter().next()
    }

    fn answer_outside(&mut self, id: String, scope: Option<Scope>, cx: &mut Context<Self>) {
        if let Some(runner) = &self.runner {
            runner.outside().answer(&id, scope);
        }
        cx.notify();
    }

    pub(super) fn render_outside_prompt(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let request = self.outside_request()?;
        let (title, detail) = words(&request);
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .px(px(12.))
                .py(px(6.))
                .rounded(px(8.))
                .text_size(px(13.))
                .cursor_pointer()
                .when(primary, |button| {
                    button.bg(palette.primary).text_color(palette.primary_foreground)
                })
                .when(!primary, |button| {
                    button
                        .border_1()
                        .border_color(palette.border)
                        .text_color(palette.foreground)
                        .hover(|style| style.bg(palette.hover))
                })
                .child(label)
        };
        let id = request.id.clone();
        let deny = {
            let id = id.clone();
            cx.listener(move |this, _, _, cx| this.answer_outside(id.clone(), None, cx))
        };
        let read = {
            let id = id.clone();
            cx.listener(move |this, _, _, cx| this.answer_outside(id.clone(), Some(Scope::Read), cx))
        };
        let act = {
            let id = id.clone();
            cx.listener(move |this, _, _, cx| this.answer_outside(id.clone(), Some(Scope::Act), cx))
        };
        let asks_to_act = request.scope == Scope::Act;
        let dialog = v_flex()
            .id("outside-consent")
            .occlude()
            .w(px(448.))
            .max_w(relative(0.94))
            .p(px(20.))
            .gap(px(10.))
            .rounded(px(14.))
            .floating_ground(14., palette.pop, false, &palette)
            .shadow(palette.pop_shadow(palette.border))
            .text_color(palette.foreground)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(palette.strong)
                    .child(title),
            )
            .child(div().text_size(px(13.)).text_color(palette.muted).child(detail))
            .child(
                h_flex()
                    .pt(px(6.))
                    .gap(px(8.))
                    .justify_end()
                    .child(button("outside-deny", "Deny", false).on_click(deny))
                    .child(button("outside-read", "Allow read only", !asks_to_act).on_click(read))
                    .when(asks_to_act, |row| {
                        row.child(button("outside-act", "Allow read and act", true).on_click(act))
                    }),
            );
        let outside_click = cx.listener(move |this, _, _, cx| this.answer_outside(id.clone(), None, cx));
        Some(
            deferred(
                div()
                    .id("outside-overlay")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .occlude()
                    .bg(palette.scrim)
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_mouse_down(MouseButton::Left, outside_click)
                    .child(dialog),
            )
            .with_priority(3)
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // `gpui_kit::*` brings GPUI's own `test` attribute: the standard one is meant here.
    use core::prelude::v1::test;

    #[test]
    fn the_prompt_says_what_the_client_asked_for() {
        let mut request = ConsentRequest {
            id: "c1".into(),
            client: "nexus CLI".into(),
            scope: Scope::Read,
            at: 0,
        };
        assert_eq!(words(&request).0, "Allow “nexus CLI” to read your Code sessions?");
        request.scope = Scope::Act;
        assert_eq!(
            words(&request).0,
            "Allow “nexus CLI” to read, start, message and stop your Code sessions?"
        );
    }
}
