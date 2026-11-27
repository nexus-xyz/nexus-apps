//! Wire the active GPUI session and its actual resource registries to navigation.
use super::*;
use std::time::{Duration, Instant};
mod state;
mod status;
pub(in crate::workspace) use state::{Navigation, Target};
use state::{scoped_path, seed};

impl Workspace {
    pub(in crate::workspace) fn navigation_id(&self) -> Option<String> {
        match &self.view {
            View::Code(view) => Some(view.id.clone()),
            _ => None,
        }
    }
    pub(in crate::workspace) fn navigation(&mut self, id: &str) -> &mut Navigation {
        self.side
            .navigation
            .entry(id.into())
            .or_insert_with(|| Navigation::new(crate::paths::workbench_manual(id)))
    }
    pub(in crate::workspace) fn manual_workbench(&mut self) {
        self.side.navigation_wake = None;
        if let Some(id) = self.navigation_id() {
            let save = !self.navigation(&id).manual;
            self.navigation(&id).interact();
            if save {
                crate::paths::save_workbench_manual(&id, true);
            }
        }
    }
    pub(in crate::workspace) fn reveal_manual_workbench(&mut self) {
        self.manual_workbench();
    }
    pub(in crate::workspace) fn offer_workbench(&mut self, target: Target, ready: bool, cx: &mut Context<Self>) {
        let Some(id) = self.navigation_id() else { return };
        let revision = self.navigation(&id).revision;
        self.offer_workbench_at(target, ready, revision, cx);
    }
    pub(in crate::workspace) fn offer_workbench_at(
        &mut self,
        target: Target,
        ready: bool,
        revision: u64,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.navigation_id() else { return };
        let automatic = match &target {
            Target::Changes(_) => crate::desktop_settings::load().follows_edits(),
            Target::Preview(_) => crate::desktop_settings::load().opens_browser_on_server(),
            _ => true,
        };
        if automatic {
            self.navigation(&id).offer(target, ready, revision);
        } else {
            self.navigation(&id).offer_filtered(target, ready, revision, false);
        }
        // Replace the previous timer: the last update in a burst gets its full quiet interval.
        {
            self.side.navigation_wake = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_millis(125)).await;
                let _ = this.update(cx, |this, cx| {
                    this.side.navigation_wake = None;
                    cx.notify();
                });
            }));
        }
        cx.notify();
    }
    fn follow_workbench(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.navigation_id() else { return };
        self.navigation(&id).follow();
        crate::paths::save_workbench_manual(&id, false);
        // Explicit Follow also makes the latest ready item available after a hidden pane is reopened.
        if let Some(target) = self.navigation(&id).latest.clone() {
            self.offer_workbench(target, true, cx);
        }
        cx.notify();
    }
    pub(in crate::workspace) fn native_browser_activity(&mut self, session: &str, tab: &str, cx: &mut Context<Self>) {
        if self.navigation_id().as_deref() == Some(session) {
            let state = self.navigation(session);
            state.native.insert(tab.into(), state.revision);
            state.scanned = None;
            cx.notify();
        }
    }
    pub(in crate::workspace) fn sync_workbench_navigation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_workbench_navigation_at(Instant::now(), window, cx);
    }
    fn sync_workbench_navigation_at(&mut self, now: Instant, window: &mut Window, cx: &mut Context<Self>) {
        let View::Code(view) = &self.view else { return };
        if !view.loaded {
            return;
        }
        let id = view.id.clone();
        let last = view.last_seq;
        let fresh = view.navigation_fresh;
        let blocks = &view.blocks;
        let registry = self
            .side
            .shared_terminal()
            .map_or(0, |view| view.read(cx).registry_revision());
        let nav = self
            .side
            .navigation
            .entry(id.clone())
            .or_insert_with(|| Navigation::new(crate::paths::workbench_manual(&id)));
        nav.sync_mode(crate::paths::workbench_manual(&id));
        if nav.baseline.is_none() {
            nav.baseline = Some(if fresh { 0 } else { last });
            if !fresh {
                seed(blocks, &mut nav.seen);
                return;
            }
        }
        let targets = if nav.scanned != Some((last, registry)) {
            nav.scanned = Some((last, registry));
            nav.collect(blocks)
        } else {
            vec![]
        };
        for (mut target, revision) in targets {
            if let Target::Changes(path) = &mut target {
                let Some(relative) = scoped_path(path, &self.side.dir) else {
                    continue;
                };
                *path = relative;
            }
            let ready = match &target {
                Target::Terminal(id) => self
                    .side
                    .shared_terminal()
                    .is_some_and(|view| view.read(cx).ready_execution(id)),
                Target::Changes(path) => {
                    std::path::Path::new(path).is_relative() || std::path::Path::new(path).starts_with(&self.side.dir)
                }
                _ => true,
            };
            if let Target::Terminal(execution) = &target {
                if !ready {
                    continue;
                }
                let state = self
                    .side
                    .shared_terminal()
                    .and_then(|view| view.read(cx).execution_status(execution));
                if !self
                    .navigation(&id)
                    .seen
                    .insert(format!("terminal:{execution}:{state:?}"))
                {
                    continue;
                }
            }
            self.offer_workbench_at(target.clone(), ready && target != Target::Agents, revision, cx);
        }
        let hidden = !self.side.open
            || self
                .side
                .changes
                .as_ref()
                .is_some_and(|view| view.read(cx).commenting());
        if let Some(target) = self.navigation(&id).take(hidden, window.is_window_active(), now) {
            self.show_workbench_target(target, true, window, cx);
        }
    }
    fn show_workbench_target(&mut self, target: Target, automatic: bool, window: &mut Window, cx: &mut Context<Self>) {
        if automatic && !self.side.open {
            return;
        }
        // Following opens a linked file only once it is found, as a chip's click finds it (`file_open::locate`): by
        // its name alone, or in the checkout the agent read it in. A link can come before the file is written, name an
        // example, or name a path on the agent's sandbox: each would open "File not found". A server's session reads
        // its files there, so its links open as before.
        if automatic
            && let Target::File(path, line) = &target
            && let Some(id) = self.navigation_id()
            && self.side.server.is_none()
        {
            let touched = match &self.view {
                View::Code(view) => crate::workspace::file_open::touched_files(&view.blocks),
                _ => Vec::new(),
            };
            let line = *line;
            let found = cx.background_executor().spawn(crate::workspace::file_open::locate(
                self.side.dir.clone(),
                path.clone(),
                touched,
            ));
            self.side.follow_file = Some(cx.spawn_in(window, async move |this, cx| {
                let found = found.await;
                let followed = this.update_in(cx, |this, window, cx| {
                    this.side.follow_file = None;
                    let following = this.navigation_id().as_deref() == Some(id.as_str())
                        && !this.navigation(&id).manual
                        && this.side.open;
                    if let (Some(path), true) = (found, following) {
                        this.reveal_workbench_target(Target::File(path, line), true, window, cx);
                    }
                });
                if let Err(error) = followed {
                    eprintln!("nexus-desktop-gpui: the window closed before a followed file opened: {error}");
                }
            }));
            return;
        }
        self.reveal_workbench_target(target, automatic, window, cx);
    }
    fn reveal_workbench_target(
        &mut self,
        target: Target,
        automatic: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // An agent's command shows in the session's Commands list, the Terminal tab a person opens. Without that list
        // there is nothing to follow: the Terminal tab would open a person's shell.
        let commands = match &target {
            Target::Terminal(_) => match self.side.shared_terminal().cloned() {
                Some(commands) => Some(commands),
                None => return,
            },
            _ => None,
        };
        self.side.open = true;
        self.side.tool = target.tool();
        self.side.side_chat_visible = false;
        self.side.connection = None;
        self.sync_side(window, cx);
        match target {
            Target::Browser(tab) => {
                if let Some(browser) = self.side.browser().cloned() {
                    browser.update(cx, |browser, cx| browser.show_agent_tab(&tab, window, cx));
                }
            }
            Target::Terminal(execution) => {
                if let Some(commands) = commands {
                    commands.update(cx, |view, cx| view.select_execution(&execution, cx));
                }
            }
            Target::Changes(path) => {
                if let Some(changes) = &self.side.changes {
                    changes.update(cx, |changes, cx| {
                        changes.follow_file(path, cx);
                        changes.agent_edited(cx);
                    });
                }
            }
            Target::File(path, line) => {
                if let Some(files) = &self.side.files {
                    files.update(cx, |files, cx| files.open_at(path, line, window, cx));
                }
            }
            Target::Page(id) => self.open_code_page(&id, automatic, cx),
            Target::Preview(url) => {
                let key = self.side.browser_key();
                let browser = self
                    .side
                    .browser()
                    .cloned()
                    .or_else(|| self.make_browser(key, None, window, cx));
                if let Some(browser) = browser {
                    browser.update(cx, |browser, cx| browser.open(&url, window, cx));
                }
            }
            Target::Agents => {}
        }
        cx.notify();
    }
    pub(in crate::workspace) fn accepts_automatic_page(&mut self, path: &str, active: bool) -> bool {
        let page = self
            .side
            .pending_pages
            .keys()
            .find(|id| path.split('/').any(|segment| segment == id.as_str()))
            .cloned();
        let Some(page) = page else { return true };
        let Some((session, revision)) = self.side.pending_pages.remove(&page) else {
            return true;
        };
        active
            && self.navigation_id().as_deref() == Some(&session)
            && self.side.open
            && self.side.navigation.get(&session).is_some_and(|state| {
                !state.manual && state.revision == revision && state.latest == Some(Target::Page(page))
            })
    }
    pub(in crate::workspace) fn render_browser_permission(
        &self,
        id: &str,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        use crate::side::browser::agent::Answer;
        let browser = self.side.browsers.get(id)?.clone();
        let request = browser.read(cx).permission_request()?;
        let allow = browser.clone();
        let decline = browser;
        let allow_id = request.id.clone();
        let decline_id = request.id;
        Some(
            h_flex()
                .gap(px(6.))
                .flex_wrap()
                .text_size(px(12.))
                .text_color(palette.muted)
                .child(format!("Agent requests browser access · {}", request.url))
                .child(
                    crate::side::chip("workbench-browser-allow", "Allow", false, palette).on_click(cx.listener(
                        move |_, _, _, cx| {
                            allow.update(cx, |browser, cx| {
                                browser.answer_request_id(&allow_id, Answer::Allow, cx)
                            });
                        },
                    )),
                )
                .child(
                    crate::side::chip("workbench-browser-decline", "Don't allow", false, palette).on_click(
                        cx.listener(move |_, _, _, cx| {
                            decline.update(cx, |browser, cx| {
                                browser.answer_request_id(&decline_id, Answer::Decline, cx)
                            });
                        }),
                    ),
                )
                .into_any_element(),
        )
    }
    fn view_workbench_once(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        self.manual_workbench();
        if let Some(id) = self.navigation_id() {
            self.navigation(&id).unread.clear();
        }
        self.show_workbench_target(target, false, window, cx);
    }

    /// A hidden workbench reports activity in the conversation, without repeating its mode controls.
    pub(in crate::workspace) fn render_background_workbench(
        &self,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.side.open {
            return None;
        }
        let id = self.navigation_id()?;
        let state = self.side.navigation.get(&id)?;
        let target = state.unread.last()?.clone();
        Some(
            h_flex()
                .gap(px(8.))
                .text_size(px(12.))
                .text_color(palette.muted)
                .child(div().flex_1().min_w(px(0.)).truncate().child(format!(
                    "Workbench activity · {} {} · {}",
                    state.unread.len(),
                    if state.unread.len() == 1 { "update" } else { "updates" },
                    target.label()
                )))
                .child(
                    crate::side::chip("workbench-view-latest", "View latest", false, palette).on_click(
                        cx.listener(move |this, _, window, cx| this.view_workbench_once(target.clone(), window, cx)),
                    ),
                )
                .into_any_element(),
        )
    }

    /// Following the agent, as one small button in the Workbench's head, not a row of its own: a green dot while it
    /// follows, a ring once paused, with the count of updates missed beside it. Clicking inside the Workbench pauses
    /// it on its own; the button is the way back. Its tooltip says the state in words (`status.rs`).
    pub(in crate::workspace) fn render_navigation(
        &self,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.side.open {
            return None;
        }
        let id = self.navigation_id()?;
        let state = self.side.navigation.get(&id)?;
        let manual = state.manual;
        let unread = state.unread.len();
        let latest = state.unread.last().or(state.latest.as_ref());
        let words = status::status(manual, unread, latest);
        let tip: SharedString = format!(
            "{}{}. {}",
            words.headline,
            words.detail.map(|detail| format!(" · {detail}")).unwrap_or_default(),
            format!("Click to {}.", words.action.to_lowercase())
        )
        .into();
        Some(
            h_flex()
                .id(if manual { "workbench-follow" } else { "workbench-pause" })
                .debug_selector(|| "workbench-follow-button".into())
                .flex_none()
                .h(px(28.))
                .min_w(px(28.))
                .px(px(10.))
                .gap(px(5.))
                .justify_center()
                .rounded(px(crate::tokens::radius::CONTROL))
                .text_size(px(11.5))
                .text_color(palette.muted)
                .cursor_pointer()
                .hover(|style| style.bg(palette.hover))
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, _, cx| {
                    if manual {
                        this.follow_workbench(cx);
                    } else {
                        this.manual_workbench();
                        cx.notify();
                    }
                }))
                .child(status::mark(manual, palette))
                .when(manual && unread > 0, |button| button.child(unread.to_string()))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
#[path = "navigation/view_tests.rs"]
mod view_tests;
