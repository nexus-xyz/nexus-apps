//! Confirm interrupted work before an update, then offer scoped batch continuation after launch.

use crate::workspace::glass::FloatingGround;
use std::collections::HashSet;

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::update_restart::Session;

use super::Workspace;
use crate::pages::ui;
use crate::palette::Palette;
use crate::update_recovery::{Entry, Recovery};

#[derive(Default)]
pub(super) struct UpdateRestart {
    confirmation: Option<Vec<Session>>,
    review: bool,
    selected: HashSet<String>,
    error: Option<String>,
    controls: Vec<FocusHandle>,
    review_ids: Vec<String>,
}

impl Workspace {
    pub(super) fn request_update_restart(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let sessions = crate::windows::update_restart_sessions(cx)?;
        if sessions.is_empty() {
            return crate::windows::install_update(&[], cx);
        }
        self.update_restart.confirmation = Some(sessions);
        self.update_restart.review = false;
        self.update_restart.error = None;
        self.update_restart.controls = (0..2).map(|_| cx.focus_handle()).collect();
        cx.notify();
        Ok(())
    }

    fn confirm_update_restart(&mut self, cx: &mut Context<Self>) {
        let Some(sessions) = self.update_restart.confirmation.clone() else {
            return;
        };
        match crate::windows::install_update(&sessions, cx) {
            Ok(()) => self.update_restart.confirmation = None,
            Err(error) => {
                // A newly active session is never covered by an older confirmation.
                if let Ok(sessions) = crate::windows::update_restart_sessions(cx) {
                    self.update_restart.confirmation = Some(sessions);
                }
                self.update_restart.error = Some(error);
            }
        }
        cx.notify();
    }

    fn recovery_entries(&self, cx: &App) -> Vec<Entry> {
        let (Some(scope), Some(identity), Some(recovery)) = (
            self.code_scope.as_deref(),
            self.gateway.marker(),
            cx.try_global::<Recovery>(),
        ) else {
            return Vec::new();
        };
        if self.account.is_none() || !crate::managed::local_cache(crate::managed::policy()) {
            return Vec::new();
        }
        recovery.pending(scope, &identity)
    }

    pub(super) fn has_update_recovery(&self, cx: &App) -> bool {
        self.update_restart.review
            || (cx
                .try_global::<Recovery>()
                .is_some_and(|recovery| !recovery.hidden && !recovery.installing)
                && !self.recovery_entries(cx).is_empty())
    }

    fn saved_agent_title(&self, id: &str) -> String {
        self.projects
            .iter()
            .filter(|project| project.machine.is_none())
            .flat_map(|project| &project.sessions)
            .find(|session| session.id == id)
            .map(|session| session.title.clone())
            .unwrap_or_else(|| "Saved agent".into())
    }

    fn review_update_agents(&mut self, cx: &mut Context<Self>) {
        let entries = self.recovery_entries(cx);
        self.update_restart.selected = entries
            .iter()
            .filter(|entry| !entry.continuing)
            .map(|entry| entry.session.id.clone())
            .collect();
        self.update_restart.controls = (0..entries.len() + 2).map(|_| cx.focus_handle()).collect();
        self.update_restart.review_ids = entries.iter().map(|entry| entry.session.id.clone()).collect();
        self.update_restart.review = true;
        self.update_restart.error = None;
        cx.notify();
    }

    fn continue_update_agents(&mut self, selected: Vec<String>, cx: &mut Context<Self>) {
        let (Some(scope), Some(identity), Some(runner)) =
            (self.code_scope.clone(), self.gateway.marker(), self.runner.clone())
        else {
            return;
        };
        let entries = match cx.global_mut::<Recovery>().claim(&scope, &identity, &selected) {
            Ok(entries) => entries,
            Err(error) => {
                self.update_restart.error = Some(error);
                cx.notify();
                return;
            }
        };
        // One shared claim covers every window. Each success leaves the journal before it can be offered again.
        for entry in entries {
            let answer = runner.continue_after_update(entry.clone());
            cx.spawn(async move |_, cx| {
                let result = answer
                    .await
                    .unwrap_or_else(|_| Err("The agent did not answer. Try again.".into()));
                cx.update(|cx| {
                    cx.global_mut::<Recovery>().finish(&entry, result);
                    crate::windows::recovery_changed(cx);
                });
            })
            .detach();
        }
        crate::windows::recovery_changed(cx);
        cx.notify();
    }

