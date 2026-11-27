//! A session's crew in one line under the composer, as the Nexus app's `CrewLine` (`crew-controls.tsx`): the tree is
//! paused or hit the spend cap, the session was stopped, results go with the next message, and Stop for a session that
//! runs nothing while sessions below it still work. The words and when they show are `nexus_workbench::crew_chips`,
//! shared with the Nexus app's `crew-chips.ts`.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::h_flex;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::crew_chips::{Crews, Session, crew_chips, running_text};

use super::Workspace;
use super::code_dock::action_button;
use crate::palette::Palette;

const STOP_TIP: &str = "Stop every session this one started, and the ones they started. Only you can start them again.";

impl Workspace {
    /// Whether session `id`'s machine sends the crew's controls: a server whose core says so, or this computer's runner.
    fn crew_controls_offered(&self, id: &str) -> bool {
        let machine = self.session_by_id(id).and_then(|session| session.machine.clone());
        match machine {
            Some(machine) => crate::machines::get().has_feature(&machine, nexus_workbench::orchestra::CONTROLS_FEATURE),
            None => self.runner.is_some(),
        }
    }

    /// The line for session `id`; None when there is nothing to say, or its machine's core is older.
    pub(super) fn render_crew_line(&self, id: &str, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let session = self.session_by_id(id)?;
        let sessions = self
            .projects
            .iter()
            .flat_map(|project| &project.sessions)
            .chain(&self.archived_code);
        let crews = Crews::new(sessions.map(|session| Session {
            id: &session.id,
            parent: session.parent.as_deref(),
            title: &session.title,
            facts: &session.crew,
        }));
        let chips = crew_chips(&session.crew, &crews.place(&session.id));
        let running = crews.running(&session.id);
        let stop = running > 0 && session.crew.lead_idle();
        if (chips.is_empty() && !stop) || !self.crew_controls_offered(id) {
            return None;
        }
        let chip = |text: String| {
            div()
                .flex_none()
                .px(px(8.))
                .py(px(2.))
                .rounded(px(6.))
                .bg(palette.selected)
                .text_color(palette.foreground)
                .child(text)
        };
        let thread = session.id.clone();
        let stop_button = stop.then(|| {
            action_button("crew-stop", Some(Lucide::Square), "Stop", false, palette)
                .tooltip(|window, cx| Tooltip::new(STOP_TIP).build(window, cx))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(runs) = this.runs(&thread) {
                        runs.stop(thread.clone());
                    }
                    cx.notify();
                }))
        });
        Some(
            h_flex()
                .id("crew-line")
                .w_full()
                .gap(px(8.))
                .justify_between()
                .px(px(4.))
                .text_size(px(12.5))
                .text_color(palette.muted)
                .child(
                    h_flex()
                        .flex_1()
                        .min_w(px(0.))
                        .flex_wrap()
                        .gap(px(6.))
                        .children(chips.into_iter().map(|one| chip(one.text)))
                        .when(stop, |line| line.child(div().child(running_text(running)))),
                )
                .children(stop_button)
                .into_any_element(),
        )
    }
}
