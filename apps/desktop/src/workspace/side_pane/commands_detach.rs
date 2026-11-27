//! Exact local Commands admission and fresh attached presentations.
use super::{Tool, Workspace};
use crate::{side::shared_terminals::SharedTerminals, workspace::commands_detach as detach};
use gpui_kit::prelude::*;
use gpui_kit::*;
impl super::SidePane {
    pub(in crate::workspace) fn clear_commands_session(&mut self, session: &str, keep_owned: bool, cx: &mut App) {
        let key = self.shared_keys.remove(session);
        if let Some(commands) = self.shared.remove(session) {
            let state = commands.update(cx, |commands, cx| commands.retain(cx));
            if keep_owned {
                if let Some(key) = key {
                    detach::park(&key, commands.entity_id(), state, cx);
                }
            }
        }
    }
    pub(in crate::workspace) fn clear_commands(&mut self, keep_owned: bool, cx: &mut App) {
        let sessions = self.shared.keys().cloned().collect::<Vec<_>>();
        for session in sessions {
            self.clear_commands_session(&session, keep_owned, cx);
        }
    }
}
impl Workspace {
    fn selected_commands_session(&self) -> Option<&str> {
        match &self.view {
            super::super::View::Code(view) if !view.is_draft() => Some(&view.id),
            _ => None,
        }
    }
    pub(in crate::workspace) fn detach_commands(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if !self.navigation_allowed() {
            return Err("This window is closed.".into());
        }
        let session = self
            .selected_commands_session()
            .ok_or("Move to window is available for a local Code session's Commands.")?
            .to_string();
        detach::revoke_stale(cx);
        let key = self
            .commands_key(&session)
            .ok_or("This local session source is not ready.")?;
        if detach::moving(&key, cx) {
            detach::focus(&key, cx);
            return Ok(());
        }
        // Focusing an already-qualified retained owner needs no replacement runner.
        // New ownership still requires concrete matching local Runs.
        let origin = self.commands_origin(&session, window.window_handle().window_id())?;
        let key = origin.key.clone();
        if self.side.shared_keys.get(&session) != Some(&key) {
            return Err("Open Commands for this session's original local source first.".into());
        }
        let commands = self
            .side
            .shared
            .get(&session)
            .cloned()
            .ok_or("Open this session's Commands first.")?;
        if !commands.read(cx).is_current() {
            return Err("The original Commands source is unavailable.".into());
        }
        if crate::windows::commands_other_copy(&key, commands.entity_id(), window.window_handle().window_id(), cx) {
            return Err("Close the other Commands copy for this session before moving it.".into());
        }
        self.side.shared.remove(&session);
        commands.update(cx, |commands, _| commands.set_owner_guard(origin.guard.clone()));
        let state = commands.update(cx, |commands, cx| commands.retain(cx));
        let owner = detach::begin(origin, &commands, state, cx)?;
        if !detach::open(owner, cx) {
            let restored = self.restore_commands_owner(owner, false, window, cx).is_some();
            cx.notify();
            return Err(if restored {
                "Couldn't open a Commands window. Your selection was returned to the session."
            } else {
                "The original Commands source is no longer available."
            }
            .into());
        }
        cx.notify();
        Ok(())
    }
    fn restore_commands_owner(
        &mut self,
        owner: u64,
        returning_only: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<SharedTerminals>> {
        let origin = detach::origin(owner, cx)?;
        if self.commands_key(&origin.key.session).as_ref() != Some(&origin.key) {
            return None;
        }
        let session = origin.key.session.clone();
        let state = detach::take_saved(owner, returning_only, cx)?;
        let guard = detach::guard(owner, cx)?;
        let commands = cx.new(|cx| SharedTerminals::restore(state, guard, window, cx));
        if !detach::attached(owner, &commands, window, cx) {
            commands.update(cx, |commands, cx| commands.retire(cx));
            return None;
        }
        self.side.clear_commands_session(&origin.key.session, true, cx);
        self.side.shared_keys.insert(origin.key.session.clone(), origin.key);
        self.side.shared.insert(session.clone(), commands.clone());
        self.rebind_shared_terminal(&commands, session, window, cx);
        Some(commands)
    }
    pub(in crate::workspace) fn restore_returned_commands(
        &mut self,
        key: &detach::Key,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<SharedTerminals>> {
        let owner = detach::owner(key, cx)?;
        self.restore_commands_owner(owner, true, window, cx)
    }
    pub(super) fn sync_returned_commands(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.selected_commands_session().map(str::to_string) else {
            return;
        };
        let Some(key) = self.commands_key(&session) else { return };
        if self.side.shared_keys.get(&session) == Some(&key) && self.side.shared.contains_key(&session) {
            return;
        }
        self.restore_returned_commands(&key, window, cx);
    }
    pub(in crate::workspace) fn show_returned_commands(
        &mut self,
        key: &detach::Key,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sync_side(window, cx);
        self.side.open = true;
        self.side.tool = Tool::Terminal;
        if !self.side.shared.contains_key(&key.session) {
            self.restore_returned_commands(key, window, cx);
        }
        cx.notify();
    }
    pub(in crate::workspace) fn selected_commands_key(&self) -> Option<detach::Key> {
        self.selected_commands_session()
            .and_then(|session| self.commands_key(session))
    }
    pub(crate) fn commands_matches_key(&self, key: &detach::Key) -> bool {
        self.commands_key(&key.session).as_ref() == Some(key)
    }

    pub(crate) fn commands_copy(&self, key: &detach::Key, commands: EntityId, cx: &App) -> bool {
        self.side.shared_keys.get(&key.session) == Some(key)
            && self
                .side
                .shared
                .get(&key.session)
                .is_some_and(|current| current.entity_id() != commands && current.read(cx).is_current())
    }
    pub(crate) fn commands_return_target(&self, key: &detach::Key) -> bool {
        if !self.navigation_allowed()
            || !key
                .identity
                .matches_credential(self.gateway.url(), self.gateway.marker().as_deref())
        {
            return false;
        }
        self.commands_matches_key(key)
            || self.commands_return.as_ref().is_some_and(|ticket| ticket.key == *key)
            || self.browser_return.as_ref().is_some_and(|ticket| {
                ticket.key.identity == key.identity
                    && ticket.key.source == key.source
                    && ticket.key.session == key.session
            })
            || self.files_return.as_ref().is_some_and(|ticket| {
                ticket.key.identity == key.identity
                    && ticket.key.source == key.source
                    && ticket.key.session == key.session
            })
    }
    pub(crate) fn retire_commands_view(&mut self, cx: &mut Context<Self>) {
        self.side.clear_commands(false, cx);
    }
    #[cfg(test)]
    pub(in crate::workspace) fn fixture_local_commands(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<SharedTerminals>> {
        self.ensure_shared_terminal(window, cx)
    }
    #[cfg(test)]
    pub(in crate::workspace) fn fixture_bind_commands(&mut self, key: detach::Key, commands: Entity<SharedTerminals>) {
        self.side.tool = Tool::Terminal;
        self.side.shared.insert(key.session.clone(), commands);
        self.side.shared_keys.insert(key.session.clone(), key);
    }
    #[cfg(test)]
    pub(in crate::workspace) fn fixture_attached_commands(&self, session: &str) -> Option<Entity<SharedTerminals>> {
        self.side.shared.get(session).cloned()
    }
    #[cfg(test)]
    pub(in crate::workspace) fn fixture_commands_empty(&self) -> bool {
        self.side.shared.is_empty()
    }
    #[cfg(test)]
    pub(in crate::workspace) fn fixture_commands_auxiliary_runs(
        &mut self,
        session: &str,
        other: &str,
        project: &str,
        runs: super::super::runs::Runs,
    ) {
        self.run_fixtures.insert(session.into(), runs.clone());
        self.run_fixtures.insert(other.into(), runs.clone());
        self.run_fixtures.insert(project.into(), runs.clone());
        self.files_fixture_runs = Some(runs.clone());
        self.browser_detach_runs = Some(runs.clone());
        self.side.side_chat_fixture_runs = Some(runs);
    }
}
