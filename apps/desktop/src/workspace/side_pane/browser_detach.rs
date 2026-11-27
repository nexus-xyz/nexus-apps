//! Browser-only transfer; the source Workspace retains no detached view.
use super::{Browser, Scope, Tool, Workspace};
use crate::{palette::Palette, workspace::browser_detach as detach};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

impl Workspace {
    pub(in crate::workspace) fn detach_browser(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if !self.navigation_allowed() {
            return Err("This window is closed.".into());
        }
        let session = match &self.view {
            super::super::View::Code(view) => view.id.clone(),
            _ => return Err("Move to window is available for a local Code session Browser.".into()),
        };
        let origin = self
            .browser_origin(&session, window.window_handle().window_id(), cx)
            .ok_or("This local session source is not ready.")?;
        let key = origin.key.clone();
        if detach::is_detached(&key, cx) {
            detach::focus_key(&key, cx);
            return Ok(());
        }
        let browser = self
            .side
            .browsers
            .get(&session)
            .cloned()
            .ok_or("Open this session's Browser first.")?;
        browser.read(cx).detach_eligible(cx)?;
        if crate::windows::browser_other_copy(&key, browser.entity_id(), window.window_handle().window_id(), cx) {
            return Err("Close the other Browser copy for this session before moving it.".into());
        }
        self.sync_draft(window, cx);
        if !detach::register(origin, &browser, window, false, cx) {
            return Err("This session Browser is already owned by another window.".into());
        }
        // Validate everything before extracting any page. The empty old entity is permanently retired.
        self.side.browser_events.remove(&session);
        self.side.browsers.remove(&session);
        let retained = browser.update(cx, |browser, cx| browser.retain(cx));
        let owner = detach::begin(&key, browser.entity_id(), retained, cx)
            .ok_or("The source authority changed before the Browser moved.")?;
        if !detach::open(owner, cx) {
            if let Some((key, retained)) = detach::rollback(owner, cx) {
                let restored = cx.new(|cx| Browser::restore(retained, window, cx));
                if detach::attached(&key, &restored, window, cx) {
                    self.bind_browser(session, restored, window, cx);
                } else {
                    restored.update(cx, |browser, cx| drop(browser.retain(cx)));
                }
            }
            cx.notify();
            return Err("Couldn't open a Browser window. Your tabs were returned to the session.".into());
        }
        cx.notify();
        Ok(())
    }
    pub(in crate::workspace) fn restore_returned_browser(
        &mut self,
        key: &detach::Key,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Browser>> {
        if self.browser_detach_key(&key.session).as_ref() != Some(key) {
            return None;
        }
        let (_, retained) = detach::take_return(key, cx)?;
        let browser = cx.new(|cx| Browser::restore(retained, window, cx));
        if !detach::attached(key, &browser, window, cx) {
            browser.update(cx, |browser, cx| drop(browser.retain(cx)));
            return None;
        }
        Some(self.bind_browser(key.session.clone(), browser, window, cx))
    }
    /// Deliberately opening the exact session adopts its waiting pages without changing the selected pane.
    pub(super) fn sync_returned_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let super::super::View::Code(view) = &self.view else {
            return;
        };
        let session = view.id.clone();
        if self.side.browsers.contains_key(&session) {
            return;
        }
        let Some(key) = self.browser_detach_key(&session) else {
            return;
        };
        self.restore_returned_browser(&key, window, cx);
    }
    pub(in crate::workspace) fn show_returned_browser(
        &mut self,
        key: &detach::Key,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sync_side(window, cx);
        self.side.open = true;
        self.side.tool = Tool::Browser;
        self.restore_returned_browser(key, window, cx);
        cx.notify();
    }
    pub(crate) fn browser_matches_key(&self, key: &detach::Key) -> bool {
        self.browser_detach_key(&key.session).as_ref() == Some(key)
    }
    /// The side pane's "Retry output". Deferred: the retry reads every window's workspace (`windows.rs`'s
    /// `browser_output_conflict`), and this one is being updated now, so a read in place panics.
    pub(in crate::workspace) fn retry_browser_output(&mut self, key: &detach::Key, owner: u64, cx: &mut Context<Self>) {
        if self.browser_matches_key(key) {
            cx.defer(move |cx| detach::retry(owner, cx));
        }
    }
    pub(crate) fn browser_copy(&self, key: &detach::Key, browser: EntityId, cx: &App) -> bool {
        if self.side.parked_browsers.iter().any(|saved| {
            saved.identity == key.identity
                && (saved.authority)()
                && saved.session == key.session
                && matches!(&key.source, super::super::runs::Source::Local { data, scope }
                if *data == saved.source.data && *scope == saved.source.scope)
        }) {
            return true;
        }
        self.browser_matches_key(key)
            && self
                .side
                .browsers
                .get(&key.session)
                .is_some_and(|current| current.entity_id() != browser && !current.read(cx).is_retired())
    }
    /// The Workbench header's "Move to window" button for the Browser, when it can move.
    pub(super) fn browser_move_button(&self, palette: Palette, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let move_key = match self.side.scope.as_ref() {
            Some(Scope::Session(session)) => self.browser_detach_key(session),
            _ => None,
        }?;
        let browser = self.side.browser()?;
        if !matches!(self.view, super::super::View::Code(_)) || browser.read(cx).detach_eligible(cx).is_err() {
            return None;
        }
        let move_browser = browser.entity_id();
        Some(
            super::move_window_button("move-browser-window", palette).on_click(cx.listener(
                move |this, _, window, cx| {
                    if !(this.browser_matches_key(&move_key)
                        && this.side.browsers.get(&move_key.session).map(Entity::entity_id) == Some(move_browser)
                        && matches!(&this.view, super::super::View::Code(view) if view.id == move_key.session))
                    {
                        return;
                    }
                    if let Err(error) = this.detach_browser(window, cx) {
                        this.composer_note = Some(error.into());
                    }
                    cx.notify();
                },
            )),
        )
    }

