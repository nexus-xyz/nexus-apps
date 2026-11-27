//! Commands controls share the original owner across attached and detached presentations.
use super::Workspace;
use crate::{palette::Palette, side::shared_terminals::SharedTerminals, workspace::commands_detach as detach};
use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;

impl Workspace {
    /// An explicit reopen may adopt a fresh accepted source; ordinary sync never renews a retired owner.
    pub(in crate::workspace) fn reopen_commands(&mut self, session: &str, cx: &mut Context<Self>) {
        if self
            .side
            .shared
            .get(session)
            .is_some_and(|commands| !commands.read(cx).is_current())
        {
            self.side.clear_commands_session(session, false, cx);
            cx.notify();
        }
    }

    pub(in crate::workspace) fn commands_modal_open(&self, cx: &App) -> bool {
        self.side
            .shared_terminal()
            .is_some_and(|commands| commands.read(cx).has_modal())
    }

    pub(super) fn route_commands_action(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        action: impl FnOnce(&mut SharedTerminals, &mut Context<SharedTerminals>) + 'static,
    ) -> bool {
        let Some(key) = self.selected_commands_key() else {
            return false;
        };
        detach::revoke_stale(cx);
        if let Some((owner, target)) = detach::live_target(&key, cx) {
            if target.window_id() == window.window_handle().window_id() {
                return false;
            }
            let expected = target.window_id();
            cx.defer(move |cx| {
                let _ = target.update(cx, |_, window, cx| {
                    if let Some(commands) = detach::target_commands(owner, expected, cx) {
                        commands.update(cx, action);
                        window.activate_window();
                    }
                });
            });
            return true;
        }
        if detach::reserved(&key, window.window_handle().window_id(), cx) {
            self.composer_note = Some("Return Commands to this session before starting another command.".into());
            cx.notify();
            return true;
        }
        false
    }

    /// The Workbench header's "Move to window" button for Commands, when they can move.
    pub(super) fn commands_move_button(&self, palette: Palette, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let move_key = self.selected_commands_key()?;
        let commands = self.side.shared_terminal()?;
        if !commands.read(cx).is_current() {
            return None;
        }
        let expected = commands.entity_id();
        Some(
            super::move_window_button("move-commands-window", palette).on_click(cx.listener(
                move |this, _, window, cx| {
                    if this.commands_modal_open(cx)
                        || this.selected_commands_key().as_ref() != Some(&move_key)
                        || this.side.shared.get(&move_key.session).map(Entity::entity_id) != Some(expected)
                    {
                        return;
                    }
                    if let Err(error) = this.detach_commands(window, cx) {
                        this.composer_note = Some(error.into());
                    }
                    cx.notify();
                },
            )),
        )
    }

    pub(super) fn render_commands_body(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let key = self.selected_commands_key();
        let commands = self.side.shared_terminal().cloned();
        let mut body = v_flex().size_full().min_h(px(0.));
        if let Some(key) = &key {
            if let Some(problem) = detach::notice(key, cx) {
                body = body.child(crate::side::note(problem, palette));
            }
        }
        if let Some(commands) = commands {
            if !commands.read(cx).is_current() {
                return body
                    .child(crate::side::note(
                        "The original Commands source is unavailable. Reopen this session after it reconnects.",
                        palette,
                    ))
                    .into_any_element();
            }
            commands.update(cx, |commands, cx| commands.allow_history(true, cx));
            return body
                .child(
                    div()
                        .id("native-commands-pane")
                        .flex_1()
                        .min_h(px(0.))
                        .overflow_hidden()
                        .px(px(crate::side::INSET))
                        .pt(px(crate::side::INSET))
                        .child(commands),
                )
                .into_any_element();
        }
        if let Some(key) = key {
            if detach::owner(&key, cx).is_some() {
                let focus = key.clone();
                let returning = key;
                body = body
                    .child(crate::side::note(
                        "This session's Commands is in another window or waiting to return.",
                        palette,
                    ))
                    .child(
                        crate::side::chip("show-session-commands", "Show Commands", false, palette).on_click(
                            cx.listener(move |this, _, _, cx| {
                                if this.selected_commands_key().as_ref() == Some(&focus) {
                                    detach::focus(&focus, cx);
                                }
                            }),
                        ),
                    )
                    .child(
                        crate::side::chip("restore-session-commands", "Return Commands", false, palette)
                            .debug_selector(|| "restore-session-commands".into())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if this.selected_commands_key().as_ref() != Some(&returning) {
                                    return;
                                }
                                detach::request_return(&returning, cx);
                                this.show_returned_commands(&returning, window, cx);
                            })),
                    );
            }
        }
        body.into_any_element()
    }
}
