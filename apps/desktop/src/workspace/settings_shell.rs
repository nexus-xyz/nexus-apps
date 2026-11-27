//! Settings' shell, as the Nexus app's (brand v5; `settings-navigation.ts`): the nav in the sidebar's
//! place and the open destination's page beside it. The registry (`settings_registry.rs`) gives both their rows.
//!
//! The nav has Back to app, a search field, then two parts: "You" (your
//! account, preferences and this computer) and the organization's (or "Personal"), labelled with its logo and your
//! role. A destination this build does not draw opens in the browser, and its row says so. A row with several sections
//! (Preferences, This computer, People, Model access…) unfolds while it is open: each section is a sub-row under it,
//! and the page is the open section's own, titled with the row's name above it. Only the open section is drawn.
//!
//! Each section is a place in the back and forward history. A step back or forward restores the page's scroll position.
//! Keys: Ctrl+, (⌘, on macOS) opens Settings, with the keys in the search field. In the nav, ↑ and ↓ move, Enter opens and / goes to search. In search,
//! ↑ and ↓ pick a result, Enter opens it and Esc clears the search. Another Esc returns to the app.

use std::cell::Cell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::settings_registry::{Dest, NavItem, Part, Visibility, layout, nav_items};
use super::settings_search::{Hit, search};
use super::settings_view::SettingsSection;
use super::{Place, View, Workspace, glyph};
use crate::motion::{self, TweenExt as _};
use crate::pages::ui;
use crate::palette::Palette;

/// How long "Saved" stays beside a control.
const SAVED_FOR: Duration = Duration::from_secs(2);
/// Below this width the page's margins narrow.
const NARROW_WIDTH: f32 = 900.;

/// What happened to a change, beside its control.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum RowStatus {
    Saving,
    Saved(Instant),
    /// It was not kept. The control shows the saved value again; the text says why.
    Failed(SharedString),
}

/// The shell's state, kept in `SettingsState` while the window lives.
type AccountScope = (String, String, String);
pub(super) type SavedPosition = (Point<Pixels>, String, usize, bool);

pub(super) type Positions = HashMap<(Option<AccountScope>, usize), SavedPosition>;

pub(super) struct Shell {
    pub account_scope: Option<AccountScope>,
    pub account_access: Option<(String, Vec<String>)>,
    pub fields: crate::settings_fields::SettingsFields,
    /// The open destination's sections, with the native view of each page section. A page section without a view
    /// opens in the browser.
    pub views: Vec<(SettingsSection, Option<AnyView>)>,
    /// Whether the last build had the account needed for its native page sections.
    pub account_ctx: Option<crate::pages::Ctx>,
    /// The section to scroll to on the next frame: its index in `views`.
    pub jump: Cell<Option<usize>>,
    /// Each history entry's scroll position, by its index in the history.
    pub positions: Positions,
    pub query_text: String,
    pub searching: bool,
    pub results_scroll: ScrollHandle,
    pub results_selected: Cell<Option<usize>>,
    /// While back or forward opens a place: the visit records nothing, and the position comes back.
    pub restoring: bool,
    pub query: Entity<InputState>,
    /// The picked search result.
    pub selected: usize,
    pub nav_focus: FocusHandle,
    /// The section Settings last showed in this window: Ctrl+, (⌘,) lands on it again.
    pub last_section: Option<SettingsSection>,
    pub nav_scroll: ScrollHandle,
    pub nav_jump: Cell<bool>,
    /// Set by a click on a nav row, read by the next open: only a pointer's change of row folds and unfolds with
    /// motion. Keys, search, history and a fresh Settings show the result in place.
    pub row_clicked: bool,
    /// Whether the open row's sub-rows move as they unfold, and the last open row's as they fold.
    pub fold_motion: bool,
    /// The open row's sections are folded by a click on the row; any open shows them again.
    pub folded: bool,
    /// The nav row or sub-row the keyboard is on.
    pub cursor: Option<NavItem>,
    /// Each control's last change, by its field's anchor.
    pub status: HashMap<&'static str, RowStatus>,
    _query: Subscription,
}

impl Shell {
    pub fn new(window: &mut Window, scroll: ScrollHandle, cx: &mut Context<Workspace>) -> Shell {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search settings"));
        let subscription = cx.subscribe_in(&query, window, |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::Change => {
                let value = this.settings.shell.query.read(cx).value().to_string();
                if value == this.settings.shell.query_text {
                    return;
                }
                this.cancel_navigation_restore();
                this.settings.shell.searching = true;
                this.settings.shell.results_selected.set(Some(0));
                this.settings.shell.results_scroll.set_offset(point(px(0.), px(0.)));
                this.settings.shell.selected = 0;
                this.settings.shell.query_text = this.settings.shell.query.read(cx).value().to_string();
                cx.notify();
            }
            InputEvent::PressEnter { .. } => this.open_selected_hit(window, cx),
            _ => {}
        });
        Shell {
            account_scope: None,
            account_access: None,
            fields: crate::settings_fields::SettingsFields::new(window, scroll),
            views: Vec::new(),
            account_ctx: None,
            jump: Cell::new(None),
            positions: HashMap::new(),
            query_text: String::new(),
            searching: false,
            results_scroll: ScrollHandle::new(),
            results_selected: Cell::new(None),
            restoring: false,
            query,
            selected: 0,
            nav_focus: cx.focus_handle(),
            last_section: None,
            nav_scroll: ScrollHandle::new(),
            nav_jump: Cell::new(false),
            row_clicked: false,
            fold_motion: false,
            folded: false,
            cursor: None,
            status: HashMap::new(),
            _query: subscription,
        }
    }

    fn shown(&self) -> Vec<SettingsSection> {
        self.views.iter().map(|(section, _)| *section).collect()
    }
}

impl SettingsSection {
    pub(super) fn needs_account(self) -> bool {
        match self {
            Self::Page(id) => !id.is_native() || id.builder().needs_account(),
            Self::Account | Self::ApiKeys | Self::GatewayLink(_) => true,
            Self::Appearance | Self::Notifications | Self::Shortcuts | Self::LocalProviders => false,
        }
    }
}

impl Workspace {
    /// Which rows this account sees (`settingsNav`'s options).
    pub(super) fn settings_visibility(&self) -> Visibility {
        let managed = crate::managed::locked_gateway(crate::managed::policy()).is_some();
        if self.is_local() {
            return Visibility::signed_out(managed);
        }
        let mut visibility =
            Visibility::from_account(self.account.as_ref(), managed).with_assistant(self.assistant_available());
        visibility.local_providers = self.byo_entry();
        visibility
    }