    fn close_update_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.update_restart.confirmation = None;
        self.update_restart.review = false;
        self.update_restart.error = None;
        self.focus_composer(window, cx);
        cx.notify();
    }

    /// Keep dialog keys from reaching agents or workspace shortcuts. Enter defaults to Cancel, never stopping work.
    pub(super) fn update_restart_key(&mut self, key: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.update_restart.confirmation.is_none() && !self.update_restart.review {
            return false;
        }
        if key.key == "escape" {
            self.close_update_dialog(window, cx);
        } else if key.key == "tab" {
            let controls = &self.update_restart.controls;
            if !controls.is_empty() {
                let next = controls
                    .iter()
                    .position(|focus| focus.is_focused(window))
                    .map_or(0, |at| {
                        if key.modifiers.shift {
                            (at + controls.len() - 1) % controls.len()
                        } else {
                            (at + 1) % controls.len()
                        }
                    });
                controls[next].focus(window, cx);
            }
        } else if key.key == "enter" || key.key == "space" {
            let focused = self
                .update_restart
                .controls
                .iter()
                .position(|focus| focus.is_focused(window));
            if self.update_restart.confirmation.is_some() {
                if focused == Some(1) {
                    self.confirm_update_restart(cx);
                } else {
                    self.close_update_dialog(window, cx);
                }
            } else {
                match focused {
                    Some(at) if at < self.update_restart.review_ids.len() => {
                        let id = self.update_restart.review_ids[at].clone();
                        if !self.update_restart.selected.remove(&id) {
                            self.update_restart.selected.insert(id);
                        }
                        cx.notify();
                    }
                    Some(at) if at == self.update_restart.review_ids.len() + 1 => {
                        self.continue_update_agents(self.update_restart.selected.iter().cloned().collect(), cx);
                    }
                    _ => self.close_update_dialog(window, cx),
                }
            }
        }
        cx.stop_propagation();
        true
    }

    /// A small, in-app prompt after a restart. Later keeps the saved agents and hides the prompt for this launch.
    pub(super) fn render_update_recovery(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let recovery = cx.try_global::<Recovery>()?;
        let entries = self.recovery_entries(cx);
        if self.panel || recovery.hidden || recovery.installing || entries.is_empty() || self.update_restart.review {
            return None;
        }
        let running = entries.iter().filter(|entry| entry.continuing).count();
        let ids: Vec<_> = entries
            .iter()
            .filter(|entry| !entry.continuing)
            .map(|entry| entry.session.id.clone())
            .collect();
        let errors = entries.iter().any(|entry| entry.error.is_some());
        let title = if running > 0 {
            format!(
                "Continuing {running} {}…",
                if running == 1 { "agent" } else { "agents" }
            )
        } else {
            format!(
                "{} {} stopped during the update",
                entries.len(),
                if entries.len() == 1 { "agent" } else { "agents" }
            )
        };
        let can_continue = !ids.is_empty() && self.runner.is_some();
        let action = if can_continue {
            ui::primary_button(
                "update-recovery-continue",
                if errors {
                    "Retry".into()
                } else {
                    format!("Continue all {}", ids.len())
                },
                palette,
            )
            .on_click(cx.listener(move |this, _, _, cx| this.continue_update_agents(ids.clone(), cx)))
            .into_any_element()
        } else {
            ui::disabled_button(
                "update-recovery-continue",
                if running > 0 { "Continuing…" } else { "Continue" },
                palette,
            )
            .into_any_element()
        };
        Some(
            deferred(
                div().absolute().bottom(px(20.)).right(px(20.)).child(
                    v_flex()
                        .id("update-recovery")
                        .occlude()
                        .w(px(440.))
                        .max_w(relative(0.94))
                        .p(px(16.))
                        .gap(px(10.))
                        .rounded(px(14.))
                        .floating_ground(14., palette.pop, false, &palette)
                        .shadow(palette.pop_shadow(palette.border))
                        .child(div().text_size(px(14.)).text_color(palette.strong).child(title))
                        .child(div().text_size(px(12.)).text_color(palette.muted).child(if errors {
                            "Some agents could not continue. Review them or try again."
                        } else {
                            "Continue from their saved conversations."
                        }))
                        .child(
                            h_flex()
                                .justify_end()
                                .gap(px(8.))
                                .child(ui::ghost_button("update-recovery-later", "Later", palette).on_click(
                                    |_, _, cx| {
                                        cx.global_mut::<Recovery>().hidden = true;
                                        crate::windows::recovery_changed(cx);
                                    },
                                ))
                                .child(
                                    ui::button("update-recovery-review", "Review", palette)
                                        .on_click(cx.listener(|this, _, _, cx| this.review_update_agents(cx))),
                                )
                                .child(action),
                        ),
                ),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }

    pub(super) fn render_update_restart(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let confirmation = self.update_restart.confirmation.as_ref();
        if confirmation.is_none() && !self.update_restart.review {
            return None;
        }
        let entries = if confirmation.is_some() {
            Vec::new()
        } else {
            self.recovery_entries(cx)
        };
        let mut card = v_flex()
            .id("update-restart-dialog")
            .occlude()
            .w(px(520.))
            .max_w(relative(0.94))
            .p(px(24.))
            .gap(px(16.))
            .rounded(px(16.))
            .floating_ground(16., palette.pop, false, &palette)
            .text_color(palette.foreground)
            .shadow(palette.pop_shadow(palette.border))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        if let Some(sessions) = confirmation {
            card = card.child(div().text_size(px(22.)).child(format!("Restart with {} {} running?", sessions.len(), if sessions.len() == 1 { "agent" } else { "agents" })))
                .child(div().text_size(px(14.)).text_color(palette.muted).child("These agents will stop while Nexus restarts. Your sessions and files will stay here."))
                .child(div().text_size(px(13.)).text_color(palette.muted).child("When Nexus opens, choose Continue all to resume them. Agents on servers and in the background service keep running."));
            let mut names = v_flex()
                .id("update-restart-agents")
                .max_h(px(200.))
                .overflow_y_scroll()
                .gap(px(8.))
                .p(px(12.))
                .rounded(px(10.))
                .bg(palette.hover);
            for session in sessions {
                names = names.child(div().text_size(px(13.)).child(self.saved_agent_title(&session.id)));
            }
            card = card.child(names);
            let mut actions = h_flex().justify_end().gap(px(8.));
            actions = actions
                .child(
                    ui::button("update-restart-cancel", "Cancel", palette)
                        .track_focus(&self.update_restart.controls[0])
                        .on_click(cx.listener(|this, _, window, cx| this.close_update_dialog(window, cx))),
                )
                .child(
                    ui::primary_button("update-restart-confirm", "Stop agents & restart", palette)
                        .track_focus(&self.update_restart.controls[1])
                        .on_click(cx.listener(|this, _, _, cx| this.confirm_update_restart(cx))),
                );
            card = card.child(actions);
        } else {
            card = card
                .child(div().text_size(px(22.)).child(if entries.is_empty() {
                    "Your agents continued"
                } else {
                    "Continue your agents"
                }))
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(palette.muted)
                        .child("Each agent resumes its saved conversation in the same project."),
                );
            let mut rows = v_flex()
                .id("update-recovery-sessions")
                .max_h(px(330.))
                .overflow_y_scroll()
                .gap(px(8.));
            for (at, entry) in entries.iter().enumerate() {
                let id = entry.session.id.clone();
                let selected = self.update_restart.selected.contains(&id);
                let name = self.saved_agent_title(&id);
                let mut row = v_flex().gap(px(4.)).child(
                    h_flex()
                        .items_center()
                        .gap(px(12.))
                        .child(
                            ui::button(
                                ("update-recovery-select", at),
                                if selected { "✓" } else { "○" },
                                palette,
                            )
                            .when_some(
                                self.update_restart
                                    .review_ids
                                    .iter()
                                    .position(|id| id == &entry.session.id)
                                    .and_then(|at| self.update_restart.controls.get(at)),
                                |button, focus| button.track_focus(focus),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.update_restart.selected.remove(&id) {
                                    this.update_restart.selected.insert(id.clone());
                                }
                                cx.notify();
                            })),
                        )
                        .child(div().flex_1().min_w(px(0.)).truncate().child(name))
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(palette.muted)
                                .child(if entry.continuing {
                                    "Continuing…"
                                } else if entry.error.is_some() {
                                    "Needs attention"
                                } else {
                                    "Stopped"
                                }),
                        ),
                );
                if let Some(error) = &entry.error {
                    let id = entry.session.id.clone();
                    row = row
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(palette.destructive)
                                .child(error.clone()),
                        )
                        .child(
                            ui::ghost_button(("update-recovery-open", at), "Open session", palette).on_click(
                                cx.listener(move |this, _, window, cx| {
                                    this.close_update_dialog(window, cx);
                                    this.open_code(&id, window, cx);
                                }),
                            ),
                        );
                }
                rows = rows.child(row);
            }
            let ids = entries
                .iter()
                .filter(|entry| !entry.continuing && self.update_restart.selected.contains(&entry.session.id))
                .map(|entry| entry.session.id.clone())
                .collect::<Vec<_>>();
            let action = if ids.is_empty() || self.runner.is_none() {
                ui::disabled_button("update-recovery-selected", "Continue", palette).into_any_element()
            } else {
                ui::primary_button("update-recovery-selected", format!("Continue {}", ids.len()), palette)
                    .when_some(self.update_restart.controls.last(), |button, focus| {
                        button.track_focus(focus)
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.continue_update_agents(ids.clone(), cx)))
                    .into_any_element()
            };
            card = card.child(rows).child(
                h_flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        ui::button("update-recovery-close", "Close", palette)
                            .when_some(
                                self.update_restart.controls.get(self.update_restart.review_ids.len()),
                                |button, focus| button.track_focus(focus),
                            )
                            .on_click(cx.listener(|this, _, window, cx| this.close_update_dialog(window, cx))),
                    )
                    .child(action),
            );
        }
        let error = self
            .update_restart
            .error
            .clone()
            .or_else(|| cx.try_global::<Recovery>().and_then(|recovery| recovery.error.clone()));
        if let Some(error) = error {
            card = card.child(div().text_size(px(13.)).text_color(palette.destructive).child(error));
        }
        Some(
            deferred(
                div()
                    .absolute()
                    .inset_0()
                    .size_full()
                    .occlude()
                    .bg(palette.scrim)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(card),
            )
            .with_priority(4)
            .into_any_element(),
        )
    }
}
