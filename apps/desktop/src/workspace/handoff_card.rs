//! The card a session's timeline shows where its agent handed its work off (`handoff_session`): "Handed
//! off to Codex", the new session's title and who pays, and Open. The session was archived when that turn ended. The
//! rule that reads the tool's answer is [`crate::code::HandedOff::parse`]; [`Workspace::render_handoff`] only draws it.

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::Workspace;
use crate::code::HandedOff;
use crate::palette::Palette;

impl Workspace {
    /// The handoff card for call `ix`.
    pub(super) fn render_handoff(
        &self,
        ix: usize,
        handed: &HandedOff,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let agent = super::code_dock::agent_name(handed.agent);
        // Who pays, from the new session's own row when the app has it.
        let payer = self.session_by_id(&handed.session_id).map(|session| match session.via {
            nexus_workbench::event::Via::Subscription => format!("{agent} on your plan"),
            nexus_workbench::event::Via::Nexus => format!("{agent} through Nexus"),
        });
        let open = handed.session_id.clone();
        v_flex()
            .id(("handoff", ix))
            .my(px(8.))
            .w_full()
            .p(px(12.))
            .gap(px(4.))
            .rounded(px(12.))
            .border_1()
            .border_color(palette.border)
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::MEDIUM)
                            .child(format!("Handed off to {agent}")),
                    )
                    .child(
                        div()
                            .id(("handoff-open", ix))
                            .px(px(10.))
                            .py(px(2.))
                            .rounded(px(6.))
                            .bg(palette.foreground)
                            .text_color(palette.background)
                            .cursor_pointer()
                            .child("Open")
                            .on_click(cx.listener(move |this, _, window, cx| this.open_code(&open, window, cx))),
                    ),
            )
            .child(div().text_size(px(13.)).text_color(palette.muted).child(match payer {
                Some(payer) => format!("{} · {payer} · takes over this worktree", handed.title),
                None => format!("{} · takes over this worktree", handed.title),
            }))
            .into_any_element()
    }
}
