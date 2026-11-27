//! A coding agent that is not installed or not signed in, in the transcript (board P3): a setup line with the command
//! and Copy under it, in place of the red error. Install and sign-in run in the Workbench terminal; when they end
//! well, the blocked start runs again with the same message, or the message goes back into the composer.

use std::rc::Rc;

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::event::SessionAgent;

use super::{View, Workspace};
use crate::agents::setup::{self as agent_setup, Gap};
use crate::code::Block;
use crate::palette::Palette;
use crate::setup::{self, Fixer, SetupLine};

/// What goes on when a fix is done.
pub(super) type Resume = Rc<dyn Fn(&mut Window, &mut App)>;

impl Workspace {
    /// This window's fixer, which also holds `resume` until a terminal step or a sign-in it starts is done.
    pub(super) fn resuming_fixer(&self, resume: Resume, cx: &mut Context<Self>) -> Fixer {
        self.fixer(cx).before(move |key, fix, _, cx| {
            if !agent_setup::resumes(fix) {
                return;
            }
            // One resume per press: a second press after a failure replaces the first.
            setup::forget(key, cx);
            // `done` runs inside the workspace's update when a Workbench shell ends: resume after it, since the resume
            // updates the workspace too.
            let resume = resume.clone();
            setup::on_done(key.clone(), cx, move |window, cx| {
                window.defer(cx, move |window, cx| resume(window, cx))
            });
        })
    }

    /// A session that could not start because its agent is missing or signed out: the setup line, and when its fix
    /// is done, the start again with the same message.
    pub(super) fn render_start_setup(
        &self,
        draft: &str,
        agent: SessionAgent,
        gap: Gap,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let this = cx.entity().downgrade();
        let id = draft.to_string();
        let resume: Resume = Rc::new(move |window, cx| {
            let retried = this.update(cx, |workspace, cx| {
                let failed = matches!(&workspace.view, View::Code(view)
                    if view.id == id && view.pending.as_ref().is_some_and(|pending| pending.failed.is_some()));
                if failed {
                    workspace.retry_pending(window, cx);
                }
            });
            if let Err(error) = retried {
                eprintln!("nexus-desktop-gpui: retry the start: {error}");
            }
        });
        let fixer = self.resuming_fixer(resume, cx);
        // The session has not started, so the line's fix is the view's one white button.
        let (line, command) = agent_line(format!("start:{draft}:agent"), agent, gap);
        agent_card(line.main(), command, &fixer, palette, cx)
    }

    /// An agent's error in a session's transcript that says it is missing or signed out: the setup line. When its fix
    /// is done, the person's last message goes back into an empty composer, to send again.
    pub(super) fn render_agent_error(&self, gap: Gap, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some(session) = self.code_session() else {
            return div().into_any_element();
        };
        let agent = session.agent;
        let id = match &self.view {
            View::Code(view) => view.id.clone(),
            _ => String::new(),
        };
        let this = cx.entity().downgrade();
        let session_id = id.clone();
        let resume: Resume = Rc::new(move |window, cx| {
            let restored = this.update(cx, |workspace, cx| {
                workspace.restore_last_message(&session_id, window, cx)
            });
            if let Err(error) = restored {
                eprintln!("nexus-desktop-gpui: restore the message: {error}");
            }
        });
        let fixer = self.resuming_fixer(resume, cx);
        // Keyed by the session, not the block's place: the place moves when an older page loads.
        let (line, command) = agent_line(format!("session:{id}:agent"), agent, gap);
        agent_card(line, command, &fixer, palette, cx)
    }

    /// The person's last message in the open session, back in the composer when it is empty.
    fn restore_last_message(&mut self, session: &str, window: &mut Window, cx: &mut Context<Self>) {
        let View::Code(view) = &self.view else {
            return;
        };
        if view.id != session || !self.composer_empty {
            return;
        }
        let Some(text) = last_message(&view.blocks) else {
            return;
        };
        self.composer
            .update(cx, |state, cx| state.set_value(text.clone(), window, cx));
        self.composer_empty = text.trim().is_empty();
        self.focus_composer(window, cx);
        cx.notify();
    }
}

/// The last thing the person wrote in a transcript.
fn last_message(blocks: &[Block]) -> Option<String> {
    blocks.iter().rev().find_map(|block| match block {
        Block::User {
            text, rewound: false, ..
        } if !text.trim().is_empty() => Some(text.clone()),
        _ => None,
    })
}

/// The line for `gap`, and the command it shows under it.
fn agent_line(key: String, agent: SessionAgent, gap: Gap) -> (SetupLine, Option<&'static str>) {
    match gap {
        Gap::Missing { here } => {
            // On a server, the Workbench here can't install it there: the command, to copy.
            let command = agent_setup::install_command(agent);
            let place = if here { "this computer" } else { "this server" };
            let install = command.filter(|_| here);
            (agent_setup::missing_line(key, agent, place, install, here), command)
        }
        Gap::SignedOut => {
            let name = nexus_workbench::driver::label(agent);
            let command = (!agent_setup::signs_in_here(agent))
                .then(|| agent_setup::login_command(agent))
                .flatten();
            (
                agent_setup::signed_out_line(key, agent, format!("{name} isn't signed in."), None, true),
                command,
            )
        }
    }
}

/// The line, then its command in monospace with Copy. The command hides while the terminal runs it.
pub(super) fn agent_card(
    line: SetupLine,
    command: Option<&str>,
    fixer: &Fixer,
    palette: Palette,
    cx: &App,
) -> AnyElement {
    let running = setup::state::run_state(line.key(), cx) == Some(setup::Run::Running);
    v_flex()
        .w_full()
        .gap(px(6.))
        .child(line.render(fixer, palette, cx))
        .when_some(command.filter(|_| !running), |card, command| {
            card.child(command_row(command, palette))
        })
        .into_any_element()
}

/// A command in monospace with Copy.
pub(super) fn command_row(command: &str, palette: Palette) -> impl IntoElement {
    h_flex()
        .w_full()
        .gap(px(8.))
        .px(px(12.))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .font_family("Geist Mono")
                .text_size(px(12.))
                .text_color(palette.muted)
                .child(command.to_string()),
        )
        .child(super::code_block::copy_button(command.to_string().into(), palette))
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    fn user(text: &str, rewound: bool) -> Block {
        Block::User {
            seq: 1,
            at: 0,
            text: text.into(),
            from: None,
            rewound,
            images: 0,
            image_data: Vec::new(),
        }
    }

    #[test]
    fn the_last_message_skips_rewound_and_empty_ones() {
        let blocks = vec![
            user("Run the decoder benchmark", false),
            Block::Error("Antigravity is not signed in. Run `agy` to sign in.".into(), None),
            user("taken back", true),
            user("  ", false),
        ];
        assert_eq!(last_message(&blocks).as_deref(), Some("Run the decoder benchmark"));
        assert_eq!(last_message(&[]), None);
    }

    #[test]
    fn a_missing_agent_shows_its_install_command_and_a_signed_out_one_its_login() {
        let (line, command) = agent_line("k".into(), SessionAgent::Codex, Gap::Missing { here: true });
        assert_eq!(line.key().as_ref(), "k");
        assert_eq!(command, agent_setup::install_command(SessionAgent::Codex));
        let (_, command) = agent_line("k".into(), SessionAgent::Antigravity, Gap::SignedOut);
        assert_eq!(command, Some("agy"));
        // Codex signs in from Settings, in the browser: no command to copy.
        let (_, command) = agent_line("k".into(), SessionAgent::Codex, Gap::SignedOut);
        assert_eq!(command, None);
    }
}
