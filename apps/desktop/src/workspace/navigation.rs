//! The primary window's journal and plain Settings presentation; fresh views perform every read.
use super::runs::Source;
use super::settings_shell::{Positions, SavedPosition};
use super::{Place, RetainedNewCode, View, Workspace};
use crate::windows::retention::Identity;
use gpui_kit::{Context, Pixels, Point, Window, px};
use std::collections::HashMap;

pub(super) struct Retained {
    places: Vec<Place>,
    at: usize,
    positions: Positions,
    sources: HashMap<usize, Source>,
    pending_field: Option<&'static str>,
}
pub(super) struct ScrollRestore {
    epoch: u64,
    at: usize,
    place: Place,
    scope: Option<(String, String, String)>,
    identity: Option<Identity>,
    offset: Point<Pixels>,
    scheduled: bool,
}
impl Workspace {
    pub(super) fn rebind_navigation_settings(
        &mut self,
        section: super::SettingsSection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let scope = self.account.as_ref().map(|account| {
            (
                self.gateway.url().to_string(),
                account.org.clone(),
                account.email.clone(),
            )
        });
        let preserve = scope.is_some() && self.settings.shell.account_scope == scope;
        let field = self.settings.shell.fields.pending_target();
        if preserve {
            self.remember_settings_position();
        }
        let restoring = self.navigation_restoring;
        self.navigation_restoring = true;
        self.open_settings(section, window, cx);
        self.navigation_restoring = restoring;
        if let Some(field) = field {
            self.cancel_navigation_restore();
            self.settings.shell.fields.request(field);
            cx.notify();
        } else if preserve {
            self.restore_settings_position(self.history_at, window, cx);
        }
    }

    pub(super) fn apply_primary_retained(
        &mut self,
        saved: super::RetainedPrimary,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.terminal_identity.as_ref() != Some(&saved.identity)
            || !saved
                .identity
                .matches_credential(self.gateway.url(), self.gateway.marker().as_deref())
        {
            return false;
        }
        let explicit = self.navigation_explicit;
        let selected = self.adopt_gateway_chat(saved.execution, window, cx);
        // Account readiness adopts a parked journal internally. Its explicit visit must not cancel
        // a Return queued before the first Account; a later real user visit still clears that ticket.
        let browser_return = self.browser_return.take();
        let files_return = self.files_return.take();
        let commands_return = self.commands_return.take();
        self.restore_navigation(saved.navigation, saved.new_code, window, cx);
        self.browser_return = browser_return;
        self.files_return = files_return;
        self.commands_return = commands_return;
        if !explicit {
            self.restore_gateway_selection(selected, window, cx);
        }
        self.restore_private_terminals(saved.terminals, window, cx);
        true
    }
    pub(super) fn navigation_settings_offset(&self) -> Point<Pixels> {
        if let Some(restore) = &self.navigation_scroll {
            if restore.epoch == self.navigation_epoch
                && restore.at == self.history_at
                && self.history.get(restore.at) == Some(&restore.place)
                && self.terminal_identity == restore.identity
                && self.settings.shell.account_scope == restore.scope
            {
                return restore.offset;
            }
        }
        self.settings.scroll.offset()
    }

    pub(super) fn accept_navigation_identity(&mut self, account: &crate::gateway::Account, cx: &mut Context<Self>) {
        let identity = self.current_identity(account);
        if self.terminal_identity != identity {
            self.discard_private_terminals(cx);
        }
        super::browser_detach::identity_changed(identity.as_ref(), cx);
        super::files_detach::identity_changed(identity.as_ref(), cx);
        super::commands_detach::identity_changed(identity.as_ref(), cx);
        self.terminal_identity = identity;
    }

