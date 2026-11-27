//! The rows over the Code session's composer after an Interrupt (`composer.tsx`'s `ContinueBar` and
//! `StillRunningBar`): Continue, the one-click way back into the interrupted turn, and what still runs with Stop all.
//! When each shows and what it says are `nexus_workbench::interrupt`'s, shared with the Nexus app.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::h_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::interrupt;

use super::Workspace;
use super::code_dock::action_button;
use super::code_view::CodeView;
use crate::palette::Palette;

/// The look of a row over the composer (`.wb-continue`): its text on the left, its buttons on the right.
pub(super) fn bar_row(id: &'static str, palette: Palette) -> Stateful<Div> {
    h_flex()
        .id(id)
        .w_full()
        .gap(px(12.))
        .justify_between()
        .py(px(8.))
        .pl(px(16.))
        .pr(px(8.))
        .rounded(px(10.))
        .border_1()
        .border_color(palette.border)
        .bg(palette.raised)
        .text_size(px(13.))
        .text_color(palette.muted)
}

/// The Continue row shows on a session whose last turn was interrupted (`status`) since the person's last message
/// (`since_message`), while nothing runs, until Continue is clicked (`clicked`). The plan limit's row takes its place
/// when the plan stopped the turn (`limited`).
pub(super) fn continue_shown(status: bool, since_message: bool, running: bool, limited: bool, clicked: bool) -> bool {
    status && since_message && !running && !limited && !clicked
}

impl Workspace {
    /// "The agent stopped before it finished this turn." with Continue, which sends the same words as the Nexus app.
    pub(super) fn render_continue_bar(
        &self,
        view: &CodeView,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let session = self.code_session()?;
        let clicked = view.continued_at == Some(view.last_seq);
        if !continue_shown(
            session.interrupted,
            view.interrupted,
            self.code_running(),
            session.limited.is_some(),
            clicked,
        ) {
            return None;
        }
        let id = view.id.clone();
        Some(
            bar_row("continue-bar", palette)
                .child(div().flex_1().min_w(px(0.)).child(interrupt::CONTINUE_TEXT))
                .child(
                    action_button("continue", Some(Lucide::Play), "Continue", true, palette).on_click(cx.listener(
                        move |this, _, _, cx| {
                            // As the Nexus app's Continue: the composer's draft stays where it is.
                            if let Some(runs) = this.runs(&id) {
                                runs.send(
                                    id.clone(),
                                    nexus_workbench::CONTINUE.to_string(),
                                    Default::default(),
                                    false,
                                );
                            }
                            if let super::View::Code(view) = &mut this.view {
                                view.continued_at = Some(view.last_seq);
                            }
                            cx.notify();
                        },
                    )),
                )
                .into_any_element(),
        )
    }

    /// After an Interrupt that left sub-agents or commands running: what still runs, and Stop all, which ends the agent
    /// and all it runs. It shows over a question or an approval too: a sub-agent's question outlives the Interrupt.
    pub(super) fn render_still_running(
        &self,
        view: &CodeView,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let session = self.code_session()?;
        let text = interrupt::still_running(view.interrupted, session.live, &view.crew)?;
        let id = view.id.clone();
        Some(
            bar_row("still-running", palette)
                .child(div().flex_1().min_w(px(0.)).child(text))
                .child(
                    action_button("stop-all", Some(Lucide::CircleStop), "Stop all", false, palette)
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(interrupt::STOP_ALL_HINT).build(window, cx)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(runs) = this.runs(&id) {
                                runs.stop(id.clone());
                            }
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::continue_shown;

    #[test]
    fn continue_shows_on_an_interrupted_session_at_rest_until_clicked() {
        assert!(continue_shown(true, true, false, false, false));
        // Clicked: hidden until the transcript moves on.
        assert!(!continue_shown(true, true, false, false, true));
        // A turn runs, or the plan's limit row shows instead.
        assert!(!continue_shown(true, true, true, false, false));
        assert!(!continue_shown(true, true, false, true, false));
        // The person wrote since, or the turn was not interrupted.
        assert!(!continue_shown(true, false, false, false, false));
        assert!(!continue_shown(false, true, false, false, false));
    }
}