    pub(super) fn render_browser_body(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let key = match self.side.scope.as_ref() {
            Some(Scope::Session(session)) => self.browser_detach_key(session),
            _ => None,
        };
        let browser = self.side.browser().cloned();
        let mut body = v_flex().size_full().min_h(px(0.));
        if let Some(key) = key.clone() {
            if let Some((owner, problem, retry)) = detach::notice(&key, cx) {
                body = body.child(
                    h_flex()
                        .gap(px(8.))
                        .p(px(10.))
                        .flex_wrap()
                        .child(crate::side::note(problem, palette))
                        .when(retry, |row| {
                            row.child(
                                crate::side::chip("retry-browser-output", "Retry output", false, palette).on_click(
                                    cx.listener(move |this, _, _, cx| this.retry_browser_output(&key, owner, cx)),
                                ),
                            )
                        }),
                );
            }
        }
        if let Some(browser) = browser {
            body.child(div().flex_1().min_h(px(0.)).child(browser))
                .into_any_element()
        } else if let Some(key) = key {
            let focus = key.clone();
            let restore = key;
            body.child(crate::side::note(
                "This session's Browser is in another window or waiting to return.",
                palette,
            ))
            .child(
                crate::side::chip("focus-session-browser", "Show Browser", false, palette).on_click(cx.listener(
                    move |this, _, _, cx| {
                        if this.browser_matches_key(&focus)
                            && matches!(&this.view, super::super::View::Code(view) if view.id == focus.session)
                        {
                            detach::focus_key(&focus, cx);
                        }
                    },
                )),
            )
            .child(
                crate::side::chip("restore-session-browser", "Return Browser", false, palette)
                    .debug_selector(|| "restore-session-browser".into())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if matches!(&this.view, super::super::View::Code(view) if view.id == restore.session)
                            && this.browser_matches_key(&restore)
                        {
                            if detach::is_detached(&restore, cx) {
                                detach::return_to_session(&restore, cx);
                            }
                            this.show_returned_browser(&restore, window, cx);
                        }
                    })),
            )
            .into_any_element()
        } else {
            body.into_any_element()
        }
    }
}