    fn settings_destination_status(&self, dest: Dest) -> Option<&'static str> {
        if self.account_gateway().is_some() {
            return None;
        }
        dest.first().map_or(Some(ui::REQUIRES_SIGN_IN), |section| {
            dest.sections()
                .iter()
                .copied()
                .chain([section])
                .all(|section| self.settings_account_status(section).is_some())
                .then_some(ui::REQUIRES_SIGN_IN)
        })
    }

    pub(super) fn settings_account_status(&self, section: SettingsSection) -> Option<&'static str> {
        if self.account_gateway().is_some() {
            return None;
        }
        section.needs_account().then_some(ui::REQUIRES_SIGN_IN)
    }

    /// Open Settings at `section`: its destination's page, scrolled to it. A destination's views are built once and
    /// kept while it stays open, so a jump between its sections keeps what they show.
    pub(super) fn open_settings(&mut self, section: SettingsSection, window: &mut Window, cx: &mut Context<Self>) {
        if !self.workspace_open()
            || !self.navigation_allowed()
            || (section == SettingsSection::LocalProviders && !self.byo_entry())
        {
            return;
        }
        if !self.is_local()
            && let SettingsSection::GatewayLink(dest) = section
        {
            self.open_dest(dest, window, cx);
            return;
        }
        if !self.navigation_restoring && !self.settings.shell.restoring {
            self.browser_return = None;
            self.files_return = None;
            self.commands_return = None;
            self.navigation_explicit = true;
            self.cancel_navigation_restore();
        }

        // Plan usage is part of Agents and plans now: a link to it opens that section.
        let section = match section {
            SettingsSection::Page(crate::pages::PageId::Subscriptions) => {
                SettingsSection::Page(crate::pages::PageId::Agents)
            }
            other => other,
        };
        let search_focused = self.settings.shell.query.read(cx).focus_handle(cx).is_focused(window);
        let entering = !matches!(self.view, View::Settings(_));
        // The system's reduce transparency may have changed since launch: read it again, and the windows follow after
        // this event (this window is busy until then).
        if entering && crate::frost::read_system() {
            cx.defer(crate::frost::apply);
        }
        let dest = section.dest();
        let shown: Vec<SettingsSection> = if dest.sections().contains(&section) {
            dest.sections().to_vec()
        } else {
            vec![section]
        };
        let account = self.page_ctx(cx);
        let account_ready = account.is_some();
        let scope = account
            .as_ref()
            .map(|ctx| (ctx.gateway.url().to_string(), ctx.org.clone(), ctx.email.clone()));
        let access = self
            .account
            .as_ref()
            .map(|account| (account.role.clone(), account.permissions.clone()));
        let changed_account = self.settings.shell.account_scope != scope;
        if changed_account {
            self.remember_settings_position();
            self.settings
                .shell
                .query
                .update(cx, |state, cx| state.set_value("", window, cx));
            self.settings
                .token_label
                .update(cx, |state, cx| state.set_value("", window, cx));
            self.settings.shell.query_text.clear();
            self.settings.shell.searching = false;
        }
        let rebuild = !matches!(self.view, View::Settings(_))
            || self.settings.shell.shown() != shown
            || self.settings.shell.account_ctx.is_some() != account_ready
            || changed_account
            || self.settings.shell.account_access != access;
        if rebuild {
            self.settings.shell.account_scope = scope;
            self.settings.shell.account_access = access;
            self.settings.shell.fields.clear();
            let device = self.device_page_ctx(cx);
            let views: Vec<(SettingsSection, Option<AnyView>)> = shown
                .iter()
                .map(|each| {
                    let view = match each {
                        SettingsSection::LocalProviders => Some(crate::pages::local_providers::build(
                            device.clone(),
                            self.readiness
                                .local
                                .as_ref()
                                .and_then(|agents| agents.as_ref().ok())
                                .map(Vec::as_slice),
                            window,
                            cx,
                        )),
                        SettingsSection::Page(id) if id.is_native() => {
                            let builder = id.builder();
                            if self.is_local() && builder.needs_account() {
                                builder.build_locked(*id, window, cx)
                            } else {
                                builder.build(*id, device.clone(), account.clone(), window, cx)
                            }
                        }
                        _ => None,
                    };
                    (*each, view)
                })
                .collect();
            // A browser-only page can hand off immediately. A native page waiting for /v1/me keeps its skeleton.
            if !self.is_local()
                && let [(SettingsSection::Page(id), None)] = views.as_slice()
                && !id.is_native()
            {
                self.open_in_browser(id.web_path(), cx);
                return;
            }
            self.settings.shell.views = views;
            self.settings.shell.account_ctx = account;
        }
        if !self.settings.shell.restoring && !matches!(self.view, View::Settings(open) if open == section) {
            self.visit(Place::Settings(section));
        }
        if section != SettingsSection::Shortcuts {
            self.settings.shortcuts.update(cx, |page, cx| page.stop(cx));
        }
        self.reset_chat();
        self.close_workbench_for_settings();
        self.view = View::Settings(section);
        self.settings.shell.last_section = Some(section);
        self.title = "Settings".into();
        self.settings.shell.fold_motion = std::mem::take(&mut self.settings.shell.row_clicked);
        self.settings.shell.folded = false;
        let in_nav = dest.row().nav_sections(&self.settings_visibility());
        self.settings.shell.cursor = Some(if in_nav.len() > 1 && in_nav.contains(&section) {
            NavItem::Section(section)
        } else {
            NavItem::Row(dest.row())
        });
        if rebuild {
            self.settings.new_token = None;
            self.settings.confirm_revoke = None;
            self.settings.token_note = None;
            self.settings.notifications.note = None;
            self.settings.copied = None;
            self.settings.shell.status.clear();
            self.settings.scroll.set_offset(point(px(0.), px(0.)));
            if shown.contains(&SettingsSection::Notifications) {
                self.settings.session_notifications = crate::notify::setting();
                self.settings.session_note = None;
                self.settings.path_notifications = crate::path_notices::setting();
                self.settings.path_note = None;
                self.refresh_channels(cx);
            }
            if shown.contains(&SettingsSection::ApiKeys)
                && let Some(account) = &self.settings.shell.account_ctx
            {
                account.gateway.list_tokens();
                self.settings
                    .token_label
                    .update(cx, |state, cx| state.focus(window, cx));
            }
        }
        // Settings opens with the keys in its search field. A move inside Settings keeps them in the nav,
        // so ↑ ↓ Enter still walk it. API keys keeps them in its name field.
        if self.is_local() || section != SettingsSection::ApiKeys {
            if entering {
                self.settings
                    .shell
                    .query
                    .update(cx, |state, cx| state.focus(window, cx));
            } else if rebuild && !search_focused {
                window.focus(&self.settings.shell.nav_focus, cx);
            }
        }
        let index = shown.iter().position(|each| *each == section).unwrap_or(0);
        self.settings.shell.jump.set(Some(index));
        cx.notify();
    }

    /// A promotion link remains in the Settings Skills section and reuses its entity/active mutation guards.
    pub(super) fn open_skills_target(
        &mut self,
        target: crate::pages::skills::Target,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.navigation_allowed() {
            return;
        }
        if self.is_local() {
            self.open_settings(SettingsSection::Page(crate::pages::PageId::Skills), window, cx);
            return;
        }
        if self.page_ctx(cx).is_none() {
            self.open_in_browser(&target.web_path(), cx);
            return;
        }
        let section = SettingsSection::Page(crate::pages::PageId::Skills);
        self.open_settings(section, window, cx);
        let page = self
            .settings
            .shell
            .views
            .iter()
            .find(|(id, _)| *id == section)
            .and_then(|(_, view)| view.clone())
            .and_then(|view| view.downcast::<crate::pages::skills::SkillsPage>().ok());
        if let Some(page) = page {
            // The identity marker can change without email/org/role changing. Never reuse that old account entity.
            if !page.read(cx).current() {
                if let Some(ctx) = self.page_ctx(cx) {
                    if let Some(view) = crate::pages::skills::build(ctx, window, cx) {
                        let page = view.clone().downcast::<crate::pages::skills::SkillsPage>().ok();
                        if let Some((_, shown)) = self.settings.shell.views.iter_mut().find(|(id, _)| *id == section) {
                            *shown = Some(view);
                        }
                        if let Some(page) = page {
                            page.update(cx, |page, cx| {
                                page.set_promotion_target(target, cx);
                            });
                        }
                    }
                }
            } else {
                page.update(cx, |page, cx| {
                    page.set_promotion_target(target, cx);
                });
            }
        }
    }

    pub(super) fn open_settings_field(
        &mut self,
        section: SettingsSection,
        anchor: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.navigation_restoring && !self.settings.shell.restoring {
            self.navigation_explicit = true;
            self.cancel_navigation_restore();
        }

        let restoring = self.settings.shell.restoring;
        if !restoring {
            self.visit(Place::SettingsField(section, anchor));
        }
        self.settings.shell.restoring = true;
        self.open_settings(section, window, cx);
        self.settings.shell.restoring = restoring;
        if !restoring {
            self.settings.shell.fields.request(anchor);
        }
        cx.notify();
    }

    /// Ctrl+, (⌘, on macOS): Settings on the section last used in this window, Account the first time, with the keys in
    /// the search field. Inside Settings, in the nav.
    pub(super) fn open_settings_from_keys(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.panel {
            return;
        }
        let entering = !matches!(self.view, View::Settings(_));
        let section = self.settings.shell.last_section.unwrap_or(SettingsSection::Account);
        self.open_settings(section, window, cx);
        if !entering {
            window.focus(&self.settings.shell.nav_focus, cx);
        }
    }

    /// Cmd+F (Ctrl+F) in Settings: the keys go to the "Search settings" field.
    pub(super) fn focus_settings_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings
            .shell
            .query
            .update(cx, |state, cx| state.focus(window, cx));
    }

    /// Keep the scroll position of the Settings page on screen, for a step back or forward to it.
    pub(super) fn remember_settings_position(&mut self) {
        if matches!(self.view, View::Settings(_)) {
            let offset = self.navigation_settings_offset();
            self.settings.shell.positions.insert(
                (self.settings.shell.account_scope.clone(), self.history_at),
                (
                    offset,
                    self.settings.shell.query_text.clone(),
                    self.settings.shell.selected,
                    self.settings.shell.searching,
                ),
            );
        }
    }

    /// After a step back or forward to `at`: the Settings page comes back where it was left.
    pub(super) fn restore_settings_position(&mut self, at: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.restore_navigation_settings(at, window, cx);
    }

    /// Open a destination's row: its first section here, else its page in the browser.
    pub(super) fn open_dest(&mut self, dest: Dest, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_local() && dest.first().is_none() {
            self.open_settings(SettingsSection::GatewayLink(dest), window, cx);
            return;
        }
        // Account › Assistant opens the Assistant's own views here: What Nexus knows first, Approvals beside it.
        if dest == Dest::Assistant && self.assistant_available() {
            self.open_native(crate::pages::PageId::AgentMemory, window, cx);
            return;
        }
        match dest.first_for(&self.settings_visibility()) {
            Some(section) => match dest.target() {
                Some(anchor) => self.open_settings_field(section, anchor, window, cx),
                None => self.open_settings(section, window, cx),
            },
            None => self.open_in_browser(&dest.web_path(), cx),
        }
    }

    fn open_hit(&mut self, hit: &Hit, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_local() && hit.section.is_none() {
            let section = SettingsSection::GatewayLink(hit.dest);
            match hit.anchor {
                Some(anchor) => self.open_settings_field(section, anchor, window, cx),
                None => self.open_settings(section, window, cx),
            }
            self.settings.shell.searching = false;
            return;
        }
        // Account › Assistant's hits open the Assistant's own views: Passwords and payments, Privacy, else What Nexus
        // knows. Without the Assistant here they open in the browser.
        if hit.dest == Dest::Assistant {
            let view = super::settings_registry::assistant_view(hit.anchor);
            if self.assistant_available() {
                self.open_native(view, window, cx);
            } else {
                self.open_in_browser(view.web_path(), cx);
            }
            self.settings.shell.searching = false;
            return;
        }
        match hit.section {
            Some(section) => match hit.anchor {
                Some(anchor) => self.open_settings_field(section, anchor, window, cx),
                None => self.open_settings(section, window, cx),
            },
            None => {
                let anchor = hit.anchor.map(|anchor| format!("#{anchor}")).unwrap_or_default();
                self.open_in_browser(&format!("{}{anchor}", hit.dest.web_path()), cx);
            }
        }
        self.settings.shell.searching = false;
    }

    fn settings_hits(&self, cx: &App) -> Vec<Hit> {
        search(&self.settings.shell.query.read(cx).value(), &self.settings_visibility())
    }

    /// Enter in the search field: the picked result. The search stays, so a step back finds it as it was.
    fn open_selected_hit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let hits = self.settings_hits(cx);
        if let Some(hit) = hits.get(self.settings.shell.selected.min(hits.len().saturating_sub(1))) {
            let hit = hit.clone();
            self.open_hit(&hit, window, cx);
        }
    }

    pub(super) fn settings_keystroke(&mut self, key: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if key.modifiers.modified() {
            return false;
        }
        let search = self.settings.shell.query.read(cx).focus_handle(cx).is_focused(window);
        // Esc in the empty search field leaves Settings, as Esc in the nav does: Settings opens in the field.
        let empty = self.settings.shell.query.read(cx).value().is_empty();
        if search && (matches!(key.key.as_str(), "up" | "down") || (key.key == "escape" && !empty)) {
            self.settings_search_key(key, window, cx);
            return true;
        }
        if self.settings.shell.nav_focus.is_focused(window)
            && matches!(
                key.key.as_str(),
                "up" | "down" | "home" | "end" | "enter" | "space" | "/"
            )
        {
            self.settings_nav_key(key, window, cx);
            return true;
        }
        if key.key == "escape"
            && let Some((_, Some(view))) = self
                .settings
                .shell
                .views
                .iter()
                .find(|(section, _)| *section == SettingsSection::LocalProviders)
            && crate::pages::local_providers::cancel_edit(view, window, cx)
        {
            cx.stop_propagation();
            return true;
        }
        false
    }

    /// Keys in the search field, before the field: ↑ and ↓ pick a result; Esc clears the search.
    fn settings_search_key(&mut self, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) {
        if keystroke.modifiers.modified() {
            return;
        }
        let empty = self.settings.shell.query.read(cx).value().is_empty();
        match keystroke.key.as_str() {
            "down" | "up" if !empty => {
                let count = self.settings_hits(cx).len();
                if count > 0 {
                    let at = self.settings.shell.selected.min(count - 1);
                    self.settings.shell.nav_jump.set(true);
                    self.settings.shell.selected = if keystroke.key == "down" {
                        (at + 1) % count
                    } else {
                        (at + count - 1) % count
                    };
                }
            }
            "escape" if !empty => {
                self.settings.shell.query_text.clear();
                self.settings.shell.searching = false;
                self.settings
                    .shell
                    .query
                    .update(cx, |state, cx| state.set_value("", window, cx));
                self.settings.shell.selected = 0;
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// Keys in the nav: ↑ and ↓ move between rows, Home and End to the ends, Enter or Space opens, / searches.
    fn settings_nav_key(&mut self, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) {
        if keystroke.modifiers.modified() {
            return;
        }
        let open = match self.view {
            View::Settings(section) => section.dest(),
            _ => Dest::Preferences,
        };
        let rows = nav_items(&self.settings_visibility(), open);
        let at = self
            .settings
            .shell
            .cursor
            .and_then(|item| rows.iter().position(|row| *row == item));
        let next = match keystroke.key.as_str() {
            "down" => at.map_or(0, |at| (at + 1).min(rows.len() - 1)),
            "up" => at.map_or(0, |at| at.saturating_sub(1)),
            "home" => 0,
            "end" => rows.len() - 1,
            "enter" | "space" => {
                match self.settings.shell.cursor {
                    Some(NavItem::Row(dest)) => self.open_dest(dest, window, cx),
                    Some(NavItem::Section(section)) => self.open_settings(section, window, cx),
                    None => {}
                }
                window.focus(&self.settings.shell.nav_focus, cx);
                cx.stop_propagation();
                return;
            }
            "/" => {
                self.settings
                    .shell
                    .query
                    .update(cx, |state, cx| state.focus(window, cx));
                cx.stop_propagation();
                return;
            }
            _ => return,
        };
        self.settings.shell.cursor = rows.get(next).copied();
        self.settings.shell.nav_jump.set(true);
        cx.stop_propagation();
        cx.notify();
    }

    /// Mark a control's change: saving, saved (shown for a moment), or failed with why.
    pub(super) fn set_row_status(&mut self, anchor: &'static str, status: RowStatus, cx: &mut Context<Self>) {
        let saved = matches!(status, RowStatus::Saved(_));
        self.settings.shell.status.insert(anchor, status);
        if saved {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(SAVED_FOR).await;
                this.update(cx, |this, cx| {
                    if let Some(RowStatus::Saved(at)) = this.settings.shell.status.get(anchor)
                        && at.elapsed() >= SAVED_FOR
                    {
                        this.settings.shell.status.remove(anchor);
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }

    /// "Saving…" or "Saved" beside a control, while either holds.
    pub(super) fn row_status_tag(&self, anchor: &str, palette: Palette) -> Option<Div> {
        let text = match self.settings.shell.status.get(anchor)? {
            RowStatus::Saving => "Saving…",
            RowStatus::Saved(_) => "Saved",
            RowStatus::Failed(_) => return None,
        };
        Some(
            h_flex()
                .flex_none()
                .gap(px(4.))
                .text_size(px(12.))
                .text_color(palette.faint)
                .when(text == "Saved", |tag| tag.child(glyph(Lucide::Check, palette.faint)))
                .child(text),
        )
    }

    /// Why a change was not kept, under its control.
    pub(super) fn row_failure(&self, anchor: &str) -> Option<SharedString> {
        match self.settings.shell.status.get(anchor)? {
            RowStatus::Failed(why) => Some(why.clone()),
            _ => None,
        }
    }

    /// Leave Settings for the last place before it, or a new session when Settings came first.
    pub(super) fn back_to_app(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.shortcuts.update(cx, |page, cx| page.stop(cx));
        let before = self.history[..self.history_at]
            .iter()
            .rposition(|place| !matches!(place, Place::Settings(_) | Place::SettingsField(_, _)));
        match before {
            Some(at) => self.go(at as isize - self.history_at as isize, window, cx),
            None => self.start_new_session(window, cx),
        }
        cx.notify();
    }

    /// The page: a header that names the destination and where it is kept, then its sections.
    pub(super) fn render_settings(
        &mut self,
        section: SettingsSection,
        palette: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.schedule_navigation_scroll(window, cx);
        if self.settings.shell.searching && !self.settings.shell.query.read(cx).value().trim().is_empty() {
            return self.render_settings_results(palette, window, cx).into_any_element();
        }
        let dest = section.dest();
        let visibility = self.settings_visibility();
        let shell = &self.settings.shell;
        // Usage is a place with its own frame and tabs: it fills the pane, full width.
        if !self.is_local()
            && matches!(section, SettingsSection::Page(id) if id.is_place())
            && let Some((_, Some(view))) = shell.views.iter().find(|(each, _)| *each == section)
        {
            return div().size_full().child(view.clone()).into_any_element();
        }
        let width = window.viewport_size().width / px(1.);
        let narrow = width < NARROW_WIDTH;
        if shell.jump.take().is_some() {
            self.settings.scroll.set_offset(point(px(0.), px(0.)));
        }
        let row = dest.row();
        let own_page = dest.sections().contains(&section);
        // A row with several sections lists them in the nav, as sub-rows (People: Members, Teams). The page is the
        // open section's own: its title, under the row's name.
        let siblings: Vec<SettingsSection> = if own_page {
            row.nav_sections(&visibility)
        } else {
            Vec::new()
        };
        let sub_page = siblings.len() > 1;
        let title = match (own_page, sub_page) {
            (true, true) => section.section_title(),
            (true, false) => row.title(),
            (false, _) => section.label(),
        };
        // Header and page share one centred column, as wide as the open section. A row's widest sibling would push
        // a narrow section's form to the left of its own header (This computer: Projects and History are wide).
        let width_cap = px(section.content_width());
        let locked = self.settings_account_status(section).is_some();
        let account_loading = !locked && section.needs_account() && shell.account_ctx.is_none();
        let mut header = v_flex()
            .w_full()
            // Settings that a setup fix opened: the way back to the session (`setup_fix.rs`).
            .children(self.fix_back(palette, cx))
            .child(
                h_flex()
                    .h(px(28.))
                    .gap(px(12.))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .text_size(px(20.))
                            .line_height(px(28.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(palette.strong)
                            .child(title),
                    )
                    .when(locked, |row| row.child(ui::requires_sign_in(palette))),
            );
        if !locked && section == SettingsSection::Page(crate::pages::PageId::Policy) {
            header = header.child(
                h_flex().mt(px(16.)).gap(px(6.)).flex_wrap().children(
                    [
                        ("Defaults", "default-model-chat"),
                        ("Model access", "default-model-claude"),
                        ("Personal plans", "personal-agent-plans"),
                        ("Web search", "web-search"),
                    ]
                    .into_iter()
                    .map(|(label, anchor)| {
                        ui::chip(
                            SharedString::from(format!("policy-index-{anchor}")),
                            label,
                            false,
                            palette,
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_settings_field(
                                SettingsSection::Page(crate::pages::PageId::Policy),
                                anchor,
                                window,
                                cx,
                            )
                        }))
                    }),
                ),
            );
        }
        if dest == Dest::Computer
            && let Some(banner) = self.render_ownership_banner(palette, cx)
        {
            header = header.child(banner.mt(px(24.)));
        }
        // Only the open tab is drawn. Every tab's view is kept while the destination is open, so a switch between
        // them keeps what each shows.
        let page = shell
            .views
            .iter()
            .find(|(each, _)| *each == section)
            .map(|(each, view)| {
                let account = shell.account_ctx.as_ref();
                let body = if locked {
                    match each {
                        SettingsSection::ApiKeys => self.render_api_keys(None, palette, cx).into_any_element(),
                        SettingsSection::Account => self.render_account(None, palette, cx).into_any_element(),
                        _ => match view {
                            Some(view) => match self.history.get(self.history_at) {
                                Some(Place::SettingsField(open, anchor)) if open == each => {
                                    match super::settings_access::render_locked_field(
                                        *each,
                                        anchor,
                                        &visibility,
                                        palette,
                                    ) {
                                        Some(row) => v_flex()
                                            .child(shell.fields.field(anchor, row, None, cx))
                                            .child(view.clone())
                                            .into_any_element(),
                                        None => shell.fields.field(anchor, view.clone(), None, cx).into_any_element(),
                                    }
                                }
                                _ => view.clone().into_any_element(),
                            },
                            None => super::settings_access::render_locked_section(
                                *each,
                                &visibility,
                                palette,
                                &shell.fields,
                                cx,
                            )
                            .into_any_element(),
                        },
                    }
                } else if account_loading {
                    ui::skeleton(3, palette).into_any_element()
                } else {
                    match each {
                        SettingsSection::Appearance => self.render_appearance(account, palette, cx).into_any_element(),
                        SettingsSection::Notifications => self.render_notifications(palette, cx).into_any_element(),
                        SettingsSection::Shortcuts => self.settings.shortcuts.clone().into_any_element(),
                        SettingsSection::ApiKeys => account.map_or_else(
                            || div().into_any_element(),
                            |ctx| self.render_api_keys(Some(ctx), palette, cx).into_any_element(),
                        ),
                        SettingsSection::Account => account.map_or_else(
                            || div().into_any_element(),
                            |ctx| self.render_account(Some(ctx), palette, cx).into_any_element(),
                        ),
                        SettingsSection::LocalProviders => div().children(view.clone()).into_any_element(),
                        SettingsSection::Page(id) => match view {
                            Some(view) => view.clone().into_any_element(),
                            None => self.render_web_section(*id, palette, cx).into_any_element(),
                        },
                        SettingsSection::GatewayLink(_) => div().into_any_element(),
                    }
                };
                let anchors: &[&'static str] = match each {
                    SettingsSection::Notifications => &["notifications"],
                    SettingsSection::Shortcuts => &["shortcuts"],
                    SettingsSection::Page(crate::pages::PageId::Servers) => &["servers"],
                    SettingsSection::Page(crate::pages::PageId::Cloud) => &["cloud"],
                    SettingsSection::Page(crate::pages::PageId::LocalSessions) => &["history"],
                    SettingsSection::Page(crate::pages::PageId::SharedMemory) => &["shared-memory"],
                    SettingsSection::Page(crate::pages::PageId::Hooks) => &["hooks"],
                    SettingsSection::Page(crate::pages::PageId::Agents) => &["plan-usage"],
                    SettingsSection::Page(crate::pages::PageId::Mcp) => &["mcp-servers"],
                    SettingsSection::Page(crate::pages::PageId::Connectors) => &["connectors"],
                    SettingsSection::Page(crate::pages::PageId::ConnectorRules) => &["who-can-use", "agent-writes"],
                    _ => &[],
                };
                let body = anchors.iter().fold(body, |body, anchor| {
                    shell.fields.field(anchor, body, None, cx).into_any_element()
                });
                v_flex()
                    .id(SharedString::from(format!("settings-section-{}", each.anchor())))
                    .w_full()
                    .min_w(px(0.))
                    .max_w(px(each.content_width()))
                    .pt(px(24.))
                    .child(div().w_full().min_w(px(0.)).child(body))
            });
        h_flex()
            .size_full()
            .items_start()
            .child(
                div()
                    .id("settings")
                    .debug_selector(|| "settings".into())
                    .h_full()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_y_scroll()
                    .track_scroll(&self.settings.scroll)
                    .on_scroll_wheel(cx.listener(|this, _, _, _| this.cancel_navigation_restore()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, _| this.cancel_navigation_restore()),
                    )
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, _| {
                        if matches!(
                            event.keystroke.key.as_str(),
                            "pageup" | "pagedown" | "home" | "end" | "up" | "down"
                        ) {
                            this.cancel_navigation_restore();
                        }
                    }))
                    .on_scroll(cx.listener(|this, _, _, cx| {
                        this.settings.shell.jump.set(None);
                        cx.notify();
                    }))
                    .px(px(if narrow { 24. } else { ui::SETTINGS_GUTTER }))
                    .pb(px(64.))
                    .child(
                        v_flex()
                            .w_full()
                            .max_w(width_cap)
                            .mx_auto()
                            .child(header)
                            .when(page.is_none() && account_loading, |body| {
                                body.child(ui::skeleton(3, palette).pt(px(24.)))
                            })
                            .children(page),
                    ),
            )
            .into_any_element()
    }

    /// The full search view keeps descriptions readable while the sidebar remains available.
    fn render_settings_results(&self, palette: Palette, window: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let hits = self.settings_hits(cx);
        let count = hits.len();
        let selected = self.settings.shell.selected.min(count.saturating_sub(1));
        let rows: Vec<_> = hits
            .into_iter()
            .enumerate()
            .map(|(ix, hit)| {
                let crumbs = hit.breadcrumb.join(" › ");
                let locked = self.is_local() && hit.requires_sign_in();
                let web = hit.section.is_none() && !locked;
                let anchor = ScrollAnchor::for_handle(self.settings.shell.results_scroll.clone());
                if selected == ix && self.settings.shell.results_selected.replace(Some(selected)) != Some(selected) {
                    let window = window.window_handle();
                    let reveal = anchor.clone();
                    let epoch = self.navigation_epoch;
                    let at = self.history_at;
                    let query = self.settings.shell.query.read(cx).value().to_string();
                    let weak = cx.weak_entity();
                    cx.defer(move |cx| {
                        let _ = window.update(cx, |_, window, cx| {
                            let current = weak.upgrade().is_some_and(|view| {
                                view.read_with(cx, |view, cx| {
                                    view.navigation_epoch == epoch
                                        && view.history_at == at
                                        && view.navigation_allowed()
                                        && matches!(view.view, View::Settings(_))
                                        && view.settings.shell.searching
                                        && view.settings.shell.selected == selected
                                        && view.settings.shell.query.read(cx).value().as_ref() == query
                                })
                            });
                            if current {
                                reveal.scroll_to(window, cx);
                            }
                        });
                    });
                }
                v_flex()
                    .id(SharedString::from(format!("settings-result-{ix}")))
                    .anchor_scroll(Some(anchor))
                    .w_full()
                    .min_w(px(0.))
                    .p(px(14.))
                    .gap(px(6.))
                    .rounded(px(12.))
                    .bg(if selected == ix {
                        palette.selected
                    } else {
                        palette.background
                    })
                    .child(
                        h_flex()
                            .gap(px(8.))
                            .child(ui::label(hit.label, palette))
                            .when(locked, |row| row.child(ui::requires_sign_in(palette))),
                    )
                    .child(ui::setting_detail(crumbs, palette))
                    .child(ui::setting_detail(hit.description, palette))
                    .child(
                        h_flex().mt(px(4.)).child(
                            ui::button(
                                SharedString::from(format!("settings-result-open-{ix}")),
                                if web { "Open in browser" } else { "Open setting" },
                                palette,
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.settings.shell.selected = ix;
                                this.open_hit(&hit, window, cx);
                            })),
                        ),
                    )
            })
            .collect();
        div()
            .id("settings-results")
            .track_scroll(&self.settings.shell.results_scroll)
            .size_full()
            .overflow_y_scroll()
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(ui::READ_WIDTH + 96.))
                    .p(px(24.))
                    .gap(px(12.))
                    .child(
                        h_flex()
                            .h(px(44.))
                            .text_size(px(20.))
                            .line_height(px(28.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(palette.strong)
                            .child("Search settings"),
                    )
                    .child(ui::lead("Find a setting by name, task or familiar label.", palette))
                    .child(ui::setting_detail(
                        format!(
                            "{count} {} · ↑ ↓ to move · Enter to open",
                            if count == 1 { "result" } else { "results" }
                        ),
                        palette,
                    ))
                    .when(count == 0, |column| {
                        column
                            .child(ui::empty(
                                "No matching settings",
                                "Try another term, or browse This computer.",
                                palette,
                            ))
                            .child(
                                ui::button("settings-empty-computer", "Browse This computer", palette).on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.settings
                                            .shell
                                            .query
                                            .update(cx, |state, cx| state.set_value("", window, cx));
                                        this.settings.shell.query_text.clear();
                                        this.settings.shell.searching = false;
                                        this.open_dest(Dest::Computer, window, cx);
                                    }),
                                ),
                            )
                    })
                    .children(rows),
            )
    }

    /// Computer's banner while the Nexus app owns the shared store: its local settings are read-only here. Take over
    /// waits for the Nexus app to quit and never takes the lock from it (`owner.rs`). Gateway settings stay usable.
    fn render_ownership_banner(&self, palette: Palette, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let note = self.owner.note()?;
        let taking_over = self.owner.polling();
        let action: Option<AnyElement> = if self.owner.can_take_over() {
            Some(
                ui::button("settings-take-over", "Take over", palette)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.take_over(window, cx);
                        cx.notify();
                    }))
                    .into_any_element(),
            )
        } else if taking_over {
            Some(
                div()
                    .flex_none()
                    .text_size(px(12.))
                    .text_color(palette.muted)
                    .child("Waiting for it to quit…")
                    .into_any_element(),
            )
        } else {
            None
        };
        Some(
            h_flex()
                .id("settings-ownership")
                .items_start()
                .gap(px(12.))
                .p(px(14.))
                .rounded(px(12.))
                // A warning's tint over the page, so it reads as the page's state rather than one more card.
                .bg(palette.warning.opacity(0.08))
                .border_1()
                .border_color(palette.warning.opacity(0.3))
                .child(div().pt(px(2.)).child(glyph(Lucide::Lock, palette.waiting())))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w(px(0.))
                        .gap(px(2.))
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(palette.strong)
                                .child("The Nexus app owns this computer's local settings"),
                        )
                        .child(ui::setting_detail(
                            format!("{note} This computer's settings show their saved values until it quits. Account settings stay available."),
                            palette,
                        )),
                )
                .children(action),
        )
    }

    /// A section this build does not draw: its page opens in the browser, by name.
    fn render_web_section(&self, id: crate::pages::PageId, palette: Palette, cx: &mut Context<Self>) -> Div {
        let detail = if self.account.is_some() {
            "This build does not draw it yet. It opens in Nexus in your browser."
        } else {
            "It shows here once your account has loaded. Until then, it opens in your browser."
        };
        h_flex()
            .gap(px(16.))
            .py(px(10.))
            .child(ui::setting_lines(id.label(), detail, palette).flex_1())
            .child(
                ui::button(
                    SharedString::from(format!("settings-web-{}", id.web_path())),
                    "Open in browser",
                    palette,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.open_in_browser(id.web_path(), cx))),
            )
    }

    /// Settings' nav in the sidebar's place.
    pub(super) fn render_settings_nav(
        &self,
        section: SettingsSection,
        palette: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // The foot's fade, in the sidebar card's fill (`frost.rs`).
        let fade = crate::frost::Look::now(&palette).panel;
        let visibility = self.settings_visibility();
        let shell = &self.settings.shell;
        let query = shell.query.read(cx).value().to_string();
        let nav_focused = shell.nav_focus.is_focused(window);
        let open = section.dest();
        let mut list: Vec<AnyElement> = Vec::new();
        let reveal = shell.nav_jump.replace(false);
        // How long a fold takes: none unless a click changed the row (keys never animate), and closing is quicker.
        let fold_motion = shell.fold_motion;
        let fold_time = move |opening: bool| match (fold_motion, opening) {
            (false, _) => Duration::ZERO,
            (true, true) => motion::COLLAPSE,
            (true, false) => motion::MENU_OUT,
        };
        if query.trim().is_empty() {
            for (ix, (part, rows)) in layout(&visibility).into_iter().enumerate() {
                list.push(
                    self.render_nav_part(part, palette)
                        .when(ix > 0, |label| label.mt(px(16.)))
                        .into_any_element(),
                );
                for dest in rows {
                    let disabled = self.settings_destination_status(dest);
                    let native = dest.first().is_some();
                    let cursor = nav_focused && shell.cursor == Some(NavItem::Row(dest));
                    if reveal && cursor {
                        shell.nav_scroll.scroll_to_item(list.len());
                    }
                    // A row with several sections unfolds while it is open: its sections are sub-rows under it, and
                    // the open one carries the selected fill instead of the row.
                    let sections = dest.nav_sections(&visibility);
                    let folds = sections.len() > 1;
                    let toggles = folds && open.row() == dest;
                    let unfolded = toggles && !shell.folded;
                    list.push(
                        nav_row(
                            SharedString::from(format!("settings-nav-{}", dest.slug())),
                            dest_icon(dest),
                            dest.title(),
                            open.row() == dest && !unfolded,
                            cursor,
                            palette,
                        )
                        .when(folds, |row| {
                            row.child(glyph(Lucide::ChevronRight, palette.faint).flex_none().with_tween(
                                SharedString::from(format!("settings-fold-{}", dest.slug())),
                                if unfolded { 1. } else { 0. },
                                motion::Tween::new(fold_time(unfolded), motion::ease_out),
                                |icon, turn| icon.rotate(radians(std::f32::consts::FRAC_PI_2 * turn)),
                            ))
                        })
                        .when_some(disabled, |row, reason| {
                            row.opacity(0.65)
                                .gap(px(4.))
                                .child(ui::requires_sign_in(palette).text_size(px(11.)))
                                .aria_label(reason)
                                .tooltip(move |window, cx| Tooltip::new(reason).build(window, cx))
                        })
                        .when(!native && disabled.is_none(), |row| {
                            row.child(div().flex_1())
                                .child(glyph(Lucide::ArrowUpRight, palette.faint))
                                .tooltip(|window, cx| Tooltip::new("Opens in your browser").build(window, cx))
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.settings.shell.cursor = Some(NavItem::Row(dest));
                            // The open row's toggle folds or unfolds its sections; the page stays.
                            if toggles {
                                let shell = &mut this.settings.shell;
                                shell.folded = !shell.folded;
                                shell.fold_motion = true;
                                cx.notify();
                                return;
                            }
                            this.settings.shell.row_clicked = true;
                            this.open_dest(dest, window, cx);
                        }))
                        .into_any_element(),
                    );
                    // A folding row stays drawn for a moment after it closes, so its sub-rows can fold away.
                    let Some(phase) = (if folds {
                        motion::presence(format!("settings-fold-{}", dest.slug()), unfolded, cx)
                    } else {
                        None
                    }) else {
                        continue;
                    };
                    let mut subs: Vec<AnyElement> = Vec::new();
                    for each in sections {
                        let cursor = nav_focused && shell.cursor == Some(NavItem::Section(each));
                        if reveal && cursor {
                            shell.nav_scroll.scroll_to_item(list.len());
                        }
                        subs.push(
                            nav_sub_row(
                                SharedString::from(format!("settings-nav-{}-{}", dest.slug(), each.anchor())),
                                each.section_title(),
                                each == section,
                                cursor,
                                palette,
                            )
                            .when_some(self.settings_account_status(each), |row, reason| {
                                row.opacity(0.65)
                                    .child(ui::requires_sign_in(palette).text_size(px(11.)))
                                    .tooltip(move |window, cx| Tooltip::new(reason).build(window, cx))
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.settings.shell.cursor = Some(NavItem::Section(each));
                                this.open_settings(each, window, cx);
                            }))
                            .into_any_element(),
                        );
                    }
                    let full = nav_sub_rows_height(subs.len());
                    let open_now = phase.open();
                    // Unfold from nothing on a click: the rows under it slide down, and the sub-rows fade in with
                    // the height, on the chevron's curve. Folding is quicker, as anything that leaves.
                    let tween = motion::Tween::new(fold_time(open_now), motion::ease_out);
                    let tween = if shell.fold_motion { tween.from(0.) } else { tween };
                    list.push(
                        nav_sub_rows(subs, palette)
                            .overflow_hidden()
                            .with_tween(
                                SharedString::from(format!("settings-subs-{}", dest.slug())),
                                if open_now { 1. } else { 0. },
                                tween,
                                move |rows, open| rows.h(px(full * open)).opacity(open),
                            )
                            .into_any_element(),
                    );
                }
            }
        } else {
            let hits = search(&query, &visibility);
            if hits.is_empty() {
                list.push(
                    div()
                        .px(px(8.))
                        .py(px(6.))
                        .text_size(px(13.))
                        .text_color(palette.faint)
                        .child("No settings match.")
                        .into_any_element(),
                );
            }
            let picked = shell.selected.min(hits.len().saturating_sub(1));
            for (ix, hit) in hits.into_iter().enumerate() {
                let crumbs = hit.breadcrumb[..hit.breadcrumb.len() - 1].join(" › ");
                let locked = self.is_local() && hit.requires_sign_in();
                let web = hit.section.is_none() && !locked;
                let chosen = hit.clone();
                if reveal && ix == picked {
                    shell.nav_scroll.scroll_to_item(list.len());
                }
                list.push(
                    h_flex()
                        .id(SharedString::from(format!("settings-hit-{ix}")))
                        .min_h(px(40.))
                        .px(px(8.))
                        .py(px(4.))
                        .gap(px(8.))
                        .rounded(px(8.))
                        .cursor_pointer()
                        .when(ix == picked, |row| row.bg(palette.selected))
                        .hover(|style| style.bg(palette.hover))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w(px(0.))
                                .child(div().text_size(px(13.)).text_color(palette.strong).child(hit.label))
                                .child(div().text_size(px(11.)).text_color(palette.faint).child(crumbs)),
                        )
                        .when(locked, |row| row.child(ui::requires_sign_in(palette)))
                        .when(web, |row| row.child(glyph(Lucide::ArrowUpRight, palette.faint)))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.settings.shell.selected = ix;
                            this.open_hit(&chosen, window, cx);
                        }))
                        .into_any_element(),
                );
            }
        }
        // The head row: Back to app, and its key.
        let back = h_flex()
            .id("settings-back")
            .tab_index(0)
            .border_2()
            .border_color(transparent_black())
            .focus_visible(|style| style.border_color(palette.foreground.opacity(0.35)))
            .h(px(28.))
            .pl(px(6.))
            .pr(px(8.))
            .gap(px(6.))
            .rounded(px(6.))
            .text_size(px(13.))
            .text_color(palette.strong)
            .cursor_pointer()
            .hover(|style| style.bg(palette.hover))
            .child(icon16(Lucide::ArrowLeft, palette.icon))
            .child("Back to app")
            .on_click(cx.listener(|this, _, window, cx| this.back_to_app(window, cx)));
        let head = h_flex()
            .h(px(44.))
            .px(px(2.))
            .gap(px(2.))
            .child(back)
            .child(div().flex_1())
            .child(
                div()
                    .pr(px(6.))
                    .text_size(px(12.))
                    .text_color(palette.faint)
                    .child("Esc"),
            );
        // On the sidebar's fill the hover fill would vanish, so search keeps a raised fill and a ring inside its edge.
        let search_box = h_flex()
            .id("settings-search")
            .mt(px(4.))
            .h(px(32.))
            .px(px(8.))
            .gap(px(8.))
            .rounded(px(6.))
            .bg(palette.raised)
            .border_1()
            .border_color(palette.border.opacity(0.7))
            .text_size(px(13.))
            .child(icon16(Lucide::Search, palette.icon))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .child(Input::new(&shell.query).appearance(false)),
            )
            .child(div().text_size(px(12.)).text_color(palette.faint).child("/"));
        v_flex()
            .w(px(self.sidebar_width()))
            .h_full()
            .child(div().px(px(8.)).child(head).child(search_box).pb(px(12.)))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h(px(0.))
                    .child(
                        div()
                            .id("settings-nav")
                            .track_scroll(&shell.nav_scroll)
                            .track_focus(&shell.nav_focus)
                            .size_full()
                            .overflow_y_scroll()
                            .px(px(8.))
                            .pb(px(24.))
                            .children(list),
                    )
                    // A 16px fade at the foot, so a nav taller than the window reads as one that scrolls.
                    .child(
                        div()
                            .absolute()
                            .bottom_0()
                            .left_0()
                            .right_0()
                            .h(px(16.))
                            .bg(linear_gradient(
                                0.,
                                linear_color_stop(fade, 0.),
                                linear_color_stop(fade.opacity(0.), 1.),
                            )),
                    ),
            )
            .into_any_element()
    }

    /// A part's label: a 16 lead (your initials; the organization's logo; the person icon for
    /// Personal), its name, and on the right your role there ("Just you" for Personal).
    fn render_nav_part(&self, part: Part, palette: Palette) -> Div {
        let account = self.account.as_ref();
        let personal = account.is_some_and(|account| account.personal);
        let (lead, name, right): (AnyElement, SharedString, SharedString) = match part {
            Part::You => (
                disc(
                    account
                        .map(|account| super::initials(&account.name))
                        .unwrap_or_default(),
                    palette,
                )
                .into_any_element(),
                "You".into(),
                SharedString::default(),
            ),
            Part::Code => (
                icon16(Lucide::Code, palette.icon).into_any_element(),
                "Code".into(),
                SharedString::default(),
            ),
            Part::Org if personal => (
                icon16(Lucide::User, palette.icon).into_any_element(),
                "Personal".into(),
                "Just you".into(),
            ),
            Part::Org => {
                let name: SharedString = account
                    .map(|account| account.org_name.clone())
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| "Organization".into())
                    .into();
                let initials = super::initials(&name);
                // Nexus's own organization wears the app's icon, never a redrawn mark. It is
                // matched by the tenant slug: a customer organization may be named "Nexus" too.
                let is_nexus = account.is_some_and(|account| account.org == "nexus-xyz");
                let lead = match account.and_then(|account| account.org_logo.clone()) {
                    // On a white tile, like the account row: a logo made for a white ground stays visible. The
                    // initials show while it loads and when it fails to load.
                    Some(url) => {
                        let (loading, failed) = (initials.clone(), initials);
                        div()
                            .flex_none()
                            .size(px(16.))
                            .rounded(px(4.))
                            .overflow_hidden()
                            .bg(gpui::white())
                            .child(
                                super::logo::logo(url, 16.)
                                    .with_loading(move || disc(loading.clone(), palette).into_any_element())
                                    .with_fallback(move || disc(failed.clone(), palette).into_any_element()),
                            )
                            .into_any_element()
                    }
                    None if is_nexus => img(crate::assets::APP_ICON)
                        .size(px(16.))
                        .flex_none()
                        .into_any_element(),
                    None => disc(initials, palette).into_any_element(),
                };
                let role = account.map(|account| role_label(&account.role)).unwrap_or("");
                (lead, name, role.into())
            }
        };
        h_flex()
            .h(px(28.))
            .px(px(8.))
            .gap(px(8.))
            .text_size(px(12.))
            .line_height(px(16.))
            .text_color(palette.faint)
            .child(h_flex().flex_none().size(px(16.)).justify_center().child(lead))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .child(name),
            )
            .child(right)
    }
}