    pub(super) fn cancel_navigation_restore(&mut self) {
        self.navigation_epoch = self.navigation_epoch.wrapping_add(1);
        self.navigation_scroll = None;
        self.settings.shell.fields.clear();
    }
    pub(super) fn clear_navigation(&mut self) {
        self.cancel_navigation_restore();
        self.history = vec![Place::Chat(None)];
        self.history_at = 0;
        self.navigation_explicit = false;
        self.navigation_sources.clear();
        self.settings.shell.positions.clear();
        self.settings.shell.account_scope = None;
        self.settings.shell.query_text.clear();
        self.settings.shell.searching = false;
        self.settings.shell.selected = 0;
    }
    pub(super) fn navigation_allowed(&self) -> bool {
        !self.navigation_retired
            && self.terminal_identity.as_ref().is_none_or(|identity| {
                identity.matches_credential(self.gateway.url(), self.gateway.marker().as_deref())
            })
    }
    pub(super) fn route_source(&self, place: &Place) -> Option<Source> {
        let (Place::Code(id) | Place::Pull(id)) = place else {
            return None;
        };
        if let Some(machine) = self
            .machine_source
            .machine_of(id)
            .or_else(|| self.cloud_machine_of_session(id).map(|cloud| cloud.id.clone()))
        {
            return Some(Source::Machine(machine));
        }
        if let Some(code) = &self.code
            && code.is_local_session(id)
        {
            let (data, _) = code.store_source();
            return Some(Source::Local {
                data: data.to_path_buf(),
                scope: nexus_workbench::session_scope::SessionScope::local_text().into(),
            });
        }
        if let Some(runner) = &self.runner {
            let (data, scope) = runner.store_source();
            return Some(Source::Local { data, scope });
        }
        Some(Source::Local {
            data: crate::paths::data_dir()?,
            scope: self.code_scope.clone()?,
        })
    }
    pub(super) fn retain_navigation(&mut self) -> Retained {
        self.remember_settings_position();
        let pending_field = self.settings.shell.fields.pending_target();
        self.cancel_navigation_restore();
        self.navigation_retired = true;
        let saved = Retained {
            places: std::mem::take(&mut self.history),
            at: self.history_at,
            positions: std::mem::take(&mut self.settings.shell.positions),
            sources: std::mem::take(&mut self.navigation_sources),
            pending_field,
        };
        self.history_at = 0;
        saved
    }
    pub(super) fn restore_navigation(
        &mut self,
        saved: Retained,
        form: Option<RetainedNewCode>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A tray/deep-link/user route already opened in this replacement outranks its parked route.
        let explicit = self
            .navigation_explicit
            .then(|| self.history.get(self.history_at).cloned())
            .flatten();
        let explicit_position = matches!(self.view, View::Settings(_)).then(|| {
            (
                self.settings.shell.account_scope.clone(),
                (
                    self.settings.scroll.offset(),
                    self.settings.shell.query_text.clone(),
                    self.settings.shell.selected,
                    self.settings.shell.searching,
                ),
            )
        });
        self.cancel_navigation_restore();
        self.navigation_retired = false;
        self.history = if saved.places.is_empty() {
            vec![Place::Chat(None)]
        } else {
            saved.places
        };
        self.history_at = saved.at.min(self.history.len() - 1);
        self.navigation_sources = saved.sources;
        self.settings.shell.positions = saved.positions;
        if let Some(explicit) = explicit {
            let restoring = self.settings.shell.restoring;
            self.settings.shell.restoring = true;
            self.visit(explicit.clone());
            self.settings.shell.restoring = restoring;
            self.open_place(explicit, window, cx);
            if let Some((scope, position)) = explicit_position {
                self.settings.shell.positions.insert((scope, self.history_at), position);
            }
            return;
        }
        self.navigation_restoring = true;
        self.open_navigation_place(window, cx);
        if matches!(self.view, View::NewCode) {
            if let Some(form) = form {
                if form
                    .project
                    .as_ref()
                    .is_some_and(|id| form.source.as_ref() != self.route_source(&Place::Code(id.clone())).as_ref())
                {
                    self.code_form.project = None;
                    self.composer_note =
                        Some("The previous project's source changed. Select its project again before sending.".into());
                } else {
                    self.code_form.project = form.project;
                }
                self.code_form.plain = form.plain;
                self.cloud_ui.on = form.cloud;
                self.code_form.use_project();
                self.sync_draft(window, cx);
            }
        }
        self.navigation_restoring = false;
        if let Some(field) = saved.pending_field.filter(|_| matches!(self.view, View::Settings(_))) {
            self.cancel_navigation_restore();
            self.settings.shell.fields.request(field);
            cx.notify();
        } else {
            self.restore_settings_position(self.history_at, window, cx);
        }
    }
    pub(super) fn open_navigation_place(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let place = self.history.get(self.history_at).cloned().unwrap_or(Place::Chat(None));
        if matches!(place, Place::Code(_) | Place::Pull(_))
            && self.navigation_sources.get(&self.history_at) != self.route_source(&place).as_ref()
        {
            self.reset_chat();
            self.composer_note =
                Some("Reconnect to this session's original source, then open it from the sidebar.".into());
            return;
        }
        if matches!(place, Place::Code(_) | Place::Pull(_)) && !self.navigation_sources.contains_key(&self.history_at) {
            self.reset_chat();
            self.composer_note = Some("Open this session from the sidebar after its source reconnects.".into());
            return;
        }
        self.open_place(place, window, cx);
    }
    pub(super) fn restore_navigation_settings(&mut self, at: usize, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.view, View::Settings(_)) || !self.navigation_allowed() {
            return;
        }
        let Some((offset, query, selected, searching)): Option<SavedPosition> = self
            .settings
            .shell
            .positions
            .get(&(self.settings.shell.account_scope.clone(), at))
            .cloned()
        else {
            return;
        };
        self.cancel_navigation_restore();
        self.settings.shell.query_text = query.clone();
        self.settings.shell.jump.set(None);
        self.settings.shell.fields.clear();
        self.settings.shell.selected = selected;
        self.settings.shell.searching = searching;
        self.settings
            .shell
            .query
            .update(cx, |state, cx| state.set_value(query, window, cx));
        self.navigation_scroll = Some(ScrollRestore {
            epoch: self.navigation_epoch,
            at,
            place: self.history[at].clone(),
            scope: self.settings.shell.account_scope.clone(),
            identity: self.terminal_identity.clone(),
            offset,
            scheduled: false,
        });
        self.schedule_navigation_scroll(window, cx);
    }
    pub(super) fn schedule_navigation_scroll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(restore) = self.navigation_scroll.as_mut() else {
            return;
        };
        if restore.scheduled {
            return;
        }
        restore.scheduled = true;
        let epoch = self.navigation_epoch;
        let weak = cx.weak_entity();
        window.on_next_frame(move |_, cx| {
            let _ = weak.update(cx, |this, cx| {
                // An older frame must not even take a newer entry's restoration ticket.
                if epoch != this.navigation_epoch || !this.navigation_allowed() {
                    return;
                }
                let Some(mut restore) = this.navigation_scroll.take() else {
                    return;
                };
                if restore.epoch != epoch
                    || restore.at != this.history_at
                    || this.history.get(restore.at) != Some(&restore.place)
                    || this.terminal_identity != restore.identity
                    || this.settings.shell.account_scope != restore.scope
                    || !matches!(this.view, View::Settings(_))
                {
                    return;
                }
                restore.scheduled = false;
                let max = this.settings.scroll.max_offset();
                let target = Point {
                    x: restore.offset.x.max(-max.x).min(px(0.)),
                    y: restore.offset.y.max(-max.y).min(px(0.)),
                };
                let changed = this.settings.scroll.offset() != target;
                if changed {
                    this.settings.scroll.set_offset(target);
                }
                if target != restore.offset {
                    this.navigation_scroll = Some(restore);
                }
                // No timer or frame loop: delayed content/resize will notify its ordinary view and
                // render schedules another attempt. User input or a new route invalidates this ticket.
                if changed {
                    cx.notify();
                }
            });
        });
    }
}

#[cfg(test)]
pub(super) mod test_backend;
#[cfg(test)]
mod tests;
