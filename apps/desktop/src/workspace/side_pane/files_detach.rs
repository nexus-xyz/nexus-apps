//! Exact original local Files admission and replaceable attached presentations.
use super::{Files, Scope, Tool, Workspace};
use crate::{palette::Palette, workspace::files_detach as detach};
use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;

impl super::SidePane {
    pub(in crate::workspace) fn clear_files(&mut self, keep_owned: bool, cx: &mut App) {
        let key = self.files_key.take();
        if let Some(files) = self.files.take() {
            let state = files.update(cx, |files, cx| files.retain(cx));
            if keep_owned {
                if let Some(key) = key {
                    detach::park(&key, files.entity_id(), state, cx);
                }
            }
        }
        self.files_error = None;
        self.files_blocked = false;
    }
}
impl Workspace {
    fn selected_files_session(&self) -> Option<&str> {
        match &self.view {
            super::super::View::Code(view) if !view.is_draft() => Some(&view.id),
            _ => None,
        }
    }
    pub(in crate::workspace) fn detach_files(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if !self.navigation_allowed() {
            return Err("This window is closed.".into());
        }
        let session = self
            .selected_files_session()
            .ok_or("Move to window is available for a local Code session's Files.")?
            .to_string();
        detach::revoke_stale(cx);
        let key = self
            .files_key(&session)
            .ok_or("This local session source is not ready.")?;
        if detach::moving(&key, cx) {
            detach::focus(&key, cx);
            return Ok(());
        }
        // Focusing an already-qualified retained owner needs no replacement runner.
        // New ownership still requires concrete matching local Runs.
        let origin = self.files_origin(&session, window.window_handle().window_id())?;
        let key = origin.key.clone();
        if self.side.files_key.as_ref() != Some(&key) {
            return Err("Open Files for this session's original local source first.".into());
        }
        let files = self
            .side
            .files
            .as_ref()
            .cloned()
            .ok_or("Open this session's Files first.")?;
        if !files.read(cx).is_current() {
            return Err("The original Files source is unavailable.".into());
        }
        if crate::windows::files_other_copy(&key, files.entity_id(), window.window_handle().window_id(), cx) {
            return Err("Close the other Files copy for this session before moving it.".into());
        }
        self.side.files = None;
        let state = files.update(cx, |files, cx| files.retain(cx));
        let owner = detach::begin(origin, &files, state, cx)?;
        if !detach::open(owner, cx) {
            let restored = self.restore_files_owner(owner, false, window, cx).is_some();
            cx.notify();
            return Err(if restored {
                "Couldn't open a Files window. Your selection was returned to the session."
            } else {
                "The original Files source is no longer available."
            }
            .into());
        }
        cx.notify();
        Ok(())
    }
    fn restore_files_owner(
        &mut self,
        owner: u64,
        returning_only: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Files>> {
        let origin = detach::origin(owner, cx)?;
        if self.files_key(&origin.key.session).as_ref() != Some(&origin.key) {
            return None;
        }
        let state = detach::take_saved(owner, returning_only, cx)?;
        let guard = detach::guard(owner, cx)?;
        let files = cx.new(|cx| Files::restore(origin.place(), state, guard, window, cx));
        if !detach::attached(owner, &files, window, cx) {
            files.update(cx, |files, cx| files.retire(cx));
            return None;
        }
        self.side.clear_files(true, cx);
        self.side.files_key = Some(origin.key);
        self.side.files = Some(files.clone());
        Some(files)
    }
    pub(in crate::workspace) fn restore_returned_files(
        &mut self,
        key: &detach::Key,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Files>> {
        let owner = detach::owner(key, cx)?;
        self.restore_files_owner(owner, true, window, cx)
    }
    pub(super) fn sync_returned_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.selected_files_session().map(str::to_string) else {
            return;
        };
        let Some(key) = self.files_key(&session) else { return };
        if self.side.files_key.as_ref() == Some(&key) && self.side.files.is_some() {
            return;
        }
        self.restore_returned_files(&key, window, cx);
    }
    pub(in crate::workspace) fn show_returned_files(
        &mut self,
        key: &detach::Key,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sync_side(window, cx);
        self.side.open = true;
        self.side.tool = Tool::Files;
        if self.side.files.is_none() {
            self.restore_returned_files(key, window, cx);
        }
        cx.notify();
    }
    pub(super) fn selected_files_key(&self) -> Option<detach::Key> {
        self.selected_files_session()
            .and_then(|session| self.files_key(session))
    }
    pub(crate) fn files_matches_key(&self, key: &detach::Key) -> bool {
        self.files_key(&key.session).as_ref() == Some(key)
    }
    /// Pending tickets only choose an app window. They grant no access until the exact Account/reader arrives.
    pub(crate) fn browser_return_target(&self, key: &super::super::browser_detach::Key) -> bool {
        if !self.navigation_allowed()
            || !key
                .identity
                .matches_credential(self.gateway.url(), self.gateway.marker().as_deref())
        {
            return false;
        }
        self.browser_matches_key(key)
            || self.browser_return.as_ref().is_some_and(|ticket| ticket.key == *key)
            || self.files_return.as_ref().is_some_and(|ticket| {
                ticket.key.identity == key.identity
                    && ticket.key.source == key.source
                    && ticket.key.session == key.session
            })
            || self.commands_return.as_ref().is_some_and(|ticket| {
                ticket.key.identity == key.identity
                    && ticket.key.source == key.source
                    && ticket.key.session == key.session
            })
    }
    pub(crate) fn files_return_target(&self, key: &detach::Key) -> bool {
        if !self.navigation_allowed()
            || !key
                .identity
                .matches_credential(self.gateway.url(), self.gateway.marker().as_deref())
        {
            return false;
        }
        self.files_matches_key(key)
            || self.files_return.as_ref().is_some_and(|ticket| ticket.key == *key)
            || self.browser_return.as_ref().is_some_and(|ticket| {
                ticket.key.identity == key.identity
                    && ticket.key.source == key.source
                    && ticket.key.session == key.session
            })
            || self.commands_return.as_ref().is_some_and(|ticket| {
                ticket.key.identity == key.identity
                    && ticket.key.source == key.source
                    && ticket.key.session == key.session
            })
    }
    pub(crate) fn files_copy(&self, key: &detach::Key, files: EntityId, cx: &App) -> bool {
        self.side.files_key.as_ref() == Some(key)
            && self
                .side
                .files
                .as_ref()
                .is_some_and(|current| current.entity_id() != files && current.read(cx).is_current())
    }
    pub(crate) fn retire_files_view(&mut self, cx: &mut Context<Self>) {
        self.side.clear_files(false, cx);
    }
    fn make_session_files(
        &mut self,
        session: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Option<Entity<Files>>, String> {
        // Account-free sessions stay attached to the current window. The reader validates the exact
        // account-free record and folder without granting the account proof needed to detach.
        if self.code.as_ref().is_some_and(|code| code.is_local_session(session)) {
            let code = self.code.as_ref().ok_or("The local reader is unavailable.")?;
            let (data, _) = code.store_source();
            let reader =
                crate::code::CodeStore::open_reading_in(data, nexus_workbench::session_scope::SessionScope::Local)?
                    .ok_or("The local reader is unavailable.")?;
            let summary = reader.summary(session)?.ok_or("This local session is unavailable.")?;
            let folder = reader.folder(session)?;
            let original = folder.clone();
            let canonical = original.path.canonicalize().map_err(|error| error.to_string())?;
            let reader = reader.reader();
            let session = session.to_string();
            let guard = std::sync::Arc::new(move || {
                reader.matches_session(&session, summary.created_at, &summary.project_id, &original)
                    && original.path.canonicalize().is_ok_and(|path| path == canonical)
            });
            if !guard() {
                return Err("This local session folder is unavailable.".into());
            }
            let files = cx.new(|cx| Files::new_guarded(crate::side::Place::Here(Ok(folder)), guard, window, cx));
            self.side.files_error = None;
            return Ok(Some(files));
        }
        let key = self.files_key(session).ok_or("This local Files source is not ready.")?;
        if let Some(files) = self.restore_returned_files(&key, window, cx) {
            return Ok(Some(files));
        }
        if detach::reserved(&key, window.window_handle().window_id(), cx) {
            self.side.files_key = Some(key);
            return Ok(None);
        }
        let (key, place, guard) = self.files_attached_source(session)?;
        let files = cx.new(|cx| Files::new_guarded(place, guard, window, cx));
        self.side.files_key = Some(key);
        self.side.files_error = None;
        Ok(Some(files))
    }
    pub(super) fn sync_files_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.side.files_blocked {
            return;
        }
        let invalid = self.side.files_key.as_ref().is_some_and(|key| {
            self.files_key(&key.session).as_ref() != Some(key)
                || self
                    .side
                    .files
                    .as_ref()
                    .is_some_and(|files| !files.read(cx).is_current())
        });
        if invalid {
            self.side.clear_files(false, cx);
            self.side.files_blocked = true;
            self.side.files_error =
                Some("The original Files source is unavailable. Reopen its session after it reconnects.".into());
            return;
        }
        if self.side.files.is_none() {
            if let Some(session) = self.selected_files_session().map(str::to_string) {
                if matches!(
                    self.scope_place(&Scope::Session(session.clone())),
                    crate::side::Place::Server(_)
                ) {
                    self.side.files_error = None;
                    return;
                }
                match self.make_session_files(&session, window, cx) {
                    Ok(files) => self.side.files = files,
                    Err(error) => self.side.files_error = Some(error),
                }
            }
        }
    }
    pub(super) fn route_files_open(
        &mut self,
        path: &str,
        line: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(session) = self.selected_files_session() else {
            return false;
        };
        let Some(key) = self.files_key(session) else {
            return false;
        };
        detach::revoke_stale(cx);
        if let Some((owner, target)) = detach::live_target(&key, cx) {
            // A local attached owner still uses the existing pane-selection path below.
            if target.window_id() == window.window_handle().window_id() {
                return false;
            }
            let expected = target.window_id();
            let path = path.to_string();
            cx.defer(move |cx| {
                if let Some((_, current)) = detach::live_target(&key, cx) {
                    if current.window_id() == expected {
                        let _ = current.update(cx, |_, window, cx| {
                            if let Some(files) = detach::target_files(owner, expected, cx) {
                                files.update(cx, |files, cx| files.open_at(path, line, window, cx));
                                window.activate_window();
                            }
                        });
                    }
                }
            });
            return true;
        }
        false
    }
    /// The Workbench header's "Move to window" button for Files, when they can move.
    pub(super) fn files_move_button(&self, palette: Palette, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let move_key = self
            .selected_files_session()
            .and_then(|session| self.files_key(session))?;
        let files = self.side.files.as_ref()?;
        if !files.read(cx).is_current() {
            return None;
        }
        let expected = files.entity_id();
        Some(
            super::move_window_button("move-files-window", palette).on_click(cx.listener(
                move |this, _, window, cx| {
                    if !(this.files_matches_key(&move_key)
                        && this.selected_files_session() == Some(move_key.session.as_str())
                        && this.side.files.as_ref().map(Entity::entity_id) == Some(expected))
                    {
                        return;
                    }
                    if let Err(error) = this.detach_files(window, cx) {
                        this.composer_note = Some(error.into());
                    }
                    cx.notify();
                },
            )),
        )
    }

    pub(super) fn render_files_body(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let key = self
            .selected_files_session()
            .and_then(|session| self.files_key(session));
        let files = self.side.files.clone();
        let mut body = v_flex().size_full().min_h(px(0.));
        if let Some(error) = self.side.files_error.clone() {
            body = body.child(crate::side::note(error, palette));
        }
        if let Some(key) = key.clone() {
            if let Some(problem) = detach::notice(&key, cx) {
                body = body.child(crate::side::note(problem, palette));
            }
        }
        if let Some(files) = files {
            return body.child(div().flex_1().min_h(px(0.)).child(files)).into_any_element();
        }
        if let Some(key) = key {
            if detach::owner(&key, cx).is_some() {
                let focus = key.clone();
                let returning = key;
                body = body
                    .child(crate::side::note(
                        "This session's Files is in another window or waiting to return.",
                        palette,
                    ))
                    .child(
                        crate::side::chip("show-session-files", "Show Files", false, palette).on_click(cx.listener(
                            move |this, _, _, cx| {
                                if this.selected_files_session() == Some(focus.session.as_str())
                                    && this.files_matches_key(&focus)
                                {
                                    detach::focus(&focus, cx);
                                }
                            },
                        )),
                    )
                    .child(
                        crate::side::chip("restore-session-files", "Return Files", false, palette)
                            .debug_selector(|| "restore-session-files".into())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if this.selected_files_session() != Some(returning.session.as_str())
                                    || !this.files_matches_key(&returning)
                                {
                                    return;
                                }
                                detach::request_return(&returning, cx);
                                this.show_returned_files(&returning, window, cx);
                            })),
                    );
            }
        }
        body.into_any_element()
    }
    #[cfg(test)]
    pub(in crate::workspace) fn fixture_terminal_views(&mut self) {
        self.side.terminal_factory = super::TerminalSet::for_view_test;
    }
    #[cfg(test)]
    pub(in crate::workspace) fn fixture_private_terminal_count(&self) -> usize {
        self.side.terminals.len()
    }
    #[cfg(test)]
    pub(in crate::workspace) fn fixture_files_navigation(&mut self, session: &str) {
        self.side
            .navigation
            .insert(session.into(), super::navigation::Navigation::new(true));
    }
    #[cfg(test)]
    pub(in crate::workspace) fn fixture_select_files_session(
        &mut self,
        session: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.view = super::super::View::Code(super::super::code_view::CodeView::new(session.into(), window, cx));
        self.side.scope = Some(Scope::Session(session.into()));
        self.side.tool = Tool::Files;
        self.side.open = true;
        // Do not read or save real workbench preferences when native pane input is exercised.
        self.fixture_files_navigation(session);
        self.side.dir = self
            .scope_folder(&Scope::Session(session.into()))
            .map(|folder| folder.path)
            .unwrap_or_default();
    }
    #[cfg(test)]
    pub(in crate::workspace) fn fixture_local_files(
        &mut self,
        session: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Entity<Files>, String> {
        let files = self
            .make_session_files(session, window, cx)?
            .ok_or("Files already belongs to another presentation.")?;
        self.side.files = Some(files.clone());
        Ok(files)
    }
    #[cfg(test)]
    pub(in crate::workspace) fn fixture_files(&self) -> Option<Entity<Files>> {
        self.side.files.clone()
    }
}