/// A 16 disc with initials, for you and for an organization without a logo.
fn disc(initials: SharedString, palette: Palette) -> Div {
    div()
        .flex()
        .flex_none()
        .size(px(16.))
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(palette.raised)
        .border_1()
        .border_color(palette.foreground.opacity(0.1))
        .text_size(px(8.))
        .line_height(px(10.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(palette.strong)
        .child(initials)
}

fn role_label(role: &str) -> &'static str {
    match role {
        "owner" => "Owner",
        "admin" => "Admin",
        "member" => "Member",
        _ => "",
    }
}

fn dest_icon(dest: Dest) -> Lucide {
    match dest {
        Dest::Account => Lucide::CircleUser,
        Dest::Assistant => Lucide::BotMessageSquare,
        Dest::Preferences => Lucide::SlidersHorizontal,
        Dest::Computer => Lucide::Laptop,
        Dest::Subsessions => Lucide::Workflow,
        Dest::Privacy => Lucide::Lock,
        Dest::Subscriptions => Lucide::Bot,
        Dest::ApiKeys => Lucide::KeyRound,
        Dest::Developer => Lucide::Blocks,
        Dest::LocalProviders => Lucide::KeyRound,
        Dest::Devices => Lucide::Smartphone,
        // Connections: the person's own accounts. Integrations: the organization's rules for them.
        Dest::Integrations => Lucide::Plug,
        Dest::ConnectorRules => Lucide::Blocks,
        Dest::Referrals => Lucide::Gift,
        Dest::General => Lucide::Building2,
        Dest::Members | Dest::Teams => Lucide::Users,
        Dest::Policy | Dest::Providers => Lucide::ShieldCheck,
        Dest::Usage | Dest::Budgets => Lucide::ChartColumn,
        Dest::Billing => Lucide::Wallet,
        Dest::Repositories => Lucide::FolderGit2,
        Dest::Audit => Lucide::ScrollText,
        Dest::Skills => Lucide::Sparkles,
        Dest::Slack => Lucide::MessageSquare,
        Dest::Design => Lucide::Eye,
        Dest::Data => Lucide::Archive,
    }
}

/// A 16 icon, the size of the nav's rows and controls.
fn icon16(name: Lucide, color: Hsla) -> Icon {
    Icon::new(name).size(px(16.)).text_color(color)
}

/// The sub-rows of an open row, on a 1px guide that hangs from the row's icon. Their labels line up with the row's.
fn nav_sub_rows(rows: Vec<AnyElement>, palette: Palette) -> Div {
    v_flex()
        .relative()
        .pt(px(SUB_ROWS_TOP))
        .pb(px(SUB_ROWS_BOTTOM))
        // The row's icon is centred 16 from its left edge (6 padding, 2 ring, half of 16).
        .ml(px(15.))
        .pl(px(9.))
        .gap(px(SUB_ROW_GAP))
        .child(
            div()
                .absolute()
                .left_0()
                .top(px(SUB_ROWS_TOP + 4.))
                .bottom(px(SUB_ROWS_BOTTOM + 4.))
                .w(px(1.))
                .bg(palette.border),
        )
        .children(rows)
}

const SUB_ROW_HEIGHT: f32 = 28.;
const SUB_ROW_GAP: f32 = 1.;
const SUB_ROWS_TOP: f32 = 2.;
const SUB_ROWS_BOTTOM: f32 = 4.;

/// The unfolded height of `count` sub-rows, which their fold eases to and from.
fn nav_sub_rows_height(count: usize) -> f32 {
    let count = count as f32;
    SUB_ROWS_TOP + count * SUB_ROW_HEIGHT + (count - 1.).max(0.) * SUB_ROW_GAP + SUB_ROWS_BOTTOM
}

/// A sub-row of the nav: 28 high, no icon. The open one takes the selected fill and the strong colour; the others
/// are muted until hovered. `cursor` draws the keyboard's ring, as on a row.
fn nav_sub_row(id: SharedString, label: &'static str, picked: bool, cursor: bool, palette: Palette) -> Stateful<Div> {
    h_flex()
        .id(id)
        .flex_none()
        .h(px(SUB_ROW_HEIGHT))
        .px(px(6.))
        .rounded(px(6.))
        .text_size(px(13.))
        .cursor_pointer()
        .border_2()
        .border_color(if cursor {
            palette.foreground.opacity(0.35)
        } else {
            transparent_black()
        })
        .when(picked, |row| row.bg(palette.selected).text_color(palette.strong))
        .when(!picked, |row| {
            row.text_color(palette.secondary)
                .hover(|style| style.bg(palette.hover).text_color(palette.strong))
        })
        .child(div().flex_1().min_w(px(0.)).truncate().child(label))
}

/// A row of the nav: 30 high, a 16 icon, the label in the text colour; picked takes the selected
/// fill. `cursor` draws the keyboard's 2px ring, inside the row so nothing moves.
fn nav_row(
    id: SharedString,
    icon: Lucide,
    label: &'static str,
    picked: bool,
    cursor: bool,
    palette: Palette,
) -> Stateful<Div> {
    h_flex()
        .id(id)
        .h(px(crate::tokens::ROW_HEIGHT))
        .px(px(6.))
        .gap(px(8.))
        .rounded(px(crate::tokens::ROW_RADIUS))
        .text_size(px(13.))
        .text_color(palette.strong)
        .cursor_pointer()
        .border_2()
        .border_color(if cursor {
            palette.foreground.opacity(0.35)
        } else {
            transparent_black()
        })
        .when(picked, |row| row.bg(palette.selected))
        .when(!picked, |row| row.hover(|style| style.bg(palette.hover)))
        .child(icon16(icon, if picked { palette.strong } else { palette.icon }))
        .child(div().flex_1().min_w(px(0.)).truncate().child(label))
}
